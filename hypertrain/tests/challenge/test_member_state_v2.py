"""R-C2 at the store: why a roster entry no longer matches a live admission, who is left out, and
when an admission may rotate."""
# ruff: noqa: F811  (the `network` and `world` fixture parameters shadow the imported fixtures)

from __future__ import annotations

import ast
import json
import unittest.mock
from pathlib import Path

import pytest

from hypertrain.beacon import BeaconRound
from hypertrain.challenge.admission_store import AdmissionError
from hypertrain.challenge.store import ChallengeStore
from hypertrain.miner.admission import sign_rotation, sign_rotation_consent
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import Keypair

from . import test_dispute_exit_v2 as disputes
from . import test_service_network_v2 as fixtures
from .audit_lease_fixture_v2 import D_AUDIT, seed_round
from .five_identity_network_v2 import activate_identities
from .test_admission_bond_v2 import COLD, HOT, OTHER, World, unclosed_round, world  # noqa: F401
from .test_service_network_v2 import network  # noqa: F401  (pytest fixture import)

EVIDENCE = "ab" * 32
TREE = Path(__file__).parents[2]
THIRD = Keypair(b"\x7a" * 32)


def seize(reason):
    def apply(n, admission, hotkey, identity):
        admission.seize_bond(identity, reason=reason, evidence_hash=EVIDENCE, now=n.now)

    return apply


def seize_a_no_show_twice(n, admission, hotkey, identity):
    """The member is back in a roster (ACTIVE) when it stays silent a second time."""
    seize("NO_SHOW")(n, admission, hotkey, identity)
    with n.store._tx():
        n.store._db.execute(
            "UPDATE admissions_v2 SET state='ACTIVE' WHERE admission_id=?", (identity,)
        )
    admission.seize_bond(identity, reason="NO_SHOW", evidence_hash="cd" * 32, now=n.now)


def rotate(n, admission, hotkey, identity):
    run = n.manifest.run_id()
    request = sign_rotation_consent(
        fixtures.REFEREE,
        run_id=run,
        coldkey=fixtures.COLD[0].ss58,
        hotkey=hotkey,
        operation_id="aa" * 32,
        expires_beacon=10000,
    )
    admission.rotate(canonicalize(sign_rotation(fixtures.COLD[0], run, request, 10000)), now=n.now)


def suspend_for_a_dispute(n, admission, hotkey, identity):
    admission.suspend(
        hotkey,
        reason="PENDING_DISPUTE",
        now=n.now,
        pending_dispute=True,
        dispute_id="dd" * 32,
        evidence_hash=EVIDENCE,
    )


def resume_on_fresh_probation(n, admission, hotkey, identity):
    admission.suspend(hotkey, reason="TEST", now=n.now)
    admission.resume(hotkey, now=n.now)


def lose_the_bond(n, admission, hotkey, identity):
    """The bond leaves the ledger without a state change of the admission (a ledger fault: every
    seizure of the product suspends)."""
    with n.store._tx():
        n.store._db.execute("UPDATE escrow_units SET units=0 WHERE bucket='admission_locked'")


def end_the_screen(n, admission, hotkey, identity):
    with n.store._tx():
        n.store._db.execute(
            "UPDATE admissions_v2 SET screen_until=? WHERE admission_id=?", (n.now - 1, identity)
        )


@pytest.mark.parametrize(
    ("change", "cause"),
    [
        (lambda *_: None, None),
        (rotate, "ROTATED"),
        (seize("TRAINING_FRAUD"), "BANNED"),
        (seize("AUDIT_MISMATCH"), "SUSPENDED"),
        (suspend_for_a_dispute, "SUSPENDED"),
        (resume_on_fresh_probation, "NOT_LIVE"),
        (end_the_screen, "SCREEN_LAPSED"),
        (seize("NO_SHOW"), "NOT_LIVE"),
        (seize_a_no_show_twice, "SUSPENDED"),
        (lose_the_bond, "UNFUNDED"),
    ],
    ids=[
        "fit",
        "rotated",
        "banned",
        "suspended-and-unfunded",
        "pending-dispute",
        "fresh-probation",
        "screen-lapsed",
        "first-no-show-is-probation",
        "second-no-show-is-suspended",
        "unfunded",
    ],
)
def test_the_cause_of_an_unfit_entry_is_named(network, change, cause):
    # Given: a rostered admission, ACTIVE and funded, that then changes through the real hooks.
    run = network.manifest.run_id()
    (identity,) = activate_identities(network, fixtures, range(1))
    admission = network.store._services(run)[1]
    hotkey = fixtures.HOT[0].ss58
    # When
    change(network, admission, hotkey, identity)
    # Then
    assert network.store._unfit_cause_v2(admission, hotkey, network.now) == cause


@pytest.mark.parametrize("name", ["_unfit_cause_v2", "_exclude_unfit_members_v2"])
def test_the_exit_helpers_are_bound_to_the_service_implementation(name):
    # The exit decides who is paid, so a run that holds a service-admission record must see the
    # implementation change when it does (the owned set of `_service_implementation_v2`).
    source = TREE / "src/hypertrain"
    paths = ("aggregator/core.py", "aggregator/tape_v2.py")
    original = ChallengeStore._service_implementation_v2(source, paths)
    tree = ast.parse((source / "challenge/store.py").read_text())
    node = next(n for n in ast.walk(tree) if isinstance(n, ast.FunctionDef) and n.name == name)
    node.body.append(ast.Pass())
    with unittest.mock.patch("hypertrain.challenge.store.ast.parse", return_value=tree):
        assert ChallengeStore._service_implementation_v2(source, paths) != original


def test_a_beacon_fault_is_not_a_cause_to_leave_a_member_out(network, monkeypatch):
    # Given: an admission that cannot read a verified beacon at this round.
    admission = network.store._services(network.manifest.run_id())[1]
    monkeypatch.setattr(admission, "beacon", lambda now: BeaconRound(now + 1, "", "", False))
    # When / Then: the fault propagates; it must not read as "the member rotated away".
    with pytest.raises(AdmissionError, match="UNVERIFIED_BEACON"):
        network.store._unfit_cause_v2(admission, fixtures.HOT[0].ss58, network.now)


def test_only_an_audited_member_that_is_not_already_excluded_is_left_out(network):
    # Given: a seeded round of five whose members have no admission (every entry is unfit);
    # members 0, 1, 2 and 4 completed their audit, 3 is still queued, 4 was already excluded.
    store, run = network.store, network.manifest.run_id()
    keys = seed_round(network, n=5)
    network.push(D_AUDIT)
    store.schedule_audits_v2(run, 0)
    unverified = {"reason": "UNVERIFIED", "evidence_hash": EVIDENCE}
    with store._tx():
        store._db.execute(
            "UPDATE audit_leases_v2 SET state='completed',result='MATCH' WHERE hotkey IN (?,?,?,?)",
            tuple(keys[i].ss58 for i in (0, 1, 2, 4)),
        )
        store._put_record_v2(run, "excluded", f"0:{keys[4].ss58}", unverified)
        # When
        store._exclude_unfit_members_v2(run, 0)
    # Then
    seen = {
        r[0]: json.loads(r[1])
        for r in store._db.execute("SELECT id,data FROM records_v2 WHERE kind='excluded'")
    }
    assert set(seen) == {f"0:{keys[i].ss58}" for i in (0, 1, 2, 4)}
    assert seen[f"0:{keys[4].ss58}"] == unverified
    for i in (0, 1, 2):
        assert seen[f"0:{keys[i].ss58}"] == {
            "reason": "INELIGIBLE",
            "stage": "aggregate",
            "cause": "ROTATED",
            "evidence_hash": sha256_hex(f"ht-ineligible/1|{run}|0|{keys[i].ss58}|ROTATED".encode()),
        }
    assert store._record_v2(run, "finality", "0")["unresolved_audits"] == 5


@pytest.mark.parametrize(
    ("result", "status"),
    [("MATCH", 409), ("MISMATCH", 200), (None, 200)],
    ids=["match", "mismatch", "no-lease-row"],
)
def test_a_miner_cannot_contest_a_verdict_that_upholds_its_work(network, result, status):
    # Given: the audit lease of the contesting miner says `result` (no row at all for None).
    if result is not None:
        with network.store._tx():
            network.store._db.execute(
                "INSERT INTO audit_leases_v2(id,run_id,w,hotkey,state,result) VALUES(?,?,?,?,?,?)",
                (
                    sha256_hex(b"lease"),
                    network.manifest.run_id(),
                    disputes.W,
                    disputes.HOT[0].ss58,
                    "completed",
                    result,
                ),
            )
    # When: it funds and opens a contest of that verdict.
    _, response = disputes.post_dispute(network)
    # Then: a MATCH has nothing to contest, so nothing is written; a MISMATCH, and a verdict
    # with no lease row to look at, are contested as before.
    assert response.status_code == status, response.text
    miner = network.store._db.execute(
        "SELECT state,pending_dispute FROM admissions_v2 WHERE hotkey=?", (disputes.HOT[0].ss58,)
    ).fetchone()
    opened = network.store._db.execute("SELECT COUNT(*) FROM disputes_v2").fetchone()[0]
    if status == 409:
        assert "NOTHING_TO_CONTEST" in response.text
        assert (tuple(miner), opened) == (("APPLIED", 0), 0)
    else:
        assert (tuple(miner), opened) == (("SUSPENDED", 1), 1)


def rotation(w: World, *, old=HOT, new=OTHER, operation: str = "33" * 32) -> bytes:
    request = sign_rotation_consent(
        new,
        run_id=w.run_id,
        coldkey=COLD.ss58,
        hotkey=old.ss58,
        operation_id=operation,
        expires_beacon=200,
    )
    return canonicalize(sign_rotation(COLD, w.run_id, request, 200))


@pytest.mark.parametrize("closed", [False, True], ids=["open-round", "closed-round"])
def test_a_rostered_admission_cannot_rotate_until_its_round_closes(world: World, closed: bool):
    # Given: one rotation applied, then a round that lists the admission.
    first = rotation(world)
    assert world.a.rotate(first, now=3).hotkey == OTHER.ss58
    unclosed_round(world, closed=closed)
    # Then: a retry of the applied rotation is still answered, whatever the round says ...
    assert world.a.rotate(first, now=4).hotkey == OTHER.ss58
    # ... and a new rotation is refused while the round is open, accepted once it is closed.
    second = rotation(world, old=OTHER, new=THIRD, operation="44" * 32)
    if closed:
        assert world.a.rotate(second, now=5).hotkey == THIRD.ss58
        return
    with pytest.raises(AdmissionError, match="ROTATION_ROUND_OPEN"):
        world.a.rotate(second, now=5)
    assert world.a.store.by_id(world.admission).hotkey == OTHER.ss58


def test_a_round_without_a_finality_record_keeps_the_rotation_and_the_bond_locked(world: World):
    # Lens m2: an opening lists the admission and no finality record exists. Nothing writes that
    # state (both records go in one transaction and none is deleted), so it fails closed.
    world.bond()
    unclosed_round(world, closed=False)
    world.a.store.db.execute("DELETE FROM records_v2 WHERE kind='finality'")
    with pytest.raises(AdmissionError, match="ROTATION_ROUND_OPEN"):
        world.a.rotate(rotation(world), now=3)
    with pytest.raises(AdmissionError, match="BOND_NOT_RELEASABLE"):
        world.a.release_bond(world.admission, now=500)
