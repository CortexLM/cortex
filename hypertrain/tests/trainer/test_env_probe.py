"""ADR 0011 part 3: can this host honor a pin? Answered by a child started under the pinned
environment, so the parent never imports torch and never trusts its own ambient."""

from __future__ import annotations

import json
import stat
import subprocess
import sys
from pathlib import Path

import pytest

from env_pin_fixtures import CPU_FP32, ambient, clean_env, needs_avx2
from hypertrain.protocol.env_pins import (
    EnvPin,
    EnvPinUnhonorable,
    pinned_environ,
    require_honorable,
)
from hypertrain.protocol.example import example_manifest

PIN = EnvPin.from_body(CPU_FP32)


@pytest.fixture(autouse=True)
def fresh_cache() -> None:
    require_honorable.cache_clear()


def fake_python(tmp_path: Path, script: str) -> Path:
    """A stand-in for the interpreter: a real child process whose behaviour the test scripts."""
    path = tmp_path / "python"
    path.write_text("#!/bin/sh\n" + script)
    path.chmod(path.stat().st_mode | stat.S_IXUSR)
    return path


@needs_avx2
def test_probe_accepts_a_honorable_pin() -> None:
    assert require_honorable(PIN) == "AVX2"


@needs_avx2
def test_probe_asks_the_pinned_environment_not_the_ambient(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    ambient(monkeypatch, ATEN_CPU_CAPABILITY="default", MKL_CBWR="COMPATIBLE", HT_FP8="1")
    assert require_honorable(PIN) == "AVX2"


@pytest.mark.parametrize(
    ("script", "message"),
    [
        pytest.param(
            "printf 'x%.0s' $(seq 1 3000) >&2\n"
            "echo \"ATen CPU capability: manifest 'AVX2', live 'DEFAULT'\" >&2\nexit 1\n",
            r"ATen CPU capability: manifest 'AVX2', live 'DEFAULT'$",
            id="refused",
        ),
        pytest.param(
            "echo 'no json here'\n", r"gave no verdict: 'no json here\\n'", id="no-verdict"
        ),
        pytest.param("echo '{\"cpu\": 1}'\n", r"gave no verdict", id="wrong-verdict"),
    ],
)
def test_unhonorable_pin_carries_the_child_message(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, script: str, message: str
) -> None:
    monkeypatch.setattr(sys, "executable", str(fake_python(tmp_path, script)))
    with pytest.raises(EnvPinUnhonorable, match=message) as refused:
        require_honorable(PIN)
    assert len(str(refused.value)) <= 1000


def test_probe_child_refuses_a_process_that_diverges() -> None:
    env = pinned_environ(clean_env(), PIN) | {"ATEN_CPU_CAPABILITY": "default"}
    done = run_probe(json.dumps(CPU_FP32), env=env)
    assert done.returncode == 1
    assert done.stdout == ""
    assert done.stderr.strip().endswith("ATen CPU capability: manifest 'AVX2', live 'DEFAULT'")


def test_probe_that_cannot_start_is_unhonorable(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(sys, "executable", str(tmp_path / "missing-python"))
    with pytest.raises(EnvPinUnhonorable, match="could not run"):
        require_honorable(PIN)


def test_probe_runs_once_per_pin_and_a_refusal_is_not_remembered(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    runs = tmp_path / "runs"
    script = f"echo run >> {runs}\n"
    monkeypatch.setattr(sys, "executable", str(fake_python(tmp_path, script + "exit 1\n")))
    for _ in range(2):
        with pytest.raises(EnvPinUnhonorable):
            require_honorable(PIN)
    assert runs.read_text().count("run") == 2
    ok = script + 'echo \'{"cpu_capability": "AVX2"}\'\n'
    monkeypatch.setattr(sys, "executable", str(fake_python(tmp_path, ok)))
    assert [require_honorable(PIN), require_honorable(PIN)] == ["AVX2", "AVX2"]
    assert runs.read_text().count("run") == 3


def run_probe(*args: str, env: dict[str, str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, "-m", "hypertrain.trainer.env_probe", *args],
        env=env,
        capture_output=True,
        text=True,
        timeout=300,
        check=False,
    )


def run_cli(path: Path) -> subprocess.CompletedProcess[str]:
    return run_probe("--manifest", str(path), env=clean_env())


@needs_avx2
def test_cli_reports_the_environment_of_a_manifest(tmp_path: Path) -> None:
    body = example_manifest().body()
    body["reference_spec"]["env"]["pinned"] = CPU_FP32
    for name, document in (
        ("v1.json", body),
        ("v2.json", {"manifest_version": 2, "training": body}),
    ):
        path = tmp_path / name
        path.write_text(json.dumps(document))
        done = run_cli(path)
        assert done.returncode == 0, done.stderr
        assert json.loads(done.stdout) == {
            "honorable": True,
            "cpu_capability": "AVX2",
            "env": CPU_FP32["present"],
            "unset": CPU_FP32["absent"],
        }


def test_cli_without_a_pin_and_with_a_bad_pin(tmp_path: Path) -> None:
    body = example_manifest().body()
    plain = tmp_path / "plain.json"
    plain.write_text(json.dumps(body))
    done = run_cli(plain)
    assert (done.returncode, json.loads(done.stdout)) == (0, {"honorable": True, "pinned": False})
    body["reference_spec"]["env"]["pinned"] = {"present": {"LD_PRELOAD": "/x"}, "absent": []}
    bad = tmp_path / "bad.json"
    bad.write_text(json.dumps(body))
    done = run_cli(bad)
    assert done.returncode == 1
    assert "LD_PRELOAD='/x' is not an allowed pin" in done.stderr
