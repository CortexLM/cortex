"""The store, the beacon route, the weights answer and the dispute settlement survive a squatted id.

ADR 0014 part 1 at the service boundary: a funded coldkey signs an ordinary client event (a
transfer, or a lock where its ledger knows no other owner to transfer to) under exactly the id the
service derives, and the service operation still commits. The ledger-level chains are in
`tests/ledger/test_escrow_key_spaces_v2.py`.
"""

from __future__ import annotations

import hmac
import importlib.util
import json
import re
import sys
from dataclasses import replace
from pathlib import Path
from types import SimpleNamespace

import pytest

from hypertrain.auditor.island_bisect import RefereeEvidence
from hypertrain.beacon.core import BeaconRound
from hypertrain.challenge.admission_store import AdmissionStore
from hypertrain.challenge.store import ESCROW_ID_NAMESPACE_META, ChallengeError, ChallengeStore
from hypertrain.data.store import LocalFSStore
from hypertrain.ledger import Params
from hypertrain.ledger.escrow_v2 import SERVER_SCOPE, Settlement, escrow_id_key
from hypertrain.protocol.envelope_v2 import seal
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages_v2 import EscrowLock, EscrowTransfer, ResolutionV2


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

    def unresolved(self) -> list[object]:
        return []


class Admissions:
    def reap_expired_trials(self, *, now: int) -> None:
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


def squat_with_a_lock(escrow, origin: str, operation_id: str) -> None:
    """COLD locks the minimum admission bond under the id. The production ledger of `hosted` issued
    units to COLD only, and a transfer goes to a key the ledger knows (ADR 0014 Part 5.2)."""
    escrow.lock(E.lock(origin, operation_id, 100), signer=E.COLD.ss58)


@pytest.fixture
def hosted(tmp_path, actual_reward, monkeypatch):
    """A store hosting a matured shadow origin and the live reward, both held by COLD."""
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
    AdmissionStore(escrow)  # the tables the real services create; the pay path reads them
    store._services_v2[s.manifest.run_id()] = (escrow, Admissions(), Notified())
    shadow = E.sign_actual_shadow(reward, E.reserve_actual_shadow(escrow, evidence))
    escrow.reward_finalize(shadow, replace(evidence, shadow=True))
    escrow.mature(E.H, tuple(shadow.origin_ids), 20)
    escrow.reward_finalize(reward, evidence)
    yield store, escrow, reward, shadow.origin_ids[0]
    store.close()


def test_the_beacon_route_matures_a_reward_when_a_client_event_holds_its_maturity_id(
    hosted,
) -> None:
    store, escrow, reward, shadow_origin = hosted
    mature_id = sha256_hex(f"ht-mature/2|{escrow.run_id}|{reward.origin_ids[0]}".encode())
    squat_with_a_lock(escrow, shadow_origin, mature_id)
    store.push_beacon({"round": 20, "signature": "00" * 48, "randomness": "11" * 32})
    assert escrow.balances(E.COLD.ss58).reward_pending == 0
    assert escrow.balances().conserved()
    key = escrow.db.execute("SELECT operation_id FROM escrow_events WHERE kind='MATURE'").fetchall()
    assert [row["operation_id"].startswith(SERVER_SCOPE) for row in key] == [True, True]


def test_the_weights_answer_is_produced_when_a_client_event_holds_its_pay_id(hosted) -> None:
    store, escrow, reward, shadow_origin = hosted
    origin = reward.origin_ids[0]
    escrow.mature(sha256_hex(b"ht-mature-direct"), tuple(reward.origin_ids), 20)
    pay_id = sha256_hex(f"ht-pay/2|{escrow.run_id}|0|{origin}|{E.COLD.ss58}".encode())
    squat_with_a_lock(escrow, shadow_origin, pay_id)
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
        "SELECT value FROM meta WHERE key=?", (ESCROW_ID_NAMESPACE_META,)
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


def hold_a_bond(n) -> tuple[EscrowLock, dict]:
    """HOT[0] joins and its coldkey locks 1000 units of origin 0 as the admission bond; returns the
    lock and the terms of the bond as the holder reads them."""
    identity = n.join(0).json()["admission_id"]
    bond = EscrowLock(
        operation_id="36" * 32,
        owner=N.COLD[0].ss58,
        units=1000,
        origin_ids=[n.origin_ids[0]],
        admission_id=identity,
        dispute_id=None,
        kind="LOCK_ADMISSION",
    )
    locked = n.client.post(n.url + "/escrow/lock", json=n.signed(N.COLD[0], "EscrowLock", bond))
    assert locked.status_code == 200, locked.text
    terms = n.bond(identity)
    assert terms.status_code == 200, terms.text
    (entry,) = terms.json()["locks"]
    assert entry["origin_ids"] == bond.origin_ids and entry["lock_id"] == bond.operation_id
    return bond, entry


def identifiers(n, entry: dict) -> set[str]:
    """What no third party may read: every origin id and lock id of the ledger, and the two hashes
    a bond release names. The all-zero hash is the 'nothing yet' value of other fields."""
    found = {row[0] for row in n.store._db.execute("SELECT origin FROM escrow_origins")}
    found |= {
        row[0]
        for row in n.store._db.execute(
            "SELECT operation_id FROM escrow_events WHERE kind LIKE 'LOCK%'"
        )
    }
    found |= {entry["finality_hash"], entry["closed_dispute_root"]}
    return found - {"0" * 64}


def test_no_public_route_returns_an_origin_id(served) -> None:
    # Origin ids are readable by their holder (who receives them from the operator) and by the
    # admin; the routes a third party can call, and the receipts of the escrow routes, carry none.
    n = served
    origins = {row[0] for row in n.store._db.execute("SELECT origin FROM escrow_origins")}
    assert len(origins) == len(N.COLD)
    bond, entry = hold_a_bond(n)
    held = n.origin_ids[1]
    lock = EscrowLock(
        operation_id="31" * 32,
        owner=N.COLD[1].ss58,
        units=100,
        origin_ids=[held],
        admission_id=None,
        dispute_id="32" * 32,
        kind="LOCK_CONTEST",
    )
    unknown = lock.model_copy(update={"operation_id": "33" * 32, "origin_ids": ["34" * 32]})
    responses = [
        n.client.get(n.url),
        n.client.get(n.url + "/admission/" + N.HOT[0].ss58),
        n.client.post(n.url + "/escrow/lock", json=n.signed(N.COLD[1], "EscrowLock", lock)),
        n.client.post(n.url + "/escrow/lock", json=n.signed(N.COLD[1], "EscrowLock", unknown)),
        n.client.post(
            n.url + "/escrow/transfer",
            json=n.signed(
                N.COLD[1], "EscrowTransfer", transfer(N.COLD[1], N.COLD[2], held, "35" * 32)
            ),
        ),
    ]
    assert [r.status_code for r in responses[:3]] == [200, 200, 200]
    assert responses[3].status_code == 409  # an unknown origin is refused by code, not echoed
    status = responses[1].json()
    assert [set(e) for e in status["bond"]["locks"]] == [
        {"units", "outcome", "releasable", "release_beacon"}
    ]
    # a caller reads back the id of its own request; the bond's lock and hashes are not its to read
    held_by_others = origins | {
        bond.operation_id,
        entry["finality_hash"],
        entry["closed_dispute_root"],
    }
    for response in responses:
        for secret in held_by_others:
            assert secret not in response.text, response.request.url


def anonymous_gets(n) -> list[str]:
    """Every GET route of the application with its parameters filled in: the run, an admission,
    each fixture hotkey, a round, every stored object, and a hash for the rest."""
    values = {
        "run_id": n.manifest.run_id(),
        "w": "0",
        "admission_id": n.store._db.execute("SELECT admission_id FROM admissions_v2").fetchone()[0],
    }
    objects = sorted(path.name for path in n.store.objects.root.glob("??/*"))
    paths: list[str] = []
    for route in n.client.app.routes:
        template = getattr(route, "path", "")
        if "GET" not in (getattr(route, "methods", None) or ()):
            continue
        choices = {
            "hotkey": [key.ss58 for key in N.HOT],
            "sha256": objects,
            **{name: [value] for name, value in values.items()},
        }
        names = re.findall(r"{(\w+)(?::\w+)?}", template)
        combos = [{}]
        for name in names:
            options = choices.get(name, ["ab" * 32])
            combos = [{**combo, name: option} for combo in combos for option in options]
        paths += [template.format(**combo) for combo in combos]
    return paths


def test_no_anonymous_route_returns_an_escrow_identifier(served) -> None:
    # Given: a bond, a contest bond and a settled lookup of every kind the ledger holds
    n = served
    bond, entry = hold_a_bond(n)
    contest = EscrowLock(
        operation_id="31" * 32,
        owner=N.COLD[1].ss58,
        units=100,
        origin_ids=[n.origin_ids[1]],
        admission_id=None,
        dispute_id="32" * 32,
        kind="LOCK_CONTEST",
    )
    assert (
        n.client.post(
            n.url + "/escrow/lock", json=n.signed(N.COLD[1], "EscrowLock", contest)
        ).status_code
        == 200
    )
    secrets = identifiers(n, entry)
    assert bond.operation_id in secrets and contest.operation_id in secrets

    # When: every GET route is asked, with no credential at all
    paths = anonymous_gets(n)
    answers = {path: n.client.get(path) for path in paths}

    # Then: none fails and none carries an origin id, a lock id or a bond settlement hash
    assert len(paths) >= 30  # the walk found the routes
    assert {path for path, a in answers.items() if a.status_code == 500} == set()
    leaked = {path for path, a in answers.items() if any(s in a.text for s in secrets)}
    assert leaked == set()
