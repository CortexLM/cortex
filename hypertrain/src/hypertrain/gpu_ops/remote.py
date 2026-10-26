"""SSH transport with experiment-scoped ED25519 TOFU.

Dedicated known_hosts (0600), StrictHostKeyChecking=yes, global/user files ignored. A first
observation is pinned once; any later scan returning a different key is rejected.
"""

from __future__ import annotations

import ipaddress
import os
import re
import shlex
import subprocess
import time
from collections.abc import Callable
from dataclasses import dataclass, field
from pathlib import Path

from hypertrain.gpu_ops.journal import Journal, fsha

_HOST = re.compile(r"[A-Za-z0-9][A-Za-z0-9.-]{0,252}")
_KEY = re.compile(r"ssh-ed25519 [A-Za-z0-9+/]{68}")


class TrustError(Exception):
    pass


class RemoteError(Exception):
    pass


def valid_endpoint(host: object, port: object) -> tuple[str, int]:
    if type(port) is not int or not 1 <= port <= 65535 or not isinstance(host, str):
        raise TrustError("invalid ssh endpoint")
    try:
        return str(ipaddress.ip_address(host)), port
    except ValueError:
        if not _HOST.fullmatch(host):
            raise TrustError("invalid ssh host") from None
        return host, port


def ensure_identity(run_dir: Path) -> Path:
    key = run_dir / "id_ed25519"
    if not key.exists():
        subprocess.run(  # noqa: S603  (fixed argv, no shell)
            [  # noqa: S607  (ssh-keygen from PATH, same as ssh/scp)
                "ssh-keygen",
                "-q",
                "-t",
                "ed25519",
                "-N",
                "",
                "-C",
                "hypertrain-phase-a",
                "-f",
                str(key),
            ],
            check=True,
            capture_output=True,
        )
    os.chmod(key, 0o600)
    return key


@dataclass
class Ssh:
    host: str
    port: int
    user: str
    identity: Path
    known_hosts: Path
    journal: Journal
    role: str
    timeout: int = 900
    sleep: Callable[[float], None] = field(default=time.sleep)
    retry_seconds: float = 15.0

    def __post_init__(self) -> None:
        self.host, self.port = valid_endpoint(self.host, self.port)

    def _hostpat(self) -> str:
        return f"[{self.host}]:{self.port}"

    def trust(self, attempts: int = 5) -> str:
        """Pin (or re-verify) the host's ED25519 key; returns 'pinned' or 'verified'."""
        seen: set[str] = set()
        for attempt in range(attempts):
            proc = subprocess.run(  # noqa: S603  (fixed argv; host validated)
                ["ssh-keyscan", "-T", "10", "-t", "ed25519", "-p", str(self.port), self.host],  # noqa: S607
                capture_output=True,
                text=True,
                timeout=60,
            )
            keys = {
                m.group(0)
                for line in proc.stdout.splitlines()
                if not line.startswith("#") and (m := _KEY.search(line))
            }
            seen |= keys
            self.journal.append(
                "keyscan", role=self.role, attempt=attempt, rc=proc.returncode, keys=sorted(keys)
            )
            if keys:
                break
            self.sleep(self.retry_seconds)
        if len(seen) != 1:
            raise TrustError(f"keyscan returned {len(seen)} distinct ed25519 keys")
        key = seen.pop()
        line = f"{self._hostpat()} {key}\n"
        if self.known_hosts.exists():
            if self.known_hosts.read_text() != line:
                self.journal.append("host_key_changed", role=self.role, observed=key)
                raise TrustError("host key changed since first observation")
            return "verified"
        tmp = self.known_hosts.with_suffix(".tmp")
        fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(fd, "w") as f:
            f.write(line)
            f.flush()
            os.fsync(f.fileno())
        os.replace(tmp, self.known_hosts)
        self.journal.append(
            "host_key_pinned", role=self.role, key=key, known_hosts=str(self.known_hosts)
        )
        return "pinned"

    def _opts(self) -> list[str]:
        return [
            "-o",
            "StrictHostKeyChecking=yes",
            "-o",
            f"UserKnownHostsFile={self.known_hosts}",
            "-o",
            "GlobalKnownHostsFile=/dev/null",
            "-o",
            "IdentitiesOnly=yes",
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=20",
            "-o",
            "ServerAliveInterval=15",
            "-i",
            str(self.identity),
        ]

    def run(self, command: str, tag: str, log_dir: Path) -> subprocess.CompletedProcess[str]:
        argv = ["ssh", *self._opts(), "-p", str(self.port), f"{self.user}@{self.host}", command]
        proc = subprocess.run(  # noqa: S603  (argv list, validated endpoint)
            argv, capture_output=True, text=True, timeout=self.timeout
        )
        log = log_dir / f"{self.role}-{tag}.log"
        log.write_text(
            f"$ {command}\nrc={proc.returncode}\n"
            f"--stdout--\n{proc.stdout}\n--stderr--\n{proc.stderr}"
        )
        self.journal.append("remote_cmd", role=self.role, tag=tag, rc=proc.returncode, log=log.name)
        return proc

    def _stream(self, command: str, tag: str, stdin: bytes | None) -> bytes:
        argv = ["ssh", *self._opts(), "-p", str(self.port), f"{self.user}@{self.host}", command]
        proc = subprocess.run(  # noqa: S603  (argv list, validated endpoint)
            argv, input=stdin or b"", capture_output=True, timeout=self.timeout
        )
        self.journal.append(
            "ssh_stream",
            role=self.role,
            tag=tag,
            rc=proc.returncode,
            stderr=proc.stderr[-500:].decode(errors="replace"),
        )
        if proc.returncode:
            raise RemoteError(f"ssh {tag} rc={proc.returncode}")
        return proc.stdout

    def put(self, local: Path, remote: str) -> None:
        q = shlex.quote(remote)
        self._stream(f"cat > {q}.part && mv {q}.part {q}", "put", local.read_bytes())

    def get(self, remote: str, local: Path) -> str:
        local.write_bytes(self._stream(f"cat {shlex.quote(remote)}", "get", None))
        return fsha(local)

    def remote_sha256(self, remote: str, log_dir: Path) -> str:
        proc = self.run(f"sha256sum {shlex.quote(remote)}", "sha-" + Path(remote).name, log_dir)
        m = re.match(r"([0-9a-f]{64}) ", proc.stdout)
        if proc.returncode or not m:
            raise RemoteError(f"remote sha256 failed for {remote}")
        return m.group(1)
