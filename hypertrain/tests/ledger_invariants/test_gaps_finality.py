"""Gaps of the finality group: what the first wave left open in T-FIN-xx (D39, gap wave).

Each test states a defence the finality path must hold ("refuses X and the state is unchanged",
"credits once", "pays the audited members"). A hole is a strict xfail whose body asserts the
defence after a control that proves the scenario reaches the point; its reason starts with the
finding id and ends with the fix lot that will flip it, `[fix-lot: <tag>]`. A step that makes a
scenario meaningful goes through `support_exit_roster.control`, which raises ScenarioError (not an
AssertionError), so that a broken scenario fails loudly instead of hiding behind the expected
failure.

The audit-queue cards T-FIN-20 and T-FIN-21 (a job that cannot be leased, a commit that expires
before the audit, a member that never commits, a job nobody serves) are closed on the base by the
audit liveness and no-show fixes (ADR 0011 parts 2 and 5) and are pinned by the base's own tests,
which the mutants of mutants_gaps_finality.py name: tests/challenge/test_no_show_v2.py,
tests/challenge/test_audit_liveness_v2.py and tests/e2e/test_no_show_resume_v2.py.
"""

from __future__ import annotations

from collections.abc import Iterator
from pathlib import Path
from types import ModuleType

import pytest

from hypertrain.aggregator.core import TapeError
from hypertrain.challenge.store import ChallengeError
from hypertrain.ledger.escrow_v2 import EscrowError
from hypertrain.protocol.envelope import ExpiredError
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages_v2 import CommitV2

from . import support_exit_roster as X
from . import support_expiry as E
from .support import Real, bare_escrow, service_world
from .support_gaps_finality import (
    ISSUED,
    TapeKit,
    answer,
    commit_over,
    commit_with_delta,
    complete_job,
    delta_post,
    finalize_post,
    leaves_post,
    resealed,
)

# --- T-FIN-01 (FIN-01): only the coordinator's Finalize is read past the signer gate -----------

NOT_AUTHORIZED = (409, "signer is not authorized for message role")


def test_a_finalize_of_the_wrong_role_is_refused_before_the_applied_round_is_looked_up(
    tmp_path: Path,
) -> None:
    with X.activated_network(tmp_path) as network:
        rnd = X.open_round(network, 0, [0, 1, 2, 3])
        world = X.world_of(network)
        X.control(
            not rnd.has_record("applied", "0") and rnd.finality()["applied"] is False,
            "round 0 is opened and nothing is applied: a lookup of its tape has nothing to find",
        )
        before = world.db_digest()

        miner = finalize_post(network, X.hot(0))
        auditor = finalize_post(network, X.service().AUDITORS[0])

        # The signer gate comes before the tape, the finality status and the included set: a key
        # of the wrong role is told so, and the master keeps nothing of the request.
        assert (answer(miner), answer(auditor)) == (NOT_AUTHORIZED, NOT_AUTHORIZED)
        assert world.db_digest() == before
        assert world.count("accepted_v2", "key LIKE '%\"Finalize\"%'") == 0
        # Control: the coordinator passes the gate and meets the first lookup behind it.
        coordinator = finalize_post(network, X.service().COORD)
        assert answer(coordinator) == (409, "missing accepted applied: 0")
        assert world.db_digest() == before


def test_the_roster_guard_answers_a_finalize_for_a_round_that_does_not_exist_before_the_signer(
    tmp_path: Path,
) -> None:
    """FIN-01, refuted by test (ordering only): the Lens expected the signer gate before every
    lookup, but finalize_v2 calls the roster guard first (store.py:8307 through
    `_service_boundary_v2`), which is documented as the guard that runs before any storage or
    arithmetic. Whoever signs, a Finalize for an unopened round is refused 409 and keeps nothing;
    the signer gate is pinned for a round that exists by the test above."""
    with X.activated_network(tmp_path) as network:
        world = X.world_of(network)
        X.control(
            world.count("records_v2", "kind IN ('round','applied','finality')") == 0,
            "no round was opened",
        )
        before = world.db_digest()

        answers = [
            answer(finalize_post(network, signer))
            for signer in (X.hot(0), X.service().AUDITORS[0], X.service().COORD)
        ]

        assert answers == [(409, "missing accepted round: 0")] * 3
        assert world.db_digest() == before


# --- T-FIN-03 (A2W-01, A2W-07): a tape input is the MATCH of its own commit ----------------------

NOT_VERIFIED = "own-assignment full replay not VERIFIED"
OTHER = "22" * 32

REPLAY_FACTS = [
    pytest.param({"result": "MISMATCH"}, id="result-mismatch"),
    pytest.param({"result": "WITHHELD"}, id="result-withheld"),
    pytest.param({"result": "BAD_PROOF"}, id="result-bad-proof"),
    pytest.param({"result": "ASSIGNMENT_VIOLATION"}, id="result-assignment-violation"),
    pytest.param({"audit_mode": "sampled"}, id="audit-mode"),
    pytest.param({"assignment_hash": OTHER}, id="assignment-hash"),
    pytest.param({"leaves_root": OTHER}, id="leaves-root"),
    pytest.param({"delta_hash": OTHER}, id="delta-hash"),
    pytest.param({"final_theta_hash": OTHER}, id="final-theta-hash"),
    pytest.param({"ef_in_hash": OTHER}, id="ef-in-hash"),
    pytest.param({"ef_out_hash": OTHER}, id="ef-out-hash"),
]


def test_an_honest_replay_of_every_commit_builds_the_tape_and_replays_to_it(tmp_path: Path) -> None:
    kit = TapeKit(tmp_path)
    tape = kit.make()
    assert sha256_hex(kit.replay(tape).to_bytes()) == tape.body.out_state
    assert [entry.hotkey for entry in tape.body.allocation.entries] == sorted(
        work.roster.hotkey for work in kit.works
    )


@pytest.mark.parametrize("change", REPLAY_FACTS)
def test_a_tape_is_never_built_over_a_replay_that_is_not_a_full_match_of_its_commit(
    tmp_path: Path, change: dict[str, str]
) -> None:
    kit = TapeKit(tmp_path)
    X.control(kit.make() is not None, "the unchanged inputs build a tape")

    with pytest.raises(TapeError, match=NOT_VERIFIED):
        kit.make(kit.with_replay(**change))


# --- T-FIN-17 (A2W-01): the entitlement of a commit is the server's -----------------------------


def test_a_commit_whose_tokens_are_not_the_servers_entitlement_is_refused_on_both_sides(
    tmp_path: Path,
) -> None:
    kit = TapeKit(tmp_path)
    manifest, samples = kit.manifest, kit.manifest.training.batch_samples()
    commit: CommitV2 = kit.commit()
    entitlement = samples * manifest.training.model.seq_len
    assert commit.tokens == entitlement
    commit.validate_assignment(manifest, samples)  # control: the exact entitlement is accepted

    for tokens in (entitlement - 1, entitlement + 1, 0, 2 * entitlement):
        wrong = commit.model_copy(update={"tokens": tokens})
        with pytest.raises(ValueError, match="tokens differ from server-derived entitlement"):
            wrong.validate_assignment(manifest, samples)


# --- T-FIN-23 (A2W-17, CRD-15): the coordinator's own audit challenge expires too ---------------


def test_a_challenge_that_lapsed_after_the_audit_cannot_block_the_reward(
    li_escrow: ModuleType, real_reward: Real
) -> None:
    setup, _, reward = real_reward
    finalized = real_reward[1].finalized_beacon
    # Control: sealed to expire at the finalize beacon itself, the same reward is issued.
    live = bare_escrow(li_escrow, setup)
    try:
        live.reward_finalize(reward, resealed(li_escrow, real_reward, "challenge", finalized))
    except ExpiredError as error:
        raise X.ScenarioError(
            "an expiry equal to the finalize beacon is already refused"
        ) from error
    except EscrowError as error:
        raise X.ScenarioError(f"the control reward was refused: {error.code}") from error
    X.control(live.balances().issued == ISSUED, "the reward is not issued with a live expiry")
    # One beacon shorter: the challenge was live at every beacon the audit used it at.
    lapsed = bare_escrow(li_escrow, setup)
    lapsed.reward_finalize(reward, resealed(li_escrow, real_reward, "challenge", finalized - 1))
    assert lapsed.balances().issued == ISSUED


# --- T-FIN-12 (A2W-07, CL-38): a committed round cannot be rebound by a later publication -------

BINDINGS = [
    pytest.param(
        {"leaves_root": OTHER}, "leaf root differs from commit", 422, id="another-leaves-root"
    ),
    pytest.param(
        {"delta_hash": OTHER}, "leaf root differs from commit", 422, id="another-delta-hash"
    ),
    pytest.param(
        {"challenge_hash": OTHER},
        "leaf publication must bind accepted commit digest",
        422,
        id="a-commit-nobody-made",
    ),
]


@pytest.mark.parametrize(("proof", "detail", "status"), BINDINGS)
def test_a_leaf_publication_that_does_not_bind_its_commit_is_refused_and_keeps_nothing(
    tmp_path: Path, proof: dict[str, str], detail: str, status: int
) -> None:
    with X.activated_network(tmp_path) as network:
        rnd = X.open_round(network, 0, [0, 1, 2, 3])
        commit = X.join_forged(rnd, 0, leaves=False)
        world = X.world_of(network)
        X.control(
            rnd.has_record("commit", rnd.identity(0))
            and not rnd.has_record("leaves", rnd.identity(0)),
            "the member committed and published nothing",
        )
        before = world.db_digest()

        refused = leaves_post(rnd, 0, commit, proof=proof)

        assert (refused.status_code, refused.json()["detail"]) == (status, detail)
        assert world.db_digest() == before
        assert not rnd.has_record("leaves", rnd.identity(0))
        # The refusal did not lock the member out: the publication that binds the commit is taken.
        assert leaves_post(rnd, 0, commit).status_code == 200
        assert rnd.has_record("leaves", rnd.identity(0))


@pytest.mark.parametrize("kind", ["one-leaf-too-few", "a-leaf-of-another-round"])
def test_a_leaf_object_that_contradicts_its_commit_is_refused_and_keeps_nothing(
    tmp_path: Path, kind: str
) -> None:
    with X.activated_network(tmp_path) as network:
        rnd = X.open_round(network, 0, [0, 1, 2, 3])
        leaves = X.forged_leaves(rnd, 0)
        if kind == "one-leaf-too-few":
            leaves = leaves[:-1]
        else:
            leaves = [leaves[0].model_copy(update={"w": rnd.w + 1}), *leaves[1:]]
        commit = commit_over(rnd, 0, leaves)
        # The object hashes to the commit's root, so only a check of its content can refuse it.
        X.control(
            X.accept(rnd, 0).status_code == 200
            and X.post_commit(rnd, 0, commit).status_code == 200,
            "intake takes the commit: it does not open the leaves",
        )
        world = X.world_of(network)
        before = world.db_digest()

        refused = leaves_post(rnd, 0, commit, leaves=leaves)

        assert (refused.status_code, refused.json()["detail"]) == (
            422,
            "leaf object does not match accepted commit",
        )
        assert world.db_digest() == before
        assert not rnd.has_record("leaves", rnd.identity(0))


@pytest.mark.parametrize("difference", [-1, 1])
def test_a_commit_that_declares_another_entitlement_than_the_servers_is_refused_at_intake(
    tmp_path: Path, difference: int
) -> None:
    with X.activated_network(tmp_path) as network:
        rnd = X.open_round(network, 0, [0, 1, 2, 3])
        X.control(X.accept(rnd, 0).status_code == 200, "the member accepted its assignment")
        honest = X.forged_commit(rnd, 0)
        wrong = honest.model_copy(update={"tokens": honest.tokens + difference})
        world = X.world_of(network)
        before = world.db_digest()

        # The store is called directly: the app has no handler for the plain ValueError the
        # entitlement check raises, so a store that lets it through answers 500 over HTTP, and one
        # that maps it answers a ChallengeError 422. Either refuses the commit and keeps nothing.
        with pytest.raises((ValueError, ChallengeError), match="tokens differ from server-derived"):
            rnd.store.training_v2(
                rnd.run_id, "commit", canonicalize(network.signed(X.hot(0), "CommitV2", wrong))
            )

        assert world.db_digest() == before
        assert not rnd.has_record("commit", rnd.identity(0))
        assert X.post_commit(rnd, 0, honest).status_code == 200  # control: the exact entitlement


def test_the_commit_window_closes_for_a_late_member_and_the_upload_window_for_a_late_delta(
    tmp_path: Path,
) -> None:
    on_time, late = b"a delta delivered in its window", b"a delta delivered after d_upload"
    with X.activated_network(tmp_path) as network:
        rnd = X.open_round(network, 0, [0, 1, 2, 3])
        for index, delta in ((0, on_time), (1, late)):
            commit = commit_with_delta(rnd, index, delta)
            X.control(
                X.accept(rnd, index).status_code == 200
                and X.post_commit(rnd, index, commit).status_code == 200
                and leaves_post(rnd, index, commit).status_code == 200,
                "the member accepted, committed to its delta and published its leaves",
            )
        X.control(X.accept(rnd, 2).status_code == 200, "a member accepted and has not committed")
        X.control(
            delta_post(rnd, 0, on_time).status_code == 200,
            "a delta posted inside its window is taken",
        )
        world = X.world_of(network)
        network.push(rnd.opening.d_commit)
        before = world.db_digest()

        accepted_late = X.post_commit(rnd, 2, X.forged_commit(rnd, 2))
        unaccepted_late = X.post_commit(rnd, 3, X.forged_commit(rnd, 3))

        for refused in (accepted_late, unaccepted_late):
            assert (refused.status_code, refused.json()["detail"]) == (
                409,
                "commit window closed",
            )
        assert world.db_digest() == before
        assert not rnd.has_record("commit", rnd.identity(2))

        network.push(rnd.opening.d_upload)

        too_late = delta_post(rnd, 1, late)

        assert (too_late.status_code, too_late.json()["detail"]) == (409, "upload window closed")
        # Only the delta posted inside its window is a stored delta of the round.
        assert not rnd.has_record("delta", rnd.identity(1))
        assert rnd.has_record("delta", rnd.identity(0))
        assert world.count("records_v2", "kind='delta'") == 1


# --- T-FIN-14 (FIN-12, FIN-13): a round opens on the applied state of its predecessor ----------


def test_a_round_does_not_open_before_its_predecessor_was_applied(tmp_path: Path) -> None:
    with X.activated_network(tmp_path) as network:
        rnd = X.open_round(network, 0, [0, 1, 2, 3])
        world = X.world_of(network)
        X.control(
            not rnd.has_record("applied", "0") and not rnd.finality()["applied"],
            "round 0 is opened and no tape was applied for it",
        )
        before = world.db_digest()

        response = X.next_opening(rnd, rnd.members)

        assert (response.status_code, response.json()["detail"]) == (
            409,
            "missing accepted applied: 0",
        )
        assert not rnd.has_record("round", "1") and not rnd.has_record("finality", "1")
        assert world.db_digest() == before


# --- the audit queue of a cheap world: four members committed, four jobs queued ----------------


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


# --- T-FIN-09 (A2W-04, A2W-11): a round whose audits are not done has no applied tape ------------


def test_a_finalize_for_a_round_whose_audits_are_not_done_is_refused_for_its_missing_tape(
    queue: X.Round,
) -> None:
    rnd = queue
    world = X.world_of(rnd.network)
    escrow = rnd.store._services(rnd.run_id)[0]
    balances = escrow.balances()
    status = rnd.finality()
    X.control(
        (status["applied"], status["unresolved_audits"]) == (False, 4)
        and [row["state"] for row in rnd.lease_rows()] == ["queued"] * 4,
        "four audit jobs are queued, none was served, no tape was applied",
    )
    refusal_of_aggregate = X.aggregate(rnd)
    X.control(
        refusal_of_aggregate.status_code == 409,
        f"the round cannot be aggregated yet: {refusal_of_aggregate.text}",
    )
    before = world.db_digest()

    response = rnd.network.client.post(
        **X.finalize_request(rnd, rnd.members, sha256_hex(b"a theta nobody applied"))
    )

    assert (response.status_code, response.json()["detail"]) == (
        409,
        "missing accepted applied: 0",
    )
    assert world.db_digest() == before
    assert escrow.balances() == balances
    assert rnd.finality() == status


# --- T-FIN-26 (FIN-22) [D70]: a round with a member that never committed can be left behind ------


def test_a_round_with_a_member_that_never_committed_can_be_left_behind(tmp_path: Path) -> None:
    """FIN-22 (D70): a round with a member that never committed leaves the round without a job for
    anybody (three survivors cannot carry it); the exit retires it, and the opening of the next
    round is not blocked by the applied state of this one, which never exists (409 'missing
    accepted applied: 0')."""
    with X.activated_network(tmp_path) as network:
        rnd = X.open_round(network, 0, [0, 1, 2, 3])
        rnd.quiet = 3
        for index in (0, 1, 2):
            X.join_forged(rnd, index)
        X.control(X.accept(rnd, rnd.quiet).status_code == 200, "the fourth member accepts")
        refused = X.schedule(rnd)  # the operator's call at d_audit
        X.control(
            refused.status_code == 409 and not rnd.lease_rows(),
            f"the three survivors cannot carry the round: no job, {refused.text}",
        )
        network.push(rnd.absolute)
        status = rnd.finality()
        X.control(
            (
                status["disposition"] == "PENDING"
                and status["applied"] is False
                and not rnd.has_record("applied", "0")
            )
            or (
                status["disposition"] == "RETIRED" and rnd.record("applied", "0")["kind"] == "null"
            ),
            "round 0 was not applied by a tape at the horizon: it is pending, or retired as a null "
            "round",
        )

        # open_after builds the opening on the state round 0 applied, when it applied one.
        response, _ = X.open_after(rnd, rnd.members)

        assert response.status_code == 200, response.text


# --- T-FIN-30 (A2W-16) [D-5]: a round the store opened must be auditable --------------------------

WIDE = (1, 2, 10, 11, 12, 150)  # d_final - d_audit = 139: wider than one audit lease (100)


def test_a_round_wider_than_one_lease_is_refused_at_opening_or_stays_auditable(
    tmp_path: Path,
) -> None:
    with service_world(tmp_path) as world:
        E.activate(world, 4)
        rnd, opened = E.open_round(world, [0, 1, 2, 3], WIDE)
        if not opened.is_success:
            # The Lens' remedy: the opening that can never be audited is refused, and kept nowhere.
            assert 400 <= opened.status_code < 500, opened.text
            assert world.count("records_v2", "kind IN ('round','finality')") == 0
            return
        E.commit_all(rnd)
        E.deliver_deltas(rnd)
        world.network.push(rnd.d_audit)
        scheduled = E.audits_request(world)
        E.control(scheduled.status_code == 200, f"audits: {scheduled.text}")
        world.network.push(E.last_audit_beacon(world))

        leased = E.lease_request(world, E.service().AUDITORS[0])

        assert leased.status_code == 200, leased.text


# --- T-FIN-19 (FIN-19, A2W-22, CL-12): the survivors of an exhausted round are paid ---------------


@pytest.fixture(scope="module")
def chain(tmp_path_factory: pytest.TempPathFactory) -> Iterator[X.Chain]:
    """Round 0 of six members: five trained and were audited MATCH, the sixth committed and went
    quiet and its job was exhausted at the horizon; about 30 s."""
    with X.activated_network(tmp_path_factory.mktemp("gaps-chain")) as network:
        yield X.Chain(network)


def test_the_survivors_of_a_round_with_an_exhausted_member_are_paid_their_whole_round(
    chain: X.Chain,
) -> None:
    rnd, theta = chain.applied0()
    assert rnd.quiet is not None
    survivors = sorted(rnd.hotkey(i) for i in chain.survivors)
    escrow = rnd.store._services(rnd.run_id)[0]
    units = escrow.policy.round_reward_units
    X.control(
        sorted(x.roster.hotkey for x in rnd.store.verified_inputs_v2(rnd.run_id, 0)) == survivors
        and rnd.hotkey(rnd.quiet) in rnd.excluded(),
        "five audited members are verified and the sixth was excluded at the horizon",
    )
    issued = escrow.balances().issued

    response = rnd.network.client.post(**X.finalize_request(rnd, chain.survivors, theta))

    assert (response.status_code, response.json()["status"]) == (200, "SETTLED")
    origins = {
        row["hotkey"]: row["issued"]
        for row in rnd.store._db.execute(
            "SELECT hotkey, issued FROM escrow_origins WHERE reward_round=0"
        )
    }
    assert origins == {hotkey: units // len(survivors) for hotkey in survivors}
    assert sum(origins.values()) == units
    balances = escrow.balances()
    assert (balances.issued, balances.reward_pending) == (issued + units, units)
    assert balances.conserved()


# --- T-FIN-28 (FIN-17, DIB-22) [D-6]: ordinary traffic does not change what a repair rests on ----


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="FIN-17+DIB-22 [D-6 decision]: _rollback_snapshot_v2 hashes every accepted_v2 row and "
    "each auditor lease poll (even one that answers 204 and leases nothing) writes one, so a poll "
    "between the two revision checks of a repair makes the repair fail 409 'rollback authority "
    "revision changed'; which writes count as unrelated is undecided. Mechanism level: the "
    "end-to-end race needs the 150 s two-round rollback world "
    "[fix-lot: decision (product choice pending)]",
)
def test_an_auditor_poll_that_leases_nothing_does_not_change_the_revision_a_repair_rests_on(
    queue: X.Round,
) -> None:
    """FIN-17+DIB-22: an auditor poll that answers 204 and leases nothing leaves the revision the
    two checks of a repair compare unchanged."""
    rnd = queue
    auditor = X.service().AUDITORS[0]
    for job in rnd.lease_rows():
        complete_job(rnd, job["id"])
    world = X.world_of(rnd.network)
    revision = rnd.store._rollback_snapshot_v2(rnd.run_id)
    intake = world.count("accepted_v2")

    poll = X.post_lease(rnd, X.lease_ask(rnd, auditor, 9))

    X.control(
        poll.status_code == 204 and world.count("accepted_v2") == intake + 1,
        f"the poll leased nothing and wrote its intake row: {poll.status_code}",
    )
    assert rnd.store._rollback_snapshot_v2(rnd.run_id) == revision
