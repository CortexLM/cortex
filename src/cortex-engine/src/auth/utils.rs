//! Authentication utility functions.
//!
//! Provides helper functions for key hashing, token generation,
//! and format validation.

use std::time::{SystemTime, UNIX_EPOCH};

use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use secrecy::SecretString;
use sha2::{Digest, Sha256};

/// Get environment variable name for a provider.
pub fn provider_env_var(_provider: &str) -> String {
    "CORTEX_AUTH_TOKEN".to_string()
}

/// Hash an API key for storage.
pub fn hash_key(key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(key.as_bytes());
    let result = hasher.finalize();
    BASE64.encode(result)
}

/// Get current Unix timestamp.
pub fn current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Generate a secure random token.
pub fn generate_token(length: usize) -> SecretString {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let bytes: Vec<u8> = (0..length).map(|_| rng.r#gen::<u8>()).collect();
    SecretString::from(BASE64.encode(bytes))
}

/// Validate an API key format.
pub fn validate_api_key_format(provider: &str, key: &str) -> bool {
    match provider {
        "openai" => key.starts_with("sk-") && key.len() > 20,
        "anthropic" => key.starts_with("sk-ant-") && key.len() > 20,
        "google" => key.len() > 20,
        _ => key.len() > 10,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_env_var() {
        assert_eq!(provider_env_var("cortex"), "CORTEX_AUTH_TOKEN");
        assert_eq!(provider_env_var("any_provider"), "CORTEX_AUTH_TOKEN");
    }

    #[test]
    fn test_validate_api_key_format() {
        assert!(validate_api_key_format("openai", "sk-1234567890abcdefghij"));
        assert!(!validate_api_key_format("openai", "invalid"));
    }

    #[test]
    fn test_generate_token() {
        use secrecy::ExposeSecret;
        let token = generate_token(32);
        assert!(!token.expose_secret().is_empty());
    }
}
