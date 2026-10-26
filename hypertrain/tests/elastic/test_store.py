"""Durable local checkpoint store: a key never names a partial or unsynced object."""

import hashlib
import subprocess
import sys
from pathlib import Path

import pytest

from hypertrain.data.store import CorruptObjectError, ObjectNotFound
from hypertrain.elastic.store import LocalCheckpointStore

DATA = b"checkpoint-state" * 4096

# The child dies by SIGKILL the moment put() asks for durability, as a host does when the
# provider pulls it in the middle of a checkpoint write.
KILLED_AT_FSYNC = """
import os, signal, sys
from pathlib import Path
from hypertrain.elastic.store import LocalCheckpointStore
store = LocalCheckpointStore(Path(sys.argv[1]))
os.fsync = lambda fd: os.kill(os.getpid(), signal.SIGKILL)
store.put(b"checkpoint-state" * 4096)
"""


def test_put_killed_before_fsync_publishes_nothing(tmp_path: Path) -> None:
    sha = hashlib.sha256(DATA).hexdigest()
    child = subprocess.run(
        [sys.executable, "-c", KILLED_AT_FSYNC, str(tmp_path)], capture_output=True, timeout=120
    )
    assert child.returncode == -9, child.stderr
    store = LocalCheckpointStore(tmp_path)
    assert not store.exists(sha)
    with pytest.raises(ObjectNotFound):
        store.get(sha)
    assert store.put(DATA) == sha
    assert store.get(sha) == DATA


def test_put_rewrites_a_torn_object_under_its_key(tmp_path: Path) -> None:
    store = LocalCheckpointStore(tmp_path)
    sha = hashlib.sha256(DATA).hexdigest()
    torn = tmp_path / sha[:2] / sha
    torn.parent.mkdir(parents=True)
    torn.write_bytes(DATA[:100])  # what a writer without fsync leaves after a power loss
    with pytest.raises(CorruptObjectError):
        store.get(sha)
    assert store.put(DATA) == sha
    assert store.get(sha) == DATA
