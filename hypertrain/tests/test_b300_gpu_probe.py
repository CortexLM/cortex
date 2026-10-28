"""scripts/b300/gpu_probe.py verdict: bitwise equality across repeats and devices, errors fail."""

from __future__ import annotations

import dataclasses
import importlib.util
import json
import os
import re
import subprocess
import sys
import types
from pathlib import Path

import pytest

from env_pin_fixtures import HOSTILE, TEST_PIN, ambient, needs_avx2

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("gpu_probe", ROOT / "scripts/b300/gpu_probe.py")
assert spec is not None and spec.loader is not None
gpu_probe = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gpu_probe)  # imports hypertrain.trainer before torch
torch = gpu_probe.torch
import hypertrain.models.kimi_tiny as kt  # noqa: E402  (after gpu_probe: determinism before torch)
import hypertrain.trainer.attention as attn  # noqa: E402

KIMI_ROOTS = ("leaves_root", "delta_hash", "state_root")


@pytest.fixture
def no_child(monkeypatch: pytest.MonkeyPatch) -> None:
    """The kimi round is a child process; a test of the other smokes does not start one."""
    monkeypatch.setattr(gpu_probe, "kimi_round", lambda device: dict.fromkeys(KIMI_ROOTS, "e" * 64))


class Child:
    """A stand-in for the process kimi_round starts: it records how it was started and answers."""

    def __init__(
        self, monkeypatch: pytest.MonkeyPatch, stdout: str = "", stderr: str = "", rc: int = 0
    ) -> None:
        self.started: list[tuple[list[str], dict[str, str]]] = []
        self.reply = (stdout, stderr, rc)
        monkeypatch.setattr(gpu_probe.subprocess, "run", self.run)

    def run(
        self, argv: list[str], *, env: dict[str, str], **_kwargs: object
    ) -> subprocess.CompletedProcess[str]:
        self.started.append((argv, env))
        stdout, stderr, rc = self.reply
        return subprocess.CompletedProcess(argv, rc, stdout, stderr)

    @property
    def device(self) -> str:
        return self.started[-1][0][self.started[-1][0].index(gpu_probe.KIMI_ROUND_FLAG) + 1]

    @property
    def pin(self) -> dict[str, object]:
        body = json.loads(self.started[-1][0][-1])
        assert isinstance(body, dict)
        return body


@pytest.mark.usefixtures("no_child")
def test_cpu_smoke_is_bitwise_stable_and_one_flipped_digest_fails():
    runs = {"cpu": [gpu_probe.workload(torch.device("cpu")) for _ in range(2)]}
    assert gpu_probe.evaluate(runs)["ok"], runs
    flipped = {**runs, "cuda:7": [{**runs["cpu"][0], "matmul_bf16": "0" * 64}]}
    verdict = gpu_probe.evaluate(flipped)
    assert not verdict["ok"] and verdict["mismatches"] == ["cuda:7 repeat 0"]


def test_a_kernel_error_on_every_device_still_fails():
    err = {"sdpa-flash": "error: CUDA error: no kernel image is available for execution"}
    verdict = gpu_probe.evaluate({f"cuda:{i}": [err, err] for i in range(8)})
    assert not verdict["ok"] and verdict["mismatches"] == []
    assert len(verdict["errors"]) == 8


def run_main(monkeypatch: pytest.MonkeyPatch, out: Path) -> int:
    argv = ["gpu_probe.py", "--out", str(out), "--device", "cpu", "--expect-gpus", "0"]
    monkeypatch.setattr(sys, "argv", [*argv, "--expect-cc", "any", "--repeats", "1"])
    return gpu_probe.main()


@pytest.mark.parametrize(
    "exc",
    [
        TypeError("unsupported operand"),
        AssertionError("x.grad is None"),
        KeyboardInterrupt(),
        SystemExit(3),
    ],
    ids=lambda e: type(e).__name__,
)
def test_any_failure_replaces_a_stale_ok_json_and_still_propagates(
    exc: BaseException, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    out = tmp_path / "gpu_probe.json"
    out.write_text(json.dumps({"ok": True, "stale": True}))

    def boom(*_args: object, **_kwargs: object) -> None:
        raise exc

    monkeypatch.setattr(gpu_probe, "attention", boom)
    with pytest.raises(type(exc)):  # a programming error is not turned into a device error
        run_main(monkeypatch, out)
    doc = json.loads(out.read_text())
    assert doc["ok"] is False and "stale" not in doc
    assert doc["error"] == f"{type(exc).__name__}: {exc}"


@pytest.mark.usefixtures("no_child")
@pytest.mark.parametrize(
    ("forced", "backend"), [(None, "math"), ("sdpa-flash", "sdpa-flash")], ids=["default", "forced"]
)
def test_the_attention_smoke_runs_the_backend_select_backend_picks(
    forced: str | None, backend: str, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(attn, "_PATHS", {})  # the CPU flash probe must not leak into other tests
    if forced is None:
        monkeypatch.delenv("HT_ATTN_BACKEND", raising=False)
    else:
        monkeypatch.setenv("HT_ATTN_BACKEND", forced)
    out = gpu_probe.workload(torch.device("cpu"))
    keys = sorted(k for k in out if k.endswith(("_out", "_dq", "_dk", "_dv")))
    assert keys == sorted(f"{backend}_{s}" for s in ("out", "dq", "dk", "dv"))


@pytest.mark.usefixtures("no_child")
def test_a_kernel_error_is_recorded_under_the_selected_backend(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    def no_kernel(*_args: object, **_kwargs: object) -> None:
        raise RuntimeError("no kernel image is available")

    monkeypatch.setenv("HT_ATTN_BACKEND", "sdpa-flash")
    monkeypatch.setattr(gpu_probe, "attention", no_kernel)
    out = gpu_probe.workload(torch.device("cpu"))
    assert out["sdpa-flash"] == "error: no kernel image is available"
    assert not any(k.startswith(("math", "sdpa-flash_")) for k in out)


@needs_avx2
def test_the_kimi_micro_roots_join_the_digests_and_a_flipped_one_is_a_mismatch() -> None:
    cpu = torch.device("cpu")
    first, second = gpu_probe.workload(cpu), gpu_probe.workload(cpu)
    doc = kt.run_manifest(dataclasses.replace(kt.MICRO, compute_dtype="bf16"))  # as production
    (res,) = kt.qualify(doc, "cpu", 1, TEST_PIN)
    for root in KIMI_ROOTS:
        assert re.fullmatch("[0-9a-f]{64}", first[f"kimi_micro_{root}"])
        assert first[f"kimi_micro_{root}"] == res[root]
    assert first == second
    flipped = {"cpu": [first], "cuda:3": [{**first, "kimi_micro_state_root": "0" * 64}]}
    verdict = gpu_probe.evaluate(flipped)
    assert not verdict["ok"] and verdict["mismatches"] == ["cuda:3 repeat 0"]


@needs_avx2
def test_the_kimi_round_runs_under_its_pin_whatever_this_process_exports(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # The attention smoke needs this process unpinned, and the trainer refuses a process that does
    # not honor the pin: the round runs in a child that gets the pinned environment. Under a hostile
    # export it yields the roots of the same round run in a process pinned from its start.
    doc = kt.run_manifest(dataclasses.replace(kt.MICRO, compute_dtype="bf16"))
    (pinned,) = kt.qualify(doc, "cpu", 1, TEST_PIN)
    ambient(monkeypatch, **HOSTILE)
    assert gpu_probe.kimi_round(torch.device("cpu")) == pinned


@pytest.mark.parametrize(
    ("device", "capability", "gpu"),
    [
        (torch.device("cpu"), None, "none"),
        (torch.device("meta"), None, "none"),
        (torch.device("cuda", 1), (10, 3), "sm_103"),
        (torch.device("cuda", 7), (12, 0), "sm_120"),
    ],
    ids=["cpu", "meta", "sm_103", "sm_120"],
)
def test_the_kimi_round_starts_on_its_device_under_the_pin_of_the_devices_class(
    device: torch.device,
    capability: tuple[int, int] | None,
    gpu: str,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    ambient(monkeypatch, HT_ATTN_BACKEND="sdpa-flash", HT_FP8="1", OMP_THREAD_LIMIT="2")
    monkeypatch.setattr(torch.cuda, "get_device_capability", lambda d: capability)
    child = Child(monkeypatch, stdout=json.dumps({"round": dict.fromkeys(KIMI_ROOTS, "e" * 64)}))

    assert gpu_probe.kimi_round(device) == dict.fromkeys(KIMI_ROOTS, "e" * 64)

    assert child.device == str(device)
    assert (child.pin["cpu"], child.pin["gpu"]) == ("avx2", gpu)
    env = child.started[-1][1]
    assert env["HT_ATTN_BACKEND"] == "math" and env["ATEN_CPU_CAPABILITY"] == "avx2"
    assert "HT_FP8" not in env and "OMP_THREAD_LIMIT" not in env


def test_a_device_class_without_a_pin_row_is_refused_before_any_child_starts(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(torch.cuda, "get_device_capability", lambda d: (9, 0))
    child = Child(monkeypatch)

    with pytest.raises(ValueError, match="sm_90"):
        gpu_probe.kimi_round(torch.device("cuda", 0))

    assert child.started == []


def test_a_programming_error_of_the_round_ends_the_child_instead_of_answering(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    def broken(*_args: object, **_kwargs: object) -> None:
        raise TypeError("not a kernel error")

    monkeypatch.setattr(kt, "qualify", broken)
    with pytest.raises(TypeError, match="not a kernel error"):
        gpu_probe.kimi_child("cpu", json.dumps(TEST_PIN.body()))


def test_a_child_that_dies_fails_the_probe_with_what_it_said(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    Child(monkeypatch, stderr="Traceback\nImportError: cannot import name 'kimi_tiny'", rc=1)

    with pytest.raises(ChildProcessError, match=r"(?s)exited 1: .*cannot import name 'kimi_tiny'"):
        gpu_probe.workload(torch.device("cpu"))


def test_a_kimi_runtime_error_is_recorded_as_a_digest_error(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    def no_kernel(*_args: object, **_kwargs: object) -> None:
        raise RuntimeError("no kernel image is available")

    # What the child prints for a RuntimeError of the round is what the parent reads.
    monkeypatch.setattr(kt, "qualify", no_kernel)
    assert gpu_probe.kimi_child("cpu", json.dumps(TEST_PIN.body())) == 0
    Child(monkeypatch, stdout=capsys.readouterr().out)
    monkeypatch.delenv("HT_ATTN_BACKEND", raising=False)  # the CPU default backend is math
    out = gpu_probe.workload(torch.device("cpu"))
    assert out["kimi_micro"] == "error: no kernel image is available"
    assert not any(k.startswith("kimi_micro_") for k in out)
    assert "matmul_fp32" in out and "math_out" in out  # the other smokes still ran
    errors = gpu_probe.evaluate({"cuda:0": [out]})["errors"]
    assert errors == ["cuda:0: kimi_micro: error: no kernel image is available"]


@needs_avx2
def test_an_image_without_kimi_still_leaves_a_failed_json(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # The pinned image installs hypertrain at build time; one built before kimi-k3-pro-tiny has no
    # such module. That must fail the probe through the JSON: an import error while the script
    # loads would end it before main() and leave an earlier run's ok:true in place. The module is
    # missing in the child only: its sitecustomize is found first on the child's path.
    block = tmp_path / "block"
    block.mkdir()
    (block / "sitecustomize.py").write_text(
        'import sys\nsys.modules["hypertrain.models.kimi_tiny"] = None\n'
    )
    path = [str(block), *filter(None, [os.environ.get("PYTHONPATH")])]
    monkeypatch.setenv("PYTHONPATH", os.pathsep.join(path))
    out = tmp_path / "gpu_probe.json"
    out.write_text(json.dumps({"ok": True, "stale": True}))
    with pytest.raises(ChildProcessError, match="kimi_tiny"):
        run_main(monkeypatch, out)
    doc = json.loads(out.read_text())
    assert doc["ok"] is False and "stale" not in doc and "kimi_tiny" in doc["error"]


B300 = {"capability": [10, 3], "sm_count": 148}
HOST = {
    "torch": "2.14.0+cu130",
    "torch_cuda": "13.0",
    "device_count": 8,
    "devices": [B300] * 8,
    "smoke": {"ok": True},
    "arch_list": [],
}


def test_the_expected_host_passes_every_check() -> None:
    assert set(gpu_probe.checks(HOST, 8, (10, 3)).values()) == {True}


WRONG_FACTS = [
    pytest.param({"torch": "2.13.0+cu130"}, "torch_cu130", id="torch"),
    pytest.param({"torch_cuda": "12.9"}, "torch_cu130", id="cuda"),
    pytest.param({"device_count": 7, "devices": [B300] * 7}, "gpu_count", id="fewer"),
    pytest.param({"device_count": 9, "devices": [B300] * 9}, "gpu_count", id="more"),
    pytest.param(
        {"devices": [B300] * 7 + [{**B300, "capability": [10, 0]}]}, "capability", id="mixed-cc"
    ),
    pytest.param(
        {"devices": [B300] * 7 + [{**B300, "sm_count": 132}]}, "same_sm_count", id="mixed-sm"
    ),
    pytest.param({"smoke": {"ok": False}}, "bitwise_smoke", id="smoke"),
]


@pytest.mark.parametrize(("change", "failed"), WRONG_FACTS)
def test_one_wrong_fact_fails_exactly_its_check(change: dict[str, object], failed: str) -> None:
    checks = gpu_probe.checks({**HOST, **change}, 8, (10, 3))
    assert [name for name, ok in checks.items() if not ok] == [failed]


@pytest.mark.parametrize(("change", "failed"), [pytest.param({}, None, id="ok"), *WRONG_FACTS])
def test_the_exit_code_and_the_json_follow_the_verdict(
    change: dict[str, object],
    failed: str | None,
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    out = tmp_path / "gpu_probe.json"
    monkeypatch.setattr(gpu_probe, "probe", lambda *_: {**HOST, **change})
    argv = ["gpu_probe.py", "--out", str(out), "--expect-gpus", "8", "--expect-cc", "10.3"]
    monkeypatch.setattr(sys, "argv", argv)
    rc = gpu_probe.main()
    doc = json.loads(out.read_text())
    # Exit code, JSON verdict and the failed check must agree, whichever fact is wrong.
    assert (rc, doc["ok"]) == ((0, True) if failed is None else (1, False))
    assert [n for n, ok in doc["checks"].items() if not ok] == ([failed] if failed else [])


def test_the_smoke_runs_on_the_device_under_test(monkeypatch: pytest.MonkeyPatch) -> None:
    # No GPU needed: the meta device has no data but takes the same code path. The backend must be
    # asked about this device and the kimi round must be started on it, whatever the current CUDA
    # device is. (select_backend answers for any non-CUDA device with "math", so only a stand-in
    # that checks its argument can tell a wrong device from the right one.)
    meta = torch.device("meta")

    def pick(device: torch.device) -> str:
        assert device == meta, f"select_backend asked about {device}"
        return "math"

    child = Child(monkeypatch, stdout=json.dumps({"round": dict.fromkeys(KIMI_ROOTS, "e" * 64)}))
    monkeypatch.setattr(gpu_probe, "select_backend", pick)
    monkeypatch.setattr(gpu_probe, "digest", lambda t: "d" * 64)
    out = gpu_probe.workload(meta)
    assert len(child.started) == 1 and child.device == "meta"
    assert out["math_out"] == "d" * 64 and out["kimi_micro_state_root"] == "e" * 64


@needs_avx2
def test_every_repeat_runs_the_whole_smoke() -> None:
    ev = gpu_probe.probe(2, "cpu")
    runs = ev["runs"]["cpu"]
    assert ev["repeats"] == 2 and len(runs) == 2
    assert all(f"kimi_micro_{root}" in run for run in runs for root in KIMI_ROOTS)
    assert runs[0] == runs[1] and ev["smoke"]["ok"]


def test_probe_runs_the_smoke_once_per_gpu_and_repeat_and_records_each_device(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # No GPU needed: the CUDA branch only asks torch for the device list and properties. A probe
    # that smoked GPU 0 under every label would give identical digests and a green verdict.
    props = [
        types.SimpleNamespace(
            name=f"B300-{i}",
            major=10,
            minor=3,
            multi_processor_count=148 + i,
            total_memory=1000 + i,
        )
        for i in range(3)
    ]
    seen: list[str] = []

    def smoke(device: torch.device) -> dict[str, str]:
        seen.append(str(device))
        return {"digest": "same"}

    monkeypatch.setattr(torch.cuda, "is_available", lambda: True)
    monkeypatch.setattr(torch.cuda, "device_count", lambda: 3)
    monkeypatch.setattr(torch.cuda, "get_device_properties", lambda d: props[d.index])
    monkeypatch.setattr(gpu_probe, "select_backend", lambda d: "math")
    monkeypatch.setattr(gpu_probe, "workload", smoke)
    ev = gpu_probe.probe(2, "cuda")
    assert seen == ["cuda:0"] * 2 + ["cuda:1"] * 2 + ["cuda:2"] * 2
    assert list(ev["runs"]) == ["cuda:0", "cuda:1", "cuda:2"] and ev["device_count"] == 3
    assert [(d["index"], d["name"], d["capability"]) for d in ev["devices"]] == [
        (i, f"B300-{i}", [10, 3]) for i in range(3)
    ]
    assert [(d["sm_count"], d["total_memory"]) for d in ev["devices"]] == [
        (148 + i, 1000 + i) for i in range(3)
    ]


def test_a_programming_error_in_the_matmul_smoke_surfaces(monkeypatch: pytest.MonkeyPatch) -> None:
    # Only a RuntimeError (a kernel error) is a device result; anything else is a defect.
    real_digest, calls = gpu_probe.digest, []

    def broken_once(t: torch.Tensor) -> str:
        calls.append(1)
        if len(calls) == 1:  # the first matmul only: the rest of the smoke would not hide a leak
            raise ValueError("not a kernel error")
        return real_digest(t)

    monkeypatch.setattr(gpu_probe, "digest", broken_once)
    with pytest.raises(ValueError, match="not a kernel error"):
        gpu_probe.workload(torch.device("cpu"))


@pytest.mark.parametrize("runs", [{}, {"cuda:0": []}], ids=["no-devices", "no-repeats"])
def test_a_probe_that_ran_nothing_does_not_pass(runs: dict[str, list[dict[str, str]]]) -> None:
    verdict = gpu_probe.evaluate(runs)
    assert verdict["ok"] is False and verdict["mismatches"] == ["no run"]


@pytest.mark.usefixtures("no_child")
def test_the_smoke_computes_in_the_miners_dtypes(monkeypatch: pytest.MonkeyPatch) -> None:
    dtypes: list[torch.dtype] = []
    real_digest, real_attention = gpu_probe.digest, gpu_probe.attention

    def spy_digest(t: torch.Tensor) -> str:
        dtypes.append(t.dtype)
        return real_digest(t)

    attention_in: list[torch.dtype] = []

    def spy_attention(
        q: torch.Tensor, k: torch.Tensor, v: torch.Tensor, backend: str
    ) -> torch.Tensor:
        attention_in.append(q.dtype)
        return real_attention(q, k, v, backend)

    monkeypatch.setattr(gpu_probe, "digest", spy_digest)
    monkeypatch.setattr(gpu_probe, "attention", spy_attention)
    gpu_probe.workload(torch.device("cpu"))
    assert dtypes[:2] == [torch.float32, torch.bfloat16]  # the two matmuls
    assert attention_in == [torch.bfloat16]
