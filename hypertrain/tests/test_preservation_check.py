import hashlib
import importlib.util
import json
from pathlib import Path

import pytest

_spec = importlib.util.spec_from_file_location(
    "preservation_check", Path(__file__).parents[1] / "scripts" / "preservation_check.py"
)
assert _spec and _spec.loader
pc = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(pc)


FREEZE = "research-quality-multiregion-20261004"
STATE = ".omo/ulw-execute/ledger.jsonl"


@pytest.fixture(autouse=True)
def workspace(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    """A private workspace with the freeze inventory, so no host path is needed."""
    root = tmp_path / "ws"
    files = {f"{FREEZE}/a.txt": b"frozen\n", STATE: b"state\n"}
    for rel, data in files.items():
        (root / rel).parent.mkdir(parents=True, exist_ok=True)
        (root / rel).write_bytes(data)
    inventory = {rel: hashlib.sha256(data).hexdigest() for rel, data in files.items()}
    (root / FREEZE / "inventory.json").write_text(json.dumps(inventory))
    (root / FREEZE / "freeze.json").write_text("{}")
    monkeypatch.setattr(pc, "ROOT", root)
    monkeypatch.setattr(pc, "FREEZE_DIR", root / FREEZE)
    return root


def _baseline(tmp_path: Path, mutate: str) -> Path:
    files = pc.snapshot()["files"]
    target = next(k for k in sorted(files) if files[k] and k.startswith(mutate))
    files[target] = "0" * 64
    out = tmp_path / "baseline.json"
    out.write_text(json.dumps({"files": files}))
    return out


def test_ignored_path_change_passes(tmp_path: Path) -> None:
    assert pc.main(["--compare", str(_baseline(tmp_path, ".omo/ulw-execute/"))]) == 0


def test_non_ignored_change_fails(tmp_path: Path) -> None:
    assert pc.main(["--compare", str(_baseline(tmp_path, "research-quality-"))]) == 1
