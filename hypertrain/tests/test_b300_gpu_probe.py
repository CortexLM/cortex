"""scripts/b300/gpu_probe.py verdict: bitwise equality across repeats and devices, errors fail."""

from __future__ import annotations

import dataclasses
import importlib.util
import json
import re
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("gpu_probe", ROOT / "scripts/b300/gpu_probe.py")
assert spec is not None and spec.loader is not None
gpu_probe = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gpu_probe)  # imports hypertrain.trainer before torch
torch = gpu_probe.torch
import hypertrain.models  # noqa: E402  (after gpu_probe: determinism before torch)
import hypertrain.models.kimi_tiny as kt  # noqa: E402
import hypertrain.trainer.attention as attn  # noqa: E402


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


KIMI_ROOTS = ("leaves_root", "delta_hash", "state_root")


def test_the_kimi_micro_roots_join_the_digests_and_a_flipped_one_is_a_mismatch() -> None:
    cpu = torch.device("cpu")
    first, second = gpu_probe.workload(cpu), gpu_probe.workload(cpu)
    doc = kt.run_manifest(dataclasses.replace(kt.MICRO, compute_dtype="bf16"))  # as production
    (res,) = kt.qualify(doc, "cpu", 1)
    for root in KIMI_ROOTS:
        assert re.fullmatch("[0-9a-f]{64}", first[f"kimi_micro_{root}"])
        assert first[f"kimi_micro_{root}"] == res[root]
    assert first == second
    flipped = {"cpu": [first], "cuda:3": [{**first, "kimi_micro_state_root": "0" * 64}]}
    verdict = gpu_probe.evaluate(flipped)
    assert not verdict["ok"] and verdict["mismatches"] == ["cuda:3 repeat 0"]


def test_a_kimi_runtime_error_is_recorded_as_a_digest_error(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    def no_kernel(*_args: object, **_kwargs: object) -> None:
        raise RuntimeError("no kernel image is available")

    monkeypatch.delenv("HT_ATTN_BACKEND", raising=False)  # the CPU default backend is math
    monkeypatch.setattr(kt, "qualify", no_kernel)
    out = gpu_probe.workload(torch.device("cpu"))
    assert out["kimi_micro"] == "error: no kernel image is available"
    assert not any(k.startswith("kimi_micro_") for k in out)
    assert "matmul_fp32" in out and "math_out" in out  # the other smokes still ran
    errors = gpu_probe.evaluate({"cuda:0": [out]})["errors"]
    assert errors == ["cuda:0: kimi_micro: error: no kernel image is available"]


def test_an_image_without_kimi_still_leaves_a_failed_json(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # The pinned image installs hypertrain at build time; one built before kimi-k3-pro-tiny has no
    # such module. That must fail the probe through the JSON: an import error while the script
    # loads would end it before main() and leave an earlier run's ok:true in place.
    out = tmp_path / "gpu_probe.json"
    out.write_text(json.dumps({"ok": True, "stale": True}))
    monkeypatch.setitem(sys.modules, "hypertrain.models.kimi_tiny", None)
    monkeypatch.delattr(hypertrain.models, "kimi_tiny")  # `from hypertrain.models import` reads it
    fresh_spec = importlib.util.spec_from_file_location(
        "gpu_probe_without_kimi", ROOT / "scripts/b300/gpu_probe.py"
    )
    assert fresh_spec is not None and fresh_spec.loader is not None
    fresh = importlib.util.module_from_spec(fresh_spec)
    fresh_spec.loader.exec_module(fresh)
    argv = ["gpu_probe.py", "--out", str(out), "--device", "cpu", "--expect-gpus", "0"]
    monkeypatch.setattr(sys, "argv", [*argv, "--expect-cc", "any", "--repeats", "1"])
    with pytest.raises(ImportError):
        fresh.main()
    doc = json.loads(out.read_text())
    assert doc["ok"] is False and "stale" not in doc and "kimi_tiny" in doc["error"]
