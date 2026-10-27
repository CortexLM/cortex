"""In-container half of the generic CUDA executor (docs/adr/0008-generic-cuda-executor.md).

`cuda_executor` sends this file's source to `python -I -c` inside the digest-pinned GPU image;
the challenge never imports it. Standard library only: the image's hypertrain may be older than
the challenge, so nothing here may depend on it. The input tar arrives on stdin (a `request.json`
member, then the job inputs), the island runs under torchrun, and a tar of the rank artifacts, the
observed environment and a closing `COMPLETE` member leaves on stdout. Nothing is written outside
a private temporary directory of the container.

Exit codes: 0 done, 2 bad request or input, 3 island failed, 4 visible devices differ from the
request, 5 unsafe output, 6 budget exhausted, 7 terminated, 8 reader gone, 9 device probe could
not run.
"""

from __future__ import annotations

import hashlib
import io
import json
import os
import signal
import subprocess
import sys
import tarfile
import tempfile
import time
from collections.abc import Callable
from pathlib import Path
from types import FrameType
from typing import IO, Any

VERSION = 1
ENV_KEYS = frozenset(
    {
        "CUBLAS_WORKSPACE_CONFIG",
        "CUDA_DISABLE_PTX_JIT",
        "OMP_NUM_THREADS",
        "MKL_NUM_THREADS",
        "PYTHONHASHSEED",
        "NVIDIA_TF32_OVERRIDE",
        "HT_IMAGE_DIGEST",
        "HT_ISLAND_TRACE",
        "HT_GPU_UUIDS",
        "CUDA_VISIBLE_DEVICES",
    }
)
TAIL = 4000
REQUEST_LIMIT = 1 << 20

PROBE = """
import json, subprocess, sys
from pathlib import Path
import torch
n = torch.cuda.device_count()
props = [torch.cuda.get_device_properties(i) for i in range(n)]
uuids = ["GPU-" + str(p.uuid).lower().removeprefix("gpu-") for p in props]
drivers = [
    subprocess.run(
        ["nvidia-smi", "-i", u, "--query-gpu=driver_version", "--format=csv,noheader"],
        check=True, capture_output=True, text=True, timeout=30,
    ).stdout.strip()
    for u in uuids
]
print(json.dumps({
    "backend": "cuda",
    "python": sys.executable,
    "torch": str(torch.__version__),
    "torch_path": str(Path(torch.__file__).resolve()),
    "cuda": torch.version.cuda,
    "gpu_uuids": uuids,
    "gpu_names": [p.name for p in props],
    "sm_counts": [p.multi_processor_count for p in props],
    "capabilities": [[p.major, p.minor] for p in props],
    "drivers": drivers,
}))
"""


class Refused(Exception):
    def __init__(self, code: int, message: str) -> None:
        super().__init__(message)
        self.code = code


def _safe(name: str) -> str:
    parts = name.split("/")
    if not name or name.startswith("/") or "\\" in name or any(p in ("", ".", "..") for p in parts):
        raise Refused(2, f"unsafe member name {name!r}")
    return name


def read_inputs(stdin: IO[bytes], work: Path) -> tuple[dict[str, Any], bytes]:
    """Extract the regular members under `work`; the request must describe exactly them."""
    seen: dict[str, str] = {}
    request_raw = b""
    try:
        with tarfile.open(fileobj=stdin, mode="r|") as archive:
            for member in archive:
                name = _safe(member.name)
                if not member.isreg() or name in seen:
                    raise Refused(2, f"member {name!r} is not a unique regular file")
                stream = archive.extractfile(member)
                assert stream is not None
                digest = hashlib.sha256()
                if name == "request.json":
                    if member.size > REQUEST_LIMIT:
                        raise Refused(2, "request.json too large")
                    request_raw = stream.read()
                    digest.update(request_raw)
                else:
                    target = work / name
                    target.parent.mkdir(parents=True, exist_ok=True)
                    with open(target, "xb") as out:
                        while chunk := stream.read(1 << 20):
                            digest.update(chunk)
                            out.write(chunk)
                seen[name] = digest.hexdigest()
    except tarfile.TarError as error:
        raise Refused(2, f"input tar unreadable: {error}") from error
    try:
        request = json.loads(request_raw)
    except ValueError as error:
        raise Refused(2, "request.json missing or malformed") from error
    seen.pop("request.json", None)
    env = request.get("env") if isinstance(request, dict) else None
    if (
        not isinstance(request, dict)
        or request.get("version") != VERSION
        or request.get("inputs") != seen
        or not isinstance(env, dict)
        or not set(env) <= ENV_KEYS
        or not all(isinstance(v, str) for v in env.values())
        or not isinstance(request.get("budget_seconds"), int)
        or not isinstance(request.get("n_gpus"), int)
    ):
        raise Refused(2, "request does not describe the received inputs")
    return request, request_raw


def probe_environment(request: dict[str, Any]) -> dict[str, Any]:
    """Observe the devices in a throwaway process so no CUDA context outlives the probe."""
    try:
        proc = subprocess.run(  # noqa: S603 - the interpreter running this file, fixed source
            [sys.executable, "-c", PROBE],
            capture_output=True,
            text=True,
            timeout=300,
            check=False,
        )
        if proc.returncode:
            raise Refused(9, "environment probe failed: " + proc.stderr[-TAIL:])
        observed = json.loads(proc.stdout.strip().splitlines()[-1])
    except (subprocess.TimeoutExpired, IndexError, ValueError) as error:
        raise Refused(9, f"environment probe gave no answer: {type(error).__name__}") from error
    if not isinstance(observed, dict):
        raise Refused(9, "environment probe answered a non-object")
    return observed


def torchrun(request: dict[str, Any], attempt: Path, budget: float, log: Path) -> int:
    """The argv of `hypertrain.miner.island_launch.launch_argv`, with a wall budget."""
    argv = [
        sys.executable,
        "-m",
        "torch.distributed.run",
        "--standalone",
        f"--nproc-per-node={request['n_gpus']}",
        "--max-restarts=0",
        "-m",
        "hypertrain.miner.island_worker",
        str(attempt),
        "cuda",
    ]
    with open(log, "xb", buffering=0) as sink:
        proc = subprocess.Popen(  # noqa: S603 - fixed argv built above
            argv, stdout=sink, stderr=sink, start_new_session=True, env=dict(os.environ)
        )
        try:
            return proc.wait(timeout=max(1.0, budget))
        except subprocess.TimeoutExpired:
            raise Refused(6, "island exceeded the wall budget") from None
        finally:
            try:
                os.killpg(proc.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            proc.wait()


class _Hashing(io.BufferedReader):
    def __init__(self, path: Path) -> None:
        super().__init__(io.FileIO(path, "rb"))
        self.digest = hashlib.sha256()

    def read(self, size: int | None = -1) -> bytes:
        data = super().read(size)
        self.digest.update(data)
        return data


def write_outputs(
    stdout: IO[bytes], attempt: Path, environment: dict[str, Any], request_raw: bytes
) -> None:
    ranks = sorted(p for p in attempt.glob("rank-*") if p.is_dir() and not p.is_symlink())
    entries = [p for rank in ranks for p in sorted(rank.rglob("*"))]
    if any(p.is_symlink() or not (p.is_file() or p.is_dir()) for p in entries):
        raise Refused(5, "island output holds a link or a special file")
    files = [p for p in entries if p.is_file()]
    if not files:
        raise Refused(5, "island produced no rank files")
    listing: dict[str, str] = {}
    with tarfile.open(fileobj=stdout, mode="w|", format=tarfile.PAX_FORMAT) as archive:
        for path in files:
            relative = path.relative_to(attempt).as_posix()
            info = tarfile.TarInfo(relative)
            info.size, info.mode = path.stat().st_size, 0o600
            with _Hashing(path) as reader:
                archive.addfile(info, reader)
            listing[relative] = reader.digest.hexdigest()
        raw = json.dumps(environment, sort_keys=True).encode()
        _add(archive, "environment.json", raw)
        listing["environment.json"] = _sha(raw)
        done = {"version": VERSION, "files": listing, "request_sha256": _sha(request_raw)}
        _add(archive, "COMPLETE", json.dumps(done, sort_keys=True).encode())


def _add(archive: tarfile.TarFile, name: str, raw: bytes) -> None:
    info = tarfile.TarInfo(name)
    info.size, info.mode = len(raw), 0o600
    archive.addfile(info, io.BytesIO(raw))


def _sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _terminated(signum: int, frame: FrameType | None) -> None:
    raise Refused(7, "terminated")


def serve(
    stdin: IO[bytes],
    stdout: IO[bytes],
    stderr: IO[str],
    *,
    probe: Callable[[dict[str, Any]], dict[str, Any]] = probe_environment,
    launch: Callable[[dict[str, Any], Path, float, Path], int] = torchrun,
) -> int:
    started = time.monotonic()
    signal.signal(signal.SIGTERM, _terminated)
    with tempfile.TemporaryDirectory(prefix="ht-exec-") as tmp:
        work = Path(tmp)
        attempt, log = work / "attempt", work / "island.log"
        attempt.mkdir()
        try:
            request, request_raw = read_inputs(stdin, attempt)
            os.environ.update(request["env"])
            environment = probe(request)
            wanted = request.get("uuids")
            if (
                not isinstance(wanted, list)
                or environment.get("gpu_uuids") != wanted
                or len(wanted) != request["n_gpus"]
            ):
                raise Refused(4, "visible GPUs differ from the requested devices")
            code = launch(
                request, attempt, request["budget_seconds"] - (time.monotonic() - started), log
            )
            if code:
                tail = log.read_bytes()[-TAIL:].decode(errors="replace") if log.exists() else ""
                raise Refused(3, f"island exited {code}: {tail}")
            write_outputs(stdout, attempt, environment, request_raw)
            stdout.flush()
        except Refused as refusal:
            stderr.write(f"cuda-executor-runner: {refusal}\n")
            return refusal.code
        except BrokenPipeError:
            return 8
    return 0


def main() -> int:
    return serve(sys.stdin.buffer, sys.stdout.buffer, sys.stderr)


if __name__ == "__main__":
    raise SystemExit(main())
