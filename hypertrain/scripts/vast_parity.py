"""Run the pre-registered full parity grid (docs/parity.md section 4) on one rented Vast host.

Reuses scripts/vast_e2e.py (Api, admission, CREATE, ssh, rescue-before-delete, watchdog,
absence check, journal); only the offer filter, upload set and remote command differ.
The grid is 150 independent single-thread CPU jobs capped at 64 processes (pre-registered), and
the simulator is CPU-only (trainer codecs and init are host tensors), so the offer is the
cheapest verified host with >= 64 physical cores (128 threads), >= 96 GB RAM and a fast clock.
The grid is started detached on the host (setsid nohup), polled over short ssh calls, and the
rows file is pulled every poll so a lost host still leaves the finished rows on disk.

  python scripts/vast_parity.py --key-file KEY --max-usd 6.5 --max-minutes 345 [--dry-run]
      [--resume-dir DIR]   # DIR (named resume/) holds a rescued parity_runs.jsonl
      [--exclude-machines 1,2] [--gate-s-per-step 2.5] [--cpu-regex '9654|9754']

Load gate: right after the 64-worker grid starts, a 65th process times 4 inner steps of a
full-scale M=8 replica set (6-step run minus 2-step run, so setup cost cancels). Above
--gate-s-per-step the host is rejected (exit 125): rescue, DELETE, parsed absence as usual.

Resume follows the re-run recipe of docs/parity.md section 5: finished rows are kept and
skipped, parity_state.json is not carried, so the 12 h cap restarts; unfinished jobs re-run
from scratch (each job is deterministic, so a re-run gives the same row).
"""

from __future__ import annotations

import importlib.util
import re
import sys
import time
from pathlib import Path
from typing import Any

_spec = importlib.util.spec_from_file_location("vast_e2e", Path(__file__).with_name("vast_e2e.py"))
assert _spec and _spec.loader
vast = importlib.util.module_from_spec(_spec)
sys.modules.setdefault("vast_e2e", vast)
_spec.loader.exec_module(vast)

EVIDENCE = vast.EVIDENCE.parent / "vast-parity"
MIN_THREADS, MIN_RAM_MB, MIN_GHZ = 128, 96_000, 3.0
REMOTE = r"""
set -u
cd /workspace/ht
OUT=/workspace/out
mkdir -p "$OUT"
export CUBLAS_WORKSPACE_CONFIG=:4096:8 OMP_NUM_THREADS=1 MKL_NUM_THREADS=1 PYTHONHASHSEED=0
export HYPERTRAIN_DATA_DIR=/workspace/ht/data PYTHONPATH=/workspace/ht/src
MINER=$(command -v hypertrain-miner || true)
PY=${MINER:+$(dirname "$MINER")/python}
PY=${PY:-/venv/main/bin/python}
if [ -d resume ]; then cp resume/parity_runs.jsonl "$OUT"/; fi
echo "$PY" >"$OUT/PY"
{ nproc; lscpu; free -g; "$PY" -c 'import torch; print(torch.__version__)'; } >"$OUT/host.txt" 2>&1
"$PY" -c 'import hypertrain; print(hypertrain.__file__)' >>"$OUT/host.txt" 2>&1
"$PY" experiments/sim/parity_grid.py --full --workers 64 --out-dir "$OUT" >"$OUT/grid.log" 2>&1
echo $? >"$OUT/EXIT"
"""


BENCH = r"""
import sys, time
sys.path.insert(0, "experiments/sim")
import parity_grid as pg
from diloco import ARMS, ChunkedShards, SimSpec, simulate

sc = pg.SCALES["full"]
tr = ChunkedShards(pg.DATA / "train", sc["seq_len"])
t = {}
for steps in (2, 6):
    cfg = pg.build_cfg(sc, 0, pg.GLOBAL_BATCH // 8)
    spec = SimSpec(M=8, H=1, steps=steps, outer="average", outer_lr=1.0, outer_momentum=0.0,
                   arm=ARMS["A0"], seed=0)
    t0 = time.time()
    simulate(spec, cfg, tr, tr.n)
    t[steps] = time.time() - t0
print(f"S_PER_STEP {(t[6] - t[2]) / 4:.4f}", flush=True)
"""
BENCH_CMD = (
    "sleep 45; cd /workspace/ht && export CUBLAS_WORKSPACE_CONFIG=:4096:8 OMP_NUM_THREADS=1 "
    "HYPERTRAIN_DATA_DIR=/workspace/ht/data PYTHONPATH=/workspace/ht/src && "
    "cat /proc/loadavg >&2 && timeout 600 $(cat /workspace/out/PY) - && cat /proc/loadavg >&2"
)


PY_FIND = '$(m=$(command -v hypertrain-miner) && echo $(dirname "$m")/python || echo python)'


PROBE_CMD = (
    "cat > /workspace/bench.py; cd /workspace/ht && export CUBLAS_WORKSPACE_CONFIG=:4096:8 "
    "OMP_NUM_THREADS=1 HYPERTRAIN_DATA_DIR=/workspace/ht/data PYTHONPATH=/workspace/ht/src; "
    f"PY={PY_FIND}; "
    "for env in plain malloc; do for n in 16 32 64; do "
    "if [ $env = malloc ]; then E='MALLOC_MMAP_THRESHOLD_=4294967296 MALLOC_TRIM_THRESHOLD_="
    "68719476736 MALLOC_TOP_PAD_=1073741824'; else E=''; fi; "
    "for i in $(seq $n); do env $E timeout 900 $PY /workspace/bench.py > /tmp/p$i.txt & done; "
    "wait; echo \"$env n=$n $(cat /tmp/p*.txt | awk '{s+=$2;k++}END{print s/k, k}')\"; "
    "rm -f /tmp/p*.txt; done; done"
)


class ParityHarness(vast.Harness):
    label_prefix = "hypertrain-parity"
    offer_desc = f">= {MIN_THREADS}-thread CPU"
    upload_paths = ["src", "experiments", "data", "pyproject.toml", "uv.lock", "README.md"]
    rsync_excludes = ["/data/raw", "/experiments/results", "/experiments/gpu_*"]

    def offer_query(self) -> dict[str, Any]:
        return {
            "cpu_cores_effective": {"gte": MIN_THREADS},
            "cpu_ram": {"gte": MIN_RAM_MB},
            "verified": {"eq": True},
            "rentable": {"eq": True},
            "rented": {"eq": False},
            "reliability": {"gt": 0.98},
            "inet_down": {"gte": 500},
            "disk_space": {"gte": self.disk_gb},
            "type": "on-demand",
            "order": [["dph_total", "asc"]],
            "limit": 128,
        }

    def eligible(self, o: Any) -> bool:
        return (
            isinstance(o, dict)
            and type(o.get("id")) is int
            and o.get("machine_id") not in vast.EXCLUDED_MACHINES
            and o.get("machine_id") not in self.a.exclude_machines
            and re.search(self.a.cpu_regex, str(o.get("cpu_name") or "")) is not None
            and o.get("verified") is not False
            and o.get("rentable") is not False
            and o.get("rented") is not True
            and (c := vast.dec(o.get("cpu_cores_effective"))) is not None
            and c >= MIN_THREADS
            and (m := vast.dec(o.get("cpu_ram"))) is not None
            and m >= MIN_RAM_MB
            and (g := vast.dec(o.get("cpu_ghz"))) is not None
            and g >= MIN_GHZ
            and (r := vast.dec(o.get("reliability", o.get("reliability2")))) is not None
            and r > vast.Decimal("0.98")
            and (d := vast.dec(o.get("inet_down"))) is not None
            and d >= 500
            and vast.dec(o.get("dph_total")) is not None
            and vast.dec(o.get("storage_cost")) is not None
        )

    def run_remote(self) -> int:
        idle = self.out / "gate_idle.txt"
        self.sh(
            "mkdir -p /workspace/out && "
            + BENCH_CMD.replace("sleep 45; ", "").replace("$(cat /workspace/out/PY)", PY_FIND),
            stdin=BENCH.encode(),
            timeout=720,
            stdout_path=idle,
        )
        self.api.log("idle_gate", out=idle.read_text()[-200:] if idle.exists() else None)
        if self.a.probe:
            probe = self.out / "probe.txt"
            self.sh(PROBE_CMD, stdin=BENCH.encode(), timeout=3000, stdout_path=probe)
            self.api.log("probe", out=probe.read_text()[-2000:] if probe.exists() else None)
            return 0
        launch = f"setsid nohup bash -c {vast.shlex.quote(REMOTE)} >/dev/null 2>&1 < /dev/null &"
        if self.sh(launch, timeout=120) != 0:
            raise RuntimeError("remote launch failed")
        self.api.log("grid_launched")
        gate = self.out / "gate.txt"
        self.sh(BENCH_CMD, stdin=BENCH.encode(), timeout=720, stdout_path=gate)
        text = gate.read_text() if gate.exists() else ""
        got = [float(x.split()[1]) for x in text.splitlines() if x.startswith("S_PER_STEP ")]
        self.result["gate_s_per_step"] = got[0] if got else None
        self.api.log("load_gate", s_per_step=self.result["gate_s_per_step"])
        if not got or got[0] > float(self.a.gate_s_per_step):
            self.result["gate_rejected"] = True
            return 125
        status, part = self.out / "exit.txt", self.out / "partial_parity_runs.jsonl"
        tmp = part.with_suffix(".tmp")
        while time.time() < self.deadline and not self.cancel.is_set():
            self.api.sleep(float(self.a.poll_seconds))
            rc = self.sh("cat /workspace/out/parity_runs.jsonl", timeout=120, stdout_path=tmp)
            if rc == 0 and (not part.exists() or tmp.stat().st_size >= part.stat().st_size):
                tmp.replace(part)  # never let a failed pull shrink the local copy
            self.sh("cat /workspace/out/EXIT 2>/dev/null", timeout=60, stdout_path=status)
            text = status.read_text().strip() if status.exists() else ""
            if text:
                self.api.log("grid_exit", rc=text)
                return int(text)
        return 124


GPU_BENCH = r"""
import sys, time
sys.path.insert(0, "experiments/sim")
import parity_grid as pg
from diloco import ARMS, ChunkedShards, SimSpec, simulate
import torch
from hypertrain.trainer.model import init_params

sc = pg.SCALES["full"]
tr = ChunkedShards(pg.DATA / "train", sc["seq_len"])
t = {}
for steps in (5, 25):
    cfg = pg.build_cfg(sc, 0, pg.GLOBAL_BATCH // 8)
    th = {n: x.to("cuda") for n, x in init_params(cfg.model).items()}
    spec = SimSpec(M=8, H=5, steps=steps, outer="nesterov", outer_lr=0.6, outer_momentum=0.9,
                   arm=ARMS["A0"], seed=0)
    t0 = time.time()
    simulate(spec, cfg, tr, tr.n, th)
    torch.cuda.synchronize()
    t[steps] = time.time() - t0
print(f"S_PER_STEP {(t[25] - t[5]) / 20:.4f}", flush=True)
"""
# MPS lets the grid's many tiny-kernel processes share the GPU concurrently instead of
# time-slicing contexts; numerics are unchanged (same kernels). Falls back silently to no MPS.
MPS_ENV = "CUDA_MPS_PIPE_DIRECTORY=/tmp/mps CUDA_MPS_LOG_DIRECTORY=/tmp/mps-log"
MPS_START = (
    f"export {MPS_ENV}; mkdir -p /tmp/mps /tmp/mps-log; "
    "nvidia-cuda-mps-control -d && sleep 2 && echo MPS_ON || echo MPS_OFF"
)
GPU_ENV = (
    f"export {MPS_ENV}; "
    "cd /workspace/ht && export CUBLAS_WORKSPACE_CONFIG=:4096:8 OMP_NUM_THREADS=1 "
    "MKL_NUM_THREADS=1 PYTHONHASHSEED=0 HYPERTRAIN_SIM_DEVICE=cuda HYPERTRAIN_SIM_BATCHED=1 "
    "HYPERTRAIN_DATA_DIR=/workspace/ht/data PYTHONPATH=/workspace/ht/src && "
    f"PY={PY_FIND} && "
)
GPU_PROBE = (
    "cat > /workspace/gbench.py; " + GPU_ENV + "for n in {ns}; do "
    "for i in $(seq $n); do timeout 900 $PY /workspace/gbench.py > /tmp/g$i.txt 2>&1 & done; "
    'wait; echo "n=$n $(grep -h S_PER_STEP /tmp/g*.txt | '
    "awk '{{s+=$2;k++}}END{{print s/k, k}}')\"; "
    "rm -f /tmp/g*.txt; done"
)
# replica-steps of the whole grid (sum over jobs of M * steps; DP = 1 replica) and a 15% margin
# for evaluation, outer steps and the larger per-replica batch of low-M cells
GRID_REPLICA_STEPS, GRID_OVERHEAD = 5_247_000, 1.15
MIN_GPU_CORES = 32  # 24-core probe: 93 replica-steps/s flat from 16 to 24 processes


class GpuParityHarness(ParityHarness):
    """1x RTX 5090: the unchanged grid with HYPERTRAIN_SIM_DEVICE=cuda, N jobs sharing the GPU.

    Order on the host: CUDA equivalence jobs (experiments/sim/parity_equiv.py, compared locally
    against the CPU run), then a real-load probe at N concurrent full-scale M=8 benchmarks for
    several N; the best aggregate rate sets --workers and projects the grid time. The host is
    rejected (exit 125) if the projection exceeds the remaining wall budget or the 12 h cap.
    """

    label_prefix = "hypertrain-parity-gpu"
    offer_desc = "1x RTX 5090"
    load_timeout_s = 900.0

    def offer_query(self) -> dict[str, Any]:
        return {
            "gpu_name": {"eq": "RTX 5090"},
            "num_gpus": {"eq": 1},
            "cpu_cores_effective": {"gte": MIN_GPU_CORES},
            "cpu_ram": {"gte": 48_000},
            "verified": {"eq": True},
            "rentable": {"eq": True},
            "rented": {"eq": False},
            "reliability": {"gt": 0.98},
            "inet_down": {"gte": 300},
            "disk_space": {"gte": self.disk_gb},
            "type": "on-demand",
            "order": [["dph_total", "asc"]],
            "limit": 64,
        }

    def eligible(self, o: Any) -> bool:
        drv = str(o.get("driver_version") or "") if isinstance(o, dict) else ""
        return (
            isinstance(o, dict)
            and type(o.get("id")) is int
            and o.get("machine_id") not in vast.EXCLUDED_MACHINES
            and o.get("machine_id") not in self.a.exclude_machines
            and o.get("gpu_name") == "RTX 5090"
            and o.get("num_gpus") == 1
            and re.match(r"(580|595)\.", drv) is not None  # drivers proven by genuine CUDA CI
            and o.get("verified") is not False
            and o.get("rentable") is not False
            and o.get("rented") is not True
            and (c := vast.dec(o.get("cpu_cores_effective"))) is not None
            and c >= MIN_GPU_CORES
            and (m := vast.dec(o.get("cpu_ram"))) is not None
            and m >= 48_000
            and (r := vast.dec(o.get("reliability", o.get("reliability2")))) is not None
            and r > vast.Decimal("0.98")
            and (d := vast.dec(o.get("inet_down"))) is not None
            and d >= 300
            and vast.dec(o.get("dph_total")) is not None
            and vast.dec(o.get("storage_cost")) is not None
        )

    def wait_ready(self) -> None:
        """Loading timeout: give up (cleanup deletes) if not ssh-ready within load_timeout_s."""
        real = self.deadline
        self.deadline = min(real, time.time() + self.load_timeout_s)
        try:
            super().wait_ready()
        finally:
            self.deadline = real

    def run_remote(self) -> int:
        host = self.out / "host.txt"
        self.sh(
            GPU_ENV + "nproc; free -g; nvidia-smi; $PY -c 'import torch; "
            "print(torch.__version__, torch.version.cuda, torch.cuda.get_device_name(0))'",
            timeout=300,
            stdout_path=host,
        )
        cores = int(vast.dec(self.offer.get("cpu_cores_effective")) or 16)
        mps = self.out / "mps.txt"
        self.sh(MPS_START, timeout=60, stdout_path=mps)
        self.result["mps"] = mps.exists() and "MPS_ON" in mps.read_text()
        self.api.log("mps", on=self.result["mps"])
        eq = self.out / "equiv.txt"
        self.sh(
            GPU_ENV + "mkdir -p /workspace/out && $PY experiments/sim/parity_equiv.py --device "
            "cuda --out /workspace/out/equiv_cuda.json 2>&1",
            timeout=1800,
            stdout_path=eq,
        )
        self.api.log("equiv_cuda", out=eq.read_text()[-500:] if eq.exists() else None)
        cmp = self.out / "equiv_compare.json"
        rc = self.sh(
            "cat > /workspace/out/equiv_cpu.json; " + GPU_ENV + "$PY experiments/sim/"
            "parity_equiv.py --compare /workspace/out/equiv_cpu.json "
            "/workspace/out/equiv_cuda.json",
            stdin=Path(self.a.equiv_cpu).read_bytes(),
            timeout=120,
            stdout_path=cmp,
        )
        self.result["equiv_pass"] = rc == 0
        self.api.log("equiv_compare", rc=rc)
        if rc != 0:  # device changes the verdict scale: do not spend on the grid
            self.result["gate_rejected"] = True
            return 126
        ns = [n for n in (8, 16, 24, 32, 48, 64) if n <= min(cores, 64)]
        if self.a.gpu_workers:
            ns = [self.a.gpu_workers]
        probe = self.out / "probe.txt"
        self.sh(
            GPU_PROBE.format(ns=" ".join(map(str, ns))),
            stdin=GPU_BENCH.encode(),
            timeout=3000,
            stdout_path=probe,
        )
        rates: dict[int, float] = {}
        for line in probe.read_text().splitlines() if probe.exists() else []:
            parts = line.split()
            if len(parts) == 3 and parts[0].startswith("n=") and parts[2] == parts[0][2:]:
                n = int(parts[0][2:])
                rates[n] = n * 8 / float(parts[1])  # replica-steps per second
        self.result["probe_replica_steps_per_s"] = rates
        if not rates:
            self.api.log("gpu_gate", rates=rates)
            self.result["gate_rejected"] = True
            return 125
        workers = max(rates, key=lambda n: (rates[n], -n))
        if self.a.gpu_workers:  # memory cap: 16 workers OOMed M>=16 jobs on 32 GB (r3)
            workers = self.a.gpu_workers
            rates[workers] = min(rates.values())
        hours = GRID_REPLICA_STEPS * GRID_OVERHEAD / rates[workers] / 3600
        left = (self.deadline - time.time()) / 3600
        self.result |= {"workers": workers, "projected_hours": round(hours, 2)}
        self.api.log("gpu_gate", rates=rates, workers=workers, hours=hours, left=left)
        if hours > min(left, 11.5):
            self.result["gate_rejected"] = True
            return 125
        remote = REMOTE.replace(
            "PYTHONHASHSEED=0",
            f"PYTHONHASHSEED=0 HYPERTRAIN_SIM_DEVICE=cuda HYPERTRAIN_SIM_BATCHED=1 {MPS_ENV} "
            "PYTORCH_CUDA_ALLOC_CONF=expandable_segments:True",
        ).replace("--workers 64", f"--workers {workers}")
        launch = f"setsid nohup bash -c {vast.shlex.quote(remote)} >/dev/null 2>&1 < /dev/null &"
        if self.sh(launch, timeout=120) != 0:
            raise RuntimeError("remote launch failed")
        self.api.log("grid_launched", workers=workers)
        return self.poll_grid()

    def poll_grid(self) -> int:
        status, part = self.out / "exit.txt", self.out / "partial_parity_runs.jsonl"
        tmp = part.with_suffix(".tmp")
        while time.time() < self.deadline and not self.cancel.is_set():
            self.api.sleep(float(self.a.poll_seconds))
            rc = self.sh("cat /workspace/out/parity_runs.jsonl", timeout=120, stdout_path=tmp)
            if rc == 0 and (not part.exists() or tmp.stat().st_size >= part.stat().st_size):
                tmp.replace(part)  # never let a failed pull shrink the local copy
            self.sh("cat /workspace/out/EXIT 2>/dev/null", timeout=60, stdout_path=status)
            text = status.read_text().strip() if status.exists() else ""
            if text:
                self.api.log("grid_exit", rc=text)
                return int(text)
        return 124

    def admit(self) -> dict[str, Any]:
        self.offer = super().admit()
        return self.offer


def main(argv: list[str] | None = None) -> int:
    argv = list(sys.argv[1:] if argv is None else argv)
    if "--evidence-dir" not in argv:
        argv += ["--evidence-dir", str(EVIDENCE)]
    opts = {
        "--poll-seconds": "300",
        "--resume-dir": "",
        "--exclude-machines": "",
        "--gate-s-per-step": "2.5",
        "--cpu-regex": "",
        "--probe": "0",
        "--gpu": "0",
        "--equiv-cpu": "",
        "--gpu-workers": "0",
    }
    for flag in opts:
        if flag in argv:
            i = argv.index(flag)
            opts[flag] = argv[i + 1]
            del argv[i : i + 2]
    resume = Path(opts["--resume-dir"]).resolve() if opts["--resume-dir"] else None
    if resume is not None:
        if not (resume / "parity_runs.jsonl").is_file():
            print("--resume-dir lacks parity_runs.jsonl", file=sys.stderr)
            return 2
        if resume.name != "resume":
            print("--resume-dir must be a directory named 'resume'", file=sys.stderr)
            return 2

    base = GpuParityHarness if opts["--gpu"] == "1" else ParityHarness

    class Bound(base):  # type: ignore[valid-type,misc]
        upload_paths = ParityHarness.upload_paths + ([str(resume)] if resume else [])

        def __init__(self, args: Any, api: Any) -> None:
            args.poll_seconds = float(opts["--poll-seconds"])
            args.gate_s_per_step = float(opts["--gate-s-per-step"])
            args.cpu_regex = opts["--cpu-regex"]
            args.probe = opts["--probe"] == "1"
            args.equiv_cpu = opts["--equiv-cpu"]
            args.gpu_workers = int(opts["--gpu-workers"])
            args.exclude_machines = {
                int(x) for x in opts["--exclude-machines"].split(",") if x.strip()
            }
            super().__init__(args, api)

    return vast.main(argv, harness=Bound)


if __name__ == "__main__":
    sys.exit(main())
