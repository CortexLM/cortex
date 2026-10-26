"""check_doc_numbers: matching tag passes, edited number fails, missing key fails."""

import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SCRIPT = ROOT / "scripts" / "check_doc_numbers.py"


def run(doc: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(SCRIPT), str(doc)], capture_output=True, text=True, check=False
    )


def test_checker(tmp_path: Path) -> None:
    ev = tmp_path / "ev.json"
    ev.write_text(json.dumps({"a": {"b": 0.0223, "c": "7"}, "xs": [1, 2]}))
    path = str(ev)  # absolute paths pass through ROOT / path unchanged
    good = tmp_path / "good.md"
    good.write_text(
        f"gap 2.23%{{ev:{path}#a.b*100}} and 7{{ev:{path}#a.c}} and 2{{ev:{path}#xs.1}}\n"
    )
    assert run(good).returncode == 0, run(good).stdout
    bad = tmp_path / "bad.md"
    bad.write_text(good.read_text().replace("2.23%", "2.24%"))
    assert run(bad).returncode == 1
    missing = tmp_path / "missing.md"
    missing.write_text(f"3{{ev:{path}#a.zzz}}\n")
    assert run(missing).returncode == 1
