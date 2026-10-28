"""`python -m hypertrain.beacon push`: keep a challenge service fed with verified quicknet rounds.

Every round comes from >= 2 agreeing relays and is BLS-verified (DrandQuicknet) before it is
posted to `/v1/admin/beacon`; the service verifies it again on ingest. Per tick it posts the latest
round first, then one older missing round, the oldest (all of them with `--once`, which has no next
tick). A missing round stays in memory until the service answers 2xx; only a restart loses the list.
Per post:
- service 2xx: next round;
- service 401/403: exit 2 (token);
- service 408/429/5xx, network error, relays unavailable or failing verification: one failure,
  retry after `--interval`, exit 1 after `--max-failures` in a row (or at once with `--once`);
- any other service status (409 conflicting round, never overwritten; 400; 404; 3xx) or a
  malformed `/v1/beacon/latest` body: exit 1.
"""

from __future__ import annotations

import json
import re
import signal
import sys
import time
from pathlib import Path
from typing import Any

import httpx

from hypertrain.beacon.core import BeaconError
from hypertrain.beacon.drand import DrandQuicknet

# Unchanged by JSON escaping and URL encoding, so redacting an echoed token by exact match holds.
# Also keeps it out of exceptions: httpx quotes an illegal header value (e.g. a newline) verbatim.
_TOKEN = re.compile(rb"[A-Za-z0-9._~-]+")
# Relay URLs (DrandQuicknet errors echo them) may carry userinfo or an `?apikey=` query.
# Userinfo runs to the last `@` before the path: a password may hold quotes or `@`.
_USERINFO = re.compile(r"(\w[\w+.-]*://)[^/\s]*@")
_QUERY = re.compile(r"[?#][^\s'\"]*")
# A path segment (not a `//host`) of >= 24 token chars is taken for a credential (`/key/<apikey>`).
_PATH_TOKEN = re.compile(r"(?<=[^/]/)[A-Za-z0-9_-]{24,}(?![A-Za-z0-9_-])")
# A rejected (429) or cut-off (408) POST was not applied, so replaying it is safe.
_RETRY = frozenset({408, 429})


def redact_urls(text: str) -> str:
    return _PATH_TOKEN.sub("***", _QUERY.sub("", _USERINFO.sub(r"\1***@", text)))


class ServiceError(Exception):
    """Non-2xx or malformed answer from the challenge service."""

    def __init__(self, status: int, message: str) -> None:
        super().__init__(message)
        self.status = status


def plan(known: int, latest: int, catch_up: int) -> list[int]:
    """Rounds to push, ascending: every round after `known`, at most the last `catch_up`."""
    return list(range(max(known + 1, latest - catch_up + 1), latest + 1))


def read_token(path: Path) -> str:
    """The admin bearer in `path`. Errors name the file, never its content."""
    raw = path.read_bytes().strip()
    if not _TOKEN.fullmatch(raw):
        raise ValueError(f"{path}: token must match [A-Za-z0-9._~-]+")
    return raw.decode()


class Pusher:
    def __init__(
        self, service: httpx.Client, relays: DrandQuicknet, token: str, catch_up: int
    ) -> None:
        self.service = service
        self.relays = relays
        self.catch_up = catch_up
        self._token = token
        # Rounds still to push, ascending. Kept across ticks: the service reports only its highest
        # round, so once the head is in nothing else says an older round is missing. A round leaves
        # the list on a 2xx and never otherwise.
        self.pending: list[int] = []

    def _body(self, r: httpx.Response) -> str:
        # Untrusted text: a proxy echoing request headers must not leak the token. Redact, then cut.
        return r.text.replace(self._token, "***")[:500]

    def _check(self, r: httpx.Response, what: str) -> None:
        if not r.is_success:
            raise ServiceError(r.status_code, f"{what}: HTTP {r.status_code}: {self._body(r)}")

    def _known(self) -> int:
        r = self.service.get("/v1/beacon/latest")
        self._check(r, "GET /v1/beacon/latest")
        try:
            known = r.json().get("round")
        except (ValueError, AttributeError, RecursionError):
            known = None
        if type(known) is not int or known < 0:
            raise ServiceError(r.status_code, f"GET /v1/beacon/latest: bad body {self._body(r)}")
        return known

    def tick(self, drain: bool = False) -> dict[str, Any]:
        latest = self.relays.latest()
        known = self._known()
        # `plan` bounds what is due when the gap is first seen (the last `--catch-up` rounds after
        # the highest the service holds); what it returned stays due until the service has it.
        self.pending = sorted({*self.pending, *plan(known, latest.round, self.catch_up)})
        # Head first: the freshness gate reads the newest round, so it reopens as soon as that one
        # is in. Then the oldest pending round, one per tick, so refilling a backlog (after a store
        # lock) does not close the gate again; `drain` (--once, no next tick) pushes all of them.
        # The service reports only its highest round, so a pusher restarted before the list is
        # empty loses it and the rounds still on it stay missing: scripts/ops/s100_test/
        # beacon_gaps.py finds them.
        order = [latest.round] if latest.round in self.pending else []
        older = [rnd for rnd in self.pending if rnd != latest.round]
        order += older if drain else older[:1]
        pushed: list[int] = []
        for rnd in order:
            br = latest if rnd == latest.round else self.relays.get(rnd)
            r = self.service.post(
                "/v1/admin/beacon",
                json={"round": br.round, "signature": br.signature, "randomness": br.randomness},
                headers={"Authorization": f"Bearer {self._token}"},
            )
            self._check(r, f"round {rnd}")
            self.pending.remove(rnd)
            pushed.append(rnd)
        return {
            "pushed": pushed,
            "latest": latest.round,
            "known": known,
            "pending": len(self.pending),
        }

    def run(self, interval: float, once: bool, max_failures: int) -> int:
        """Tick until stopped: 0 on SIGTERM/Ctrl-C or after `once`, 1 protocol, 2 token."""
        failures = 0
        previous = signal.signal(signal.SIGTERM, signal.default_int_handler)
        try:
            while True:
                try:
                    print(json.dumps(self.tick(drain=once)), flush=True)
                    failures = 0
                # DrandQuicknet lets ValueError (relay body without an int round) and
                # RecursionError (absurdly nested relay JSON) through.
                except (
                    ServiceError,
                    BeaconError,
                    ValueError,
                    RecursionError,
                    httpx.HTTPError,
                ) as e:
                    if isinstance(e, ServiceError) and e.status < 500 and e.status not in _RETRY:
                        print(e, file=sys.stderr, flush=True)
                        return 2 if e.status in (401, 403) else 1
                    failures += 1
                    why = redact_urls(str(e))
                    msg = f"failure {failures}/{max_failures}: {type(e).__name__}: {why}"
                    print(msg, file=sys.stderr, flush=True)
                    if once or failures >= max_failures:
                        return 1
                if once:
                    return 0
                time.sleep(interval)
        except KeyboardInterrupt:
            return 0
        finally:
            signal.signal(signal.SIGTERM, previous)
