use rustix::fs::{open, Mode, OFlags};
use std::{
    fs::{self, File},
    io::Read,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::Path,
};

pub fn private_file(path: &Path, maximum: u64) -> Result<String, &'static str> {
    String::from_utf8(private_bytes(path, maximum)?).map_err(|_| "invalid private Atlas file")
}

pub fn private_bytes(path: &Path, maximum: u64) -> Result<Vec<u8>, &'static str> {
    if !path.is_absolute() || fs::canonicalize(path).ok().as_deref() != Some(path) {
        return Err("unsafe private Atlas file");
    }
    let uid = rustix::process::geteuid().as_raw();
    let parent = path.parent().ok_or("unsafe private Atlas directory")?;
    for (depth, directory) in parent.ancestors().enumerate() {
        let stat = fs::symlink_metadata(directory).map_err(|_| "private directory unavailable")?;
        let mode = stat.permissions().mode();
        let private_parent = depth == 0 && (stat.uid() != uid || mode & 0o077 != 0);
        let writable_ancestor = mode & 0o022 != 0 && !(stat.uid() == 0 && mode & 0o1000 != 0);
        if !stat.is_dir()
            || (stat.uid() != 0 && stat.uid() != uid)
            || private_parent
            || writable_ancestor
        {
            return Err("unsafe private Atlas directory");
        }
    }
    // Validate the opened object; never reopen a key after checking its pathname.
    let file = File::from(
        open(
            path,
            OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
            Mode::empty(),
        )
        .map_err(|_| "private Atlas file unavailable")?,
    );
    let stat = file
        .metadata()
        .map_err(|_| "private Atlas file unavailable")?;
    if !stat.is_file()
        || stat.permissions().mode() & 0o077 != 0
        || stat.uid() != uid
        || stat.nlink() != 1
        || stat.len() > maximum
    {
        return Err("unsafe private Atlas file");
    }
    let mut bytes = Vec::new();
    file.take(maximum.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| "invalid private Atlas file")?;
    if bytes.is_empty() || bytes.len() as u64 > maximum {
        return Err("invalid private Atlas file");
    }
    Ok(bytes)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn private_tempdir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
        dir
    }

    #[test]
    fn private_files_reject_links_public_permissions_and_oversize() {
        let dir = private_tempdir();
        let file = dir.path().join("config");
        fs::write(&file, "secret-free-fixture").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(private_file(&file, 100).is_ok());
        assert!(private_file(&file, 2).is_err());
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&file, &link).unwrap();
        assert!(private_file(&link, 100).is_err());
        fs::hard_link(&file, dir.path().join("hard-link")).unwrap();
        assert!(private_file(&file, 100).is_err());
        fs::remove_file(dir.path().join("hard-link")).unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(private_file(&file, 100).is_err());
    }

    #[test]
    fn rejects_public_parents_and_writable_ancestors() {
        let root = private_tempdir();
        let parent = root.path().join("private");
        fs::create_dir(&parent).unwrap();
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o700)).unwrap();
        let file = parent.join("config");
        fs::write(&file, "fixture").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(private_file(&file, 100).is_ok());
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(private_file(&file, 100).is_err());
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o777)).unwrap();
        assert!(private_file(&file, 100).is_err());
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    }

    #[test]
    fn accepts_binary_keys_without_utf8_or_a_second_read() {
        let dir = private_tempdir();
        let file = dir.path().join("signer");
        fs::write(&file, [0xff; 32]).unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
        let bytes = private_bytes(&file, 256).unwrap();
        assert_eq!(
            challenge_keys::parse_challenge_secret(&bytes).unwrap(),
            [0xff; 32]
        );
        assert!(private_file(&file, 256).is_err());
    }

    #[test]
    fn rejects_special_files_without_blocking() {
        let dir = private_tempdir();
        let fifo = dir.path().join("fifo");
        rustix::fs::mknodat(
            rustix::fs::CWD,
            &fifo,
            rustix::fs::FileType::Fifo,
            Mode::RUSR | Mode::WUSR,
            0,
        )
        .unwrap();
        assert!(private_bytes(&fifo, 256).is_err());
        assert!(private_bytes(dir.path(), 256).is_err());
    }
}
