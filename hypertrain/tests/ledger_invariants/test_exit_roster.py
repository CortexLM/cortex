"""A round must never be frozen by one roster member, one auditor or one retired round (D39).

Package exit_roster. Every test states a defence of the audit and credit path: the audit queue
serves every job that can be served, a member that cannot finish is excluded with a recorded
cause instead of stopping the others, a signed lease ask grants one lease, a round that lost a
member is still settled and the next round credited. The world is the v2 service of
`tests/e2e/test_rollback_service_v2.py` on a private copy of its fixtures
(`support_exit_roster`); the members that matter to a scenario are real (the tiny CPU island
runs) and the others are forged commits that are well formed.

Holes are strict xfails that assert the desired defence after a control that reaches the point
under test (`X.control`, which a strict xfail cannot swallow). The Lens ids are candidates until a
test says otherwise; every xfail below was run without its marker and failed where its reason
says. The FINALIZE_ORDER safety half of CRD-04 is pinned at the ledger by test_credit.py.
"""

from __future__ import annotations

from collections.abc import Iterator
from pathlib import Path

import pytest

from hypertrain.aggregator.core import FinalityError
from hypertrain.challenge.admission_store import AdmissionError
from hypertrain.miner.admission import sign_rotation
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.messages_v2 import RotateRequest

from . import support_exit_roster as X


def complete(rnd: X.Round, job_id: str) -> None:
    """What the lease logic reads of a finished audit: its job no longer runs, so its auditor
    holds no lease. complete_audit_v2 also writes the verdict and counts the round down; a forged
    member has no verdict to sign, and the tests that use this do not look at either."""
    with rnd.store._tx():
        rnd.store._db.execute("UPDATE audit_leases_v2 SET state='completed' WHERE id=?", (job_id,))


# Worlds of one round of four members, built on the routes (about 0.3 s each)


@pytest.fixture
def silent(tmp_path: Path) -> Iterator[X.Round]:
    """Round 0 of five members: four committed and published their leaves, the fifth accepted
    its assignment and went quiet. Four members are what an aggregation needs."""
    with X.activated_network(tmp_path) as network:
        rnd = X.open_round(network, 0, [0, 1, 2, 3, 4])
        rnd.quiet = 4
        for index in (0, 1, 2, 3):
            X.join_forged(rnd, index)
        X.control(X.accept(rnd, rnd.quiet).status_code == 200, "the quiet member accepts")
        yield rnd


@pytest.fixture
def queue(tmp_path: Path) -> Iterator[X.Round]:
    """Round 0 of four members who all committed and published, audits scheduled at d_audit:
    four queued jobs, nobody has leased any."""
    with X.activated_network(tmp_path) as network:
        rnd = X.open_round(network, 0, [0, 1, 2, 3])
        for index in rnd.members:
            X.join_forged(rnd, index)
        response = X.schedule(rnd)
        X.control(response.status_code == 200, response.text)
        yield rnd


# A roster member that never commits (FIN-21, A2W-20, LV-06)


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="FIN-21+A2W-20+LV-06: a roster member that accepted and never commits stops the audit "
    "scheduling of the whole round: schedule_audits_v2 raises 409 'missing accepted commit' on "
    "its empty slot, no job exists for the members that did commit and nothing excludes the "
    "quiet one",
)
def test_a_member_that_never_commits_does_not_stop_the_audits_of_the_others(
    silent: X.Round,
) -> None:
    rnd = silent
    committed = [i for i in rnd.members if i != rnd.quiet]
    X.control(
        [rnd.has_record("commit", rnd.identity(i)) for i in rnd.members] == [True] * 4 + [False]
        and all(rnd.has_record("leaves", rnd.identity(i)) for i in committed),
        "four commits with their leaves are accepted, the fifth member has none",
    )
    X.control(rnd.lease_rows() == [], "no job exists before the operator schedules the audits")

    response = X.schedule(rnd)

    assert response.status_code == 200, response.text
    assert {row["hotkey"] for row in rnd.lease_rows()} == {rnd.hotkey(i) for i in committed}
    assert set(rnd.excluded()) == {rnd.hotkey(rnd.quiet)}
    assert rnd.finality()["unresolved_audits"] == len(committed)


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="FIN-21+A2W-20+LV-06: no bound holds for a member that stays quiet: past the final "
    "beacon it has neither a job in a terminal state nor an exclusion record, so the round "
    "keeps its audit counter above zero for ever",
)
def test_every_roster_member_is_in_a_terminal_audit_state_once_the_horizon_has_passed(
    silent: X.Round,
) -> None:
    rnd = silent
    X.control(
        [rnd.has_record("commit", rnd.identity(i)) for i in rnd.members] == [True] * 4 + [False],
        "four members committed, the fifth did not",
    )
    X.schedule(rnd)  # the operator's call, at d_audit

    rnd.network.push(rnd.absolute + 1)

    terminal = {
        row["hotkey"] for row in rnd.lease_rows() if row["state"] in ("completed", "exhausted")
    } | set(rnd.excluded())
    assert terminal == {rnd.hotkey(i) for i in rnd.members}
    assert rnd.finality()["unresolved_audits"] == 0


# Eligibility is judged once, at accept (AD-07)


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="AD-07: eligibility is checked at accept only: a member whose work screen lapsed "
    "after its accept still commits, so it joins the roster of an audit it can no longer be "
    "aggregated in",
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


# A job that cannot be built (A2W-18) and the order of the queue


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


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="FIN-22+A2W-18: lease_v2 picks the head of the queue before it reads that job's "
    "prerequisites, so a job without its leaves record answers every ask with 409 'missing "
    "accepted leaves' and the three jobs behind it are never leased",
)
def test_a_job_that_cannot_be_built_does_not_starve_the_queue_behind_it(queue: X.Round) -> None:
    rnd = queue
    head, *rest = rnd.queue_order()
    auditor = X.service().AUDITORS[0]
    with X.without_record(rnd, "leaves", rnd.identity(head)):
        X.control(
            all(rnd.has_record("leaves", rnd.identity(i)) for i in rest)
            and [row["state"] for row in rnd.lease_rows()] == ["queued"] * 4,
            "only the head job lacks a prerequisite; all four jobs wait",
        )

        response = X.post_lease(rnd, X.lease_ask(rnd, auditor, 1))

        assert response.status_code == 200, response.text
        assert response.json()["body"]["start_state"]["hotkey"] in {rnd.hotkey(i) for i in rest}


# A lease that lapses (A2W-19, LV-05)


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="LV-05+A2W-19: one lapsed lease writes a permanent auditor-disabled record: 403 "
    "'silent auditor disabled pending review' answers every later ask of that auditor, however "
    "late, and no route lifts it; with two pinned auditors two lapses end all auditing",
)
def test_an_auditor_whose_lease_lapsed_is_not_disabled_for_ever(queue: X.Round) -> None:
    rnd = queue
    auditor = X.service().AUDITORS[0]
    leased = X.post_lease(rnd, X.lease_ask(rnd, auditor, 1))
    X.control(leased.status_code == 200, "the auditor holds a lease")
    expires = rnd.lease_rows()[0]["expires"]
    rnd.network.push(expires)
    lapsed = rnd.lease_rows()[0]
    X.control(
        (lapsed["state"], lapsed["attempts"]) == ("queued", 1),
        "the lease lapsed once and its job is back in the queue",
    )

    rnd.network.push(expires + 1000)  # a thousand rounds later

    response = X.post_lease(rnd, X.lease_ask(rnd, auditor, 2))
    assert response.status_code != 403, response.text


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="LV-05+A2W-19: an auditor is disabled for a lapse it did not cause: the island job of "
    "a committed member is staged only when somebody fetches GET .../job/{hotkey}, execute "
    "answers 409 'missing accepted island-job' for a member that never did, and the lease that "
    "could not be used runs out and disables the auditor for ever",
)
def test_an_auditor_is_not_disabled_for_a_job_the_master_cannot_execute(queue: X.Round) -> None:
    rnd = queue
    auditor = X.service().AUDITORS[0]
    head = rnd.queue_order()[0]
    leased = X.post_lease(rnd, X.lease_ask(rnd, auditor, 1))
    X.control(leased.status_code == 200, "the auditor holds a lease")
    job = leased.json()["body"]
    executed = rnd.post(
        f"/worker/jobs/{job['job_id']}/execute",
        rnd.network.signed(
            auditor,
            "Receipt",
            {"w": rnd.w, "commit_hash": job["lease_nonce"], "received_round": rnd.network.now},
        ),
    )
    X.control(
        executed.status_code == 409
        and "island-job" in executed.text
        and not rnd.has_record("island-job", rnd.identity(head))
        and not rnd.disabled(auditor),
        "the master cannot execute the job: its member never fetched the island job",
    )

    rnd.network.push(job["lease_expires"])

    assert not rnd.disabled(auditor)


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
    "to the auditor once its previous one is over",
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


# The horizon of the audit jobs (A2W-20 held half, FIN-22)


def test_the_horizon_exhausts_every_unleased_job_and_excludes_its_member_as_infrastructure(
    queue: X.Round,
) -> None:
    """A2W-20, FIN-22 (held half): a job nobody leased is exhausted when the beacon reaches its
    absolute round, not before; its member is excluded as INFRASTRUCTURE (never as a failed audit)
    and the audit counter of the round falls to zero. No auditor was at fault."""
    rnd = queue
    auditors = X.service().AUDITORS
    rnd.network.push(rnd.absolute - 1)
    assert [row["state"] for row in rnd.lease_rows()] == ["queued"] * 4
    assert rnd.finality()["unresolved_audits"] == 4 and rnd.excluded() == {}

    rnd.network.push(rnd.absolute)

    assert [(row["state"], row["result"]) for row in rnd.lease_rows()] == [
        ("exhausted", "AUDIT_INFRASTRUCTURE")
    ] * 4
    assert rnd.excluded() == {rnd.hotkey(i): "INFRASTRUCTURE" for i in rnd.members}
    assert rnd.finality()["unresolved_audits"] == 0
    assert not any(rnd.disabled(auditor) for auditor in auditors)


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="FIN-22: a round whose every audit job was exhausted has no exit: the opening of the "
    "next round reads the applied state of this one, which never exists, so the master answers "
    "409 'missing accepted applied: 0' and no operation voids or skips the round [candidate]",
)
def test_a_round_whose_audits_all_exhausted_can_be_left_behind(queue: X.Round) -> None:
    rnd = queue
    rnd.network.push(rnd.absolute)
    status = rnd.finality()
    X.control(
        (status["unresolved_audits"], status["unresolved_disputes"]) == (0, 0)
        and set(rnd.excluded()) == {rnd.hotkey(i) for i in rnd.members}
        and not rnd.has_record("applied", "0"),
        "nothing is outstanding in round 0: the horizon settled every member, none was applied",
    )

    response = X.next_opening(rnd, rnd.members)

    assert response.status_code == 200, response.text


# A beacon outage (FIN-24)


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="FIN-24: a lease that expires during a beacon outage is penalised exactly like a "
    "silent auditor: the first push after the outage writes the permanent auditor-disabled "
    "record, although every ask the auditor could have made was refused 503 meanwhile [candidate]",
)
def test_a_beacon_outage_does_not_disable_an_auditor(queue: X.Round) -> None:
    rnd = queue
    auditor = X.service().AUDITORS[0]
    leased = X.post_lease(rnd, X.lease_ask(rnd, auditor, 1))
    X.control(leased.status_code == 200, "the auditor holds a lease")
    expires = rnd.lease_rows()[0]["expires"]
    X.outage(rnd, expires + 10)
    paused = X.post_lease(rnd, X.lease_ask(rnd, X.service().AUDITORS[1], 2))
    X.control(
        paused.status_code == 503 and not rnd.disabled(auditor),
        "during the outage the master refuses every ask and the auditor is not disabled",
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
    "stake stays locked; the expiry leg and the contest leg of the invariant are pinned by "
    "test_admission_hardening_v2 and test_dispute_exit_v2 [candidate]",
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


# A two-round run: a member that is exhausted out of round 0, then round 1 (module narrative)


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


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="A2W-21+LCK-18: verified_inputs_v2 raises 409 'missing accepted delta' for a "
    "member whose audit matched but whose delta never arrived, so one absent artifact stops the "
    "aggregation of the five audited members instead of excluding that member with a recorded "
    "cause",
)
def test_a_member_without_an_accepted_delta_is_excluded_and_the_round_goes_on(
    chain: X.Chain, exhausted: X.Round
) -> None:
    rnd = exhausted
    member, others = chain.survivors[1], [chain.survivors[0], *chain.survivors[2:]]
    X.control(
        not rnd.finality()["applied"]
        and rnd.excluded() == {rnd.hotkey(rnd.quiet): "INFRASTRUCTURE"}
        and all(rnd.has_record("verdict", rnd.identity(i)) for i in chain.survivors),
        "five members passed their audit, the sixth was exhausted, nothing is applied yet",
    )
    with X.without_record(rnd, "delta", rnd.identity(member)):
        X.control(
            all(rnd.has_record("delta", rnd.identity(i)) for i in others),
            "only that member's delta is missing",
        )

        response = X.aggregate(rnd)

        assert response.status_code == 200, response.text
        assert rnd.hotkey(member) in rnd.excluded()


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="AD-07+A2W-21+LCK-18: verified_inputs_v2 raises 403 'no longer eligible' for a member "
    "whose work screen lapsed after its accept, so the aggregation of the four other audited "
    "members stops instead of excluding that member with a recorded cause",
)
def test_a_member_whose_screen_lapsed_is_excluded_and_the_round_goes_on(
    chain: X.Chain, exhausted: X.Round
) -> None:
    rnd = exhausted
    member = chain.survivors[2]
    X.control(
        not rnd.finality()["applied"] and X.eligible(rnd, member) is True,
        "nothing is applied yet and the member is eligible",
    )
    with X.screen_expired(rnd, member):
        X.control(X.eligible(rnd, member) is False, "its work screen lapsed")

        response = X.aggregate(rnd)

        assert response.status_code == 200, response.text
        assert rnd.hotkey(member) in rnd.excluded()


def test_an_excluded_member_is_left_out_of_the_tape_and_the_others_are_applied(
    chain: X.Chain, exhausted: X.Round
) -> None:
    """A2W-21 (held half), FIN-19: a roster member with an exclusion record is skipped by the
    aggregation, which applies the model state of the members that were audited; the quiet
    member's job was exhausted at the horizon, not served."""
    rnd, theta = chain.applied0()  # the aggregation under test
    quiet = rnd.hotkey(rnd.quiet)
    assert [row["state"] for row in rnd.lease_rows() if row["hotkey"] == quiet] == ["exhausted"]
    assert chain.tape0_inputs == sorted(rnd.hotkey(i) for i in chain.survivors)
    assert quiet not in chain.tape0_inputs
    status = rnd.finality()
    assert (status["applied"], status["unresolved_audits"]) == (True, 0)
    assert status["state_hash"] == theta


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="FIN-09: aggregate_v2 applies a round that the lease horizon marked EXCLUDED and keeps "
    "rollback_complete, so a round whose model state was applied counts as closed by exclusion "
    "although no rollback record exists",
)
def test_no_applied_round_is_closed_by_exclusion_without_a_rollback_record(
    applied0: tuple[X.Round, str],
) -> None:
    rnd, _ = applied0
    status = rnd.finality()
    X.control(
        status["applied"]
        and status["unresolved_audits"] == 0
        and rnd.excluded() == {rnd.hotkey(rnd.quiet): "INFRASTRUCTURE"}
        and not rnd.has_record("rollback", "0"),
        "applied, nothing outstanding, one member excluded by the horizon, no rollback",
    )

    assert not (status["disposition"] == "EXCLUDED" and status["rollback_complete"]), status


@pytest.mark.xfail(
    strict=True,
    raises=FinalityError,
    reason="FIN-19+A2W-22: one exhausted job marks the whole round EXCLUDED and finality_v2.settle "
    "refuses an EXCLUDED round, so the five members whose audits matched are never settled or "
    "paid for it",
)
def test_the_survivors_of_a_round_with_an_exhausted_member_are_settled(
    chain: X.Chain, applied0: tuple[X.Round, str]
) -> None:
    rnd, theta = applied0
    request = X.finalize_request(rnd, chain.survivors, theta)
    verified = rnd.store.verified_inputs_v2(rnd.run_id, 0)
    X.control(
        sorted(x.roster.hotkey for x in verified) == request["json"]["body"]["included"],
        "the Finalize names exactly the members the master verified",
    )

    response = rnd.network.client.post(**request)

    assert response.status_code == 200, response.text
    assert rnd.finality()["disposition"] == "SETTLED"


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="CRD-04+FIN-08+A2W-22: reward_finalize credits round w only when w-1 is credited and "
    "a round retired by exhaustion never is, so the coordinator's valid Finalize for the next "
    "round is refused 409 FINALIZE_ORDER and no later round can ever be paid",
)
def test_a_round_after_a_round_retired_by_exhaustion_is_credited(
    chain: X.Chain, applied1: tuple[X.Round, str]
) -> None:
    r1, theta = applied1
    r0 = chain.r0
    status = r1.finality()
    verified = r1.store.verified_inputs_v2(r1.run_id, 1)
    X.control(
        r0.finality()["unresolved_audits"] == 0
        and bool(r0.excluded())
        and (status["applied"], status["unresolved_audits"], status["unresolved_disputes"])
        == (True, 0, 0)
        and sorted(x.roster.hotkey for x in verified) == sorted(r1.hotkey(i) for i in r1.members),
        "round 0 has nothing outstanding and lost a member to the horizon; round 1 is complete: "
        "four audited members, applied, all of them verified by the master",
    )

    response = r1.network.client.post(**X.finalize_request(r1, r1.members, theta))

    assert response.status_code == 200, response.text
    assert r1.finality()["disposition"] == "SETTLED"


@pytest.mark.xfail(
    strict=True,
    raises=AdmissionError,
    reason="CRD-13+LCK-18: RotateRequest has no roster check and verified_inputs_v2 asks "
    "admission.status for the hotkey the round was opened with, so a member that rotates after "
    "its audit stalls aggregate_v2 and finalize_v2 for everybody with AdmissionError "
    "UNKNOWN_HOTKEY",
)
def test_a_hotkey_rotated_after_the_audits_does_not_stall_the_round(
    applied1: tuple[X.Round, str],
) -> None:
    rnd, _ = applied1
    member, others = rnd.members[0], rnd.members[1:]
    store = rnd.store
    X.control(
        len(store.verified_inputs_v2(rnd.run_id, 1)) == 4,
        "all four members are verified before the rotation",
    )

    rotation = sign_rotation(
        X.cold(member),
        rnd.run_id,
        RotateRequest(
            hotkey=rnd.hotkey(member), new_hotkey=X.hot(X.SPARE).ss58, operation_id="df" * 32
        ),
        10_000,
    )
    rotated = rnd.network.client.post(
        rnd.network.url + "/admission/" + rnd.hotkey(member) + "/rotate", json=rotation
    )

    if rotated.status_code == 200:  # accepted: the round must still resolve for the others
        inputs = store.verified_inputs_v2(rnd.run_id, 1)
        assert {x.roster.hotkey for x in inputs} >= {rnd.hotkey(i) for i in others}
    else:  # refused: the member keeps the hotkey the round was opened with
        assert 400 <= rotated.status_code < 500, rotated.text
        assert len(store.verified_inputs_v2(rnd.run_id, 1)) == 4
