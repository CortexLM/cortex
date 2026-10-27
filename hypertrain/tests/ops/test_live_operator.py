"""live_operator `open` accepts any window up to the 200-round cap: two audit leases fit in it."""

from __future__ import annotations

import json
import sqlite3
from contextlib import closing
from pathlib import Path
from typing import Any

import pytest
from scripts.ops import live_operator
from scripts.ops.live_operator import check_open_shape, finalize_expiry

from hypertrain.ops.manifest_v2 import main as manifest_main
from hypertrain.protocol import envelope_v2
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages import f32hex
from hypertrain.protocol.messages_v2 import PolicyHashes, RoundOpenV2, RunManifestV2

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


@pytest.mark.parametrize(("margin", "refused"), [(-1, True), (0, False)], ids=["short", "exact"])
def test_open_refuses_a_hotkey_whose_screen_ends_before_the_round_does(
    master_args: list[str], monkeypatch: pytest.MonkeyPatch, margin: int, refused: bool
) -> None:
    # Given: one eligible hotkey whose screen ends `margin` rounds from the d_final of the opening.
    now, offsets, hotkey = 1000, window(98), Keypair(b"\x01" * 32).ss58
    d_final = now + offsets[5]
    state = Path(master_args[5])
    relays = json.loads((state.parent / "inputs" / "relays.json").read_text())
    with closing(sqlite3.connect(state / "challenge.db")) as db, db:
        db.execute(
            "INSERT INTO records_v2 VALUES(?,?,?,?)",
            (master_args[3], "registry", "current", json.dumps({"body": relays})),
        )

    def call(self: Any, method: str, path: str, body: object = None, admin: bool = False) -> Any:
        if (method, path) == ("GET", ""):
            return {"now_round": now}
        if (method, path) == ("GET", "/admission/" + hotkey):
            record = {
                "admission_id": sha256_hex(b"admission"),
                "coldkey": "cold0",
                "state": "ACTIVE",
                "screen_until": d_final + margin,
            }
            return {"eligible": True, "record": record}
        raise Reached

    monkeypatch.setattr(live_operator.Master, "call", call)
    args = [*open_args(master_args, 98)]
    args[args.index("--hotkey") + 1] = hotkey
    # When / Then: the short screen is refused by name; the exact one gets as far as the master.
    with pytest.raises(SystemExit if refused else Reached) as raised:
        live_operator.main(args)
    if refused:
        message = f"screen of {hotkey} ends at round {d_final - 1}, before d_final {d_final}"
        assert message in str(raised.value)


def test_finalize_lists_the_members_the_aggregate_step_left_out(
    master_args: list[str], monkeypatch: pytest.MonkeyPatch
) -> None:
    """A member whose delta never arrived is excluded while the master aggregates, so the
    Finalize the operator signs must be built after that call, not before."""
    args = live_operator.argparse.Namespace(
        master=master_args[1],
        run_id=master_args[3],
        state_dir=Path(master_args[5]),
        coord_key=Path(master_args[7]),
        admin_token_file=Path(master_args[9]),
    )
    master = live_operator.Master(args)
    manifest = master.manifest()
    keys = [Keypair(bytes([i + 1]) * 32) for i in range(3)]
    roster = [
        {
            "hotkey": key.ss58,
            "slot": slot,
            "q_i": f32hex(1),
            "admission_id": sha256_hex(f"a{slot}".encode()),
            "coldkey_group": f"cold{slot}",
            "state": "ACTIVE",
            "eligible_weight": 4194304,
        }
        for slot, key in enumerate(keys)
    ]
    opening = RoundOpenV2(
        w=0,
        prev_final_hash="0" * 64,
        theta_hash="11" * 32,
        outer_state_hash="0" * 64,
        center_hash="0" * 64,
        roster_hash=sha256_hex(canonicalize(roster)),
        honeypot_commit="0" * 64,
        d_open=2,
        d_assign=3,
        d_commit=4,
        d_audit=5,
        d_upload=6,
        d_final=9,
        contract_version=2,
        policy_hashes=PolicyHashes.model_validate(
            {name: getattr(manifest.network, name) for name in PolicyHashes.model_fields}
        ),
        registry_epoch=0,
        start_state_index_hash="22" * 32,
        audit_mode="anchored-full",
        roster=roster,
    )
    envelope = envelope_v2.seal(master.coord, "RoundOpenV2", master.run_id, opening, 9)
    database = Path(master_args[5]) / "challenge.db"

    def put(kind: str, identity: str, data: object) -> None:
        with closing(sqlite3.connect(database)) as db, db:
            db.execute(
                "INSERT INTO records_v2 VALUES(?,?,?,?)",
                (master.run_id, kind, identity, json.dumps(data)),
            )

    put("round", "0", envelope)
    for key in keys:
        put("commit", f"0:{key.ss58}", {"body": {}})
    sent: list[tuple[str, dict]] = []

    late = 700  # the master is far past d_final (9) when the operator finalizes

    def call(self: Any, method: str, path: str, body: Any = None, admin: bool = False) -> Any:
        if method == "GET":
            return {"now_round": late}
        sent.append((path, body))
        if path.endswith("/aggregate"):
            put("excluded", f"0:{keys[2].ss58}", {"reason": "NO_SHOW", "stage": "delta"})
            return {"theta_hash": "33" * 32}
        return {}

    monkeypatch.setattr(live_operator.Master, "call", call)

    live_operator.finalize(master, 0)

    assert [path for path, _ in sent] == ["/admin/rounds/0/aggregate", "/admin/rounds/0/finalize"]
    assert sent[1][1]["body"]["included"] == sorted(key.ss58 for key in keys[:2])
    # The Finalize is a fresh request: it must still be live when it reaches the master.
    assert sent[1][1]["exp_drand"] == finalize_expiry(9, late) > late


@pytest.mark.parametrize(("d_final", "now", "expected"), [(500, 100, 1500), (500, 900, 1900)])
def test_finalize_envelope_outlives_a_finalize_posted_after_d_final(
    d_final: int, now: int, expected: int
) -> None:
    # The operator may finalize at d_final + N; the envelope must not expire before the post.
    assert finalize_expiry(d_final, now) == expected
    assert finalize_expiry(d_final, now) > now
