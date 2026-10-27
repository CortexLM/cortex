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


def test_serve_checks_the_store_before_it_spends_the_island_budget(
    monkeypatch: pytest.MonkeyPatch, fast_clock: Clock
) -> None:
    fake = TimedFakeR2()
    fake.fault_key("GetObject", "offer.json", 503, 503, 503)
    launched: list[int] = []

    def never(*args: Any) -> int:
        launched.append(1)
        return 0

    run = drive(fake, monkeypatch, launch=never, answering=False)
    assert (run.code, launched) == (11, [])
    assert "artifact store unreachable" in run.err and CANARY not in run.err


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


def test_serve_prints_no_url_when_the_http_layer_raises_one_in_its_message(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    def seam(method: str, url: str, *rest: Any) -> Any:
        raise ValueError(f"cannot reach {url}")  # what a stdlib error would put in its text

    for answering in (False,):
        run = drive(TimedFakeR2(), monkeypatch, seam=seam, answering=answering)
        assert run.code == 11 and "X-Amz-" not in run.err and "r2.test" not in run.err


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
