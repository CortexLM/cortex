"""The relay verifies one drand beacon per period, not one per call, and says why it refused."""

from __future__ import annotations

import hashlib
import json
import threading
import time
from collections.abc import AsyncIterator
from contextlib import asynccontextmanager
from pathlib import Path

import anyio
import httpx
import pytest
from fastapi import FastAPI
from pydantic import BaseModel, ValidationError
from test_transport_v2 import MASTER, MINER, RUN, H, Rig

from hypertrain.beacon.core import BeaconRound, BeaconUnavailable, round_at_or_after
from hypertrain.beacon.drand import QUICKNET_GENESIS, QUICKNET_PERIOD
from hypertrain.protocol import relay_envelope
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.relay_envelope import RelayEnvelope
from hypertrain.protocol.relay_messages import (
    RelayAssignment,
    RelayNetworkManifest,
    RelayReceipt,
    UploadGrant,
)
from hypertrain.relay import app as relay_app
from hypertrain.relay.app import AcceptRequest, RelayConfig, create_app

STALE = "verified beacon stale; eligibility/deadlines frozen"
# With d the seconds since drand genesis the expected round is ceil(d / 3), so a round starts
# at d % 3 == 1: the first clock opens a round, the second is one second before the next one.
JUST_AFTER_BOUNDARY = QUICKNET_GENESIS + 61.0
ONE_SECOND_BEFORE_BOUNDARY = QUICKNET_GENESIS + 60.0


def expected_round(wall: float) -> int:
    return round_at_or_after(int(wall), QUICKNET_GENESIS, QUICKNET_PERIOD) - 1


class Clock:
    """Wall and monotonic clocks that move together; installed as relay.app.time."""

    def __init__(self, wall: float) -> None:
        self.wall, self.mono = wall, 5000.0

    def time(self) -> float:
        return self.wall

    def monotonic(self) -> float:
        return self.mono

    def advance(self, seconds: float) -> None:
        self.wall += seconds
        self.mono += seconds


class Drand:
    """Controlled drand client: counts latest() calls, takes `cost` fake seconds per call."""

    def __init__(self, clock: Clock) -> None:
        self.clock, self.lock, self.calls = clock, threading.Lock(), 0
        self.cost, self.behind, self.pinned = 0.0, 0, None
        self.verified, self.error = True, None

    def latest(self) -> BeaconRound:
        with self.lock:
            self.calls += 1
            observed = expected_round(self.clock.wall)
        time.sleep(0.05)  # real time: concurrent callers must overlap the fetch
        if self.error is not None:
            raise self.error
        with self.lock:
            self.clock.advance(self.cost)
        number = observed - self.behind if self.pinned is None else self.pinned
        return BeaconRound(number, "00" * 48, "11" * 32, bls_verified=self.verified)


async def burst(client: httpx.AsyncClient, count: int) -> list[httpx.Response]:
    answers: list[httpx.Response] = []

    async def probe() -> None:
        answers.append(await client.get("/probe"))

    async with anyio.create_task_group() as tasks:
        for _ in range(count):
            tasks.start_soon(probe)
    return answers


@asynccontextmanager
async def serving(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, start: float, max_uploads: int | None = None
) -> AsyncIterator[tuple[httpx.AsyncClient, Drand, Clock]]:
    """The real runtime() lifespan on a local backing; only drand and the clock are injected."""
    clock = Clock(start)
    drand = Drand(clock)
    monkeypatch.setattr(relay_app, "time", clock)
    monkeypatch.setattr(relay_app, "DrandQuicknet", lambda: drand)
    secrets = tmp_path / "secrets"
    secrets.mkdir()
    (secrets / "k1.key").write_text(bytes([3] * 32).hex())
    (secrets / "k2.key").write_text(bytes([4] * 32).hex())
    (secrets / "drain.token").write_text("d" * 32)
    async with httpx.AsyncClient() as backing:
        registry = Rig(tmp_path / "rig", backing).registry
    network = RelayNetworkManifest(
        run_id=RUN,
        base_manifest_hash=H,
        registry_hash=registry.digest(),
        specs_hash=hashlib.sha256(
            canonicalize([s.body() for s in registry.specs], allow_float=False)
        ).hexdigest(),
        assignment_policy="master-observed-median3-v1",
    )

    def sealed(model: BaseModel) -> RelayEnvelope:
        return relay_envelope.parse_envelope(
            relay_envelope.seal(MASTER, type(model).__name__, RUN, model, 10_000)
        )

    config = RelayConfig(
        run_id=RUN,
        master_public=MASTER.ss58,
        master_url="https://master.test",
        registry=sealed(registry),
        network_manifest=sealed(network),
        relay_id="eu-1",
        region="eu",
        active_key="k1",
        signing_files={"k1": "k1.key", "k2": "k2.key"},
        observer_public_keys=[MASTER.ss58, MINER.ss58],
        local_backing=str(tmp_path / "store"),
        **({} if max_uploads is None else {"max_uploads": max_uploads}),
    )
    (tmp_path / "relay.json").write_text(json.dumps(config.model_dump(mode="json")))
    monkeypatch.setenv("RELAY_CONFIG", str(tmp_path / "relay.json"))
    monkeypatch.setenv("RELAY_SECRETS", str(secrets))
    app = FastAPI()
    async with relay_app.runtime(app):
        async with httpx.AsyncClient(
            transport=httpx.ASGITransport(app=app), base_url="https://relay.test"
        ) as client:
            clock.advance(QUICKNET_PERIOD)  # startup fetched once; start from an empty period
            drand.calls = 0
            yield client, drand, clock


def test_concurrent_calls_share_one_slow_fetch(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # Four uploads on the s100 relay each verified drand (seconds of BLS) per capability and
    # chunk call; together they outlived the 2-round bound and every chunk got 400.
    async def scenario() -> None:
        async with serving(tmp_path, monkeypatch, JUST_AFTER_BOUNDARY) as (client, drand, clock):
            drand.cost = 2.5
            first = expected_round(clock.wall)
            answers = await burst(client, 6)

            assert [a.status_code for a in answers] == [200] * 6
            assert {a.json()["beacon"] for a in answers} == {first}
            assert drand.calls == 1

    anyio.run(scenario)


def test_beacon_is_held_for_one_period_from_the_end_of_the_fetch(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    async def scenario() -> None:
        async with serving(tmp_path, monkeypatch, JUST_AFTER_BOUNDARY) as (client, drand, clock):
            drand.cost = 2.5
            held = (await client.get("/probe")).json()["beacon"]
            assert drand.calls == 1

            clock.advance(QUICKNET_PERIOD - 1.0)
            reused = await client.get("/probe")
            assert (reused.status_code, reused.json()["beacon"], drand.calls) == (200, held, 1)

            clock.advance(1.0)  # exactly one period after the fetch ended
            renewed = await client.get("/probe")
            assert renewed.status_code == 200 and drand.calls == 2
            assert renewed.json()["beacon"] > held

    anyio.run(scenario)


def test_failed_fetch_is_shared_by_concurrent_calls_and_retried_by_the_next(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    async def scenario() -> None:
        async with serving(tmp_path, monkeypatch, JUST_AFTER_BOUNDARY) as (client, drand, _):
            drand.error = BeaconUnavailable("0/2 relays answered")
            down = await burst(client, 4)
            assert [(a.status_code, a.json()) for a in down] == [
                (503, {"error": "BeaconUnavailable"})
            ] * 4
            assert drand.calls == 1

            drand.error = None
            assert (await client.get("/probe")).status_code == 200 and drand.calls == 2
            assert (await client.get("/probe")).status_code == 200 and drand.calls == 2

    anyio.run(scenario)


@pytest.mark.parametrize("fault", ["unverified", "stale", "ahead"])
def test_unusable_beacon_is_refused_on_every_call_and_not_reused(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, fault: str
) -> None:
    async def scenario() -> None:
        async with serving(tmp_path, monkeypatch, JUST_AFTER_BOUNDARY) as (client, drand, _):
            drand.verified = fault != "unverified"
            drand.behind = {"stale": 3, "ahead": -3}.get(fault, 0)
            for attempt in (1, 2, 3):
                refused = await client.get("/probe")
                assert refused.status_code == 400 and refused.json()["detail"] == STALE
                assert drand.calls == attempt

            drand.verified, drand.behind = True, 0
            assert (await client.get("/probe")).status_code == 200 and drand.calls == 4
            assert (await client.get("/probe")).status_code == 200 and drand.calls == 4

    anyio.run(scenario)


def test_held_beacon_is_refused_once_the_wall_clock_makes_it_stale(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    async def scenario() -> None:
        async with serving(tmp_path, monkeypatch, ONE_SECOND_BEFORE_BOUNDARY) as (
            client,
            drand,
            clock,
        ):
            # drand sits two rounds behind: still inside the bound, held for the period.
            drand.pinned = expected_round(clock.wall) - 2
            assert (await client.get("/probe")).status_code == 200
            assert (await client.get("/probe")).status_code == 200 and drand.calls == 1

            clock.advance(1.0)  # the next round starts: the pinned beacon is now 3 behind
            refused = await client.get("/probe")
            assert refused.status_code == 400 and refused.json()["detail"] == STALE
            assert drand.calls == 2

    anyio.run(scenario)


class Interrupted(BaseException):
    """Not an Exception: the fetching caller leaves without an outcome to share."""


def test_waiting_caller_fetches_again_when_the_fetching_caller_is_interrupted() -> None:
    clock = Clock(JUST_AFTER_BOUNDARY)
    fetches: list[int] = []

    def latest() -> BeaconRound:
        fetches.append(1)
        time.sleep(0.05)
        if len(fetches) == 1:
            raise Interrupted
        return BeaconRound(expected_round(clock.wall), "00" * 48, "11" * 32, bls_verified=True)

    async def scenario() -> None:
        cache = relay_app.BeaconCache(latest, clock.time, clock.monotonic)
        outcomes: dict[str, int | str] = {}

        async def call(name: str) -> None:
            try:
                outcomes[name] = await cache.now()
            except Interrupted:
                outcomes[name] = "interrupted"

        async with anyio.create_task_group() as tasks:
            tasks.start_soon(call, "fetcher")
            await anyio.sleep(0.01)  # the fetch is in flight: this caller waits for it
            tasks.start_soon(call, "waiter")

        assert outcomes == {"fetcher": "interrupted", "waiter": expected_round(clock.wall)}
        assert len(fetches) == 2

    anyio.run(scenario)


def test_relay_error_text_is_returned_and_other_rejections_stay_opaque(tmp_path: Path) -> None:
    async def scenario() -> None:
        async with httpx.AsyncClient() as backing:
            rig = Rig(tmp_path, backing)
            async with httpx.AsyncClient(
                transport=httpx.ASGITransport(app=create_app(rig.relay, "t" * 32)),
                base_url="https://relay.test",
            ) as wire:
                grant = "ab" * 32
                relay_error = await wire.get(
                    f"/v1/uploads/{grant}/acks", headers={"X-Upload-Grant": "a" * 8193}
                )
                assert relay_error.status_code == 400
                assert relay_error.json() == {
                    "error": "RelayError",
                    "detail": "grant header exceeds bound",
                }

                validation = await wire.post(
                    f"/v1/uploads/{grant}/accept", content=b'{"grant": "SENTINEL-INPUT"}'
                )
                envelope = await wire.post(
                    f"/v1/uploads/{grant}/retention", content=b'{"SENTINEL-INPUT": '
                )
                assert [r.status_code for r in (validation, envelope)] == [400, 400]
                assert validation.json() == {"error": "ValidationError"}
                assert envelope.json() == {"error": "MalformedEnvelope"}
                assert "SENTINEL-INPUT" not in validation.text + envelope.text

    anyio.run(scenario)


async def accept_request(tmp_path: Path, w: int) -> tuple[str, bytes]:
    """Path and body accepting round w, bound to the network manifest the served relay pinned."""
    config = RelayConfig.model_validate_json((tmp_path / "relay.json").read_bytes())
    pinned = RelayNetworkManifest.model_validate(config.network_manifest.body).digest()
    async with httpx.AsyncClient() as backing:
        rig = Rig(tmp_path / "rig", backing)
    grant, manifest, raw, assignment, opened = rig.upload(b"round-%d" % w, nonce=f"{w:064x}", w=w)
    bound = RelayAssignment.model_validate({**assignment.body, "manifest_hash": pinned})
    body = AcceptRequest(
        grant=raw, manifest=manifest, assignment=rig.seal(bound), round_open=opened
    )
    return f"/v1/uploads/{grant.digest()}/accept", body.model_dump_json().encode()


@pytest.mark.parametrize("max_uploads", [None, 1])
def test_relay_json_max_uploads_bounds_the_held_uploads(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, max_uploads: int | None
) -> None:
    # The optional key is written only when set (an older relay image rejects unknown keys);
    # unset leaves the count unbounded, set bounds it, and the pod stays ready either way.
    async def scenario() -> None:
        async with serving(tmp_path, monkeypatch, JUST_AFTER_BOUNDARY, max_uploads) as (
            client,
            _,
            _,
        ):
            written = json.loads((tmp_path / "relay.json").read_text())
            assert ("max_uploads" in written) == (max_uploads is not None)
            answers = []
            for w in (1, 2):
                path, body = await accept_request(tmp_path, w)
                answers.append(await client.post(path, content=body))
            assert [a.status_code for a in answers] == [200, 200 if max_uploads is None else 400]
            if max_uploads is not None:
                assert answers[1].json()["detail"] == "retained upload quota exhausted"
            assert (await client.get("/readyz")).status_code == 200

    anyio.run(scenario)


def test_relay_json_refuses_a_max_uploads_that_admits_nothing(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # A zero limit would refuse every grant while /readyz stays green.
    async def scenario() -> None:
        with pytest.raises(ValidationError, match="greater than or equal to 1"):
            async with serving(tmp_path, monkeypatch, JUST_AFTER_BOUNDARY, 0):
                pass

    anyio.run(scenario)


async def hold_a_receipted_upload(tmp_path: Path) -> tuple[UploadGrant, str]:
    """A complete upload in the journal the served relay reads; returns the grant, receipt pin."""
    async with httpx.AsyncClient() as client:
        rig = Rig(tmp_path / "store", client)
        data = b"round-1"
        grant, manifest, raw, assignment, opened = rig.upload(data, nonce="01" * 32)
        await rig.relay.accept(raw, manifest, assignment, opened)
        await rig.put(grant, data)
        record = (await rig.relay.state()).uploads[grant.digest()]
        assert record.receipt is not None
        return grant, RelayReceipt.model_validate(record.receipt.body).digest()
