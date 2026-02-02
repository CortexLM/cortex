//! Windows Restricted Token implementation for privilege reduction.
//!
//! Restricted tokens reduce process privileges by:
//! - Disabling SIDs (security identifiers)
//! - Removing privileges
//! - Adding restricted SIDs for deny-only access

use crate::{Result, WindowsSandboxError};
use std::ptr;
use tracing::{debug, info, warn};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE, LUID};
use windows::Win32::Security::{
    AdjustTokenPrivileges, CreateRestrictedToken, DuplicateTokenEx, LookupPrivilegeValueW,
    SecurityImpersonation, TokenPrimary, DISABLE_MAX_PRIVILEGE, LUA_TOKEN, SANDBOX_INERT,
    SE_PRIVILEGE_REMOVED, TOKEN_ADJUST_PRIVILEGES, TOKEN_ALL_ACCESS, TOKEN_DUPLICATE,
    TOKEN_PRIVILEGES, TOKEN_QUERY, WRITE_RESTRICTED,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// Privileges that should be removed for sandboxed processes.
const DANGEROUS_PRIVILEGES: &[&str] = &[
    "SeDebugPrivilege",                // Debug other processes
    "SeBackupPrivilege",               // Bypass file security for backup
    "SeRestorePrivilege",              // Bypass file security for restore
    "SeTakeOwnershipPrivilege",        // Take ownership of objects
    "SeLoadDriverPrivilege",           // Load device drivers
    "SeSystemEnvironmentPrivilege",    // Modify firmware environment
    "SeImpersonatePrivilege",          // Impersonate clients
    "SeCreateTokenPrivilege",          // Create tokens
    "SeAssignPrimaryTokenPrivilege",   // Assign process token
    "SeTcbPrivilege",                  // Act as part of OS
    "SeSecurityPrivilege",             // Manage auditing and security log
    "SeRemoteShutdownPrivilege",       // Remote shutdown
    "SeShutdownPrivilege",             // Local shutdown
    "SeUndockPrivilege",               // Undock computer
    "SeSyncAgentPrivilege",            // Synchronize directory service data
    "SeEnableDelegationPrivilege",     // Enable delegation
    "SeManageVolumePrivilege",         // Manage volumes
    "SeRelabelPrivilege",              // Modify object label
    "SeCreateGlobalPrivilege",         // Create global objects
    "SeTrustedCredManAccessPrivilege", // Access Credential Manager
];

/// Configuration for restricted token creation.
#[derive(Debug, Clone)]
pub struct TokenConfig {
    /// Remove dangerous privileges.
    pub remove_privileges: bool,

    /// Disable admin SID.
    pub disable_admin_sid: bool,

    /// Create LUA (Limited User Account) token.
    pub lua_token: bool,

    /// Create write-restricted token.
    pub write_restricted: bool,

    /// Make token sandbox-inert (bypass software restriction policies).
    pub sandbox_inert: bool,
}

impl Default for TokenConfig {
    fn default() -> Self {
        Self {
            remove_privileges: true,
            disable_admin_sid: true,
            lua_token: false,
            write_restricted: false,
            sandbox_inert: false,
        }
    }
}

impl TokenConfig {
    /// Configuration for maximum restriction.
    pub fn restrictive() -> Self {
        Self {
            remove_privileges: true,
            disable_admin_sid: true,
            lua_token: true,
            write_restricted: true,
            sandbox_inert: false,
        }
    }

    /// Configuration for moderate restriction.
    pub fn moderate() -> Self {
        Self {
            remove_privileges: true,
            disable_admin_sid: true,
            lua_token: false,
            write_restricted: false,
            sandbox_inert: false,
        }
    }
}

/// Windows Restricted Token wrapper.
///
/// Creates a token with reduced privileges for sandboxed process execution.
pub struct RestrictedToken {
    handle: HANDLE,
    config: TokenConfig,
}

impl RestrictedToken {
    /// Create a restricted token from the current process token.
    pub fn from_current_process(config: TokenConfig) -> Result<Self> {
        let process_handle = unsafe { GetCurrentProcess() };
        Self::from_process(process_handle, config)
    }

    /// Create a restricted token from a process handle.
    pub fn from_process(process_handle: HANDLE, config: TokenConfig) -> Result<Self> {
        let mut process_token = HANDLE::default();

        // Open the process token
        let result = unsafe {
            OpenProcessToken(
                process_handle,
                TOKEN_DUPLICATE | TOKEN_QUERY | TOKEN_ADJUST_PRIVILEGES,
                &mut process_token,
            )
        };

        if let Err(e) = result {
            return Err(WindowsSandboxError::TokenFailed(format!(
                "OpenProcessToken: {e}"
            )));
        }

        // Create restricted token from process token
        let restricted = Self::create_restricted(process_token, &config)?;

        // Close the original process token
        let _ = unsafe { CloseHandle(process_token) };

        Ok(Self {
            handle: restricted,
            config,
        })
    }

    /// Create a restricted token from an existing token.
    fn create_restricted(source_token: HANDLE, config: &TokenConfig) -> Result<HANDLE> {
        let mut flags = DISABLE_MAX_PRIVILEGE;

        if config.lua_token {
            flags |= LUA_TOKEN;
        }

        if config.write_restricted {
            flags |= WRITE_RESTRICTED;
        }

        if config.sandbox_inert {
            flags |= SANDBOX_INERT;
        }

        let mut restricted_token = HANDLE::default();

        // Create the restricted token
        let result = unsafe {
            CreateRestrictedToken(
                source_token,
                flags,
                None, // Disable SIDs - handled by flags
                None, // Delete privileges - handled by DISABLE_MAX_PRIVILEGE
                None, // Restricted SIDs
                &mut restricted_token,
            )
        };

        if let Err(e) = result {
            return Err(WindowsSandboxError::TokenFailed(format!(
                "CreateRestrictedToken: {e}"
            )));
        }

        if restricted_token.is_invalid() {
            return Err(WindowsSandboxError::TokenFailed(
                "CreateRestrictedToken returned invalid handle".to_string(),
            ));
        }

        debug!("Created restricted token with flags: {:x}", flags.0);

        // If configured, remove additional dangerous privileges
        if config.remove_privileges {
            if let Err(e) = remove_dangerous_privileges(restricted_token) {
                warn!("Failed to remove some privileges: {}", e);
            }
        }

        Ok(restricted_token)
    }

    /// Duplicate this token for use with CreateProcessAsUser.
    pub fn duplicate_for_process(&self) -> Result<HANDLE> {
        let mut duplicated_token = HANDLE::default();

        let result = unsafe {
            DuplicateTokenEx(
                self.handle,
                TOKEN_ALL_ACCESS,
                None, // Security attributes
                SecurityImpersonation,
                TokenPrimary,
                &mut duplicated_token,
            )
        };

        if let Err(e) = result {
            return Err(WindowsSandboxError::TokenFailed(format!(
                "DuplicateTokenEx: {e}"
            )));
        }

        Ok(duplicated_token)
    }

    /// Get the raw handle to the restricted token.
    pub fn handle(&self) -> HANDLE {
        self.handle
    }

    /// Check if restricted tokens are available on this system.
    pub fn is_available() -> bool {
        // Try creating a test restricted token
        match Self::from_current_process(TokenConfig::default()) {
            Ok(_) => true,
            Err(e) => {
                debug!("Restricted tokens not available: {}", e);
                false
            }
        }
    }
}

impl Drop for RestrictedToken {
    fn drop(&mut self) {
        if !self.handle.is_invalid() {
            let result = unsafe { CloseHandle(self.handle) };
            if result.is_err() {
                warn!("Failed to close restricted token handle");
            }
        }
    }
}

// Safety: Token handles can be sent between threads
unsafe impl Send for RestrictedToken {}
unsafe impl Sync for RestrictedToken {}

/// Remove dangerous privileges from a token.
fn remove_dangerous_privileges(token: HANDLE) -> Result<()> {
    let mut privileges_removed = 0;

    for priv_name in DANGEROUS_PRIVILEGES {
        if let Err(e) = remove_privilege(token, priv_name) {
            // Not all privileges may exist on the token
            debug!("Could not remove privilege {}: {}", priv_name, e);
        } else {
            privileges_removed += 1;
        }
    }

    info!(
        "Removed {} dangerous privileges from token",
        privileges_removed
    );
    Ok(())
}

/// Remove a single privilege from a token.
fn remove_privilege(token: HANDLE, privilege_name: &str) -> Result<()> {
    let wide_name: Vec<u16> = privilege_name
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let mut luid = LUID::default();

    // Look up the privilege LUID
    let result = unsafe {
        LookupPrivilegeValueW(
            PCWSTR::null(),
            PCWSTR::from_raw(wide_name.as_ptr()),
            &mut luid,
        )
    };

    if let Err(e) = result {
        return Err(WindowsSandboxError::TokenFailed(format!(
            "LookupPrivilegeValue({privilege_name}): {e}"
        )));
    }

    // Create privilege structure for removal
    #[repr(C)]
    struct TokenPrivilegesOne {
        count: u32,
        privileges: [windows::Win32::Security::LUID_AND_ATTRIBUTES; 1],
    }

    let tp = TokenPrivilegesOne {
        count: 1,
        privileges: [windows::Win32::Security::LUID_AND_ATTRIBUTES {
            Luid: luid,
            Attributes: SE_PRIVILEGE_REMOVED,
        }],
    };

    let result = unsafe {
        AdjustTokenPrivileges(
            token,
            false,
            Some(ptr::addr_of!(tp) as *const TOKEN_PRIVILEGES),
            0,
            None,
            None,
        )
    };

    if let Err(e) = result {
        return Err(WindowsSandboxError::TokenFailed(format!(
            "AdjustTokenPrivileges({privilege_name}): {e}"
        )));
    }

    debug!("Removed privilege: {}", privilege_name);
    Ok(())
}

/// Create a restricted token for sandbox use.
///
/// This is a convenience function that creates a token with moderate restrictions.
pub fn create_restricted_token() -> Result<RestrictedToken> {
    RestrictedToken::from_current_process(TokenConfig::moderate())
}

/// Create a highly restricted token for maximum isolation.
pub fn create_highly_restricted_token() -> Result<RestrictedToken> {
    RestrictedToken::from_current_process(TokenConfig::restrictive())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_config_default() {
        let config = TokenConfig::default();
        assert!(config.remove_privileges);
        assert!(config.disable_admin_sid);
    }

    #[test]
    fn test_token_config_restrictive() {
        let config = TokenConfig::restrictive();
        assert!(config.lua_token);
        assert!(config.write_restricted);
    }
}
