import importlib.util
import json
from pathlib import Path

_spec = importlib.util.spec_from_file_location(
    "preservation_check", Path(__file__).parents[1] / "scripts" / "preservation_check.py"
)
assert _spec and _spec.loader
pc = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(pc)


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
