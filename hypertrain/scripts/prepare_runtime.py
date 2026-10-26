"""Task-8 qualification run-time inputs from real material; no keys are generated or printed.

freeze    frozen source snapshot of a tree (same shape the prepare freeze fixture builds)
context   one uid247 JoinRequest (hot + approved escrow-cold signature) and its seed_context
financial GET-only provider snapshot, owned-host offers/observations, owner authorization
"""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import io
import json
import math
import os
import shutil
import sys
import tarfile
import time
import urllib.parse
from decimal import Decimal
from pathlib import Path
from typing import Any

OPERATION_EXTRA = (
    "scripts/network_gpu_operation.py",
    "scripts/network_gpu_seed.py",
    "scripts/network_service_proof.py",
    "scripts/network_authority_snapshot.py",
)


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def write(path: Path, value: Any, mode: int = 0o644) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    data = value if isinstance(value, bytes) else json.dumps(value, sort_keys=True).encode()
    with path.open("xb") as stream:
        os.fchmod(stream.fileno(), mode)
        stream.write(data)
    return path


def freeze(tree: Path, snapshot: Path, image_ref: str) -> dict[str, Any]:
    """Copy exactly the qualification + operator sources; record maps and archives by hash."""
    sys.path[:0] = [str(tree / "src")]
    from hypertrain.gpu_ops import network_qualification

    canonical = network_qualification.sources(tree)
    staged = {rel: digest(tree / rel) for rel in sorted(set(canonical) | set(OPERATION_EXTRA))}
    copy = snapshot / "tree"
    for rel in staged:
        source = tree / rel
        if source.is_symlink() or not source.is_file():
            raise SystemExit("source must be a regular file: " + rel)
        (copy / rel).parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source, copy / rel)
    if network_qualification.sources(copy) != canonical:
        raise SystemExit("frozen copy differs from the tree map")
    maps = {
        "qualification-sources.json": canonical,
        "operation-sources.json": {
            **canonical,
            "scripts/network_gpu_operation.py": staged["scripts/network_gpu_operation.py"],
        },
        "sources.json": staged,
        "files.json": {
            rel: {
                "origin": str(copy / rel),
                "size": (copy / rel).stat().st_size,
                "sha256": d,
            }
            for rel, d in staged.items()
        },
    }
    for name, values in maps.items():
        write(snapshot / name, values)
    write(snapshot / "qualification-source.tar", network_qualification.bundle(copy))
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w") as archive:
        for rel in staged:
            data = (copy / rel).read_bytes()
            member = tarfile.TarInfo(rel)
            member.size, member.mode, member.mtime = len(data), 0o600, 0
            archive.addfile(member, io.BytesIO(data))
    write(snapshot / "effective-source.tar", buffer.getvalue())

    def artifact(path: Path) -> dict[str, Any]:
        return {"path": str(path), "bytes": path.stat().st_size, "sha256": digest(path)}

    record = {
        "snapshot": str(snapshot),
        "archive": artifact(snapshot / "effective-source.tar"),
        "qualification_archive": artifact(snapshot / "qualification-source.tar"),
        "maps": {name: artifact(snapshot / name) for name in maps},
        "approved_pins": {
            rel: staged[rel]
            for rel in (
                "scripts/network_service_proof.py",
                "scripts/network_gpu_operation.py",
                "experiments/gpu_network_v2/orchestrate.py",
            )
        },
        "service_source_observed": {},
        "qualification_parser": {},
        "image_candidate": {"ref": image_ref},
        "paid_GO": False,
    }
    write(snapshot / "freeze.json", record)
    return {
        "freeze": str(snapshot / "freeze.json"),
        "sha256": digest(snapshot / "freeze.json"),
    }


def seed(path: Path) -> bytes:
    """Raw 32-byte or 64-hex-char 0600 seed; contents never leave this process."""
    if path.is_symlink() or not path.is_file() or path.stat().st_mode & 0o077:
        raise SystemExit(f"{path}: regular 0600 seed file required")
    raw = path.read_bytes()
    if len(raw) == 32:
        return raw
    text = raw.strip()
    if len(text) == 64:
        return bytes.fromhex(text.decode("ascii"))
    raise SystemExit(f"{path}: 32-byte or 64-hex seed required")


def context(
    tree: Path,
    out: Path,
    job_path: Path,
    hotkey_seed: Path,
    cold_seed: Path,
    owner: str,
    expires_beacon: int,
    destination: Path,
) -> dict[str, Any]:
    sys.path[:0] = [str(tree / "src")]
    from hypertrain.miner.admission import sign_join
    from hypertrain.protocol.envelope_v2 import verify_join
    from hypertrain.protocol.keys import Keypair
    from hypertrain.protocol.messages_v2 import HardwareHint, IslandJobV1

    job = IslandJobV1.model_validate_json(job_path.read_bytes())
    profile = json.loads((out / "profile.json").read_bytes())
    hot, cold = Keypair(seed(hotkey_seed)), Keypair(seed(cold_seed))
    if hot.ss58 != json.loads((out / "index.json").read_bytes())["hotkey"]:
        raise SystemExit("hotkey seed is not the manifest index hotkey")
    request_id = hashlib.sha256(f"task8-qualification|{job.run_id}|{hot.ss58}".encode()).hexdigest()
    join = sign_join(
        hot,
        cold,
        run_id=job.run_id,
        request_id=request_id,
        expires_beacon=expires_beacon,
        policy_hash=job.manifest.network.admission_policy_hash,
        hardware_hint=HardwareHint(
            device_name=profile["admission"]["gpu_name"],
            device_count=profile["layout"]["n_gpus"],
            driver=profile["runtime"]["driver_allowlist"][0],
        ),
    )
    if not verify_join(join, job.run_id, 1):
        raise SystemExit("join does not verify")
    join_path = write(destination / "join.json", join.model_dump(mode="json"))
    admission_id = hashlib.sha256(
        f"join|{job.run_id}|{join.coldkey}|{join.request_id}".encode()
    ).hexdigest()
    seed_context = {
        "run_id": job.run_id,
        "owner": owner,
        "hotkey": join.hotkey,
        "coldkey": join.coldkey,
        "request_id": join.request_id,
        "policy_hash": join.policy_hash,
        "job_sha256": digest(job_path),
        "admission_id": admission_id,
        "state": "PROBATION",
        "join_sha256": digest(join_path),
    }
    context_path = write(destination / "seed_context.json", seed_context)
    return {
        "join": str(join_path),
        "seed_context": str(context_path),
        "hotkey": join.hotkey,
        "coldkey": join.coldkey,
        "admission_id": admission_id,
        # Same identity qualifies the same job on both hosts: pass these for _h0 and _h1.
        "builder_files": [
            f"{kind}_{role}={path}"
            for kind, path in (("join", join_path), ("seed_context", context_path))
            for role in ("h0", "h1")
        ],
    }


def financial(
    tree: Path,
    config: Path,
    key_file: Path,
    source_message: str,
    destination: Path,
) -> dict[str, Any]:
    """GET-only. Offers are the two owned instances' live rows (adopt: no market offers)."""
    sys.path[:0] = [str(tree / "src")]
    spec = importlib.util.spec_from_file_location(
        "runtime_controller", tree / "experiments/gpu_network_v2/orchestrate.py"
    )
    assert spec is not None and spec.loader is not None
    controller = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = controller
    spec.loader.exec_module(controller)
    from hypertrain.gpu_ops.journal import Journal
    from hypertrain.gpu_ops.provider import Provider

    cfg = json.loads(config.read_bytes())
    profile = json.loads(Path(cfg["network_profile_file"]).read_bytes())
    run = destination / "provider-run"
    run.mkdir(parents=True, mode=0o700)
    provider = Provider(
        "https://console.vast.ai",
        key_file.read_text().strip(),
        run,
        Journal(run),
        False,
    )
    requested = int(time.time())
    snapshot = controller.refresh_readonly(provider)
    account = provider.call("GET", "/api/v0/users/current/", "runtime-account")
    received = int(time.time())
    owned = controller.owned_instances(snapshot, cfg)
    if len(owned) not in (0, 2) or not account.ok():
        raise SystemExit("fresh inventory must hold the two owned instances or none (admit)")
    rows = {row["machine_id"]: row for row in snapshot["instances"]}
    if not owned:
        # Fresh admit: live asks on the signed config machines, GET only.
        gpus = controller.admission_limits(profile)["gpus"]
        for h in cfg["hosts"]:
            q = {
                "machine_id": {"eq": h["machine_id"]},
                "num_gpus": {"eq": gpus},
                "rentable": {"eq": True},
                "rented": {"eq": False},
            }
            r = provider.call(
                "GET",
                "/api/v0/bundles/?" + urllib.parse.urlencode({"q": json.dumps(q)}),
                "runtime-offer",
            )
            asks = r.parsed.get("offers") if r.ok() else None
            if not isinstance(asks, list) or not asks:
                raise SystemExit("no fresh ask for machine " + str(h["machine_id"]))
            rows[h["machine_id"]] = min(asks, key=lambda o: (Decimal(str(o["dph_total"])), o["id"]))
            # Mission floor re-checked on the live ask that create will use, not the base snapshot.
            for link in ("inet_up", "inet_down"):
                v = rows[h["machine_id"]].get(link)
                if not (
                    isinstance(v, (int, float))
                    and not isinstance(v, bool)
                    and math.isfinite(v)
                    and v >= 800
                ):
                    raise SystemExit(f"fresh ask {link} below 800 Mbit/s for {h['machine_id']}")
        received = int(time.time())
    hours = Decimal(controller.lifecycle_timing(profile)["horizon"]) / 3600
    # Foreign account instances for a full horizon; the owned pair is priced by the plan's offers.
    # A stopped instance bills storage only; anything else (running, loading, unknown) bills dph.
    mine = {h["machine_id"] for h in cfg["hosts"]}
    liabilities = (
        sum(
            Decimal(
                str(
                    r["storage_total_cost"]
                    if r.get("actual_status") == "exited" and r.get("cur_state") == "stopped"
                    else r["dph_total"]
                )
            )
            for r in snapshot["instances"]
            if r["machine_id"] not in mine
        )
        * hours
    )
    # Spend so far on the adopted pair: provider charge rows, or run time x rate if larger
    # (daily charge rows lag). Upper bound, never an invented zero.
    sources = {"instance-" + str(row["id"]) for row in owned}
    # Fresh admit has no owned rows yet: every instance charge in the window counts.
    charged = sum(
        Decimal(str(c["amount"]))
        for c in snapshot["charges"]
        if c.get("source") in sources or (not owned and c.get("type") == "instance")
    )
    accrued = sum(
        Decimal(str(row["dph_total"])) * Decimal(received - row["start_date"]) / 3600
        for row in owned
    )
    prior_attempts_usd = f"{max(charged, accrued):.6f}"
    verified = int(time.time())
    stamp = {
        "requested_unix": requested,
        "received_unix": received,
        "verified_unix": verified,
    }
    snapshot.update(
        stamp,
        account_id=account.parsed["id"],
        uncapped_charges=False,
        prior_attempts_usd=prior_attempts_usd,
        active_liabilities_usd=f"{liabilities:.6f}",
        prior_attempts_basis={
            "charged_usd": str(charged),
            "accrued_usd": f"{accrued:.6f}",
        },
    )
    keys = (
        "id",
        "machine_id",
        "num_gpus",
        "gpu_name",
        "dph_total",
        "storage_cost",
        "inet_up_cost",
        "inet_down_cost",
        "disk_space",
        "cpu_cores_effective",
        "cpu_ram",
        "driver_version",
    )
    offers = [{**{k: rows[h["machine_id"]].get(k) for k in keys}, **stamp} for h in cfg["hosts"]]
    observations = [
        {
            "machine_id": h["machine_id"],
            "actual_status": rows[h["machine_id"]].get("actual_status"),
            **stamp,
        }
        for h in cfg["hosts"]
    ]
    limits = controller.admission_limits(profile)
    authorization = {
        "experiment": "hypertrain-network-v2",
        "currency": "USD",
        "ceiling_usd": limits["ceiling_usd"],
        "max_instances": 2,
        "max_seconds_per_instance": controller.lifecycle_timing(profile)["horizon"],
        "source_message": source_message,
        "date": time.strftime("%Y-%m-%d", time.gmtime(verified)),
    }
    names = {
        "financial": snapshot,
        "offers": offers,
        "observations": observations,
        "authorization": authorization,
    }
    paths = {
        name: write(destination / (name + ".json"), value, 0o600) for name, value in names.items()
    }
    return {
        "files": [f"{n}={p}" for n, p in paths.items()],
        "verified_unix": verified,
        "must_seal_before_unix": verified + 30,
    }


def evidence(base: Path, destination: Path) -> dict[str, Any]:
    """Pre-launch qualification evidence from root-reviewed hashes; nothing is measured here."""
    record = json.loads(base.read_bytes())["record"]
    files = {name: Path(item["path"]) for name, item in record["files"].items()}
    if any(digest(files[name]) != item["sha256"] for name, item in record["files"].items()):
        raise SystemExit("base file changed since preparation")
    profile = json.loads(files["profile"].read_bytes())
    if "sha256:" + digest(files["registry"]) != profile["runtime"]["image_digest"]:
        raise SystemExit("registry index is not the pinned image digest")
    value = {
        # Lifecycle/supervisor sources are the task-26/28 root-gated hashes bound in the base.
        "root_verified_bounded_lifecycle": True,
        "registry_image_verified": True,
        "candidate_image_digest": profile["runtime"]["image_digest"],
        "registry_index_sha256": digest(files["registry"]),
        "registry_platform_sha256": digest(files["registry_platform"]),
        "staged_contract_sha256": digest(files["sources"]),
        "staged_bundle_sha256": digest(files["bundle"]),
        "deadline_supervisor_contract_sha256": digest(files["lifecycle_hashes"]),
        "measured_runtime": None,
    }
    path = write(destination / "evidence.json", value)
    return {"file": f"evidence={path}", "sha256": digest(path)}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="step", required=True)
    f = sub.add_parser("freeze")
    f.add_argument("--tree", type=Path, required=True)
    f.add_argument("--snapshot", type=Path, required=True)
    f.add_argument("--image-ref", required=True)
    c = sub.add_parser("context")
    for name in ("tree", "out", "job", "hotkey-seed", "cold-seed", "destination"):
        c.add_argument("--" + name, type=Path, required=True)
    c.add_argument("--owner", required=True)
    c.add_argument("--expires-beacon", type=int, required=True)
    m = sub.add_parser("financial")
    for name in ("tree", "config", "key-file", "destination"):
        m.add_argument("--" + name, type=Path, required=True)
    m.add_argument("--source-message", required=True)
    v = sub.add_parser("evidence")
    v.add_argument("--base", type=Path, required=True)
    v.add_argument("--destination", type=Path, required=True)
    a = parser.parse_args()
    os.umask(0o077)
    if a.step == "evidence":
        result = evidence(a.base.resolve(), a.destination.resolve())
    elif a.step == "freeze":
        result = freeze(a.tree.resolve(), a.snapshot.resolve(), a.image_ref)
    elif a.step == "context":
        result = context(
            a.tree.resolve(),
            a.out.resolve(),
            a.job.resolve(),
            a.hotkey_seed,
            a.cold_seed,
            a.owner,
            a.expires_beacon,
            a.destination.resolve(),
        )
    else:
        result = financial(
            a.tree.resolve(),
            a.config.resolve(),
            a.key_file,
            a.source_message,
            a.destination.resolve(),
        )
    print(json.dumps(result, sort_keys=True))


if __name__ == "__main__":
    main()
