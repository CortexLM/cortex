import hashlib
import json
import os
import threading
from pathlib import Path

import pytest

from hypertrain.auditor.replay import pack_state
from hypertrain.data.store import (
    CorruptObjectError,
    LocalFSStore,
    ObjectNotFound,
    StoreError,
    _publish,
    fetch_to,
    get_large,
    parse_index,
    put_file,
    put_large,
)
from hypertrain.protocol.messages_v2 import (
    INDEX_MAGIC,
    MAX_MANIFEST_BYTES,
    MAX_PARTS,
    PART_BYTES,
    ArtifactRef,
    PartIndexV1,
)
from hypertrain.trainer.compress import CompressConfig, compress


class Recorder:
    """LocalFSStore wrapper that records publish order."""

    def __init__(self, root: Path) -> None:
        self.store = LocalFSStore(root)
        self.puts: list[bytes] = []

    def put(self, data: bytes) -> str:
        self.puts.append(data)
        return self.store.put(data)


def _blob(n: int) -> bytes:
    return os.urandom(n)


@pytest.mark.parametrize("n", [1, PART_BYTES - 1, PART_BYTES])
def test_plain_unchanged(tmp_path: Path, n: int) -> None:
    rec, data = Recorder(tmp_path / "s"), _blob(n)
    assert put_large(data, rec.put) == hashlib.sha256(data).hexdigest()
    assert rec.puts == [data]
    src = tmp_path / "f"
    src.write_bytes(data)
    assert put_file(src, rec.put) == hashlib.sha256(data).hexdigest()


@pytest.mark.parametrize("n", [PART_BYTES + 1, 3 * PART_BYTES, 3 * PART_BYTES + 7])
def test_roundtrip_index_last(tmp_path: Path, n: int) -> None:
    rec, data = Recorder(tmp_path / "s"), _blob(n)
    src, dest = tmp_path / "src", tmp_path / "out" / "dest"
    src.write_bytes(data)
    dest.parent.mkdir()
    ref = put_file(src, rec.put)
    index = parse_index(rec.puts[-1])
    assert index is not None and hashlib.sha256(rec.puts[-1]).hexdigest() == ref
    assert index.sha256 == hashlib.sha256(data).hexdigest() and index.size == n
    assert [hashlib.sha256(p).hexdigest() for p in rec.puts[:-1]] == index.parts
    ArtifactRef(sha256=ref, size=len(rec.puts[-1]))
    assert put_large(data, rec.put) == ref
    fetch_to(ref, rec.store.get, dest)
    assert dest.read_bytes() == data and get_large(ref, rec.store.get) == data
    assert os.listdir(dest.parent) == ["dest"]


def test_real_serializers_never_look_like_index(tmp_path: Path) -> None:
    import torch  # after hypertrain imports: determinism setup must precede torch

    torch.manual_seed(0)
    theta = {"w": torch.randn(300_000), "b": torch.randn(8)}
    state = pack_state(theta)
    delta, _ = compress(CompressConfig(), theta, {k: torch.zeros_like(v) for k, v in theta.items()})
    rec = Recorder(tmp_path / "s")
    put_large(state, rec.put)
    assert len(state) > PART_BYTES and parse_index(rec.puts[-1]) is not None
    for blob in (state, delta, b"[]", b"{}", b'{"parts":[]}', INDEX_MAGIC[:-1]):
        assert parse_index(blob) is None
        assert get_large(put_large(blob, rec.put), rec.store.get) == blob


def test_plain_with_magic_prefix_rejected(tmp_path: Path) -> None:
    rec = Recorder(tmp_path / "s")
    with pytest.raises(StoreError):
        put_large(INDEX_MAGIC + b'{"parts":[]}', rec.put)
    assert rec.puts == []


def _setup(tmp_path: Path, n: int = 3 * PART_BYTES + 5) -> tuple[Recorder, bytes, PartIndexV1]:
    rec, data = Recorder(tmp_path / "s"), _blob(n)
    put_large(data, rec.put)
    index = parse_index(rec.puts[-1])
    assert index is not None
    return rec, data, index


def _bad(rec: Recorder, index: PartIndexV1, **kw: object) -> str:
    return rec.store.put(INDEX_MAGIC + json.dumps(index.body() | kw).encode())


def _assert_rejected(tmp_path: Path, rec: Recorder, ref: str, exc: type[Exception]) -> None:
    out = tmp_path / "out"
    out.mkdir(exist_ok=True)
    (out / "dest").write_bytes(b"old")
    with pytest.raises(exc):
        fetch_to(ref, rec.store.get, out / "dest")
    assert os.listdir(out) == ["dest"] and (out / "dest").read_bytes() == b"old"


def test_tampered_indexes_rejected(tmp_path: Path) -> None:
    rec, _, index = _setup(tmp_path)
    p = index.parts
    swapped = [p[1], p[0], *p[2:]]
    junk = hashlib.sha256(b"x").hexdigest()
    corrupt = [
        rec.store.put(
            PartIndexV1.model_validate(index.body() | {"parts": swapped}).encode()
        ),  # reordered
        rec.store.put(
            PartIndexV1.model_validate(index.body() | {"size": index.size - 1}).encode()
        ),  # truncated size
        rec.store.put(
            PartIndexV1.model_validate(index.body() | {"sha256": junk}).encode()
        ),  # stale whole sha
        rec.store.put(index.encode() + b" "),  # non-canonical tail
        rec.store.put(INDEX_MAGIC + b'{"parts":'),  # truncated json
        _bad(rec, index, extra=1),
        _bad(rec, index, parts=p[:-1]),  # count mismatch
        _bad(rec, index, size=index.size + PART_BYTES),
        rec.store.put(INDEX_MAGIC + b" " * MAX_MANIFEST_BYTES),  # oversize
    ]
    for ref in corrupt:
        _assert_rejected(tmp_path, rec, ref, CorruptObjectError)


def test_missing_or_dirty_part_rejected(tmp_path: Path) -> None:
    rec, _, index = _setup(tmp_path)
    path = rec.store._path(index.parts[1])
    ref = hashlib.sha256(index.encode()).hexdigest()
    path.write_bytes(path.read_bytes()[:-1])  # dirty on-disk part
    _assert_rejected(tmp_path, rec, ref, CorruptObjectError)
    path.unlink()
    _assert_rejected(tmp_path, rec, ref, ObjectNotFound)


def test_lying_get_and_put_rejected(tmp_path: Path) -> None:
    rec, data, index = _setup(tmp_path)
    ref = hashlib.sha256(index.encode()).hexdigest()
    shorter = data[: 2 * PART_BYTES]  # misleading store serving another object
    with pytest.raises(CorruptObjectError):
        get_large(ref, lambda sha: shorter if sha == index.parts[1] else rec.store.get(sha))
    with pytest.raises(StoreError):
        put_large(data, lambda b: "0" * 64)


def test_cancel_mid_fetch_leaves_no_temp(tmp_path: Path) -> None:
    rec, _, index = _setup(tmp_path)
    ref = hashlib.sha256(index.encode()).hexdigest()
    calls = 0

    def get(sha: str) -> bytes:
        nonlocal calls
        calls += 1
        if calls == 3:
            raise KeyboardInterrupt
        return rec.store.get(sha)

    out = tmp_path / "out"
    out.mkdir()
    with pytest.raises(KeyboardInterrupt):
        fetch_to(ref, get, out / "dest")
    assert os.listdir(out) == []


def test_bounds() -> None:
    assert (
        len(
            PartIndexV1(
                sha256="0" * 64, size=MAX_PARTS * PART_BYTES, parts=["0" * 64] * MAX_PARTS
            ).encode()
        )
        <= MAX_MANIFEST_BYTES
    )
    with pytest.raises(ValueError):
        PartIndexV1(sha256="0" * 64, size=PART_BYTES, parts=["0" * 64])
    with pytest.raises(ValueError):
        PartIndexV1(
            sha256="0" * 64, size=MAX_PARTS * PART_BYTES + 1, parts=["0" * 64] * (MAX_PARTS + 1)
        )


def test_oversize_input_never_publishes_index(tmp_path: Path) -> None:
    rec = Recorder(tmp_path / "s")
    small = b"\0" * PART_BYTES
    calls = 0

    def chunks() -> bytes:
        nonlocal calls
        calls += 1
        return small if calls <= MAX_PARTS + 1 else b""

    with pytest.raises(StoreError):
        _publish(chunks, rec.put)
    assert all(parse_index(b) is None for b in rec.puts)


def test_parallel_fetch_writes_parts_in_order_despite_out_of_order_completion(
    tmp_path: Path,
) -> None:
    import threading

    # Given: a 9-part object; part 0 is only served once part 1 has been fetched, so a serial
    # fetch would deadlock (bounded by the timeout) and in-order output needs real reordering.
    rec, data, index = _setup(tmp_path, 8 * PART_BYTES + 77)
    ref = hashlib.sha256(rec.puts[-1]).hexdigest()
    second_done = threading.Event()

    def get(sha: str) -> bytes:
        if sha == index.parts[0]:
            assert second_done.wait(timeout=10), "parts were not fetched concurrently"
        body = rec.store.get(sha)
        if sha == index.parts[1]:
            second_done.set()
        return body

    out = tmp_path / "out"
    out.mkdir()
    # When: fetched with 4 workers.
    fetch_to(ref, get, out / "dest", workers=4)
    # Then: byte-identical reassembly.
    assert (out / "dest").read_bytes() == data


def test_parallel_fetch_failing_part_leaves_no_temp_and_stops_queueing(tmp_path: Path) -> None:
    rec, _, index = _setup(tmp_path, 8 * PART_BYTES + 1)
    ref = hashlib.sha256(rec.puts[-1]).hexdigest()
    asked: list[str] = []

    def get(sha: str) -> bytes:
        asked.append(sha)
        if sha == index.parts[0]:
            raise ObjectNotFound(sha)
        return rec.store.get(sha)

    out = tmp_path / "out"
    out.mkdir()
    with pytest.raises(ObjectNotFound):
        fetch_to(ref, get, out / "dest", workers=2)
    # Window is 4 * workers = 8 parts: the 9th is never requested.
    assert os.listdir(out) == [] and index.parts[-1] not in asked


def test_parallel_publish_puts_parts_concurrently_and_index_last(tmp_path: Path) -> None:
    # Given: a 9-part file; part 0's PUT only completes once part 1's PUT has started, so a
    # serial publisher would fail the wait instead of passing.
    data = _blob(8 * PART_BYTES + 77)
    src = tmp_path / "f"
    src.write_bytes(data)
    serial = Recorder(tmp_path / "serial")
    want = put_file(src, serial.put)
    rec, second_started, lock = Recorder(tmp_path / "s"), threading.Event(), threading.Lock()

    def put(chunk: bytes) -> str:
        if chunk == data[:PART_BYTES]:
            assert second_started.wait(timeout=10), "parts were not put concurrently"
        if chunk == data[PART_BYTES : 2 * PART_BYTES]:
            second_started.set()
        with lock:
            return rec.put(chunk)

    # When: published with 4 workers.
    ref = put_file(src, put, workers=4)
    # Then: same ref as serial, index last, and it lists the parts in file order.
    assert ref == want and parse_index(rec.puts[-1]) == parse_index(serial.puts[-1])
    assert all(parse_index(b) is None for b in rec.puts[:-1])
    assert get_large(ref, rec.store.get) == data


def test_parallel_publish_failing_part_never_puts_index(tmp_path: Path) -> None:
    data = _blob(8 * PART_BYTES + 1)
    src = tmp_path / "f"
    src.write_bytes(data)
    rec, lock = Recorder(tmp_path / "s"), threading.Lock()

    def put(chunk: bytes) -> str:
        if chunk == data[3 * PART_BYTES : 4 * PART_BYTES]:
            raise StoreError("boom")
        with lock:
            return rec.put(chunk)

    with pytest.raises(StoreError, match="boom"):
        put_file(src, put, workers=2)
    # Window is 2 * workers = 4 parts in flight: no index, and the tail is never queued.
    assert all(parse_index(b) is None for b in rec.puts)
    assert data[-1:] not in rec.puts
