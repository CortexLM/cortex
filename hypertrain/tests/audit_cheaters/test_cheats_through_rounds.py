"""Cheating miners through whole v2 rounds: where each cheat is caught and what the ledger does.

Every scenario opens round 0 for the five graduated identities, runs the honest miners 0-3 first
(a copier needs a peer), then the cheater as miner 4, and drives the real path from the miner
through intake, audit, aggregate, finalize and get_weights. The verdict always comes from the
production audit via World.settle(). The audit-level behaviour of each double is proven in
test_cheats_audit.py; the relay and admission defences are in the other files of this directory.

The fixture manifest has H=1, J=1, so a round has two leaves (t=0 and t=1): a skipped optimizer
step, a wrong shard, a wrong rng counter or an altered last step first show in leaf 1, a wrong
start state in leaf 0. The multi-step positions are proven at audit level (H=4).
"""

from __future__ import annotations

import hypertrain.trainer  # noqa: F401

# isort: split
import contextlib
import json
from collections.abc import Iterator
from dataclasses import dataclass
from typing import Any

import pytest
from ac_cheats import BY_NAME
from ac_world import MinerOutcome, Observation, Settlement, Weights, World, record

HONEST_MINERS = (0, 1, 2, 3)
CHEATER = 4
FULL_MASS = 1_000_000
# What each of five honest miners earns in a round: pinned by the inert case below.
FIFTH = FULL_MASS // 5
INTAKE_KINDS = ("AcceptV2", "CommitV2", "DeltaManifestV2", "WorkProof")
INTAKE_RECORDS = ("accept", "commit", "delta", "leaves")
# The hole itself is the xfail of test_missing_sanctions_and_wedges.py.
SANCTION_HOLE = "AC-SANCTION"


@dataclass(frozen=True)
class Caught:
    """A cheat that intake accepts and the audit catches at `first_bad_leaf`."""

    name: str
    first_bad_leaf: int | None


CAUGHT = (
    Caught("altered-weights-one-ulp", 1),
    Caught("forged-delta-valid-hash", None),
    Caught("copied-peer-upload-resigned", 1),
    Caught("skipped-optimizer-step", 1),
    Caught("wrong-data-shard", 1),
    Caught("wrong-seed-forged-rng-ctr", 1),
    Caught("wrong-initial-theta", 0),
    Caught("commit-lie-final-theta-hash", None),
    Caught("commit-lie-ef-out-hash", None),
)


@dataclass(frozen=True)
class Refusal:
    """A cheat that the master refuses at intake; `kinds` and `records` are what the cheater had
    legitimately posted before the refused call, hence all the master may hold for it."""

    name: str
    endpoint: str
    status: int
    detail: str
    kinds: tuple[str, ...]
    records: tuple[str, ...]


REFUSALS = (
    Refusal(
        "commit-lie-leaves-root",
        "POST /leaves",
        422,
        "leaf object does not match accepted commit",
        ("AcceptV2", "CommitV2"),
        ("accept", "commit"),
    ),
    Refusal(
        "commit-lie-delta-hash",
        "POST /upload-grant",
        409,
        "one current signed relay assignment required",
        ("AcceptV2", "CommitV2", "WorkProof"),
        ("accept", "commit", "leaves"),
    ),
    Refusal(
        "commit-lie-delta-bytes",
        "POST /upload-grant",
        409,
        "one current signed relay assignment required",
        ("AcceptV2", "CommitV2", "WorkProof"),
        ("accept", "commit", "leaves"),
    ),
)


@dataclass(frozen=True)
class CheatRound:
    """One settled round: four honest miners and the cheater, everything a scenario asserts on."""

    cheat: str
    honest: tuple[str, ...]
    cheater: str
    outcomes: list[MinerOutcome]
    settlement: Settlement
    paid: Weights
    seen: Observation
    reward_origins: dict[str, int]

    @property
    def verdict(self) -> Any:
        return next(v for v in self.settlement.verdicts if v.hotkey == self.cheater)

    @property
    def cheater_admission(self) -> dict[str, Any]:
        return self.seen.admissions[self.cheater]


def play(world: World, cheat_name: str) -> CheatRound:
    """Open round 0 for all five identities, run 0-3 honestly, then the cheater, then settle."""
    world.open_round(0)
    cheats: dict[int, Any] = dict.fromkeys(HONEST_MINERS)
    cheats[CHEATER] = BY_NAME[cheat_name]
    outcomes = world.run_roster(cheats)
    settlement = world.settle(0)
    paid = world.weights()
    seen = world.observe()
    origins = {
        hotkey: issued
        for hotkey, issued in world.network.store._db.execute(
            "SELECT hotkey,issued FROM escrow_origins WHERE reward_round IS NOT NULL"
        )
    }
    return CheatRound(
        cheat_name,
        tuple(sorted(world.hotkey(i) for i in HONEST_MINERS)),
        world.hotkey(CHEATER),
        outcomes,
        settlement,
        paid,
        seen,
        origins,
    )


def intake_trace(world: World, hotkey: str) -> tuple[list[str], list[str]]:
    """What the master holds for `hotkey`: its accepted_v2 keys (replay reservations and the
    authentication nonces) and the kinds of its intake records of round 0."""
    db = world.network.store._db
    keys = sorted(
        key
        for (key,) in db.execute("SELECT key FROM accepted_v2")
        if json.loads(key.removeprefix("auth:"))[2] == hotkey
    )
    records = sorted(
        kind
        for (kind,) in db.execute(
            "SELECT kind FROM records_v2 WHERE run_id=? AND id=? AND kind IN (?,?,?,?)",
            (world.run_id, f"0:{hotkey}", *INTAKE_RECORDS),
        )
    )
    return keys, records


def kinds_of(keys: list[str]) -> list[str]:
    return sorted({json.loads(key.removeprefix("auth:"))[1] for key in keys})


@contextlib.contextmanager
def real_server_errors(world: World) -> Iterator[None]:
    """Answer an uncaught server exception with the bare 500 a real client receives.

    The TestClient re-raises it instead, which the miner driver would record as an `error`.
    """
    transport = world.network.client._transport
    previous = transport.raise_server_exceptions
    transport.raise_server_exceptions = False
    try:
        yield
    finally:
        transport.raise_server_exceptions = previous


def evidence(round_: CheatRound, hole: str | None = SANCTION_HOLE) -> dict[str, Any]:
    cheat = BY_NAME[round_.cheat]
    outcome = round_.outcomes[CHEATER]
    verdict = next((v for v in round_.settlement.verdicts if v.hotkey == round_.cheater), None)
    excluded = round_.seen.excluded.get(f"0:{round_.cheater}")
    return {
        "test": "cheat-through-round",
        "cheat": cheat.cheat,
        "name": cheat.name,
        "seam": cheat.seam,
        "intake": {
            "status": outcome.status,
            "endpoint": outcome.endpoint,
            "http_status": outcome.http_status,
            "detail": outcome.detail,
        },
        "verdict": None if verdict is None else verdict.result,
        "first_bad_leaf": None if verdict is None else verdict.first_bad_leaf,
        "excluded_reason": None if excluded is None else excluded["reason"],
        "settled": round_.settlement.settled,
        "cheater_weight": round_.paid.weights.get(round_.cheater, 0.0),
        "honest_weights": sorted(round_.paid.weights.get(h, 0.0) for h in round_.honest),
        "total_paid": sum(round_.paid.weights.values()),
        "reward_origins": dict(sorted(round_.reward_origins.items())),
        "cheater_admission": {
            k: round_.cheater_admission[k] for k in ("state", "strikes", "pending_dispute")
        },
        "honest_effect": {
            "states": sorted({round_.seen.admissions[h]["state"] for h in round_.honest}),
            "strikes": sorted({round_.seen.admissions[h]["strikes"] for h in round_.honest}),
            "excluded_records": [h for h in round_.honest if f"0:{h}" in round_.seen.excluded],
        },
        "escrow_conserved": round_.seen.escrow["conserved"],
        "hole": hole,
    }


def assert_honest_unaffected(round_: CheatRound) -> None:
    for hotkey in round_.honest:
        admission = round_.seen.admissions[hotkey]
        assert (admission["state"], admission["strikes"], admission["pending_dispute"]) == (
            "ACTIVE",
            0,
            0,
        ), hotkey
        assert f"0:{hotkey}" not in round_.seen.excluded
    assert round_.seen.escrow["conserved"] is True


def assert_no_sanction_state_change(world: World, hotkeys: list[str]) -> None:
    """A refusal is not an audit result: nobody is excluded, suspended or struck for it."""
    seen = world.observe()
    assert seen.excluded == {}
    for hotkey in hotkeys:
        admission = seen.admissions[hotkey]
        assert (admission["state"], admission["strikes"], admission["pending_dispute"]) == (
            "ACTIVE",
            0,
            0,
        )


@pytest.mark.parametrize("case", CAUGHT, ids=lambda case: case.name)
def test_cheat_accepted_at_intake_is_caught_at_audit_and_earns_nothing(
    ac_clone: World, case: Caught
) -> None:
    """Defence: a cheat that intake cannot see is MISMATCH at the audit, excluded as UNVERIFIED,
    and gets no reward origin, while the round still settles for the four honest miners, who
    are paid equally, no less than in an all-honest round, and keep a clean admission.

    Accepted at intake and caught at audit is the expected stage for every row of this table.
    Whether the caught miner's slice burns or goes to its peers is undecided (docs/mechanism.md
    section 4 against finalize_v2), so only what holds under both designs is asserted; the
    observed amounts are in the evidence record.
    """
    round_ = play(ac_clone, case.name)
    record(evidence(round_))

    assert [o.status for o in round_.outcomes] == ["UPLOADED"] * 5
    assert round_.settlement.results == {
        **dict.fromkeys(round_.honest, "MATCH"),
        round_.cheater: "MISMATCH",
    }
    assert all(v.completed for v in round_.settlement.verdicts)
    assert round_.verdict.first_bad_leaf == case.first_bad_leaf
    assert [v.first_bad_leaf for v in round_.settlement.verdicts if v.result == "MATCH"] == [
        None
    ] * 4
    assert {row["hotkey"]: row["result"] for row in round_.seen.leases} == {
        **dict.fromkeys(round_.honest, "MATCH"),
        round_.cheater: "MISMATCH",
    }
    assert list(round_.seen.excluded) == [f"0:{round_.cheater}"]
    assert round_.seen.excluded[f"0:{round_.cheater}"]["reason"] == "UNVERIFIED"
    assert round_.settlement.settled, round_.settlement.stalled_at
    assert round_.settlement.included == round_.honest
    assert round_.paid.status_code == 200
    honest_paid = [round_.paid.weights.get(h, 0.0) for h in round_.honest]
    assert len(set(honest_paid)) == 1 and honest_paid[0] >= FIFTH, honest_paid
    assert {h: w for h, w in round_.paid.weights.items() if h not in round_.honest and w} == {}
    assert sum(round_.paid.weights.values()) <= FULL_MASS
    assert round_.reward_origins.get(round_.cheater, 0) == 0
    assert [round_.reward_origins.get(h, 0) for h in round_.honest] == honest_paid
    assert_honest_unaffected(round_)


def test_altered_state_file_alone_is_accepted_matched_and_paid_like_an_honest_miner(
    ac_clone: World,
) -> None:
    """Inert case, the 'no harm done' row: editing only state.safetensors after the commitments
    were computed changes nothing the network consumes (the audit recomputes the state; nobody
    reads the file back), so the miner is UPLOADED, MATCH and paid exactly as the honest four."""
    round_ = play(ac_clone, "altered-weights-state-file-only")
    record(evidence(round_, hole=None))

    assert [o.status for o in round_.outcomes] == ["UPLOADED"] * 5
    assert round_.settlement.results == dict.fromkeys((*round_.honest, round_.cheater), "MATCH")
    assert round_.verdict.first_bad_leaf is None
    assert round_.seen.excluded == {}
    assert round_.settlement.settled, round_.settlement.stalled_at
    assert len(round_.settlement.included) == 5
    everyone = (*round_.honest, round_.cheater)
    assert round_.paid.weights == dict.fromkeys(everyone, FULL_MASS / 5)
    assert round_.reward_origins == dict.fromkeys(everyone, FULL_MASS // 5)
    assert_honest_unaffected(round_)
    assert round_.cheater_admission["state"] == "ACTIVE"


@pytest.mark.parametrize("case", REFUSALS, ids=lambda case: case.name)
def test_a_lie_the_master_can_check_at_intake_is_refused_and_leaves_no_trace(
    ac_clone: World, case: Refusal
) -> None:
    """Defence: a commit that its own follow-up uploads contradict is refused at the stage that
    can see it, with a 4xx and a reason; the refused kind leaves no record and no replay
    reservation (the nonce is not burned), and the four honest miners' intake state is the same
    before and after the attempt. The round is not settled here."""
    world = ac_clone
    cheater = world.hotkey(CHEATER)
    world.open_round(0)
    honest = [world.hotkey(i) for i in HONEST_MINERS]
    uploaded = world.run_roster(dict.fromkeys(HONEST_MINERS))
    before = {hotkey: intake_trace(world, hotkey) for hotkey in honest}

    outcome = world.run_miner(CHEATER, BY_NAME[case.name])
    keys, records = intake_trace(world, cheater)
    record(
        {
            "test": "cheat-refused-at-intake",
            "name": case.name,
            "cheat": BY_NAME[case.name].cheat,
            "seam": BY_NAME[case.name].seam,
            "intake": {
                "status": outcome.status,
                "endpoint": outcome.endpoint,
                "http_status": outcome.http_status,
                "detail": outcome.detail,
            },
            "cheater_records": records,
            "cheater_reservations": len(keys),
            "hole": None,
        }
    )

    assert [o.status for o in uploaded] == ["UPLOADED"] * 4
    assert (outcome.status, outcome.endpoint, outcome.http_status, outcome.detail) == (
        "refused",
        case.endpoint,
        case.status,
        case.detail,
    )
    assert records == list(case.records)
    assert kinds_of(keys) == list(case.kinds)
    for hotkey in honest:
        assert intake_trace(world, hotkey) == before[hotkey]
        assert kinds_of(before[hotkey][0]) == list(INTAKE_KINDS)
        assert before[hotkey][1] == list(INTAKE_RECORDS)
    assert_no_sanction_state_change(world, honest + [cheater])


def test_a_truncated_step_count_records_nothing_at_commit(ac_clone: World) -> None:
    """Defence: a commit with one leaf fewer than the manifest's H//J + 1 is not accepted: the
    miner stays at its accept, no commit record and no commit reservation exist, the honest
    four are untouched. The answer is recorded as a client sees it (see real_server_errors);
    that it is a bare 500 today is the xfail below."""
    world = ac_clone
    cheater = world.hotkey(CHEATER)
    world.open_round(0)
    honest = [world.hotkey(i) for i in HONEST_MINERS]
    world.run_roster(dict.fromkeys(HONEST_MINERS))
    before = {hotkey: intake_trace(world, hotkey) for hotkey in honest}

    with real_server_errors(world):
        outcome = world.run_miner(CHEATER, BY_NAME["truncated-step-count"])
    keys, records = intake_trace(world, cheater)
    record(
        {
            "test": "truncated-step-count-intake",
            "name": "truncated-step-count",
            "cheat": 5,
            "seam": "artifacts",
            "intake": {
                "status": outcome.status,
                "endpoint": outcome.endpoint,
                "http_status": outcome.http_status,
                "detail": outcome.detail,
            },
            "cheater_records": records,
            "cheater_reservations": len(keys),
            "hole": "AC-COMMIT-500",
        }
    )

    assert (outcome.status, outcome.endpoint) == ("refused", "POST /commit"), outcome.error
    assert records == ["accept"]
    assert kinds_of(keys) == ["AcceptV2"]
    for hotkey in honest:
        assert intake_trace(world, hotkey) == before[hotkey]
    assert_no_sanction_state_change(world, honest + [cheater])


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="AC-COMMIT-500 (MINEUR): CommitV2.validate_assignment raises a bare ValueError "
    "('leaf count differs from manifest', store.py:6183) that app.py:425-432 does not map, so a "
    "malformed commit is a bare 500 'Internal Server Error' instead of a 4xx with a reason. The "
    "rollback leaves no record and the nonce free, so only the answer is wrong",
)
def test_a_truncated_step_count_is_refused_with_a_client_error_at_commit(ac_clone: World) -> None:
    """Defence (desired): the commit of a miner that claims fewer leaves than the manifest is
    refused at POST /commit with a 4xx and a reason, like every other refused commit (409
    'commit window closed', 422 'commit EF differs from authenticated anchor'), and nothing
    is recorded or reserved for it."""
    world = ac_clone
    cheater = world.hotkey(CHEATER)
    world.open_round(0)
    with real_server_errors(world):
        outcome = world.run_miner(CHEATER, BY_NAME["truncated-step-count"])
    keys, records = intake_trace(world, cheater)

    assert outcome.status == "refused", outcome.error
    assert outcome.endpoint == "POST /commit"
    assert outcome.http_status is not None and 400 <= outcome.http_status < 500
    assert outcome.detail and outcome.detail != "Internal Server Error"
    assert records == ["accept"]
    assert kinds_of(keys) == ["AcceptV2"]
