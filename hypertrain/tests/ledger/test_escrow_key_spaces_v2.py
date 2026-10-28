"""A signed client event must never decide the outcome of an operation the service derives itself.

ADR 0014 part 1: the events the service writes live in their own key space (`srv:`, keyed with the
coordinator's id key); client events keep their bare `Hex64` id. Every chain below squats the id the
service derives with an ordinary, valid client event and then runs the service operation.
"""

from __future__ import annotations

import ast
import hmac
import importlib.util
import inspect
import json
import sys
from collections.abc import Callable
from dataclasses import dataclass, replace
from pathlib import Path

import pytest

import hypertrain
from hypertrain.ledger.escrow_v2 import (
    SERVER_SCOPE,
    EscrowError,
    EscrowV2,
    FinalityEvidence,
    Settlement,
    escrow_id_key,
)
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages_v2 import (
    EscrowOperation,
    EscrowRelease,
    EscrowTransfer,
    RewardFinalize,
)

spec = importlib.util.spec_from_file_location(
    "escrow_key_spaces_fixture", Path(__file__).parent / "test_escrow_v2.py"
)
assert spec is not None and spec.loader is not None
E = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = E
spec.loader.exec_module(E)
actual_reward = E.actual_reward

ID_KEY = E.ID_KEY
MATURE_BEACON = 20
SETTLE_LOCK = "22" * 32


class LegacyWriter(EscrowV2):
    """The writer that existed before the server key space: every event under its bare id."""

    def _key(self, operation_id: str, server: bool) -> str:
        return operation_id


@dataclass(frozen=True, slots=True)
class Round:
    escrow: EscrowV2
    evidence: FinalityEvidence
    live: RewardFinalize
    shadow: RewardFinalize
    origin: str  # the shadow origin: one unit of it is held by OTHER, the rest by COLD
    path: Path


def client_transfer(
    owner: E.Keypair, recipient: E.Keypair, origin: str, operation_id: str
) -> EscrowTransfer:
    return EscrowTransfer(
        operation_id=operation_id,
        owner=owner.ss58,
        recipient=recipient.ss58,
        units=1,
        origin_ids=[origin],
        admission_id=None,
        dispute_id=None,
    )


def squat(escrow: EscrowV2, origin: str, operation_id: str) -> None:
    """OTHER, which earned nothing, signs one ordinary transfer of its single unit under the id."""
    escrow.transfer(client_transfer(E.OTHER, E.COLD, origin, operation_id), signer=E.OTHER.ss58)
    row = escrow.db.execute(
        "SELECT kind FROM escrow_events WHERE operation_id=?", (operation_id,)
    ).fetchone()
    assert row is not None and row["kind"] == "TRANSFER"


def build_round(tmp_path: Path, actual_reward, *, cls=EscrowV2, id_key=ID_KEY) -> Round:
    """Shadow round minted and matured, live w=0 reward not issued, OTHER holds one free unit."""
    s, evidence, live = actual_reward
    path = tmp_path / "ledger.db"
    tmp_path.mkdir(parents=True, exist_ok=True)
    e = E.shadow_ledger(s, path, live.finalize_hash)
    e.__class__ = cls
    e.id_key = id_key
    shadow = E.sign_actual_shadow(live, E.reserve_actual_shadow(e, evidence))
    e.reward_finalize(shadow, replace(evidence, shadow=True))
    e.mature(E.H, tuple(shadow.origin_ids), MATURE_BEACON)
    origin = shadow.origin_ids[0]
    e.transfer(client_transfer(E.COLD, E.OTHER, origin, "66" * 32), signer=E.COLD.ss58)
    assert e.balances(E.OTHER.ss58).available == 1
    return Round(e, evidence, live, shadow, origin, path)


def derived(r: Round) -> dict[str, str]:
    """The ids the service derives for the operations below, from public inputs only."""
    run, live_origin = r.escrow.run_id, r.live.origin_ids[0]
    return {
        "reward_finalize": sha256_hex(f"ht-reward-finalize-v2|{run}|{r.live.w}".encode()),
        "mature": sha256_hex(f"ht-mature/2|{run}|{live_origin}".encode()),
        "pay": sha256_hex(f"ht-pay/2|{run}|0|{live_origin}|{E.COLD.ss58}".encode()),
    }


def issue_live(r: Round) -> None:
    r.escrow.reward_finalize(r.live, r.evidence)
    assert r.escrow.db.execute("SELECT w FROM escrow_finalized").fetchone()["w"] == 0


def mature_live(r: Round, operation_id: str) -> None:
    r.escrow.mature(operation_id, tuple(r.live.origin_ids), MATURE_BEACON)
    assert r.escrow.balances(E.COLD.ss58).reward_pending == 0


def pay_live(r: Round, operation_id: str) -> None:
    op = EscrowOperation(
        operation_id=operation_id,
        owner=E.COLD.ss58,
        units=1,
        origin_ids=[r.live.origin_ids[0]],
        admission_id=None,
        dispute_id=None,
    )
    r.escrow.pay(op, signer=E.COLD.ss58)
    assert r.escrow.balances(E.COLD.ss58).paid == 1


def run_reward_finalize(r: Round) -> None:
    issue_live(r)


def run_mature(r: Round) -> None:
    issue_live(r)
    mature_live(r, derived(r)["mature"])


def run_pay(r: Round) -> None:
    issue_live(r)
    mature_live(r, sha256_hex(b"ht-mature-direct"))
    pay_live(r, derived(r)["pay"])


OPERATIONS: dict[str, Callable[[Round], None]] = {
    "reward_finalize": run_reward_finalize,
    "mature": run_mature,
    "pay": run_pay,
}


@pytest.fixture
def round_zero(tmp_path: Path, actual_reward) -> Round:
    return build_round(tmp_path, actual_reward)


def test_service_operation_runs_when_a_client_event_holds_an_unrelated_id(
    round_zero: Round,
) -> None:
    # Control of the two-sided boundary: green with and without the key space split.
    r = round_zero
    squat(r.escrow, r.origin, "77" * 32)
    issue_live(r)
    assert r.escrow.balances().issued == 2_000_000 and r.escrow.balances().conserved()


@pytest.mark.parametrize("operation", sorted(OPERATIONS))
def test_service_operation_runs_when_a_client_event_holds_its_derived_id(
    round_zero: Round, operation: str
) -> None:
    # Given: a client event under exactly the id the service derives for this operation.
    r = round_zero
    squat(r.escrow, r.origin, derived(r)[operation])
    # When: the service runs the operation (valid signature, valid evidence).
    OPERATIONS[operation](r)
    # Then: it committed and the chain verifies; the client's event decided nothing.
    r.escrow.verify()
    assert r.escrow.balances().conserved()


def locked_ledger(tmp_path: Path, outcome: str = "MATCH", kind: str = "LOCK_ADMISSION"):
    status = Settlement(
        finality_hash=E.H,
        closed_dispute_root=E.H,
        release_beacon=10,
        unresolved=False,
        outcome=outcome,  # type: ignore[arg-type]
        dispute_id=E.H if kind == "LOCK_CONTEST" else None,
    )
    e = E.ledger(E.setup(), tmp_path / "ledger.db", settlement=lambda _: status)
    e.contest_settlement = lambda _lock: status
    e.id_key = ID_KEY
    cold, other = E.fund(e)
    lock = E.lock(cold, SETTLE_LOCK, 500)
    if kind == "LOCK_CONTEST":
        lock = lock.model_copy(update={"kind": kind, "admission_id": None, "dispute_id": E.H})
    e.lock(lock, signer=E.COLD.ss58)
    return e, cold, other, lock


def release_body(lock, operation_id: str, units: int = 500) -> EscrowRelease:
    return EscrowRelease(
        operation_id=operation_id,
        owner=E.COLD.ss58,
        units=units,
        origin_ids=lock.origin_ids,
        admission_id=lock.admission_id,
        dispute_id=lock.dispute_id,
        lock_id=lock.operation_id,
        finality_hash=E.H,
        closed_dispute_root=E.H,
    )


def test_settlement_release_runs_when_a_client_event_holds_the_settle_id(tmp_path: Path) -> None:
    e, cold, other, lock = locked_ledger(tmp_path)
    settle_id = sha256_hex(canonicalize(["a-dispute-id", "settle"]))
    # OTHER owns the funded origin `other`; it squats the id the dispute settlement derives.
    e.transfer(client_transfer(E.OTHER, E.COLD, other, settle_id), signer=E.OTHER.ss58)
    e.release_settlement(release_body(lock, settle_id), signer=E.COLD.ss58, now_beacon=10)
    assert e.balances(E.COLD.ss58).admission_locked == 0 and e.balances().conserved()
    e.verify()


def test_slash_runs_when_a_client_event_holds_its_id(tmp_path: Path) -> None:
    e, cold, other, lock = locked_ledger(tmp_path, outcome="FRAUD", kind="LOCK_CONTEST")
    slash_id = sha256_hex(canonicalize(["a-dispute-id", "settle"]))
    e.transfer(client_transfer(E.OTHER, E.COLD, other, slash_id), signer=E.OTHER.ss58)
    e.slash(release_body(lock, slash_id), authority=E.COORD.ss58, now_beacon=10)
    assert e.balances().burned == 500 and e.balances().conserved()


def test_a_client_release_cannot_write_into_the_server_key_space(tmp_path: Path) -> None:
    e, cold, other, lock = locked_ledger(tmp_path)
    settle_id = sha256_hex(canonicalize(["a-dispute-id", "settle"]))
    # The owner releases part of its own lock under the id the service would derive...
    e.release(release_body(lock, settle_id, 100), signer=E.COLD.ss58, now_beacon=10)
    # ...which is a client event: the service still settles the rest under the same id.
    e.release_settlement(release_body(lock, settle_id, 400), signer=E.COLD.ss58, now_beacon=10)
    keys = {
        row["operation_id"]
        for row in e.db.execute("SELECT operation_id FROM escrow_events WHERE kind='RELEASE'")
    }
    assert settle_id in keys and len(keys) == 2
    assert [k for k in keys if k != settle_id][0].startswith(SERVER_SCOPE)
    assert e.balances(E.COLD.ss58).admission_locked == 0


def test_every_ledger_operation_writes_its_own_key_space(tmp_path: Path, actual_reward) -> None:
    r = build_round(tmp_path / "a", actual_reward, id_key=ID_KEY)
    issue_live(r)
    mature_live(r, derived(r)["mature"])
    pay_live(r, derived(r)["pay"])
    r.escrow.lock(E.lock(r.origin, "88" * 32, 100), signer=E.COLD.ss58)
    keys = {
        (row["kind"], row["operation_id"])
        for row in r.escrow.db.execute("SELECT kind, operation_id FROM escrow_events")
    }
    kinds = {kind for kind, _ in keys}
    assert {"SHADOW_REWARD_FINALIZE", "REWARD_FINALIZE", "MATURE", "PAY", "LOCK_ADMISSION"} <= kinds
    for kind, key in keys:
        if kind in {"LOCK_ADMISSION", "LOCK_CONTEST", "TRANSFER"}:
            assert len(key) == 64 and not key.startswith(SERVER_SCOPE)  # the client's own id
        else:
            assert key.startswith(SERVER_SCOPE) and len(key) == len(SERVER_SCOPE) + 64


def test_server_keys_are_keyed_deterministic_and_not_derivable_from_the_raw_id(
    tmp_path: Path, actual_reward
) -> None:
    r = build_round(tmp_path / "a", actual_reward)
    issue_live(r)
    raw = derived(r)["reward_finalize"]
    stored = r.escrow.db.execute(
        "SELECT operation_id FROM escrow_events WHERE kind='REWARD_FINALIZE'"
    ).fetchone()["operation_id"]
    # The oracle is written here, not taken from the ledger: srv: + HMAC-SHA256(id key, raw id).
    assert stored == SERVER_SCOPE + hmac.digest(ID_KEY, raw.encode(), "sha256").hex()
    assert stored != SERVER_SCOPE + raw  # the public formula does not give the stored key
    # Deterministic for one key, another for another key.
    other = escrow_id_key(bytes(32))
    assert r.escrow._key(raw, True) == stored
    r.escrow.id_key = other
    assert r.escrow._key(raw, True) != stored


@pytest.mark.parametrize("operation", ["reward_finalize", "mature", "pay"])
def test_a_ledger_without_the_id_key_refuses_every_server_operation(
    round_zero: Round, operation: str
) -> None:
    # A verifier built without the coordinator secret must not write predictable ids.
    r = round_zero
    if operation != "reward_finalize":
        issue_live(r)  # the state under test is set up while the key is still there
    if operation == "pay":
        mature_live(r, derived(r)["mature"])
    r.escrow.id_key = None
    before = r.escrow.db.execute("SELECT COUNT(*) FROM escrow_events").fetchone()[0]
    with pytest.raises(EscrowError, match="ID_KEY_REQUIRED"):
        if operation == "reward_finalize":
            r.escrow.reward_finalize(r.live, r.evidence)
        elif operation == "mature":
            mature_live(r, derived(r)["mature"])
        else:
            pay_live(r, derived(r)["pay"])
    assert r.escrow.db.execute("SELECT COUNT(*) FROM escrow_events").fetchone()[0] == before


def test_a_chain_written_before_the_key_space_replays_verifies_and_heals(
    tmp_path: Path, actual_reward
) -> None:
    s, evidence, live = actual_reward
    # Given: the old writer put server events under bare ids, and a client squatted the live id.
    old = build_round(tmp_path, actual_reward, cls=LegacyWriter)
    squat(old.escrow, old.origin, derived(old)["reward_finalize"])
    with pytest.raises(EscrowError, match="OPERATION_CONFLICT"):  # the blocked chain (F1)
        old.escrow.reward_finalize(live, evidence)
    shadow_receipt = old.escrow.reward_finalize(old.shadow, replace(evidence, shadow=True))
    old.escrow.db.close()
    # When: the chain is opened by the code that has the server key space.
    e = E.shadow_ledger(s, old.path, live.finalize_hash)
    e.id_key = ID_KEY
    # Then: it verifies, an exact replay of an old server event returns its stored receipt...
    e.verify()
    assert e.reward_finalize(old.shadow, replace(evidence, shadow=True)) == shadow_receipt
    # ...and the squatted live operation now succeeds under its own key.
    e.reward_finalize(live, evidence)
    keys = [row["operation_id"] for row in e.db.execute("SELECT operation_id FROM escrow_events")]
    assert derived(old)["reward_finalize"] in keys  # the squatter's row is still there, inert
    assert any(k.startswith(SERVER_SCOPE) for k in keys)
    e.verify()
    assert e.balances().issued == 2_000_000 and e.balances().conserved()


def test_a_legacy_row_on_the_id_with_another_request_is_never_taken_for_the_operation(
    round_zero: Round,
) -> None:
    # The fallback to a bare id must compare the request: a squatter's row is not the operation.
    r = round_zero
    squat(r.escrow, r.origin, derived(r)["mature"])
    issue_live(r)
    before = r.escrow.db.execute("SELECT COUNT(*) FROM escrow_events").fetchone()[0]
    mature_live(r, derived(r)["mature"])
    assert r.escrow.db.execute("SELECT COUNT(*) FROM escrow_events").fetchone()[0] == before + 1


@pytest.mark.parametrize(
    ("key", "kind", "operation_id", "keyed", "expected"),
    [
        ("aa" * 32, "TRANSFER", "aa" * 32, True, True),  # a client event is stored as chosen
        ("aa" * 32, "TRANSFER", "bb" * 32, True, False),  # ...and must name its own receipt id
        (
            SERVER_SCOPE + "aa" * 32,
            "TRANSFER",
            "aa" * 32,
            False,
            False,
        ),  # never in the server space
        (SERVER_SCOPE + "aa" * 32, "LOCK_ADMISSION", "aa" * 32, False, False),
        (SERVER_SCOPE + "aa" * 32, "LOCK_CONTEST", "aa" * 32, True, False),
        ("aa" * 32, "MATURE", "aa" * 32, True, True),  # a server event written before the split
        (SERVER_SCOPE + "aa" * 32, "MATURE", "bb" * 32, False, True),  # shape only, no id key
        (SERVER_SCOPE + "aa" * 31, "MATURE", "bb" * 32, False, False),  # a malformed server key
    ],
)
def test_binding_of_a_stored_key_to_its_receipt_id(
    tmp_path: Path,
    actual_reward,
    key: str,
    kind: str,
    operation_id: str,
    keyed: bool,
    expected: bool,
) -> None:
    r = build_round(tmp_path, actual_reward)
    r.escrow.id_key = ID_KEY if keyed else None
    assert r.escrow._binds(key, kind, operation_id) is expected


def test_binding_with_the_id_key_is_exact(tmp_path: Path, actual_reward) -> None:
    r = build_round(tmp_path, actual_reward, id_key=ID_KEY)
    assert r.escrow._binds(r.escrow._key("cc" * 32, True), "MATURE", "cc" * 32)
    assert not r.escrow._binds(
        SERVER_SCOPE + "cc" * 32, "MATURE", "cc" * 32
    )  # raw id is not the key


def test_verify_with_the_id_key_refuses_a_server_key_that_does_not_name_its_receipt(
    tmp_path: Path,
) -> None:
    e = E.ledger(E.setup(), tmp_path / "ledger.db")
    E.fund(e)  # a genesis and a maturity, both server events under the id key
    e.verify()
    e.id_key = escrow_id_key(bytes(32))  # a ledger holding another key cannot bind them
    with pytest.raises(EscrowError, match="EVENT_CHAIN"):
        e.verify()


def test_verify_without_the_id_key_proves_disjointness_and_shape_of_a_keyed_chain(
    tmp_path: Path, actual_reward
) -> None:
    # The read-only authority snapshot has no coordinator secret (ADR 0014 part 1.7).
    r = build_round(tmp_path, actual_reward, id_key=ID_KEY)
    issue_live(r)
    r.escrow.id_key = None
    r.escrow.verify()  # the chain, the shapes and the key spaces; not the keyed binding


# ---------------------------------------------------------------------------------------------
# I4: the server API is separate by type. A call site of a client operation must be a client route,
# a call site of a server operation must not be reachable from a request handler's own id.

SRC = Path(hypertrain.__file__).parent
LEDGER_METHODS = {
    "lock",
    "transfer",
    "release",
    "release_settlement",
    "slash",
    "pay",
    "mature",
    "reward_finalize",
    "test_genesis",
    "production_bootstrap",
}
# Every call of a ledger operation in the source, as (file, enclosing function): add a site here
# only after deciding which key space its operation id belongs to (ADR 0014 part 1.4).
ALLOWED_CALL_SITES: dict[str, set[tuple[str, str]]] = {
    "lock": {
        ("challenge/store.py", "admission_v2"),
        ("challenge/admission_store.py", "reconcile_lock"),
    },
    "transfer": {("challenge/store.py", "admission_v2")},
    "release": {("challenge/store.py", "admission_v2")},
    "release_settlement": {
        ("challenge/disputes_v2.py", "settle_lock"),
        ("challenge/admission.py", "release_bond"),
    },
    "slash": {
        ("challenge/disputes_v2.py", "settle_lock"),
        ("challenge/admission.py", "seize_bond"),
    },
    "pay": {("challenge/store.py", "_pay_v2")},
    "mature": {("challenge/store.py", "push_beacon"), ("challenge/store.py", "genesis_v2")},
    "reward_finalize": {
        ("challenge/store.py", "finalize_v2"),
        ("challenge/store.py", "shadow_reward_v2"),
    },
    "test_genesis": {("challenge/store.py", "genesis_v2")},
    "production_bootstrap": {("challenge/store.py", "production_bootstrap_v2")},
}


def escrow_names(function: ast.AST) -> set[str]:
    """Names that hold the ledger inside one function: `escrow`, its aliases and unpackings."""
    names = {"escrow"}
    for node in ast.walk(function):
        if not isinstance(node, ast.Assign):
            continue
        value = node.value
        holds = (
            isinstance(value, ast.Attribute)
            and value.attr == "escrow"  # e = self.escrow
            or isinstance(value, ast.Call)
            and getattr(value.func, "id", "") == "EscrowV2"  # e = EscrowV2(...)
            or isinstance(value, ast.Subscript)
            and isinstance(value.value, ast.Call)
            and getattr(value.value.func, "attr", "") == "_services"  # e = self._services(r)[0]
        )
        unpacks = isinstance(value, ast.Call) and getattr(value.func, "attr", "") == "_services"
        for target in node.targets:
            if holds and isinstance(target, ast.Name):
                names.add(target.id)
            if unpacks and isinstance(target, ast.Tuple) and isinstance(target.elts[0], ast.Name):
                names.add(target.elts[0].id)  # escrow, admission, disputes = self._services(r)
    return names


def calls_in(tree: ast.AST, relative: str) -> dict[str, set[tuple[str, str]]]:
    found: dict[str, set[tuple[str, str]]] = {name: set() for name in LEDGER_METHODS}
    for function in ast.walk(tree):
        if not isinstance(function, ast.FunctionDef | ast.AsyncFunctionDef):
            continue
        holders = escrow_names(function)
        for node in ast.walk(function):
            if not (isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute)):
                continue
            receiver = node.func.value
            name = (
                receiver.attr
                if isinstance(receiver, ast.Attribute)
                else getattr(receiver, "id", "")
            )
            if node.func.attr in LEDGER_METHODS and name in holders:
                found[node.func.attr].add((relative, function.name))
    return found


def ledger_call_sites() -> dict[str, set[tuple[str, str]]]:
    found: dict[str, set[tuple[str, str]]] = {name: set() for name in LEDGER_METHODS}
    for path in sorted(SRC.rglob("*.py")):
        relative = path.relative_to(SRC).as_posix()
        if relative != "ledger/escrow_v2.py":
            for name, sites in calls_in(ast.parse(path.read_text()), relative).items():
                found[name] |= sites
    return found


def test_the_call_site_walk_sees_a_ledger_held_under_another_name() -> None:
    source = """
class Handler:
    def a(self):
        e = self.escrow
        e.release(op, signer=s, now_beacon=1)
    def b(self, run):
        ledger, admission, disputes = self._services(run)
        ledger.slash(op)
    def c(self, run):
        held = self._services(run)[0]
        held.pay(op, signer=s)
    def d(self):
        ledger = EscrowV2(db, manifest)
        ledger.release_settlement(op)
    def e(self):
        self.lock.release()
"""
    found = calls_in(ast.parse(source), "x.py")
    assert found["release"] == {("x.py", "a")} and found["slash"] == {("x.py", "b")}
    assert found["pay"] == {("x.py", "c")} and found["release_settlement"] == {("x.py", "d")}


def test_every_call_site_of_a_ledger_operation_is_a_known_one() -> None:
    found = ledger_call_sites()
    for method, sites in found.items():
        unknown = sites - ALLOWED_CALL_SITES[method]
        assert not unknown, f"escrow.{method} called from an unreviewed site: {sorted(unknown)}"
    # A site that disappeared must leave the table too, or the table stops meaning anything.
    for method, sites in ALLOWED_CALL_SITES.items():
        assert sites <= found[method], (
            f"escrow.{method} no longer called from {sites - found[method]}"
        )


def test_a_genesis_replay_returns_the_receipt_stored_under_the_server_key(tmp_path: Path) -> None:
    e = E.ledger(E.setup(), tmp_path / "ledger.db")
    e.id_key = ID_KEY
    E.fund(e)
    row = e.db.execute(
        "SELECT operation_id, request, receipt FROM escrow_events WHERE kind='TEST_GENESIS'"
    ).fetchone()
    assert row["operation_id"].startswith(SERVER_SCOPE)
    unsigned = E.TestGenesis.model_validate(
        {**json.loads(row["request"]), "authority_sig": "0" * 128}
    )
    genesis = E.TestGenesis.model_validate(
        {**unsigned.body(), "authority_sig": E.COORD.sign(E.authority_message(unsigned)).hex()}
    )
    assert e.test_genesis(genesis).model_dump_json() == row["receipt"]


def test_no_helper_defaults_to_a_key_space() -> None:
    # `release` is the client's and `release_settlement` the service's: there is no flag to forget.
    assert "server" not in inspect.signature(EscrowV2.release).parameters
    assert "server" not in inspect.signature(EscrowV2.release_settlement).parameters
    # The private helpers that pick the key space cannot be called without saying which.
    for helper in (
        EscrowV2._existing,
        EscrowV2._event,
        EscrowV2._owner_operation,
        EscrowV2._release,
    ):
        parameter = inspect.signature(helper).parameters["server"]
        assert parameter.default is inspect.Parameter.empty, helper.__name__
