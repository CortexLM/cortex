from __future__ import annotations

import json
import os
import shutil
import sys
from collections.abc import Iterator
from pathlib import Path
from typing import Any

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parent))
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "trainer"))

import common  # noqa: E402,F401  (determinism before torch)
import harness  # noqa: E402

# Set HT_E2E_EVIDENCE to keep the per-scenario evidence; by default it goes to a tmp dir.
_EVIDENCE_ENV = os.environ.get("HT_E2E_EVIDENCE")


@pytest.fixture(scope="session")
def evidence_root(tmp_path_factory: pytest.TempPathFactory) -> Path:
    return Path(_EVIDENCE_ENV) if _EVIDENCE_ENV else tmp_path_factory.mktemp("e2e-evidence")


@pytest.fixture
def scenario_dir(request: pytest.FixtureRequest, tmp_path: Path, evidence_root: Path) -> Path:
    if harness.NO_REPLAY:
        return tmp_path / "evidence"
    d = evidence_root / request.node.name
    shutil.rmtree(d, ignore_errors=True)
    d.mkdir(parents=True)
    return d


@pytest.fixture
def record(scenario_dir: Path) -> Iterator[dict[str, Any]]:
    out: dict[str, Any] = {}
    yield out
    scenario_dir.mkdir(parents=True, exist_ok=True)
    (scenario_dir / "result.json").write_text(json.dumps(out, indent=1, default=str))


@pytest.fixture
def world(tmp_path: Path, scenario_dir: Path, record: dict[str, Any]) -> Iterator[Any]:
    made: list[harness.World] = []

    def make(m: Any, roster: dict[Any, dict[str, Any]], **kw: Any) -> harness.World:
        w = harness.World(tmp_path / f"w{len(made)}", m, roster, logdir=scenario_dir, **kw)
        made.append(w)
        return w

    yield make
    exits: dict[str, Any] = {}
    for w in made:
        exits.update(w.stop())
        exits.update({n: p.exit_code for n, p in w.procs.items()})
    record["process_exit_codes"] = exits
    (scenario_dir / "processes.json").write_text(
        json.dumps(
            {
                n: {
                    "pid": p.proc.pid,
                    "exit": p.exit_code,
                    "log": str(p.log_path),
                    "calls": p.calls,
                }
                for w in made
                for n, p in w.procs.items()
            },
            indent=1,
            default=str,
        )
    )
