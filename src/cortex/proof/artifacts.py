"""Content-addressed artifacts and a file-only miner credential vault."""

from __future__ import annotations

import hashlib
import io
import os
import re
import stat
import tarfile
from collections.abc import Iterable
from pathlib import Path, PurePosixPath

from cortex.errors import ServiceError

MAX_ARTIFACT_BYTES = 5 * 1024 * 1024
HEX = re.compile(r"^[0-9a-f]{64}$")
ENV = re.compile(r"^[A-Z][A-Z0-9_]{0,63}$")
RESERVED = {
    "PATH",
    "HOME",
    "LANG",
    "XDG_RUNTIME_DIR",
    "LD_PRELOAD",
    "LD_LIBRARY_PATH",
    "PYTHONPATH",
    "PYTHONHOME",
    "BASH_ENV",
    "ENV",
}

_CLOEXEC = getattr(os, "O_CLOEXEC", 0)
_DIRECTORY_FLAGS = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | _CLOEXEC
_FILE_FLAGS = os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | _CLOEXEC


def valid_env_name(name: object) -> bool:
    return (
        isinstance(name, str)
        and ENV.fullmatch(name) is not None
        and not name.startswith("PROOF_")
        and name not in RESERVED
    )


def check_env(params: dict[str, str], env: dict[str, str]) -> dict[str, str]:
    required = params.get("miner_byok")
    allowed = set(filter(None, params.get("miner_env_allowlist", "").split(",")))
    if required:
        allowed.add(required)
    if len(env) > 8:
        raise ServiceError(400, "too many env variables")
    for name in allowed | env.keys():
        if not valid_env_name(name):
            raise ServiceError(400, "invalid env name")
    for name, value in env.items():
        if name not in allowed:
            raise ServiceError(400, f"undeclared env name: {name}")
        if not value.strip() or len(value) > 4096 or not value.isprintable():
            raise ServiceError(400, f"invalid env value: {name}")
    if required and required not in env:
        raise ServiceError(400, f"required env missing: {required}")
    return env.copy()


def verify_artifact(data: bytes, expected: str, *, limit: int = MAX_ARTIFACT_BYTES) -> None:
    if not data or len(data) > limit:
        raise ServiceError(400, "artifact size invalid")
    if not HEX.fullmatch(expected) or hashlib.sha256(data).hexdigest() != expected:
        raise ServiceError(400, "artifact digest mismatch")
    try:
        with tarfile.open(fileobj=io.BytesIO(data), mode="r:") as archive:
            seen: set[str] = set()
            has_content = False
            total = 0
            for count, member in enumerate(archive, 1):
                path = PurePosixPath(member.name)
                if (
                    count > 10000
                    or path.is_absolute()
                    or ".." in path.parts
                    or not path.parts
                    or "\\" in member.name
                    or member.name in seen
                    or not (member.isfile() or member.isdir())
                ):
                    raise ServiceError(400, "unsafe artifact member")
                seen.add(member.name)
                total += member.size
                if member.size < 0 or total > limit or member.sparse is not None:
                    raise ServiceError(400, "artifact expanded size invalid")
                if member.isfile() and member.size:
                    stream = archive.extractfile(member)
                    if stream is None or len(stream.read()) != member.size:
                        raise ServiceError(400, "truncated artifact")
                    has_content = True
            if not has_content:
                raise ServiceError(400, "artifact has no content")
    except (tarfile.TarError, OSError, ValueError):
        raise ServiceError(400, "artifact must be an uncompressed tar") from None


class FileVault:
    """Private submission directories; values never enter database records."""

    def __init__(self, root: Path):
        self.root = root
        try:
            root.mkdir(parents=True, mode=0o700, exist_ok=True)
            descriptor = os.open(root, _DIRECTORY_FLAGS)
        except OSError:
            raise ServiceError(503, "vault directory must be private") from None
        try:
            self._validate_directory(os.fstat(descriptor))
        finally:
            os.close(descriptor)

    @staticmethod
    def _validate_directory(metadata: os.stat_result) -> None:
        if (
            not stat.S_ISDIR(metadata.st_mode)
            or metadata.st_uid != os.geteuid()
            or stat.S_IMODE(metadata.st_mode) != 0o700
        ):
            raise ServiceError(503, "vault directory must be private")

    @staticmethod
    def _validate_key(key: str, *, stored: bool = False) -> None:
        if not isinstance(key, str) or HEX.fullmatch(key) is None:
            message = "invalid stored vault key" if stored else "invalid vault key"
            raise ServiceError(503 if stored else 400, message)

    @staticmethod
    def _validate_names(names: Iterable[object]) -> list[str]:
        result: list[str] = []
        for name in names:
            if not isinstance(name, str) or not valid_env_name(name):
                raise ServiceError(503, "invalid stored env name")
            result.append(name)
        if len(result) != len(set(result)):
            raise ServiceError(503, "invalid stored env name")
        return sorted(result)

    def _open_root(self) -> int:
        descriptor = -1
        try:
            descriptor = os.open(self.root, _DIRECTORY_FLAGS)
            self._validate_directory(os.fstat(descriptor))
            return descriptor
        except (OSError, ServiceError):
            if descriptor >= 0:
                os.close(descriptor)
            raise ServiceError(503, "vault directory must be private") from None

    def _open_directory(self, root: int, key: str, *, missing_ok: bool = False) -> int | None:
        try:
            descriptor = os.open(key, _DIRECTORY_FLAGS, dir_fd=root)
        except FileNotFoundError:
            if missing_ok:
                return None
            raise ServiceError(503, "miner credential directory missing") from None
        except OSError:
            raise ServiceError(503, "miner credential vault contains unsafe entry") from None
        try:
            self._validate_directory(os.fstat(descriptor))
        except ServiceError:
            os.close(descriptor)
            raise
        return descriptor

    @staticmethod
    def _validate_file(metadata: os.stat_result) -> None:
        if not stat.S_ISREG(metadata.st_mode):
            raise ServiceError(503, "miner credential must be a regular file")
        if metadata.st_uid != os.geteuid():
            raise ServiceError(503, "miner credential has the wrong owner")
        if stat.S_IMODE(metadata.st_mode) != 0o600:
            raise ServiceError(503, "miner credential is not private")
        if metadata.st_nlink != 1:
            raise ServiceError(503, "miner credential must have exactly one link")

    @classmethod
    def _file_descriptor(cls, directory: int, name: str) -> tuple[int, os.stat_result]:
        if not valid_env_name(name):
            raise ServiceError(503, "invalid stored env name")
        descriptor = -1
        try:
            descriptor = os.open(name, _FILE_FLAGS, dir_fd=directory)
            metadata = os.fstat(descriptor)
            path_metadata = os.stat(name, dir_fd=directory, follow_symlinks=False)
            cls._validate_file(metadata)
        except OSError:
            if descriptor >= 0:
                os.close(descriptor)
            raise ServiceError(503, "miner credential vault contains unsafe entry") from None
        except ServiceError:
            if descriptor >= 0:
                os.close(descriptor)
            raise
        if (metadata.st_dev, metadata.st_ino) != (path_metadata.st_dev, path_metadata.st_ino):
            os.close(descriptor)
            raise ServiceError(503, "miner credential changed during validation")
        return descriptor, metadata

    @classmethod
    def _scan_directory(cls, descriptor: int) -> dict[str, tuple[int, int]]:
        try:
            with os.scandir(descriptor) as entries:
                names = sorted(entry.name for entry in entries)
        except OSError:
            raise ServiceError(503, "miner credential vault unavailable") from None
        result: dict[str, tuple[int, int]] = {}
        for name in names:
            file_descriptor, metadata = cls._file_descriptor(descriptor, name)
            os.close(file_descriptor)
            result[name] = (metadata.st_dev, metadata.st_ino)
        return result

    @classmethod
    def _read_credential(cls, directory: int, name: str) -> str:
        descriptor, _ = cls._file_descriptor(directory, name)
        try:
            with os.fdopen(descriptor, encoding="utf-8") as stream:
                value = stream.read(4097)
        except (OSError, UnicodeError):
            raise ServiceError(503, "miner credential unavailable") from None
        if not value.strip() or len(value) > 4096 or not value.isprintable():
            raise ServiceError(503, "miner credential unavailable")
        return value

    def put(self, key: str, env: dict[str, str]) -> None:
        self._validate_key(key)
        for name, value in env.items():
            if not valid_env_name(name):
                raise ServiceError(400, "invalid env name")
            if not isinstance(value, str) or not value.strip() or len(value) > 4096:
                raise ServiceError(400, f"invalid env value: {name}")
            if not value.isprintable():
                raise ServiceError(400, f"invalid env value: {name}")
        if not env:
            return
        try:
            root = self._open_root()
            try:
                try:
                    os.mkdir(key, 0o700, dir_fd=root)
                    os.fsync(root)
                except FileExistsError:
                    pass
                directory = self._open_directory(root, key)
                if directory is None:  # pragma: no cover - missing_ok is false
                    raise ServiceError(503, "miner credential vault unavailable")
                try:
                    for name, value in sorted(env.items()):
                        descriptor = os.open(
                            name,
                            os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | _CLOEXEC,
                            0o600,
                            dir_fd=directory,
                        )
                        try:
                            os.fchmod(descriptor, 0o600)
                            payload = value.encode()
                            offset = 0
                            while offset < len(payload):
                                written = os.write(descriptor, payload[offset:])
                                if written <= 0:
                                    raise OSError("credential write made no progress")
                                offset += written
                            os.fsync(descriptor)
                            self._validate_file(os.fstat(descriptor))
                        finally:
                            os.close(descriptor)
                    os.fsync(directory)
                finally:
                    os.close(directory)
            finally:
                os.close(root)
        except (OSError, ServiceError) as error:
            try:
                self.delete(key)
            except ServiceError:
                raise ServiceError(503, "miner credential vault cleanup failed") from error
            if isinstance(error, ServiceError):
                raise
            raise ServiceError(503, "miner credential vault unavailable") from None

    def get(self, key: str, names: list[str]) -> dict[str, str]:
        self._validate_key(key, stored=True)
        expected = self._validate_names(names)
        if not expected:
            return {}
        root = self._open_root()
        try:
            directory = self._open_directory(root, key)
            if directory is None:  # pragma: no cover - missing_ok is false
                raise ServiceError(503, "miner credential directory missing")
            try:
                if sorted(self._scan_directory(directory)) != expected:
                    raise ServiceError(503, "miner credential set mismatch")
                return {name: self._read_credential(directory, name) for name in expected}
            finally:
                os.close(directory)
        finally:
            os.close(root)

    def delete(self, key: str) -> None:
        """Remove terminal-job credentials without following links or directories."""
        self._validate_key(key, stored=True)
        root = self._open_root()
        try:
            directory = self._open_directory(root, key, missing_ok=True)
            if directory is None:
                return
            try:
                entries = self._scan_directory(directory)
                for name, identity in entries.items():
                    current, metadata = self._file_descriptor(directory, name)
                    os.close(current)
                    if (metadata.st_dev, metadata.st_ino) != identity:
                        raise ServiceError(503, "miner credential changed during cleanup")
                for name in entries:
                    os.unlink(name, dir_fd=directory)
                os.fsync(directory)
            finally:
                os.close(directory)
            os.rmdir(key, dir_fd=root)
            os.fsync(root)
        except ServiceError:
            raise
        except OSError:
            raise ServiceError(503, "miner credential vault cleanup failed") from None
        finally:
            os.close(root)

    def reconcile(self, expected: dict[str, list[str]]) -> None:
        """Validate active credentials, then remove fully validated terminal orphans."""
        normalized: dict[str, list[str]] = {}
        for key, names in expected.items():
            self._validate_key(key, stored=True)
            normalized[key] = self._validate_names(names)

        root = self._open_root()
        actual: dict[str, list[str]] = {}
        try:
            try:
                with os.scandir(root) as iterator:
                    entries = [
                        (entry.name, entry.is_dir(follow_symlinks=False)) for entry in iterator
                    ]
            except OSError:
                raise ServiceError(503, "miner credential vault unavailable") from None
            for name, is_directory in entries:
                if HEX.fullmatch(name) is None or not is_directory:
                    raise ServiceError(503, "miner credential vault contains unsafe entry")
                directory = self._open_directory(root, name)
                if directory is None:  # pragma: no cover - missing_ok is false
                    raise ServiceError(503, "miner credential directory missing")
                try:
                    names = sorted(self._scan_directory(directory))
                    actual[name] = names
                    if name in normalized:
                        if names != normalized[name]:
                            raise ServiceError(503, "miner credential set mismatch")
                        for credential_name in names:
                            self._read_credential(directory, credential_name)
                finally:
                    os.close(directory)
            for key, names in normalized.items():
                if names and key not in actual:
                    raise ServiceError(503, "miner credential directory missing")
        finally:
            os.close(root)

        for key, names in actual.items():
            if key not in normalized or not names:
                self.delete(key)
