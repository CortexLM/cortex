"""Shared builders of the ledger-invariants lot. Not collected: no test_ prefix.

Sibling test modules are loaded once per process by `load_fixture_module`. Take `actual_reward`,
`verified_shadow`, `ledger` and `setup` from the same module object: `VERIFIED_SHADOW_STATES`
lives in it, and a reward built by one copy is unknown to the replay stand-in of another.
"""

from __future__ import annotations

import hashlib
import importlib.util
import json
import sqlite3
import sys
import time
from collections.abc import Callable, Iterable, Iterator, Mapping, Sequence
from contextlib import contextmanager
from dataclasses import dataclass, replace
from pathlib import Path
from types import ModuleType, SimpleNamespace
from typing import Any

import httpx
from fastapi import FastAPI
from fastapi.testclient import TestClient
from starlette.routing import BaseRoute, Mount

from hypertrain.beacon.core import BeaconRound
from hypertrain.challenge.admission_store import AdmissionStore
from hypertrain.challenge.app import Config, create_app
from hypertrain.challenge.store import ChallengeError, ChallengeStore
from hypertrain.data.store import LocalFSStore
from hypertrain.ledger import Params
from hypertrain.ledger.escrow_v2 import (
    EscrowError,
    EscrowReceipt,
    EscrowV2,
    FinalityEvidence,
    Settlement,
    authority_message,
    digest,
    reward_origin_id,
)
from hypertrain.protocol.envelope_v2 import EnvelopeV2, seal
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages import Finalize
from hypertrain.protocol.messages_v2 import OriginAllocation, RewardFinalize

LOT = Path(__file__).resolve().parent
TESTS = LOT.parent
ESCROW_FIXTURE = ("li_escrow_fixture", "ledger/test_escrow_v2.py")
SERVICE_FIXTURE = ("li_service_fixture", "challenge/test_service_network_v2.py")

H = "11" * 32  # the fixture's own constant; test_infra_guards pins the equality
FROZEN_UNIX = 1_900_000_000
INTERNAL_BEARER = "integration-internal"
WORKER_BEARER = "integration-worker"
SLUG = "hypertrain"

Real = tuple[Any, FinalityEvidence, RewardFinalize]


def load_fixture_module(name: str, relative: str) -> ModuleType:
    """Import `tests/<relative>` by path once per process, under a stable unique name."""
    cached = sys.modules.get(name)
    if cached is not None:
        return cached
    spec = importlib.util.spec_from_file_location(name, TESTS / relative)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    try:
        spec.loader.exec_module(module)
    except BaseException:
        del sys.modules[name]
        raise
    return module


def error_code(call: Callable[[], object]) -> str:
    """Code of the EscrowError `call` raises. Any other outcome fails: a test pins a code."""
    try:
        call()
    except EscrowError as error:
        return error.code
    except Exception as error:
        raise AssertionError(
            f"expected an EscrowError, got {type(error).__name__}: {error}"
        ) from error
    raise AssertionError("expected an EscrowError, nothing was raised")


def memory_db() -> sqlite3.Connection:
    # isolation_level=None: EscrowV2.tx() issues BEGIN IMMEDIATE / SAVEPOINT itself.
    return sqlite3.connect(":memory:", isolation_level=None, check_same_thread=False)


def match_settlement(finality_hash: str, release_beacon: int = 20) -> Settlement:
    return Settlement(
        finality_hash=finality_hash,
        closed_dispute_root=H,
        release_beacon=release_beacon,
        unresolved=False,
        outcome="MATCH",
    )


def registered_settlements(table: Mapping[str, Settlement]) -> Callable[[str], Settlement]:
    """The store's callback: only a settlement record that exists is accepted."""

    def settlement(reference: str) -> Settlement:
        if reference not in table:
            raise EscrowError("UNACCEPTED_SETTLEMENT")
        return table[reference]

    return settlement


def registered_owners(table: Mapping[str, str]) -> Callable[[str], str]:
    """The store's callback: an unknown hotkey is a 403, not somebody else's coldkey."""

    def owner(hotkey: str) -> str:
        if hotkey not in table:
            raise ChallengeError(403, "hotkey is not current accepted identity")
        return table[hotkey]

    return owner


def known_replay(fixture: ModuleType) -> Callable[[FinalityEvidence], str]:
    """Replay stand-in: accepts only a tape the fixture really executed twice."""

    def replay(evidence: FinalityEvidence) -> str:
        if evidence.tape_hash not in fixture.VERIFIED_SHADOW_STATES:
            raise EscrowError("NO_REPLAY_ACCEPTANCE")
        return str(fixture.VERIFIED_SHADOW_STATES[evidence.tape_hash])

    return replay


def bare_escrow(
    fixture: ModuleType,
    setup: Any,
    *,
    replay: Callable[[FinalityEvidence], str] | None = None,
    settlement: Callable[[str], Settlement] | Mapping[str, Settlement] | None = None,
    owner_of: Callable[[str], str] | Mapping[str, str] | None = None,
    strict: bool = False,
) -> EscrowV2:
    """An EscrowV2 on a private in-memory database, no store.

    Permissive (the default) keeps the callbacks of `test_escrow_v2.ledger`: any settlement
    reference is an accepted MATCH of finality hash H (a reward origin needs its own finalize
    hash, so pass `settlement=lambda _: match_settlement(reward.finalize_hash)`) and any
    unknown hotkey belongs to OTHER, so a test about "the settlement must be accepted" proves
    nothing there. `strict=True` refuses what the store refuses: UNACCEPTED_SETTLEMENT unless
    the reference is registered, and a 403 ChallengeError for a hotkey that is not an owner
    key. A Mapping is a registry in both modes. `replay` defaults to `known_replay(fixture)`.
    """
    if isinstance(settlement, Mapping):
        settlement = registered_settlements(settlement)
    elif settlement is None and strict:
        settlement = registered_settlements({})
    if isinstance(owner_of, Mapping):
        owner_of = registered_owners(owner_of)
    elif owner_of is None and strict:
        owner_of = registered_owners({fixture.HOT.ss58: fixture.COLD.ss58})
    escrow: EscrowV2 = fixture.ledger(
        setup, memory_db(), settlement=settlement, replay=replay or known_replay(fixture)
    )
    if owner_of is not None:
        escrow.owner_of = owner_of
    return escrow


def at(escrow: EscrowV2, rnd: int) -> int:
    """Chain time of drand round `rnd` for the hosted manifest."""
    beacon = escrow.manifest.training.beacon
    return beacon.genesis_time + (rnd - 1) * beacon.period


def mature(
    escrow: EscrowV2, reward: RewardFinalize, *, operation_id: str = "55" * 32
) -> EscrowReceipt:
    """Vest every origin of `reward` at its own maturity round."""
    return escrow.mature(operation_id, tuple(reward.origin_ids), reward.mature_at)


def pays(escrow: EscrowV2) -> int:
    return int(
        escrow.db.execute("SELECT COUNT(*) FROM escrow_events WHERE kind='PAY'").fetchone()[0]
    )


def mass(answer: Mapping[str, Any]) -> int:
    """Weight mass of a get_weights answer: what it pays plus what the epoch burns."""
    return int(sum(answer["weights"].values())) + int(answer["metadata"]["units_burned_this_epoch"])


def resign_reward(fixture: ModuleType, reward: RewardFinalize, **changes: Any) -> RewardFinalize:
    """`reward` with `changes` applied and the authority signature made again by COORD."""
    unsigned = RewardFinalize.model_validate(
        {**reward.body(), **changes, "authority_sig": "0" * 128}
    )
    signature = fixture.COORD.sign(authority_message(unsigned)).hex()
    return RewardFinalize.model_validate({**unsigned.body(), "authority_sig": signature})


def signed_variant(
    fixture: ModuleType, real: Real, **final_changes: Any
) -> tuple[RewardFinalize, FinalityEvidence]:
    """A COORD-signed Finalize/RewardFinalize pair differing from the fixture's Finalize.

    The origin split is recomputed for the fixture's single audited worker, so a refused
    variant is refused for its Finalize content and not for a malformed allocation.
    """
    setup, evidence, reward = real
    run_id = setup.manifest.run_id()
    final = Finalize.model_validate({**evidence.finalize.body, **final_changes})
    envelope = EnvelopeV2.model_validate(seal(fixture.COORD, "Finalize", run_id, final, 200))
    work = evidence.works[0]
    origin = OriginAllocation(
        origin_id=reward_origin_id(
            run_id, reward.w, work.commit.signer, digest(final), fixture.ID_KEY
        ),
        owner=work.owner,
        units=reward.budget_units,
        mature_at=reward.mature_at,
    )
    changed = resign_reward(
        fixture,
        reward,
        finalize_hash=digest(final),
        origin_ids=[origin.origin_id],
        allocation_hash=sha256_hex(canonicalize([origin.body()])),
    )
    return changed, replace(evidence, finalize=envelope)


@dataclass
class Hosted:
    """A ChallengeStore and the EscrowV2 of one run sharing one connection and one lock."""

    store: ChallengeStore
    escrow: EscrowV2
    reward: RewardFinalize
    evidence: FinalityEvidence
    run_id: str
    fraud: set[str]
    real_settlement: bool

    def finalize(self) -> EscrowReceipt:
        """What the service does at finalize: the reward and, with the real callback, the
        accepted settlement record of each origin, in one transaction."""
        with self.store._tx():
            receipt = self.escrow.reward_finalize(self.reward, self.evidence)
            if self.real_settlement:
                for origin in self.reward.origin_ids:
                    self.store._put_record_v2(
                        self.run_id,
                        "settlement",
                        origin,
                        Settlement(
                            finality_hash=self.reward.finalize_hash,
                            closed_dispute_root=sha256_hex(canonicalize([])),
                            release_beacon=self.reward.mature_at,
                            unresolved=False,
                            outcome="MATCH",
                        ).body(),
                    )
        return receipt

    def answer(self, epoch: int, rnd: int = 20) -> dict[str, Any]:
        """The epoch answer of `ChallengeStore.weights` for an epoch ending at drand round rnd."""
        answer: dict[str, Any] = json.loads(self.store.weights(epoch, at(self.escrow, rnd), 1))
        return answer


@contextmanager
def hosted_reward(
    root: Path, fixture: ModuleType, real: Real, *, real_settlement: bool
) -> Iterator[Hosted]:
    """Host the real w=0 reward in a store; nothing is finalized yet.

    `real_settlement=False` is the `test_v2_weights_pay.paying` stub (MATCH, or FRAUD for a
    reference added to `Hosted.fraud`); True is `ChallengeStore._settlement_v2`, which accepts
    only the records `Hosted.finalize` writes. The settlement is the only real callback: the
    store's `_services` is replaced on this private instance by the escrow built here.
    """
    setup, evidence, reward = real
    run_id = setup.manifest.run_id()
    store = ChallengeStore(
        root / "state",
        # The v1 genesis deliberately differs from the drand genesis the maturity round uses.
        Params(SLUG, setup.manifest.training.beacon.genesis_time - 600, 300, 1, 1),
        None,
        None,
        lambda payload: None,
        LocalFSStore(root / "objects"),
    )
    fraud: set[str] = set()

    def stub(reference: str) -> Settlement:
        return Settlement(
            finality_hash=reward.finalize_hash,
            closed_dispute_root=H,
            release_beacon=reward.mature_at,
            unresolved=False,
            outcome="FRAUD" if reference in fraud else "MATCH",
        )

    try:
        escrow: EscrowV2 = fixture.ledger(
            setup,
            store._db,
            replay=known_replay(fixture),
            settlement=(lambda ref: store._settlement_v2(run_id, ref)) if real_settlement else stub,
        )
        escrow.mutex = store._lock
        AdmissionStore(escrow)  # the tables the real services create; weights() reads them
        store._db.execute(
            "INSERT INTO runs VALUES(?,?,?,'created',NULL)",
            (run_id, setup.manifest.model_dump_json(), "{}"),
        )
        store._services = lambda _run_id: (escrow, None, None)
        AdmissionStore(escrow)  # the tables the real services create; _payee_v2 reads them
        yield Hosted(store, escrow, reward, evidence, run_id, fraud, real_settlement)
    finally:
        store.close()


@dataclass(frozen=True, order=True)
class Route:
    method: str  # an HTTP method, or MOUNT for a sub-application
    path: str
    name: str


def routes(app: FastAPI) -> list[Route]:
    """Every (method, path) the application serves, mounts included and their routes prefixed."""

    def walk(items: Iterable[BaseRoute], prefix: str) -> Iterator[Route]:
        for item in items:
            path = prefix + getattr(item, "path", "")
            if isinstance(item, Mount):
                yield Route("MOUNT", path, item.name or "")
                yield from walk(item.routes, path)
                continue
            for method in sorted(getattr(item, "methods", None) or {"WEBSOCKET"}):
                yield Route(method, path, getattr(item, "name", ""))

    return sorted(walk(app.routes, ""))


@contextmanager
def frozen_time(at_unix: float) -> Iterator[None]:
    original = time.time
    time.time = lambda: at_unix
    try:
        yield
    finally:
        time.time = original


def unverified_beacon(data: Mapping[str, Any]) -> BeaconRound:
    return BeaconRound(data["round"], data["signature"], data["randomness"], False)


def table_names(db: sqlite3.Connection) -> list[str]:
    return [
        row[0]
        for row in db.execute(
            "SELECT name FROM sqlite_master WHERE type='table' "
            "AND name NOT LIKE 'sqlite_%' ORDER BY name"
        )
    ]


@dataclass
class World:
    """The v2 `network` fixture plus a second app on its store that knows all three bearers."""

    network: Any
    app: FastAPI
    client: TestClient
    admin: dict[str, str]
    internal: dict[str, str]
    worker: dict[str, str]

    @property
    def store(self) -> ChallengeStore:
        store: ChallengeStore = self.network.store
        return store

    @property
    def run_id(self) -> str:
        run_id: str = self.network.manifest.run_id()
        return run_id

    def count(self, table: str, where: str = "1", params: Sequence[Any] = ()) -> int:
        """Rows of `table` of the root store matching the SQL condition `where`."""
        with self.store._lock:
            if table not in table_names(self.store._db):
                raise LookupError(f"no table {table}")
            row = self.store._db.execute(
                f'SELECT COUNT(*) FROM "{table}" WHERE {where}', tuple(params)
            ).fetchone()
        return int(row[0])

    def db_digest(self) -> str:
        """Hash of every row of every table of the root store and its run sub-stores."""
        hasher = hashlib.sha256()
        for store in self.store.stores():
            with store._lock:
                for name in table_names(store._db):
                    rows = sorted(
                        repr(tuple(row)) for row in store._db.execute(f'SELECT * FROM "{name}"')
                    )
                    hasher.update(repr((name, rows)).encode())
        return hasher.hexdigest()

    def get_weights(
        self,
        epoch: int,
        *,
        epoch_at: int | None = None,
        headers: Mapping[str, str] | None = None,
    ) -> httpx.Response:
        """GET /internal/v1/get_weights; `headers={}` sends no credential at all."""
        query = f"epoch={epoch}" + ("" if epoch_at is None else f"&epoch_at={epoch_at}")
        return self.client.get(
            "/internal/v1/get_weights?" + query,
            headers=self.internal if headers is None else dict(headers),
        )


@contextmanager
def service_world(root: Path, *, now: int = FROZEN_UNIX) -> Iterator[World]:
    """The v2 `network` fixture built with `time.time` frozen at `now`, so the run id is a
    function of `now` alone, plus an app on the same store holding the internal, admin and
    worker bearers (the fixture's own app holds the admin bearer only)."""
    service = load_fixture_module(*SERVICE_FIXTURE)
    root.mkdir(parents=True, exist_ok=True)
    generator = service.network.__wrapped__(root, SimpleNamespace(param=None))
    network = None
    try:
        with frozen_time(now):
            network = next(generator)
        store = network.store
        genesis = network.manifest.training.beacon.genesis_time
        for name, token in (("internal", INTERNAL_BEARER), ("worker", WORKER_BEARER)):
            (root / name).write_text(token)
        app = create_app(
            Config(
                SLUG,
                store.state_dir,
                "https://master.test",
                root / "internal",
                root / "admin",
                root / "worker",
                root / "coord.key",
                store.owner_hotkey,
                store.params,
            ),
            clock=lambda: genesis + (network.now - 1) * 3,
            transport=service.subnet(),
            verify_beacon=unverified_beacon,
            objects=store.objects,
            _store=store,
        )
        yield World(
            network,
            app,
            TestClient(app),
            service.admin(),
            {
                "Authorization": f"Bearer {INTERNAL_BEARER}",
                "x-platform-challenge-slug": SLUG,
            },
            {"Authorization": f"Bearer {WORKER_BEARER}"},
        )
    finally:
        generator.close()
        if network is not None:
            network.store.close()
