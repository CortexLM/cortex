//! PKCE (Proof Key for Code Exchange) implementation.
//!
//! This implements RFC 7636 for enhanced security in OAuth 2.0 flows.
//!
//! SECURITY: Only S256 challenge method is supported. The Plain method
//! is explicitly NOT implemented as it defeats the purpose of PKCE.

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use sha2::{Digest, Sha256};

/// PKCE challenge method.
///
/// SECURITY: Only S256 is supported. Plain method is intentionally omitted
/// as it provides no security benefit and defeats the purpose of PKCE.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChallengeMethod {
    /// S256 challenge method (SHA-256 hash, base64url encoded).
    /// This is the only secure PKCE method per RFC 7636.
    S256,
}

impl std::fmt::Display for ChallengeMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChallengeMethod::S256 => write!(f, "S256"),
        }
    }
}

/// PKCE code verifier and challenge pair.
#[derive(Debug, Clone)]
pub struct PkceChallenge {
    /// The code verifier (secret, kept client-side).
    pub verifier: String,
    /// The code challenge (SHA-256 hash of verifier, sent to authorization server).
    pub challenge: String,
    /// The challenge method used (always S256).
    pub method: ChallengeMethod,
}

impl PkceChallenge {
    /// Generate a new PKCE challenge using S256 method.
    ///
    /// This generates a cryptographically random verifier and computes
    /// the S256 challenge (SHA-256 hash, base64url-encoded without padding).
    pub fn new() -> Self {
        let verifier = generate_verifier();
        let challenge = compute_s256_challenge(&verifier);

        Self {
            verifier,
            challenge,
            method: ChallengeMethod::S256,
        }
    }

    /// Create a PKCE challenge from an existing verifier using S256.
    ///
    /// # Arguments
    /// * `verifier` - A code verifier string (should be 43-128 characters, URL-safe)
    ///
    /// # Returns
    /// A PkceChallenge with the computed S256 challenge.
    pub fn from_verifier(verifier: String) -> Self {
        let challenge = compute_s256_challenge(&verifier);

        Self {
            verifier,
            challenge,
            method: ChallengeMethod::S256,
        }
    }
}

impl Default for PkceChallenge {
    fn default() -> Self {
        Self::new()
    }
}

/// Generate a cryptographically random code verifier.
///
/// Per RFC 7636, the verifier should be 43-128 characters long,
/// using only unreserved URI characters: [A-Z] / [a-z] / [0-9] / "-" / "." / "_" / "~"
///
/// We use 32 random bytes base64url-encoded (no padding), resulting in 43 characters.
fn generate_verifier() -> String {
    use rand::Rng;

    let mut rng = rand::rng();
    let bytes: Vec<u8> = (0..32).map(|_| rng.random()).collect();
    URL_SAFE_NO_PAD.encode(&bytes)
}

/// Compute the S256 code challenge from a verifier.
///
/// S256: BASE64URL(SHA256(ASCII(code_verifier)))
fn compute_s256_challenge(verifier: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    let hash = hasher.finalize();
    URL_SAFE_NO_PAD.encode(hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pkce_generation() {
        let pkce = PkceChallenge::new();

        // Verifier should be exactly 43 characters (32 bytes base64url encoded)
        assert_eq!(pkce.verifier.len(), 43);

        // Challenge should be exactly 43 characters (32 bytes SHA-256 hash base64url encoded)
        assert_eq!(pkce.challenge.len(), 43);

        // Challenge should be different from verifier (it's a hash)
        assert_ne!(pkce.verifier, pkce.challenge);

        // Method should be S256
        assert_eq!(pkce.method, ChallengeMethod::S256);
    }

    #[test]
    fn test_pkce_from_verifier() {
        let verifier = "test_verifier_with_enough_characters_to_be_valid";
        let pkce = PkceChallenge::from_verifier(verifier.to_string());

        assert_eq!(pkce.verifier, verifier);
        assert_ne!(pkce.challenge, verifier);
        assert_eq!(pkce.method, ChallengeMethod::S256);
    }

    #[test]
    fn test_pkce_deterministic() {
        // Same verifier should produce same challenge
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let pkce1 = PkceChallenge::from_verifier(verifier.to_string());
        let pkce2 = PkceChallenge::from_verifier(verifier.to_string());

        assert_eq!(pkce1.challenge, pkce2.challenge);
    }

    #[test]
    fn test_verifier_character_set() {
        // Verify that generated verifiers only contain URL-safe base64 characters
        for _ in 0..100 {
            let pkce = PkceChallenge::new();
            for c in pkce.verifier.chars() {
                assert!(
                    c.is_ascii_alphanumeric() || c == '-' || c == '_',
                    "Invalid character in verifier: {c}"
                );
            }
        }
    }
}
