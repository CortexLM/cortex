//! Challenge signing key load (`BASE_CHALLENGE_SK_FILE`, mode 0600 file).

use std::fs;
use std::path::Path;

use crypto::{secret_from_bytes, KEY_LEN};
use thiserror::Error;

/// Key load / expand errors.
#[derive(Debug, Error)]
pub enum ChallengeKeyError {
    /// I/O failure reading the secret file.
    #[error("read challenge secret: {0}")]
    Io(#[from] std::io::Error),
    /// Secret file is not 32 raw bytes or 64 hex chars.
    #[error("challenge secret must be 32 raw bytes or 64 hex chars, got {0} bytes")]
    BadLength(usize),
    /// Hex decode failed.
    #[error("challenge secret hex decode: {0}")]
    Hex(String),
    /// Mini-secret expand failed.
    #[error("invalid challenge mini-secret")]
    InvalidSecret,
}

/// Load a 32-byte mini-secret from `path` (raw bytes or hex text).
///
/// # Errors
///
/// See [`ChallengeKeyError`].
pub fn load_challenge_secret(path: &Path) -> Result<[u8; KEY_LEN], ChallengeKeyError> {
    let raw = fs::read(path)?;
    parse_challenge_secret(&raw)
}

/// Parse an already securely read mini-secret without reopening its source.
///
/// # Errors
///
/// See [`ChallengeKeyError`].
pub fn parse_challenge_secret(raw: &[u8]) -> Result<[u8; KEY_LEN], ChallengeKeyError> {
    if raw.len() == KEY_LEN {
        let mut out = [0u8; KEY_LEN];
        out.copy_from_slice(raw);
        // Validate expandable.
        secret_from_bytes(&out).map_err(|_| ChallengeKeyError::InvalidSecret)?;
        return Ok(out);
    }
    let text = std::str::from_utf8(raw).map_err(|e| ChallengeKeyError::Hex(e.to_string()))?;
    let trimmed = text.trim();
    let hex_s = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
        .unwrap_or(trimmed);
    let bytes = hex::decode(hex_s).map_err(|e| ChallengeKeyError::Hex(e.to_string()))?;
    if bytes.len() != KEY_LEN {
        return Err(ChallengeKeyError::BadLength(bytes.len()));
    }
    let mut out = [0u8; KEY_LEN];
    out.copy_from_slice(&bytes);
    secret_from_bytes(&out).map_err(|_| ChallengeKeyError::InvalidSecret)?;
    Ok(out)
}

/// Derive the 32-byte public key from a mini-secret.
///
/// # Errors
///
/// [`ChallengeKeyError::InvalidSecret`] if the mini-secret is malformed.
pub fn public_key_from_secret(secret: &[u8; KEY_LEN]) -> Result<[u8; KEY_LEN], ChallengeKeyError> {
    let sk = secret_from_bytes(secret).map_err(|_| ChallengeKeyError::InvalidSecret)?;
    Ok(sk.to_public().to_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::unwrap_used)]
    fn parses_binary_and_hex_without_reopening_a_path() {
        let raw = [0xff; KEY_LEN];
        assert_eq!(parse_challenge_secret(&raw).unwrap(), raw);
        for text in [hex::encode(raw), format!(" \n0x{} \n", hex::encode(raw))] {
            assert_eq!(parse_challenge_secret(text.as_bytes()).unwrap(), raw);
        }
        assert!(parse_challenge_secret(b"").is_err());
        assert!(parse_challenge_secret(&[0xff; KEY_LEN + 1]).is_err());
        assert!(parse_challenge_secret(b"invalid").is_err());
    }
}
