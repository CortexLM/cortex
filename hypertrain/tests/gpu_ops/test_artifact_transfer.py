"""Challenge half of the R2 artifact transfer (docs/adr/0013-executor-artifacts-via-r2.md).

A `Broker` and its `Session` run against a fake R2. The container is either the real runner
(`serve`, its HTTP seam bridged to the same fake) or a few lines written by hand to lie, stop short
or abuse the mailbox. Nothing here starts ssh or docker: tests/gpu_ops/test_artifact_dispatch.py
runs the whole executor."""

from __future__ import annotations

import io
import json
import os
import random
import re
import time
from collections.abc import Callable, Iterator
from pathlib import Path
from typing import Any
from urllib.parse import parse_qs, urlsplit

import httpx
import pytest

from hypertrain.gpu_ops import artifact_transfer as at
from hypertrain.gpu_ops import cuda_executor_runner as runner
from hypertrain.gpu_ops.cuda_executor import Aborted, AttemptFailed, HostRefused
from hypertrain.gpu_ops.journal import Journal
from hypertrain.protocol.jcs import canonicalize
from hypertrain.storage.cas import BLOCK_BYTES, BlockList, PartPlan, scan

from .artifact_fakes import (
    BUCKET,
    FILES,
    MiB,
    TimedFakeR2,
    bridge,
    client_for,
    credentials_file,
    isolated_process,
    launch_tree,
    oracle,
    package,
    probe_ok,
    request_bytes,
    sha,
)
from .fake_cuda_host import wait_for

CANARY = "CANARY" + "7" * 26


@pytest.fixture(autouse=True)
def process_state(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Iterator[None]:
    yield from isolated_process(tmp_path, monkeypatch)


class Job:
    """What the broker reads of an island job: the run, the digest and the deadline."""

    run_id = "a" * 64

    def __init__(self, seconds: float = 600.0) -> None:
        self.deadline = time.time() + seconds

    def digest(self) -> str:
        return "b" * 64


def make_broker(
    tmp_path: Path, fake: TimedFakeR2, **over: Any
) -> tuple[at.Broker, at.ArtifactSettings]:
    credentials = credentials_file(tmp_path, "https://r2.test")
    fields: dict[str, Any] = {
        "credentials_file": credentials,
        "part_bytes": 8 * MiB,
        "workers": 3,
        "poll_ms": 10,
        "offer_wait_seconds": 20,
        "request_timeout_seconds": 20,
        "retry_attempts": 3,
        "retry_base_ms": 1,
        "max_artifact_bytes": 1 << 30,
    }
    settings = at.ArtifactSettings(**{**fields, **over})
    state = tmp_path / "state"
    state.mkdir(exist_ok=True)
    broker = at.Broker(
        settings,
        at.load_credentials(credentials),
        Journal(state),
        transport=httpx.MockTransport(fake),
        sleep=lambda seconds: None,
    )
    return broker, settings


def kinds(broker: at.Broker) -> list[str]:
    if not broker.journal.path.exists():
        return []
    return [json.loads(line)["kind"] for line in broker.journal.path.read_text().splitlines()]


def open_session(
    broker: at.Broker, job: Job | None = None, request: bytes = b"request"
) -> at.Session:
    session = broker.begin(job or Job(), "audit")  # type: ignore[arg-type]
    assert session is not None
    session.bind(request)
    return session


def mailbox(session: at.Session, fake: TimedFakeR2) -> tuple[httpx.Client, bytes | None]:
    """A hand-written container's view: its PUT URL, and what the challenge answered."""
    held = fake.objects.get(session.prefix + "offer.json")
    return client_for(fake).http, None if held is None else held[0]


def declare(session: at.Session, fake: TimedFakeR2, body: bytes) -> bytes:
    """Put a declaration as the container would (the presigned URL, the signed header) and wait
    for the challenge's thread to answer it; returns offer.json."""
    http = client_for(fake).http
    sent = http.put(session.block["declare_put"], content=body, headers={"if-none-match": "*"})
    assert sent.status_code == 200
    session.start()
    assert session.thread is not None
    session.thread.join(10)
    assert not session.thread.is_alive()
    return mailbox(session, fake)[1] or b""


def synthetic(
    request_sha256: str, size: int, part_bytes: int = 8 * MiB, **over: Any
) -> dict[str, Any]:
    """A declaration with made-up hashes of the right shape: the broker checks shape, not bytes."""
    declared: dict[str, Any] = {
        "version": 1,
        "request_sha256": request_sha256,
        "size": size,
        "sha256": sha(b"whole"),
        "part_bytes": part_bytes,
        "blocks": [sha(b"block %d" % i) for i in range(-(-size // BLOCK_BYTES))],
        "parts": [sha(b"part %d" % i) for i in range(-(-size // part_bytes))],
    }
    declared.update(over)
    return declared


def encode(declared: dict[str, Any]) -> bytes:
    return json.dumps(declared).encode()


class Pair:
    def __init__(self, session: at.Session, code: int, out: bytes, err: str, raw: bytes) -> None:
        self.session, self.code, self.out, self.err, self.raw = session, code, out, err, raw


def run_pair(
    broker: at.Broker, fake: TimedFakeR2, monkeypatch: pytest.MonkeyPatch, job: Job | None = None
) -> Pair:
    """The real runner against this broker, the runner in the main thread as in the container."""
    session = broker.begin(job or Job(), "audit")  # type: ignore[arg-type]
    assert session is not None
    raw = request_bytes(FILES, session.block)
    session.bind(raw)
    monkeypatch.setattr(runner, "http_send", bridge(fake))
    session.start()
    out, err = io.BytesIO(), io.StringIO()
    code = runner.serve(
        io.BytesIO(package(FILES, raw)), out, err, probe=probe_ok, launch=launch_tree
    )
    session.stop_thread()
    return Pair(session, code, out.getvalue(), err.getvalue(), raw)


def leftovers(fake: TimedFakeR2, session: at.Session) -> list[str]:
    return sorted(k for k in fake.objects if k.startswith(session.prefix))


# -- the round trip ------------------------------------------------------------------------------


def test_the_runner_and_the_broker_hand_over_the_tar_the_classic_path_streams(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    fake = TimedFakeR2()
    sentinel = ("runs/other/exec/" + "c" * 64 + "/x/r/" + "d" * 64, "runs/other/keep")
    for key in sentinel:
        fake.objects[key] = (b"someone else's", {})
    broker, settings = make_broker(tmp_path, fake)

    pair = run_pair(broker, fake, monkeypatch)
    session = pair.session
    assert pair.code == 0, pair.err

    status = tmp_path / "status.out"
    status.write_bytes(pair.out)
    session.collect(status, lambda: None, lost=False)

    expected = oracle(pair.raw)
    assert status.read_bytes() == expected  # the verified object replaced the status line
    assert sorted(os.listdir(tmp_path)) == sorted(["status.out", "r2-exec.env", "state", "tmp"])
    record = session.record()
    assert record["sha256"] == sha(expected) and record["size"] == len(expected)
    assert record["parts"] == PartPlan(len(expected), 8 * MiB).count
    assert all(type(v) in (int, str) for v in record.values())  # no float reaches the provenance
    canonicalize(record)

    session.close()
    assert fake.uploads == {}
    # the object and its block list are gone; the two mailbox objects wait for the lifecycle rule
    assert leftovers(fake, session) == [
        session.prefix + "declare.json",
        session.prefix + "offer.json",
    ]
    assert all(key in fake.objects for key in sentinel)
    assert kinds(broker) == [
        "exec_artifact_begin",
        "exec_artifact_offered",
        "exec_artifact_done",
        "exec_artifact_end",
    ]
    del settings


def test_each_attempt_gets_keys_nobody_else_has_and_the_journal_knows_them_first(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    fake = TimedFakeR2()
    broker, _ = make_broker(tmp_path, fake)
    seen: list[list[str]] = []
    original = broker.client.presign

    def spy(*args: Any, **kwargs: Any) -> str:
        seen.append(kinds(broker))  # what the journal held when the first URL was made
        return original(*args, **kwargs)

    monkeypatch.setattr(broker.client, "presign", spy)
    first, second = open_session(broker), open_session(broker)
    assert seen[0] == ["exec_artifact_begin"]
    assert first.prefix != second.prefix
    pattern = r"runs/a{64}/exec/b{64}/[0-9a-f]{32}/"
    assert re.fullmatch(pattern, first.prefix) and re.fullmatch(pattern, second.prefix)


# -- the declaration is the host's word, read with care -------------------------------------


BAD = [
    pytest.param(lambda d: d.update(size=0, blocks=[], parts=[]), "malformed", id="size-zero"),
    pytest.param(lambda d: d.update(size=1 << 40), "too_large", id="size-over-the-cap"),
    pytest.param(lambda d: d.update(size=1.5e9), "malformed", id="size-float"),
    pytest.param(lambda d: d.update(size=True), "malformed", id="size-boolean"),
    pytest.param(lambda d: d.update(version=True), "malformed", id="version-boolean"),
    pytest.param(lambda d: d.update(version=2), "malformed", id="version-2"),
    pytest.param(
        lambda d: d.update(part_bytes=16 * MiB), "malformed", id="part-size-chosen-by-host"
    ),
    pytest.param(lambda d: d["blocks"].pop(), "malformed", id="one-block-short"),
    pytest.param(lambda d: d["parts"].append(sha(b"x")), "malformed", id="one-part-too-many"),
    pytest.param(
        lambda d: d["blocks"].__setitem__(0, d["blocks"][0].upper()),
        "malformed",
        id="upper-case-hex",
    ),
    pytest.param(lambda d: d["parts"].__setitem__(0, "z" * 64), "malformed", id="not-hex"),
    pytest.param(lambda d: d.update(sha256=None), "malformed", id="no-whole-hash"),
    pytest.param(
        lambda d: d.update(request_sha256="0" * 64), "wrong_request", id="another-request"
    ),
    pytest.param(lambda d: d.update(surprise=1), "malformed", id="extra-key"),
    pytest.param(lambda d: d.pop("blocks"), "malformed", id="missing-key"),
    pytest.param(
        lambda d: d.update(sha256=BlockList(d["size"], tuple(d["blocks"])).digest()),
        "malformed",
        id="whole-hash-is-the-block-list-key",
    ),
]


@pytest.mark.parametrize(("lie", "code"), BAD)
def test_a_declaration_that_lies_is_refused_before_anything_is_created(
    tmp_path: Path, lie: Callable[[dict[str, Any]], None], code: str
) -> None:
    fake = TimedFakeR2()
    broker, _ = make_broker(tmp_path, fake)
    session = open_session(broker, request=b"this attempt")
    declared = synthetic(sha(b"this attempt"), 9 * MiB + 3)
    lie(declared)

    answer = json.loads(declare(session, fake, encode(declared)))

    assert answer == {"version": 1, "refused": code}
    assert fake.served("CreateMultipartUpload") == []
    written = [row[1] for row in fake.served("PutObject")]
    assert written == [session.prefix + "declare.json", session.prefix + "offer.json"]
    assert session.state == "refused" and not session.failed.is_set()


@pytest.mark.parametrize(
    "raw",
    [b"{{{", b"[" * 20_000, b"null", b"\xff\xfe", b" " * 50],
    ids=["not-json", "nesting-bomb", "null", "not-utf8", "whitespace"],
)
def test_a_declaration_that_is_not_a_document_is_refused(tmp_path: Path, raw: bytes) -> None:
    fake = TimedFakeR2()
    broker, _ = make_broker(tmp_path, fake)
    session = open_session(broker)
    assert json.loads(declare(session, fake, raw)) == {"version": 1, "refused": "malformed"}
    assert fake.served("CreateMultipartUpload") == []


def test_a_declaration_over_the_cap_is_refused_without_being_read(tmp_path: Path) -> None:
    fake = TimedFakeR2()
    broker, settings = make_broker(tmp_path, fake, max_artifact_bytes=1 << 20)
    session = open_session(broker)
    junk = b"x" * (64 * 1024)  # the cap for a 1 MiB artifact is a few KiB
    assert broker.declaration_cap < len(junk)

    assert json.loads(declare(session, fake, junk)) == {"version": 1, "refused": "too_large"}

    assert fake.served("GetObject", "declare.json") == []  # sized by HEAD, never fetched
    assert fake.served("HeadObject", "declare.json")
    del settings


def test_the_declaration_is_read_once_whatever_the_host_does_next(tmp_path: Path) -> None:
    fake = TimedFakeR2()
    broker, _ = make_broker(tmp_path, fake)
    session = open_session(broker, request=b"r")
    first = synthetic(sha(b"r"), 9 * MiB + 3)
    offer = json.loads(declare(session, fake, encode(first)))
    assert offer["declare_sha256"] == sha(encode(first))
    # the host rewrites its declaration afterwards (write-once is not a property the design needs)
    fake.objects[session.prefix + "declare.json"] = (encode(synthetic(sha(b"r"), 3 * MiB)), {})
    assert len(fake.served("GetObject", "declare.json")) == 1
    assert session.declared is not None and session.declared.size == 9 * MiB + 3


@pytest.mark.parametrize("size", [2_100_000_000, 11_640_000_000, 103_000_000_000])
def test_the_control_plane_scales_to_the_artifact_sizes_of_the_adr(
    tmp_path: Path, size: int
) -> None:
    fake = TimedFakeR2()
    broker, settings = make_broker(
        tmp_path, fake, part_bytes=16 * MiB, max_artifact_bytes=128 << 30
    )
    session = open_session(broker, request=b"r")
    body = encode(synthetic(sha(b"r"), size, 16 * MiB))
    assert len(body) < broker.declaration_cap

    raw = declare(session, fake, body)

    offer = json.loads(raw)
    parts = -(-size // (16 * MiB))
    assert len(offer["parts"]) == parts and len(offer["list"]) == -(-parts // 1000)
    assert len(raw) < runner.OFFER_LIMIT  # what the container is willing to read
    assert len(fake.served("CreateMultipartUpload")) == 1
    sidecar = session.store.key(
        BlockList(size, tuple(synthetic(sha(b"r"), size)["blocks"])).digest()
    )
    assert sidecar in fake.objects
    del settings


# -- the answering thread cannot fail silently -------------------------------------------------


def test_a_store_that_breaks_while_waiting_ends_the_attempt_with_a_class_name(
    tmp_path: Path,
) -> None:
    fake = TimedFakeR2()
    broker, _ = make_broker(tmp_path, fake)
    session = open_session(broker)
    fake.fault_key("HeadObject", "declare.json", 503, 503, 503, 503, 503, 503)
    session.start()
    assert session.failed.wait(10)
    assert session.failure == "offer thread: R2Error"
    assert session.thread is not None
    session.thread.join(10)
    assert not session.thread.is_alive()


def test_a_refusal_that_cannot_be_written_also_ends_the_attempt(tmp_path: Path) -> None:
    fake = TimedFakeR2()
    broker, _ = make_broker(tmp_path, fake)
    session = open_session(broker, request=b"r")
    fake.fault_key("PutObject", "offer.json", *([503] * 6))
    http = client_for(fake).http
    http.put(session.block["declare_put"], content=b"{{{", headers={"if-none-match": "*"})
    session.start()
    assert session.failed.wait(10)
    assert session.failure == "refusal could not be written: R2Error"


def test_closing_while_the_upload_is_being_opened_leaves_no_open_upload(tmp_path: Path) -> None:
    fake = TimedFakeR2()
    broker, _ = make_broker(tmp_path, fake)
    session = open_session(broker, request=b"r")
    entered, release = fake.gate("CreateMultipartUpload")
    http = client_for(fake).http
    body = encode(synthetic(sha(b"r"), 9 * MiB + 3))
    http.put(session.block["declare_put"], content=body, headers={"if-none-match": "*"})
    session.start()
    assert entered.wait(10)  # the thread is inside CreateMultipartUpload

    closer = __import__("threading").Thread(target=session.close)
    closer.start()  # waits for the thread to end (bounded), then cleans
    release.set()
    closer.join(30)
    assert not closer.is_alive()
    if session.thread is not None:
        session.thread.join(10)

    assert fake.uploads == {}  # the upload the thread opened after the close was aborted by it


# -- what the host uploaded is checked before it is believed --------------------------------


def upload_as_host(fake: TimedFakeR2, session: at.Session, data: bytes, parts: list[int]) -> None:
    """The container's side of the offer, by hand: the parts of `data` that are listed."""
    assert wait_for(lambda: session.state == "offered")
    offer = json.loads(mailbox(session, fake)[1] or b"")
    plan = PartPlan(len(data), 8 * MiB)
    http = client_for(fake).http
    for number in parts:
        offset, length = plan.span(number)
        body = data[offset : offset + length]
        sent = http.put(
            offer["parts"][number - 1], content=body, headers={"x-amz-content-sha256": sha(body)}
        )
        assert sent.status_code == 200


def host_declaring(session: at.Session, fake: TimedFakeR2, claimed: bytes, sent: bytes) -> None:
    """A declaration whose whole hash is `claimed`'s but whose blocks and parts are `sent`'s."""
    plan = PartPlan(len(sent), 8 * MiB)
    s = scan(len(sent), lambda off, n: sent[off : off + n], plan)
    declared = synthetic(
        sha(b"r"),
        len(sent),
        sha256=sha(claimed),
        blocks=list(s.blocks.blocks),
        parts=list(s.part_sha256),
    )
    http = client_for(fake).http
    http.put(session.block["declare_put"], content=encode(declared), headers={"if-none-match": "*"})
    session.start()


def test_a_host_that_declares_other_bytes_than_it_uploads_is_refused_and_leaves_nothing(
    tmp_path: Path,
) -> None:
    fake = TimedFakeR2()
    broker, _ = make_broker(tmp_path, fake)
    session = open_session(broker, request=b"r")
    claimed = random.Random(1).randbytes(9 * MiB + 3)
    sent = random.Random(2).randbytes(len(claimed))
    host_declaring(session, fake, claimed, sent)
    upload_as_host(fake, session, sent, [1, 2])
    session.stop_thread()
    status = tmp_path / "status.out"
    status.write_bytes(
        json.dumps({"artifact": "uploaded", "sha256": sha(claimed), "size": len(sent)}).encode()
    )

    with pytest.raises(HostRefused, match="differ from the hashes"):
        session.collect(status, lambda: None, lost=False)

    assert sorted(os.listdir(tmp_path)) == sorted(["status.out", "r2-exec.env", "state", "tmp"])
    session.close()
    assert fake.uploads == {} and leftovers(fake, session) == [
        session.prefix + "declare.json",
        session.prefix + "offer.json",
    ]


def test_a_container_that_exits_zero_with_a_part_missing_has_not_delivered(
    tmp_path: Path,
) -> None:
    fake = TimedFakeR2()
    broker, _ = make_broker(tmp_path, fake)
    session = open_session(broker, request=b"r")
    data = random.Random(3).randbytes(17 * MiB)
    host_declaring(session, fake, data, data)
    upload_as_host(fake, session, data, [1, 3])  # part 2 never arrives
    session.stop_thread()
    status = tmp_path / "status.out"
    status.write_bytes(
        json.dumps({"artifact": "uploaded", "sha256": sha(data), "size": len(data)}).encode()
    )

    assert not session.parts_held()
    with pytest.raises(AttemptFailed, match="artifact upload failed: StoreError"):
        session.collect(status, lambda: None, lost=False)
    session.close()
    assert fake.uploads == {}


def test_a_status_line_that_does_not_say_what_was_declared_is_not_believed(
    tmp_path: Path,
) -> None:
    fake = TimedFakeR2()
    broker, _ = make_broker(tmp_path, fake)
    session = open_session(broker, request=b"r")
    data = random.Random(4).randbytes(9 * MiB)
    host_declaring(session, fake, data, data)
    upload_as_host(fake, session, data, [1, 2])
    session.stop_thread()
    assert session.parts_held()  # complete on the store: only the status line is wrong
    status = tmp_path / "status.out"
    for line in (
        b"",
        b"{}",
        b"x" * 3000,
        json.dumps({"artifact": "uploaded", "sha256": "0" * 64, "size": len(data)}).encode(),
    ):
        status.write_bytes(line)
        with pytest.raises(AttemptFailed, match="status line"):
            session.collect(status, lambda: None, lost=False)
    session.close()


def test_a_download_stops_when_the_lease_is_cancelled_and_leaves_no_file(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    fake = TimedFakeR2()
    broker, _ = make_broker(tmp_path, fake)
    pair = run_pair(broker, fake, monkeypatch)
    status = tmp_path / "status.out"
    status.write_bytes(pair.out)
    calls: list[int] = []

    def stop() -> None:
        calls.append(1)
        if len(calls) == 2:
            raise Aborted("lease cancelled")

    with pytest.raises(Aborted):
        pair.session.collect(status, stop, lost=False)

    assert status.read_bytes() == pair.out  # the status line was not replaced by half a tar
    assert sorted(os.listdir(tmp_path)) == sorted(["status.out", "r2-exec.env", "state", "tmp"])
    pair.session.close()


def test_a_host_lost_after_its_last_part_is_salvaged_from_what_the_store_holds(
    tmp_path: Path,
) -> None:
    fake = TimedFakeR2()
    broker, _ = make_broker(tmp_path, fake)
    session = open_session(broker, request=b"r")
    data = random.Random(5).randbytes(17 * MiB)
    host_declaring(session, fake, data, data)
    upload_as_host(fake, session, data, [1, 2, 3])
    session.stop_thread()
    status = tmp_path / "status.out"
    status.write_bytes(b"")  # ssh was cut: no status line

    assert session.parts_held()
    session.collect(status, lambda: None, lost=True)
    assert status.read_bytes() == data  # whole sha256 and every block vouch for it, not the exit
    session.close()


# -- time to live of the URLs ------------------------------------------------------------------


@pytest.mark.parametrize(
    ("left", "ttl"), [(1, 31), (10, 40), (3570, 3600), (100_000, 3600)], ids=str
)
def test_urls_live_as_long_as_the_lease_plus_a_margin_and_never_past_the_cap(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, left: int, ttl: int
) -> None:
    fake = TimedFakeR2()
    broker, _ = make_broker(tmp_path, fake)
    now = time.time()
    job = Job()
    job.deadline = now + left
    monkeypatch.setattr(time, "time", lambda: now)
    session = broker.begin(job, "audit")  # type: ignore[arg-type]
    assert session is not None
    for name in ("declare_put", "offer_get"):
        assert parse_qs(urlsplit(session.block[name]).query)["X-Amz-Expires"] == [str(ttl)]


def test_a_lease_that_is_over_gets_no_urls(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    fake = TimedFakeR2()
    broker, _ = make_broker(tmp_path, fake)
    job = Job(seconds=-5)
    with pytest.raises(Aborted):
        broker.begin(job, "audit")  # type: ignore[arg-type]


def test_an_operation_that_is_not_listed_runs_the_classic_way(tmp_path: Path) -> None:
    fake = TimedFakeR2()
    broker, _ = make_broker(tmp_path, fake)
    assert broker.begin(Job(), "reference") is None  # type: ignore[arg-type]
    assert fake.ledger == [] and kinds(broker) == []


@pytest.mark.parametrize(("fallback", "outcome"), [("classic", None), ("fail", AttemptFailed)])
def test_a_store_that_is_down_at_the_start_falls_back_or_fails_as_configured(
    tmp_path: Path, fallback: str, outcome: type[Exception] | None
) -> None:
    fake = TimedFakeR2()
    fake.fault_key("HeadObject", "declare.json", *([503] * 10))
    broker, _ = make_broker(tmp_path, fake, fallback=fallback)
    if outcome is None:
        assert broker.begin(Job(), "audit") is None  # type: ignore[arg-type]
    else:
        with pytest.raises(outcome):
            broker.begin(Job(), "audit")  # type: ignore[arg-type]
    assert kinds(broker) == ["exec_artifact_unavailable"]


# -- the credential -------------------------------------------------------------------------


def test_the_credential_file_must_be_private_small_and_for_a_cortex_bucket(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    good = credentials_file(tmp_path, "https://r2.test")
    assert at.load_credentials(good).bucket == BUCKET

    monkeypatch.setenv("R2_BUCKET", "hypertrain-data")  # the process environment does not count
    assert at.load_credentials(good).bucket == BUCKET

    wide = tmp_path / "wide.env"
    wide.write_bytes(good.read_bytes())
    wide.chmod(0o640)
    with pytest.raises(ValueError, match="mode 0600"):
        at.load_credentials(wide)

    link = tmp_path / "link.env"
    link.symlink_to(good)
    with pytest.raises(ValueError, match="regular file"):
        at.load_credentials(link)

    broad = tmp_path / "broad.env"
    broad.write_text(good.read_text() + "CLOUDFLARE_" + "API_TOKEN=x\n")
    broad.chmod(0o600)
    with pytest.raises(ValueError, match="other than the R2 settings"):
        at.load_credentials(broad)

    for name, text in {
        "bucket": good.read_text().replace(BUCKET, "hypertrain-data"),
        "http": good.read_text().replace("https://r2.test", "http://r2.test"),
    }.items():
        other = tmp_path / f"{name}.env"
        other.write_text(text)
        other.chmod(0o600)
        with pytest.raises(ValueError):
            at.load_credentials(other)


def test_settings_refuse_what_r2_would_refuse(tmp_path: Path) -> None:
    good = credentials_file(tmp_path, "https://r2.test")
    with pytest.raises(ValueError, match="4 MiB"):
        at.ArtifactSettings(credentials_file=good, part_bytes=10 * MiB)
    with pytest.raises(ValueError, match="10,000 parts"):
        at.ArtifactSettings(credentials_file=good, part_bytes=8 * MiB, max_artifact_bytes=100 << 30)
    with pytest.raises(ValueError):
        at.ArtifactSettings(credentials_file=good, operations=())
    with pytest.raises(ValueError):
        at.ArtifactSettings(credentials_file=good, workers=True)  # type: ignore[arg-type]


# -- no URL anywhere it should not be --------------------------------------------------------


def signatures(urls: list[str]) -> list[str]:
    return [parse_qs(urlsplit(u).query)["X-Amz-Signature"][0] for u in urls]


def test_no_url_reaches_a_log_a_journal_or_an_error_and_the_harvest_can_see_one(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    fake = TimedFakeR2()
    broker, settings = make_broker(tmp_path, fake)
    issued: list[str] = []
    original = broker.client.presign

    def spy(*args: Any, **kwargs: Any) -> str:
        url = original(*args, **kwargs)
        issued.append(url)
        return url

    monkeypatch.setattr(broker.client, "presign", spy)
    texts: list[str] = []

    pair = run_pair(broker, fake, monkeypatch)  # success path
    status = tmp_path / "status.out"
    status.write_bytes(pair.out)
    pair.session.collect(status, lambda: None, lost=False)
    texts += [pair.err, repr(pair.session), repr(broker), repr(settings), pair.out.decode()]
    pair.session.close()

    bad = open_session(broker, request=b"r")  # a refused declaration
    declare(bad, fake, b"{{{")
    texts.append(bad.failure)
    bad.close()

    lying = open_session(broker, request=b"r")  # a lying host, its refusal text
    claimed = random.Random(6).randbytes(9 * MiB)
    host_declaring(lying, fake, claimed, random.Random(7).randbytes(len(claimed)))
    upload_as_host(fake, lying, random.Random(7).randbytes(len(claimed)), [1, 2])
    lying.stop_thread()
    status.write_bytes(
        json.dumps({"artifact": "uploaded", "sha256": sha(claimed), "size": len(claimed)}).encode()
    )
    with pytest.raises(HostRefused) as refused:
        lying.collect(status, lambda: None, lost=False)
    texts += [str(refused.value), repr(refused.value), lying.failure]
    lying.close()

    texts.append(broker.journal.path.read_text())
    harvest = signatures(issued)
    assert len(harvest) >= 4 and all(len(s) == 64 for s in harvest)
    # the positive control: the harvest is real, and the one place the URLs travel has them
    assert all(
        s in pair.raw.decode()
        for s in signatures([pair.session.block["declare_put"], pair.session.block["offer_get"]])
    )
    haystack = "\n".join(texts)
    assert not [s for s in harvest if s in haystack]
    assert "X-Amz-" not in haystack and "r2.test" not in haystack
