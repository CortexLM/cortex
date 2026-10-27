"""Container half of the R2 artifact transfer (docs/adr/0013-executor-artifacts-via-r2.md).

`serve` runs in this process with an injected probe and launch. Its one HTTP seam is wired to a
fake R2 (in-process) or to a loopback server (the wire tests). The challenge side of the mailbox
is written by hand in artifact_fakes.Challenge, so these tests check the runner against the
contract and not against the broker that implements the other half."""

from __future__ import annotations

import ast
import io
import json
import random
import sys
import tarfile
import time
import tracemalloc
from collections.abc import Callable, Iterator
from pathlib import Path
from typing import Any

import pytest

from hypertrain.gpu_ops import cuda_executor as ce
from hypertrain.gpu_ops import cuda_executor_runner as runner
from hypertrain.storage.cas import PartPlan, scan
from hypertrain.storage.offer import finish_upload

from .artifact_fakes import (
    FILES,
    U0,
    Challenge,
    Clock,
    MiB,
    TimedFakeR2,
    bridge,
    client_for,
    isolated_process,
    launch_tree,
    moved_clock,
    oracle,
    package,
    probe_ok,
    request_bytes,
    serve_http,
    sha,
    write_tree,
)

DIGEST = "sha256:" + "ab" * 32
CANARY = "CANARYSIGNATURE" + "0" * 20  # a value no honest code path may print


@pytest.fixture(autouse=True)
def process_state(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Iterator[None]:
    yield from isolated_process(tmp_path, monkeypatch)


@pytest.fixture
def fast_clock(monkeypatch: pytest.MonkeyPatch) -> Clock:
    return moved_clock(monkeypatch)


class Run:
    def __init__(self, code: int, out: bytes, err: str, raw: bytes, challenge: Challenge) -> None:
        self.code, self.out, self.err, self.raw, self.challenge = code, out, err, raw, challenge


def drive(
    fake: TimedFakeR2,
    monkeypatch: pytest.MonkeyPatch,
    *,
    block: dict[str, Any] | None = None,
    answer: dict[str, Any] | None = None,
    ttl: int = 600,
    launch: Callable[..., int] = launch_tree,
    seam: Callable[..., Any] | None = None,
    answering: bool = True,
) -> Run:
    client = client_for(fake)
    challenge = Challenge(fake, client, ttl=ttl)
    monkeypatch.setattr(runner, "http_send", seam or bridge(fake))
    raw = request_bytes(FILES, challenge.request_block(**(block or {})))
    challenge.raw = raw
    thread = challenge.answer(**(answer or {})) if answering else None
    out, err = io.BytesIO(), io.StringIO()
    code = runner.serve(io.BytesIO(package(FILES, raw)), out, err, probe=probe_ok, launch=launch)
    if thread is not None:
        thread.join(30)
    return Run(code, out.getvalue(), err.getvalue(), raw, challenge)


# -- the tar as an addressable layout ------------------------------------------------------------


def test_layout_serves_exactly_the_bytes_write_outputs_streams(tmp_path: Path) -> None:
    attempt = tmp_path / "attempt"
    write_tree(attempt, size=3 * MiB + 77, ranks=2)
    environment, request = {"backend": "cuda", "gpu_uuids": [U0]}, b'{"a": 1}'
    streamed = io.BytesIO()
    runner.write_outputs(streamed, attempt, environment, request)

    layout, listing = runner.build_artifact(attempt, environment, request, workers=2)

    expected = streamed.getvalue()
    assert layout.size == len(expected)
    assert layout.read_at(0, layout.size) == expected
    with tarfile.open(fileobj=io.BytesIO(expected)) as archive:
        done = json.loads(archive.extractfile("COMPLETE").read())  # type: ignore[union-attr]
    assert done["files"] == {
        **listing,
        "environment.json": sha(json.dumps(environment, sort_keys=True).encode()),
    }
    # windows over the seams: header/data, data/padding, padding/header, trailer, record pad
    rng = random.Random(7)
    seams = [512, 1024, 1536, 10240, len(expected) - 10240, len(expected) - 1024, len(expected) - 1]
    for seam in seams + [rng.randrange(len(expected)) for _ in range(40)]:
        for length in (1, 511, 512, 513, 4096):
            start = max(0, min(seam - length // 2, len(expected) - 1))
            length = min(length, len(expected) - start)
            assert layout.read_at(start, length) == expected[start : start + length]
    layout.close()


def test_layout_header_of_a_member_over_8_gib_is_a_pax_header(tmp_path: Path) -> None:
    big = tmp_path / "big.bin"
    with big.open("wb") as handle:
        handle.truncate((8 << 30) + 1)  # sparse: the data is never read
    layout = runner.Layout([("rank-0/big.bin", big)])
    header = tarfile.TarInfo("rank-0/big.bin")
    header.size, header.mode = (8 << 30) + 1, 0o600
    pax = header.tobuf(tarfile.PAX_FORMAT, tarfile.ENCODING, "surrogateescape")
    assert len(pax) == 1536  # an extended 'size' record: the plain header cannot hold 8 GiB + 1
    assert layout.read_at(0, len(pax)) == pax
    padded = ((8 << 30) + 1 + 511) // 512 * 512
    assert layout.size == (len(pax) + padded + 1024 + 10240 - (len(pax) + padded + 1024) % 10240)
    layout.close()


def test_hash_layout_is_what_the_storage_scan_computes(tmp_path: Path) -> None:
    attempt = tmp_path / "attempt"
    write_tree(attempt, size=9 * MiB + 3, ranks=1)
    layout, _ = runner.build_artifact(attempt, {"backend": "cuda"}, b"{}", workers=3)

    whole, blocks, parts = runner.hash_layout(layout, 8 * MiB, workers=3)

    expected = scan(layout.size, layout.read_at, PartPlan(layout.size, 8 * MiB))
    assert whole == expected.sha256
    assert blocks == list(expected.blocks.blocks)
    assert parts == list(expected.part_sha256)
    layout.close()


# -- what ships in the container -------------------------------------------------------------


def test_the_shipped_runner_is_stdlib_only_and_fits_one_argv() -> None:
    tree = ast.parse(ce.RUNNER_SOURCE)
    modules = {
        alias.name.split(".")[0]
        for node in ast.walk(tree)
        if isinstance(node, ast.Import)
        for alias in node.names
    } | {
        node.module.split(".")[0]
        for node in ast.walk(tree)
        if isinstance(node, ast.ImportFrom) and node.module and node.level == 0
    }
    assert modules <= sys.stdlib_module_names, sorted(modules - sys.stdlib_module_names)
    command = ce.run_command(["docker"], "repo@" + DIGEST, "ht-exec-0", "0" * 64, [U0], "default")
    assert len(command.encode()) <= 96 * 1024  # one argv element must stay under 128 KiB (E2BIG)


def test_the_runner_source_runs_where_it_is_not_a_module() -> None:
    """The fake host (and `python -c`) executes the source in a bare namespace: a decorator that
    looks the module up in sys.modules (dataclass under `from __future__ import annotations`)
    fails there, so the transfer code uses plain classes."""
    namespace: dict[str, Any] = {"__name__": "cuda_executor_runner_under_test"}
    exec(compile(ce.RUNNER_SOURCE, "<runner>", "exec"), namespace)  # noqa: S102
    assert callable(namespace["build_artifact"]) and callable(namespace["send_artifact"])


# -- serve: the transfer -------------------------------------------------------------------------


def test_serve_uploads_the_classic_tar_and_prints_one_status_line(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    fake = TimedFakeR2()
    run = drive(fake, monkeypatch)

    assert run.code == 0, run.err
    status = json.loads(run.out)
    assert run.out.count(b"\n") == 1 and len(run.out) <= 1024
    expected = oracle(run.raw)
    assert status["sha256"] == sha(expected) and status["size"] == len(expected)
    declared = json.loads(run.challenge.declared or b"")
    assert declared["request_sha256"] == sha(run.raw)  # bound to this very request
    assert declared["size"] == len(expected) and declared["part_bytes"] == 8 * MiB
    store, offer = run.challenge.upload
    assert finish_upload(store, offer) == sha(expected)
    assert store.get(sha(expected)) == expected  # the object is the tar write_outputs streams
    plan = PartPlan(len(expected), 8 * MiB)
    assert len(fake.served("UploadPart")) == plan.count
    assert len(fake.served("CreateMultipartUpload")) == 1
    assert all(row[2] == 200 for row in fake.served("UploadPart"))


def test_serve_gives_the_island_the_budget_minus_the_transfer_reserve(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    seen: list[float] = []

    def launch(request: dict[str, Any], attempt: Path, budget: float, log: Path) -> int:
        seen.append(budget)
        return launch_tree(request, attempt, budget, log)

    run = drive(TimedFakeR2(), monkeypatch, block={"reserve_seconds": 200}, launch=launch)
    assert run.code == 0, run.err
    assert 600 - 200 - 30 < seen[0] <= 600 - 200  # request budget_seconds is 600


@pytest.mark.parametrize(
    ("faults", "said"),
    [((503, 503, 503), "HTTP 503"), ((403,), "HTTP 403")],
    ids=["store-down", "url-refused"],
)
def test_serve_checks_the_store_before_it_spends_the_island_budget(
    monkeypatch: pytest.MonkeyPatch, fast_clock: Clock, faults: tuple[int, ...], said: str
) -> None:
    fake = TimedFakeR2()
    fake.fault_key("GetObject", "offer.json", *faults)
    launched: list[int] = []

    def never(*args: Any) -> int:
        launched.append(1)
        return 0

    run = drive(fake, monkeypatch, launch=never, answering=False)
    assert (run.code, launched) == (11, [])
    assert f"artifact store unreachable: {said}" in run.err and CANARY not in run.err


@pytest.mark.parametrize(
    ("edit", "reason"),
    [
        pytest.param(lambda o: o.update(refused="deadline"), "refused", id="refused"),
        pytest.param(
            lambda o: o.update(declare_sha256="0" * 64), "does not answer", id="other-declaration"
        ),
        pytest.param(lambda o: o["parts"].pop(), "does not answer", id="missing-part"),
        pytest.param(lambda o: o.update(ttl_s=0), "does not answer", id="ttl-zero"),
        pytest.param(
            lambda o: o["parts"].__setitem__(0, "https://elsewhere.test/x?a=b"),
            "does not answer",
            id="part-on-another-origin",
        ),
        pytest.param(
            lambda o: o.update(list=["https://elsewhere.test/x"]),
            "does not answer",
            id="list-on-another-origin",
        ),
        pytest.param(lambda o: o.update(extra=1), "does not answer", id="unknown-field"),
    ],
)
def test_serve_sends_nothing_for_an_offer_that_is_not_for_its_declaration(
    monkeypatch: pytest.MonkeyPatch, edit: Callable[[dict[str, Any]], None], reason: str
) -> None:
    fake = TimedFakeR2()
    sent_elsewhere: list[str] = []
    inner = bridge(fake)

    def seam(method: str, url: str, *rest: Any) -> Any:
        if "elsewhere.test" in url:
            sent_elsewhere.append(url)
        return inner(method, url, *rest)

    run = drive(fake, monkeypatch, answer={"edit": edit}, seam=seam)
    assert run.code == 10 and reason in run.err
    assert sent_elsewhere == [] and fake.served("UploadPart") == []


def test_serve_treats_an_expired_url_as_final_and_throttling_as_retryable(
    monkeypatch: pytest.MonkeyPatch, fast_clock: Clock
) -> None:
    fake = TimedFakeR2()
    fake.fault_key("UploadPart", "", 503, None, 503)  # throttled twice, then served
    run = drive(fake, monkeypatch)
    assert run.code == 0, run.err
    puts = fake.served("UploadPart")
    assert (
        sorted(row[2] for row in puts).count(503) == 2
        and len(puts) == run.challenge.upload[1].plan.count + 2
    )

    fresh = TimedFakeR2(Clock(time.time()))
    expired = drive(
        fresh,
        monkeypatch,
        answer={"after_open": lambda upload_id: fresh.clock.advance(7200)},  # type: ignore[union-attr]
    )
    assert expired.code == 10 and "HTTP 403" in expired.err
    assert len(fresh.served("expired")) == 1  # the first expired part ended it: no backoff loop


def test_serve_sends_only_the_parts_r2_does_not_hold_yet(monkeypatch: pytest.MonkeyPatch) -> None:
    """A part of this very upload that an earlier try of the container delivered is not resent."""
    fake = TimedFakeR2()

    def deliver_part_one(challenge: Challenge) -> None:
        store, offer = challenge.upload
        part = offer.parts[0]
        first = oracle(challenge.raw)[: offer.part_bytes]
        sent = store.client.http.put(
            part.url, content=first, headers={"x-amz-content-sha256": part.sha256}
        )
        assert sent.status_code == 200

    run = drive(fake, monkeypatch, answer={"after_open": deliver_part_one})

    assert run.code == 0, run.err
    store, offer = run.challenge.upload
    assert len(fake.served("UploadPart")) == offer.plan.count  # the seed plus the others, no resend
    assert finish_upload(store, offer) == sha(oracle(run.raw))


def test_serve_holds_a_few_parts_in_memory_and_not_the_artifact(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """128 MiB of rank files, 8 MiB parts, two workers: the peak is a few parts, not the file."""
    sent: list[int] = []
    declared: list[bytes] = []

    def seam(
        method: str, url: str, headers: Any, body: bytes | None, timeout: float, limit: int
    ) -> tuple[int, dict[str, str], bytes]:
        if method == "PUT" and url.endswith("declare.json"):
            declared.append(body or b"")
            return 200, {}, b""
        if method == "PUT":  # a part: the signed hash must be the hash of the body, then drop it
            assert body is not None and headers["x-amz-content-sha256"] == sha(body)
            sent.append(len(body))
            return 200, {}, b""
        if "offer.json" in url:
            if not declared:
                return 404, {}, b""
            count = len(json.loads(declared[0])["parts"])
            offer = {
                "version": 1,
                "declare_sha256": sha(declared[0]),
                "ttl_s": 600,
                "parts": [f"https://r2.test/part/{n}" for n in range(1, count + 1)],
                "list": ["https://r2.test/list/0"],
            }
            return 200, {}, json.dumps(offer).encode()
        return 200, {}, b"<ListPartsResult></ListPartsResult>"

    def big(request: dict[str, Any], attempt: Path, budget: float, log: Path) -> int:
        (attempt / "rank-0").mkdir()
        with (attempt / "rank-0" / "state.safetensors").open("wb") as handle:
            for index in range(16):
                handle.write(bytes([index]) * (8 * MiB))
        return 0

    fake = TimedFakeR2()
    tracemalloc.start()
    try:
        run = drive(
            fake,
            monkeypatch,
            block={"workers": 2, "declare_put": "https://r2.test/declare.json"},
            launch=big,
            seam=seam,
            answering=False,
        )
        peak = tracemalloc.get_traced_memory()[1]
    finally:
        tracemalloc.stop()
    assert run.code == 0, run.err
    assert len(sent) == 17 and sum(sent) > 128 * MiB  # 128 MiB plus headers, in 8 MiB parts
    assert peak < 64 * MiB, peak


def test_serve_gives_up_on_an_offer_that_never_comes(
    monkeypatch: pytest.MonkeyPatch, fast_clock: Clock
) -> None:
    fake = TimedFakeR2()
    run = drive(fake, monkeypatch, answering=False, block={"offer_wait_s": 5})
    assert run.code == 10 and "offer" in run.err
    assert fast_clock.time() - 1000.0 < 60  # bounded by offer_wait_s, not by the job budget


def test_serve_refuses_a_request_with_a_url_it_would_not_print_and_prints_none(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    bad = f"https://r2.test/bkt/x y?X-Amz-Signature={CANARY}"  # whitespace: InvalidURL echoes it
    run = drive(TimedFakeR2(), monkeypatch, block={"declare_put": bad}, answering=False)
    assert run.code == 2 and CANARY not in run.err


@pytest.mark.parametrize(
    ("declare", "offer"),
    [
        ("http://r2.test/bkt/declare.json?a=b", "http://r2.test/bkt/offer.json?a=b"),
        ("http://10.0.0.5/bkt/declare.json?a=b", "http://10.0.0.5/bkt/offer.json?a=b"),
        ("ftp://r2.test/bkt/declare.json", "ftp://r2.test/bkt/offer.json"),
        ("https://user:pass@r2.test/bkt/declare.json?a=b", "https://user:pass@r2.test/bkt/o?a=b"),
        ("https://r2.test/bkt/declare.json?a=b#fragment", "https://r2.test/bkt/offer.json?a=b"),
        ("/bkt/declare.json?a=b", "/bkt/offer.json?a=b"),
        ("https://r2.test/bkt/declare.json?a=b", "https://elsewhere.test/bkt/offer.json?a=b"),
        ("https://r2.test/bkt/declare.json?a=b", "https://r2.test:8443/bkt/offer.json?a=b"),
        ("https://r2.test/bkt/declare.json?a=b", 7),
    ],
    ids=[
        "plain-http",
        "plain-http-private-address",
        "other-scheme",
        "user-info",
        "fragment",
        "no-origin",
        "offer-on-another-host",
        "offer-on-another-port",
        "offer-not-a-string",
    ],
)
def test_serve_refuses_urls_that_are_not_https_on_one_origin(
    monkeypatch: pytest.MonkeyPatch, declare: str, offer: Any
) -> None:
    """Each flaw is in a URL of its own; the other one is on the same origin, so that the origin
    pin cannot be what refuses it."""
    launched: list[int] = []

    def never(*args: Any) -> int:
        launched.append(1)
        return 0

    block = {"declare_put": declare, "offer_get": offer}
    run = drive(TimedFakeR2(), monkeypatch, block=block, answering=False, launch=never)
    assert (run.code, launched) == (2, []) and "not acceptable" in run.err


@pytest.mark.parametrize(
    "change",
    [
        {"workers": 0},
        {"workers": 33},
        {"workers": True},
        {"part_bytes": 1 * MiB},
        {"part_bytes": 6 * MiB},
        {"part_bytes": 128 * MiB},
        {"reserve_seconds": -1},
        {"retry_attempts": 0},
        {"retry_attempts": 11},
        {"timeout_s": 0},
        {"poll_ms": 1},
        {"offer_wait_s": 0},
        {"version": 2},
        {"surprise": 1},
        {"declare_put": None},
    ],
    ids=str,
)
def test_serve_refuses_a_request_block_outside_what_it_knows(
    monkeypatch: pytest.MonkeyPatch, change: dict[str, Any]
) -> None:
    run = drive(TimedFakeR2(), monkeypatch, block=change, answering=False)
    assert run.code == 2 and "request artifact" in run.err


def test_serve_refuses_a_request_block_with_a_field_missing(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    fake = TimedFakeR2()
    challenge = Challenge(fake, client_for(fake))
    block = challenge.request_block()
    del block["timeout_s"]
    raw = request_bytes(FILES, block)
    out, err = io.BytesIO(), io.StringIO()
    code = runner.serve(
        io.BytesIO(package(FILES, raw)), out, err, probe=probe_ok, launch=launch_tree
    )
    assert code == 2 and "other fields" in err.getvalue()


@pytest.mark.parametrize(("stage", "code"), [("preflight", 11), ("transfer", 10)])
def test_serve_prints_no_url_when_the_http_layer_raises_one_in_its_message(
    monkeypatch: pytest.MonkeyPatch, stage: str, code: int
) -> None:
    fake = TimedFakeR2()
    inner = bridge(fake)

    def seam(method: str, url: str, *rest: Any) -> Any:
        if stage == "preflight" or method == "PUT":
            raise ValueError(f"cannot reach {url}")  # what a stdlib error puts in its text
        return inner(method, url, *rest)

    run = drive(fake, monkeypatch, seam=seam, answering=False)
    assert run.code == code and "ValueError" in run.err
    assert "X-Amz-" not in run.err and "r2.test" not in run.err


def test_the_preflight_does_not_wait_out_a_long_request_timeout(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    timeouts: list[float] = []

    def seam(method: str, url: str, headers: Any, body: Any, timeout: float, limit: int) -> Any:
        timeouts.append(timeout)
        return 404, {}, b""

    drive(TimedFakeR2(), monkeypatch, block={"timeout_s": 600}, seam=seam, answering=False)
    assert timeouts[0] == runner.PREFLIGHT_TIMEOUT  # a host without a route must fail in seconds


def test_serve_does_not_use_urls_after_the_lifetime_the_offer_announced(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    fake = TimedFakeR2()
    run = drive(fake, monkeypatch, answer={"edit": lambda offer: offer.update(ttl_s=1)})
    assert run.code == 10 and "url expired" in run.err
    assert fake.served("UploadPart") == []


def test_serve_sends_nothing_of_a_file_that_changed_after_it_was_hashed(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    fake = TimedFakeR2()
    inner = bridge(fake)
    attempts: list[Path] = []

    def launch(request: dict[str, Any], attempt: Path, budget: float, log: Path) -> int:
        attempts.append(attempt)
        return launch_tree(request, attempt, budget, log)

    def seam(method: str, url: str, headers: Any, *rest: Any) -> Any:
        reply = inner(method, url, headers, *rest)
        if method == "PUT" and "declare.json" in url:  # the declaration is out: now alter the file
            path = attempts[0] / "rank-0" / "state.safetensors"
            data = bytearray(path.read_bytes())
            data[100] ^= 1
            path.write_bytes(bytes(data))
        return reply

    run = drive(fake, monkeypatch, launch=launch, seam=seam)
    assert run.code == 10 and "changed while it was being sent" in run.err
    _, offer = run.challenge.upload
    assert 1 not in fake.uploads[offer.upload_id][2]  # the part with the altered byte never left


def test_throttling_is_waited_out_as_the_store_asks(
    monkeypatch: pytest.MonkeyPatch, fast_clock: Clock
) -> None:
    replies = iter(
        [
            (429, {"retry-after": "3"}, b""),  # the store names a delay
            (429, {"retry-after": ""}, b""),  # none named: a throttle waits at least a second
            (503, {"retry-after": "9999"}, b""),  # an absurd delay is capped
            (503, {"retry-after": ""}, b""),  # a plain server error backs off briefly
            (200, {}, b"ok"),
        ]
    )
    monkeypatch.setattr(runner, "http_send", lambda *args: next(replies))
    waits: list[float] = []
    monkeypatch.setattr(runner, "_sleep", waits.append)
    block = Challenge(TimedFakeR2(), client_for(TimedFakeR2())).request_block(
        retry_attempts=5, retry_base_ms=1
    )
    spec = runner.Transfer(block, deadline=fast_clock.time() + 600)

    assert runner._call(spec, "PUT", "https://r2.test/x") == (200, b"ok")

    assert 3 <= waits[0] <= 16 and 1 <= waits[1] < 3
    assert waits[2] == runner.RETRY_CAP and waits[3] < 1


def test_a_transfer_past_its_deadline_sends_nothing(
    monkeypatch: pytest.MonkeyPatch, fast_clock: Clock
) -> None:
    sent: list[str] = []
    monkeypatch.setattr(runner, "http_send", lambda *args: sent.append("x") or (200, {}, b""))
    block = Challenge(TimedFakeR2(), client_for(TimedFakeR2())).request_block()
    spec = runner.Transfer(block, deadline=fast_clock.time())  # already reached
    with pytest.raises(runner.TransferError, match="deadline reached"):
        runner._call(spec, "GET", "https://r2.test/x")
    assert sent == []


def test_serve_accepts_a_declaration_whose_first_reply_was_lost(
    monkeypatch: pytest.MonkeyPatch, fast_clock: Clock
) -> None:
    fake = TimedFakeR2()
    fake.fault("PutObject", 503, after=True)  # the declaration is stored, its reply is lost
    run = drive(fake, monkeypatch)
    assert run.code == 0, run.err
    declared = [row[2] for row in fake.served("PutObject", "declare.json")]
    assert declared == [503, 412]  # the retry meets the stored object and takes it as its own


# -- the real client on a real socket ------------------------------------------------------------


def test_the_stdlib_client_completes_a_transfer_over_loopback_http(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    fake = TimedFakeR2()
    front = serve_http(fake)
    try:
        client = client_for(fake, front.base)
        challenge = Challenge(fake, client, part_bytes=8 * MiB)
        raw = request_bytes(FILES, challenge.request_block(workers=4))
        thread = challenge.answer()
        out, err = io.BytesIO(), io.StringIO()
        code = runner.serve(
            io.BytesIO(package(FILES, raw)), out, err, probe=probe_ok, launch=launch_tree
        )
        thread.join(30)
    finally:
        front.close()
    assert code == 0, err.getvalue()
    store, offer = challenge.upload
    assert finish_upload(store, offer) == sha(oracle(raw))
    assert store.get(offer.sha256) == oracle(raw)


def test_the_stdlib_client_reads_an_early_refusal_instead_of_a_broken_pipe() -> None:
    """R2 answers 403 to an expired part before it has read the body and closes the socket; the
    client must report that status, twice in a row (the second call has to reconnect)."""
    front = serve_http(TimedFakeR2(), early={"PUT": 403})
    try:
        for _ in range(2):
            status, _headers, _body = runner.http_send(
                "PUT", f"{front.base}/x", {"x-test": "1"}, b"x" * (6 * MiB), 10, 1024
            )
            assert status == 403
    finally:
        front.close()


def test_the_stdlib_client_refuses_a_reply_bigger_than_the_limit_it_was_given() -> None:
    fake = TimedFakeR2()
    front = serve_http(fake)
    try:
        client = client_for(fake, front.base)
        fake.objects["big"] = (b"x" * (2 * MiB), {})
        url = client.presign("GET", "big")
        status, _, body = runner.http_send("GET", url, {}, None, 10, 3 * MiB)
        assert (status, len(body)) == (200, 2 * MiB)
        with pytest.raises(runner.TransferError, match="response too large"):
            runner.http_send("GET", url, {}, None, 10, 1 * MiB)
    finally:
        front.close()
