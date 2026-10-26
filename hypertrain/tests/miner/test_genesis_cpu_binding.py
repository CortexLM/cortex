"""Actual protected caller construction, metadata only; no kernel or tensor child."""

import ast
from pathlib import Path
from types import SimpleNamespace

from hypertrain.miner.island_launch import CapacityAttempt


def test_original_genesis_caller_selects_half_cpu_without_global_override(tmp_path: Path) -> None:
    # Given: the actual source assignment, not a copied test constructor.
    root = Path(__file__).parents[2] / "src/hypertrain"
    store_tree = ast.parse((root / "challenge/store.py").read_text())
    caller = next(
        n
        for n in ast.walk(store_tree)
        if isinstance(n, ast.FunctionDef) and n.name == "_prepare_capacity_genesis_v2"
    )
    assignment = next(
        n
        for n in caller.body
        if isinstance(n, ast.Assign)
        and any(isinstance(t, ast.Name) and t.id == "capacity" for t in n.targets)
    )
    charges = []
    namespace = {
        "CapacityAttempt": CapacityAttempt,
        "identity": "11" * 32,
        "digest": "22" * 32,
        "run_id": "33" * 32,
        "estimate": {"outer_work_units": 728},
        "self": SimpleNamespace(
            state_dir=tmp_path, _service_charge_v2=lambda *a, **k: charges.append((a, k)) or True
        ),
    }
    # When: compile only that metadata construction; no preparation/manager call.
    exec(
        compile(ast.Module(body=[assignment], type_ignores=[]), "actual-genesis-caller", "exec"),
        namespace,
    )
    attempt = namespace["capacity"]
    # Then: default runner remains400%; this original protected attempt explicitly50%.
    assert attempt.cpu_quota == "50%" and attempt.charge()
    assert charges == [(("33" * 32, -1, "genesis", 1), {"kind": "outer", "units": 728})]
    assert (
        CapacityAttempt("44" * 32, "55" * 32, tmp_path / "legacy", lambda: True).cpu_quota is None
    )
    runner_tree = ast.parse((root / "miner/island_launch.py").read_text())
    default = next(
        n
        for n in runner_tree.body
        if isinstance(n, ast.Assign)
        and any(isinstance(t, ast.Name) and t.id == "_CAPACITY_CPU_PERCENT" for t in n.targets)
    )
    assert ast.literal_eval(default.value) == 400
    runner = next(
        n
        for n in runner_tree.body
        if isinstance(n, ast.FunctionDef) and n.name == "run_capacity_argv"
    )
    runner_code = ast.unparse(runner)
    assert "capacity.cpu_quota" in runner_code and "observed['cpu.max'] != cpu_max" in runner_code
    assert runner_code.count("'cpu_quota': cpu_quota") == 2
    opening = next(
        n
        for n in ast.walk(store_tree)
        if isinstance(n, ast.FunctionDef) and n.name == "open_round_v2"
    )
    assert "execution.get('cpu_quota') != attempt['cpu_quota']" in ast.unparse(opening)
