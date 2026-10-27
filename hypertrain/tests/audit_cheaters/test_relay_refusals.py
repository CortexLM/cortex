"""A cheating miner cannot buy time, bytes or a second slot from the relay.

Every test drives the real relay app (``create_app``) over ASGI with the real signed
envelopes and a local-FS backing; only the beacon is injected. Each refusal is checked for
its reason AND for what it left behind (no custody ack, no pin, no receipt, no stored
chunk), and the honest upload of the same bytes is shown to still succeed.

Already covered over the same layer by ``tests/relay`` and therefore not repeated here:
grant signature/region/epoch/expiry/run/manifest at unit level
(test_transport_v2.py::test_grant_rejected_when_not_authorized), chunk size/hash at unit level
(::test_chunk_rejected_when_size_or_hash_differs), the unsigned chunk PUT without a
capability (::test_unsigned_chunk_http_when_known_grant_hash_is_not_capability), two replicas
accepting the same grant (::test_replicas_when_racing_same_upload_keep_original_receipts) and
the unverified/stale/ahead beacon (test_beacon_cache.py::test_unusable_beacon_is_refused_*).
"""

from __future__ import annotations

import base64
import hashlib
import importlib.util
import sys
from collections.abc import AsyncIterator
from contextlib import asynccontextmanager
from pathlib import Path

import anyio
import httpx
import pytest

from hypertrain.beacon.core import BeaconRound, round_at_or_after
from hypertrain.beacon.drand import QUICKNET_GENESIS, QUICKNET_PERIOD
from hypertrain.data.stream_store import MetadataConflict, StreamStore
from hypertrain.protocol import envelope_v2
from hypertrain.protocol.envelope import SignatureError
from hypertrain.protocol.messages_v2 import RoundOpenV2
from hypertrain.protocol.relay_envelope import RelayEnvelope
from hypertrain.protocol.relay_messages import (
    AcceptedUploadAck,
    ChunkCustodyAck,
    RelayAssignment,
    RelayReceipt,
    UploadChunk,
    UploadChunkManifest,
    UploadGrant,
)
from hypertrain.relay.app import BeaconCache, create_app
from hypertrain.relay.client import receipt_verified

# The relay rig is a test fixture of tests/relay; load it by path, as tests/e2e does for its
# fixtures, because tests/audit_cheaters is not on the import path of tests/relay.
_source = importlib.util.spec_from_file_location(
    "ac_relay_transport_rig", Path(__file__).parents[1] / "relay/test_transport_v2.py"
)
assert _source is not None and _source.loader is not None
_rig = importlib.util.module_from_spec(_source)
sys.modules[_source.name] = _rig
_source.loader.exec_module(_rig)
Rig, MASTER, MINER, RELAY, NEXT, RUN, H = (
    _rig.Rig,
    _rig.MASTER,
    _rig.MINER,
    _rig.RELAY,
    _rig.NEXT,
    _rig.RUN,
    _rig.H,
)

DEADLINE = 35  # RoundOpenV2.d_upload of Rig.upload; the grant itself expires at 40
OTHER_NONCE = "56" * 32
BOUND = "grant is not bound to this region/assignment/round/size"
CHUNK = 4 << 20


class Upload:
    """One signed upload as the miner would hold it: grant, manifest, assignment, round."""

    def __init__(self, rig: Rig, data: bytes, *, w: int = 1, nonce: str = H) -> None:
        self.data = data
        self.grant, self.manifest, self.raw, self.assignment, self.opened = rig.upload(
            data, nonce=nonce, w=w
        )
        self.hash = self.grant.digest()
        self.capability = base64.b64encode(self.raw.model_dump_json().encode()).decode()

    def accept_body(self, **override: object) -> dict[str, object]:
        body: dict[str, object] = {
            "grant": self.raw.model_dump(mode="json"),
            "manifest": self.manifest.body(),
            "assignment": self.assignment.model_dump(mode="json"),
            "round_open": self.opened.model_dump(mode="json"),
        }
        return {**body, **override}

    def chunk_sha(self, index: int = 0) -> str:
        return self.manifest.chunks[index].chunk_sha256


@asynccontextmanager
async def serving(
    path: Path, *, peers: int = 0
) -> AsyncIterator[tuple[Rig, httpx.AsyncClient, list[httpx.AsyncClient]]]:
    """The rig, its relay app over ASGI, and one more app per extra replica on shared storage."""
    async with httpx.AsyncClient() as backing:
        rig = Rig(path, backing)
        replicas = [rig.new_relay() for _ in range(peers)]
        clients: list[httpx.AsyncClient] = []
        try:
            for relay in [rig.relay, *replicas]:
                clients.append(
                    httpx.AsyncClient(
                        transport=httpx.ASGITransport(
                            app=create_app(relay, "d" * 32), raise_app_exceptions=False
                        ),
                        base_url="https://relay.test",
                    )
                )
            yield rig, clients[0], clients[1:]
        finally:
            for client in clients:
                await client.aclose()


async def accept(wire: httpx.AsyncClient, upload: Upload, **override: object) -> httpx.Response:
    return await wire.post(f"/v1/uploads/{upload.hash}/accept", json=upload.accept_body(**override))


async def put(
    wire: httpx.AsyncClient,
    upload: Upload,
    data: bytes | AsyncIterator[bytes],
    index: int = 0,
    *,
    capability: str | None = None,
) -> httpx.Response:
    return await wire.put(
        f"/v1/uploads/{upload.hash}/chunks/{index}",
        content=data,
        headers={"X-Upload-Grant": capability or upload.capability},
    )


class Overlap:
    """Makes the first journal writes of concurrent replicas genuinely overlap.

    On one event loop the replicas would otherwise run one after the other, never contend on the
    journal's compare-and-swap, and a store that ignored the etag would pass a race test. Each of
    the first `callers` writes waits until all of them have read the journal, so exactly one wins
    the swap and the others retry against the winner's state. `conflicts` counts the swaps lost.
    """

    def __init__(self, store: StreamStore, callers: int = 2) -> None:
        self.swap_of = store.compare_swap
        self.callers, self.arrived, self.conflicts = callers, 0, 0
        self.ready = anyio.Event()

    async def compare_swap(self, name: str, data: bytes, etag: str | None) -> None:
        if self.arrived < self.callers:
            self.arrived += 1
            if self.arrived == self.callers:
                self.ready.set()
            with anyio.fail_after(10):
                await self.ready.wait()
        try:
            await self.swap_of(name, data, etag)
        except MetadataConflict:
            self.conflicts += 1
            raise


def overlapping(rig: Rig, monkeypatch: pytest.MonkeyPatch) -> Overlap:
    overlap = Overlap(rig.store)
    monkeypatch.setattr(rig.store, "compare_swap", overlap.compare_swap)
    return overlap


def stored(rig: Rig, sha: str) -> bool:
    return rig.store.store._path(sha).exists()


async def assert_no_custody(rig: Rig, upload: Upload) -> None:
    """Nothing the relay would have to honour later exists for this grant."""
    state = await rig.relay.state()
    record = state.uploads.get(upload.hash)
    if record is not None:
        assert not record.chunks and not record.pins and record.receipt is None
    assert not state.publications
    assert rig.relay.inflight == 0


async def assert_honest_upload_succeeds(
    rig: Rig, wire: httpx.AsyncClient, upload: Upload, *, accepted: bool = False
) -> None:
    """The same bytes, sent honestly, are accepted and end in one signed receipt."""
    if not accepted:
        assert (await accept(wire, upload)).status_code == 200
    for index, chunk in enumerate(upload.manifest.chunks):
        start = chunk.off
        sent = await put(wire, upload, upload.data[start : start + chunk.len], index)
        assert sent.status_code == 200
    record = (await rig.relay.state()).uploads[upload.hash]
    assert record.receipt is not None
    receipt = RelayReceipt.model_validate(record.receipt.body)
    assert receipt.delta_hash == hashlib.sha256(upload.data).hexdigest()
    assert stored(rig, receipt.delta_hash)


def reason(response: httpx.Response) -> object:
    return response.json().get("detail", response.json().get("error"))


def test_late_chunk_is_refused_before_a_byte_is_stored(tmp_path: Path) -> None:
    """A chunk one round after the upload deadline gets no custody ack; at the deadline it does."""

    async def scenario() -> None:
        async with serving(tmp_path) as (rig, wire, _):
            punctual = Upload(rig, b"a" * 40, w=1)
            late = Upload(rig, b"b" * 40, w=2, nonce=OTHER_NONCE)
            for upload in (punctual, late):
                opportunity = await accept(wire, upload)
                assert opportunity.status_code == 200
                assert opportunity.json()["body"]["service_deadline"] == DEADLINE

            rig.clock = DEADLINE
            assert (await put(wire, punctual, punctual.data)).status_code == 200
            assert (await rig.relay.state()).uploads[punctual.hash].receipt is not None

            rig.clock = DEADLINE + 1
            refused = await put(wire, late, late.data)
            assert refused.status_code == 400
            assert reason(refused) == "chunk index/deadline invalid"
            await assert_no_custody(rig, late)
            assert not stored(rig, late.chunk_sha())

    anyio.run(scenario)


def test_chunk_that_lands_after_the_deadline_gets_no_custody_ack(tmp_path: Path) -> None:
    """Starting before the deadline is not enough: custody is signed only for timely persistence."""

    async def scenario() -> None:
        async with serving(tmp_path) as (rig, wire, _):
            slow = Upload(rig, b"s" * 40)
            assert (await accept(wire, slow)).status_code == 200
            rig.clock = DEADLINE

            async def crossing_the_deadline() -> AsyncIterator[bytes]:
                yield slow.data
                rig.clock = DEADLINE + 1

            refused = await put(wire, slow, crossing_the_deadline())
            assert refused.status_code == 400
            assert reason(refused) == "chunk persisted after agreed deadline; no timely custody"
            await assert_no_custody(rig, slow)
            assert (await rig.relay.state()).uploads[slow.hash].receipt is None

    anyio.run(scenario)


def test_grant_is_accepted_at_the_deadline_round_and_refused_one_round_later(
    tmp_path: Path,
) -> None:
    """The relay opens no upload window once the deadline round has passed."""

    async def scenario() -> None:
        async with serving(tmp_path) as (rig, wire, _):
            on_time = Upload(rig, b"a" * 40, w=1)
            too_late = Upload(rig, b"b" * 40, w=2, nonce=OTHER_NONCE)

            rig.clock = DEADLINE
            granted = await accept(wire, on_time)
            assert granted.status_code == 200
            assert AcceptedUploadAck.model_validate(granted.json()["body"]).service_deadline == (
                DEADLINE
            )

            rig.clock = DEADLINE + 1
            refused = await accept(wire, too_late)
            assert refused.status_code == 400
            assert reason(refused) == "upload deadline passed"
            assert list((await rig.relay.state()).uploads) == [on_time.hash]
            assert (await put(wire, too_late, too_late.data)).status_code == 400
            await assert_no_custody(rig, too_late)

    anyio.run(scenario)


def _forged_inputs(rig: Rig, upload: Upload) -> dict[str, dict[str, object]]:
    """Each way a miner can present a master-signed grant with the wrong surroundings."""
    other_round = Upload(rig, upload.data, w=2)
    assignment = RelayAssignment.model_validate(upload.assignment.body)

    def assignment_with(**change: object) -> dict[str, object]:
        sealed = rig.seal(RelayAssignment.model_validate({**assignment.body(), **change}))
        return {"assignment": sealed.model_dump(mode="json")}

    extended = upload.opened.model_dump(mode="json")
    extended["body"] = {**extended["body"], "d_upload": 1000}
    own_round = envelope_v2.parse_envelope(
        envelope_v2.seal(
            MINER,
            "RoundOpenV2",
            RUN,
            RoundOpenV2.model_validate(upload.opened.body),
            1000,
        )
    )
    other_registry = envelope_v2.parse_envelope(
        envelope_v2.seal(
            MASTER,
            "RoundOpenV2",
            RUN,
            RoundOpenV2.model_validate({**upload.opened.body, "registry_epoch": 1}),
            50,
        )
    )
    return {
        "round-open-of-another-round": {"round_open": other_round.opened.model_dump(mode="json")},
        "round-open-of-another-registry-epoch": {
            "round_open": other_registry.model_dump(mode="json")
        },
        "assignment-of-another-miner": assignment_with(hotkey=NEXT.ss58),
        "assignment-of-another-epoch": assignment_with(assignment_epoch=1),
        "assignment-of-another-round": {
            "assignment": other_round.assignment.model_dump(mode="json")
        },
        "assignment-to-another-relay": assignment_with(primary_id="us-1"),
        "assignment-of-another-network": assignment_with(manifest_hash="34" * 32),
        "assignment-signed-by-the-miner": {
            "assignment": rig.seal(assignment, MINER).model_dump(mode="json")
        },
        "round-open-signed-by-the-miner": {"round_open": own_round.model_dump(mode="json")},
        "round-open-with-extended-deadline": {"round_open": extended},
    }


BOUND_REFUSALS = {
    "round-open-of-another-round": BOUND,
    "round-open-of-another-registry-epoch": BOUND,
    "assignment-of-another-miner": BOUND,
    "assignment-of-another-epoch": BOUND,
    "assignment-of-another-round": BOUND,
    "assignment-to-another-relay": BOUND,
    "assignment-of-another-network": BOUND,
    "assignment-signed-by-the-miner": "SignatureError",
    "round-open-signed-by-the-miner": "SignatureError",
    "round-open-with-extended-deadline": "SignatureError",
}


@pytest.mark.parametrize("fault", sorted(BOUND_REFUSALS))
def test_grant_is_refused_with_another_rounds_open_or_a_foreign_assignment(
    tmp_path: Path, fault: str
) -> None:
    """A grant only opens an upload with its own signed round, assignment and relay."""

    async def scenario() -> None:
        async with serving(tmp_path) as (rig, wire, _):
            upload = Upload(rig, b"a" * 40)
            refused = await accept(wire, upload, **_forged_inputs(rig, upload)[fault])
            assert refused.status_code == 400
            assert reason(refused) == BOUND_REFUSALS[fault]
            assert not (await rig.relay.state()).uploads
            await assert_honest_upload_succeeds(rig, wire, upload)

    anyio.run(scenario)


async def accept_with_swapped_manifest(wire: httpx.AsyncClient, upload: Upload) -> httpx.Response:
    """The chunk list names another hash than the one the master signed into the grant."""
    other = hashlib.sha256(b"x").hexdigest()
    swapped = UploadChunkManifest(
        size=upload.manifest.size,
        chunks=[UploadChunk(index=0, off=0, len=upload.manifest.size, chunk_sha256=other)],
    )
    return await accept(wire, upload, manifest=swapped.body())


def test_grant_with_a_chunk_manifest_that_is_not_the_signed_one_opens_nothing(
    tmp_path: Path,
) -> None:
    """A manifest naming other chunk hashes than the grant commits to is never accepted.

    Holds whatever the status of the refusal (today an unhandled 500, see the xfail below): the
    no-custody guarantee does not depend on how the refusal is worded.
    """

    async def scenario() -> None:
        async with serving(tmp_path) as (rig, wire, _):
            upload = Upload(rig, b"a" * 40)
            refused = await accept_with_swapped_manifest(wire, upload)
            assert not refused.is_success
            assert not (await rig.relay.state()).uploads
            await assert_honest_upload_succeeds(rig, wire, upload)

    anyio.run(scenario)


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="RELAY-01 (MINEUR): UploadChunkManifest.validate_grant (relay_messages.py) raises a "
    "bare ValueError that create_app has no handler for, so the refusal is an unhandled 500 with "
    "no reason; nothing is stored and the relay stays live (the test above), and an honest miner "
    "never sends it because relay/client.py runs the same check locally",
)
def test_grant_with_a_swapped_chunk_manifest_is_refused_with_a_reason(tmp_path: Path) -> None:
    """Every refusal of a grant is a 400 that names its reason, as the other grant checks are."""

    async def scenario() -> None:
        async with serving(tmp_path) as (rig, wire, _):
            refused = await accept_with_swapped_manifest(wire, Upload(rig, b"a" * 40))
            assert refused.status_code == 400
            assert reason(refused) == "chunk list differs from signed grant"

    anyio.run(scenario)


def test_second_grant_for_the_same_round_and_miner_is_refused(tmp_path: Path) -> None:
    """One miner holds one reservation per round: a second grant cannot open a second slot."""

    async def scenario() -> None:
        async with serving(tmp_path) as (rig, wire, _):
            first = Upload(rig, b"abc", w=1)
            second = Upload(rig, b"abd", w=1, nonce=OTHER_NONCE)
            granted = await accept(wire, first)
            assert granted.status_code == 200

            refused = await accept(wire, second)
            assert refused.status_code == 400
            assert reason(refused) == "conflicting run/round/miner reservation"
            assert list((await rig.relay.state()).uploads) == [first.hash]
            assert (await put(wire, second, second.data)).status_code == 400
            await assert_no_custody(rig, second)
            assert not stored(rig, second.chunk_sha())

            repeated = await accept(wire, first)
            assert repeated.status_code == 200 and repeated.json() == granted.json()
            await assert_honest_upload_succeeds(rig, wire, first, accepted=True)

    anyio.run(scenario)


def test_grant_larger_than_the_relay_object_limit_is_refused(tmp_path: Path) -> None:
    """The relay never reserves more than its signed max_object_bytes for one grant."""

    async def scenario() -> None:
        async with serving(tmp_path) as (rig, wire, _):
            limit = rig.registry.specs[0].max_object_bytes
            for size, w, status in ((limit + 1, 2, 400), (limit, 1, 200)):
                upload = Upload(rig, b"a", w=w)
                chunks = [
                    UploadChunk(
                        index=i,
                        off=i * CHUNK,
                        len=min(CHUNK, size - i * CHUNK),
                        chunk_sha256=H,
                    )
                    for i in range(-(-size // CHUNK))
                ]
                upload.manifest = UploadChunkManifest(size=size, chunks=chunks)
                upload.grant = UploadGrant.model_validate(
                    {
                        **upload.grant.body(),
                        "size": size,
                        "chunk_manifest_hash": upload.manifest.chunk_manifest_hash(),
                    }
                )
                upload.raw, upload.hash = rig.seal(upload.grant), upload.grant.digest()
                answer = await accept(wire, upload, grant=upload.raw.model_dump(mode="json"))
                assert answer.status_code == status
                if status == 400:
                    assert reason(answer) == BOUND
                    assert upload.hash not in (await rig.relay.state()).uploads
            assert len((await rig.relay.state()).uploads) == 1

    anyio.run(scenario)


WRONG_BYTES = {
    "truncated": (0, b"ab"),
    "extended": (0, b"abcd"),
    "empty": (0, b""),
    "same-size-wrong": (0, b"xyz"),
}


@pytest.mark.parametrize("fault", sorted(WRONG_BYTES))
def test_chunk_bytes_that_are_not_the_granted_ones_get_no_custody_ack(
    tmp_path: Path, fault: str
) -> None:
    """Bytes other than the manifest chunk are refused: nothing signed, pinned or stored."""

    async def scenario() -> None:
        async with serving(tmp_path) as (rig, wire, _):
            upload = Upload(rig, b"abc")
            assert (await accept(wire, upload)).status_code == 200
            index, payload = WRONG_BYTES[fault]
            refused = await put(wire, upload, payload, index)
            assert not refused.is_success
            assert reason(refused) == "CorruptObjectError"
            await assert_no_custody(rig, upload)
            assert not stored(rig, upload.chunk_sha())
            assert not stored(rig, hashlib.sha256(payload).hexdigest())
            await assert_honest_upload_succeeds(rig, wire, upload, accepted=True)

    anyio.run(scenario)


@pytest.mark.parametrize("index", [-1, 1])
def test_chunk_outside_the_granted_manifest_is_refused(tmp_path: Path, index: int) -> None:
    """A chunk index the manifest does not list cannot be filled, whatever the bytes."""

    async def scenario() -> None:
        async with serving(tmp_path) as (rig, wire, _):
            upload = Upload(rig, b"abc")
            assert (await accept(wire, upload)).status_code == 200
            refused = await put(wire, upload, upload.data, index)
            assert refused.status_code == 400
            assert reason(refused) == "chunk index invalid"
            await assert_no_custody(rig, upload)
            assert not stored(rig, upload.chunk_sha())
            await assert_honest_upload_succeeds(rig, wire, upload, accepted=True)

    anyio.run(scenario)


def test_duplicate_chunk_put_is_idempotent_and_cannot_replace_the_first_custody(
    tmp_path: Path,
) -> None:
    """A repeated chunk returns the original ack, adds no pin, and other bytes cannot replace it."""

    async def scenario() -> None:
        async with serving(tmp_path) as (rig, wire, _):
            upload = Upload(rig, b"a" * CHUNK + b"suffix")
            assert (await accept(wire, upload)).status_code == 200
            first = await put(wire, upload, upload.data[:CHUNK], 0)
            again = await put(wire, upload, upload.data[:CHUNK], 0)
            assert (first.status_code, again.status_code) == (200, 200)
            assert again.json() == first.json()

            swap = await put(wire, upload, b"z" * CHUNK, 0)
            assert not swap.is_success and reason(swap) == "CorruptObjectError"
            record = (await rig.relay.state()).uploads[upload.hash]
            assert len(record.chunks) == 1 and len(record.pins) == 1
            assert record.receipt is None
            assert ChunkCustodyAck.model_validate(first.json()["body"]).chunk_sha256 == (
                upload.chunk_sha(0)
            )

            await assert_honest_upload_succeeds(rig, wire, upload, accepted=True)
            record = (await rig.relay.state()).uploads[upload.hash]
            assert len(record.pins) == 3  # two chunks and one receipt
            replay = await put(wire, upload, upload.data[:CHUNK], 0)
            assert replay.json() == first.json()
            assert (await rig.relay.state()).uploads[upload.hash].receipt == record.receipt
            assert len((await rig.relay.state()).uploads[upload.hash].pins) == 3

    anyio.run(scenario)


def _capability_forgeries(rig: Rig, upload: Upload, other: Upload) -> dict[str, str]:
    def encoded(raw: RelayEnvelope) -> str:
        return base64.b64encode(raw.model_dump_json().encode()).decode()

    return {
        "signed-by-the-miner": encoded(rig.seal(upload.grant, MINER)),
        "capability-of-another-grant": other.capability,
        "not-base64": "not base64!",
    }


CAPABILITY_REFUSALS = {
    "signed-by-the-miner": "SignatureError",
    "capability-of-another-grant": "upload capability path mismatch",
    "not-base64": "malformed signed upload capability",
}


@pytest.mark.parametrize("fault", sorted(CAPABILITY_REFUSALS))
def test_chunk_put_needs_the_master_signed_grant_of_its_own_path(
    tmp_path: Path, fault: str
) -> None:
    """A known grant hash is no capability: the PUT must carry the master-signed grant itself."""

    async def scenario() -> None:
        async with serving(tmp_path) as (rig, wire, _):
            upload = Upload(rig, b"abc", w=1)
            other = Upload(rig, b"abd", w=2, nonce=OTHER_NONCE)
            for each in (upload, other):
                assert (await accept(wire, each)).status_code == 200
            forged = _capability_forgeries(rig, upload, other)[fault]
            refused = await put(wire, upload, upload.data, capability=forged)
            assert refused.status_code == 400
            assert reason(refused) == CAPABILITY_REFUSALS[fault]
            await assert_no_custody(rig, upload)
            await assert_no_custody(rig, other)
            assert not stored(rig, upload.chunk_sha())
            await assert_honest_upload_succeeds(rig, wire, upload, accepted=True)

    anyio.run(scenario)


def test_replicas_racing_conflicting_grants_reserve_exactly_one(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Two replicas on shared storage cannot both grant the same miner and round, even when both
    read the journal before either writes (one loses the compare-and-swap and retries)."""

    async def scenario() -> None:
        async with serving(tmp_path, peers=1) as (rig, first_wire, (second_wire,)):
            overlap = overlapping(rig, monkeypatch)
            first = Upload(rig, b"abc")
            second = Upload(rig, b"abd", nonce=OTHER_NONCE)
            answers: dict[str, httpx.Response] = {}

            async def race(name: str, wire: httpx.AsyncClient, upload: Upload) -> None:
                answers[name] = await accept(wire, upload)

            async with anyio.create_task_group() as tasks:
                tasks.start_soon(race, "first", first_wire, first)
                tasks.start_soon(race, "second", second_wire, second)

            assert overlap.conflicts == 1
            assert sorted(a.status_code for a in answers.values()) == [200, 400]
            winner, loser = (
                (first, second) if answers["first"].status_code == 200 else (second, first)
            )
            loss = answers["second" if winner is first else "first"]
            assert reason(loss) == "conflicting run/round/miner reservation"
            assert list((await rig.relay.state()).uploads) == [winner.hash]
            await assert_no_custody(rig, loser)
            await assert_honest_upload_succeeds(rig, second_wire, winner, accepted=True)

    anyio.run(scenario)


def test_replicas_racing_the_same_chunk_keep_one_custody_ack(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """The same chunk sent to two replicas at once yields one signed ack and one pin, even when
    both read the journal before either writes."""

    async def scenario() -> None:
        async with serving(tmp_path, peers=1) as (rig, first_wire, (second_wire,)):
            upload = Upload(rig, b"a" * CHUNK + b"suffix")
            assert (await accept(first_wire, upload)).status_code == 200
            overlap = overlapping(rig, monkeypatch)
            answers: list[httpx.Response] = []

            async def race(wire: httpx.AsyncClient) -> None:
                answers.append(await put(wire, upload, upload.data[:CHUNK]))

            async with anyio.create_task_group() as tasks:
                tasks.start_soon(race, first_wire)
                tasks.start_soon(race, second_wire)

            assert overlap.conflicts >= 1
            assert [a.status_code for a in answers] == [200, 200]
            assert answers[0].json() == answers[1].json()
            record = (await rig.relay.state()).uploads[upload.hash]
            assert list(record.chunks) == ["0"] and len(record.pins) == 1
            assert not (await rig.relay.state()).publications

    anyio.run(scenario)


@pytest.mark.parametrize("fault", ["tampered-body", "receipt-of-another-upload", "after-deadline"])
def test_receipt_that_the_relay_did_not_issue_for_this_grant_is_refused(
    tmp_path: Path, fault: str
) -> None:
    """A receipt must be the relay's own, for this exact grant, issued before the deadline.

    A receipt re-signed by the miner is already refused by test_transport_v2.py
    (test_forged_receipt_when_master_verifies_durable_claim).
    """

    async def scenario() -> None:
        async with serving(tmp_path) as (rig, wire, _):
            mine = Upload(rig, b"abc", w=1)
            theirs = Upload(rig, b"abd", w=2, nonce=OTHER_NONCE)
            for each in (mine, theirs):
                await assert_honest_upload_succeeds(rig, wire, each)
            records = (await rig.relay.state()).uploads
            receipt = records[mine.hash].receipt
            assert receipt is not None
            issued = RelayReceipt.model_validate(receipt.body)
            presented, before = receipt, DEADLINE
            if fault == "tampered-body":
                presented = receipt.model_copy(
                    update={"body": {**receipt.body, "received_drand": 1}}
                )
            elif fault == "receipt-of-another-upload":
                presented = records[theirs.hash].receipt
                assert presented is not None
            else:
                before = issued.received_drand - 1

            def verify(raw: RelayEnvelope, received_before: int) -> RelayReceipt:
                return receipt_verified(
                    raw,
                    registry=rig.registry,
                    relay_id="eu-1",
                    run_id=RUN,
                    grant=mine.grant,
                    received_before=received_before,
                )

            assert verify(receipt, DEADLINE) == issued
            with pytest.raises(SignatureError, match="forged/mismatched durability receipt"):
                verify(presented, before)

    anyio.run(scenario)


class DrandBehind:
    """Wall clock and a verified drand that reports the round `lag` rounds before the true one."""

    def __init__(self) -> None:
        self.wall, self.mono, self.lag = 0.0, 5000.0, 0

    def true_round(self) -> int:
        return round_at_or_after(int(self.wall), QUICKNET_GENESIS, QUICKNET_PERIOD) - 1

    def at_round(self, number: int, lag: int) -> None:
        """One second into round `number`, and past the cache's one-period hold."""
        self.wall = QUICKNET_GENESIS + QUICKNET_PERIOD * (number - 1) + 1.0
        self.mono += 2 * QUICKNET_PERIOD
        self.lag = lag
        assert self.true_round() == number

    def latest(self) -> BeaconRound:
        return BeaconRound(self.true_round() - self.lag, "00" * 48, "11" * 32, bls_verified=True)


def test_relay_accepts_a_chunk_at_most_two_rounds_after_the_deadline(tmp_path: Path) -> None:
    """The relay clock may lag by two rounds, so a chunk can land that late; never later.

    This is the by-design residual of the beacon cache: at three rounds of lag the relay
    refuses with its stale-beacon reason instead of reading an older round as current. The
    master's own bound (store.py _fresh_v2, `now < emitted - 2`) is covered by
    tests/challenge/test_capacity_outer_v2.py, not here.
    """

    async def scenario() -> None:
        async with serving(tmp_path) as (rig, wire, _):
            drand = DrandBehind()
            cache = BeaconCache(drand.latest, lambda: drand.wall, lambda: drand.mono)
            rig.relay.now = cache.now
            uploads = [
                Upload(rig, bytes([97 + i]) * 40, w=i + 1, nonce=f"{i + 1:02x}" * 32)
                for i in range(3)
            ]
            drand.at_round(20, lag=0)
            for each in uploads:
                assert (await accept(wire, each)).status_code == 200

            drand.at_round(DEADLINE + 2, lag=2)
            assert (await put(wire, uploads[0], uploads[0].data)).status_code == 200
            receipt = (await rig.relay.state()).uploads[uploads[0].hash].receipt
            assert receipt is not None
            assert RelayReceipt.model_validate(receipt.body).received_drand == DEADLINE

            drand.at_round(DEADLINE + 3, lag=2)
            past = await put(wire, uploads[1], uploads[1].data)
            assert (past.status_code, reason(past)) == (400, "chunk index/deadline invalid")

            drand.at_round(DEADLINE + 2, lag=3)
            frozen = await put(wire, uploads[2], uploads[2].data)
            assert (frozen.status_code, reason(frozen)) == (
                400,
                "verified beacon stale; eligibility/deadlines frozen",
            )
            for refused in uploads[1:]:
                await assert_no_custody(rig, refused)
                assert not stored(rig, refused.chunk_sha())

    anyio.run(scenario)
