"""Consequences of a caught cheat that need more than one round.

Two stories drive the real v2 path on CPU with the tiny model, each once per module:

* replay: round 0 with five honest miners; round 1 where identity 4 resubmits its own round-0
  product with the leaves relabelled to round 1 (cheat 2) while the four others stay honest;
  round 2, opened for the four honest miners, where identity 3 resubmits its round-1 leaves
  unchanged (the second replay variant needs a miner whose round-1 leaves exist and a round that
  can still be opened, which a round-1 roster holding the relabelled replayer cannot offer).
* continuation: round 0 where identity 4 copies a peer's upload (cheat 4) and is excluded;
  round 1 for the four survivors.

The audit-level proof of both doubles is in test_cheats_audit.py, and the missing sanction of a
caught round cheater is AC-SANCTION in test_missing_sanctions_and_wedges.py. The tests here assert
what only a second round can show: intake of a replay, retries of a settled round's envelopes,
payment of the following rounds, anchors, and who can be rostered again.
"""

from __future__ import annotations

import time
from dataclasses import dataclass
from typing import Any

import pytest
from ac_cheats import BY_NAME
from ac_multiround import (
    Probe,
    accepted_receipt,
    commit_resigned_with_another_claim,
    post_round_opening_of_previous_roster,
    post_training,
    record_ids,
    stored_envelope,
    weights_of_the_next_epoch,
)
from ac_world import MinerOutcome, Observation, Settlement, Weights, World, record

ROSTER = list(range(5))
SURVIVORS = list(range(4))
CHEATER = 4
RELABELLED = BY_NAME["replay-previous-round-relabelled"]
VERBATIM = BY_NAME["replay-previous-round-verbatim"]
COPIED = BY_NAME["copied-peer-upload-resigned"]
FULL_MASS = 1_000_000  # units one epoch pays
FIFTH = FULL_MASS / 5  # five equal miners, one round
ACTIVE_CLEAN = {"state": "ACTIVE", "strikes": 0, "pending_dispute": 0}


@dataclass(frozen=True)
class ReplayStory:
    hotkeys: tuple[str, ...]
    anchors_after_round0: set[str]
    receipts: dict[str, dict[str, Any]]
    reposted: dict[str, Probe]
    resigned: Probe
    commit_before_resigned: dict[str, Any]
    commit_after_resigned: dict[str, Any]
    outcomes: list[MinerOutcome]
    round1: Settlement
    anchors_final: set[str]
    verbatim: MinerOutcome
    leaves_before_verbatim: set[str]
    leaves_after_verbatim: set[str]
    paid_round0: Weights
    paid_round1: Weights
    seen: Observation
    seconds: float


@dataclass(frozen=True)
class ContinuationStory:
    hotkeys: tuple[str, ...]
    outcomes0: list[MinerOutcome]
    round0: Settlement
    anchors_after_round0: set[str]
    unanchored: Probe
    outcomes1: list[MinerOutcome]
    round1: Settlement
    paid_round0: Weights
    paid_round1: Weights
    seen: Observation
    seconds: float


def settled(settlement: Settlement) -> Settlement:
    assert settlement.settled, [(s.name, s.status, s.detail) for s in settlement.steps]
    return settlement


def drive_replay(world: World) -> ReplayStory:
    started = time.monotonic()
    hotkeys = tuple(world.hotkey(i) for i in ROSTER)
    cheater = hotkeys[CHEATER]

    world.open_round(0)
    world.run_roster(dict.fromkeys(ROSTER), 0)
    settled(world.settle(0))
    anchors = record_ids(world, "anchor")

    world.open_round(1)
    envelopes = {k: stored_envelope(world, k, 0, cheater) for k in ("commit", "delta")}
    receipts = {k: accepted_receipt(world, e) for k, e in envelopes.items()}
    reposted = {k: post_training(world, k, e) for k, e in envelopes.items()}
    commit_before = stored_envelope(world, "commit", 0, cheater)
    resigned = post_training(world, "commit", commit_resigned_with_another_claim(world, CHEATER, 0))
    commit_after = stored_envelope(world, "commit", 0, cheater)

    outcomes = world.run_roster({**dict.fromkeys(SURVIVORS), CHEATER: RELABELLED}, 1)
    round1 = world.settle(1)

    leaves_before = record_ids(world, "leaves")
    world.open_round(2, SURVIVORS)
    verbatim = world.run_miner(3, VERBATIM, 2)
    leaves_after = record_ids(world, "leaves")
    anchors_final = record_ids(world, "anchor")

    paid0 = world.weights()
    paid1 = weights_of_the_next_epoch(world)
    return ReplayStory(
        hotkeys,
        anchors,
        receipts,
        reposted,
        resigned,
        commit_before,
        commit_after,
        outcomes,
        round1,
        anchors_final,
        verbatim,
        leaves_before,
        leaves_after,
        paid0,
        paid1,
        world.observe(),
        time.monotonic() - started,
    )


def drive_continuation(world: World) -> ContinuationStory:
    started = time.monotonic()
    hotkeys = tuple(world.hotkey(i) for i in ROSTER)

    world.open_round(0)
    outcomes0 = world.run_roster({**dict.fromkeys(SURVIVORS), CHEATER: COPIED}, 0)
    round0 = settled(world.settle(0))
    assert round0.finalize_hash is not None
    anchors = record_ids(world, "anchor")

    unanchored = post_round_opening_of_previous_roster(world, 1, round0.finalize_hash)

    world.open_round(1, SURVIVORS)
    outcomes1 = world.run_roster(dict.fromkeys(SURVIVORS), 1)
    round1 = world.settle(1)

    paid0 = world.weights()
    paid1 = weights_of_the_next_epoch(world)
    return ContinuationStory(
        hotkeys,
        outcomes0,
        round0,
        anchors,
        unanchored,
        outcomes1,
        round1,
        paid0,
        paid1,
        world.observe(),
        time.monotonic() - started,
    )


def run_story(snapshot: Any, factory: pytest.TempPathFactory, name: str, drive: Any) -> Any:
    world = snapshot.clone(factory.mktemp(name))
    try:
        with world:
            return drive(world)
    finally:
        world.close()


@pytest.fixture(scope="module")
def replay(ac_snapshot: Any, tmp_path_factory: pytest.TempPathFactory) -> ReplayStory:
    return run_story(ac_snapshot, tmp_path_factory, "replay", drive_replay)


@pytest.fixture(scope="module")
def continuation(ac_snapshot: Any, tmp_path_factory: pytest.TempPathFactory) -> ContinuationStory:
    return run_story(ac_snapshot, tmp_path_factory, "continuation", drive_continuation)


def honest(story: ReplayStory | ContinuationStory) -> list[str]:
    return [h for i, h in enumerate(story.hotkeys) if i != CHEATER]


def standing(seen: Observation, hotkeys: list[str]) -> dict[str, dict[str, Any]]:
    return {h: {k: seen.admissions[h][k] for k in ACTIVE_CLEAN} for h in hotkeys}


# --- cheat 2: a replayed result ------------------------------------------------------------------


def test_relabelled_replay_passes_intake_and_the_audit_catches_it_at_leaf_zero(
    replay: ReplayStory,
) -> None:
    """Defence: the relabelled round-0 product of a miner is well-formed for round 1 and its leaves
    root matches its commit, so intake accepts it; the audit replays from the round-1 start state,
    is MISMATCH at the first leaf and excludes the replayer alone."""
    cheater = replay.hotkeys[CHEATER]
    intake = replay.outcomes[CHEATER]
    verdicts = {v.hotkey: v for v in replay.round1.verdicts}
    record(
        {
            "test": "replayed-round-relabelled",
            "cheat": RELABELLED.cheat,
            "double": RELABELLED.name,
            "seam": RELABELLED.seam,
            "intake": {"status": intake.status, "http_status": intake.http_status},
            "verdict": verdicts[cheater].result,
            "first_bad_leaf": verdicts[cheater].first_bad_leaf,
            "excluded": replay.seen.excluded.get(f"1:{cheater}"),
            "settled": replay.round1.settled,
            "cheater_weight": replay.paid_round1.weights.get(cheater),
            "honest_weights": {h: w for h, w in replay.paid_round1.weights.items() if h != cheater},
            "cheater_admission": replay.seen.admissions[cheater],
            "honest_admissions": {h: replay.seen.admissions[h]["state"] for h in honest(replay)},
            "hole": "AC-SANCTION",
            "story_seconds": round(replay.seconds, 1),
        }
    )

    assert [o.status for o in replay.outcomes] == ["UPLOADED"] * 5
    assert replay.round1.results == {**dict.fromkeys(honest(replay), "MATCH"), cheater: "MISMATCH"}
    assert {h: v.first_bad_leaf for h, v in verdicts.items()} == {
        **dict.fromkeys(honest(replay)),
        cheater: 0,
    }
    assert all(v.completed for v in verdicts.values())
    assert list(replay.seen.excluded) == [f"1:{cheater}"]
    assert replay.seen.excluded[f"1:{cheater}"]["reason"] == "UNVERIFIED"


def test_round_holding_a_replay_settles_and_pays_the_honest_miners_equally(
    replay: ReplayStory,
) -> None:
    """Defence: a caught replay costs the honest miners nothing. The round still settles with the
    four honest miners included, they are paid equally and never less than for the honest round
    before, the replayer earns nothing from round 1, the epoch pays no more than its mass and every
    unit it does not pay is burned, and no honest admission is touched.

    Whether the slice of the replayer burns or goes to its peers is not asserted: docs/mechanism.md
    section 4 (v1 ledger) and finalize_v2 disagree and no v2 document decides (AC-REDISTRIBUTION,
    an ADR item). The observed split is recorded."""
    cheater = replay.hotkeys[CHEATER]
    round1_pay = replay.paid_round1.weights
    burned = replay.paid_round1.metadata["units_burned_this_epoch"]
    record(
        {
            "test": "round-after-a-caught-replayer-is-paid",
            "cheat": RELABELLED.cheat,
            "double": RELABELLED.name,
            "weights_round0": replay.paid_round0.weights,
            "honest_weights_round1": round1_pay,
            "units_burned_this_epoch": burned,
            "slice_of_the_replayer": "paid to its peers" if burned == 0 else "burned",
            "hole": "AC-REDISTRIBUTION",
        }
    )

    assert replay.round1.settled
    assert list(replay.round1.included) == sorted(honest(replay))
    assert replay.paid_round0.weights == dict.fromkeys(replay.hotkeys, FIFTH)
    assert set(round1_pay) == set(honest(replay)) and cheater not in round1_pay
    assert len(set(round1_pay.values())) == 1
    assert all(round1_pay[h] >= replay.paid_round0.weights[h] for h in honest(replay))
    assert sum(round1_pay.values()) + burned == FULL_MASS
    assert replay.seen.escrow["conserved"] is True
    assert standing(replay.seen, honest(replay)) == dict.fromkeys(honest(replay), ACTIVE_CLEAN)


def test_stored_round_zero_envelopes_posted_again_return_their_receipts_and_change_nothing(
    replay: ReplayStory,
) -> None:
    """Defence: after round 0 is settled a miner's retry of its own accepted commit or delta
    envelope is answered with the receipt it got the first time and writes no row, so it cannot
    be taken for a second acceptance."""
    for kind in ("commit", "delta"):
        probe = replay.reposted[kind]
        assert probe.answer.status == 200
        assert probe.answer.body == replay.receipts[kind]
        assert probe.changed_nothing


def test_round_zero_commit_signed_again_with_another_claim_is_refused_and_changes_nothing(
    replay: ReplayStory,
) -> None:
    """Defence: a miner cannot replace its accepted commit after the fact. The same commit signed
    again with another final_theta_hash is refused as a conflict; the stored commit is the first."""
    assert replay.resigned.answer.status == 409
    assert replay.resigned.answer.detail == "conflicting signed operation nonce"
    assert replay.resigned.changed_nothing
    assert replay.commit_after_resigned == replay.commit_before_resigned


def test_verbatim_replay_is_refused_at_leaves_and_records_nothing(replay: ReplayStory) -> None:
    """Defence: leaves that still say the earlier round are refused when the miner publishes them
    for a later one (422 at POST /leaves), and the refusal stores no leaves object."""
    refused = replay.verbatim
    record(
        {
            "test": "replayed-round-verbatim",
            "cheat": VERBATIM.cheat,
            "double": VERBATIM.name,
            "seam": VERBATIM.seam,
            "intake": {
                "status": refused.status,
                "http_status": refused.http_status,
                "endpoint": refused.endpoint,
                "detail": refused.detail,
            },
        }
    )

    assert refused.status == "refused"
    assert (refused.http_status, refused.endpoint) == (422, "POST /leaves")
    assert refused.detail == "leaf object does not match accepted commit"
    assert replay.leaves_after_verbatim == replay.leaves_before_verbatim


def test_miners_honest_in_a_round_keep_their_anchors_and_a_caught_replayer_gets_none(
    replay: ReplayStory,
) -> None:
    """Defence: a MATCH leaves the miner its anchor, a MISMATCH does not. Every miner of the honest
    round 0 holds a round-0 anchor; of round 1, which holds the replay, exactly the four honest
    miners hold a round-1 anchor (the next opening restores a miner's start state from it)."""
    cheater = replay.hotkeys[CHEATER]
    round1 = {a for a in replay.anchors_final if a.startswith("1:")}

    assert {f"0:{h}" for h in replay.hotkeys} <= replay.anchors_after_round0
    assert round1 == {f"1:{h}" for h in honest(replay)}
    assert f"1:{cheater}" not in replay.anchors_final


# --- cheat 4: an excluded miner and the rounds that follow ----------------------------------------


def test_survivors_of_a_round_with_an_exclusion_run_the_next_round_and_keep_their_pay(
    continuation: ContinuationStory,
) -> None:
    """Defence: the network goes on after an exclusion. The four survivors open round 1, run it,
    settle it and are paid for it, with no admission touched."""
    c = continuation
    cheater = c.hotkeys[CHEATER]
    intake = c.outcomes0[CHEATER]
    verdict = next(v for v in c.round0.verdicts if v.hotkey == cheater)
    record(
        {
            "test": "copied-peer-then-continuation",
            "cheat": COPIED.cheat,
            "double": COPIED.name,
            "seam": COPIED.seam,
            "intake": {"status": intake.status, "http_status": intake.http_status},
            "verdict": verdict.result,
            "first_bad_leaf": verdict.first_bad_leaf,
            "excluded": c.seen.excluded.get(f"0:{cheater}"),
            "settled": [c.round0.settled, c.round1.settled],
            "cheater_weight": c.paid_round0.weights.get(cheater),
            "honest_weights": [c.paid_round0.weights, c.paid_round1.weights],
            "cheater_admission": c.seen.admissions[cheater],
            "honest_admissions": {h: c.seen.admissions[h]["state"] for h in honest(c)},
            "hole": "AC-SANCTION",
            "story_seconds": round(c.seconds, 1),
        }
    )

    assert [o.status for o in c.outcomes0] == ["UPLOADED"] * 5
    assert c.round0.results == {**dict.fromkeys(honest(c), "MATCH"), cheater: "MISMATCH"}
    assert list(c.seen.excluded) == [f"0:{cheater}"]
    assert c.seen.excluded[f"0:{cheater}"]["reason"] == "UNVERIFIED"
    assert [o.status for o in c.outcomes1] == ["UPLOADED"] * 4
    assert c.round1.results == dict.fromkeys(honest(c), "MATCH")
    assert c.round1.settled and list(c.round1.included) == sorted(honest(c))
    # Valid whether the slice of the caught copier burns or goes to its peers: an epoch pays at most
    # its mass and only the survivors, the survivors earn the same over the two epochs (which of
    # them an epoch pays first depends on that choice), and never less than two honest fifths.
    epochs = (c.paid_round0, c.paid_round1)
    totals = {h: sum(paid.weights.get(h, 0.0) for paid in epochs) for h in honest(c)}
    assert all(set(paid.weights) <= set(honest(c)) for paid in epochs)
    assert len(set(totals.values())) == 1
    assert all(total >= 2 * FIFTH for total in totals.values())
    for paid in epochs:
        assert sum(paid.weights.values()) + paid.metadata["units_burned_this_epoch"] == FULL_MASS
    assert c.seen.escrow["conserved"] is True
    assert standing(c.seen, honest(c)) == dict.fromkeys(honest(c), ACTIVE_CLEAN)


def test_excluded_cheater_holds_no_anchor_and_cannot_be_rostered_in_the_next_round(
    continuation: ContinuationStory,
) -> None:
    """Defence: a non-MATCH round persists no anchor for its miner, so a round-1 opening that
    rosters the excluded miner is refused by the master (409 missing accepted anchor) and leaves
    no row behind.

    The no-row check is also guarded by the story, not only by this test: a refusal that committed
    its nonce would make the survivors' opening of round 1 answer 409 "conflicting signed operation
    nonce" and error the fixture (reproduced by letting the store's transaction commit on that
    refusal), so such a regression shows up as an error of every test of the story."""
    c = continuation
    cheater = c.hotkeys[CHEATER]

    assert {f"0:{h}" for h in honest(c)} <= c.anchors_after_round0
    assert f"0:{cheater}" not in c.anchors_after_round0
    assert c.unanchored.answer.status == 409
    assert c.unanchored.answer.detail == f"missing accepted anchor: 0:{cheater}"
    assert c.unanchored.changed_nothing
