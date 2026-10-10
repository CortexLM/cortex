"""Manifest geometry and framing must be checked before decoding any tensor."""

import struct

import pytest
import torch

import hypertrain.trainer.compress as codec
from hypertrain.trainer.config import CompressConfig


def framed(kind: str, entries: list[tuple[bytes, tuple[int, ...], bytes]]) -> bytes:
    out = [codec.MAGIC[kind], struct.pack("<I", len(entries))]
    for name, shape, body in entries:
        out += [
            struct.pack("<Q", len(name)),
            name,
            struct.pack("<I", len(shape)),
            b"".join(struct.pack("<Q", dim) for dim in shape),
            struct.pack("<Q", len(body)),
            body,
        ]
    return b"".join(out)


def body(kind: str) -> bytes:
    return (
        struct.pack("<Ifb", 256, 1.0, 1)
        if kind == "dense-int8"
        else struct.pack("<QffIB", 1, 1.0, 1.0, 0, 0)
    )


@pytest.mark.parametrize("kind", list(codec.MAGIC))
def test_legal_roundtrip_and_scalar_vector_geometry(kind: str) -> None:
    # Given: actual encoder, both temperature scalar and genuine one-element vector.
    delta = {
        "scalar": torch.tensor(2.0),
        "vector": torch.tensor([-1.0]),
        "w": torch.arange(12, dtype=torch.float32).reshape(3, 4),
    }
    cfg = CompressConfig(kind, topk_frac=0.25, bits=2 if kind == "sparseloco" else 8)
    payload, ef = codec.compress(cfg, delta, {n: torch.zeros_like(x) for n, x in delta.items()})
    shapes = {n: tuple(x.shape) for n, x in delta.items()}
    # When
    validated = codec.validate_payload(payload, shapes)
    decoded_kind, decoded = codec.decompress(payload, expected_shapes=shapes)
    # Then: encoder bytes and legacy API results remain identical.
    assert validated == decoded_kind == kind
    _, legacy = codec.decompress(payload)
    assert (
        codec.compress(cfg, delta, {n: torch.zeros_like(x) for n, x in delta.items()})[0] == payload
    )
    for name, value in decoded.items():
        assert value.dtype == torch.float32 and tuple(value.shape) == shapes[name]
        assert torch.equal(value, legacy[name])
        assert torch.equal(ef[name], delta[name] - value)


@pytest.mark.parametrize("kind", list(codec.MAGIC))
@pytest.mark.parametrize(
    "attack",
    [
        "huge-dim",
        "zero-dim",
        "scalar-vector",
        "missing",
        "extra",
        "wrong-name",
        "duplicate",
        "unsorted",
        "bad-count",
        "bad-rank",
        "bad-name-length",
        "bad-body-length",
        "truncated",
        "trailing",
        "magic",
        "utf8",
        "bad-scale",
        "bad-topk",
        "bad-index",
        "bad-padding",
        "byte-limit",
    ],
)
def test_hostile_metadata_rejects_before_decode_or_allocation(
    kind: str,
    attack: str,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # Given: a valid first tensor ensures later errors cannot trigger early decode.
    entries = [(b"a", (), body(kind)), (b"b", (1,), body(kind))]
    shapes = {"a": (), "b": (1,)}
    if attack == "huge-dim":
        entries[1] = (b"b", (2**63,), body(kind))
    elif attack == "zero-dim":
        entries[1] = (b"b", (0,), body(kind))
    elif attack == "scalar-vector":
        entries[0] = (b"a", (1,), body(kind))
    elif attack == "missing":
        entries.pop()
    elif attack == "extra":
        entries.append((b"c", (), body(kind)))
    elif attack == "wrong-name":
        entries[1] = (b"c", (1,), body(kind))
    elif attack == "duplicate":
        entries[1] = (b"a", (1,), body(kind))
    elif attack == "unsorted":
        entries.reverse()
    elif attack == "utf8":
        entries[1] = (b"\xff", (1,), body(kind))
    elif attack in ("bad-scale", "bad-topk", "bad-index", "bad-padding"):
        if kind == "dense-int8":
            bad = struct.pack("<Ifb", 256, float("nan"), 1)
        elif attack == "bad-scale":
            bad = struct.pack("<QffIB", 1, float("nan"), 1.0, 0, 0)
        elif attack == "bad-topk":
            bad = struct.pack("<QffIB", 2, 1.0, 1.0, 0, 0)
        elif attack == "bad-index":
            bad = struct.pack("<QffIB", 1, 1.0, 1.0, 1, 0)
        else:
            bad = struct.pack("<QffIB", 1, 1.0, 1.0, 0, 4)
        entries[1] = (b"b", (1,), bad)
    payload = framed(kind, entries)
    prefix = len(codec.MAGIC[kind])
    if attack == "bad-count":
        payload = payload[:prefix] + struct.pack("<I", 2**32 - 1) + payload[prefix + 4 :]
    elif attack == "bad-rank":
        pos = prefix + 4 + 8 + 1
        payload = payload[:pos] + struct.pack("<I", 2**32 - 1) + payload[pos + 4 :]
    elif attack == "bad-name-length":
        pos = prefix + 4
        payload = payload[:pos] + struct.pack("<Q", 2**64 - 1) + payload[pos + 8 :]
    elif attack == "bad-body-length":
        pos = prefix + 4 + 8 + 1 + 4
        payload = payload[:pos] + struct.pack("<Q", 2**64 - 1) + payload[pos + 8 :]
    elif attack == "truncated":
        payload = payload[:-1]
    elif attack == "trailing":
        payload += b"x"
    elif attack == "magic":
        payload = b"X" + payload[1:]
    limit = len(payload) - 1 if attack == "byte-limit" else 65536
    counts = {"decode": 0, "allocate": 0}

    def forbidden_decode(*args: object, **kwargs: object) -> None:
        counts["decode"] += 1
        pytest.fail("metadata rejection reached decoder")

    def forbidden_allocate(*args: object, **kwargs: object) -> None:
        counts["allocate"] += 1
        pytest.fail("metadata rejection allocated tensor")

    for name in ("_int8_decode", "_sparse_decode"):
        monkeypatch.setattr(codec, name, forbidden_decode)
    for name in ("zeros", "empty", "ones", "full", "tensor", "from_numpy"):
        monkeypatch.setattr(torch, name, forbidden_allocate)
    # When / Then: both metadata-only and validated-decode entries reject with zero allocation.
    with pytest.raises(ValueError):
        codec.validate_payload(payload, shapes, max_payload_bytes=limit)
    with pytest.raises(ValueError):
        codec.decompress(payload, expected_shapes=shapes, max_payload_bytes=limit)
    assert counts == {"decode": 0, "allocate": 0}


@pytest.mark.parametrize("kind", list(codec.MAGIC))
def test_exact_raw_byte_bound(kind: str) -> None:
    # Given: valid scalar encoding padded only via its legal UTF-8 parameter name.
    overhead = len(framed(kind, [(b"", (), body(kind))]))
    name = "x" * (65536 - overhead)
    payload = framed(kind, [(name.encode(), (), body(kind))])
    assert len(payload) == 65536
    # When / Then: equality is accepted; one byte smaller bound and one byte excess reject.
    assert codec.validate_payload(payload, {name: ()}) == kind
    with pytest.raises(ValueError, match="byte limit"):
        codec.validate_payload(payload, {name: ()}, max_payload_bytes=65535)
    with pytest.raises(ValueError, match="byte limit"):
        codec.validate_payload(payload + b"x", {name: ()})


@pytest.mark.parametrize(
    "kind,bad",
    [
        ("dense-int8", struct.pack("<Ifb", 255, 1.0, 1)),
        ("dense-int8", struct.pack("<Ifb", 256, 0.0, 1)),
        ("dense-int8", struct.pack("<Ifb", 256, 1.0, -128)),
        ("dense-int8", struct.pack("<If", 256, 1.0)),
        ("sparseloco", struct.pack("<Qff", 0, 1.0, 1.0)),
        ("sparseloco", struct.pack("<QffIB", 1, 2.0, 1.0, 0, 0)),
        ("sparseloco", struct.pack("<QffIIB", 2, 1.0, 1.0, 0, 0, 0)),
        ("sparseloco", struct.pack("<QffIIB", 2, 1.0, 1.0, 1, 0, 0)),
        ("sparseloco", struct.pack("<QffIB", 1, 1.0, 1.0, 0, 0) + b"\x00"),
    ],
)
def test_exact_codec_body_invariants_before_decode(
    kind: str,
    bad: bytes,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # Given: same expected numel, invalid block/scale/code/count/index/length only.
    shape = (1,) if kind == "dense-int8" else (2,)
    payload = framed(kind, [(b"w", shape, bad)])
    calls = []

    def forbidden(*args: object) -> None:
        calls.append("decode")
        pytest.fail("invalid body reached tensor decoder")

    monkeypatch.setattr(codec, "_int8_decode", forbidden)
    monkeypatch.setattr(codec, "_sparse_decode", forbidden)
    # When / Then
    with pytest.raises(ValueError):
        codec.decompress(payload, expected_shapes={"w": shape})
    assert calls == []
