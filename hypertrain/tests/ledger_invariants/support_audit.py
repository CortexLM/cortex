"""Builders private to the audit package. Not collected: no test_ prefix.

One real round is prepared once per pytest process and kept as snapshots of the store and its
state directory, one per stage: five SQL-activated identities (s0), round 0 committed by all of
them with one wrong delta hash (committed), audits scheduled (s1), two leases executed by the
master (s2), all five audits completed (s3), aggregated (s4), finalized (s5). A stage is built on
first use from the previous snapshot, and a test restores the stage it needs into a private
network, so the replays (4 s each) are paid once and a mutant run that needs s1 never builds s5.

The state directory is restored to the path it was written under: records name absolute
directories (anchors, audit geometry), so every network of a world lives in the same `live`
directory, one at a time.
"""

from __future__ import annotations

import hashlib
import itertools
import json
import shutil
import sqlite3
from collections.abc import Callable, Iterator
from contextlib import contextmanager
from dataclasses import dataclass, field
from pathlib import Path
from types import ModuleType, SimpleNamespace
from typing import Any

import httpx
import pytest

from env_pin_fixtures import honest_env
from hypertrain.challenge import audit_cooldown_v2
from hypertrain.challenge.store import ChallengeStore
from hypertrain.ledger.escrow_v2 import reward_origin_id
from hypertrain.protocol import envelope_v2
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages import Finalize
from hypertrain.protocol.messages_v2 import (
    AuditPolicyV2,
    EscrowLock,
    RunManifestV2,
    WorkScreenV2,
)
from hypertrain.protocol.messages_v2 import RankWorkResult as Rank

from .support import (
    FROZEN_UNIX,
    enforce_mismatch_sanction,
    frozen_time,
    load_fixture_module,
    table_names,
)

ROLLBACK = ("li_audit_rollback_fixture", "e2e/test_rollback_service_v2.py")
LIVE_ONLY = ("challenge.db", "challenge.db-wal", "challenge.db-shm")
REQUESTS = itertools.count(1)
# What ADR 0011 (audit-integrity) changed in the store, detected on the tree under test so that
# the module still reads an older tree: a lapse cools the auditor down instead of banning it, a
# job that ran out of attempts gets a second window before its miner is dropped, and an
# aggregation whose survivors cannot carry the round is refused by name. The exit reaper of Part 8
# retires a round that nobody left can carry (a null round, closed without reward) instead of
# leaving it PENDING for ever, and answers 409 ROUND_RETIRED to new work on it.
COOLS_DOWN = hasattr(ChallengeStore, "_cooling_until_v2")
REARMS = hasattr(ChallengeStore, "_rearm_audit_job_v2")
CHECKS_SURVIVORS = hasattr(ChallengeStore, "_require_survivors_v2")
RETIRES = hasattr(ChallengeStore, "_retire_round_v2")
SIGN_WINDOW = getattr(audit_cooldown_v2, "SIGN_WINDOW_ROUNDS", None)
WINDOWS = 2 if REARMS else 1
# A job nobody audited leaves the round without being a verdict on the round: the disposition of
# a round whose job was dropped is untouched, where an older tree voided the round for everybody.
DROP_DISPOSITION = "PENDING" if REARMS else "EXCLUDED"
# When every job of the round is dropped nobody can carry it, and a tree with the reaper closes it
# as RETIRED: still no reward and no void.
EMPTIED_DISPOSITION = "RETIRED" if RETIRES else DROP_DISPOSITION
# What a request to schedule the audits of a round with no audit window left is answered: the
# horizon check, or on a tree with the reaper the retirement that the missing window caused.
HORIZON_REFUSAL = (503, "audit absolute horizon exhausted")
NO_WINDOW_REFUSALS = {HORIZON_REFUSAL} | (
    {(409, "ROUND_RETIRED: round 0 reached no tape and takes no work")} if RETIRES else set()
)


class Halt(BaseException):
    """Raised by `Halting` to stop `execute_round` where the next request would be sent."""


class Halting:
    """Stands in for the TestClient of a network: every request passes through except the first
    POST whose URL ends with `suffix`, which stops the round exactly there."""

    def __init__(self, client: Any, suffix: str) -> None:
        self.client, self.suffix = client, suffix

    def __getattr__(self, name: str) -> Any:
        return getattr(self.client, name)

    def post(self, url: str, *args: Any, **kwargs: Any) -> Any:
        if url.endswith(self.suffix):
            raise Halt
        return self.client.post(url, *args, **kwargs)


def rollback_module() -> ModuleType:
    """tests/e2e/test_rollback_service_v2.py, loaded once: `execute_round` and its fixtures."""
    return load_fixture_module(*ROLLBACK)


@dataclass
class Snapshot:
    """The store and its state directory at one stage of the prepared round."""

    database: Path
    state: Path
    now: int


@dataclass
class Cast:
    """The five miners and the three auditors of a world, and who forges."""

    hot: list[Keypair]
    cold: list[Keypair]
    auditors: list[Keypair]  # AUDITORS[0], AUDITORS[1] and hot[3], a miner that is also allowlisted
    forger: Keypair
    keys: dict[str, Keypair] = field(default_factory=dict)

    def __post_init__(self) -> None:
        self.keys = {k.ss58: k for k in (*self.hot, *self.cold, *self.auditors)}

    @property
    def honest(self) -> list[Keypair]:
        return [k for k in self.hot if k.ss58 != self.forger.ss58]


@contextmanager
def activated_network(root: Path) -> Iterator[tuple[Any, Cast]]:
    """The v2 service network with five miner identities and three allowlisted auditors, built
    with `time.time` frozen so that the run id is a function of FROZEN_UNIX alone (the job id of
    a miner, and so the order of the queue, is then the same in every process)."""
    fixtures = rollback_module().fixtures
    patch = pytest.MonkeyPatch()
    hot = [Keypair(bytes([80 + i]) * 32) for i in range(5)]
    cold = [Keypair(bytes([90 + i]) * 32) for i in range(5)]
    auditors = [*fixtures.AUDITORS[:2], hot[3]]
    patch.setattr(fixtures, "HOT", hot)
    patch.setattr(fixtures, "COLD", cold)
    patch.setattr(fixtures, "AUDITORS", auditors)
    original_setup = fixtures.fixture.setup

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

    patch.setattr(fixtures.fixture, "setup", carry_setup)
    original_genesis = fixtures.Genesis

    def full_genesis(**fields: Any) -> Any:
        fields["total_units"] = sum(origin.units for origin in fields["origins"])
        return original_genesis(**fields)

    patch.setattr(fixtures, "Genesis", full_genesis)
    shutil.rmtree(root, ignore_errors=True)
    root.mkdir(parents=True)
    generator = fixtures.network.__wrapped__(root, SimpleNamespace(param=None))
    network = None
    try:
        with frozen_time(FROZEN_UNIX):
            network = next(generator)
        enforce_mismatch_sanction(network.store)
        run = network.manifest.run_id()
        # The forger's job is the first of the queue, so the first lease is its audit.
        forger = min(
            (k for k in hot if k not in auditors),
            key=lambda k: job_id(run, k.ss58),
        )
        yield network, Cast(hot, cold, auditors, forger)
    finally:
        generator.close()
        if network is not None:
            network.store.close()
        patch.undo()


def job_id(run_id: str, hotkey: str, w: int = 0) -> str:
    return sha256_hex(f"ht-audit-job/2|{run_id}|{w}|{hotkey}".encode())


def snapshot_of(network: Any, directory: Path) -> Snapshot:
    """An SQLite backup of the store and a copy of every other file of its state directory."""
    store: ChallengeStore = network.store
    directory.mkdir(parents=True)
    database = directory / "snapshot.db"
    target = sqlite3.connect(database)
    with store._lock:
        store._db.backup(target)
    target.close()
    state = directory / "state"
    shutil.copytree(store.state_dir, state, ignore=shutil.ignore_patterns(*LIVE_ONLY, "*.sock"))
    return Snapshot(database, state, network.now)


def restore(network: Any, snapshot: Snapshot) -> None:
    """Make a freshly built network of the same run the one the snapshot was taken from."""
    store: ChallengeStore = network.store
    store._services_v2.clear()
    source = sqlite3.connect(snapshot.database)
    with store._lock:
        source.backup(store._db)
    source.close()
    shutil.copytree(snapshot.state, store.state_dir, dirs_exist_ok=True)
    network.now = snapshot.now
    # Built from the restored tables here: a service is not constructible inside a transaction.
    store._services(network.manifest.run_id())
    network.push(network.now)


def activate_identities(network: Any, cast: Cast) -> None:
    """Join, fund and mark ACTIVE (12 clean trials) the five miners without running a trial."""
    spec = network.manifest.training.reference_spec
    # The size of a lock is the policy's: the ledger refuses an admission lock below a tenth of it.
    lock_units = network.store._services(network.manifest.run_id())[0].policy.S_min_units
    for i in range(5):
        if i:
            network.push(network.now + 101)
        response = network.join(i)
        assert response.status_code == 200, response.text
        identity = response.json()["admission_id"]
        lock = EscrowLock(
            operation_id=f"{i + 1:064x}",
            owner=cast.cold[i].ss58,
            units=lock_units,
            origin_ids=[network.origin_ids[i]],
            admission_id=identity,
            dispute_id=None,
            kind="LOCK_ADMISSION",
        )
        response = network.client.post(
            network.url + "/escrow/lock", json=network.signed(cast.cold[i], "EscrowLock", lock)
        )
        assert response.status_code == 200, response.text
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
                Rank(
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


def commit_round(network: Any, cast: Cast) -> None:
    """Round 0 for all five miners, up to the request that would schedule the audits. The forger
    commits a wrong delta hash over honest leaves and uploads no delta."""
    real = network.client
    network.client = Halting(real, "/audits")
    try:
        rollback_module().execute_round(
            network, 0, forge={cast.forger.ss58: {"delta_hash": "aa" * 32}}
        )
    except Halt:
        pass
    else:
        raise AssertionError("execute_round did not reach the audit scheduling request")
    finally:
        network.client = real


def receipt(network: Any, key: Keypair, commit_hash: str | None = None) -> dict[str, Any]:
    """A signed Receipt as the lease and execute routes expect it: commit_hash is the lease nonce
    for execute and anything unique for a lease request."""
    body = {
        "w": 0,
        "commit_hash": commit_hash or f"{next(REQUESTS):064x}",
        "received_round": network.now,
    }
    signed: dict[str, Any] = network.signed(key, "Receipt", body)
    return signed


def schedule(network: Any) -> httpx.Response:
    return network.client.post(network.url + "/admin/rounds/0/audits", headers=admin())


def lease(network: Any, key: Keypair) -> httpx.Response:
    return network.client.post(network.url + "/worker/lease", json=receipt(network, key))


def execute(network: Any, key: Keypair, job: dict[str, Any]) -> httpx.Response:
    return network.client.post(
        network.url + "/worker/jobs/" + job["job_id"] + "/execute",
        json=receipt(network, key, job["lease_nonce"]),
    )


def complete(network: Any, key: Keypair, job_id: str, verdict: dict[str, Any]) -> httpx.Response:
    return network.client.post(
        network.url + "/worker/jobs/" + job_id + "/complete",
        json=network.signed(key, "ReplayVerdict", verdict),
    )


def admin() -> dict[str, str]:
    headers: dict[str, str] = rollback_module().fixtures.admin()
    return headers


def audit(network: Any, key: Keypair) -> tuple[dict[str, Any], dict[str, Any]]:
    """`key` leases the next queued job, has the master replay it and signs the verdict the master
    computed: returns (the job, the verdict)."""
    leased = lease(network, key)
    assert leased.status_code == 200, leased.text
    job = leased.json()["body"]
    executed = execute(network, key, job)
    assert executed.status_code == 200, executed.text
    verdict: dict[str, Any] = executed.json()["verdict"]
    return job, verdict


def lease_rows(store: ChallengeStore) -> dict[str, dict[str, Any]]:
    """Every audit lease of the store by miner hotkey."""
    with store._lock:
        return {
            row["hotkey"]: dict(row)
            for row in store._db.execute("SELECT * FROM audit_leases_v2 ORDER BY id")
        }


def records(store: ChallengeStore, *kinds: str) -> dict[tuple[str, str], str]:
    """The records_v2 rows of `kinds` (all kinds if none is named) by (kind, id)."""
    with store._lock:
        rows = store._db.execute("SELECT kind,id,data FROM records_v2").fetchall()
    return {(r[0], r[1]): r[2] for r in rows if not kinds or r[0] in kinds}


def digest_of(store: ChallengeStore, *, without: tuple[str, ...] = ()) -> str:
    """Hash of every row of every table of the store and its run sub-stores, less the tables
    named in `without`."""
    hasher = hashlib.sha256()
    for part in store.stores():
        with part._lock:
            for name in table_names(part._db):
                if name in without:
                    continue
                rows = sorted(
                    repr(tuple(row)) for row in part._db.execute(f'SELECT * FROM "{name}"')
                )
                hasher.update(repr((name, rows)).encode())
    return hasher.hexdigest()


def canonical(body: dict[str, Any]) -> bytes:
    return canonicalize(body)


def reward_origin(
    run_id: str, hotkey: str, finalize_hash: str, w: int = 0, *, escrow: Any = None
) -> str:
    """The origin id of a live reward as `escrow` issues it: the ledger derives it (under its own
    key when it has one), the module function is the derivation of a ledger that has none."""
    if escrow is not None and hasattr(escrow, "reward_origin"):
        derived: str = escrow.reward_origin(w, hotkey, finalize_hash)
        return derived
    return reward_origin_id(run_id, w, hotkey, finalize_hash)


def finalize_request(network: Any, theta_hash: str, included: list[str]) -> dict[str, Any]:
    fixtures = rollback_module().fixtures
    final = Finalize(
        w=0, final_theta_hash_w1=theta_hash, included=sorted(included), entitlements_root="0" * 64
    )
    signed: dict[str, Any] = network.signed(fixtures.COORD, "Finalize", final)
    return signed


class Comparator:
    """The comparison an audit ends with (`execute_island_audit`), re-run on an executed lease with
    a launch that serves the publication the master already made: the training is not repeated,
    only the decision, so one committed value can be altered at a time."""

    def __init__(self, network: Any, miner: Keypair, row: dict[str, Any]) -> None:
        from hypertrain.protocol.messages_v2 import AuditJobV2, IslandJobV1

        self.network, self.miner, self.row = network, miner, row
        self.store: ChallengeStore = network.store
        self.run = network.manifest.run_id()
        self.job = AuditJobV2.model_validate(
            self.store._record_v2(self.run, "audit-job", row["id"])
        )
        self.commit: dict[str, Any] = dict(self.job.commit_envelope["body"])
        geometry = self.store._record_v2(self.run, "audit-geometry", f"{row['id']}:{row['nonce']}")
        descriptor = envelope_v2.load_json(self.store.objects.get(geometry["descriptor_hash"]))
        self.publication = IslandJobV1.model_validate(descriptor["job"])
        self.published = Path(descriptor["directory"])
        self.inputs = self.store.state_dir / "audit-v2" / row["id"] / row["nonce"]

    def outcome(
        self, *, commit: dict[str, Any] | None = None, preimage: dict[int, str] | None = None
    ) -> Any:
        from hypertrain.auditor.replay import AnchorCache
        from hypertrain.auditor.worker import LeaseGuard, execute_island_audit
        from hypertrain.miner.island_launch import validate_artifacts
        from hypertrain.protocol.messages_v2 import AuditJobV2

        changes: dict[str, Any] = {}
        if commit:
            sealed = envelope_v2.seal(
                self.miner, "CommitV2", self.run, {**self.commit, **commit}, 10000
            )
            changes["commit_envelope"] = sealed
        if preimage:
            leaves = [p.model_dump(mode="json") for p in self.job.preimages]
            for index, loss in preimage.items():
                leaves[index]["loss_f32"] = loss
            changes["preimages"] = leaves
        job = AuditJobV2.model_validate({**self.job.model_dump(mode="json"), **changes})
        cache = AnchorCache()
        self.store._restore_anchor_v2(self.run, self.miner.ss58, -1, cache)
        deadline = self.publication.deadline
        with LeaseGuard(self.row["expires"], self.row["absolute"], deadline) as guard:
            outcome, _ = execute_island_audit(
                job,
                self.inputs,
                cache,
                guard,
                now_round=self.network.now,
                deadline_unix=deadline,
                backend="cpu",
                objects=self.store.objects.get,
                publication_job=self.publication,
                publication_directory=self.published,
                launch=lambda j, d, **_: validate_artifacts(j, d / "published"),
            )
        return outcome


class World:
    """The prepared round and its stage snapshots; stages are built on first use."""

    def __init__(self, root: Path) -> None:
        self.root = root
        self.live = root / "live"
        self.snapshots: dict[str, Snapshot] = {}
        self.builders: dict[str, tuple[str | None, Callable[[Any, Cast], None]]] = {
            "s0": (None, self._build_s0),
            "committed": ("s0", self._build_committed),
            "s1": ("committed", self._build_s1),
            "s2": ("s1", self._build_s2),
            "s3": ("s2", self._build_s3),
            "s4": ("s3", self._build_s4),
            "s5": ("s4", self._build_s5),
        }

    @staticmethod
    def _build_s0(network: Any, cast: Cast) -> None:
        activate_identities(network, cast)

    @staticmethod
    def _build_committed(network: Any, cast: Cast) -> None:
        commit_round(network, cast)

    @staticmethod
    def _build_s1(network: Any, cast: Cast) -> None:
        response = schedule(network)
        assert response.status_code == 200, response.text
        assert len(response.json()["jobs"]) == 5

    @staticmethod
    def _build_s2(network: Any, cast: Cast) -> None:
        """The first auditor holds the forger's executed lease, the second an honest one."""
        for key in cast.auditors[:2]:
            audit(network, key)
        held = {r["hotkey"] for r in lease_rows(network.store).values() if r["state"] == "running"}
        assert cast.forger.ss58 in held

    @staticmethod
    def _build_s3(network: Any, cast: Cast) -> None:
        """Every audit completed: the two executed leases, then the three still queued."""
        store = network.store
        for row in lease_rows(store).values():
            if row["state"] == "running":
                verdict = store._record_v2(network.manifest.run_id(), "executed-verdict", row["id"])
                response = complete(network, cast.keys[row["auditor"]], row["id"], verdict)
                assert response.status_code == 200, response.text
        for key in itertools.islice(itertools.cycle(cast.auditors[:2]), 3):
            job, verdict = audit(network, key)
            response = complete(network, key, job["job_id"], verdict)
            assert response.status_code == 200, response.text
        assert {r["state"] for r in lease_rows(store).values()} == {"completed"}

    @staticmethod
    def _build_s4(network: Any, cast: Cast) -> None:
        response = network.client.post(network.url + "/admin/rounds/0/aggregate", headers=admin())
        assert response.status_code == 200, response.text

    @staticmethod
    def _build_s5(network: Any, cast: Cast) -> None:
        applied = network.store._record_v2(network.manifest.run_id(), "applied", "0")
        body = finalize_request(network, applied["theta_hash"], [k.ss58 for k in cast.honest])
        response = network.client.post(
            network.url + "/admin/rounds/0/finalize", json=body, headers=admin()
        )
        assert response.status_code == 200, response.text

    def stage(self, name: str) -> Snapshot:
        """The snapshot of a stage, built (with its predecessors) if this is its first use."""
        if name in self.snapshots:
            return self.snapshots[name]
        previous, build = self.builders[name]
        parent = None if previous is None else self.stage(previous)
        with activated_network(self.live) as (network, cast):
            if parent is not None:
                restore(network, parent)
            build(network, cast)
            snapshot = snapshot_of(network, self.root / name)
        self.snapshots[name] = snapshot
        return snapshot

    @contextmanager
    def at(self, name: str) -> Iterator[tuple[Any, Cast]]:
        """A private network restored to stage `name`; it is gone when the block ends."""
        snapshot = self.stage(name)
        with activated_network(self.live) as (network, cast):
            restore(network, snapshot)
            yield network, cast


SESSION_WORLDS: list[World] = []


def session_world(factory: pytest.TempPathFactory) -> World:
    """The one World of the pytest process. test_audit.py and test_audit_2.py both ask for it: a
    fixture imported into a second module is a second fixture, which would rebuild every stage."""
    if not SESSION_WORLDS:
        SESSION_WORLDS.append(World(factory.mktemp("audit-world")))
    return SESSION_WORLDS[0]


def detail(response: Any) -> tuple[int, str]:
    return response.status_code, response.json()["detail"]


def audit_kinds(store: ChallengeStore) -> dict[tuple[str, str], str]:
    """Every record an audit writes or reads, to compare before and after a refused call."""
    return records(
        store,
        "replay",
        "verdict",
        "executed-verdict",
        "excluded",
        "contest",
        "finality",
        "audit-job",
        "audit-geometry",
        "audit-execution",
        "audit-attempt-failure",
        "audit-rearm",
        "auditor-disabled",
        "auditor-cooldown",
    )


def running(store: ChallengeStore) -> dict[str, dict[str, Any]]:
    return {h: r for h, r in lease_rows(store).items() if r["state"] == "running"}


def honest_lease(store: ChallengeStore, cast: Cast) -> dict[str, Any]:
    """The executed lease of stage s2 that is not the forger's. The queue is ordered by job id, a
    hash of the run id, so which honest miner it belongs to changes with the manifest: a test
    reads the row and takes the miner from its hotkey."""
    (row,) = (r for h, r in running(store).items() if h != cast.forger.ss58)
    return row


def open_rows(store: ChallengeStore) -> list[dict[str, Any]]:
    """The jobs that are neither completed nor exhausted."""
    return [r for r in lease_rows(store).values() if r["state"] in ("queued", "running")]


def run_out_the_windows(network: Any, store: ChallengeStore) -> int:
    """Push the beacon to the end of the window of the open jobs until none is open; returns how
    many windows that took (one, or two on a tree that re-arms an exhausted job once)."""
    windows = 0
    while open_rows(store):
        assert windows < WINDOWS, "an audit job is still open after its last window"
        network.push(max(r["absolute"] for r in open_rows(store)))
        windows += 1
    return windows


def drop_job(network: Any, store: ChallengeStore, hotkey: str) -> None:
    """Push the beacon to the end of every window of one job, whatever the others do, until it
    is exhausted."""
    for _ in range(WINDOWS):
        row = lease_rows(store)[hotkey]
        if row["state"] == "exhausted":
            return
        network.push(row["absolute"])
    assert lease_rows(store)[hotkey]["state"] == "exhausted"


def drop_job_alone(network: Any, store: ChallengeStore, hotkey: str, within: int = 5) -> None:
    """Exhaust one job through every window it has while the other jobs keep theirs. The jobs of a
    round share one deadline, so this test-owned stand-in ends each window of the job `within`
    rounds after the beacon: the counter can then be read between two exhaustions, and the beacon
    stays before the lapse of the leases that are held."""
    for _ in range(WINDOWS):
        row = lease_rows(store)[hotkey]
        if row["state"] == "exhausted":
            return
        target = network.now + within
        with store._tx():
            store._db.execute(
                "UPDATE audit_leases_v2 SET absolute=? WHERE id=?", (target, row["id"])
            )
        network.push(target)
    assert lease_rows(store)[hotkey]["state"] == "exhausted"


def audit_horizon(store: ChallengeStore, run_id: str, w: int = 0) -> int:
    """The `absolute` of the audit jobs of round w. The last round at which the audits can still
    be scheduled is this minus two; from this minus one there is no audit window left."""
    body = store._record_v2(run_id, "round", str(w))["body"]
    return int(
        min(body["d_final"], body["d_audit"] + audit_policy(store, run_id).absolute_rounds())
    )


def audit_policy(store: ChallengeStore, run_id: str) -> AuditPolicyV2:
    manifest = store._run_v2(run_id)
    return AuditPolicyV2.model_validate_json(store.objects.get(manifest.network.audit_policy_hash))


def silence_of(
    store: ChallengeStore, run_id: str, row: dict[str, Any]
) -> tuple[str, dict[str, Any], tuple[int, str]]:
    """What a lease that lapsed unanswered leaves against its holder, as (the kind of the record
    kept under the holder's hotkey, that record, the refusal the holder meets when it asks again).
    A tree that cools the auditor down releases it `lease_rounds` after the lapse, which is the
    expiry of the lease, or on a tree that revokes (ADR 0011 Part 9) the end of the sign window
    of a verdict the master stored, if that comes first."""
    if not COOLS_DOWN:
        return (
            "auditor-disabled",
            {"job_id": row["id"], "reason": "LEASE_EXPIRED"},
            (403, "silent auditor disabled pending review"),
        )
    since = row["expires"]
    marker = records(store, "audit-execution").get(
        ("audit-execution", f"{row['id']}:{row['nonce']}")
    )
    if SIGN_WINDOW is not None and marker is not None and json.loads(marker)["state"] == "EXECUTED":
        since = min(since, json.loads(marker)["at"] + SIGN_WINDOW)
    until = since + audit_policy(store, run_id).lease_rounds
    record = {
        "job_id": row["id"],
        "reason": "AUDITOR_SILENT",
        "strikes": 1,
        "since": since,
        "until": until,
    }
    return "auditor-cooldown", record, (403, f"auditor cooldown until round {until}")


def settled_state(store: ChallengeStore) -> tuple[list[dict[str, Any]], dict[tuple[str, str], str]]:
    return list(lease_rows(store).values()), audit_kinds(store)
