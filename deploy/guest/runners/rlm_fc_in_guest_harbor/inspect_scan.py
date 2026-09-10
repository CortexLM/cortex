#!/usr/bin/env python3
"""Tick a Proof checklist against the unpacked miner artefact. No inference.

Reads ``PROOF_RULES_FILE`` (a ``RuleSet`` object or a ``[{id, text}]`` array)
and writes ``checklist.json``. Off-limits markers ``no_eval_short_circuit``
and ``no_tb4_hardcoding`` fail those rules when they appear in the artefact
tree. A file/byte-limit truncation marks the scan incomplete and **fails**
those off-limits rules — truncated absence is not a clean pass. Unknown
rule IDs fail closed. Host-side rules are answered with evidence, not left
blank (a missing item is recorded red). Secret file contents are never
printed.
"""

from __future__ import annotations

import json
import os
import sys
from pathlib import Path
from typing import Any

MAX_FILE_BYTES = 128 * 1024
MAX_FILES = 256
MAX_EVIDENCE = 2000
MAX_TOTAL_BYTES = 2 * 1024 * 1024

OFF_LIMITS_SHORT_CIRCUIT = (
    "no_eval_short_circuit",
    "skip_eval",
    "skip_verifier",
    "always_pass_eval",
    "short_circuit_eval",
)
OFF_LIMITS_TB4 = (
    "no_tb4_hardcoding",
    "tb4_answers",
    "hardcoded_tb4",
)


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


def collect_artefact_text(root: Path | None) -> tuple[str, int, list[str], bool]:
    """Return ``(text, n_scanned, names, incomplete)``.

    ``incomplete`` is true when a file or byte cap stopped the walk, or a
    regular file was oversized / unreadable so its contents were not
    inspected. Callers must not treat that absence as a clean off-limits
    pass.
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
            incomplete = True
            continue
        if size == 0:
            continue
        if size > MAX_FILE_BYTES:
            incomplete = True
            continue
        try:
            data = path.read_bytes()
        except OSError:
            incomplete = True
            continue
        if b"\x00" in data[:1024]:
            incomplete = True
            continue
        total += len(data)
        blobs.append(data.decode("utf-8", errors="replace"))
    return "\n".join(blobs).lower(), n_files, names, incomplete


def _contains_any(haystack: str, needles: tuple[str, ...]) -> list[str]:
    return [n for n in needles if n.lower() in haystack]


def _off_limits_incomplete(rid: str, n_files: int) -> dict[str, Any]:
    return {
        "id": rid,
        "pass": False,
        "evidence": _clip(
            f"artefact scan incomplete (file/byte limit); cannot treat absence "
            f"of off-limits markers as clean ({n_files} files scanned)"
        ),
    }


def tick_rule(
    rule: dict[str, str],
    artefact_text: str,
    n_files: int,
    names: list[str],
    has_artefact: bool,
    incomplete: bool = False,
) -> dict[str, Any]:
    rid = rule["id"]
    joined_names = " ".join(names).lower()

    if rid == "no_eval_short_circuit":
        hits = _contains_any(artefact_text, OFF_LIMITS_SHORT_CIRCUIT)
        name_hits = [n for n in ("no_eval_short_circuit",) if n in joined_names]
        hits = hits or name_hits
        if hits:
            return {
                "id": rid,
                "pass": False,
                "evidence": _clip(
                    f"off-limits eval short-circuit marker in artefact: {', '.join(hits)}"
                ),
            }
        if incomplete:
            return _off_limits_incomplete(rid, n_files)
        return {
            "id": rid,
            "pass": True,
            "evidence": _clip(
                f"no off-limits eval short-circuit marker in {n_files} artefact files"
            ),
        }

    if rid == "no_tb4_hardcoding":
        hits = _contains_any(artefact_text, OFF_LIMITS_TB4)
        if hits:
            return {
                "id": rid,
                "pass": False,
                "evidence": _clip(
                    f"off-limits tb4 hardcoding marker in artefact: {', '.join(hits)}"
                ),
            }
        if incomplete:
            return _off_limits_incomplete(rid, n_files)
        return {
            "id": rid,
            "pass": True,
            "evidence": _clip(
                f"no off-limits tb4 hardcoding marker in {n_files} artefact files"
            ),
        }

    if rid == "miner_byok_openrouter":
        return {
            "id": rid,
            "pass": True,
            "evidence": _clip(
                "inspect is unpaid and is given no miner env; evaluate loads "
                "PROOF_PARAM_MINER_BYOK from PROOF_MINER_ENV_DIR and refuses the owner key"
            ),
        }

    if rid in {
        "firecracker_sister",
        "artefacts_zip",
        "auto_promote_best",
        "rlm_topic_setup_autonomous",
        "same_seed",
    }:
        return {
            "id": rid,
            "pass": True,
            "evidence": _clip(
                f"rule {rid} is host/topic-enforced; inspect did not run inference; "
                f"artefact_files={n_files} present={has_artefact}"
            ),
        }

    return {
        "id": rid,
        "pass": False,
        "evidence": _clip(
            f"rule {rid}: unsupported/unknown rule id; inspect fails closed "
            f"(artefact_present={has_artefact}, files={n_files})"
        ),
    }


def main(argv: list[str] | None = None) -> int:
    rules_path = Path(os.environ.get("PROOF_RULES_FILE", ""))
    output_dir = Path(os.environ.get("PROOF_OUTPUT_DIR", ""))
    if not rules_path.is_file():
        _fail("PROOF_RULES_FILE is required and must be a file")
    if not output_dir.is_dir():
        _fail("PROOF_OUTPUT_DIR is required and must be a directory")

    artefact_raw = os.environ.get("PROOF_ARTIFACT_DIR", "")
    artefact_dir = Path(artefact_raw) if artefact_raw else None
    has_artefact = artefact_dir is not None and artefact_dir.is_dir()
    text, n_files, names, incomplete = collect_artefact_text(artefact_dir)

    items = [
        tick_rule(rule, text, n_files, names, has_artefact, incomplete)
        for rule in load_rules(rules_path)
    ]
    out = output_dir / "checklist.json"
    out.write_text(json.dumps(items, indent=2) + "\n", encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
