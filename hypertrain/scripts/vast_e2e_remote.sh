#!/usr/bin/env bash
# Remote half of scripts/vast_e2e.py. Runs on the rented 2x RTX 5090 host from /workspace/ht.
# HT_E2E_BACKEND=cpu (gloo) rehearses the identical flow on a CPU host; default is cuda (nccl).
set -uo pipefail
HT=${HT_E2E_SRC:-/workspace/ht}
OUT=${HT_E2E_OUT:-/workspace/out}
BACKEND=${HT_E2E_BACKEND:-cuda}
mkdir -p "$OUT"
cd "$HT"
export CUBLAS_WORKSPACE_CONFIG=:4096:8 CUDA_DISABLE_PTX_JIT=1 OMP_NUM_THREADS=1 PYTHONHASHSEED=0
export HT_IMAGE_DIGEST=sha256:c16a28dd29df300eca0cfb5c5aeacb8947681708634e3cbc8639b530321fe2e2

step() {
  local name=$1; shift
  "$@" >"$OUT/$name.log" 2>&1
  local rc=$?
  printf '%s\t%s\n' "$name" "$rc" >>"$OUT/steps.tsv"
  return $rc
}
: >"$OUT/steps.tsv"

if [ "$BACKEND" = cuda ]; then
  step nvidia_smi nvidia-smi
  MINER=$(command -v hypertrain-miner || true)
  PY=${MINER:+$(dirname "$MINER")/python}
  PY=${PY:-/venv/main/bin/python}
  if ! step pip_install "$PY" -m pip install --no-deps -e "$HT"; then
    step uv_install env VIRTUAL_ENV="$(dirname "$(dirname "$PY")")" uv-pinned pip install --no-deps -e "$HT"
  fi
  "$PY" -m pip install -q pytest==9.1.1 >"$OUT/pytest_install.log" 2>&1 \
    || env VIRTUAL_ENV="$(dirname "$(dirname "$PY")")" uv-pinned pip install pytest==9.1.1 >>"$OUT/pytest_install.log" 2>&1
else
  PY=${HT_E2E_PYTHON:-python}
fi
if ! "$PY" -c "import hypertrain,sys; sys.exit(not hypertrain.__file__.startswith('$HT/src'))"; then
  export PYTHONPATH="$HT/src${PYTHONPATH:+:$PYTHONPATH}"
  echo "editable install not active; PYTHONPATH=$HT/src" >>"$OUT/steps.tsv.notes"
fi
"$PY" -c "import hypertrain; print(hypertrain.__file__)" >"$OUT/hypertrain_path.txt" 2>&1

if [ "$BACKEND" = cuda ]; then
  if "$PY" -c "import hypertrain.miner.admission as a; assert hasattr(a, 'main')" 2>/dev/null; then
    step selfcheck "$PY" -m hypertrain.miner.admission selfcheck
  fi
  grep -rlE "skipif\([^)]*cuda|requires_cuda|mark\.cuda|cuda\.is_available\(\)" tests \
    --include='test_*.py' | sort >"$OUT/cuda_test_files.txt" || true
  if [ -s "$OUT/cuda_test_files.txt" ]; then
    step cuda_pytest "$PY" -m pytest -q -p no:cacheprovider $(cat "$OUT/cuda_test_files.txt")
  fi
fi

step network_e2e "$PY" - "$BACKEND" "$OUT" <<'PYEOF'
import contextlib, hashlib, http.server, importlib.util, json, os, shutil, socket, subprocess
import sys, threading, time
from pathlib import Path
from types import SimpleNamespace

backend, out = sys.argv[1], Path(sys.argv[2])
root = Path.cwd()
work = Path(os.environ.get("HT_E2E_WORK", "/tmp/ht-e2e-work"))
shutil.rmtree(work, ignore_errors=True)
work.mkdir(parents=True)
result = {"backend": backend, "ok": False}

import torch
import uvicorn
import hypertrain.trainer  # noqa: F401
from hypertrain.auditor.replay import pack_state, unpack_state
from hypertrain.miner.island_launch import launch_island, validate_artifacts
from hypertrain.protocol.messages_v2 import RunManifestV2

spec = importlib.util.spec_from_file_location("svc", root / "tests/challenge/test_service_network_v2.py")
svc = importlib.util.module_from_spec(spec); sys.modules["svc"] = spec.loader and svc
spec.loader.exec_module(svc)
esc = svc.fixture
N = 2
if backend == "cuda":
    driver = subprocess.run(["nvidia-smi", "--query-gpu=driver_version", "--format=csv,noheader"],
                            check=True, capture_output=True, text=True).stdout.split()
    sm = torch.cuda.get_device_properties(0).multi_processor_count
    assert torch.cuda.device_count() == N, torch.cuda.device_count()
    result.update(driver=driver, sm_count=sm, gpus=[torch.cuda.get_device_name(i) for i in range(N)])
else:
    driver, sm = ["cpu-test"], 170

base_setup = esc.setup
def setup(mode="test"):
    s = base_setup(mode)
    b = s.manifest.body()
    ref = b["training"]["reference_spec"]
    ref["layout"].update(pp=1, n_gpus=N, dp_size=N, ep_size=1, zero1=False)
    ref.update(image_digest=os.environ["HT_IMAGE_DIGEST"], driver_allowlist=sorted(set(driver)),
               sm_count=sm)
    b["training"]["inner"].update(H=2, J=1)
    return esc.Setup(RunManifestV2.model_validate(b), s.policy, s.admission_policy, s.rows, s.tree)
esc.setup = setup

# Fake subnet-100 master: /v1/metagraph/latest lists exactly the miner hotkey.
HOT, COLD = svc.HOT[0], svc.COLD[0]
class Master(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        body = json.dumps({"netuid": 100, "hotkeys": {HOT.ss58: 7}}).encode()
        self.send_response(200 if self.path == "/v1/metagraph/latest" else 404)
        self.send_header("Content-Type", "application/json"); self.end_headers()
        self.wfile.write(body)
    def log_message(self, *a): pass
master = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Master)
threading.Thread(target=master.serve_forever, daemon=True).start()
mg = json.loads(urllib_get := __import__("urllib.request").request.urlopen(
    f"http://127.0.0.1:{master.server_port}/v1/metagraph/latest", timeout=5).read())
assert mg["netuid"] == 100 and HOT.ss58 in mg["hotkeys"], "miner not registered on subnet 100"
result["metagraph_uid"] = mg["hotkeys"][HOT.ss58]

class Ready(uvicorn.Server):
    def __init__(self, c): super().__init__(c); self.ready = threading.Event()
    async def startup(self, sockets=None):
        await super().startup(sockets=sockets); self.ready.set()

(work / "svc").mkdir()
gen = svc.network.__wrapped__(work / "svc", SimpleNamespace(param=None))
net = next(gen)
manifest = net.manifest
sock = socket.socket(); sock.bind(("127.0.0.1", 0)); port = sock.getsockname()[1]
server = Ready(uvicorn.Config(net.client.app, log_level="error"))
threading.Thread(target=lambda: server.run(sockets=[sock]), daemon=True).start()
assert server.ready.wait(30)
api = f"http://127.0.0.1:{port}"

def keyfile(name, kp_seed):
    p = work / name; p.write_bytes(kp_seed); p.chmod(0o600); return p
hotf, coldf = keyfile("hot.key", bytes([80]) * 32), keyfile("cold.key", bytes([90]) * 32)
cfg = work / "miner.toml"
cfg.write_text("\n".join(f"{k} = {json.dumps(str(v))}" for k, v in {
    "api": api, "keyfile": hotf, "workdir": work / "miner", "state_source": "local",
    "image_digest": manifest.training.reference_spec.image_digest, "run_id": manifest.run_id(),
    "owner_hotkey": svc.OWNER.ss58, "device": backend}.items()) + "\n")
miner = shutil.which("hypertrain-miner")
assert miner, "hypertrain-miner not on PATH"

def cli(*args):
    p = subprocess.run([miner, *args, "--config", str(cfg)], capture_output=True, text=True,
                       timeout=1800)
    (out / f"miner-{args[0]}-{int(time.time()*1e3)}.log").write_text(p.stdout + "\n---\n" + p.stderr)
    assert p.returncode == 0, (args, p.returncode, p.stderr[-2000:])
    return json.loads(p.stdout.strip().splitlines()[-1])

joined = cli("join-v2", "--coldkey", str(coldf), "--request-id", hashlib.sha256(b"vast-e2e").hexdigest(),
             "--expiry", "10000")
result["join"] = {k: joined.get(k) for k in ("admission_id", "status")}
result["admission_status"] = cli("status-v2")

setup_v = esc.Setup(manifest, net.setup.policy, net.setup.admission_policy, net.setup.rows, net.setup.tree)
per = manifest.training.batch_samples()

def stage(d, w, start=None, ef=None):
    job = esc.stage(setup_v, d, tuple(range(w * per, (w + 1) * per)), w)
    upd = {"deadline": int(time.time()) + 1800}
    if start is not None:
        (d / "start_state").write_bytes(start); (d / "ef_in").write_bytes(ef)
        upd |= {"start_state_sha256": hashlib.sha256(start).hexdigest(),
                "ef_in_sha256": hashlib.sha256(ef).hexdigest()}
    job = job.model_copy(update=upd)
    (d / "job.json").write_text(job.model_dump_json())
    return job

def digest(d):
    s = json.loads((d / "rank-0/summary.json").read_bytes())["commitments"]
    files = {f"rank-{r}/{n}": hashlib.sha256((d / f"rank-{r}/{n}").read_bytes()).hexdigest()
             for r in range(N) for n in ("leaves.json", "delta.bin", "state.safetensors", "ef.safetensors")}
    return {k: s[k] for k in ("leaves_root", "delta_hash", "final_theta_hash", "state_root",
                              "ef_out_hash")} | {"files": files}

rounds, start, ef = [], None, None
for w in range(2):
    d = work / f"round-{w}"; d.mkdir()
    job = stage(d, w, start, ef)
    t0 = time.time(); paths = cli("run-v2", "--job", str(d / "job.json")); dt = time.time() - t0
    pub = Path(paths["delta"]).parents[1]
    validate_artifacts(job, pub)
    mine = digest(pub)
    # Auditor replay: independent fresh launch of the same job, same backend.
    a = work / f"audit-{w}"; a.mkdir()
    for rel in job.object_paths.values(): shutil.copyfile(d / rel, a / rel)
    audit = digest(launch_island(job, a, backend=backend, trace=True).directory)
    rounds.append({"w": w, "seconds": round(dt, 2), "miner": mine, "auditor": audit,
                   "auditor_match": audit == mine})
    theta, st = unpack_state((pub / "rank-0/state.safetensors").read_bytes())
    start, ef = pack_state(theta), (pub / "rank-0/ef.safetensors").read_bytes()
    for n in ("summary.json", "leaves.json"):
        shutil.copyfile(pub / f"rank-0/{n}", out / f"round-{w}-rank-0-{n}")

# Repeat round 0 from scratch on the same host: must be bitwise identical.
r = work / "repeat-0"; r.mkdir()
job0 = stage(r, 0)
again = digest(launch_island(job0, r, backend=backend, trace=True).directory)
result["repeat_round0"] = again
result["repeat_match"] = again == rounds[0]["miner"]
result["rounds"] = rounds
result["chained"] = rounds[0]["miner"]["final_theta_hash"] != rounds[1]["miner"]["final_theta_hash"]
result["ok"] = all(x["auditor_match"] for x in rounds) and result["repeat_match"] and result["chained"]
server.should_exit = True
(out / "e2e.json").write_text(json.dumps(result, indent=2, sort_keys=True))
print(json.dumps({"ok": result["ok"]}))
sys.exit(0 if result["ok"] else 1)
PYEOF

"$PY" - "$OUT" "$BACKEND" <<'PYEOF'
import json, sys
from pathlib import Path
out = Path(sys.argv[1])
steps = dict(line.split("\t") for line in (out / "steps.tsv").read_text().splitlines() if line)
steps = {k: int(v) for k, v in steps.items()}
e2e = json.loads((out / "e2e.json").read_text()) if (out / "e2e.json").exists() else None
files = (out / "cuda_test_files.txt")
required = ["network_e2e"] + (["nvidia_smi"] if sys.argv[2] == "cuda" else [])
ok = all(steps.get(k) == 0 for k in required) and bool(e2e and e2e["ok"]) and all(
    steps[k] == 0 for k in ("selfcheck", "cuda_pytest") if k in steps)
json.dump({"ok": ok, "backend": sys.argv[2], "steps": steps, "e2e": e2e,
           "cuda_test_files": files.read_text().split() if files.exists() else None},
          open(out / "result.json", "w"), indent=2, sort_keys=True)
print("RESULT ok=%s steps=%s" % (ok, steps))
sys.exit(0 if ok else 1)
PYEOF
