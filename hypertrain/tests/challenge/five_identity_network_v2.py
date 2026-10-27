"""Five (or `size`) ACTIVE identities on the service-network fixture, without the 12-trial ladder.

`test_service_network_v2_e2e.py::test_four_actual_funded_identities_graduate_shadow_only` covers
the real ladder (admission, reference, proof, finalization, twelve times per identity, minutes).
A test that needs an activated roster and is about something else (a round, a dispute) calls
`activate_identities`, which joins and funds through the real routes and then sets the state the
ladder would have left with one UPDATE. Nothing here reads or writes a shared cache.
"""

from __future__ import annotations

from collections.abc import Iterator
from types import SimpleNamespace
from typing import Any

import pytest

from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages_v2 import (
    EscrowLock,
    RankWorkResult,
    RunManifestV2,
    WorkScreenV2,
)


def activate_identities(
    network: Any, fixtures: Any, indexes: range = range(5), *, units: int = 1000
) -> list[str]:
    """Join, fund and activate `fixtures.HOT[i]` for each index; return the admission ids."""
    spec = network.manifest.training.reference_spec
    identities = []
    for i in indexes:
        if i:
            network.push(network.now + 101)
        response = network.join(i)
        assert response.status_code == 200, response.text
        identity = response.json()["admission_id"]
        lock = EscrowLock(
            operation_id=f"{i + 1:064x}",
            owner=fixtures.COLD[i].ss58,
            units=units,
            origin_ids=[network.origin_ids[i]],
            admission_id=identity,
            dispute_id=None,
            kind="LOCK_ADMISSION",
        )
        response = network.client.post(
            network.url + "/escrow/lock", json=network.signed(fixtures.COLD[i], "EscrowLock", lock)
        )
        assert response.status_code == 200, response.text
        screen = WorkScreenV2(
            evidence_kind="work-proof-v1",
            admission_id=identity,
            nonce=sha256_hex(b"nonce"),
            policy_hash=network.manifest.network.admission_policy_hash,
            image_digest=spec.image_digest,
            layout=spec.layout,
            challenge_hash=sha256_hex(b"challenge"),
            rank_results=[
                RankWorkResult(
                    rank=rank,
                    fingerprint=sha256_hex(b"rank"),
                    elapsed_ns=1,
                    allocated_bytes=1,
                    reserved_bytes=1,
                    total_bytes=1 << 30,
                )
                for rank in range(spec.layout.n_gpus)
            ],
            artifact_hashes=[sha256_hex(b"artifact")],
        )
        with network.store._tx():
            network.store._db.execute(
                "UPDATE admissions_v2 SET state='ACTIVE',clean_count=12,screen_json=?,"
                "screen_until=1000000000 WHERE admission_id=?",
                (screen.model_dump_json(), identity),
            )
        identities.append(identity)
    return identities


def push_through(network: Any, number: int) -> None:
    """Push every beacon round up to `number`, one by one, as the real pusher does.

    `network.push(n)` jumps straight to `n` and sets the clock to it: the master reads a jump as a
    pause of its beacon (ADR 0011 Part 8), which is never a miner's silence. A test that needs a
    silence to count walks the rounds.
    """
    for rnd in range(network.now + 1, number + 1):
        network.push(rnd)


def push_late(network: Any, fixtures: Any, number: int, *, wall: int) -> None:
    """Deliver beacon round `number` while the wall clock is at round `wall`: the pusher is late,
    and the master refused its callers meanwhile."""
    network.now = number
    genesis = network.manifest.training.beacon.genesis_time
    network.store.clock = lambda: genesis + (wall - 1) * 3
    response = network.client.post(
        "/v1/admin/beacon", json=fixtures.fixture_beacon(number), headers=fixtures.admin()
    )
    assert response.status_code == 200, response.text


def five_identity_network(tmp_path_factory: Any, fixtures: Any, *, size: int = 5) -> Iterator[Any]:
    """Body of a module-scoped fixture: the fixtures module with `size` keys, all ACTIVE.

    Carry state policy and a genesis that covers `size` coldkeys, as `active_network` in
    `tests/e2e/test_rollback_service_v2.py` sets them up, so round 1 can follow round 0.
    """
    patch = pytest.MonkeyPatch()
    patch.setattr(fixtures, "HOT", [Keypair(bytes([80 + i]) * 32) for i in range(size)])
    patch.setattr(fixtures, "COLD", [Keypair(bytes([90 + i]) * 32) for i in range(size)])
    original_setup = fixtures.fixture.setup
    original_genesis = fixtures.Genesis

    def carry_setup() -> Any:
        setup = original_setup()
        body = setup.manifest.body()
        body["training"]["inner"].update(state_policy="carry", rewarmup_steps=0)
        return fixtures.fixture.Setup(
            RunManifestV2.model_validate(body),
            setup.policy,
            setup.admission_policy,
            setup.rows,
            setup.tree,
        )

    def full_genesis(**fields: Any) -> Any:
        fields["total_units"] = sum(origin.units for origin in fields["origins"])
        return original_genesis(**fields)

    patch.setattr(fixtures.fixture, "setup", carry_setup)
    patch.setattr(fixtures, "Genesis", full_genesis)
    generator = fixtures.network.__wrapped__(
        tmp_path_factory.mktemp("five-identities"), SimpleNamespace(param=None)
    )
    network = next(generator)
    try:
        activate_identities(network, fixtures, range(size))
        yield network
    finally:
        generator.close()
        patch.undo()
