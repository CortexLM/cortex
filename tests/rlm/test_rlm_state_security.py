"""RLM journals and knowledge must reject aliases and replaceable lock state."""

import hashlib
import os

import pytest
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey

from cortex.rlm import KnowledgeStore, RunJournal, ToolRejected
from cortex.state import StateSecurityError


@pytest.mark.parametrize("kind", ["journal", "knowledge"])
def test_rlm_database_rejects_hardlink_before_modifying_other_file(tmp_path, kind):
    directory = tmp_path / "state"
    directory.mkdir(mode=0o700)
    target = tmp_path / "other.sqlite3"
    target.touch(mode=0o600)
    path = directory / ("runs.sqlite3" if kind == "journal" else "knowledge.sqlite3")
    os.link(target, path)

    with pytest.raises(StateSecurityError):
        if kind == "journal":
            RunJournal(directory).close()
        else:
            KnowledgeStore(
                path, owner_public_key=Ed25519PrivateKey.generate().public_key().public_bytes_raw()
            ).close()

    assert target.read_bytes() == b""


def test_knowledge_database_rejects_replaceable_parent(tmp_path):
    directory = tmp_path / "public"
    directory.mkdir(mode=0o700)
    directory.chmod(0o777)

    with pytest.raises(StateSecurityError):
        KnowledgeStore(
            directory / "knowledge.sqlite3",
            owner_public_key=Ed25519PrivateKey.generate().public_key().public_bytes_raw(),
        ).close()

    assert list(directory.iterdir()) == []


@pytest.mark.parametrize("attack", ["hardlink", "fifo", "permissive"])
def test_journal_rejects_unsafe_claim_before_entering_job(tmp_path, attack):
    journal = RunJournal(tmp_path / "journal")
    run_id = "topic/job"
    lock = journal.directory / (hashlib.sha256(run_id.encode()).hexdigest() + ".lock")
    if attack == "hardlink":
        target = tmp_path / "outside.lock"
        target.touch(mode=0o600)
        os.link(target, lock)
    elif attack == "fifo":
        os.mkfifo(lock, mode=0o600)
    else:
        lock.touch(mode=0o600)
        lock.chmod(0o666)
    entered = False

    try:
        with pytest.raises(StateSecurityError):
            with journal.claim(run_id):
                entered = True
        assert not entered
    finally:
        journal.close()


def test_journal_claim_excludes_other_instance_and_releases_after_failure(tmp_path):
    directory = tmp_path / "journal"
    first, second = RunJournal(directory), RunJournal(directory)

    try:
        with pytest.raises(RuntimeError, match="job interrupted"):
            with first.claim("topic/job"):
                with pytest.raises(ToolRejected, match="already running"):
                    with second.claim("topic/job"):
                        pytest.fail("same job cannot run concurrently")
                raise RuntimeError("job interrupted")
        with second.claim("topic/job"):
            digest = second.archive("topic/job", "measured evidence")
        assert first.read_archive("topic/job", digest) == "measured evidence"
    finally:
        first.close()
        second.close()
