"""Interruption notices from EC2 IMDSv2 and the signed leave message."""

import datetime as dt
import json
import threading
from collections.abc import Iterator
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import httpx
import pytest
from pydantic import ValidationError

from hypertrain.elastic.clock import FakeClock
from hypertrain.elastic.notice import (
    MAX_HEADS,
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


@pytest.mark.parametrize("error", [httpx.ConnectError, httpx.ReadTimeout])
def test_an_unreachable_imds_raises_a_notice_error(error: type[httpx.TransportError]) -> None:
    def down(request: httpx.Request) -> httpx.Response:
        raise error("no route to 169.254.169.254", request=request)

    client = httpx.Client(base_url="http://imds.test", transport=httpx.MockTransport(down))
    with pytest.raises(NoticeError):
        AwsSpotNotice(FakeClock(), client=client, wall=lambda: WALL).poll()


class LinkLocal(BaseHTTPRequestHandler):
    """A real IMDSv2 endpoint on 127.0.0.1: a session, then no scheduled interruption."""

    def do_PUT(self) -> None:  # noqa: N802
        self.send_response(200)
        self.end_headers()
        self.wfile.write(b"session")

    def do_GET(self) -> None:  # noqa: N802
        self.send_response(404)
        self.end_headers()

    def log_message(self, *args: object) -> None:
        pass


@pytest.fixture
def imds_url() -> Iterator[str]:
    server = ThreadingHTTPServer(("127.0.0.1", 0), LinkLocal)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    yield f"http://127.0.0.1:{server.server_address[1]}"
    server.shutdown()


def test_the_imds_client_never_goes_through_a_proxy(
    imds_url: str, monkeypatch: pytest.MonkeyPatch
) -> None:
    for name in ("NO_PROXY", "no_proxy"):
        monkeypatch.delenv(name, raising=False)
    for name in ("HTTP_PROXY", "http_proxy", "ALL_PROXY"):
        monkeypatch.setenv(name, "http://127.0.0.1:9")  # nothing listens there
    assert AwsSpotNotice(FakeClock(), base_url=imds_url).poll() is None


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
        epoch="ab" * 32,
        reason="interruption",
        provider_time="2026-10-26T12:02:00Z",
        heads=[ShardHead(shard=2, head="ab" * 32)],
    )
    signed = sign_leave(body, key)
    assert verify_leave(signed) == body
    too_many = [{"shard": i, "head": None} for i in range(MAX_HEADS + 1)]
    with pytest.raises(ValidationError):
        LeaveNotice.model_validate({**body.body(), "heads": too_many})
    for change in (
        {"run_id": "run-2"},
        {"member": "m02"},
        {"lease": 5},
        {"epoch": "cd" * 32},
        {"reason": "operator"},
        {"provider_time": "2026-10-26T12:03:00Z"},
        {"heads": [ShardHead(shard=2, head=None)]},
    ):
        tampered = signed.model_copy(update={"notice": body.model_copy(update=change)})
        with pytest.raises(NoticeError):
            verify_leave(tampered)
