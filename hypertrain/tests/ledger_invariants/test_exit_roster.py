"""A round must never be frozen by one roster member, one auditor or one retired round (D39).

Package exit_roster. Every test states a defence of the audit and credit path: the audit queue
serves every job that can be served, a member that cannot finish is left out of its round with a
recorded cause instead of stopping the others, a signed lease ask grants one lease, a round that
lost a member is still settled and the next round credited. The world is the v2 service of
`tests/e2e/test_rollback_service_v2.py` on a private copy of its fixtures
(`support_exit_roster`); the members that matter to a scenario are real (the tiny CPU island
runs) and the others are forged commits that are well formed.

v3 (audit-integrity ADR 0011, e0969448) closed most of what this package first found as holes:
a member that never commits (AUD-06), a job without leaves, a lapsed lease (AUD-01 cooldown), an
exhausted job (second window, then its miner leaves the round and the round is not voided), a
delta that never arrives. tests/challenge/test_no_show_v2.py, test_audit_liveness_v2.py and
tests/e2e/test_no_show_resume_v2.py pin those one by one; the mutants of this package that revert
them are killed by those tests (mutants_exit_roster). What is left here is what they do not
reach: the same lot seen through the routes and through two real rounds, and the holes that no
lot has landed yet. What the exit owes the member who caused it (the sanction of a no-show, a
loss on the master's side, a rotation or a contest while the round is open, a convicted member)
is test_exit_roster_2.py.

Holes are strict xfails that assert the desired defence after a control that reaches the point
under test (`X.control`, which a strict xfail cannot swallow). Each reason ends with the lot that
will flip it: `[fix-lot: <tag>]`. The Lens ids are candidates until a test says otherwise; every
xfail below was run without its marker and failed where its reason says. The FINALIZE_ORDER
safety half of CRD-04 is pinned at the ledger by test_credit.py, the finalize of a round after
its evidence expired by test_expiry.py.
"""

from __future__ import annotations

from collections.abc import Iterator
from pathlib import Path

import pytest

from hypertrain.protocol.hashing import sha256_hex

from . import support_exit_roster as X


def complete(rnd: X.Round, job_id: str) -> None:
    """What the lease logic reads of a finished audit: its job no longer runs, so its auditor
    holds no lease. complete_audit_v2 also writes the verdict and counts the round down; a forged
    member has no verdict to sign, and the tests that use this do not look at either."""
    with rnd.store._tx():
        rnd.store._db.execute("UPDATE audit_leases_v2 SET state='completed' WHERE id=?", (job_id,))


def deliver_deltas(rnd: X.Round, members: list[int]) -> None:
    """The delta manifest of each member, stored as an accepted upload is. A member that committed
    and delivered nothing by d_upload has left the round (ADR 0011 Part 8), so a test about the
    audit windows of members that did deliver needs these records."""
    for index in members:
        commit = rnd.record("commit", rnd.identity(index))["body"]
        manifest = {
            "w": rnd.w,
            "hotkey": rnd.hotkey(index),
            "delta_hash": commit["delta_hash"],
            "size": commit["delta_bytes"],
            "uri": "sha256:" + commit["delta_hash"],
            "format": "ht-sparse-v1",
            "chunks": [{"off": 0, "len": commit["delta_bytes"], "sha256": commit["delta_hash"]}],
            "grant_hash": "0" * 64,
            "master_acceptance_hash": "0" * 64,
        }
        with rnd.store._tx():
            rnd.store._put_record_v2(
                rnd.run_id,
                "delta",
                rnd.identity(index),
                rnd.network.signed(X.hot(index), "DeltaManifestV2", manifest),
            )


# Worlds of one round of four members, built on the routes (about 0.3 s each)


@pytest.fixture
def silent(tmp_path: Path) -> Iterator[X.Round]:
    with X.silent_round(tmp_path) as rnd:
        yield rnd


@pytest.fixture
def queue(tmp_path: Path) -> Iterator[X.Round]:
    with X.queued_round(tmp_path) as rnd:
        yield rnd


# A roster member that never commits, and the horizon (FIN-21, A2W-20, LV-06)


def test_every_roster_member_is_in_a_terminal_audit_state_once_the_horizon_has_passed(
    silent: X.Round,
) -> None:
    """FIN-21, A2W-20, LV-06 (closed by AUD-06 and AUD-01): the member that never committed leaves
    the round when the audits are scheduled (NO_SHOW); the four jobs nobody serves get a second
    window and their members leave the round at its end (INFRASTRUCTURE). The audit counter of
    the round falls to zero. No survivor can carry what is left, so the exit reaper retires the
    round in the same push (ADR 0011 Part 8, decision 8) instead of leaving it pending."""
    rnd = silent
    committed = [i for i in rnd.members if i != rnd.quiet]
    X.control(
        [rnd.has_record("commit", rnd.identity(i)) for i in rnd.members] == [True] * 4 + [False],
        "four members committed, the fifth did not",
    )
    X.schedule(rnd)  # the operator's call, at d_audit
    deliver_deltas(rnd, committed)  # they are left out for the audit nobody serves, not for silence

    X.run_out_audit_windows(rnd)

    terminal = {
        row["hotkey"] for row in rnd.lease_rows() if row["state"] in ("completed", "exhausted")
    } | set(rnd.excluded())
    assert terminal == {rnd.hotkey(i) for i in rnd.members}
    assert rnd.excluded() == {
        rnd.hotkey(rnd.quiet): "NO_SHOW",
        **{rnd.hotkey(i): "INFRASTRUCTURE" for i in committed},
    }
    status = rnd.finality()
    assert (status["unresolved_audits"], status["disposition"]) == (0, "RETIRED")


# Eligibility is judged once, at accept (AD-07)


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="AD-07: eligibility is checked at accept only: a member whose work screen lapsed "
    "after its accept still commits, so it joins the roster of an audit it can no longer be "
    "aggregated in [fix-lot: none (no design yet)]",
)
def test_a_member_that_lost_eligibility_after_accept_cannot_commit(tmp_path: Path) -> None:
    with X.activated_network(tmp_path) as network:
        rnd = X.open_round(network, 0, [0, 1, 2, 3])
        for index in rnd.members:
            X.control(X.accept(rnd, index).status_code == 200, "every member accepts")
        member = 3
        first = X.post_commit(rnd, 0, X.forged_commit(rnd, 0))
        X.control(first.status_code == 200, "an eligible member commits in this window")
        with X.screen_expired(rnd, member):
            X.control(
                X.eligible(rnd, member) is False and rnd.has_record("accept", rnd.identity(member)),
                "the member was accepted and is no longer eligible",
            )

            response = X.post_commit(rnd, member, X.forged_commit(rnd, member))

            assert response.status_code >= 400, response.text
            assert not rnd.has_record("commit", rnd.identity(member))


# The order of the queue (A2W-18)


def test_the_job_first_in_the_queue_is_the_first_to_be_leased(queue: X.Round) -> None:
    """A2W-18 (control): lease_v2 serves the queue by (created, id) and the lease belongs to
    the auditor that asked; the job of the member it names is running, the other three wait."""
    rnd = queue
    head = rnd.queue_order()[0]
    auditor = X.service().AUDITORS[0]

    response = X.post_lease(rnd, X.lease_ask(rnd, auditor, 1))

    assert response.status_code == 200, response.text
    body = response.json()["body"]
    assert body["start_state"]["hotkey"] == rnd.hotkey(head)
    assert body["auditor_id"] == auditor.ss58
    assert [(row["hotkey"], row["state"]) for row in rnd.lease_rows()] == [
        (rnd.hotkey(i), "running" if i == head else "queued") for i in rnd.queue_order()
    ]


# A signed lease ask is single use (SG-10, A2W-24)


def test_an_ask_repeated_while_its_lease_runs_is_refused_and_grants_nothing(
    queue: X.Round,
) -> None:
    """A2W-24, SG-10 (held half): while the first lease of an auditor runs, the identical ask is
    refused 409 'one live lease per auditor' and the store does not change."""
    rnd = queue
    auditor = X.service().AUDITORS[0]
    ask = X.lease_ask(rnd, auditor, 1)
    assert X.post_lease(rnd, ask).status_code == 200
    world = X.world_of(rnd.network)
    rows, before = rnd.lease_rows(), world.db_digest()

    again = X.post_lease(rnd, ask)

    assert again.status_code == 409, again.text[:200]
    assert rnd.lease_rows() == rows
    assert world.db_digest() == before


def test_the_same_ask_nonce_with_another_body_is_refused_and_grants_nothing(
    queue: X.Round,
) -> None:
    """SG-10, A2W-24 (held half): a lease ask is keyed by (signer, w, commit_hash); a second
    body under the same key is refused 409, also when the auditor holds no lease, and the store
    does not change."""
    rnd = queue
    auditor = X.service().AUDITORS[0]
    first = X.post_lease(rnd, X.lease_ask(rnd, auditor, 1))
    assert first.status_code == 200, first.text[:200]
    complete(rnd, first.json()["body"]["job_id"])
    world = X.world_of(rnd.network)
    rows, before = rnd.lease_rows(), world.db_digest()

    other = X.lease_ask(rnd, auditor, 1, received=rnd.network.now + 1)
    response = X.post_lease(rnd, other)

    assert response.status_code == 409, response.text[:200]
    assert rnd.lease_rows() == rows
    assert world.db_digest() == before


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="SG-10+A2W-24: _authenticated_v2 refuses a different body under a known nonce but "
    "accepts the identical bytes again, so a lease ask observed on the wire grants a new lease "
    "to the auditor once its previous one is over [fix-lot: none (no design yet)]",
)
def test_a_signed_lease_ask_grants_at_most_one_lease(queue: X.Round) -> None:
    rnd = queue
    auditor = X.service().AUDITORS[0]
    ask = X.lease_ask(rnd, auditor, 1)
    first = X.post_lease(rnd, ask)
    X.control(first.status_code == 200, "the first ask is granted")
    complete(rnd, first.json()["body"]["job_id"])
    states = [row["state"] for row in rnd.lease_rows()]
    X.control(
        states.count("queued") == 3 and "running" not in states,
        "the auditor holds no running lease and three jobs wait",
    )

    again = X.post_lease(rnd, ask)

    if again.status_code == 200:  # an idempotent answer must be the lease already granted
        assert again.json()["body"]["job_id"] == first.json()["body"]["job_id"]
    assert [row["auditor"] for row in rnd.lease_rows()].count(auditor.ss58) == 1


# A round with no member left has no exit (FIN-22)


# Closed by audit-integrity-2 (e06cb795, D70): the exit reaper retires the round at the beacon.
def test_a_round_after_a_round_nobody_audited_is_opened_and_credited(queue: X.Round) -> None:
    rnd = queue
    X.run_out_audit_windows(rnd)
    X.walk(rnd, rnd.network.now + 1)  # whatever closes a round on the beacon gets its turn
    status = rnd.finality()
    X.control(
        (status["unresolved_audits"], status["unresolved_disputes"]) == (0, 0)
        and [row["state"] for row in rnd.lease_rows()] == ["exhausted"] * len(rnd.members),
        "nothing is outstanding in round 0: both windows ended and every audit job is given up",
    )

    response, following = X.open_after(rnd, rnd.members)

    assert response.status_code == 200 and following is not None, response.text
    X.train(following, following.members)
    X.control(X.schedule(following).status_code == 200, "the audits of round 1 are scheduled")
    X.audit_queue(following, len(following.members), first_tag=50)
    aggregated = X.aggregate(following)
    X.control(aggregated.status_code == 200, aggregated.text)
    theta = str(aggregated.json()["theta_hash"])
    finalized = following.network.client.post(
        **X.finalize_request(following, following.members, theta)
    )
    assert finalized.status_code == 200, finalized.text
    assert set(following.paid()) == {following.hotkey(i) for i in following.members}


# A beacon outage (FIN-24)


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="FIN-24: a lease that expires during a beacon outage is penalised exactly like a "
    "silent auditor: the first push after the outage books AUDITOR_SILENT, a cooldown and a spent "
    "attempt, although every ask the auditor could have made was refused 503 meanwhile "
    "[candidate] [fix-lot: decision (product choice pending)]",
)
def test_a_beacon_outage_does_not_put_an_auditor_on_cooldown(queue: X.Round) -> None:
    rnd = queue
    auditor = X.service().AUDITORS[0]
    leased = X.post_lease(rnd, X.lease_ask(rnd, auditor, 1))
    X.control(leased.status_code == 200, "the auditor holds a lease")
    expires = rnd.lease_rows()[0]["expires"]
    X.outage(rnd, expires + 10)
    paused = X.post_lease(rnd, X.lease_ask(rnd, X.service().AUDITORS[1], 2))
    X.control(
        paused.status_code == 503 and not rnd.disabled(auditor),
        "during the outage the master refuses every ask and the auditor is not on cooldown",
    )

    rnd.network.push(expires + 10)  # the pusher is back and catches up in one jump

    assert not rnd.disabled(auditor)
    assert rnd.lease_rows()[0]["state"] == "running"


# A suspension must have an exit (LCK-19)


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="LCK-19: a miner suspended for a trial proof that differs from the reference has no "
    "production exit: its pending reference is a derived id that no contest and no settlement "
    "record answers to, so POST /admission/recover ends in 409 UNACCEPTED_SETTLEMENT and the "
    "stake stays locked (ADR 0010 leaves it open); the expiry leg and the contest leg of the "
    "invariant are pinned by test_admission_hardening_v2 and test_dispute_exit_v2 [candidate] "
    "[fix-lot: decision (product choice pending)]",
)
def test_a_miner_suspended_for_a_trial_mismatch_has_a_way_back(tmp_path: Path) -> None:
    with X.activated_network(tmp_path) as network:
        run_id, miner = network.manifest.run_id(), 0
        admission = network.store._services(run_id)[1]
        record = admission.store.record(X.hot(miner).ss58)
        evidence = sha256_hex(b"exit-roster|trial mismatch evidence")
        pending = sha256_hex(
            f"ht-admission-dispute-v2|{run_id}|{record.admission_id}|{evidence}".encode()
        )
        with network.store._tx():  # what Admission.proof writes when the proof differs
            admission.suspend(
                X.hot(miner).ss58,
                reason="PENDING_REPLAY_MISMATCH",
                now=network.now,
                pending_dispute=True,
                dispute_id=pending,
                evidence_hash=evidence,
            )
        escrow = network.store._services(run_id)[0]
        row = network.client.get(network.url + "/admission/" + X.hot(miner).ss58).json()["record"]
        X.control(
            (row["state"], row["pending_dispute"]) == ("SUSPENDED", True)
            and escrow.balances(X.cold(miner).ss58).admission_locked == 1000,
            "the miner is suspended with its reference bound and its stake still locked",
        )

        response = network.client.post(
            network.url + "/admission/recover",
            json=network.signed(
                X.hot(miner),
                "Receipt",
                {"w": 0, "commit_hash": pending, "received_round": network.now},
            ),
        )

        assert response.status_code == 200, response.text
        assert response.json()["record"]["state"] == "PROBATION"


# A two-round run: a member that leaves round 0 at the horizon, then round 1 (module narrative)


@pytest.fixture(scope="module")
def chain(tmp_path_factory: pytest.TempPathFactory) -> Iterator[X.Chain]:
    with X.activated_network(tmp_path_factory.mktemp("chain")) as network:
        yield X.Chain(network)


@pytest.fixture(scope="module")
def exhausted(chain: X.Chain) -> X.Round:
    """Round 0 at the horizon of its audit jobs, before its aggregation."""
    return chain.r0


@pytest.fixture(scope="module")
def applied0(chain: X.Chain, exhausted: X.Round) -> tuple[X.Round, str]:
    """Round 0 aggregated: (round, model state hash)."""
    return chain.applied0()


@pytest.fixture(scope="module")
def applied1(chain: X.Chain, applied0: tuple[X.Round, str]) -> tuple[X.Round, str]:
    """Round 1, opened on four survivors of round 0, audited and aggregated."""
    return chain.applied1()


def test_an_excluded_member_is_left_out_of_the_tape_and_the_others_are_applied(
    chain: X.Chain, exhausted: X.Round
) -> None:
    """A2W-21 (held half), FIN-19: a roster member with an exclusion record is skipped by the
    aggregation, which applies the model state of the members that were audited; the quiet
    member's job ran out at the horizon, it was not served."""
    rnd, theta = chain.applied0()  # the aggregation under test
    quiet = rnd.hotkey(rnd.quiet)
    assert [row["state"] for row in rnd.lease_rows() if row["hotkey"] == quiet] == ["exhausted"]
    assert chain.tape0_inputs == sorted(rnd.hotkey(i) for i in chain.survivors)
    assert quiet not in chain.tape0_inputs
    status = rnd.finality()
    assert (status["applied"], status["unresolved_audits"]) == (True, 0)
    assert status["state_hash"] == theta


def test_no_applied_round_is_closed_by_exclusion_without_a_rollback_record(
    applied0: tuple[X.Round, str],
) -> None:
    """FIN-09 (closed by AUD-01 decision 9): a member that leaves a round at the horizon is an
    exclusion of that member, never a disposition of the round: the applied round stays PENDING
    and open for its settlement instead of counting as closed with no rollback record. The
    quiet member published no leaves, so its job is dropped at the first horizon as a NO_SHOW
    (ADR 0011 Part 8, decision 3)."""
    rnd, _ = applied0
    status = rnd.finality()
    X.control(
        status["applied"]
        and status["unresolved_audits"] == 0
        and rnd.excluded() == {rnd.hotkey(rnd.quiet): "NO_SHOW"}
        and not rnd.has_record("rollback", "0"),
        "applied, nothing outstanding, one member excluded by the horizon, no rollback",
    )

    assert not (status["disposition"] == "EXCLUDED" and status["rollback_complete"]), status
    assert status["disposition"] == "PENDING", status


def test_the_survivors_of_a_round_with_an_exhausted_member_are_settled(
    chain: X.Chain, applied0: tuple[X.Round, str]
) -> None:
    """FIN-19, A2W-22 (closed by AUD-01 decision 9): the Finalize of the five members whose audits
    matched settles the round and pays them, and only them; the member that left at the horizon
    is not in it."""
    rnd, theta = applied0
    request = X.finalize_request(rnd, chain.survivors, theta)
    verified = rnd.store.verified_inputs_v2(rnd.run_id, 0)
    X.control(
        sorted(x.roster.hotkey for x in verified) == request["json"]["body"]["included"],
        "the Finalize names exactly the members the master verified",
    )

    response = chain.settled0()

    assert response.status_code == 200, response.text
    assert rnd.finality()["disposition"] == "SETTLED"
    assert set(rnd.paid()) == {rnd.hotkey(i) for i in chain.survivors}


def test_a_round_after_a_round_with_a_lost_member_is_credited(
    chain: X.Chain, applied1: tuple[X.Round, str]
) -> None:
    """CRD-04, FIN-08, A2W-22 (closed by AUD-01 decision 9): reward_finalize credits round w only
    when w-1 is credited; round 0 lost a member at the horizon and was credited for the others, so
    the coordinator's Finalize of round 1 is accepted."""
    r1, theta = applied1
    r0 = chain.r0
    status = r1.finality()
    verified = r1.store.verified_inputs_v2(r1.run_id, 1)
    X.control(
        r0.finality()["disposition"] == "SETTLED"
        and bool(r0.excluded())
        and (status["applied"], status["unresolved_audits"], status["unresolved_disputes"])
        == (True, 0, 0)
        and sorted(x.roster.hotkey for x in verified) == sorted(r1.hotkey(i) for i in r1.members),
        "round 0 lost a member to the horizon and was settled for the others; round 1 is "
        "complete: four audited members, applied, all of them verified by the master",
    )

    response = r1.network.client.post(**X.finalize_request(r1, r1.members, theta))

    assert response.status_code == 200, response.text
    assert r1.finality()["disposition"] == "SETTLED"
    assert set(r1.paid()) == {r1.hotkey(i) for i in r1.members}
