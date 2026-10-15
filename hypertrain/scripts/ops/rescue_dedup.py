"""Native network-v2 rescue with deduplicated transport (r14 recovery adapter, outside the 118 map).

Runs the unchanged NetworkRuntime.rescue(role, fresh=True): same generation, same pinned SSH,
same bound/deadline, same verify_export, same remote-digest check and the same native
network_rescued journal row. Only stream_bounded is replaced in this process: the host sends
each distinct file content once and the exact export tar (out/<name> + manifest.json) is
rebuilt locally from hash-verified blobs. Read-only on the host; the producer is sent on stdin.
"""

# ruff: noqa: E501  (verbatim controller tool)
from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import os
import re
import shlex
import shutil
import signal
import subprocess
import sys
import tarfile
import threading
import time
from collections.abc import Callable
from pathlib import Path, PurePosixPath

HEX = re.compile(r"[0-9a-f]{64}")
MANIFEST_MAX = 1048576  # verify_export's own manifest bound
MAX_FILES = 100000

PRODUCER = r"""
import hashlib, io, json, os, sys, tarfile
files, sent = {}, set()
with tarfile.open(fileobj=sys.stdout.buffer, mode="w|", format=tarfile.PAX_FORMAT) as out:
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
            sha = digest.hexdigest()
            files[os.path.relpath(path, "out")] = {"bytes": size, "sha256": sha}
            if sha in sent:
                continue
            with open(path, "rb") as f:
                info = tarfile.TarInfo("blobs/" + sha)
                info.size = os.fstat(f.fileno()).st_size
                out.addfile(info, f)
            sent.add(sha)
    meta = json.dumps(files, sort_keys=True).encode()
    info = tarfile.TarInfo("manifest.json")
    info.size = len(meta)
    out.addfile(info, io.BytesIO(meta))
sys.stdout.buffer.flush()
print(hashlib.sha256(meta).hexdigest(), file=sys.stderr)
"""


def native_command(root: str, python: str) -> str:
    """Byte-identical to the command NetworkRuntime.rescue builds (orchestrate.py:2546-2550)."""
    return (
        f"cd {shlex.quote(root)} && env PYTHONPATH=src {shlex.quote(python)} "
        "experiments/gpu_network_v2/orchestrate.py export out - "
        "experiments/gpu_network_v2/profile.json"
    )


def command_mode(command: str, root: str, python: str) -> tuple[str, list[str]] | None:
    """(mode, known) of a native export command: ("full", []), ("hashes", []) or
    ("dedup", [KNOWN_SHA256 ...]) as NetworkRuntime.rescue appends them; None otherwise."""
    base = native_command(root, python)
    if command == base:
        return "full", []
    words = command.removeprefix(base + " ").split(" ") if command.startswith(base + " ") else []
    if words == ["hashes"]:
        return "hashes", []
    if words[:1] == ["dedup"] and all(HEX.fullmatch(k) for k in words[1:]):
        if " ".join([base, *words]) == command:
            return "dedup", words[1:]
    return None


def checked_manifest(raw: bytes, reject: Callable[[str], Exception]) -> dict:
    files = json.loads(raw)
    if not isinstance(files, dict) or not 0 < len(files) <= MAX_FILES:
        raise reject("network_rescue_dedup_manifest_shape")
    for name, want in files.items():
        path = PurePosixPath(name)
        if (
            not isinstance(name, str)
            or not name
            or "\\" in name
            or path.is_absolute()
            or str(path) != name
            or any(part in ("", ".", "..") for part in path.parts)
            or not isinstance(want, dict)
            or set(want) != {"bytes", "sha256"}
            or type(want["bytes"]) is not int
            or want["bytes"] < 0
            or not isinstance(want["sha256"], str)
            or not HEX.fullmatch(want["sha256"])
        ):
            raise reject("network_rescue_dedup_manifest_entry")
    return files


def make_stream(orch, root: str, python: str, retained: int):
    """Drop-in for orchestrate.stream_bounded with the same arguments and failure contract."""
    reject = orch.Reject
    native_stream = orch.stream_bounded

    def stream(argv: list[str], archive: Path, log: Path, bound: int, timeout: float) -> None:
        mode = command_mode(argv[-1], root, python)
        if mode is None:
            raise reject("network_rescue_dedup_unexpected_command")
        if mode[0] != "full":
            # hashes / dedup exports already carry each needed content once: native stream.
            return native_stream(argv, archive, log, bound, timeout)
        end = time.monotonic() + timeout
        blobs = archive.parent / (archive.name + ".blobs")
        blobs.mkdir(mode=0o700)
        try:
            with log.open("wb") as err:
                proc = subprocess.Popen(  # noqa: S603  (pinned argv, only the command differs)
                    [*argv[:-1], f"cd {shlex.quote(root)} && {shlex.quote(python)} -"],
                    stdin=subprocess.PIPE,
                    stdout=subprocess.PIPE,
                    stderr=err,
                    start_new_session=True,
                )

                def kill() -> None:
                    try:
                        os.killpg(proc.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass

                timer = threading.Timer(timeout, kill)
                timer.start()
                meta, sizes, received, ok = None, {}, 0, False
                try:
                    assert proc.stdin is not None and proc.stdout is not None
                    proc.stdin.write(PRODUCER.encode())
                    proc.stdin.close()
                    with tarfile.open(fileobj=proc.stdout, mode="r|") as source:
                        for member in source:
                            if meta is not None or not member.isreg():
                                raise reject("network_rescue_dedup_member")
                            received += member.size
                            if received > bound:
                                raise reject("network_rescue_archive_budget_exceeded")
                            data = source.extractfile(member)
                            assert data is not None
                            if member.name == "manifest.json":
                                if member.size > MANIFEST_MAX:
                                    raise reject("rescue_manifest_missing_or_oversized")
                                meta = data.read(MANIFEST_MAX + 1)
                                continue
                            sha = member.name.removeprefix("blobs/")
                            if member.name != "blobs/" + sha or not HEX.fullmatch(sha):
                                raise reject("network_rescue_dedup_member")
                            if sha in sizes or len(sizes) >= MAX_FILES:
                                raise reject("network_rescue_dedup_member")
                            digest, size = hashlib.sha256(), 0
                            with (blobs / sha).open("xb") as sink:
                                while block := data.read(1 << 22):
                                    if time.monotonic() > end:
                                        raise reject("network_rescue_deadline_exceeded")
                                    digest.update(block)
                                    sink.write(block)
                                    size += len(block)
                            if digest.hexdigest() != sha or size != member.size:
                                raise reject("rescue_content_hash_mismatch")
                            sizes[sha] = size
                    ok = True
                except (tarfile.ReadError, EOFError) as exc:  # stream cut (kill/ssh drop)
                    late = time.monotonic() > end
                    raise reject(
                        "network_rescue_deadline_exceeded"
                        if late
                        else "network_rescue_stream_truncated"
                    ) from exc
                finally:
                    if not ok:
                        kill()
                    proc.stdout.close() if proc.stdout else None
                    rc = proc.wait()
                    timer.cancel()
            if rc:
                raise reject("network_export_failed")
            if meta is None or len(meta) > MANIFEST_MAX:
                raise reject("rescue_manifest_missing_or_oversized")
            files = checked_manifest(meta, reject)
            if sum(w["bytes"] for w in files.values()) > retained:
                raise reject("actual_artifact_budget_exceeded")
            if {w["sha256"]: w["bytes"] for w in files.values()} != sizes:
                raise reject("network_rescue_dedup_blob_set")
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
                out.addfile(info, __import__("io").BytesIO(meta))
                out.close()
                sink.flush()
                os.fsync(sink.fileno())
        finally:
            shutil.rmtree(blobs, ignore_errors=True)

    return stream


def load_orchestrate(tree: Path):
    """Same module file the native cleanup loads (launcher.py:720-730)."""
    spec = importlib.util.spec_from_file_location(
        "network_cleanup_runtime", tree / "experiments/gpu_network_v2/orchestrate.py"
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--run-dir", type=Path, required=True)
    parser.add_argument("--role", choices=("h0", "h1"), required=True)
    tree = os.environ.get("HT_TREE") or sys.exit("HT_TREE must name the controller tree")
    parser.add_argument("--tree", type=Path, default=Path(tree))
    args = parser.parse_args(argv)
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
    lifecycle = Orchestrator(dict(cfg), journal, provider, args.run_dir, actor="rescue-dedup")
    if lifecycle.cleanup_started() or lifecycle.receipt(args.role) is None:
        raise SystemExit("cleanup started or role not owned")
    orch = load_orchestrate(args.tree)
    profile = json.loads(Path(cfg["network_profile_file"]).read_bytes())
    runtime = orch.NetworkRuntime(lifecycle, profile)
    python = lifecycle.cfg.get("remote_python", profile["runtime"]["remote_python"])
    retained = orch.artifact_budget(profile)["retained_bytes_per_host"]
    orch.stream_bounded = make_stream(orch, lifecycle.remote_root(args.role), python, retained)
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
