from __future__ import annotations

import json
from typing import Any

import pytest
import torch

from hypertrain.miner import admission


def test_cpu_devices_hash_equal_and_pass() -> None:
    r = admission.compute_selfcheck(["cpu", "cpu"])
    assert r["ok"] and r["hashes_equal"], r["failures"]
    assert len(r["sha256"]) == 64
    assert r["determinism"]["deterministic_algorithms"] is True
    for row in r["devices"]:
        assert row["sha256"] == r["sha256"]
        assert row["allocatable_bytes"] >= row["target_bytes"] > 0
        assert row["workload_seconds"] > 0


def test_corrupted_device_fails(monkeypatch: pytest.MonkeyPatch) -> None:
    real = admission._workload
    calls = {"n": 0}

    def corrupt(device: str) -> list[Any]:
        out = real(device)
        calls["n"] += 1
        if calls["n"] == 2:  # second "GPU" flips one ulp in its first output
            t = out[0].detach().clone()
            t.view(-1).view(torch.int32)[0] ^= 1
            out[0] = t
        return out

    monkeypatch.setattr(admission, "_workload", corrupt)
    r = admission.compute_selfcheck(["cpu", "cpu"])
    assert not r["ok"] and not r["hashes_equal"] and r["sha256"] is None
    assert r["devices"][0]["sha256"] != r["devices"][1]["sha256"]
    assert "output hashes differ across devices" in r["failures"]


def test_vram_shortfall_fails(monkeypatch: pytest.MonkeyPatch) -> None:
    real = admission._allocate_chunk
    given = {"n": 0}

    def spoofed(device: str, nbytes: int) -> Any:
        given["n"] += 1
        if given["n"] > 2:  # device "reports" 64 MiB but only backs two 8 MiB chunks
            raise torch.OutOfMemoryError("spoofed VRAM")
        return real(device, nbytes)

    monkeypatch.setattr(admission, "CHUNK_BYTES", 8 << 20)
    monkeypatch.setattr(admission, "_allocate_chunk", spoofed)
    r = admission.compute_selfcheck(["cpu"])
    assert not r["ok"] and r["hashes_equal"]
    assert r["devices"][0]["allocatable_bytes"] == 16 << 20
    assert any("allocatable" in f for f in r["failures"])


def test_cli_exit_codes(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    assert admission.main(["selfcheck", "--devices", "cpu,cpu"]) == 0
    assert json.loads(capsys.readouterr().out)["ok"] is True
    monkeypatch.setattr(admission, "_reported_bytes", lambda device: 1 << 40)
    monkeypatch.setattr(
        admission, "_allocate_chunk", lambda d, n: (_ for _ in ()).throw(torch.OutOfMemoryError())
    )
    assert admission.main(["selfcheck", "--devices", "cpu"]) == 1
    assert json.loads(capsys.readouterr().out)["ok"] is False


def test_bad_device_index_rejected() -> None:
    with pytest.raises(ValueError):
        admission.compute_selfcheck([-1])
    with pytest.raises(ValueError):
        admission.compute_selfcheck([])
