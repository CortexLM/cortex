"""Finalize and get_weights at the service boundary: double credit, unaudited and unfinalized
epochs, races around finalize, and the vesting window, driven over the real HTTP routes.

The escrow-level half (the ledger refusing a second origin set, a stale included set, a racing
get_weights) is test_ledger_defences.py. Here the same questions are asked of the master as a
client sees them: POST /admin/rounds/{w}/audits|aggregate|finalize, POST /worker/jobs/{id}/complete
and GET /internal/v1/get_weights, on a round of five miners (four honest, one copier whose audit is
a MISMATCH and who is excluded) that is played once and restored per test (ac_finalize.py).

Every docstring states the defence. An `xfail(strict=True, raises=AssertionError)` test is a
hole: its body asserts the DESIRED defence and flips to a failure the day the hole is closed; a
setup step that must hold today raises ac_precondition.PreconditionError instead, so a broken
scenario shows as FAILED and never hides behind the expected failure. The race tests send their
requests from several threads through one TestClient. The store runs each request as one
transaction under its lock, so what is raced is the order of whole requests and the assertions
hold in every order.
Requests that touch the shared connection outside that lock (FIN-3) are not raced at all: a lock
audit of the connection finds them in one thread, deterministically.

How an excluded miner's slice of the round reward is split (paid to its peers, as finalize_v2 does
today, or burned, as docs/mechanism.md section 4 describes for the v1 ledger) is a contested design
question. The tests here therefore derive the issued amount from the world (issued_units) and
assert properties that hold under either answer; the amount they saw goes to the evidence record.
"""

from __future__ import annotations

from collections.abc import Callable, Iterator
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import ac_cheats
import ac_finalize as fin
import pytest
from ac_finalize import Stage
from ac_precondition import precondition
from ac_world import GraduatedSnapshot, World, record

from hypertrain.ledger import FULL_SHARE_MASS

CHEAT = ac_cheats.BY_NAME["copied-peer-upload-resigned"]
HONEST_INDEXES = (0, 1, 2, 3)
CHEATER_INDEX = 4
ROUND_REWARD = 1_000_000  # round budget in units; what is issued depends on the slice rule above
MISSING_REPLAY = "missing accepted replay"
LATE_LEASE = "audit result outside owned immutable lease"
FORGED_VERDICT = "signed verdict differs from independent real replay"
CONFLICT = "conflicting signed operation nonce"
WRONG_INCLUDED = "finalized included set differs from full accepted authority"


@dataclass(frozen=True)
class Stages:
    """Round 0 of five miners captured at three stops, plus what the play showed."""

    uploaded: Stage  # all five UPLOADED, no audit scheduled
    partial: Stage  # audits scheduled, four of five completed
    audited: Stage  # every audit completed, nothing aggregated
    honest: tuple[str, ...]
    cheater: str
    intake: dict[str, Any]
    audit: dict[str, tuple[str, int | None]]  # hotkey -> (verdict, first_bad_leaf)
    theta_hash: str


@pytest.fixture(scope="module")
def stages(ac_snapshot: GraduatedSnapshot, tmp_path_factory: pytest.TempPathFactory) -> Stages:
    root = tmp_path_factory.mktemp("finalize-stages")
    world = ac_snapshot.clone(root / "play")
    try:
        with world:
            world.open_round(0)
            plays = {**dict.fromkeys(HONEST_INDEXES), CHEATER_INDEX: CHEAT}
            outcomes = world.run_roster(plays)
            assert [o.status for o in outcomes] == ["UPLOADED"] * 5, outcomes
            honest = tuple(world.hotkey(i) for i in HONEST_INDEXES)
            cheater = world.hotkey(CHEATER_INDEX)
            uploaded = fin.freeze(world, root / "uploaded", cheater)
            scheduled = fin.schedule_audits(world, 0)
            assert scheduled.status_code == 200, scheduled.text
            done = fin.run_audits(world, 0, limit=4)
            partial = fin.freeze(world, root / "partial", cheater)
            done += fin.run_audits(world, 0)
            audited = fin.freeze(world, root / "audited", cheater)
    finally:
        world.close()
    audit = {d.hotkey: (d.verdict["result"], d.verdict["first_bad_leaf"]) for d in done}
    assert audit[cheater][0] == "MISMATCH" and {audit[h][0] for h in honest} == {"MATCH"}, audit
    probe = fin.thaw(ac_snapshot, audited, root / "probe")
    try:
        with probe:
            applied = fin.aggregate(probe, 0)
            assert applied.status_code == 200, applied.text
    finally:
        probe.close()
    return Stages(
        uploaded,
        partial,
        audited,
        honest,
        cheater,
        {"status": outcomes[CHEATER_INDEX].status, "detail": outcomes[CHEATER_INDEX].detail},
        audit,
        applied.json()["theta_hash"],
    )


@pytest.fixture
def fork(ac_snapshot: GraduatedSnapshot, tmp_path: Path) -> Iterator[Callable[[Stage], World]]:
    """Restore a captured stage into a fresh installed World; closed at teardown."""
    worlds: list[World] = []

    def restore(stage: Stage) -> World:
        world = fin.thaw(ac_snapshot, stage, tmp_path / f"fork-{len(worlds)}")
        worlds.append(world)
        world.install()
        return world

    yield restore
    for world in reversed(worlds):
        world.close()


def settle_audited(world: World, stages: Stages) -> tuple[dict[str, Any], Any]:
    """Aggregate and finalize the audited round with the honest set; (finalize answer, request)."""
    applied = fin.aggregate(world, 0)
    assert applied.status_code == 200, applied.text
    request = fin.signed_finalize(world, 0, applied.json()["theta_hash"], list(stages.honest))
    settled = fin.finalize(world, 0, request)
    assert settled.status_code == 200 and settled.json()["status"] == "SETTLED", settled.text
    return settled.json(), request


def issued_units(world: World) -> int:
    """Units the settled round issued to its four honest miners: four equal origins, between a
    fifth (the cheater's slice burned) and a quarter (shared among the four) of the budget."""
    units = [issued for _, issued, _ in fin.reward_origins(world)]
    assert len(units) == 4 and len(set(units)) == 1, units
    assert ROUND_REWARD // 5 <= units[0] <= ROUND_REWARD // 4, units
    return sum(units)


def mature_at(world: World) -> int:
    return max(m for _, _, m in fin.reward_origins(world))


def detail(answer: Any) -> str:
    return str(answer.json()["detail"])


def burn_facts(answer: Any) -> tuple[int, int, str]:
    meta = answer.json()["metadata"]
    return meta["units_paid"], meta["units_burned_this_epoch"], meta["reason"]


def note(test: str, stages: Stages, world: World, answer: Any = None, **extra: Any) -> None:
    """The evidence row of the ADR table for this scenario (a no-op unless the file is set)."""
    seen = world.observe()
    weights = None if answer is None else answer.json()["weights"]
    sanction = seen.admissions[stages.cheater]
    record(
        {
            "test": test,
            "cheat": CHEAT.cheat,
            "double": CHEAT.name,
            "seam": CHEAT.seam,
            "intake": stages.intake,
            "audit": list(stages.audit[stages.cheater]),
            "excluded_reason": seen.excluded.get(f"0:{stages.cheater}", {}).get("reason"),
            "round_settled": seen.finality.get("0", {}).get("disposition") == "SETTLED",
            "cheater_weight": None if weights is None else weights.get(stages.cheater, 0.0),
            "honest_weights": None
            if weights is None
            else [weights.get(h, 0.0) for h in stages.honest],
            "cheater_admission": {k: sanction[k] for k in ("state", "strikes", "pending_dispute")},
            "honest_unaffected": all(
                seen.admissions[h]["state"] == "ACTIVE" and seen.admissions[h]["strikes"] == 0
                for h in stages.honest
            ),
            **extra,
        }
    )


# -- A. get_weights on an empty, unaudited or unfinalized epoch ----------------------------------


def test_graduated_run_without_a_round_answers_200_and_burns_the_whole_mass(
    ac_clone: World,
) -> None:
    """Defence: an epoch of a run in which no round was ever opened is a 200 with an empty weight
    map and the full share mass burned; the identities' admission locks are collateral, never
    emission."""
    # Given: five graduated identities holding genesis units and admission locks, no round.
    world = ac_clone
    now = world.network.now
    # When
    first = fin.ask_weights(world, 0, now)
    # Then
    assert first.status_code == 200, first.text
    assert first.json()["weights"] == {}
    assert burn_facts(first) == (0, FULL_SHARE_MASS, "ok")
    assert fin.ask_weights(world, 0, now).content == first.content
    assert fin.reward_origins(world) == [] and world.observe().escrow["paid"] == 0


@pytest.mark.parametrize("state", ["never_audited", "audited_and_aggregated"])
def test_round_without_a_finalize_pays_nothing_after_d_final(
    fork: Callable[[Stage], World], stages: Stages, state: str
) -> None:
    """Defence: a round that was never finalized earns nothing, however far its work got. With the
    beacon past d_final, get_weights answers 200 with an empty map and the whole mass burned, the
    answer is final, and no reward origin exists. An UPLOADED round that was never audited cannot
    even be aggregated (no replay)."""
    # Given: round 0 only UPLOADED, or fully audited and aggregated, and the beacon past d_final.
    audited = state == "audited_and_aggregated"
    world = fork(stages.audited if audited else stages.uploaded)
    opening = world.opening(0)
    if audited:
        assert fin.aggregate(world, 0).status_code == 200
    if opening.d_audit > world.network.now:
        world.network.push(opening.d_audit)
    world.network.push(opening.d_final + 1)
    # When
    answer = fin.ask_weights(world, 0, opening.d_final + 1)
    # Then
    assert answer.status_code == 200, answer.text
    assert answer.json()["weights"] == {}
    assert burn_facts(answer) == (0, FULL_SHARE_MASS, "ok")
    assert fin.ask_weights(world, 0, opening.d_final + 1).content == answer.content
    assert fin.reward_origins(world) == [] and world.observe().escrow["paid"] == 0
    if audited:
        late = fin.finalize(
            world, 0, fin.signed_finalize(world, 0, stages.theta_hash, list(stages.honest))
        )
        note("audited-round-never-finalized", stages, world, answer, late_finalize=late.status_code)
    else:
        audits = fin.post(world, "/admin/rounds/0/audits", admin=True)
        aggregate = fin.aggregate(world, 0)
        assert aggregate.status_code == 409 and MISSING_REPLAY in detail(aggregate)
        note(
            "unaudited-round-after-d-final",
            stages,
            world,
            answer,
            audits_after_d_final=[audits.status_code, detail(audits)],
        )


def test_epoch_answered_before_finalize_stays_final_and_a_later_epoch_pays_the_reward(
    fork: Callable[[Stage], World], stages: Stages
) -> None:
    """Defence: an audited and aggregated round pays nothing until it is finalized. The epoch
    answered meanwhile stays final when the round is then finalized and matures: the reward is paid
    by the next epoch, never retroactively, the genesis units are never emitted and the cheater
    gets nothing."""
    # Given: the audited round aggregated and epoch 0 asked before the Finalize.
    world = fork(stages.audited)
    applied = fin.aggregate(world, 0)
    assert applied.status_code == 200, applied.text
    unfinalized = fin.ask_weights(world, 0, world.network.now)
    assert unfinalized.status_code == 200 and unfinalized.json()["weights"] == {}
    assert burn_facts(unfinalized) == (0, FULL_SHARE_MASS, "ok")
    assert fin.reward_origins(world) == []
    # When: the round is finalized, matures, and epochs 0 and 1 are asked.
    request = fin.signed_finalize(world, 0, applied.json()["theta_hash"], list(stages.honest))
    settled = fin.finalize(world, 0, request)
    assert settled.status_code == 200 and settled.json()["status"] == "SETTLED", settled.text
    maturity = mature_at(world)
    issued = issued_units(world)
    world.network.push(maturity + 1)
    again = fin.ask_weights(world, 0, maturity + 2)
    next_epoch = fin.ask_weights(world, 1, maturity + 3)
    # Then: epoch 0 is the first answer byte for byte; epoch 1 pays the four honest miners alone.
    assert again.content == unfinalized.content
    assert next_epoch.json()["weights"] == dict.fromkeys(stages.honest, issued / 4)
    assert burn_facts(next_epoch) == (issued, FULL_SHARE_MASS - issued, "ok")
    escrow = world.observe().escrow
    assert escrow["paid"] == issued and escrow["conserved"] is True
    note("epoch-answered-before-finalize", stages, world, next_epoch, issued_units=issued)


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="FIN-2 (reframed): the non-service branch of open_round_v2 (store.py:5117) only checks "
    "d_open > now and d_final - d_audit >= 2, so a RoundOpenV2 whose d_final lies 300 rounds after "
    "d_audit is accepted. The audit challenge expires at min(d_final, d_audit + 2 * lease_rounds) "
    "= d_audit + 200 and finalize re-checks it, so a Finalize signed after that and before d_final "
    "is refused 'envelope expired' and the honest miners lose the reward. The service-admitted "
    "opening (store.py:5109) and scripts/ops/live_operator.py:119-120 already refuse the gap",
)
def test_round_opening_whose_final_deadline_outlives_the_audit_horizon_is_refused(
    ac_clone: World,
) -> None:
    """Desired defence: the master refuses, with a 422, a RoundOpenV2 whose d_final is more than
    twice the audit lease (200 rounds here) after d_audit, as the service-admitted opening already
    does, instead of accepting a round whose finalize window cannot be served. The gap of 101 to
    200 rounds (live_operator.py accepts it, the S7 rule does not) is a policy question and is
    only recorded."""
    # Given: five graduated identities and no round; an opening with d_final = d_audit + 300.
    world = ac_clone
    # When
    refusal = fin.open_with_final_gap(world, 300)
    # Then
    if refusal is None:
        opening = world.opening(0)
        precondition(opening.d_final - opening.d_audit == 300, "the opening was not widened")
    assert refusal is not None and refusal.startswith("round open: 422"), refusal


# -- B. double credit at the HTTP boundary ---------------------------------------------------------


def test_finalize_posted_twice_returns_the_first_answer_and_issues_nothing_twice(
    fork: Callable[[Stage], World], stages: Stages
) -> None:
    """Defence: the same Finalize posted again, or re-signed with the same body, answers exactly
    like the first and leaves one origin set, one REWARD_FINALIZE event and the issued total
    unchanged."""
    # Given: a settled round.
    world = fork(stages.audited)
    first, request = settle_audited(world, stages)
    issued = world.observe().escrow["issued"]
    origins = fin.reward_origins(world)
    digest = fin.state_digest(world)
    resigned = fin.signed_finalize(world, 0, first["theta_hash"], list(stages.honest))
    # When
    same = fin.finalize(world, 0, request)
    renewed = fin.finalize(world, 0, resigned)
    # Then
    assert same.status_code == 200 and same.json() == first
    assert renewed.status_code == 200 and renewed.json() == first
    assert fin.state_digest(world) == digest
    assert fin.reward_origins(world) == origins and issued_units(world) > 0
    events = fin.rows(world, "SELECT COUNT(*) FROM escrow_events WHERE kind='REWARD_FINALIZE'")
    assert events == [(1,)]
    assert world.observe().escrow["issued"] == issued
    note("finalize-posted-twice", stages, world)


@pytest.mark.parametrize("variant", ["other_entitlements_root", "other_included_set"])
def test_second_finalize_with_other_content_is_refused_without_state_change(
    fork: Callable[[Stage], World], stages: Stages, variant: str
) -> None:
    """Defence: a round finalizes once. A second, validly signed Finalize of the same round with
    another entitlements_root or another included set is refused with 409 and changes no record,
    event, origin or unit. Three layers refuse it (the signed-operation nonce, the stored finalize
    body, the escrow operation id); the first one answers."""
    # Given: a settled round.
    world = fork(stages.audited)
    first, _ = settle_audited(world, stages)
    digest = fin.state_digest(world)
    honest = list(stages.honest)
    other = (
        fin.signed_finalize(world, 0, first["theta_hash"], honest, "22" * 32)
        if variant == "other_entitlements_root"
        else fin.signed_finalize(world, 0, first["theta_hash"], honest[:-1])
    )
    # When
    refused = fin.finalize(world, 0, other)
    # Then
    assert refused.status_code == 409 and detail(refused) == CONFLICT
    assert fin.state_digest(world) == digest
    assert len(fin.reward_origins(world)) == 4


@pytest.mark.parametrize("included", ["with_the_excluded_cheater", "without_an_audited_miner"])
def test_finalize_with_a_wrong_included_set_mints_nothing_and_leaves_the_round_open(
    fork: Callable[[Stage], World], stages: Stages, included: str
) -> None:
    """Defence: Finalize.included must equal the set of miners with a MATCH audit. A set that
    contains the excluded cheater, or omits an audited honest miner, is refused with 422 and mints
    nothing; the correct Finalize of the same round is still accepted afterwards."""
    # Given: the audited round aggregated, not finalized.
    world = fork(stages.audited)
    applied = fin.aggregate(world, 0)
    theta = applied.json()["theta_hash"]
    honest = list(stages.honest)
    wrong = [*honest, stages.cheater] if included == "with_the_excluded_cheater" else honest[:-1]
    digest = fin.state_digest(world)
    # When
    refused = fin.finalize(world, 0, fin.signed_finalize(world, 0, theta, wrong))
    # Then
    assert refused.status_code == 422 and detail(refused) == WRONG_INCLUDED
    assert fin.state_digest(world) == digest
    assert fin.reward_origins(world) == []
    accepted = fin.finalize(world, 0, fin.signed_finalize(world, 0, theta, honest))
    assert accepted.status_code == 200 and accepted.json()["status"] == "SETTLED"
    assert len(fin.reward_origins(world)) == 4


def test_aggregate_posted_twice_returns_the_same_tape_and_settles_on_it(
    fork: Callable[[Stage], World], stages: Stages
) -> None:
    """Defence: aggregating a round again, before or after it is finalized, returns the tape that
    was applied the first time and writes no second applied record; the finalize settles on exactly
    that tape."""
    # Given
    world = fork(stages.audited)
    first = fin.aggregate(world, 0)
    # When
    second = fin.aggregate(world, 0)
    settle_audited(world, stages)
    third = fin.aggregate(world, 0)
    # Then
    assert first.status_code == second.status_code == third.status_code == 200
    assert first.json() == second.json() == third.json()
    assert first.json()["theta_hash"] == stages.theta_hash
    applied = fin.rows(world, "SELECT data FROM records_v2 WHERE kind='applied'")
    assert len(applied) == 1 and first.json()["tape_hash"] in applied[0][0]
    replayed = fin.rows(world, "SELECT data FROM records_v2 WHERE kind='replayed-finality'")
    assert len(replayed) == 1 and first.json()["tape_hash"] in replayed[0][0]


# -- C. races around finalize ----------------------------------------------------------------------


def test_concurrent_finalize_posts_of_one_round_settle_it_once(
    fork: Callable[[Stage], World], stages: Stages
) -> None:
    """Defence: six Finalize POSTs of one round released together, three of one body and three of
    another, settle it exactly once. Whichever body arrives first wins; its copies answer 200 with
    the same receipt, the other body answers 409, and one origin set and one REWARD_FINALIZE event
    exist."""
    # Given: the aggregated round and two validly signed, different Finalize requests.
    world = fork(stages.audited)
    applied = fin.aggregate(world, 0)
    theta, honest = applied.json()["theta_hash"], list(stages.honest)
    bodies = {
        "base": fin.signed_finalize(world, 0, theta, honest),
        "other": fin.signed_finalize(world, 0, theta, honest, "22" * 32),
    }
    labels = ["base"] * 3 + ["other"] * 3
    # When
    answers = fin.run_together(
        [lambda b=bodies[label]: fin.finalize(world, 0, b) for label in labels]
    )
    # Then
    stored = fin.rows(world, "SELECT data FROM records_v2 WHERE kind='finalize' AND id='0'")
    winner = "base" if '"entitlements_root":"' + "0" * 64 in stored[0][0] else "other"
    for label, answer in zip(labels, answers, strict=True):
        if label == winner:
            assert answer.status_code == 200 and answer.json()["status"] == "SETTLED"
        else:
            assert answer.status_code == 409 and detail(answer) == CONFLICT
    receipts = [a.json() for a, label in zip(answers, labels, strict=True) if label == winner]
    assert receipts == receipts[:1] * 3
    assert len(fin.reward_origins(world)) == 4
    events = fin.rows(world, "SELECT COUNT(*) FROM escrow_events WHERE kind='REWARD_FINALIZE'")
    assert events == [(1,)] and world.observe().escrow["conserved"] is True


def test_late_audit_traffic_racing_finalize_changes_neither_sanction_nor_reward(
    fork: Callable[[Stage], World], stages: Stages
) -> None:
    """Defence: audit completions that arrive while the round is being finalized, or after it,
    cannot exclude a rewarded miner, rescue the excluded one or move a reward. A repeat of a stored
    verdict answers 200 with the same result; a verdict signed by the auditor but flipped (the
    cheater to MATCH, an honest miner to MISMATCH) is refused in every order, by the replay key that
    already holds the genuine verdict (409) or else by the comparison with the replay (422)."""
    # Given: the aggregated round, the stored verdicts, and two forged ones signed by the auditors.
    world = fork(stages.audited)
    applied = fin.aggregate(world, 0)
    request = fin.signed_finalize(world, 0, applied.json()["theta_hash"], list(stages.honest))
    audits = {a.hotkey: a for a in fin.audit_records(world)}
    honest_audit, cheater_audit = audits[stages.honest[0]], audits[stages.cheater]
    flips = [
        (cheater_audit, {**cheater_audit.verdict, "result": "MATCH", "first_bad_leaf": None}),
        (honest_audit, {**honest_audit.verdict, "result": "MISMATCH", "first_bad_leaf": 1}),
    ]
    forged = [
        lambda a=a, v=v: fin.post(
            world,
            f"/worker/jobs/{a.job_id}/complete",
            world.network.signed(a.auditor, "ReplayVerdict", v),
        )
        for a, v in flips
    ]
    repeats = [
        lambda a=a: fin.post(world, f"/worker/jobs/{a.job_id}/complete", a.signed)
        for a in audits.values()
    ]
    before = world.observe()
    # When
    answers = fin.run_together([lambda: fin.finalize(world, 0, request), *repeats, *forged])
    # Then
    settled, repeated, refused = (
        answers[0],
        answers[1 : 1 + len(audits)],
        answers[1 + len(audits) :],
    )
    assert settled.status_code == 200 and settled.json()["status"] == "SETTLED"
    results = [a.verdict["result"] for a in audits.values()]
    assert [(r.status_code, r.json()) for r in repeated] == [(200, {"result": x}) for x in results]
    assert {(r.status_code, detail(r)) for r in refused} <= {
        (409, CONFLICT),
        (422, FORGED_VERDICT),
    }
    after = world.observe()
    issued = issued_units(world)
    assert after.excluded == before.excluded and set(after.excluded) == {f"0:{stages.cheater}"}
    assert {lease["result"] for lease in after.leases} == {"MATCH", "MISMATCH"}
    assert after.escrow["issued"] == before.escrow["issued"] + issued
    world.network.push(mature_at(world) + 1)
    paid = fin.ask_weights(world, 0, mature_at(world) + 2)
    assert paid.json()["weights"] == dict.fromkeys(stages.honest, issued / 4)
    note("late-audit-traffic-racing-finalize", stages, world, paid)


def _round_view(world: World) -> Any:
    n = world.network
    return n.client.get(f"{n.url}/rounds/0")


UNLOCKED_REQUESTS = [
    pytest.param(
        lambda world: fin.aggregate(world, 0),
        id="aggregate",
        marks=pytest.mark.xfail(
            strict=True,
            raises=AssertionError,
            reason="FIN-3: aggregate_v2 (store.py:8063, 8137) calls _service_boundary_v2 and "
            "require_roster_v2 before it takes the store lock, so a second request on the shared "
            "connection can interleave with it: concurrent aggregate POSTs of one round answer a "
            "spurious 409 'missing accepted round' or raise sqlite3.InterfaceError",
        ),
    ),
    pytest.param(
        _round_view,
        id="round_view",
        marks=pytest.mark.xfail(
            strict=True,
            raises=AssertionError,
            reason="FIN-3: round_view_v2 (store.py:6086, the public GET /rounds/{w} that every "
            "miner calls) runs _service_boundary_v2 before its transaction, on the shared "
            "connection and outside the store lock",
        ),
    ),
    pytest.param(
        lambda world: fin.ask_weights(world, 5, world.network.now),
        id="get_weights",
        marks=pytest.mark.xfail(
            strict=True,
            raises=AssertionError,
            reason="LED-1 (minor): Store.weights reads sqlite_master, escrow_finalized and runs "
            "between its two transactions (store.py:2064-2074) without the store lock; a failure "
            "there is a 500 before the answer is persisted, so a retry is safe, and Cortex asks "
            "once per epoch",
        ),
    ),
]


@pytest.mark.parametrize("request_of", UNLOCKED_REQUESTS)
def test_requests_use_the_shared_connection_only_under_the_store_lock(
    request: pytest.FixtureRequest,
    fork: Callable[[Stage], World],
    stages: Stages,
    request_of: Callable[[World], Any],
) -> None:
    """Desired defence: the master serves every request on one SQLite connection from a thread
    pool, so every statement a request issues must hold the store lock; one that does not can
    interleave with a locked request on the same cached statement and answer a spurious 4xx/5xx
    or a garbled row. The lock audit finds an unlocked statement in a single request, with no race
    and so no dependence on the CPU count."""
    # Given: the audited round aggregated (so that aggregate is the idempotent second call).
    world = fork(stages.audited)
    first = fin.aggregate(world, 0)
    precondition(first.status_code == 200, f"first aggregate: {first.text}")
    # When
    with fin.lock_audit(world) as unlocked:
        answer = request_of(world)
    # Then
    precondition(answer.status_code == 200, f"audited request: {answer.text}")
    record(
        {
            "test": "shared-connection-lock-audit",
            "request": request.node.callspec.id,
            "unlocked_statements": list(unlocked),
        }
    )
    assert unlocked == []


def test_round_cannot_settle_while_an_audit_is_incomplete(
    fork: Callable[[Stage], World], stages: Stages
) -> None:
    """Defence: with one audit of five still queued, aggregate is refused (no replay) and so is a
    Finalize (no applied tape); nothing is recorded or minted. The round is not wedged: once the
    last audit completes it aggregates and settles."""
    # Given: four audits completed, one queued.
    world = fork(stages.partial)
    honest = list(stages.honest)
    digest = fin.state_digest(world)
    # When
    aggregate = fin.aggregate(world, 0)
    early = fin.finalize(world, 0, fin.signed_finalize(world, 0, stages.theta_hash, honest))
    # Then: both refused, state untouched.
    assert aggregate.status_code == 409 and MISSING_REPLAY in detail(aggregate)
    assert early.status_code == 409 and detail(early) == "missing accepted applied: 0"
    assert fin.state_digest(world) == digest and fin.reward_origins(world) == []
    # And the last audit lets the round settle.
    assert len(fin.run_audits(world, 0)) == 1
    settled, _ = settle_audited(world, stages)
    assert settled["status"] == "SETTLED" and len(fin.reward_origins(world)) == 4


def test_completion_after_the_lease_expired_is_refused_and_settles_nothing(
    fork: Callable[[Stage], World], stages: Stages
) -> None:
    """Defence: an auditor that replays but completes after its lease expired gets 409 'audit
    result outside owned immutable lease'. The lease is not completed, no verdict or replay is
    recorded for the miner, none is excluded as UNVERIFIED, and the round still cannot be
    aggregated. Whether the expired row is re-queued or, at the absolute horizon, exhausted
    depends on the round's offsets (FIN-1), so only what holds for both is asserted."""
    # Given: the last audit leased and replayed, then the beacon passes the 100-round lease.
    world = fork(stages.partial)
    executed = fin.execute_next(world, 0, 0)
    assert executed is not None
    world.network.push(world.network.now + 101)
    # When
    late = fin.complete(world, executed)
    # Then
    identity = f"0:{executed.hotkey}"
    assert late.status_code == 409 and detail(late) == LATE_LEASE
    lease = fin.rows(world, "SELECT state,result FROM audit_leases_v2 WHERE id=?", executed.job_id)
    assert len(lease) == 1 and lease[0][0] != "completed" and lease[0][1] == "AUDIT_INFRASTRUCTURE"
    recorded = fin.rows(
        world, "SELECT kind FROM records_v2 WHERE id=? AND kind IN ('replay','verdict')", identity
    )
    assert recorded == []
    excluded = fin.rows(
        world, "SELECT data FROM records_v2 WHERE kind='excluded' AND id=?", identity
    )
    assert all('"UNVERIFIED"' not in row[0] for row in excluded)
    assert fin.aggregate(world, 0).status_code == 409
    assert fin.reward_origins(world) == []


def test_verdict_that_differs_from_the_replay_is_refused_before_it_can_exclude_a_miner(
    fork: Callable[[Stage], World], stages: Stages
) -> None:
    """Defence: the auditor signs exactly the verdict the master replayed. A signed verdict that
    says MISMATCH for the audit the master replayed as MATCH is refused with 422 and cannot
    exclude the miner; the genuine verdict is then accepted and the audit completes."""
    # Given: the last audit leased and replayed (a MATCH), and a signed MISMATCH for it.
    world = fork(stages.partial)
    executed = fin.execute_next(world, 0, 0)
    assert executed is not None and executed.verdict["result"] == "MATCH"
    flipped = {**executed.verdict, "result": "MISMATCH", "first_bad_leaf": 1}
    # When
    forged = fin.complete(world, executed, flipped)
    # Then
    assert forged.status_code == 422 and detail(forged) == FORGED_VERDICT
    identity = f"0:{executed.hotkey}"
    assert (
        fin.rows(world, "SELECT 1 FROM records_v2 WHERE kind='excluded' AND id=?", identity) == []
    )
    lease = fin.rows(world, "SELECT state,result FROM audit_leases_v2 WHERE id=?", executed.job_id)
    assert lease == [("running", None)]
    genuine = fin.complete(world, executed)
    assert genuine.status_code == 200 and genuine.json() == {"result": "MATCH"}


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="FIN-1: AuditJobV2 caps lease_expires at created_beacon + 100 (messages_v2.py:450) and "
    "created_beacon is d_audit, not the lease start, so for H <= 30 (lease 100) a lease granted "
    "after d_audit never has room: the re-queued audit of an expired lease is refused 409 'lease "
    "exceeds 100 beacon rounds' (or, when d_final - d_audit <= 100, the row reaches the absolute "
    "horizon with attempts 1 of max_attempts 2 and its honest miner is excluded as "
    "INFRASTRUCTURE). It does not depend on the round offsets",
)
def test_expired_audit_lease_is_taken_over_by_the_other_auditor_and_the_round_settles(
    fork: Callable[[Stage], World], stages: Stages
) -> None:
    """Desired defence: when an auditor lets its lease expire, the audit is re-queued for the
    other auditor (policy max_attempts=2), which completes it, and the round then settles."""
    # Given: the last audit leased by auditor 0, which lets the lease expire.
    world = fork(stages.partial)
    precondition(fin.execute_next(world, 0, 0) is not None, "no audit was left to lease")
    world.network.push(world.network.now + 101)
    # When: auditor 1 asks for the re-queued audit.
    retaken = fin.lease_next(world, 0, 1)
    # Then
    assert retaken.status_code == 200, retaken.text
    second = fin.execute_leased(world, 0, 1, retaken)
    assert fin.complete(world, second).status_code == 200
    settled, _ = settle_audited(world, stages)
    assert settled["status"] == "SETTLED"


def test_contest_opened_after_finalize_neither_mints_nor_pays_twice(
    fork: Callable[[Stage], World], stages: Stages
) -> None:
    """Defence: the excluded cheater's funded contest of its verdict, opened after the round was
    finalized, only moves its bond from available to dispute_locked. It issues no unit, adds no
    origin, and the four honest miners are paid their reward exactly once."""
    # Given: a settled round and its escrow totals.
    world = fork(stages.audited)
    settle_audited(world, stages)
    issued = issued_units(world)
    before = world.observe().escrow
    origins = fin.reward_origins(world)
    # When
    locked, dispute = fin.open_contest(world, CHEATER_INDEX)
    # Then: the contest is accepted as a dispute and nothing was minted.
    assert locked.status_code == 200 and dispute.status_code == 200, (locked.text, dispute.text)
    after = world.observe().escrow
    assert after["issued"] == before["issued"] and after["conserved"] is True
    assert after["dispute_locked"] == 100 and after["available"] == before["available"] - 100
    assert fin.reward_origins(world) == origins
    # And the reward is paid once, to the honest miners only.
    maturity = mature_at(world)
    world.network.push(maturity + 1)
    paid = fin.ask_weights(world, 0, maturity + 2)
    assert paid.json()["weights"] == dict.fromkeys(stages.honest, issued / 4)
    assert fin.ask_weights(world, 0, maturity + 3).content == paid.content
    assert world.observe().escrow["paid"] == issued
    finality = fin.rows(world, "SELECT data FROM records_v2 WHERE kind='finality'")
    note(
        "contest-after-finalize",
        stages,
        world,
        paid,
        unresolved_disputes_while_reward_paid='"unresolved_disputes":1' in finality[0][0],
    )


# -- D. vesting window -----------------------------------------------------------------------------


def test_reward_matures_only_after_its_vesting_window_and_an_early_epoch_stays_unpaid(
    fork: Callable[[Stage], World], stages: Stages
) -> None:
    """Defence: a settled round's reward vests for E_vest_rounds (10 in the fixture) after the
    beacon round it was finalized at and is reward_pending until the master's beacon reaches
    mature_at. An epoch asked before that pays nothing, burns its whole mass and is final (the trap
    of MINE_MULTI_ROUND_V2_BREAK=mature in scripts/mine_multi_round_v2.py): asking it again after
    maturity returns the same bytes, and the reward is paid exactly once, by the next epoch.
    Epoch 1 asked twice returns the same bytes and epoch 2 has nothing left to pay."""
    # Given: a round settled at the current beacon round; its reward is pending.
    world = fork(stages.audited)
    vesting = world.network.manifest.training.verify.E_vest_rounds
    finalized_at = world.network.now
    settle_audited(world, stages)
    maturity = mature_at(world)
    issued = issued_units(world)
    pending = world.observe().escrow
    assert maturity == finalized_at + vesting
    assert pending["reward_pending"] == issued and pending["paid"] == 0
    # When: the beacon stops one round short of maturity and epoch 0 is asked.
    world.network.push(maturity - 1)
    early = fin.ask_weights(world, 0, maturity - 1)
    short = world.observe().escrow
    world.network.push(maturity)
    vested = world.observe().escrow
    late = fin.ask_weights(world, 0, maturity + 1)
    epoch1 = fin.ask_weights(world, 1, maturity + 2)
    epoch2 = fin.ask_weights(world, 2, maturity + 3)
    # Then: nothing matured one round early, everything matured on the round itself.
    assert short["reward_pending"] == issued and short["available"] == pending["available"]
    assert early.status_code == 200 and early.json()["weights"] == {}
    assert burn_facts(early) == (0, FULL_SHARE_MASS, "ok")
    assert vested["reward_pending"] == 0
    assert vested["available"] == pending["available"] + issued
    assert late.content == early.content
    assert epoch1.json()["weights"] == dict.fromkeys(stages.honest, issued / 4)
    assert burn_facts(epoch1) == (issued, FULL_SHARE_MASS - issued, "ok")
    assert fin.ask_weights(world, 1, maturity + 4).content == epoch1.content
    assert epoch2.json()["weights"] == {} and burn_facts(epoch2) == (0, FULL_SHARE_MASS, "ok")
    final = world.observe().escrow
    assert final["paid"] == issued and final["conserved"] is True
    note("vesting-window", stages, world, epoch1, issued_units=issued)


def test_epoch_asked_while_the_master_beacon_lags_maturity_pays_each_unit_once(
    fork: Callable[[Stage], World], stages: Stages
) -> None:
    """Defence: payment follows the beacon the master has pushed (push_beacon matures a reward),
    not the epoch's own time, so an epoch that ends past a reward's mature_at but is asked while
    the master's beacon still lags it (LAG-1) may find the reward unpaid. Whichever epoch pays it,
    the answer is 200 and final (asked again after the beacon caught up it is the same bytes), and
    across this epoch and the next every honest miner is paid its origin exactly once, with the
    escrow conserved. Refusing the ask is no option: the contract is a 200 for every epoch.
    docs/network-v2.md:266-269 promises payment of units with mature_at <= round(epoch_at); it is
    true only once the master's beacon caught up."""
    # Given: a settled round, the master's beacon still before mature_at.
    world = fork(stages.audited)
    settle_audited(world, stages)
    maturity = mature_at(world)
    issued = issued_units(world)
    precondition(world.network.now < maturity, "the master beacon already matured the reward")
    # When: the platform asks for an epoch that ends 5 rounds after maturity, the beacon catches
    # up, the same epoch is asked again and the next epoch is asked.
    lagging = fin.ask_weights(world, 0, maturity + 5)
    world.network.push(maturity + 1)
    again = fin.ask_weights(world, 0, maturity + 6)
    following = fin.ask_weights(world, 1, maturity + 10)
    # Then
    assert lagging.status_code == 200 and following.status_code == 200
    assert again.content == lagging.content
    first, second = lagging.json()["weights"], following.json()["weights"]
    assert {h: first.get(h, 0) + second.get(h, 0) for h in {*first, *second}} == dict.fromkeys(
        stages.honest, issued / 4
    )
    escrow = world.observe().escrow
    assert escrow["paid"] == issued and escrow["conserved"] is True
    note(
        "epoch-asked-while-the-master-beacon-lags",
        stages,
        world,
        following,
        issued_units=issued,
        units_paid_by_the_lagging_epoch=sum(first.values()),
    )
