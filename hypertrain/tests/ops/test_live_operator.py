"""live_operator `open` accepts any window up to the 200-round cap: two audit leases fit in it."""

from __future__ import annotations

import json
import sqlite3
from contextlib import closing
from pathlib import Path
from typing import Any

import pytest
from scripts.ops import live_operator
from scripts.ops.live_operator import check_open_shape

from hypertrain.ops.manifest_v2 import main as manifest_main
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages_v2 import RunManifestV2

HOTKEYS = ["a", "b"]


def window(gap: int) -> list[int]:
    """Six open offsets whose d_final - d_audit is `gap` (d_upload > d_commit holds)."""
    return [3, 6, 66, 68, 150, 68 + gap]


def test_the_200_round_cap_binds_for_a_wide_window() -> None:
    """Mutant guard (green before and after): the service horizon stays the one upper bound."""
    check_open_shape(0, HOTKEYS, window(200))
    with pytest.raises(SystemExit, match="need 2 <= d_final-d_audit <= 200"):
        check_open_shape(0, HOTKEYS, window(201))


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


@pytest.mark.parametrize("gap", [98, 100, 101, 192, 200])
def test_cli_open_with_an_auditable_window_reaches_the_master(
    master_args: list[str], master_calls: list[tuple[str, str]], gap: int
) -> None:
    with pytest.raises(Reached):
        live_operator.main(open_args(master_args, gap))
    assert master_calls == [("GET", "")]
