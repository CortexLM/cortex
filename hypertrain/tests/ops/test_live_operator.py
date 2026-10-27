"""live_operator `open` refuses an audit window whose leases the store cannot issue."""

from __future__ import annotations

import json
import sqlite3
from contextlib import closing
from pathlib import Path
from typing import Any

import pytest
from scripts.ops import live_operator
from scripts.ops.live_operator import check_open_args, finalize_expiry

from hypertrain.ops.manifest_v2 import main as manifest_main
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages_v2 import (
    MAX_STEP_SECONDS_BOUND,
    RunManifestV2,
    audit_lease_rounds,
)

HOTKEYS = ["a", "b"]


def window(gap: int) -> list[int]:
    """Six open offsets whose d_final - d_audit is `gap` (d_upload > d_commit holds)."""
    return [3, 6, 66, 68, 150, 68 + gap]


def test_s7_run_window_is_refused_for_small_h() -> None:
    # Offsets of S7 run 42f68b45 round 0: every audit lease answered 409 and nothing was audited.
    with pytest.raises(SystemExit, match=r"192.*100|100.*192"):
        check_open_args(0, HOTKEYS, [3, 6, 66, 68, 150, 260], 1)


@pytest.mark.parametrize("h", [1, 30])
@pytest.mark.parametrize("gap", [98, 100])
def test_gap_within_the_lease_is_accepted(h: int, gap: int) -> None:
    assert audit_lease_rounds(h, MAX_STEP_SECONDS_BOUND) == 100
    check_open_args(0, HOTKEYS, window(gap), h)


@pytest.mark.parametrize("h", [1, 30])
@pytest.mark.parametrize("gap", [101, 192, 200])
def test_gap_beyond_the_lease_is_refused(h: int, gap: int) -> None:
    lease = audit_lease_rounds(h, MAX_STEP_SECONDS_BOUND)
    with pytest.raises(SystemExit) as refused:
        check_open_args(0, HOTKEYS, window(gap), h)
    assert f"{gap} rounds" in str(refused.value)
    assert f"{lease} beacon rounds" in str(refused.value)
    assert f"H={h}" in str(refused.value)


def test_longer_lease_for_large_h_is_not_capped_at_100() -> None:
    h = 31
    lease = audit_lease_rounds(h, MAX_STEP_SECONDS_BOUND)
    assert 101 < lease
    check_open_args(0, HOTKEYS, window(101), h)
    check_open_args(0, HOTKEYS, window(200), h)


def test_the_200_round_cap_still_binds_when_the_lease_is_longer() -> None:
    h = 512
    assert audit_lease_rounds(h, MAX_STEP_SECONDS_BOUND) > 201
    check_open_args(0, HOTKEYS, window(200), h)
    with pytest.raises(SystemExit, match="need 2 <= d_final-d_audit <= 200"):
        check_open_args(0, HOTKEYS, window(201), h)


def test_beyond_both_bounds_the_long_standing_200_message_is_the_one_shown() -> None:
    with pytest.raises(SystemExit, match="need 2 <= d_final-d_audit <= 200"):
        check_open_args(0, HOTKEYS, window(300), 1)


class Reached(Exception):
    """Raised by the stubbed master call: the open path got as far as the network."""


@pytest.fixture
def master_args(tmp_path: Path) -> list[str]:
    """CLI global arguments for a state dir holding a real H=1 manifest and no live master."""
    coord_seed = bytes(range(32))
    inputs = tmp_path / "inputs"
    assert manifest_main(["example", "--out", str(inputs)]) == 0
    training_path, policies_path = inputs / "training.json", inputs / "policies.json"
    training = json.loads(training_path.read_text())
    training["coord_pubkey"] = Keypair(coord_seed).ss58
    training_path.write_text(json.dumps(training))
    policies = json.loads(policies_path.read_text())
    policies["economics"]["reward_authority"] = training["coord_pubkey"]
    policies_path.write_text(json.dumps(policies))
    owner_seed = tmp_path / "owner.seed"
    owner_seed.write_bytes(b"\x77" * 32)
    owner_seed.chmod(0o600)
    built = tmp_path / "built"
    assert (
        manifest_main(
            [
                "build",
                "--training",
                str(training_path),
                "--policies",
                str(policies_path),
                "--relays",
                str(inputs / "relays.json"),
                "--owner-seed",
                str(owner_seed),
                "--exp-round",
                "1000000",
                "--state-dir",
                str(tmp_path / "build-state"),
                "--out",
                str(built),
            ]
        )
        == 0
    )
    manifest = RunManifestV2.model_validate_json((built / "manifest.json").read_bytes())
    assert manifest.training.inner.H == 1
    state = tmp_path / "state"
    (state / "objects").mkdir(parents=True)
    with closing(sqlite3.connect(state / "challenge.db")) as db, db:
        db.execute("CREATE TABLE runs(run_id TEXT, manifest TEXT)")
        db.execute("CREATE TABLE records_v2(run_id TEXT, kind TEXT, id TEXT, data TEXT)")
        db.execute("INSERT INTO runs VALUES(?, ?)", (manifest.run_id(), manifest.model_dump_json()))
    coord_file = tmp_path / "coord.key"
    coord_file.write_bytes(coord_seed)
    coord_file.chmod(0o600)
    token = tmp_path / "admin.token"
    token.write_text("t" * 40)
    return [
        "--master",
        "http://127.0.0.1:9",
        "--run-id",
        manifest.run_id(),
        "--state-dir",
        str(state),
        "--coord-key",
        str(coord_file),
        "--admin-token-file",
        str(token),
    ]


@pytest.fixture
def master_calls(monkeypatch: pytest.MonkeyPatch) -> list[tuple[str, str]]:
    calls: list[tuple[str, str]] = []

    def call(self: Any, method: str, path: str, body: object = None, admin: bool = False) -> Any:
        calls.append((method, path))
        raise Reached

    monkeypatch.setattr(live_operator.Master, "call", call)
    return calls


def open_args(master_args: list[str], gap: int) -> list[str]:
    offsets = ",".join(str(x) for x in window(gap))
    return [*master_args, "open", "--w", "0", "--hotkey", "h0", "--offsets", offsets]


def test_cli_open_refuses_an_unleasable_window_before_any_master_call(
    master_args: list[str], master_calls: list[tuple[str, str]]
) -> None:
    with pytest.raises(SystemExit, match=r"192 rounds.*100 beacon rounds"):
        live_operator.main(open_args(master_args, 192))
    assert master_calls == []


@pytest.mark.parametrize("gap", [98, 100])
def test_cli_open_with_a_leasable_window_reaches_the_master(
    master_args: list[str], master_calls: list[tuple[str, str]], gap: int
) -> None:
    with pytest.raises(Reached):
        live_operator.main(open_args(master_args, gap))
    assert master_calls == [("GET", "")]


def test_cli_open_checks_the_window_against_the_manifest_h(
    master_args: list[str],
    master_calls: list[tuple[str, str]],
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """The stored manifest has H=1 (100-round lease); a manifest with H=31 admits gap 150."""
    stored = live_operator.Master.manifest

    def manifest_with_h31(self: Any) -> RunManifestV2:
        m = stored(self)
        inner = m.training.inner.model_copy(update={"H": 31})
        training = m.training.model_copy(update={"inner": inner})
        return m.model_copy(update={"training": training})

    def stop_after_the_window_check(cls: Any, manifest: RunManifestV2) -> Any:
        raise Reached

    assert audit_lease_rounds(31, MAX_STEP_SECONDS_BOUND) >= 150
    monkeypatch.setattr(live_operator.Master, "manifest", manifest_with_h31)
    monkeypatch.setattr(
        live_operator.TrainConfig, "from_manifest_v2", classmethod(stop_after_the_window_check)
    )
    with pytest.raises(Reached):
        live_operator.main(open_args(master_args, 150))
    assert master_calls == []


@pytest.mark.parametrize(("d_final", "now", "expected"), [(500, 100, 1500), (500, 900, 1900)])
def test_finalize_envelope_outlives_a_finalize_posted_after_d_final(
    d_final: int, now: int, expected: int
) -> None:
    # The operator may finalize at d_final + N; the envelope must not expire before the post.
    assert finalize_expiry(d_final, now) == expected
    assert finalize_expiry(d_final, now) > now
