//! Login command handlers.

use crate::styled_output::{print_dim, print_error, print_info, print_success, print_warning};
use cortex_common::CliConfigOverrides;
use cortex_login::{
    AuthMode, CredentialsStoreMode, SecureAuthData, load_auth_with_fallback, logout_with_fallback,
    safe_format_key, save_auth_with_fallback,
};
use std::collections::HashSet;
use std::io::{IsTerminal, Read};
use std::path::PathBuf;

/// Check for duplicate config override keys and warn the user.
fn check_duplicate_config_overrides(config_overrides: &CliConfigOverrides) {
    let mut seen_keys = HashSet::new();
    let mut has_duplicates = false;

    for raw in &config_overrides.raw_overrides {
        // Extract key from KEY=VALUE format
        if let Some(key) = raw.split('=').next() {
            let key = key.trim();
            if !seen_keys.insert(key.to_string()) {
                has_duplicates = true;
            }
        }
    }

    if has_duplicates {
        print_warning(
            "Duplicate config override keys detected. Only the last value for each key will be used.",
        );
    }
}

/// Get the cortex home directory using unified directory management.
fn get_cortex_home() -> PathBuf {
    cortex_common::get_cortex_home().unwrap_or_else(|| {
        dirs::home_dir()
            .map(|h| h.join(".cortex"))
            .unwrap_or_else(|| PathBuf::from(".cortex"))
    })
}

/// Run login with API key.
pub async fn run_login_with_api_key(config_overrides: CliConfigOverrides, api_key: String) -> ! {
    check_duplicate_config_overrides(&config_overrides);
    let cortex_home = get_cortex_home();

    // Ensure cortex home directory exists for encrypted file fallback
    if let Err(e) = std::fs::create_dir_all(&cortex_home) {
        print_error(&format!(
            "Failed to create cortex home directory at {}: {e}\n\n\
             For headless/CI environments, set CORTEX_API_KEY environment variable instead:\n  \
             export CORTEX_API_KEY=your-api-key",
            cortex_home.display()
        ));
        std::process::exit(1);
    }

    // Create secure auth data
    let data = SecureAuthData::with_api_key(api_key);

    // Use save_auth_with_fallback for automatic keyring -> encrypted file fallback
    match save_auth_with_fallback(&cortex_home, &data) {
        Ok(mode) => {
            match mode {
                CredentialsStoreMode::Keyring => {
                    print_success("Logged in successfully. Credentials stored in system keyring.");
                }
                CredentialsStoreMode::EncryptedFile => {
                    print_success("Logged in successfully. Credentials stored in encrypted file.");
                    print_dim("System keyring unavailable, using encrypted file storage.");
                }
                CredentialsStoreMode::File => {
                    print_success("Logged in successfully (legacy storage).");
                }
            }
            std::process::exit(0);
        }
        Err(e) => {
            // Provide helpful error message for headless environments (Issue #1969)
            let error_str = e.to_string();
            if error_str.contains("keyring") || error_str.contains("secret service") {
                print_error(&format!(
                    "Login failed in headless environment: {e}\n\n\
                     For headless/CI environments, use the CORTEX_API_KEY environment variable:\n  \
                     export CORTEX_API_KEY=your-api-key\n\n\
                     Or on Linux, install a secret service like gnome-keyring or kwallet."
                ));
            } else {
                print_error(&format!("Login failed: {e}"));
            }
            std::process::exit(1);
        }
    }
}

/// Run login with device code flow.
pub async fn run_login_with_device_code(
    config_overrides: CliConfigOverrides,
    issuer_base_url: Option<String>,
    client_id: Option<String>,
) -> ! {
    check_duplicate_config_overrides(&config_overrides);
    let cortex_home = get_cortex_home();

    let mut opts = cortex_login::device_code_auth::DeviceCodeOptions::new(
        cortex_home,
        client_id.unwrap_or_else(|| cortex_login::CLIENT_ID.to_string()),
    );

    if let Some(issuer) = issuer_base_url {
        opts.issuer = issuer;
    }

    match cortex_login::run_device_code_login(opts).await {
        Ok(()) => {
            print_success("Logged in successfully.");
            std::process::exit(0);
        }
        Err(e) => {
            print_error(&format!("Login failed: {e}"));
            std::process::exit(1);
        }
    }
}

/// Run login status check.
pub async fn run_login_status(config_overrides: CliConfigOverrides) -> ! {
    check_duplicate_config_overrides(&config_overrides);
    let cortex_home = get_cortex_home();

    // Check environment variables first (CORTEX_AUTH_TOKEN and CORTEX_API_KEY)
    if let Ok(token) = std::env::var("CORTEX_AUTH_TOKEN")
        && !token.is_empty()
    {
        print_success(&format!(
            "Authenticated via CORTEX_AUTH_TOKEN environment variable: {}",
            safe_format_key(&token)
        ));
        std::process::exit(0);
    }

    if let Ok(token) = std::env::var("CORTEX_API_KEY")
        && !token.is_empty()
    {
        print_success(&format!(
            "Authenticated via CORTEX_API_KEY environment variable: {}",
            safe_format_key(&token)
        ));
        std::process::exit(0);
    }

    // Use load_auth_with_fallback for automatic keyring -> encrypted file -> legacy fallback
    match load_auth_with_fallback(&cortex_home) {
        Ok(Some(auth)) => match auth.mode {
            AuthMode::ApiKey => {
                if let Some(key) = auth.get_token() {
                    print_success(&format!(
                        "Logged in using an API key: {}",
                        safe_format_key(key)
                    ));
                    std::process::exit(0);
                } else {
                    print_warning("Logged in but no token available.");
                    std::process::exit(1);
                }
            }
            AuthMode::OAuth => {
                print_success("Logged in using OAuth.");
                if auth.is_expired() {
                    print_warning("Token may be expired.");
                }
                std::process::exit(0);
            }
        },
        Ok(None) => {
            print_info("Not logged in.");
            std::process::exit(1);
        }
        Err(e) => {
            print_error(&format!("Failed to check login status: {e}"));
            std::process::exit(1);
        }
    }
}

/// Run logout.
///
/// # Arguments
/// * `config_overrides` - CLI configuration overrides
/// * `skip_confirmation` - If true, skip the confirmation prompt (--yes flag)
pub async fn run_logout(config_overrides: CliConfigOverrides, skip_confirmation: bool) -> ! {
    check_duplicate_config_overrides(&config_overrides);
    let cortex_home = get_cortex_home();

    // Check if user is logged in first
    match load_auth_with_fallback(&cortex_home) {
        Ok(Some(_)) => {
            // User is logged in, ask for confirmation if terminal is interactive
            // unless --yes flag is passed
            if !skip_confirmation && std::io::stdin().is_terminal() {
                eprint!(
                    "Are you sure you want to log out? This will remove your stored credentials. [y/N]: "
                );
                let _ = std::io::Write::flush(&mut std::io::stderr());

                let mut input = String::new();
                if std::io::stdin().read_line(&mut input).is_ok() {
                    let input = input.trim().to_lowercase();
                    if input != "y" && input != "yes" {
                        print_info("Logout cancelled.");
                        std::process::exit(0);
                    }
                }
            }
        }
        Ok(None) => {
            print_info("Not logged in.");
            std::process::exit(0);
        }
        Err(e) => {
            print_error(&format!("Failed to check login status: {e}"));
            std::process::exit(1);
        }
    }

    // Use logout_with_fallback to clear credentials from all storage locations
    // (keyring, encrypted file, and legacy file) since save_auth_with_fallback
    // may have stored them in encrypted file when keyring was unavailable
    match logout_with_fallback(&cortex_home) {
        Ok(true) => {
            print_success("Logged out successfully. Credentials have been removed.");
            std::process::exit(0);
        }
        Ok(false) => {
            print_info("Not logged in.");
            std::process::exit(0);
        }
        Err(e) => {
            print_error(&format!("Failed to log out: {e}"));
            std::process::exit(1);
        }
    }
}

/// Read API key from stdin.
pub fn read_api_key_from_stdin() -> String {
    let mut stdin = std::io::stdin();

    if stdin.is_terminal() {
        print_error(
            "The --with-api-key flag expects input from stdin. Try piping it: \
             `printenv OPENAI_API_KEY | cortex login --with-api-key`",
        );
        std::process::exit(1);
    }

    print_info("Reading API key from stdin...");

    let mut buffer = String::new();
    if let Err(err) = stdin.read_to_string(&mut buffer) {
        print_error(&format!("Failed to read API key from stdin: {err}"));
        std::process::exit(1);
    }

    let api_key = buffer.trim().to_string();
    if api_key.is_empty() {
        print_error("No API key provided via stdin.");
        std::process::exit(1);
    }

    api_key
}
