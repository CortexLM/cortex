#!/usr/bin/env python3
"""Discover the miner harness to run. Terminus-2 is never the evaluate default.

Primary path is a **custom Python** agent in the artefact. Harbor BaseAgent
subclasses, an explicit ``harness.json``, and a classic ``run.sh`` are also
accepted. Built-in Harbor names (``terminus-2``, …) are used only when:

* baseline has no miner artefact staged (topic ``PROOF_PARAM_HARBOR_AGENT``), or
* the miner artefact explicitly names one in ``harness.json``.

Evaluate never silently wraps a staged artefact as a built-in.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path

import resolve_agent

KINDS = frozenset({"python", "harbor", "script", "builtin"})
KIND_ALIASES = {
    "py": "python",
    "custom": "python",
    "custom-python": "python",
    "custom_python": "python",
    "baseagent": "harbor",
    "harbor-agent": "harbor",
    "sh": "script",
    "shell": "script",
    "run.sh": "script",
}
HARNESS_JSON_NAMES = (
    "harness.json",
    "recipe/harness.json",
    "agent/harness.json",
    "recipe/agent/harness.json",
)
SCRIPT_NAMES = (
    "run.sh",
    "recipe/run.sh",
    "harness.sh",
    "recipe/harness.sh",
    "agent/run.sh",
)


def _fail(msg: str, code: int = 2) -> None:
    print(msg, file=sys.stderr)
    raise SystemExit(code)


def _emit(fields: dict[str, str]) -> None:
    for key in (
        "kind",
        "import_path",
        "pythonpath",
        "wrapper",
        "source",
        "entry",
        "builtin",
    ):
        print(f"{key}={fields.get(key, '')}")


def _norm_kind(raw: str) -> str:
    k = raw.strip().lower()
    k = KIND_ALIASES.get(k, k)
    if k not in KINDS:
        _fail(f"harness.json kind {raw!r} is not one of {sorted(KINDS)}")
    return k


def _artefact_file(art: Path, rel: str) -> Path | None:
    if not rel or ".." in Path(rel).parts or rel.startswith("/"):
        return None
    path = art / rel
    if path.is_file():
        return path
    return None


def _load_harness_json(art: Path) -> tuple[dict, str] | None:
    for rel in HARNESS_JSON_NAMES:
        path = art / rel
        if not path.is_file():
            continue
        try:
            obj = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError, UnicodeError) as e:
            _fail(f"cannot read {path}: {e}")
        if not isinstance(obj, dict):
            _fail(f"{path} must be a JSON object")
        return obj, f"artifact_dir/{rel}"
    return None


def _script_entry(art: Path) -> tuple[str, str] | None:
    for rel in SCRIPT_NAMES:
        path = art / rel
        if path.is_file():
            return rel, f"artifact_dir/{rel}"
    return None


def _agent_candidate(art: Path) -> tuple[Path, str] | None:
    for rel, label in (("agent", "artifact_dir/agent"), ("recipe/agent", "artifact_dir/recipe/agent")):
        d = art / rel
        if d.is_dir() and resolve_agent.is_agent_dir(d):
            return d, label
    return None


def _fields_from_agent_dir(agent_dir: Path, source: str) -> dict[str, str]:
    info = resolve_agent.inspect_agent(agent_dir)
    kind = info["kind"]
    wrapper = "1" if kind == "python" else "0"
    return {
        "kind": kind,
        "import_path": info["import_path"],
        "pythonpath": info["pythonpath"],
        "wrapper": wrapper,
        "source": source,
        "entry": "",
        "builtin": "",
    }


def _fields_from_json(art: Path, obj: dict, source: str) -> dict[str, str]:
    kind = _norm_kind(str(obj.get("kind") or "python"))
    if kind == "builtin":
        name = str(obj.get("name") or obj.get("agent") or "").strip()
        if not name or ":" in name or "/" in name or ".." in name:
            _fail(f"{source} builtin harness must set name to a Harbor built-in (not a path)")
        return {
            "kind": "builtin",
            "import_path": "",
            "pythonpath": "",
            "wrapper": "0",
            "source": source,
            "entry": "",
            "builtin": name,
        }
    if kind == "script":
        rel = str(obj.get("entry") or obj.get("path") or "run.sh").strip()
        path = _artefact_file(art, rel)
        if path is None:
            _fail(f"{source} script entry {rel!r} is not a file inside the artefact")
        return {
            "kind": "script",
            "import_path": "",
            "pythonpath": "",
            "wrapper": "0",
            "source": source,
            "entry": rel,
            "builtin": "",
        }
    import_path = str(obj.get("import_path") or obj.get("agent") or "").strip()
    agent_dir = None
    label = source
    cand = _agent_candidate(art)
    if import_path:
        if ":" not in import_path or import_path.startswith("acp:"):
            _fail(f"{source} import_path must be module.path:ClassName, got {import_path!r}")
        if cand is not None:
            agent_dir, _label = cand
            resolve_agent._assert_import_origin(import_path, agent_dir)
            pythonpath = str(agent_dir.resolve().parent)
        else:
            pythonpath = str(art.resolve())
    elif cand is not None:
        return _fields_from_agent_dir(cand[0], cand[1])
    else:
        _fail(f"{source} {kind} harness needs import_path or an agent/ directory")
    wrapper = "1" if kind == "python" else "0"
    if kind == "harbor":
        wrapper = "0"
    return {
        "kind": kind,
        "import_path": import_path,
        "pythonpath": pythonpath,
        "wrapper": wrapper,
        "source": source,
        "entry": "",
        "builtin": "",
    }


def select(job: str, art: Path | None, topic_agent: str) -> dict[str, str]:
    if art is not None:
        named = _load_harness_json(art)
        if named is not None:
            return _fields_from_json(art, named[0], named[1])
        cand = _agent_candidate(art)
        if cand is not None:
            return _fields_from_agent_dir(cand[0], cand[1])
        script = _script_entry(art)
        if script is not None:
            rel, source = script
            if job == "evaluate":
                return {
                    "kind": "script",
                    "import_path": "",
                    "pythonpath": "",
                    "wrapper": "0",
                    "source": source,
                    "entry": rel,
                    "builtin": "",
                }
            # baseline with only a script still runs it rather than wrapping
            # the operator built-in over miner files.
            return {
                "kind": "script",
                "import_path": "",
                "pythonpath": "",
                "wrapper": "0",
                "source": source,
                "entry": rel,
                "builtin": "",
            }
        if job == "evaluate":
            _fail(
                f"evaluate staged an artefact at {art} but found no custom Python "
                "agent, Harbor agent, harness.json, or run.sh; refusing topic "
                "built-in fallback (that was the scoring gap)"
            )
        _fail(
            f"artefact staged at {art} has no harness at agent/, recipe/agent, "
            "harness.json, or run.sh; topic agent fallback is only for baseline "
            "with PROOF_ARTIFACT_DIR unset"
        )
    if job == "evaluate":
        _fail("evaluate requires PROOF_ARTIFACT_DIR")
    topic_agent = topic_agent.strip()
    if not topic_agent:
        _fail("constraints.params.harbor_agent is required for baseline without a miner harness")
    return {
        "kind": "builtin",
        "import_path": "",
        "pythonpath": "",
        "wrapper": "0",
        "source": "topic",
        "entry": "",
        "builtin": topic_agent,
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--job", default=os.environ.get("PROOF_JOB", ""))
    parser.add_argument("--artifact-dir", default=os.environ.get("PROOF_ARTIFACT_DIR", ""))
    parser.add_argument(
        "--topic-agent",
        default=os.environ.get("PROOF_PARAM_HARBOR_AGENT", ""),
    )
    args = parser.parse_args(argv)
    job = args.job.strip()
    if job not in {"baseline", "evaluate"}:
        _fail(f"--job must be baseline or evaluate, got {job!r}")
    art = Path(args.artifact_dir) if args.artifact_dir.strip() else None
    if art is not None and not art.is_dir():
        _fail(f"PROOF_ARTIFACT_DIR is not a directory: {art}")
    fields = select(job, art, args.topic_agent)
    if job == "evaluate" and fields["kind"] == "builtin" and not _load_harness_json(
        art if art is not None else Path("/nonexistent")
    ):
        _fail(
            f"evaluate -a must be a miner harness, not built-in {fields['builtin']}"
        )
    _emit(fields)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
