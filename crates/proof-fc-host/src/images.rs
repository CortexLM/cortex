//! Digest-pinned images. An image boots only if the bytes on disk hash to
//! the `sha256:` the control plane (RLM image) or the operator (kernel,
//! sister image) pinned. Verified files are remembered by `(len, mtime)` so
//! a multi-GiB rootfs is hashed once per change, not once per boot.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use proof_vm_agent::HvError;
use sha2::{Digest, Sha256};

/// The 64 hex chars of a `sha256:<hex>` pin (case-insensitive, trimmed).
#[must_use]
pub fn digest_hex(pin: &str) -> Option<String> {
    let hex = pin
        .trim()
        .strip_prefix("sha256:")?
        .trim()
        .to_ascii_lowercase();
    (hex.len() == 64 && hex.chars().all(|c| c.is_ascii_hexdigit())).then_some(hex)
}

/// `<image_dir>/sha256-<hex>.ext4` for a pin.
///
/// # Errors
///
/// [`HvError::Image`] when the pin is malformed or the file is absent.
pub fn image_path(image_dir: &Path, pin: &str) -> Result<PathBuf, HvError> {
    let hex =
        digest_hex(pin).ok_or_else(|| HvError::Image(format!("{pin:?} is not sha256:<hex>")))?;
    let path = image_dir.join(format!("sha256-{hex}.ext4"));
    if path.is_file() {
        Ok(path)
    } else {
        Err(HvError::Image(format!(
            "sha256:{hex} (no {} on this host)",
            path.display()
        )))
    }
}

/// sha256 hex of a file, streamed.
///
/// # Errors
///
/// [`HvError::Backend`] on I/O.
pub fn sha256_file(path: &Path) -> Result<String, HvError> {
    let mut f = std::fs::File::open(path)
        .map_err(|e| HvError::Backend(format!("open {}: {e}", path.display())))?;
    let mut h = Sha256::new();
    std::io::copy(&mut f, &mut h)
        .map_err(|e| HvError::Backend(format!("read {}: {e}", path.display())))?;
    Ok(hex::encode(h.finalize()))
}

fn stamp(path: &Path) -> Option<(u64, SystemTime)> {
    let m = std::fs::metadata(path).ok()?;
    Some((m.len(), m.modified().ok()?))
}

/// Remembers which files verified against which digest.
#[derive(Default)]
pub struct ImageCache {
    verified: Mutex<HashMap<PathBuf, (String, u64, SystemTime)>>,
}

impl ImageCache {
    /// Verify `path` hashes to `pin`, hashing only when the file changed.
    ///
    /// # Errors
    ///
    /// [`HvError::Image`] on mismatch, [`HvError::Backend`] on I/O.
    pub async fn verify(&self, path: &Path, pin: &str) -> Result<(), HvError> {
        let want = digest_hex(pin)
            .ok_or_else(|| HvError::Image(format!("{pin:?} is not sha256:<hex>")))?;
        let now = stamp(path)
            .ok_or_else(|| HvError::Image(format!("{} is not readable", path.display())))?;
        {
            let cache = self
                .verified
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some((hex, len, mtime)) = cache.get(path) {
                if *hex == want && (*len, *mtime) == now {
                    return Ok(());
                }
            }
        }
        let p = path.to_path_buf();
        let got = tokio::task::spawn_blocking(move || sha256_file(&p))
            .await
            .map_err(|e| HvError::Backend(format!("hash task: {e}")))??;
        if got != want {
            return Err(HvError::Image(format!(
                "{} hashes to sha256:{got}, pin is sha256:{want}",
                path.display()
            )));
        }
        self.verified
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(path.to_path_buf(), (want, now.0, now.1));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("proof-fc-images-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("dir");
        d
    }

    #[test]
    fn pins_are_sha256_hex_and_images_are_named_by_them() {
        let hex = "ab".repeat(32);
        assert_eq!(
            digest_hex(&format!(" sha256:{} ", hex.to_uppercase())),
            Some(hex.clone())
        );
        assert_eq!(digest_hex(&hex), None, "prefix required");
        assert_eq!(digest_hex("sha256:abc"), None);
        assert_eq!(digest_hex(""), None);
        let d = dir("paths");
        let err = image_path(&d, &format!("sha256:{hex}")).expect_err("absent");
        assert!(matches!(err, HvError::Image(_)), "{err}");
        std::fs::write(d.join(format!("sha256-{hex}.ext4")), b"x").expect("write");
        assert_eq!(
            image_path(&d, &format!("sha256:{hex}")).expect("present"),
            d.join(format!("sha256-{hex}.ext4"))
        );
        assert!(image_path(&d, "not-a-pin").is_err());
    }

    #[tokio::test]
    async fn verify_hashes_once_per_change_and_refuses_a_mismatch() {
        let d = dir("verify");
        let file = d.join("rootfs.ext4");
        std::fs::write(&file, b"rootfs bytes").expect("write");
        let good = format!("sha256:{}", sha256_file(&file).expect("hash"));
        let cache = ImageCache::default();
        cache.verify(&file, &good).await.expect("matches");
        cache.verify(&file, &good).await.expect("cached");
        let bad = format!("sha256:{}", "00".repeat(32));
        let err = cache.verify(&file, &bad).await.expect_err("mismatch");
        assert!(matches!(err, HvError::Image(_)), "{err}");
        assert!(err.to_string().contains("pin is"), "{err}");
        std::fs::write(&file, b"tampered after verification").expect("rewrite");
        let err = cache
            .verify(&file, &good)
            .await
            .expect_err("changed bytes are re-hashed");
        assert!(matches!(err, HvError::Image(_)), "{err}");
        assert!(cache.verify(&d.join("missing"), &good).await.is_err());
        assert!(cache.verify(&file, "junk").await.is_err());
    }
}
