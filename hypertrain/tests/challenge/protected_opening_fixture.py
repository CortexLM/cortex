"""Explicit CLI only: real 728/N1/H2 admission custody. Never collected by pytest."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import selectors
import subprocess
import sys
import time
from pathlib import Path
from typing import Literal, TypedDict

APPROVAL = "RUN-80-H2-PLUS-GENESIS-640CPU-1800WALL"
TARGETS = (12, 12, 12, 4)
REPO = Path(__file__).resolve().parents[2]


def manager(unit: str) -> dict[str, str]:
    result = subprocess.run(
        ["systemctl", "show", unit], capture_output=True, text=True, check=True, timeout=1
    )
    return dict(line.split("=", 1) for line in result.stdout.splitlines() if "=" in line)


def native_seconds(value: str) -> float:
    import re

    if value == "0":
        return 0.0
    match = re.fullmatch(r"([0-9]+(?:\.[0-9]+)?)(us|ms|s|min)", value)
    if match is None:
        raise RuntimeError("native guard must be finite with exact duration unit")
    return float(match[1]) * {"us": 0.000001, "ms": 0.001, "s": 1, "min": 60}[match[2]]


def terminal_collect(root: Path, pins: Path) -> None:
    """Native ExecStopPost: main has exited; retain own bounded future write/exit charge."""
    authority = load(pins)
    if (root / "budget-authority.json").exists():
        prepare_allocation_budget(root, pins)
    outer = manager(os.environ["CAPACITY_CONTROLLER_UNIT"])
    group = Path("/sys/fs/cgroup") / outer["ControlGroup"].lstrip("/")
    own = Path("/proc/self/cgroup").read_text().strip().split("::", 1)[1]
    if (
        outer["MainPID"] != "0"
        or own != outer["ControlGroup"]
        or outer["Slice"] != authority["owned_slice"]
        or os.environ["INVOCATION_ID"] != outer["InvocationID"]
        or outer["KillMode"] != "control-group"
        or outer["KillSignal"] != "9"
        or not 0 < native_seconds(outer["TimeoutStopUSec"]) <= 1
    ):
        raise RuntimeError("collector lacks exact native terminal invocation/guard")
    parent = group.parent
    if (
        (parent / "cpu.max").read_text().strip() != "50000 100000"
        or (parent / "memory.max").read_text().strip() != "1073741824"
        or (parent / "memory.swap.max").read_text().strip() != "0"
        or sorted(os.sched_getaffinity(0)) != [0, 1, 2, 3]
        or os.getpriority(os.PRIO_PROCESS, 0) != 19
    ):
        raise RuntimeError("collector parent quota differs")
    # One native stop hook <=1s, .5CPU quota plus one100ms-period burst.
    reserve = 550_000
    budget = load(root / "budget.json")
    measured = int(
        dict(line.split() for line in (parent / "cpu.stat").read_text().splitlines())["usage_usec"]
    )
    if (
        type(authority["final_tail_reserve_usec"]) is not int
        or authority["final_tail_reserve_usec"] < reserve
        or budget["carry_cpu_debit"] + measured + reserve > 640_000_000
        or time.time() + 1 > budget["cutoff"]
        or any(
            pid != str(os.getpid())
            for path in parent.rglob("cgroup.procs")
            for pid in path.read_text().split()
        )
    ):
        raise RuntimeError("terminal collector descendants/budget/tail not certified")
    save(
        root / "owned-final-counter.json",
        {
            "measured_owned_cpu_usec": measured,
            "reserved_write_exit_cpu_usec": reserve,
            "reading_not_final_future_exit": True,
            "parent_retained": True,
            "invocation_id": os.environ["INVOCATION_ID"],
            "slice": outer["Slice"],
            "shared_PID1_journald": "NOT_ATTRIBUTED",
            "resume_allowed": False,
        },
    )


def authenticate(root: Path, scope: Path) -> dict:
    """Live controller, manager timer and pinned slice, before any product import."""
    record = load(scope / "before.json")
    budget = load(root / "budget.json")
    pid = record["controller_pid"]
    if (
        scope.parent != root / "scopes"
        or budget["cpu_limit_usec"] != 640_000_000
        or budget["cutoff"]
        != budget["started"] + (1_800_000_000 - budget["carry_wall_debit"]) / 1_000_000
        or type(budget["carry_cpu_debit"]) is not int
        or not 0 <= budget["carry_cpu_debit"] < 640_000_000
        or type(budget["carry_wall_debit"]) is not int
        or not 0 <= budget["carry_wall_debit"] < 1_800_000_000
        or record["carry_cpu_debit"] != budget["carry_cpu_debit"]
        or record["carry_wall_debit"] != budget["carry_wall_debit"]
        or record["sources"] != sources()
        or record["cutoff"] != budget["cutoff"]
        or record["started"] != budget["started"]
        or record["deadline"] > budget["cutoff"]
        or time.time() >= record["deadline"]
        or record["remaining_cpu_usec"] <= 0
        or Path(f"/proc/{pid}/stat").read_text().split(") ", 1)[1].split()[19]
        != record["controller_start"]
        or str(Path(__file__).resolve()).encode() not in Path(f"/proc/{pid}/cmdline").read_bytes()
    ):
        raise RuntimeError("untrusted inside controller/cutoff")
    import fcntl

    with (root / "controller.lock").open("a") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            pass
        else:
            raise RuntimeError("external controller lock absent")
    shared_stat = Path("/tmp/hypertrain-network-cpu.lock").stat()
    shared_key = (
        f"{os.major(shared_stat.st_dev):02x}:{os.minor(shared_stat.st_dev):02x}:"
        f"{shared_stat.st_ino}"
    )
    if not any(
        line.split()[4] == str(pid) and line.split()[5] == shared_key
        for line in Path("/proc/locks").read_text().splitlines()
        if len(line.split()) >= 6
    ):
        raise RuntimeError("external controller does not own original shared CPU lock")
    lock_stat = (root / "controller.lock").stat()
    lock_key = (
        f"{os.major(lock_stat.st_dev):02x}:{os.minor(lock_stat.st_dev):02x}:{lock_stat.st_ino}"
    )
    if not any(
        line.split()[4] == str(pid) and line.split()[5] == lock_key
        for line in Path("/proc/locks").read_text().splitlines()
        if len(line.split()) >= 6
    ):
        raise RuntimeError("controller PID does not own original lock")
    unit, timer, deadline_service = (manager(record[k]) for k in ("unit", "timer", "timer_service"))
    group = "/" + Path("/proc/self/cgroup").read_text().strip().split("::", 1)[1].lstrip("/")
    slice_info = manager(record["slice"])
    parent = Path("/sys/fs/cgroup") / slice_info["ControlGroup"].lstrip("/")
    kernel = {
        n: (parent / n).read_text().strip() for n in ("memory.max", "memory.swap.max", "cpu.max")
    }
    if (
        unit["MainPID"] != str(os.getpid())
        or unit["ActiveState"] != "active"
        or unit["ControlGroup"] != group
        or unit["Slice"] != record["slice"]
        or timer["ActiveState"] != "active"
        or timer["Unit"] != record["timer_service"]
        or time.strftime("%Y-%m-%d %H:%M:%S", time.gmtime(int(record["deadline"])))
        not in timer["TimersCalendar"]
        or record["unit"] not in deadline_service["ExecStart"]
        or deadline_service["Slice"] != record["slice"]
        or "systemctl" not in deadline_service["ExecStart"]
        or "stop" not in deadline_service["ExecStart"]
        or kernel != {"memory.max": "1073741824", "memory.swap.max": "0", "cpu.max": "50000 100000"}
        or group != slice_info["ControlGroup"] + "/" + record["unit"]
        or sorted(os.sched_getaffinity(0)) != [0, 1, 2, 3]
        or os.getpriority(os.PRIO_PROCESS, 0) != 19
    ):
        raise RuntimeError("inside lacks exact controller manager/kernel authority")
    saved = load(scope / "controller-kernel.json")
    if saved != {
        "controller_pid": pid,
        "controller_start": record["controller_start"],
        "unit": record["unit"],
        "slice": record["slice"],
        "kernel": kernel,
        "cutoff": record["cutoff"],
    }:
        raise RuntimeError("controller kernel receipt differs")
    controller_group = Path(f"/proc/{pid}/cgroup").read_text().strip().split("::", 1)[1]
    if not controller_group.startswith(slice_info["ControlGroup"] + "/"):
        raise RuntimeError("external controller escaped shared budget slice")
    return record


def save(path: Path, value: dict) -> None:
    """Atomic fsync metadata checkpoint; no imported training code."""
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".pending")
    with temporary.open("w") as stream:
        json.dump(value, stream, sort_keys=True, separators=(",", ":"))
        stream.flush()
        os.fsync(stream.fileno())
    os.replace(temporary, path)
    fd = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def load(path: Path) -> dict:
    with path.open() as stream:
        return json.load(stream)


def carry_budget(root: Path, pins: dict) -> tuple[int, int]:
    """Require a root-reviewed original-bound debit, in integer microseconds."""
    raw = Path(pins["carry_receipt"]).read_bytes()
    if hashlib.sha256(raw).hexdigest() != pins["carry_receipt_sha256"]:
        raise RuntimeError("carry receipt differs from reviewed root authority")
    receipt = json.loads(raw)
    original = Path(receipt["original_root"])
    fresh = receipt["status"] == "ROOT_APPROVED_FRESH_ALLOCATION"
    if (
        receipt["status"] not in {"ROOT_APPROVED_CARRY", "ROOT_APPROVED_FRESH_ALLOCATION"}
        or receipt["next_root"] != str(root)
        or original == root
        or (
            fresh
            and (
                receipt["user_approval_sha256"] != pins["user_approval_sha256"]
                or not receipt["user_approval_sha256"]
                or receipt["allocation_id"] == receipt["original_allocation_id"]
            )
        )
        or receipt["current_sources"] != sources()
        or receipt["original_budget_sha256"]
        != hashlib.sha256((original / "budget.json").read_bytes()).hexdigest()
        or hashlib.sha256(Path(receipt["original_terminal_path"]).read_bytes()).hexdigest()
        != receipt["original_terminal_sha256"]
    ):
        raise RuntimeError("carry receipt original/current subject differs")
    cpu, wall = receipt["carry_cpu_debit"], receipt["carry_wall_debit"]
    if fresh and (cpu != 0 or wall != 0 or receipt["original_unknown_cpu_reserve"] != 640_000_000):
        raise RuntimeError("fresh allocation must preserve old unknown liability separately")
    if (
        type(cpu) is not int
        or not 0 <= cpu <= 640_000_000
        or type(wall) is not int
        or not 0 <= wall <= 1_800_000_000
    ):
        raise RuntimeError("carry debit exceeds strict cumulative CPU/wall ceiling")
    if cpu == 640_000_000 or wall == 1_800_000_000:
        raise RuntimeError("no certified remaining inclusive budget")
    return cpu, wall


def inclusive_window(cpu: int, wall: int, owned: int, reserve: int) -> int:
    """Owned parent counter plus separately certified external/final reserves only."""
    if any(type(value) is not int or value < 0 for value in (cpu, wall, owned, reserve)):
        raise RuntimeError("invalid strict owned budget counter/reserve")
    total = owned + reserve
    if cpu + total >= 640_000_000 or wall >= 1_800_000_000:
        raise RuntimeError("inclusive owned budget exhausted")
    return total


def prepare_allocation_budget(root: Path, approved_pins: Path) -> dict:
    """Persist ROOT-bound allocation clock before arming any native collector."""
    import math

    pins = load(approved_pins)
    frozen = sources()
    if (
        pins.get("subject") != "LOCALTEST_EXACT728_CURRENT_STORE"
        or pins.get("backend") != "cpu-test"
        or pins.get("full_sources") != frozen
        or type(pins.get("owned_parent_baseline_usec")) is not int
        or pins.get("owned_parent_baseline_usec") != 0
    ):
        raise RuntimeError("pre-admission ROOT current allocation authority differs")
    cpu, wall = carry_budget(root, pins)
    receipt = load(Path(pins["carry_receipt"]))
    allocation = receipt["allocation_budget"]
    started, cutoff = allocation["started"], allocation["cutoff"]
    source_hash = hashlib.sha256(
        json.dumps(frozen, sort_keys=True, separators=(",", ":")).encode()
    ).hexdigest()
    if (
        not receipt.get("allocation_id")
        or allocation["allocation_id"] != receipt["allocation_id"]
        or allocation["sources_sha256"] != source_hash
        or allocation["owned_slice"] != pins["owned_slice"]
        or type(allocation["carry_cpu_debit"]) is not int
        or type(allocation["carry_wall_debit"]) is not int
        or allocation["carry_cpu_debit"] != cpu
        or allocation["carry_wall_debit"] != wall
        or type(started) not in (int, float)
        or type(cutoff) not in (int, float)
        or not math.isfinite(started)
        or not math.isfinite(cutoff)
        or started > time.time()
        or cutoff != started + (1_800_000_000 - wall) / 1_000_000
    ):
        raise RuntimeError("pre-admission immutable allocation clock/source/debit differs")
    budget = {
        "started": started,
        "cutoff": cutoff,
        "cpu_limit_usec": 640_000_000,
        "carry_cpu_debit": cpu,
        "carry_wall_debit": wall,
    }
    binding = {
        "allocation_id": receipt["allocation_id"],
        "carry_receipt_sha256": pins["carry_receipt_sha256"],
        "sources_sha256": source_hash,
        "owned_slice": pins["owned_slice"],
        "budget": budget,
    }
    for path, value in (
        (root / "budget-authority.json", binding),
        (root / "budget.json", budget),
    ):
        if path.exists():
            if load(path) != value:
                raise RuntimeError("immutable pre-admission allocation budget changed")
        else:
            save(path, value)
    return budget


def cpu_fallback_deadline(
    remaining_usec: int, sample_time: float, arm_time: float, wall_end: float
) -> float:
    """Absolute trigger, anchored before sampling; never extend for delayed arming."""
    import math

    if (
        type(remaining_usec) is not int
        or remaining_usec <= 0
        or not all(math.isfinite(value) for value in (sample_time, arm_time, wall_end))
        or not 0 <= arm_time - sample_time <= 1
    ):
        raise RuntimeError("invalid/exhausted CPU sample or unbounded native arming delay")
    # 100ms quota period plus four100ms retained-runtime slots: .45 CPU-s.
    # Two seconds cover stop/ExecStopPost; actual arm delay is charged conservatively.
    endpoint = min(wall_end, sample_time + (remaining_usec - 450_000) / 500_000)
    trigger = endpoint - 2 - (arm_time - sample_time)
    if trigger <= arm_time:
        raise RuntimeError("no positive native CPU fallback window")
    return trigger


def sources() -> dict[str, str]:
    """Freeze final product plus fixture; no runtime imports or profile requalification."""
    paths = list((REPO / "src/hypertrain").rglob("*.py"))
    paths += [
        Path(__file__),
        REPO / "tests/challenge/test_service_network_v2.py",
        REPO / "tests/ledger/test_escrow_v2.py",
    ]
    return {
        str(p.relative_to(REPO)): hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted(paths)
    }


def supervise(root: Path, approved_pins: Path) -> None:
    """Separate controller owns whole-tree kill, cumulative CPU accounting and wall deadline."""
    import fcntl
    import uuid

    root.mkdir(parents=True, exist_ok=True)
    with (
        (root / "controller.lock").open("a") as owned,
        Path("/tmp/hypertrain-network-cpu.lock").open("a") as shared,
    ):
        fcntl.flock(owned, fcntl.LOCK_EX | fcntl.LOCK_NB)
        # Subscribe/queue at caller monitor; wait here without starting the budget.
        fcntl.flock(shared, fcntl.LOCK_EX | fcntl.LOCK_NB)
        frozen = sources()
        pins = load(approved_pins)
        if (
            pins.get("subject") != "LOCALTEST_EXACT728_CURRENT_STORE"
            or pins.get("backend") != "cpu-test"
        ):
            raise RuntimeError("local test current-subject authority missing")
        if pins.get("full_sources") != frozen:
            raise RuntimeError("reviewed complete fixture/controller/source pins stale")
        if (
            pins["fixture_sha256"] != frozen["tests/challenge/protected_opening_fixture.py"]
            or pins["store_sha256"] != frozen["src/hypertrain/challenge/store.py"]
            or pins["service_fixture_sha256"]
            != frozen["tests/challenge/test_service_network_v2.py"]
        ):
            raise RuntimeError("reviewed admission pins stale; no automatic refresh/signing")
        for relative, digest in pins["modules"].items():
            if frozen["src/hypertrain/" + relative] != digest:
                raise RuntimeError("reviewed product module pin stale")
        import ast

        store_tree = ast.parse((REPO / "src/hypertrain/challenge/store.py").read_text())
        implementation = next(
            n
            for n in ast.walk(store_tree)
            if isinstance(n, ast.FunctionDef) and n.name == "_service_implementation_v2"
        )
        names = next(
            ast.literal_eval(n.value)
            for n in implementation.body
            if isinstance(n, ast.Assign)
            and any(isinstance(t, ast.Name) and t.id == "owned" for t in n.targets)
        )
        actual_map = {
            n.name: ast.dump(n, include_attributes=False)
            for n in ast.walk(store_tree)
            if isinstance(n, (ast.FunctionDef, ast.ClassDef)) and n.name in names
        }
        actual_map.update(pins["modules"])
        if (
            hashlib.sha256(
                json.dumps(actual_map, sort_keys=True, separators=(",", ":")).encode()
            ).hexdigest()
            != pins["implementation_hash"]
        ):
            raise RuntimeError("reviewed capacity implementation pin stale")
        source_path = root / "sources.json"
        if source_path.exists() and load(source_path) != frozen:
            raise RuntimeError("source changed; original fixture cannot resume/requalify")
        save(source_path, frozen)
        scope_directories = list((root / "scopes").iterdir()) if (root / "scopes").exists() else []
        if any(
            not path.is_dir() or not (path / "before.json").is_file() for path in scope_directories
        ):
            raise RuntimeError("missing original scope identity; budget custody lost")
        prior = [path / "before.json" for path in scope_directories]
        if prior:
            raise RuntimeError(
                "resume requires post-terminal native reconciliation; no pre-exit reset"
            )
        controller_tag = "ht-opening-budget-" + hashlib.sha256(str(root).encode()).hexdigest()
        retained = subprocess.run(
            ["journalctl", "--no-pager", "--output=json", "-t", controller_tag],
            capture_output=True,
            text=True,
            check=True,
        )
        original_debits = [json.loads(line) for line in retained.stdout.splitlines()]
        if len(original_debits) != len(prior):
            raise RuntimeError("missing/extra original accounting history; no budget reset")
        carry_cpu, carry_wall = carry_budget(root, pins)
        for path in prior:
            prior_record = load(path)
            if type(prior_record.get("attempt")) is not int or prior_record["attempt"] < 1:
                raise RuntimeError("invalid original controller attempt")
            if (
                prior_record.get("carry_cpu_debit") != carry_cpu
                or prior_record.get("carry_wall_debit") != carry_wall
            ):
                raise RuntimeError("original carry debit changed; no budget reset")
        prior.sort(key=lambda path: load(path)["attempt"])
        consumed = carry_cpu
        started = time.time()
        original_budget = load(root / "budget.json") if prior else None
        if (root / "budget-authority.json").exists():
            original_budget = prepare_allocation_budget(root, approved_pins)
            started = original_budget["started"]
        if original_budget is not None and (
            type(original_budget.get("cpu_limit_usec")) is not int
            or original_budget["cpu_limit_usec"] != 640_000_000
        ):
            raise RuntimeError("invalid strict original CPU budget")
        for attempt_number, before in enumerate(prior, 1):
            after = before.with_name("after.json")
            if not after.exists():
                raise RuntimeError("missing full slice+controller accounting; no automatic resume")
            record, debit, kernel = (
                load(before),
                load(after),
                load(before.with_name("kernel-final.json")),
            )
            assert original_budget is not None
            if type(debit.get("identity", {}).get("attempt")) is not int:
                raise RuntimeError("invalid original debit identity attempt")
            for original_receipt in (debit, kernel):
                if any(
                    type(original_receipt["identity"].get(k)) is not int
                    or original_receipt["identity"][k] < 1
                    for k in ("attempt", "controller_pid")
                ):
                    raise RuntimeError("invalid strict original receipt PID/attempt")
            scope_id = before.parent.name
            expected_unit = "ht-opening-fixture-" + scope_id + ".service"
            identity = {
                k: record[k]
                for k in (
                    "attempt",
                    "run_root",
                    "unit",
                    "slice",
                    "controller_pid",
                    "controller_start",
                    "sources",
                    "started",
                    "cutoff",
                )
            }
            for original_receipt, fields in (
                (
                    record,
                    ("attempt", "controller_pid", "remaining_cpu_usec", "prior_total_cpu_usec"),
                ),
                (
                    debit,
                    (
                        "cpu_usec",
                        "total_cpu_usec",
                        "kernel_cpu_usec",
                        "controller_cpu_usec",
                        "logger_pid",
                    ),
                ),
                (kernel, ("cpu_usec",)),
            ):
                if any(
                    type(original_receipt.get(k)) is not int or original_receipt[k] < 0
                    for k in fields
                ):
                    raise RuntimeError("invalid strict nonbool nonnegative original CPU counter")
            kernel_raw = before.with_name("kernel-final.json").read_bytes()
            if (
                record["attempt"] != attempt_number
                or record["run_root"] != str(root)
                or record["unit"] != expected_unit
                or record["slice"] != pins["owned_slice"]
                or record["sources"] != frozen
                or record["prior_total_cpu_usec"] != consumed
                or record["remaining_cpu_usec"] != 640_000_000 - consumed
                or record["started"] != original_budget["started"]
                or record["cutoff"] != original_budget["cutoff"]
                or original_budget["cpu_limit_usec"] != 640_000_000
                or record["cutoff"] != record["started"] + (1_800_000_000 - carry_wall) / 1_000_000
                or record["deadline"] > record["cutoff"]
                or kernel["identity"] != identity
                or debit["identity"] != identity
                or kernel["manifest_sha256"]
                != hashlib.sha256(
                    json.dumps(
                        load(root / "fixture.json")["manifest"],
                        sort_keys=True,
                        separators=(",", ":"),
                    ).encode()
                ).hexdigest()
                or debit["manifest_sha256"] != kernel["manifest_sha256"]
                or debit["kernel_sha256"] != hashlib.sha256(kernel_raw).hexdigest()
                or debit["invocation_id"] != kernel["invocation_id"]
                or debit["units"] != [record["unit"], record["timer"], record["timer_service"]]
                or record["timer"] != expected_unit.removesuffix(".service") + "-deadline.timer"
                or record["timer_service"]
                != expected_unit.removesuffix(".service") + "-deadline.service"
                or debit["cleaned"] is not True
                or debit["kernel_cpu_usec"] < kernel["cpu_usec"]
                or debit["cpu_usec"] != debit["kernel_cpu_usec"] + debit["controller_cpu_usec"]
                or debit["total_cpu_usec"] != consumed + debit["cpu_usec"]
                or debit["total_cpu_usec"] > 640_000_000
            ):
                raise RuntimeError("original accounting identity/cumulative debit inconsistent")
            # Independent manager journal, not a caller-rehashed local receipt.
            for suffix, original_receipt in (("kernel", kernel), ("controller", debit)):
                result = subprocess.run(
                    [
                        "journalctl",
                        "--no-pager",
                        "--output=json",
                        "-t",
                        f"ht-opening-{scope_id}-kernel" if suffix == "kernel" else controller_tag,
                    ],
                    capture_output=True,
                    text=True,
                    check=True,
                )
                entries = [json.loads(line) for line in result.stdout.splitlines()]
                matches = [
                    entry
                    for entry in entries
                    if entry.get("MESSAGE")
                    == json.dumps(original_receipt, sort_keys=True, separators=(",", ":"))
                    and entry.get("_UID") == str(os.getuid())
                ]
                if (
                    len(matches) != 1
                    or (
                        suffix == "kernel"
                        and (
                            matches[0].get("_SYSTEMD_INVOCATION_ID") != kernel["invocation_id"]
                            or matches[0].get("_SYSTEMD_UNIT") != record["unit"]
                        )
                    )
                    or (
                        suffix == "controller"
                        and matches[0].get("_PID") != str(debit["logger_pid"])
                    )
                ):
                    raise RuntimeError("lost/inconsistent independent original accounting journal")
            consumed = debit["total_cpu_usec"]
            started = record["started"]
        remaining = 640_000_000 - consumed
        wall_end = started + (1_800_000_000 - carry_wall) / 1_000_000
        outer = manager(os.environ["CAPACITY_CONTROLLER_UNIT"])
        slice_name = pins["owned_slice"]
        parent_info = manager(slice_name)
        parent_group = Path("/sys/fs/cgroup") / parent_info["ControlGroup"].lstrip("/")
        # Timestamp BEFORE counter read; a later lookup must never renew this sample.
        sample_monotonic = time.monotonic()
        sample_wall = time.time()
        sampled_cpu = int(
            dict(line.split() for line in (parent_group / "cpu.stat").read_text().splitlines())[
                "usage_usec"
            ]
        )
        controller_path = Path("/proc/self/cgroup").read_text().strip().split("::", 1)[1]
        if (
            outer["MainPID"] != str(os.getpid())
            or outer["Slice"] != slice_name
            or outer["ControlGroup"] != controller_path
            or not controller_path.startswith(parent_info["ControlGroup"] + "/")
            or outer["KillMode"] != "control-group"
            or outer["KillSignal"] != "9"
            or parent_info["ActiveState"] != "active"
        ):
            raise RuntimeError("controller must start inside persistent owned budget slice")
        fallback = manager(pins["whole_parent_fallback_service"])
        fallback_timer = manager(pins["whole_parent_fallback_timer"])
        fallback_deadline = pins["whole_parent_fallback_deadline"]
        expected_argv = "/usr/bin/systemctl --no-block stop " + slice_name
        expected_collector = (
            str(Path(__file__).resolve())
            + " "
            + str(root)
            + " --collect --approved-pins "
            + str(approved_pins)
        )
        if (
            "argv[]=" + expected_argv + " ;" not in fallback["ExecStart"]
            or fallback_timer["ActiveState"] != "active"
            or fallback_timer["Unit"] != pins["whole_parent_fallback_service"]
            or native_seconds(fallback_timer["AccuracyUSec"]) > 0.000001
            or native_seconds(fallback_timer["RandomizedDelayUSec"]) != 0
            or time.strftime("%Y-%m-%d %H:%M:%S", time.gmtime(int(fallback_deadline)))
            not in fallback_timer["TimersCalendar"]
            or fallback["Slice"] != slice_name
            or fallback["KillMode"] != "control-group"
            or fallback["KillSignal"] != "9"
            or not 0 < native_seconds(fallback["RuntimeMaxUSec"]) <= 1
            or not 0 < native_seconds(fallback["TimeoutStopUSec"]) <= 1
            or not time.time() < fallback_deadline <= wall_end - 1
            or not 0 < native_seconds(outer["RuntimeMaxUSec"]) <= fallback_deadline - started
            or not 0 < native_seconds(outer["TimeoutStopUSec"]) <= 1
            or "argv[]=" + sys.executable + " " + expected_collector + " ;"
            not in outer["ExecStopPost"]
        ):
            raise RuntimeError("missing independently armed whole-owned-parent fallback")
        if any(
            (parent_group / key).read_text().strip() != value
            for key, value in {
                "cpu.max": "50000 100000",
                "memory.max": "1073741824",
                "memory.swap.max": "0",
            }.items()
        ):
            raise RuntimeError("owned parent kernel caps differ")
        external = 0  # Shared PID1/journald excluded from the owned-test allocation.
        tail = pins["final_tail_reserve_usec"]
        # Independent native boundary: 2s stop/hook +1s arming +.45CPU quota allowance.
        if type(tail) is not int or tail < 2_000_000:
            raise RuntimeError("missing bounded owned write/exit reserve")
        reserve = external + tail

        def parent_cpu() -> int:
            return int(
                dict(line.split() for line in (parent_group / "cpu.stat").read_text().splitlines())[
                    "usage_usec"
                ]
            )

        baseline = pins["owned_parent_baseline_usec"]
        if type(baseline) is not int or baseline != 0:
            raise RuntimeError("owned parent must be new empty allocation with zero baseline")
        if prior:
            final_prior = load(prior[-1].with_name("owned-final-counter.json"))
            if final_prior["identity"] != load(prior[-1].with_name("after.json"))["identity"]:
                raise RuntimeError("missing original final owned counter identity")
            baseline = final_prior["cpu_usec"]
            if type(baseline) is not int or not 0 <= baseline <= parent_cpu():
                raise RuntimeError("persistent owned counter decreased or disappeared")

        def owned_cpu() -> int:
            return parent_cpu() - baseline

        if (parent_group / "cpu.max.burst").read_text().strip() != "0" or sorted(
            os.sched_getaffinity(0)
        ) != [0, 1, 2, 3]:
            raise RuntimeError("CPU fallback quota burst/placement differs")
        arm_time = time.monotonic()
        latest_trigger = cpu_fallback_deadline(
            remaining - sampled_cpu + baseline,
            sample_monotonic,
            arm_time,
            sample_monotonic + wall_end - sample_wall,
        )
        native_trigger = sample_monotonic + fallback_deadline - sample_wall
        controller_start = int(outer["ExecMainStartTimestampMonotonic"]) / 1_000_000
        controller_terminal = (
            controller_start
            + native_seconds(outer["RuntimeMaxUSec"])
            + native_seconds(outer["TimeoutStopUSec"])
        )
        if native_trigger > latest_trigger or controller_terminal > native_trigger + 2:
            raise RuntimeError("native whole-parent lifetime exceeds CPU-derived absolute deadline")
        inclusive_window(consumed, carry_wall, owned_cpu(), reserve)
        # .5CPU quota bounds future use; reserve includes sample/stop/final tail.
        deadline = min(
            wall_end - 1,
            fallback_deadline - 1,
            time.time() + (remaining - owned_cpu() - reserve) / 500_000,
        )
        if remaining <= 2_000_000 or deadline <= time.time():
            raise RuntimeError("original CPU/wall budget exhausted")
        scope = root / "scopes" / uuid.uuid4().hex
        scope.mkdir(parents=True)
        unit = "ht-opening-fixture-" + scope.name + ".service"
        timer_service = unit.removesuffix(".service") + "-deadline.service"
        timer = timer_service.removesuffix(".service") + ".timer"
        budget_path = root / "budget.json"
        if not budget_path.exists():
            save(
                budget_path,
                {
                    "started": started,
                    "cutoff": wall_end,
                    "cpu_limit_usec": 640_000_000,
                    "carry_cpu_debit": carry_cpu,
                    "carry_wall_debit": carry_wall,
                },
            )
        budget = load(budget_path)
        if budget != {
            "started": started,
            "cutoff": wall_end,
            "cpu_limit_usec": 640_000_000,
            "carry_cpu_debit": carry_cpu,
            "carry_wall_debit": carry_wall,
        }:
            raise RuntimeError("immutable overall budget changed")
        controller_start = (
            Path(f"/proc/{os.getpid()}/stat").read_text().split(") ", 1)[1].split()[19]
        )
        save(
            scope / "before.json",
            {
                "unit": unit,
                "started": started,
                "deadline": deadline,
                "remaining_cpu_usec": remaining,
                "sources": frozen,
                "cutoff": budget["cutoff"],
                "controller_pid": os.getpid(),
                "controller_start": controller_start,
                "slice": slice_name,
                "timer": timer,
                "timer_service": timer_service,
                "attempt": len(prior) + 1,
                "run_root": str(root),
                "prior_total_cpu_usec": consumed,
                "carry_cpu_debit": carry_cpu,
                "carry_wall_debit": carry_wall,
                "owned_counter_baseline_usec": baseline,
                "cpu_sample_usec": sampled_cpu,
                "cpu_sample_monotonic": sample_monotonic,
                "cpu_guard_checked_monotonic": arm_time,
                "latest_native_trigger_monotonic": latest_trigger,
            },
        )
        save(
            scope / "controller-kernel.json",
            {
                "controller_pid": os.getpid(),
                "controller_start": controller_start,
                "unit": unit,
                "slice": slice_name,
                "kernel": {
                    n: (parent_group / n).read_text().strip()
                    for n in ("memory.max", "memory.swap.max", "cpu.max")
                },
                "cutoff": budget["cutoff"],
            },
        )
        subprocess.run(
            [
                "systemd-run",
                "--unit=" + timer_service,
                "--slice=" + slice_name,
                "--on-calendar="
                + time.strftime("%Y-%m-%d %H:%M:%S UTC", time.gmtime(int(deadline))),
                "--timer-property=AccuracySec=1us",
                "--timer-property=RandomizedDelaySec=0",
                "/usr/bin/systemctl",
                "stop",
                unit,
            ],
            check=True,
        )
        proc = None
        cpu = 0
        group = None
        import signal

        def cancelled(signum: int, frame: object) -> None:
            raise InterruptedError("capacity controller cancelled; owned cleanup required")

        signal.signal(signal.SIGTERM, cancelled)
        signal.signal(signal.SIGINT, cancelled)
        try:
            with (
                (scope / "stdout.log").open("xb") as stdout,
                (scope / "stderr.log").open("xb") as stderr,
            ):
                proc = subprocess.Popen(
                    [
                        "systemd-run",
                        "--quiet",
                        "--wait",
                        "--pipe",
                        "--unit=" + unit,
                        "--slice=" + slice_name,
                        "--property=Type=exec",
                        "--property=ExecStopPost="
                        + sys.executable
                        + " "
                        + str(Path(__file__).resolve())
                        + " --account "
                        + str(scope),
                        "--property=MemoryMax=1073741824",
                        "--property=MemorySwapMax=0",
                        "--property=CPUQuota=50%",
                        "--property=CPUQuotaPeriodSec=100ms",
                        "--property=CPUAffinity=0 1 2 3",
                        "--property=Nice=19",
                        "--property=OOMPolicy=kill",
                        "--property=KillMode=control-group",
                        "--property=KillSignal=SIGKILL",
                        "--property=TimeoutStopSec=1",
                        "--property=Restart=no",
                        "--property=RuntimeMaxSec=" + str(max(1, int(deadline - time.time()))),
                        "--working-directory=" + str(REPO),
                        "/usr/bin/env",
                        "OMP_NUM_THREADS=1",
                        "MKL_NUM_THREADS=1",
                        "CUBLAS_WORKSPACE_CONFIG=:4096:8",
                        "PYTHONDONTWRITEBYTECODE=1",
                        sys.executable,
                        str(Path(__file__).resolve()),
                        "--inside",
                        str(root),
                        "--scope",
                        str(scope),
                    ],
                    stdout=stdout,
                    stderr=stderr,
                )
                with selectors.DefaultSelector() as events:
                    pidfd = os.pidfd_open(proc.pid)
                    try:
                        events.register(pidfd, selectors.EVENT_READ)
                        while True:
                            # Budget timer only; trials await the synchronous original API.
                            exited = events.select(timeout=0.25)
                            if group is None:
                                result = subprocess.run(
                                    ["systemctl", "show", unit, "-p", "ControlGroup", "--value"],
                                    capture_output=True,
                                    text=True,
                                    check=True,
                                    timeout=1,
                                )
                                if result.stdout.strip():
                                    group = parent_group
                            if group is not None and (group / "cpu.stat").exists():
                                stats = dict(
                                    line.split()
                                    for line in (group / "cpu.stat").read_text().splitlines()
                                )
                                cpu = int(stats["usage_usec"]) - baseline
                            if consumed + cpu + reserve >= 640_000_000 or time.time() >= deadline:
                                subprocess.run(
                                    [
                                        "systemctl",
                                        "stop",
                                        unit,
                                    ],
                                    check=True,
                                    timeout=1,
                                )
                            if exited:
                                break
                    finally:
                        os.close(pidfd)
                code = proc.wait()
                if code:
                    raise RuntimeError(f"fixture unit exit {code}; inspect durable scope logs")
        finally:
            receipt_path = scope / "kernel-final.json"
            accounted = receipt_path.exists()
            if accounted:
                final_kernel = load(receipt_path)
                identity = {
                    k: load(scope / "before.json")[k]
                    for k in (
                        "attempt",
                        "run_root",
                        "unit",
                        "slice",
                        "controller_pid",
                        "controller_start",
                        "sources",
                        "started",
                        "cutoff",
                    )
                }
                if (
                    type(final_kernel.get("cpu_usec")) is not int
                    or final_kernel["cpu_usec"] < 0
                    or final_kernel.get("identity") != identity
                ):
                    raise RuntimeError("invalid current original kernel counter/identity")
                logged = subprocess.run(
                    [
                        "journalctl",
                        "--no-pager",
                        "--output=json",
                        "-t",
                        f"ht-opening-{scope.name}-kernel",
                    ],
                    capture_output=True,
                    text=True,
                    check=True,
                )
                entries = [json.loads(line) for line in logged.stdout.splitlines()]
                if (
                    sum(
                        entry.get("MESSAGE")
                        == json.dumps(final_kernel, sort_keys=True, separators=(",", ":"))
                        and entry.get("_SYSTEMD_INVOCATION_ID") == final_kernel["invocation_id"]
                        for entry in entries
                    )
                    != 1
                ):
                    raise RuntimeError("missing independent current kernel accounting")
                cpu = final_kernel["cpu_usec"]
            elif group is not None and (group / "cpu.stat").exists():
                assert group is not None
                cpu = int(
                    dict(line.split() for line in (group / "cpu.stat").read_text().splitlines())[
                        "usage_usec"
                    ]
                )
            if (parent_group / "cpu.stat").exists():
                cpu = max(
                    cpu,
                    int(
                        dict(
                            line.split()
                            for line in (parent_group / "cpu.stat").read_text().splitlines()
                        )["usage_usec"]
                    ),
                )
            # Failed dispatch may never load the worker/deadline service.
            for owned_unit in (unit, timer, timer_service):
                if manager(owned_unit)["LoadState"] != "not-found":
                    subprocess.run(["systemctl", "stop", owned_unit], check=True, timeout=1)
                if manager(owned_unit)["ActiveState"] not in {"inactive", "failed"}:
                    raise RuntimeError("owned capacity cleanup not terminal")
            if proc is not None:
                proc.wait(timeout=15)
            if (
                group is not None
                and group.exists()
                and any(
                    pid != str(os.getpid())
                    for p in group.rglob("cgroup.procs")
                    for pid in p.read_text().split()
                )
            ):
                raise RuntimeError("fixture descendants survived; custody not completed")
            if not accounted:
                raise RuntimeError("missing kernel accounting; cannot resume")
            measured = owned_cpu()
            total = inclusive_window(consumed, carry_wall, measured, reserve)
            if consumed + total > 640_000_000 or time.time() >= budget["cutoff"]:
                raise RuntimeError("inclusive controller/genesis budget exhausted")
            debit = {
                "cpu_usec": total,
                "total_cpu_usec": consumed + total,
                "units": [unit, timer, timer_service],
                "cleaned": True,
                "identity": identity,
                "kernel_sha256": hashlib.sha256(receipt_path.read_bytes()).hexdigest(),
                "invocation_id": final_kernel["invocation_id"],
                "kernel_cpu_usec": measured,
                "controller_cpu_usec": reserve,
                "manifest_sha256": final_kernel["manifest_sha256"],
                "accounting_kind": "OWNED_PARENT_PLUS_CERTIFIED_RESERVES",
                "measured_owned_cpu_usec": measured,
                "external_bound_usec": external,
                "final_tail_reserve_usec": tail,
            }
            with subprocess.Popen(
                ["systemd-cat", "--identifier=" + controller_tag],
                stdin=subprocess.PIPE,
                text=True,
            ) as logger:
                debit["logger_pid"] = logger.pid
                logger.communicate(
                    json.dumps(debit, sort_keys=True, separators=(",", ":")) + "\n", timeout=1
                )
                if logger.returncode != 0:
                    raise RuntimeError("original controller journal write failed")
            save(scope / "after.json", debit)
            subprocess.run(
                ["systemctl", "reset-failed", unit, timer, timer_service], check=False, timeout=1
            )
            final_owned = owned_cpu()
            if final_owned - measured > tail:
                raise RuntimeError("final owned accounting tail exceeded certified reserve")
            inclusive_window(consumed, carry_wall, final_owned, reserve)
            save(
                scope / "owned-final-counter.json",
                {
                    "cpu_usec": parent_cpu(),
                    "slice": slice_name,
                    "identity": identity,
                    "parent_retained": True,
                    "exit_tail_reserve_usec": tail,
                },
            )


def generate(root: Path, scope: Path) -> None:
    """Original signed HTTP paths; independent fullH reference and miner artifacts each trial."""
    authority = authenticate(root, scope)
    import importlib.util
    import shutil
    import threading
    from contextlib import ExitStack
    from dataclasses import dataclass
    from types import SimpleNamespace
    from typing import Any

    import numpy as np
    import pytest
    from fastapi.testclient import TestClient
    from numpy.typing import NDArray
    from pydantic import JsonValue

    import hypertrain.trainer  # noqa: F401
    from hypertrain.auditor.replay import unpack_state
    from hypertrain.challenge.app import Config, create_app
    from hypertrain.data.trial_assignment import trial_samples
    from hypertrain.gpu_ops import work_screen
    from hypertrain.ledger import Params
    from hypertrain.miner.island_launch import IslandArtifacts, launch_island, validate_artifacts
    from hypertrain.protocol import envelope_v2
    from hypertrain.protocol.envelope import body_digest
    from hypertrain.protocol.hashing import MerkleTree, sha256_hex
    from hypertrain.protocol.jcs import canonicalize
    from hypertrain.protocol.messages import Finalize
    from hypertrain.protocol.messages_v2 import (
        AdmissionPolicyV2,
        AggregationPolicyV2,
        EconomicsPolicyV2,
        EscrowLock,
        IslandJobV1,
        JoinChallenge,
        RunManifestV2,
        WorkProof,
    )
    from hypertrain.trainer.compress import state_hash
    from hypertrain.trainer.config import TrainConfig
    from hypertrain.trainer.model import init_params

    if sources() != load(root / "sources.json"):
        raise RuntimeError("source map changed before fixture start")
    cgroup = Path("/sys/fs/cgroup") / Path("/proc/self/cgroup").read_text().strip().split("::", 1)[
        1
    ].lstrip("/")
    kernel = {
        n: (cgroup / n).read_text().strip()
        for n in ("memory.max", "memory.swap.max", "cpu.max", "memory.oom.group")
    }
    if (
        kernel
        != {
            "memory.max": "1073741824",
            "memory.swap.max": "0",
            "cpu.max": "50000 100000",
            "memory.oom.group": "1",
        }
        or sorted(os.sched_getaffinity(0)) != [0, 1, 2, 3]
        or os.getpriority(os.PRIO_PROCESS, 0) != 19
    ):
        raise RuntimeError("whole fixture kernel resource mismatch")
    save(
        root / "kernel.json",
        {**kernel, "cgroup": str(cgroup), "affinity": [0, 1, 2, 3], "nice": 19},
    )
    spec = importlib.util.spec_from_file_location(
        "protected_original_service", REPO / "tests/challenge/test_service_network_v2.py"
    )
    assert spec is not None and spec.loader is not None
    service = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = service
    spec.loader.exec_module(service)
    original_setup = service.fixture.setup

    @dataclass(frozen=True)
    class Setup:
        manifest: RunManifestV2
        policy: EconomicsPolicyV2
        admission_policy: AdmissionPolicyV2
        rows: NDArray[np.uint32]
        tree: MerkleTree

    def exact_setup() -> Setup:
        setup = original_setup()
        body = setup.manifest.body()
        body["training"]["model"].update(d_ff=8, seq_len=2, param_count=728)
        body["training"]["inner"].update(
            H=2, J=1, micro_batch=1, grad_accum=1, state_policy="carry"
        )
        body["training"]["outer"]["opt"] = "nesterov"
        body["training"]["reference_spec"]["layout"].update(
            pp=1, n_gpus=1, dp_size=1, ep_size=1, zero1=False
        )
        rows: NDArray[np.uint32] = (np.arange(256 * 3).reshape(256, 3) % 16).astype("<u4")
        tree = MerkleTree([r.tobytes() for r in rows])
        body["training"]["dataset"].update(
            n_samples=256,
            depth=8,
            assign_unit=1,
            merkle_root=tree.root.hex(),
            unit_sha256_root=MerkleTree(
                [bytes.fromhex(sha256_hex(r.tobytes())) for r in rows]
            ).root.hex(),
        )
        manifest = RunManifestV2.model_validate(body)
        body["training"]["init_state_hash"] = state_hash(
            init_params(TrainConfig.from_manifest_v2(manifest).model)
        )
        return Setup(
            RunManifestV2.model_validate(body), setup.policy, setup.admission_policy, rows, tree
        )

    def aggregation(**values: JsonValue) -> AggregationPolicyV2:
        return AggregationPolicyV2.model_validate({**values, "cclip_iters": 1})

    with ExitStack() as stack:
        patch = stack.enter_context(pytest.MonkeyPatch.context())
        patch.setattr(service.fixture, "setup", exact_setup)
        patch.setattr(service, "AggregationPolicyV2", aggregation)
        fixture_path = root / "fixture.json"
        if fixture_path.exists():
            checkpoint = load(fixture_path)
            manifest = RunManifestV2.model_validate(checkpoint["manifest"])
            live = root / "live"
            config = Config(
                "hypertrain",
                live / "state",
                "https://master.test",
                None,
                live / "admin",
                None,
                live / "coord.key",
                service.OWNER.ss58,
                Params(
                    "hypertrain",
                    manifest.training.beacon.genesis_time,
                    4320,
                    1,
                    manifest.training.verify.E_vest_rounds,
                ),
            )
            from hypertrain.beacon.core import BeaconRound

            app = create_app(
                config,
                verify_beacon=lambda data: BeaconRound(
                    data["round"], data["signature"], data["randomness"], False
                ),
                transport=service.subnet(),
            )
            client = stack.enter_context(TestClient(app))
            network = service.Network(client, manifest, None, checkpoint["origin_ids"])
            network.now = network.store._now(network.store._db)
            network.push(network.now)
        else:
            live = root / "live"
            live.mkdir()
            generator = service.network.__wrapped__(live, SimpleNamespace())
            network = next(generator)
            stack.callback(generator.close)
            save(
                fixture_path,
                {"manifest": network.manifest.body(), "origin_ids": network.origin_ids},
            )
        run = network.manifest.run_id()
        escrow, admission, _ = network.store._services(run)
        escrow.verify()

        class WorkContext(TypedDict):
            trial: int
            hotkey: str
            operation: str

        context: WorkContext = {"trial": -1, "hotkey": "", "operation": ""}

        def verify_completed(row: dict) -> None:
            """Revalidate finalized facts, signatures and both publications; never repair."""
            trial, hotkey = row["epoch"], row["hotkey"]
            directory = root / "trials" / str(trial)
            completed = load(directory / "completed.json")
            raw = (directory / "completed.json").read_bytes()
            if (
                hashlib.sha256(raw).hexdigest()
                != load(directory / "completed-sha256.json")["sha256"]
            ):
                raise RuntimeError("completed journal hash differs")
            result_row = network.store._db.execute(
                "SELECT * FROM admission_trial_results WHERE epoch=?", (trial,)
            ).fetchone()
            facts = {
                k: row[k]
                for k in (
                    "epoch",
                    "admission_id",
                    "nonce",
                    "received_beacon",
                    "finalized_beacon",
                    "evidence_hash",
                )
            }
            if (
                completed["manifest"] != run
                or completed["trial"] != trial
                or completed["hotkey"] != hotkey
                or completed["trial_record"] != facts
                or completed["result_record"] != dict(result_row)
                or result_row["outcome"] != "MATCH"
                or row["finalized_beacon"] is None
            ):
                raise RuntimeError("completed trial original journal differs")
            signed_challenge = load(directory / "challenge.json")
            challenge = JoinChallenge.model_validate(signed_challenge["body"])
            proof_bundle = load(directory / "proof.json")
            final = load(directory / "finalize.json")
            envelopes = [
                (signed_challenge, "JoinChallenge", service.COORD.ss58),
                (proof_bundle["proof"], "WorkProof", hotkey),
                (proof_bundle["screen"], "WorkScreenV2", hotkey),
                (final, "Finalize", service.COORD.ss58),
            ]
            for wire, kind, signer in envelopes:
                env = envelope_v2.parse_envelope(wire)
                if (env.run_id, env.type, env.signer) != (
                    run,
                    kind,
                    signer,
                ) or not envelope_v2.verify_envelope(wire):
                    raise RuntimeError("completed trial signature authority differs")
            expected = WorkProof.model_validate_json(result_row["reference"])
            if (
                challenge.body() != json.loads(result_row["challenge"])
                or challenge.manifest_hash != run
                or challenge.nonce != row["nonce"]
                or challenge.admission_id != row["admission_id"]
                or proof_bundle["proof"] != json.loads(result_row["proof"])
                or proof_bundle["screen"] != json.loads(result_row["screen"])
                or expected.body() != proof_bundle["proof"]["body"]
                or row["evidence_hash"] != expected.digest()
                or final["body"]["w"] != trial
                or final["body"]["included"] != [hotkey]
                or final["body"]["entitlements_root"] != expected.digest()
            ):
                raise RuntimeError("completed proof/finality binding differs")
            reservation = network.store._db.execute(
                "SELECT digest,receipt FROM admission_reservations WHERE reservation=?",
                (f"trial-final|{row['admission_id']}|{trial}",),
            ).fetchone()
            original_challenge = network.store._db.execute(
                "SELECT receipt FROM admission_reservations WHERE reservation=?",
                (f"challenge|{row['admission_id']}|{row['nonce']}",),
            ).fetchone()
            if (
                reservation is None
                or reservation["digest"] != body_digest(final["body"])
                or json.loads(reservation["receipt"]) != final
                or original_challenge is None
                or json.loads(original_challenge[0]) != signed_challenge
            ):
                raise RuntimeError("original finalized signature reservation differs")
            for operation in ("reference", "miner"):
                counter = root / "work" / f"{trial}-{operation}"
                before, after = load(counter / "before.json"), load(counter / "after.json")
                job = IslandJobV1.model_validate(before["job"])
                original_directory = (
                    network.store.state_dir / "trials-v2" / row["nonce"]
                    if operation == "reference"
                    else root / "miner" / str(trial)
                )
                if (
                    before["manifest"],
                    before["trial"],
                    before["hotkey"],
                    before["operation"],
                    before["directory"],
                ) != (run, trial, hotkey, operation, str(original_directory)):
                    raise RuntimeError("original publication identity differs")
                if (
                    job.manifest != network.manifest
                    or job.w != trial
                    or before["job_hash"] != job.digest()
                    or before["inputs"]
                    != {
                        n: sha256_hex((original_directory / p).read_bytes())
                        for n, p in job.object_paths.items()
                    }
                    or after
                    != {
                        **before,
                        "publication": {
                            n: sha256_hex(
                                (original_directory / "published/rank-0" / n).read_bytes()
                            )
                            for n in (
                                "state.safetensors",
                                "ef.safetensors",
                                "delta.bin",
                                "leaves.json",
                            )
                        },
                        "summary": sha256_hex(
                            (original_directory / "published/rank-0/summary.json").read_bytes()
                        ),
                    }
                    or completed[operation] != after
                ):
                    raise RuntimeError("original input/publication custody differs")
                artifacts = validate_artifacts(job, original_directory / "published")
                if [
                    sha256_hex(p.read_bytes())
                    for p in (artifacts.state, artifacts.ef, artifacts.delta, artifacts.leaves)
                ] != [r.sha256 for r in expected.artifact_refs]:
                    raise RuntimeError("original independent publication differs from signed proof")
                theta, _ = unpack_state(artifacts.state.read_bytes())
                if final["body"]["final_theta_hash_w1"] != state_hash(theta):
                    raise RuntimeError("completed final theta differs")
            record = completed["status"]["record"]
            ordinal = network.store._db.execute(
                "SELECT COUNT(*) FROM admission_trials WHERE admission_id=? AND epoch<=? "
                "AND finalized_beacon IS NOT NULL",
                (row["admission_id"], trial),
            ).fetchone()[0]
            if (
                record["admission_id"] != row["admission_id"]
                or record["hotkey"] != hotkey
                or record["clean_count"] != ordinal
            ):
                raise RuntimeError("completed status differs from original finalization count")
            if record["state"] != ("ACTIVE" if ordinal >= 12 else "PROBATION") or record[
                "canary_blocks"
            ] != list(range(1, min((ordinal - 1) // 4 + 1, 3) + 1)):
                raise RuntimeError("completed original canary/state history differs")

        for completed_row in network.store._db.execute(
            "SELECT t.*,a.hotkey FROM admission_trials t "
            "JOIN admissions_v2 a ON a.admission_id=t.admission_id "
            "WHERE t.finalized_beacon IS NOT NULL"
        ):
            verify_completed(dict(completed_row))
        for identity_row in network.store._db.execute(
            "SELECT admission_id,clean_count FROM admissions_v2"
        ):
            actual_count = network.store._db.execute(
                "SELECT COUNT(*) FROM admission_trials WHERE admission_id=? "
                "AND finalized_beacon IS NOT NULL",
                (identity_row["admission_id"],),
            ).fetchone()[0]
            if actual_count != identity_row["clean_count"]:
                raise RuntimeError("clean_count lacks original completed trial custody")

        def counted(
            job: IslandJobV1,
            directory: Path,
            *,
            backend: Literal["cpu", "cuda"],
            cancel: threading.Event | None = None,
            trace: bool = False,
        ) -> IslandArtifacts:
            authenticate(root, scope)
            trial, hotkey, operation = context["trial"], context["hotkey"], context["operation"]
            counter = root / "work" / f"{trial}-{operation}"
            before = {
                "manifest": run,
                "trial": trial,
                "hotkey": hotkey,
                "operation": operation,
                "job_hash": job.digest(),
                "job": job.body(),
                "directory": str(directory),
                "inputs": {
                    name: sha256_hex((directory / rel).read_bytes())
                    for name, rel in job.object_paths.items()
                },
            }
            counter.mkdir(parents=True, exist_ok=True)
            if (counter / "before.json").exists():
                if (
                    load(counter / "before.json") != before
                    or not (directory / "published").exists()
                ):
                    raise RuntimeError(
                        "interrupted/changed attempt cannot be repeated automatically"
                    )
            else:
                if len(list((root / "work").glob("*/before.json"))) >= 80:
                    raise RuntimeError("80 trajectory hard counter exhausted")
                save(counter / "before.json", before)
            artifacts = launch_island(job, directory, backend=backend, cancel=cancel, trace=trace)
            publication = {
                p.name: sha256_hex(p.read_bytes())
                for p in (artifacts.state, artifacts.ef, artifacts.delta, artifacts.leaves)
            }
            after = {
                **before,
                "publication": publication,
                "summary": sha256_hex((artifacts.directory / "rank-0/summary.json").read_bytes()),
            }
            if (counter / "after.json").exists() and load(counter / "after.json") != after:
                raise RuntimeError("completed publication changed")
            if not (counter / "after.json").exists():
                save(counter / "after.json", after)
            return artifacts

        patch.setattr(work_screen, "launch_island", counted)
        for index, target in enumerate(TARGETS):
            hotkey = service.HOT[index].ss58
            row = network.store._db.execute(
                "SELECT * FROM admissions_v2 WHERE hotkey=?", (hotkey,)
            ).fetchone()
            if row is None:
                if index:
                    network.push(network.now + 101)
                joined = network.join(index)
                assert joined.status_code == 200, joined.text
                identity = joined.json()["admission_id"]
            else:
                identity = row["admission_id"]
            lock_path = root / "locks" / f"{index}.json"
            if not lock_path.exists():
                lock = EscrowLock(
                    operation_id=f"{index + 1:064x}",
                    owner=service.COLD[index].ss58,
                    units=1000,
                    origin_ids=[network.origin_ids[index]],
                    admission_id=identity,
                    dispute_id=None,
                    kind="LOCK_ADMISSION",
                )
                save(lock_path, network.signed(service.COLD[index], "EscrowLock", lock))
            response = network.client.post(network.url + "/escrow/lock", json=load(lock_path))
            assert response.status_code == 200, response.text
            while admission.store.by_id(identity).clean_count < target:
                current = network.store._db.execute(
                    "SELECT * FROM admissions_v2 WHERE admission_id=?", (identity,)
                ).fetchone()
                pending = network.store._db.execute(
                    "SELECT outcome FROM admission_trial_results WHERE epoch=?",
                    (current["trial_epoch"],),
                ).fetchone()
                if pending is None or pending["outcome"] != "OPEN":
                    emitted = (
                        int((time.time() - network.manifest.training.beacon.genesis_time) // 3) + 1
                    )
                    network.push(max(network.now + 1, emitted))
                response = network.client.get(network.url + "/join/" + identity + "/challenge")
                assert response.status_code == 200, response.text
                challenge = JoinChallenge.model_validate(response.json()["body"])
                trial = network.store._db.execute(
                    "SELECT trial_epoch FROM admissions_v2 WHERE admission_id=?", (identity,)
                ).fetchone()[0]
                context["trial"] = trial
                context["hotkey"] = hotkey
                context["operation"] = "reference"
                checkpoint_dir = root / "trials" / str(trial)
                save(checkpoint_dir / "challenge.json", response.json())
                response = network.client.post(
                    network.url + "/admin/join/" + identity + "/reference", headers=service.admin()
                )
                assert response.status_code == 200, response.text
                samples = tuple(
                    trial_samples(
                        network.manifest,
                        identity,
                        challenge.nonce,
                        network.store._beacon_v2(challenge.seed_beacon),
                    )
                )
                job, reference_directory = network.store.stage_trial_v2(
                    run, challenge, samples, trial
                )
                miner_directory = root / "miner" / str(trial)
                miner_directory.mkdir(parents=True, exist_ok=True)
                for relative in job.object_paths.values():
                    source, destination = reference_directory / relative, miner_directory / relative
                    if destination.exists():
                        assert destination.read_bytes() == source.read_bytes()
                    else:
                        destination.parent.mkdir(parents=True, exist_ok=True)
                        shutil.copyfile(source, destination)
                context["operation"] = "miner"
                screen, proof = work_screen.screen_work(
                    job,
                    miner_directory,
                    challenge,
                    now_beacon=network.now,
                    backend="cpu",
                    launch=counted,
                )
                reference = validate_artifacts(job, reference_directory / "published")
                actual = validate_artifacts(job, miner_directory / "published")
                assert reference.directory != actual.directory
                assert [
                    p.read_bytes()
                    for p in (reference.state, reference.ef, reference.delta, reference.leaves)
                ] == [
                    p.read_bytes() for p in (actual.state, actual.ef, actual.delta, actual.leaves)
                ]
                signed_path = checkpoint_dir / "proof.json"
                if not signed_path.exists():
                    save(
                        signed_path,
                        {
                            "proof": network.signed(service.HOT[index], "WorkProof", proof),
                            "screen": network.signed(service.HOT[index], "WorkScreenV2", screen),
                        },
                    )
                response = network.client.post(network.url + "/join/proof", json=load(signed_path))
                assert response.status_code == 200, response.text
                theta, _ = unpack_state(actual.state.read_bytes())
                final = Finalize(
                    w=trial,
                    final_theta_hash_w1=state_hash(theta),
                    included=[hotkey],
                    entitlements_root=proof.digest(),
                )
                final_path = checkpoint_dir / "finalize.json"
                if not final_path.exists():
                    save(final_path, network.signed(service.COORD, "Finalize", final))
                response = network.client.post(
                    network.url + "/admin/join/" + identity + "/finalize",
                    json=load(final_path),
                    headers=service.admin(),
                )
                assert response.status_code == 200, response.text
                assert (
                    network.store._db.execute(
                        "SELECT outcome FROM admission_trial_results WHERE epoch=?", (trial,)
                    ).fetchone()[0]
                    == "MATCH"
                )
                save(
                    checkpoint_dir / "completed.json",
                    {
                        "manifest": run,
                        "trial": trial,
                        "hotkey": hotkey,
                        "status": response.json(),
                        "reference": load(root / "work" / f"{trial}-reference/after.json"),
                        "miner": load(root / "work" / f"{trial}-miner/after.json"),
                        "trial_record": dict(
                            network.store._db.execute(
                                "SELECT * FROM admission_trials WHERE epoch=?", (trial,)
                            ).fetchone()
                        ),
                        "result_record": dict(
                            network.store._db.execute(
                                "SELECT * FROM admission_trial_results WHERE epoch=?", (trial,)
                            ).fetchone()
                        ),
                    },
                )
                save(
                    checkpoint_dir / "completed-sha256.json",
                    {"sha256": sha256_hex((checkpoint_dir / "completed.json").read_bytes())},
                )
            status = admission.status(hotkey, now=network.now)
            assert status.eligible and status.record.clean_count == target
            assert status.record.state == ("ACTIVE" if index < 3 else "PROBATION")
            assert status.record.canary_blocks == ((1, 2, 3) if index < 3 else (1,))
        escrow.verify()
        statuses = [admission.status(k.ss58, now=network.now) for k in service.HOT]
        assert all(s.eligible and s.funding.locked_units == 1000 for s in statuses)
        assert len(list((root / "work").glob("*/after.json"))) == 80
        assert (
            network.store._db.execute(
                "SELECT COUNT(*) FROM admission_trial_results WHERE outcome='MATCH'"
            ).fetchone()[0]
            == 40
        )
        # Missing completion custody stops; never synthesize it from clean_count.
        for row in network.store._db.execute(
            "SELECT t.epoch,t.admission_id,a.hotkey FROM admission_trials t "
            "JOIN admissions_v2 a ON a.admission_id=t.admission_id "
            "WHERE t.finalized_beacon IS NOT NULL"
        ):
            directory = root / "trials" / str(row["epoch"])
            if not (directory / "completed.json").exists():
                raise RuntimeError("missing original completion custody; cannot skip/reconstruct")
        profile = network.store._service_profile_v2(run)
        # Existing charged genesis runner: test-only manager placement, no product unlock.
        authenticate(root, scope)
        from hypertrain.protocol.messages import Receipt, f32hex
        from hypertrain.protocol.messages_v2 import PolicyHashes, RosterEntryV2, RoundOpenV2

        profile_bytes = canonicalize(profile.model_dump(mode="json"))
        owner_path = root / "genesis-owner.json"
        if not owner_path.exists():
            save(
                owner_path,
                network.signed(
                    service.OWNER,
                    "Receipt",
                    Receipt(w=0, commit_hash=sha256_hex(profile_bytes), received_round=network.now),
                ),
            )
        network.store.bootstrap_service_capacity_v2(
            run, profile_bytes, canonicalize(load(owner_path))
        )
        entries = [
            RosterEntryV2(
                hotkey=s.record.hotkey,
                slot=i,
                q_i=f32hex(1),
                admission_id=s.record.admission_id,
                coldkey_group=s.record.coldkey,
                state=s.record.state,
                eligible_weight=4194304,
            )
            for i, s in enumerate(statuses)
        ]
        final_round = min(
            network.now + 100,
            int((authority["deadline"] - network.manifest.training.beacon.genesis_time) // 3) + 1,
        )
        intent = RoundOpenV2(
            w=0,
            prev_final_hash="0" * 64,
            theta_hash="0" * 64,
            outer_state_hash="0" * 64,
            center_hash="0" * 64,
            roster_hash=sha256_hex(canonicalize([e.body() for e in entries])),
            honeypot_commit="0" * 64,
            d_open=network.now + 20,
            d_assign=network.now + 21,
            d_commit=network.now + 40,
            d_audit=network.now + 41,
            d_upload=network.now + 42,
            d_final=final_round,
            contract_version=2,
            policy_hashes=PolicyHashes.model_validate(
                {k: getattr(network.manifest.network, k) for k in PolicyHashes.model_fields}
            ),
            registry_epoch=network.store._registry_v2(network.manifest).epoch,
            start_state_index_hash="0" * 64,
            audit_mode="anchored-full",
            roster=entries,
        )
        intent_path = root / "genesis-intent.json"
        if not intent_path.exists():
            save(intent_path, network.signed(service.COORD, "RoundOpenV2", intent))
        original_popen = subprocess.Popen

        def contained_popen(argv: list[str], **kwargs: Any) -> subprocess.Popen:
            if (
                argv
                and Path(argv[0]).name == "systemd-run"
                and any(x.startswith("--unit=ht-cap-") for x in argv)
            ):
                authenticate(root, scope)
                unit_name = next(x.split("=", 1)[1] for x in argv if x.startswith("--unit="))
                journal = root / "genesis-units" / f"{unit_name}.json"
                if journal.exists():
                    raise RuntimeError("genesis manager unit recorded; never reset or rerun")
                save(
                    journal,
                    {
                        "unit": unit_name,
                        "slice": authority["slice"],
                        "deadline": authority["deadline"],
                        "argv": argv,
                    },
                )
                argv = [argv[0], "--slice=" + authority["slice"], *argv[1:]]
                if unit_name.endswith(".service") and "--wait" in argv:
                    if "--property=CPUQuota=50%" not in argv:
                        raise RuntimeError("original genesis caller did not bind 50% quota")
            return original_popen(argv, **kwargs)

        patch.setattr(subprocess, "Popen", contained_popen)
        prepared_path = root / "genesis-prepared.json"
        if prepared_path.exists():
            if load(prepared_path) != network.store._record_v2(run, "genesis-prepared", "run"):
                raise RuntimeError("original prepared genesis custody differs")
        else:
            # _prepare commits service-charge through CapacityAttempt BEFORE manager launch.
            prepared = network.store._prepare_capacity_genesis_v2(
                run, canonicalize(load(intent_path))
            )
            save(prepared_path, prepared)
        if time.time() >= authority["deadline"]:
            raise RuntimeError("overall admission+genesis cutoff elapsed")
        # Real prepared objects/current runtime, real signed HTTP publication; no tensor parent.
        from hypertrain.challenge.store import ChallengeError

        prepared = load(prepared_path)
        body = RoundOpenV2.model_validate(load(intent_path)["body"])
        result = prepared["result"]
        body = body.model_copy(
            update={
                "theta_hash": result["starts"][0]["theta_hash"],
                "outer_state_hash": result["outer_hashes"]["outer_state_hash"],
                "center_hash": result["outer_hashes"]["center_hash"],
                "start_state_index_hash": sha256_hex(canonicalize(result["starts"])),
            }
        )
        signed_opening = network.signed(service.COORD, "RoundOpenV2", body)
        save(root / "opening-request.json", signed_opening)

        def forbidden_parent(*args: Any, **kwargs: Any) -> None:
            raise RuntimeError("protected opening reached parent tensor/child launch")

        import hypertrain.auditor.replay as replay_module
        import hypertrain.trainer.model as model_module

        patch.setattr(model_module, "init_params", forbidden_parent)
        patch.setattr(replay_module.AnchorCache, "genesis", forbidden_parent)
        patch.setattr(replay_module, "pack_state", forbidden_parent)
        patch.setattr(replay_module, "unpack_state", forbidden_parent)
        patch.setattr(network.store, "_restore_anchor_v2", forbidden_parent)
        original_record = network.store._record_v2
        negatives = []
        for fault in ("source", "roster", "prepared-digest", "cas"):
            changes = network.store._db.total_changes
            calls = 0

            def stale_record(r: str, kind: str, identity: str, fault: str = fault) -> dict:
                nonlocal calls
                value = original_record(r, kind, identity)
                if fault == "source" and kind == "service-admission":
                    value = {**value, "body": {**value["body"], "implementation_hash": "99" * 32}}
                if kind == "genesis-prepared":
                    calls += 1
                    if fault == "prepared-digest" or (fault == "cas" and calls == 2):
                        value = {**value, "request_hash": "99" * 32}
                return value

            candidate = signed_opening
            if fault == "roster":
                wrong = body.model_copy(
                    update={
                        "roster": list(reversed(body.roster)),
                        "roster_hash": sha256_hex(
                            canonicalize([e.body() for e in reversed(body.roster)])
                        ),
                    }
                )
                candidate = network.signed(service.COORD, "RoundOpenV2", wrong)
            with pytest.MonkeyPatch.context() as negative_patch:
                negative_patch.setattr(network.store, "_record_v2", stale_record)
                try:
                    network.store.open_round_v2(run, canonicalize(candidate))
                except ChallengeError as exc:
                    expected = {
                        "source": "SERVICE_PROFILE_AUTHORITY",
                        "roster": "SERVICE_PREPARED_OPEN_BINDING",
                        "prepared-digest": "SERVICE_PREPARED_OPEN_BINDING",
                        "cas": "SERVICE_PROTECTED_OPEN_CAS",
                    }[fault]
                    if expected not in str(exc):
                        raise
                    negatives.append(
                        {
                            "fault": fault,
                            "error": str(exc),
                            "sql_changes": network.store._db.total_changes - changes,
                            "injected_read_race": fault in ("source", "prepared-digest", "cas"),
                        }
                    )
                else:
                    raise RuntimeError("stale opening unexpectedly published")
            if (
                network.store._db.total_changes != changes
                or (network.store.state_dir / "anchors-v2").exists()
            ):
                raise RuntimeError("stale opening published before authority/CAS guard")
        save(root / "opening-negatives.json", {"controls": negatives})
        calls_before = len(list((root / "work").glob("*/before.json")))
        response = network.client.post(
            network.url + "/admin/rounds", json=signed_opening, headers=service.admin()
        )
        save(
            root / "opening-response.json",
            {"status_code": response.status_code, "body": response.json()},
        )
        assert response.status_code == 200, response.text
        assert response.json() == signed_opening
        assert len(list((root / "work").glob("*/before.json"))) == calls_before == 80
        assert original_record(run, "round", "0") == signed_opening
        for start in result["starts"]:
            assert original_record(run, "start", "0:" + start["hotkey"]) == start
            assert (
                original_record(run, "anchor", "-1:" + start["hotkey"])["anchor_hash"]
                == start["parent_anchor_hash"]
            )
        changes = network.store._db.total_changes
        repeated = network.client.post(
            network.url + "/admin/rounds", json=signed_opening, headers=service.admin()
        )
        assert repeated.status_code == 409 and network.store._db.total_changes == changes
        save(
            root / "opening-repeat.json",
            {"status_code": repeated.status_code, "body": repeated.json(), "sql_changes": 0},
        )
        save(
            root / "eligible.json",
            {
                "manifest": network.manifest.body(),
                "run_id": run,
                "profile": profile.model_dump(mode="json"),
                "beacon": network.now,
                "source_map": load(root / "sources.json"),
                "clean_counts": list(TARGETS),
                "trajectories": 80,
                "finalizations": 40,
                "genesis_receipt": load(prepared_path),
                "opening": response.json(),
            },
        )
        print("ELIGIBLE_EXACT728_3ACTIVE12_1PROBATION4", flush=True)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", type=Path)
    parser.add_argument("--approve", choices=[APPROVAL])
    parser.add_argument("--inside", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--account", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--collect", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--prepare-budget", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--scope", type=Path, help=argparse.SUPPRESS)
    parser.add_argument("--approved-pins", type=Path)
    args = parser.parse_args()
    root = args.root.resolve()
    if args.prepare_budget:
        if args.approved_pins is None:
            parser.error("pre-admission requires ROOT allocation/carry pins")
        prepare_allocation_budget(root, args.approved_pins.resolve())
    elif args.collect:
        if args.approved_pins is None:
            parser.error("collector requires original root pins")
        terminal_collect(root, args.approved_pins.resolve())
    elif args.account:
        record = load(root / "before.json")
        group = Path("/sys/fs/cgroup") / manager(record["slice"])["ControlGroup"].lstrip("/")
        cpu = (
            int(
                dict(line.split() for line in (group / "cpu.stat").read_text().splitlines())[
                    "usage_usec"
                ]
            )
            - record["owned_counter_baseline_usec"]
        )
        identity = {
            k: record[k]
            for k in (
                "attempt",
                "run_root",
                "unit",
                "slice",
                "controller_pid",
                "controller_start",
                "sources",
                "started",
                "cutoff",
            )
        }
        fixture_path = root.parent.parent / "fixture.json"
        manifest_hash = (
            hashlib.sha256(
                json.dumps(
                    load(fixture_path)["manifest"], sort_keys=True, separators=(",", ":")
                ).encode()
            ).hexdigest()
            if fixture_path.exists()
            else None
        )
        receipt = {
            "cpu_usec": cpu,
            "memory_events": (group / "memory.events").read_text(),
            "identity": identity,
            "manifest_sha256": manifest_hash,
            "invocation_id": os.environ["INVOCATION_ID"],
        }
        subprocess.run(
            ["systemd-cat", "--identifier=" + f"ht-opening-{root.name}-kernel"],
            input=json.dumps(receipt, sort_keys=True, separators=(",", ":")) + "\n",
            text=True,
            check=True,
        )
        save(root / "kernel-final.json", receipt)
    elif args.inside:
        if args.scope is None:
            parser.error("missing external controller scope")
        generate(root, args.scope.resolve())
    elif args.approve != APPROVAL:
        parser.error("explicit user approval required; no default trajectory execution")
    else:
        if args.approved_pins is None:
            parser.error("independently approved final admission pins required")
        supervise(root, args.approved_pins.resolve())


if __name__ == "__main__":
    main()
