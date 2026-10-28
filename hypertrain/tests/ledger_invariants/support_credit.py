"""Builders private to the credit package. Not collected: no test_ prefix.

`build_plan` signs a complete reward round for any number of miners with test keys and no model
execution: commits, coordinator challenges, auditor MATCH verdicts, the Finalize and the
RewardFinalize. The replay callback of such a round is `Plan.replay`, a stand-in that accepts the
Finalize's own state hash: what it exercises is the ledger's allocation and binding rules, not
the training replay, which `real_reward` covers.
"""

from __future__ import annotations

import hashlib
import inspect
import sqlite3
from collections.abc import Callable, Iterator, Mapping, Sequence
from dataclasses import dataclass, field, replace
from types import ModuleType, SimpleNamespace
from typing import Any

import pytest

from env_pin_fixtures import honest_env
from hypertrain.challenge.store import ChallengeStore
from hypertrain.ledger.escrow_v2 import (
    EscrowError,
    EscrowV2,
    FinalityEvidence,
    RewardWork,
    Settlement,
    ShadowReservationEvidence,
    authority_message,
    bootstrap_origin_id,
    digest,
    genesis_allocation_hash,
    genesis_origin_id,
    reward_origin_id,
)
from hypertrain.miner import admission as miner_admission
from hypertrain.protocol.envelope_v2 import EnvelopeV2, seal
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages import Finalize, ReplayEnv, ReplayVerdict
from hypertrain.protocol.messages_v2 import (
    AuditChallengeV2,
    CommitV2,
    EconomicsPolicyV2,
    EscrowLock,
    OriginAllocation,
    ProductionBootstrap,
    RankWorkResult,
    RewardFinalize,
    RotateRequest,
    RunManifestV2,
    ShadowReservationRequestV1,
    ShadowReservationV1,
    ShadowRewardFinalizeV1,
    TestGenesis,
    WorkScreenV2,
)

from .support import H, load_fixture_module, memory_db, registered_owners, table_names

BUDGET = 1_000_000
SEED_MINER = 0x40
SEED_OWNER = 0x60
EXPIRY = 200


def miner(index: int) -> Keypair:
    return Keypair(bytes([SEED_MINER + index]) * 32)


def owner(index: int) -> Keypair:
    return Keypair(bytes([SEED_OWNER + index]) * 32)


def repin(setup: Any, **policy_changes: Any) -> Any:
    """`setup` under an economics policy with `policy_changes`; the run id follows the policy."""
    policy = EconomicsPolicyV2.model_validate({**setup.policy.body(), **policy_changes})
    body = setup.manifest.body()
    body["network"]["economics_policy_hash"] = digest(policy)
    return replace(setup, policy=policy, manifest=RunManifestV2.model_validate(body))


def issued_id(fixture: ModuleType, derive: Callable[..., str], *parts: Any) -> str:
    """The origin id the ledger of `fixture` derives with `derive` (genesis, bootstrap or reward).

    Where the derivation is keyed (`id_key`, ADR 0014 part 1) the fixture's own key is passed, the
    one its ledgers are built with; an earlier tree derives it from the public values alone.
    """
    if "id_key" in inspect.signature(derive).parameters:
        return derive(*parts, fixture.ID_KEY)
    return derive(*parts)


def split_units(budget: int, hotkeys: Sequence[str]) -> dict[str, int]:
    """The documented split for equal tokens: floor(budget / n), and the remainder one unit at a
    time to the first hotkeys in UTF-8 byte order."""
    ordered = sorted(hotkeys, key=lambda hotkey: hotkey.encode())
    base, remainder = divmod(budget, len(ordered))
    return {hotkey: base + (1 if rank < remainder else 0) for rank, hotkey in enumerate(ordered)}


@dataclass
class Item:
    """One miner's work before sealing: edit a field, then `Plan.build` signs it."""

    key: Keypair
    owner: str
    commit: CommitV2
    challenge: AuditChallengeV2
    verdict: ReplayVerdict
    commit_signer: Keypair
    challenge_signer: Keypair
    verdict_signer: Keypair
    sample_ids: tuple[int, ...] = (0,)
    commit_exp: int = EXPIRY
    challenge_exp: int = EXPIRY
    verdict_exp: int = EXPIRY

    @property
    def hotkey(self) -> str:
        return self.key.ss58


@dataclass
class Plan:
    fixture: ModuleType
    setup: Any
    w: int
    items: list[Item]
    theta: str
    entitlements_root: str = H
    included: list[str] | None = None
    finalize_signer: Keypair | None = None
    finalize_exp: int = EXPIRY
    finalized_beacon: int = 10
    vesting_beacon: int = 20
    mature_at: int = 20
    tape_hash: str = field(default="")

    @property
    def run_id(self) -> str:
        run_id: str = self.setup.manifest.run_id()
        return run_id

    @property
    def owners(self) -> dict[str, str]:
        return {item.hotkey: item.owner for item in self.items}

    def replay(self, evidence: FinalityEvidence) -> str:
        return Finalize.model_validate(evidence.finalize.body).final_theta_hash_w1

    def finalize(self) -> Finalize:
        included = self.included
        if included is None:
            included = sorted(item.hotkey for item in self.items)
        return Finalize(
            w=self.w,
            final_theta_hash_w1=self.theta,
            included=included,
            entitlements_root=self.entitlements_root,
        )

    def evidence(self) -> FinalityEvidence:
        signer = self.finalize_signer or self.fixture.COORD
        final = EnvelopeV2.model_validate(
            seal(signer, "Finalize", self.run_id, self.finalize(), self.finalize_exp)
        )
        works = tuple(
            RewardWork(
                EnvelopeV2.model_validate(
                    seal(i.commit_signer, "CommitV2", self.run_id, i.commit, i.commit_exp)
                ),
                EnvelopeV2.model_validate(
                    seal(i.verdict_signer, "ReplayVerdict", self.run_id, i.verdict, i.verdict_exp)
                ),
                EnvelopeV2.model_validate(
                    seal(
                        i.challenge_signer,
                        "AuditChallengeV2",
                        self.run_id,
                        i.challenge,
                        i.challenge_exp,
                    )
                ),
                i.owner,
                i.sample_ids,
            )
            for i in self.items
        )
        return FinalityEvidence(
            final,
            works,
            self.tape_hash,
            False,
            self.finalized_beacon,
            self.vesting_beacon,
        )

    def reward(
        self,
        evidence: FinalityEvidence,
        *,
        units: Mapping[str, int] | None = None,
        signer: Keypair | None = None,
        **changes: Any,
    ) -> RewardFinalize:
        """The coordinator-signed reward for `evidence`, allocated by `split_units`.

        `units` replaces the split (hotkey -> units); `changes` replace RewardFinalize fields
        before signing. The signature is made again, so only the change under test can refuse.
        """
        final_hash = digest(Finalize.model_validate(evidence.finalize.body))
        owners = {w.commit.signer: w.owner for w in evidence.works}
        hotkeys = sorted(owners, key=lambda hotkey: hotkey.encode())
        split = dict(units) if units is not None else split_units(BUDGET, hotkeys)
        mature_at = changes.get("mature_at", self.mature_at)
        origins = [
            OriginAllocation(
                origin_id=issued_id(
                    self.fixture, reward_origin_id, self.run_id, self.w, hotkey, final_hash
                ),
                owner=owners[hotkey],
                units=split[hotkey],
                mature_at=mature_at,
            )
            for hotkey in hotkeys
            if split.get(hotkey)
        ]
        body = RewardFinalize(
            run_id=self.run_id,
            w=self.w,
            finalize_hash=final_hash,
            tape_hash=evidence.tape_hash,
            verdict_root=sha256_hex(
                canonicalize(
                    [digest(ReplayVerdict.model_validate(w.verdict.body)) for w in evidence.works]
                )
            ),
            allocation_hash=sha256_hex(canonicalize([o.body() for o in origins])),
            budget_units=BUDGET,
            origin_ids=[o.origin_id for o in origins],
            mature_at=mature_at,
            authority_sig="0" * 128,
        ).body()
        unsigned = RewardFinalize.model_validate({**body, **changes})
        key = signer or self.fixture.COORD
        return RewardFinalize.model_validate(
            {**unsigned.body(), "authority_sig": key.sign(authority_message(unsigned)).hex()}
        )

    def build(self, **reward_changes: Any) -> tuple[RewardFinalize, FinalityEvidence]:
        evidence = self.evidence()
        return self.reward(evidence, **reward_changes), evidence


def build_plan(
    fixture: ModuleType,
    setup: Any,
    miners: Sequence[Keypair],
    *,
    w: int = 0,
    owners: Mapping[str, str] | None = None,
    entitlements_root: str = H,
) -> Plan:
    """A plan for `w` with an audited MATCH for each of `miners` (owner(i) owns miner(i))."""
    training = setup.manifest.training
    tokens = training.batch_samples() * training.model.seq_len
    theta = sha256_hex(f"theta|{setup.manifest.run_id()}|{w}".encode())
    items = []
    for index, key in enumerate(miners):
        leaves = sha256_hex(f"leaves|{w}|{key.ss58}".encode())
        commit = CommitV2(
            w=w,
            hotkey=key.ss58,
            leaf_scheme="ht-leaf-v1",
            n_leaves=training.inner.H // training.inner.J + 1,
            leaves_root=leaves,
            metrics_root=H,
            final_theta_hash=theta,
            ef_in_hash=H,
            ef_out_hash=H,
            delta_hash=sha256_hex(f"delta|{w}|{key.ss58}".encode()),
            delta_bytes=1,
            tokens=tokens,
        )
        challenge = AuditChallengeV2(
            w=w,
            target=key.ss58,
            beacon_round=1,
            beacon_sig_sha256=H,
            mode="full",
            segments=[],
            reasons=["probation"],
            serve_deadline=EXPIRY,
            anchor_hash=H,
            audit_mode="anchored-full",
        )
        verdict = ReplayVerdict(
            challenge_hash=digest(challenge),
            first_bad_leaf=None,
            result="MATCH",
            recomputed_leaves_root=leaves,
            replay_env=ReplayEnv(
                image_digest=training.reference_spec.image_digest,
                driver=training.reference_spec.driver_allowlist[0],
                gpu_uuid_sha256=H,
                sm_count=1,
            ),
        )
        items.append(
            Item(
                key,
                (owners or {}).get(key.ss58) or owner(index).ss58,
                commit,
                challenge,
                verdict,
                key,
                fixture.COORD,
                fixture.AUDITOR,
            )
        )
    return Plan(
        fixture,
        setup,
        w,
        items,
        theta,
        entitlements_root=entitlements_root,
        tape_hash=sha256_hex(f"tape|{setup.manifest.run_id()}|{w}".encode()),
    )


def open_ledger(
    fixture: ModuleType,
    setup: Any,
    database: Any,
    owners: Mapping[str, str],
    replay: Callable[[FinalityEvidence], str],
    settlement: Callable[[str], Settlement] | None = None,
) -> EscrowV2:
    """A ledger on `database` (a path or a connection) behind test-owned service callbacks."""
    escrow: EscrowV2 = fixture.ledger(setup, database, settlement=settlement, replay=replay)
    escrow.owner_of = registered_owners(owners)
    return escrow


def ledger_rows(escrow: EscrowV2) -> dict[str, Any]:
    """Every durable fact of the ledger: a refusal must leave this value unchanged."""
    db = escrow.db
    return {
        name: sorted((tuple(row) for row in db.execute(f'SELECT * FROM "{name}"')), key=repr)
        for name in table_names(db)
    }


def database_digest(store: ChallengeStore) -> str:
    """Hash of every row of every table of the store database (the ledger shares it)."""
    hasher = hashlib.sha256()
    with store._lock:
        for name in table_names(store._db):
            rows = sorted(repr(tuple(row)) for row in store._db.execute(f'SELECT * FROM "{name}"'))
            hasher.update(repr((name, rows)).encode())
    return hasher.hexdigest()


def reward_origins(escrow: EscrowV2) -> list[tuple[int, str]]:
    """(round, hotkey) of every LIVE reward origin: shadow origins reuse the reward_round column
    for their trial epoch and are excluded through escrow_shadow_finalized."""
    return [
        (row[0], row[1])
        for row in escrow.db.execute(
            "SELECT reward_round, hotkey FROM escrow_origins WHERE hotkey IS NOT NULL "
            "AND origin NOT IN (SELECT origin FROM escrow_shadow_finalized) "
            "ORDER BY reward_round, hotkey"
        )
    ]


def event_count(escrow: EscrowV2, kind: str | None = None) -> int:
    sql = "SELECT COUNT(*) FROM escrow_events" + (" WHERE kind=?" if kind else "")
    return int(escrow.db.execute(sql, (kind,) if kind else ()).fetchone()[0])


def make_genesis(
    fixture: ModuleType,
    run_id: str,
    *,
    allocation: Sequence[tuple[str, int, int]] | None = None,
    signer: Keypair | None = None,
    pinned_hash: str | None = None,
    origin_ids: Mapping[int, str] | None = None,
    claimed_run: str | None = None,
) -> TestGenesis:
    """The coordinator-signed genesis of the fixture's pinned allocation, or a variant of it."""
    allocation = allocation or [
        (fixture.COLD.ss58, 20000, 1),
        (fixture.OTHER.ss58, 10000, 1),
    ]
    allocation_hash = genesis_allocation_hash(allocation)
    origins = [
        OriginAllocation(
            origin_id=(origin_ids or {}).get(
                i, issued_id(fixture, genesis_origin_id, run_id, allocation_hash, i)
            ),
            owner=holder,
            units=units,
            mature_at=mature_at,
        )
        for i, (holder, units, mature_at) in enumerate(allocation)
    ]
    unsigned = TestGenesis(
        run_id=claimed_run or run_id,
        allocation_hash=pinned_hash or allocation_hash,
        total_units=sum(units for _, units, _ in allocation),
        origins=origins,
        authority_sig="0" * 128,
    )
    key = signer or fixture.COORD
    return TestGenesis.model_validate(
        {**unsigned.body(), "authority_sig": key.sign(authority_message(unsigned)).hex()}
    )


def make_bootstrap(
    fixture: ModuleType,
    run_id: str,
    allocation: Sequence[tuple[str, int, int]],
    *,
    operation: str,
    signer: Keypair | None = None,
    pinned_hash: str | None = None,
    origin_ids: Mapping[int, str] | None = None,
    claimed_run: str | None = None,
) -> ProductionBootstrap:
    allocation_hash = genesis_allocation_hash(allocation)
    origins = [
        OriginAllocation(
            origin_id=(origin_ids or {}).get(
                i, issued_id(fixture, bootstrap_origin_id, run_id, allocation_hash, i)
            ),
            owner=holder,
            units=units,
            mature_at=mature_at,
        )
        for i, (holder, units, mature_at) in enumerate(allocation)
    ]
    unsigned = ProductionBootstrap(
        run_id=claimed_run or run_id,
        operation_id=operation,
        allocation_hash=pinned_hash or allocation_hash,
        total_units=sum(units for _, units, _ in allocation),
        origins=origins,
        authority_sig="0" * 128,
    )
    key = signer or fixture.COORD
    return ProductionBootstrap.model_validate(
        {**unsigned.body(), "authority_sig": key.sign(authority_message(unsigned)).hex()}
    )


def reserve_shadow_round(
    fixture: ModuleType, escrow: EscrowV2, plan: Plan, admission_id: str
) -> tuple[ShadowReservationV1, ShadowRewardFinalizeV1, FinalityEvidence]:
    """Reserve the next shadow ordinal for the single miner of `plan` (trial epoch = plan.w) and
    sign the shadow reward for it. Returns (reservation, shadow reward, shadow evidence)."""
    evidence = plan.evidence()
    item = plan.items[0]
    request = ShadowReservationRequestV1(
        run_id=plan.run_id,
        admission_id=admission_id,
        hotkey=item.hotkey,
        coldkey=item.owner,
        trial_epoch=plan.w,
        commit_hash=digest(item.commit),
        finalize_hash=digest(plan.finalize()),
        custody_hash=evidence.tape_hash,
        graph_hash=H,
        economic_admission_hash=H,
        nonce=H,
        assignment_hash=H,
        finalized_beacon=evidence.finalized_beacon,
        mature_at=evidence.finalized_beacon + escrow.manifest.training.verify.E_vest_rounds,
    )
    custody = ShadowReservationEvidence(
        finalize=evidence.finalize,
        commit=evidence.works[0].commit,
        owner=item.owner,
        sample_ids=item.sample_ids,
        custody_hash=evidence.tape_hash,
        reference_final_state_hash=plan.theta,
    )
    binding = escrow.reserve_shadow(request, custody)
    shadow = fixture.sign_actual_shadow(plan.reward(evidence), binding)
    return binding, shadow, replace(evidence, shadow=True)


STRANGER = owner(9)
HOTKEYS = [miner(0), miner(1)]


def refusal(call: Callable[[], object]) -> str:
    """What the ledger answers: the EscrowError code, or the class name of any other refusal."""
    try:
        call()
    except EscrowError as error:
        return error.code
    except Exception as error:
        return type(error).__name__
    raise AssertionError("expected a refusal, the call succeeded")


def finalize_refusal(escrow: EscrowV2, reward: Any, evidence: Any) -> str:
    return refusal(lambda: escrow.reward_finalize(reward, evidence))


def ledger_for(
    fixture: ModuleType,
    setup: Any,
    plan: Plan,
    database: Any = None,
    owners: Mapping[str, str] | None = None,
    settlement: Callable[[str], Settlement] | None = None,
) -> EscrowV2:
    """A ledger on `database` (default: a private in-memory one) serving `plan`'s owners."""
    return open_ledger(
        fixture,
        setup,
        database or memory_db(),
        owners or plan.owners,
        plan.replay,
        settlement,
    )


# --- finalize_v2 on a real audited and aggregated round ---------------------------------------

ROLLBACK_FIXTURE = ("li_rollback_fixture", "e2e/test_rollback_service_v2.py")


@dataclass
class AuditedRound:
    """Four ACTIVE identities, round 0 executed, audited (four MATCH) and aggregated; the
    coordinator has not finalized it. `restore()` puts the store back to that moment."""

    network: Any
    fixtures: Any
    applied: dict[str, Any]
    snapshot: sqlite3.Connection

    @property
    def store(self) -> Any:
        return self.network.store

    @property
    def run_id(self) -> str:
        run_id: str = self.network.manifest.run_id()
        return run_id

    @property
    def hotkeys(self) -> list[str]:
        return sorted(key.ss58 for key in self.fixtures.HOT)

    def restore(self) -> None:
        """The state after aggregation, services rebuilt outside any transaction."""
        self.store._services_v2.clear()
        self.snapshot.backup(self.store._db)
        self.store._services(self.run_id)

    def finalize_request(self, **changes: Any) -> dict[str, Any]:
        final = Finalize(
            **{
                "w": 0,
                "final_theta_hash_w1": self.applied["theta_hash"],
                "included": self.hotkeys,
                "entitlements_root": H,
                **changes,
            }
        )
        signed: dict[str, Any] = self.network.signed(self.fixtures.COORD, "Finalize", final)
        return signed

    def finalize(self, **changes: Any) -> Any:
        return self.network.client.post(
            self.network.url + "/admin/rounds/0/finalize",
            json=self.finalize_request(**changes),
            headers=self.fixtures.admin(),
        )


def build_audited_round(
    tmp_path_factory: pytest.TempPathFactory,
) -> Iterator[AuditedRound]:
    """About 25 s: four real CPU islands, their relay uploads and four independent replays. The
    identities are activated by SQL, as tests/e2e/test_dispute_exit_v2_e2e.py does, because the
    twelve-trial ladder has its own test."""
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

    patch.setattr(fixtures.fixture, "setup", carry_setup)
    patch.setattr(fixtures, "Genesis", full_genesis)
    generator = fixtures.network.__wrapped__(
        tmp_path_factory.mktemp("credit-round"), SimpleNamespace(param=None)
    )
    network = next(generator)
    snapshot = sqlite3.connect(":memory:", check_same_thread=False)
    try:
        spec = network.manifest.training.reference_spec
        for index in range(4):
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
                env=honest_env(spec.env.pin()),
            )
            with network.store._tx():
                network.store._db.execute(
                    "UPDATE admissions_v2 SET state='ACTIVE',clean_count=12,screen_json=?,"
                    "screen_until=1000000000 WHERE admission_id=?",
                    (screen.model_dump_json(), identity),
                )
        applied = rollback.execute_round(network, 0)
        network.store._db.backup(snapshot)
        yield AuditedRound(network, fixtures, applied, snapshot)
    finally:
        snapshot.close()
        generator.close()
        patch.undo()


def credited(round_: AuditedRound) -> list[tuple[str, int, int, str]]:
    """(hotkey, units, round, owner) of every live origin of the store's ledger."""
    rows = round_.store._db.execute(
        "SELECT o.hotkey,o.issued,o.reward_round,u.owner FROM escrow_origins o "
        "JOIN escrow_units u ON u.origin=o.origin WHERE o.hotkey IS NOT NULL ORDER BY o.hotkey"
    ).fetchall()
    return [(r["hotkey"], r["issued"], r["reward_round"], r["owner"]) for r in rows]


def rotation_envelope(
    run_id: str,
    cold: Keypair,
    old_hotkey: str,
    new_hot: Keypair,
    operation_id: str,
    expires_beacon: int = 10_000,
) -> dict[str, Any]:
    """The coldkey's sealed request to move the admission of `old_hotkey` to `new_hot`.

    Where the model carries the new hotkey's consent (`new_hot_sig`, ADR 0010 part 1) it is given
    by `new_hot` for exactly this coldkey, run and expiry; an earlier tree takes the three fields.
    """
    consent = getattr(miner_admission, "sign_rotation_consent", None)
    if consent is None or "new_hot_sig" not in RotateRequest.model_fields:
        request = RotateRequest.model_validate(
            {"hotkey": old_hotkey, "new_hotkey": new_hot.ss58, "operation_id": operation_id}
        )
    else:
        request = consent(
            new_hot,
            run_id=run_id,
            coldkey=cold.ss58,
            hotkey=old_hotkey,
            operation_id=operation_id,
            expires_beacon=expires_beacon,
        )
    sealed: dict[str, Any] = miner_admission.sign_rotation(cold, run_id, request, expires_beacon)
    return sealed
