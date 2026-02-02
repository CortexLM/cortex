//! Authentication and authorization module.
//!
//! Provides secure authentication, token management, and access control
//! for the Cortex CLI and API services.
//!
//! Security features:
//! - OS keychain storage for API keys
//! - Encrypted tokens at rest
//! - Secure memory handling with secrecy crate
//! - Automatic memory zeroization

mod manager;
mod secure;
mod store;
mod types;
mod utils;

// Re-export all public types for backwards compatibility
pub use manager::AuthManager;
pub use secure::{AuthCredential, SecureApiKey, SecureOAuth2Token};
pub use store::CredentialStore;
pub use types::{
    ApiKeyCredential, AuthConfig, CredentialType, CredentialValidation, JwtClaims, OAuth2Token,
    ProviderCredentialInfo,
};
pub use utils::{generate_token, validate_api_key_format};

/// Keyring service name for credentials.
#[allow(dead_code)]
const KEYRING_SERVICE: &str = "cortex-cli";

/// Keyring account for OAuth tokens.
#[allow(dead_code)]
const KEYRING_OAUTH_ACCOUNT: &str = "oauth-tokens";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_api_key_credential() {
        let cred = ApiKeyCredential::new("openai", "sk-test1234567890abcdef");
        assert_eq!(cred.provider, "openai");
        assert!(cred.key_prefix.starts_with("sk-test1"));
        assert!(cred.validate("sk-test1234567890abcdef"));
        assert!(!cred.validate("wrong-key"));
    }

    #[test]
    fn test_secure_api_key() {
        let key = SecureApiKey::new("openai", "sk-test1234567890abcdef");
        assert_eq!(key.provider(), "openai");
        assert_eq!(key.expose(), "sk-test1234567890abcdef");
        // redacted() returns first 8 chars + "..." + last 4 chars
        assert!(key.redacted().starts_with("sk-test1"));
        assert!(key.redacted().contains("..."));
    }

    #[test]
    fn test_oauth2_token_expiry() {
        let token = SecureOAuth2Token::new(
            "test-access-token",
            "Bearer",
            utils::current_timestamp() + 3600,
            None,
            vec![],
            "test",
        );
        assert!(!token.is_expired());
        assert!(!token.expires_soon(300));
    }

    #[test]
    fn test_jwt_claims() {
        let claims = JwtClaims::new("user123", "Cortex", 3600);
        assert!(!claims.is_expired());
        assert!(claims.is_valid_time());
    }
}
