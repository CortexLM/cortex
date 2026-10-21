"""Hosted production bootstrap route and economic-bound record writer."""

from __future__ import annotations

import importlib.util
import json
import sys
from pathlib import Path

import pytest

from hypertrain.challenge.admission_store import AdmissionError
from hypertrain.challenge.store import ChallengeError, ChallengeStore
from hypertrain.data.store import LocalFSStore
from hypertrain.ledger import Params
from hypertrain.protocol.envelope_v2 import UnauthorizedSigner, seal, verify_envelope
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages_v2 import DisputePolicyV2, RunManifestV2

_spec = importlib.util.spec_from_file_location(
    "production_economics_fixture",
    Path(__file__).parents[1] / "ledger/test_production_bootstrap_v2.py",
)
assert _spec is not None and _spec.loader is not None
B = importlib.util.module_from_spec(_spec)
sys.modules[_spec.name] = B
_spec.loader.exec_module(B)
E = B.E


def hosted(tmp_path: Path, monkeypatch, *, cuda: bool, **economics):
    """Production run in a real store; `cuda` stands in for an installed CUDA qualification."""
    s = B.pinned(E.setup("production"), **economics)
    dispute = DisputePolicyV2(
        referees=[E.OTHER.ss58],
        max_open=32,
        max_transcript_entries=256,
        max_entry_bytes=65536,
        fanout=2,
        max_referee_reassignments=1,
        max_referee_cost_units=100,
    )
    body = s.manifest.body()
    body["network"]["dispute_policy_hash"] = dispute.digest()
    manifest = RunManifestV2.model_validate(body)
    objects = LocalFSStore(tmp_path / "objects")
    for policy in (s.policy, s.admission_policy, dispute):
        assert objects.put(canonicalize(policy.body())) == policy.digest()
    store = ChallengeStore(
        tmp_path / "state",
        Params("hypertrain", manifest.training.beacon.genesis_time, 300, 1, 1),
        E.COORD,
        None,
        lambda payload: None,
        objects,
    )
    store._db.execute(
        "INSERT INTO runs VALUES(?,?,?,'created',NULL)",
        (manifest.run_id(), manifest.model_dump_json(), "{}"),
    )
    if cuda:
        monkeypatch.setattr(store, "_backend_v2", lambda m: "cuda")
    return store, manifest.run_id()


def beacon(store, number: int, *, bls: bool) -> None:
    store._db.execute(
        "INSERT INTO beacon VALUES(?,?,?,?)", (number, "aa" * 48, "bb" * 32, int(bls))
    )


def signed(run_id: str, signer=E.COORD) -> bytes:
    return canonicalize(seal(signer, "ProductionBootstrap", run_id, B.bootstrap(run_id), 10000))


@pytest.fixture
def close():
    stores = []
    yield stores.append
    for store in stores:
        store.close_v2_notifications()
        store.close()


def issued(store) -> int:
    return store._db.execute("SELECT COALESCE(SUM(issued),0) FROM escrow_origins").fetchone()[0]


def test_bootstrap_route_refuses_cpu_backend(tmp_path, monkeypatch, close) -> None:
    store, run_id = hosted(tmp_path, monkeypatch, cuda=False)
    close(store)
    beacon(store, 1, bls=True)
    with pytest.raises(ChallengeError, match="production CUDA qualification required"):
        store.production_bootstrap_v2(run_id, signed(run_id))
    # Refused before any escrow service (or ledger table) exists for the run.
    assert store._services_v2 == {}
    assert (
        store._db.execute("SELECT 1 FROM sqlite_master WHERE name='escrow_origins'").fetchone()
        is None
    )


def test_bootstrap_route_requires_bls_beacon(tmp_path, monkeypatch, close) -> None:
    # Given: an unverified (fixture/HTTP-relayed) latest beacon round.
    store, run_id = hosted(tmp_path, monkeypatch, cuda=True)
    close(store)
    beacon(store, 1, bls=False)
    # When / Then
    with pytest.raises(AdmissionError, match="UNVERIFIED_BEACON"):
        store.production_bootstrap_v2(run_id, signed(run_id))
    assert issued(store) == 0
    # A BLS-verified round lets the same signed request through, once.
    beacon(store, 2, bls=True)
    receipt = store.production_bootstrap_v2(run_id, signed(run_id))
    assert receipt["ledger_mode"] == "production"
    assert issued(store) == 30000


def test_bootstrap_route_rejects_wrong_envelope_signer(tmp_path, monkeypatch, close) -> None:
    store, run_id = hosted(tmp_path, monkeypatch, cuda=True)
    close(store)
    beacon(store, 1, bls=True)
    with pytest.raises(UnauthorizedSigner):
        store.production_bootstrap_v2(run_id, signed(run_id, E.OTHER))
    assert issued(store) == 0


def test_economic_bound_refused_when_bound_false(tmp_path, monkeypatch, close) -> None:
    # Given: q*s*beta*(R+S) = 1100 units < G = 1101.
    store, run_id = hosted(tmp_path, monkeypatch, cuda=True, G_max_units=1101)
    close(store)
    beacon(store, 1, bls=True)
    _, admission, _ = store._services(run_id)
    # When / Then
    with pytest.raises(ChallengeError, match="economic bound"):
        store.economic_bound_v2(run_id)
    assert (
        store._db.execute("SELECT COUNT(*) FROM records_v2 WHERE kind='economic-bound'").fetchone()[
            0
        ]
        == 0
    )
    assert admission.conservative_bound() is False


def test_economic_bound_written_once_and_immutable(tmp_path, monkeypatch, close) -> None:
    # Given: q*s*beta*(R+S) = 1100 units >= G = 1100 (exact boundary).
    store, run_id = hosted(tmp_path, monkeypatch, cuda=True, G_max_units=1100)
    close(store)
    beacon(store, 1, bls=True)
    _, admission, _ = store._services(run_id)
    assert admission.conservative_bound() is False
    # When
    record = store.economic_bound_v2(run_id)
    raw = store._db.execute(
        "SELECT data FROM records_v2 WHERE kind='economic-bound' AND id='run'"
    ).fetchone()[0]
    # Then: coordinator-signed, admission now sees the bound.
    assert record["receipt"]["signer"] == E.COORD.ss58 and verify_envelope(record["receipt"])
    assert admission.conservative_bound() is True
    # A later call (new beacon, fresh randomized signature possible) never rewrites it.
    beacon(store, 2, bls=True)
    assert store.economic_bound_v2(run_id) == record
    after = store._db.execute(
        "SELECT data FROM records_v2 WHERE kind='economic-bound' AND id='run'"
    ).fetchone()[0]
    assert after == raw
    # A stored record bound to another policy is a conflict, not silently replaced.
    store._db.execute(
        "UPDATE records_v2 SET data=json_set(data,'$.economics_policy_hash',?) "
        "WHERE kind='economic-bound'",
        ("ff" * 32,),
    )
    with pytest.raises(ChallengeError, match="immutable economic-bound"):
        store.economic_bound_v2(run_id)
    assert (
        json.loads(
            store._db.execute("SELECT data FROM records_v2 WHERE kind='economic-bound'").fetchone()[
                0
            ]
        )["economics_policy_hash"]
        == "ff" * 32
    )
