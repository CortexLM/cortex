"""Interruption notices and the signed "leaving" message of the elastic core.

A NoticeSource tells a host that its provider will take it away: AwsSpotNotice reads the EC2
instance metadata service (IMDSv2) and FakeNoticeSource replays a schedule. A notice carries a
deadline on the host's own monotonic clock. The host answers with a LeaveNotice signed by the
server key it registered at join; the coordinator verifies it against that key, the epoch of
its own start and the live lease token before releasing the member's shards.
"""

from __future__ import annotations

import datetime as dt
import json
from collections.abc import Callable
from dataclasses import dataclass
from typing import Annotated, Literal, Protocol

import httpx
from pydantic import Field, StringConstraints

from hypertrain.elastic.clock import Clock
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages import SS58, Hex64
from hypertrain.protocol.messages_v2 import U64, Identifier, Positive, WireModel
from hypertrain.protocol.server_keys import Sig, check_sig

LEAVE_DOMAIN = b"hypertrain/elastic-leave/1|"
IMDS = "http://169.254.169.254"
SESSION_PATH = "/latest/api/token"
INSTANCE_ACTION_PATH = "/latest/meta-data/spot/instance-action"
SESSION_TTL_S = 21600
MAX_HEADS = 1024


class NoticeError(RuntimeError):
    pass


@dataclass(frozen=True, slots=True)
class Interruption:
    """`action` is the provider's verb (terminate, stop, hibernate); `deadline_ms` is when the
    host goes away, on the host's clock; `provider_time` is the provider's own timestamp."""

    action: str
    deadline_ms: int
    provider_time: str


class NoticeSource(Protocol):
    def poll(self) -> Interruption | None: ...


class FakeNoticeSource:
    """Delivers one scheduled interruption once the clock reaches `at_ms`."""

    def __init__(self, clock: Clock) -> None:
        self.clock = clock
        self._due: tuple[int, Interruption] | None = None

    def schedule(self, at_ms: int, deadline_ms: int, action: str = "terminate") -> None:
        if deadline_ms < at_ms:
            raise ValueError("a notice cannot arrive after its deadline")
        self._due = (at_ms, Interruption(action, deadline_ms, f"fake+{deadline_ms}ms"))

    def poll(self) -> Interruption | None:
        if self._due is None or self.clock.now_ms() < self._due[0]:
            return None
        return self._due[1]


def _utc_now() -> dt.datetime:
    return dt.datetime.now(dt.UTC)


class AwsSpotNotice:
    """EC2 Spot interruption notice through IMDSv2.

    GET /latest/meta-data/spot/instance-action answers 404 until EC2 schedules an interruption,
    then {"action": "terminate"|"stop"|"hibernate", "time": "YYYY-MM-DDTHH:MM:SSZ"} about two
    minutes ahead; hibernation starts at once, so a hibernate notice has no time left. The
    session token is fetched with PUT /latest/api/token and refreshed on 401. Any other answer,
    and an unreachable or silent IMDS, raises NoticeError: the caller keeps working under its
    lease, which still bounds the loss. In a container the token PUT needs an IMDS hop limit of
    2 or host networking, or it times out.
    """

    def __init__(
        self,
        clock: Clock,
        *,
        client: httpx.Client | None = None,
        base_url: str = IMDS,
        wall: Callable[[], dt.datetime] = _utc_now,
        timeout: float = 1.0,
    ) -> None:
        self.clock = clock
        self.wall = wall
        # IMDS is link-local: never through HTTP(S)_PROXY, as the AWS SDKs do
        self._client = client or httpx.Client(base_url=base_url, timeout=timeout, trust_env=False)
        self._token: tuple[str, int] | None = None

    def _send(self, method: str, path: str, headers: dict[str, str]) -> httpx.Response:
        try:
            return self._client.request(method, path, headers=headers)
        except httpx.HTTPError as e:
            raise NoticeError(f"IMDS unreachable ({type(e).__name__})") from None

    def _session_token(self) -> str:
        now = self.clock.now_ms()
        if self._token is not None and now < self._token[1]:
            return self._token[0]
        r = self._send(
            "PUT",
            SESSION_PATH,
            {"X-aws-ec2-metadata-token-ttl-seconds": str(SESSION_TTL_S)},
        )
        if r.status_code != 200 or not r.text:
            raise NoticeError(f"IMDS token: HTTP {r.status_code}")
        # refresh a minute early so a token never expires between two polls
        self._token = (r.text, now + (SESSION_TTL_S - 60) * 1000)
        return r.text

    def _instance_action(self) -> httpx.Response:
        headers = {"X-aws-ec2-metadata-token": self._session_token()}
        return self._send("GET", INSTANCE_ACTION_PATH, headers)

    def poll(self) -> Interruption | None:
        r = self._instance_action()
        if r.status_code == 401:
            self._token = None
            r = self._instance_action()
        if r.status_code == 404:
            return None
        if r.status_code != 200:
            raise NoticeError(f"IMDS instance-action: HTTP {r.status_code}")
        try:
            body = json.loads(r.content)
            action, stamp = body["action"], body["time"]
            when = dt.datetime.strptime(stamp, "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=dt.UTC)
        except (ValueError, KeyError, TypeError) as e:
            raise NoticeError(f"IMDS instance-action: malformed ({type(e).__name__})") from None
        if action not in ("terminate", "stop", "hibernate"):
            raise NoticeError(f"IMDS instance-action: unknown action {action!r}")
        left_ms = (
            0 if action == "hibernate" else (when - self.wall()) // dt.timedelta(milliseconds=1)
        )
        left_ms = max(0, left_ms)
        return Interruption(action, self.clock.now_ms() + left_ms, stamp)


Stamp = Annotated[str, StringConstraints(max_length=64)]


class ShardHead(WireModel):
    shard: U64
    head: Hex64 | None


class LeaveNotice(WireModel):
    """A member's last word: it leaves under lease `lease` of the coordinator start `epoch`
    (a log restored from a copy may reissue the token, never the epoch). `heads` are the last
    links it saw accepted and `provider_time` is the provider's interruption timestamp:
    evidence for the audit, kept on the member's Departure, never trusted over the
    coordinator's own heads."""

    version: Literal[1] = 1
    run_id: Identifier
    member: Identifier
    signer: SS58
    lease: Positive
    epoch: Hex64
    reason: Literal["interruption", "operator"]
    provider_time: Stamp
    heads: Annotated[list[ShardHead], Field(max_length=MAX_HEADS)]

    def signing_message(self) -> bytes:
        return LEAVE_DOMAIN + canonicalize(self.body(), allow_float=False)


class SignedLeave(WireModel):
    notice: LeaveNotice
    signature: Sig


def sign_leave(notice: LeaveNotice, key: Keypair) -> SignedLeave:
    if key.ss58 != notice.signer:
        raise ValueError("the notice names another signer")
    return SignedLeave(notice=notice, signature=key.sign(notice.signing_message()).hex())


def verify_leave(signed: SignedLeave) -> LeaveNotice:
    """The notice, if its signature verifies for the signer it names."""
    if not check_sig(signed.notice.signer, signed.notice.signing_message(), signed.signature):
        raise NoticeError("leave notice signature does not verify")
    return signed.notice
