"""Self-tests of the shared support of the ledger-invariants lot.

Each later test module of the lot stands on `real_reward`, `bare_escrow`, `hosted_reward`,
`signed_variant` and `service_world`. A control that is not accepted, a stub that accepts what
the store refuses, or a run id that changes with the second would make those tests vacuous or
flaky, so each is pinned here. The hygiene scan keeps the lot's own tests honest. The mutation
runner is exercised on a tiny synthetic package, never on hypertrain/src.
"""

from __future__ import annotations

import ast
import json
import os
import re
import sqlite3
import sys
from pathlib import Path
from types import ModuleType
from typing import Any

import pytest
from fastapi import FastAPI
from hypothesis import given, settings
from hypothesis import strategies as st

from hypertrain.challenge.store import ChallengeError
from hypertrain.ledger import FULL_SHARE_MASS
from hypertrain.ledger.escrow_v2 import EscrowError

from .support import (
    ESCROW_FIXTURE,
    FROZEN_UNIX,
    LOT,
    SLUG,
    H,
    Real,
    Route,
    bare_escrow,
    error_code,
    hosted_reward,
    load_fixture_module,
    mass,
    match_settlement,
    mature,
    pays,
    resign_reward,
    routes,
    service_world,
    signed_variant,
)

ISSUED = 1_000_000


def reward_events(escrow) -> int:
    return int(
        escrow.db.execute(
            "SELECT COUNT(*) FROM escrow_events WHERE kind='REWARD_FINALIZE'"
        ).fetchone()[0]
    )


def test_fixture_module_is_loaded_once_and_owns_the_replay_states(
    li_escrow: ModuleType, real_reward: Real
) -> None:
    assert load_fixture_module(*ESCROW_FIXTURE) is li_escrow
    assert sys.modules[ESCROW_FIXTURE[0]] is li_escrow
    assert real_reward[1].tape_hash in li_escrow.VERIFIED_SHADOW_STATES
    assert H == li_escrow.H


def test_real_reward_is_issued_once_and_replays_with_the_same_receipt(
    li_escrow: ModuleType, real_reward: Real
) -> None:
    setup, evidence, reward = real_reward
    escrow = bare_escrow(li_escrow, setup)
    receipt = escrow.reward_finalize(reward, evidence)
    balances = escrow.balances()
    assert (balances.issued, balances.reward_pending) == (ISSUED, ISSUED)
    assert balances.conserved()
    assert escrow.reward_finalize(reward, evidence) == receipt
    assert escrow.balances() == balances
    assert reward_events(escrow) == 1


def test_hosted_store_finalizes_the_same_reward_once(
    li_escrow: ModuleType, real_reward: Real, tmp_path: Path
) -> None:
    with hosted_reward(tmp_path, li_escrow, real_reward, real_settlement=False) as hosted:
        receipt = hosted.finalize()
        assert hosted.finalize() == receipt
        balances = hosted.escrow.balances()
        assert (balances.issued, balances.reward_pending) == (ISSUED, ISSUED)
        assert balances.conserved()
        assert reward_events(hosted.escrow) == 1
    with pytest.raises(sqlite3.ProgrammingError):
        hosted.store._db.execute("SELECT 1")


def test_real_settlement_accepts_only_the_record_the_service_writes(
    li_escrow: ModuleType, real_reward: Real, tmp_path: Path
) -> None:
    _, _, reward = real_reward
    with hosted_reward(tmp_path / "stub", li_escrow, real_reward, real_settlement=False) as stub:
        stub.escrow.reward_finalize(stub.reward, stub.evidence)
        mature(stub.escrow, reward)
        assert stub.escrow.balances().available == ISSUED
    with hosted_reward(tmp_path / "real", li_escrow, real_reward, real_settlement=True) as real:
        real.escrow.reward_finalize(real.reward, real.evidence)
        assert error_code(lambda: mature(real.escrow, reward)) == "UNACCEPTED_SETTLEMENT"
        assert real.escrow.balances().reward_pending == ISSUED
        real.finalize()
        mature(real.escrow, reward)
        paid = real.answer(0)
        assert paid["weights"] == {li_escrow.HOT.ss58: float(ISSUED)}
        assert mass(paid) == FULL_SHARE_MASS
        assert (pays(real.escrow), real.escrow.balances().paid) == (1, ISSUED)


def test_strict_bare_escrow_refuses_what_the_store_refuses(
    li_escrow: ModuleType, real_reward: Real
) -> None:
    setup, evidence, reward = real_reward
    loose = bare_escrow(
        li_escrow, setup, settlement=lambda _: match_settlement(reward.finalize_hash)
    )
    loose.reward_finalize(reward, evidence)
    mature(loose, reward)
    assert loose.balances().available == ISSUED

    strict = bare_escrow(li_escrow, setup, strict=True)
    strict.reward_finalize(reward, evidence)
    assert error_code(lambda: mature(strict, reward)) == "UNACCEPTED_SETTLEMENT"
    assert strict.balances().reward_pending == ISSUED

    registry = {origin: match_settlement(reward.finalize_hash) for origin in reward.origin_ids}
    accepted = bare_escrow(li_escrow, setup, strict=True, settlement=registry)
    accepted.reward_finalize(reward, evidence)
    mature(accepted, reward)
    assert accepted.balances().available == ISSUED

    stranger = bare_escrow(li_escrow, setup, strict=True, owner_of={})
    with pytest.raises(ChallengeError) as refused:
        stranger.reward_finalize(reward, evidence)
    assert refused.value.status == 403
    assert stranger.balances().issued == 0


def test_a_resigned_reward_without_a_change_is_the_accepted_control(
    li_escrow: ModuleType, real_reward: Real
) -> None:
    setup, evidence, reward = real_reward
    escrow = bare_escrow(li_escrow, setup)
    escrow.reward_finalize(resign_reward(li_escrow, reward), evidence)
    assert escrow.balances().issued == ISSUED


@pytest.mark.parametrize(
    ("change", "code"),
    [
        ({"budget_units": ISSUED + 1}, "ROUND_BUDGET"),
        ({"mature_at": 19}, "FINALITY_BINDING_OR_VESTING"),
        ({"run_id": H}, "ORIGIN_AUTHORITY"),
    ],
)
def test_a_resigned_reward_with_one_changed_field_is_refused_with_its_own_code(
    li_escrow: ModuleType, real_reward: Real, change: dict[str, object], code: str
) -> None:
    setup, evidence, reward = real_reward
    escrow = bare_escrow(li_escrow, setup)
    variant = resign_reward(li_escrow, reward, **change)
    assert error_code(lambda: escrow.reward_finalize(variant, evidence)) == code
    assert escrow.balances().issued == 0


def test_a_signed_variant_is_a_valid_second_finalize_of_the_same_round(
    li_escrow: ModuleType, real_reward: Real
) -> None:
    setup, evidence, reward = real_reward
    other, other_evidence = signed_variant(li_escrow, real_reward, entitlements_root="22" * 32)
    assert other.origin_ids != reward.origin_ids

    alone = bare_escrow(li_escrow, setup)
    alone.reward_finalize(other, other_evidence)
    assert alone.balances().issued == ISSUED

    first = bare_escrow(li_escrow, setup)
    first.reward_finalize(reward, evidence)
    assert error_code(lambda: first.reward_finalize(other, other_evidence)) == "OPERATION_CONFLICT"
    assert first.balances().issued == ISSUED


def test_error_code_returns_the_code_and_refuses_any_other_outcome() -> None:
    def refuse() -> None:
        raise EscrowError("ROUND_BUDGET")

    def refuse_otherwise() -> None:
        raise ChallengeError(403, "no")

    assert error_code(refuse) == "ROUND_BUDGET"
    with pytest.raises(AssertionError, match="nothing was raised"):
        error_code(lambda: None)
    with pytest.raises(AssertionError, match="got ChallengeError"):
        error_code(refuse_otherwise)
    with pytest.raises(AssertionError, match="got ValueError"):
        error_code(lambda: int("x"))


def test_run_id_is_a_function_of_the_frozen_clock_alone(tmp_path: Path) -> None:
    with (
        service_world(tmp_path / "a") as first,
        service_world(tmp_path / "b") as second,
        service_world(tmp_path / "c", now=FROZEN_UNIX + 1) as later,
    ):
        assert first.network.manifest.training.beacon.genesis_time == FROZEN_UNIX - 1
        assert first.run_id == second.run_id
        assert later.run_id != first.run_id


def test_get_weights_needs_the_internal_bearer_and_a_refusal_changes_no_state(
    tmp_path: Path,
) -> None:
    with service_world(tmp_path) as world:
        before = world.db_digest()
        refused = {
            "no bearer": world.get_weights(0, headers={}),
            "admin bearer": world.get_weights(0, headers=world.admin),
            "other slug": world.get_weights(
                0, headers={**world.internal, "x-platform-challenge-slug": "opentype"}
            ),
        }
        assert {name: r.status_code for name, r in refused.items()} == {
            "no bearer": 401,
            "admin bearer": 401,
            "other slug": 403,
        }
        assert world.db_digest() == before
        assert world.count("weights_runs") == 0

        answer = world.get_weights(0)
        assert answer.status_code == 200
        assert (answer.json()["challenge_slug"], answer.json()["epoch"]) == (SLUG, 0)
        assert world.count("weights_runs", "epoch=?", (0,)) == 1
        stored = world.db_digest()
        assert stored != before

        again = world.get_weights(0)
        assert again.content == answer.content
        assert world.db_digest() == stored


def test_every_bearer_class_is_configured_and_only_its_own_is_accepted(tmp_path: Path) -> None:
    probes = {
        "admin": ("get", "/v1/aggregator/runs/r/rounds/0/inputs"),
        "worker": ("post", "/v1/worker/lease"),
    }
    with service_world(tmp_path) as world:
        before = world.db_digest()
        bearers = {
            "admin": world.admin,
            "worker": world.worker,
            "internal": world.internal,
            "none": {},
        }
        seen = {
            (owner, bearer): getattr(world.client, method)(path, headers=headers).status_code
            for owner, (method, path) in probes.items()
            for bearer, headers in bearers.items()
        }
        # Past the bearer: an unknown round (404) and an empty job queue (204), never 503.
        assert seen == {
            (owner, bearer): 401 for owner in probes for bearer in bearers if owner != bearer
        } | {("admin", "admin"): 404, ("worker", "worker"): 204}
        assert world.db_digest() == before


def test_service_world_closes_the_store_it_built(tmp_path: Path) -> None:
    with service_world(tmp_path) as world:
        db = world.store._db
        assert db.execute("SELECT COUNT(*) FROM runs").fetchone()[0] == 1
    with pytest.raises(sqlite3.ProgrammingError):
        db.execute("SELECT 1")


def test_count_reads_the_store_and_rejects_a_table_that_does_not_exist(tmp_path: Path) -> None:
    with service_world(tmp_path) as world:
        assert world.count("escrow_origins") == 4
        assert world.count("escrow_origins", "issued=?", (10000,)) == 4
        assert world.count("escrow_origins", "issued=?", (7,)) == 0
        with pytest.raises(LookupError):
            world.count("not_a_table")


def test_routes_recurse_into_mounts_and_prefix_their_paths() -> None:
    inner = FastAPI(docs_url=None, redoc_url=None, openapi_url=None)
    inner.get("/leaf", name="read_leaf")(lambda: None)
    inner.post("/leaf", name="write_leaf")(lambda: None)
    app = FastAPI(docs_url=None, redoc_url=None, openapi_url=None)
    app.get("/top", name="top")(lambda: None)
    app.mount("/sub", inner, name="sub")
    assert routes(app) == [
        Route("GET", "/sub/leaf", "read_leaf"),
        Route("GET", "/top", "top"),
        Route("MOUNT", "/sub", "sub"),
        Route("POST", "/sub/leaf", "write_leaf"),
    ]


def test_the_real_route_table_lists_get_weights_and_the_public_mount(tmp_path: Path) -> None:
    with service_world(tmp_path) as world:
        table = routes(world.app)
    assert Route("GET", "/internal/v1/get_weights", "get_weights") in table
    assert [r.path for r in table if r.method == "MOUNT"] == ["/public"]
    assert any(r.path.startswith("/public/") for r in table)
    assert len(table) == len(set(table))


DRAWN: list[int] = []


@given(st.integers())
def draw(value: int) -> None:
    DRAWN.append(value)


def test_the_li_profile_makes_hypothesis_tests_of_this_directory_reproducible_and_bounded() -> None:
    runs = []
    for _ in range(2):
        DRAWN.clear()
        draw()
        runs.append(list(DRAWN))
    assert len(runs[0]) == 40
    assert runs[0] == runs[1]


def test_the_li_profile_is_not_left_loaded_for_the_rest_of_the_session() -> None:
    assert settings.get_current_profile_name() != "li"


FINDING_ID = re.compile(r"[A-Z]{2,4}(-[A-Z]?[0-9A-Za-z]+)+")
FORBIDDEN_MARKS = frozenset({"skip", "skipif", "od", "gpu", "cloudflare"})
FORBIDDEN_CALLS = frozenset({"skip", "importorskip", "xfail"})
CLOCKS = frozenset({"perf_counter", "perf_counter_ns", "monotonic", "monotonic_ns", "process_time"})


def is_mark(node: ast.Attribute) -> bool:
    owner = node.value
    return (isinstance(owner, ast.Attribute) and owner.attr == "mark") or (
        isinstance(owner, ast.Name) and owner.id == "mark"
    )


def xfail_findings(call: ast.Call) -> list[str]:
    keywords = {keyword.arg: keyword.value for keyword in call.keywords}
    strict, reason = keywords.get("strict"), keywords.get("reason")
    found = []
    if not (isinstance(strict, ast.Constant) and strict.value is True):
        found.append(f"line {call.lineno}: xfail needs strict=True")
    if "raises" not in keywords:
        found.append(f"line {call.lineno}: xfail needs raises=")
    if not (
        isinstance(reason, ast.Constant)
        and isinstance(reason.value, str)
        and FINDING_ID.match(reason.value)
    ):
        found.append(f"line {call.lineno}: xfail reason must start with a finding id")
    return found


def reads_clock(expression: ast.AST, tainted: set[str]) -> bool:
    for node in ast.walk(expression):
        if isinstance(node, ast.Call):
            callee = node.func
            name = callee.attr if isinstance(callee, ast.Attribute) else getattr(callee, "id", "")
            if name in CLOCKS:
                return True
        if isinstance(node, ast.Name) and node.id in tainted:
            return True
    return False


def is_constant(node: ast.AST) -> bool:
    if isinstance(node, ast.UnaryOp):
        node = node.operand
    if isinstance(node, ast.Constant):
        return isinstance(node.value, int | float) and not isinstance(node.value, bool)
    return isinstance(node, ast.Name) and node.id.isupper()


def timed_names(tree: ast.AST) -> set[str]:
    """Names assigned from an expression that reads a clock, directly or through other names."""
    tainted: set[str] = set()
    grown = True
    while grown:
        grown = False
        for node in ast.walk(tree):
            if isinstance(node, ast.Assign):
                targets = node.targets
            elif isinstance(node, ast.AnnAssign | ast.AugAssign | ast.NamedExpr):
                targets = [node.target]
            else:
                continue
            value = node.value
            if value is None or not reads_clock(value, tainted):
                continue
            for target in targets:
                if isinstance(target, ast.Name) and target.id not in tainted:
                    tainted.add(target.id)
                    grown = True
    return tainted


def hygiene_findings(source: str) -> list[str]:
    tree = ast.parse(source)
    parent = {child: node for node in ast.walk(tree) for child in ast.iter_child_nodes(node)}
    tainted = timed_names(tree)
    found: list[str] = []
    for node in ast.walk(tree):
        if isinstance(node, ast.Attribute) and is_mark(node) and node.attr in FORBIDDEN_MARKS:
            found.append(f"line {node.lineno}: pytest.mark.{node.attr} is forbidden")
        elif isinstance(node, ast.Attribute) and is_mark(node) and node.attr == "xfail":
            call = parent.get(node)
            if isinstance(call, ast.Call) and call.func is node:
                found.extend(xfail_findings(call))
            else:
                found.append(f"line {node.lineno}: xfail without arguments")
        elif (
            isinstance(node, ast.Attribute)
            and isinstance(node.value, ast.Name)
            and node.value.id == "pytest"
            and node.attr in FORBIDDEN_CALLS
        ):
            found.append(f"line {node.lineno}: pytest.{node.attr}() is forbidden")
        elif isinstance(node, ast.ImportFrom) and node.module == "pytest":
            found.extend(
                f"line {node.lineno}: pytest.{alias.name}() is forbidden"
                for alias in node.names
                if alias.name in FORBIDDEN_CALLS
            )
        elif isinstance(node, ast.Compare):
            operands = [node.left, *node.comparators]
            if any(reads_clock(o, tainted) for o in operands) and any(
                is_constant(o) for o in operands
            ):
                found.append(f"line {node.lineno}: a duration is compared to a constant")
    return sorted(found)


def lines(*rows: str) -> str:
    return "\n".join(rows)


@pytest.mark.parametrize(
    ("source", "expected"),
    [
        (
            lines(
                "import pytest",
                "@pytest.mark.xfail(strict=True, raises=EscrowError, reason='ESC-12: a hole')",
                "def test_a(): ...",
                "@pytest.mark.xfail(strict=True, raises=ValueError, reason='AUD-7a: a hole')",
                "def test_b(): ...",
            ),
            [],
        ),
        (
            lines(
                "import time",
                "def test_a():",
                "    before = time.perf_counter()",
                "    after = time.monotonic()",
                "    assert after > before",
                "    assert len([]) < 2.0",
            ),
            [],
        ),
        (
            lines("import pytest", "@pytest.mark.xfail", "def test_a(): ..."),
            ["line 2: xfail without arguments"],
        ),
        (
            lines(
                "import pytest",
                "@pytest.mark.xfail(raises=ValueError, reason='ESC-1: a hole')",
                "def test_a(): ...",
            ),
            ["line 2: xfail needs strict=True"],
        ),
        (
            lines(
                "import pytest",
                "@pytest.mark.xfail(strict=True, reason='ESC-1: a hole')",
                "def test_a(): ...",
            ),
            ["line 2: xfail needs raises="],
        ),
        (
            lines(
                "import pytest",
                "@pytest.mark.xfail(strict=True, raises=ValueError, reason='a hole')",
                "def test_a(): ...",
            ),
            ["line 2: xfail reason must start with a finding id"],
        ),
        (
            lines(
                "import pytest",
                "@pytest.mark.xfail(strict=True, raises=ValueError, reason='CREDIT-1: a hole')",
                "def test_a(): ...",
            ),
            ["line 2: xfail reason must start with a finding id"],
        ),
        (
            lines("import pytest", "pytestmark = pytest.mark.od", "pytest.importorskip('torch')"),
            ["line 2: pytest.mark.od is forbidden", "line 3: pytest.importorskip() is forbidden"],
        ),
        (
            lines(
                "from pytest import mark, skip",
                "@mark.skipif(True, reason='no')",
                "def test_a(): ...",
            ),
            ["line 1: pytest.skip() is forbidden", "line 2: pytest.mark.skipif is forbidden"],
        ),
        (
            lines(
                "import time",
                "def test_a():",
                "    start = time.perf_counter()",
                "    assert time.perf_counter() - start < 2.0",
            ),
            ["line 4: a duration is compared to a constant"],
        ),
        (
            lines(
                "import time",
                "LIMIT = 5",
                "def test_a():",
                "    started = time.monotonic()",
                "    elapsed = time.monotonic() - started",
                "    assert elapsed < LIMIT",
            ),
            ["line 6: a duration is compared to a constant"],
        ),
    ],
)
def test_hygiene_scan_reports_exactly_the_violations_of_a_source(
    source: str, expected: list[str]
) -> None:
    assert hygiene_findings(source) == expected


def test_every_test_module_of_the_lot_passes_the_hygiene_scan() -> None:
    modules = sorted(LOT.glob("test_*.py"))
    assert Path(__file__) in modules
    assert {path.name: hygiene_findings(path.read_text()) for path in modules} == {
        path.name: [] for path in modules
    }


RUNNER = ("li_mutant_runner", "../scripts/check_ledger_invariant_mutants.py")
TINY_SOURCE = (
    "def add(a: int, b: int) -> int:\n    return a + b\n\n\ndef limit() -> int:\n    return 10\n"
)
TINY_TESTS = (
    "from tinypkg import add, limit\n\n\n"
    "def test_add():\n    assert add(2, 3) == 5\n\n\n"
    "def test_limit_is_positive():\n    assert limit() > 0\n"
)


@pytest.fixture(scope="module")
def runner() -> ModuleType:
    return load_fixture_module(*RUNNER)


def tiny_entry(**changes: object) -> dict[str, object]:
    return {
        "id": "add-subtracts",
        "invariants": ["INV-1"],
        "path": "src/tinypkg/__init__.py",
        "old": "return a + b",
        "new": "return a - b",
        "tests": ["tests/test_tiny.py::test_add"],
        "why": "the sum becomes a difference.",
    } | changes


KILLED = tiny_entry()
SURVIVOR = tiny_entry(
    id="limit-moves",
    old="return 10",
    new="return 11",
    tests=["tests/test_tiny.py::test_limit_is_positive"],
    why="the limit changes but its test only checks the sign.",
)
BROKEN = tiny_entry(
    id="import-breaks",
    old="def add(a: int, b: int) -> int:",
    new="import zz_not_a_module\n\n\ndef add(a: int, b: int) -> int:",
    why="the module no longer imports.",
)


def tiny_project(runner: ModuleType, root: Path, entries: list[dict[str, object]]) -> Any:
    """A throw-away project with the layout of hypertrain/, so the real tree is never a target."""
    (root / "src/tinypkg").mkdir(parents=True)
    (root / "src/tinypkg/__init__.py").write_text(TINY_SOURCE)
    (root / "tests/ledger_invariants").mkdir(parents=True)
    (root / "tests/test_tiny.py").write_text(TINY_TESTS)
    (root / "tests/ledger_invariants/mutants_tiny.py").write_text(f"MUTANTS = {entries!r}\n")
    return runner.Project(root, "tinypkg")


def files_of(root: Path) -> dict[str, tuple[bytes, int]]:
    """Content and modification time of every file: an edit that is restored still shows."""
    return {
        str(path.relative_to(root)): (path.read_bytes(), path.stat().st_mtime_ns)
        for path in sorted(root.rglob("*"))
        if path.is_file() and "__pycache__" not in path.parts
    }


def test_runner_reports_killed_survived_and_error_and_never_edits_the_tree(
    runner: ModuleType, tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    project = tiny_project(runner, tmp_path / "proj", [KILLED, SURVIVOR, BROKEN])
    before = files_of(project.root)
    code = runner.main(
        ["--jobs", "2", "--workdir", str(tmp_path / "work"), "--json", str(tmp_path / "r.json")],
        project,
    )
    printed = capsys.readouterr().out.splitlines()
    report = json.loads((tmp_path / "r.json").read_text())
    assert code == 1
    verdicts = sorted(printed[:-1])
    # A module that fails to import is pytest exit 2, or 4 once its node id then matches nothing.
    assert verdicts[0] in {
        "ERROR import-breaks - pytest exit 2",
        "ERROR import-breaks - pytest exit 4",
    }
    assert verdicts[1:] == [
        "KILLED add-subtracts tests/test_tiny.py::test_add",
        "SURVIVED limit-moves",
    ]
    assert printed[-1] == "1/3 mutants killed"
    assert [(m["id"], m["status"]) for m in report["mutants"]] == [
        ("add-subtracts", "KILLED"),
        ("limit-moves", "SURVIVED"),
        ("import-breaks", "ERROR"),
    ]
    assert [m["exit_code"] for m in report["mutants"][:2]] == [1, 0]
    assert report["mutants"][2]["exit_code"] in {2, 4}
    assert report["mutants"][0]["failed"] == ["tests/test_tiny.py::test_add"]
    assert report["baseline"]["exit_code"] == 0
    assert (report["killed"], report["survived"], report["error"]) == (1, 1, 1)
    assert files_of(project.root) == before
    assert list((tmp_path / "work").iterdir()) == []


@pytest.mark.parametrize(
    ("code", "output", "expected"),
    [
        (0, "1 passed", ("SURVIVED", [], "")),
        (1, "FAILED t.py::test_a - assert 1 == 2\n1 failed", ("KILLED", ["t.py::test_a"], "")),
        (1, "ERROR t.py::test_a - boom\n1 error", ("ERROR", [], "pytest exit 1 without a failed")),
        (2, "Interrupted: 1 error during collection", ("ERROR", [], "pytest exit 2")),
        (4, "ERROR: not found", ("ERROR", [], "pytest exit 4")),
        (5, "no tests ran", ("ERROR", [], "pytest exit 5")),
        (None, "", ("ERROR", [], "timeout after 9 s")),
    ],
)
def test_only_exit_one_with_a_failed_test_is_a_kill(
    runner: ModuleType, code: int | None, output: str, expected: tuple[str, list[str], str]
) -> None:
    status, failed, detail = runner.classify(code, output, 9)
    assert (status, failed) == expected[:2]
    assert detail.startswith(expected[2])


def test_a_run_past_its_timeout_is_killed_and_reports_no_exit_code(
    runner: ModuleType, tmp_path: Path
) -> None:
    code, _ = runner.run(
        [sys.executable, "-c", "import time; time.sleep(60)"],
        cwd=tmp_path,
        env=dict(os.environ),
        timeout=0.5,
    )
    assert code is None


def test_runner_selects_mutants_and_exits_zero_when_all_of_them_are_killed(
    runner: ModuleType, tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    project = tiny_project(runner, tmp_path / "proj", [KILLED, SURVIVOR])
    assert runner.main(["--only", "nope"], project) == 2
    assert capsys.readouterr().err == "unknown mutant ids: nope\n"
    work = str(tmp_path / "work")
    assert runner.main(["--only", "add-subtracts", "--workdir", work], project) == 0
    assert capsys.readouterr().out.splitlines() == [
        "KILLED add-subtracts tests/test_tiny.py::test_add",
        "1/1 mutants killed",
    ]


def test_runner_stops_when_the_unmutated_copy_already_fails(
    runner: ModuleType, tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    project = tiny_project(runner, tmp_path / "proj", [KILLED])
    (project.root / "tests/test_tiny.py").write_text(TINY_TESTS.replace("== 5", "== 6"))
    assert runner.main(["--workdir", str(tmp_path / "work")], project) == 1
    printed = capsys.readouterr().out
    assert printed.startswith("the unmutated copy fails (pytest exit 1):\n")
    assert "KILLED" not in printed and "SURVIVED" not in printed


def test_runner_lists_id_invariants_tests_and_why(
    runner: ModuleType, tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    project = tiny_project(runner, tmp_path / "proj", [KILLED])
    assert runner.main(["--list"], project) == 0
    assert capsys.readouterr().out.splitlines() == [
        "add-subtracts",
        "  invariants: INV-1",
        "  path: src/tinypkg/__init__.py",
        "  test: tests/test_tiny.py::test_add",
        "  why: the sum becomes a difference.",
        "1 mutants",
    ]


def test_a_snippet_must_occur_exactly_once(
    runner: ModuleType, tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    twice = tiny_entry(id="twice", old="return", new="yield")
    absent = tiny_entry(id="absent", old="return 99", new="return 100")
    project = tiny_project(runner, tmp_path / "proj", [KILLED, twice, absent])
    assert runner.main(["--list"], project) == 2
    assert capsys.readouterr().err.splitlines() == [
        "mutants_tiny.py: twice: snippet found 2 times in src/tinypkg/__init__.py",
        "mutants_tiny.py: absent: snippet found 0 times in src/tinypkg/__init__.py",
    ]


@pytest.mark.parametrize(
    ("changes", "problem"),
    [
        ({"id": "bad id"}, "id must be a non-empty word"),
        ({"invariants": []}, "invariants must be a non-empty list"),
        ({"path": "src/other/x.py"}, "path must be a .py file under src/tinypkg/"),
        ({"path": "src/tinypkg/../x.py"}, "path must be a .py file under src/tinypkg/"),
        ({"old": ""}, "old must be a non-empty string"),
        ({"new": "return a + b"}, "old and new are the same"),
        ({"tests": []}, "tests must be a non-empty list"),
        ({"tests": ["tests/test_tiny.py"]}, "tests must be a non-empty list"),
        ({"why": "one\ntwo"}, "why must be one non-empty line"),
    ],
)
def test_a_malformed_entry_is_refused_with_its_reason(
    runner: ModuleType, tmp_path: Path, changes: dict[str, object], problem: str
) -> None:
    mutants, problems = runner.parse_table(
        [tiny_entry(**changes)], "mutants_tiny.py", runner.Project(tmp_path, "tinypkg")
    )
    assert mutants == []
    assert len(problems) == 1 and problem in problems[0]


def test_an_entry_must_have_exactly_the_documented_keys(runner: ModuleType, tmp_path: Path) -> None:
    project = runner.Project(tmp_path, "tinypkg")
    missing = {k: v for k, v in tiny_entry().items() if k != "why"}
    extra = tiny_entry(severity="high")
    mutants, problems = runner.parse_table([missing, extra, "text"], "mutants_tiny.py", project)
    assert mutants == []
    assert problems == [
        "mutants_tiny.py: add-subtracts: keys differ (missing ['why'], unexpected [])",
        "mutants_tiny.py: add-subtracts: keys differ (missing [], unexpected ['severity'])",
        "mutants_tiny.py[2]: not a dict",
    ]


def test_check_project_reports_duplicate_ids_missing_files_and_uncompilable_mutants(
    runner: ModuleType, tmp_path: Path
) -> None:
    project = tiny_project(runner, tmp_path / "proj", [])
    entries = [
        tiny_entry(),
        tiny_entry(),
        tiny_entry(id="syntax", new="return (a +"),
        tiny_entry(id="no-test", tests=["tests/missing.py::test_x"]),
        tiny_entry(id="no-file", path="src/tinypkg/gone.py"),
    ]
    mutants, problems = runner.parse_table(entries, "mutants_tiny.py", project)
    assert problems == []
    found = runner.check_project(project, mutants)
    assert found[1].startswith("mutants_tiny.py: syntax: the mutated source does not compile: ")
    assert found[:1] + found[2:] == [
        "mutants_tiny.py: add-subtracts: duplicate id",
        "mutants_tiny.py: no-test: tests/missing.py does not exist",
        "mutants_tiny.py: no-file: src/tinypkg/gone.py does not exist",
    ]


def test_the_work_directory_may_not_be_inside_the_worktree(
    runner: ModuleType, tmp_path: Path
) -> None:
    (tmp_path / "repo/.git").mkdir(parents=True)
    project = tiny_project(runner, tmp_path / "repo/pkg", [KILLED])
    runner.check_workdir(project, tmp_path / "elsewhere")
    with pytest.raises(runner.RunnerError, match="refusing to work inside the worktree"):
        runner.check_workdir(project, tmp_path / "repo/scratch")


def test_the_probe_refuses_a_copy_that_is_not_the_code_under_test(
    runner: ModuleType, tmp_path: Path
) -> None:
    project = runner.Project()
    copy = tmp_path / "copy"
    runner.make_copy(project, copy)
    runner.probe_package(project, copy)
    assert runner.child_env(copy).get("TMPDIR") == os.environ.get("TMPDIR")
    assert (copy / "tests").resolve() == project.root / "tests"
    assert list((copy / "src").rglob("__pycache__")) == []
    empty = tmp_path / "empty"
    (empty / "src").mkdir(parents=True)
    with pytest.raises(runner.RunnerError, match="expected the private copy"):
        runner.probe_package(project, empty)


def test_every_mutant_table_of_the_lot_is_well_formed_and_matches_the_current_tree(
    runner: ModuleType,
) -> None:
    project = runner.Project()
    mutants, problems = runner.load_tables(project)
    assert problems + runner.check_project(project, mutants) == []
