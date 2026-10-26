"""Vast HTTP adapter. Raw responses persisted 0600 before parsing; credentials never logged.

Writes (non-GET) are refused unless the base URL is loopback (mock) or ``live=True``.
"""

from __future__ import annotations

import base64
import json
import math
import os
import re
import stat
import time
import urllib.error
import urllib.parse
import urllib.request
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from hypertrain.gpu_ops.journal import Journal, durable_write, sha256

CAP = 4 * 1048576
DEFAULT_KEY_FILE = "/root/.config/image-training-pilot/vast-key"
Sleeper = Callable[[float], None]


class _NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *args: Any, **kwargs: Any) -> None:
        return None


_OPENER = urllib.request.build_opener(_NoRedirect)


class RefusedWrite(Exception):
    pass


class KeyFileError(Exception):
    pass


_REDACT = [
    (re.compile(r"(api_key=)[^&\s\"']+", re.I), r"\1***"),
    (re.compile(r"(Bearer\s+)[A-Za-z0-9._~+/=-]+"), r"\1***"),
]


def redact(text: str, secret: str | None = None) -> str:
    """Mask api_key query values, bearer tokens and the literal secret."""
    if secret:
        text = text.replace(secret, "***")
    for pattern, repl in _REDACT:
        text = pattern.sub(repl, text)
    return text


def read_api_key(path: str | Path = DEFAULT_KEY_FILE) -> str:
    """Key must be a regular file owned by us with mode 0600 (no group/other bits)."""
    p = Path(path)
    st = p.lstat()
    if not stat.S_ISREG(st.st_mode) or st.st_uid != os.getuid():
        raise KeyFileError("key file must be a regular file owned by the current user")
    if stat.S_IMODE(st.st_mode) & 0o077:
        raise KeyFileError("key file mode must be 0600")
    key = p.read_text().strip()
    if not re.fullmatch(r"[A-Za-z0-9]{16,128}", key):
        raise KeyFileError("key file content malformed")
    return key


def loopback(base: str) -> bool:
    return urllib.parse.urlsplit(base).hostname in ("127.0.0.1", "localhost", "::1")


@dataclass
class Response:
    status: int | None
    parsed: Any
    retry_after: float | None
    raw_file: str
    error: str | None

    def ok(self) -> bool:
        return self.status == 200 and isinstance(self.parsed, dict)


def list_rows(r: Response) -> list[dict[str, Any]] | None:
    """Complete instance list (HTTP 200, success true, no next_token, rows with int ids) or None.

    None means UNKNOWN; callers must never treat it as absence.
    """
    p = r.parsed if r.ok() else None
    if not p or p.get("success") is not True or p.get("next_token"):
        return None
    rows = p.get("instances")
    if not isinstance(rows, list):
        return None
    if not all(isinstance(i, dict) and type(i.get("id")) is int for i in rows):
        return None
    return rows


class Provider:
    def __init__(
        self,
        base: str,
        key: str,
        run_dir: str | Path,
        journal: Journal,
        live: bool,
        sleep: Sleeper | None = None,
    ) -> None:
        self.base, self.key, self.live = base.rstrip("/"), key, live
        self.writes_allowed = loopback(base) or live
        self.virtual = loopback(base) and os.environ.get("HT_GPU_VIRTUAL_TIME") == "1"
        self.offset = 0.0
        self.sleep = sleep or (self._virtual_sleep if self.virtual else time.sleep)
        self.raw = Path(run_dir) / "raw"
        self.raw.mkdir(mode=0o700, exist_ok=True)
        self.journal = journal
        self.pace: dict[str, float] = {}
        self.last: dict[str, float] = {}

    def _virtual_sleep(self, seconds: float) -> None:
        self.offset += max(0.0, seconds)

    def monotonic(self) -> float:
        return time.monotonic() + self.offset

    @staticmethod
    def endpoint(method: str, path: str) -> str:
        return method + " " + re.sub(r"/\d+(?=/|$)", "/N", path.split("?")[0])

    def call(
        self, method: str, path: str, tag: str, body: Any = None, timeout: float = 30
    ) -> Response:
        if method != "GET" and not self.writes_allowed:
            raise RefusedWrite(f"{method} {path.split('?')[0]} refused without --live")
        endpoint = self.endpoint(method, path)
        interval = self.pace.get(endpoint, self.pace.get("*", 0.0))
        gap = interval - (self.monotonic() - self.last.get(endpoint, -1e18))
        if gap > 0:
            self.journal.append("pace_wait", endpoint=endpoint, seconds=gap)
            self.sleep(gap)
        data = json.dumps(body).encode() if body is not None else None
        request = urllib.request.Request(  # noqa: S310  (base_url from operator config)
            self.base + path,
            data=data,
            method=method,
            headers={"Authorization": "Bearer " + self.key, "Content-Type": "application/json"},
        )
        t0 = time.time()
        status: int | None = None
        raw, error, retry_hdr = b"", None, None
        try:
            with _OPENER.open(request, timeout=timeout) as resp:
                status, raw = resp.status, resp.read(CAP + 1)
        except urllib.error.HTTPError as e:
            status, raw, retry_hdr = e.code, e.read(CAP + 1), e.headers.get("Retry-After")
        except Exception as e:  # network failure is evidence, never a silent pass
            error = redact(type(e).__name__ + ": " + str(e)[:500], self.key)
        self.last[endpoint] = self.monotonic()
        name = f"{time.time_ns()}-{os.getpid()}-{tag}.json"
        raw_text = redact(raw[:CAP].decode("utf-8", errors="replace"), self.key)
        record = {
            "method": method,
            "path": redact(path, self.key),
            "http_status": status,
            "error": error,
            "retry_after": retry_hdr,
            "requested_unix": t0,
            "response_unix": time.time(),
            "bytes": len(raw),
            "truncated": len(raw) > CAP,
            "sha256": sha256(raw),
            "raw_base64": base64.b64encode(raw_text.encode()).decode(),
        }
        durable_write(self.raw / name, json.dumps(record).encode())
        self.journal.append(
            "provider_call",
            vmono=self.last[endpoint],  # Response completion used by pacing, not post-fsync time.
            method=method,
            path=redact(path, self.key),
            http_status=status,
            error=error,
            raw_file=name,
            sha256=record["sha256"],
            write=method != "GET",
        )
        parsed = None
        if raw and len(raw) <= CAP:
            try:
                parsed = json.loads(raw)
            except ValueError:
                parsed = None
        retry: float | None
        try:
            retry = float(retry_hdr) if retry_hdr is not None else None
        except ValueError:
            retry = None
        if retry is not None and not (math.isfinite(retry) and retry >= 0):
            retry = None
        return Response(status, parsed, None if retry is None else min(retry, 30.0), name, error)
