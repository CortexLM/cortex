"""Helpers of the escrow gap tests (test_gaps_escrow*.py). Not collected: no test_ prefix.

The static scan reads the source of the package, never its behaviour: it lists where the credit
tables are written so a test can compare the list with the writers the ledger relies on.
"""

from __future__ import annotations

import ast
import re
from collections.abc import Callable, Iterator
from contextlib import contextmanager
from pathlib import Path
from types import ModuleType
from typing import Any

import httpx

import hypertrain
from hypertrain.auditor.island_bisect import RefereeEvidence
from hypertrain.challenge.admission_store import AdmissionError
from hypertrain.challenge.disputes_v2 import Turn
from hypertrain.challenge.store import ChallengeStore
from hypertrain.ledger.escrow_v2 import EscrowV2, Settlement
from hypertrain.miner import admission as miner_admission
from hypertrain.protocol.envelope_v2 import seal
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages_v2 import EscrowLock, EscrowRelease, ResolutionV2, RotateRequest

from .support import World, service_world
from .support_escrow_laws import Funded, funded, lock_op, release_op
from .support_locks import Lab, Miner, Rig, ident, settled_status
from .support_opids import DisputeWorld, fresh_id, sink_transfer
from .support_weights import Metagraph, add_origin, live_gate

SOURCE = Path(hypertrain.__file__).parent

# The write verbs that matter to an append-only table, the way a statement spells them.
WRITE = re.compile(
    r"\b(?P<verb>INSERT(?:\s+OR\s+\w+)?\s+INTO|REPLACE\s+INTO|UPDATE|DELETE\s+FROM"
    r"|DROP\s+TABLE|ALTER\s+TABLE)\s+\"?(?P<table>\w+)",
    re.IGNORECASE,
)

Site = tuple[str, str, str, str]  # (module, function, verb, table)

# What stores a record by kind: the writer itself and the one that stores a derived fact only when
# it changed (a pass-through: its callers name the kind, so they are scanned as record stores).
RECORD_STORES = frozenset({"_put_record_v2", "_put_changed_v2"})


def statements(tree: ast.AST) -> Iterator[tuple[str, str]]:
    """(innermost function name, string constant) of every string in a module."""
    stack: list[str] = []

    def walk(node: ast.AST) -> Iterator[tuple[str, str]]:
        named = isinstance(node, ast.FunctionDef | ast.AsyncFunctionDef)
        if named:
            stack.append(node.name)  # type: ignore[union-attr]
        if isinstance(node, ast.Constant) and isinstance(node.value, str):
            yield (stack[-1] if stack else "<module>"), node.value
        for child in ast.iter_child_nodes(node):
            yield from walk(child)
        if named:
            stack.pop()

    yield from walk(tree)


def calls(tree: ast.AST) -> Iterator[tuple[str, ast.Call]]:
    """(innermost function name, call) of every call in a module."""
    stack: list[str] = []

    def walk(node: ast.AST) -> Iterator[tuple[str, ast.Call]]:
        named = isinstance(node, ast.FunctionDef | ast.AsyncFunctionDef)
        if named:
            stack.append(node.name)  # type: ignore[union-attr]
        if isinstance(node, ast.Call):
            yield (stack[-1] if stack else "<module>"), node
        for child in ast.iter_child_nodes(node):
            yield from walk(child)
        if named:
            stack.pop()

    yield from walk(tree)


def callers_of(name: str) -> set[tuple[str, str]]:
    """(module, function) of every call to a function or method called `name` in the package."""
    found: set[tuple[str, str]] = set()
    for path in sorted(SOURCE.rglob("*.py")):
        module = path.relative_to(SOURCE).as_posix()
        for function, call in calls(ast.parse(path.read_text())):
            if getattr(call.func, "attr", getattr(call.func, "id", "")) == name:
                found.add((module, function))
    return found


def sql_writers(tables: frozenset[str]) -> set[Site]:
    """Every (module, function, verb, table) of an SQL write to one of `tables` in the package."""
    found: set[Site] = set()
    for path in sorted(SOURCE.rglob("*.py")):
        module = path.relative_to(SOURCE).as_posix()
        for function, text in statements(ast.parse(path.read_text())):
            for match in WRITE.finditer(text):
                table = match["table"].lower()
                if table in tables:
                    verb = " ".join(match["verb"].upper().split())
                    found.add((module, function, verb, table))
    return found


def record_writers(kinds: frozenset[str]) -> tuple[set[tuple[str, str, str]], set[tuple[str, str]]]:
    """Who stores a record of one of `kinds` through one of the `RECORD_STORES`.

    Returns the (module, function, kind) of every call whose kind is a literal in `kinds`, and
    the (module, function) of every call whose kind is not a literal at all.
    """
    literal: set[tuple[str, str, str]] = set()
    dynamic: set[tuple[str, str]] = set()
    for path in sorted(SOURCE.rglob("*.py")):
        module = path.relative_to(SOURCE).as_posix()
        for function in ast.walk(ast.parse(path.read_text())):
            if not isinstance(function, ast.FunctionDef | ast.AsyncFunctionDef):
                continue
            for call in ast.walk(function):
                if not (
                    isinstance(call, ast.Call)
                    and getattr(call.func, "attr", getattr(call.func, "id", "")) in RECORD_STORES
                    and len(call.args) >= 2
                ):
                    continue
                kind = call.args[1]
                if not (isinstance(kind, ast.Constant) and isinstance(kind.value, str)):
                    dynamic.add((module, function.name))
                elif kind.value in kinds:
                    literal.add((module, function.name, kind.value))
    return literal, dynamic


def keys_selecting(function: str, module: str, variable: str) -> set[str]:
    """The literal keys of every `{...}[variable]` lookup in `function` of `module`: the values
    `variable` can take there without a KeyError."""
    tree = ast.parse((SOURCE / module).read_text())
    keys: set[str] = set()
    for node in ast.walk(tree):
        if not (isinstance(node, ast.FunctionDef) and node.name == function):
            continue
        for lookup in ast.walk(node):
            if (
                isinstance(lookup, ast.Subscript)
                and isinstance(lookup.value, ast.Dict)
                and isinstance(lookup.slice, ast.Name)
                and lookup.slice.id == variable
            ):
                keys.update(
                    key.value
                    for key in lookup.value.keys
                    if isinstance(key, ast.Constant) and isinstance(key.value, str)
                )
    return keys


def seed_pending_origin(world: World, origin: str, owner: str, units: int, mature_at: int) -> None:
    """A bootstrap-like origin that vests at `mature_at`: the genesis origins of the fixture all
    vest at the first beacon, so a later maturity needs one more origin.

    One unit of the owner goes to the sink next, so that the snapshot of the event chain holds the
    new rows and the ledger still verifies when a restarted store builds it again."""
    escrow = world.store._services(world.run_id)[0]
    with world.store._lock:
        escrow.db.execute(
            "INSERT INTO escrow_origins VALUES(?,?,?,NULL,NULL)", (origin, units, mature_at)
        )
        escrow.db.execute(
            "INSERT INTO escrow_units VALUES(?,?,'reward_pending','',?)", (origin, owner, units)
        )
        escrow.transfer(
            sink_transfer(owner, fresh_id("snapshot-" + origin), world.network.origin_ids[0]),
            signer=owner,
        )


def units_in(store: ChallengeStore, origin: str, bucket: str) -> int:
    """Units of `origin` in `bucket`, read from the database and never through the services."""
    with store._lock:
        row = store._db.execute(
            "SELECT COALESCE(SUM(units),0) FROM escrow_units WHERE origin=? AND bucket=?",
            (origin, bucket),
        ).fetchone()
    return int(row[0])


@contextmanager
def restarted(world: World) -> Iterator[ChallengeStore]:
    """The store of `world` closed and a new one opened on the same state directory: the state a
    restarted process has before any request builds the per-run services."""
    old = world.store
    old.close()
    store = ChallengeStore(
        old.state_dir, old.params, old.coord, old.owner_hotkey, old.verify_beacon, old.objects
    )
    store.clock = old.clock
    try:
        yield store
    finally:
        store.close()


@contextmanager
def live_world(
    root: Path, origins: list[tuple[str, str, str, int]], registered: set[str]
) -> Iterator[tuple[World, Metagraph]]:
    """A service whose run holds matured live origins (id, hotkey, owner, units) and whose master
    lists `registered`. The clock is not moved: see `at_epoch_four` for the answer of an epoch."""
    with service_world(root) as world:
        escrow = world.store._services(world.run_id)[0]
        live_gate(world.store._db)
        for origin, hotkey, owner, units in origins:
            add_origin(escrow, origin, hotkey, units, 1, owner)
        metagraph = world.app.state.metagraph
        master = Metagraph(metagraph.netuid, registered)
        metagraph.client = httpx.AsyncClient(transport=master.transport())
        metagraph._cache = None
        yield world, master


def at_epoch_four(world: World) -> int:
    """Move the clock four epochs after genesis, so that epochs 0..4 are honest; returns it. The
    last pushed beacon does not move."""
    world.network.now = 5000
    return int(world.store.clock())


def rotate(world: World, coldkey: Keypair, hotkey: str, new_hotkey: Keypair) -> httpx.Response:
    """The signed rotation of the admission of `hotkey` to `new_hotkey`, posted to the route. The
    one place that knows the shape of the request: where the base asks for the new hotkey's
    consent (`sign_rotation_consent`, RTA-01) the new hotkey gives it, on an earlier base the
    three-field body is the whole request."""
    run_id, operation_id, expires = world.run_id, fresh_id("rotate"), 1000
    consent = getattr(miner_admission, "sign_rotation_consent", None)
    if consent is None:
        request = RotateRequest.model_validate(
            {"hotkey": hotkey, "new_hotkey": new_hotkey.ss58, "operation_id": operation_id}
        )
    else:
        request = consent(
            new_hotkey,
            run_id=run_id,
            coldkey=coldkey.ss58,
            hotkey=hotkey,
            operation_id=operation_id,
            expires_beacon=expires,
        )
    envelope = miner_admission.sign_rotation(coldkey, run_id, request, expires)
    return world.client.post(world.network.url + f"/admission/{hotkey}/rotate", json=envelope)


def ask_for_the_challenge(world: World, key: Keypair, admission_id: str) -> httpx.Response:
    """The owner of an admission asks for its trial challenge. The one place that knows the shape
    of the request: a POST whose body is the owner's signed envelope where the base has it
    (RTA-04, `Network.challenge`), the earlier GET signed in headers where it does not."""
    ask = getattr(world.network, "challenge", None)
    if ask is not None:
        return ask(admission_id, key)
    message = f"hypertrain/trial-challenge/2|{world.run_id}|{admission_id}".encode()
    return world.client.get(
        world.network.url + f"/join/{admission_id}/challenge",
        headers={"x-hotkey": key.ss58, "x-trial-signature": key.sign(message).hex()},
    )


def resolve_fraud(world: DisputeWorld, turn: Turn, now: int) -> Turn:
    """The referee's FRAUD resolution against the miner, on evidence the service registered."""
    evidence = RefereeEvidence(
        dispute_id=turn.dispute_id,
        transcript_hash=turn.transcript_hash,
        reason="FRAUD",
        loser=world.fixture.HOT.ss58,
        predecessor_hash="11" * 32,
        inputs_hash="22" * 32,
        output_hash="33" * 32,
        op_spec="verified.actual.op",
    )
    world.disputes.register_evidence(evidence)
    resolution = ResolutionV2(
        dispute_id=turn.dispute_id,
        transcript_hash=turn.transcript_hash,
        reason="FRAUD",
        loser=world.fixture.HOT.ss58,
        evidence_hash=evidence.digest(),
    )
    raw = canonicalize(seal(world.referee, "ResolutionV2", world.run_id, resolution, 100000))
    return world.disputes.resolution(raw, now=now)


def resolved_and_settled(
    world: DisputeWorld,
    turn: Turn,
    resolve: Callable[[DisputeWorld, Turn, int], Turn],
    settle: Callable[[Callable[[], None]], None] = lambda step: step(),
) -> Turn:
    """What the resolution route does: the referee's resolution and the settlement of the bond in
    one transaction, so a refused settlement also takes the resolution back. A control calls it
    as it is: a refusal there fails the test. The run under test passes `service_step`."""
    with world.escrow.tx():
        closed = resolve(world, turn, 3)
        settle(lambda: world.disputes.settle_lock(turn.dispute_id, now=3))
    return closed


CONTEST_UNITS = 1000


def contested_bond(
    fixture: ModuleType, verdict: Settlement
) -> tuple[Funded, EscrowLock, EscrowRelease]:
    """A ledger settled by `verdict`, COLD's 1000-unit contest bond, and the request that settles
    all of it."""
    fx = funded(fixture, settlement=lambda _reference: verdict)
    bond = lock_op(fixture, fx.cold, units=CONTEST_UNITS, kind="LOCK_CONTEST", operation="22" * 32)
    fx.escrow.lock(bond, signer=fixture.COLD.ss58)
    return fx, bond, release_op(bond, operation="33" * 32)


def ledger_state(escrow: EscrowV2) -> tuple[list[Any], list[Any], list[Any]]:
    """Every row an escrow operation may write: unit rows, events and their snapshots."""

    def rows(sql: str) -> list[Any]:
        return [tuple(row) for row in escrow.db.execute(sql)]

    return (
        rows("SELECT * FROM escrow_units ORDER BY origin, owner, bucket, ref"),
        rows("SELECT seq, operation_id, event_hash FROM escrow_events ORDER BY seq"),
        rows("SELECT operation_id FROM escrow_snapshots ORDER BY operation_id"),
    )


def abort_snapshot_insert(escrow: EscrowV2) -> None:
    """A storage failure after the movement and the event row, before the snapshot: the last
    statement of an operation, so everything it did before is what the rollback must undo."""
    escrow.db.execute(
        "CREATE TRIGGER li_storage_failure BEFORE INSERT ON escrow_snapshots "
        "BEGIN SELECT RAISE(ABORT, 'storage failure'); END"
    )


def code(call: Callable[[], object]) -> str:
    """The code of the AdmissionError `call` raises; any other outcome fails the test."""
    try:
        call()
    except AdmissionError as error:
        return error.code
    except Exception as error:
        raise AssertionError(f"expected an AdmissionError, got {type(error).__name__}") from error
    raise AssertionError("expected an AdmissionError, nothing was raised")


def admission_snapshot(lab: Lab) -> tuple[tuple[object, ...], ...]:
    """Every admission table of a bare admission service as one comparable value."""
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


def funded_cohort(rig: Rig) -> list[Miner]:
    """Four ACTIVE miners, each staked with the required amount."""
    return [rig.admit(index) for index in range(4)]


def lose_stake(rig: Rig, miner: Miner) -> None:
    """Test-only edit that keeps the ledger conserved: the miner's admission stake sits in the
    burned bucket, where a slash of its lock would leave it."""
    with rig.world.store._tx():
        rig.escrow.db.execute(
            "UPDATE escrow_units SET bucket='burned' WHERE owner=? AND bucket='admission_locked'",
            (miner.coldkey,),
        )


def seed_round_origin(
    rig: Rig, miner: Miner, label: str, *, units: int = 500, mature_at: int = 5
) -> str:
    """What `finalize_v2` leaves for one miner of round 0: the issued origin pending in the miner's
    coldkey and its accepted settlement record. Round 0 is finalized once, for all of them."""
    origin, final = ident(f"round-origin-{label}"), ident("round-finalize")
    record = settled_status(
        finality_hash=final,
        closed_dispute_root=sha256_hex(canonicalize([])),
        release_beacon=mature_at,
    )
    store, db = rig.world.store, rig.escrow.db
    with store._tx():
        db.execute(
            "INSERT INTO escrow_origins VALUES(?,?,?,0,?)", (origin, units, mature_at, miner.hotkey)
        )
        db.execute(
            "INSERT INTO escrow_units VALUES(?,?,'reward_pending','',?)",
            (origin, miner.coldkey, units),
        )
        db.execute("INSERT OR IGNORE INTO escrow_finalized VALUES(0,?)", (final,))
        store._put_record_v2(rig.run_id, "settlement", origin, record.body())
    return origin
