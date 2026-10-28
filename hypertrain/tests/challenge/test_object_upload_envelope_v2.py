"""An object upload is authenticated by a signed ht/2 request in its body, never by a header.

The body is a frame: 4 bytes (big endian) giving the length of the signed `ObjectUploadRequest`
envelope, the envelope, then the object. The envelope binds the uploader, the sha256 and the
length, so the master checks the signature, the pinned size limit and the uploader before it reads
a byte of the object.
"""

# ruff: noqa: F811  (the imported `served` fixture is asked for by name)
from __future__ import annotations

import importlib.util
import json
import socket
import sys
from pathlib import Path

import pytest

from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import Keypair

from .test_event_loop_free_v2 import served  # noqa: F401

spec = importlib.util.spec_from_file_location(
    "object_upload_envelope_fixture", Path(__file__).parent / "test_service_network_v2.py"
)
assert spec is not None and spec.loader is not None
fixtures = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = fixtures
spec.loader.exec_module(fixtures)
network = fixtures.network
HOT = fixtures.HOT
HORIZON = 20  # rounds a request may be signed ahead: the bound documented in network-v2.md
DATA = b"an admission object"
SIGNATURE = "object signature mismatch"
NOT_ADMITTED = "hotkey is not current accepted identity"


def framed(head: bytes, body: bytes) -> bytes:
    return len(head).to_bytes(4, "big") + head + body


def forged(n):
    """Everything right except one hex digit of the signature."""
    frame = n.object_frame(HOT[0], DATA)
    length = int.from_bytes(frame[:4], "big")
    envelope = json.loads(frame[4 : 4 + length])
    envelope["sig"] = ("0" if envelope["sig"][0] != "0" else "1") + envelope["sig"][1:]
    return n.client.post(
        n.url + "/objects/" + sha256_hex(DATA), content=framed(canonicalize(envelope), DATA)
    )


def raw(n, content):
    return n.client.post(n.url + "/objects/" + sha256_hex(DATA), content=content)


def padded(n):
    """A genuine, validly signed request whose envelope is padded with blanks past the bound."""
    frame = n.object_frame(HOT[0], DATA)
    length = int.from_bytes(frame[:4], "big")
    head = frame[4 : 4 + length] + b" " * 4096
    return raw(n, framed(head, frame[4 + length :]))


def cut_short(n):
    """A genuine request whose frame promises ten more bytes of head than it carries."""
    frame = n.object_frame(HOT[0], DATA)
    length = int.from_bytes(frame[:4], "big")
    return raw(n, (length + 10).to_bytes(4, "big") + frame[4 : 4 + length])


REFUSALS = [
    ("forged signature", forged, 403, SIGNATURE),
    ("another run", lambda n: n.put_object(HOT[0], DATA, run_id="ff" * 32), 403, SIGNATURE),
    (
        "another sha than the path",
        lambda n: n.put_object(HOT[0], DATA, path_sha="ab" * 32),
        403,
        SIGNATURE,
    ),
    ("past its expiry", lambda n: n.put_object(HOT[0], DATA, exp=n.now - 1), 403, SIGNATURE),
    (
        "further ahead than the bound",
        lambda n: n.put_object(HOT[0], DATA, exp=n.now + HORIZON + 1),
        403,
        SIGNATURE,
    ),
    ("no frame", lambda n: raw(n, b"xy"), 403, SIGNATURE),
    ("empty body", lambda n: raw(n, b""), 403, SIGNATURE),
    (
        "head longer than the bound",
        lambda n: raw(n, (1 << 20).to_bytes(4, "big") + b"{}"),
        403,
        SIGNATURE,
    ),
    ("head padded past the bound", padded, 403, SIGNATURE),
    ("head cut short", cut_short, 403, SIGNATURE),
    (
        "hotkey not on the subnet",
        lambda n: n.put_object(Keypair(bytes([201]) * 32), DATA),
        403,
        "the hotkey is not registered on the subnet",
    ),
    ("hotkey not admitted", lambda n: n.put_object(HOT[1], DATA), 403, NOT_ADMITTED),
    (
        "size over the pinned limit",
        lambda n: n.put_object(
            HOT[0], DATA, size=n.setup.admission_policy.artifact_limits.max_object_bytes + 1
        ),
        413,
        "object exceeds pinned admission limit",
    ),
    (
        "more bytes than signed",
        lambda n: n.put_object(HOT[0], DATA + b"y", sha=sha256_hex(DATA), size=len(DATA)),
        413,
        "object longer than its signed size",
    ),
    (
        "fewer bytes than signed",
        lambda n: n.put_object(HOT[0], DATA, size=len(DATA) + 5),
        422,
        "object length differs",
    ),
    (
        "other bytes than signed",
        lambda n: n.put_object(HOT[0], b"x" * len(DATA), sha=sha256_hex(DATA)),
        422,
        "object hash differs",
    ),
]


@pytest.mark.parametrize(
    ("name", "call", "status", "detail"), REFUSALS, ids=[case[0] for case in REFUSALS]
)
def test_a_refused_upload_stores_nothing(network, name, call, status, detail):
    network.join()
    network.push(network.now + 5)  # so that "past its expiry" is a real past
    before = network.store._db.total_changes
    seeded = {f for f in network.store.objects.root.rglob("*") if f.is_file()}
    refused = call(network)
    assert (refused.status_code, refused.json()["detail"]) == (status, detail), name
    assert {f for f in network.store.objects.root.rglob("*") if f.is_file()} == seeded
    assert network.store._db.total_changes == before


def test_a_signed_upload_is_stored_and_good_for_the_bound_and_not_one_round_more(network):
    network.join()
    sha = sha256_hex(DATA)
    frame = network.object_frame(HOT[0], DATA, exp=network.now + HORIZON)  # the furthest accepted
    url = network.url + "/objects/" + sha
    stored = network.client.post(url, content=frame)
    assert (stored.status_code, stored.json()) == (200, {"sha256": sha})
    assert network.store.objects.get(sha) == DATA
    # It reserves nothing: the identical bytes are good until they expire.
    assert network.client.post(url, content=frame).status_code == 200
    network.push(network.now + HORIZON)
    assert network.client.post(url, content=frame).status_code == 200
    network.push(network.now + 1)
    assert network.client.post(url, content=frame).status_code == 403


def test_the_old_header_signed_put_is_gone(network):
    network.join()
    sha = sha256_hex(DATA)
    signature = HOT[0].sign(f"hypertrain/object/2|{network.manifest.run_id()}|{sha}".encode())
    response = network.client.put(
        network.url + "/objects/" + sha,
        content=DATA,
        headers={"x-hotkey": HOT[0].ss58, "x-object-signature": signature.hex()},
    )
    assert response.status_code == 405, response.text
    assert not network.store.objects._path(sha).exists()


def test_a_declared_head_over_the_bound_is_refused_before_the_rest_of_the_body(network, served):
    root, _ = served(network.client.app)
    declared = 50 << 20  # the client promises 50 MiB, sends a first chunk and then waits
    head = (
        f"POST {network.url}/objects/{sha256_hex(DATA)} HTTP/1.1\r\nHost: x\r\n"
        f"Content-Length: {declared}\r\n\r\n"
    ).encode()
    with socket.create_connection(("127.0.0.1", int(root.rsplit(":", 1)[1])), timeout=10) as sock:
        sock.sendall(head + (1 << 20).to_bytes(4, "big") + b"{" * 1000)
        assert sock.recv(4096).startswith(b"HTTP/1.1 403")


def test_a_body_longer_than_its_signed_length_is_refused_before_the_rest_is_read(network, served):
    network.join()
    root, _ = served(network.client.app)
    frame = network.object_frame(HOT[0], DATA)
    head = (
        f"POST {network.url}/objects/{sha256_hex(DATA)} HTTP/1.1\r\nHost: x\r\n"
        f"Content-Length: {50 << 20}\r\n\r\n"  # promises 50 MiB, sends 8 KiB and waits
    ).encode()
    with socket.create_connection(("127.0.0.1", int(root.rsplit(":", 1)[1])), timeout=10) as sock:
        sock.sendall(head + frame + b"y" * 8192)
        assert sock.recv(4096).startswith(b"HTTP/1.1 413")
