"""Check numeric claims in docs against evidence JSON.

A claim is a number immediately followed by a tag: `0.5{ev:path/to.json#dot.key.path}`.
Optional `*factor` scales the stored value, e.g. `2.23%{ev:f.json#a.b*100}`.
The doc number must equal the stored value rounded to the decimals written in the doc.
Paths resolve against the project root (parent of scripts/). Relative markdown links are
checked for existence too. Exit 0 when every claim matches, 1 on any mismatch/missing, 2 on usage.
"""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TAG = re.compile(
    r"(?P<num>[-+]?\d+(?:\.\d+)?(?:[eE][-+]?\d+)?)(?P<pct>%?)"
    r"\{ev:(?P<path>[^#}]+)#(?P<key>[^}*]+?)(?:\*(?P<mul>[-+]?\d+(?:\.\d+)?))?\}"
)
LINK = re.compile(r"\]\((?!https?:|mailto:|#)([^)#\s]+)")
_cache: dict[Path, object] = {}


def lookup(path: Path, key: str) -> float:
    if path not in _cache:
        _cache[path] = json.loads(path.read_text())
    node = _cache[path]
    for part in key.split("."):
        if isinstance(node, list):
            node = node[int(part)]
        elif isinstance(node, dict):
            node = node[part]
        else:
            raise KeyError(part)
    if isinstance(node, bool) or not isinstance(node, (int, float, str)):
        raise TypeError(f"value at {key} is not a number")
    return float(node)


def check(doc: Path) -> tuple[int, list[str]]:
    text = doc.read_text()
    errors: list[str] = []
    count = 0
    for m in TAG.finditer(text):
        count += 1
        line = text.count("\n", 0, m.start()) + 1
        where = f"{doc}:{line}: {m.group(0)}"
        try:
            value = lookup(ROOT / m.group("path"), m.group("key"))
        except (OSError, KeyError, IndexError, ValueError, TypeError) as exc:
            errors.append(f"{where}: cannot read evidence ({type(exc).__name__}: {exc})")
            continue
        value *= float(m.group("mul") or 1)
        digits = len(m.group("num").split(".")[1]) if "." in m.group("num") else 0
        if "e" in m.group("num").lower():
            ok = abs(value - float(m.group("num"))) <= abs(value) * 1e-9
        else:
            ok = abs(value - float(m.group("num"))) <= 0.5 * 10**-digits * (1 + 1e-9)
        if not ok:
            errors.append(f"{where}: doc says {m.group('num')}, evidence says {value!r}")
    for m in LINK.finditer(text):
        target = (doc.parent / m.group(1)).resolve()
        if not target.exists():
            errors.append(f"{doc}: broken link {m.group(1)}")
    if count == 0:
        errors.append(f"{doc}: no tagged numeric claims found")
    return count, errors


def main(argv: list[str]) -> int:
    if not argv:
        print("usage: check_doc_numbers.py DOC.md [DOC.md ...]", file=sys.stderr)
        return 2
    failed = False
    for name in argv:
        count, errors = check(Path(name).resolve())
        print(f"{name}: {count} tagged claims, {len(errors)} problems")
        for line in errors:
            print("  FAIL", line)
        failed = failed or bool(errors)
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
