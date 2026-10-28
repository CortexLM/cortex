"""Finality and the round state machine: what the protocol must refuse and must keep.

The pure predicates of `finality_v2`, the ledger's reward rules, the store's transaction helper
and the legacy v1 finalize are exercised here; the real v2 service is in test_finality_2.py. A
refusal is pinned by its exception type and text; a defence that does not hold today is a strict
xfail that names the invariant.
"""

from __future__ import annotations

import sqlite3
from collections import Counter
from collections.abc import Iterator
from dataclasses import replace
from pathlib import Path
from typing import Any

import pytest
from hypothesis import given
from hypothesis import strategies as st

from hypertrain.aggregator.core import FinalityError
from hypertrain.challenge.finality_v2 import (
    RoundStatus,
    exclude,
    finish_rollback,
    require_apply,
    require_open,
    settle,
)
from hypertrain.ledger.escrow_v2 import EscrowReceipt, EscrowV2
from hypertrain.protocol.envelope import UnauthorizedSigner
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages import Finalize, Rollback

from .support import Real, error_code, load_fixture_module
from .support_finality import (
    ANOTHER,
    FINAL,
    THETA,
    ZERO,
    FailingCommit,
    Rounds,
    plain_store,
)

RUN = "ab" * 32
MINER = Keypair(bytes([0x31]) * 32).ss58


def status(w: int = 0, **changes: Any) -> RoundStatus:
    base: dict[str, Any] = {
        "run_id": RUN,
        "w": w,
        "applied": True,
        "disposition": "PENDING",
        "unresolved_disputes": 0,
        "unresolved_audits": 0,
        "rollback_complete": False,
        "state_hash": THETA,
        "resolution_hash": None,
    }
    return RoundStatus(**(base | changes))


def finalize_of(w: int = 0, theta: str = THETA) -> Finalize:
    return Finalize(w=w, final_theta_hash_w1=theta, included=[MINER], entitlements_root=ZERO)


# -- FIN-02: a round settles only when every precondition holds ---------------------------------

SETTLE_REFUSED = [
    pytest.param({"applied": False}, "round cannot settle", id="no applied tape"),
    pytest.param({"unresolved_audits": 1}, "round cannot settle", id="an audit is open"),
    pytest.param({"unresolved_disputes": 1}, "round cannot settle", id="a dispute is open"),
    pytest.param(
        {"disposition": "EXCLUDED", "resolution_hash": FINAL},
        "round cannot settle",
        id="the round is excluded",
    ),
    pytest.param({"state_hash": ANOTHER}, "round cannot settle", id="another applied theta"),
    pytest.param({"w": 1}, "round cannot settle", id="another round"),
    pytest.param({"resolution_hash": ANOTHER}, "conflicting finality", id="another finalize"),
]


@pytest.mark.parametrize(("changes", "message"), SETTLE_REFUSED)
def test_a_round_that_may_not_settle_is_refused_by_settle(
    changes: dict[str, Any], message: str
) -> None:
    # FIN-02: applied, no open audit or dispute, not excluded, bound to this finalize.
    clean = status()
    assert settle(clean, finalize_of(), FINAL).disposition == "SETTLED"
    with pytest.raises(FinalityError, match=message):
        settle(status(**changes), finalize_of(), FINAL)


def test_settle_changes_nothing_but_the_disposition_and_the_resolution() -> None:
    clean = status()
    settled = settle(clean, finalize_of(), FINAL)
    assert settled == replace(clean, disposition="SETTLED", resolution_hash=FINAL)
    assert settle(settled, finalize_of(), FINAL) == settled


# -- FIN-10: round states move one way ------------------------------------------------------------

hashes = st.sampled_from([THETA, FINAL, ANOTHER])
statuses = st.builds(
    RoundStatus,
    run_id=st.just(RUN),
    w=st.just(0),
    applied=st.booleans(),
    disposition=st.sampled_from(["PENDING", "SETTLED", "EXCLUDED"]),
    unresolved_disputes=st.integers(0, 2),
    unresolved_audits=st.integers(0, 2),
    rollback_complete=st.booleans(),
    state_hash=st.just(THETA),
    resolution_hash=st.one_of(st.none(), hashes),
).filter(lambda s: s.disposition == "PENDING" or s.resolution_hash is not None)
steps = st.one_of(
    st.tuples(st.just("settle"), hashes),
    st.tuples(st.just("exclude"), hashes),
    st.tuples(st.just("finish"), hashes),
)


def finish(current: RoundStatus, evidence: str) -> RoundStatus:
    rollback = Rollback(
        w=0,
        excluded=[MINER],
        old_theta_hash_w2=ANOTHER,
        new_theta_hash_w2=THETA,
        new_outer_state_hash=FINAL,
        recomputed=["agg_w", "step_w", "agg_w1", "step_w1"],
        cause_hashes=[evidence],
    )
    return finish_rollback(current, rollback, THETA, FINAL)


@given(statuses, st.lists(steps, max_size=6))
def test_a_round_status_never_moves_backwards(start: RoundStatus, plan: list[Any]) -> None:
    # FIN-10: applied never turns false, a settled or excluded round never returns to PENDING,
    # an audit or dispute counter is never raised by a transition, a resolution never changes.
    rank = {"PENDING": 0, "EXCLUDED": 1, "SETTLED": 2}
    current = start
    for name, evidence in plan:
        try:
            if name == "settle":
                after = settle(current, finalize_of(), evidence)
            elif name == "exclude":
                after = exclude(current, evidence)
            else:
                after = finish(current, evidence)
        except FinalityError:
            continue
        assert after.applied >= current.applied
        assert after.disposition == current.disposition or (
            current.disposition == "PENDING" and rank[after.disposition] > 0
        )
        if current.disposition == "SETTLED":
            assert after == current
        assert (after.unresolved_audits, after.unresolved_disputes) == (
            current.unresolved_audits,
            current.unresolved_disputes,
        )
        assert current.resolution_hash in (None, after.resolution_hash)
        current = after


def test_a_settled_round_cannot_be_excluded_and_a_conflicting_exclusion_is_refused() -> None:
    # FIN-15 (second guard): late fraud on a settled round never rewrites model history.
    settled = status(disposition="SETTLED", resolution_hash=FINAL)
    with pytest.raises(FinalityError, match="cannot rewrite model history"):
        exclude(settled, ANOTHER)
    excluded = exclude(status(), FINAL)
    assert (excluded.disposition, excluded.resolution_hash) == ("EXCLUDED", FINAL)
    assert exclude(excluded, FINAL) == excluded
    with pytest.raises(FinalityError, match="conflicting exclusion"):
        exclude(excluded, ANOTHER)


def test_an_exclusion_closes_a_round_only_when_no_applied_tape_is_left_to_repair() -> None:
    # FIN-12 + FIN-10: an applied round that is excluded keeps the w+2 gate shut until its
    # rollback ran; a round that never applied has nothing to repair.
    applied = exclude(status(), FINAL)
    assert (applied.rollback_complete, applied.closed) == (False, False)
    never_applied = exclude(status(applied=False), FINAL)
    assert (never_applied.rollback_complete, never_applied.closed) == (True, True)


# -- FIN-12: at most two speculative rounds -------------------------------------------------------

OPEN_REFUSED = [
    pytest.param(2, [status(0), status(1)], "w\\+2 blocked", id="round 0 is not settled"),
    pytest.param(
        2,
        [status(0, disposition="EXCLUDED", rollback_complete=False), status(1)],
        "w\\+2 blocked",
        id="round 0 is excluded but not repaired",
    ),
    pytest.param(
        2,
        [status(0, disposition="SETTLED", unresolved_disputes=1), status(1)],
        "w\\+2 blocked",
        id="a dispute is open on round 0",
    ),
    pytest.param(
        2,
        [status(0, disposition="SETTLED", unresolved_audits=1), status(1)],
        "w\\+2 blocked",
        id="an audit is open on round 0",
    ),
    pytest.param(2, [status(0)], "complete sequential history", id="round 1 is missing"),
    pytest.param(1, [status(0), status(1)], "complete sequential history", id="round 1 exists"),
    pytest.param(3, [status(0), status(2)], "complete sequential history", id="a gap"),
    pytest.param(1, [status(0), status(0)], "invalid finality history", id="a duplicate row"),
    pytest.param(
        1, [status(0, unresolved_audits=-1)], "invalid finality history", id="a negative counter"
    ),
    pytest.param(
        1, [replace(status(0), run_id=ANOTHER)], "invalid finality history", id="another run"
    ),
    pytest.param(-1, [], "invalid finality history", id="a negative round"),
    pytest.param(True, [status(0)], "invalid finality history", id="a boolean round"),
]


@pytest.mark.parametrize(("w", "history", "message"), OPEN_REFUSED)
def test_a_round_does_not_open_without_a_complete_closed_history(
    w: int, history: list[RoundStatus], message: str
) -> None:
    with pytest.raises(FinalityError, match=message):
        require_open(RUN, w, history)


@pytest.mark.parametrize(
    ("w", "history"),
    [
        pytest.param(0, [], id="the first round"),
        pytest.param(1, [status(0)], id="the second round opens on an unsettled first"),
        pytest.param(2, [status(0, disposition="SETTLED"), status(1)], id="first settled"),
        pytest.param(
            2,
            [status(0, disposition="EXCLUDED", rollback_complete=True), status(1)],
            id="first excluded and repaired",
        ),
    ],
)
def test_a_round_opens_once_every_round_two_back_is_closed(
    w: int, history: list[RoundStatus]
) -> None:
    require_open(RUN, w, history)


def test_a_round_is_applied_only_on_an_applied_predecessor() -> None:
    require_apply(RUN, 1, [status(0)])
    with pytest.raises(FinalityError, match="predecessor not applied"):
        require_apply(RUN, 1, [status(0, applied=False)])


actions = st.lists(
    st.tuples(st.sampled_from(["open", "apply", "settle", "repair"]), st.integers(0, 5)),
    max_size=24,
)


@given(actions)
def test_no_schedule_of_the_gates_leaves_a_third_round_unsettled(plan: list[Any]) -> None:
    # FIN-12: drive the pure gates with arbitrary open / apply / settle / repair requests; each
    # request moves state only when its gate lets it through.
    history: list[RoundStatus] = []
    for name, pick in plan:
        try:
            if name == "open":
                require_open(RUN, len(history), history)
                history.append(status(len(history), applied=False, unresolved_audits=1))
            elif history:
                w = pick % len(history)
                row = history[w]
                if name == "apply":
                    require_apply(RUN, w, history[:w])
                    history[w] = replace(row, applied=True, unresolved_audits=0)
                elif name == "settle":
                    history[w] = settle(row, finalize_of(w), FINAL)
                else:
                    history[w] = replace(exclude(row, FINAL), rollback_complete=True)
        except FinalityError:
            continue
        assert all(row.closed for row in history[:-2])
        assert [row.w for row in history] == list(range(len(history)))


# -- FIN-14: a rollback is complete only with replayed outputs and a matching cause ---------------


def rollback_of(**changes: Any) -> Rollback:
    base: dict[str, Any] = {
        "w": 0,
        "excluded": [MINER],
        "old_theta_hash_w2": ANOTHER,
        "new_theta_hash_w2": THETA,
        "new_outer_state_hash": FINAL,
        "recomputed": ["agg_w", "step_w", "agg_w1", "step_w1"],
        "cause_hashes": [FINAL],
    }
    return Rollback(**(base | changes))


def test_a_rollback_completes_an_exclusion_only_with_the_replayed_outputs() -> None:
    excluded = exclude(status(), FINAL)
    repaired = finish_rollback(excluded, rollback_of(), THETA, FINAL)
    assert (repaired.disposition, repaired.rollback_complete) == ("EXCLUDED", True)
    assert repaired.closed


NO_EXCLUSION = "rollback has no pending applied exclusion"
NO_REPLAY = "rollback lacks complete authenticated replay"


@pytest.mark.parametrize(
    ("current", "model", "theta", "outer", "message"),
    [
        pytest.param(status(), rollback_of(), THETA, FINAL, NO_EXCLUSION, id="never excluded"),
        pytest.param(
            exclude(status(applied=False), FINAL),
            rollback_of(),
            THETA,
            FINAL,
            NO_EXCLUSION,
            id="never applied",
        ),
        pytest.param(
            exclude(status(), FINAL),
            rollback_of(w=1),
            THETA,
            FINAL,
            NO_EXCLUSION,
            id="another round",
        ),
        pytest.param(
            exclude(status(), FINAL),
            rollback_of(recomputed=["agg_w", "step_w"]),
            THETA,
            FINAL,
            NO_REPLAY,
            id="partial recomputation",
        ),
        pytest.param(
            exclude(status(), FINAL), rollback_of(), ANOTHER, FINAL, NO_REPLAY, id="another theta"
        ),
        pytest.param(
            exclude(status(), FINAL),
            rollback_of(),
            THETA,
            ANOTHER,
            NO_REPLAY,
            id="another outer state",
        ),
        pytest.param(
            exclude(status(), FINAL),
            rollback_of(cause_hashes=[ANOTHER]),
            THETA,
            FINAL,
            NO_REPLAY,
            id="a cause that is not the exclusion",
        ),
    ],
)
def test_a_rollback_without_complete_authenticated_replay_is_refused(
    current: RoundStatus, model: Rollback, theta: str, outer: str, message: str
) -> None:
    with pytest.raises(FinalityError) as refused:
        finish_rollback(current, model, theta, outer)
    assert str(refused.value) == message


# -- FIN-06 / FIN-07: one credit per (run, round, miner), rounds minted in order ------------------


@pytest.fixture(scope="module")
def rounds(li_escrow: Any, real_reward: Real) -> Rounds:
    return Rounds(li_escrow, real_reward, cap_rounds=3)


def reward_origins(escrow: EscrowV2) -> Counter[tuple[int, str]]:
    return Counter(
        (int(row[0]), str(row[1]))
        for row in escrow.db.execute(
            "SELECT reward_round, hotkey FROM escrow_origins WHERE reward_round IS NOT NULL"
        )
    )


@given(st.lists(st.tuples(st.integers(0, 2), st.integers(0, 1)), max_size=9))
def test_every_round_is_credited_once_and_in_order_whatever_is_attempted(
    rounds: Rounds, attempts: list[tuple[int, int]]
) -> None:
    # FIN-06 + FIN-07: attempt (round, variant) finalizes in any order, repeated and conflicting.
    escrow = rounds.escrow()
    hot = rounds.fixture.HOT.ss58
    accepted: dict[int, tuple[int, EscrowReceipt]] = {}
    for w, variant in attempts:
        item = rounds.build(w, variant)

        def finalize(item: Any = item) -> EscrowReceipt:
            return escrow.reward_finalize(item.reward, item.evidence)

        if w in accepted:
            first, receipt = accepted[w]
            if first == variant:
                assert finalize() == receipt
            else:
                assert error_code(finalize) == "OPERATION_CONFLICT"
        elif w == len(accepted):
            accepted[w] = (variant, finalize())
        else:
            assert error_code(finalize) == "FINALIZE_ORDER"
        assert [r[0] for r in escrow.db.execute("SELECT w FROM escrow_finalized ORDER BY w")] == [
            *range(len(accepted))
        ]
        assert reward_origins(escrow) == Counter({(round_w, hot): 1 for round_w in accepted})
        assert {
            row[0]
            for row in escrow.db.execute(
                "SELECT origin FROM escrow_origins WHERE reward_round IS NOT NULL"
            )
        } == {origin for w, (v, _) in accepted.items() for origin in rounds.build(w, v).origin_ids}
        assert escrow.balances().issued == rounds.budget * len(accepted)
        assert escrow.balances().conserved()


def test_an_origin_that_already_exists_is_never_credited_again(rounds: Rounds) -> None:
    # FIN-06: origin ids are unique; a finalize that would reissue one changes nothing.
    escrow = rounds.escrow()
    item = rounds.build(0)
    escrow.db.execute(
        "INSERT INTO escrow_origins VALUES(?,?,?,?,?)",
        (item.origin_ids[0], 1, 20, 0, rounds.fixture.HOT.ss58),
    )
    assert error_code(lambda: escrow.reward_finalize(item.reward, item.evidence)) == (
        "DUPLICATE_ORIGIN"
    )
    assert escrow.db.execute("SELECT COUNT(*) FROM escrow_events").fetchone()[0] == 0
    assert escrow.db.execute("SELECT COUNT(*) FROM escrow_finalized").fetchone()[0] == 0
    assert escrow.db.execute("SELECT issued FROM escrow_origins").fetchall()[0][0] == 1


# -- FIN-18: a miner that is not both included and proven is paid nothing --------------------------


def mismatched_work(rounds: Rounds, case: str) -> tuple[Any, Any]:
    both, alone = rounds.build(0, 0, miners=2), rounds.build(0, 0, miners=1)
    if case == "included without work":
        # The coordinator's signature names two miners; only one has verified work.
        return both.reward, replace(both.evidence, works=both.evidence.works[:1])
    # Verified work of a miner the coordinator did not include.
    return alone.reward, replace(
        alone.evidence, works=(*alone.evidence.works, both.evidence.works[1])
    )


@pytest.mark.parametrize(
    ("case", "code"),
    [
        ("included without work", "NO_VERIFIED_WORK_OR_EXCLUSIONS"),
        ("work without inclusion", "UNVERIFIED_REWARD_WORK"),
    ],
)
def test_a_finalize_whose_included_set_and_work_differ_credits_nobody(
    rounds: Rounds, case: str, code: str
) -> None:
    reward, evidence = mismatched_work(rounds, case)
    escrow = rounds.escrow()
    assert error_code(lambda: escrow.reward_finalize(reward, evidence)) == code
    assert escrow.balances().issued == 0
    assert escrow.db.execute("SELECT COUNT(*) FROM escrow_origins").fetchone()[0] == 0
    assert escrow.db.execute("SELECT COUNT(*) FROM escrow_events").fetchone()[0] == 0


def test_a_finalize_credits_exactly_the_miners_it_includes_and_has_work_for(
    rounds: Rounds,
) -> None:
    both = rounds.build(0, 0, miners=2)
    escrow = rounds.escrow()
    escrow.reward_finalize(both.reward, both.evidence)
    paid = {
        row[0]: row[1] for row in escrow.db.execute("SELECT hotkey, issued FROM escrow_origins")
    }
    assert sorted(paid) == sorted(work.commit.signer for work in both.evidence.works)
    assert sum(paid.values()) == rounds.budget


AUDIT_NOT_PROVEN = [
    ("mismatch", "UNVERIFIED_REWARD_WORK"),
    ("bad leaf named", "UNVERIFIED_REWARD_WORK"),
    ("another leaves root", "UNVERIFIED_REWARD_WORK"),
    ("foreign driver", "UNVERIFIED_REWARD_WORK"),
    ("foreign image", "UNVERIFIED_REWARD_WORK"),
    ("another challenge", "REWARD_AUDIT_BINDING"),
]


@pytest.mark.parametrize(("tamper", "code"), AUDIT_NOT_PROVEN)
def test_a_miner_is_credited_only_for_work_a_pinned_auditor_matched(
    rounds: Rounds, tamper: str, code: str
) -> None:
    # FIN-03: the ledger re-reads each verdict; the coordinator's Finalize alone pays nothing.
    escrow = rounds.escrow()
    bad = rounds.build(0, tamper=tamper)
    assert error_code(lambda: escrow.reward_finalize(bad.reward, bad.evidence)) == code
    assert escrow.balances().issued == 0
    assert escrow.db.execute("SELECT COUNT(*) FROM escrow_events").fetchone()[0] == 0
    good = rounds.build(0)
    escrow.reward_finalize(good.reward, good.evidence)
    assert escrow.balances().issued == rounds.budget


@pytest.mark.parametrize(
    ("tamper", "code"),
    [
        ("foreign finalize signer", "FINALITY_AUTHORITY"),
        ("another replayed theta", "FINALITY_REPLAY"),
    ],
)
def test_the_ledger_credits_nothing_for_a_finalize_it_cannot_authenticate_and_replay(
    rounds: Rounds, tamper: str, code: str
) -> None:
    # FIN-01: the ledger re-derives the finalize signer and the replayed theta by itself.
    escrow = rounds.escrow()
    bad = rounds.build(0, tamper=tamper)
    assert error_code(lambda: escrow.reward_finalize(bad.reward, bad.evidence)) == code
    assert escrow.balances().issued == 0
    good = rounds.build(0)
    escrow.reward_finalize(good.reward, good.evidence)
    assert escrow.balances().issued == rounds.budget


def test_a_verdict_of_an_unpinned_auditor_credits_nobody(rounds: Rounds) -> None:
    escrow = rounds.escrow()
    bad = rounds.build(0, tamper="unpinned auditor")
    with pytest.raises(UnauthorizedSigner, match="not authorized for message role"):
        escrow.reward_finalize(bad.reward, bad.evidence)
    assert escrow.balances().issued == 0


def test_an_audit_challenge_the_reward_authority_did_not_sign_credits_nobody(
    rounds: Rounds,
) -> None:
    # FIN-03: the challenge a verdict answers must come from the reward authority.
    escrow = rounds.escrow()
    bad = rounds.build(0, tamper="foreign challenge signer")
    with pytest.raises(UnauthorizedSigner, match="not authorized for message role"):
        escrow.reward_finalize(bad.reward, bad.evidence)
    assert escrow.balances().issued == 0


def test_a_miner_is_not_credited_for_a_verdict_it_signed_itself(rounds: Rounds) -> None:
    # FIN-03: a pinned auditor key is accepted on any verdict, but never on its own commit.
    escrow = rounds.escrow()
    escrow.auditors = escrow.auditors | {rounds.fixture.HOT.ss58}
    bad = rounds.build(0, tamper="self audit")
    assert error_code(lambda: escrow.reward_finalize(bad.reward, bad.evidence)) == (
        "UNVERIFIED_REWARD_WORK"
    )
    assert escrow.balances().issued == 0
    good = rounds.build(0)
    escrow.reward_finalize(good.reward, good.evidence)
    assert escrow.balances().issued == rounds.budget


# -- FIN-25: the store's transaction helper leaves no transaction open -----------------------------


@pytest.fixture
def plain(tmp_path: Path) -> Iterator[Any]:
    store = plain_store(tmp_path)
    yield store
    store.close()


def probe(store: Any) -> list[str]:
    return [row[0] for row in store._db.execute("SELECT key FROM meta WHERE key LIKE 'probe%'")]


def test_a_transaction_commits_on_success_and_is_closed(plain: Any) -> None:
    with plain._tx() as db:
        db.execute("INSERT INTO meta VALUES('probe-ok', '1')")
        assert db.in_transaction
    assert not plain._db.in_transaction
    assert probe(plain) == ["probe-ok"]


class Interrupt(BaseException):
    """What a signal handler or a thread cancellation raises: not an Exception."""


@pytest.mark.parametrize("error", [ValueError("boom"), Interrupt(), sqlite3.IntegrityError("pk")])
def test_a_transaction_rolls_back_whatever_ends_it_and_is_closed(
    plain: Any, error: BaseException
) -> None:
    with pytest.raises(type(error)) as raised, plain._tx() as db:
        db.execute("INSERT INTO meta VALUES('probe-lost', '1')")
        raise error
    assert raised.value is error
    assert not plain._db.in_transaction
    assert probe(plain) == []
    with plain._tx() as db:
        db.execute("INSERT INTO meta VALUES('probe-next', '1')")
    assert probe(plain) == ["probe-next"]


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="FIN-25+D-FIN-11: a failing COMMIT rolls the transaction back and frees the "
    "connection [fix-lot: none (no design yet)]",
)
def test_a_transaction_whose_commit_fails_is_rolled_back_and_closed(plain: Any) -> None:
    real = plain._db
    with plain._tx() as db:  # control: the scenario reaches COMMIT and a healthy one succeeds
        db.execute("INSERT INTO meta VALUES('probe-control', '1')")
    plain._db = FailingCommit(real)
    try:
        with pytest.raises(sqlite3.OperationalError, match="disk is full"), plain._tx() as db:
            db.execute("INSERT INTO meta VALUES('probe-doomed', '1')")
        assert not real.in_transaction
    finally:
        plain._db = real
    assert probe(plain) == ["probe-control"]
    with plain._tx() as db:
        db.execute("INSERT INTO meta VALUES('probe-after', '1')")
    assert probe(plain) == ["probe-after", "probe-control"]


# -- FIN-27: the legacy v1 finalize and its file ledger -------------------------------------------


class Legacy:
    """One v1 run on the cheap in-process app of tests/challenge, up to the verge of finalize."""

    def __init__(self, conf: Any, net: Any) -> None:
        self.conf, self.net = conf, net
        self.store = net.c.app.state.store

    def ready(self) -> tuple[list[str], dict[str, Any]]:
        net, conf = self.net, self.conf
        net.start()
        body = net.body(0)
        net.push(body["d_assign"])
        for miner in net.miners:
            assert net.accept(miner, 0).status_code == 200
        net.push(body["d_assign"] + 4)
        for miner in net.miners:
            assert net.commit(miner, 0).json()["status"] == "COMMITTED"
            assert net.delta(miner, 0).status_code == 200
        net.push(body["d_audit"])
        lease_headers = conf.bearer(conf.WORKER)
        while (lease := net.c.post("/v1/worker/lease", headers=lease_headers)).status_code == 200:
            assert net.verdict(lease.json(), "MATCH").status_code == 200
        net.push(body["d_upload"])
        assert net.aggregate(0).status_code == 200
        return sorted(miner.ss58 for miner in net.miners), body

    def journal(self) -> tuple[int, list[tuple[int, str, int]]]:
        """(last finalized round, (round, hotkey, amount) of every entitlement) of the journal."""
        state = self.store.ledger.state()
        return state.last_finalized, [(e.w, e.hotkey, e.amount) for e in state.entitlements]

    def refused(self, included: list[str]) -> tuple[int, str]:
        answer = self.net.finalize(0, included)
        return answer.status_code, answer.json()["detail"]


@pytest.fixture
def legacy(tmp_path: Path) -> Iterator[Legacy]:
    conf = load_fixture_module("li_challenge_conftest", "challenge/conftest.py")
    secrets = conf.secrets_dir.__wrapped__(tmp_path)
    master = conf.Master()
    with conf.make_client(tmp_path / "state", secrets, master, [1_800_000_000.0]) as client:
        yield Legacy(conf, conf.Net(client, master))


def test_legacy_finalize_is_journaled_and_recorded_together_and_only_once(legacy: Legacy) -> None:
    included, body = legacy.ready()
    untouched = (-1, [])
    assert legacy.refused(included) == (409, "d_final not reached")
    assert legacy.journal() == untouched
    legacy.net.push(body["d_final"])
    assert legacy.refused(included[:2]) == (422, f"included must be {included}")
    assert legacy.journal() == untouched
    assert legacy.net.round(0)["state"] != "FINAL"

    done = legacy.net.finalize(0, included)
    assert done.status_code == 200, done.text
    journal = legacy.journal()
    assert journal[0] == 0 and {w for w, _, _ in journal[1]} == {0}
    assert legacy.net.round(0)["state"] == "FINAL"
    assert legacy.refused(included) == (409, "duplicate (run_id, w, type, signer) message")
    assert legacy.journal() == journal


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="FIN-27+D-FIN-12: a legacy finalize interrupted after its journal append can be "
    "retried [fix-lot: none (no design yet)]",
)
def test_a_legacy_finalize_interrupted_after_the_journal_append_can_be_retried(
    legacy: Legacy,
) -> None:
    included, body = legacy.ready()
    legacy.net.push(body["d_final"])
    real_finalize = legacy.store.ledger.finalize
    appended: list[int] = []

    def die_after_the_append(w: int, at: int) -> None:
        real_finalize(w, at)
        appended.append(w)
        if len(appended) == 1:
            raise OSError("the process died after the journal append")

    legacy.store.ledger.finalize = die_after_the_append
    with pytest.raises(OSError, match="journal append"):
        legacy.net.finalize(0, included)
    assert appended == [0]  # control: the interruption came after the journal moved
    assert legacy.net.round(0)["state"] != "FINAL"

    retry = legacy.net.finalize(0, included)
    assert retry.status_code == 200, retry.text
    assert legacy.net.round(0)["state"] == "FINAL"
    assert legacy.journal()[0] == 0
