"""Authentication of the HTTP routes, part 2: header signatures, GET side effects, liveness.

Package http_auth of the ledger-invariants lot. The tests marked xfail state a defence the
service does not hold on this tree; each carries a control that reaches the point under test.
"""

from __future__ import annotations

import asyncio
import base64
import hashlib
import threading
from pathlib import Path
from typing import Any

import httpx
import pytest
from fastapi.testclient import TestClient

from hypertrain.data.store import ObjectNotFound
from hypertrain.data.stream_store import spool
from hypertrain.protocol import envelope_v2, relay_envelope
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages import Receipt
from hypertrain.protocol.messages_v2 import WorkProof
from hypertrain.protocol.relay_messages import (
    ChunkCustodyAck,
    CustodyRelease,
    UploadGrant,
)
from hypertrain.relay import app as relay_app
from hypertrain.relay.app import create_app as create_relay_app
from hypertrain.relay.core import RelayError, Settlement

from .support import load_fixture_module, service_world
from .support_gaps_http import expect
from .support_http_auth import (
    HASH,
    LockSpy,
    changed,
    detail,
    durable_state,
    log_record_writes,
    open_round,
    put_object,
    quiet_client,
    seed_relay_scenario,
    sibling_runtime,
    sibling_service,
    watch_capture,
)

RELAY_FIXTURE = ("li_relay_fixture", "relay/test_transport_v2.py")


# SG-05 ------------------------------------------------------------------------------------


# Closed by audit-integrity-2 ADR0010-P3 (b018d425, the shape of 5bd7537f): the poll is signed in
# its body with an expiry.
def test_a_dispute_watch_captured_in_an_earlier_round_is_refused_later(tmp_path: Path) -> None:
    party = sibling_service().HOT[0]
    with service_world(tmp_path) as world:
        n = world.network
        replay = watch_capture(n, party)
        # Control: in the round it was made the capture lets the party watch.
        watch = detail(replay())
        expect(watch == (200, []), f"the capture does not open the watch in its own round: {watch}")
        n.push(300)
        assert replay().status_code == 403, "the same capture still opens the watch 299 rounds on"


# SG-13 ------------------------------------------------------------------------------------


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason=(
        "SG-13+HTP-10: the beacon pin table does not grow by a row for every round that an "
        "anonymous status read touches [fix-lot: none (no design yet)]"
    ),
)
def test_pin_rows_do_not_pile_up_one_per_round_of_anonymous_status_reads(tmp_path: Path) -> None:
    service = sibling_service()
    rounds = 120
    with service_world(tmp_path) as world:
        n = world.network
        expect(n.join(0).status_code == 200, "the miner did not join")
        client = quiet_client(world.app)
        status = f"/v2/runs/{world.run_id}/admission/{service.HOT[0].ss58}"
        hosted = world.store.for_run(world.run_id)

        def pins() -> int:
            with hosted._lock:
                return int(hosted._db.execute("SELECT COUNT(*) FROM beacon_pins_v2").fetchone()[0])

        def read_each_round(first: int) -> None:
            for number in range(first, first + rounds):
                n.push(number)
                expect(client.get(status).status_code == 200, "the status read is not served")

        read_each_round(2)
        middle = pins()
        read_each_round(2 + rounds)
        # A pruning or a retention window flattens the second phase; a table that keeps
        # every pin adds one row per round, exactly as the first phase did.
        assert pins() - middle < rounds, f"{pins() - middle} pin rows added in {rounds} rounds"


# GT-04 ------------------------------------------------------------------------------------


def observer_challenge(client: TestClient, url: str) -> dict[str, Any]:
    response = client.get(url)
    expect(response.status_code == 200, f"the observer challenge is not issued: {response.text}")
    body: dict[str, Any] = response.json()
    return body


def observer_answer(world: Any, scenario: Any, challenge: dict[str, Any]) -> dict[str, Any]:
    """The pinned observer's signed response to `challenge`."""
    return envelope_v2.seal(
        scenario.observer, "Receipt", world.run_id, Receipt.model_validate(challenge["body"]), 9999
    )


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason=(
        "GT-04+HTP-07: an anonymous second GET of a relay-observer challenge does not void the "
        "pending one [fix-lot: escrow-relay-fixes ADR0014-P8]"
    ),
)
def test_a_second_anonymous_observer_challenge_does_not_void_the_pending_one(
    tmp_path: Path,
) -> None:
    with service_world(tmp_path) as world:
        scenario = seed_relay_scenario(world)
        client = quiet_client(world.app)
        url = world.network.url + f"/relay-observers/{scenario.grant_hash}/{scenario.observer.ss58}"
        # Control: one challenge, answered by the observer, is accepted.
        first = observer_challenge(client, url)
        accepted = client.post(url, json=observer_answer(world, scenario, first))
        expect(accepted.status_code == 200, f"the observer's answer is refused: {accepted.text}")
        # A stranger who knows the grant and the observer asks again before the observer answers;
        # what that second ask is told does not matter, only that the pending challenge survives.
        pending = observer_challenge(client, url)
        client.get(url)
        answer = client.post(url, json=observer_answer(world, scenario, pending))
        assert answer.status_code == 200, detail(answer)


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason=(
        "GT-04+HTP-07: a relay-observer challenge asked for in a later round is never a 500 "
        "[fix-lot: escrow-relay-fixes ADR0014-P8]"
    ),
)
def test_an_observer_challenge_asked_for_in_a_later_round_is_a_mapped_answer(
    tmp_path: Path,
) -> None:
    with service_world(tmp_path) as world:
        scenario = seed_relay_scenario(world)
        client = quiet_client(world.app)
        url = world.network.url + f"/relay-observers/{scenario.grant_hash}/{scenario.observer.ss58}"
        issued = client.get(url).status_code
        expect(issued == 200, f"the first round does not issue a challenge: {issued}")
        world.network.push(5)
        later = client.get(url)
        assert later.status_code != 500, "an unmapped error answers the ask of the next round"


# GT-05 ------------------------------------------------------------------------------------


def test_repeated_anonymous_reads_of_a_round_leave_the_stored_state_as_the_first_left_it(
    tmp_path: Path,
) -> None:
    """GT-05. Round view, island job and relay assignment are derived once and then only read."""
    with service_world(tmp_path) as world:
        hotkeys = open_round(world)
        run, miner = world.run_id, hotkeys[0].ss58
        client = quiet_client(world.app)
        urls = [
            f"/v2/runs/{run}/rounds/0",
            f"/v2/runs/{run}/rounds/0/job/{miner}",
            f"/v2/runs/{run}/rounds/0/relay/{miner}",
        ]
        first = {url: client.get(url) for url in urls}
        assert {url: r.status_code for url, r in first.items()} == dict.fromkeys(urls, 200)
        settled = durable_state(world)
        for url in urls:
            repeated = [client.get(url) for _ in range(3)]
            assert {r.status_code for r in repeated} == {200}
            assert len({r.content for r in repeated}) == 1
        assert changed(settled, durable_state(world)) == set()
        # The job and assignment answers are the stored ones, whichever call asks.
        assert client.get(urls[1]).content == first[urls[1]].content


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason=(
        "GT-05+HTP-02: a repeated anonymous read of the round view rewrites no stored record "
        "[fix-lot: none (no design yet)]"
    ),
)
def test_a_repeated_anonymous_round_view_rewrites_no_stored_record(tmp_path: Path) -> None:
    with service_world(tmp_path) as world:
        open_round(world)
        client = quiet_client(world.app)
        url = f"/v2/runs/{world.run_id}/rounds/0"
        hosted = world.store.for_run(world.run_id)
        expect(client.get(url).status_code == 200, "the round view is not served")
        with hosted._lock:
            written = log_record_writes(hosted._db)
        expect(client.get(url).status_code == 200, "the repeated round view is not served")
        with hosted._lock:
            rewritten = written()
    assert rewritten == [], f"{len(rewritten)} stored records rewritten by the second read"


# LV-02 ------------------------------------------------------------------------------------


# Closed by escrow-relay-fixes ADR0014-P8 (b61c6389): the admission lookup runs on a worker thread.
def test_a_signed_object_upload_never_takes_the_store_lock_on_the_event_loop(
    tmp_path: Path,
) -> None:
    service = sibling_service()
    miner = service.HOT[0]
    with service_world(tmp_path) as world:
        n = world.network
        expect(n.join(0).status_code == 200, "the miner did not join")
        hosted = world.store.for_run(world.run_id)
        loop_thread = n.client.portal.call(threading.get_ident)
        spy = LockSpy(hosted._lock)
        hosted._lock = spy
        try:
            stored = put_object(n, miner, b"an admission artifact")
        finally:
            hosted._lock = spy.inner
        expect(stored.status_code == 200, f"the upload is not served: {stored.text}")
        expect(spy.holders, "the upload never touched the store lock")
        assert loop_thread not in spy.holders, "the event loop thread waited on the store lock"


# LV-03 ------------------------------------------------------------------------------------


# Closed by escrow-relay-fixes ADR0014-P6 (dae84220): the shipped relay wires the master's view.
def test_the_shipped_relay_does_not_refuse_a_release_for_want_of_a_settlement_journal(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    runtime = sibling_runtime()
    built: list[Any] = []
    create = relay_app.create_app

    def capture(relay: Any, drain_token: str) -> Any:
        built.append(relay)
        return create(relay, drain_token)

    def unreachable(request: httpx.Request) -> httpx.Response:
        raise httpx.ConnectError("no master in this test", request=request)

    monkeypatch.setattr(relay_app, "create_app", capture)
    monkeypatch.setattr(
        relay_app,
        "http_client",
        lambda: httpx.AsyncClient(transport=httpx.MockTransport(unreachable)),
    )
    release = CustodyRelease(
        grant_hash=HASH,
        custody_hashes=[HASH],
        retention_hash=HASH,
        finality_hash=HASH,
        vesting_beacon=1,
        closed_dispute_root=HASH,
        release_beacon=11,
    )

    async def scenario() -> str:
        async with runtime.serving(tmp_path, monkeypatch, runtime.JUST_AFTER_BOUNDARY):
            expect(len(built) == 1, "the entry point did not build exactly one relay")
            sealed = relay_envelope.seal(
                runtime.MASTER, "CustodyRelease", runtime.RUN, release, 10_000
            )
            try:
                await built[0].release(relay_envelope.parse_envelope(sealed))
            except Exception as error:
                return str(error)
            return ""

    answer = asyncio.run(scenario())
    assert "settlement journal not configured" not in answer, answer


def sealed_release(rig: Any, grant: UploadGrant, custody: ChunkCustodyAck) -> Any:
    return rig.seal(
        CustodyRelease(
            grant_hash=grant.digest(),
            custody_hashes=[custody.digest()],
            retention_hash=custody.digest(),
            finality_hash=HASH,
            vesting_beacon=1,
            closed_dispute_root=HASH,
            release_beacon=11,
        )
    )


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason=(
        "LV-03+TRN-02: a released upload frees its slot in the retained-upload cap "
        "[fix-lot: escrow-relay-fixes ADR0014-P6]"
    ),
)
def test_a_released_upload_frees_its_slot_in_the_retained_upload_cap(tmp_path: Path) -> None:
    fixture = load_fixture_module(*RELAY_FIXTURE)
    capacity = 2  # the cap is a constant; the property is that finished uploads leave it

    async def scenario() -> None:
        async with httpx.AsyncClient() as wire:
            rig = fixture.Rig(tmp_path, wire)
            rig.relay.max_uploads = capacity

            async def settlement(grant_hash: str) -> Settlement:
                return Settlement(HASH, 1, HASH, 1, 1)

            rig.relay.settlement = settlement
            served = []
            for w in range(1, capacity + 1):
                data = f"round {w}".encode()
                grant, manifest, raw, assignment, opened = rig.upload(data, nonce=f"{w:064x}", w=w)
                await rig.relay.accept(raw, manifest, assignment, opened)
                served.append(
                    (grant, ChunkCustodyAck.model_validate((await rig.put(grant, data)).body))
                )
            # Control: the cap is what stops the next round's grant while nothing is released.
            grant, manifest, raw, assignment, opened = rig.upload(b"next", nonce="ee" * 32, w=9)
            with pytest.raises(RelayError, match="retained upload quota exhausted"):
                await rig.relay.accept(raw, manifest, assignment, opened)
            # Finality releases custody of both served rounds.
            for earlier, custody in served:
                await rig.relay.release(sealed_release(rig, earlier, custody))
            try:
                await rig.relay.accept(raw, manifest, assignment, opened)
            except RelayError as error:
                assert "retained upload quota" not in str(error), "released uploads still count"

    asyncio.run(scenario())


# LV-07 ------------------------------------------------------------------------------------


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason=(
        "LV-07+TRN-05: one hotkey cannot store more objects than any proof may reference "
        "[fix-lot: escrow-relay-fixes ADR0014-P8 (TRN-05)]"
    ),
)
def test_one_hotkey_cannot_store_more_objects_than_a_proof_can_reference(tmp_path: Path) -> None:
    limit = next(
        m.max_length
        for m in WorkProof.model_fields["artifact_refs"].metadata
        if hasattr(m, "max_length")
    )
    service = sibling_service()
    miner = service.HOT[0]
    with service_world(tmp_path) as world:
        n = world.network
        expect(n.join(0).status_code == 200, "the miner did not join")

        def store(i: int) -> int:
            return put_object(n, miner, f"artifact {i}".encode()).status_code

        statuses = [store(i) for i in range(limit + 1)]
        expect(statuses[0] == 200, "a signed upload of an admitted miner is not served")
        accepted = statuses.count(200)
        assert accepted == limit, (
            f"{accepted} objects stored for one hotkey; {limit} is the most a proof lists"
        )


# LV-08 ------------------------------------------------------------------------------------


def capability(envelope: Any) -> str:
    return base64.b64encode(canonicalize(envelope.model_dump(mode="json"))).decode()


def test_a_relay_chunk_needs_an_unexpired_master_grant_and_the_exact_length_and_hash(
    tmp_path: Path,
) -> None:
    """LV-08. Over HTTP, anything but the master's grant and the granted bytes stores nothing."""
    fixture = load_fixture_module(*RELAY_FIXTURE)

    async def scenario() -> None:
        async with httpx.AsyncClient() as backing:
            rig = fixture.Rig(tmp_path, backing)
            grant, manifest, raw, assignment, opened = rig.upload(b"abc")
            await rig.relay.accept(raw, manifest, assignment, opened)
            other = UploadGrant.model_validate({**grant.body(), "nonce": "34" * 32})
            app = create_relay_app(rig.relay, "t" * 32)
            async with httpx.AsyncClient(
                transport=httpx.ASGITransport(app=app), base_url="https://relay.test"
            ) as wire:
                url = f"/v1/uploads/{grant.digest()}/chunks/0"

                async def put(body: bytes, header: str | None) -> tuple[int, str, int]:
                    headers = {} if header is None else {"X-Upload-Grant": header}
                    response = await wire.put(url, content=body, headers=headers)
                    stored = (await rig.relay.state()).uploads[grant.digest()].chunks
                    return response.status_code, response.json().get("error", ""), len(stored)

                good = capability(raw)
                refused = {
                    "no capability": await put(b"abc", None),
                    "grant not signed by the master": await put(
                        b"abc", capability(rig.seal(grant, fixture.MINER))
                    ),
                    "master grant for another upload": await put(
                        b"abc", capability(rig.seal(other))
                    ),
                    "master grant that has expired": await put(
                        b"abc", capability(rig.seal(grant, exp=rig.clock - 1))
                    ),
                    "one byte short": await put(b"ab", good),
                    "one byte long": await put(b"abcd", good),
                    "right length, wrong bytes": await put(b"xyz", good),
                }
                rig.clock = 41  # past the upload deadline of the grant
                refused["after the upload deadline"] = await put(b"abc", good)
                rig.clock = 20
                # No refused request left its bytes in the durable backing either.
                with spool() as sink, pytest.raises(ObjectNotFound):
                    await rig.store.get(hashlib.sha256(b"abc").hexdigest(), 3, sink)
                served = await put(b"abc", good)
            assert refused == {
                "no capability": (400, "MalformedEnvelope", 0),
                "grant not signed by the master": (400, "SignatureError", 0),
                "master grant for another upload": (400, "RelayError", 0),
                "master grant that has expired": (400, "SignatureError", 0),
                "one byte short": (503, "CorruptObjectError", 0),
                "one byte long": (503, "CorruptObjectError", 0),
                "right length, wrong bytes": (503, "CorruptObjectError", 0),
                "after the upload deadline": (400, "RelayError", 0),
            }
            assert served == (200, "", 1)

    asyncio.run(scenario())
