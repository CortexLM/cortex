from __future__ import annotations

import json
import sqlite3
from pathlib import Path

import pytest

from hypertrain.beacon.core import BeaconRound, BeaconUnavailable, BeaconVerificationError
from hypertrain.beacon.drand import QUICKNET_GENESIS, QUICKNET_PERIOD, quicknet_push_beacon
from hypertrain.beacon.freshness import FreshnessGate

VECTORS = json.loads((Path(__file__).parent / "vectors.json").read_text())["rounds"]


def test_exact_seed_retained_when_beacon_eclipsed_then_arrives(tmp_path: Path) -> None:
    # Given: pin/pause committed before seed arrives; process restarts.
    database = tmp_path / "freshness.sqlite"
    vector = VECTORS[0]
    r = vector["round"]
    now = QUICKNET_GENESIS + (r - 1) * QUICKNET_PERIOD
    beacon = quicknet_push_beacon()
    with sqlite3.connect(database) as db:
        gate = FreshnessGate(db, "run")
        gate.pin("draw", r)
        with pytest.raises(BeaconUnavailable):
            gate.refresh(
                "draw",
                beacon,
                now_seconds=now,
                genesis_seconds=QUICKNET_GENESIS,
                period_seconds=QUICKNET_PERIOD,
                max_lag_rounds=0,
            )
        assert gate.status("draw").paused
    with sqlite3.connect(database) as db:
        gate = FreshnessGate(db, "run")
        with pytest.raises(BeaconVerificationError):
            gate.pin("draw", r + 1)
        beacon.ingest(vector)
        # When
        draw = gate.refresh(
            "draw",
            beacon,
            now_seconds=now,
            genesis_seconds=QUICKNET_GENESIS,
            period_seconds=QUICKNET_PERIOD,
            max_lag_rounds=0,
        )
        # Then
        assert draw.round == r
        assert gate.require_fresh("draw").signature_hash == vector["randomness"]


def test_old_seed_not_replaced_when_fresh_health_round_required() -> None:
    # Given
    with sqlite3.connect(":memory:") as db:
        vector = VECTORS[0]
        r = vector["round"]
        beacon = quicknet_push_beacon()
        beacon.ingest(vector)
        gate = FreshnessGate(db, "run")
        gate.pin("draw", r)
        now = QUICKNET_GENESIS + r * QUICKNET_PERIOD
        # When / Then
        with pytest.raises(BeaconUnavailable):
            gate.refresh(
                "draw",
                beacon,
                now_seconds=now,
                genesis_seconds=QUICKNET_GENESIS,
                period_seconds=QUICKNET_PERIOD,
                max_lag_rounds=0,
            )
        assert gate.status("draw").pinned_round == r
        with pytest.raises(BeaconUnavailable):
            gate.require_fresh("draw")


class WrongRoundBeacon:
    def round_at_or_after(self, t: int) -> int:
        return 1

    def get(self, round: int) -> BeaconRound:
        v = VECTORS[0]
        return BeaconRound(v["round"] + 1, v["signature"], v["randomness"], True)


def test_invalid_signature_rejected_when_provider_claims_verified() -> None:
    # Given
    with sqlite3.connect(":memory:") as db:
        gate = FreshnessGate(db, "run")
        r = VECTORS[0]["round"] + 1
        gate.pin("draw", r)
        # When / Then
        with pytest.raises(BeaconUnavailable):
            gate.refresh(
                "draw",
                WrongRoundBeacon(),
                now_seconds=QUICKNET_GENESIS + (r - 1) * QUICKNET_PERIOD,
                genesis_seconds=QUICKNET_GENESIS,
                period_seconds=QUICKNET_PERIOD,
                max_lag_rounds=0,
            )
        assert gate.status("draw").paused
