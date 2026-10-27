"""The token mark: durable, replaced atomically, locked, and never silently reset or lowered."""

import errno
import fcntl
import os
import subprocess
import sys
from pathlib import Path

import pytest

from hypertrain.elastic.tokens import MAX_TOKEN, LocalTokenLog, MemoryTokenLog, TokenLogError

# The child dies by SIGKILL the moment save() asks for the new mark to be durable, as a host
# does when the provider pulls it in the middle of a join. `kind` is the file or the directory
# whose fsync kills it; the other goes through. At the file's fsync the mark must already be in
# the file (exit 3 otherwise: the writer did not flush its buffer before asking for durability).
KILLED_AT_FSYNC = """
import os, signal, stat, sys
from pathlib import Path
from hypertrain.elastic.tokens import LocalTokenLog
log = LocalTokenLog(Path(sys.argv[1]))
log.load()
sync = os.fsync
def fsync(fd):
    mode = os.fstat(fd).st_mode
    if stat.S_ISREG(mode) and sys.argv[2] == "file":
        if os.fstat(fd).st_size != len(b"2"):
            os._exit(3)
        os.kill(os.getpid(), signal.SIGKILL)
    if stat.S_ISDIR(mode) and sys.argv[2] == "directory":
        os.kill(os.getpid(), signal.SIGKILL)
    sync(fd)
os.fsync = fsync
log.save(2)
"""

# A coordinator starting on the log in another process: it must be refused at once, not queued.
LOAD = """
import sys
from pathlib import Path
from hypertrain.elastic.tokens import LocalTokenLog, TokenLogError
try:
    print(LocalTokenLog(Path(sys.argv[1])).load())
except TokenLogError:
    sys.exit(7)
"""


def test_a_local_log_keeps_the_mark_for_the_next_process(tmp_path: Path) -> None:
    path = tmp_path / "run-1" / "tokens"
    log = LocalTokenLog.create(path)
    assert log.load() == 0
    log.save(1)
    log.save(7)  # a token spent by a failed save leaves a gap: marks only go up
    assert LocalTokenLog.read(path) == 7
    assert path.stat().st_mode & 0o777 == 0o600
    assert sorted(p.name for p in path.parent.iterdir()) == [".tokens.lock", "tokens"]


def test_a_missing_log_is_an_error_not_a_new_run(tmp_path: Path) -> None:
    path = tmp_path / "tokens"
    with pytest.raises(TokenLogError):
        LocalTokenLog(path).load()  # a lost volume must not look like mark 0
    LocalTokenLog.create(path).close()
    with pytest.raises(TokenLogError):
        LocalTokenLog.create(path, mark=9)  # a new run never overwrites a log
    assert LocalTokenLog.read(path) == 0
    assert LocalTokenLog.create(tmp_path / "restored", mark=41).load() == 41
    for wild in (-1, MAX_TOKEN + 1):  # a mark the log could not load back
        with pytest.raises(TokenLogError):
            LocalTokenLog.create(tmp_path / "wild", mark=wild)


@pytest.mark.parametrize(
    "content", [b"", b"seven", b"01", b"7\n", str(MAX_TOKEN + 1).encode(), b"9" * 5000]
)
def test_a_log_that_does_not_parse_is_an_error_not_a_reset(tmp_path: Path, content: bytes) -> None:
    path = tmp_path / "tokens"
    path.write_bytes(content)
    with pytest.raises(TokenLogError):
        LocalTokenLog(path).load()


def test_a_log_refuses_to_save_a_token_that_does_not_follow_its_mark(tmp_path: Path) -> None:
    path = tmp_path / "tokens"
    log = LocalTokenLog.create(path)
    log.save(3)
    for bad in (3, 2, MAX_TOKEN + 1):  # a repeat, a lower token, one the wire cannot carry
        with pytest.raises(TokenLogError):
            log.save(bad)
    assert LocalTokenLog.read(path) == 3
    log.save(MAX_TOKEN)  # the last token the wire can carry is still granted
    with pytest.raises(TokenLogError):
        log.save(MAX_TOKEN + 1)  # and the next one is refused by the range, not by the lock
    log.close()
    assert LocalTokenLog(path).load() == MAX_TOKEN  # a log at that mark still loads
    memory = MemoryTokenLog()
    for bad in (0, MAX_TOKEN + 1):
        with pytest.raises(TokenLogError):
            memory.save(bad)


def test_a_second_coordinator_cannot_hold_a_log_that_is_held(tmp_path: Path) -> None:
    path = tmp_path / "tokens"
    first = LocalTokenLog.create(path)
    with pytest.raises(TokenLogError):
        LocalTokenLog(path).load()  # it would hand out the tokens `first` is handing out
    first.close()
    second = LocalTokenLog(path)
    assert second.load() == 0  # the first coordinator is gone: the lock went with it
    with pytest.raises(TokenLogError):
        first.save(1)  # a log that was closed no longer saves
    second.save(1)


def test_a_coordinator_in_another_process_is_refused_at_once_by_a_held_log(tmp_path: Path) -> None:
    path = tmp_path / "tokens"
    log = LocalTokenLog.create(path)
    log.save(4)

    def start() -> subprocess.CompletedProcess[bytes]:
        # a timeout is how a start that queues behind the holder fails here, instead of hanging
        return subprocess.run(
            [sys.executable, "-c", LOAD, str(path)], capture_output=True, timeout=30
        )

    assert start().returncode == 7
    log.close()
    started = start()
    assert (started.returncode, started.stdout) == (0, b"4\n")


def test_a_log_that_is_dropped_gives_up_its_lock(tmp_path: Path) -> None:
    path = tmp_path / "tokens"
    LocalTokenLog.create(path)  # nobody keeps it: a coordinator that is gone holds nothing
    assert LocalTokenLog(path).load() == 0


def test_a_forked_copy_of_a_coordinator_cannot_save(tmp_path: Path) -> None:
    path = tmp_path / "tokens"
    log = LocalTokenLog.create(path)
    pid = os.fork()
    if pid == 0:  # the child shares the descriptor, so it shares the lock and nothing refuses it
        code = 1
        try:
            log.save(1)
        except TokenLogError:
            code = 0
        finally:
            os._exit(code)
    _, status = os.waitpid(pid, 0)
    assert os.waitstatus_to_exitcode(status) == 0
    assert LocalTokenLog.read(path) == 0
    log.save(1)  # the process that took the lock keeps the right to save


@pytest.mark.parametrize("how", ["removed", "replaced"])
def test_a_log_whose_lock_file_is_gone_refuses_to_save(tmp_path: Path, how: str) -> None:
    path = tmp_path / "tokens"
    log = LocalTokenLog.create(path)
    sidecar = tmp_path / ".tokens.lock"
    sidecar.unlink()  # a tmp cleaner, `git clean`, an operator tidying *.lock
    if how == "replaced":
        sidecar.write_bytes(b"")  # a second coordinator would now lock this file instead
    with pytest.raises(TokenLogError):
        log.save(1)
    assert LocalTokenLog.read(path) == 0


def test_a_log_that_fails_to_start_does_not_keep_the_lock(tmp_path: Path) -> None:
    path = tmp_path / "tokens"
    LocalTokenLog.create(path).close()
    with pytest.raises(TokenLogError) as exists:
        LocalTokenLog.create(path)  # the traceback keeps the half-built log alive
    path.write_bytes(b"seven")
    broken = LocalTokenLog(path)
    with pytest.raises(TokenLogError) as unreadable:
        broken.load()  # and the caller may keep this one
    path.write_bytes(b"0")
    assert LocalTokenLog(path).load() == 0  # repaired: nothing is left holding the lock
    assert (exists.type, unreadable.type) == (TokenLogError, TokenLogError)


def test_a_start_that_is_refused_leaves_no_descriptor_open(tmp_path: Path) -> None:
    path = tmp_path / "tokens"
    log = LocalTokenLog.create(path)
    before = len(os.listdir("/proc/self/fd"))
    for _ in range(20):  # a supervisor retrying every second must not run out of descriptors
        with pytest.raises(TokenLogError):
            LocalTokenLog(path).load()
    assert len(os.listdir("/proc/self/fd")) == before
    log.close()


def test_a_filesystem_without_locks_is_not_reported_as_a_held_log(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    def refuse(fd: int, operation: int) -> None:
        raise OSError(errno.ENOLCK, "No locks available")

    monkeypatch.setattr(fcntl, "flock", refuse)
    before = len(os.listdir("/proc/self/fd"))
    with pytest.raises(OSError, match="No locks available") as failed:
        LocalTokenLog(tmp_path / "tokens").load()  # another coordinator is not what went wrong
    assert not isinstance(failed.value, TokenLogError)
    assert len(os.listdir("/proc/self/fd")) == before


def test_a_log_that_cannot_be_read_is_a_token_log_error(tmp_path: Path) -> None:
    folder = tmp_path / "tokens"
    folder.mkdir()  # a mount point, or a path somebody else made
    with pytest.raises(TokenLogError):
        LocalTokenLog(folder).load()
    with pytest.raises(TokenLogError):
        LocalTokenLog.read(folder)


def test_a_log_refuses_to_save_after_the_stored_mark_moved(tmp_path: Path) -> None:
    path = tmp_path / "tokens"
    log = LocalTokenLog.create(path)
    log.save(5)
    path.write_bytes(b"2")  # an older copy restored under a running coordinator
    with pytest.raises(TokenLogError):
        log.save(6)
    path.write_bytes(b"9")  # or a newer one: the log did not write it either
    with pytest.raises(TokenLogError):
        log.save(10)


def test_a_log_is_where_it_was_made_whatever_the_path_it_was_given(tmp_path: Path) -> None:
    real = tmp_path / "real"
    LocalTokenLog.create(real).close()
    link = tmp_path / "link"
    link.symlink_to(real)
    log = LocalTokenLog(link)
    log.load()
    log.save(1)
    assert link.is_symlink() and LocalTokenLog.read(real) == 1  # the replace went to the target


def test_a_save_that_cannot_replace_the_file_keeps_the_previous_mark(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    path = tmp_path / "tokens"
    log = LocalTokenLog.create(path)

    def refuse(src: object, dst: object) -> None:
        raise OSError("disk full")

    monkeypatch.setattr(os, "replace", refuse)
    with pytest.raises(OSError):
        log.save(2)
    monkeypatch.undo()
    assert LocalTokenLog.read(path) == 0
    assert not [p for p in tmp_path.iterdir() if p.name.endswith(".tmp")]
    log.save(2)  # the failure left the log usable, and the next token may skip the lost one
    assert LocalTokenLog.read(path) == 2


def test_a_save_killed_before_fsync_keeps_the_previous_mark(tmp_path: Path) -> None:
    path = tmp_path / "tokens"
    LocalTokenLog.create(path).save(1)  # dropped at once: the child can take the lock
    child = subprocess.run(
        [sys.executable, "-c", KILLED_AT_FSYNC, str(path), "file"], capture_output=True, timeout=300
    )
    assert child.returncode == -9, child.stderr  # 3 would be a mark still in the writer's buffer
    assert LocalTokenLog.read(path) == 1


def test_a_save_asks_for_the_directory_to_be_synced_after_the_rename(tmp_path: Path) -> None:
    path = tmp_path / "tokens"
    LocalTokenLog.create(path).save(1)
    child = subprocess.run(
        [sys.executable, "-c", KILLED_AT_FSYNC, str(path), "directory"],
        capture_output=True,
        timeout=300,
    )
    assert child.returncode == -9, child.stderr  # a save that never syncs the directory survives
    assert LocalTokenLog.read(path) == 2  # and the rename was done before it asked


def test_a_directory_sync_that_fails_after_the_rename_leaves_the_log_usable(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    path = tmp_path / "tokens"
    log = LocalTokenLog.create(path)

    def refuse(_: object) -> None:
        raise OSError("input/output error")

    monkeypatch.setattr("hypertrain.elastic.tokens._fsync_dir", refuse)
    with pytest.raises(OSError):
        log.save(2)
    monkeypatch.undo()
    assert LocalTokenLog.read(path) == 2  # the rename happened, so the object kept step with it
    log.save(3)
    assert LocalTokenLog.read(path) == 3
