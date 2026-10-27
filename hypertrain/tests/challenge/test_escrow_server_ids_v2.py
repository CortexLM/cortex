"""The store, the beacon route, the weights answer and the dispute settlement survive a squatted id.

ADR 0014 part 1 at the service boundary: a funded coldkey signs an ordinary transfer under exactly
the id the service derives, and the service operation still commits. The ledger-level chains are in
`tests/ledger/test_escrow_key_spaces_v2.py`.
"""

from __future__ import annotations

import hmac
import importlib.util
import json
import sys
from dataclasses import replace
from pathlib import Path
from types import SimpleNamespace

import pytest

from hypertrain.auditor.island_bisect import RefereeEvidence
from hypertrain.beacon.core import BeaconRound
from hypertrain.challenge.store import ChallengeError, ChallengeStore
from hypertrain.data.store import LocalFSStore
from hypertrain.ledger import Params
from hypertrain.ledger.escrow_v2 import SERVER_SCOPE, Settlement, escrow_id_key
from hypertrain.protocol.envelope_v2 import seal
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages_v2 import EscrowTransfer, ResolutionV2


def load(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


HERE = Path(__file__).parent
E = load("escrow_server_ids_ledger", HERE.parent / "ledger/test_escrow_v2.py")
D = load("escrow_server_ids_disputes", HERE / "test_disputes_v2.py")
N = load("escrow_server_ids_network", HERE / "test_service_network_v2.py")
actual_reward = E.actual_reward
network = N.network


class Notified:
    def notify(self) -> None:
        return None


def transfer(owner: Keypair, recipient: Keypair, origin: str, operation_id: str) -> EscrowTransfer:
    return EscrowTransfer(
        operation_id=operation_id,
        owner=owner.ss58,
        recipient=recipient.ss58,
        units=1,
        origin_ids=[origin],
        admission_id=None,
        dispute_id=None,
    )


def squat(escrow, owner: Keypair, origin: str, operation_id: str) -> None:
    escrow.transfer(transfer(owner, E.COLD, origin, operation_id), signer=owner.ss58)


@pytest.fixture
def hosted(tmp_path, actual_reward, monkeypatch):
    """A store hosting a matured shadow origin and the live reward, OTHER holding one free unit."""
    s, evidence, reward = actual_reward
    store = ChallengeStore(
        tmp_path / "state",
        Params("hypertrain", s.manifest.training.beacon.genesis_time - 600, 300, 1, 1),
        None,
        None,
        lambda payload: BeaconRound(payload["round"], "00" * 48, "11" * 32, True),
        LocalFSStore(tmp_path / "objects"),
    )
    escrow = E.ledger(
        s,
        store._db,
        replay=E.verified_shadow,
        settlement=lambda ref: Settlement(
            finality_hash=reward.finalize_hash,
            closed_dispute_root=E.H,
            release_beacon=20,
            unresolved=False,
            outcome="MATCH",
        ),
    )
    escrow.mutex = store._lock
    store._db.execute(
        "INSERT INTO runs VALUES(?,?,?,'created',NULL)",
        (s.manifest.run_id(), s.manifest.model_dump_json(), "{}"),
    )
    monkeypatch.setattr(store, "_services", lambda run_id: (escrow, None, None))
    store._services_v2[s.manifest.run_id()] = (escrow, None, Notified())
    shadow = E.sign_actual_shadow(reward, E.reserve_actual_shadow(escrow, evidence))
    escrow.reward_finalize(shadow, replace(evidence, shadow=True))
    escrow.mature(E.H, tuple(shadow.origin_ids), 20)
    origin = shadow.origin_ids[0]
    escrow.transfer(transfer(E.COLD, E.OTHER, origin, "66" * 32), signer=E.COLD.ss58)
    escrow.reward_finalize(reward, evidence)
    yield store, escrow, reward, origin
    store.close()


def test_the_beacon_route_matures_a_reward_when_a_client_event_holds_its_maturity_id(
    hosted,
) -> None:
    store, escrow, reward, other_origin = hosted
    mature_id = sha256_hex(f"ht-mature/2|{escrow.run_id}|{reward.origin_ids[0]}".encode())
    squat(escrow, E.OTHER, other_origin, mature_id)
    store.push_beacon({"round": 20, "signature": "00" * 48, "randomness": "11" * 32})
    assert escrow.balances(E.COLD.ss58).reward_pending == 0
    assert escrow.balances().conserved()
    key = escrow.db.execute("SELECT operation_id FROM escrow_events WHERE kind='MATURE'").fetchall()
    assert [row["operation_id"].startswith(SERVER_SCOPE) for row in key] == [True, True]


def test_the_weights_answer_is_produced_when_a_client_event_holds_its_pay_id(hosted) -> None:
    store, escrow, reward, other_origin = hosted
    origin = reward.origin_ids[0]
    escrow.mature(sha256_hex(b"ht-mature-direct"), tuple(reward.origin_ids), 20)
    pay_id = sha256_hex(f"ht-pay/2|{escrow.run_id}|0|{origin}|{E.COLD.ss58}".encode())
    squat(escrow, E.OTHER, other_origin, pay_id)
    beacon = escrow.manifest.training.beacon
    answer = json.loads(store.weights(0, beacon.genesis_time + 19 * beacon.period, 1))
    assert answer["weights"] == {E.HOT.ss58: 1_000_000.0}
    pays = escrow.db.execute("SELECT operation_id FROM escrow_events WHERE kind='PAY'").fetchall()
    assert len(pays) == 1 and pays[0]["operation_id"].startswith(SERVER_SCOPE)


def test_a_dispute_settles_when_a_client_event_holds_its_settlement_id(tmp_path: Path) -> None:
    d, f, ref, _, raw = D.service(tmp_path)
    dispute_id = sha256_hex(
        canonicalize([d.escrow.run_id, d.contest_of("11" * 32).verdict_hash, f.HOT.ss58])
    )
    settle_id = sha256_hex(canonicalize([dispute_id, "settle"]))
    other_origin = d.db.execute(
        "SELECT origin FROM escrow_units WHERE owner=? AND units>0", (f.OTHER.ss58,)
    ).fetchone()[0]
    squat(d.escrow, E.OTHER, other_origin, settle_id)
    turn = d.open(raw, now=1)
    assert turn.dispute_id == dispute_id
    evidence = RefereeEvidence(
        dispute_id=turn.dispute_id,
        transcript_hash=turn.transcript_hash,
        reason="FRAUD",
        loser=f.HOT.ss58,
        predecessor_hash="11" * 32,
        inputs_hash="22" * 32,
        output_hash="33" * 32,
        op_spec="verified.actual.op",
    )
    d.register_evidence(evidence)
    resolution = ResolutionV2(
        dispute_id=turn.dispute_id,
        transcript_hash=turn.transcript_hash,
        reason="FRAUD",
        loser=f.HOT.ss58,
        evidence_hash=evidence.digest(),
    )
    d.resolution(canonicalize(seal(ref, "ResolutionV2", d.run_id, resolution, 100000)), now=3)
    d.settle_lock(turn.dispute_id, now=3, referee_cost=10)
    assert d.escrow.balances().burned > 0 and d.escrow.balances().conserved()
    d.escrow.verify()


@pytest.fixture
def served(tmp_path_factory):
    generator = network.__wrapped__(tmp_path_factory.mktemp("id-key"), SimpleNamespace(param=None))
    n = next(generator)
    try:
        yield n
    finally:
        generator.close()


def test_the_store_derives_every_server_id_under_the_coordinator_id_key(served) -> None:
    n = served
    run = n.manifest.run_id()
    escrow = n.store._services(run)[0]
    key = escrow_id_key(n.store.coord._secret)
    assert escrow.id_key == key
    genesis = escrow.db.execute(
        "SELECT operation_id FROM escrow_events WHERE kind='TEST_GENESIS'"
    ).fetchone()["operation_id"]
    raw = sha256_hex(f"ht-genesis-v2|{run}".encode())
    assert genesis == SERVER_SCOPE + hmac.digest(key, raw.encode(), "sha256").hex()
    assert genesis != SERVER_SCOPE + raw  # the public formula does not give the stored key
    pinned = n.store._db.execute(
        "SELECT value FROM meta WHERE key='escrow-id-namespace-v2'"
    ).fetchone()["value"]
    assert pinned == sha256_hex(key)


def test_a_changed_coordinator_secret_fails_closed(served) -> None:
    n = served
    run = n.manifest.run_id()
    n.store._services(run)  # pins the namespace of the first secret
    n.store._services_v2.clear()
    n.store.coord = Keypair(bytes(range(1, 33)))
    with pytest.raises(ChallengeError) as error:
        n.store._services(run)
    assert error.value.status == 503 and "escrow id namespace differs" in str(error.value)
    assert run not in n.store._services_v2  # nothing was built with the wrong key
