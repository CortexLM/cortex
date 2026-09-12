#!/usr/bin/env python3
"""Tick a Proof checklist against the unpacked miner artefact. No inference.

Reads ``PROOF_RULES_FILE`` (a ``RuleSet`` object or a ``[{id, text}]`` array)
and writes ``checklist.json``. **How each rule is ticked is topic data**, not
a list compiled here — the signed ``constraints.params`` name it:

* ``inspect_marker_rules`` (``PROOF_PARAM_INSPECT_MARKER_RULES``):
  ``<rule_id>:<marker>|<marker>;<rule_id>:<marker>`` — a rule that **fails**
  when any of its markers (case-insensitive substrings) appears in the
  artefact text or file names. The rule-id strings themselves are never
  markers: a README or comment that names the rule is compliance language.
  A file / byte-limit truncation marks the scan incomplete and **fails**
  every marker rule — truncated absence is not a clean pass.
* ``inspect_attested_rules`` (``PROOF_PARAM_INSPECT_ATTESTED_RULES``): comma
  list of rule ids the host / topic enforce outside this scan (sandbox
  attestation, BYOK routing, seed, promotion policy, …). They pass with
  evidence saying so; inspect ran no inference for them.

A rule the signed topic names in neither list **fails closed** with
evidence naming the two params — an unknown rule is never a silent pass.
Secret file contents are never printed.
"""

from __future__ import annotations

import json
import os
import re
import sys
from pathlib import Path
from typing import Any

MAX_FILE_BYTES = 128 * 1024
MAX_FILES = 256
MAX_EVIDENCE = 2000
MAX_TOTAL_BYTES = 2 * 1024 * 1024
PARAM_MARKER_RULES = "PROOF_PARAM_INSPECT_MARKER_RULES"
PARAM_ATTESTED_RULES = "PROOF_PARAM_INSPECT_ATTESTED_RULES"
RULE_ID = re.compile(r"^[a-z0-9][a-z0-9_-]{1,63}$")


def _fail(msg: str, code: int = 2) -> None:
    print(msg, file=sys.stderr)
    raise SystemExit(code)


def _clip(text: str) -> str:
    text = " ".join(text.split())
    if len(text) > MAX_EVIDENCE:
        return text[: MAX_EVIDENCE - 3] + "..."
    return text


def load_rules(path: Path) -> list[dict[str, str]]:
    try:
        obj = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as e:
        _fail(f"cannot read rules from {path}: {e}")
    rules: Any
    if isinstance(obj, list):
        rules = obj
    elif isinstance(obj, dict) and isinstance(obj.get("rules"), list):
        rules = obj["rules"]
    else:
        _fail(f"{path} is not a RuleSet or a rule array")
    out: list[dict[str, str]] = []
    for item in rules:
        if not isinstance(item, dict):
            continue
        rid = item.get("id")
        if isinstance(rid, str) and rid:
            text = item.get("text") if isinstance(item.get("text"), str) else ""
            out.append({"id": rid, "text": text})
    if not out:
        _fail(f"{path} has no rules to tick")
    return out


def parse_marker_rules(raw: str | None) -> dict[str, tuple[str, ...]]:
    """``rule:marker|marker;rule:marker`` → ``{rule: (markers…)}``.

    Fails closed on a malformed entry: a rule with no marker, a marker equal
    to a rule id, or an id that is not a checklist id shape.
    """
    out: dict[str, tuple[str, ...]] = {}
    if not raw or not raw.strip():
        return out
    for entry in raw.split(";"):
        entry = entry.strip()
        if not entry:
            continue
        rid, sep, markers_raw = entry.partition(":")
        rid = rid.strip()
        if not sep or not RULE_ID.match(rid):
            _fail(f"{PARAM_MARKER_RULES}: {entry!r} is not <rule_id>:<marker>|<marker>")
        markers = tuple(
            m.strip().lower() for m in markers_raw.split("|") if m.strip()
        )
        if not markers:
            _fail(f"{PARAM_MARKER_RULES}: rule {rid!r} names no marker")
        if rid in out:
            _fail(f"{PARAM_MARKER_RULES}: rule {rid!r} listed twice")
        out[rid] = markers
    # A marker that occurs inside a rule id would fail miners who name the
    # rule in a README; the topic must pick markers that are not part of
    # any compliance vocabulary it publishes.
    for rid, markers in out.items():
        for m in markers:
            if any(m in other for other in out):
                _fail(
                    f"{PARAM_MARKER_RULES}: marker {m!r} of rule {rid!r} occurs inside a rule id; "
                    "naming a rule is compliance language, not a cheat marker"
                )
    return out


def parse_attested_rules(raw: str | None) -> frozenset[str]:
    if not raw or not raw.strip():
        return frozenset()
    ids: set[str] = set()
    for part in re.split(r"[,\s]+", raw.strip()):
        if not part:
            continue
        if not RULE_ID.match(part):
            _fail(f"{PARAM_ATTESTED_RULES}: {part!r} is not a checklist rule id")
        ids.add(part)
    return frozenset(ids)


def collect_artefact_text(root: Path | None) -> tuple[str, int, list[str], bool]:
    """Return ``(text, n_scanned, names, incomplete)``.

    ``incomplete`` is true when a file or byte cap stopped the walk before
    every regular file was considered. Callers must not treat a truncated
    scan as proof that an off-limits marker is absent.
    """
    if root is None or not root.is_dir():
        return "", 0, [], False
    blobs: list[str] = []
    names: list[str] = []
    total = 0
    n_files = 0
    incomplete = False
    for path in sorted(root.rglob("*")):
        if not path.is_file():
            continue
        rel = path.relative_to(root).as_posix()
        if ".." in Path(rel).parts:
            continue
        if n_files >= MAX_FILES or total >= MAX_TOTAL_BYTES:
            incomplete = True
            break
        names.append(rel)
        n_files += 1
        try:
            size = path.stat().st_size
        except OSError:
            continue
        if size == 0 or size > MAX_FILE_BYTES:
            continue
        try:
            data = path.read_bytes()
        except OSError:
            continue
        if b"\x00" in data[:1024]:
            continue
        total += len(data)
        blobs.append(data.decode("utf-8", errors="replace"))
    return "\n".join(blobs).lower(), n_files, names, incomplete


def _off_limits_hits(
    artefact_text: str, names_joined: str, needles: tuple[str, ...]
) -> list[str]:
    """Needles found in artefact text or file names (order preserved)."""
    found: list[str] = []
    for needle in needles:
        key = needle.lower()
        if key in artefact_text or key in names_joined:
            found.append(needle)
    return found


def tick_rule(
    rule: dict[str, str],
    artefact_text: str,
    n_files: int,
    names: list[str],
    has_artefact: bool,
    incomplete: bool = False,
    marker_rules: dict[str, tuple[str, ...]] | None = None,
    attested: frozenset[str] = frozenset(),
) -> dict[str, Any]:
    rid = rule["id"]
    joined_names = " ".join(names).lower()
    marker_rules = marker_rules or {}

    if rid in marker_rules:
        hits = _off_limits_hits(artefact_text, joined_names, marker_rules[rid])
        if hits:
            return {
                "id": rid,
                "pass": False,
                "evidence": _clip(
                    f"off-limits marker for rule {rid} in artefact: {', '.join(hits)}"
                ),
            }
        if incomplete:
            return {
                "id": rid,
                "pass": False,
                "evidence": _clip(
                    f"artefact scan incomplete (file/byte limit); cannot treat absence "
                    f"of off-limits markers as clean ({n_files} files scanned)"
                ),
            }
        return {
            "id": rid,
            "pass": True,
            "evidence": _clip(
                f"none of the {len(marker_rules[rid])} signed markers for rule {rid} "
                f"in {n_files} artefact files"
            ),
        }

    if rid in attested:
        return {
            "id": rid,
            "pass": True,
            "evidence": _clip(
                f"rule {rid} is host/topic-enforced (signed inspect_attested_rules); "
                f"inspect ran no inference; artefact_files={n_files} present={has_artefact}"
            ),
        }

    return {
        "id": rid,
        "pass": False,
        "evidence": _clip(
            f"rule {rid}: the signed topic names it in neither inspect_marker_rules nor "
            f"inspect_attested_rules; inspect fails closed rather than guess "
            f"(artefact_present={has_artefact}, files={n_files})"
        ),
    }


def main(argv: list[str] | None = None) -> int:
    del argv
    rules_path = Path(os.environ.get("PROOF_RULES_FILE", ""))
    output_dir = Path(os.environ.get("PROOF_OUTPUT_DIR", ""))
    if not rules_path.is_file():
        _fail("PROOF_RULES_FILE is required and must be a file")
    if not output_dir.is_dir():
        _fail("PROOF_OUTPUT_DIR is required and must be a directory")
    marker_rules = parse_marker_rules(os.environ.get(PARAM_MARKER_RULES))
    attested = parse_attested_rules(os.environ.get(PARAM_ATTESTED_RULES))
    if both := sorted(set(marker_rules) & attested):
        _fail(f"rules listed as both marker and attested: {', '.join(both)}")

    artefact_raw = os.environ.get("PROOF_ARTIFACT_DIR", "")
    artefact_dir = Path(artefact_raw) if artefact_raw else None
    has_artefact = artefact_dir is not None and artefact_dir.is_dir()
    text, n_files, names, incomplete = collect_artefact_text(artefact_dir)

    items = [
        tick_rule(rule, text, n_files, names, has_artefact, incomplete, marker_rules, attested)
        for rule in load_rules(rules_path)
    ]
    out = output_dir / "checklist.json"
    out.write_text(json.dumps(items, indent=2) + "\n", encoding="utf-8")
    n_red = sum(1 for i in items if not i["pass"])
    print(
        f"inspect_scan: {len(items)} rules ticked ({n_red} red); "
        f"{len(marker_rules)} marker rules, {len(attested)} attested rules from the signed topic",
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
