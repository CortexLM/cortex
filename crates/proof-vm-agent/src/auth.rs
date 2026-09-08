//! Bearer authentication from a token **file**.
//!
//! The file is re-read on every check, so an operator rotates the token by
//! rewriting the file — no restart, no env var, nothing in a process listing.
//! A missing or empty file refuses every request (fail-closed). Tokens are
//! compared as SHA-256 digests in constant time and are never logged.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

/// Why a request was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AuthError {
    /// No usable token on the host: nothing can authenticate.
    #[error("agent has no bearer token file; refusing every request")]
    NoToken,
    /// Header missing, malformed, or wrong.
    #[error("bearer rejected")]
    Rejected,
}

/// Bearer check backed by a token file.
pub struct BearerAuth {
    token_file: PathBuf,
}

impl std::fmt::Debug for BearerAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BearerAuth")
            .field("token_file", &self.token_file)
            .finish()
    }
}

impl BearerAuth {
    /// Authenticate against the trimmed contents of `token_file`.
    #[must_use]
    pub fn from_file(token_file: &Path) -> Self {
        Self {
            token_file: token_file.to_path_buf(),
        }
    }

    /// The file this checks (for logs; never the contents).
    #[must_use]
    pub fn token_file(&self) -> &Path {
        &self.token_file
    }

    fn expected_digest(&self) -> Option<[u8; 32]> {
        let raw = std::fs::read_to_string(&self.token_file).ok()?;
        let token = raw.trim();
        if token.is_empty() {
            return None;
        }
        Some(Sha256::digest(token.as_bytes()).into())
    }

    /// Whether the host has a usable token at all.
    #[must_use]
    pub fn configured(&self) -> bool {
        self.expected_digest().is_some()
    }

    /// Check an `Authorization` header value.
    ///
    /// # Errors
    ///
    /// [`AuthError::NoToken`] when the file is missing / empty,
    /// [`AuthError::Rejected`] otherwise on mismatch.
    pub fn accepts(&self, authorization: Option<&str>) -> Result<(), AuthError> {
        let expected = self.expected_digest().ok_or(AuthError::NoToken)?;
        let presented = authorization
            .and_then(|h| {
                h.strip_prefix("Bearer ")
                    .or_else(|| h.strip_prefix("bearer "))
            })
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .ok_or(AuthError::Rejected)?;
        let got: [u8; 32] = Sha256::digest(presented.as_bytes()).into();
        if bool::from(got.ct_eq(&expected)) {
            Ok(())
        } else {
            Err(AuthError::Rejected)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("proof-vm-agent-auth-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        dir.join(name)
    }

    #[test]
    fn a_missing_or_empty_file_refuses_everything() {
        let auth = BearerAuth::from_file(&tmp("missing"));
        assert!(!auth.configured());
        assert_eq!(
            auth.accepts(Some("Bearer anything")),
            Err(AuthError::NoToken)
        );
        let empty = tmp("empty");
        std::fs::write(&empty, " \n").expect("write");
        let auth = BearerAuth::from_file(&empty);
        assert_eq!(auth.accepts(Some("Bearer x")), Err(AuthError::NoToken));
    }

    #[test]
    fn debug_shows_the_path_never_the_token() {
        let file = tmp("debug");
        std::fs::write(&file, "debug-token-not-a-real-secret\n").expect("write");
        let auth = BearerAuth::from_file(&file);
        let dump = format!("{auth:?}");
        assert!(dump.contains("token_file"), "{dump}");
        assert!(!dump.contains("debug-token-not-a-real-secret"), "{dump}");
        assert_eq!(auth.token_file(), file.as_path());
    }

    #[test]
    fn the_file_contents_are_the_token_and_rotate_without_restart() {
        let file = tmp("token");
        std::fs::write(&file, "  first-token-not-a-real-secret \n").expect("write");
        let auth = BearerAuth::from_file(&file);
        assert!(auth.configured());
        auth.accepts(Some("Bearer first-token-not-a-real-secret"))
            .expect("exact");
        auth.accepts(Some("bearer first-token-not-a-real-secret "))
            .expect("case-insensitive scheme, trimmed");
        assert_eq!(auth.accepts(None), Err(AuthError::Rejected));
        assert_eq!(auth.accepts(Some("Bearer ")), Err(AuthError::Rejected));
        assert_eq!(
            auth.accepts(Some("Basic first-token-not-a-real-secret")),
            Err(AuthError::Rejected)
        );
        assert_eq!(
            auth.accepts(Some("Bearer first-token-not-a-real-secre")),
            Err(AuthError::Rejected)
        );
        std::fs::write(&file, "second-token-not-a-real-secret\n").expect("rotate");
        assert_eq!(
            auth.accepts(Some("Bearer first-token-not-a-real-secret")),
            Err(AuthError::Rejected)
        );
        auth.accepts(Some("Bearer second-token-not-a-real-secret"))
            .expect("rotated");
    }
}
