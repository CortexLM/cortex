import importlib.util
import subprocess
import sys
from pathlib import Path
from types import SimpleNamespace
from typing import Any

import pytest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
spec = importlib.util.spec_from_file_location(
    "mine_multi_round_v2", ROOT / "scripts" / "mine_multi_round_v2.py"
)
assert spec is not None and spec.loader is not None
mine_multi_round_v2 = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mine_multi_round_v2)


def test_kill_probe_guard_fails_when_job_not_staged(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """run-v2 exits before its island job is staged: no kill happened, so no resume was tested."""
    m = mine_multi_round_v2
    reruns: list[int] = []
    monkeypatch.setattr(m, "open_round", lambda svc, n, w, prev: SimpleNamespace(w=w))
    monkeypatch.setattr(m, "settle", lambda svc, n, w: "final")
    monkeypatch.setattr(m, "run_v2", lambda stages, conf, w, i: reruns.append(i))
    monkeypatch.setattr(m.smoke, "reap", lambda marker: [])
    monkeypatch.setattr(
        subprocess, "run", lambda *a, **kw: SimpleNamespace(returncode=0, stdout="", stderr="")
    )
    monkeypatch.setattr(
        subprocess, "Popen", lambda *a, **kw: SimpleNamespace(poll=lambda: 0, wait=lambda: 0)
    )
    # No record of any kind: the island-job row never appears.
    db = SimpleNamespace(execute=lambda sql, args: SimpleNamespace(fetchone=lambda: (0,)))
    n: Any = SimpleNamespace(
        manifest=SimpleNamespace(run_id=lambda: "run"),
        store=SimpleNamespace(_db=db, _record_v2=lambda rid, kind, ident: {}),
    )
    svc = SimpleNamespace(HOT=[SimpleNamespace(ss58="hot0")])
    probe: dict[str, Any] = {}
    configs = [tmp_path / f"miner-{i}.toml" for i in range(4)]
    with pytest.raises(RuntimeError, match="kill probe did not interrupt a staged job"):
        m.restart_probes([], svc, n, configs, "prev", tmp_path, probe)
    assert probe["kill_mid_round"]["killed_after_job_staged"] is False
    # The rerun of miner 0 in the kill round never ran: it would have reported UPLOADED.
    assert reruns == [0, 1, 2, 3, 1, 2, 3]
