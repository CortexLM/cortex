"""Generic in-guest boundary for an evaluator to call an untrusted miner program.

Only explicitly supplied inputs and artifact bytes enter the child namespace.
Evaluator files, holdouts, credentials and report paths are never mounted there.
The guest must provide bubblewrap, prlimit and libseccomp; there is no fallback.
"""

from __future__ import annotations

import ctypes
import ctypes.util
import errno
import json
import os
import signal
import stat
import subprocess
import tempfile
from pathlib import Path

from .guest import GuestIdentity
from .models import VmError

MAX_JSON = 1024 * 1024
MAX_CANDIDATE = 64 * 1024 * 1024


def _socket_filter():
    library = ctypes.util.find_library("seccomp")
    if library is None:
        raise VmError("candidate isolation requires libseccomp")
    seccomp = ctypes.CDLL(library)
    seccomp.seccomp_init.argtypes = [ctypes.c_uint32]
    seccomp.seccomp_init.restype = ctypes.c_void_p
    seccomp.seccomp_release.argtypes = [ctypes.c_void_p]
    seccomp.seccomp_syscall_resolve_name.argtypes = [ctypes.c_char_p]
    seccomp.seccomp_syscall_resolve_name.restype = ctypes.c_int
    seccomp.seccomp_rule_add.argtypes = [
        ctypes.c_void_p,
        ctypes.c_uint32,
        ctypes.c_int,
        ctypes.c_uint,
    ]
    seccomp.seccomp_export_bpf.argtypes = [ctypes.c_void_p, ctypes.c_int]
    context = seccomp.seccomp_init(
        0x7FFF0000
    )  # default allow; namespaces constrain files/processes
    if not context:
        raise VmError("candidate seccomp initialization failed")
    output = tempfile.TemporaryFile()
    try:
        # Network namespaces do not isolate AF_VSOCK. Deny socket creation itself,
        # plus namespace/kernel introspection escape surfaces for untrusted code.
        for name in (
            "socket",
            "socketpair",
            "ptrace",
            "process_vm_readv",
            "process_vm_writev",
            "bpf",
            "perf_event_open",
            "keyctl",
            "add_key",
            "request_key",
            "mount",
            "umount2",
            "pivot_root",
            "chroot",
            "setns",
            "unshare",
            "reboot",
            "kexec_load",
            "open_by_handle_at",
            "io_uring_setup",
            "io_uring_enter",
            "io_uring_register",
        ):
            number = seccomp.seccomp_syscall_resolve_name(name.encode())
            if (
                number < 0
                or seccomp.seccomp_rule_add(context, 0x00050000 | errno.EPERM, number, 0) != 0
            ):
                raise VmError("candidate seccomp rule unavailable")
        if seccomp.seccomp_export_bpf(context, output.fileno()) != 0:
            raise VmError("candidate seccomp export failed")
        output.seek(0)
        return output
    except BaseException:
        output.close()
        raise
    finally:
        seccomp.seccomp_release(context)


def candidate_command(
    artifact: Path,
    filter_fd: int,
    argv: list[str],
    *,
    wall_seconds: int,
    memory_mib: int,
) -> list[str]:
    if not argv or len(argv) > 64 or any(not a or len(a) > 4096 or "\0" in a for a in argv):
        raise VmError("invalid candidate argv")
    if not 1 <= wall_seconds <= 7200 or not 64 <= memory_mib <= 32768:
        raise VmError("candidate resource limit outside guest ceilings")
    args = [
        "/usr/bin/prlimit",
        f"--as={memory_mib * 1024 * 1024}",
        f"--cpu={wall_seconds}",
        "--nproc=64",
        "--nofile=64",
        "--fsize=1048576",
        "--",
        "/usr/bin/bwrap",
        "--unshare-all",
        "--unshare-user",
        "--disable-userns",
        "--die-with-parent",
        "--new-session",
        "--cap-drop",
        "ALL",
        "--uid",
        "65534",
        "--gid",
        "65534",
        "--clearenv",
        "--setenv",
        "PATH",
        "/usr/local/bin:/usr/bin:/bin",
        "--setenv",
        "LANG",
        "C.UTF-8",
        "--setenv",
        "LD_LIBRARY_PATH",
        "/usr/local/lib:/usr/lib",
        "--setenv",
        "HOME",
        "/tmp",
        "--ro-bind",
        "/usr",
        "/usr",
        "--proc",
        "/proc",
        "--dev",
        "/dev",
        "--size",
        "16777216",
        "--tmpfs",
        "/tmp",
        "--ro-bind",
        str(artifact),
        "/artifact",
        "--perms",
        "0777",
        "--size",
        "67108864",
        "--tmpfs",
        "/work",
        "--chdir",
        "/work",
        "--seccomp",
        str(filter_fd),
    ]
    for name in ("lib", "lib64", "bin", "sbin"):
        path = Path("/") / name
        if path.is_symlink():
            args.extend(["--symlink", os.readlink(path), str(path)])
        elif path.exists():
            args.extend(["--ro-bind", str(path), str(path)])
    args.extend(["--", *argv])
    return args


def _copy_artifact(source: Path, destination: Path) -> None:
    """Copy regular bytes only; never follow miner-controlled filesystem links."""
    if source.is_symlink() or not source.is_dir():
        raise VmError("verified candidate artifact directory required")
    destination.mkdir(mode=0o755)
    total, count = 0, 0
    for path in source.rglob("*"):
        relative = path.relative_to(source)
        target = destination / relative
        info = path.lstat()
        if stat.S_ISDIR(info.st_mode):
            target.mkdir(mode=0o755, parents=True, exist_ok=True)
            continue
        if not stat.S_ISREG(info.st_mode):
            raise VmError("candidate artifact contains a special file")
        count += 1
        total += info.st_size
        if count > 10_000 or total > MAX_CANDIDATE:
            raise VmError("candidate artifact exceeds bounds")
        fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
        with os.fdopen(fd, "rb") as content:
            actual = os.fstat(content.fileno())
            if not stat.S_ISREG(actual.st_mode) or actual.st_size != info.st_size:
                raise VmError("candidate artifact changed while copying")
            target.parent.mkdir(mode=0o755, parents=True, exist_ok=True)
            with target.open("xb") as output:
                remaining = info.st_size
                while remaining:
                    chunk = content.read(min(remaining, 65536))
                    if not chunk:
                        raise VmError("candidate artifact changed while copying")
                    output.write(chunk)
                    remaining -= len(chunk)
                if content.read(1):
                    raise VmError("candidate artifact changed while copying")
            target.chmod(0o644)


def run_candidate(
    argv: list[str],
    inputs: object,
    *,
    wall_seconds: int = 30,
    memory_mib: int = 1024,
) -> object:
    """Send JSON on stdin; return bounded JSON stdout from a networkless child.

    The generated evaluator retains expected answers and computes the score itself.
    Candidate stdout is data; it is never copied into the trusted report.json.
    """
    identity = GuestIdentity.from_system()
    if identity.kind != "experiment":
        raise VmError("candidate execution requires a dedicated experiment guest")
    artifact = Path(os.environ.get("PROOF_ARTIFACT_DIR", ""))
    if (
        not artifact.is_absolute()
        or not artifact.is_relative_to(Path("/workspace"))
        or artifact != artifact.resolve()
        or not artifact.is_dir()
    ):
        raise VmError("verified candidate artifact directory required")
    return _run_isolated(artifact, argv, inputs, wall_seconds=wall_seconds, memory_mib=memory_mib)


def _run_isolated(
    artifact: Path,
    argv: list[str],
    inputs: object,
    *,
    wall_seconds: int,
    memory_mib: int,
) -> object:
    raw = json.dumps(inputs, allow_nan=False).encode()
    if len(raw) > MAX_JSON:
        raise VmError("candidate input too large")
    with (
        tempfile.TemporaryDirectory(prefix="proof-candidate-") as directory,
        _socket_filter() as rules,
    ):
        root = Path(directory)
        root.chmod(0o755)
        candidate = root / "artifact"
        _copy_artifact(artifact, candidate)
        command = candidate_command(
            candidate, rules.fileno(), argv, wall_seconds=wall_seconds, memory_mib=memory_mib
        )
        # File output is bounded by RLIMIT_FSIZE, avoiding an unbounded PIPE buffer.
        with tempfile.TemporaryFile() as output, tempfile.TemporaryFile() as errors:
            process = subprocess.Popen(
                command,
                stdin=subprocess.PIPE,
                stdout=output,
                stderr=errors,
                pass_fds=(rules.fileno(),),
                start_new_session=True,
                env={"PATH": "/usr/bin:/bin"},
            )
            try:
                process.communicate(raw, timeout=wall_seconds)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait()
                raise VmError("candidate deadline exceeded") from None
            finally:
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
            if process.returncode:
                # Exit status is useful evidence; stderr may contain miner secrets.
                raise VmError(f"candidate execution failed (exit {process.returncode})")
            output.seek(0)
            data = output.read(MAX_JSON + 1)
            if len(data) > MAX_JSON:
                raise VmError("candidate output too large")
            try:
                return json.loads(
                    data, parse_constant=lambda _: (_ for _ in ()).throw(ValueError())
                )
            except (ValueError, UnicodeError):
                raise VmError("candidate returned invalid JSON") from None
