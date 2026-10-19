import importlib.util
import json
import os
import signal
import stat
import subprocess
import sys
import time
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location(
    "mine_smoke_v2", ROOT / "scripts" / "mine_smoke_v2.py"
)
assert spec is not None and spec.loader is not None
mine_smoke_v2 = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mine_smoke_v2)
pytestmark = pytest.mark.fast  # listed in scripts/check-fast.sh


def ok(name: str) -> dict[str, object]:
    return {"name": name, "mode": "subprocess", "rc": 0, "ms": 1}


ALL = [ok(n) for n in mine_smoke_v2.REQUIRED]


def test_imposed_evidence_path_refuses_a_symlink(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    victim = tmp_path / "victim"
    link = tmp_path / "ev.json"
    link.symlink_to(victim)
    monkeypatch.setenv("MINE_SMOKE_V2_EVIDENCE", str(link))
    with pytest.raises(OSError):
        mine_smoke_v2.evidence_file()
    assert not victim.exists()


def test_imposed_evidence_path_is_private_and_kept_until_the_write(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    target = tmp_path / "ev.json"
    target.write_text("previous run")
    target.chmod(0o644)
    monkeypatch.setenv("MINE_SMOKE_V2_EVIDENCE", str(target))
    out, fd = mine_smoke_v2.evidence_file()
    try:
        assert out == target and stat.S_IMODE(target.stat().st_mode) == 0o600
        assert target.read_text() == "previous run"
    finally:
        os.close(fd)


def test_default_evidence_is_a_fresh_private_file(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.delenv("MINE_SMOKE_V2_EVIDENCE", raising=False)
    paths = []
    for _ in range(2):
        out, fd = mine_smoke_v2.evidence_file()
        os.close(fd)
        paths.append(out)
        assert stat.S_IMODE(out.stat().st_mode) == 0o600
    try:
        assert paths[0] != paths[1]
    finally:
        for p in paths:
            p.unlink()


def test_all_required_stages_green_is_pass() -> None:
    assert mine_smoke_v2.verdict([*ALL, {**ok("graduate-peers"), "mode": "in-process"}]) is None


@pytest.mark.parametrize(
    "broken",
    [{"rc": 2}, {"rc": None}, {"skipped": True}],
    ids=["failed", "never-finished", "skipped"],
)
def test_one_bad_stage_fails_the_run_and_is_named(broken: dict[str, object]) -> None:
    stages = [*ALL[:1], {**ok("probe-v2"), **broken}, *ALL[1:]]
    assert "probe-v2" in mine_smoke_v2.verdict(stages)


def test_a_missing_required_stage_fails_the_run() -> None:
    assert "run-v2" in mine_smoke_v2.verdict(ALL[:-1])


def test_stage_records_rc_and_raises_on_failure(monkeypatch: pytest.MonkeyPatch) -> None:
    class Done:
        returncode, stdout, stderr = 3, "", "boom"

    monkeypatch.setattr(mine_smoke_v2.subprocess, "run", lambda *a, **k: Done)
    stages: list[dict[str, object]] = []
    with pytest.raises(RuntimeError, match="probe-v2"):
        mine_smoke_v2.stage(stages, "probe-v2", ["true"])
    assert stages[0]["rc"] == 3 and stages[0]["mode"] == "subprocess" and "ms" in stages[0]
    assert "probe-v2" in mine_smoke_v2.verdict(stages)


def test_main_fails_on_a_post_stage_check_even_when_every_stage_is_green(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    def chain(tmp: Path, ev: dict, stack: object) -> None:
        ev["stages"].extend(ALL)
        raise RuntimeError("run-v2: delta recorded: False")

    evidence = tmp_path / "ev.json"
    monkeypatch.setenv("MINE_SMOKE_V2_EVIDENCE", str(evidence))
    monkeypatch.setattr(mine_smoke_v2, "chain", chain)
    assert mine_smoke_v2.main() == 1
    capsys.readouterr()
    ev = json.loads(evidence.read_text())
    assert ev["result"] == "FAIL" and "delta recorded" in ev["failed"]


def test_sigterm_still_writes_fail_evidence(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    def chain(tmp: Path, ev: dict, stack: object) -> None:
        ev["stages"].append(ok("join-v2"))
        os.kill(os.getpid(), signal.SIGTERM)  # what `timeout` sends at the budget
        time.sleep(5)

    evidence = tmp_path / "ev.json"
    evidence.write_text('{"result": "PASS"}')  # a previous run's verdict must not survive
    monkeypatch.setenv("MINE_SMOKE_V2_EVIDENCE", str(evidence))
    monkeypatch.setattr(mine_smoke_v2, "chain", chain)
    previous = signal.getsignal(signal.SIGTERM)
    try:
        assert mine_smoke_v2.main() == 1
    finally:
        signal.signal(signal.SIGTERM, previous)
    capsys.readouterr()
    ev = json.loads(evidence.read_text())
    assert ev["result"] == "FAIL" and "budget exceeded" in ev["failed"]
    assert [s["name"] for s in ev["stages"]] == ["join-v2"]


def test_reap_kills_a_detached_process_naming_the_run_dir(tmp_path: Path) -> None:
    marker = str(tmp_path / "attempt")
    proc = subprocess.Popen(  # noqa: S603 - fixed argv
        [sys.executable, "-c", "import time; time.sleep(60)", marker], start_new_session=True
    )
    try:
        assert proc.pid in mine_smoke_v2.reap(str(tmp_path))
        assert proc.wait(timeout=10) == -signal.SIGKILL
    finally:
        proc.kill()
