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
import threading
import time
import xml.etree.ElementTree as ET
from collections.abc import Callable, Iterator
from pathlib import Path
from types import SimpleNamespace
from typing import Any
from urllib.parse import parse_qs, urlsplit

import httpx
import pytest

from hypertrain.data.store import ObjectNotFound, StoreError
from hypertrain.gpu_ops import artifact_transfer as at
from hypertrain.gpu_ops import cuda_executor_runner as runner
from hypertrain.gpu_ops.cuda_executor import Aborted, AttemptFailed, HostRefused
from hypertrain.gpu_ops.journal import Journal
from hypertrain.protocol.jcs import canonicalize
from hypertrain.storage.cas import BLOCK_BYTES, BlockList, PartPlan, scan
from hypertrain.storage.r2 import ObjectConflict, ObjectInfo, R2Error, UploadGone

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
    measured,
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


@pytest.fixture(autouse=True)
def plenty_of_disk(monkeypatch: pytest.MonkeyPatch) -> None:
    """The disk gate reads the free space of the machine; a test of something else must not
    depend on what other jobs on the host have left of it."""
    monkeypatch.setattr(
        at, "shutil", SimpleNamespace(disk_usage=lambda path: SimpleNamespace(free=1 << 50))
    )


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
        io.BytesIO(package(FILES, raw)),
        out,
        err,
        probe=probe_ok,
        launch=launch_tree,
        measure=measured,
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
    sentinel = (
        "exec/other/" + "c" * 64 + "/x/r/" + "d" * 64,  # another job's attempt
        "exec/" + "a" * 64 + "/" + "c" * 64 + "/n/declare.json",  # another job of this run
        "runs/keep",
    )
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

    assert session.store.workers == settings.workers  # the download uses what was configured
    session.close()
    assert fake.uploads == {}
    assert leftovers(fake, session) == []  # object, block list and both mailbox objects are gone
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
    pattern = r"exec/a{64}/b{64}/[0-9a-f]{32}/"
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
        lambda d: d.update(part_bytes=16 * MiB, parts=d["parts"][:1]),  # lists agree with it
        "malformed",
        id="part-size-chosen-by-host",
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


def test_the_declaration_is_read_by_range_even_if_the_object_grows_after_it_was_sized(
    tmp_path: Path,
) -> None:
    fake = TimedFakeR2()
    broker, _ = make_broker(tmp_path, fake)
    session = open_session(broker, request=b"r")
    key = session.prefix + "declare.json"
    small = encode(synthetic(sha(b"r"), 9 * MiB + 3))
    http = client_for(fake).http
    http.put(session.block["declare_put"], content=small, headers={"if-none-match": "*"})
    fake.hooks["GetObject"] = lambda: fake.objects.__setitem__(key, (b"x" * (4 * MiB), {}))

    session.start()
    assert session.thread is not None
    session.thread.join(10)

    (read,) = fake.served("GetObject", "declare.json")
    assert read[4] == len(small)  # the bytes HEAD announced, not the 4 MiB that replaced them
    assert json.loads(mailbox(session, fake)[1] or b"") == {"version": 1, "refused": "malformed"}


def test_a_declaration_url_cannot_overwrite_what_it_has_written(tmp_path: Path) -> None:
    fake = TimedFakeR2()
    broker, _ = make_broker(tmp_path, fake)
    session = open_session(broker)
    http = client_for(fake).http
    url, headers = session.block["declare_put"], {"if-none-match": "*"}
    assert http.put(url, content=b"first", headers=headers).status_code == 200
    assert http.put(url, content=b"second", headers=headers).status_code == 412
    assert fake.objects[session.prefix + "declare.json"][0] == b"first"


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


def test_a_store_that_stays_broken_while_waiting_ends_the_attempt_with_a_class_name(
    tmp_path: Path,
) -> None:
    fake = TimedFakeR2()
    broker, _ = make_broker(tmp_path, fake, store_blip_seconds=1)
    session = open_session(broker)
    fake.fault_key("HeadObject", "declare.json", *([503] * 5000))
    session.start()
    assert session.failed.wait(15)
    assert session.failure == "offer thread: R2Error"
    assert session.thread is not None
    session.thread.join(10)
    assert not session.thread.is_alive()


def test_a_blip_of_the_store_while_the_island_runs_does_not_cost_the_attempt(
    tmp_path: Path,
) -> None:
    fake = TimedFakeR2()
    broker, _ = make_broker(tmp_path, fake)  # retry_attempts 3: six faults are two failed rounds
    session = open_session(broker, request=b"r")
    fake.fault_key("HeadObject", "declare.json", *([503] * 6))
    session.start()
    time.sleep(0.2)  # the island is still running
    body = encode(synthetic(sha(b"r"), 9 * MiB + 3))
    client_for(fake).http.put(
        session.block["declare_put"], content=body, headers={"if-none-match": "*"}
    )
    assert wait_for(lambda: session.state == "offered")
    assert not session.failed.is_set()
    session.close()


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

    closer = threading.Thread(target=session.close)
    closer.start()  # waits for the thread to end (bounded), then cleans
    release.set()
    closer.join(30)
    assert not closer.is_alive()
    if session.thread is not None:
        session.thread.join(10)

    assert fake.uploads == {}  # the upload the thread opened after the close was aborted by it
    assert leftovers(fake, session) == []  # and its block list and the mailbox went with it


def test_an_upload_the_session_does_not_know_about_is_found_by_its_key_and_aborted(
    tmp_path: Path,
) -> None:
    fake = TimedFakeR2()
    broker, _ = make_broker(tmp_path, fake)
    session = open_session(broker, request=b"r")
    data = random.Random(9).randbytes(9 * MiB)
    plan = PartPlan(len(data), 8 * MiB)
    s = scan(len(data), lambda off, n: data[off : off + n], plan)
    session.declared = at.Declared(len(data), s.sha256, s.blocks.blocks, s.part_sha256)
    # offer_upload failed after CreateMultipartUpload: nothing recorded the upload id
    broker.client.create_multipart(session.store.key(s.sha256))
    assert len(fake.uploads) == 1

    session.close()
    assert fake.uploads == {}


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
    assert fake.uploads == {} and leftovers(fake, session) == []


def test_a_part_that_is_held_but_short_is_not_a_delivered_part(tmp_path: Path) -> None:
    fake = TimedFakeR2()
    broker, _ = make_broker(tmp_path, fake)
    session = open_session(broker, request=b"r")
    data = random.Random(8).randbytes(9 * MiB)
    short = b"tiny"
    plan = PartPlan(len(data), 8 * MiB)
    s = scan(len(data), lambda off, n: data[off : off + n], plan)
    declared = synthetic(
        sha(b"r"),
        len(data),
        sha256=sha(data),
        blocks=list(s.blocks.blocks),
        parts=[s.part_sha256[0], sha(short)],  # the host binds part 2 to four bytes
    )
    client_for(fake).http.put(
        session.block["declare_put"], content=encode(declared), headers={"if-none-match": "*"}
    )
    session.start()
    assert wait_for(lambda: session.state == "offered")
    offer = json.loads(mailbox(session, fake)[1] or b"")
    http = client_for(fake).http
    for url, body in zip(offer["parts"], (data[: 8 * MiB], short), strict=True):
        sent = http.put(url, content=body, headers={"x-amz-content-sha256": sha(body)})
        assert sent.status_code == 200
    session.stop_thread()

    assert not session.parts_held()  # part 2 is there, but at 4 bytes and not at 1 MiB + 3
    session.close()


def test_closing_waits_for_the_answering_thread_to_stop(tmp_path: Path) -> None:
    fake = TimedFakeR2()
    broker, _ = make_broker(tmp_path, fake)
    session = open_session(broker)
    entered, release = fake.gate("HeadObject")
    session.start()
    assert entered.wait(10)  # the thread is inside a request

    closer = threading.Thread(target=session.close)
    closer.start()
    closer.join(0.3)
    assert closer.is_alive()  # close() does not return while the thread can still act
    release.set()
    closer.join(15)
    assert not closer.is_alive()


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


def test_an_object_that_is_not_the_one_declared_is_not_fetched(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    fake = TimedFakeR2()
    broker, _ = make_broker(tmp_path, fake)
    pair = run_pair(broker, fake, monkeypatch)
    status = tmp_path / "status.out"
    status.write_bytes(pair.out)
    real = broker.client.head_object

    def other_blocks(key: str) -> ObjectInfo | None:
        info = real(key)
        return info and ObjectInfo(info.size, info.sha256, {**info.meta, "blocks": "0" * 64})

    monkeypatch.setattr(broker.client, "head_object", other_blocks)
    with pytest.raises(AttemptFailed, match="not the one that was declared"):
        pair.session.collect(status, lambda: None, lost=False)
    assert [
        r for r in fake.served("GetObject") if r[1].endswith(pair.session.declared.sha256)
    ] == []  # type: ignore[union-attr]
    pair.session.close()


def test_two_downloads_at_once_do_not_both_count_on_the_same_free_disk(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    fake = TimedFakeR2()
    broker, _ = make_broker(tmp_path, fake)
    monkeypatch.setattr(
        at, "shutil", SimpleNamespace(disk_usage=lambda path: SimpleNamespace(free=5 << 30))
    )
    first = broker.reserve(3 << 30, tmp_path)  # one attempt has set 3 GiB aside
    with pytest.raises(AttemptFailed, match="not enough disk"):
        broker.reserve(3 << 30, tmp_path)  # 5 GiB free is enough for one, not for both
    broker.release(first)
    assert broker.reserve(3 << 30, tmp_path) == 3 << 30


def test_a_disk_too_small_for_the_artifact_is_found_out_before_the_download(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    fake = TimedFakeR2()
    broker, _ = make_broker(tmp_path, fake)
    pair = run_pair(broker, fake, monkeypatch)
    status = tmp_path / "status.out"
    status.write_bytes(pair.out)
    monkeypatch.setattr(
        at, "shutil", SimpleNamespace(disk_usage=lambda path: SimpleNamespace(free=1 << 20))
    )

    with pytest.raises(AttemptFailed, match="not enough disk"):
        pair.session.collect(status, lambda: None, lost=False)
    assert [
        r for r in fake.served("GetObject") if r[1].endswith(pair.session.declared.sha256)
    ] == []  # type: ignore[union-attr]
    pair.session.close()


def test_a_store_that_cannot_answer_about_the_parts_is_a_no_and_not_a_crash(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    fake = TimedFakeR2()
    broker, _ = make_broker(tmp_path, fake)
    pair = run_pair(broker, fake, monkeypatch)
    session = pair.session
    assert session.parts_held()

    def garbled(*args: Any, **kwargs: Any) -> Any:
        raise ET.ParseError("not xml")

    monkeypatch.setattr(broker.client, "list_parts", garbled)
    assert not session.parts_held()  # the host stays lost
    status = tmp_path / "status.out"
    status.write_bytes(pair.out)
    with pytest.raises(AttemptFailed, match="artifact upload failed: ParseError"):
        session.collect(status, lambda: None, lost=False)  # finish_upload lists the parts too
    session.close()


@pytest.mark.parametrize(
    "error", [OSError, ObjectNotFound, UploadGone, ObjectConflict, StoreError, R2Error]
)
def test_every_storage_error_after_the_upload_is_an_attempt_failure_that_names_its_class(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, error: type[Exception]
) -> None:
    fake = TimedFakeR2()
    broker, _ = make_broker(tmp_path, fake)
    pair = run_pair(broker, fake, monkeypatch)
    status = tmp_path / "status.out"
    status.write_bytes(pair.out)

    def broken(*args: Any, **kwargs: Any) -> Any:
        raise error("secret detail that must not travel")

    real_finish = at.finish_upload
    monkeypatch.setattr(at, "finish_upload", broken)
    with pytest.raises(AttemptFailed) as caught:
        pair.session.collect(status, lambda: None, lost=False)
    assert str(caught.value) == f"artifact upload failed: {error.__name__}"
    monkeypatch.setattr(at, "finish_upload", real_finish)
    with pytest.raises(AttemptFailed) as download:
        monkeypatch.setattr(at.R2ObjectStore, "fetch_to", broken)
        pair.session.collect(status, lambda: None, lost=False)
    assert str(download.value) == f"artifact download failed: {error.__name__}"
    pair.session.close()


def test_closing_never_raises_and_the_journal_says_what_could_not_be_cleaned(
    tmp_path: Path,
) -> None:
    fake = TimedFakeR2()
    broker, _ = make_broker(tmp_path, fake)
    session = open_session(broker, request=b"r")
    declare(session, fake, encode(synthetic(sha(b"r"), 9 * MiB + 3)))
    assert session.state == "offered" and len(fake.uploads) == 1
    fake.fault_key("AbortMultipartUpload", "", *([503] * 10))
    fake.fault_key("DeleteObject", "", *([503] * 40))

    session.close()  # the store refuses every cleanup call: this must still return

    recorded = kinds(broker)
    assert recorded.count("exec_artifact_cleanup_failed") >= 3
    assert recorded[-1] == "exec_artifact_end"


def test_a_status_line_that_names_another_size_or_state_is_refused(tmp_path: Path) -> None:
    fake = TimedFakeR2()
    broker, _ = make_broker(tmp_path, fake)
    session = open_session(broker, request=b"r")
    data = random.Random(11).randbytes(9 * MiB)
    host_declaring(session, fake, data, data)
    upload_as_host(fake, session, data, [1, 2])
    session.stop_thread()
    status = tmp_path / "status.out"
    for line in (
        {"artifact": "pending", "sha256": sha(data), "size": len(data)},
        {"artifact": "uploaded", "sha256": sha(data), "size": len(data) + 1},
    ):
        status.write_bytes(json.dumps(line).encode())
        with pytest.raises(AttemptFailed, match="status line"):
            session.collect(status, lambda: None, lost=False)
    session.close()


@pytest.mark.parametrize("slack", [-1, 0], ids=["one-byte-short", "exactly-enough"])
def test_the_disk_gate_is_two_point_two_times_the_artifact_plus_one_gib(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, slack: int
) -> None:
    fake = TimedFakeR2()
    broker, _ = make_broker(tmp_path, fake)
    pair = run_pair(broker, fake, monkeypatch)
    status = tmp_path / "status.out"
    status.write_bytes(pair.out)
    size = pair.session.declared.size  # type: ignore[union-attr]
    asked: list[Path] = []

    def usage(path: Path) -> SimpleNamespace:
        asked.append(path)
        return SimpleNamespace(free=int(2.2 * size) + (1 << 30) + slack)

    monkeypatch.setattr(at, "shutil", SimpleNamespace(disk_usage=usage))
    if slack < 0:
        with pytest.raises(AttemptFailed, match="not enough disk"):
            pair.session.collect(status, lambda: None, lost=False)
    else:
        pair.session.collect(status, lambda: None, lost=False)
    assert asked == [tmp_path]  # the disk the download is written to
    pair.session.close()


def test_the_journal_records_what_a_sweep_by_hand_would_need(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    fake = TimedFakeR2()
    broker, settings = make_broker(tmp_path, fake)
    job = Job()
    pair = run_pair(broker, fake, monkeypatch, job)
    status = tmp_path / "status.out"
    status.write_bytes(pair.out)
    pair.session.collect(status, lambda: None, lost=False)
    session = pair.session
    session.close()

    records = [json.loads(line) for line in broker.journal.path.read_text().splitlines()]
    begin, offered, done, end = records
    assert (begin["nonce"], begin["prefix"]) == (session.nonce, session.prefix)
    assert begin["expires_unix"] == int(job.deadline) + settings.url_margin_seconds
    declared = session.declared
    assert declared is not None
    assert (offered["size"], offered["parts"]) == (declared.size, len(declared.parts))
    assert (done["sha256"], done["size"]) == (declared.sha256, declared.size)
    assert 0 <= done["upload_ms"] < 30_000 and 0 <= done["download_ms"] < 30_000  # milliseconds
    assert end["nonce"] == session.nonce


WIRE = {
    "part_bytes": "part_bytes",
    "workers": "workers",
    "reserve_seconds": "reserve_seconds",
    "poll_ms": "poll_ms",
    "offer_wait_s": "offer_wait_seconds",
    "retry_attempts": "retry_attempts",
    "retry_base_ms": "retry_base_ms",
    "timeout_s": "request_timeout_seconds",
}


class _Scripted:
    """Stands in for the `time` module inside artifact_transfer: a scripted monotonic clock."""

    def __init__(self, *readings: float) -> None:
        self._readings = iter(readings)

    def __getattr__(self, name: str) -> Any:
        return getattr(time, name)

    def monotonic(self) -> float:
        return next(self._readings)


def test_the_announced_lifetime_is_what_the_urls_have_left_less_elapsed_poll_and_slack(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    fake = TimedFakeR2()
    broker, _ = make_broker(tmp_path, fake)  # poll_ms 10: one second rounded up
    session = open_session(broker, request=b"r")
    monkeypatch.setattr(at, "time", _Scripted(100.0, 103.2))  # 3.2 s between making and announcing
    offer = json.loads(declare(session, fake, encode(synthetic(sha(b"r"), 9 * MiB + 3))))
    lived = int(parse_qs(urlsplit(offer["parts"][0]).query)["X-Amz-Expires"][0])
    assert offer["ttl_s"] == lived - 4 - 1 - 5


def test_the_size_and_the_declaration_are_accepted_exactly_at_their_caps(tmp_path: Path) -> None:
    fake = TimedFakeR2()
    broker, _ = make_broker(tmp_path, fake, max_artifact_bytes=1 << 30)
    assert broker.declaration_cap == 4096 + 72 * (256 + 128)
    body = encode(synthetic(sha(b"r"), 1 << 30))
    for pad, answer in ((0, None), (1, "too_large")):
        session = open_session(broker, request=b"r")
        padded = body + b" " * (broker.declaration_cap - len(body) + pad)
        assert len(padded) == broker.declaration_cap + pad
        got = json.loads(declare(session, fake, padded))
        assert got.get("refused") == answer
        session.close()


@pytest.mark.parametrize("wire", sorted(WIRE))
@pytest.mark.parametrize("extreme", ["ge", "le"])
def test_every_setting_reaches_the_container_as_its_own_field_within_the_runner_bounds(
    tmp_path: Path, wire: str, extreme: str
) -> None:
    field = at.ArtifactSettings.model_fields[WIRE[wire]]
    value = next(getattr(m, extreme) for m in field.metadata if hasattr(m, extreme))
    fake = TimedFakeR2()
    broker, _ = make_broker(tmp_path, fake, **{WIRE[wire]: value})
    session = broker.begin(Job(seconds=100_000), "audit")  # type: ignore[arg-type]
    assert session is not None
    assert session.block[wire] == value
    assert runner.artifact_block({"artifact": session.block}) is not None  # its bounds hold it
    others = {w: session.block[w] for w in WIRE if w != wire}
    defaults = {w: getattr(broker.settings, WIRE[w]) for w in WIRE if w != wire}
    assert others == defaults  # no other field was swapped with it


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
    ("left", "ttl"), [(30, 60), (100, 130), (3570, 3600), (100_000, 100_030)], ids=str
)
def test_the_mailbox_urls_live_as_long_as_the_lease_plus_a_margin(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, left: int, ttl: int
) -> None:
    fake = TimedFakeR2()
    broker, _ = make_broker(tmp_path, fake, reserve_seconds=0)
    now = time.time()
    job = Job()
    job.deadline = now + left
    monkeypatch.setattr(time, "time", lambda: now)
    session = broker.begin(job, "audit")  # type: ignore[arg-type]
    assert session is not None
    for name in ("declare_put", "offer_get"):
        assert parse_qs(urlsplit(session.block[name]).query)["X-Amz-Expires"] == [str(ttl)]


def test_part_urls_never_live_past_the_cap_though_the_lease_is_longer(tmp_path: Path) -> None:
    fake = TimedFakeR2()
    broker, _ = make_broker(tmp_path, fake)
    session = open_session(broker, Job(seconds=100_000), request=b"r")
    offer = json.loads(declare(session, fake, encode(synthetic(sha(b"r"), 9 * MiB + 3))))
    for url in (offer["parts"][0], offer["list"][0]):
        assert parse_qs(urlsplit(url).query)["X-Amz-Expires"] == ["3600"]
    assert offer["ttl_s"] < 3600  # and the container is told less than that


def test_the_lifetime_announced_to_the_container_is_shorter_than_the_urls_have_left(
    tmp_path: Path,
) -> None:
    fake = TimedFakeR2()
    broker, _ = make_broker(tmp_path, fake)
    session = open_session(broker, request=b"r")
    offer = json.loads(declare(session, fake, encode(synthetic(sha(b"r"), 9 * MiB + 3))))
    lived = int(parse_qs(urlsplit(offer["parts"][0]).query)["X-Amz-Expires"][0])
    assert offer["ttl_s"] <= lived - at.OFFER_SLACK  # it starts counting later than they do


def test_an_offer_that_would_be_dead_on_arrival_is_refused(tmp_path: Path) -> None:
    fake = TimedFakeR2()
    broker, _ = make_broker(tmp_path, fake, url_margin_seconds=0, reserve_seconds=0)
    job = Job(seconds=at.MIN_ISLAND_SECONDS + 4)  # urls of 34 s: 34 - 1 - 5 is fine...
    session = open_session(broker, job, request=b"r")
    session.job.deadline = time.time() + 5  # ...but by the time the offer is made 5 s are left
    assert json.loads(declare(session, fake, encode(synthetic(sha(b"r"), 9 * MiB + 3)))) == {
        "version": 1,
        "refused": "deadline",
    }


def test_a_lease_too_short_for_the_reserve_runs_the_ssh_way(tmp_path: Path) -> None:
    fake = TimedFakeR2()
    broker, _ = make_broker(tmp_path, fake, reserve_seconds=60)
    assert broker.begin(Job(seconds=60 + at.MIN_ISLAND_SECONDS - 5), "audit") is None  # type: ignore[arg-type]
    assert kinds(broker) == ["exec_artifact_skipped"] and fake.ledger == []
    assert broker.begin(Job(seconds=60 + at.MIN_ISLAND_SECONDS + 5), "audit") is not None  # type: ignore[arg-type]


def test_a_lease_that_ends_before_the_offer_is_made_gets_a_refusal(tmp_path: Path) -> None:
    fake = TimedFakeR2()
    broker, _ = make_broker(tmp_path, fake)
    session = open_session(broker, request=b"r")
    session.job.deadline = time.time() - 1  # the island ran past the lease
    assert json.loads(declare(session, fake, encode(synthetic(sha(b"r"), 9 * MiB + 3)))) == {
        "version": 1,
        "refused": "deadline",
    }
    assert fake.served("CreateMultipartUpload") == []


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
        "loopback-lookalike": good.read_text().replace(
            "https://r2.test", "http://127.0.0.1.evil.example"
        ),
        "loopback-user-info": good.read_text().replace(
            "https://r2.test", "http://127.0.0.1@evil.example"
        ),
        "a-name-that-resolves-to-loopback": good.read_text().replace(
            "https://r2.test", "http://localhost:9000"
        ),
        "no-host": good.read_text().replace("https://r2.test", "https://"),
    }.items():
        other = tmp_path / f"{name}.env"
        other.write_text(text)
        other.chmod(0o600)
        with pytest.raises(ValueError):
            at.load_credentials(other)


def test_plain_http_is_for_a_literal_loopback_address_only(tmp_path: Path) -> None:
    good = credentials_file(tmp_path, "http://127.0.0.1:9000")
    assert at.load_credentials(good).url == "http://127.0.0.1:9000"
    six = credentials_file(tmp_path, "http://[::1]:9000")
    assert at.load_credentials(six).url == "http://[::1]:9000"


def test_the_process_environment_does_not_steer_the_traffic_to_the_store(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("HTTPS_PROXY", "http://proxy.invalid:3128")
    monkeypatch.setenv("SSL_CERT_FILE", str(tmp_path / "other-ca.pem"))
    broker, _ = make_broker(tmp_path, TimedFakeR2())
    assert broker.client.http.trust_env is False and broker._cleanup.http.trust_env is False


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
