import hashlib
import io
import os
import sqlite3
import tarfile

import pytest

from cortex.errors import ServiceError
from cortex.proof.artifacts import FileVault, check_env, verify_artifact
from cortex.proof.store import ProofStore


def tar_bytes(name="solution.py", body=b"print('experiment')\n", kind=tarfile.REGTYPE):
    output = io.BytesIO()
    with tarfile.open(fileobj=output, mode="w") as archive:
        member = tarfile.TarInfo(name)
        member.type = kind
        member.size = len(body)
        archive.addfile(member, io.BytesIO(body))
    return output.getvalue()


@pytest.mark.parametrize(
    "name,body,kind",
    [
        ("../escape", b"payload", tarfile.REGTYPE),
        ("/etc/profile", b"payload", tarfile.REGTYPE),
        ("empty", b"", tarfile.REGTYPE),
        ("link", b"", tarfile.SYMTYPE),
    ],
)
def test_artifact_rejects_escape_links_and_contentless_archives(name, body, kind):
    data = tar_bytes(name, body, kind)
    with pytest.raises(ServiceError):
        verify_artifact(data, hashlib.sha256(data).hexdigest())


def test_artifact_identity_is_the_exact_served_tar():
    data = tar_bytes()
    verify_artifact(data, hashlib.sha256(data).hexdigest())
    with pytest.raises(ServiceError, match="digest mismatch"):
        verify_artifact(data, "aa" * 32)


def test_vault_survives_restart_and_stores_private_files(tmp_path):
    directory = tmp_path / "vault"
    key = "ab" * 32
    FileVault(directory).put(key, {"MINER_API_KEY": "test-only-value"})
    restored = FileVault(directory).get(key, ["MINER_API_KEY"])
    assert restored == {"MINER_API_KEY": "test-only-value"}
    assert (directory / key).stat().st_mode & 0o777 == 0o700
    assert (directory / key / "MINER_API_KEY").stat().st_mode & 0o777 == 0o600


def test_vault_refuses_symlinked_submission_directory(tmp_path):
    vault = FileVault(tmp_path / "vault")
    outside = tmp_path / "outside"
    outside.mkdir()
    (vault.root / ("ab" * 32)).symlink_to(outside, target_is_directory=True)
    with pytest.raises(ServiceError):
        vault.put("ab" * 32, {"MINER_API_KEY": "test-only"})
    assert list(outside.iterdir()) == []


def test_vault_delete_removes_only_the_terminal_job_directory(tmp_path):
    vault = FileVault(tmp_path / "vault")
    first, second = "ab" * 32, "cd" * 32
    vault.put(first, {"MINER_API_KEY": "first"})
    vault.put(second, {"MINER_API_KEY": "second"})

    vault.delete(first)

    assert not (vault.root / first).exists()
    assert vault.get(second, ["MINER_API_KEY"]) == {"MINER_API_KEY": "second"}
    vault.delete(first)  # idempotent recovery cleanup


def test_failed_vault_write_removes_partial_credentials(tmp_path):
    vault = FileVault(tmp_path / "vault")
    key = "ab" * 32
    vault.put(key, {"MINER_API_KEY": "first"})

    with pytest.raises(ServiceError, match="vault unavailable"):
        vault.put(key, {"MINER_API_KEY": "replacement"})

    assert not (vault.root / key).exists()


def test_empty_vault_operations_do_not_create_a_job_directory(tmp_path):
    vault = FileVault(tmp_path / "vault")
    key = "ab" * 32

    vault.put(key, {})
    restored = vault.get(key, [])

    assert restored == {}
    assert not (vault.root / key).exists()


def test_vault_reconciliation_removes_orphans_and_preserves_active_credentials(tmp_path):
    vault = FileVault(tmp_path / "vault")
    active, orphan = "ab" * 32, "cd" * 32
    vault.put(active, {"MINER_API_KEY": "active-secret"})
    vault.put(orphan, {"OLD_API_KEY": "orphan-secret"})

    vault.reconcile({active: ["MINER_API_KEY"]})

    assert vault.get(active, ["MINER_API_KEY"]) == {"MINER_API_KEY": "active-secret"}
    assert not (vault.root / orphan).exists()


def test_vault_reconciliation_fails_when_active_credential_is_missing(tmp_path):
    vault = FileVault(tmp_path / "vault")

    with pytest.raises(ServiceError, match="credential.*missing"):
        vault.reconcile({"ab" * 32: ["MINER_API_KEY"]})


@pytest.mark.parametrize("kind", ["file", "symlink"])
def test_vault_reconciliation_rejects_unexpected_root_entries_without_removing_them(tmp_path, kind):
    vault = FileVault(tmp_path / "vault")
    unexpected = vault.root / "unexpected"
    if kind == "file":
        unexpected.write_text("unsafe")
    else:
        unexpected.symlink_to(tmp_path)

    with pytest.raises(ServiceError, match="unsafe entry"):
        vault.reconcile({})

    assert unexpected.exists() or unexpected.is_symlink()


def test_vault_rejects_hardlinked_credentials_without_unlinking_them(tmp_path):
    vault = FileVault(tmp_path / "vault")
    key = "ab" * 32
    vault.put(key, {"MINER_API_KEY": "active-secret"})
    credential = vault.root / key / "MINER_API_KEY"
    second_link = tmp_path / "credential-link"
    os.link(credential, second_link)

    with pytest.raises(ServiceError, match="exactly one link"):
        vault.delete(key)

    assert credential.read_text() == "active-secret"
    assert second_link.read_text() == "active-secret"


def test_vault_refuses_to_delete_a_permissive_credential(tmp_path):
    vault = FileVault(tmp_path / "vault")
    key = "ab" * 32
    vault.put(key, {"MINER_API_KEY": "active-secret"})
    credential = vault.root / key / "MINER_API_KEY"
    credential.chmod(0o640)

    with pytest.raises(ServiceError, match="not private"):
        vault.delete(key)

    assert credential.read_text() == "active-secret"


def test_vault_reconciliation_rejects_extra_credentials_for_an_active_job(tmp_path):
    vault = FileVault(tmp_path / "vault")
    key = "ab" * 32
    vault.put(key, {"MINER_API_KEY": "active-secret", "EXTRA_KEY": "unexpected-secret"})

    with pytest.raises(ServiceError, match="credential set mismatch"):
        vault.reconcile({key: ["MINER_API_KEY"]})

    assert (vault.root / key / "EXTRA_KEY").exists()


@pytest.mark.parametrize(
    "env", [{}, {"OTHER": "secret"}, {"PATH": "/tmp"}, {"MINER_API_KEY": "a\nb"}]
)
def test_topic_byok_policy_is_enforced_without_silent_drops(env):
    with pytest.raises(ServiceError):
        check_env({"miner_byok": "MINER_API_KEY"}, env)


def test_nonce_is_single_use_across_connections_and_restarts(tmp_path):
    path = tmp_path / "proof.sqlite3"
    hotkey, nonce, payload_digest, job_id = (
        "aa" * 32,
        "bb" * 32,
        "cc" * 32,
        "dd" * 32,
    )
    with ProofStore(path) as first, ProofStore(path) as second:
        first.reserve_nonce(hotkey, nonce, payload_digest, job_id, epoch=4)
        with pytest.raises(ServiceError, match="submit_nonce reused"):
            second.reserve_nonce(hotkey, nonce, "ee" * 32, "ff" * 32, epoch=4)
    with ProofStore(path) as restarted:
        with pytest.raises(ServiceError, match="submit_nonce reused"):
            restarted.reserve_nonce(hotkey, nonce, payload_digest, job_id, epoch=4)
        assert restarted.lookup_submission(hotkey, nonce, payload_digest) == {
            "id": job_id,
            "hotkey": hotkey,
            "epoch": 4,
            "status": "failed",
            "reason": "submission intake did not complete",
        }
        assert restarted.lookup_submission(hotkey, nonce, "ee" * 32) is None
    with sqlite3.connect(path) as connection:
        assert connection.execute("SELECT count(*) FROM proof_nonces").fetchone()[0] == 1
