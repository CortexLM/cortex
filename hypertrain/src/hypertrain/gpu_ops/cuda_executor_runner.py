"""In-container half of the generic CUDA executor (docs/adr/0008-generic-cuda-executor.md).

`cuda_executor` sends this file's source to `python -I -c` inside the digest-pinned GPU image;
the challenge never imports it. Standard library only: the image's hypertrain may be older than
the challenge, so nothing here may depend on it. The input tar arrives on stdin (a `request.json`
member, then the job inputs), the island runs under torchrun, and a tar of the rank artifacts, the
observed environment and a closing `COMPLETE` member leaves on stdout. Nothing is written outside
a private temporary directory of the container.

Exit codes: 0 done, 2 bad request or input, 3 island failed, 4 visible devices differ from the
request, 5 unsafe output, 6 budget exhausted, 7 terminated, 8 reader gone, 9 device probe could
not run, 10 artifact transfer failed, 11 artifact store unreachable.

With an `artifact` block in request.json (docs/adr/0013-executor-artifacts-via-r2.md) the result
tar does not leave on stdout: it is hashed in place, declared in R2 and uploaded to presigned URLs
the challenge answers with, and stdout carries one status line. Plain classes only below: this file
also runs under `exec` in a namespace that is not a module.
"""

from __future__ import annotations

import bisect
import hashlib
import http.client
import io
import ipaddress
import json
import os
import random
import re
import signal
import ssl
import subprocess
import sys
import tarfile
import tempfile
import threading
import time
import urllib.parse
import xml.etree.ElementTree as ET
from collections.abc import Callable, Mapping, Sequence
from concurrent.futures import ThreadPoolExecutor
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
ARTIFACT_FAILED = 10
ARTIFACT_UNREACHABLE = 11
BLOCK_BYTES = 4 << 20
PART_BYTES = (8 << 20, 64 << 20)  # R2 refuses non-final parts under 5 MiB; the challenge picks one
MAX_PARTS = 10_000
LIST_PAGE = 1000
OFFER_LIMIT = 32 << 20
STATUS_LIMIT = 1 << 10
SLICE_BYTES = 1 << 20
RETRY_STATUS = frozenset({429, 500, 502, 503, 504})
RETRY_CAP = 16.0
EXPIRY_SLACK = 2  # seconds taken off the lifetime the challenge announced for its URLs

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


def _monotonic() -> float:
    return time.monotonic()


def _sleep(seconds: float) -> None:
    time.sleep(seconds)


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


def rank_files(attempt: Path) -> list[Path]:
    """The regular files under rank-*/ in the order the tar carries them."""
    ranks = sorted(p for p in attempt.glob("rank-*") if p.is_dir() and not p.is_symlink())
    entries = [p for rank in ranks for p in sorted(rank.rglob("*"))]
    if any(p.is_symlink() or not (p.is_file() or p.is_dir()) for p in entries):
        raise Refused(5, "island output holds a link or a special file")
    files = [p for p in entries if p.is_file()]
    if not files:
        raise Refused(5, "island produced no rank files")
    return files


def write_outputs(
    stdout: IO[bytes], attempt: Path, environment: dict[str, Any], request_raw: bytes
) -> None:
    files = rank_files(attempt)
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


# ---- artifact transfer (docs/adr/0013-executor-artifacts-via-r2.md) -----------------------------


class TransferError(Exception):
    """A transfer step failed. `reason` is a fixed word, an HTTP status or an exception class
    name, never text taken from a URL, a reply or an exception message."""

    def __init__(self, reason: str, *, fatal: bool = False) -> None:
        super().__init__(reason)
        self.reason, self.fatal = reason, fatal


class Layout:
    """The tar `write_outputs` streams, addressable by offset. Headers, padding and the closing
    blocks are built here; the data of a file is read from disk when asked for, so the result is
    neither written a second time nor held in memory."""

    def __init__(self, members: Sequence[tuple[str, Path | bytes]]) -> None:
        self._starts: list[int] = []
        self._ends: list[int] = []
        self._pieces: list[bytes | Path] = []
        self._fds: dict[Path, int] = {}
        self._lock = threading.Lock()
        offset = 0
        for name, source in members:
            size = len(source) if isinstance(source, bytes) else source.stat().st_size
            info = tarfile.TarInfo(name)
            info.size, info.mode = size, 0o600
            header = info.tobuf(tarfile.PAX_FORMAT, tarfile.ENCODING, "surrogateescape")
            offset = self._add(offset, header, len(header))
            offset = self._add(offset, source, size)
            offset = self._add(offset, bytes(-size % tarfile.BLOCKSIZE))
        offset = self._add(offset, bytes(2 * tarfile.BLOCKSIZE))
        self.size = self._add(offset, bytes(-offset % tarfile.RECORDSIZE))

    def _add(self, offset: int, piece: bytes | Path, length: int | None = None) -> int:
        length = len(piece) if length is None and isinstance(piece, bytes) else length or 0
        if length:
            self._starts.append(offset)
            self._ends.append(offset + length)
            self._pieces.append(piece)
        return offset + length

    def read_at(self, offset: int, length: int) -> bytes:
        if offset < 0 or length < 0 or offset + length > self.size:
            raise ValueError("outside the layout")
        out = bytearray()
        index = bisect.bisect_right(self._starts, offset) - 1
        while len(out) < length:
            start, piece = self._starts[index], self._pieces[index]
            lo = offset + len(out) - start
            hi = min(self._ends[index], offset + length) - start
            out += piece[lo:hi] if isinstance(piece, bytes) else self._read(piece, lo, hi - lo)
            index += 1
        return bytes(out)

    def _read(self, path: Path, offset: int, length: int) -> bytes:
        with self._lock:
            fd = self._fds.get(path)
            if fd is None:
                fd = self._fds[path] = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
        out = bytearray()
        while len(out) < length:
            chunk = os.pread(fd, length - len(out), offset + len(out))
            if not chunk:
                raise Refused(5, "island output changed while it was being sent")
            out += chunk
        return bytes(out)

    def close(self) -> None:
        with self._lock:
            for fd in self._fds.values():
                os.close(fd)
            self._fds.clear()


def _file_sha(path: Path) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as stream:
        while chunk := stream.read(1 << 20):
            digest.update(chunk)
    return digest.hexdigest()


def build_artifact(
    attempt: Path, environment: dict[str, Any], request_raw: bytes, workers: int
) -> tuple[Layout, dict[str, str]]:
    """The layout of what `write_outputs` would stream, and the sha256 of each rank file."""
    files = rank_files(attempt)
    names = [path.relative_to(attempt).as_posix() for path in files]
    with ThreadPoolExecutor(workers) as pool:
        listing = dict(zip(names, pool.map(_file_sha, files), strict=True))
    raw = json.dumps(environment, sort_keys=True).encode()
    done = {
        "version": VERSION,
        "files": {**listing, "environment.json": _sha(raw)},
        "request_sha256": _sha(request_raw),
    }
    members: list[tuple[str, Path | bytes]] = [*zip(names, files, strict=True)]
    members += [("environment.json", raw), ("COMPLETE", json.dumps(done, sort_keys=True).encode())]
    return Layout(members), listing


def hash_layout(layout: Layout, part_bytes: int, workers: int) -> tuple[str, list[str], list[str]]:
    """sha256 of the whole tar, of each 4 MiB block and of each part. The whole-object digest is
    one sequential chain; blocks and parts run on the other threads."""
    size = layout.size

    def whole() -> str:
        digest = hashlib.sha256()
        for start in range(0, size, BLOCK_BYTES):
            digest.update(layout.read_at(start, min(BLOCK_BYTES, size - start)))
        return digest.hexdigest()

    def part(number: int) -> tuple[str, list[str]]:
        first, end = number * part_bytes, min(size, (number + 1) * part_bytes)
        digest, blocks = hashlib.sha256(), []
        for start in range(first, end, BLOCK_BYTES):
            data = layout.read_at(start, min(BLOCK_BYTES, end - start))
            digest.update(data)
            blocks.append(hashlib.sha256(data).hexdigest())
        return digest.hexdigest(), blocks

    with ThreadPoolExecutor(workers + 1) as pool:
        chain = pool.submit(whole)
        results = list(pool.map(part, range(-(-size // part_bytes))))
        return chain.result(), [b for _, blocks in results for b in blocks], [d for d, _ in results]


def _loopback(host: str | None) -> bool:
    try:
        return host is not None and ipaddress.ip_address(host).is_loopback
    except ValueError:
        return False


def check_url(url: object, origin: tuple[str, str] | None = None) -> tuple[str, str]:
    """(scheme, host:port) of a URL the challenge handed over. https, or plain http to a literal
    loopback address; no user info, no fragment; the origin of the first URL for all the others."""
    try:
        if (
            not isinstance(url, str)
            or len(url) > 4096
            or not url.isascii()
            or not url.isprintable()
        ):
            raise ValueError
        parts = urllib.parse.urlsplit(url)
        _ = parts.port  # a malformed port raises here
        secure = parts.scheme == "https"
        if not (secure or (parts.scheme == "http" and _loopback(parts.hostname))):
            raise ValueError
        if " " in url or "@" in parts.netloc or parts.fragment or not parts.path.startswith("/"):
            raise ValueError
        found = (parts.scheme, parts.netloc)
        if origin is not None and found != origin:
            raise ValueError
    except ValueError:
        raise TransferError("bad url", fatal=True) from None
    return found


ARTIFACT_FIELDS: dict[str, tuple[int, int]] = {
    "version": (1, 1),
    "part_bytes": PART_BYTES,
    "workers": (1, 32),
    "reserve_seconds": (0, 86400),
    "poll_ms": (10, 60_000),
    "offer_wait_s": (1, 3600),
    "retry_attempts": (1, 10),
    "retry_base_ms": (1, 10_000),
    "timeout_s": (1, 600),
}


def artifact_block(request: dict[str, Any]) -> dict[str, Any] | None:
    """The validated `artifact` block of request.json, or None for the classic stdout transfer."""
    block = request.get("artifact")
    if block is None:
        return None
    if not isinstance(block, dict) or set(block) != {*ARTIFACT_FIELDS, "declare_put", "offer_get"}:
        raise Refused(2, "request artifact block has other fields than the runner knows")
    for name, (low, high) in ARTIFACT_FIELDS.items():
        if type(block[name]) is not int or not low <= block[name] <= high:
            raise Refused(2, f"request artifact field {name} is out of range")
    if block["part_bytes"] % BLOCK_BYTES:
        raise Refused(2, "request artifact part size is not a whole number of 4 MiB blocks")
    try:
        check_url(block["offer_get"], check_url(block["declare_put"]))
    except TransferError:
        raise Refused(2, "request artifact URL is not acceptable") from None
    return block


class Transfer:
    """The settings and clocks of one transfer."""

    def __init__(self, block: dict[str, Any], deadline: float) -> None:
        self.attempts, self.base = block["retry_attempts"], block["retry_base_ms"] / 1000
        self.timeout, self.workers = block["timeout_s"], block["workers"]
        self.part_bytes, self.poll = block["part_bytes"], block["poll_ms"] / 1000
        self.offer_wait = block["offer_wait_s"]
        self.deadline = deadline  # monotonic
        self.expires: float | None = None  # monotonic: when the URLs of the offer stop working


_connections = threading.local()


def http_send(
    method: str,
    url: str,
    headers: Mapping[str, str],
    body: bytes | None,
    timeout: float,
    limit: int,
) -> tuple[int, dict[str, str], bytes]:
    """One request on this thread's connection to the origin: no redirect, no proxy variable, the
    default TLS context, an explicit Content-Length, the body sent in slices that each get the
    timeout. Returns (status, {'retry-after': ...}, body of at most `limit` bytes)."""
    parts = urllib.parse.urlsplit(url)
    host, key = parts.hostname or "", f"{parts.scheme}://{parts.netloc}"
    cache: dict[str, http.client.HTTPConnection] = _connections.__dict__.setdefault("cache", {})
    conn = cache.get(key)
    if conn is None:
        if parts.scheme == "https":
            context = ssl.create_default_context()
            conn = http.client.HTTPSConnection(host, parts.port, timeout=timeout, context=context)
        else:
            conn = http.client.HTTPConnection(host, parts.port, timeout=timeout)
        cache[key] = conn
    if conn.sock is not None:
        conn.sock.settimeout(timeout)
    target = parts.path + ("?" + parts.query if parts.query else "")
    try:
        try:
            conn.putrequest(method, target, skip_accept_encoding=True)
            for name, value in headers.items():
                conn.putheader(name, value)
            if body is not None:
                conn.putheader("Content-Length", str(len(body)))
            conn.endheaders()
            if body is not None:
                view = memoryview(body)
                for start in range(0, len(view), SLICE_BYTES):
                    conn.send(view[start : start + SLICE_BYTES])
        except (BrokenPipeError, ConnectionResetError):
            pass  # the server may have answered before it read the body: read that answer
        response = conn.getresponse()
        data = response.read(limit + 1)
        status, after = response.status, response.getheader("retry-after") or ""
    except (OSError, http.client.HTTPException, ValueError) as error:
        cache.pop(key, None)
        conn.close()
        raise TransferError(f"transport {type(error).__name__}") from None
    if status not in (200, 201, 204, 404):  # a refusal may leave the socket half used
        cache.pop(key, None)
        conn.close()
    if len(data) > limit:
        raise TransferError("response too large", fatal=True)
    return status, {"retry-after": after}, data


def _wait(spec: Transfer, attempt: int, throttled: bool, after: str) -> float:
    wait = random.uniform(0, min(RETRY_CAP, spec.base * 2**attempt))
    if throttled:
        wait = max(wait, 1.0)
    if after.isdigit():
        wait = max(wait, min(RETRY_CAP, float(after)))
    return min(wait, max(0.0, spec.deadline - _monotonic()))


def _call(
    spec: Transfer,
    method: str,
    url: str,
    headers: Mapping[str, str] | None = None,
    body: bytes | None = None,
    *,
    ok: Sequence[int] = (200,),
    limit: int = STATUS_LIMIT,
) -> tuple[int, bytes]:
    """http_send with the retry rules of storage.r2.Retry: 429, 5xx and a transport error back off
    and try again; any other status, an expired URL and the deadline end it."""
    reason = "no attempt"
    for attempt in range(spec.attempts):
        now = _monotonic()
        if now >= spec.deadline:
            raise TransferError("deadline reached", fatal=True)
        if spec.expires is not None and now >= spec.expires:
            raise TransferError("url expired", fatal=True)
        throttled, after = False, ""
        try:
            status, back, data = http_send(method, url, headers or {}, body, spec.timeout, limit)
        except TransferError as error:
            if error.fatal:
                raise
            reason = error.reason
        else:
            if status in ok:
                return status, data
            if status not in RETRY_STATUS:
                raise TransferError(f"HTTP {status}", fatal=True)
            reason, throttled, after = f"HTTP {status}", status == 429, back.get("retry-after", "")
        if attempt + 1 < spec.attempts:
            _sleep(_wait(spec, attempt, throttled, after))
    raise TransferError(reason, fatal=True)


def preflight(block: dict[str, Any]) -> None:
    """Before the island spends its budget: can this host reach the store at all? The offer does
    not exist yet, so a reachable store answers 404."""
    try:
        _call(Transfer(block, _monotonic() + 60), "GET", block["offer_get"], ok=(404,))
    except TransferError as error:
        raise Refused(ARTIFACT_UNREACHABLE, f"artifact store unreachable: {error.reason}") from None
    except Exception as error:  # noqa: BLE001 - nothing of a library message may reach stderr
        raise Refused(
            ARTIFACT_UNREACHABLE, f"artifact store unreachable: {type(error).__name__}"
        ) from None


def _offer(spec: Transfer, block: dict[str, Any], declared: bytes, count: int) -> Any:
    """Wait for the challenge's answer and check it is the answer to `declared`."""
    give_up = min(spec.deadline, _monotonic() + spec.offer_wait)
    while True:
        status, data = _call(spec, "GET", block["offer_get"], ok=(200, 404), limit=OFFER_LIMIT)
        if status == 200:
            break
        if _monotonic() >= give_up:
            raise TransferError("no offer in time", fatal=True)
        _sleep(spec.poll)
    try:
        offer = json.loads(data)
    except ValueError:
        raise TransferError("offer is not json", fatal=True) from None
    if isinstance(offer, dict) and isinstance(offer.get("refused"), str):
        code = offer["refused"]
        word = code if re.fullmatch(r"[a-z0-9_-]{1,40}", code) else "unknown"
        raise TransferError(f"offer refused {word}", fatal=True)
    wrong = TransferError("offer does not answer the declaration", fatal=True)
    keys = {"version", "declare_sha256", "ttl_s", "parts", "list"}
    if not isinstance(offer, dict) or set(offer) != keys or offer["version"] != 1:
        raise wrong
    ttl, urls, pages = offer["ttl_s"], offer["parts"], offer["list"]
    if (
        offer["declare_sha256"] != _sha(declared)
        or type(ttl) is not int
        or not 1 <= ttl <= 604800
        or not isinstance(urls, list)
        or not isinstance(pages, list)
        or len(urls) != count
        or len(pages) != -(-count // LIST_PAGE)
    ):
        raise wrong
    try:
        origin = check_url(block["declare_put"])
        for url in [*urls, *pages]:
            check_url(url, origin)
    except TransferError:
        raise wrong from None
    return offer


def _held(spec: Transfer, pages: list[str]) -> dict[int, int]:
    """Part number -> size of every part the store already holds, from the presigned ListParts."""
    held: dict[int, int] = {}
    for url in pages:
        _, data = _call(spec, "GET", url, limit=OFFER_LIMIT)
        try:
            for element in ET.fromstring(data).iter():  # noqa: S314 - reply of our own bucket
                if element.tag.rpartition("}")[2] == "Part":
                    fields = {child.tag.rpartition("}")[2]: child.text for child in element}
                    held[int(fields["PartNumber"] or 0)] = int(fields["Size"] or 0)
        except (ET.ParseError, KeyError, ValueError):
            raise TransferError("malformed part list", fatal=True) from None
    return held


def _send_artifact(
    block: dict[str, Any],
    attempt: Path,
    environment: dict[str, Any],
    request_raw: bytes,
    stdout: IO[bytes],
    deadline: float,
) -> None:
    spec = Transfer(block, deadline)
    layout, _ = build_artifact(attempt, environment, request_raw, spec.workers)
    try:
        size, step = layout.size, spec.part_bytes
        whole, blocks, parts = hash_layout(layout, step, spec.workers)
        if len(parts) > MAX_PARTS:
            raise TransferError("artifact needs too many parts", fatal=True)
        declared = json.dumps(
            {
                "version": 1,
                "request_sha256": _sha(request_raw),
                "size": size,
                "sha256": whole,
                "part_bytes": step,
                "blocks": blocks,
                "parts": parts,
            },
            sort_keys=True,
            separators=(",", ":"),
        ).encode()
        # a 412 is a retried PUT whose first reply was lost: the declaration is already there
        _call(
            spec,
            "PUT",
            block["declare_put"],
            {"if-none-match": "*"},
            declared,
            ok=(200, 201, 204, 412),
        )
        offer = _offer(spec, block, declared, len(parts))
        spec.expires = _monotonic() + offer["ttl_s"] - EXPIRY_SLACK
        held = _held(spec, offer["list"])
        todo = [n for n in range(len(parts)) if held.get(n + 1) != min(step, size - n * step)]

        def put(n: int) -> None:
            data = layout.read_at(n * step, min(step, size - n * step))
            if _sha(data) != parts[n]:
                raise TransferError("island output changed while it was being sent", fatal=True)
            _call(spec, "PUT", offer["parts"][n], {"x-amz-content-sha256": parts[n]}, data)

        pool = ThreadPoolExecutor(spec.workers)
        try:
            for future in [pool.submit(put, n) for n in todo]:
                future.result()
        finally:  # a failed part cancels the queued ones instead of sending them
            pool.shutdown(wait=True, cancel_futures=True)
    finally:
        layout.close()
    status = {"artifact": "uploaded", "sha256": whole, "size": size, "parts": len(parts)}
    stdout.write(json.dumps({**status, "sent": len(todo)}, sort_keys=True).encode() + b"\n")


def send_artifact(
    block: dict[str, Any],
    attempt: Path,
    environment: dict[str, Any],
    request_raw: bytes,
    stdout: IO[bytes],
    deadline: float,
) -> None:
    """Declare the tar, wait for the offer, upload what the store does not hold; one status line."""
    try:
        _send_artifact(block, attempt, environment, request_raw, stdout, deadline)
    except Refused:
        raise
    except TransferError as error:
        raise Refused(ARTIFACT_FAILED, f"artifact transfer failed: {error.reason}") from None
    except Exception as error:  # noqa: BLE001 - nothing of a library message may reach stderr
        raise Refused(
            ARTIFACT_FAILED, f"artifact transfer failed: {type(error).__name__}"
        ) from None


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
    started = _monotonic()
    signal.signal(signal.SIGTERM, _terminated)
    with tempfile.TemporaryDirectory(prefix="ht-exec-") as tmp:
        work = Path(tmp)
        attempt, log = work / "attempt", work / "island.log"
        attempt.mkdir()
        try:
            request, request_raw = read_inputs(stdin, attempt)
            artifact = artifact_block(request)
            if artifact is not None:
                preflight(artifact)
            os.environ.update(request["env"])
            environment = probe(request)
            wanted = request.get("uuids")
            if (
                not isinstance(wanted, list)
                or environment.get("gpu_uuids") != wanted
                or len(wanted) != request["n_gpus"]
            ):
                raise Refused(4, "visible GPUs differ from the requested devices")
            reserve = 0 if artifact is None else artifact["reserve_seconds"]
            code = launch(
                request,
                attempt,
                request["budget_seconds"] - reserve - (_monotonic() - started),
                log,
            )
            if code:
                tail = log.read_bytes()[-TAIL:].decode(errors="replace") if log.exists() else ""
                raise Refused(3, f"island exited {code}: {tail}")
            if artifact is None:
                write_outputs(stdout, attempt, environment, request_raw)
            else:
                deadline = started + request["budget_seconds"]
                send_artifact(artifact, attempt, environment, request_raw, stdout, deadline)
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
