"""A v2 admission network whose run is hosted by a CHILD `ChallengeStore`.

A challenge state directory keeps its first run in the root store; every later run lives in an
isolated sub-store (`ChallengeStore._create_child`) with its own SQLite connection, its own
`threading.RLock` and its own object store, and `root.push_beacon` forwards each round to it. The
real S13 run on s100 lived in such a child. The `network` fixture of test_service_network_v2.py
always lands in the root, so a test that must hold for both layouts also asks for `child_network`:

    from .child_run_fixture_v2 import child_network, pending_reference  # noqa: F401

The fixture builds the ordinary `network` first (its run stays in the root as the occupant), then
creates a second run through `POST /v2/admin/runs`, which the root hosts in a child. Nothing else
differs: same policies, dataset, auditors, genesis time and timing.

Three things differ from the root layout and are easy to get wrong:

- `child_network.store` is the CHILD that hosts the run (`Network.store` is the root);
  `child_network.root` is the root store.
- `ChallengeStore._open_child` copies the root's `_role_launch` when the child is created, so a
  test fakes the executor on `child_network.store`, never on the root.
- The child's clock follows the root's until a test assigns one; `child_network.push()` assigns
  the clock of `child_network.store`, as `Network.push` does for the root store.
"""

from __future__ import annotations

from collections.abc import Iterator
from dataclasses import dataclass, field
from pathlib import Path
from types import SimpleNamespace
from typing import Any

import pytest

from hypertrain.challenge.store import ChallengeStore
from hypertrain.ledger.escrow_v2 import authority_message, genesis_origin_id
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages_v2 import (
    EconomicsPolicyV2,
    EscrowLock,
    OriginAllocation,
    RunManifestV2,
)
from hypertrain.protocol.messages_v2 import (
    TestGenesis as Genesis,
)

from .test_service_network_v2 import COLD, COORD, HOT, OWNER, Network, admin, network


@dataclass
class ChildNetwork(Network):
    """A `Network` whose run is hosted by a child store; see the module docstring."""

    occupant: RunManifestV2 = field(kw_only=True)

    @property
    def root(self) -> ChallengeStore:
        """The root store, which hosts only the occupant run."""
        return self.client.app.state.store

    @property
    def store(self) -> ChallengeStore:
        """The CHILD store hosting this run (`Network.store` is the root)."""
        return self.root.for_run(self.manifest.run_id())


def hosted_in_child(occupant: Network) -> ChildNetwork:
    """Create a second, fully set up run next to `occupant` (which holds the root store)."""
    body = occupant.manifest.body()
    # A manifest needs all three capabilities exactly once and reads no order from them, so a
    # reversed list is a distinct run (distinct run_id) with the timing of the occupant.
    body["network"]["capabilities"].reverse()
    manifest = RunManifestV2.model_validate(body)
    run = ChildNetwork(
        occupant.client, manifest, occupant.setup, [], occupant.now, occupant=occupant.manifest
    )
    response = run.client.post(
        "/v2/admin/runs",
        content=canonicalize(run.signed(OWNER, "RunManifestV2", manifest)),
        headers=admin(),
    )
    assert response.status_code == 201, response.text
    child = run.store
    assert child is not run.root, "the root already hosts the occupant: the new run must be a child"
    # The child has its own object store: the dataset objects are put there, not in the root's.
    data = occupant.setup
    response = run.client.put(
        run.url + "/admin/dataset",
        json={
            "samples_hash": child.objects.put(data.rows.tobytes()),
            "proofs_hash": child.objects.put(
                canonicalize([[p.hex() for p in data.tree.proof(i)] for i in range(len(data.rows))])
            ),
        },
        headers=admin(),
    )
    assert response.status_code == 200, response.text
    allocation = EconomicsPolicyV2.model_validate_json(
        child.objects.get(manifest.network.economics_policy_hash)
    ).genesis_allocation_hash
    origins = [
        OriginAllocation(
            origin_id=genesis_origin_id(manifest.run_id(), allocation, i),
            owner=key.ss58,
            units=10000,
            mature_at=1,
        )
        for i, key in enumerate(COLD)
    ]
    genesis = Genesis(
        run_id=manifest.run_id(),
        allocation_hash=allocation,
        total_units=40000,
        origins=origins,
        authority_sig="0" * 128,
    )
    genesis = genesis.model_copy(
        update={"authority_sig": COORD.sign(authority_message(genesis)).hex()}
    )
    response = run.client.post(
        run.url + "/admin/genesis",
        content=canonicalize(run.signed(COORD, "TestGenesis", genesis)),
        headers=admin(),
    )
    assert response.status_code == 200, response.text
    run.origin_ids = [o.origin_id for o in origins]
    return run


@pytest.fixture
def child_network(tmp_path: Path) -> Iterator[ChildNetwork]:
    # Driven through the fixture function itself, so a test module needs no `network` of its own.
    generator = network.__wrapped__(tmp_path, SimpleNamespace(param=None))
    occupant = next(generator)
    try:
        yield hosted_in_child(occupant)
    finally:
        generator.close()


def pending_reference(network: Any, index: int = 0, *, http: bool = False) -> str:
    """An admission with an accepted challenge, whose reference is the next thing to run.

    Works for any layout: it drives the services of the store that hosts the run
    (`network.store.for_run(run_id)`: the root for `Network`, the child for `ChildNetwork`).
    The challenge is opened on the service, or through the owner's signed route if `http`."""
    run_id = network.manifest.run_id()
    assert network.join(index).status_code == 200
    hosted = network.store.for_run(run_id)
    _, admission, _ = hosted._services(run_id)
    admission_id = hosted._db.execute(
        "SELECT admission_id FROM admissions_v2 WHERE hotkey=?", (HOT[index].ss58,)
    ).fetchone()[0]
    lock = EscrowLock(
        operation_id=sha256_hex(f"pending-reference-{index}".encode()),
        owner=COLD[index].ss58,
        units=1000,
        origin_ids=[network.origin_ids[index]],
        admission_id=admission_id,
        dispute_id=None,
        kind="LOCK_ADMISSION",
    )
    admission.lock(canonicalize(network.signed(COLD[index], "EscrowLock", lock)), now=network.now)
    network.push(network.now + 1)
    if http:
        response = network.challenge(admission_id)
        assert response.status_code == 200, response.text
    else:
        admission.challenge(admission_id, now=network.now)
    admission.backend = "cuda"  # the reference goes to the role launch, as for a CUDA run
    return str(admission_id)
