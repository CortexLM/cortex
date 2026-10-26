"""Native network-v2 rescue over N parallel channels (run-m4 Modal recovery adapter).

Same contract as rescue_dedup.py: runs the unchanged NetworkRuntime.rescue(role, fresh=True)
(same generation, pinned SSH, bound, deadline, verify_export, remote manifest digest check and
native network_rescued row). Only stream_bounded is replaced in this process:
  1. pinned SSH: host hashes out/ and returns the manifest; its digest goes to the native log.
  2. distinct blobs split over N workers; each worker streams a tar of blobs either over its
     own pinned SSH channel (--via ssh) or as a direct TCP connection from the host to this
     controller (--via tcp, launched over pinned SSH, authenticated by a one-time token).
  3. every blob is sha256-checked against the SSH-delivered manifest, then the exact export
     tar is rebuilt locally. Integrity is anchored on the pinned SSH channel, never on TCP.
"""

# ruff: noqa: E501, S104  (verbatim controller tool: rescue listeners bind all interfaces)
from __future__ import annotations

import argparse
import hashlib
import io
import json
import os
import secrets
import shlex
import signal
import socket
import subprocess
import sys
import tarfile
import threading
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from rescue_dedup import (  # noqa: E402
    HEX,
    MANIFEST_MAX,
    checked_manifest,
    command_mode,
    load_orchestrate,
)

MANIFEST = r"""
import hashlib, json, os, sys
files = {}
for dirpath, dirs, names in os.walk("out"):
    dirs.sort()
    for d in dirs:
        if os.path.islink(os.path.join(dirpath, d)):
            sys.exit("export_symlink_rejected " + os.path.join(dirpath, d))
    for name in sorted(names):
        path = os.path.join(dirpath, name)
        if os.path.islink(path) or not os.path.isfile(path):
            sys.exit("export_symlink_rejected " + path)
        digest = hashlib.sha256()
        with open(path, "rb") as f:
            while block := f.read(1 << 22):
                digest.update(block)
            size = f.tell()
        files[os.path.relpath(path, "out")] = {"bytes": size, "sha256": digest.hexdigest()}
meta = json.dumps(files, sort_keys=True).encode()
sys.stdout.buffer.write(meta)
sys.stdout.buffer.flush()
print(hashlib.sha256(meta).hexdigest(), file=sys.stderr)
"""

RANGES = r"""
import json, os, socket
spec = json.loads(SPEC)
s = socket.socket()
s.setsockopt(socket.SOL_SOCKET, socket.SO_SNDBUF, 1 << 25)
s.settimeout(30)
s.connect((spec["host"], spec["port"]))
s.settimeout(None)
s.sendall(spec["token"].encode() + b"\n")
for path, offset, length in spec["chunks"]:
    with open(os.path.join("out", path), "rb") as f:
        sent = 0
        while sent < length:
            n = s.sendfile(f, offset + sent, length - sent)
            if not n:
                raise SystemExit("short_read " + path)
            sent += n
s.shutdown(socket.SHUT_WR)
s.close()
"""

CHUNK = 64 << 20

SENDER = r"""
import json, os, socket, sys, tarfile
spec = json.loads(SPEC)
if spec["via"] == "tcp":
    s = socket.create_connection((spec["host"], spec["port"]), timeout=30)
    s.settimeout(None)
    s.sendall(spec["token"].encode() + b"\n")
    sink = s.makefile("wb", buffering=1 << 22)
else:
    sink = sys.stdout.buffer
with tarfile.open(fileobj=sink, mode="w|", format=tarfile.PAX_FORMAT) as out:
    for sha, path in spec["items"]:
        with open(os.path.join("out", path), "rb") as f:
            info = tarfile.TarInfo("blobs/" + sha)
            info.size = os.fstat(f.fileno()).st_size
            out.addfile(info, f)
sink.flush()
if spec["via"] == "tcp":
    s.shutdown(socket.SHUT_WR)
    s.close()
"""


def make_stream(
    orch,
    root: str,
    python: str,
    retained: int,
    via: str,
    workers: int,
    public_ip: str,
    base_port: int,
):
    reject = orch.Reject

    native_stream = orch.stream_bounded

    def stream(argv: list[str], archive: Path, log: Path, bound: int, timeout: float) -> None:
        # Hash-only (" hashes") and dedup (" dedup [sha ...]") exports: the native single stream
        # already carries each needed content once with its exact tar shape; keep it untouched.
        mode = command_mode(argv[-1], root, python)
        if mode is None:
            raise reject("network_rescue_dedup_unexpected_command")
        if mode[0] != "full":
            return native_stream(argv, archive, log, bound, timeout)
        end = time.monotonic() + timeout
        prefix = argv[:-1]
        remote = f"cd {shlex.quote(root)} && {shlex.quote(python)} -"
        procs: list[subprocess.Popen] = []
        lock = threading.Lock()

        def spawn(script: str, out=subprocess.PIPE, err=subprocess.PIPE) -> subprocess.Popen:
            p = subprocess.Popen(  # noqa: S603  (pinned argv, only the command differs)
                [*prefix, remote],
                stdin=subprocess.PIPE,
                stdout=out,
                stderr=err,
                start_new_session=True,
            )
            with lock:
                procs.append(p)
            assert p.stdin is not None
            p.stdin.write(script.encode())
            p.stdin.close()
            return p

        def kill_all() -> None:
            with lock:
                for p in procs:
                    try:
                        import os

                        os.killpg(p.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass

        timer = threading.Timer(timeout, kill_all)
        timer.start()
        blobs = archive.parent / (archive.name + ".blobs")
        blobs.mkdir(mode=0o700)
        try:
            p = spawn(MANIFEST)
            assert p.stdout is not None and p.stderr is not None
            meta = p.stdout.read(MANIFEST_MAX + 1)
            err = p.stderr.read(65536)
            p.wait()
            log.write_bytes(err)
            if p.returncode:
                raise reject("network_export_failed")
            if len(meta) > MANIFEST_MAX:
                raise reject("rescue_manifest_missing_or_oversized")
            files = checked_manifest(meta, reject)
            if sum(w["bytes"] for w in files.values()) > retained:
                raise reject("actual_artifact_budget_exceeded")
            distinct: dict[str, tuple[str, int]] = {}
            for name in sorted(files):
                distinct.setdefault(files[name]["sha256"], (name, files[name]["bytes"]))
            if sum(n for _, n in distinct.values()) > bound:
                raise reject("network_rescue_archive_budget_exceeded")
            errors: list[BaseException] = []
            if via == "range":
                import os

                chunks = []
                for sha, (name, size) in sorted(distinct.items()):
                    with (blobs / sha).open("xb") as f:
                        f.truncate(size)
                    chunks += [(sha, name, o, min(CHUNK, size - o)) for o in range(0, size, CHUNK)]
                parts: list[list[tuple[str, str, int, int]]] = [[] for _ in range(workers)]
                for k, c in enumerate(chunks):
                    parts[k % workers].append(c)
                parts = [p for p in parts if p]

                def ranged(i: int, part: list[tuple[str, str, int, int]]) -> None:
                    try:
                        token = secrets.token_hex(32)
                        srv = socket.create_server(("0.0.0.0", base_port + i))
                        srv.settimeout(max(1.0, min(120.0, end - time.monotonic())))
                        try:
                            spec = {
                                "host": public_ip,
                                "port": base_port + i,
                                "token": token,
                                "chunks": [(n, o, ln) for _, n, o, ln in part],
                            }
                            p = spawn(
                                "SPEC = " + repr(json.dumps(spec)) + "\n" + RANGES,
                                out=subprocess.DEVNULL,
                            )
                            for _ in range(8):
                                conn, _addr = srv.accept()
                                conn.settimeout(max(1.0, end - time.monotonic()))
                                f = conn.makefile("rb", buffering=1 << 22)
                                if f.readline(80).strip() == token.encode():
                                    break
                                f.close()
                                conn.close()
                            else:
                                raise reject("network_rescue_stream_unauthenticated")
                            try:
                                for sha, _n, off, ln in part:
                                    fd = os.open(blobs / sha, os.O_WRONLY)
                                    try:
                                        got = 0
                                        while got < ln:
                                            if time.monotonic() > end:
                                                raise reject("network_rescue_deadline_exceeded")
                                            block = f.read(min(1 << 22, ln - got))
                                            if not block:
                                                raise reject("network_rescue_stream_truncated")
                                            os.pwrite(fd, block, off + got)
                                            got += len(block)
                                    finally:
                                        os.close(fd)
                                if f.read(1):
                                    raise reject("network_rescue_dedup_member")
                            finally:
                                f.close()
                                conn.close()
                            if p.wait(timeout=max(1.0, end - time.monotonic())) != 0:
                                raise reject("network_export_failed")
                        finally:
                            srv.close()
                    except OSError as exc:
                        late = time.monotonic() > end
                        errors.append(
                            reject(
                                "network_rescue_deadline_exceeded"
                                if late
                                else "network_rescue_stream_truncated"
                            )
                        )
                        errors[-1].__cause__ = exc
                        kill_all()
                    except BaseException as exc:  # noqa: BLE001
                        errors.append(exc)
                        kill_all()

                threads = [
                    threading.Thread(target=ranged, args=(i, p)) for i, p in enumerate(parts)
                ]
                for t in threads:
                    t.start()
                for t in threads:
                    t.join()
                if errors:
                    raise errors[0]
                for sha, (_name, size) in distinct.items():
                    if time.monotonic() > end:
                        raise reject("network_rescue_deadline_exceeded")
                    with (blobs / sha).open("rb") as f:
                        if hashlib.file_digest(f, "sha256").hexdigest() != sha or f.tell() != size:
                            raise reject("rescue_content_hash_mismatch")
            groups: list[list[tuple[str, str]]] = [[] for _ in range(workers)]
            loads = [0] * workers
            for sha, (name, size) in sorted(distinct.items(), key=lambda kv: -kv[1][1]):
                i = loads.index(min(loads))
                groups[i].append((sha, name))
                loads[i] += size
            groups = [g for g in groups if g] if via != "range" else []

            def receive(source, want: set[str]) -> None:
                got = set()
                with tarfile.open(fileobj=source, mode="r|") as tar:
                    for member in tar:
                        sha = member.name.removeprefix("blobs/")
                        if (
                            not member.isreg()
                            or member.name != "blobs/" + sha
                            or not HEX.fullmatch(sha)
                            or sha not in want
                            or sha in got
                        ):
                            raise reject("network_rescue_dedup_member")
                        data = tar.extractfile(member)
                        assert data is not None
                        digest, size = hashlib.sha256(), 0
                        with (blobs / sha).open("xb") as sink:
                            while block := data.read(1 << 22):
                                if time.monotonic() > end:
                                    raise reject("network_rescue_deadline_exceeded")
                                digest.update(block)
                                sink.write(block)
                                size += len(block)
                        if digest.hexdigest() != sha or size != distinct[sha][1]:
                            raise reject("rescue_content_hash_mismatch")
                        got.add(sha)
                if got != want:
                    raise reject("network_rescue_dedup_blob_set")

            def worker(i: int, items: list[tuple[str, str]]) -> None:
                try:
                    spec = {"via": via, "items": items}
                    want = {sha for sha, _ in items}
                    if via == "ssh":
                        p = spawn("SPEC = " + repr(json.dumps(spec)) + "\n" + SENDER)
                        assert p.stdout is not None
                        try:
                            receive(p.stdout, want)
                        finally:
                            if p.wait() != 0:
                                raise reject("network_export_failed")
                        return
                    token = secrets.token_hex(32)
                    srv = socket.create_server(("0.0.0.0", base_port + i), reuse_port=False)
                    srv.settimeout(max(1.0, min(120.0, end - time.monotonic())))
                    try:
                        spec.update(host=public_ip, port=base_port + i, token=token)
                        p = spawn(
                            "SPEC = " + repr(json.dumps(spec)) + "\n" + SENDER,
                            out=subprocess.DEVNULL,
                        )
                        for _ in range(8):  # ignore stray connections without the token
                            conn, _addr = srv.accept()
                            conn.settimeout(max(1.0, end - time.monotonic()))
                            f = conn.makefile("rb", buffering=1 << 22)
                            if f.readline(80).strip() == token.encode():
                                break
                            f.close()
                            conn.close()
                        else:
                            raise reject("network_rescue_stream_unauthenticated")
                        try:
                            receive(f, want)
                        finally:
                            f.close()
                            conn.close()
                        if p.wait(timeout=max(1.0, end - time.monotonic())) != 0:
                            raise reject("network_export_failed")
                    finally:
                        srv.close()
                except (tarfile.ReadError, EOFError, OSError) as exc:
                    late = time.monotonic() > end
                    errors.append(
                        reject(
                            "network_rescue_deadline_exceeded"
                            if late
                            else "network_rescue_stream_truncated"
                        )
                    )
                    errors[-1].__cause__ = exc
                    kill_all()
                except BaseException as exc:  # noqa: BLE001  (first error re-raised below)
                    errors.append(exc)
                    kill_all()

            threads = [threading.Thread(target=worker, args=(i, g)) for i, g in enumerate(groups)]
            for t in threads:
                t.start()
            for t in threads:
                t.join()
            if errors:
                raise errors[0]
            with archive.open("xb") as sink, tarfile.open(fileobj=sink, mode="w") as out:
                for name in sorted(files):
                    if time.monotonic() > end:
                        raise reject("network_rescue_deadline_exceeded")
                    info = tarfile.TarInfo("out/" + name)
                    info.size, info.mode = files[name]["bytes"], 0o600
                    with (blobs / files[name]["sha256"]).open("rb") as blob:
                        out.addfile(info, blob)
                info = tarfile.TarInfo("manifest.json")
                info.size, info.mode = len(meta), 0o600
                out.addfile(info, io.BytesIO(meta))
                out.close()
                sink.flush()
                import os

                os.fsync(sink.fileno())
        finally:
            timer.cancel()
            kill_all()
            import shutil

            shutil.rmtree(blobs, ignore_errors=True)

    return stream


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--run-dir", type=Path, required=True)
    parser.add_argument("--role", choices=("h0", "h1"), required=True)
    tree = os.environ.get("HT_TREE") or sys.exit("HT_TREE must name the controller tree")
    parser.add_argument("--tree", type=Path, default=Path(tree))
    parser.add_argument("--via", choices=("ssh", "tcp", "range"), required=True)
    parser.add_argument("--workers", type=int, default=4)
    parser.add_argument("--public-ip", default="")
    parser.add_argument("--base-port", type=int, default=18560)
    args = parser.parse_args(argv)
    if args.via in ("tcp", "range") and not args.public_ip:
        raise SystemExit("--public-ip required for --via tcp")
    from hypertrain.gpu_ops.journal import Journal
    from hypertrain.gpu_ops.launcher import Orchestrator
    from hypertrain.gpu_ops.provider import Provider, read_api_key

    cfg = json.loads((args.run_dir / "config.json").read_text())
    journal = Journal(args.run_dir)
    opened = journal.last("transaction_open")
    if cfg.get("cleanup_contract") != "network-v2" or opened is None:
        raise SystemExit("not a network-v2 transaction")
    provider = Provider(
        str(cfg["base_url"]),
        read_api_key(str(cfg["key_file"])),
        args.run_dir,
        journal,
        bool(opened["live"]),
    )
    lifecycle = Orchestrator(dict(cfg), journal, provider, args.run_dir, actor="rescue-parallel")
    if lifecycle.cleanup_started() or lifecycle.receipt(args.role) is None:
        raise SystemExit("cleanup started or role not owned")
    orch = load_orchestrate(args.tree)
    profile = json.loads(Path(cfg["network_profile_file"]).read_bytes())
    runtime = orch.NetworkRuntime(lifecycle, profile)
    python = lifecycle.cfg.get("remote_python", profile["runtime"]["remote_python"])
    retained = orch.artifact_budget(profile)["retained_bytes_per_host"]
    orch.stream_bounded = make_stream(
        orch,
        lifecycle.remote_root(args.role),
        python,
        retained,
        args.via,
        args.workers,
        args.public_ip,
        args.base_port,
    )
    row = runtime.rescue(args.role, fresh=True)
    print(
        json.dumps(
            {
                "role": args.role,
                "generation": row["generation"],
                "tar": row["tar"],
                "tar_sha256": row["tar_sha256"],
                "files": len(row["files"]),
                "elapsed_seconds": round(row["elapsed_seconds"], 1),
            },
            sort_keys=True,
        )
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
