"""Three original reviewer source checks; stdlib AST only, no fixture import."""

import ast
import hashlib
import json
import runpy
import subprocess
import sys
import tempfile
import time
import uuid
from pathlib import Path


def main() -> None:
    source = Path(__file__).with_name("protected_opening_fixture.py").read_text()
    tree = ast.parse(source)
    functions = {n.name: n for n in ast.walk(tree) if isinstance(n, ast.FunctionDef)}
    generate = functions["generate"]
    assert isinstance(generate.body[1], ast.Assign)
    assert isinstance(generate.body[1].value, ast.Call)
    assert isinstance(generate.body[1].value.func, ast.Name)
    assert generate.body[1].value.func.id == "authenticate"
    authority = ast.get_source_segment(source, functions["authenticate"])
    assert authority is not None
    for field in (
        "controller_start",
        "/proc/locks",
        "MainPID",
        "ControlGroup",
        "TimersCalendar",
        "controller-kernel.json",
        "budget.json",
        "cutoff",
    ):
        assert field in authority
    body = ast.get_source_segment(source, generate)
    assert body is not None
    assert body.index("verify_completed(dict(completed_row))") < body.index("for index, target")
    custody = ast.get_source_segment(source, functions["verify_completed"])
    assert custody is not None
    for field in (
        "completed-sha256.json",
        "verify_envelope",
        "trial_record",
        "result_record",
        "reference",
        "miner",
        "validate_artifacts",
        "final_theta_hash_w1",
    ):
        assert field in custody
    assert "recovered_finality" not in source
    supervisor = ast.get_source_segment(source, functions["supervise"])
    placement = ast.get_source_segment(source, functions["contained_popen"])
    assert supervisor is not None and placement is not None
    assert "parent_group" in supervisor and "controller_path" in supervisor
    assert "original_controller_group" not in supervisor
    assert "resource.getrusage" not in supervisor
    assert "os.cpu_count" not in source
    assert "ROOT_VERIFIED_NATIVE_EDGE_BOUND" not in source
    assert "terminal_collect" in functions
    collector = ast.get_source_segment(source, functions["terminal_collect"])
    assert "MainPID" in collector and "INVOCATION_ID" in collector
    assert "measured + reserve" in collector and "time.time() + 1" in collector
    assert "resume_allowed" in collector
    assert "whole_parent_fallback_deadline" in supervisor
    assert "argv[]=" in supervisor and "TimeoutStopUSec" in supervisor
    assert "MemoryOOMGroup" not in supervisor
    assert '"--property=OOMPolicy=kill"' in supervisor
    assert '"memory.oom.group": "1"' in body
    assert (
        "cpu.stat" in supervisor and "640_000_000" in supervisor and "1_800_000_000" in supervisor
    )
    assert "carry_budget(root, pins)" in supervisor
    assert "inclusive_window(consumed, carry_wall, measured, reserve)" in supervisor
    assert supervisor.index('["systemctl", "reset-failed"') < supervisor.index(
        "final_owned = owned_cpu()"
    )
    assert "signal.SIGTERM, cancelled" in supervisor and "signal.SIGINT, cancelled" in supervisor
    assert (
        '"--property=OOMPolicy=kill"' in supervisor
        and '"--property=KillMode=control-group"' in supervisor
    )
    assert "missing full slice+controller accounting" in supervisor
    for field in (
        "type(original_receipt.get(k)) is not int",
        "original_receipt[k] < 0",
        "prior_total_cpu_usec",
        "kernel_sha256",
        "invocation_id",
        "manifest_sha256",
        "journalctl",
        "_SYSTEMD_INVOCATION_ID",
        "lost/inconsistent independent original accounting journal",
    ):
        assert field in supervisor
    assert supervisor.index('consumed = debit["total_cpu_usec"]') < supervisor.index(
        'slice_name = pins["owned_slice"]'
    )
    assert 'consumed += load(after)["cpu_usec"]' not in supervisor
    assert "--slice=" in placement and "CPUQuota=50%" in placement
    assert "_prepare_capacity_genesis_v2" in body
    assert '"genesis_receipt": load(prepared_path)' in body
    assert '"/admin/rounds"' in body and '"opening": response.json()' in body
    assert "SERVICE_PROTECTED_OPEN_CAS" in body and "opening-negatives.json" in body
    shared_lock = next(
        n
        for n in ast.walk(functions["supervise"])
        if isinstance(n, ast.Call)
        and ast.unparse(n.func) == "fcntl.flock"
        and n.args
        and ast.unparse(n.args[0]) == "shared"
    )
    assert ast.unparse(shared_lock) == "fcntl.flock(shared, fcntl.LOCK_EX | fcntl.LOCK_NB)"
    shared_lock_source = ast.get_source_segment(source, shared_lock)
    assert shared_lock_source is not None
    assert supervisor.index(shared_lock_source) < supervisor.index("started = time.time()")
    assert '"/tmp/hypertrain-network-cpu.lock"' in supervisor and '"--no-fork"' not in supervisor
    assert "reviewed admission pins stale" in supervisor
    assert not any(isinstance(n, ast.FunctionDef) and n.name.startswith("test_") for n in tree.body)
    module = runpy.run_path(str(Path(__file__).with_name("protected_opening_fixture.py")))
    with tempfile.TemporaryDirectory() as directory:
        original = Path(directory) / "original"
        original.mkdir()
        (original / "budget.json").write_text('{"cpu_limit_usec":640000000}')
        terminal = original / "terminal.json"
        terminal.write_text('{"exit":1}')
        candidate = Path(directory) / "next"
        receipt = {
            "status": "ROOT_APPROVED_CARRY",
            "original_root": str(original),
            "next_root": str(candidate),
            "current_sources": module["sources"](),
            "original_budget_sha256": hashlib.sha256(
                (original / "budget.json").read_bytes()
            ).hexdigest(),
            "original_terminal_path": str(terminal),
            "original_terminal_sha256": hashlib.sha256(terminal.read_bytes()).hexdigest(),
            "carry_cpu_debit": 10,
            "carry_wall_debit": 20,
        }
        receipt_path = Path(directory) / "carry.json"

        def consume(value):
            receipt_path.write_text(json.dumps(value))
            return module["carry_budget"](
                candidate,
                {
                    "carry_receipt": str(receipt_path),
                    "carry_receipt_sha256": hashlib.sha256(receipt_path.read_bytes()).hexdigest(),
                },
            )

        assert consume(receipt) == (10, 20)
        for field, value in (
            ("carry_cpu_debit", True),
            ("carry_cpu_debit", -1),
            ("carry_cpu_debit", 640_000_001),
            ("carry_wall_debit", float("inf")),
            ("carry_wall_debit", 1_800_000_001),
            ("carry_cpu_debit", 640_000_000),
        ):
            try:
                consume({**receipt, field: value})
            except RuntimeError:
                pass
            else:
                raise AssertionError("invalid carry admitted")
        (original / "budget.json").unlink()
        try:
            consume(receipt)
        except FileNotFoundError:
            pass
        else:
            raise AssertionError("missing original budget admitted")
    assert module["inclusive_window"](0, 0, 100, 200) == 300
    deadline = module["cpu_fallback_deadline"]
    immediate = deadline(640_000_000, 100.0, 100.0, 1900.0)
    delayed = deadline(640_000_000, 100.0, 100.75, 1900.0)
    assert immediate == 1377.1
    assert delayed == immediate - 0.75
    # Stalled Python observer cannot extend native endpoint or consume beyond640.
    assert (immediate + 2 - 100) * 500_000 + 450_000 <= 640_000_000
    assert deadline(640_000_000, 100.0, 100.5, 110.0) == 107.5
    assert deadline(630_000_000, 100.0, 100.0, 1900.0) == 1357.1
    for remaining, sample, arm, cutoff in (
        (0, 100.0, 100.0, 1900.0),
        (1_000_000, 100.0, 100.0, 1900.0),
        (640_000_000, 100.0, 102.0, 1900.0),
        (640_000_000, 100.0, 100.0, 99.0),
    ):
        try:
            deadline(remaining, sample, arm, cutoff)
        except RuntimeError:
            pass
        else:
            raise AssertionError("exhausted/delayed native deadline admitted")
    for cpu, wall, owned, reserve in (
        (639_999_000, 0, 900, 100),
        (0, 1_800_000_000, 0, 0),
        (0, 0, True, 0),
        (0, 0, -1, 0),
    ):
        try:
            module["inclusive_window"](cpu, wall, owned, reserve)
        except RuntimeError:
            pass
        else:
            raise AssertionError("owned counter/reserve exhaustion ignored")
    print(
        "CARRY_NEGATIVES_INCLUSIVE_CLEANUP_PASS; "
        "STATIC_AUTHORITY_CUSTODY_GENESIS_CHECKS_OK; 80_RUNTIME_UNRUN"
    )


def preadmission_collector_regression() -> None:
    """Real native refusal/ExecStopPost: no model, timer wait or mocked collector."""
    fixture = Path(__file__).with_name("protected_opening_fixture.py").resolve()
    module = runpy.run_path(str(fixture))
    suffix = uuid.uuid4().hex
    slice_name = "htpreadmission" + suffix + ".slice"
    unit = "htpreadmission" + suffix + ".service"

    def call(args: list[str]) -> str:
        result = subprocess.run(args, capture_output=True, text=True, timeout=5)
        assert result.returncode == 0, (args, result.stderr)
        return result.stdout

    with tempfile.TemporaryDirectory() as directory:
        folder = Path(directory)
        original = folder / "original"
        original.mkdir()
        module["save"](original / "budget.json", {"cpu_limit_usec": 640_000_000})
        module["save"](original / "terminal.json", {"test_original_terminal": True})
        root = folder / "next"
        pins_path = folder / "pins.json"
        carry_path = folder / "carry.json"
        frozen = module["sources"]()
        source_hash = hashlib.sha256(
            json.dumps(frozen, sort_keys=True, separators=(",", ":")).encode()
        ).hexdigest()
        started = time.time()
        carry = {
            "status": "ROOT_APPROVED_CARRY",
            "allocation_id": "REGRESSION_LOCAL_ONLY_" + suffix,
            "original_root": str(original),
            "next_root": str(root),
            "current_sources": frozen,
            "original_budget_sha256": hashlib.sha256(
                (original / "budget.json").read_bytes()
            ).hexdigest(),
            "original_terminal_path": str(original / "terminal.json"),
            "original_terminal_sha256": hashlib.sha256(
                (original / "terminal.json").read_bytes()
            ).hexdigest(),
            "carry_cpu_debit": 160342917,
            "carry_wall_debit": 0,
            "allocation_budget": {
                "allocation_id": "REGRESSION_LOCAL_ONLY_" + suffix,
                "sources_sha256": source_hash,
                "owned_slice": slice_name,
                "carry_cpu_debit": 160342917,
                "carry_wall_debit": 0,
                "started": started,
                "cutoff": started + 1800,
            },
        }
        module["save"](carry_path, carry)
        modules = {
            name: frozen["src/hypertrain/" + name]
            for name in (
                "aggregator/capacity_worker.py", "aggregator/core.py",
                "aggregator/rollback_v2.py", "aggregator/tape_v2.py", "auditor/replay.py",
                "miner/island_launch.py", "trainer/compress.py", "trainer/config.py",
                "trainer/model.py",
            )
        }
        store_tree = ast.parse((fixture.parents[2] / "src/hypertrain/challenge/store.py").read_text())
        implementation = next(
            node for node in ast.walk(store_tree)
            if isinstance(node, ast.FunctionDef) and node.name == "_service_implementation_v2"
        )
        names = next(
            ast.literal_eval(node.value) for node in implementation.body
            if isinstance(node, ast.Assign)
            and any(isinstance(target, ast.Name) and target.id == "owned" for target in node.targets)
        )
        actual = {
            node.name: ast.dump(node, include_attributes=False) for node in ast.walk(store_tree)
            if isinstance(node, (ast.FunctionDef, ast.ClassDef)) and node.name in names
        }
        actual.update(modules)
        pins = {
            "subject": "LOCALTEST_EXACT728_CURRENT_STORE", "backend": "cpu-test",
            "full_sources": frozen, "owned_slice": slice_name,
            "owned_parent_baseline_usec": 0, "final_tail_reserve_usec": 2000000,
            "carry_receipt": str(carry_path),
            "carry_receipt_sha256": hashlib.sha256(carry_path.read_bytes()).hexdigest(),
            "fixture_sha256": frozen["tests/challenge/protected_opening_fixture.py"],
            "store_sha256": frozen["src/hypertrain/challenge/store.py"],
            "service_fixture_sha256": frozen["tests/challenge/test_service_network_v2.py"],
            "modules": modules,
            "implementation_hash": hashlib.sha256(
                json.dumps(actual, sort_keys=True, separators=(",", ":")).encode()
            ).hexdigest(),
            "whole_parent_fallback_service": "ht-intentionally-unarmed-" + suffix + ".service",
            "whole_parent_fallback_timer": "ht-intentionally-unarmed-" + suffix + ".timer",
            "whole_parent_fallback_deadline": started + 100,
        }
        module["save"](pins_path, pins)
        # Establish ROOT-bound budget BEFORE a service has an ExecStopPost hook.
        prepared = module["prepare_allocation_budget"](root, pins_path)
        immutable = (root / "budget.json").read_bytes()
        assert prepared["started"] == started and prepared["carry_cpu_debit"] == 160342917
        try:
            call(["systemctl", "start", slice_name])
            call(["systemctl", "set-property", "--runtime", slice_name, "CPUQuota=50%",
                  "CPUQuotaPeriodSec=100ms", "MemoryMax=1073741824", "MemorySwapMax=0"])
            group = Path("/sys/fs/cgroup") / call(
                ["systemctl", "show", slice_name, "-p", "ControlGroup", "--value"]
            ).strip().lstrip("/")
            assert int(dict(line.split() for line in (group / "cpu.stat").read_text().splitlines())["usage_usec"]) == 0
            hook = sys.executable + " " + str(fixture) + " " + str(root) + " --collect --approved-pins " + str(pins_path)
            result = subprocess.run([
                "systemd-run", "--quiet", "--wait", "--pipe", "--unit=" + unit,
                "--slice=" + slice_name, "--property=Type=exec", "--property=RuntimeMaxSec=10",
                "--property=TimeoutStopSec=1", "--property=KillMode=control-group",
                "--property=KillSignal=SIGKILL", "--property=Nice=19",
                "--property=CPUAffinity=0 1 2 3", "--property=ExecStopPost=" + hook,
                "--setenv=CAPACITY_CONTROLLER_UNIT=" + unit,
                "--setenv=OMP_NUM_THREADS=1", "--setenv=MKL_NUM_THREADS=1",
                "--setenv=OPENBLAS_NUM_THREADS=1", "--setenv=PYTHONDONTWRITEBYTECODE=1",
                sys.executable, str(fixture), str(root), "--approved-pins", str(pins_path),
                "--approve", module["APPROVAL"],
            ], capture_output=True, text=True, timeout=15)
            assert result.returncode != 0, result.stdout
            # Outer target owns sharedlock; original supervisor must refuse it
            # BEFORE creating budget/scopes, while real ExecStopPost still seals.
            assert "BlockingIOError" in result.stderr, result.stderr
            terminal = module["load"](root / "owned-final-counter.json")
            assert terminal["resume_allowed"] is False
            assert terminal["measured_owned_cpu_usec"] > 0
            assert not (root / "scopes").exists() and not (root / "fixture.json").exists()
            assert (root / "budget.json").read_bytes() == immutable
            final = int(dict(line.split() for line in (group / "cpu.stat").read_text().splitlines())["usage_usec"])
            assert final - terminal["measured_owned_cpu_usec"] <= terminal["reserved_write_exit_cpu_usec"]
            assert not any(path.read_text().strip() for path in group.rglob("cgroup.procs"))
            print("PREADMISSION_COLLECTOR_NATIVE_PASS " + json.dumps({
                "slice": slice_name, "unit": unit, "kernel_final_cpu_usec": final,
                "terminal": terminal, "budget": prepared, "model_work": False,
            }), flush=True)
        finally:
            call(["systemctl", "stop", unit, slice_name])
            call(["systemctl", "reset-failed", unit])

def setup_accounting_regression() -> None:
    """Exercise original exact setup and pre-fixture accounting expression."""
    import importlib.util

    import numpy as np

    import hypertrain.trainer  # noqa: F401
    from hypertrain.protocol.hashing import MerkleTree, sha256_hex
    from hypertrain.protocol.messages_v2 import RunManifestV2
    from hypertrain.trainer.compress import state_hash
    from hypertrain.trainer.config import TrainConfig
    from hypertrain.trainer.model import init_params

    fixture = Path(__file__).with_name("protected_opening_fixture.py")
    tree = ast.parse(fixture.read_text())
    node = next(n for n in ast.walk(tree) if isinstance(n, ast.FunctionDef) and n.name == "exact_setup")
    spec = importlib.util.spec_from_file_location(
        "capacity_setup_regression", fixture.parents[2] / "tests/ledger/test_escrow_v2.py"
    )
    assert spec is not None and spec.loader is not None
    original = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = original
    spec.loader.exec_module(original)
    namespace = {
        "np": np, "original_setup": original.setup, "Setup": original.Setup,
        "MerkleTree": MerkleTree, "sha256_hex": sha256_hex, "RunManifestV2": RunManifestV2,
        "state_hash": state_hash, "init_params": init_params, "TrainConfig": TrainConfig,
    }
    exec(compile(ast.Module(body=[node], type_ignores=[]), str(fixture), "exec"), namespace)
    setup = namespace["exact_setup"]()
    assert setup.manifest.training.dataset.assign_unit == 1
    assert setup.manifest.training.model.param_count == 728
    assert setup.manifest.training.inner.H == 2
    account = next(n for n in tree.body if isinstance(n, ast.FunctionDef) and n.name == "main")
    expression = next(
        n for n in ast.walk(account) if isinstance(n, ast.Assign)
        and any(isinstance(t, ast.Name) and t.id == "manifest_hash" for t in n.targets)
    )
    with tempfile.TemporaryDirectory() as directory:
        path = Path(directory) / "fixture.json"
        values = {"fixture_path": path, "hashlib": hashlib, "json": json,
                  "load": lambda p: json.loads(p.read_text())}
        code = compile(ast.Module(body=[expression], type_ignores=[]), str(fixture), "exec")
        exec(code, values)
        assert values["manifest_hash"] is None
        path.write_text(json.dumps({"manifest": setup.manifest.body()}))
        exec(code, values)
        assert values["manifest_hash"] == hashlib.sha256(
            json.dumps(setup.manifest.body(), sort_keys=True, separators=(",", ":")).encode()
        ).hexdigest()
    print("EXACT_SETUP_728_H2_ASSIGN_UNIT_EARLY_ACCOUNTING_PASS", flush=True)

if __name__ == "__main__":
    if sys.argv[1:] == ["--preadmission-collector"]:
        preadmission_collector_regression()
    elif sys.argv[1:] == ["--setup-accounting"]:
        setup_accounting_regression()
    else:
        main()
