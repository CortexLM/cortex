"""Builders private to the bounds package of the ledger-invariants lot. Not collected.

`synthetic_round` signs the finality evidence of a round for any number of workers, so the
ledger's reward arithmetic can be driven without the 8 s CPU training of `real_reward`. The
replay of the final state is the only stand-in: `accept_replay` returns the Finalize's own final
hash. Everything else (signatures, audit binding, assignment, owner, allocation hash, origin ids,
budget, issuance cap) is checked by the real `EscrowV2`.
"""

from __future__ import annotations

import inspect
import json
from collections.abc import Callable, Iterator, Mapping, Sequence
from contextlib import contextmanager
from dataclasses import dataclass, replace
from pathlib import Path
from types import ModuleType, SimpleNamespace
from typing import Any

import pytest
from pydantic import ValidationError

from hypertrain.challenge.admission import Admission
from hypertrain.challenge.store import ChallengeStore
from hypertrain.challenge.trust_v2 import FundedStatus, audit_floor
from hypertrain.data.store import LocalFSStore
from hypertrain.ledger import Params, escrow_v2
from hypertrain.ledger.escrow_v2 import (
    EscrowV2,
    FinalityEvidence,
    RewardWork,
    authority_message,
    digest,
    genesis_allocation_hash,
)
from hypertrain.protocol import envelope_v2
from hypertrain.protocol.envelope_v2 import EnvelopeV2, seal
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages import Finalize, ReplayEnv, ReplayVerdict, f32hex
from hypertrain.protocol.messages_v2 import (
    AuditChallengeV2,
    CommitV2,
    EconomicsPolicyV2,
    EscrowLock,
    OriginAllocation,
    PolicyHashes,
    ProductionBootstrap,
    RankWorkResult,
    RewardFinalize,
    RosterEntryV2,
    RoundOpenV2,
    RunManifestV2,
    TestGenesis,
    WorkScreenV2,
)
from hypertrain.trainer.compress import state_hash
from hypertrain.trainer.config import TrainConfig
from hypertrain.trainer.model import init_params

from .support import (
    SERVICE_FIXTURE,
    H,
    bare_escrow,
    load_fixture_module,
    match_settlement,
    memory_db,
)

MATURE_AT = 20
FINALIZED_BEACON = 10
PPM = 10**6
U64_MAX = 2**53 - 1
K = Keypair(bytes(range(32))).ss58

BASE_POLICY: dict[str, Any] = {
    "ledger_mode": "test",
    "G_max_units": 100,
    "R_collectible_units": 100,
    "S_min_units": 1000,
    "beta_ppm": PPM,
    "gammaV_units": 0,
    "s_lower_ppm": PPM,
    "q_floor": PPM,
    "genesis_allocation_hash": H,
    "reward_authority": K,
    "round_reward_units": 100,
    "max_total_issuance": 100_000,
    "shadow_bootstrap_rounds": 12,
}


def econ(**fields: Any) -> EconomicsPolicyV2:
    return EconomicsPolicyV2.model_validate({**BASE_POLICY, **fields})


def id_key_of(holder: Any) -> bytes | None:
    """The key under which a ledger derives the ids of its origins, read from the ledger itself or
    from the fixture module whose `ledger()` builds it. None where the ids are not keyed."""
    return getattr(holder, "id_key", None) or getattr(holder, "ID_KEY", None)


def origin_id_of(holder: Any, kind: str, *parts: Any) -> str:
    """The documented origin id `<kind>_origin_id(*parts)` ('genesis', 'bootstrap' or 'reward') as
    the ledger `holder` derives it. The derivation takes the key of the ledger when it has one
    (escrow-relay-fixes, ADR 0014 part 1), so no test names a key or a derivation of its own."""
    derive = getattr(escrow_v2, f"{kind}_origin_id")
    if "id_key" in inspect.signature(derive).parameters:
        return str(derive(*parts, id_key_of(holder)))
    return str(derive(*parts))


def refuses(call: Callable[[], object]) -> bool:
    """True when `call` raises a pydantic ValidationError, False when it returns."""
    try:
        call()
    except ValidationError:
        return True
    return False


def repinned(setup: Any, *, epochs_per_round: int | None = None, **policy: Any) -> Any:
    """`setup` with `policy` fields replaced; the manifest pins the new digest, so the run id moves.

    `epochs_per_round` also moves the budget the ledger derives for a round, which keeps
    MANIFEST_REWARD_BUDGET satisfied when a test wants a different round size.
    """
    changed = EconomicsPolicyV2.model_validate({**setup.policy.body(), **policy})
    body = setup.manifest.body()
    body["network"]["economics_policy_hash"] = digest(changed)
    if epochs_per_round is not None:
        body["training"]["budget"]["epochs_per_round"] = epochs_per_round
    return replace(setup, policy=changed, manifest=RunManifestV2.model_validate(body))


def worker(index: int) -> tuple[Keypair, Keypair]:
    """(hotkey, coldkey) of the test worker `index`, 0..31. Seeds avoid the fixture's own keys."""
    return Keypair(bytes([0x40 + index]) * 32), Keypair(bytes([0x80 + index]) * 32)


def split(budget: int, hotkeys: Sequence[str]) -> dict[str, int]:
    """The documented split of a round reward between workers who audited the same sample count:
    floor(budget / n) each, and the remainder one unit at a time to the first hotkeys in UTF-8
    order."""
    ordered = sorted(hotkeys, key=lambda h: h.encode())
    base, remainder = divmod(budget, len(ordered))
    return {h: base + (1 if rank < remainder else 0) for rank, h in enumerate(ordered)}


def accept_replay(evidence: FinalityEvidence) -> str:
    """Stand-in for the independent replay: the final state it 'recomputes' is the signed one."""
    return str(evidence.finalize.body["final_theta_hash_w1"])


@dataclass(frozen=True)
class Round:
    reward: RewardFinalize
    evidence: FinalityEvidence
    owners: dict[str, str]  # hotkey -> coldkey, for the ledger's owner_of
    units: dict[str, int]  # hotkey -> units the reward body allocates


def synthetic_round(
    fixture: ModuleType,
    setup: Any,
    w: int,
    workers: Sequence[tuple[Keypair, Keypair]],
    *,
    units: Mapping[str, int] | None = None,
    budget: int | None = None,
) -> Round:
    """A coordinator-signed Finalize/RewardFinalize pair for round `w` and the workers' evidence.

    Each worker has one signed commit, an audit challenge and a MATCH verdict of the fixture's
    auditor. `units` overrides the allocation the reward body states (default: `split`), `budget`
    the budget it states (default: the policy's round reward).
    """
    run_id = setup.manifest.run_id()
    manifest = setup.manifest
    hotkeys = [hot.ss58 for hot, _ in workers]
    final = Finalize(
        w=w,
        final_theta_hash_w1=sha256_hex(f"theta|{w}".encode()),
        included=sorted(hotkeys),
        entitlements_root=H,
    )
    final_env = EnvelopeV2.model_validate(seal(fixture.COORD, "Finalize", run_id, final, 200))
    leaves_root = sha256_hex(f"leaves|{w}".encode())
    works, verdict_digests = [], []
    for hot, cold in workers:
        commit = CommitV2(
            w=w,
            hotkey=hot.ss58,
            leaf_scheme="ht-leaf-v1",
            n_leaves=manifest.training.inner.H // manifest.training.inner.J + 1,
            leaves_root=leaves_root,
            metrics_root=H,
            final_theta_hash=final.final_theta_hash_w1,
            ef_in_hash=H,
            ef_out_hash=H,
            delta_hash=H,
            delta_bytes=1,
            tokens=manifest.training.batch_samples() * manifest.training.model.seq_len,
        )
        challenge = AuditChallengeV2(
            w=w,
            target=hot.ss58,
            beacon_round=1,
            beacon_sig_sha256=H,
            mode="full",
            segments=[],
            reasons=["probation"],
            serve_deadline=200,
            anchor_hash=H,
            audit_mode="anchored-full",
        )
        verdict = ReplayVerdict(
            challenge_hash=digest(challenge),
            first_bad_leaf=None,
            result="MATCH",
            recomputed_leaves_root=leaves_root,
            replay_env=ReplayEnv(
                image_digest=manifest.training.reference_spec.image_digest,
                driver=manifest.training.reference_spec.driver_allowlist[0],
                gpu_uuid_sha256=H,
                sm_count=1,
            ),
        )
        verdict_digests.append(digest(verdict))
        works.append(
            RewardWork(
                EnvelopeV2.model_validate(seal(hot, "CommitV2", run_id, commit, 200)),
                EnvelopeV2.model_validate(
                    seal(fixture.AUDITOR, "ReplayVerdict", run_id, verdict, 200)
                ),
                EnvelopeV2.model_validate(
                    seal(fixture.COORD, "AuditChallengeV2", run_id, challenge, 200)
                ),
                cold.ss58,
                (0,),
            )
        )
    evidence = FinalityEvidence(
        final_env,
        tuple(works),
        sha256_hex(f"tape|{w}".encode()),
        False,
        FINALIZED_BEACON,
        MATURE_AT,
    )
    paid = dict(units) if units is not None else split(setup.policy.round_reward_units, hotkeys)
    cold_of = {hot.ss58: cold.ss58 for hot, cold in workers}
    origins = [
        OriginAllocation(
            origin_id=origin_id_of(fixture, "reward", run_id, w, hotkey, digest(final)),
            owner=cold_of[hotkey],
            units=paid[hotkey],
            mature_at=MATURE_AT,
        )
        for hotkey in sorted(paid, key=lambda h: h.encode())
        if paid[hotkey]
    ]
    unsigned = RewardFinalize(
        run_id=run_id,
        w=w,
        finalize_hash=digest(final),
        tape_hash=evidence.tape_hash,
        verdict_root=sha256_hex(canonicalize(verdict_digests)),
        allocation_hash=sha256_hex(canonicalize([o.body() for o in origins])),
        budget_units=budget if budget is not None else setup.policy.round_reward_units,
        origin_ids=[o.origin_id for o in origins],
        mature_at=MATURE_AT,
        authority_sig="0" * 128,
    )
    reward = RewardFinalize.model_validate(
        {**unsigned.body(), "authority_sig": fixture.COORD.sign(authority_message(unsigned)).hex()}
    )
    return Round(reward, evidence, cold_of, paid)


def owned(rounds: Sequence[Round]) -> dict[str, str]:
    return {hotkey: cold for r in rounds for hotkey, cold in r.owners.items()}


def ledger_for(fixture: ModuleType, setup: Any, rounds: Sequence[Round]) -> EscrowV2:
    """A bare in-memory ledger that knows the owners of `rounds`' workers and accepts the
    stand-in replay. Settlement answers MATCH at the rounds' maturity for the finalize hash of
    the round the origin belongs to."""
    finalized = {r.reward.origin_ids[0]: r.reward.finalize_hash for r in rounds}

    def settlement(reference: str) -> Any:
        for round_ in rounds:
            if reference in round_.reward.origin_ids:
                return match_settlement(round_.reward.finalize_hash, MATURE_AT)
        return match_settlement(next(iter(finalized.values())), MATURE_AT)

    return bare_escrow(
        fixture, setup, replay=accept_replay, owner_of=owned(rounds), settlement=settlement
    )


def pinned_bootstrap(
    fixture: ModuleType,
    run_id: str,
    allocation: Sequence[tuple[str, int, int]],
    *,
    operation: str = "44" * 32,
    signer: Keypair | None = None,
    pin: str | None = None,
    origin_ids: Sequence[str] | None = None,
    ledger: Any = None,
) -> ProductionBootstrap:
    """A ProductionBootstrap for `allocation` [(owner, units, mature_at)], signed by `signer`
    (default: the coordinator). `pin` overrides the allocation_hash it states and `origin_ids`
    the ids of its origins (default: the documented bootstrap origin ids, derived as `ledger`
    does when the ledger is not the one `fixture` builds)."""
    allocation_hash = pin or genesis_allocation_hash(allocation)
    ids = origin_ids or [
        origin_id_of(fixture if ledger is None else ledger, "bootstrap", run_id, allocation_hash, i)
        for i in range(len(allocation))
    ]
    unsigned = ProductionBootstrap(
        run_id=run_id,
        operation_id=operation,
        allocation_hash=allocation_hash,
        total_units=sum(units for _, units, _ in allocation),
        origins=[
            OriginAllocation(origin_id=ids[i], owner=owner, units=units, mature_at=mature_at)
            for i, (owner, units, mature_at) in enumerate(allocation)
        ],
        authority_sig="0" * 128,
    )
    key = signer or fixture.COORD
    return unsigned.model_copy(
        update={"authority_sig": key.sign(authority_message(unsigned)).hex()}
    )


def nothing_issued(escrow: EscrowV2) -> bool:
    """No event, no origin and no unit: a refused issuance left the ledger untouched."""
    events = escrow.db.execute("SELECT COUNT(*) FROM escrow_events").fetchone()[0]
    origins = escrow.db.execute("SELECT COUNT(*) FROM escrow_origins").fetchone()[0]
    return bool(escrow.balances().issued == 0 and events == 0 and origins == 0)


def pinned_production(fixture: ModuleType, **update: Any) -> tuple[Any, EscrowV2, list[Any]]:
    """A production setup pinning the 30,000-unit bootstrap of the fixture's two coldkeys, and a
    ledger on it. Returns (setup, ledger, allocation)."""
    allocation = [(fixture.COLD.ss58, 20_000, 1), (fixture.OTHER.ss58, 10_000, 1)]
    setup = repinned(
        fixture.setup("production"),
        bootstrap_allocation_hash=genesis_allocation_hash(allocation),
        **update,
    )
    return setup, fixture.ledger(setup, memory_db()), allocation


def genesis_request(
    fixture: ModuleType,
    run_id: str,
    alloc: Sequence[tuple[str, int, int]],
    *,
    pin: str | None = None,
    origin_ids: Mapping[int, str] | None = None,
) -> TestGenesis:
    """A coordinator-signed TestGenesis of `alloc` (default pin: the hash of `alloc` itself);
    `origin_ids` replaces the id of the origins it names, by index."""
    allocation_hash = pin or genesis_allocation_hash(alloc)
    unsigned = TestGenesis(
        run_id=run_id,
        allocation_hash=allocation_hash,
        total_units=sum(units for _, units, _ in alloc),
        origins=[
            OriginAllocation(
                origin_id=(origin_ids or {}).get(
                    i, origin_id_of(fixture, "genesis", run_id, allocation_hash, i)
                ),
                owner=owner,
                units=units,
                mature_at=mature_at,
            )
            for i, (owner, units, mature_at) in enumerate(alloc)
        ],
        authority_sig="0" * 128,
    )
    return unsigned.model_copy(
        update={"authority_sig": fixture.COORD.sign(authority_message(unsigned)).hex()}
    )


class Floors:
    """One policy seen by the three implementations of the per-miner stake floor: the admission
    status (what a round roster is checked against), the tape gate (`audit_floor`) and the
    operator tool (`coldkey_ops.required_units`). The ledger is a test-mode one funded by the
    fixture's genesis; the admission is a SQL row, so no trial is run."""

    def __init__(self, fixture: ModuleType, objects: LocalFSStore, **policy: Any) -> None:
        self.fixture = fixture
        self.setup = repinned(fixture.setup("test"), **policy)
        self.escrow = fixture.ledger(self.setup, memory_db())
        fixture.fund(self.escrow)
        self.admission = Admission(
            self.escrow,
            self.setup.admission_policy,
            fixture.COORD,
            beacon=lambda now: SimpleNamespace(round=now, bls_verified=False),
            stage_reference=lambda *_: (_ for _ in ()).throw(AssertionError("no trial here")),
            objects=objects,
            ip_secret=b"private-prefix-key",
            qualified=lambda _: True,
            conservative_bound=lambda: True,
            backend="cpu",
        )
        self.escrow.db.execute(
            "INSERT INTO admissions_v2(admission_id, run_id, hotkey, coldkey, state, clean_count, "
            "canary_blocks, strikes, pending_dispute, reason, policy_hash, received_beacon, "
            "seed_beacon, screen_until) VALUES(?,?,?,?,'ACTIVE',12,'1,2,3',0,0,'',?,1,1,?)",
            (H, self.escrow.run_id, fixture.HOT.ss58, fixture.COLD.ss58, H, 10**9),
        )
        self.locked = 0

    def lock_to(self, units: int) -> None:
        """Lock `units` in total for the admission (one more lock for the difference)."""
        extra = units - self.locked
        if extra <= 0:
            return
        origin = self.escrow.db.execute(
            "SELECT origin FROM escrow_units WHERE owner=? AND bucket='available' AND units>0",
            (self.fixture.COLD.ss58,),
        ).fetchone()[0]
        lock = EscrowLock(
            operation_id=f"{self.locked + 1:064x}",
            owner=self.fixture.COLD.ss58,
            units=extra,
            origin_ids=[origin],
            admission_id=H,
            dispute_id=None,
            kind="LOCK_ADMISSION",
        )
        self.escrow.lock(lock, signer=self.fixture.COLD.ss58)
        self.locked = units

    def status(self) -> tuple[bool, FundedStatus]:
        status = self.admission.status(self.fixture.HOT.ss58, now=1)
        return status.eligible, status.funding

    def tape_gate(self, funding: FundedStatus) -> tuple[bool, int]:
        roster = RosterEntryV2(
            hotkey=self.fixture.HOT.ss58,
            slot=0,
            q_i=f32hex(1),
            admission_id=H,
            coldkey_group=self.fixture.COLD.ss58,
            state="ACTIVE",
            eligible_weight=1 << 22,
        )
        floor = audit_floor(
            self.escrow.policy,
            self.escrow.manifest.training.verify,
            roster,
            funding,
            run_id=self.escrow.run_id,
            clean_finalizations=12,
            reference_reward_units=self.escrow.policy.R_collectible_units,
        )
        return floor.influence_allowed, floor.required_locked_units


ECONOMICS_FIXTURE = ("li_economics_fixture", "challenge/test_production_economics_v2.py")
CAPACITY_FIXTURE = ("li_capacity_fixture", "challenge/test_service_capacity_v2.py")


@dataclass
class Hosting:
    """A ChallengeStore and the EscrowV2 of one run sharing one connection and one lock."""

    store: ChallengeStore
    escrow: EscrowV2
    run_id: str

    def weights(self, epoch: int, drand_round: int = MATURE_AT) -> dict[str, Any]:
        """The epoch answer of `ChallengeStore.weights` for an epoch ending at `drand_round`."""
        beacon = self.escrow.manifest.training.beacon
        epoch_at = beacon.genesis_time + (drand_round - 1) * beacon.period
        answer: dict[str, Any] = json.loads(self.store.weights(epoch, epoch_at, 1))
        return answer


@contextmanager
def hosting(
    root: Path, fixture: ModuleType, setup: Any, rounds: Sequence[Round]
) -> Iterator[Hosting]:
    """Host `rounds`' ledger in a store as `ChallengeStore.weights` reads it. The replay and the
    settlement are the stand-ins of `ledger_for`; the store's own bridge to weights is real."""
    run_id = setup.manifest.run_id()
    store = ChallengeStore(
        root / "state",
        # The v1 genesis deliberately differs from the drand genesis the maturity round uses.
        Params("hypertrain", setup.manifest.training.beacon.genesis_time - 600, 300, 1, 1),
        None,
        None,
        lambda payload: None,
        LocalFSStore(root / "objects"),
    )
    try:
        escrow = fixture.ledger(
            setup,
            store._db,
            replay=accept_replay,
            settlement=lambda ref: next(
                match_settlement(r.reward.finalize_hash, MATURE_AT)
                for r in rounds
                if ref in r.reward.origin_ids
            ),
        )
        escrow.owner_of = lambda hotkey: owned(rounds)[hotkey]
        escrow.mutex = store._lock
        store._db.execute(
            "INSERT INTO runs VALUES(?,?,?,'created',NULL)",
            (run_id, setup.manifest.model_dump_json(), "{}"),
        )
        store._services = lambda _run_id: (escrow, None, None)  # type: ignore[method-assign,assignment]
        yield Hosting(store, escrow, run_id)
    finally:
        store.close()


@dataclass
class Production:
    """A production run in a real store (`test_production_economics_v2.hosted`): the CUDA
    qualification is stood in for, beacons are SQL rows, the bootstrap pins 30,000 units."""

    eco: ModuleType
    store: ChallengeStore
    run_id: str

    @property
    def escrow(self) -> EscrowV2:
        return self.store._services(self.run_id)[0]

    @property
    def admission(self) -> Admission:
        return self.store._services(self.run_id)[1]

    def status(self) -> Any:
        return self.admission.status(self.eco.E.HOT.ss58, now=self.store._now(self.store._db))

    def bootstrap_body(
        self, *, signer: Keypair | None = None, operation: str | None = None
    ) -> ProductionBootstrap:
        """The fixture's pinned 30,000-unit bootstrap, its origin ids derived by the ledger of this
        run (the store keys them with its own coordinator secret, the fixture module cannot)."""
        return pinned_bootstrap(
            self.eco.E,
            self.run_id,
            self.eco.B.ALLOC,
            operation=operation or self.eco.B.OP,
            signer=signer,
            ledger=self.escrow,
        )

    def signed_bootstrap(
        self, *, signer: Keypair | None = None, operation: str | None = None
    ) -> bytes:
        """`bootstrap_body` in the coordinator's envelope, as the route takes it."""
        body = self.bootstrap_body(signer=signer, operation=operation)
        return canonicalize(seal(self.eco.E.COORD, "ProductionBootstrap", self.run_id, body, 10000))

    def bootstrap(self) -> dict[str, Any]:
        """Beacon 1 (BLS-verified), the coordinator-signed bootstrap, and its maturity."""
        self.eco.beacon(self.store, 1, bls=True)
        receipt: dict[str, Any] = self.store.production_bootstrap_v2(
            self.run_id, self.signed_bootstrap()
        )
        origins = tuple(o.origin_id for o in self.bootstrap_body().origins)
        self.escrow.mature("55" * 32, origins, 1)
        return receipt

    def admit_and_lock(self, units: int) -> None:
        """The fixture hotkey as an ACTIVE admission holding `units` locked of its coldkey's."""
        E = self.eco.E
        origin = self.bootstrap_body().origins[0].origin_id
        self.store._db.execute(
            "INSERT INTO admissions_v2(admission_id, run_id, hotkey, coldkey, state, clean_count, "
            "canary_blocks, strikes, pending_dispute, reason, policy_hash, received_beacon, "
            "seed_beacon, screen_until) VALUES(?,?,?,?,'ACTIVE',12,'1,2,3',0,0,'',?,1,1,?)",
            (H, self.run_id, E.HOT.ss58, E.COLD.ss58, H, 10**9),
        )
        lock = EscrowLock(
            operation_id="22" * 32,
            owner=E.COLD.ss58,
            units=units,
            origin_ids=[origin],
            admission_id=H,
            dispute_id=None,
            kind="LOCK_ADMISSION",
        )
        self.escrow.lock(lock, signer=E.COLD.ss58)


@contextmanager
def production_run(root: Path, *, cuda: bool = True, **economics: Any) -> Iterator[Production]:
    eco = load_fixture_module(*ECONOMICS_FIXTURE)
    patch = pytest.MonkeyPatch()
    store, run_id = eco.hosted(root, patch, cuda=cuda, **economics)
    try:
        yield Production(eco, store, run_id)
    finally:
        patch.undo()
        store.close_v2_notifications()
        store.close()


@contextmanager
def capacity_run(root: Path, *, reserve: bool) -> Iterator[tuple[ModuleType, ChallengeStore, Any]]:
    """The 32-worker capacity run of test_service_capacity_v2: (module, store, manifest). With
    `reserve` the owner-signed profile is admitted and round 0 holds its resource reservation."""
    capacity = load_fixture_module(*CAPACITY_FIXTURE)
    (root / "net").mkdir(parents=True)
    (root / "run").mkdir()
    network_generator = capacity.service.network.__wrapped__(
        root / "net", SimpleNamespace(param=None)
    )
    network = next(network_generator)
    # The fixture takes the pytest `request` where it builds the environment-pinned variant
    # (audit-integrity-2); the unpinned run asks for no parameter.
    tiny = capacity.tiny.__wrapped__
    extra = (
        (SimpleNamespace(param=None),) if "request" in inspect.signature(tiny).parameters else ()
    )
    run_generator = tiny(network, root / "run", *extra)
    try:
        run = next(run_generator)
        store, manifest = capacity.reserved(run) if reserve else run
        yield capacity, store, manifest
    finally:
        next(run_generator, None)
        next(network_generator, None)


@dataclass
class Creation:
    """A fresh store, and the owner-signed manifest envelope that would create a run on it."""

    store: ChallengeStore
    manifest: RunManifestV2
    raw: bytes

    def run_rows(self) -> int:
        return int(self.store._db.execute("SELECT COUNT(*) FROM runs").fetchone()[0])


@contextmanager
def creation(
    world: Any, root: Path, economics: EconomicsPolicyV2 | None, *, pinned: str | None = None
) -> Iterator[Creation]:
    """Run creation on an empty store for the service world's manifest with another economics
    policy: `economics` (stored with the other policy objects) or the bare hash `pinned`, whose
    object does not exist."""
    service = load_fixture_module(*SERVICE_FIXTURE)
    network = world.network
    body = network.manifest.body()
    body["network"]["economics_policy_hash"] = pinned or economics.digest()  # type: ignore[union-attr]
    manifest = RunManifestV2.model_validate(body)
    objects = LocalFSStore(root / "objects")
    for name in (*PolicyHashes.model_fields, "relay_registry_hash"):
        if name != "economics_policy_hash":
            objects.put(network.store.objects.get(getattr(manifest.network, name)))
    if economics is not None:
        objects.put(canonicalize(economics.body()))
    store = ChallengeStore(
        root / "state",
        network.store.params,
        service.COORD,
        service.OWNER.ss58,
        verify_beacon=network.store.verify_beacon,
        objects=objects,
    )
    try:
        store.push_beacon(service.fixture_beacon(1))
        raw = canonicalize(
            envelope_v2.seal(service.OWNER, "RunManifestV2", manifest.run_id(), manifest, 100)
        )
        yield Creation(store, manifest, raw)
    finally:
        store.close_v2_notifications()
        store.close()


def roster_entry(world: Any, index: int, admission_id: str, **changes: Any) -> dict[str, Any]:
    """The roster entry a coordinator would sign for the service world's identity `index`."""
    service = load_fixture_module(*SERVICE_FIXTURE)
    return {
        "hotkey": service.HOT[index].ss58,
        "slot": index,
        "q_i": f32hex(1),
        "admission_id": admission_id,
        "coldkey_group": service.COLD[index].ss58,
        "state": "ACTIVE",
        "eligible_weight": 4194304,
        **changes,
    }


def activate_identity(world: Any, index: int = 0) -> str:
    """Join, lock 1000 units and mark identity `index` ACTIVE by SQL (the 12-trial ladder has its
    own tests). Returns its admission id."""
    service = load_fixture_module(*SERVICE_FIXTURE)
    network = world.network
    joined = network.join(index)
    assert joined.status_code == 200, joined.text
    identity: str = joined.json()["admission_id"]
    lock = EscrowLock(
        operation_id=f"{index + 1:064x}",
        owner=service.COLD[index].ss58,
        units=1000,
        origin_ids=[network.origin_ids[index]],
        admission_id=identity,
        dispute_id=None,
        kind="LOCK_ADMISSION",
    )
    locked = network.client.post(
        network.url + "/escrow/lock", json=network.signed(service.COLD[index], "EscrowLock", lock)
    )
    assert locked.status_code == 200, locked.text
    reference = network.manifest.training.reference_spec
    screen = WorkScreenV2(
        evidence_kind="work-proof-v1",
        admission_id=identity,
        nonce=sha256_hex(b"nonce"),
        policy_hash=network.manifest.network.admission_policy_hash,
        image_digest=reference.image_digest,
        layout=reference.layout,
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
            for rank in range(reference.layout.n_gpus)
        ],
        artifact_hashes=[sha256_hex(b"artifact")],
    )
    with network.store._tx():
        network.store._db.execute(
            "UPDATE admissions_v2 SET state='ACTIVE',clean_count=12,screen_json=?,"
            "screen_until=1000000000 WHERE admission_id=?",
            (screen.model_dump_json(), identity),
        )
    return identity


def round_opening(world: Any, roster: list[dict[str, Any]]) -> RoundOpenV2:
    """Round 0 of the service world over `roster`, scheduled just after the latest beacon. Its
    start-state index is a placeholder, so only the roster gates can accept or refuse it."""
    network = world.network
    manifest = network.manifest
    theta = init_params(TrainConfig.from_manifest_v2(manifest).model)
    return RoundOpenV2(
        w=0,
        prev_final_hash="0" * 64,
        theta_hash=state_hash(theta),
        outer_state_hash="0" * 64,
        center_hash="0" * 64,
        roster_hash=sha256_hex(canonicalize(roster)),
        honeypot_commit="0" * 64,
        d_open=network.now + 1,
        d_assign=network.now + 2,
        d_commit=network.now + 30,
        d_audit=network.now + 31,
        d_upload=network.now + 150,
        d_final=network.now + 200,
        contract_version=2,
        policy_hashes=PolicyHashes.model_validate(
            {name: getattr(manifest.network, name) for name in PolicyHashes.model_fields}
        ),
        registry_epoch=0,
        start_state_index_hash="00" * 32,
        audit_mode="anchored-full",
        roster=roster,
    )
