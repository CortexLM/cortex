import importlib.util
import stat
import tempfile
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("mine_smoke", ROOT / "scripts" / "mine_smoke.py")
assert spec is not None and spec.loader is not None
mine_smoke = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mine_smoke)


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
