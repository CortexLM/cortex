"""Authentication of the HTTP routes, part 2: header signatures, GET side effects, liveness.

Package http_auth of the ledger-invariants lot. The tests marked xfail state a defence the
service does not hold on this tree; each carries a control that reaches the point under test.
"""

from __future__ import annotations

import ast
import asyncio
import base64
import hashlib
import threading
from pathlib import Path
from typing import Any

import httpx
import pytest
from fastapi.testclient import TestClient

import hypertrain
from hypertrain.data.store import ObjectNotFound
from hypertrain.data.stream_store import spool
from hypertrain.protocol import envelope_v2
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages import Receipt
from hypertrain.protocol.messages_v2 import WorkProof
from hypertrain.protocol.relay_messages import (
    ChunkCustodyAck,
    CustodyRelease,
    UploadGrant,
)
from hypertrain.relay.app import create_app as create_relay_app
from hypertrain.relay.core import RelayError, Settlement

from .support import load_fixture_module, service_world
from .support_http_auth import (
    HASH,
    LockSpy,
    changed,
    detail,
    durable_state,
    open_round,
    quiet_client,
    seed_relay_scenario,
    sibling_service,
)

RELAY_FIXTURE = ("li_relay_fixture", "relay/test_transport_v2.py")
SOURCE = Path(hypertrain.__file__).resolve().parent


# SG-05 ------------------------------------------------------------------------------------


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="SG-05+HTP-03: a dispute-watch header signed in an earlier round is refused later",
)
def test_a_dispute_watch_header_captured_in_an_earlier_round_is_refused_later(
    tmp_path: Path,
) -> None:
    service = sibling_service()
    party = service.HOT[0]
    with service_world(tmp_path) as world:
        n = world.network
        text = f"hypertrain/watch/2|{n.manifest.run_id()}|{party.ss58}".encode()
        headers = {"x-dispute-signature": party.sign(text).hex()}
        url = n.url + "/disputes"
        # Control: in the round it was made the header lets the party watch.
        assert detail(n.client.get(url, params={"party": party.ss58}, headers=headers)) == (200, [])
        n.push(300)
        later = n.client.get(url, params={"party": party.ss58}, headers=headers)
        assert later.status_code == 403, "the same header still opens the watch 299 rounds on"


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="SG-05+HTP-03: a trial-job header signed in an earlier round is refused later",
)
def test_a_trial_job_header_captured_in_an_earlier_round_is_refused_later(tmp_path: Path) -> None:
    service = sibling_service()
    owner = service.HOT[0]
    with service_world(tmp_path) as world:
        n = world.network
        identity = n.join(0).json()["admission_id"]
        n.push(2)
        assert n.client.get(n.url + f"/join/{identity}/challenge").status_code == 200
        text = f"hypertrain/trial-job/2|{n.manifest.run_id()}|{identity}".encode()
        headers = {"x-hotkey": owner.ss58, "x-trial-signature": owner.sign(text).hex()}
        url = n.url + f"/join/{identity}/job"
        # Control: in the round it was made the header exports the trial job.
        assert n.client.get(url, headers=headers).status_code == 200
        n.push(50)  # still inside the 100-round trial, 48 rounds after the signature
        later = n.client.get(url, headers=headers)
        assert later.status_code == 403, "the same header still exports the job 48 rounds on"


# SG-13 ------------------------------------------------------------------------------------


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="SG-13+HTP-10: the beacon pin table does not grow by a row for every round that an "
    "anonymous status read touches",
)
def test_pin_rows_do_not_pile_up_one_per_round_of_anonymous_status_reads(tmp_path: Path) -> None:
    service = sibling_service()
    rounds = 120
    with service_world(tmp_path) as world:
        n = world.network
        assert n.join(0).status_code == 200
        client = quiet_client(world.app)
        status = f"/v2/runs/{world.run_id}/admission/{service.HOT[0].ss58}"
        hosted = world.store.for_run(world.run_id)

        def pins() -> int:
            with hosted._lock:
                return int(hosted._db.execute("SELECT COUNT(*) FROM beacon_pins_v2").fetchone()[0])

        def read_each_round(first: int) -> None:
            for number in range(first, first + rounds):
                n.push(number)
                assert client.get(status).status_code == 200

        read_each_round(2)
        middle = pins()
        read_each_round(2 + rounds)
        # A pruning or a retention window flattens the second phase; a table that keeps
        # every pin adds one row per round, exactly as the first phase did.
        assert pins() - middle < rounds, f"{pins() - middle} pin rows added in {rounds} rounds"


# GT-04 ------------------------------------------------------------------------------------


def observer_challenge(client: TestClient, url: str) -> dict[str, Any]:
    response = client.get(url)
    assert response.status_code == 200, response.text
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
    reason="GT-04+HTP-07: an anonymous second GET of a relay-observer challenge does not "
    "void the pending one",
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
        assert accepted.status_code == 200, accepted.text
        # A stranger who knows the grant and the observer asks again before the observer answers.
        pending = observer_challenge(client, url)
        observer_challenge(client, url)
        answer = client.post(url, json=observer_answer(world, scenario, pending))
        assert answer.status_code == 200, detail(answer)


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="GT-04+HTP-07: a relay-observer challenge asked for in a later round is never a 500",
)
def test_an_observer_challenge_asked_for_in_a_later_round_is_a_mapped_answer(
    tmp_path: Path,
) -> None:
    with service_world(tmp_path) as world:
        scenario = seed_relay_scenario(world)
        client = quiet_client(world.app)
        url = world.network.url + f"/relay-observers/{scenario.grant_hash}/{scenario.observer.ss58}"
        assert client.get(url).status_code == 200  # control: the first round issues a challenge
        world.network.push(5)
        later = client.get(url)
        assert later.status_code < 500, f"answered {later.status_code} in the next round"


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
    reason="GT-05+HTP-02: a repeated anonymous read of the round view performs no write",
)
def test_a_repeated_anonymous_round_view_rewrites_nothing(tmp_path: Path) -> None:
    with service_world(tmp_path) as world:
        open_round(world)
        client = quiet_client(world.app)
        url = f"/v2/runs/{world.run_id}/rounds/0"
        hosted = world.store.for_run(world.run_id)
        assert client.get(url).status_code == 200
        writes = hosted._db.total_changes
        assert client.get(url).status_code == 200
        assert hosted._db.total_changes == writes, (
            f"{hosted._db.total_changes - writes} rows rewritten by the second read"
        )


# LV-01 / LV-02 ----------------------------------------------------------------------------


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="LV-01+ADM-03: no request holds the store lock while a trial reference is computed",
)
def test_the_store_lock_is_free_while_a_trial_reference_is_being_computed(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    with service_world(tmp_path) as world:
        n, run = world.network, world.run_id
        hosted = world.store.for_run(run)
        admission = hosted._services(run)[1]
        started, resume = threading.Event(), threading.Event()

        def computing(*args: Any, **kwargs: Any) -> str:
            # Stand-in for the trusted reference run, which takes minutes on a real model.
            started.set()
            assert resume.wait(timeout=60)
            return HASH

        monkeypatch.setattr(admission, "prepare_reference", computing)
        reply: list[httpx.Response] = []
        caller = threading.Thread(
            target=lambda: reply.append(
                n.client.post(
                    n.url + f"/admin/join/{HASH}/reference", headers=sibling_service().admin()
                )
            )
        )
        assert hosted._lock.acquire(blocking=False)  # control: idle, the lock is there to take
        hosted._lock.release()
        caller.start()
        try:
            assert started.wait(timeout=60), "the reference computation never started"
            free = hosted._lock.acquire(blocking=False)
            if free:
                hosted._lock.release()
            assert free, "the store lock is held for as long as the reference computes"
        finally:
            resume.set()
            caller.join(timeout=60)
        assert not caller.is_alive() and reply[0].status_code == 200


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="LV-02+ADM-04: a signed object upload never takes the store lock on the event loop",
)
def test_a_signed_object_upload_never_takes_the_store_lock_on_the_event_loop(
    tmp_path: Path,
) -> None:
    service = sibling_service()
    miner = service.HOT[0]
    with service_world(tmp_path) as world:
        n = world.network
        assert n.join(0).status_code == 200
        hosted = world.store.for_run(world.run_id)
        loop_thread = n.client.portal.call(threading.get_ident)
        spy = LockSpy(hosted._lock)
        hosted._lock = spy
        try:
            body = b"an admission artifact"
            sha = hashlib.sha256(body).hexdigest()
            signature = miner.sign(f"hypertrain/object/2|{world.run_id}|{sha}".encode()).hex()
            stored = n.client.put(
                n.url + "/objects/" + sha,
                content=body,
                headers={"x-hotkey": miner.ss58, "x-object-signature": signature},
            )
        finally:
            hosted._lock = spy.inner
        assert stored.status_code == 200, stored.text  # control: the upload is served
        assert spy.holders, "the upload never touched the store lock"
        assert loop_thread not in spy.holders, "the event loop thread waited on the store lock"


# LV-03 ------------------------------------------------------------------------------------


def runtime_relay_keywords() -> set[str]:
    """Keyword arguments the shipped entry point passes when it builds the `Relay`."""
    tree = ast.parse((SOURCE / "relay/app.py").read_text())
    runtime = next(
        n for n in ast.walk(tree) if isinstance(n, ast.AsyncFunctionDef) and n.name == "runtime"
    )
    builds = [
        node
        for node in ast.walk(runtime)
        if isinstance(node, ast.Call) and getattr(node.func, "id", "") == "Relay"
    ]
    assert len(builds) == 1
    return {k.arg for k in builds[0].keywords if k.arg}


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="LV-03+TRN-02: the shipped relay entry point gives the relay the settlement journal "
    "that releasing a retained upload needs",
)
def test_the_shipped_relay_is_built_with_the_settlement_a_release_needs() -> None:
    keywords = runtime_relay_keywords()
    wiring = {"run_id", "registry", "backing", "now"}
    assert wiring <= keywords  # control: this is the call that builds the served relay
    assert "settlement" in keywords


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
    raises=RelayError,
    reason="LV-03+TRN-02: a released upload frees its slot in the retained-upload cap",
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
            await rig.relay.accept(raw, manifest, assignment, opened)

    asyncio.run(scenario())


# LV-07 ------------------------------------------------------------------------------------


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="LV-07+TRN-05: one hotkey cannot store more objects than any proof may reference",
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
        assert n.join(0).status_code == 200
        accepted = 0
        for i in range(limit + 1):
            body = f"artifact {i}".encode()
            sha = hashlib.sha256(body).hexdigest()
            signature = miner.sign(f"hypertrain/object/2|{world.run_id}|{sha}".encode()).hex()
            stored = n.client.put(
                n.url + "/objects/" + sha,
                content=body,
                headers={"x-hotkey": miner.ss58, "x-object-signature": signature},
            )
            accepted += stored.status_code == 200
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
