"""Builders of the bootstrap gap tests (D39, group bootstrap). Not collected: no test_ prefix.

The small service helpers repeat those of `test_opids_2.py` on purpose: a test module is not a
support module, and the gap tests must keep working when a sibling test file is reorganised. The
senders of client events and the control that a squat reached the ledger are the shared ones of
`support_opids.py`.
"""

from __future__ import annotations

import sqlite3
from collections.abc import Callable, Iterator
from dataclasses import dataclass
from types import SimpleNamespace
from typing import Any

import pytest

from env_pin_fixtures import honest_env
from hypertrain.challenge.store import ChallengeError
from hypertrain.ledger.escrow_v2 import EscrowError
from hypertrain.protocol.envelope_v2 import seal
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages import Receipt
from hypertrain.protocol.messages_v2 import (
    EconomicsPolicyV2,
    EscrowLock,
    RankWorkResult,
    RunManifestV2,
    WorkScreenV2,
)

from .support import SERVICE_FIXTURE, World, load_fixture_module
from .support_bounds import Creation, host_attestation
from .support_credit import ROLLBACK_FIXTURE, AuditedRound
from .support_opids import StepRefused

# --- the beacon route and the ids the service derives (T-BOOTSTRAP-02) --------------------------


def escrow_of(world: World) -> Any:
    return world.store._services(world.run_id)[0]


def seed_pending_origin(world: World, origin: str, owner: str, units: int, mature_at: int) -> None:
    """A bootstrap-like origin that vests at `mature_at` (the genesis origins of the fixture all
    vest at the first beacon, so a later maturity needs one more origin)."""
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


def open_overdue_trial(world: World, index: int = 0, epoch: int = 777) -> None:
    """An admitted identity with an OPEN trial whose deadline round (3) has passed once the
    beacon reaches round 4: the bookkeeping the beacon route does after the maturity block."""
    joined = world.network.join(index)
    assert joined.status_code == 200, joined.text
    hotkey = load_fixture_module(*SERVICE_FIXTURE).HOT[index].ss58
    with world.store._tx():
        world.store._db.execute(
            "INSERT INTO admission_trial_results(epoch,challenge) VALUES(?,?)",
            (epoch, '{"deadline_beacon":3}'),
        )
        world.store._db.execute(
            "UPDATE admissions_v2 SET trial_epoch=?,proof_digest=NULL WHERE hotkey=?",
            (epoch, hotkey),
        )


def trial_outcome(world: World, epoch: int = 777) -> str:
    row = world.store._db.execute(
        "SELECT outcome FROM admission_trial_results WHERE epoch=?", (epoch,)
    ).fetchone()
    return str(row[0])


def maturity_id_of(world: World, origin: str) -> str:
    """The operation id of the MATURE event that vested `origin` (read from the ledger, so the
    id formula is never copied into a test)."""
    escrow = escrow_of(world)
    request = '["' + origin + '"]'
    with world.store._lock:
        rows = escrow.db.execute(
            "SELECT operation_id FROM escrow_events WHERE kind='MATURE' AND request=?", (request,)
        ).fetchall()
    assert len(rows) == 1, f"no single MATURE event for {origin}: {len(rows)}"
    return str(rows[0][0])


# --- a finalize_v2 on an audited round of N workers (T-BOOTSTRAP-14) -----------------------------


@dataclass
class SmallAuditedRound(AuditedRound):
    """`AuditedRound` for `size` identities instead of the fixture's four."""

    size: int = 0

    @property
    def hotkeys(self) -> list[str]:
        return sorted(key.ss58 for key in self.fixtures.HOT[: self.size])


def build_small_audited_round(
    tmp_path_factory: pytest.TempPathFactory, size: int
) -> Iterator[SmallAuditedRound]:
    """`size` ACTIVE identities, round 0 executed, audited (all MATCH) and aggregated; the
    coordinator has not finalized it. Same construction as `build_audited_round`, only the
    roster is larger (the aggregation needs four workers to fill the weight quantum) so that
    the round budget does not divide evenly between the workers. The identities the service
    fixture knows are replaced for the life of the generator, as the active_network fixture of
    tests/e2e/test_rollback_service_v2.py does for five."""
    rollback = load_fixture_module(*ROLLBACK_FIXTURE)
    fixtures = rollback.fixtures
    patch = pytest.MonkeyPatch()
    original_setup, original_genesis = fixtures.fixture.setup, fixtures.Genesis

    def carry_setup() -> Any:
        base = original_setup()
        body = base.manifest.body()
        body["training"]["inner"].update(state_policy="carry", rewarmup_steps=0)
        return fixtures.fixture.Setup(
            RunManifestV2.model_validate(body),
            base.policy,
            base.admission_policy,
            base.rows,
            base.tree,
        )

    def full_genesis(**fields: Any) -> Any:
        fields["total_units"] = sum(origin.units for origin in fields["origins"])
        return original_genesis(**fields)

    patch.setattr(fixtures, "HOT", [Keypair(bytes([80 + i]) * 32) for i in range(size)])
    patch.setattr(fixtures, "COLD", [Keypair(bytes([90 + i]) * 32) for i in range(size)])
    patch.setattr(fixtures.fixture, "setup", carry_setup)
    patch.setattr(fixtures, "Genesis", full_genesis)
    generator = fixtures.network.__wrapped__(
        tmp_path_factory.mktemp("gaps-bootstrap-round"), SimpleNamespace(param=None)
    )
    network = next(generator)
    snapshot = sqlite3.connect(":memory:", check_same_thread=False)
    try:
        spec = network.manifest.training.reference_spec
        for index in range(size):
            if index:
                network.push(network.now + 101)
            joined = network.join(index)
            assert joined.status_code == 200, joined.text
            identity = joined.json()["admission_id"]
            lock = EscrowLock(
                operation_id=f"{index + 1:064x}",
                owner=fixtures.COLD[index].ss58,
                units=1000,
                origin_ids=[network.origin_ids[index]],
                admission_id=identity,
                dispute_id=None,
                kind="LOCK_ADMISSION",
            )
            locked = network.client.post(
                network.url + "/escrow/lock",
                json=network.signed(fixtures.COLD[index], "EscrowLock", lock),
            )
            assert locked.status_code == 200, locked.text
            screen = WorkScreenV2(
                env=honest_env(),
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
        applied = rollback.execute_round(network, 0)
        network.store._db.backup(snapshot)
        yield SmallAuditedRound(network, fixtures, applied, snapshot, size)
    finally:
        snapshot.close()
        generator.close()
        patch.undo()


# --- run creation (T-BOOTSTRAP-10, T-BOOTSTRAP-18) ---------------------------------------------


def world_economics(world: World, **fields: Any) -> EconomicsPolicyV2:
    """The economics policy of the service world with `fields` replaced. The manifest that pins
    it is built by `support_bounds.creation`."""
    pinned = EconomicsPolicyV2.model_validate_json(
        world.store.objects.get(world.network.manifest.network.economics_policy_hash)
    )
    return EconomicsPolicyV2.model_validate({**pinned.body(), **fields})


def production_economics(world: World, **fields: Any) -> EconomicsPolicyV2:
    """`world_economics` made a production policy (no genesis pin), with `fields` replaced."""
    return world_economics(
        world, **{"ledger_mode": "production", "genesis_allocation_hash": None, **fields}
    )


def install_production_qualification(created: Creation) -> None:
    """The owner-signed CUDA qualification a production run stands on, stored for the run
    `created` would make, so that its creation is refused for what is under test and not for the
    missing record. No GPU result exists here: the record is what `_bootstrap_backend_v2` writes,
    assembled from the manifest. It carries the host attestation block of strict evidence, which a
    base that demands attested hosts refuses a production run without and one that does not
    ignores. The row is written before the run row: `records_v2` is keyed by run id alone."""
    owner = load_fixture_module(*SERVICE_FIXTURE).OWNER
    store, run_id = created.store, created.manifest.run_id()
    reference = created.manifest.training.reference_spec
    evidence = {
        "run_id": run_id,
        "backend": "cuda",
        "reference_hash": sha256_hex(canonicalize(reference.model_dump(mode="json"))),
        "layout_hash": reference.layout.model_dump_json(),
        "host_attestation": host_attestation(),
    }
    authority = store.objects.put(canonicalize(evidence))
    receipt = Receipt(w=0, commit_hash=authority, received_round=1)
    record = {
        **evidence,
        "authority_hash": authority,
        "receipt": seal(owner, "Receipt", run_id, receipt, 10_000),
    }
    with store._tx():
        store._put_record_v2(run_id, "qualification", reference.image_digest, record)


def challenge_refusal(call: Callable[[], object]) -> ChallengeError | None:
    """The ChallengeError `call` raises, None when it returns."""
    try:
        call()
    except ChallengeError as error:
        return error
    return None


def refusal_of(call: Callable[[], object]) -> tuple[str, str] | None:
    """(kind, reason) of the refusal `call` raises: a ChallengeError as its status and detail, an
    EscrowError as its code. None when the call returns."""
    try:
        call()
    except ChallengeError as error:
        return "ChallengeError", f"{error.status} {error.detail}"
    except EscrowError as error:
        return "EscrowError", error.code
    return None
