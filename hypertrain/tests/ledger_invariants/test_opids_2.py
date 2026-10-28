"""Operation ids over HTTP: signed escrow routes, the beacon route and the miner's own lock command.

Every world is the v2 service of the lot with the clock frozen, so the run id and every id the
service derives are the same in the control world and in the world a bystander writes to.
"""

from __future__ import annotations

import contextlib
import threading
from pathlib import Path
from typing import Any

import httpx
import pytest

from hypertrain.ledger.escrow_v2 import EscrowError, EscrowV2
from hypertrain.miner.coldkey_ops import escrow_lock
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages_v2 import EscrowLock, EscrowTransfer

from .support import SERVICE_FIXTURE, World, load_fixture_module, service_world
from .support_opids import StepRefused, client_model, contest_lock, fresh_id, refused, self_transfer

# The outbox refuses a second body under a queued lock id. A ledger that reads the replay before
# the lock is queued (`require_lock_capacity`, ADR 0014 part 4) refuses it first with its own code.
LOCK_CONFLICTS = ("OUTBOX_CONFLICT",) + (
    ("OPERATION_CONFLICT",) if hasattr(EscrowV2, "require_lock_capacity") else ()
)


def escrow_of(world: World) -> Any:
    return world.store._services(world.run_id)[0]


def signed_post(world: World, route: str, key: Keypair, kind: str, body: Any) -> Any:
    envelope = world.network.signed(key, kind, body)
    return world.client.post(world.network.url + "/escrow/" + route, json=envelope)


def client_event_over_http(
    world: World, kind: str, key: Keypair, operation_id: str, origin: str
) -> Any:
    """What a funded coldkey sends to the public escrow routes, under an id of its choice."""
    if kind == "transfer":
        event = client_model(lambda: self_transfer(key.ss58, operation_id, origin))
        return signed_post(world, "transfer", key, "EscrowTransfer", event)
    event = client_model(lambda: contest_lock(key.ss58, operation_id, origin))
    return signed_post(world, "lock", key, "EscrowLock", event)


def seed_pending_origin(world: World, origin: str, owner: str, units: int, mature_at: int) -> None:
    """A bootstrap-like origin that vests at `mature_at`: the genesis origins of the fixture all
    vest at the first beacon, so a later maturity needs one more origin."""
    escrow = escrow_of(world)
    with world.store._lock:
        escrow.db.execute(
            "INSERT INTO escrow_origins VALUES(?,?,?,NULL,NULL)", (origin, units, mature_at)
        )
        escrow.db.execute(
            "INSERT INTO escrow_units VALUES(?,?,'reward_pending','',?)", (origin, owner, units)
        )


def push_round(world: World, number: int) -> None:
    """The beacon route; a refusal is the service refusing a step it takes for its own account."""
    try:
        world.network.push(number)
    except AssertionError as error:
        raise StepRefused(f"the beacon route refused round {number}: {error}") from error


def maturity_ids(world: World) -> dict[str, str]:
    """operation id of each MATURE event, by the origins it vested (its request)."""
    escrow = escrow_of(world)
    with world.store._lock:
        rows = escrow.db.execute(
            "SELECT operation_id, request FROM escrow_events WHERE kind='MATURE'"
        )
        return {row[1]: row[0] for row in rows}


@pytest.mark.parametrize("kind", ["transfer", "lock"])
@pytest.mark.xfail(
    strict=True,
    raises=StepRefused,
    reason="ESC-04+BND-24+SG-11: a client event cannot take the id the service derives for the "
    "maturity of an origin, so the beacon route always matures what is due "
    "[fix-lot: escrow-relay-fixes ADR0014-P1]",
)
def test_a_client_event_never_blocks_the_maturity_id_over_http(tmp_path: Path, kind: str) -> None:
    cold = load_fixture_module(*SERVICE_FIXTURE).COLD
    pending = fresh_id("pending-origin")
    request = canonicalize([pending]).decode()

    # Given: the control run of the beacon at round 5 matures the origin and shows its id.
    with service_world(tmp_path / "control") as world:
        world.network.push(2)
        seed_pending_origin(world, pending, cold[0].ss58, 500, mature_at=5)
        world.network.push(5)
        maturity_id = maturity_ids(world)[request]
        effect = escrow_of(world).balances(cold[0].ss58)
    assert (effect.available, effect.reward_pending) == (10500, 0)

    # When: a funded coldkey signs an event with that id on the public route, before round 5.
    with service_world(tmp_path / "contended") as world:
        world.network.push(2)
        seed_pending_origin(world, pending, cold[0].ss58, 500, mature_at=5)
        origin = world.network.origin_ids[1]
        neutral = client_event_over_http(world, kind, cold[1], fresh_id("neutral"), origin)
        assert neutral.status_code == 200, neutral.text  # control: the route takes its events
        refused(lambda: client_event_over_http(world, kind, cold[1], maturity_id, origin))

        # Then: the beacon route still answers 200 and the origin is available as in the control.
        push_round(world, 5)
        assert escrow_of(world).balances(cold[0].ss58) == effect


def owner_keyfile(directory: Path, key_seed: bytes) -> Path:
    path = directory / "owner.key"
    path.write_text(key_seed.hex())
    path.chmod(0o600)
    return path


@pytest.mark.parametrize("kind", ["transfer", "lock"])
@pytest.mark.xfail(
    strict=True,
    raises=StepRefused,
    reason="ESC-05+LCK-13: an event of another key cannot make the admission lock of the owner "
    "fail, although the id of the lock command is a function of public values; ADR 0014 "
    "part 1.10 (ESC-D5) leaves the client key space shared until Forge decides "
    "[fix-lot: decision (product choice pending)]",
)
def test_a_third_party_event_never_makes_the_owner_lock_command_fail(
    tmp_path: Path, kind: str
) -> None:
    service = load_fixture_module(*SERVICE_FIXTURE)
    hot, cold, bystander = service.HOT[0], service.COLD[0], service.COLD[1]
    keyfile = owner_keyfile(tmp_path, bytes([90]) * 32)
    assert Keypair(bytes([90]) * 32).ss58 == cold.ss58

    def lock_command(world: World, *, dry_run: bool) -> Any:
        return escrow_lock(
            world.client,
            "http://testserver",
            world.run_id,
            keyfile,
            hot.ss58,
            1000,
            [world.network.origin_ids[0]],
            dry_run=dry_run,
        )

    # Given: the control, where the owner runs its lock command and nobody else signs anything.
    with service_world(tmp_path / "control") as world:
        assert world.network.join().status_code == 200
        world.network.push(2)
        command_id = lock_command(world, dry_run=True)["operation_id"]
        served = lock_command(world, dry_run=False)
    assert served["locked_units"] == 1000

    # When: another funded coldkey signs an event with the id of that command before the owner.
    with service_world(tmp_path / "contended") as world:
        assert world.network.join().status_code == 200
        world.network.push(2)
        origin = world.network.origin_ids[1]
        neutral = client_event_over_http(world, kind, bystander, fresh_id("neutral"), origin)
        assert neutral.status_code == 200, neutral.text
        client_event_over_http(world, kind, bystander, command_id, origin)

        # Then: the owner's command succeeds as in the control. The event hash chains every event
        # before it, so it differs between the two worlds by construction and is not compared.
        try:
            outcome = lock_command(world, dry_run=False)
        except httpx.HTTPStatusError as error:
            raise StepRefused(
                f"the lock command was refused: {error.response.status_code} {error.response.text}"
            ) from error
        assert {**outcome, "event_hash": None} == {**served, "event_hash": None}


# --- SG-09: one signed operation id, one body ----------------------------------------------------


def transfer_of(owner: Keypair, recipient: Keypair, operation_id: str, origin: str, units: int):
    return EscrowTransfer(
        operation_id=operation_id,
        owner=owner.ss58,
        recipient=recipient.ss58,
        units=units,
        origin_ids=[origin],
        admission_id=None,
        dispute_id=None,
    )


@pytest.mark.parametrize("kind", ["transfer", "lock"])
def test_a_signed_operation_sent_twice_is_applied_once_and_another_body_is_refused(
    tmp_path: Path, kind: str
) -> None:
    cold = load_fixture_module(*SERVICE_FIXTURE).COLD
    with service_world(tmp_path) as world:
        world.network.push(2)
        origin = world.network.origin_ids[0]
        route, message = (
            ("transfer", "EscrowTransfer") if kind == "transfer" else ("lock", "EscrowLock")
        )

        def operation(units: int):
            if kind == "transfer":
                return transfer_of(cold[0], cold[1], fresh_id("sg-09"), origin, units)
            return contest_lock(cold[0].ss58, fresh_id("sg-09"), origin, units=units)

        def post(units: int):
            return world.client.post(
                world.network.url + "/escrow/" + route,
                json=world.network.signed(cold[0], message, operation(units)),
            )

        # Given: the operation is applied once.
        first = post(10)
        assert first.status_code == 200, first.text
        stored, applied = world.db_digest(), escrow_of(world).balances(cold[0].ss58)
        assert applied.available == 9990

        # When: the same body is sent again, signed again (a fresh signature of the same body).
        again = post(10)

        # Then: the answer is the first one and the units moved once.
        assert (again.status_code, again.json()) == (200, first.json())
        assert world.db_digest() == stored
        assert escrow_of(world).balances(cold[0].ss58) == applied

        # And another body under the same id is refused by the signed-nonce record, state untouched.
        other_body = post(20)
        assert (other_body.status_code, other_body.json()["detail"]) == (
            409,
            "conflicting signed operation nonce",
        )
        assert world.db_digest() == stored


# --- ES-08: a refused escrow request leaves nothing behind ---------------------------------------


def test_a_refused_escrow_request_leaves_no_reservation_no_event_and_no_burnt_id(
    tmp_path: Path,
) -> None:
    cold = load_fixture_module(*SERVICE_FIXTURE).COLD
    with service_world(tmp_path) as world:
        world.network.push(2)
        origin = world.network.origin_ids[0]
        url = world.network.url + "/escrow/transfer"
        applied = transfer_of(cold[0], cold[1], fresh_id("es-08-applied"), origin, 10)
        assert (
            world.client.post(
                url, json=world.network.signed(cold[0], "EscrowTransfer", applied)
            ).status_code
            == 200
        )
        stored = world.db_digest()

        # Given: requests the service must refuse, each for its own reason.
        unfunded_id = fresh_id("es-08-unfunded")
        stranger = transfer_of(cold[1], cold[2], fresh_id("es-08-stranger"), origin, 5)
        unknown_origin = transfer_of(cold[0], cold[1], fresh_id("es-08-origin"), fresh_id("x"), 5)
        refusals = {
            "unfunded": (
                transfer_of(cold[0], cold[1], unfunded_id, origin, 10**6),
                cold[0],
                "UNFUNDED_OR_UNKNOWN_ORIGIN",
            ),
            "signed by another key": (stranger, cold[0], "OWNER_AUTHORITY"),
            "unknown origin": (unknown_origin, cold[0], "UNFUNDED_OR_UNKNOWN_ORIGIN"),
            "another body under a used id": (
                applied.model_copy(update={"units": 11}),
                cold[0],
                "conflicting signed operation nonce",
            ),
        }

        # When / Then: each is a 409 with its reason and the whole store is exactly as before.
        for name, (operation, key, reason) in refusals.items():
            response = world.client.post(
                url, json=world.network.signed(key, "EscrowTransfer", operation)
            )
            assert (name, response.status_code, response.json()["detail"]) == (name, 409, reason)
            assert world.db_digest() == stored, name

        # And a refused id is not burnt: the funded request that reuses it is applied.
        funded = transfer_of(cold[0], cold[1], unfunded_id, origin, 10)
        retry = world.client.post(url, json=world.network.signed(cold[0], "EscrowTransfer", funded))
        assert retry.status_code == 200, retry.text
        assert escrow_of(world).balances(cold[1].ss58).available == 10020


# --- LCK-12: lock creation is exactly once and atomic with its receipt ---------------------------


def admission_lock(owner: Keypair, admission_id: str, operation_id: str, origin: str, units: int):
    return EscrowLock(
        operation_id=operation_id,
        owner=owner.ss58,
        units=units,
        origin_ids=[origin],
        admission_id=admission_id,
        dispute_id=None,
        kind="LOCK_ADMISSION",
    )


def test_an_admission_lock_is_applied_once_and_a_refused_one_leaves_no_outbox_row(
    tmp_path: Path,
) -> None:
    cold = load_fixture_module(*SERVICE_FIXTURE).COLD
    with service_world(tmp_path) as world:
        admission_id = world.network.join().json()["admission_id"]
        world.network.push(2)
        origin = world.network.origin_ids[0]
        url = world.network.url + "/escrow/lock"
        lock = admission_lock(cold[0], admission_id, fresh_id("lck-12"), origin, 1000)

        def post(operation: EscrowLock) -> Any:
            return world.client.post(
                url, json=world.network.signed(cold[0], "EscrowLock", operation)
            )

        # Given: the lock is applied and acknowledged, so nothing is pending for the admission.
        first = post(lock)
        assert first.status_code == 200, first.text
        stored = world.db_digest()
        assert world.count("admission_outbox", "receipt IS NOT NULL") == 1
        assert not world.store._services(world.run_id)[1].store.pending_outbox(admission_id)

        # When: the same lock comes again, then a different body under its id, then a lock the
        # ledger cannot fund.
        again = post(lock)
        other_body = post(lock.model_copy(update={"units": 2000}))
        unfunded = post(admission_lock(cold[0], admission_id, fresh_id("lck-12-b"), origin, 10**6))

        # Then: one lock was applied; the replay returned its event; the refusals added nothing,
        # not even an unacknowledged outbox row that would block the admission.
        assert (again.status_code, again.json()) == (200, first.json())
        assert other_body.status_code == 409
        assert other_body.json()["detail"] in LOCK_CONFLICTS
        assert (unfunded.status_code, unfunded.json()["detail"]) == (
            409,
            "UNFUNDED_OR_UNKNOWN_ORIGIN",
        )
        assert world.db_digest() == stored
        assert world.count("escrow_events", "kind='LOCK_ADMISSION'") == 1
        assert escrow_of(world).locked(admission_id, cold[0].ss58)[0] == 1000


def test_an_unacknowledged_outbox_row_survives_a_refusal_only_outside_the_services_transaction(
    tmp_path: Path,
) -> None:
    cold = load_fixture_module(*SERVICE_FIXTURE).COLD
    with service_world(tmp_path) as world:
        admission_id = world.network.join().json()["admission_id"]
        world.network.push(2)
        escrow, admission = world.store._services(world.run_id)[:2]

        def unfunded_lock(label: str) -> bytes:
            operation = admission_lock(
                cold[0], admission_id, fresh_id(label), world.network.origin_ids[0], 10**6
            )
            return canonicalize(world.network.signed(cold[0], "EscrowLock", operation))

        # When: the ledger refuses a lock it cannot fund, inside the transaction the service opens
        # for every request (the stand-in for admission_v2), then outside any transaction.
        with pytest.raises(EscrowError, match="UNFUNDED_OR_UNKNOWN_ORIGIN"), world.store._tx():
            admission.lock(unfunded_lock("lck-12-inside"), now=2)
        inside = world.count("admission_outbox")
        with pytest.raises(EscrowError, match="UNFUNDED_OR_UNKNOWN_ORIGIN"):
            admission.lock(unfunded_lock("lck-12-outside"), now=2)

        # Then: inside, the outbox row is rolled back with the request; outside, the row is
        # committed before the ledger refuses and the admission is left with a pending lock.
        assert inside == 0
        assert world.count("admission_outbox", "receipt IS NULL") == 1
        assert admission.store.pending_outbox(admission_id)
        assert escrow.locked(admission_id, cold[0].ss58)[0] == 0


# --- ESC-18: the engine and the store serialize on one lock --------------------------------------


@pytest.mark.parametrize("shared", [True, False])
def test_the_ledger_waits_for_a_store_transaction_only_while_they_share_the_lock(
    tmp_path: Path, shared: bool
) -> None:
    cold = load_fixture_module(*SERVICE_FIXTURE).COLD
    with service_world(tmp_path) as world:
        world.network.push(2)
        escrow, store = escrow_of(world), world.store
        if not shared:
            escrow.mutex = threading.RLock()  # stand-in: a user of the connection outside the lock
        operation = self_transfer(cold[1].ss58, fresh_id("mutex"), world.network.origin_ids[1])
        holding, release, applied = threading.Event(), threading.Event(), threading.Event()

        def holder() -> None:
            with contextlib.suppress(RuntimeError), store._tx():
                store._db.execute("INSERT OR REPLACE INTO meta VALUES('li-holder', 'open')")
                holding.set()
                release.wait(timeout=30)
                raise RuntimeError("the transaction of the store fails")

        def writer() -> None:
            escrow.transfer(operation, signer=cold[1].ss58)
            applied.set()

        # Given: the store holds a transaction open on the shared connection.
        threads = [threading.Thread(target=holder), threading.Thread(target=writer)]
        threads[0].start()
        assert holding.wait(timeout=30)

        # When: the ledger is asked for an operation meanwhile, then the store's transaction fails.
        threads[1].start()
        finished_meanwhile = applied.wait(timeout=1 if shared else 30)
        release.set()
        for thread in threads:
            thread.join(timeout=30)
            assert not thread.is_alive()

        # Then: sharing the lock, the operation waited and is durable; otherwise it joined the
        # failing transaction, was answered, and was rolled back with it.
        assert applied.is_set()
        assert finished_meanwhile is (not shared)
        assert world.count("escrow_events", "operation_id=?", (operation.operation_id,)) == int(
            shared
        )
        assert world.count("meta", "key=?", ("li-holder",)) == 0
