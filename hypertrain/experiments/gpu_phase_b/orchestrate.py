"""Phase B (todo 16) on the todo-12 lifecycle: same journal, admission gate, single CREATE per
host, rescue-before-DELETE, parsed absence, deadline supervisor. Only the Phase-specific parts
are overridden: 8-GPU offers, staged files, per-host jobs (run in parallel across hosts), the
optional network measurements and the evidence/verdict.

Usage: orchestrate.py run --config CFG.json --run-dir DIR [--live]
       orchestrate.py supervise DIR            (spawned by the launcher; never by hand)
"""

from __future__ import annotations

import argparse
import io
import json
import os
import re
import select
import shlex
import subprocess
import sys
import tarfile
import time
import urllib.parse
from concurrent.futures import ThreadPoolExecutor
from decimal import Decimal
from pathlib import Path
from typing import Any

from hypertrain.gpu_ops import budget, launcher, provider, supervisor
from hypertrain.gpu_ops.journal import Journal, Record, durable_write, flock, fsha
from hypertrain.gpu_ops.launcher import Orchestrator, Reject, shard_path

HERE = Path(__file__).resolve().parent
REMOTE_SHARD = "data/phase-b/shard.u32"
IPERF_PORT = 5201
WATCH_RE = "OutOfMemoryError|out of memory|Watchdog caught|collective operation timeout"
WATCH_SECONDS = 5
ARMS = ("det", "base")
SAME_KEYS = (
    "leaves",
    "leaves_root",
    "delta_hash",
    "final_theta_hash",
    "topk_index_sha256",
    "topk_value_sha256",
)
# Vast PUT /asks/ responses carry per-instance secrets; mask them in every raw log (todo-12 F/U).
provider._REDACT.append(
    (re.compile(r'("(?:instance_api_key|jupyter_token)"\s*:\s*")[^"]*'), r"\1***")
)


def stage_files(tree: Path, cfg: dict[str, Any]) -> dict[str, Path]:
    files = {
        str(p.relative_to(tree)): p
        for p in sorted((tree / "src/hypertrain").rglob("*.py"))
        if "__pycache__" not in p.parts
    }
    for rel in (
        "experiments/gpu_phase_b/run_phase_b.py",
        "experiments/gpu_phase_b/nccl_bench.py",
        "experiments/gpu_phase_b/phase_b.json",
        "docker/phase-a/requirements.txt",
    ):
        files[rel] = tree / rel
    files[REMOTE_SHARD] = shard_path(cfg)
    return files


def iperf_endpoint(show: Any) -> tuple[str, int] | None:
    """Public (ip, host port) for container port 5201/tcp from a Vast instance record."""
    if not isinstance(show, dict):
        return None
    ip, ports = show.get("public_ipaddr"), show.get("ports")
    maps = ports.get(f"{IPERF_PORT}/tcp") if isinstance(ports, dict) else None
    if not isinstance(ip, str) or not ip.strip() or not isinstance(maps, list):
        return None
    for m in maps:
        port = str(m.get("HostPort", "")) if isinstance(m, dict) else ""
        if port.isdigit() and 0 < int(port) < 65536:
            return ip.strip(), int(port)
    return None


def compute_verdict(
    det: dict[str, dict[str, Any] | None], machines: dict[str, Any], n_gpus: int
) -> dict[str, Any]:
    """Cross-host bitwise verdict over the det arm (rank 0 JSON per host)."""
    missing = [r for r, v in det.items() if not v or "leaves_root" not in v]
    if missing:
        return {"verdict": "CENSORED", "reason": "missing det runs: " + ",".join(missing)}
    rs = {r: v for r, v in det.items() if v}
    off = [
        r
        for r, v in rs.items()
        if v["env"].get("device") != "cuda"
        or v["env"].get("sm_counts") != [170]
        or v["env"].get("device_count") != n_gpus
        or v.get("n_gpus") != n_gpus
        or v.get("ranks_agree") is not True
    ]
    roles = sorted(rs)
    ref = rs[roles[0]]
    mismatches: list[dict[str, Any]] = [
        {"role": r, "fields": [k for k in SAME_KEYS if rs[r].get(k) != ref.get(k)]}
        for r in roles[1:]
        if any(rs[r].get(k) != ref.get(k) for k in SAME_KEYS)
    ]
    for m in mismatches:
        a, b = ref["leaves"], rs[m["role"]]["leaves"]
        m["first_divergent_leaf"] = next(
            (i for i, (x, y) in enumerate(zip(a, b, strict=False)) if x != y), None
        )
    distinct = len({machines.get(r) for r in roles} - {None})
    if off:
        verdict, reason = "CENSORED", "off hardware/layout gate: " + ",".join(off)
    elif len(roles) < 2 or distinct != len(roles):
        verdict, reason = "CENSORED", "need >=2 hosts with distinct machine_ids"
    else:
        verdict, reason = ("FAIL" if mismatches else "PASS"), None
    return {
        "verdict": verdict,
        "reason": reason,
        "reference_role": roles[0],
        "reference_leaves_root": ref["leaves_root"],
        "reference_delta_hash": ref["delta_hash"],
        "mismatches": mismatches,
        "off_hardware_gate": off,
        "distinct_machine_ids": distinct,
    }


def overhead(det: dict[str, Any], base: dict[str, Any]) -> dict[str, Any]:
    """tok/s(base) / tok/s(det) per host; PASS <= 1.5, WATCH (1.5, 1.6], REPLAN > 1.6."""
    per = {
        r: round(base[r]["tok_per_s"] / det[r]["tok_per_s"], 4)
        for r in det
        if (det.get(r) or {}).get("tok_per_s") and (base.get(r) or {}).get("tok_per_s")
    }
    if not per or len(per) != len(det):
        return {"verdict": "CENSORED", "per_host_ratio": per, "max_ratio": None}
    worst = max(per.values())
    status = "PASS" if worst <= 1.5 else "WATCH" if worst <= 1.6 else "REPLAN"
    return {"verdict": status, "per_host_ratio": per, "max_ratio": worst}


class PhaseBOrchestrator(Orchestrator):
    def __init__(
        self,
        cfg: dict[str, Any],
        j: Journal,
        p: provider.Provider,
        run_dir: Path,
        actor: str = "launcher",
    ) -> None:
        super().__init__(cfg, j, p, run_dir, actor)
        create_env = cfg.get("create_env")
        if create_env:
            plain = p.call

            def call(
                method: str, path: str, tag: str, body: Any = None, timeout: float = 30
            ) -> provider.Response:
                if method == "PUT" and path.startswith("/api/v0/asks/") and isinstance(body, dict):
                    body = {**body, "env": dict(create_env)}
                return plain(method, path, tag, body, timeout)

            p.call = call  # type: ignore[method-assign]

    def offer(self, role: str, tag: str) -> dict[str, Any]:
        h, n = self.host[role], int(self.cfg["num_gpus"])
        q = {
            "machine_id": {"eq": h["machine_id"]},
            "rentable": {"eq": True},
            "rented": {"eq": False},
        }
        r = self.p.call(
            "GET", "/api/v0/bundles/?" + urllib.parse.urlencode({"q": json.dumps(q)}), tag
        )
        offers = r.parsed.get("offers") if r.ok() else None
        if not isinstance(offers, list):
            raise Reject(f"offer_unknown:{role}:HTTP{r.status}")
        cap = Decimal(str(h["max_dph_total"]))
        fit = [
            o
            for o in offers
            if isinstance(o, dict)
            and o.get("machine_id") == h["machine_id"]
            and o.get("gpu_name") == "RTX 5090"
            and o.get("num_gpus") == n
            and type(o.get("id")) is int
            and (d := budget.dec(o.get("dph_total"))) is not None
            and d <= cap
        ]
        self.j.append(
            "offer_resolved", role=role, seen=len(offers), eligible=[o["id"] for o in fit]
        )
        if not fit:
            raise Reject(f"offer_gone:{role}")
        return min(fit, key=lambda o: (Decimal(str(o["dph_total"])), o["id"]))

    def ensure_supervisor(self) -> Record:
        try:
            with flock(self.dir / "supervisor.lock", blocking=False):
                pass
        except BlockingIOError:
            ready = self.j.last("supervisor_ready")
            assert ready is not None
            return ready
        err = open(self.dir / "supervisor.stderr", "ab")
        proc = subprocess.Popen(  # noqa: S603  (this interpreter + this script)
            [sys.executable, "-B", str(Path(__file__).resolve()), "supervise", str(self.dir)],
            stdout=subprocess.PIPE,
            stderr=err,
            stdin=subprocess.DEVNULL,
            start_new_session=True,
            env=os.environ.copy(),
        )
        err.close()
        assert proc.stdout is not None
        if not select.select([proc.stdout], [], [], 60)[0]:
            raise Reject("supervisor_not_ready")
        line = proc.stdout.readline().decode().strip()
        proc.stdout.close()
        ready = self.j.last("supervisor_ready")
        if line != "READY" or not ready or ready["supervisor_pid"] != proc.pid:
            raise Reject("supervisor_not_ready:" + line[:100])
        return ready

    def stage_tar(self) -> Path:
        out = self.dir / "stage.tar"
        if out.exists():
            return out
        files = stage_files(Path(self.cfg["tree"]), self.cfg)
        buf = io.BytesIO()
        with tarfile.open(fileobj=buf, mode="w") as tar:
            for rel, src in sorted(files.items()):
                data = src.read_bytes()
                info = tarfile.TarInfo(rel)
                info.size, info.mode, info.mtime = len(data), 0o644, 0
                tar.addfile(info, io.BytesIO(data))
        durable_write(out, buf.getvalue())
        self.j.append("stage_built", tar_sha256=fsha(out), files=len(files))
        return out

    def jobs(self) -> list[tuple[str, str, bool]]:
        return [("*", "phase_b", False)]

    def _guard(self) -> None:
        if time.time() > self.deadline() - float(self.cfg.get("cleanup_margin_seconds", 900)):
            raise Reject("deadline_margin_reached")
        if self.cleanup_started():
            raise Reject("supervisor_cleanup_started")

    def _remote(self, role: str, name: str, argv: list[str]) -> str | None:
        if done := self.j.last("job_done", role=role, name=name):
            return str(done["exit"])
        self._guard()
        full = [
            "env",
            "CUBLAS_WORKSPACE_CONFIG=:4096:8",
            "CUDA_DISABLE_PTX_JIT=1",
            "PYTHONPATH=src",
            "PYTHONDONTWRITEBYTECODE=1",
            "OMP_NUM_THREADS=1",
            "PYTORCH_CUDA_ALLOC_CONF=expandable_segments:True",
            *argv,
        ]
        full[1:1] = [f"{k}={v}" for k, v in sorted(self.cfg.get("remote_env", {}).items())]
        self.j.append("job_started", role=role, name=name, argv=full)
        q = shlex.quote
        log = f"out/{name}.log"
        # Log-watch: on OOM / NCCL watchdog text, kill torchrun (it tears down its ranks) and
        # report exit 97 at once instead of waiting for the NCCL timeout.
        cmd = (
            f"cd {q(self.remote_root(role))} && {{ {shlex.join(full)} > {log} 2>&1 & p=$!; w=0;"
            f" while kill -0 $p 2>/dev/null; do if [ $w = 0 ] && grep -qE {q(WATCH_RE)} {log};"
            f" then w=1; echo WATCH_KILL >> {log}; kill $p; fi; sleep {WATCH_SECONDS}; done;"
            f" wait $p; rc=$?; [ $w = 1 ] && rc=97; echo EXIT=$rc; }}"
        )
        proc = self.ssh(role).run(cmd, "job-" + name, self.dir / "logs")
        code = proc.stdout.strip().rsplit("EXIT=", 1)[-1] if "EXIT=" in proc.stdout else None
        self.j.append("job_done", role=role, name=name, ssh_rc=proc.returncode, exit=code)
        return code

    def _torchrun(self, script: str, *args: str) -> list[str]:
        return [
            self.cfg.get("remote_python", "python3"),
            "-m",
            "torch.distributed.run",
            "--standalone",
            "--nproc-per-node",
            str(self.cfg["num_gpus"]),
            f"experiments/gpu_phase_b/{script}",
            *args,
        ]

    def host_jobs(self, role: str) -> None:
        """probe, det, base; any nonzero exit aborts -> rescue + DELETE now.

        probe = the full det round (H steps, leaves, outer compress, real dims) with the per-rank
        85% memory gate; it is run first so a memory fault surfaces before the timed arms, and
        its hashes double as an intra-host repeat of det.
        """
        for arm in ("probe", *ARMS):
            argv = self._torchrun(
                "run_phase_b.py",
                "--arm",
                "det" if arm == "probe" else arm,
                "--profile",
                self.cfg["profile"],
                "--device",
                self.cfg.get("device", "cuda"),
                "--shard",
                REMOTE_SHARD,
                "--shard-sha256",
                fsha(shard_path(self.cfg)),
                "--out",
                f"out/{arm}.json",
            )
            if "@" in self.cfg["image"]:
                argv += ["--image-digest", self.cfg["image"].rsplit("@", 1)[1]]
            code = self._remote(role, arm, argv)
            if code != "0":
                raise Reject(f"remote_exit:{role}:{arm}:{code}")

    def net_jobs(self) -> None:
        """Optional: only if time before the admitted deadline covers it (cost already admitted)."""
        need = float(self.cfg.get("cleanup_margin_seconds", 900)) + float(
            self.cfg.get("net_budget_seconds", 600)
        )
        if not self.cfg.get("net_measurements") or time.time() > self.deadline() - need:
            self.j.append("net_skipped", reason="disabled_or_no_time")
            return
        dev = self.cfg.get("device", "cuda")
        for role in self.roles:
            self._remote(
                role,
                "nccl",
                self._torchrun("nccl_bench.py", "--device", dev, "--out", "out/nccl.json"),
            )
        server, clients = self.roles[0], self.roles[1:]
        rec = self.receipt(server)
        assert rec is not None
        _, shown = self.show(rec["instance_id"], "iperf-endpoint")
        ep = iperf_endpoint(shown)
        self.j.append("iperf_endpoint", role=server, endpoint=ep)
        if ep is None:
            return
        if (
            self._remote(server, "iperf_server", ["sh", "-c", f"iperf3 -s -D -p {IPERF_PORT}"])
            != "0"
        ):
            return
        for c in clients:
            for cc in ("cubic", "bbr"):
                for par in (1, 8, 32):
                    self._remote(
                        c,
                        f"iperf-{server}-{cc}-P{par}",
                        [
                            "sh",
                            "-c",
                            f"iperf3 -J -c {ep[0]} -p {ep[1]} -t 10 -P {par} -C {cc}"
                            f" > out/iperf-{server}-{cc}-P{par}.json",
                        ],
                    )

    def run_job(self, role: str, name: str, negative: bool) -> None:
        with ThreadPoolExecutor(len(self.roles)) as ex:
            futures = [ex.submit(self.host_jobs, r) for r in self.roles]
            errors = [e for f in futures if (e := f.exception()) is not None]
        if errors:
            raise errors[0]
        self.net_jobs()

    def _rescued(self, role: str, name: str) -> dict[str, Any] | None:
        f = self.dir / "rescue" / role / name
        try:
            v = json.loads(f.read_text())
        except (OSError, ValueError):
            return None
        return v if isinstance(v, dict) else None

    def evidence(self, outcome: str, credit: Any, inv: dict[str, Any]) -> dict[str, Any]:
        snap = json.loads(Path(self.cfg["budget_snapshot"]).read_text())
        ready = {r: self.j.last("instance_ready", role=r) or {} for r in self.roles}
        machines = {r: ready[r].get("machine_id") for r in self.roles}
        runs = {a: {r: self._rescued(r, f"{a}.json") for r in self.roles} for a in ARMS}
        verdict = compute_verdict(runs["det"], machines, int(self.cfg["num_gpus"]))
        if outcome != "completed" and verdict["verdict"] == "PASS":
            verdict = {**verdict, "verdict": "CENSORED", "reason": outcome}
        iperf = {
            f.stem: _iperf_summary(f)
            for r in self.roles
            for f in sorted((self.dir / "rescue" / r).glob("iperf-*.json"))
        }
        cap = min(budget.GLOBAL_CAP_USD, Decimal(str(snap["credit_usd"]))) - Decimal(
            str(self.cfg["phase_a_settled_debit_usd"])
        )
        debit = None
        if credit is not None:
            whole = max(Decimal(0), Decimal(str(snap["credit_usd"])) - Decimal(str(credit)))
            debit = str(
                max(Decimal(0), whole - Decimal(str(self.cfg["phase_a_settled_debit_usd"])))
            )
        admitted = self.admitted() or {}
        return {
            "task": "todo 16 Phase B cross-host 8x5090 island round",
            "phase": "B",
            "outcome": outcome,
            **verdict,
            "overhead": overhead(runs["det"], runs["base"]),
            "probe": {
                r: {
                    k: (self._rescued(r, "probe.json") or {}).get(k)
                    for k in ("leaves_root", "delta_hash", "per_rank", "ranks_over_mem_limit")
                }
                for r in self.roles
            },
            "intra_host_repeat": {
                r: (self._rescued(r, "probe.json") or {}).get("delta_hash") is not None
                and (self._rescued(r, "probe.json") or {}).get("leaves_root")
                == (runs["det"][r] or {}).get("leaves_root")
                and (self._rescued(r, "probe.json") or {}).get("delta_hash")
                == (runs["det"][r] or {}).get("delta_hash")
                for r in self.roles
            },
            "image": self.cfg["image"],
            "image_digest": self.cfg["image"].rsplit("@", 1)[-1]
            if "@" in self.cfg["image"]
            else None,
            "staging": self.cfg.get("staging"),
            "stage_tar_sha256": (self.j.last("stage_built") or {}).get("tar_sha256"),
            "runs": [
                {"role": r, "arm": a, "machine_id": machines[r], "result": runs[a][r]}
                for a in ARMS
                for r in self.roles
            ],
            "hosts": [
                {
                    "role": r,
                    **{k: ready[r].get(k) for k in ("instance_id", "machine_id", "driver_version")},
                }
                for r in self.roles
            ],
            "network": {
                "nccl": {r: self._rescued(r, "nccl.json") for r in self.roles} or "CENSORED",
                "iperf": iperf or "CENSORED",
                "iperf_endpoint": (self.j.last("iperf_endpoint") or {}).get("endpoint"),
                "skipped": bool(self.j.last("net_skipped")),
            },
            "admission": {
                k: admitted.get(k) or (self.j.last("admission_rejected") or {}).get(k)
                for k in ("plan", "quotes")
            }
            | {"failure": (self.j.last("admission_failed") or {}).get("reason")},
            "cost": {
                "cap_usd": str(cap),
                "basis": "snapshot-0 credit minus final credit minus settled Phase A debit"
                " (prepaid, conservative)",
                "lines": [{"item": "whole_debit_phase_b", "usd": debit}],
                "total_usd": debit,
            },
            "inventory": inv,
            "journal": str(self.j.path),
        }


def _iperf_summary(f: Path) -> dict[str, Any]:
    try:
        d = json.loads(f.read_text())
        end = d["end"]
        return {
            "sent_Gbps": round(end["sum_sent"]["bits_per_second"] / 1e9, 4),
            "received_Gbps": round(end["sum_received"]["bits_per_second"] / 1e9, 4),
            "retransmits": end["sum_sent"].get("retransmits"),
            "congestion": end.get("sender_tcp_congestion"),
        }
    except (OSError, ValueError, KeyError, TypeError):
        return {"verdict": "CENSORED", "reason": "iperf output missing or malformed"}


def main(argv: list[str]) -> int:
    # The todo-12 entry points look Orchestrator up at call time; point them at Phase B.
    launcher.Orchestrator = PhaseBOrchestrator  # type: ignore[misc]
    supervisor.Orchestrator = PhaseBOrchestrator  # type: ignore[misc]
    if argv[1:2] == ["supervise"]:
        return supervisor.main(argv[2])
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    r = sub.add_parser("run")
    r.add_argument("--config", required=True)
    r.add_argument("--run-dir", required=True)
    r.add_argument("--live", action="store_true")
    return launcher.run(ap.parse_args(argv[1:]))


if __name__ == "__main__":
    sys.exit(main(sys.argv))
