"""Staged worlds and raw service calls for test_finalize_and_epoch_service.py.

World.settle() runs a round from audit to finalize in one go and records what each step answered.
The finalize and get_weights scenarios need the stops in between: a round that was uploaded but
never audited, one with four of five audits complete, one fully audited but not aggregated. Playing
a round up to each stop costs half a minute, so a stage is captured once (SQLite backup plus the
state tree, the same copy ensure_snapshot makes for the graduated network) and every test restores
its own World from it in about a second. A stage is captured before any get_weights answer, so the
v1 ledger journal restored with it is still at its init record.
"""

from __future__ import annotations

import json
import os
import shutil
import sqlite3
import threading
import traceback
from collections.abc import Callable, Iterator
from contextlib import contextmanager
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from ac_world import GraduatedSnapshot, World
from httpx import Response

from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages import Finalize
from hypertrain.protocol.messages_v2 import DisputeV2, EscrowLock

PERIOD = 3  # seconds per drand round of the fixture beacon


@dataclass(frozen=True)
class Stage:
    """A captured World: where its files are, how many audit leases it spent, who is in it."""

    root: Path
    source: str
    leases: int
    roster: tuple[str, ...]
    cheater: str | None

    @property
    def honest(self) -> tuple[str, ...]:
        return tuple(h for h in self.roster if h != self.cheater)


def freeze(world: World, root: Path, cheater: str | None) -> Stage:
    """Copy the world's database and state tree under `root`."""
    store = world.network.store
    root.mkdir(parents=True)
    with sqlite3.connect(root / "challenge.db") as copy:
        store._db.backup(copy)
    for entry in store.state_dir.iterdir():
        if entry.is_dir():
            shutil.copytree(entry, root / entry.name)
    roster = tuple(e.hotkey for e in world.opening(0).roster)
    return Stage(root, str(store.state_dir), world._leases, roster, cheater)


def thaw(snapshot: GraduatedSnapshot, stage: Stage, scratch: Path) -> World:
    """A new World (graduated clone) holding the captured stage; the caller installs it."""
    world = snapshot.clone(scratch)
    n = world.network
    try:
        n.store._services_v2.clear()
        with sqlite3.connect(stage.root / "challenge.db") as source:
            source.backup(n.store._db)
        # Anchor records hold the absolute path they were written to; point them at the copy.
        n.store._db.execute(
            "UPDATE records_v2 SET data=replace(data,?,?) WHERE instr(data,?)>0",
            (stage.source, str(n.store.state_dir), stage.source),
        )
        for entry in stage.root.iterdir():
            if entry.is_dir():
                shutil.copytree(entry, n.store.state_dir / entry.name, dirs_exist_ok=True)
        n.now = n.store._now(n.store._db)
        n.push(n.now)
        # Built here because EscrowV2 sets a PRAGMA that SQLite refuses inside a transaction,
        # and the first aggregate or finalize would otherwise build it inside one.
        n.store._services(world.run_id)
    except BaseException:
        world.close()
        raise
    world._leases = stage.leases
    return world


# -- audits, one stop at a time ------------------------------------------------------------------


@dataclass(frozen=True)
class Executed:
    """An audit that was leased and replayed, whose signed verdict is not yet posted."""

    job_id: str
    hotkey: str
    auditor: Any
    verdict: dict[str, Any]


def post(world: World, path: str, body: Any = None, admin: bool = False) -> Response:
    n = world.network
    headers = world.svc.admin() if admin else None
    return n.client.post(n.url + path, json=body, headers=headers)


def schedule_audits(world: World, w: int) -> Response:
    """Push d_audit when needed and queue one audit per rostered miner."""
    opening = world.opening(w)
    if opening.d_audit > world.network.now:
        world.network.push(opening.d_audit)
    return post(world, f"/admin/rounds/{w}/audits", admin=True)


def _receipt(world: World, auditor: Any, w: int, nonce: str) -> Any:
    n = world.network
    return n.signed(auditor, "Receipt", {"w": w, "commit_hash": nonce, "received_round": n.now})


def lease_next(world: World, w: int, auditor_index: int = 0) -> Response:
    """POST /worker/lease as auditor `auditor_index` with a fresh single-use receipt."""
    auditor = world.svc.AUDITORS[auditor_index]
    world._leases += 1
    nonce = sha256_hex(f"audit-cheaters-lease|{world._leases}".encode())
    return post(world, "/worker/lease", _receipt(world, auditor, w, nonce))


def execute_leased(world: World, w: int, auditor_index: int, leased: Response) -> Executed:
    """Replay the audit that `leased` (a 200 answer of lease_next) handed to the auditor."""
    assert leased.status_code == 200, leased.text
    job = leased.json()["body"]
    auditor = world.svc.AUDITORS[auditor_index]
    done = post(
        world,
        f"/worker/jobs/{job['job_id']}/execute",
        _receipt(world, auditor, w, job["lease_nonce"]),
    )
    assert done.status_code == 200, done.text
    body = done.json()
    return Executed(job["job_id"], body["identity"].split(":", 1)[1], auditor, body["verdict"])


def execute_next(world: World, w: int, auditor_index: int = 0) -> Executed | None:
    """Lease the next queued audit and replay it; None when no audit is queued."""
    leased = lease_next(world, w, auditor_index)
    if leased.status_code == 204:
        return None
    return execute_leased(world, w, auditor_index, leased)


def complete(world: World, executed: Executed, verdict: dict[str, Any] | None = None) -> Response:
    """Post the signed verdict of an executed audit (the replayed one unless `verdict` is given)."""
    signed = world.network.signed(
        executed.auditor, "ReplayVerdict", executed.verdict if verdict is None else verdict
    )
    return post(world, f"/worker/jobs/{executed.job_id}/complete", signed)


def run_audits(world: World, w: int, limit: int | None = None) -> list[Executed]:
    """Execute and complete up to `limit` queued audits (all of them by default)."""
    done: list[Executed] = []
    while limit is None or len(done) < limit:
        executed = execute_next(world, w, len(done) % 2)
        if executed is None:
            break
        accepted = complete(world, executed)
        assert accepted.status_code == 200, accepted.text
        done.append(executed)
    return done


# -- aggregate, finalize, get_weights ------------------------------------------------------------


def aggregate(world: World, w: int) -> Response:
    return post(world, f"/admin/rounds/{w}/aggregate", admin=True)


def signed_finalize(
    world: World,
    w: int,
    theta_hash: str,
    included: list[str],
    entitlements_root: str = "0" * 64,
) -> Any:
    final = Finalize(
        w=w,
        final_theta_hash_w1=theta_hash,
        included=sorted(included),
        entitlements_root=entitlements_root,
    )
    return world.network.signed(world.svc.COORD, "Finalize", final)


def finalize(world: World, w: int, signed: Any) -> Response:
    return post(world, f"/admin/rounds/{w}/finalize", signed, admin=True)


def at_round(world: World, rnd: int) -> int:
    """Unix time of drand round `rnd` of the fixture beacon."""
    return int(world.network.manifest.training.beacon.genesis_time + (rnd - 1) * PERIOD)


def ask_weights(world: World, epoch: int, rnd: int) -> Response:
    """GET /internal/v1/get_weights for `epoch` as of drand round `rnd`.

    The epoch and its time are explicit so that a scenario can ask several epochs in order
    without moving the beacon; the internal token is a throwaway as in World.weights."""
    n = world.network
    route = next(
        r for r in n.client.app.routes if getattr(r, "path", "") == "/internal/v1/get_weights"
    )
    config = route.endpoint.__closure__[
        route.endpoint.__code__.co_freevars.index("config")
    ].cell_contents
    token = os.urandom(16).hex()
    token_file = world.scratch / "internal"
    token_file.write_text(token)
    token_file.chmod(0o600)
    config.internal_token_file = token_file
    return n.client.get(
        "/internal/v1/get_weights",
        params={"epoch": epoch, "epoch_at": at_round(world, rnd)},
        headers={"Authorization": f"Bearer {token}", "x-platform-challenge-slug": config.slug},
    )


def reward_origins(world: World) -> list[tuple[str, int, int]]:
    """(hotkey, issued units, mature_at) of every reward origin, ordered by hotkey."""
    rows = world.network.store._db.execute(
        "SELECT hotkey,issued,mature_at FROM escrow_origins "
        "WHERE reward_round IS NOT NULL ORDER BY hotkey"
    )
    return [(r[0], r[1], r[2]) for r in rows]


def rows(world: World, query: str, *args: Any) -> list[tuple[Any, ...]]:
    return [tuple(r) for r in world.network.store._db.execute(query, args)]


def state_digest(world: World) -> str:
    """One hash of every row that a finalize can write: records, escrow events, origins, units."""
    db = world.network.store._db
    tables = ("records_v2", "escrow_events", "escrow_origins", "escrow_units", "escrow_finalized")
    dump = {t: sorted(repr(tuple(r)) for r in db.execute(f"SELECT * FROM {t}")) for t in tables}
    return sha256_hex(canonicalize(dump))


@dataclass(frozen=True)
class AuditRecord:
    """A completed audit as the master stored it: the job, its auditor and the signed verdict."""

    job_id: str
    hotkey: str
    auditor: Any
    signed: dict[str, Any]
    verdict: dict[str, Any]


def audit_records(world: World, w: int = 0) -> list[AuditRecord]:
    store, rid = world.network.store, world.run_id
    leases = store._db.execute(
        "SELECT id,hotkey,auditor FROM audit_leases_v2 "
        "WHERE run_id=? AND w=? AND state='completed' ORDER BY hotkey",
        (rid, w),
    ).fetchall()
    return [
        AuditRecord(
            lease["id"],
            lease["hotkey"],
            next(k for k in world.svc.AUDITORS if k.ss58 == lease["auditor"]),
            store._record_v2(rid, "verdict", f"{w}:{lease['hotkey']}"),
            store._record_v2(rid, "executed-verdict", lease["id"]),
        )
        for lease in leases
    ]


def open_contest(world: World, miner_index: int, bond: int = 100) -> tuple[Response, Response]:
    """The miner's funded contest of its own round-0 verdict: the coldkey locks the bond, then
    the hotkey posts the dispute (the two POSTs of tests/e2e/test_rollback_service_v2.py)."""
    n, store = world.network, world.network.store
    hot, cold = world.svc.HOT[miner_index], world.svc.COLD[miner_index]
    contest = json.loads(
        store._db.execute(
            "SELECT data FROM records_v2 WHERE kind='contest' AND json_extract(data,'$.miner')=? "
            "AND json_extract(data,'$.w')=0",
            (hot.ss58,),
        ).fetchone()[0]
    )
    dispute_id = sha256_hex(canonicalize([world.run_id, contest["verdict_hash"], hot.ss58]))
    origin = store._db.execute(
        "SELECT origin FROM escrow_units WHERE owner=? AND bucket='available' AND units>=?",
        (cold.ss58, bond),
    ).fetchone()[0]
    lock = EscrowLock(
        operation_id=sha256_hex(f"audit-cheaters-bond|{dispute_id}".encode()),
        owner=cold.ss58,
        units=bond,
        origin_ids=[origin],
        admission_id=None,
        dispute_id=dispute_id,
        kind="LOCK_CONTEST",
    )
    locked = n.client.post(n.url + "/escrow/lock", json=n.signed(cold, "EscrowLock", lock))
    dispute = DisputeV2(
        hotkey=hot.ss58,
        verdict_hash=contest["verdict_hash"],
        action="contest",
        bond_lock=lock.operation_id,
    )
    return locked, n.client.post(n.url + "/dispute", json=n.signed(hot, "DisputeV2", dispute))


class _AuditedConnection:
    """The store's SQLite connection, recording each statement issued without the store lock."""

    def __init__(self, real: Any, lock: Any, unlocked: list[str]) -> None:
        self._real, self._lock, self._unlocked = real, lock, unlocked

    def execute(self, sql: str, *args: Any) -> Any:
        if not self._lock._is_owned():
            self._unlocked.append(" ".join(sql.split())[:90])
        return self._real.execute(sql, *args)

    def __getattr__(self, name: str) -> Any:
        return getattr(self._real, name)


@contextmanager
def lock_audit(world: World) -> Iterator[list[str]]:
    """Yield the statements the store runs on its shared connection while the calling thread does
    not hold the store lock; the connection is put back on exit. One thread suffices: the store
    serves every request on one connection, so an unlocked statement is a race with any other."""
    store = world.network.store
    real = store._db
    unlocked: list[str] = []
    store._db = _AuditedConnection(real, store._lock, unlocked)
    try:
        yield unlocked
    finally:
        store._db = real


def open_with_final_gap(world: World, gap: int) -> str | None:
    """Open round 0 with d_final = d_audit + gap (the other offsets as the smoke opening sets
    them); the master's refusal text, or None when it accepted the opening."""
    n = world.network
    real = n.signed

    def signed(signer: Any, name: str, model: Any) -> Any:
        if name == "RoundOpenV2":
            model = model.model_copy(update={"d_final": model.d_audit + gap})
        return real(signer, name, model)

    n.signed = signed
    try:
        world.open_round(0)
    except RuntimeError as refusal:
        return str(refusal)
    finally:
        n.signed = real
    return None


def run_together(calls: list[Callable[[], Response]]) -> list[Response]:
    """Run every call in its own thread, released together by a barrier; answers in call order."""
    barrier = threading.Barrier(len(calls))
    answers: list[Response | None] = [None] * len(calls)
    failures: list[str] = []

    def body(index: int, call: Callable[[], Response]) -> None:
        barrier.wait(timeout=30)
        try:
            answers[index] = call()
        except BaseException as error:  # reported to the test thread, never swallowed
            failures.append("".join(traceback.format_exception(error)))

    threads = [threading.Thread(target=body, args=(i, c)) for i, c in enumerate(calls)]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join(timeout=180)
        assert not thread.is_alive(), "a request did not finish"
    assert not failures, failures
    return [a for a in answers if a is not None]
