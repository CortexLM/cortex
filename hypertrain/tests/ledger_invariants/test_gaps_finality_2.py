"""Gaps of the finality group on the staged service (D39, gap wave, second file).

`Stage` (support_finality) is built once per module: five ACTIVE identities, round 0 audited with
one forged commit (four MATCH, one MISMATCH) and aggregated, round 1 audited and aggregated for the
four honest miners. Every test starts from a byte-identical copy of that database. A hole is a
strict xfail whose body asserts the defence after a control that proves the scenario reaches the
point; its reason starts with the finding id.
"""

from __future__ import annotations

import itertools
import json
from collections import Counter
from collections.abc import Iterator
from typing import Any

import pytest
from fastapi.testclient import TestClient

from hypertrain.aggregator.core import FinalityError
from hypertrain.challenge.store import ChallengeError
from hypertrain.ledger import FULL_SHARE_MASS
from hypertrain.protocol.envelope import body_digest

from . import support_exit_roster as X
from .support import at
from .support_finality import ANOTHER, FINAL, Stage, build_stage, refusal
from .support_gaps_finality import credits, finalize_effects, finalize_of, lapse_screen

# -- the staged service ---------------------------------------------------------------------------


@pytest.fixture(scope="module")
def frozen(tmp_path_factory: pytest.TempPathFactory) -> Iterator[Stage]:
    yield from build_stage(tmp_path_factory)


@pytest.fixture
def stage(frozen: Stage) -> Stage:
    frozen.reset()
    return frozen


def honest_hotkeys(stage: Stage) -> list[str]:
    return sorted(key.ss58 for key in stage.honest)


def nothing_was_finalized(stage: Stage) -> None:
    assert stage.rows("SELECT COUNT(*) FROM escrow_finalized") == [(0,)]
    assert stage.rows("SELECT COUNT(*) FROM escrow_events WHERE kind='REWARD_FINALIZE'") == [(0,)]
    assert credits(stage) == Counter()
    assert stage.record("finality", "0")["disposition"] == "PENDING"
    assert stage.records("finalize", "finalize-result", "replayed-finality", "settlement") == []


# --- T-FIN-05 (FIN-04, FIN-23): a finalize under a stale beacon is paused and writes nothing ------


def test_a_finalize_under_a_stale_beacon_is_paused_with_nothing_written_and_settles_later(
    stage: Stage,
) -> None:
    envelope = stage.signed_finalize()
    escrow = stage.store._services(stage.run_id)[0]
    issued = escrow.balances().issued
    genesis = stage.network.manifest.training.beacon.genesis_time
    stage.store.clock = lambda: genesis + (stage.network.now - 1 + 3) * 3
    before = stage.digest()

    paused = stage.post_finalize(0, envelope)

    assert (paused.status_code, paused.json()["detail"]) == (503, "beacon infrastructure paused")
    assert stage.digest() == before
    nothing_was_finalized(stage)
    # Control: once the beacon is fresh again the same bytes settle, once.
    stage.store.clock = stage.clock
    settled = stage.post_finalize(0, envelope)
    assert (settled.status_code, settled.json()["status"]) == (200, "SETTLED")
    assert credits(stage) == Counter({(0, hotkey): 1 for hotkey in honest_hotkeys(stage)})
    assert escrow.balances().issued == issued + escrow.policy.round_reward_units
    assert [kind for kind, _, _ in finalize_effects(stage)].count("finalize") == 1


# --- M-02 (WGT-10, FIN-02): a refused finalize pays nothing in the epoch answer -------------------

LATE = 100_000  # a drand round after every maturity and the end of every epoch asked here
EPOCHS = itertools.count()  # the v1 ledger of the store keeps its answers across Stage.reset()


def epoch_answer(stage: Stage, rnd: int = LATE) -> dict[str, Any]:
    """The answer of `ChallengeStore.weights` for a new epoch that ends at drand round `rnd`.

    Every call asks a new epoch: the in-memory ledger of the store would otherwise remember the
    answer of an earlier test as final, whatever the database was reset to.
    """
    escrow = stage.store._services(stage.run_id)[0]
    answer: dict[str, Any] = json.loads(stage.store.weights(next(EPOCHS), at(escrow, rnd), 1))
    return answer


def pay_events(stage: Stage) -> int:
    return int(stage.rows("SELECT COUNT(*) FROM escrow_events WHERE kind='PAY'")[0][0])


REFUSED_ROUNDS = [
    pytest.param({"unresolved_audits": 1}, id="an audit is open"),
    pytest.param({"unresolved_disputes": 1}, id="a dispute is open"),
    pytest.param({"disposition": "EXCLUDED", "resolution_hash": FINAL}, id="the round is excluded"),
]


def refuse_round(stage: Stage, w: int, change: dict[str, Any] | None) -> None:
    """Make round `w` one a Finalize must refuse: a status that cannot settle, or (without a
    `change`) an applied record of a repair, which is model history and never mints."""
    if change is None:
        stage.put("applied", str(w), stage.record("applied", str(w)) | {"kind": "repair"})
    else:
        stage.put("finality", str(w), stage.record("finality", str(w)) | change)


@pytest.mark.parametrize(
    "change", [*REFUSED_ROUNDS, pytest.param(None, id="the round is a repair")]
)
def test_an_epoch_after_a_refused_finalize_burns_the_whole_share_and_pays_nobody(
    stage: Stage, change: dict[str, Any] | None
) -> None:
    refuse_round(stage, 0, change)
    refused = refusal(stage.finalize)
    X.control(refused[0] in (FinalityError, ChallengeError), f"the finalize was refused: {refused}")

    answer = epoch_answer(stage)

    meta = answer["metadata"]
    assert answer["weights"] == {}
    assert (meta["units_paid"], meta["units_burned_this_epoch"], meta["reason"]) == (
        0,
        FULL_SHARE_MASS,
        "ok",
    )
    epoch = stage.rows("SELECT epoch FROM weights_runs")
    assert len(epoch) == 1 and json.loads(stage.store.weights(epoch[0][0], 1, 1)) == answer
    assert pay_events(stage) == 0 and credits(stage) == Counter()


@pytest.mark.parametrize(
    "change", [*REFUSED_ROUNDS, pytest.param(None, id="the round is a repair")]
)
def test_a_refused_round_adds_no_weight_to_the_epoch_answer_of_a_settled_one(
    stage: Stage, change: dict[str, Any] | None
) -> None:
    stage.finalize(0)
    refuse_round(stage, 1, change)
    refused = refusal(lambda: stage.finalize(1))
    X.control(refused[0] in (FinalityError, ChallengeError), f"round 1 was refused: {refused}")
    escrow = stage.store._services(stage.run_id)[0]
    units = escrow.policy.round_reward_units
    vested = stage.network.now + stage.network.manifest.training.verify.E_vest_rounds
    stage.network.push(vested)  # the beacon reaches the maturity of the round 0 origins
    X.control(
        stage.rows(
            "SELECT COALESCE(SUM(units),0) FROM escrow_units WHERE bucket='available' AND origin "
            "IN (SELECT origin FROM escrow_origins WHERE reward_round=0)"
        )
        == [(units,)],
        "the origins of round 0 are matured and can be paid",
    )

    answer = epoch_answer(stage)

    meta = answer["metadata"]
    # Round 0 alone is emitted: its four origins, in full, and not a unit of the refused round or
    # of the collateral origins of the genesis.
    assert answer["weights"] == {hotkey: units // 4 for hotkey in honest_hotkeys(stage)}
    assert (meta["units_paid"], meta["units_burned_this_epoch"]) == (units, FULL_SHARE_MASS - units)
    assert pay_events(stage) == 4
    assert credits(stage) == Counter({(0, hotkey): 1 for hotkey in honest_hotkeys(stage)})


# --- T-FIN-24 (A2W-21, AD-07, CL-06): a member that lost eligibility does not freeze finalize ----


def test_a_member_whose_screen_lapsed_after_the_aggregation_does_not_freeze_the_finalize(
    stage: Stage,
) -> None:
    lapsed = stage.honest[0].ss58
    applied = sorted(
        item["roster"]["hotkey"] for item in stage.record("tape-inputs", "0")["inputs"]
    )
    X.control(applied == honest_hotkeys(stage), "the tape applied the four audited members")
    lapse_screen(stage, lapsed)
    admission = stage.network.client.get(f"{stage.network.url}/admission/{lapsed}")
    X.control(
        admission.status_code == 200 and admission.json()["eligible"] is False,
        "the work screen of the member lapsed",
    )
    escrow = stage.store._services(stage.run_id)[0]
    issued = escrow.balances().issued

    response = stage.post_finalize(0, stage.signed_finalize(included=applied))

    assert (response.status_code, response.json().get("status")) == (200, "SETTLED"), response.text
    assert credits(stage) == Counter({(0, hotkey): 1 for hotkey in applied})
    assert escrow.balances().issued == issued + escrow.policy.round_reward_units


# --- T-FIN-34 (FIN-26, D-FIN-10): a finality refusal answers 409 with the reason ------------------

INCLUDED_DIFFERS = "finalized included set differs from full accepted authority"


def test_a_refused_finalize_and_a_refused_opening_answer_409_with_their_reason(
    stage: Stage,
) -> None:
    quiet = TestClient(stage.network.client.app, raise_server_exceptions=False)
    headers, url = stage.fixtures.admin(), stage.network.url
    # Control: a refusal the service maps already carries its reason.
    left_out = quiet.post(
        f"{url}/admin/rounds/0/finalize",
        json=stage.signed_finalize(included=honest_hotkeys(stage)[:3]),
        headers=headers,
    )
    X.control(
        left_out.status_code == 422 and left_out.json()["detail"] == INCLUDED_DIFFERS,
        f"the mapped refusal answers with its reason: {left_out.status_code}",
    )

    cannot_settle = quiet.post(
        f"{url}/admin/rounds/0/finalize",
        json=stage.signed_finalize(final_theta_hash_w1=ANOTHER),
        headers=headers,
    )
    blocked = quiet.post(f"{url}/admin/rounds", content=stage.signed_opening(2), headers=headers)

    assert (cannot_settle.status_code, blocked.status_code) == (409, 409)
    assert "round cannot settle" in cannot_settle.json()["detail"]
    assert "w+2 blocked until settlement or exclusion plus rollback" in blocked.json()["detail"]


# --- GAP-01: a detected cheat that is not contested has a consequence beyond the round -----------


def standing(stage: Stage, hotkey: str, coldkey: str) -> tuple[str, int, int]:
    """(admission state, strikes, locked stake) of one identity."""
    row = stage.rows("SELECT state, strikes FROM admissions_v2 WHERE hotkey=?", hotkey)[0]
    escrow = stage.store._services(stage.run_id)[0]
    return str(row[0]), int(row[1]), escrow.balances(coldkey).admission_locked


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="GAP-01 [decision: what the penalty is]: a miner whose forged commit the master replay "
    "refuted, and who opens no contest, keeps an ACTIVE admission, zero strikes and its full "
    "stake: the only consequence is that its own round is not paid, so repeating the forgery in "
    "every round costs it nothing [fix-lot: audit-integrity-2 ADR0011-P8 D47]",
)
def test_a_detected_cheat_that_is_not_contested_has_a_consequence_beyond_the_round(
    stage: Stage,
) -> None:
    forger, cold = stage.forger.ss58, stage.fixtures.COLD
    verdict = stage.record("verdict", f"0:{forger}")["body"]["result"]
    X.control(
        verdict == "MISMATCH" and [i for _, i, _ in stage.records("excluded")] == [f"0:{forger}"],
        "the master replay refuted the forger's commit and the round excluded it",
    )
    stage.finalize(0)  # the round settles without it
    # A penalty may wait for the window in which the forger could contest the verdict.
    window = stage.network.manifest.training.verify.T.dispute_per_level
    stage.network.push(stage.network.now + 2 * window)
    honest = [standing(stage, k.ss58, cold[i].ss58) for i, k in enumerate(stage.honest)]
    X.control(len(set(honest)) == 1, f"the honest four stand alike: {honest}")

    forged = standing(stage, forger, cold[4].ss58)

    assert honest[0][:2] == ("ACTIVE", 0)  # nobody else is touched
    assert forged != honest[0], f"the cheat left no trace in its standing: {forged}"


# --- T-FIN-18 (CRD-04, FIN-08, DIB-21): the round after two repaired rounds is minted -----------


def repaired(stage: Stage, w: int) -> None:
    """Round `w` as a completed rollback leaves it (store.py rollback_v2): the applied record is
    model history of kind `repair`, and the round is EXCLUDED with its rollback complete, which
    counts as closed. A test-owned stand-in: the service's own two-round repair is a 150 s run,
    and nothing below reads how the repaired model was computed."""
    stage.put("applied", str(w), stage.record("applied", str(w)) | {"kind": "repair"})
    stage.put(
        "finality",
        str(w),
        stage.record("finality", str(w))
        | {
            "disposition": "EXCLUDED",
            "rollback_complete": True,
            "resolution_hash": body_digest({"cause": "a seeded referee resolution"}),
        },
    )


def test_the_round_after_two_repaired_rounds_is_minted_and_the_repaired_ones_never_are(
    stage: Stage,
) -> None:
    for w in (0, 1):
        repaired(stage, w)
    escrow = stage.store._services(stage.run_id)[0]
    before = escrow.balances()
    third = stage.rollback.execute_round(stage.network, 2, hotkeys=stage.honest)
    status = stage.record("finality", "2")
    X.control(
        (status["applied"], status["unresolved_audits"], status["unresolved_disputes"])
        == (True, 0, 0)
        and stage.record("applied", "1")["kind"] == "repair",
        "round 2 is audited and applied on top of two repaired rounds",
    )
    refused = stage.post_finalize(0, stage.signed_finalize(0))
    X.control(
        (refused.status_code, refused.json()["detail"])
        == (409, "repair evidence is model-only; cannot mint miner rewards")
        and escrow.balances() == before,
        "a repaired round does not mint",
    )

    minted = stage.post_finalize(2, finalize_of(stage, 2, third["theta_hash"]))

    assert (minted.status_code, minted.json().get("status")) == (200, "SETTLED"), minted.text
    assert credits(stage) == Counter({(2, hotkey): 1 for hotkey in honest_hotkeys(stage)})
    assert escrow.balances().issued == before.issued + escrow.policy.round_reward_units
