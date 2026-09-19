"""Fail-closed filesystem preparation for durable SQLite state."""

from __future__ import annotations

import os
import stat
from pathlib import Path


class StateSecurityError(RuntimeError):
    """The configured state path does not satisfy the local trust boundary."""


def _directory_fd(path: Path) -> int:
    flags = os.O_RDONLY | os.O_CLOEXEC | os.O_DIRECTORY | os.O_NOFOLLOW
    try:
        return os.open(path, flags)
    except OSError as error:
        raise StateSecurityError(f"{path}: expected a real directory") from error


def _create_parent(path: Path) -> None:
    try:
        path.mkdir(parents=True, mode=0o700, exist_ok=True)
    except OSError as error:
        raise StateSecurityError(f"{path}: cannot create SQLite parent directory") from error


def secure_state_directory(path: str | Path) -> Path:
    """Create a private state directory or validate an existing one exactly."""

    directory = Path(path)
    created = False
    try:
        os.mkdir(directory, 0o700)
        created = True
    except FileNotFoundError:
        _create_parent(directory.parent)
        try:
            os.mkdir(directory, 0o700)
            created = True
        except FileExistsError:
            pass
        except OSError as error:
            raise StateSecurityError(f"{directory}: cannot create state directory") from error
    except FileExistsError:
        pass
    except OSError as error:
        raise StateSecurityError(f"{directory}: cannot create state directory") from error

    descriptor = _directory_fd(directory)
    try:
        if created:
            os.fchmod(descriptor, 0o700)
        metadata = os.fstat(descriptor)
        if not stat.S_ISDIR(metadata.st_mode):
            raise StateSecurityError(f"{directory}: state path is not a directory")
        if metadata.st_uid != os.geteuid():
            raise StateSecurityError(f"{directory}: state directory has the wrong owner")
        if stat.S_IMODE(metadata.st_mode) != 0o700:
            raise StateSecurityError(f"{directory}: state directory mode must be 0700")
    finally:
        os.close(descriptor)
    return directory


def _validate_sqlite_parent(path: Path) -> None:
    parent = path.parent
    if not parent.exists():
        _create_parent(parent)
    descriptor = _directory_fd(parent)
    try:
        metadata = os.fstat(descriptor)
        if metadata.st_mode & 0o022:
            raise StateSecurityError(
                f"{parent}: SQLite parent must not be writable by group or others"
            )
    finally:
        os.close(descriptor)


def secure_sqlite_path(path: str | Path) -> str | Path:
    """Pre-open and validate a SQLite file without following special files."""

    if str(path) == ":memory:":
        return ":memory:"

    database = Path(path)
    _validate_sqlite_parent(database)
    flags = os.O_RDWR | os.O_CLOEXEC | os.O_NONBLOCK | os.O_NOFOLLOW
    created = False
    try:
        descriptor = os.open(database, flags | os.O_CREAT | os.O_EXCL, 0o600)
        created = True
    except FileExistsError:
        try:
            descriptor = os.open(database, flags | os.O_CREAT, 0o600)
        except OSError as error:
            raise StateSecurityError(f"{database}: unsafe SQLite state path") from error
    except OSError as error:
        raise StateSecurityError(f"{database}: unsafe SQLite state path") from error

    try:
        if created:
            os.fchmod(descriptor, 0o600)
        metadata = os.fstat(descriptor)
        if not stat.S_ISREG(metadata.st_mode):
            raise StateSecurityError(f"{database}: SQLite state must be a regular file")
        if metadata.st_uid != os.geteuid():
            raise StateSecurityError(f"{database}: SQLite state has the wrong owner")
        if stat.S_IMODE(metadata.st_mode) != 0o600:
            raise StateSecurityError(f"{database}: SQLite state mode must be 0600")
        if metadata.st_nlink != 1:
            raise StateSecurityError(f"{database}: SQLite state must have exactly one link")
    finally:
        os.close(descriptor)
    return database


def prepare_master_state(path: str | Path) -> Path:
    """Prepare every durable database before a master service opens SQLite."""

    directory = secure_state_directory(path)
    for name in ("gateway.sqlite3", "bounty.sqlite3", "proof.sqlite3", "emission.sqlite3"):
        secure_sqlite_path(directory / name)
    return directory
