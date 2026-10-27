"""Cheat 8 at the v2 service level: a late, absent or honestly evicted miner is not a cheater.

Every round below rosters five graduated miners, four honest and one absent in one of the ways
`ac_absence.CLASSES` lists, and goes through the real path (run-v2, audit, aggregate, finalize,
get_weights) on CPU. The non-sanction contract is asserted for each class. The liveness defence
(the four honest miners still settle without the absent one) does not hold today: the classes that
stall on a different master branch each have an `xfail(strict=True, raises=AssertionError)`
reproducer whose assertions are the desired defence (a late commit leaves the master in the
state of a no-show, so it shares that reproducer). The trial-stage absence of an applicant is
driven over HTTP on the same fixture network without graduating anybody.
"""

from __future__ import annotations

from pathlib import Path
from types import SimpleNamespace
from typing import Any

import ac_absence as A
import pytest
from ac_precondition import precondition
from ac_world import World, load_service, record

from hypertrain.miner.admission import sign_join
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages_v2 import HardwareHint

NOT_A_CHEAT = ("UNVERIFIED", "FRAUD")
STALL_TAIL = (
    "nothing writes an `excluded` record for a roster member that is missing its commit, leaves "
    "or delta, and schedule_audits_v2 (store.py:6387), lease_v2 (6593) and verified_inputs_v2 "
    "(7904) do not skip one; no operator route unsticks the round (re-opening w answers 409 "
    "'conflicting signed operation nonce', opening w+1 answers 'missing accepted applied: 0'), "
    "ADR 0002:24"
)


def excluded_reason(world: World, index: int) -> Any:
    return world.observe().excluded.get(f"0:{world.hotkey(index)}", {}).get("reason")


@pytest.mark.parametrize("name", A.CLASSES)
def test_an_absent_miner_is_never_sanctioned_as_a_cheater(ac_clone: World, name: str) -> None:
    """Defence: whichever way a rostered miner is absent, the master excludes nobody as a failed
    audit or a fraud and neither strikes, suspends, disputes, slashes nor unlocks any of the five
    miners; the answers the master gave are kept as evidence."""
    world = ac_clone
    before = {i: A.standing(world, i) for i in A.ROSTER}
    burned = world.observe().escrow["burned"]

    run = A.run_class(world, name)
    settlement = world.settle(0)
    paid = world.weights()
    seen = world.observe()

    assert [o.status for o in run.honest] == ["UPLOADED"] * 4
    assert not run.absent[-1].uploaded
    assert all(s["eligible"] and s["locked"] == 1000 for s in before.values())
    assert {i: A.standing(world, i) for i in A.ROSTER} == before
    assert not {e["reason"] for e in seen.excluded.values()} & set(NOT_A_CHEAT)
    assert "MISMATCH" not in settlement.results.values()
    assert seen.escrow["burned"] == burned and seen.escrow["conserved"] is True
    record(A.evidence(world, "absent-miner-is-not-sanctioned", name, run, settlement, paid))


@pytest.mark.parametrize("name", [A.LATE_COMMIT, A.EVICTED_LATE])
def test_a_commit_after_d_commit_is_refused_and_leaves_no_record(
    ac_clone: World, name: str
) -> None:
    """Defence: a commit posted once the master clock reached d_commit is answered 409 'commit
    window closed' and leaves no commit record, whether the miner was merely late or came back
    from an eviction."""
    world = ac_clone
    run = A.run_class(world, name)
    identities = {i: f"0:{world.hotkey(i)}" for i in A.ROSTER}

    late = run.absent[-1]
    assert (late.status, late.http_status, late.detail, late.endpoint) == (
        "refused",
        409,
        "commit window closed",
        "POST /commit",
    )
    assert world.network.now >= run.opening.d_commit
    commits = {
        i: world.network.store._db.execute(
            "SELECT COUNT(*) FROM records_v2 WHERE run_id=? AND kind='commit' AND id=?",
            (world.run_id, identity),
        ).fetchone()[0]
        for i, identity in identities.items()
    }
    # The four honest rows are the control: they prove the query finds a commit when there is one.
    assert commits == {**dict.fromkeys(A.HONEST, 1), A.ABSENT: 0}
    record(A.evidence(world, "late-commit-refused", name, run, world.settle(0)))


def test_an_evicted_miner_that_reruns_before_d_commit_is_uploaded_matched_and_paid(
    ac_clone: World,
) -> None:
    """Defence: a miner whose first launch died, leaving only an attempt directory, retrains on
    its rerun before d_commit and ends UPLOADED, MATCH and paid like its four honest peers."""
    world = ac_clone
    opening = world.open_round(0, A.ROSTER)
    before = {i: A.standing(world, i) for i in A.ROSTER}
    honest = world.run_roster({i: None for i in A.HONEST})
    killed = A.EvictedMidLaunch()

    first = world.run_miner(A.ABSENT, killed)
    left = [p for p in killed.attempts if p.is_dir()]
    assert first.error == "Evicted: host reclaimed during the launch"
    assert len(left) == 1 and not (left[0].parent / "published").exists()
    assert world.network.now < opening.d_commit

    second = world.run_miner(A.ABSENT)
    settlement = world.settle(0)
    paid = world.weights()
    hotkeys = [world.hotkey(i) for i in A.ROSTER]

    assert [o.status for o in honest] == ["UPLOADED"] * 4
    assert second.uploaded, second
    assert settlement.settled, [(s.name, s.status, s.detail) for s in settlement.steps]
    assert settlement.results == dict.fromkeys(hotkeys, "MATCH")
    assert list(settlement.included) == sorted(hotkeys)
    assert paid.weights == dict.fromkeys(hotkeys, 200000.0)
    assert {i: A.standing(world, i) for i in A.ROSTER} == before
    run = A.Run(opening, honest, [first, second])
    record(
        A.evidence(
            world, "evicted-then-rerun-before-d-commit", "evicted-mid-launch", run, settlement, paid
        )
    )


def test_an_exhausted_audit_lease_excludes_the_miner_as_infrastructure_not_as_a_mismatch(
    ac_clone: World,
) -> None:
    """Defence: a miner that committed but never published its leaves is excluded when its audit
    lease is exhausted with reason INFRASTRUCTURE and result AUDIT_INFRASTRUCTURE, never as
    UNVERIFIED or MISMATCH, and the standing of the five miners is unchanged. Which honest rows
    are exhausted with it is the liveness question of the xfail below, not asserted here."""
    world = ac_clone
    absent = world.hotkey(A.ABSENT)
    before = {i: A.standing(world, i) for i in A.ROSTER}
    run = A.run_class(world, A.NO_LEAVES)
    settlement = world.settle(0)
    db = world.network.store._db
    absolute = db.execute(
        "SELECT absolute FROM audit_leases_v2 WHERE run_id=? AND hotkey=?", (world.run_id, absent)
    ).fetchone()[0]
    assert world.observe().excluded == {}

    # The window ends at d_final, where the commit expires too: AUD-07 opens no second window that
    # nobody could lease, so the job is dropped as INFRASTRUCTURE at once (ADR 0011 Part 2).
    world.advance_beacon(absolute)
    seen = world.observe()
    lease = next(row for row in seen.leases if row["hotkey"] == absent)

    assert lease["state"] == "exhausted" and lease["result"] == "AUDIT_INFRASTRUCTURE"
    assert seen.excluded[f"0:{absent}"]["reason"] == "INFRASTRUCTURE"
    assert "MISMATCH" not in {row["result"] for row in seen.leases}
    assert "UNVERIFIED" not in {e["reason"] for e in seen.excluded.values()}
    assert {i: A.standing(world, i) for i in A.ROSTER} == before
    record(A.evidence(world, "lease-exhausted-is-infrastructure", A.NO_LEAVES, run, settlement))


def liveness_defence(world: World, name: str) -> None:
    """Once the absence is certain, the four honest miners still audit, aggregate, finalize and
    are paid; the absent one is classified as an absence and not paid."""
    run = A.run_class(world, name)
    precondition(
        [o.status for o in run.honest] == ["UPLOADED"] * 4 and not run.absent[-1].uploaded,
        f"the scenario did not play: {[(o.status, o.error) for o in [*run.honest, *run.absent]]}",
    )
    settlement = world.settle(0)
    if not settlement.settled:
        # Commit, leaves and delta are all due before d_upload; the audit challenges expire at
        # the lease horizon, so the master has no reason to wait longer than that.
        world.advance_beacon(run.opening.d_upload + 1)
        settlement = A.finish_round(world, settlement)
    honest = sorted(world.hotkey(i) for i in A.HONEST)
    absent = world.hotkey(A.ABSENT)
    record(
        A.evidence(
            world,
            "honest-round-settles-without-the-absent",
            name,
            run,
            settlement,
            hole="AC-LIVENESS-" + name,
        )
    )
    if not settlement.settled:
        step, fragment, _ = STALLS[name]
        first = next(s for s in settlement.steps if not s.ok and s.status != 204)
        precondition(
            (first.name, first.status) == (step, 409) and fragment in str(first.detail),
            f"the round stalls elsewhere than the reason says: {first.name} {first.status} "
            f"{first.detail}",
        )

    assert settlement.settled, [(s.name, s.status, s.detail) for s in settlement.steps if not s.ok]
    assert list(settlement.included) == honest
    assert all(settlement.results.get(h) == "MATCH" for h in honest)
    assert "MISMATCH" not in settlement.results.values()
    excluded = world.observe().excluded
    assert not {f"0:{h}" for h in honest} & set(excluded), "an honest miner was excluded too"
    assert excluded[f"0:{absent}"]["reason"] not in NOT_A_CHEAT
    paid = world.weights()
    shares = {h: paid.weights.get(h, 0) for h in honest}
    assert min(shares.values()) > 0 and len(set(shares.values())) == 1, shares
    assert paid.weights.get(absent, 0) == 0


# name -> (first failing step, fragment of its 409 detail, why the round stops there)
STALLS = {
    A.NO_SHOW: (
        "audits",
        "missing accepted commit",
        "schedule_audits_v2 answers 409 'missing accepted commit' for the whole round",
    ),
    A.NO_LEAVES: (
        "aggregate",
        "missing accepted delta",
        "lease_v2 skips a job without leaves (the honest rows are audited), but the member is "
        "dropped as INFRASTRUCTURE only when its job is exhausted after two audit windows, so "
        "aggregate answers 409 'missing accepted delta' until then",
    ),
    A.NO_DELTA: (
        "aggregate",
        "missing accepted delta",
        "aggregate answers 409 'missing accepted delta' although every audit matched",
    ),
}


@pytest.mark.parametrize(
    "name",
    [
        pytest.param(
            name,
            marks=pytest.mark.xfail(
                strict=True,
                raises=AssertionError,
                reason=f"AC-LIVENESS-{name}: {why}",
            ),
        )
        if name == A.NO_LEAVES
        else name
        for name, (_, _, why) in STALLS.items()
    ],
)
def test_the_honest_round_settles_without_the_absent_miner(ac_clone: World, name: str) -> None:
    """Defence: a rostered miner's absence must not stop the four honest miners' round. Once the
    deadline has passed the round audits, aggregates and finalizes without it, excludes only the
    absent miner, classifies that as an absence (not a fraud), and pays the four honest miners
    equally and the absent one nothing. A late commit leaves the master in the state of a no-show
    (no commit record, same branch), so it has no reproducer of its own."""
    liveness_defence(ac_clone, name)


@pytest.fixture
def applicants(tmp_path: Path):
    """The fixture network of the service tests with nobody graduated: `join(i)` applies."""
    svc = load_service(5)
    generator = svc.network.__wrapped__(tmp_path, SimpleNamespace(param=None))
    try:
        yield svc, next(generator)
    finally:
        generator.close()


def challenge_of(n: Any, admission_id: str) -> Any:
    return n.challenge(admission_id)


def record_of(n: Any, hotkey: Any) -> dict[str, Any]:
    response = n.client.get(n.url + "/admission/" + hotkey.ss58)
    precondition(response.status_code == 200, response.text)
    return response.json()["record"]


def join_with(n: Any, svc: Any, hot: int, cold: int) -> Any:
    request = sign_join(
        svc.HOT[hot],
        svc.COLD[cold],
        run_id=n.manifest.run_id(),
        request_id=sha256_hex(f"evicted-applicant-{hot}-{cold}".encode()),
        expires_beacon=10000,
        policy_hash=n.manifest.network.admission_policy_hash,
        hardware_hint=HardwareHint(device_name="cpu", device_count=1, driver="cpu"),
    )
    return n.client.post(n.url + "/join", content=canonicalize(request.body()))


def evicted_during_trial(n: Any) -> tuple[str, dict[str, Any]]:
    """Applicant 1 opens its trial and its host is reclaimed; the beacon passes the deadline."""
    admission_id = n.join(1).json()["admission_id"]
    n.push(2)
    opened = challenge_of(n, admission_id)
    precondition(opened.status_code == 200, opened.text)
    challenge = opened.json()["body"]
    n.push(challenge["deadline_beacon"] + 1)
    return admission_id, challenge


def test_an_applicant_that_misses_its_trial_deadline_is_not_struck_or_accused(applicants) -> None:
    """Defence: an applicant whose trial expired because it never proved it gets no strike, no
    pending dispute and no ban, and its trial is not recorded as a mismatch or a fraud. The
    suspension that follows the expiry today is the subject of the xfail below."""
    svc, n = applicants
    admission_id, _ = evicted_during_trial(n)

    returned = challenge_of(n, admission_id)
    row = record_of(n, svc.HOT[1])
    db = n.store._db
    outcomes = [
        r[0] for r in db.execute("SELECT outcome FROM admission_trial_results ORDER BY epoch")
    ]
    pending = db.execute("SELECT COUNT(*) FROM admission_pending").fetchone()[0]

    assert returned.status_code < 500
    assert row["strikes"] == 0 and not row["pending_dispute"] and row["state"] != "BANNED"
    assert outcomes and not set(outcomes) & {"MISMATCH", "FRAUD"}
    assert pending == 0
    record(
        {
            "test": "applicant-trial-expired",
            "cheat": 8,
            "name": "evicted-during-trial",
            "seam": "time",
            "intake": {"status": returned.status_code, "endpoint": "GET /join/{id}/challenge"},
            "absent_admission": {k: row[k] for k in ("state", "reason", "strikes")},
            "pending_dispute": row["pending_dispute"],
            "trial_outcome": outcomes,
            "hole": None,
        }
    )


def test_an_honestly_evicted_applicant_is_not_locked_out_by_its_missed_trial(applicants) -> None:
    """Defence: a miner evicted during its trial, not accused of anything, is not left suspended
    for good by the missed deadline."""
    svc, n = applicants
    admission_id, expired = evicted_during_trial(n)

    returned = challenge_of(n, admission_id)
    state = record_of(n, svc.HOT[1])
    elsewhere = join_with(n, svc, hot=2, cold=1)
    record(
        {
            "test": "evicted-applicant-has-no-route-back",
            "cheat": 8,
            "name": "evicted-during-trial",
            "seam": "time",
            "absent_admission": {k: state[k] for k in ("state", "reason", "strikes")},
            "challenge_status": returned.status_code,
            "same_nonce_returned": returned.json().get("body", {}).get("nonce") == expired["nonce"],
            "other_hotkey_same_coldkey": {
                "status": elsewhere.status_code,
                "detail": elsewhere.text,
            },
            "hole": "AC-EVICTED-TRIAL",
        }
    )

    assert state["state"] != "SUSPENDED", state["reason"]
