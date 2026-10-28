import importlib.util
import json
import os
import stat
import tempfile
from pathlib import Path

import pytest

from env_pin_fixtures import HOSTILE, child, clean_env

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("mine_smoke", ROOT / "scripts" / "mine_smoke.py")
assert spec is not None and spec.loader is not None
mine_smoke = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mine_smoke)


def test_the_script_pins_its_process_and_its_actors_whatever_the_caller_exported() -> None:
    # The miner, aggregator and auditor processes of the round inherit the script's environment
    # and train in-process: a shell that exports none of the pin, or the wrong values, must not
    # decide what they compute.
    code = (
        "import importlib.util, os, sys\n"
        "spec = importlib.util.spec_from_file_location('mine_smoke', sys.argv[1])\n"
        "module = importlib.util.module_from_spec(spec)\n"
        "spec.loader.exec_module(module)\n"
        "from hypertrain.protocol.example import example_manifest\n"
        "from hypertrain.trainer.determinism import require_env_pin\n"
        "require_env_pin(example_manifest().reference_spec.env.pin())\n"
        "from harness import ONE_THREAD\n"
        "pin = example_manifest().reference_spec.env.pin()\n"
        "assert all(ONE_THREAD.get(k) == v for k, v in pin.present), ONE_THREAD\n"
        "assert not any(k in ONE_THREAD for k in pin.absent), ONE_THREAD\n"
    )
    done = child(code, {**clean_env(), **HOSTILE}, str(ROOT / "scripts" / "mine_smoke.py"))
    assert done.returncode == 0, done.stderr


def test_default_evidence_is_a_fresh_private_file_per_run(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    def no_round(**kw: object) -> None:
        raise RuntimeError("no round in this test")

    monkeypatch.delenv("MINE_SMOKE_EVIDENCE", raising=False)
    monkeypatch.setattr(tempfile, "tempdir", str(tmp_path))
    monkeypatch.setattr(mine_smoke, "manifest", no_round)
    planted = tmp_path / "mine-smoke-evidence.json"
    planted.symlink_to(tmp_path / "victim")
    assert mine_smoke.main() == 1
    assert mine_smoke.main() == 1
    capsys.readouterr()
    files = sorted(p for p in tmp_path.rglob("*.json") if p != planted)
    assert len(files) == 2 and files[0] != files[1]
    for f in files:
        assert stat.S_IMODE(f.stat().st_mode) == 0o600
        assert "no round in this test" in f.read_text()
    assert not (tmp_path / "victim").exists()


def test_imposed_evidence_path_is_private_and_kept_until_the_write(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    target = tmp_path / "ev.json"
    target.write_text("previous run")
    target.chmod(0o644)
    monkeypatch.setenv("MINE_SMOKE_EVIDENCE", str(target))
    out, fd = mine_smoke.evidence_file()
    try:
        assert out == target and stat.S_IMODE(target.stat().st_mode) == 0o600
        assert target.read_text() == "previous run"  # not truncated before the run
    finally:
        os.close(fd)


def test_mine_smoke_stop_failure_still_writes_evidence(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    class World:
        def __init__(self, *args: object) -> None:
            pass

        def miner(self, k: object) -> None:
            raise RuntimeError("no round in this test")

        def stop(self) -> None:
            raise TimeoutError("process wait timed out")

    target = tmp_path / "ev.json"
    monkeypatch.setenv("MINE_SMOKE_EVIDENCE", str(target))
    monkeypatch.setattr(mine_smoke, "manifest", lambda **kw: None)
    monkeypatch.setattr(mine_smoke, "World", World)
    assert mine_smoke.main() == 1
    capsys.readouterr()
    ev = json.loads(target.read_text())
    assert ev["result"] == "FAIL"
    assert ev["cleanup_failed"] == "TimeoutError: process wait timed out"
    assert "no round in this test" in ev["failed"]
