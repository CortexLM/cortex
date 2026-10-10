"""Manifest-driven torchrun parent. Publish only complete, independently checked rank artifacts."""

from __future__ import annotations

import hashlib
import json
import os
import shlex
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import threading
import time
import uuid
from collections.abc import Callable, Mapping
from dataclasses import dataclass
from pathlib import Path
from typing import Literal

from hypertrain.gpu_ops.journal import durable_write, flock
from hypertrain.protocol.envelope_v2 import load_json
from hypertrain.protocol.hashing import MerkleTree
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages import LeafPreimage
from hypertrain.protocol.messages_v2 import IslandJobV1, RankWorkResult


class IslandFailure(RuntimeError):
    """No artifact may be committed after this failure."""


@dataclass(frozen=True, slots=True)
class IslandArtifacts:
    directory: Path
    state: Path
    ef: Path
    delta: Path
    leaves: Path
    ranks: tuple[RankWorkResult, ...]


@dataclass(frozen=True, slots=True)
class CapacityAttempt:
    """Internal owner-validated identity, shared lock and committed ledger debit seam."""

    identity: str
    profile_hash: str
    lock_path: Path
    charge: Callable[[], bool]
    cpu_quota: Literal["50%", "400%"] | None = None


_CAPACITY_MEMORY = 1 << 30
_CAPACITY_CPU_PERCENT = 400
_CAPACITY_EXEC = """import json,os,socket,sys,time
c=json.load(open(sys.argv[1]))
g=open('/proc/self/cgroup').read().strip().split('::',1)[1]
p='/sys/fs/cgroup'+g
names=('memory.max','memory.swap.max','cpu.max','memory.oom.group')
v={n:open(p+'/'+n).read().strip() for n in names}
v.update(cgroup=g,affinity=sorted(os.sched_getaffinity(0)),nice=os.getpriority(os.PRIO_PROCESS,0),pid=os.getpid())
s=socket.socket(socket.AF_UNIX);s.connect(c['socket']);s.sendall(json.dumps(v).encode()+b'\\n')
assert s.recv(1)==b'G'
s.close()
assert time.time()<c['deadline']
os.execve(c['argv'][0],c['argv'],c['env'])
"""
_CAPACITY_STOP = """import os,sys
g=open('/proc/self/cgroup').read().strip().split('::',1)[1]
events=open('/sys/fs/cgroup'+g+'/memory.events').read()
with open(sys.argv[1],'x') as f:
 f.write(events);f.flush();os.fsync(f.fileno())
"""


def _manager(*argv: str, check: bool = True) -> str:
    try:
        result = subprocess.run(  # noqa: S603 - fixed manager argv and owned unit names
            argv, capture_output=True, text=True, timeout=15, check=False
        )
    except OSError as exc:
        if check:
            raise IslandFailure("capacity manager unavailable") from exc
        return "manager-unavailable"
    if check and result.returncode:
        raise IslandFailure(f"capacity manager failure: {result.stderr[-2000:]}")
    return result.stdout.strip()


def run_capacity_argv(
    argv: list[str],
    directory: Path,
    deadline: int,
    capacity: CapacityAttempt,
    *,
    env: Mapping[str, str],
    cancel: threading.Event | None = None,
) -> None:
    """One real manager-owned attempt. Never reset debit/deadline or retry on restart."""
    cpu_quota = capacity.cpu_quota or f"{_CAPACITY_CPU_PERCENT}%"
    cpu_max = f"{int(cpu_quota.removesuffix('%')) * 1000} 100000"
    if len(capacity.identity) != 64 or len(capacity.profile_hash) != 64:
        raise IslandFailure("capacity identity binding")
    try:
        bytes.fromhex(capacity.identity + capacity.profile_hash)
    except ValueError as exc:
        raise IslandFailure("capacity identity binding") from exc
    directory.mkdir(parents=True, exist_ok=True)
    with flock(capacity.lock_path):
        if time.time() >= deadline or cancel is not None and cancel.is_set():
            raise IslandFailure("capacity deadline/cancellation")
        record = directory / "capacity-attempt.json"
        if record.exists():
            saved = load_json(record.read_bytes())
            if not isinstance(saved, dict) or (
                saved.get("identity") != capacity.identity
                or saved.get("profile_hash") != capacity.profile_hash
                or saved.get("cpu_quota") != cpu_quota
            ):
                raise IslandFailure("capacity recovery identity differs")
            unit = str(saved["unit"])
            base = unit.removesuffix(".service")
            if not base.startswith("ht-cap-") or len(base.removeprefix("ht-cap-")) != 32:
                raise IslandFailure("capacity recovery unit differs")
            bytes.fromhex(base.removeprefix("ht-cap-"))
            if (
                saved["timer"] != base + "-deadline.timer"
                or saved["timer_service"] != base + "-deadline.service"
            ):
                raise IslandFailure("capacity recovery timer differs")
            names = (unit, base + "-deadline.timer", base + "-deadline.service")
            if not (directory / "capacity-cleaned.json").exists():
                status = _manager(
                    "systemctl", "show", unit, "-p", "Result", "-p", "ExecMainStatus", check=False
                )
                durable_write(
                    directory / "capacity-recovery.json",
                    canonicalize({"status": status, "unit": unit}),
                )
                for name in names:
                    _manager("systemctl", "stop", name, check=False)
                _manager("systemctl", "reset-failed", *names, check=False)
                for name in names:
                    state = _manager("systemctl", "show", name, "-p", "ActiveState", check=False)
                    if "ActiveState=active" in state or "ActiveState=activating" in state:
                        raise IslandFailure("capacity recovery cleanup incomplete")
                durable_write(
                    directory / "capacity-cleaned.json", canonicalize({"units": list(names)})
                )
            raise IslandFailure("capacity attempt already recorded; recovery required")
        if not capacity.charge():
            raise IslandFailure("capacity attempt already charged; recovery required")
        unit = "ht-cap-" + uuid.uuid4().hex + ".service"
        timer_service = unit.removesuffix(".service") + "-deadline.service"
        timer = timer_service.removesuffix(".service") + ".timer"
        durable_write(
            record,
            canonicalize(
                {
                    "identity": capacity.identity,
                    "profile_hash": capacity.profile_hash,
                    "cpu_quota": cpu_quota,
                    "deadline": deadline,
                    "unit": unit,
                    "timer": timer,
                    "timer_service": timer_service,
                }
            ),
        )
        observed: dict[str, str | int | list[int]] = {}
        status = ""
        proc: subprocess.Popen[bytes] | None = None
        memory_events: int | None = None
        try:
            with tempfile.TemporaryDirectory(prefix="ht-cap-ipc-") as tmp:
                address = str(Path(tmp) / "ready.sock")
                with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as listener:
                    listener.bind(address)
                    listener.listen(1)
                    listener.settimeout(min(15, max(0.001, deadline - time.time())))
                    config = directory / "capacity-exec.json"
                    durable_write(
                        config,
                        canonicalize(
                            {
                                "socket": address,
                                "argv": argv,
                                "env": dict(env),
                                "deadline": deadline,
                                "cpu_quota": cpu_quota,
                            }
                        ),
                    )
                    _manager(
                        "systemd-run",
                        f"--unit={timer_service}",
                        f"--on-calendar=@{deadline}",
                        "--timer-property=AccuracySec=1us",
                        "--timer-property=RandomizedDelaySec=0",
                        "--property=RemainAfterExit=yes",
                        "--property=Nice=19",
                        "/usr/bin/systemctl",
                        "kill",
                        "--kill-whom=all",
                        "--signal=SIGKILL",
                        unit,
                    )
                    properties = [
                        "Type=exec",
                        f"MemoryMax={_CAPACITY_MEMORY}",
                        "MemorySwapMax=0",
                        f"CPUQuota={cpu_quota}",
                        "CPUQuotaPeriodSec=100ms",
                        "CPUAffinity=0 1 2 3",
                        "Nice=19",
                        "OOMPolicy=kill",
                        "KillMode=control-group",
                        "KillSignal=SIGKILL",
                        "Restart=no",
                        "TimeoutStopSec=1s",
                        "NoNewPrivileges=yes",
                        "CapabilityBoundingSet=",
                        "ProtectControlGroups=yes",
                        "RuntimeMaxSec=600s",
                        "ExecStopPost="
                        + shlex.join(
                            [
                                sys.executable,
                                "-c",
                                _CAPACITY_STOP,
                                str((directory / "capacity-memory-events.txt").resolve()),
                            ]
                        ),
                    ]
                    with (
                        open(directory / "capacity-stdout.log", "xb") as stdout,
                        open(directory / "capacity-stderr.log", "xb") as stderr,
                    ):
                        executable = shutil.which("systemd-run")
                        if executable is None:
                            raise IslandFailure("capacity manager unavailable")
                        proc = subprocess.Popen(  # noqa: S603 - fixed manager/package wrapper
                            [
                                executable,
                                f"--unit={unit}",
                                "--wait",
                                "--pipe",
                                "--expand-environment=no",
                                *["--property=" + p for p in properties],
                                sys.executable,
                                "-c",
                                _CAPACITY_EXEC,
                                str(config.resolve()),
                            ],
                            stdout=stdout,
                            stderr=stderr,
                        )
                        connection, _ = listener.accept()
                        with connection:
                            connection.settimeout(5)
                            raw = bytearray()
                            while not raw.endswith(b"\n"):
                                chunk = connection.recv(4096)
                                if not chunk or len(raw) + len(chunk) > 8192:
                                    raise IslandFailure("capacity readiness malformed")
                                raw.extend(chunk)
                            observed = json.loads(raw)
                            if (
                                observed["memory.max"] != str(_CAPACITY_MEMORY)
                                or observed["memory.swap.max"] != "0"
                                or observed["cpu.max"] != cpu_max
                                or observed["memory.oom.group"] != "1"
                                or observed["affinity"] != [0, 1, 2, 3]
                                or observed["nice"] != 19
                                or not str(observed["cgroup"]).endswith("/" + unit)
                            ):
                                raise IslandFailure("capacity kernel settings differ")
                            group = Path("/sys/fs/cgroup") / str(observed["cgroup"]).lstrip("/")
                            memory_events = os.open(group / "memory.events", os.O_RDONLY)
                            durable_write(
                                directory / "capacity-observed.json", canonicalize(observed)
                            )
                            if time.time() >= deadline or cancel is not None and cancel.is_set():
                                raise IslandFailure("capacity deadline/cancellation")
                            connection.sendall(b"G")
                        done = threading.Event()
                        condition = vars(cancel)["_cond"] if cancel is not None else None

                        def cancelled() -> None:
                            assert cancel is not None and condition is not None
                            with condition:
                                condition.wait_for(
                                    lambda: done.is_set() or cancel.is_set(),
                                    timeout=max(0, deadline - time.time()),
                                )
                            if not done.is_set():
                                _manager(
                                    "systemctl",
                                    "kill",
                                    "--kill-whom=all",
                                    "--signal=SIGKILL",
                                    unit,
                                    check=False,
                                )

                        watcher = None
                        if cancel is not None:
                            watcher = threading.Thread(target=cancelled, name="capacity-cancel")
                            watcher.start()
                        try:
                            proc.wait(timeout=max(0.001, deadline - time.time()) + 5)
                        finally:
                            done.set()
                            if condition is not None:
                                with condition:
                                    condition.notify_all()
                            if watcher is not None:
                                watcher.join()
                        status = _manager(
                            "systemctl",
                            "show",
                            unit,
                            "-p",
                            "Result",
                            "-p",
                            "ExecMainStatus",
                            "-p",
                            "MemoryPeak",
                            check=False,
                        )
                        if (
                            proc.returncode != 0
                            or cancel is not None
                            and cancel.is_set()
                            or time.time() >= deadline
                        ):
                            raise IslandFailure("capacity execution failed: " + status)
        except (OSError, subprocess.SubprocessError, ValueError, KeyError) as exc:
            raise IslandFailure("capacity launch failed closed: " + str(exc)) from exc
        finally:
            if not status:
                status = _manager(
                    "systemctl", "show", unit, "-p", "Result", "-p", "ExecMainStatus", check=False
                )
            if memory_events is not None:
                try:
                    os.lseek(memory_events, 0, os.SEEK_SET)
                    status += "\n" + os.read(memory_events, 8192).decode()
                except OSError as exc:
                    status += "\nmemory events unavailable: " + str(exc)
                finally:
                    os.close(memory_events)
            counters_path = directory / "capacity-memory-events.txt"
            if counters_path.exists():
                status += "\n" + counters_path.read_text()
            durable_write(
                directory / "capacity-result.json",
                canonicalize({"observed": observed, "status": status}),
            )
            for name in (unit, timer, timer_service):
                _manager("systemctl", "stop", name, check=False)
            _manager("systemctl", "reset-failed", unit, timer, timer_service, check=False)
            if proc is not None and proc.poll() is None:
                proc.wait(timeout=15)
            for name in (unit, timer, timer_service):
                result = _manager("systemctl", "show", name, "-p", "ActiveState", check=False)
                if (
                    "manager-unavailable" in result
                    or "ActiveState=active" in result
                    or "ActiveState=activating" in result
                ):
                    raise IslandFailure("capacity cleanup incomplete: " + name)
            group = Path("/sys/fs/cgroup") / str(observed.get("cgroup", "nonexistent")).lstrip("/")
            if group.exists() and (group / "cgroup.procs").read_text().strip():
                raise IslandFailure("capacity descendants survived")
            durable_write(
                directory / "capacity-cleaned.json",
                canonicalize({"units": [unit, timer, timer_service]}),
            )


def gpu_uuid(raw: object) -> str:
    """nvidia-smi form `GPU-<36 hex/dash>`; torch's _CUuuid str omits the `GPU-` prefix."""
    text = str(raw).strip().lower().removeprefix("gpu-")
    if len(text) != 36 or text.count("-") != 4:
        raise IslandFailure(f"malformed GPU UUID {raw!r}")
    bytes.fromhex(text.replace("-", ""))
    return "GPU-" + text


def confined(directory: Path, relative: str) -> Path:
    path = (directory / relative).resolve(strict=True)
    if not path.is_relative_to(directory.resolve()) or not path.is_file():
        raise IslandFailure(f"object escapes attempt directory: {relative}")
    return path


def launch_argv(job: IslandJobV1, attempt: Path, backend: Literal["cpu", "cuda"]) -> list[str]:
    """No fixed logical-rank ceiling; parent interpreter is also the worker interpreter."""
    return [
        sys.executable,
        "-m",
        "torch.distributed.run",
        "--standalone",
        f"--nproc-per-node={job.manifest.training.reference_spec.layout.n_gpus}",
        "--max-restarts=0",
        "-m",
        "hypertrain.miner.island_worker",
        str(attempt),
        backend,
    ]


def validate_artifacts(job: IslandJobV1, directory: Path) -> IslandArtifacts:
    """Recompute state, leaf, EF and byte commitments, not just rank-reported summaries."""
    import hypertrain.trainer  # noqa: F401
    from hypertrain.auditor.replay import tensor_root, unpack_state
    from hypertrain.trainer.compress import compress, state_hash
    from hypertrain.trainer.config import TrainConfig
    from hypertrain.trainer.island import IslandAssignment
    from hypertrain.trainer.loop import _make_leaf, batch_hash
    from hypertrain.trainer.model import param_shapes
    from hypertrain.trainer.optim import uses_muon
    from hypertrain.trainer.rng import rng_ctr

    cfg = TrainConfig.from_manifest_v2(job.manifest)
    n = job.manifest.training.reference_spec.layout.n_gpus
    identity = hashlib.sha256(canonicalize(job.model_dump(mode="json"))).hexdigest()
    baseline = None
    backend = None
    ranks = []
    for rank in range(n):
        d = confined(directory, f"rank-{rank}/summary.json").parent
        raw = load_json((d / "summary.json").read_bytes())
        if not isinstance(raw, dict) or raw.get("job_hash") != identity:
            raise IslandFailure("rank job binding mismatch")
        if raw.get("backend") not in ("cpu", "cuda"):
            raise IslandFailure("rank backend missing")
        if backend is not None and backend != raw["backend"]:
            raise IslandFailure("rank backend disagreement")
        backend = raw["backend"]
        work = RankWorkResult.model_validate(raw["work"])
        if work.rank != rank:
            raise IslandFailure("rank identity mismatch")
        ranks.append(work)
        th, st = unpack_state(confined(directory, f"rank-{rank}/state.safetensors").read_bytes())
        ef, ef_st = unpack_state(confined(directory, f"rank-{rank}/ef.safetensors").read_bytes())
        delta = confined(directory, f"rank-{rank}/delta.bin").read_bytes()
        shapes = param_shapes(cfg.model)
        if st is None or ef_st is not None or set(th) != set(shapes) or set(ef) != set(shapes):
            raise IslandFailure("incomplete final state")
        if any(tuple(th[k].shape) != s or tuple(ef[k].shape) != s for k, s in shapes.items()):
            raise IslandFailure("final state shape mismatch")
        adam = {k for k in th if not uses_muon(cfg.inner, k)}
        expected_step = (job.global_step0 if cfg.inner.state_policy == "carry" else 0) + cfg.inner.H
        if (
            set(st.m) != set(th)
            or set(st.v) != adam
            or st.step != expected_step
            or st.step != raw["optimizer_step"]
        ):
            raise IslandFailure("incomplete optimizer state")
        if any(tuple(x.shape) != shapes[k] for state in (st.m, st.v) for k, x in state.items()):
            raise IslandFailure("optimizer state shape mismatch")
        pres = [
            LeafPreimage.model_validate(x)
            for x in json.loads(confined(directory, f"rank-{rank}/leaves.json").read_bytes())
        ]
        if len(pres) != cfg.inner.n_leaves:
            raise IslandFailure("leaf count mismatch")
        a = IslandAssignment(job.run_id, job.w, tuple(job.sample_ids), job.global_step0, n)
        for i, p in enumerate(pres):
            if p.run_id != job.run_id or p.w != job.w or p.t != i * cfg.inner.J:
                raise IslandFailure("leaf identity mismatch")
            ids = (
                tuple(s for t in range(p.t - cfg.inner.J + 1, p.t + 1) for s in a.batch_ids(cfg, t))
                if p.t
                else ()
            )
            if p.batch_ids_sha256 != batch_hash(ids) or p.rng_ctr != rng_ctr(
                job.run_id, job.w, p.t, -1
            ):
                raise IslandFailure("leaf assignment/rng mismatch")
            chk, opt = unpack_state(
                confined(directory, f"rank-{rank}/checkpoints/{p.t}.safetensors").read_bytes()
            )
            from hypertrain.trainer.loop import stage_states

            expected_checkpoint_step = (
                job.global_step0 if cfg.inner.state_policy == "carry" else 0
            ) + p.t
            if (
                opt is None
                or opt.step != expected_checkpoint_step
                or stage_states(cfg, chk, opt) != p.stages
            ):
                raise IslandFailure("checkpoint leaf state mismatch")
        last = pres[-1]
        from hypertrain.protocol.messages import f32val

        if (
            _make_leaf(
                cfg, a, last.t, th, st, f32val(last.loss_f32), f32val(last.norm_f32)
            ).preimage
            != last
        ):
            raise IslandFailure("final leaf state mismatch")
        actual = {
            "leaves_root": MerkleTree([bytes.fromhex(p.digest()) for p in pres]).root.hex(),
            "final_theta_hash": state_hash(th),
            "state_root": tensor_root(th, st),
            "ef_out_hash": state_hash(ef),
            "delta_hash": hashlib.sha256(delta).hexdigest(),
            "leaves": [p.digest() for p in pres],
        }
        start_blob = confined(directory, job.object_paths["start_state"]).read_bytes()
        ef_blob = confined(directory, job.object_paths["ef_in"]).read_bytes()
        v0_blob = confined(directory, job.object_paths["v0"]).read_bytes()
        if (
            hashlib.sha256(start_blob).hexdigest() != job.start_state_sha256
            or hashlib.sha256(ef_blob).hexdigest() != job.ef_in_sha256
            or hashlib.sha256(v0_blob).hexdigest() != job.v0_sha256
        ):
            raise IslandFailure("published input hash mismatch")
        from hypertrain.trainer.optim import init_state

        theta0, carry0 = unpack_state(start_blob)
        v0, _ = unpack_state(v0_blob)
        initial = init_state(
            cfg.inner,
            theta0,
            carry0 if cfg.inner.state_policy == "carry" else None,
            v0 if cfg.inner.state_policy == "derived" else None,
        )
        if _make_leaf(cfg, a, 0, theta0, initial, 0.0, 0.0).preimage != pres[0]:
            raise IslandFailure("initial leaf differs from authenticated start")
        ef0, _ = unpack_state(ef_blob)
        payload, ef_expected = compress(cfg.compress, {k: theta0[k] - th[k] for k in th}, ef0)
        if payload != delta or state_hash(ef_expected) != state_hash(ef):
            raise IslandFailure("terminal compression/EF mismatch")
        if raw["commitments"] != actual or baseline is not None and baseline != actual:
            raise IslandFailure("rank artifact disagreement or corruption")
        baseline = actual
    d = directory / "rank-0"
    return IslandArtifacts(
        directory,
        d / "state.safetensors",
        d / "ef.safetensors",
        d / "delta.bin",
        d / "leaves.json",
        tuple(ranks),
    )


def launch_island(
    job: IslandJobV1,
    directory: Path,
    *,
    backend: Literal["cpu", "cuda"] = "cuda",
    cancel: threading.Event | None = None,
    trace: bool = False,
    capacity: CapacityAttempt | None = None,
    devices: tuple[str, ...] | None = None,
) -> IslandArtifacts:
    """Inputs in directory; deadline is Unix seconds. Restart returns the validated publication.

    devices: CUDA only, exactly layout.n_gpus GPU UUIDs; ranks see only these (in order).
    """
    job = IslandJobV1.model_validate(job.model_dump(mode="json"))
    n_gpus = job.manifest.training.reference_spec.layout.n_gpus
    if devices is not None:
        devices = tuple(gpu_uuid(d) for d in devices)
        if backend != "cuda" or len(devices) != n_gpus or len(set(devices)) != n_gpus:
            raise IslandFailure(f"need {n_gpus} distinct CUDA devices, got {devices}")
    directory = directory.resolve()
    directory.mkdir(parents=True, exist_ok=True)
    with flock(directory / "launch.lock"):
        if time.time() >= job.deadline or cancel is not None and cancel.is_set():
            raise IslandFailure("job deadline/cancellation")
        published = directory / "published"
        if published.exists():
            summary = load_json(confined(published, "rank-0/summary.json").read_bytes())
            if not isinstance(summary, dict) or summary.get("backend") != backend:
                raise IslandFailure("CPU publication cannot serve as CUDA oracle")
            if trace:
                for rank in range(job.manifest.training.reference_spec.layout.n_gpus):
                    if not (published / f"rank-{rank}/trace.json").is_file():
                        raise IslandFailure("publication has no requested actual-path trace")
                    confined(published, f"rank-{rank}/trace.json")
            artifacts = validate_artifacts(job, published)
            if time.time() >= job.deadline or cancel is not None and cancel.is_set():
                raise IslandFailure("job expired during publication validation")
            return artifacts
        with tempfile.TemporaryDirectory(prefix="attempt-", dir=directory) as tmp:
            attempt = Path(tmp)
            for relative in job.object_paths.values():
                source = confined(directory, relative)
                target = attempt / relative
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(source, target)
            durable_write(attempt / "job.json", canonicalize(job.model_dump(mode="json")))
            ref = job.manifest.training.reference_spec
            env = dict(
                os.environ,
                CUBLAS_WORKSPACE_CONFIG=ref.env.CUBLAS_WORKSPACE_CONFIG,
                OMP_NUM_THREADS="1",
                CUDA_DISABLE_PTX_JIT=ref.env.CUDA_DISABLE_PTX_JIT,
                HT_ISLAND_TRACE="1" if trace else "0",
            )
            if devices is not None:
                env.update(CUDA_VISIBLE_DEVICES=",".join(devices), HT_GPU_UUIDS=",".join(devices))
            if capacity is not None:
                run_capacity_argv(
                    launch_argv(job, attempt, backend),
                    directory / "capacity-runtime",
                    job.deadline,
                    capacity,
                    env=env,
                    cancel=cancel,
                )
                # Parent-side tensor artifact validation remains unqualified for heavy32.
            else:
                _launch_unbounded(job, attempt, backend, env, cancel)
            validate_artifacts(job, attempt)
            if time.time() >= job.deadline or cancel is not None and cancel.is_set():
                raise IslandFailure("job expired before atomic publication")
            os.rename(attempt, published)
            fd = os.open(directory, os.O_RDONLY | os.O_DIRECTORY)
            try:
                os.fsync(fd)
            finally:
                os.close(fd)
        return validate_artifacts(job, published)


def _launch_unbounded(
    job: IslandJobV1,
    attempt: Path,
    backend: Literal["cpu", "cuda"],
    env: dict[str, str],
    cancel: threading.Event | None,
) -> None:
    """Original ordinary launcher lifecycle; no new resource policy for legacy callers."""
    with (
        open(attempt / "stdout.log", "xb", buffering=0) as stdout,
        open(attempt / "stderr.log", "xb+", buffering=0) as stderr,
    ):
        proc = subprocess.Popen(  # noqa: S603 - fixed interpreter/package, typed layout
            launch_argv(job, attempt, backend),
            env=env,
            stdout=stdout,
            stderr=stderr,
            start_new_session=True,
        )
        done = threading.Event()

        def terminate() -> None:
            try:
                os.killpg(proc.pid, signal.SIGKILL)
            except ProcessLookupError:
                return

        # Python 3.12 Event has no public wait-any subscription. Share its condition
        # to wake on cancellation OR completion without altering the caller's event.
        condition: threading.Condition | None = (
            vars(cancel)["_cond"] if cancel is not None else None
        )

        def cancelled() -> None:
            assert cancel is not None and condition is not None
            with condition:
                condition.wait_for(
                    lambda: done.is_set() or cancel.is_set(),
                    timeout=max(0.0, job.deadline - time.time()),
                )
            if not done.is_set():
                terminate()

        watcher = None
        if cancel is not None:
            watcher = threading.Thread(target=cancelled, name="island-cancellation")
            watcher.start()
        try:
            proc.wait(timeout=max(0.001, job.deadline - time.time()))
        except subprocess.TimeoutExpired as e:
            terminate()
            proc.wait()
            raise IslandFailure("island deadline expired") from e
        finally:
            done.set()
            if condition is not None:
                with condition:
                    condition.notify_all()
            if watcher is not None:
                watcher.join()
            terminate()
            os.fsync(stdout.fileno())
            os.fsync(stderr.fileno())
        if proc.returncode != 0 or cancel is not None and cancel.is_set():
            stderr.seek(max(0, stderr.seek(0, os.SEEK_END) - 4000))
            tail = stderr.read(4000).decode(errors="replace")
            raise IslandFailure(f"rank failure {proc.returncode}: {tail}")
