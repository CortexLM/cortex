#!/usr/bin/env python3
"""proof-experiment-smoke — run ONE in-guest paid job for any custom-family
Proof topic, from its signed document, on a task set you choose.

The smoke is a **topic shape, not a code path**: the same signed topic
document, with `constraints.params.tasks` (or `n_tasks`) narrowed to one
item, drives the same run request, the same guest agent, and the same
operator adaptor a paid evaluate would. Nothing here scores, persists a
row, or touches the control plane's store — it prints what the guest
returned so development can verify the RLM evaluate path in minutes
instead of hours, on any topic, before a topic is re-signed or an image is
re-baked.

Drivers (``--driver``):

  agent   (default) spawn the real ``proof-vm-guest-agent --stdio`` and speak
          its frames (hello → stage_secrets → stage_pack → stage_artifact →
          run). This IS the guest path minus Firecracker / vsock. Needs the
          agent binary, the adaptor tree, the pack tar, and the harness
          tooling (Harbor + Docker) on this host.
  exec    exec the adaptor's ``run`` directly with the guest environment
          contract derived here (a faithful re-statement of the Rust
          ``job_env``; the ``agent`` driver is the authoritative one). No
          Rust binary needed.
  orch    POST the same job to a KVM-host ``proof-vm-orchestrator``: create a
          dedicated experiment VM for it, run, tear down. A real Firecracker
          guest booted from the pinned image (whatever adaptor that image
          carries). Bearer travels from a file, never argv.

Secrets: the miner BYOK value is read from the environment variable named
by ``--byok-env`` (never argv, never printed, redacted from every dump);
owner key files are read by path. ``--dry-run`` prints the derived job
request (redacted) and the adaptor environment and exits 0.

Exit codes: 0 = the guest answered Done (report printed); 1 = Failed / the
run refused; 2 = usage or a precondition this script checks itself.

Examples (see docs/runbooks/proof-experiment-smoke.md):

  # structural proof for ANY topic document, no execution:
  proof-experiment-smoke.py --topic-json topic.json --job evaluate \
      --tasks task-a --artifact-tar recipe.tar --dry-run

  # on the KVM host, real guest agent + adaptor + Harbor, one task:
  OPENROUTER_API_KEY=… proof-experiment-smoke.py --driver agent \
      --topic-id tbench --cp https://gateway.cortex.foundation/challenge/proof \
      --guest-agent ./proof-vm-guest-agent --runner-dir deploy/guest/runners/rlm_fc_in_guest_harbor \
      --pack-tar /var/lib/proof-vm/packs/sha256-<hex>.tar --job evaluate \
      --tasks <one-task> --artifact-tar recipe.tar --byok-env OPENROUTER_API_KEY \
      --set agent_exception_policy=zero --set exec_timeout_s=900
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import io
import json
import os
import re
import shutil
import ssl
import struct
import subprocess
import sys
import tarfile
import tempfile
import time
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any

API_VERSION = 1
RUN_REQUEST_SCHEMA = 1
STAGED_ARTEFACT_SCHEME = "proof-artefact"
MAX_FRAME_BYTES = 256 * 1024 * 1024
REDACTED = "[REDACTED]"
PARAM_RUNNER = "in_guest_benchmark_runner"
PARAM_RUNNER_ALIAS = "baseline_runner"
PARAM_PACK_DIGEST = "experiment_pack_digest"
PARAM_TASKS = "tasks"
PARAM_N_TASKS = "n_tasks"
PARAM_MINER_BYOK = "miner_byok"
PARAM_KEY = re.compile(r"^[a-z0-9][a-z0-9_-]{1,63}$")
ENV_NAME = re.compile(r"^[A-Z][A-Z0-9_]{0,63}$")
HEX64 = re.compile(r"^[0-9a-f]{64}$")
DEFAULT_RUNNER_DIR = Path(__file__).resolve().parents[1] / "guest" / "runners" / "rlm_fc_in_guest_harbor"


class Refuse(Exception):
    """A precondition this script checks itself (exit 2)."""


def log(msg: str) -> None:
    print(f"[smoke] {msg}", file=sys.stderr, flush=True)


def sha256_hex(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def read_bytes(path: str, what: str) -> bytes:
    try:
        return Path(path).read_bytes()
    except OSError as e:
        raise Refuse(f"cannot read {what} {path}: {e}") from e


def check_uncompressed_tar(data: bytes, what: str) -> None:
    """The guest and the KVM host refuse gzip, non-tar, or content-less bytes;
    say so here before anything boots."""
    if not data:
        raise Refuse(f"{what} is empty")
    if data[:2] == b"\x1f\x8b":
        raise Refuse(f"{what} is gzip; Proof artefacts and packs are uncompressed tars")
    try:
        with tarfile.open(fileobj=io.BytesIO(data), mode="r:") as tf:
            if not any(m.isfile() and m.size > 0 for m in tf.getmembers()):
                raise Refuse(f"{what} holds no regular file with content")
    except tarfile.TarError as e:
        raise Refuse(f"{what} is not a tar archive: {e}") from e


def contained_members(tf: tarfile.TarFile, dest: Path) -> list[tarfile.TarInfo]:
    """Every member of a miner / operator tar, or a refusal.

    Regular files and directories only (no links, devices, or FIFOs — the
    guest's own unpack skips those, this dev path refuses them), every path
    made of plain relative segments (no absolute path, no ``.`` / ``..``
    anywhere, no empty segment), and every normalised target strictly under
    ``dest``. Checked on **every** Python, before any byte is written, so the
    result does not depend on whether the stdlib ``data`` filter exists.
    """
    root = dest.resolve()
    members: list[tarfile.TarInfo] = []
    for member in tf.getmembers():
        name = member.name
        if not (member.isfile() or member.isdir()):
            raise Refuse(f"tar member {name!r} is not a regular file or directory; refusing")
        parts = name.split("/")
        if not name or name.startswith("/") or any(p in ("", ".", "..") for p in parts) or "\\" in name:
            raise Refuse(f"tar member {name!r} is not a plain relative path; refusing")
        target = (root / name).resolve()
        if target != root and root not in target.parents:
            raise Refuse(f"tar member {name!r} resolves outside the extraction directory; refusing")
        # The mode is never trusted beyond the executable bit; ownership never.
        member.mode = (0o755 if member.isdir() else 0o644) | (member.mode & 0o111)
        member.uid = member.gid = 0
        member.uname = member.gname = ""
        members.append(member)
    return members


def extract_tar(data: bytes, dest: Path) -> None:
    """Unpack an uncompressed tar under ``dest`` and nowhere else.

    Members are validated by [`contained_members`] first; the stdlib
    ``data`` filter is then applied as well where this Python has it
    (3.12+, or 3.11.4+ / 3.10.12+ / 3.9.17+ / 3.8.17+ with the backport).
    """
    dest.mkdir(parents=True, exist_ok=True)
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:") as tf:
        members = contained_members(tf, dest)
        if hasattr(tarfile, "data_filter"):
            tf.extractall(dest, members=members, filter="data")
        else:  # pragma: no cover - interpreters predating the tar filter
            for member in members:
                tf.extract(member, dest, set_attrs=False)  # noqa: S202 - members contained above
                os.chmod(dest / member.name, member.mode)


# ---------------------------------------------------------------------------
# topic document → run request
# ---------------------------------------------------------------------------


def load_topic(args: argparse.Namespace) -> dict[str, Any]:
    if args.topic_json:
        try:
            doc = json.loads(Path(args.topic_json).read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as e:
            raise Refuse(f"cannot read --topic-json {args.topic_json}: {e}") from e
    else:
        if not args.topic_id or not args.cp:
            raise Refuse("give --topic-json FILE, or --topic-id ID with --cp URL")
        url = f"{args.cp.rstrip('/')}/v1/proof/topics/{args.topic_id}"
        log(f"GET {url}")
        try:
            with urllib.request.urlopen(url, timeout=30) as resp:  # noqa: S310 - operator URL
                doc = json.loads(resp.read().decode("utf-8"))
        except (urllib.error.URLError, json.JSONDecodeError, OSError) as e:
            raise Refuse(f"cannot fetch the topic document: {e}") from e
    if isinstance(doc, dict) and isinstance(doc.get("topic"), dict):
        doc = doc["topic"]
    if not isinstance(doc, dict) or not isinstance(doc.get("id"), str):
        raise Refuse("the topic document has no `id`")
    metric = doc.get("metric") or {}
    if metric.get("family") != "custom":
        raise Refuse(
            f"topic {doc['id']} is metric.family={metric.get('family')!r}; the in-guest "
            "smoke is for custom-family topics only (nll / throughput are the Lium harvest)"
        )
    return doc


def parse_set(values: list[str]) -> dict[str, str]:
    out: dict[str, str] = {}
    for item in values or []:
        key, sep, value = item.partition("=")
        key = key.strip()
        if not sep or not PARAM_KEY.match(key):
            raise Refuse(f"--set needs <slug>=<value>, got {item!r}")
        if not value.strip() or len(value) > 256 or any(ch in value for ch in "\r\n"):
            raise Refuse(f"--set {key}: value must be one printable line of at most 256 chars")
        out[key] = value
    return out


def build_request(
    doc: dict[str, Any],
    *,
    job: str,
    tasks: str | None,
    n_tasks: int | None,
    overrides: dict[str, str],
    artifact_digest: str | None,
    artifact_uri: str | None,
    claim: str,
    deadline_s: int | None,
    miner_env: dict[str, str],
    seed: int | None,
) -> dict[str, Any]:
    """The `CustomRunRequest` a control plane would build for this topic,
    with the smoke's task selection folded into the signed params."""
    constraints = json.loads(json.dumps(doc.get("constraints") or {}))
    params: dict[str, str] = dict(constraints.get("params") or {})
    runner = params.get(PARAM_RUNNER) or params.get(PARAM_RUNNER_ALIAS)
    if not runner:
        raise Refuse(
            f"topic {doc['id']} selects no in-guest runner (constraints.params.{PARAM_RUNNER} / "
            f"{PARAM_RUNNER_ALIAS}); the experiment-VM smoke does not apply to it"
        )
    if tasks:
        params[PARAM_TASKS] = tasks
    if n_tasks is not None:
        params[PARAM_N_TASKS] = str(n_tasks)
    params.update(overrides)
    if len(params) > 32:
        raise Refuse(f"constraints.params would carry {len(params)} keys; the shape cap is 32")
    constraints["params"] = params
    metric = doc.get("metric") or {}
    baseline = doc.get("baseline") or {}
    eval_executor = doc.get("eval_executor") or {}
    inference = doc.get("inference") or {}
    topic_deadline = eval_executor.get("max_proof_deadline_s")
    if deadline_s is None:
        deadline_s = int(topic_deadline) if isinstance(topic_deadline, int) else 7200
    if job == "evaluate" and not artifact_digest:
        raise Refuse("evaluate needs --artifact-tar (or --artifact-uri with --artifact-digest)")
    submission_digest = sha256_hex(
        f"smoke:{doc['id']}:{job}:{artifact_digest or ''}:{time.time_ns()}".encode()
    )
    return {
        "schema_version": RUN_REQUEST_SCHEMA,
        "topic_id": doc["id"],
        "custom_id": (metric.get("custom_id") or "").strip(),
        "primary": (metric.get("primary") or "").strip(),
        "direction": metric.get("direction") or "max",
        "epsilon_rel": float(metric.get("epsilon_rel") or 0.0),
        "submission_digest": submission_digest,
        "artifact_digest": artifact_digest or "",
        "artifact_uri": artifact_uri,
        "claim": claim,
        "flops_budget": int(doc.get("flops_budget") or 0),
        "declared_flops": 0,
        "constraints": constraints,
        **({"miner_env": miner_env} if miner_env else {}),
        # The smoke ticks no checklist and mints no spend token; these echo
        # into the report and are never compared against a store.
        "rules_version": 1,
        "rules_digest": "0" * 64,
        "seed": int(seed if seed is not None else baseline.get("seed") or 0),
        "judge": {
            "offer_id": "smoke-no-judge",
            "model_ref": str(inference.get("model") or ""),
            "config_commitment": "0" * 64,
        },
        "sandbox": {
            "firecracker_required": bool(constraints.get("firecracker_required", False)),
            "deadline_s": int(deadline_s),
        },
        "executor_commitment": None,
    }


def build_job(job: str, request: dict[str, Any]) -> dict[str, Any]:
    if job == "baseline":
        return {"job": "baseline", "request": request}
    return {
        "job": "evaluate",
        "request": request,
        "checklist_digest": "0" * 64,
        "rules_version": request["rules_version"],
    }


def redact(obj: Any, secrets: list[str]) -> Any:
    text = json.dumps(obj)
    for s in secrets:
        if s:
            text = text.replace(json.dumps(s)[1:-1], REDACTED)
    return json.loads(text)


def redact_request_for_print(job_doc: dict[str, Any], secrets: list[str]) -> dict[str, Any]:
    doc = json.loads(json.dumps(job_doc))
    req = doc.get("request", {})
    if req.get("miner_env"):
        req["miner_env"] = {k: REDACTED for k in req["miner_env"]}
    if req.get("artifact_tar"):
        req["artifact_tar"] = f"<{len(req['artifact_tar'])} b64 chars>"
    return redact(doc, secrets)


# ---------------------------------------------------------------------------
# guest env contract (exec driver / --dry-run)
# ---------------------------------------------------------------------------


def param_env_name(key: str) -> str:
    return "PROOF_PARAM_" + "".join("_" if c == "-" else c.upper() for c in key)


def adaptor_env(
    request: dict[str, Any],
    *,
    job: str,
    runner: str,
    work: Path,
    output: Path,
    pack_dir: Path | None,
    pack_digest: str | None,
    artifact_dir: Path | None,
    secrets_dir: Path,
    secret_files: list[str],
    miner_env_dir: Path | None,
    miner_names: list[str],
) -> dict[str, str]:
    """The `PROOF_*` contract of `crates/proof-vm-guest/src/runner.rs::job_env`.

    Kept in the same order and with the same names so a diff against the
    Rust source is mechanical. The `agent` driver runs the Rust code itself.
    """
    params = request["constraints"].get("params") or {}
    seen: dict[str, str] = {}
    for k in params:
        name = param_env_name(k)
        if name in seen:
            raise Refuse(f"constraints.params {seen[name]!r} and {k!r} both map to {name}")
        seen[name] = k
    env = {
        "PROOF_RUNNER_ID": runner,
        "PROOF_JOB": job,
        "PROOF_TOPIC_ID": request["topic_id"],
        "PROOF_CUSTOM_ID": request["custom_id"],
        "PROOF_PRIMARY_METRIC": request["primary"],
        "PROOF_METRIC_DIRECTION": request["direction"],
        "PROOF_SUBMISSION_DIGEST": request["submission_digest"],
        "PROOF_ARTIFACT_DIGEST": request["artifact_digest"],
        "PROOF_SEED": str(request["seed"]),
        "PROOF_DEADLINE_S": str(request["sandbox"]["deadline_s"]),
        "PROOF_DECLARED_FLOPS": str(request["declared_flops"]),
        "PROOF_FLOPS_BUDGET": str(request["flops_budget"]),
        "PROOF_OUTPUT_DIR": str(output),
        "PROOF_WORK_DIR": str(work),
        "PROOF_SECRETS_DIR": str(secrets_dir),
        "PROOF_SECRET_FILES": ",".join(secret_files),
    }
    if pack_dir is not None:
        env["PROOF_PACK_DIR"] = str(pack_dir)
        env["PROOF_PACK_DIGEST"] = pack_digest or ""
    if artifact_dir is not None:
        env["PROOF_ARTIFACT_DIR"] = str(artifact_dir)
    if request["constraints"].get("model_pin"):
        env["PROOF_MODEL_PIN"] = request["constraints"]["model_pin"]
    if request["constraints"].get("task_slice"):
        env["PROOF_TASK_SLICE"] = request["constraints"]["task_slice"]
    for k, v in params.items():
        env[param_env_name(k)] = v
    env["PROOF_CLAIM_FILE"] = str(work / "claim.txt")
    if miner_env_dir is not None:
        env["PROOF_MINER_ENV_DIR"] = str(miner_env_dir)
        env["PROOF_MINER_ENV_NAMES"] = ",".join(miner_names)
    base = {
        "PATH": "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
        "LANG": "C.UTF-8",
        "HOME": os.environ.get("HOME", "/root"),
        "XDG_RUNTIME_DIR": os.environ.get("XDG_RUNTIME_DIR", f"/run/user/{os.getuid()}"),
    }
    return {**base, **env}


# ---------------------------------------------------------------------------
# drivers
# ---------------------------------------------------------------------------


def driver_exec(
    args: argparse.Namespace,
    job: str,
    request: dict[str, Any],
    runner: str,
    pack_bytes: bytes | None,
    pack_digest: str | None,
    artifact_bytes: bytes | None,
    miner_env: dict[str, str],
    secrets: list[str],
    root: Path,
) -> int:
    runner_dir = Path(args.runner_dir).resolve()
    entry = runner_dir / "run"
    if not os.access(entry, os.X_OK):
        raise Refuse(f"{entry} is not an executable adaptor entrypoint")
    if runner_dir.name != runner:
        log(f"warning: adaptor dir is named {runner_dir.name!r}, the topic selects runner {runner!r}")
    work = root / "work" / f"0001-{job}"
    output = work / "output"
    output.mkdir(parents=True)
    (work / "claim.txt").write_text(request["claim"], encoding="utf-8")
    pack_dir: Path | None = None
    if args.pack_dir:
        pack_dir = Path(args.pack_dir).resolve()
    elif pack_bytes is not None:
        pack_dir = root / "packs" / (pack_digest or "pack").replace("sha256:", "")
        pack_dir.mkdir(parents=True)
        extract_tar(pack_bytes, pack_dir)
    else:
        raise Refuse("exec driver needs --pack-dir DIR or --pack-tar FILE")
    artifact_dir: Path | None = None
    if artifact_bytes is not None:
        artifact_dir = work / "artifact"
        artifact_dir.mkdir(parents=True)
        extract_tar(artifact_bytes, artifact_dir)
        (work / "artifact.tar").write_bytes(artifact_bytes)
    secrets_dir = root / "secrets"
    secrets_dir.mkdir(mode=0o700, exist_ok=True)
    secret_files: list[str] = []
    for name, path in (args.owner_key_file or []):
        (secrets_dir / name).write_bytes(read_bytes(path, "owner key file"))
        os.chmod(secrets_dir / name, 0o600)
        secret_files.append(name)
    miner_dir: Path | None = None
    if miner_env:
        miner_dir = secrets_dir / "miner"
        miner_dir.mkdir(mode=0o700, exist_ok=True)
        for name, value in miner_env.items():
            (miner_dir / name).write_text(value, encoding="utf-8")
            os.chmod(miner_dir / name, 0o600)
    env = adaptor_env(
        request,
        job=job,
        runner=runner,
        work=work,
        output=output,
        pack_dir=pack_dir,
        pack_digest=pack_digest,
        artifact_dir=artifact_dir,
        secrets_dir=secrets_dir,
        secret_files=sorted(secret_files),
        miner_env_dir=miner_dir,
        miner_names=sorted(miner_env),
    )
    env.update(miner_env)
    if args.path_prepend:
        env["PATH"] = f"{args.path_prepend}:{env['PATH']}"
    if args.dry_run:
        print(json.dumps({"driver": "exec", "entrypoint": str(entry), "env": redact(env, secrets)}, indent=2))
        return 0
    log(f"exec {entry} (job={job}, work={work})")
    deadline = int(request["sandbox"]["deadline_s"])
    started = time.monotonic()
    try:
        proc = subprocess.run(  # noqa: S603 - operator adaptor, operator host
            [str(entry)],
            cwd=work,
            env=env,
            stdin=subprocess.DEVNULL,
            capture_output=True,
            timeout=deadline,
            check=False,
        )
    except subprocess.TimeoutExpired:
        log(f"adaptor cut at the deadline of {deadline}s; no report is evidence")
        return 1
    wall = time.monotonic() - started
    tail = redact_text(proc.stderr.decode("utf-8", errors="replace")[-8000:], secrets)
    report_path = output / "report.json"
    if proc.returncode != 0 or not report_path.is_file():
        log(f"adaptor exit {proc.returncode} after {wall:.0f}s; no report.json → this would be Failed (503, no row)")
        sys.stderr.write(tail + "\n")
        return 1
    report = json.loads(report_path.read_text(encoding="utf-8"))
    if not isinstance(report.get("primary_value"), (int, float)):
        log("report.json has no finite primary_value → Failed")
        return 1
    print_outcome({"driver": "exec", "wall_s": round(wall), "exit": proc.returncode, "report": report}, secrets, args.out)
    return 0


def redact_text(text: str, secrets: list[str]) -> str:
    for s in secrets:
        if s:
            text = text.replace(s, REDACTED)
    return text


def write_frame(stream: Any, value: dict[str, Any]) -> None:
    body = json.dumps(value).encode("utf-8")
    if len(body) > MAX_FRAME_BYTES:
        raise Refuse(f"frame of {len(body)} bytes exceeds the {MAX_FRAME_BYTES} cap")
    stream.write(struct.pack(">I", len(body)) + body)
    stream.flush()


def read_frame(stream: Any) -> dict[str, Any]:
    head = stream.read(4)
    if len(head) < 4:
        raise Refuse("the guest agent closed the stream without answering")
    (n,) = struct.unpack(">I", head)
    if n > MAX_FRAME_BYTES:
        raise Refuse(f"frame of {n} bytes exceeds the cap")
    body = b""
    while len(body) < n:
        chunk = stream.read(n - len(body))
        if not chunk:
            raise Refuse("the guest agent closed the stream mid-frame")
        body += chunk
    return json.loads(body.decode("utf-8"))


def staged_file(name: str, data: bytes) -> dict[str, str]:
    return {"name": name, "bytes_b64": base64.b64encode(data).decode("ascii")}


def driver_agent(
    args: argparse.Namespace,
    job: str,
    request: dict[str, Any],
    runner: str,
    pack_bytes: bytes | None,
    pack_digest: str | None,
    artifact_bytes: bytes | None,
    secrets: list[str],
    root: Path,
) -> int:
    if not args.guest_agent:
        raise Refuse("agent driver needs --guest-agent PATH (the proof-vm-guest-agent binary)")
    agent = Path(args.guest_agent).resolve()
    if not os.access(agent, os.X_OK):
        raise Refuse(f"{agent} is not executable")
    if pack_bytes is None or not pack_digest:
        raise Refuse("agent driver needs --pack-tar FILE (the guest verifies and unpacks it)")
    runner_dir = Path(args.runner_dir).resolve()
    if not os.access(runner_dir / "run", os.X_OK):
        raise Refuse(f"{runner_dir}/run is not an executable adaptor entrypoint")
    runners = root / "runners"
    runners.mkdir()
    if args.path_prepend:
        # The guest agent never inherits PATH (contract). To point a dev run
        # at a harness venv, install a copy of the adaptor whose `run` shim
        # prepends the given dirs, then execs the original entrypoint.
        installed = runners / runner
        shutil.copytree(runner_dir, installed, symlinks=True)
        (installed / "run").rename(installed / ".run.orig")
        (installed / "run").write_text(
            "#!/bin/sh\n"
            f"export PATH={json.dumps(args.path_prepend)}:$PATH\n"
            'exec "$(dirname "$0")/.run.orig" "$@"\n',
            encoding="utf-8",
        )
        os.chmod(installed / "run", 0o755)
        log(f"adaptor copied to {installed} with PATH prepended by {args.path_prepend} (dev shim)")
    else:
        (runners / runner).symlink_to(runner_dir)
    for sub in ("secrets", "packs", "work"):
        (root / sub).mkdir(mode=0o700 if sub == "secrets" else 0o755)
    cmd = [
        str(agent),
        "--stdio",
        "--runners-dir",
        str(runners),
        "--secrets-dir",
        str(root / "secrets"),
        "--pack-root",
        str(root / "packs"),
        "--work-root",
        str(root / "work"),
    ]
    if args.allow_plain_http:
        cmd.append("--allow-plain-http")
    frames: list[dict[str, Any]] = [
        {"type": "hello", "api_version": API_VERSION, "topic_id": request["topic_id"], "vm_id": f"{request['topic_id']}-smoke"},
    ]
    owner_files = [staged_file(name, read_bytes(path, "owner key file")) for name, path in (args.owner_key_file or [])]
    if owner_files:
        frames.append({"type": "stage_secrets", "files": owner_files})
    frames.append({"type": "stage_pack", "digest": pack_digest, "pack_tar": staged_file("pack.tar", pack_bytes)})
    if artifact_bytes is not None:
        frames.append(
            {
                "type": "stage_artifact",
                "digest": request["artifact_digest"],
                "artifact_tar": staged_file("artifact.tar", artifact_bytes),
            }
        )
    frames.append({"type": "run", "job": build_job(job, request)})
    if args.dry_run:
        print(
            json.dumps(
                {
                    "driver": "agent",
                    "command": cmd,
                    "frames": [f["type"] for f in frames],
                    "job": redact_request_for_print(frames[-1]["job"], secrets),
                },
                indent=2,
            )
        )
        return 0
    log(f"spawn {' '.join(cmd)}")
    env = {k: v for k, v in os.environ.items() if k in ("PATH", "HOME", "LANG", "RUST_LOG", "XDG_RUNTIME_DIR", "TMPDIR")}
    env.setdefault("RUST_LOG", "info")
    proc = subprocess.Popen(  # noqa: S603 - our own binary
        cmd, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=sys.stderr, env=env
    )
    assert proc.stdin is not None and proc.stdout is not None
    started = time.monotonic()
    answer: dict[str, Any] | None = None
    try:
        for frame in frames:
            write_frame(proc.stdin, frame)
            answer = read_frame(proc.stdout)
            kind = answer.get("type")
            if kind == "failed":
                log(f"guest answered Failed to {frame['type']}: {redact_text(str(answer.get('error')), secrets)}")
                return 1
            log(f"{frame['type']} → {kind}")
    finally:
        try:
            proc.stdin.close()
        except OSError:
            pass
        try:
            proc.wait(timeout=30)
        except subprocess.TimeoutExpired:
            proc.kill()
    wall = time.monotonic() - started
    if not answer or answer.get("type") != "done":
        log(f"unexpected final answer: {answer}")
        return 1
    output = answer.get("output") or {}
    report = output.get("body")
    if output.get("output") == "evaluated" and isinstance(report, dict):
        report = report.get("report")
    print_outcome({"driver": "agent", "wall_s": round(wall), "output": output.get("output"), "report": report}, secrets, args.out)
    if args.keep_work:
        log(f"guest work root kept at {root / 'work'}")
    return 0


def orch_http(
    method: str,
    url: str,
    body: dict[str, Any] | None,
    token: str,
    ca: str | None,
    timeout: int,
) -> tuple[int, dict[str, Any]]:
    data = json.dumps(body).encode("utf-8") if body is not None else None
    req = urllib.request.Request(url, data=data, method=method)
    req.add_header("Authorization", f"Bearer {token}")
    if data is not None:
        req.add_header("Content-Type", "application/json")
    ctx = ssl.create_default_context(cafile=ca) if ca else ssl.create_default_context()
    try:
        with urllib.request.urlopen(req, timeout=timeout, context=ctx) as resp:  # noqa: S310
            return resp.status, json.loads(resp.read().decode("utf-8") or "{}")
    except urllib.error.HTTPError as e:
        try:
            return e.code, json.loads(e.read().decode("utf-8") or "{}")
        except (json.JSONDecodeError, OSError):
            return e.code, {"error": str(e)}


def driver_orch(
    args: argparse.Namespace,
    job: str,
    request: dict[str, Any],
    runner: str,
    pack_digest: str | None,
    artifact_bytes: bytes | None,
    secrets: list[str],
) -> int:
    if not (args.orch_url and args.orch_token_file and args.image_digest):
        raise Refuse("orch driver needs --orch-url, --orch-token-file, and --image-digest sha256:…")
    if not re.match(r"^sha256:[0-9a-f]{64}$", args.image_digest):
        raise Refuse("--image-digest must be sha256:<64 hex> (read PROOF_RLM_VM_IMAGE_DIGEST; never invent one)")
    if "cortex.foundation" in args.orch_url:
        raise Refuse("refusing a production host for a smoke run")
    if not args.ack_image_adaptor:
        raise Refuse(
            "the orch driver boots the PINNED guest image, and this smoke only narrows the run to one "
            "task if the adaptor baked into that image reads constraints.params.tasks. An image baked "
            "before this adaptor (e.g. the e79be pin kept until RE-LOCK) ignores the knob, runs the "
            "topic's whole selection, and holds an experiment slot for hours. Pass "
            "--ack-image-adaptor only when --image-digest names an image re-baked with this adaptor; "
            "until then use --driver agent / exec on the host."
        )
    token = read_bytes(args.orch_token_file, "bearer file").decode("utf-8").strip()
    if not token:
        raise Refuse(f"bearer file {args.orch_token_file} is empty")
    secrets = [*secrets, token]
    pack_digest = pack_digest or request["constraints"]["params"].get(PARAM_PACK_DIGEST)
    if not pack_digest:
        raise Refuse("no experiment_pack_digest on the topic and no --pack-tar to hash")
    base = args.orch_url.rstrip("/")
    # Never take a slot from a live paid evaluate: one experiment VM already
    # running means a miner (or the operator) is in flight on this host.
    code, health = orch_http("GET", f"{base}/v1/health", None, token, args.orch_ca, 30)
    if code != 200:
        raise Refuse(f"GET /v1/health → HTTP {code}: {redact(health, secrets)}")
    if not health.get("ready", False):
        raise Refuse(f"the KVM host is not ready: {health.get('reason') or 'no reason given'}")
    live = int(health.get("experiment_vms") or 0)
    cap = int(health.get("max_experiment_vms") or 0)
    if live > 0 and not args.allow_shared_capacity:
        raise Refuse(
            f"{live} experiment vm(s) already running on this host (max {cap}); a smoke must not compete "
            "with a live paid evaluate for its slot. Wait for the slot to free, or pass "
            "--allow-shared-capacity when the operator has confirmed the running vm is not a paid run."
        )
    if cap and live >= cap:
        raise Refuse(f"the host runs {live} of at most {cap} experiment vms; no capacity for a smoke")
    log(f"KVM host ready; experiment_vms={live}/{cap}")
    spec = {
        "spec": {
            "topic_id": request["topic_id"],
            "template": {"image_digest": args.image_digest, "vcpus": args.vcpus, "mem_mib": args.mem_mib},
            "sandbox": request["sandbox"],
            "retain": "destroy",
            "experiment": {"runner": runner, "pack": {"digest": pack_digest}, "disk_mib": args.disk_mib},
        }
    }
    job_doc = build_job(job, request)
    if artifact_bytes is not None:
        job_doc["request"]["artifact_tar"] = base64.b64encode(artifact_bytes).decode("ascii")
    run_body = {"topic_id": request["topic_id"], "job": job_doc}
    if args.dry_run:
        print(
            json.dumps(
                {
                    "driver": "orch",
                    "create": {"POST": f"{base}/v1/vms", "body": spec},
                    "run": {"POST": f"{base}/v1/vms/<vm_id>/jobs", "body": redact_request_for_print(run_body, secrets)},
                    "teardown": {"DELETE": f"{base}/v1/vms/<vm_id>", "body": {"topic_id": request["topic_id"], "policy": "destroy"}},
                },
                indent=2,
            )
        )
        return 0
    log(f"POST {base}/v1/vms (experiment vm: runner={runner} pack={pack_digest[:19]}… {args.vcpus}vCPU/{args.mem_mib}MiB/{args.disk_mib}MiB)")
    code, created = orch_http("POST", f"{base}/v1/vms", spec, token, args.orch_ca, 660)
    if code != 201:
        log(f"create → HTTP {code}: {redact(created, secrets)}")
        return 1
    vm_id = created.get("handle", {}).get("vm_id")
    log(f"experiment vm {vm_id} booted; POST /v1/vms/{vm_id}/jobs (job={job}, deadline {request['sandbox']['deadline_s']}s)")
    started = time.monotonic()
    rc = 1
    try:
        code, ran = orch_http(
            "POST", f"{base}/v1/vms/{vm_id}/jobs", run_body, token, args.orch_ca, int(request["sandbox"]["deadline_s"]) + 120
        )
        wall = time.monotonic() - started
        if code != 200:
            log(f"job → HTTP {code} after {wall:.0f}s: {redact(ran, secrets)}")
        else:
            output = ran.get("output") or {}
            report = output.get("body")
            if output.get("output") == "evaluated" and isinstance(report, dict):
                report = report.get("report")
            print_outcome(
                {"driver": "orch", "vm_id": vm_id, "wall_s": round(wall), "output": output.get("output"), "sister": ran.get("sister"), "report": report},
                secrets,
                args.out,
            )
            rc = 0
    finally:
        policy = "destroy" if (rc == 0 or not args.retain_on_fail) else "retain"
        code, down = orch_http(
            "DELETE", f"{base}/v1/vms/{vm_id}", {"topic_id": request["topic_id"], "policy": policy}, token, args.orch_ca, 660
        )
        log(f"teardown ({policy}) → HTTP {code} confirmed={down.get('confirmed')} state={down.get('state')}")
        if code != 200 or not down.get("confirmed"):
            log(f"reconcile {vm_id} on the KVM host: the agent did not confirm the {policy}")
            rc = 1
    return rc


def adaptor_tree_sha256(runner_dir: Path) -> str:
    """One digest over the adaptor tree that ran (path + bytes of every
    regular file, sorted; `tests/` and caches excluded) — what an operator
    pastes beside the outcome so the evidence names the exact adaptor."""
    h = hashlib.sha256()
    for path in sorted(p for p in runner_dir.rglob("*") if p.is_file()):
        rel = path.relative_to(runner_dir).as_posix()
        if rel.startswith("tests/") or "__pycache__" in rel or rel == ".run.orig":
            continue
        h.update(rel.encode("utf-8") + b"\0")
        h.update(path.read_bytes())
        h.update(b"\0")
    return h.hexdigest()


class Evidence:
    """Identities of one smoke run, printed for the PR / issue paste."""

    def __init__(self) -> None:
        self.fields: dict[str, Any] = {}

    def set(self, **kv: Any) -> None:
        self.fields.update({k: v for k, v in kv.items() if v is not None})


EVIDENCE = Evidence()


def print_outcome(outcome: dict[str, Any], secrets: list[str], out: str | None) -> None:
    redacted = redact(outcome, secrets)
    report = redacted.get("report") or {}
    ev = report.get("evidence") or {}
    summary = {
        "primary_value": report.get("primary_value"),
        "claim_holds": report.get("claim_holds"),
        "sandboxed": report.get("sandboxed"),
        "n_scored": ev.get("n_scored"),
        "n_measured": ev.get("n_measured"),
        "n_agent_exceptions": ev.get("n_agent_exceptions"),
        "agent_exception_policy": ev.get("agent_exception_policy"),
        "trials": ev.get("trials"),
        "runner": ev.get("runner"),
        "harness_kind": ev.get("harness_kind"),
        "wall_s": redacted.get("wall_s"),
    }
    log("Done — the guest returned a report (this smoke persists nothing):")
    print(json.dumps(summary, indent=2))
    dumped = json.dumps(redacted, indent=2) + "\n"
    if out:
        Path(out).write_text(dumped, encoding="utf-8")
        log(f"full outcome written to {out}")
    EVIDENCE.set(
        driver=redacted.get("driver"),
        primary_value=report.get("primary_value"),
        n_scored=ev.get("n_scored"),
        n_measured=ev.get("n_measured"),
        n_agent_exceptions=ev.get("n_agent_exceptions"),
        trials=[t.get("name") for t in (ev.get("trials") or []) if isinstance(t, dict)],
        outcome_sha256=sha256_hex(dumped.encode("utf-8")),
    )
    log("evidence (paste into the PR / issue; no secret, no host path):")
    print(json.dumps({"smoke_evidence": EVIDENCE.fields}, indent=2, sort_keys=True))


# ---------------------------------------------------------------------------
# main
# ---------------------------------------------------------------------------


def parse_owner_key(value: str) -> tuple[str, str]:
    name, sep, path = value.partition("=")
    if not sep or not name or not path:
        raise argparse.ArgumentTypeError("--owner-key-file needs NAME=PATH")
    return name, path


def main(argv: list[str] | None = None) -> int:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    src = p.add_argument_group("topic")
    src.add_argument("--topic-json", help="signed topic document (JSON file)")
    src.add_argument("--topic-id", help="topic id to fetch from --cp")
    src.add_argument("--cp", help="Proof origin, e.g. https://gateway.cortex.foundation/challenge/proof (GET only)")
    sel = p.add_argument_group("job")
    sel.add_argument("--job", choices=("baseline", "evaluate"), default="evaluate")
    sel.add_argument("--tasks", help="constraints.params.tasks override: exactly these task names (one = smoke)")
    sel.add_argument("--n-tasks", type=int, help="constraints.params.n_tasks override (1 = smoke)")
    sel.add_argument("--set", action="append", default=[], metavar="KEY=VALUE", help="override / add a constraints.params entry (repeatable)")
    sel.add_argument("--claim", default="single-task smoke; not a submission", help="claim text")
    sel.add_argument("--deadline-s", type=int, help="sandbox.deadline_s override (default: the topic's max_proof_deadline_s)")
    sel.add_argument("--seed", type=int, help="seed override (default: the topic baseline seed)")
    inputs = p.add_argument_group("inputs")
    inputs.add_argument("--pack-tar", help="the experiment pack tar (uncompressed); its sha256 becomes experiment_pack_digest")
    inputs.add_argument("--pack-dir", help="exec driver: an already unpacked pack directory")
    inputs.add_argument("--artifact-tar", help="miner artefact tar (uncompressed); evaluate; staged as proof-artefact://")
    inputs.add_argument("--artifact-uri", help="compat: a miner-hosted locator (needs --artifact-digest)")
    inputs.add_argument("--artifact-digest", help="64-hex sha256 of the bytes --artifact-uri serves")
    inputs.add_argument("--byok-env", action="append", default=[], metavar="NAME", help="miner BYOK: read NAME from this process env into miner_env (never argv)")
    inputs.add_argument("--owner-key-file", action="append", type=parse_owner_key, default=[], metavar="NAME=PATH", help="owner key staged into the guest secrets dir (baseline)")
    drv = p.add_argument_group("driver")
    drv.add_argument("--driver", choices=("agent", "exec", "orch"), default="agent")
    drv.add_argument("--guest-agent", help="agent driver: proof-vm-guest-agent binary")
    drv.add_argument("--runner-dir", default=str(DEFAULT_RUNNER_DIR), help="agent/exec: the adaptor tree (default: the in-repo Harbor reference adaptor)")
    drv.add_argument("--allow-plain-http", action="store_true", help="agent driver: accept http:// artefact locators")
    drv.add_argument("--path-prepend", metavar="DIR[:DIR]", help="dev: dirs the adaptor's PATH starts with (harness venv, fake docker); the guest contract itself never inherits PATH")
    drv.add_argument("--orch-url", help="orch driver: https://<kvm-host>:8200")
    drv.add_argument("--orch-token-file", help="orch driver: bearer FILE (same bytes as the host's token)")
    drv.add_argument("--orch-ca", help="orch driver: CA bundle for the agent's TLS")
    drv.add_argument("--image-digest", help="orch driver: sha256: digest of the guest image (PROOF_RLM_VM_IMAGE_DIGEST)")
    drv.add_argument("--vcpus", type=int, default=16)
    drv.add_argument("--mem-mib", type=int, default=32768)
    drv.add_argument("--disk-mib", type=int, default=32768)
    drv.add_argument("--retain-on-fail", action="store_true", help="orch driver: retain the VM when the job failed")
    drv.add_argument(
        "--ack-image-adaptor",
        action="store_true",
        help="orch driver: I confirm --image-digest names a guest image re-baked with an adaptor that reads "
        "constraints.params.tasks (an older pin runs the whole selection and holds a slot for hours)",
    )
    drv.add_argument(
        "--allow-shared-capacity",
        action="store_true",
        help="orch driver: proceed although an experiment vm is already running on the host (operator "
        "confirmed it is not a live paid evaluate); default refuses so a smoke never takes a live slot",
    )
    outg = p.add_argument_group("output")
    outg.add_argument("--dry-run", action="store_true", help="print the derived job / env (redacted) and exit")
    outg.add_argument("--out", help="write the full (redacted) outcome JSON here")
    outg.add_argument(
        "--work-root",
        help="keep guest/adaptor work under this dir instead of a temp dir. On a KVM host use a directory "
        "under /var/lib/proof/<your-wd>/ — the only scratch the operator policy allows there",
    )
    outg.add_argument("--keep-work", action="store_true", help="do not delete the temp work root")
    args = p.parse_args(argv)

    try:
        doc = load_topic(args)
        overrides = parse_set(args.set)
        params = (doc.get("constraints") or {}).get("params") or {}
        runner = (params.get(PARAM_RUNNER) or params.get(PARAM_RUNNER_ALIAS) or "").strip()
        # Miner BYOK: names from the topic (or --byok-env), values from this env only.
        miner_env: dict[str, str] = {}
        secrets: list[str] = []
        byok_names = [n.strip() for n in (params.get(PARAM_MINER_BYOK) or "").split(",") if n.strip()]
        wanted = list(dict.fromkeys([*byok_names, *args.byok_env])) if args.job == "evaluate" else list(args.byok_env)
        for name in wanted:
            if not ENV_NAME.match(name):
                raise Refuse(f"{name!r} is not a miner env variable name")
            value = os.environ.get(name, "")
            if not value:
                if name in byok_names and args.job == "evaluate":
                    if args.dry_run:
                        log(f"dry-run: miner BYOK {name} is not exported; the real run refuses without it")
                        miner_env[name] = "<unset>"
                        continue
                    raise Refuse(f"the topic requires miner BYOK {name}; export it in this shell (never on argv)")
                continue
            miner_env[name] = value
            secrets.append(value)
        pack_bytes = pack_digest = None
        if args.pack_tar:
            pack_bytes = read_bytes(args.pack_tar, "pack tar")
            check_uncompressed_tar(pack_bytes, "pack tar")
            pack_digest = f"sha256:{sha256_hex(pack_bytes)}"
            pinned = (params.get(PARAM_PACK_DIGEST) or "").strip().lower()
            if pinned and pinned != pack_digest:
                log(f"warning: --pack-tar hashes to {pack_digest}, the topic pins {pinned}; the job carries the tar's digest")
            overrides.setdefault(PARAM_PACK_DIGEST, pack_digest)
        artifact_bytes = artifact_digest = artifact_uri = None
        if args.artifact_tar:
            artifact_bytes = read_bytes(args.artifact_tar, "artifact tar")
            check_uncompressed_tar(artifact_bytes, "artifact tar")
            if len(artifact_bytes) > 5 * 1024 * 1024:
                raise Refuse("artifact tar exceeds the 5 MiB staged-inject cap")
            artifact_digest = sha256_hex(artifact_bytes)
            artifact_uri = f"{STAGED_ARTEFACT_SCHEME}://{artifact_digest}"
        elif args.artifact_uri:
            if not args.artifact_digest or not HEX64.match(args.artifact_digest.lower()):
                raise Refuse("--artifact-uri needs --artifact-digest (64 hex)")
            artifact_digest = args.artifact_digest.lower()
            artifact_uri = args.artifact_uri
        request = build_request(
            doc,
            job=args.job,
            tasks=args.tasks,
            n_tasks=args.n_tasks,
            overrides=overrides,
            artifact_digest=artifact_digest,
            artifact_uri=artifact_uri,
            claim=args.claim,
            deadline_s=args.deadline_s,
            miner_env=miner_env,
            seed=args.seed,
        )
        selected = request["constraints"]["params"].get(PARAM_TASKS) or request["constraints"]["params"].get(PARAM_N_TASKS)
        log(
            f"topic={doc['id']} runner={runner} job={args.job} "
            f"tasks={request['constraints']['params'].get(PARAM_TASKS, '<pack default>')} "
            f"n_tasks={request['constraints']['params'].get(PARAM_N_TASKS, '-')} "
            f"deadline_s={request['sandbox']['deadline_s']} byok={sorted(miner_env) or '-'}"
        )
        if not selected:
            log("warning: no --tasks / --n-tasks; this runs the topic's full selection, not a smoke")
        runner_dir = Path(args.runner_dir).resolve()
        EVIDENCE.set(
            topic_id=doc["id"],
            job=args.job,
            runner=runner,
            tasks=request["constraints"]["params"].get(PARAM_TASKS),
            n_tasks=request["constraints"]["params"].get(PARAM_N_TASKS),
            pack_digest=request["constraints"]["params"].get(PARAM_PACK_DIGEST),
            artifact_digest=artifact_digest,
            params_overridden=sorted(overrides),
            adaptor_tree_sha256=adaptor_tree_sha256(runner_dir) if runner_dir.is_dir() else None,
        )
        if args.driver == "orch":
            return driver_orch(args, args.job, request, runner, pack_digest, artifact_bytes, secrets)
        root = Path(args.work_root).resolve() if args.work_root else Path(tempfile.mkdtemp(prefix="proof-smoke-"))
        root.mkdir(parents=True, exist_ok=True)
        try:
            if args.driver == "exec":
                return driver_exec(args, args.job, request, runner, pack_bytes, pack_digest, artifact_bytes, miner_env, secrets, root)
            return driver_agent(args, args.job, request, runner, pack_bytes, pack_digest, artifact_bytes, secrets, root)
        finally:
            if not (args.keep_work or args.work_root or args.dry_run):
                shutil.rmtree(root, ignore_errors=True)
            elif not args.dry_run:
                log(f"work root kept at {root}")
    except Refuse as e:
        log(f"refused: {e}")
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
