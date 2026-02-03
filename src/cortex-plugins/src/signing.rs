//! Plugin signing and verification.
//!
//! Provides ed25519-based signature verification for plugin authenticity
//! and SHA256 checksum computation for integrity verification.

use ed25519_dalek::{Signature, VerifyingKey, Verifier};
use sha2::{Digest, Sha256};

use crate::{PluginError, Result};

/// Plugin signature verification using ed25519.
///
/// The signer maintains a list of trusted public keys and can verify
/// plugin signatures against them.
#[derive(Debug, Default)]
pub struct PluginSigner {
    /// Trusted public keys for signature verification
    trusted_keys: Vec<VerifyingKey>,
}

impl PluginSigner {
    /// Create a new plugin signer with no trusted keys.
    ///
    /// Use `add_trusted_key` to add keys that can verify plugin signatures.
    pub fn new() -> Self {
        Self {
            trusted_keys: Vec::new(),
        }
    }

    /// Add a trusted public key for signature verification.
    ///
    /// # Arguments
    /// * `key_bytes` - 32-byte ed25519 public key
    ///
    /// # Errors
    /// Returns an error if the key bytes are invalid.
    pub fn add_trusted_key(&mut self, key_bytes: &[u8]) -> Result<()> {
        if key_bytes.len() != 32 {
            return Err(PluginError::SignatureError(format!(
                "Invalid public key length: expected 32 bytes, got {}",
                key_bytes.len()
            )));
        }

        let key_array: [u8; 32] = key_bytes.try_into().map_err(|_| {
            PluginError::SignatureError("Failed to convert key bytes to array".to_string())
        })?;

        let verifying_key = VerifyingKey::from_bytes(&key_array).map_err(|e| {
            PluginError::SignatureError(format!("Invalid ed25519 public key: {}", e))
        })?;

        self.trusted_keys.push(verifying_key);
        tracing::debug!("Added trusted signing key");

        Ok(())
    }

    /// Add a trusted key from a hex-encoded string.
    ///
    /// # Arguments
    /// * `hex_key` - Hex-encoded 32-byte ed25519 public key (64 hex characters)
    ///
    /// # Errors
    /// Returns an error if the hex string is invalid or the key is invalid.
    pub fn add_trusted_key_hex(&mut self, hex_key: &str) -> Result<()> {
        let key_bytes = hex::decode(hex_key).map_err(|e| {
            PluginError::SignatureError(format!("Invalid hex-encoded key: {}", e))
        })?;

        self.add_trusted_key(&key_bytes)
    }

    /// Get the number of trusted keys.
    pub fn trusted_key_count(&self) -> usize {
        self.trusted_keys.len()
    }

    /// Check if any trusted keys are configured.
    pub fn has_trusted_keys(&self) -> bool {
        !self.trusted_keys.is_empty()
    }

    /// Verify a plugin's signature against the trusted keys.
    ///
    /// Returns `true` if the signature is valid and signed by any trusted key.
    /// Returns `false` if no trusted keys match the signature.
    ///
    /// # Arguments
    /// * `wasm_bytes` - The WASM module bytes to verify
    /// * `signature` - The 64-byte ed25519 signature
    ///
    /// # Errors
    /// Returns an error if the signature format is invalid.
    pub fn verify_plugin(&self, wasm_bytes: &[u8], signature: &[u8]) -> Result<bool> {
        if self.trusted_keys.is_empty() {
            tracing::warn!("No trusted keys configured - signature verification skipped");
            return Ok(false);
        }

        if signature.len() != 64 {
            return Err(PluginError::SignatureError(format!(
                "Invalid signature length: expected 64 bytes, got {}",
                signature.len()
            )));
        }

        let sig_array: [u8; 64] = signature.try_into().map_err(|_| {
            PluginError::SignatureError("Failed to convert signature bytes to array".to_string())
        })?;

        let sig = Signature::from_bytes(&sig_array);

        // Try each trusted key
        for key in &self.trusted_keys {
            if key.verify(wasm_bytes, &sig).is_ok() {
                tracing::debug!("Plugin signature verified successfully");
                return Ok(true);
            }
        }

        tracing::warn!("Plugin signature verification failed - no trusted key matched");
        Ok(false)
    }

    /// Verify a plugin signature from hex-encoded signature string.
    ///
    /// # Arguments
    /// * `wasm_bytes` - The WASM module bytes to verify
    /// * `signature_hex` - Hex-encoded 64-byte ed25519 signature (128 hex characters)
    ///
    /// # Errors
    /// Returns an error if the hex string or signature is invalid.
    pub fn verify_plugin_hex(&self, wasm_bytes: &[u8], signature_hex: &str) -> Result<bool> {
        let signature_bytes = hex::decode(signature_hex).map_err(|e| {
            PluginError::SignatureError(format!("Invalid hex-encoded signature: {}", e))
        })?;

        self.verify_plugin(wasm_bytes, &signature_bytes)
    }

    /// Compute SHA256 checksum of data and return as hex string.
    ///
    /// This is used to verify plugin integrity before loading.
    ///
    /// # Arguments
    /// * `data` - The data to compute checksum for
    ///
    /// # Returns
    /// A lowercase hex-encoded SHA256 hash (64 characters).
    pub fn compute_checksum(data: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(data);
        let result = hasher.finalize();
        hex::encode(result)
    }

    /// Verify that data matches an expected checksum.
    ///
    /// # Arguments
    /// * `data` - The data to verify
    /// * `expected_checksum` - The expected hex-encoded SHA256 hash
    ///
    /// # Returns
    /// `true` if the checksum matches, `false` otherwise.
    pub fn verify_checksum(data: &[u8], expected_checksum: &str) -> bool {
        let computed = Self::compute_checksum(data);
        computed.eq_ignore_ascii_case(expected_checksum)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Test key pair generated for testing purposes only
    // Private key: used only for signing in tests
    // Public key: the key we add as trusted
    const TEST_PUBLIC_KEY_HEX: &str =
        "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a";

    #[test]
    fn test_signer_new() {
        let signer = PluginSigner::new();
        assert_eq!(signer.trusted_key_count(), 0);
        assert!(!signer.has_trusted_keys());
    }

    #[test]
    fn test_add_trusted_key_hex() {
        let mut signer = PluginSigner::new();
        signer.add_trusted_key_hex(TEST_PUBLIC_KEY_HEX).unwrap();

        assert_eq!(signer.trusted_key_count(), 1);
        assert!(signer.has_trusted_keys());
    }

    #[test]
    fn test_add_trusted_key_invalid_length() {
        let mut signer = PluginSigner::new();
        let result = signer.add_trusted_key(&[0u8; 16]);

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Invalid public key length"));
    }

    #[test]
    fn test_add_trusted_key_invalid_hex() {
        let mut signer = PluginSigner::new();
        let result = signer.add_trusted_key_hex("invalid_hex");

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Invalid hex-encoded key"));
    }

    #[test]
    fn test_verify_no_trusted_keys() {
        let signer = PluginSigner::new();
        let result = signer.verify_plugin(&[1, 2, 3], &[0u8; 64]).unwrap();

        assert!(!result);
    }

    #[test]
    fn test_verify_invalid_signature_length() {
        let mut signer = PluginSigner::new();
        signer.add_trusted_key_hex(TEST_PUBLIC_KEY_HEX).unwrap();

        let result = signer.verify_plugin(&[1, 2, 3], &[0u8; 32]);

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Invalid signature length"));
    }

    #[test]
    fn test_compute_checksum() {
        let data = b"hello world";
        let checksum = PluginSigner::compute_checksum(data);

        // SHA256 of "hello world"
        assert_eq!(
            checksum,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    #[test]
    fn test_verify_checksum_valid() {
        let data = b"test data";
        let checksum = PluginSigner::compute_checksum(data);

        assert!(PluginSigner::verify_checksum(data, &checksum));
    }

    #[test]
    fn test_verify_checksum_case_insensitive() {
        let data = b"test data";
        let checksum = PluginSigner::compute_checksum(data);

        assert!(PluginSigner::verify_checksum(data, &checksum.to_uppercase()));
    }

    #[test]
    fn test_verify_checksum_invalid() {
        let data = b"test data";
        let wrong_checksum = "0000000000000000000000000000000000000000000000000000000000000000";

        assert!(!PluginSigner::verify_checksum(data, wrong_checksum));
    }

    #[test]
    fn test_compute_checksum_empty() {
        let data = b"";
        let checksum = PluginSigner::compute_checksum(data);

        // SHA256 of empty string
        assert_eq!(
            checksum,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn test_verify_plugin_hex_invalid_hex() {
        let mut signer = PluginSigner::new();
        signer.add_trusted_key_hex(TEST_PUBLIC_KEY_HEX).unwrap();

        let result = signer.verify_plugin_hex(&[1, 2, 3], "not_valid_hex");

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Invalid hex-encoded signature"));
    }
}
