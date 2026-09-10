//! What the host stages into the guest: owner key material (tmpfs) and the
//! experiment pack (writable disk), both by name-checked files only.

use std::path::{Path, PathBuf};

use proof_experiment::PackRef;
use proof_vm_proto::guest::StagedFile;
use proof_vm_proto::tar::verify_artifact;

/// Longest file name the guest accepts from the host.
const MAX_NAME_LEN: usize = 128;

/// Smallest secret worth redacting (shorter strings would blank ordinary text).
pub const MIN_SECRET_LEN: usize = 8;

/// One staged experiment pack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedPack {
    /// `sha256:<hex>` the bytes verified against.
    pub digest: String,
    /// Where the tree is unpacked.
    pub dir: PathBuf,
    /// Bytes of the tar as received.
    pub bytes: u64,
}

/// A single plain path segment: no separators, not `.` / `..`, printable.
pub fn safe_name(name: &str) -> Result<&str, String> {
    let n = name.trim();
    let ok = !n.is_empty()
        && n.len() <= MAX_NAME_LEN
        && n != "."
        && n != ".."
        && !n.contains(['/', '\\'])
        && !n.chars().any(char::is_control);
    if ok {
        Ok(n)
    } else {
        Err(format!(
            "staged file name {name:?} is not a plain file name"
        ))
    }
}

/// Write `files` under `dir` (created 0700), each 0600, replacing any
/// earlier file of the same name. With `owner`, the directory and files are
/// handed to that uid / gid — the unprivileged user adaptors run as — so a
/// root agent can stage what a rootless adaptor reads. Returns how many were
/// written.
pub fn stage_secrets(
    dir: &Path,
    files: &[StagedFile],
    owner: Option<(u32, u32)>,
) -> Result<usize, String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
        .map_err(|e| format!("chmod {}: {e}", dir.display()))?;
    let own = |path: &Path| -> Result<(), String> {
        if let Some((uid, gid)) = owner {
            std::os::unix::fs::chown(path, Some(uid), Some(gid))
                .map_err(|e| format!("chown {}: {e}", path.display()))?;
        }
        Ok(())
    };
    own(dir)?;
    for f in files {
        let name = safe_name(&f.name)?;
        let bytes = f.bytes().map_err(|e| e.to_string())?;
        let tmp = dir.join(format!(".{name}.tmp"));
        std::fs::write(&tmp, &bytes).map_err(|e| format!("write secret {name}: {e}"))?;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("chmod secret {name}: {e}"))?;
        own(&tmp)?;
        std::fs::rename(&tmp, dir.join(name)).map_err(|e| format!("place secret {name}: {e}"))?;
    }
    Ok(files.len())
}

/// Subdirectory of the guest secrets root holding the **miner's** own BYOK
/// values, one file per variable.
///
/// Kept apart from the owner key material the host stages beside it: these
/// belong to the miner whose submission is running, they arrive with the job
/// rather than at boot, and they are not listed in `PROOF_SECRET_FILES`. An
/// adaptor that would rather read a file than an environment variable finds
/// them at `$PROOF_MINER_ENV_DIR/<NAME>`.
pub const MINER_ENV_SUBDIR: &str = "miner";

/// Write one file per miner BYOK variable under
/// `<secrets_dir>/<MINER_ENV_SUBDIR>/`, 0700 dir and 0600 files owned by the
/// user the adaptor runs as. Returns the directory.
///
/// The names have already been held to the signed topic's allowlist by the
/// control plane and re-checked by the caller; `safe_name` refuses anything
/// that is not a plain file name whatever happens upstream.
pub fn stage_miner_env(
    secrets_dir: &Path,
    vars: &[(String, String)],
    owner: Option<(u32, u32)>,
) -> Result<PathBuf, String> {
    // Create / lock down the parent first: on a VM where the host staged no
    // owner key material it may not exist yet, and it must not be world-readable.
    stage_secrets(secrets_dir, &[], owner)?;
    let dir = secrets_dir.join(MINER_ENV_SUBDIR);
    let files: Vec<StagedFile> = vars
        .iter()
        .map(|(name, value)| StagedFile::new(name, value.as_bytes()))
        .collect();
    stage_secrets(&dir, &files, owner)?;
    Ok(dir)
}

/// The staged secret values (trimmed, long enough to matter) — what the
/// agent redacts from everything it sends back. Never logged.
#[must_use]
pub fn secret_values(dir: &Path) -> Vec<Vec<u8>> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        if !entry.path().is_file() {
            continue;
        }
        if let Ok(bytes) = std::fs::read(entry.path()) {
            let trimmed = String::from_utf8_lossy(&bytes).trim().to_owned();
            if trimmed.len() >= MIN_SECRET_LEN {
                out.push(trimmed.into_bytes());
            }
        }
    }
    out
}

/// Names of the staged secret files (what an adaptor may look for).
#[must_use]
pub fn secret_names(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|e| e.path().is_file())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !n.starts_with('.'))
        .collect();
    names.sort();
    names
}

/// Unpack an **uncompressed tar** into `dest`: regular files and directories
/// only (no links, no devices), never outside `dest`. Directories come out
/// `0755` and files `0644` plus the archive's executable bit — ownership and
/// special bits are never preserved. Returns the number of files written.
pub fn unpack_tar(bytes: &[u8], dest: &Path) -> Result<usize, String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(dest).map_err(|e| format!("mkdir {}: {e}", dest.display()))?;
    let mut archive = tar::Archive::new(bytes);
    archive.set_preserve_permissions(false);
    archive.set_preserve_ownerships(false);
    archive.set_unpack_xattrs(false);
    let mut files = 0usize;
    let entries = archive.entries().map_err(|e| format!("tar: {e}"))?;
    for entry in entries {
        let mut entry = entry.map_err(|e| format!("tar entry: {e}"))?;
        let kind = entry.header().entry_type();
        if !(kind.is_file() || kind.is_dir()) {
            // Links and specials are not part of a recipe tree; skipping them
            // (rather than failing) keeps packs with stray symlinks usable
            // while never creating an escape hatch under `dest`.
            continue;
        }
        let path = entry
            .path()
            .map_err(|e| format!("tar path: {e}"))?
            .into_owned();
        if path
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
        {
            return Err(format!(
                "tar member {} is not a plain relative path",
                path.display()
            ));
        }
        let mode = entry.header().mode().unwrap_or(0o644);
        let unpacked = entry
            .unpack_in(dest)
            .map_err(|e| format!("unpack {}: {e}", path.display()))?;
        if !unpacked {
            continue;
        }
        let on_disk = dest.join(&path);
        let perm = if kind.is_dir() {
            0o755
        } else {
            files += 1;
            0o644 | (mode & 0o111)
        };
        std::fs::set_permissions(&on_disk, std::fs::Permissions::from_mode(perm))
            .map_err(|e| format!("chmod {}: {e}", on_disk.display()))?;
    }
    Ok(files)
}

/// Verify `pack_tar` against `digest` (shape, then sha256) and unpack it to
/// `<root>/<hex>/`, replacing an earlier copy.
pub fn stage_pack(root: &Path, digest: &str, pack_tar: &StagedFile) -> Result<StagedPack, String> {
    let want = PackRef {
        path: None,
        digest: digest.trim().to_owned(),
    };
    let hex = want
        .hex()
        .ok_or_else(|| format!("pack digest {digest:?} is not sha256:<64 hex>"))?
        .to_ascii_lowercase();
    let bytes = pack_tar.bytes().map_err(|e| e.to_string())?;
    verify_artifact(&bytes, &hex).map_err(|e| format!("pack does not verify: {e}"))?;
    let dir = root.join(&hex);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| format!("replace {}: {e}", dir.display()))?;
    }
    let files = unpack_tar(&bytes, &dir)?;
    if files == 0 {
        return Err("pack unpacked no regular file".into());
    }
    Ok(StagedPack {
        digest: format!("sha256:{hex}"),
        dir,
        bytes: bytes.len() as u64,
    })
}

/// The staged pack, iff it is the one the topic pins.
pub fn pack_for(staged: Option<&StagedPack>, want: &PackRef) -> Result<StagedPack, String> {
    let Some(pack) = staged else {
        return Err(format!(
            "no experiment pack staged in this vm; the topic pins {} (the host stages it at boot on an experiment vm)",
            want.digest
        ));
    };
    if !pack.digest.eq_ignore_ascii_case(want.digest.trim()) {
        return Err(format!(
            "staged pack is {}, the topic pins {}",
            pack.digest, want.digest
        ));
    }
    Ok(pack.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use proof_vm_proto::tar::fixtures::{archive, member};
    use sha2::{Digest, Sha256};

    fn root(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "proof-vm-guest-staging-{}-{tag}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("dir");
        d
    }

    #[test]
    fn secrets_land_in_a_private_dir_under_plain_names_only() {
        use std::os::unix::fs::PermissionsExt;
        let d = root("secrets").join("secrets");
        let files = vec![
            StagedFile::new("inference_key", b"owner-key-not-a-real-secret\n"),
            StagedFile::new(" other_key ", b"short"),
        ];
        assert_eq!(stage_secrets(&d, &files, None).expect("staged"), 2);
        let mode = std::fs::metadata(&d).expect("dir").permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
        let mode = std::fs::metadata(d.join("inference_key"))
            .expect("file")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(
            std::fs::read(d.join("other_key")).expect("trimmed name"),
            b"short"
        );
        assert_eq!(secret_names(&d), vec!["inference_key", "other_key"]);
        let values = secret_values(&d);
        assert_eq!(
            values,
            vec![b"owner-key-not-a-real-secret".to_vec()],
            "short values are not redaction material"
        );
        for bad in ["../escape", "a/b", "", ".", "..", "nul\0byte"] {
            let err = stage_secrets(&d, &[StagedFile::new(bad, b"x")], None).expect_err(bad);
            assert!(err.contains("plain file name"), "{bad:?}: {err}");
        }
        let _ = std::fs::remove_dir_all(d.parent().expect("parent"));
    }

    #[test]
    fn packs_verify_then_unpack_plain_trees_and_bind_to_the_topic_pin() {
        let r = root("packs");
        let tar = archive(&[
            member("pack/", b'5', b""),
            member("pack/task.toml", b'0', b"[task]\nname = \"x\"\n"),
            member("pack/link", b'2', b""),
        ]);
        let hex = hex::encode(Sha256::digest(&tar));
        let digest = format!("sha256:{hex}");
        let staged = stage_pack(&r, &digest, &StagedFile::new("pack.tar", &tar)).expect("staged");
        assert_eq!(staged.digest, digest);
        assert_eq!(staged.dir, r.join(&hex));
        assert_eq!(staged.bytes, tar.len() as u64);
        assert_eq!(
            std::fs::read_to_string(staged.dir.join("pack/task.toml")).expect("file"),
            "[task]\nname = \"x\"\n"
        );
        assert!(!staged.dir.join("pack/link").exists(), "links are skipped");
        // Restaging replaces the tree.
        stage_pack(&r, &digest, &StagedFile::new("pack.tar", &tar)).expect("restaged");
        let wrong = format!("sha256:{}", "00".repeat(32));
        let err = stage_pack(&r, &wrong, &StagedFile::new("pack.tar", &tar)).expect_err("mismatch");
        assert!(err.contains("hashes to"), "{err}");
        assert!(stage_pack(&r, "latest", &StagedFile::new("pack.tar", &tar)).is_err());
        let escape = archive(&[member("../escape.txt", b'0', b"x")]);
        let escape_hex = hex::encode(Sha256::digest(&escape));
        let err = stage_pack(
            &r,
            &format!("sha256:{escape_hex}"),
            &StagedFile::new("pack.tar", &escape),
        )
        .expect_err("escape");
        assert!(err.contains("not a plain relative path"), "{err}");
        assert!(!r.join("escape.txt").exists());
        let want = PackRef {
            path: None,
            digest: digest.clone(),
        };
        assert_eq!(pack_for(Some(&staged), &want).expect("same pin"), staged);
        let other = PackRef {
            path: None,
            digest: wrong,
        };
        assert!(pack_for(Some(&staged), &other)
            .expect_err("other pin")
            .contains("the topic pins"));
        assert!(pack_for(None, &want)
            .expect_err("none")
            .contains("no experiment pack staged"));
        let _ = std::fs::remove_dir_all(&r);
    }
}
