"""HTTP boundary of the signed envelopes: run binding, expiry, the clock and the freshness gate.

SG-01 and ESC-21: an envelope belongs to its run, its expiry and its signature, on every route
type. CK-01: expiry is judged on the stored beacon. CK-02: a stale beacon refuses admission
mutations before their signature is read. CK-03: a missing seed round is a 503. SG-04: no
signed expiry is bounded. Holes are strict xfails asserting the desired defence; see the
header of test_expiry.py for the control convention.
"""

from __future__ import annotations

import hashlib
from collections.abc import Iterator
from pathlib import Path
from typing import Any

import pytest
from fastapi.testclient import TestClient

from hypertrain.miner.admission import sign_join, sign_rotation
from hypertrain.protocol.envelope_v2 import seal
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages_v2 import EscrowTransfer, HardwareHint, RotateRequest

from .support import World, service_world
from .support_expiry import (
    activate,
    audited,
    cold,
    control,
    files,
    foreign_run_id,
    hot,
    lease_request,
    open_round,
    post_commit,
    publish_leaves,
    service,
    signed_requests,
    stale,
    tables,
    touched,
)

# resolution is absent: the dispute it resolves is read before its envelope, so a request for a
# dispute that does not exist is refused for that reason, not for the run it names.
ROUTES = [
    "lock",
    "transfer",
    "release",
    "rotate",
    "recover",
    "dispute",
    "bisect",
    "state-serve",
    "accept",
    "commit",
    "delta",
    "leaves",
    "join-proof",
    "lease",
    "execute",
    "complete",
    "genesis",
    "open",
    "finalize",
]
ADMISSION_ROUTES = [
    "lock",
    "transfer",
    "release",
    "rotate",
    "recover",
    "dispute",
    "bisect",
    "resolution",
    "state-serve",
]
PAUSED = "beacon infrastructure paused"


@pytest.fixture(scope="module")
def routes_world(tmp_path_factory: pytest.TempPathFactory) -> Iterator[tuple[World, Any, Any]]:
    """Four ACTIVE members, an audited round and a live lease: every route reaches its envelope.

    Every test using it sends refusals only, so the world is shared; a test that moves the
    clock puts it back."""
    with service_world(tmp_path_factory.mktemp("expiry-routes")) as world:
        rnd = audited(world)
        leased = lease_request(world, service().AUDITORS[0])
        control(leased.status_code == 200, f"lease: {leased.text}")
        yield world, rnd, leased.json()["body"]


def envelopes_of(request: Any, change: Any) -> Any:
    """`request.payload` with `change` applied to each of its envelopes."""
    payload = request.payload
    if isinstance(payload, dict) and set(payload) == {"proof", "screen"}:
        return {name: change(envelope) for name, envelope in payload.items()}
    return change(payload)


def sealed_for(world: World, rnd: Any, job: Any, route: str, run_id: str, expiry: int) -> Any:
    return signed_requests(world, rnd, run_id, expiry, job)[route]


VARIANTS = {
    "sealed-for-another-run": "envelope belongs to another run",
    "relabeled-to-this-run": "sr25519 signature does not verify",
    "expiry-extended": "sr25519 signature does not verify",
    "expired": "envelope expired",
}


@pytest.mark.parametrize("variant", list(VARIANTS))
@pytest.mark.parametrize("route", ROUTES)
def test_a_signed_envelope_is_bound_to_its_run_its_expiry_and_its_signature(
    routes_world: tuple[World, Any, Any], route: str, variant: str
) -> None:
    world, rnd, job = routes_world
    now = world.network.now
    foreign = foreign_run_id(world)
    if variant == "sealed-for-another-run":
        request = sealed_for(world, rnd, job, route, foreign, 10_000)
        sent = request.payload
    elif variant == "relabeled-to-this-run":
        request = sealed_for(world, rnd, job, route, foreign, 10_000)
        sent = envelopes_of(request, lambda e: {**e, "run_id": world.run_id})
    elif variant == "expiry-extended":
        request = sealed_for(world, rnd, job, route, world.run_id, 10_000)
        sent = envelopes_of(request, lambda e: {**e, "exp_drand": e["exp_drand"] + 1000})
    else:
        request = sealed_for(world, rnd, job, route, world.run_id, now - 1)
        sent = request.payload
    before, written = tables(world), files(world)
    refused = request.post(world, payload=sent)
    assert (refused.status_code, refused.json()["detail"]) == (409, VARIANTS[variant])
    assert touched(before, tables(world)) == [] and files(world) == written


# --- ESC-21: an escrow envelope is idempotent by operation, bound to its expiry -------------------


def transfer_request(world: World, operation: str, units: int, expiry: int = 10_000) -> Any:
    network = world.network
    body = EscrowTransfer(
        operation_id=operation,
        owner=cold(0).ss58,
        units=units,
        origin_ids=[network.origin_ids[0]],
        admission_id=None,
        dispute_id=None,
        recipient=cold(1).ss58,
    )
    return network.client.post(
        network.url + "/escrow/transfer",
        json=network.signed(cold(0), "EscrowTransfer", body, expiry=expiry),
    )


def available(world: World, index: int) -> int:
    escrow = world.store._services(world.run_id)[0]
    return int(escrow.balances(cold(index).ss58).available)


def test_a_replayed_escrow_transfer_moves_units_once_and_a_changed_body_is_refused(
    tmp_path: Path,
) -> None:
    with service_world(tmp_path) as world:
        start = (available(world, 0), available(world, 1))
        first = transfer_request(world, "a1" * 32, 100)
        # The same operation signed again: signatures are randomized, the semantics are not.
        again = transfer_request(world, "a1" * 32, 100)
        assert (first.status_code, again.status_code) == (200, 200)
        assert again.json() == first.json()
        moved = (available(world, 0), available(world, 1))
        assert moved == (start[0] - 100, start[1] + 100)
        before = tables(world)
        changed = transfer_request(world, "a1" * 32, 200)
        assert (changed.status_code, changed.json()["detail"]) == (
            409,
            "conflicting signed operation nonce",
        )
        assert (available(world, 0), available(world, 1)) == moved
        assert touched(before, tables(world)) == []


def test_an_escrow_envelope_is_live_through_its_expiry_beacon_and_refused_after(
    tmp_path: Path,
) -> None:
    with service_world(tmp_path) as world:
        world.network.push(10)
        start = (available(world, 0), available(world, 1))
        before = tables(world)
        late = transfer_request(world, "b1" * 32, 100, expiry=9)
        assert (late.status_code, late.json()["detail"]) == (409, "envelope expired")
        assert (available(world, 0), available(world, 1)) == start
        assert touched(before, tables(world)) == []
        last = transfer_request(world, "b2" * 32, 100, expiry=10)
        assert last.status_code == 200, last.text
        assert (available(world, 0), available(world, 1)) == (start[0] - 100, start[1] + 100)


# --- CK-01: expiry is judged on the stored beacon; the wall clock only decides currency -----------


def test_expiry_is_judged_on_the_stored_beacon_whatever_the_wall_clock_says(
    tmp_path: Path,
) -> None:
    with service_world(tmp_path) as world:
        world.network.push(10)
        stale(world, 2)  # two rounds ahead of the stored beacon: still current
        ahead = transfer_request(world, "c1" * 32, 1, expiry=10)
        assert ahead.status_code == 200, ahead.text
        genesis = world.network.manifest.training.beacon.genesis_time
        world.store.clock = lambda: genesis  # the wall clock says round 1, the beacon says 10
        behind = transfer_request(world, "c2" * 32, 1, expiry=9)
        assert (behind.status_code, behind.json()["detail"]) == (409, "envelope expired")


def test_the_freshness_gate_tolerates_two_rounds_of_lag_and_refuses_the_third(
    tmp_path: Path,
) -> None:
    with service_world(tmp_path) as world:
        world.network.push(10)
        outcomes = []
        for lag in range(5):
            stale(world, lag)
            response = transfer_request(world, f"{lag + 1:064x}", 1)
            outcomes.append((lag, response.status_code))
        assert outcomes == [(0, 200), (1, 200), (2, 200), (3, 503), (4, 503)]
        stale(world, 0)
        resumed = transfer_request(world, "d9" * 32, 1)
        assert resumed.status_code == 200, resumed.text


# --- CK-02: a stale beacon refuses admission mutations before their signature is read ----------


def forged(request: Any) -> Any:
    return envelopes_of(request, lambda e: {**e, "sig": "0" * 128})


@pytest.mark.parametrize("route", [r for r in ADMISSION_ROUTES if r != "resolution"])
def test_a_stale_beacon_refuses_an_admission_mutation_before_its_signature_is_read(
    routes_world: tuple[World, Any, Any], route: str
) -> None:
    world, rnd, job = routes_world
    request = sealed_for(world, rnd, job, route, world.run_id, 10_000)
    before = tables(world)
    try:
        stale(world, 3)
        refused = request.post(world, payload=forged(request))
    finally:
        stale(world, 0)
    assert (refused.status_code, refused.json()["detail"]) == (503, PAUSED)
    # The pause itself is the one durable trace; nothing the request asked for is.
    assert set(touched(before, tables(world))) <= {"0:beacon_pins_v2"}
    # Control: with a fresh beacon the same forged request gets as far as its signature.
    fresh = request.post(world, payload=forged(request))
    assert (fresh.status_code, fresh.json()["detail"]) == (409, "sr25519 signature does not verify")


def test_a_stale_beacon_refuses_a_resolution_before_the_dispute_it_names_is_read(
    routes_world: tuple[World, Any, Any],
) -> None:
    world, rnd, _ = routes_world
    from hypertrain.protocol.messages_v2 import ResolutionV2

    one = "11" * 32
    body = ResolutionV2(
        dispute_id=one, transcript_hash=one, reason="MATCH", loser=None, evidence_hash=one
    )
    envelope = seal(service().REFEREE, "ResolutionV2", world.run_id, body, 10_000)
    url = world.network.url + "/resolution"
    try:
        stale(world, 3)
        refused = world.network.client.post(url, json=envelope)
    finally:
        stale(world, 0)
    assert (refused.status_code, refused.json()["detail"]) == (503, PAUSED)
    fresh = world.network.client.post(url, json=envelope)
    assert (fresh.status_code, fresh.json()["detail"]) == (409, "UNKNOWN_DISPUTE")


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason=(
        "CK-02: a trial proof is read for its signature before the freshness gate, so a forged "
        "proof under a stale beacon is a 409 about the signature, not a 503 [candidate]"
    ),
)
def test_a_stale_beacon_refuses_a_trial_proof_before_its_signature_is_read(
    routes_world: tuple[World, Any, Any],
) -> None:
    world, rnd, job = routes_world
    request = sealed_for(world, rnd, job, "join-proof", world.run_id, 10_000)
    try:
        stale(world, 3)
        valid = request.post(world)
        forged_proof = request.post(world, payload=forged(request))
    finally:
        stale(world, 0)
    control((valid.status_code, valid.json()["detail"]) == (503, PAUSED), valid.text)
    assert (forged_proof.status_code, forged_proof.json()["detail"]) == (503, PAUSED)


def object_put(world: World, data: bytes) -> Any:
    sha = hashlib.sha256(data).hexdigest()
    message = f"hypertrain/object/2|{world.run_id}|{sha}".encode()
    return world.network.client.put(
        world.network.url + f"/objects/{sha}",
        content=data,
        headers={"x-hotkey": hot(0).ss58, "x-object-signature": hot(0).sign(message).hex()},
    )


def stale_object_put(tmp_path: Path) -> None:
    with service_world(tmp_path) as world:
        control(world.network.join(0).status_code == 200, "join")
        control(object_put(world, b"fresh object").status_code == 200, "a fresh put is refused")
        stale(world, 3)
        before = files(world)
        refused = object_put(world, b"stale object")
        assert refused.status_code == 503, refused.text
        assert files(world) == before


def stale_leaves(tmp_path: Path) -> None:
    with service_world(tmp_path) as world:
        activate(world, 4)
        rnd, opened = open_round(world, [0, 1, 2, 3], (1, 2, 10, 11, 12, 40))
        control(opened.status_code == 200, f"open: {opened.text}")
        world.network.push(rnd.d_assign)
        for index in (0, 1):
            control(post_commit(rnd, index, 10_000).status_code == 200, f"commit {index}")
        control(publish_leaves(rnd, 1).status_code == 200, "a fresh publication is refused")
        stale(world, 3)
        before = tables(world)
        refused = publish_leaves(rnd, 0)
        assert refused.status_code == 503, refused.text
        assert touched(before, tables(world)) == []


def stale_export(tmp_path: Path) -> None:
    with service_world(tmp_path) as world:
        identity = world.network.join(0).json()["admission_id"]
        url = f"{world.network.url}/admin/trial/{identity}/export"
        fresh = world.network.client.post(url, headers=world.admin)
        control((fresh.status_code, fresh.json()["detail"]) == (409, "NO_CHALLENGE"), fresh.text)
        stale(world, 3)
        refused = world.network.client.post(url, headers=world.admin)
        assert refused.status_code == 503, refused.text


@pytest.mark.parametrize(
    "runner",
    [
        pytest.param(
            stale_object_put,
            id="object-put",
            marks=pytest.mark.xfail(
                strict=True,
                raises=AssertionError,
                reason=(
                    "CK-02: the object PUT is not behind the freshness gate, so it stores while "
                    "the beacon is stale instead of answering 503 [candidate]"
                ),
            ),
        ),
        pytest.param(
            stale_leaves,
            id="leaves-publication",
            marks=pytest.mark.xfail(
                strict=True,
                raises=AssertionError,
                reason=(
                    "CK-02: the leaf publication is not behind the freshness gate, so it records "
                    "while the beacon is stale instead of answering 503 [candidate]"
                ),
            ),
        ),
        pytest.param(
            stale_export,
            id="trial-export",
            marks=pytest.mark.xfail(
                strict=True,
                raises=AssertionError,
                reason=(
                    "CK-02: the trial export is not behind the freshness gate, so it answers its "
                    "own state while the beacon is stale instead of 503 [candidate]"
                ),
            ),
        ),
    ],
)
def test_a_stale_beacon_refuses_an_object_put_a_leaf_publication_and_a_trial_export(
    tmp_path: Path, runner: Any
) -> None:
    runner(tmp_path)


# --- CK-03: a missed seed round is a 503, not a crash --------------------------------------------


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason=(
        "CK-03: the round that seeds a trial challenge was never pushed and the request dies "
        "with an unhandled BeaconUnavailable (HTTP 500) instead of a retryable 503"
    ),
)
def test_a_challenge_request_for_a_seed_round_that_was_not_pushed_is_a_503(tmp_path: Path) -> None:
    with service_world(tmp_path) as world:
        identity = world.network.join(0).json()["admission_id"]
        seed = world.store._db.execute("SELECT seed_beacon FROM admissions_v2").fetchone()[0]
        world.network.push(seed + 1)  # the seed round itself is skipped
        quiet = TestClient(world.app, raise_server_exceptions=False)
        url = world.network.url + f"/join/{identity}/challenge"
        refused = quiet.get(url)
        # Control: pushing the missing round later lets the very same request through.
        backfill = world.network.client.post(
            "/v1/admin/beacon", json=service().fixture_beacon(seed), headers=world.admin
        )
        control(backfill.status_code == 200, f"backfill: {backfill.text}")
        control(quiet.get(url).status_code == 200, "the challenge is not served after backfill")
        assert refused.status_code == 503, refused.text


# --- SG-04: no signed expiry is bounded ---------------------------------------------------------

FAR = 2**53 - 1


def far_transfer(world: World) -> Any:
    control(transfer_request(world, "e1" * 32, 1, expiry=50).status_code == 200, "near transfer")
    return lambda: transfer_request(world, "e2" * 32, 1, expiry=FAR)


def far_rotation(world: World) -> Any:
    network = world.network
    control(network.join(0).status_code == 200, "join")
    request = RotateRequest(
        hotkey=hot(0).ss58, new_hotkey=service().REFEREE.ss58, operation_id="e3" * 32
    )
    return lambda: network.client.post(
        network.url + "/admission/" + hot(0).ss58 + "/rotate",
        json=sign_rotation(cold(0), world.run_id, request, FAR),
    )


def far_join(world: World) -> Any:
    network = world.network

    def join(index: int, expires: int) -> Any:
        request = sign_join(
            hot(index),
            cold(index),
            run_id=world.run_id,
            request_id=sha256_hex(f"far-{index}".encode()),
            expires_beacon=expires,
            policy_hash=network.manifest.network.admission_policy_hash,
            hardware_hint=HardwareHint(device_name="cpu", device_count=1, driver="cpu"),
        )
        return network.client.post(network.url + "/join", content=canonicalize(request.body()))

    control(join(0, 50).status_code == 200, "near join")
    return lambda: join(1, FAR)


@pytest.mark.parametrize(
    "setup",
    [
        pytest.param(
            far_transfer,
            id="escrow-transfer",
            marks=pytest.mark.xfail(
                strict=True,
                raises=AssertionError,
                reason=(
                    "SG-04: an escrow transfer whose expiry is the largest canonical integer is "
                    "accepted, so a captured request stays replayable for ever [candidate]"
                ),
            ),
        ),
        pytest.param(
            far_rotation,
            id="rotation",
            marks=pytest.mark.xfail(
                strict=True,
                raises=AssertionError,
                reason=(
                    "SG-04: a rotation whose expiry is the largest canonical integer is accepted, "
                    "so a captured request stays replayable for ever [candidate]"
                ),
            ),
        ),
        pytest.param(
            far_join,
            id="join",
            marks=pytest.mark.xfail(
                strict=True,
                raises=AssertionError,
                reason=(
                    "SG-04: a join whose expiry is the largest canonical integer is accepted, "
                    "so a captured request stays replayable for ever [candidate]"
                ),
            ),
        ),
    ],
)
def test_a_signed_request_with_an_absurd_expiry_is_refused(tmp_path: Path, setup: Any) -> None:
    with service_world(tmp_path) as world:
        send = setup(world)  # control: the same request with a near expiry went through
        before = tables(world)
        response = send()
        assert not response.is_success, response.text
        assert touched(before, tables(world)) == []
