"""live_operator `open` accepts any window up to the cap of the audit policy (200 rounds unless a
TEST manifest raises it): two audit leases fit in it, and a pinned re-audit window must fit too."""

from __future__ import annotations

import json
import sqlite3
from contextlib import closing
from dataclasses import replace
from pathlib import Path
from typing import Any

import pytest
from scripts.ops import live_operator
from scripts.ops.live_operator import (
    check_gap_cap,
    check_open_shape,
    check_reaudit_gap,
    finalize_expiry,
)

from hypertrain.aggregator.core import OuterState
from hypertrain.auditor.replay import AnchorCache, tensor_root
from hypertrain.challenge.audit_cooldown_v2 import reaudit_gap_needed
from hypertrain.ops.manifest_v2 import main as manifest_main
from hypertrain.protocol import envelope_v2
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages import f32hex
from hypertrain.protocol.messages_v2 import (
    AuditPolicyV2,
    PolicyHashes,
    RoundOpenV2,
    RunManifestV2,
    reaudit_rounds_for,
)
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


def build_master_args(
    tmp_path: Path,
    reaudit_rounds: int | None = None,
    gap_cap_rounds: int | None = None,
    ledger_mode: str = "test",
) -> list[str]:
    """CLI global arguments for a state dir holding a real H=1 manifest and no live master; the
    audit policy pins `reaudit_rounds` and raises `gap_cap_rounds` when they are given."""
    coord_seed = bytes(range(32))
    inputs = tmp_path / "inputs"
    assert manifest_main(["example", "--out", str(inputs), "--ledger-mode", ledger_mode]) == 0
    training_path, policies_path = inputs / "training.json", inputs / "policies.json"
    training = json.loads(training_path.read_text())
    training["coord_pubkey"] = Keypair(coord_seed).ss58
    training_path.write_text(json.dumps(training))
    policies = json.loads(policies_path.read_text())
    policies["economics"]["reward_authority"] = training["coord_pubkey"]
    if reaudit_rounds is not None:
        policies["audit"]["reaudit_rounds"] = reaudit_rounds
    if gap_cap_rounds is not None:
        policies["audit"]["gap_cap_rounds"] = gap_cap_rounds
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
                str(tmp_path / "state"),
                "--out",
                str(built),
            ]
        )
        == 0
    )
    manifest = RunManifestV2.model_validate_json((built / "manifest.json").read_bytes())
    assert manifest.training.inner.H == 1
    state = tmp_path / "state"
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
def master_args(tmp_path: Path) -> list[str]:
    return build_master_args(tmp_path)


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


# D88: the whole re-audit window of a policy that pins `reaudit_rounds` must fit before d_final. The
# 101 M replays of 144 s and 256 s (3 s a round, a lease of 100 rounds, a sign window of 20 and a
# margin of 4): 100 rounds of window for 144 s (the lease floors the 96 the formula gives) and 171
# for 256 s; the minimum gap is `1 + min(L, replay + W) + T + m` with the replay read as T / 2.
REPLAYS = [
    pytest.param(144, 100, 175, id="144s"),  # 1 + min(100, 50 + 20) + 100 + 4
    pytest.param(256, 171, 276, id="256s"),  # 1 + min(100, 86 + 20) + 171 + 4
]


@pytest.mark.parametrize(("replay_s", "window", "needed"), REPLAYS)
def test_the_minimum_gap_is_the_one_whose_window_fits_before_d_final(
    replay_s: int, window: int, needed: int
) -> None:
    assert reaudit_rounds_for(replay_s, 100) == window
    assert reaudit_gap_needed(100, window) == needed


@pytest.mark.parametrize(("replay_s", "window", "needed"), REPLAYS)
def test_open_refuses_a_gap_one_round_short_of_the_minimum_and_names_it(
    tmp_path: Path,
    master_calls: list[tuple[str, str]],
    replay_s: int,
    window: int,
    needed: int,
) -> None:
    master_args = build_master_args(tmp_path, window, gap_cap_rounds=max(200, needed))
    with pytest.raises(SystemExit) as raised:
        live_operator.main(open_args(master_args, needed - 1))
    message = str(raised.value)
    assert f"gap {needed - 1} (d_final-d_audit)" in message and f"needs {needed} rounds" in message
    assert "withheld MISMATCH" in message and "dropped instead of re-audited" in message
    assert master_calls == []


@pytest.mark.parametrize(("replay_s", "window", "needed"), REPLAYS)
def test_open_accepts_the_minimum_gap_when_the_manifest_raises_the_cap(
    tmp_path: Path,
    master_calls: list[tuple[str, str]],
    replay_s: int,
    window: int,
    needed: int,
) -> None:
    master_args = build_master_args(tmp_path, window, gap_cap_rounds=max(200, needed))
    with pytest.raises(Reached):
        live_operator.main(open_args(master_args, needed))
    assert master_calls == [("GET", "")]


def test_open_refuses_the_256_s_minimum_when_the_manifest_leaves_the_cap_at_200(
    tmp_path: Path, master_calls: list[tuple[str, str]]
) -> None:
    # the minimum gap of a 256 s replay is above the default cap: the cap is what refuses, and the
    # message says which parameter lifts it (the same gap passes in the test above)
    master_args = build_master_args(tmp_path, 171)
    with pytest.raises(SystemExit, match="need 2 <= d_final-d_audit <= 200"):
        live_operator.main(open_args(master_args, 276))
    with pytest.raises(SystemExit, match="gap_cap_rounds"):
        live_operator.main(open_args(master_args, 276))
    assert master_calls == []
    # and the guard itself says the policy's cap is below the minimum
    audit = AuditPolicyV2(
        lease_rounds=100,
        max_attempts=2,
        max_concurrent_per_auditor=1,
        max_running_jobs=2,
        max_queued_jobs=32,
        max_anchor_age_rounds=1,
        max_steps_per_attempt=2,
        reaudit_rounds=171,
    )
    with pytest.raises(SystemExit, match=r"gap_cap_rounds \(200\) is below it"):
        check_reaudit_gap(audit, 200)


def test_a_cap_above_the_default_is_for_a_test_manifest_only() -> None:
    audit = AuditPolicyV2(
        lease_rounds=100,
        max_attempts=2,
        max_concurrent_per_auditor=1,
        max_running_jobs=2,
        max_queued_jobs=32,
        max_anchor_age_rounds=1,
        max_steps_per_attempt=2,
        reaudit_rounds=171,
        gap_cap_rounds=300,
    )
    check_gap_cap(audit, "test")
    with pytest.raises(SystemExit, match="only a TEST manifest may lift the cap"):
        check_gap_cap(audit, "production")
    check_open_shape(0, HOTKEYS, window(276), cap=300)
    with pytest.raises(SystemExit, match="<= 300"):
        check_open_shape(0, HOTKEYS, window(301), cap=300)


@pytest.mark.parametrize(
    ("ledger_mode", "cap", "gap", "opens"),
    [("production", None, 200, True), ("production", 201, 201, False), ("test", 201, 201, True)],
    ids=["production-at-the-default", "production-above-it", "test-lifts-it"],
)
def test_cli_open_of_a_production_run_stops_at_the_default_cap(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    master_calls: list[tuple[str, str]],
    ledger_mode: str,
    cap: int | None,
    gap: int,
    opens: bool,
) -> None:
    """d88c3: a production run opens at the default cap exactly, and only a TEST ledger lifts it.
    The builder refuses a raised production cap since D88: a run built before that rule is built
    here with the builder's checks minus that one."""
    checks = AuditPolicyV2.validate_training
    monkeypatch.setattr(
        AuditPolicyV2, "validate_training", lambda self, training, **_: checks(self, training)
    )
    master_args = build_master_args(tmp_path, gap_cap_rounds=cap, ledger_mode=ledger_mode)
    with pytest.raises(Reached if opens else SystemExit) as raised:
        live_operator.main(open_args(master_args, gap))
    assert opens or "only a TEST manifest may lift the cap" in str(raised.value)
    assert master_calls == ([("GET", "")] if opens else [])


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

    late = 700  # the master is far past d_final (9) when the operator finalizes

    def call(self: Any, method: str, path: str, body: Any = None, admin: bool = False) -> Any:
        if method == "GET":
            return {"now_round": late}
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
    # The Finalize is a fresh request: it must still be live when it reaches the master.
    assert sent[1][1]["exp_drand"] == finalize_expiry(9, late) > late


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


@pytest.mark.parametrize(("d_final", "now", "expected"), [(500, 100, 1500), (500, 900, 1900)])
def test_finalize_envelope_outlives_a_finalize_posted_after_d_final(
    d_final: int, now: int, expected: int
) -> None:
    # The operator may finalize at d_final + N; the envelope must not expire before the post.
    assert finalize_expiry(d_final, now) == expected
    assert finalize_expiry(d_final, now) > now


def test_reseed_posts_the_admin_route_of_that_admission_and_nothing_else(
    master_args: list[str], monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    """RELAY-03: the operator step a stranded miner is told to ask for."""
    calls: list[tuple[str, str, object, bool]] = []

    def call(self: Any, method: str, path: str, body: object = None, admin: bool = False) -> Any:
        calls.append((method, path, body, admin))
        return {"seed_beacon": 1226}

    monkeypatch.setattr(live_operator.Master, "call", call)
    assert live_operator.main([*master_args, "reseed", "--admission-id", "ab" * 32]) == 0
    assert calls == [("POST", "/admin/join/" + "ab" * 32 + "/reseed", None, True)]
    assert json.loads(capsys.readouterr().out) == {"seed_beacon": 1226}
