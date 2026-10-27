"""Builders private to the gap tests of the admission-lock group. Not collected: no test_ prefix.

They sit on top of `support`, `support_locks` and `support_weights`: a service world with admitted
miners, a rotation posted through the public route, a store reopened on the state directory of a
running one, and a recorder for the calls a beacon push makes.
"""

from __future__ import annotations

import json
import sqlite3
from collections.abc import Callable, Iterator
from contextlib import contextmanager
from pathlib import Path
from typing import Any

import httpx

from hypertrain.challenge.store import ChallengeStore
from hypertrain.miner.admission import sign_rotation
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages_v2 import ArtifactRef, RotateRequest, WorkProof

from .support import SERVICE_FIXTURE, World, load_fixture_module
from .support_admission_http import precondition
from .support_locks import STAKE, Miner, Rig, ident
from .support_weights import parse


def service() -> Any:
    return load_fixture_module(*SERVICE_FIXTURE)


def spare_hotkey() -> Keypair:
    """A hotkey the fake master lists, that never joined and that no protocol role pins."""
    return service().HOT[3]


def rotate_admission(
    n: Any, old: Keypair, cold: Keypair, new_hot: Keypair, label: str = "gap-rotation"
) -> httpx.Response:
    """The coldkey moves the admission of hotkey `old` to `new_hot`, through the public route."""
    request = RotateRequest(
        hotkey=old.ss58, new_hotkey=new_hot.ss58, operation_id=sha256_hex(label.encode())
    )
    return n.client.post(
        f"{n.url}/admission/{old.ss58}/rotate",
        json=sign_rotation(cold, n.manifest.run_id(), request, 10_000),
    )


def rotate(rig: Rig, miner: Miner, new_hot: Keypair, label: str = "gap-rotation") -> httpx.Response:
    """The coldkey of `miner` moves its admission to `new_hot`, through the public route."""
    return rotate_admission(rig.net, miner.hot, miner.cold, new_hot, label)


def holder(rig: Rig, miner: Miner) -> str:
    """The hotkey that holds the admission of `miner` now."""
    with rig.world.store._lock:
        return str(rig.admission.store.by_id(miner.admission_id).hotkey)


def weights_at_now(world: World, epoch: int = 0) -> dict[str, Any]:
    """The answer of the internal get_weights route for the drand round the store stands at."""
    response = world.get_weights(epoch, epoch_at=int(world.store.clock()))
    precondition(response.status_code == 200, f"get_weights answered {response.text}")
    return parse(response.content)


class Calls:
    """Records the positional arguments of every call to a method of one object, and still runs
    it. A call that raises is recorded too."""

    def __init__(self, owner: object, name: str) -> None:
        self.calls: list[tuple[Any, ...]] = []
        original: Callable[..., Any] = getattr(owner, name)

        def recording(*args: Any, **kwargs: Any) -> Any:
            self.calls.append(args)
            return original(*args, **kwargs)

        setattr(owner, name, recording)


@contextmanager
def reopened(world: World) -> Iterator[ChallengeStore]:
    """The store of `world` closed and opened again on the same state directory, as a process
    restart leaves it: the v2 services of no run are built until something asks for them."""
    original = world.store
    original.close_v2_notifications()
    original._db.close()
    store = ChallengeStore(
        original.state_dir,
        original.params,
        service().COORD,
        service().OWNER.ss58,
        original.verify_beacon,
        original.objects,
    )
    store.clock = original.clock
    try:
        yield store
    finally:
        store.close_v2_notifications()
        store._db.close()


def buckets_of(db: sqlite3.Connection, origin: str) -> dict[str, int]:
    """Bucket -> units of one reward origin, read straight from the ledger table."""
    return {
        row[0]: int(row[1])
        for row in db.execute(
            "SELECT bucket,SUM(units) FROM escrow_units WHERE origin=? GROUP BY bucket", (origin,)
        )
    }


def beacon(number: int) -> dict[str, Any]:
    """The beacon payload of drand round `number` of the service fixture."""
    payload: dict[str, Any] = json.loads(json.dumps(service().fixture_beacon(number)))
    return payload


def lock_is_free(store: ChallengeStore) -> bool:
    """Whether another thread could take the store lock right now (never waits for it)."""
    free = store._lock.acquire(blocking=False)
    if free:
        store._lock.release()
    return free


def proof_request(rig: Rig, miner: Miner) -> dict[str, Any]:
    """The body a miner posts to /join/proof: a signed WorkProof and a signed work screen.

    Their content is not what the tests below are about: the route authenticates the signer and
    asks for the reference before it reads either of them.
    """
    proof = WorkProof(
        admission_id=miner.admission_id,
        challenge_hash=ident("gap-challenge"),
        leaves_root=ident("gap-leaves"),
        delta_hash=ident("gap-delta"),
        artifact_refs=[ArtifactRef(sha256=ident("gap-artifact"), size=1)],
    )
    return {
        "proof": rig.net.signed(miner.hot, "WorkProof", proof),
        "screen": rig.net.signed(miner.hot, "WorkScreenV2", rig.screen(miner)),
    }


def job_headers(rig: Rig, miner: Miner) -> dict[str, str]:
    """The owner's signed header pair of the trial /job route."""
    message = f"hypertrain/trial-job/2|{rig.run_id}|{miner.admission_id}".encode()
    return {"x-hotkey": miner.hotkey, "x-trial-signature": miner.hot.sign(message).hex()}


def funded_cohort(rig: Rig) -> list[Miner]:
    """Four ACTIVE miners, each holding exactly the required stake."""
    return [rig.admit(index) for index in range(4)]


def reduce_stake(rig: Rig, miner: Miner, units: int = 1) -> None:
    """Test-only ledger edit: `units` of the miner's admission stake move back to its free
    balance on the same origin, so the ledger still conserves every origin and only the stake
    the admission can count is short."""
    origin = rig.net.origin_ids[miner.index]
    with rig.world.store._tx():
        db = rig.escrow.db
        locked = db.execute(
            "UPDATE escrow_units SET units=units-? WHERE origin=? AND owner=? "
            "AND bucket='admission_locked'",
            (units, origin, miner.coldkey),
        )
        freed = db.execute(
            "UPDATE escrow_units SET units=units+? WHERE origin=? AND owner=? "
            "AND bucket='available'",
            (units, origin, miner.coldkey),
        )
    precondition((locked.rowcount, freed.rowcount) == (1, 1), "the stake rows were not found")
    held = rig.escrow.locked(miner.admission_id, miner.coldkey)[0]
    precondition(held == STAKE - units, f"the admission holds {held} locked units")
    precondition(rig.escrow.balances().conserved(), "the ledger edit broke conservation")


def snapshot(lab: Any) -> tuple[tuple[object, ...], ...]:
    """Every admission table of a bare Admission service as one comparable value."""
    db = lab.a.store.db
    return tuple(
        tuple(tuple(row) for row in db.execute(f"SELECT * FROM {table} ORDER BY 1,2"))  # noqa: S608
        for table in (
            "admissions_v2",
            "admission_history",
            "admission_transitions",
            "admission_pending",
            "admission_outbox",
        )
    )


def poison_shadow_origin(n: Any, body: dict[str, Any], fault: str) -> None:
    """Make the original custody or identity of the credited shadow origin stop validating."""
    svc = service()
    if fault == "the shadow-custody record is missing":
        n.store._db.execute("DELETE FROM records_v2 WHERE kind='shadow-custody'")
    elif fault == "a retained publication changed":
        reference = n.store._record_v2(
            n.manifest.run_id(),
            "shadow-execution",
            body["auditor_receipt"]["body"]["reference_operation_hash"],
        )
        (Path(reference["directory"]) / "rank-0/delta.bin").write_bytes(b"moved or edited later")
    elif fault == "the admission was rotated to a spare hotkey":
        rotated = rotate_admission(n, svc.HOT[0], svc.COLD[0], spare_hotkey())
        precondition(
            rotated.status_code == 200 or 400 <= rotated.status_code < 500,
            f"the rotation was neither accepted nor refused: {rotated.status_code}",
        )
