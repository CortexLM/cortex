#!/usr/bin/env python3
"""Materialize byte-exact /data/hypertrain-{role} roots from verified local custody.

Offline, read-only on custody. Native order (orchestrate continue): rescue -> artifact
manifest check -> record_qualification, so the accepted network_qualification_done must
bind (artifact_manifest_sha256) to the LAST network_rescued that precedes it. Sources: that
rescue tar (sha + verify_export), base review record (bundle/profile/registry/sources sha),
run-dir cli-qualification-{role}.json (copied byte-identical, never rewritten) and
cli-receipt-{role}.json. Every byte is hashed while streaming; regular files only; no
absolute, '..', duplicate or link members. Builds <root>.partial-<pid>, renames only after
all checks; if native check_inputs then rejects, removes only the root this call published
and writes materialize-failed-{role}.json, so a retry starts clean.
usage: task8-materialize-roots.py RUN_DIR REVIEW_BASE ROLE [ROLE...]"""

import hashlib
import json
import os
import shutil
import sys
import tarfile
from pathlib import Path

_TREE = os.environ.get("HT_TREE") or sys.exit("HT_TREE must name the controller tree")
sys.path[:0] = [f"{_TREE}/src", f"{_TREE}/experiments/gpu_network_v2"]
import orchestrate as orch  # noqa: E402

ROOT_PREFIX = "/data/hypertrain-"
from hypertrain.gpu_ops import network_qualification as nq  # noqa: E402
from hypertrain.gpu_ops.journal import durable_write, fsha  # noqa: E402

CHUNK = 1 << 20


class Refused(Exception):
    pass


def safe(rel: str) -> Path:
    path = Path(rel)
    if not rel or path.is_absolute() or ".." in path.parts or "\\" in rel:
        raise Refused("unsafe_path " + rel)
    return path


def put(source, member, target: Path, sha: str, size: int) -> None:
    if not member.isfile() or member.size != size:
        raise Refused("member_not_regular_or_size " + member.name)
    target.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    digest, seen = hashlib.sha256(), 0
    stream = source.extractfile(member)
    with target.open("xb") as out:
        while block := stream.read(CHUNK):
            seen += len(block)
            if seen > size:
                raise Refused("member_oversize " + member.name)
            digest.update(block)
            out.write(block)
        out.flush()
        os.fsync(out.fileno())
    os.chmod(target, 0o600)
    if seen != size or digest.hexdigest() != sha:
        raise Refused("member_hash " + member.name)


def copy(src: Path, target: Path, sha: str) -> None:
    data = src.read_bytes()
    if hashlib.sha256(data).hexdigest() != sha:
        raise Refused("input_hash " + str(src))
    durable_write(target, data)


GENERATION_KINDS = (
    "network_execution_intent",
    "network_job_started",
    "network_job_done",
    "network_qualification_captured",
    "network_qualification_failed",
)


def select(rows: list[dict], role: str) -> tuple[dict | None, dict]:
    """Accepted done row when present (native order); otherwise the LAST network_rescued
    after this role's network_qualification_captured whose generation equals the native
    rescue generation recomputed from the journal. done-absent is reported, never invented."""
    done = [
        i
        for i, r in enumerate(rows)
        if r["kind"] == "network_qualification_done" and r.get("role") == role
    ]
    if done:
        accepted = rows[done[-1]]["result"]
        if accepted.get("passed") is not True or accepted.get("root_verified") is not True:
            raise Refused("qualification_not_passed_root_verified")
        before = [
            r for r in rows[: done[-1]] if r["kind"] == "network_rescued" and r.get("role") == role
        ]
        if not before:
            raise Refused("no_rescue_before_qualification_done")
        rescue = before[-1]
        if (
            accepted.get("artifact_manifest_sha256")
            != hashlib.sha256(json.dumps(rescue["files"], sort_keys=True).encode()).hexdigest()
        ):
            raise Refused("accepted_result_not_bound_to_rescue")
        return accepted, rescue
    captured = [
        i
        for i, r in enumerate(rows)
        if r["kind"] == "network_qualification_captured" and r.get("role") == role
    ]
    if not captured:
        raise Refused("no_qualification_captured")
    terminal = [
        r
        for r in rows
        if r.get("role") == role
        and r["kind"] in ("network_qualification_captured", "network_qualification_failed")
    ]
    intents = [
        r
        for r in rows
        if r.get("role") == role
        and r["kind"] == "network_execution_intent"
        and r.get("phase") == "qualification"
    ]
    if (
        terminal[-1]["kind"] != "network_qualification_captured"
        or len(intents) != 1
        or intents[0].get("executions") != 2
    ):
        raise Refused("qualification_capture_or_intent_invalid")
    after = [
        r
        for r in rows[captured[-1] + 1 :]
        if r["kind"] == "network_rescued" and r.get("role") == role
    ]
    if not after:
        raise Refused("no_rescue_after_qualification_captured")
    rescue = after[-1]
    generation = orch.sha256(
        json.dumps(
            [r for r in rows if r.get("role") == role and r["kind"] in GENERATION_KINDS],
            sort_keys=True,
        ).encode()
    )
    if rescue.get("generation") != generation:
        raise Refused("rescue_generation_differs_from_journal")
    return None, rescue


def materialize(run: Path, base: dict, role: str) -> dict:
    rows = [json.loads(line) for line in (run / "journal.jsonl").read_text().splitlines() if line]
    accepted, rescue = select(rows, role)
    config_path = run / f"cli-qualification-{role}.json"
    config_bytes = config_path.read_bytes()
    cfg = json.loads(config_bytes)
    root = Path(cfg["tree"])
    if str(root) != ROOT_PREFIX + role or cfg["role"] != role:
        raise Refused("config_root_or_role")
    expect = {
        "profile": root / "profile.json",
        "registry_manifest": root / "registry.json",
        "lifecycle_receipts": root / "receipts.json",
        "seed_job": root / "out/seed/job.json",
    }
    if any(Path(cfg[k]) != v for k, v in expect.items()):
        raise Refused("config_paths_outside_root")
    if root.exists() or root.is_symlink():
        raise Refused("destination_exists " + str(root))
    archive = Path(rescue["tar"])
    if fsha(archive) != rescue["tar_sha256"]:
        raise Refused("rescue_tar_changed")
    profile = json.loads(Path(base["files"]["profile"]["path"]).read_bytes())
    # Re-verify in the row's recorded mode: full rows (no dedup_known) against the full bound.
    bound, hashes_only, dedup = orch.rescue_mode(profile, rescue)
    files = orch.verify_export(archive, bound, hashes_only, dedup)
    carried = set(orch.carried_names(files, hashes_only, dedup))
    if files != rescue["files"]:
        raise Refused("rescue_manifest_differs_from_journal")
    # Native continue binds accepted result_sha256 to the captured result.json bytes
    # (orchestrate 3444);
    # here the same bytes must sit in the rescued out/ tree and name this exact config + host.
    result_rel = str(Path(cfg["output"]).relative_to(root / "out") / "result.json")
    result_sha = files.get(result_rel, {}).get("sha256")
    if result_sha is None or accepted is not None and result_sha != accepted.get("result_sha256"):
        raise Refused("accepted_result_sha_mismatch")
    with tarfile.open(archive) as source:
        captured = json.loads(source.extractfile("out/" + result_rel).read())
    if (
        captured.get("config_sha256") != hashlib.sha256(config_bytes).hexdigest()
        or captured.get("role") != role
        or captured.get("instance_id") != cfg["instance_id"]
        or captured.get("machine_id") != cfg["machine_id"]
    ):
        raise Refused("captured_result_config_binding")
    sources = json.loads(Path(base["files"]["sources"]["path"]).read_bytes())
    if (
        fsha(base["files"]["sources"]["path"]) != base["files"]["sources"]["sha256"]
        or sources != cfg["sources"]
    ):
        raise Refused("sources_map_mismatch")
    bundle = Path(base["files"]["bundle"]["path"])
    if fsha(bundle) != base["files"]["bundle"]["sha256"]:
        raise Refused("bundle_hash")
    partial = Path(f"{root}.partial-{os.getpid()}")
    partial.mkdir(mode=0o700)
    try:
        with tarfile.open(bundle) as source:
            members = source.getmembers()
            if len({m.name for m in members}) != len(members) or sorted(
                m.name for m in members
            ) != sorted(sources):
                raise Refused("bundle_coverage_differs_from_signed_map")
            for m in members:
                put(source, m, partial / safe(m.name), sources[m.name], m.size)
        # dedup: a repeated content travels once, under its first carried path.
        by_sha = {files[n]["sha256"]: n for n in sorted(carried, reverse=True)}
        with tarfile.open(archive) as source:
            for name, want in sorted(files.items()):
                if name not in carried and (hashes_only or want["sha256"] not in by_sha):
                    # hash-only rescue: proven by custody.json; dedup: content held locally
                    # (known input), bytes not carried. Hash is still in the manifest.
                    continue
                carrier = name if name in carried else by_sha[want["sha256"]]
                put(
                    source,
                    source.getmember("out/" + carrier),
                    partial / "out" / safe(name),
                    want["sha256"],
                    want["bytes"],
                )
        copy(
            Path(base["files"]["profile"]["path"]), partial / "profile.json", cfg["profile_sha256"]
        )
        copy(
            Path(base["files"]["registry"]["path"]),
            partial / "registry.json",
            cfg["image_digest"].removeprefix("sha256:"),
        )
        copy(
            run / f"cli-receipt-{role}.json",
            partial / "receipts.json",
            cfg["lifecycle_receipts_sha256"],
        )
        durable_write(partial / "qualification.json", config_bytes)
        for path in partial.rglob("*"):
            if path.is_symlink() or not (path.is_file() or path.is_dir()):
                raise Refused("non_regular_output " + str(path))
        partial.rename(root)
    except BaseException:
        if partial.is_dir() and not partial.is_symlink():
            shutil.rmtree(partial)  # only this invocation's own partial tree
        raise
    try:
        nq.check_inputs(nq.Qualification.model_validate_json(config_bytes))
    except BaseException as exc:
        shutil.rmtree(root)  # the root this call just published; nothing else existed there
        failure = {
            "status": "CHECK_INPUTS_REJECTED_ROOT_REMOVED",
            "role": role,
            "error": f"{type(exc).__name__}: {exc}",
            "rescue_tar_sha256": rescue["tar_sha256"],
            "config_sha256": hashlib.sha256(config_bytes).hexdigest(),
        }
        durable_write(
            run / f"materialize-failed-{role}.json", json.dumps(failure, sort_keys=True).encode()
        )
        raise
    receipt = {
        "status": "MATERIALIZED_CHECK_INPUTS_OK",
        "role": role,
        "root": str(root),
        "config_sha256": hashlib.sha256(config_bytes).hexdigest(),
        "rescue_tar": str(archive),
        "rescue_tar_sha256": rescue["tar_sha256"],
        "rescue_generation": rescue["generation"],
        "qualification_done": "present" if accepted is not None else "absent",
        "qualification_result_sha256": result_sha,
        "rescue_manifest_sha256": hashlib.sha256(
            json.dumps(files, sort_keys=True).encode()
        ).hexdigest(),
        "rescue_files": len(files),
        "rescue_bytes": sum(v["bytes"] for v in files.values()),
        "bundle_sha256": base["files"]["bundle"]["sha256"],
        "sources_sha256": base["files"]["sources"]["sha256"],
        "source_files": len(sources),
        "profile_sha256": cfg["profile_sha256"],
        "registry_sha256": cfg["image_digest"].removeprefix("sha256:"),
        "receipts_sha256": cfg["lifecycle_receipts_sha256"],
    }
    durable_write(run / f"materialized-{role}.json", json.dumps(receipt, sort_keys=True).encode())
    return receipt


def main() -> int:
    run, base = Path(sys.argv[1]), json.loads(Path(sys.argv[2]).read_bytes())["record"]
    rc = 0
    for role in sys.argv[3:]:
        try:
            out = materialize(run, base, role)
            print(
                "MATERIALIZED",
                role,
                fsha(run / f"materialized-{role}.json"),
                json.dumps(out, sort_keys=True),
                flush=True,
            )
        except (
            Refused,
            orch.Reject,
            nq.Reject,
            OSError,
            tarfile.TarError,
            KeyError,
            ValueError,
        ) as exc:
            print("MATERIALIZE_REJECT", role, type(exc).__name__, exc, flush=True)
            rc = 1
    return rc


if __name__ == "__main__":
    raise SystemExit(main())
