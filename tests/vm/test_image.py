import hashlib
import io
import os
import subprocess
import tarfile

import pytest

from cortex.vm.image import (
    build_rootfs,
    extract_rootfs,
    filesystem_uuid,
    mkfs_command,
    normalize_inode_ctimes,
    normalize_timestamps,
    rootfs_filter,
)


def image_tar(path, *, extra=()):
    with tarfile.open(path, "w") as archive:
        for name in ("opt/cortex/bin/python", "opt/cortex/bin/proof-guest-init"):
            entry = tarfile.TarInfo(name)
            entry.mode, entry.size = 0o755, 4
            archive.addfile(entry, io.BytesIO(b"test"))
        for name, target in (
            ("usr/sbin/init", "/opt/cortex/bin/proof-guest-init"),
            ("sbin", "usr/sbin"),
        ):
            entry = tarfile.TarInfo(name)
            entry.type, entry.linkname = tarfile.SYMTYPE, target
            archive.addfile(entry)
        for entry, data in extra:
            archive.addfile(entry, io.BytesIO(data) if data else None)


def test_guest_absolute_symlinks_resolve_inside_export_and_keep_executable_init(tmp_path):
    archive, root = tmp_path / "image.tar", tmp_path / "root"
    image_tar(archive)
    extract_rootfs(archive, root, max_bytes=1024)
    assert (root / "sbin/init").resolve() == root / "opt/cortex/bin/proof-guest-init"
    assert os.access(root / "sbin/init", os.X_OK)
    assert not os.path.isabs(os.readlink(root / "usr/sbin/init"))


@pytest.mark.parametrize(
    "attack", ["traversal", "symlink", "symlink_write", "hardlink", "device", "size"]
)
def test_rootfs_export_rejects_host_escape_and_unbounded_content(tmp_path, attack):
    archive, root = tmp_path / "image.tar", tmp_path / "root"
    member = tarfile.TarInfo("escape")
    extra = []
    if attack == "traversal":
        member.name, member.size = "../escape", 1
    elif attack in {"symlink", "symlink_write"}:
        member.type, member.linkname = tarfile.SYMTYPE, "../../escape"
    elif attack == "hardlink":
        member.type, member.linkname = tarfile.LNKTYPE, "../escape"
    elif attack == "device":
        member.type = tarfile.CHRTYPE
    else:
        member.size = 2048
    extra.append((member, b"x" * member.size))
    if attack == "symlink_write":
        child = tarfile.TarInfo("escape/child")
        child.size = 1
        extra.append((child, b"x"))
    image_tar(archive, extra=extra)
    with pytest.raises((ValueError, tarfile.FilterError)):
        extract_rootfs(archive, root, max_bytes=1024)
    assert not (tmp_path / "escape").exists()


def test_rootfs_security_filter_preserves_oci_owner_and_mode(tmp_path):
    member = tarfile.TarInfo("usr/bin/helper")
    member.mode, member.uid, member.gid = 0o4755, 42, 43

    filtered = rootfs_filter(member, str(tmp_path))

    assert filtered is not None
    assert (filtered.mode, filtered.uid, filtered.gid) == (0o4755, 42, 43)


def test_builder_requires_actual_digest_before_contacting_container_engine(tmp_path):
    with pytest.raises(ValueError, match="digest"):
        build_rootfs("guest:latest", tmp_path / "root.ext4")
    assert list(tmp_path.iterdir()) == []


def test_ext4_identity_and_mkfs_options_are_deterministic(tmp_path):
    image = "sha256:" + "12" * 32

    first = filesystem_uuid(image)
    second = filesystem_uuid("registry.example/cortex@" + image)
    command = mkfs_command(tmp_path / "root", tmp_path / "root.ext4", image)

    assert first == second == "12121212-1212-5212-9212-121212121212"
    assert command[command.index("-U") + 1] == first
    extended = command[command.index("-E") + 1]
    assert "lazy_itable_init=0" in extended
    assert "lazy_journal_init=0" in extended
    assert f"hash_seed={first}" in extended
    assert command[command.index("-O") + 1] == "^orphan_file"


def test_rootfs_metadata_uses_one_source_date_epoch(tmp_path):
    root = tmp_path / "root"
    child = root / "directory" / "file"
    child.parent.mkdir(parents=True)
    child.write_text("content")
    link = root / "link"
    link.symlink_to("directory/file")

    normalize_timestamps(root, 946684800)

    expected = 946684800 * 1_000_000_000
    assert root.stat().st_mtime_ns == expected
    assert child.parent.stat().st_mtime_ns == expected
    assert child.stat().st_mtime_ns == expected
    assert link.lstat().st_mtime_ns == expected


def test_inode_ctime_normalization_makes_ext4_bytes_reproducible(tmp_path):
    root = tmp_path / "root"
    runtime = root / "opt/cortex/bin/python"
    runtime.parent.mkdir(parents=True)
    runtime.write_bytes(b"python")
    runtime.chmod(0o755)
    init = root / "sbin/init"
    init.parent.mkdir()
    init.symlink_to("../opt/cortex/bin/python")
    normalize_timestamps(root, 946684800)
    source_inodes = len(
        {(path.lstat().st_dev, path.lstat().st_ino) for path in (root, *root.rglob("*"))}
    )
    environment = os.environ | {
        "E2FSPROGS_FAKE_TIME": "946684800",
        "LC_ALL": "C",
        "TZ": "UTC",
    }
    image_ref = "sha256:" + "34" * 32
    images = [tmp_path / "first.ext4", tmp_path / "second.ext4"]
    for image, ctime in zip(images, (111, 222), strict=True):
        with image.open("xb") as stream:
            stream.truncate(32 * 1024 * 1024)
        subprocess.run(mkfs_command(root, image, image_ref), check=True, env=environment)
        # Host ctime on the root, and a relatime host refreshing atime while
        # mke2fs reads the last source file. debugfs keeps only the last -R,
        # so both edits go through one command file.
        script = tmp_path / f"{image.stem}.debugfs"
        script.write_text(
            f"set_inode_field <2> ctime @{ctime}\n"
            f"set_inode_field <{10 + source_inodes}> atime @{ctime}\n"
        )
        subprocess.run(
            ["debugfs", "-w", "-f", str(script), str(image)],
            check=True,
            env=environment,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )

    def digest(path):
        with path.open("rb") as stream:
            return hashlib.file_digest(stream, "sha256").digest()

    assert digest(images[0]) != digest(images[1])

    for image in images:
        normalize_inode_ctimes(
            image,
            946684800,
            expected_source_inodes=source_inodes,
            environment=environment,
        )

    if digest(images[0]) != digest(images[1]):
        first, second = (image.read_bytes() for image in images)
        offsets = [index for index, (a, b) in enumerate(zip(first, second, strict=True)) if a != b]
        version = subprocess.run(
            ["mkfs.ext4", "-V"], capture_output=True, text=True, check=False
        ).stderr.strip()
        raise AssertionError(
            f"ext4 bytes differ after ctime normalization ({version}); "
            f"{len(offsets)} bytes at {offsets[:32]}"
        )
