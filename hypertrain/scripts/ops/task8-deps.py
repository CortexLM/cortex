"""Fresh task-8 hosts: controller boot/attach/trust, one pinned dependency batch, seed pre-stage.

Runs between admit and qualification on the admitted run dir. Every SSH call is the controller's
DeadlineSsh over its pinned known_hosts, capped at the signed cutoff minus the full qualification
window (Q + hard margin); a failure on one host cancels the other at its next SSH boundary.
"""

import importlib.util
import json
import os
import re
import shlex
import sys
import threading
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import hf_seed  # noqa: E402

from hypertrain.gpu_ops.remote import RemoteError, TrustError, ensure_identity  # noqa: E402

TREE = Path(os.environ.get("HT_TREE") or sys.exit("HT_TREE must name the controller tree"))
spec = importlib.util.spec_from_file_location(
    "orchestrate", TREE / "experiments/gpu_network_v2/orchestrate.py"
)
assert spec is not None and spec.loader is not None
cli = importlib.util.module_from_spec(spec)
sys.modules["orchestrate"] = cli
spec.loader.exec_module(cli)

RUN = Path(sys.argv[1])
BASE = json.loads(Path(sys.argv[2]).read_bytes())["record"]
FILES = {k: Path(v["path"]) for k, v in BASE["files"].items()}
PROFILE = json.loads(FILES["profile"].read_bytes())
JOB = json.loads(FILES[BASE["role_inputs"]["job"]].read_bytes())
UPLOADS = {"job.json": FILES[BASE["role_inputs"]["job"]]} | {
    JOB["object_paths"][k]: FILES[v] for k, v in BASE["role_inputs"]["objects"].items()
}
if any(not re.fullmatch(r"[A-Za-z0-9_][A-Za-z0-9_.-]*", rel) for rel in UPLOADS):
    raise SystemExit("seed object paths must be plain file names")
GPUS = cli.admission_limits(PROFILE)["gpus"]
PINS = (
    "fastapi==0.142.2 uvicorn==0.54.0 httpx==0.28.1 pydantic==2.13.5 py-sr25519-bindings==0.2.4 "
    "numpy==2.5.3 safetensors==0.8.0 py-ecc==8.0.0 pyarrow==25.0.1 tokenizers==0.23.2"
)
PROBE = (
    "import pathlib,torch,fastapi,uvicorn,httpx,pydantic,sr25519,numpy,safetensors,py_ecc,"
    "pyarrow,tokenizers; "
    "assert pathlib.Path(torch.__file__).resolve().is_relative_to('/venv/main'); "
    "assert torch.__version__=='2.14.0+cu130' and torch.version.cuda=='13.0'; "
    f"n=torch.cuda.device_count(); assert torch.cuda.is_available() and n=={GPUS}; "
    f"assert all(torch.cuda.get_device_properties(i).multi_processor_count=="
    f"{PROFILE['admission']['sm_count']} for i in range(n)); "
    "print('DEPS_OK',n,torch.__version__)"
)
VENV = "/opt/hypertrain/venv/bin/python"
# hypertrain.models.opendecision imports the external opendecision-arch-20261008 package; the
# remote venv gets the exact trusted copy (/data/pydeps == /root/distributed-decision-training/
# opendecision-arch-20261008/src/opendecision, same 15 file hashes) on a .pth, never pip/index.
ARCH = Path("/data/pydeps/opendecision")
ARCH_REMOTE = "/opt/hypertrain/opendecision-arch-20261008/src"
ARCH_SHA256 = {
    "__init__.py": "269301f53d89a0f09b3d58a2a321544ccdf88ecef1e1a8b9bb5ed883b3aea371",
    "calibrate.py": "d0ae39c24c13469030f06b33a2701dce063b6be53588e48c41fc2009636ada91",
    "cascade.py": "c3d747ec1c27aa5bffe32f6bc813ee7c76d7ce428b3224afe853925f0e9e540e",
    "coherence.py": "f610e7e5ad9c47a75e6349651157a6f0e40fd773b5fca8d9c1539127f836998a",
    "decider.py": "9a85968f3a736f2a273c73768e0772aeef145afa69399821487cb05ba62b892a",
    "losses.py": "292a7b7e0f1466c1ddbd8153154f215da5fbb797bffffd9c976c09f4e9f35f02",
    "masking.py": "a785e1589cef654a898417c73928181eeb794fb5e928db2f38eb66eca7bd4e79",
    "model.py": "4821c2e22c19a033ee8ae4932d96fcde88c9ac2601efe42d9380968a5a8b9a8e",
    "presets.py": "5d0e1dcae90b5f3c0808d08c4ae4c34582ca7dfb4397661d907c7e8af90af1cd",
    "records.py": "2c11f4c91c99ebc7db65098ca6c8632df7e6883397a96a5df013a4aff37b3281",
    "schema.py": "4ae15a66b4228a6691a5b2854535aa80251c9a1ad74b98275baca454899f5787",
    "serve.py": "c81cb0bdc6e6cf4665b0034f61705757885a70af45baea2b1a3d5317ad27d9e7",
    "teacher.py": "57ac31ffd014c95eca1dfa071bec0952e7776ac1f9a205064e6c8f9a167acf26",
    "tokenizer.py": "5422c7bfd66cf13c794473ac73619c8513d126cfc35556af09786d8321efe3bc",
    "train.py": "e92cc2708d6bc7ebd4600a8bd588071e50090ff632d1e2623cfe656fd8065b7a",
}
if {p.name for p in ARCH.glob("*.py")} != set(ARCH_SHA256) or any(
    cli.fsha(ARCH / name) != digest for name, digest in ARCH_SHA256.items()
):
    raise SystemExit("opendecision arch source does not match the pinned hashes")
ARCH_LINK = (
    f"set -e; d=$({VENV} -c 'import sysconfig; print(sysconfig.get_paths()[\"purelib\"])'); "
    f'echo {ARCH_REMOTE} > "$d/zz-opendecision-arch.pth"; '
    f"{VENV} -c 'import opendecision.model, opendecision.masking; print(opendecision.__file__)'"
)
INSTALL = (
    f"set -e; [ -x {VENV} ] || "
    "/venv/main/bin/python -m venv --system-site-packages /opt/hypertrain/venv; "
    f"{VENV} -m pip install --no-input --disable-pip-version-check -q {PINS}; "
    f"{VENV} -c {shlex.quote(PROBE)}"
)

cfg = json.loads((RUN / "config.json").read_bytes())
j = cli.Journal(RUN)
window = j.last("fresh_window")
if window is None or {r["role"] for r in j.all("receipt")} != {"h0", "h1"}:
    raise SystemExit("fresh admitted run with h0/h1 receipts required")
provider = cli.Provider(cfg["base_url"], cli.read_api_key(cfg["key_file"]), RUN, j, True)
orch = cli.Orchestrator(cfg, j, provider, RUN)
reserve = (
    PROFILE["microqualification"]["qualification_deadline_seconds"]
    + PROFILE["schedule"]["hard_margin_seconds"]
)
CUTOFF = min(orch.deadline(), window["cutoff_unix"]) - reserve
LOGS = RUN / "logs"
CANCEL = threading.Event()


PUT_STREAMS, PUT_SPLIT = 16, 256 << 20


def put(ssh, local: Path, remote: str, attempts: int = 5) -> None:
    # One direct TCP stream measured 21 Mbit/s on some Vast hosts (v3c h1): large files go as
    # PUT_STREAMS parallel byte ranges, joined remotely; staged_match then proves size+sha256.
    size = local.stat().st_size
    if size > PUT_SPLIT:
        return put_ranges(ssh, local, remote, size, attempts)
    return put_one(ssh, local, remote, attempts)


def put_ranges(ssh, local: Path, remote: str, size: int, attempts: int) -> None:
    step = -(-size // PUT_STREAMS)
    q = shlex.quote(remote)

    def part(i: int) -> None:
        with local.open("rb") as f:
            f.seek(i * step)
            data = f.read(step)
        for attempt in range(attempts):
            try:
                ssh._stream(f"cat > {q}.r{i:02d}", "put-range", data)
                return
            except RemoteError as e:
                if "rc=255" not in str(e) or attempt == attempts - 1 or CANCEL.is_set():
                    raise
                CANCEL.wait(3)

    n = -(-size // step)
    with ThreadPoolExecutor(n) as pool:
        list(pool.map(part, range(n)))
    joined = " ".join(f"{q}.r{i:02d}" for i in range(n))
    ssh.timeout = 600
    if ssh.run(
        f"cat {joined} > {q}.part && rm -f {joined} && mv {q}.part {q}", "put-join", LOGS
    ).returncode:
        raise RuntimeError(f"put_join_failed {remote}")


def put_one(ssh, local: Path, remote: str, attempts: int) -> None:
    # Modal TCP tunnel resets some fresh SSH handshakes (rc 255, kex reset); retry transport only.
    for attempt in range(attempts):
        try:
            return ssh.put(local, remote)
        except RemoteError as e:
            if "rc=255" not in str(e) or attempt == attempts - 1:
                raise
            CANCEL.wait(3)


# Seed objects that are header + zeros + genesis theta are rebuilt on the host from the pinned HF
# checkpoint (controller ingress is not the bottleneck there); only the header crosses the link.
HF_CKPT = "/root/hf-genesis.safetensors"  # outside the run root rescue/audit read
HF_PLANS = {
    rel: hf_seed.plan(path) for rel, path in UPLOADS.items() if path.stat().st_size > PUT_SPLIT
}


def hf_build(ssh, role: str, local: Path, remote: str, plan, ckpt: str) -> bool:
    head, copy = plan
    try:
        ssh.timeout = 1800
        if copy and ssh.run(hf_seed.download_cmd(ckpt), "hf-download", LOGS).returncode:
            print(role, "HF_DOWNLOAD_FAILED", flush=True)
            return False
        ssh.timeout = 600
        ssh._stream(
            f"{VENV} -c " + shlex.quote(hf_seed.BUILD),
            "hf-build",
            hf_seed.payload(head, copy, remote, ckpt, local.stat().st_size),
        )
        return cli.staged_match(ssh, remote, local, LOGS)
    except (RemoteError, OSError) as e:
        print(role, "HF_BUILD_FAILED", remote, e, flush=True)
        return False


def prepare(role: str) -> str:
    try:
        orch.boot(role)
        orch.attach(role)
        # Signed config pins keyscan_attempts=1 and sshd starts after "running": retry the whole
        # controller trust (0 keys writes no known_hosts) until sshd answers or CUTOFF.
        while True:
            try:
                orch.trust(role)
                break
            except TrustError as e:
                if "returned 0 distinct" not in str(e) or time.time() + 15 > CUTOFF:
                    raise
                CANCEL.wait(15)
                if CANCEL.is_set():
                    raise
        ssh = cli.deadline_ssh(orch, role, CUTOFF)
        ssh.cancel = CANCEL
        ssh.timeout = 1200
        installed = ssh.run(INSTALL, "deps-install", LOGS)
        # The provider proxy may not yet recognize the just-attached key.
        while (
            installed.returncode == 255
            and "Permission denied (publickey)" in installed.stderr
            and time.time() + 15 < CUTOFF
        ):
            CANCEL.wait(15)
            if CANCEL.is_set():
                break
            installed = ssh.run(INSTALL, "deps-install", LOGS)
        if installed.returncode or not installed.stdout.strip().startswith(f"DEPS_OK {GPUS} "):
            raise RuntimeError(f"{role} deps_install_failed rc={installed.returncode}")
        print(role, installed.stdout.strip(), flush=True)
        ssh.timeout = 60
        if ssh.run(f"mkdir -p {ARCH_REMOTE}/opendecision", "deps-arch-mkdir", LOGS).returncode:
            raise RuntimeError(f"{role} arch_mkdir_failed")
        for name in sorted(ARCH_SHA256):
            remote = f"{ARCH_REMOTE}/opendecision/{name}"
            if not cli.staged_match(ssh, remote, ARCH / name, LOGS):
                put(ssh, ARCH / name, remote)
                if not cli.staged_match(ssh, remote, ARCH / name, LOGS):
                    raise RuntimeError(f"{role} arch_transport_size_or_hash {name}")
        linked = ssh.run(ARCH_LINK, "deps-arch-link", LOGS)
        if linked.returncode or linked.stdout.strip() != f"{ARCH_REMOTE}/opendecision/__init__.py":
            raise RuntimeError(f"{role} arch_import_failed rc={linked.returncode}")
        print(role, "ARCH_OK", len(ARCH_SHA256), flush=True)
        seed = orch.remote_root(role) + "/out/seed"
        ssh.timeout = 60
        if ssh.run(f"mkdir -p {shlex.quote(seed)}", "deps-mkdir", LOGS).returncode:
            raise RuntimeError(f"{role} seed_mkdir_failed")
        for rel, path in sorted(UPLOADS.items(), key=lambda kv: -kv[1].stat().st_size):
            ssh.timeout = 600
            if cli.staged_match(ssh, seed + "/" + rel, path, LOGS):
                print(role, "REUSED", rel, flush=True)
                continue
            plan = HF_PLANS.get(rel)
            if plan and hf_build(ssh, role, path, seed + "/" + rel, plan, HF_CKPT):
                print(role, "HF_BUILT", rel, hf_seed.REVISION, flush=True)
                continue
            ssh.timeout = 7200
            put(ssh, path, seed + "/" + rel)
            ssh.timeout = 600
            if not cli.staged_match(ssh, seed + "/" + rel, path, LOGS):
                raise RuntimeError(f"{role} seed_transport_size_or_hash {rel}")
            print(role, "STAGED", rel, path.stat().st_size, flush=True)
        ssh.timeout = 60
        ssh.run(f"rm -f {HF_CKPT}* {shlex.quote(seed)}/*.hf-part", "hf-cleanup", LOGS)
        return f"{role} DEPS_AND_SEED_READY"
    except BaseException:
        CANCEL.set()
        raise


# Direct ssh makes h0/h1 ready in the same second: both attach() threads would race the
# unlocked ensure_identity() ssh-keygen. Create the run key once, before the pool.
ensure_identity(RUN)

with ThreadPoolExecutor(2) as pool:
    for line in pool.map(prepare, ("h0", "h1")):
        print(line, flush=True)
