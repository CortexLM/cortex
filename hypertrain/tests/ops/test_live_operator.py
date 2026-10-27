"""live_operator `open` accepts any window up to the 200-round cap: two audit leases fit in it."""

from __future__ import annotations

import json
import sqlite3
from contextlib import closing
from dataclasses import replace
from pathlib import Path
from typing import Any

import pytest
from scripts.ops import live_operator
from scripts.ops.live_operator import check_open_shape

from hypertrain.aggregator.core import OuterState
from hypertrain.auditor.replay import AnchorCache, tensor_root
from hypertrain.ops.manifest_v2 import main as manifest_main
from hypertrain.protocol import envelope_v2
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages import f32hex
from hypertrain.protocol.messages_v2 import PolicyHashes, RoundOpenV2, RunManifestV2
from hypertrain.trainer.compress import state_hash
from hypertrain.trainer.config import TrainConfig
from hypertrain.trainer.model import init_params

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


def open_args(
    master_args: list[str], gap: int, *, hotkeys: int = 5, tolerate: int | None = None
) -> list[str]:
    offsets = ",".join(str(x) for x in window(gap))
    roster = [arg for i in range(hotkeys) for arg in ("--hotkey", f"h{i}")]
    extra = [] if tolerate is None else ["--tolerate", str(tolerate)]
    return [*master_args, "open", "--w", "0", *roster, *extra, "--offsets", offsets]


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


# A roster of R members tolerates R - 4 silent ones (the Q/4 caps need four members): the default
# asks for one, so four hotkeys must be asked for with --tolerate 0.
@pytest.mark.parametrize(
    ("hotkeys", "tolerate"),
    [(5, None), (4, 0), (6, 2)],
    ids=["five-by-default", "four-asked-for", "six-for-two"],
)
def test_cli_open_accepts_a_roster_that_tolerates_what_was_asked(
    master_args: list[str],
    master_calls: list[tuple[str, str]],
    hotkeys: int,
    tolerate: int | None,
) -> None:
    with pytest.raises(Reached):
        live_operator.main(open_args(master_args, 98, hotkeys=hotkeys, tolerate=tolerate))
    assert master_calls == [("GET", "")]


@pytest.mark.parametrize(
    ("hotkeys", "tolerate"),
    [(4, None), (5, 2), (3, 0)],
    ids=["four-by-default", "five-for-two", "three-for-none"],
)
def test_cli_open_refuses_a_roster_that_tolerates_less_than_asked(
    master_args: list[str],
    master_calls: list[tuple[str, str]],
    hotkeys: int,
    tolerate: int | None,
) -> None:
    asked = 1 if tolerate is None else tolerate
    with pytest.raises(SystemExit) as refused:
        live_operator.main(open_args(master_args, 98, hotkeys=hotkeys, tolerate=tolerate))
    message = str(refused.value)
    assert master_calls == []
    assert f"{hotkeys} hotkeys tolerate {max(hotkeys - 4, 0)}" in message
    assert f"--tolerate {asked}" in message
    assert f"open at least {4 + asked} hotkeys" in message


def test_cli_open_refuses_a_negative_tolerance(
    master_args: list[str], master_calls: list[tuple[str, str]]
) -> None:
    with pytest.raises(SystemExit, match="--tolerate must be >= 0"):
        live_operator.main(open_args(master_args, 98, hotkeys=3, tolerate=-1))
    assert master_calls == []


@pytest.mark.parametrize(
    ("banned", "sibling", "paid"),
    [(None, None, [0, 1]), (0, None, [1]), (2, 1, [0])],
    ids=["nobody-banned", "first-input-banned", "input-of-a-banned-coldkey"],
)
def test_finalize_lists_the_inputs_of_the_applied_tape_minus_a_banned_coldkey(
    master_args: list[str],
    monkeypatch: pytest.MonkeyPatch,
    banned: int | None,
    sibling: int | None,
    paid: list[int],
) -> None:
    """A member whose delta never arrived is excluded while the master aggregates, so the
    Finalize the operator signs must be built after that call, not before. It lists the inputs
    of the tape that call applied, less every admission of a coldkey convicted since (the
    sibling `sibling` runs the coldkey of `banned`, which need not be an input itself)."""
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
    with closing(sqlite3.connect(database)) as db, db:
        db.execute("CREATE TABLE admissions_v2(admission_id TEXT, state TEXT, coldkey TEXT)")
        for slot, entry in enumerate(roster):
            state = "BANNED" if slot == banned else "ACTIVE"
            coldkey = f"cold{banned if slot == sibling else slot}"
            db.execute(
                "INSERT INTO admissions_v2 VALUES(?,?,?)", (entry["admission_id"], state, coldkey)
            )
    sent: list[tuple[str, dict]] = []

    def call(self: Any, method: str, path: str, body: Any = None, admin: bool = False) -> Any:
        sent.append((path, body))
        if path.endswith("/aggregate"):
            put("excluded", f"0:{keys[2].ss58}", {"reason": "NO_SHOW", "stage": "delta"})
            put("tape-inputs", "0", {"inputs": [{"roster": roster[slot]} for slot in (0, 1)]})
            return {"theta_hash": "33" * 32}
        return {}

    monkeypatch.setattr(live_operator.Master, "call", call)

    live_operator.finalize(master, 0)

    assert [path for path, _ in sent] == ["/admin/rounds/0/aggregate", "/admin/rounds/0/finalize"]
    assert sent[1][1]["body"]["included"] == sorted(keys[slot].ss58 for slot in paid)


@pytest.mark.parametrize("closure", ["finalized", "retired", "neither"])
def test_open_chains_to_the_finalize_of_the_round_before_or_to_the_evidence_of_its_retirement(
    master_args: list[str], monkeypatch: pytest.MonkeyPatch, closure: str
) -> None:
    # Given: round 0 closed as `closure` says, and one member holding the anchor of round 0
    state, run = Path(master_args[5]), master_args[3]
    master = live_operator.Master(
        live_operator.argparse.Namespace(
            master=master_args[1],
            run_id=run,
            state_dir=state,
            coord_key=Path(master_args[7]),
            admin_token_file=Path(master_args[9]),
        )
    )
    manifest, hotkey = master.manifest(), Keypair(b"\x01" * 32).ss58
    theta = init_params(TrainConfig.from_manifest_v2(manifest).model)
    cache = AnchorCache()
    anchor = replace(cache.genesis(manifest, hotkey, theta), w=0)
    cache.entries[(run, hotkey, 0, cache.layout_hash(manifest))] = anchor
    initial = OuterState.init({k: v.detach().cpu().numpy() for k, v in theta.items()})
    finalize_body, evidence_hash = {"w": 0, "included": [hotkey]}, "ab" * 32
    records = {
        ("applied", "0"): {"out_state": master.objects.put(initial.to_bytes())},
        ("anchor", f"0:{hotkey}"): {
            "path": str(cache.persist(state / "anchors-v2" / hotkey, anchor)),
            "anchor_hash": anchor.anchor_hash,
            "proof_hash": anchor.proof_hash,
            "state_root": tensor_root(anchor.theta, anchor.state),
            "ef_hash": state_hash(anchor.ef),
            "backend": "genesis",
        },
        ("registry", "current"): {
            "body": json.loads((state.parent / "inputs" / "relays.json").read_text())
        },
    }
    if closure == "finalized":
        records[("finalize", "0")] = {"body": finalize_body}
    if closure == "retired":
        records[("retired", "0")] = {"evidence_hash": evidence_hash, "cause": "INFEASIBLE"}
    with closing(sqlite3.connect(state / "challenge.db")) as db, db:
        for (kind, identity), data in records.items():
            db.execute(
                "INSERT INTO records_v2 VALUES(?,?,?,?)", (run, kind, identity, json.dumps(data))
            )
    sent: list[Any] = []

    def call(self: Any, method: str, path: str, body: Any = None, admin: bool = False) -> Any:
        if (method, path) == ("GET", ""):
            return {"now_round": 1000}
        if (method, path) == ("GET", "/admission/" + hotkey):
            record = {
                "admission_id": sha256_hex(b"admission"),
                "coldkey": "cold0",
                "state": "ACTIVE",
                "screen_until": 10**6,
            }
            return {"eligible": True, "record": record}
        sent.append(body)
        return {}

    monkeypatch.setattr(live_operator.Master, "call", call)
    # one member is enough to see where the opening chains; the floor has its own tests above
    monkeypatch.setattr(live_operator, "check_roster_floor", lambda *_: None)
    args = [*master_args, "open", "--w", "1", "--hotkey", hotkey, "--offsets"]
    args.append(",".join(str(x) for x in window(98)))

    # When / Then: the opening of round 1 chains to the finalize of round 0, or to the evidence
    # of its retirement; a round that is neither is refused by name
    if closure == "neither":
        with pytest.raises(SystemExit, match="missing accepted finalize:0"):
            live_operator.main(args)
        assert sent == []
    else:
        live_operator.main(args)
        expected = (
            sha256_hex(canonicalize(finalize_body)) if closure == "finalized" else evidence_hash
        )
        assert [s["body"]["prev_final_hash"] for s in sent] == [expected]
