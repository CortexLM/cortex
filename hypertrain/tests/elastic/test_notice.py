"""Interruption notices from EC2 IMDSv2 and the signed leave message."""

import datetime as dt
import json

import httpx
import pytest

from hypertrain.elastic.clock import FakeClock
from hypertrain.elastic.notice import (
    AwsSpotNotice,
    FakeNoticeSource,
    LeaveNotice,
    NoticeError,
    ShardHead,
    sign_leave,
    verify_leave,
)
from hypertrain.protocol.keys import Keypair

WALL = dt.datetime(2026, 10, 26, 12, 0, 0, tzinfo=dt.UTC)
ACTION = "/latest/meta-data/spot/instance-action"


class Imds:
    """IMDSv2: sessions from PUT /latest/api/token, instance-action 404 until a notice."""

    def __init__(self, status: int = 404, body: object = None) -> None:
        self.status, self.body = status, body
        self.sessions: list[str] = []
        self.expired: set[str] = set()

    def __call__(self, request: httpx.Request) -> httpx.Response:
        if request.method == "PUT" and request.url.path == "/latest/api/token":
            assert request.headers["X-aws-ec2-metadata-token-ttl-seconds"] == "21600"
            self.sessions.append(f"session-{len(self.sessions)}")
            return httpx.Response(200, text=self.sessions[-1])
        assert (request.method, request.url.path) == ("GET", ACTION)
        session = request.headers.get("X-aws-ec2-metadata-token")
        if session not in self.sessions or session in self.expired:
            return httpx.Response(401)
        if self.body is None:
            return httpx.Response(self.status)
        return httpx.Response(self.status, content=json.dumps(self.body).encode())


def source(imds: Imds, clock: FakeClock) -> AwsSpotNotice:
    client = httpx.Client(base_url="http://imds.test", transport=httpx.MockTransport(imds))
    return AwsSpotNotice(clock, client=client, wall=lambda: WALL)


def test_no_scheduled_interruption_is_no_notice_and_reuses_the_session() -> None:
    imds = Imds()
    spot = source(imds, FakeClock(5_000))
    assert spot.poll() is None
    assert spot.poll() is None
    assert imds.sessions == ["session-0"]


def test_the_deadline_is_the_provider_time_on_the_host_clock() -> None:
    imds = Imds(200, {"action": "terminate", "time": "2026-10-26T12:02:00Z"})
    notice = source(imds, FakeClock(5_000)).poll()
    assert notice is not None
    assert (notice.action, notice.deadline_ms) == ("terminate", 5_000 + 120_000)
    assert notice.provider_time == "2026-10-26T12:02:00Z"


def test_a_hibernate_notice_leaves_no_time() -> None:
    imds = Imds(200, {"action": "hibernate", "time": "2026-10-26T12:02:00Z"})
    notice = source(imds, FakeClock(5_000)).poll()
    assert notice is not None and notice.deadline_ms == 5_000


def test_an_expired_session_is_renewed_once() -> None:
    imds = Imds()
    spot = source(imds, FakeClock())
    assert spot.poll() is None
    imds.expired.add("session-0")
    assert spot.poll() is None
    assert imds.sessions == ["session-0", "session-1"]


@pytest.mark.parametrize(
    ("status", "body"),
    [
        (200, {"action": "reboot", "time": "2026-10-26T12:02:00Z"}),
        (200, {"action": "terminate", "time": "in two minutes"}),
        (200, {"action": "terminate"}),
        (500, None),
    ],
)
def test_an_unexpected_answer_raises(status: int, body: object) -> None:
    with pytest.raises(NoticeError):
        source(Imds(status, body), FakeClock()).poll()


def test_the_fake_source_delivers_its_notice_from_its_time_on() -> None:
    clock = FakeClock()
    fake = FakeNoticeSource(clock)
    fake.schedule(1_000, 121_000)
    assert fake.poll() is None
    clock.set(1_000)
    notice = fake.poll()
    assert notice is not None and notice.deadline_ms == 121_000


def test_the_leave_signature_covers_every_field() -> None:
    key = Keypair(bytes([7]) * 32)
    body = LeaveNotice(
        run_id="run-1",
        member="m01",
        signer=key.ss58,
        lease=4,
        reason="interruption",
        provider_time="2026-10-26T12:02:00Z",
        heads=[ShardHead(shard=2, head="ab" * 32)],
    )
    signed = sign_leave(body, key)
    assert verify_leave(signed) == body
    for change in (
        {"run_id": "run-2"},
        {"member": "m02"},
        {"lease": 5},
        {"reason": "operator"},
        {"provider_time": "2026-10-26T12:03:00Z"},
        {"heads": [ShardHead(shard=2, head=None)]},
    ):
        tampered = signed.model_copy(update={"notice": body.model_copy(update=change)})
        with pytest.raises(NoticeError):
            verify_leave(tampered)
