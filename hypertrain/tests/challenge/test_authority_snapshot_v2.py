"""Portable finalized admissions retain original authority; no training in this test."""

from __future__ import annotations

import importlib.util
import json
import shutil
import sqlite3
import sys
from pathlib import Path

import pytest
from fastapi.testclient import TestClient
from pydantic import Field, create_model

from hypertrain.challenge.app import create_app
from hypertrain.protocol import envelope_v2
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location(
    "portable_authority_adapter", ROOT / "scripts/network_authority_snapshot.py"
)
assert spec is not None and spec.loader is not None
adapter = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = adapter
spec.loader.exec_module(adapter)
fixture_spec = importlib.util.spec_from_file_location(
    "hermetic_authority_contract", Path(__file__).parent / "authority_fixture_v2.py"
)
assert fixture_spec is not None and fixture_spec.loader is not None
authority_fixture = importlib.util.module_from_spec(fixture_spec)
sys.modules[fixture_spec.name] = authority_fixture
fixture_spec.loader.exec_module(authority_fixture)


@pytest.fixture(scope="module")
def package(tmp_path_factory):
    root = tmp_path_factory.mktemp("hermetic-od-authority")
    source, directory = root / "source", root / "bundle"
    run_id, originals = authority_fixture.build_authority(source)
    with pytest.MonkeyPatch.context() as patch:
        # Test-local run pin and declared metadata oracle; production adapter unchanged.
        patch.setattr(adapter, "RUN", run_id)
        patch.setattr(
            adapter,
            "Bundle",
            create_model(
                "HermeticBundle",
                __base__=adapter.Bundle,
                run_id=(str, Field(default=run_id, pattern="^" + run_id + "$")),
            ),
        )
        patch.setattr(
            adapter,
            "Bundle",
            create_model(
                "HermeticBundle",
                __base__=adapter.Bundle,
                run_id=(str, Field(default=run_id, pattern="^" + run_id + "$")),
            ),
        )
        patch.setattr(adapter, "validate_artifacts", authority_fixture.metadata_artifacts)
        digest = adapter.export(source, directory)
        yield directory, digest, originals


def test_original_export_validates_and_bootstraps_actual_service(package, tmp_path):
    directory, digest, originals = package
    bundle = adapter.validate(directory, digest)
    secrets = tmp_path / "roles"
    secrets.mkdir(mode=0o700)
    for role, public in bundle.public_roles.items():
        path = secrets / (role + ".seed")
        path.write_bytes(originals[public])
        path.chmod(0o600)
    token = secrets / "admin.token"
    token.write_text("portable-test-admin")
    token.chmod(0o600)
    state = tmp_path / "service"
    with adapter.bootstrap(directory, digest, state, secrets, "https://master.test") as config:
        app = create_app(config, verify_beacon=adapter.fixture_beacon)
        with TestClient(app):
            store = app.state.store
            escrow, admission, _ = store._services(adapter.RUN)
            escrow.verify()
            rows = store._db.execute("SELECT * FROM admissions_v2").fetchall()
            assert len(rows) == 4
            for row in rows:
                restored_now = store._now(store._db)
                assert restored_now >= bundle.beacon_round
                status = admission.status(row["hotkey"], now=restored_now)
                assert status.record.state == "ACTIVE" and status.eligible
                assert status.record.clean_count == 12 and status.funding.locked_units == 1000
            assert store._db.execute("SELECT COUNT(*) FROM accepted_v2").fetchone()[0] == 1
            assert store._db.execute("SELECT COUNT(*) FROM admission_trials").fetchone()[0] == 48
        store._db.close()
    bad = secrets / "coord.seed"
    bad.write_bytes(bytes(32))
    rejected = tmp_path / "rejected"
    with pytest.raises(ValueError, match="public identity"):
        with adapter.bootstrap(directory, digest, rejected, secrets, "https://master.test"):
            pytest.fail("wrong original role accepted")
    assert not rejected.exists()


@pytest.mark.parametrize(
    "fault",
    [
        "object",
        "reference",
        "foreign",
        "traversal",
        "digest",
        "screen",
        "strike",
        "pending",
        "funding",
        "rotation",
        "signature",
        "finality",
        "assignment",
    ],
)
def test_modified_package_rejects_before_restore(package, tmp_path, fault):
    original, digest, _ = package
    directory = tmp_path / "changed"
    shutil.copytree(original, directory)
    body = json.loads((directory / "bundle.json").read_bytes())
    if fault == "object":
        relative = next(k for k in body["files"] if k.startswith("objects/"))
        (directory / relative).unlink()
    elif fault in (
        "reference",
        "foreign",
        "screen",
        "strike",
        "pending",
        "funding",
        "rotation",
        "signature",
        "finality",
        "assignment",
    ):
        with sqlite3.connect(directory / "challenge.db") as db:
            if fault == "reference":
                db.execute("UPDATE admission_trial_results SET reference='{}' WHERE epoch=0")
            elif fault == "screen":
                db.execute("UPDATE admissions_v2 SET screen_until=0")
            elif fault == "strike":
                db.execute("UPDATE admissions_v2 SET strikes=1")
            elif fault == "pending":
                db.execute("UPDATE admissions_v2 SET pending_dispute=1")
            elif fault == "funding":
                db.execute("UPDATE escrow_units SET units=0 WHERE bucket='admission_locked'")
            elif fault == "rotation":
                db.execute("UPDATE admission_history SET coldkey='wrong-owner'")
            elif fault == "signature":
                signed = json.loads(
                    db.execute(
                        "SELECT proof FROM admission_trial_results WHERE epoch=0"
                    ).fetchone()[0]
                )
                signed["sig"] = "00" * 64
                db.execute(
                    "UPDATE admission_trial_results SET proof=? WHERE epoch=0",
                    (canonicalize(signed).decode(),),
                )
            elif fault == "finality":
                key, raw = db.execute(
                    "SELECT reservation,receipt FROM admission_reservations "
                    "WHERE reservation LIKE 'trial-final|%' LIMIT 1"
                ).fetchone()
                signed = envelope_v2.parse_envelope(raw)
                changed = dict(signed.body)
                changed["included"] = [authority_fixture.service.AUDITORS[0].ss58]
                resigned = envelope_v2.seal(
                    authority_fixture.service.COORD,
                    "Finalize",
                    adapter.RUN,
                    changed,
                    signed.exp_drand,
                )
                db.execute(
                    "UPDATE admission_reservations SET receipt=? WHERE reservation=?",
                    (canonicalize(resigned).decode(), key),
                )
            elif fault == "assignment":
                db.execute("UPDATE admission_trials SET nonce=? WHERE epoch=0", ("ff" * 32,))
            else:
                db.execute("UPDATE runs SET run_id=?", ("ff" * 32,))
        raw = (directory / "challenge.db").read_bytes()
        body["files"]["challenge.db"] = {"sha256": sha256_hex(raw), "size": len(raw)}
    elif fault == "traversal":
        body["files"]["../escape"] = body["files"]["challenge.db"]
    else:
        digest = "00" * 32
    if fault in (
        "reference",
        "foreign",
        "traversal",
        "screen",
        "strike",
        "pending",
        "funding",
        "rotation",
        "signature",
        "finality",
        "assignment",
    ):
        raw = canonicalize(body)
        (directory / "bundle.json").write_bytes(raw)
        digest = sha256_hex(raw)
    restored = tmp_path / "restored"
    with pytest.raises((ValueError, FileNotFoundError)) as rejected:
        with adapter.bootstrap(
            directory, digest, restored, tmp_path / "absent-secrets", "https://master.test"
        ):
            pytest.fail("tampered authority accepted")
    assert not restored.exists()
    if fault == "screen":
        assert "current funded admission is ineligible" in str(rejected.value)
