import json
import os
import secrets
import signal
from collections.abc import Callable
from pathlib import Path
from typing import Any

import httpx
import pytest

import hypertrain.beacon.__main__ as beacon_cli
import hypertrain.beacon.push as push
from hypertrain.beacon import BeaconRound, BeaconUnavailable, DrandQuicknet
from hypertrain.beacon.push import Pusher, plan

VECTORS = json.loads((Path(__file__).parent / "vectors.json").read_text())["rounds"]
V0 = VECTORS[0]
# Round right after V0, fetched 2026-10-18 from api.drand.sh and drand.cloudflare.com (identical).
R1 = {
    "round": 1000001,
    "randomness": "9f45f439afd81e9846b3b4dc5e3e6051922c73c8459d18e9d507b52ddbd884ff",
    "signature": (
        "a5bd91e5e2d8c0bf51bffdfad87eef34348fd9c0b2df2bee39db90bdef7e1399"
        "b1a77bb2fe98b24d84c0936a306c4218"
    ),
}
ADMIN_TOKEN = secrets.token_hex(16)
RELAY_ARGS = ["--relay", "https://a.test", "--relay", "https://b.test"]
REAL_CLIENT = httpx.Client


class Service:
    """Fake challenge service: GET /v1/beacon/latest answers from `gets`, POSTs get `post`."""

    def __init__(
        self, gets: list[int | httpx.Response | Exception] | None = None, post: int = 200
    ) -> None:
        self.gets = gets or [0]
        self.post = post
        self.seen: list[str] = []
        self.posts: list[tuple[dict[str, Any], str | None]] = []

    def handler(self, req: httpx.Request) -> httpx.Response:
        self.seen.append(f"{req.method} {req.url.path}")
        if req.method == "GET":
            assert req.url.path == "/v1/beacon/latest"
            got = self.gets.pop(0) if len(self.gets) > 1 else self.gets[0]
            if isinstance(got, Exception):
                raise got
            return (
                got if isinstance(got, httpx.Response) else httpx.Response(200, json={"round": got})
            )
        assert (req.method, req.url.path) == ("POST", "/v1/admin/beacon")
        auth = req.headers.get("authorization")
        self.posts.append((json.loads(req.content), auth))
        # Echo the bearer like a misconfigured proxy, straddling the 500-char log cut.
        return httpx.Response(self.post, text="x" * 470 + f"echo {auth}")


def relay_client(per_host: dict[str, dict[str, Any] | int | str] | None = None) -> httpx.Client:
    """Relays serving V0/R1 by path, or `per_host` bodies: dict = JSON, int = status, str = text."""
    by_path = {"latest": R1, str(V0["round"]): V0, str(R1["round"]): R1}

    def handler(req: httpx.Request) -> httpx.Response:
        if per_host is None:
            return httpx.Response(200, json=by_path[req.url.path.rsplit("/", 1)[1]])
        got = per_host[req.url.host]
        if isinstance(got, int):
            return httpx.Response(got)
        return (
            httpx.Response(200, text=got) if isinstance(got, str) else httpx.Response(200, json=got)
        )

    return httpx.Client(transport=httpx.MockTransport(handler))


@pytest.fixture
def wire(monkeypatch: pytest.MonkeyPatch) -> Callable[..., list[float]]:
    def install(svc: Service, relays: httpx.Client | None = None) -> list[float]:
        relays = relays or relay_client()
        transport = httpx.MockTransport(svc.handler)
        monkeypatch.setattr(beacon_cli, "DrandQuicknet", lambda r: DrandQuicknet(r, client=relays))
        monkeypatch.setattr(httpx, "Client", lambda **kw: REAL_CLIENT(transport=transport, **kw))
        slept: list[float] = []
        monkeypatch.setattr(push.time, "sleep", slept.append)
        return slept

    return install


@pytest.fixture
def token_file(tmp_path: Path) -> Path:
    path = tmp_path / "admin.token"
    path.write_text(f" {ADMIN_TOKEN}\r\n")
    return path


@pytest.fixture
def cli(token_file: Path, capsys: pytest.CaptureFixture[str]) -> Callable[..., Any]:
    def run(
        *extra: str, token: Path = token_file, api: str = "http://svc.test", relays: Any = None
    ) -> tuple[object, str, str]:
        argv = ["push", "--api", api, "--admin-token-file", str(token)]
        try:
            rc: object = beacon_cli.main(
                [*argv, *(RELAY_ARGS if relays is None else relays), *extra]
            )
        except SystemExit as e:
            rc = e.code
        out, err = capsys.readouterr()
        # No 8-char slice of the token anywhere: a cut-then-redact bug leaks a prefix.
        assert not any(ADMIN_TOKEN[i : i + 8] in out + err for i in range(len(ADMIN_TOKEN) - 7))
        return rc, out, err

    return run


def http(status: int) -> httpx.Response:
    return httpx.Response(status, text="busy")


def payload(v: dict[str, Any]) -> dict[str, Any]:
    return {"round": v["round"], "signature": v["signature"], "randomness": v["randomness"]}


def test_plan_pushes_after_known_bounded_by_catch_up() -> None:
    assert plan(0, 5, 20) == [1, 2, 3, 4, 5]
    assert plan(100, 105, 3) == [103, 104, 105]
    assert plan(105, 105, 3) == []


def test_tick_pushes_the_head_then_the_older_round_with_bearer(wire: Any, cli: Any) -> None:
    svc = Service(gets=[0])
    slept = wire(svc)
    rc, out, _ = cli("--catch-up", "2", "--once")
    assert (rc, slept) == (0, [])
    assert svc.posts == [
        (payload(R1), f"Bearer {ADMIN_TOKEN}"),
        (payload(V0), f"Bearer {ADMIN_TOKEN}"),
    ]
    assert [json.loads(line) for line in out.splitlines()] == [
        {"pushed": [R1["round"], V0["round"]], "latest": R1["round"], "known": 0, "pending": 0}
    ]


@pytest.mark.parametrize(
    ("status", "rc"), [(409, 1), (400, 1), (404, 1), (302, 1), (401, 2), (403, 2)]
)
def test_rejected_push_exits_at_once_without_further_posts(
    wire: Any, cli: Any, status: int, rc: int
) -> None:
    svc = Service(gets=[0], post=status)
    slept = wire(svc)
    got, out, err = cli("--catch-up", "2", "--max-failures", "3")
    assert (got, out, slept) == (rc, "", [])
    assert [p["round"] for p, _ in svc.posts] == [R1["round"]]
    assert f"round {R1['round']}: HTTP {status}: " in err and "echo Bearer ***" in err


@pytest.mark.parametrize("status", [408, 429, 503])
def test_transient_push_status_is_retried(wire: Any, cli: Any, status: int) -> None:
    svc = Service(gets=[0], post=status)
    slept = wire(svc)
    rc, out, err = cli("--catch-up", "2", "--max-failures", "2", "--interval", "0.25")
    assert (rc, out, slept) == (1, "", [0.25])
    assert [p["round"] for p, _ in svc.posts] == [R1["round"], R1["round"]]
    assert f"failure 2/2: ServiceError: round {R1['round']}: HTTP {status}" in err


def test_only_consecutive_failures_count(wire: Any, cli: Any) -> None:
    down = httpx.ConnectError("refused")
    svc = Service(gets=[http(503), down, R1["round"], http(502), down, http(500)])
    slept = wire(svc)
    rc, out, err = cli("--interval", "0.25", "--max-failures", "3")
    assert (rc, slept) == (1, [0.25] * 5)
    assert svc.seen == ["GET /v1/beacon/latest"] * 6
    assert [json.loads(line) for line in out.splitlines()] == [
        {"pushed": [], "latest": R1["round"], "known": R1["round"], "pending": 0}
    ]
    assert "failure 3/3" in err


def test_once_failed_tick_exits_1(wire: Any, cli: Any) -> None:
    slept = wire(Service(gets=[http(503)]))
    rc, out, err = cli("--once")
    assert (rc, out, slept) == (1, "", [])
    assert "failure 1/10: ServiceError: GET /v1/beacon/latest: HTTP 503: busy" in err


@pytest.mark.parametrize(
    ("relays", "error"),
    [
        ({"a.test": V0, "b.test": VECTORS[1]}, "BeaconVerificationError: relays disagree"),
        ({"a.test": V0, "b.test": 503}, "BeaconUnavailable"),
        ({"a.test": V0, "b.test": {}}, "ValueError"),
        ({"a.test": V0, "b.test": "[" * 100_000 + "]" * 100_000}, "RecursionError"),
    ],
    ids=["disagree", "down", "no-round", "nested"],
)
def test_relay_failure_pushes_nothing_and_counts_failures(
    wire: Any, cli: Any, relays: dict[str, Any], error: str
) -> None:
    svc = Service()
    slept = wire(svc, relay_client(relays))
    rc, out, err = cli("--interval", "0.25", "--max-failures", "2")
    assert (rc, out, slept, svc.seen) == (1, "", [0.25], [])
    assert f"failure 1/2: {error}" in err and f"failure 2/2: {error}" in err


@pytest.mark.parametrize(
    "content", ["  \n", "Zq7Xw\nLeakMe9\n", "bGVhay1tZQ+/=="], ids=["empty", "newline", "base64"]
)
def test_unusable_token_file_exits_2_before_any_request(
    wire: Any, cli: Any, tmp_path: Path, content: str
) -> None:
    svc = Service()
    wire(svc)
    bad = tmp_path / "bad.token"
    bad.write_text(content)
    rc, out, err = cli("--once", token=bad)
    assert (rc, out, svc.seen) == (2, "", [])
    assert "token must match [A-Za-z0-9._~-]+" in err
    assert not any(part in err for part in content.split())


@pytest.mark.parametrize(
    ("args", "kw"),
    [
        ((), {"relays": ["--relay", "https://a.test"]}),
        ((), {"relays": ["--relay", "https://a.test", "--relay", "https://a.test/"]}),
        ((), {"relays": ["--relay", "https://a.test", "--relay", "b.test"]}),
        ((), {"api": "ftp://svc.test"}),
        ((), {"api": "http://[::1"}),
        (("--interval", "0"), {}),
        (("--interval", "nan"), {}),
        (("--catch-up", "0"), {}),
    ],
    ids=["one-relay", "dup-relay", "relay-scheme", "api-scheme", "api-invalid", "interval-0"]
    + ["interval-nan", "catch-up-0"],
)
def test_bad_usage_exits_2_before_any_request(
    wire: Any, cli: Any, args: tuple[str, ...], kw: dict[str, Any]
) -> None:
    svc = Service()
    wire(svc)
    rc, out, _ = cli(*args, "--once", **kw)
    assert (rc, out, svc.seen) == (2, "", [])


def test_sigterm_exits_0_and_restores_handler(wire: Any, cli: Any) -> None:
    class Killed(Exception):
        pass

    def outer(*_: object) -> None:
        raise Killed

    class Terminating(Service):
        def handler(self, req: httpx.Request) -> httpx.Response:
            os.kill(os.getpid(), signal.SIGTERM)
            return super().handler(req)

    wire(Terminating())
    previous = signal.signal(signal.SIGTERM, outer)
    try:
        rc, _, _ = cli()
        assert rc == 0
        assert signal.getsignal(signal.SIGTERM) is outer
    finally:
        signal.signal(signal.SIGTERM, previous)


def test_failure_message_redacts_relay_credentials(wire: Any, cli: Any) -> None:
    relay = "https://u:PA'SSW@relay.example/x?apikey=SECRETKEY#SECFRAG"
    wire(Service(), relay_client({"relay.example": 503, "b.test": V0}))
    rc, out, err = cli("--once", relays=["--relay", relay, "--relay", "https://b.test"])
    assert (rc, out) == (1, "")
    assert "failure 1/" in err and "***@relay.example/x" in err
    assert "SSW" not in err and "SECRETKEY" not in err and "SECFRAG" not in err


PATH_SECRET = "sk_" + "A1b2C3d4E5f6G7h8I9j0K1l2"


def test_failure_message_redacts_relay_path_credential(wire: Any, cli: Any) -> None:
    relay = f"https://relay.example/key/{PATH_SECRET}"
    wire(Service(), relay_client({"relay.example": 503, "b.test": V0}))
    rc, out, err = cli("--once", relays=["--relay", relay, "--relay", "https://b.test"])
    assert (rc, out) == (1, "")
    assert "failure 1/" in err and "https://relay.example/key/***" in err
    assert PATH_SECRET not in err and PATH_SECRET[:8] not in err


def test_live_check_redacts_relays_and_error(wire: Any, capsys: pytest.CaptureFixture[str]) -> None:
    relay = f"https://u:PASSW@relay.example/{PATH_SECRET}?apikey=SECRETKEY"
    wire(Service(), relay_client({"relay.example": 503, "b.test": 503}))
    rc = beacon_cli.main(["check", "--live", "--relay", relay, "--relay", "https://b.test"])
    out = json.loads(capsys.readouterr().out)
    assert (rc, out["live"]) == (3, "CENSORED(network)")
    assert out["relays"] == ["https://***@relay.example/***", "https://b.test"]
    assert "relay.example/***" in out["error"] and "503" in out["error"]
    text = json.dumps(out)
    assert PATH_SECRET not in text and "PASSW" not in text and "SECRETKEY" not in text


def test_redact_urls_keeps_short_segments_and_plain_text() -> None:
    text = f"GET https://h.test/v1/{PATH_SECRET}/latest failed; see /{PATH_SECRET[:23]}"
    assert push.redact_urls(text) == (
        f"GET https://h.test/v1/***/latest failed; see /{PATH_SECRET[:23]}"
    )
    host = "https://a-relay-hostname-of-24-chars.test/x"
    assert push.redact_urls(host) == host


class Ledger:
    """Fake challenge service that keeps the rounds it accepts and reports the highest one."""

    def __init__(self, held: tuple[int, ...] = ()) -> None:
        self.held = set(held)
        self.posted: list[int] = []  # every POST received, answered or not
        self.refuse: dict[int, int | Exception] = {}  # round -> answer to its next POST only
        # round -> failure the client sees after the service has applied the round
        self.applied_then_lost: dict[int, Exception] = {}

    def handler(self, req: httpx.Request) -> httpx.Response:
        if req.method == "GET":
            return httpx.Response(200, json={"round": max(self.held, default=0)})
        number = json.loads(req.content)["round"]
        self.posted.append(number)
        refused = self.refuse.pop(number, None)
        if isinstance(refused, Exception):
            raise refused
        if refused is not None:
            return httpx.Response(refused, text="busy")
        self.held.add(number)
        lost = self.applied_then_lost.pop(number, None)
        if lost is not None:
            raise lost
        return httpx.Response(200, json={"round": number})


class Chain(DrandQuicknet):
    """Relays that know every round up to `head`; `down` lists rounds they fail to serve."""

    def __init__(self, head: int) -> None:
        super().__init__(("https://a.test", "https://b.test"))
        self.head = head
        self.down: set[int] = set()

    def _round(self, number: int) -> BeaconRound:
        return BeaconRound(number, f"sig{number}", f"rnd{number}", True)

    def latest(self) -> BeaconRound:
        return self._round(self.head)

    def get(self, round: int) -> BeaconRound:
        if round in self.down:
            raise BeaconUnavailable(f"round {round} not served")
        return self._round(round)


def pusher_for(ledger: Ledger, chain: Chain, catch_up: int) -> Pusher:
    client = httpx.Client(transport=httpx.MockTransport(ledger.handler), base_url="http://svc.test")
    return Pusher(client, chain, ADMIN_TOKEN, catch_up)


def test_head_goes_first_and_each_tick_adds_only_the_oldest_older_round() -> None:
    ledger, chain = Ledger(), Chain(20)
    pusher = pusher_for(ledger, chain, 6)
    assert pusher.tick() == {"pushed": [20, 15], "latest": 20, "known": 0, "pending": 4}
    chain.head = 21  # a new head goes before the backlog, which keeps its own order
    assert pusher.tick() == {"pushed": [21, 16], "latest": 21, "known": 20, "pending": 3}
    chain.head = 22
    assert pusher.tick()["pushed"] == [22, 17]
    assert ledger.posted == [20, 15, 21, 16, 22, 17]


def test_a_backlog_is_refilled_one_round_per_tick_and_every_round_once() -> None:
    ledger = Ledger()
    pusher = pusher_for(ledger, Chain(20), 6)
    ticks = [pusher.tick()["pushed"] for _ in range(7)]
    assert ticks == [[20, 15], [16], [17], [18], [19], [], []]
    assert sorted(ledger.posted) == list(range(15, 21))  # none skipped, none pushed twice


def test_a_round_that_was_due_is_never_dropped_while_the_window_slides() -> None:
    ledger, chain = Ledger(), Chain(20)
    pusher = pusher_for(ledger, chain, 3)
    assert pusher.tick()["pushed"] == [20, 18]  # 19 is left pending
    chain.head = 23  # the window of the last 3 rounds is now 21..23, and 19 is outside it
    assert pusher.tick()["pushed"] == [23, 19]
    assert [pusher.tick()["pushed"] for _ in range(3)] == [[21], [22], []]
    assert sorted(ledger.posted) == list(range(18, 24))  # none skipped, none pushed twice


def test_a_round_older_than_the_first_plan_is_not_pushed() -> None:
    # `--catch-up` bounds what the first plan finds: the last 3 rounds, not the ones before them
    ledger = Ledger()
    pusher = pusher_for(ledger, Chain(20), 3)
    assert [pusher.tick()["pushed"] for _ in range(3)] == [[20, 18], [19], []]
    assert min(ledger.posted) == 18


def test_once_pushes_every_round_that_is_due_because_it_has_no_next_tick(capsys: Any) -> None:
    ledger = Ledger()
    code = pusher_for(ledger, Chain(20), 6).run(0.2, True, 3)
    assert code == 0
    assert ledger.posted == [20, 15, 16, 17, 18, 19]  # the head first, then oldest first
    assert json.loads(capsys.readouterr().out)["pending"] == 0


def test_cron_runs_with_a_fresh_pusher_each_leave_no_round_missing() -> None:
    # a new process per run keeps no list: every run must push what it found in one tick
    ledger, chain = Ledger(), Chain(20)
    for head in (20, 40, 60):
        chain.head = head
        assert pusher_for(ledger, chain, 20).run(0.2, True, 3) == 0
    assert ledger.held == set(range(1, 61))


def test_a_post_the_service_applied_but_the_client_gave_up_on_is_posted_again_harmlessly() -> None:
    ledger = Ledger()
    ledger.applied_then_lost[9] = httpx.ReadTimeout("store lock held")
    pusher = pusher_for(ledger, Chain(9), 3)
    with pytest.raises(httpx.ReadTimeout):
        pusher.tick()
    assert ledger.held == {9}  # the service has the head although the pusher never saw the answer
    assert [pusher.tick()["pushed"] for _ in range(3)] == [[9, 7], [8], []]
    assert ledger.posted.count(9) == 2 and ledger.held == {7, 8, 9}


def post_503(chain: Chain, ledger: Ledger) -> None:
    ledger.refuse[7] = 503


def post_timeout(chain: Chain, ledger: Ledger) -> None:
    ledger.refuse[7] = httpx.ReadTimeout("store lock held")


def relay_down(chain: Chain, ledger: Ledger) -> None:
    chain.down.add(7)


@pytest.mark.parametrize(
    ("break_it", "error"),
    [
        (post_503, push.ServiceError),
        (post_timeout, httpx.ReadTimeout),
        (relay_down, BeaconUnavailable),
    ],
    ids=["post-503", "post-timeout", "relay-down"],
)
def test_a_failed_older_round_stays_pending_and_is_pushed_by_a_later_tick(
    break_it: Callable[[Chain, Ledger], None], error: type[Exception]
) -> None:
    ledger, chain = Ledger(), Chain(9)
    pusher = pusher_for(ledger, chain, 3)
    break_it(chain, ledger)
    with pytest.raises(error):
        pusher.tick()
    assert ledger.held == {9}  # the head went in before the older round failed
    chain.down.clear()
    # The service now reports 9: only the pending list still knows that 7 and 8 are missing.
    assert [pusher.tick()["pushed"] for _ in range(3)] == [[7], [8], []]
    assert ledger.held == {7, 8, 9}


def test_a_failed_head_is_retried_before_any_older_round() -> None:
    ledger = Ledger()
    ledger.refuse[9] = 503
    pusher = pusher_for(ledger, Chain(9), 3)
    with pytest.raises(push.ServiceError, match="round 9: HTTP 503"):
        pusher.tick()
    assert ledger.posted == [9]  # nothing older goes in while the head is refused
    assert pusher.tick()["pushed"] == [9, 7]
