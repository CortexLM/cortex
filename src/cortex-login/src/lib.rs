#![allow(warnings, clippy::all)]
//! Cortex Login - Authentication module for Cortex CLI.
//!
//! Provides various authentication methods:
//! - API key authentication
//! - Device code OAuth flow
//! - PKCE OAuth flow
//!
//! Security features:
//! - OS keychain integration (Windows Credential Manager, macOS Keychain, Linux Secret Service)
//! - Encrypted file storage with AES-256-GCM
//! - Secure memory handling with secrecy crate
//! - File permissions enforcement (0600)

// Core modules
pub mod constants;
pub mod types;
mod utils;

// Storage backends
mod encrypted;
pub mod keyring;
mod legacy;

// High-level APIs
mod storage;
mod token;

// Authentication flows
pub mod device_code_auth;
pub mod pkce;
mod server;

// Re-exports from constants
pub use constants::{
    API_KEY_ENV_VAR, CLIENT_ID, DEFAULT_ISSUER, KEYRING_ACCOUNT, KEYRING_SERVICE,
    KEYRING_SERVICE_LEGACY, KEYRING_SERVICE_NEW,
};

// Re-exports from types
pub use types::{AuthData, AuthMode, CredentialsStoreMode, SecureAuthData};

// Re-exports from storage
pub use storage::{
    delete_auth, has_valid_auth, load_auth, load_auth_with_fallback, login_with_api_key, logout,
    logout_with_fallback, migrate_to_secure_storage, save_auth, save_auth_with_fallback,
};

// Re-exports from keyring (for direct access if needed)
pub use keyring::{delete_from_keyring, load_from_keyring, save_to_keyring};

// Re-exports from token
pub use token::get_auth_token;

// Re-exports from utils
pub use utils::safe_format_key;

// Re-exports from device_code_auth
pub use device_code_auth::run_device_code_login;

// Re-exports from server
pub use server::{LoginServer, ServerOptions, ShutdownHandle, run_login_server};
