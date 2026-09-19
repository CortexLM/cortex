"""Export a measured OCI guest image to an ext4 rootfs without mounting it."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import posixpath
import re
import stat
import subprocess
import tarfile
import tempfile
import uuid
from pathlib import Path, PurePosixPath

DEFAULT_SOURCE_DATE_EPOCH = 946684800  # 2000-01-01T00:00:00Z
IMAGE_REFERENCE = re.compile(r"(?:[a-z0-9][a-z0-9._:/-]*@)?sha256:([0-9a-f]{64})")


def rootfs_filter(member: tarfile.TarInfo, destination: str) -> tarfile.TarInfo | None:
    """Reject host escapes while preserving the OCI filesystem metadata."""
    path = PurePosixPath(member.name)
    if path.is_absolute() or ".." in path.parts or "\0" in member.name:
        raise ValueError("unsafe rootfs member path")
    root = Path(destination).resolve()
    target_path = (root / Path(*path.parts)).resolve(strict=False)
    if target_path != root and not target_path.is_relative_to(root):
        raise ValueError("rootfs member resolves outside guest root")
    if member.isdev() or member.isfifo():
        # /dev is supplied by devtmpfs at guest boot, never by an image export.
        if path.parts and path.parts[0] == "dev":
            return None
        raise ValueError("rootfs contains a special file outside /dev")
    if not (member.isfile() or member.isdir() or member.issym() or member.islnk()):
        raise ValueError("rootfs contains an unsupported archive member")
    if member.issym():
        link = member.linkname
        target = posixpath.normpath(
            link.lstrip("/") if link.startswith("/") else posixpath.join(str(path.parent), link)
        )
        if target == ".." or target.startswith("../"):
            raise ValueError("rootfs symlink escapes guest root")
        member = member.replace(linkname=posixpath.relpath(target, str(path.parent)))
    elif member.islnk():
        hardlink = PurePosixPath(member.linkname)
        if hardlink.is_absolute() or ".." in hardlink.parts or "\0" in member.linkname:
            raise ValueError("rootfs hardlink escapes guest root")
        link_target = (root / Path(*hardlink.parts)).resolve(strict=False)
        if link_target != root and not link_target.is_relative_to(root):
            raise ValueError("rootfs hardlink resolves outside guest root")
    return member


def verify_extracted_metadata(archive: Path, destination: Path) -> int:
    """Prove that host extraction retained ownership and mode before mke2fs."""
    with tarfile.open(archive, "r:") as source:
        for original in source:
            member = rootfs_filter(original, str(destination))
            if member is None:
                continue
            target = destination / Path(*PurePosixPath(member.name).parts)
            try:
                actual = target.lstat()
            except OSError:
                raise ValueError("rootfs member missing after extraction") from None
            if member.uid is not None and actual.st_uid != member.uid:
                raise ValueError("rootfs extraction did not preserve numeric ownership")
            if member.gid is not None and actual.st_gid != member.gid:
                raise ValueError("rootfs extraction did not preserve numeric ownership")
            if not member.issym() and stat.S_IMODE(actual.st_mode) != member.mode:
                raise ValueError("rootfs extraction did not preserve file mode")
    return len(
        {
            (path.lstat().st_dev, path.lstat().st_ino)
            for path in (destination, *destination.rglob("*"))
        }
    )


def extract_rootfs(archive: Path, destination: Path, *, max_bytes: int) -> None:
    total = 0
    with tarfile.open(archive, "r:") as source:
        for member in source:
            total += member.size
            if total > max_bytes:
                raise ValueError("rootfs export exceeds selected disk capacity")
            source.extract(member, destination, filter=rootfs_filter, numeric_owner=True)
    if not (destination / "opt/cortex/bin/python").exists():
        raise ValueError("image contains no Cortex guest Python runtime")
    init = destination / "sbin/init"
    if not init.exists() or not os.access(init, os.X_OK):
        raise ValueError("image contains no executable guest init")


def filesystem_uuid(image: str) -> str:
    match = IMAGE_REFERENCE.fullmatch(image)
    if match is None:
        raise ValueError("guest image must be an actual OCI digest or local content-addressed ID")
    return str(uuid.UUID(bytes=bytes.fromhex(match.group(1))[:16], version=5))


def normalize_timestamps(root: Path, source_date_epoch: int) -> None:
    """Remove extraction-time metadata before mke2fs walks the exported tree."""
    timestamp = source_date_epoch * 1_000_000_000
    paths = sorted(root.rglob("*"), key=lambda path: (len(path.parts), str(path)), reverse=True)
    for path in paths:
        os.utime(path, ns=(timestamp, timestamp), follow_symlinks=False)
    os.utime(root, ns=(timestamp, timestamp), follow_symlinks=False)


def mkfs_command(root: Path, result: Path, image: str) -> list[str]:
    identity = filesystem_uuid(image)
    return [
        "mkfs.ext4",
        "-q",
        "-F",
        "-m",
        "0",
        "-L",
        "cortex-proof",
        "-U",
        identity,
        "-E",
        f"lazy_itable_init=0,lazy_journal_init=0,hash_seed={identity},root_owner=0:0",
        "-O",
        "^orphan_file",
        "-d",
        str(root),
        str(result),
    ]


def _filesystem_counts(image: Path, environment: dict[str, str]) -> tuple[int, int]:
    output = subprocess.check_output(
        ["tune2fs", "-l", str(image)],
        text=True,
        env=environment,
        stderr=subprocess.DEVNULL,
    )

    def field(name: str) -> int:
        match = re.search(rf"(?m)^{re.escape(name)}:\s+(\d+)\s*$", output)
        if match is None:
            raise ValueError("could not inspect ext4 inode allocation")
        return int(match.group(1))

    return field("Inode count"), field("Free inodes")


def normalize_inode_ctimes(
    image: Path,
    source_date_epoch: int,
    *,
    expected_source_inodes: int,
    environment: dict[str, str],
) -> None:
    """Normalize ctime and atime after mke2fs.

    ctime is copied from the host and os.utime cannot set it. atime is set by
    os.utime, but a relatime mount refreshes it again when mke2fs reads the
    file, so the copied value depends on the build host's mount options.
    """
    inode_count, free_inodes = _filesystem_counts(image, environment)
    used = inode_count - free_inodes
    # A fresh mke2fs image allocates the ten reserved/lost+found inodes and then
    # the source tree as one contiguous prefix. Refuse a shape that violates it.
    if used != expected_source_inodes + 10 or not 11 <= used < inode_count:
        raise ValueError("unexpected ext4 inode allocation")
    for inode, expected in ((used, "marked in use"), (used + 1, "not in use")):
        probe = subprocess.check_output(
            ["debugfs", "-R", f"testi <{inode}>", str(image)],
            text=True,
            env=environment,
            stderr=subprocess.DEVNULL,
        )
        if expected not in probe:
            raise ValueError("ext4 inode allocation is not a contiguous fresh image")
    with tempfile.NamedTemporaryFile("w", dir=image.parent, prefix="ctime-", delete=False) as file:
        commands = Path(file.name)
        for inode in range(2, used + 1):
            file.write(f"set_inode_field <{inode}> ctime @{source_date_epoch}\n")
            file.write(f"set_inode_field <{inode}> atime @{source_date_epoch}\n")
    try:
        subprocess.run(
            ["debugfs", "-w", "-f", str(commands), str(image)],
            check=True,
            env=environment,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
    finally:
        commands.unlink(missing_ok=True)


def build_rootfs(
    image: str,
    output: Path,
    *,
    size_mib: int = 4096,
    engine: str = "docker",
    source_date_epoch: int = DEFAULT_SOURCE_DATE_EPOCH,
) -> dict:
    identity = filesystem_uuid(image)
    if type(source_date_epoch) is not int or not 1 <= source_date_epoch <= 2_147_483_647:
        raise ValueError("source date epoch must be an integer in the portable ext4 range")
    if not 1024 <= size_mib <= 32768:
        raise ValueError("rootfs size must be between 1024 and 32768 MiB")
    if engine not in {"docker", "podman"}:
        raise ValueError("unsupported OCI engine")
    output = output.absolute()
    if output.exists() or output.is_symlink():
        raise ValueError("refusing to overwrite an existing rootfs")
    output.parent.mkdir(parents=True, exist_ok=True)
    container = ""
    try:
        with tempfile.TemporaryDirectory(prefix="proof-rootfs-", dir=output.parent) as temporary:
            scratch = Path(temporary)
            container = subprocess.check_output(
                [engine, "create", "--network", "none", "--entrypoint", "/bin/true", image],
                text=True,
            ).strip()
            if not re.fullmatch(r"[0-9a-f]{12,64}", container):
                raise ValueError("OCI engine returned an invalid container ID")
            archive, root = scratch / "image.tar", scratch / "root"
            root.mkdir()
            subprocess.run([engine, "export", "--output", str(archive), container], check=True)
            extract_rootfs(archive, root, max_bytes=(size_mib - 128) * 1024 * 1024)
            source_inodes = verify_extracted_metadata(archive, root)
            normalize_timestamps(root, source_date_epoch)
            result = scratch / "rootfs.ext4"
            with result.open("xb") as stream:
                stream.truncate(size_mib * 1024 * 1024)
                stream.flush()
                os.fsync(stream.fileno())
            environment = os.environ.copy()
            environment.update(
                {
                    "E2FSPROGS_FAKE_TIME": str(source_date_epoch),
                    "LC_ALL": "C",
                    "TZ": "UTC",
                }
            )
            subprocess.run(mkfs_command(root, result, image), check=True, env=environment)
            normalize_inode_ctimes(
                result,
                source_date_epoch,
                expected_source_inodes=source_inodes,
                environment=environment,
            )
            subprocess.run(
                ["e2fsck", "-fn", str(result)],
                check=True,
                env=environment,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
            with result.open("rb") as stream:
                measured = hashlib.file_digest(stream, "sha256").hexdigest()
            os.link(result, output)  # atomic publication, never replace a competing file
            return {
                "rootfs": str(output),
                "sha256": measured,
                "source_image": image,
                "source_date_epoch": source_date_epoch,
                "filesystem_uuid": identity,
                "size_mib": size_mib,
            }
    finally:
        if container and re.fullmatch(r"[0-9a-f]{12,64}", container):
            subprocess.run(
                [engine, "rm", container],
                check=False,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )


def main(argv: list[str] | None = None) -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--image", required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--size-mib", type=int, default=4096)
    parser.add_argument("--engine", choices=["docker", "podman"], default="docker")
    parser.add_argument("--source-date-epoch", type=int, default=DEFAULT_SOURCE_DATE_EPOCH)
    args = parser.parse_args(argv)
    if os.geteuid() != 0:
        parser.error("run as root or within a rootless podman unshare user namespace")
    try:
        print(
            json.dumps(
                build_rootfs(
                    args.image,
                    args.output,
                    size_mib=args.size_mib,
                    engine=args.engine,
                    source_date_epoch=args.source_date_epoch,
                )
            )
        )
    except (ValueError, OSError, subprocess.SubprocessError, tarfile.TarError) as error:
        raise SystemExit(f"guest rootfs build failed: {type(error).__name__}") from None


if __name__ == "__main__":
    main()
