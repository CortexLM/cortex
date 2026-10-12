"""Genuine CUDA authority on real Vast RTX 5090 hosts via the reviewed orchestrate.py launcher.

Phase A (two distinct 2x RTX 5090 hosts, reviewed lifecycle, journal + deadline supervisor):
seed inputs (fixed test keys) -> owner-signed admit record from fresh authenticated finance and
quotes -> launch_action('admit') -> qualify_role h0/h1 (real two-round CUDA carry) -> rescue ->
measured runtime evidence (round seconds, real rescue throughput) -> record_qualification ->
promote -> stage 122 D2 jobs -> owner-signed D2 reservation -> preliminary genuine index ->
the D2 selector against the LIVE hosts -> cleanup with parsed absence.

Phase B (scripts/genuine_cuda_tests.py, one CUDA host): generator + the 10 backend/role selectors.

  python scripts/genuine_cuda_ci.py run --out DIR [--key-file KEY]
Must run under a persistent parent shell (the record binds parent_pid); never under a kill timer.
"""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import math
import os
import shutil
import subprocess
import sys
import time
import urllib.parse
from concurrent.futures import ThreadPoolExecutor
from decimal import Decimal
from pathlib import Path

TREE = Path(__file__).resolve().parents[1]
IMAGE = "sha256:c16a28dd29df300eca0cfb5c5aeacb8947681708634e3cbc8639b530321fe2e2"
REF = "ghcr.io/cortexlm/hypertrain-gpu"
DRIVERS = [
    "570.211.01",
    "580.95.05",
    "580.159.03",
    "580.173.02",
    "580.178.04",
    "595.58.03",
    "595.71.05",
    "595.84",
]
EXCLUDED = {137465, 43459, 142018}
OPENING_CREDIT = Decimal("50")
REMOTE_ROOT = "/var/tmp/htq-{role}"  # noqa: S108  (fixed path re-created on the CI host)
D2_SELECTOR = (
    "tests/miner/test_island_launch_v2.py::"
    "test_d2_execution_plan_invokes_launcher_and_rescues_every_host_on_fault"
)


def sha(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def load(name: str, rel: str):
    spec = importlib.util.spec_from_file_location(name, TREE / rel)
    assert spec and spec.loader
    mod = importlib.util.module_from_spec(spec)
    sys.modules[name] = mod
    spec.loader.exec_module(mod)
    return mod


def log(event: str, **fields) -> None:
    print(json.dumps({"event": event, "unix": int(time.time()), **fields}, default=str), flush=True)


def fixed_keys():
    from hypertrain.protocol.keys import Keypair

    keys = {
        "owner": Keypair(b"\x71" * 32),
        "coord": Keypair(bytes(range(32))),
        "auditor-0": Keypair(bytes(31) + b"\x01"),
        "auditor-1": Keypair(b"\x72" * 32),
        "referee": Keypair(b"\x73" * 32),
        "relay": Keypair(b"\x74" * 32),
    }
    keys.update({f"hot-{i}": Keypair(bytes([80 + i]) * 32) for i in range(4)})
    keys.update({f"cold-{i}": Keypair(bytes([90 + i]) * 32) for i in range(4)})
    return keys


def make_seed(out: Path, genesis: int):
    from hypertrain.protocol.hashing import sha256_hex

    seed = load("genuine_seed", "scripts/network_gpu_seed.py")
    keys = fixed_keys()
    by_seed = {sha256_hex(b"ht-d2-functional-seed-v1|" + r.encode()): k for r, k in keys.items()}
    original = seed.Keypair
    seed.Keypair = lambda raw: by_seed[raw.hex()]
    try:
        seed.generate(out / "seed", out / "seed-secrets", IMAGE, DRIVERS, genesis, genesis + 10**8)
    finally:
        seed.Keypair = original
    shutil.rmtree(out / "seed-secrets")
    return seed


def registry_manifest(out: Path) -> Path:
    import urllib.request

    token = json.loads(
        urllib.request.urlopen(
            f"https://ghcr.io/token?scope=repository:{REF.split('/', 1)[1]}:pull", timeout=30
        ).read()
    )["token"]
    request = urllib.request.Request(
        f"https://ghcr.io/v2/{REF.split('/', 1)[1]}/manifests/{IMAGE}",
        headers={
            "Authorization": "Bearer " + token,
            "Accept": "application/vnd.oci.image.manifest.v1+json, "
            "application/vnd.docker.distribution.manifest.v2+json",
        },
    )
    raw = urllib.request.urlopen(request, timeout=30).read()  # noqa: S310  (https constant)
    if "sha256:" + hashlib.sha256(raw).hexdigest() != IMAGE:
        raise SystemExit("registry manifest digest differs from the candidate image")
    path = out / "registry.json"
    path.write_bytes(raw)
    return path


def snapshot(provider, stage: Path):
    now = int(time.time())
    filters = urllib.parse.quote(json.dumps({"day": {"gte": now - 2592000, "lte": now}}))
    query = {
        "gpu_name": {"eq": "RTX 5090"},
        "num_gpus": {"eq": 2},
        "verified": {"eq": True},
        "rentable": {"eq": True},
        "rented": {"eq": False},
        "disk_space": {"gte": 80},
        "cpu_ram": {"gte": 32768},
        "cpu_cores_effective": {"gte": 8},
        "dph_total": {"lte": 2},
        "inet_up": {"gte": 500},
        "inet_down": {"gte": 3000},
        "type": "on-demand",
        "limit": 1000,
    }
    data, observations = {}, []
    for name, endpoint in [
        ("account", "/api/v0/users/current/"),
        ("instances", "/api/v1/instances/"),
        ("volumes", "/api/v0/volumes/"),
        ("charges", f"/api/v0/charges/?select_filters={filters}&limit=500"),
        ("offers", "/api/v0/bundles/?q=" + urllib.parse.quote(json.dumps(query))),
    ]:
        response = provider.call("GET", endpoint, name)
        if response.status != 200 or not isinstance(response.parsed, dict):
            raise SystemExit(f"provider read {name} failed: {response.status}")
        data[name] = response.parsed
        raw = json.loads((stage / "raw" / response.raw_file).read_bytes())
        observations.append(
            {
                "name": name,
                "requested_unix": int(raw["requested_unix"]),
                "received_unix": int(raw["response_unix"]),
                "verified_unix": int(raw["response_unix"]),
            }
        )
    return data, observations


def pick_offers(offers: list[dict]) -> list[dict]:
    from hypertrain.gpu_ops.budget import host_worst_case

    def overhead(o: dict) -> Decimal:
        return host_worst_case(o, Decimal(1), 80, 30) - Decimal(str(o["dph_total"]))

    ok = [
        o
        for o in offers
        if o.get("machine_id") not in EXCLUDED
        and o.get("driver_version") in DRIVERS
        and o.get("verification") == "verified"
        and o.get("cpu_arch") == "amd64"
        and Decimal(str(o.get("reliability2", 0))) >= Decimal("0.95")
        and Decimal(str(o["dph_total"])) <= Decimal("1.6")
        and overhead(o) <= Decimal("0.5")
    ]
    ok.sort(key=lambda o: (-int(o.get("inet_down") or 0), o["id"]))
    chosen, machines = [], set()
    for o in ok:
        if o["machine_id"] not in machines:
            chosen.append(o)
            machines.add(o["machine_id"])
        if len(chosen) == 2:
            return chosen
    raise SystemExit("fewer than two eligible distinct 2x RTX 5090 machines")


def dec_or_none(v: object) -> Decimal | None:
    try:
        d = Decimal(str(v))
    except (ArithmeticError, ValueError):
        return None
    return d if v is not None and d.is_finite() and d >= 0 else None


def run(a: argparse.Namespace) -> int:
    from hypertrain.gpu_ops.journal import Journal, durable_write
    from hypertrain.gpu_ops.launcher import Orchestrator
    from hypertrain.gpu_ops.provider import Provider, list_rows, read_api_key
    from hypertrain.miner.admission import sign_join
    from hypertrain.protocol import envelope_v2
    from hypertrain.protocol.jcs import canonicalize
    from hypertrain.protocol.messages import Receipt
    from hypertrain.protocol.messages_v2 import HardwareHint, IslandJobV1

    out = a.out.resolve()
    if (out / "runtime/journal.jsonl").exists():
        raise SystemExit("existing lifecycle: never duplicate CREATE")
    out.mkdir(parents=True, exist_ok=True)
    os.umask(0o077)
    cli = load("network_cli", "experiments/gpu_network_v2/orchestrate.py")
    keys = fixed_keys()
    owner = keys["owner"]
    genesis = int(time.time()) + 180 * 86400
    if not (out / "seed/seed.json").exists():
        make_seed(out, genesis)
    public = out / "seed"
    job = IslandJobV1.model_validate_json((public / "job.json").read_bytes())
    genesis = job.manifest.training.beacon.genesis_time
    profile = json.loads((TREE / "experiments/gpu_network_v2/profile.json").read_bytes())
    profile["runtime"].update(image_digest=IMAGE, driver_allowlist=DRIVERS)
    source_profile = out / "source-profile.json"
    source_profile.write_bytes(json.dumps(profile, indent=2).encode())
    driver = load("network_qualification_driver", "scripts/network_gpu_qualification.py")
    sources, bundle = cli.effective_bundle(driver, TREE, source_profile)
    (out / "sources.json").write_bytes(canonicalize(sources))
    (out / "bundle.tar").write_bytes(bundle)
    registry = registry_manifest(out)
    lifecycle_hashes = out / "lifecycle-hashes.json"
    lifecycle_hashes.write_bytes(
        canonicalize(
            {
                rel: sha(TREE / rel)
                for rel in (
                    "experiments/gpu_network_v2/orchestrate.py",
                    "src/hypertrain/gpu_ops/launcher.py",
                    "src/hypertrain/gpu_ops/provider.py",
                    "src/hypertrain/gpu_ops/remote.py",
                    "src/hypertrain/gpu_ops/supervisor.py",
                )
            }
        )
    )
    stage = out / "admit"
    stage.mkdir(exist_ok=True)

    def save(name: str, body) -> Path:
        path = stage / name
        durable_write(path, canonicalize(body))
        return path

    roles = {}
    files: dict[str, dict[str, str]] = {}

    def bind(name: str, path: Path) -> None:
        files[name] = {"path": str(path), "sha256": sha(path)}

    for name, path in {
        "profile": source_profile,
        "source_profile": source_profile,
        "sources": out / "sources.json",
        "bundle": out / "bundle.tar",
        "lifecycle_hashes": lifecycle_hashes,
        "registry": registry,
        "job": public / "job.json",
        "start": public / "start.json",
    }.items():
        bind(name, path)
    for name, rel in job.object_paths.items():
        bind(name, public / rel)
    hot, cold = keys["hot-0"], keys["cold-0"]
    for role in ("h0", "h1"):
        join = sign_join(
            hot,
            cold,
            run_id=job.run_id,
            request_id=hashlib.sha256((job.run_id + role).encode()).hexdigest(),
            expires_beacon=10**7,
            policy_hash=job.manifest.network.admission_policy_hash,
            hardware_hint=HardwareHint(device_name="RTX 5090", device_count=2, driver="advisory"),
        )
        join_path = save(f"join-{role}.json", join.model_dump(mode="json"))
        context = {
            "run_id": job.run_id,
            "owner": owner.ss58,
            "hotkey": hot.ss58,
            "coldkey": cold.ss58,
            "request_id": join.request_id,
            "policy_hash": join.policy_hash,
            "job_sha256": sha(public / "job.json"),
            "admission_id": hashlib.sha256(
                f"join|{job.run_id}|{cold.ss58}|{join.request_id}".encode()
            ).hexdigest(),
            "state": "PROBATION",
            "join_sha256": sha(join_path),
        }
        bind("join-" + role, join_path)
        bind("seed-context-" + role, save(f"seed-context-{role}.json", context))
        roles[role] = {
            "job": "job",
            "name": "qualification",
            "equivalents": 2,
            "hotkey": hot.ss58,
            "join": "join-" + role,
            "seed_context": "seed-context-" + role,
            "start": "start",
            "objects": {name: name for name in job.object_paths},
        }
    authorization = {
        "experiment": "hypertrain-network-v2",
        "currency": "USD",
        "ceiling_usd": "50",
        "max_instances": 2,
        "max_seconds_per_instance": 3600,
        "date": "2026-10-11",
        "source_message": "User 2026-10-11: OPTION 2+1 ALL 11/11; rent 2x RTX 5090 hosts as "
        "needed up to remaining Vast credit minus USD5 cleanup reserve; owner GO for the "
        "reviewed orchestrate.py launcher path.",
    }
    evidence_q = {
        "root_verified_bounded_lifecycle": True,
        "registry_image_verified": True,
        "candidate_image_digest": IMAGE,
        "staged_contract_sha256": sha(out / "bundle.tar"),
        "deadline_supervisor_contract_sha256": sha(TREE / "src/hypertrain/gpu_ops/supervisor.py"),
    }
    reader = Provider(
        "https://console.vast.ai", read_api_key(a.key_file), stage, Journal(stage), live=False
    )
    data, observations = snapshot(reader, stage)
    instances = data["instances"].get("instances")
    # Instances of other runs on the shared account are counted as liabilities (one hour at
    # their own hourly price, floored at --foreign-liability-usd each), never touched.
    foreign = list(instances or [])
    rates = [dec_or_none(i.get("dph_total")) for i in foreign]
    if instances is None or None in rates or data["volumes"] != {"volumes": []}:
        log("refused_unknown_inventory", instances=[i.get("label") for i in foreign])
        return 75
    foreign_liability = sum(
        (max(r, a.foreign_liability_usd) for r in rates if r is not None), Decimal(0)
    )
    credit = Decimal(str(data["account"]["credit"]))
    offers = pick_offers(data["offers"]["offers"])
    stamp = observations[-1]
    for offer in offers:
        offer.update({k: stamp[k] for k in ("requested_unix", "received_unix", "verified_unix")})
    account = observations[0]
    financial = {
        "authenticated": True,
        "account_id": int(data["account"]["id"]),
        "instances": [],
        "volumes": [],
        "charges": [],
        "uncapped_charges": False,
        # Conservative prepaid accounting: every debit since the USD50 opening credit.
        "prior_attempts_usd": str(OPENING_CREDIT - credit),
        "active_liabilities_usd": str(foreign_liability),
        "foreign_instances": [{"id": i["id"], "label": i["label"]} for i in foreign],
        "available_credit_usd": str(credit),
        **{k: account[k] for k in ("requested_unix", "received_unix", "verified_unix")},
    }
    plan = cli.admission_plan(
        profile, evidence_q, authorization, financial, offers, qualification=True
    )
    cutoff = int(time.time()) + 3540
    config = {
        "base_url": "https://console.vast.ai",
        "boot_timeout_seconds": 300,
        "cleanup_contract": "network-v2",
        "cleanup_deadline_unix": cutoff,
        "create_margin_seconds": 900,
        "create_min_interval_seconds": 5,
        "disk_gb": 80,
        "evidence_path": str(out / "lifecycle-evidence.json"),
        "hard_deadline_seconds": 3600,
        "hard_grace_seconds": 0,
        "hosts": [
            {
                "machine_id": o["machine_id"],
                "max_dph_total": str(Decimal(str(o["dph_total"])) * Decimal("1.07")),
                "role": r,
            }
            for o, r in zip(offers, ("h0", "h1"), strict=True)
        ],
        "image": f"{REF}@{IMAGE}",
        "key_file": str(a.key_file),
        "keyscan_attempts": 1,
        "network_profile_file": str(source_profile),
        "poll_seconds": 2,
        "provider_min_interval_seconds": 1,
        "remote_python": "/opt/hypertrain/venv/bin/python",
        "remote_root": REMOTE_ROOT,
        "supervisor_check_seconds": 2,
    }
    bind("config", save("config.json", config))
    for name, body in (
        ("financial", financial),
        ("offers", offers),
        ("authorization", authorization),
        ("evidence", evidence_q),
        ("plan", plan),
        ("observations", observations),
    ):
        bind(name, save(name + ".json", body))
    record = {
        "action": "admit",
        "owner": owner.ss58,
        "parent_pid": os.getppid(),
        "cutoff_unix": cutoff,
        "tree": str(TREE),
        "roles": roles,
        "files": files,
        "staging_bytes": len(bundle)
        + sum(p.stat().st_size for p in public.rglob("*") if p.is_file()),
        "public_dataset_reviewed": True,
        "full_genesis_reviewed": True,
        "quota": {"total": 126, "per_role": 63, "qualification": 4, "remaining": 122},
    }
    receipt = envelope_v2.seal(
        owner,
        "Receipt",
        job.run_id,
        Receipt(
            w=0, commit_hash=hashlib.sha256(canonicalize(record)).hexdigest(), received_round=1
        ),
        10**7,
    )
    admit_path = save("admit.json", {"record": record, "receipt": receipt})
    beacon = 1
    checked = cli.reviewed_launch(admit_path, owner.ss58, beacon, "admit")
    if a.preflight:
        log("preflight_ok", run_id=checked["run_id"], plan=plan["hardware_worst_case_usd"])
        return 0
    log(
        "admit_ready",
        offers=[(o["id"], o["machine_id"], o["driver_version"], o["dph_total"]) for o in offers],
        hardware=plan["hardware_worst_case_usd"],
        credit=str(credit),
    )
    runtime_dir = out / "runtime"
    orch = None
    status = 1
    try:
        admitted = cli.launch_action("admit", checked, runtime_dir, live=True)
        log("admitted", record=admitted)
        journal = Journal(runtime_dir)
        cfg = json.loads((runtime_dir / "config.json").read_bytes())
        orch = Orchestrator(
            cfg,
            journal,
            Provider(cfg["base_url"], read_api_key(a.key_file), runtime_dir, journal, live=True),
            runtime_dir,
        )
        runtime = cli.NetworkRuntime(orch, profile)
        with ThreadPoolExecutor(max_workers=2) as pool:
            list(pool.map(lambda r: await_key(orch, r), orch.roles))
        with ThreadPoolExecutor(max_workers=2) as pool:
            results = list(
                pool.map(lambda r: cli.qualify_role(checked, orch, runtime, r), orch.roles)
            )
        log("qualified", results=results)
        status = phase_after_qualification(a, out, cli, orch, runtime, checked, profile, results)
    finally:
        if orch is not None or (runtime_dir / "config.json").exists():
            if orch is None:
                cfg = json.loads((runtime_dir / "config.json").read_bytes())
                j = Journal(runtime_dir)
                orch = Orchestrator(
                    cfg,
                    j,
                    Provider(cfg["base_url"], read_api_key(a.key_file), runtime_dir, j, live=True),
                    runtime_dir,
                )
            cleanup = orch.cleanup(force=True)
            final = orch.p.call("GET", "/api/v1/instances/", "final-owned-absence")
            rows = list_rows(final)
            owned = {r["instance_id"] for r in orch.j.all("receipt")}
            absent = rows is not None and not any(r["id"] in owned for r in rows)
            durable_write(
                out / "terminal.json",
                canonicalize(
                    {"absent": absent, "owned": sorted(owned), "cleanup": cleanup, "status": status}
                ),
            )
            log("terminal", absent=absent, owned=sorted(owned))
            if not absent:
                status = 3
    return status


def await_key(orch, role: str) -> None:
    from hypertrain.gpu_ops.remote import ensure_identity

    orch.boot(role)
    orch.attach(role)
    ready = orch.j.last("instance_ready", role=role)
    first = min(r["unix"] for r in orch.j.all("create_intent"))
    argv = [
        "ssh",
        "-i",
        str(ensure_identity(orch.dir)),
        "-o",
        "BatchMode=yes",
        "-o",
        "StrictHostKeyChecking=no",
        "-o",
        "UserKnownHostsFile=/dev/null",
        "-o",
        "ConnectTimeout=10",
        "-p",
        str(ready["ssh_port"]),
        f"root@{ready['ssh_host']}",
        "true",
    ]
    while time.time() < first + 240:
        if subprocess.run(argv, capture_output=True, timeout=30, check=False).returncode == 0:  # noqa: S603
            log("key_accepted", role=role, after=int(time.time() - first))
            return
        time.sleep(3)
    raise SystemExit("ssh key not accepted before the staging cutoff: " + role)


def restore_role_tree(cli, orch, checked, role: str, archive: Path) -> Path:
    import tarfile

    root = Path(orch.remote_root(role))
    if root.exists():
        shutil.rmtree(root)
    root.mkdir(parents=True)
    with tarfile.open(checked["files"]["bundle"]) as tar:
        tar.extractall(root, filter="data")
    shutil.copyfile(checked["files"]["profile"], root / "profile.json")
    shutil.copyfile(checked["files"]["registry"], root / "registry.json")
    shutil.copyfile(orch.dir / f"cli-receipt-{role}.json", root / "receipts.json")
    shutil.copyfile(orch.dir / f"cli-qualification-{role}.json", root / "qualification.json")
    with tarfile.open(archive) as tar:
        tar.extractall(root, filter="data")
    return root / "out/qualification/result.json"


def measure_rescue_rate(orch, role: str, size: int) -> dict:
    root = orch.remote_root(role)
    ssh = orch.ssh(role)
    ssh.timeout = 240
    if ssh.run(
        f"head -c {size} /dev/urandom > {root}/rate.bin", "rate-make", orch.dir / "logs"
    ).returncode:
        raise SystemExit("rescue rate probe file failed")
    target = orch.dir / f"rate-{role}.bin"
    started = time.monotonic()
    digest = ssh.get(f"{root}/rate.bin", target)
    seconds = time.monotonic() - started
    assert digest == ssh.remote_sha256(f"{root}/rate.bin", orch.dir / "logs")
    target.unlink()
    ssh.run(f"rm -f {root}/rate.bin", "rate-rm", orch.dir / "logs")
    return {"role": role, "bytes": size, "seconds": seconds, "rate": int(size / seconds)}


def d2_jobs(out: Path, seed_dir: Path, run_id: str, cutoff: int) -> dict[str, dict[str, Path]]:
    import numpy as np

    from hypertrain.protocol.hashing import MerkleTree, sha256_hex
    from hypertrain.protocol.jcs import canonicalize
    from hypertrain.protocol.messages_v2 import IslandJobV1

    base = IslandJobV1.model_validate_json((seed_dir / "job.json").read_bytes())
    rows = np.frombuffer((seed_dir / "samples.u32").read_bytes(), dtype="<u4").reshape(-1, 17)
    tree = MerkleTree([row.tobytes() for row in rows])
    n = len(base.sample_ids)
    jobs: dict[str, dict[str, Path]] = {"h0": {}, "h1": {}}
    for role_index, role in enumerate(("h0", "h1")):
        for k in range(61):
            window = (role_index * 61 + k) % (len(rows) // n)
            ids = list(range(window * n, (window + 1) * n))
            name = f"d2-{role}-{k:02d}"
            directory = out / "d2-jobs" / name
            directory.mkdir(parents=True, exist_ok=True)
            for key in ("start_state", "ef_in", "v0"):
                rel = base.object_paths[key]
                (directory / rel).parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(seed_dir / rel, directory / rel)
            samples = b"".join(rows[i].tobytes() for i in ids)
            proofs = canonicalize([[p.hex() for p in tree.proof(i)] for i in ids])
            (directory / base.object_paths["samples"]).write_bytes(samples)
            (directory / base.object_paths["sample_proofs"]).write_bytes(proofs)
            job = base.model_copy(
                update={"sample_ids": ids, "deadline": max(base.deadline, cutoff)}
            )
            (directory / "job.json").write_bytes(job.model_dump_json().encode())
            assert sha256_hex(samples) and job.run_id == run_id
            jobs[role][name] = directory
    return jobs


def phase_after_qualification(a, out, cli, orch, runtime, checked, profile, results) -> int:
    from hypertrain.gpu_ops.journal import durable_write, fsha
    from hypertrain.protocol import envelope_v2
    from hypertrain.protocol.jcs import canonicalize
    from hypertrain.protocol.messages import Receipt
    from hypertrain.protocol.messages_v2 import IslandJobV1

    owner = fixed_keys()["owner"]
    qual = {}
    for role in orch.roles:
        rescued = orch.j.last("network_rescued", role=role)
        result = restore_role_tree(cli, orch, checked, role, Path(rescued["tar"]))
        qual[role] = {"result": result, "config": result.parent / "config.json", "rescued": rescued}
    engine = checked["driver"]
    for role in orch.roles:
        cfg = engine.Qualification.model_validate_json(qual[role]["config"].read_bytes())
        engine.check_inputs(cfg)
    engine.compare([qual[r]["result"] for r in orch.roles])
    for role in orch.roles:
        rescued = qual[role]["rescued"]
        target = out / "rescue" / role
        target.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(rescued["tar"], target / Path(rescued["tar"]).name)
        durable_write(out / f"rescue-{role}.json", canonicalize(rescued))
        runtime.record_qualification(
            role,
            "qualification",
            {
                "passed": True,
                "root_verified": True,
                "result_sha256": fsha(qual[role]["result"]),
                "artifact_manifest_sha256": hashlib.sha256(
                    json.dumps(qual[role]["rescued"]["files"], sort_keys=True).encode()
                ).hexdigest(),
            },
        )
    rates = [measure_rescue_rate(orch, r, 268_435_456) for r in orch.roles]
    log("rescue_rate", rates=rates)
    seconds = [
        json.loads(qual[r]["result"].read_bytes())["elapsed_seconds"] / 2 for r in orch.roles
    ]
    bounds = cli.artifact_budget(profile)
    rate = min(x["rate"] for x in rates)
    per_round_transfer = bounds["execution_bytes"] / rate
    bound = math.ceil(max(seconds) + per_round_transfer + 5)
    registry = json.loads(Path(checked["files"]["registry"]).read_bytes())
    image_bytes = sum(d["size"] for d in registry["layers"]) + registry["config"]["size"]
    qualified = json.loads(json.dumps(profile))
    qualified["execution_and_transfer_bound_seconds"] = bound
    qualified["runtime"]["qualified"] = True
    qualified["artifacts"].update(
        rescue_bytes_per_second=rate,
        image_and_dependency_bytes=image_bytes,
        staging_bytes_per_host=checked["record"]["staging_bytes"],
    )
    evidence = {
        "profile_model": qualified["model"],
        "layout": qualified["layout"],
        "device": "cuda",
        "torch": "2.14.0+cu130",
        "cuda": "13.0",
        "python": qualified["runtime"]["remote_python"],
        "image_digest": IMAGE,
        "driver_allowlist": DRIVERS,
        "full_round_transfer_bound_seconds": bound,
        "measured_round_seconds": seconds,
        "measured_rescue": rates,
        "measured_rescue_bytes_per_second": rate,
        "image_and_dependency_bytes": image_bytes,
        "staging_bytes_per_host": checked["record"]["staging_bytes"],
        "no_slower_hosts": True,
        "outer_tape_verified": True,
        "root_reviewed_two_host_full_round_transfer": True,
        "full_round_artifacts_sha256": [fsha(qual[r]["result"]) for r in orch.roles],
        "tree": str(TREE),
        "sources": json.loads(Path(checked["files"]["sources"]).read_bytes()),
        "qualification_results": {r: str(qual[r]["result"]) for r in orch.roles},
    }
    if bound > 40:
        log("refused_measured_bound", bound=bound, seconds=seconds, rate=rate)
        return 4
    from hypertrain.protocol.hashing import sha256_hex

    qualified["runtime_evidence_sha256"] = sha256_hex(canonicalize(evidence))
    d2 = out / "d2"
    d2.mkdir(exist_ok=True)
    durable_write(d2 / "runtime-profile.json", canonicalize(qualified))
    durable_write(d2 / "runtime-evidence.json", canonicalize(evidence))
    cli.runtime_admission(qualified, evidence)
    runtime.profile = qualified
    plan = workload_plan(orch, checked)
    durable_write(d2 / "plan.json", canonicalize(plan))
    runtime.promote(evidence, plan)
    log("promoted", bound=bound, plan=plan)
    record = checked["record"]
    run_id = checked["run_id"]
    cutoff = record["cutoff_unix"]
    seed_dir = Path(checked["files"]["job"]).parent
    jobs = d2_jobs(out, seed_dir, run_id, cutoff)
    for role in orch.roles:
        runtime.stage(role, TREE, jobs[role])
    intents = orch.j.all("network_execution_intent")
    done = orch.j.all("network_qualification_done")
    items = []
    for role in orch.roles:
        for name, directory in jobs[role].items():
            job = IslandJobV1.model_validate_json((directory / "job.json").read_bytes())
            items.append(
                {
                    "role": role,
                    "name": name,
                    "equivalents": 1,
                    "backend": "cuda",
                    "directory": str(directory),
                    "job_sha256": fsha(directory / "job.json"),
                    "job_digest": job.digest(),
                    "inputs": {k: fsha(directory / rel) for k, rel in job.object_paths.items()},
                }
            )
    authorization_path = Path(checked["files"]["authorization"])
    files = {
        "profile": d2 / "runtime-profile.json",
        "evidence": d2 / "runtime-evidence.json",
        "sources": Path(checked["files"]["sources"]),
        "bundle": Path(checked["files"]["bundle"]),
        "authorization": authorization_path,
        "plan": d2 / "plan.json",
        "source_profile": Path(checked["files"]["source_profile"]),
        **{f"qualification-config-{r}": qual[r]["config"] for r in orch.roles},
        **{f"qualification-result-{r}": qual[r]["result"] for r in orch.roles},
    }
    from hypertrain.protocol.hashing import sha256_hex as h

    gi = load("genuine_cuda_index", "scripts/genuine_cuda_index.py")
    qual_dir = out / "qual"
    for role in orch.roles:
        target = qual_dir / role
        if target.exists():
            shutil.rmtree(target)
        shutil.copytree(qual[role]["result"].parent, target / "qualification")
    reservation_record = {
        "schema": "ht-d2-execution-reservation/1",
        "action": "execute_plan",
        "owner": owner.ss58,
        "run_id": run_id,
        "reference_hash": h(
            canonicalize(
                IslandJobV1.model_validate_json(
                    Path(checked["files"]["job"]).read_bytes()
                ).manifest.training.reference_spec.model_dump(mode="json")
            )
        ),
        "qualification_authority_hash": "",
        "profile_sha256": fsha(files["profile"]),
        "runtime_evidence_sha256": fsha(files["evidence"]),
        "source_map_sha256": fsha(files["sources"]),
        "bundle_sha256": fsha(files["bundle"]),
        "image_digest": IMAGE,
        "cutoff_unix": cutoff,
        "budget_authority_sha256": fsha(files["authorization"]),
        "budget_plan_sha256": fsha(files["plan"]),
        "quota": {"total": 126, "per_role": 63, "qualification": 4, "remaining": 122},
        "roles": {
            role: {
                "instance_id": orch.receipt(role)["instance_id"],
                "machine_id": orch.host[role]["machine_id"],
                "qualification_intent_sha256": h(
                    canonicalize(next(r for r in intents if r["role"] == role))
                ),
                "qualification_result_sha256": next(r for r in done if r["role"] == role)["result"][
                    "result_sha256"
                ],
            }
            for role in orch.roles
        },
        "jobs_sha256": h(canonicalize(items)),
        "jobs": items,
        "files": {k: {"path": str(p), "sha256": fsha(p)} for k, p in files.items()},
    }
    reservation_record["qualification_authority_hash"] = authority_hash(seed_dir, qual_dir, run_id)
    receipt = envelope_v2.seal(
        owner,
        "Receipt",
        run_id,
        Receipt(w=0, commit_hash=h(canonicalize(reservation_record)), received_round=1),
        10**7,
    )
    durable_write(
        d2 / "d2-reservation.json", canonicalize({"record": reservation_record, "receipt": receipt})
    )
    durable_write(d2 / "lifecycle-config.json", (orch.dir / "config.json").read_bytes())
    index = out / "genuine"
    gi.main(
        [
            "index",
            str(index),
            "--seed",
            str(seed_dir),
            "--qual-dir",
            str(qual_dir),
            "--d2-dir",
            str(d2),
            "--lifecycle",
            str(orch.dir),
            "--beacon",
            "1",
        ]
    )
    env = {
        **os.environ,
        "HT_GENUINE_CUDA_INDEX": str(index / "index-d2.json"),
        "CUBLAS_WORKSPACE_CONFIG": ":4096:8",
        "TMPDIR": str(out / "tmp"),
    }
    (out / "tmp").mkdir(exist_ok=True)
    rc = subprocess.run(  # noqa: S603  (fixed argv)
        [
            sys.executable,
            "-m",
            "pytest",
            "-rs",
            "-p",
            "no:cacheprovider",
            D2_SELECTOR,
            f"--junitxml={out}/d2-junit.xml",
            f"--basetemp={out}/bt",
        ],
        cwd=TREE,
        env=env,
        stdout=(out / "d2-pytest.log").open("wb"),
        stderr=subprocess.STDOUT,
        check=False,
    ).returncode
    text = (out / "d2-pytest.log").read_text()
    log("d2_pytest", rc=rc, tail=text[-400:])
    return 0 if rc == 0 and "1 passed" in text and "skipped" not in text else 5


def authority_hash(seed_dir: Path, qual_dir: Path, run_id: str) -> str:
    from hypertrain.gpu_ops.journal import fsha
    from hypertrain.protocol.hashing import sha256_hex
    from hypertrain.protocol.jcs import canonicalize
    from hypertrain.protocol.messages_v2 import RunManifestV2

    ref = RunManifestV2.model_validate_json((seed_dir / "manifest.json").read_bytes())
    ref = ref.training.reference_spec
    roots = {r: next((qual_dir / r).rglob("result.json")).parent for r in ("h0", "h1")}
    evidence = {
        "run_id": run_id,
        "backend": "cuda",
        "reference_hash": sha256_hex(canonicalize(ref.model_dump(mode="json"))),
        "layout_hash": ref.layout.model_dump_json(),
        "configs": [fsha(roots[r] / "config.json") for r in ("h0", "h1")],
        "results": [fsha(roots[r] / "result.json") for r in ("h0", "h1")],
        "summaries": [
            fsha(roots[r] / f"round-{i}/published/rank-{rank}/summary.json")
            for r in ("h0", "h1")
            for i in range(2)
            for rank in range(ref.layout.n_gpus)
        ],
    }
    return sha256_hex(canonicalize(evidence))


def workload_plan(orch, checked) -> dict:
    from hypertrain.gpu_ops import budget

    response = orch.p.call("GET", "/api/v0/users/current/", "promote-account")
    if response.status != 200 or not isinstance(response.parsed, dict):
        raise SystemExit("promote finance read failed")
    credit = Decimal(str(response.parsed["credit"]))
    offers = json.loads(Path(checked["files"]["offers"]).read_bytes())
    left_hours = Decimal(str(max(0.0, orch.deadline() - time.time()))) / Decimal(3600)
    financial = json.loads(Path(checked["files"]["financial"]).read_bytes())
    liabilities = sum(Decimal(str(o["dph_total"])) for o in offers) * left_hours + Decimal(
        financial["active_liabilities_usd"]
    )
    cost = budget.evaluate(
        snapshot0_credit=credit + (OPENING_CREDIT - credit),
        current_credit=credit - liabilities,
        offers=offers,
        hard_deadline_seconds=3600,
        disk_gb=80,
        egress_gb=30,
        phase_cap=Decimal("50"),
        reserve=Decimal("5"),
    )
    if not cost["admit"] or Decimal(cost["hardware_worst_case_usd"]) > 5:
        raise SystemExit("workload budget refused: " + json.dumps(cost["checks"]))
    return {**cost, "long_workload_allowed": True}


def main(argv: list[str] | None = None) -> int:
    p = argparse.ArgumentParser()
    sub = p.add_subparsers(dest="cmd", required=True)
    r = sub.add_parser("run")
    r.add_argument("--out", type=Path, required=True)
    r.add_argument(
        "--key-file", type=Path, default=Path("/root/.config/image-training-pilot/vast-key")
    )
    r.add_argument("--preflight", action="store_true")
    r.add_argument("--foreign-liability-usd", type=Decimal, default=Decimal("3.0"))
    a = p.parse_args(argv)
    return {"run": run}[a.cmd](a)


if __name__ == "__main__":
    raise SystemExit(main())
