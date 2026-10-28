"""Builders private to the ledger gap wave (test_gaps_ledger.py). Not collected: no test_ prefix.

Two kinds of helper live here: the environment scan behind the lot's scrub list (M-17), and the
small builders the store-level credit gaps share (a challenge re-bound to another round, the
audited round's clock and Finalize requests).
"""

from __future__ import annotations

import ast
import json
import shutil
from collections.abc import Iterator
from contextlib import contextmanager
from dataclasses import asdict, dataclass, replace
from pathlib import Path
from types import ModuleType
from typing import Any

import httpx
import pytest

from hypertrain.challenge.finality_v2 import RoundStatus, exclude
from hypertrain.ledger import Ledger
from hypertrain.protocol.keys import Keypair

from .support import load_fixture_module
from .support_credit import ROLLBACK_FIXTURE, AuditedRound

# Every name `Config.from_env` (challenge/app.py) reads. A knob added there that is not listed
# here fails test_the_scrub_list_names_every_environment_variable_the_challenge_and_ledger_read.
ENV_SCRUB = (
    "CHALLENGE_SLUG",
    "CHALLENGE_STATE_DIR",
    "CHALLENGE_MASTER_URL",
    "CHALLENGE_NETUID",
    "CHALLENGE_INTERNAL_TOKEN_FILE",
    "CHALLENGE_ADMIN_TOKEN_FILE",
    "CHALLENGE_WORKER_TOKEN_FILE",
    "HYPERTRAIN_COORD_KEY_FILE",
    "HYPERTRAIN_OWNER_HOTKEY",
    "HYPERTRAIN_GENESIS_UNIX",
    "HYPERTRAIN_EPOCH_SECONDS",
    "HYPERTRAIN_EPOCHS_PER_ROUND",
    "HYPERTRAIN_VEST_ROUNDS",
    "HYPERTRAIN_ROLE_DISPATCH",
    "HYPERTRAIN_CUDA_EXECUTOR",
)


def is_environ(node: ast.AST) -> bool:
    """`os.environ`."""
    return (
        isinstance(node, ast.Attribute)
        and node.attr == "environ"
        and isinstance(node.value, ast.Name)
        and node.value.id == "os"
    )


def is_environ_reader(node: ast.AST) -> bool:
    """`os.environ.get` or `os.getenv`, called or not."""
    if not isinstance(node, ast.Attribute):
        return False
    if node.attr == "get" and is_environ(node.value):
        return True
    return node.attr == "getenv" and isinstance(node.value, ast.Name) and node.value.id == "os"


def reader_names(scope: ast.AST) -> set[str]:
    """Names that read one environment variable per call in `scope`: an alias such as
    `env = os.environ.get`, and a helper whose first parameter is passed to `os.environ.get`
    (the `path` of Config.from_env)."""
    readers = {
        target.id
        for node in ast.walk(scope)
        if isinstance(node, ast.Assign) and is_environ_reader(node.value)
        for target in node.targets
        if isinstance(target, ast.Name)
    }
    for node in ast.walk(scope):
        if isinstance(node, ast.FunctionDef) and node.args.args:
            first = node.args.args[0].arg
            for call in ast.walk(node):
                if (
                    isinstance(call, ast.Call)
                    and is_environ_reader(call.func)
                    and call.args
                    and isinstance(call.args[0], ast.Name)
                    and call.args[0].id == first
                ):
                    readers.add(node.name)
    return readers


def string_constant(node: ast.AST) -> str | None:
    if isinstance(node, ast.Constant) and isinstance(node.value, str):
        return node.value
    return None


def environment_names(source: str) -> set[str]:
    """The environment variables `source` reads by a literal name: `os.environ.get("X")`,
    `os.getenv("X")`, `os.environ["X"]`, and a call of an alias or helper found by `reader_names`.
    A whole-environment copy names nothing and is not reported here."""
    tree = ast.parse(source)
    readers = reader_names(tree)
    found: set[str] = set()
    for node in ast.walk(tree):
        if isinstance(node, ast.Call) and node.args:
            callee = node.func
            direct = is_environ_reader(callee)
            aliased = isinstance(callee, ast.Name) and callee.id in readers
            name = string_constant(node.args[0]) if direct or aliased else None
        elif isinstance(node, ast.Subscript) and is_environ(node.value):
            name = string_constant(node.slice)
        else:
            continue
        if name is not None:
            found.add(name)
    return found


def scrub_environment(monkeypatch: pytest.MonkeyPatch) -> None:
    """Delete every name of ENV_SCRUB from the environment of the running test."""
    for name in ENV_SCRUB:
        monkeypatch.delenv(name, raising=False)


POISONED_BEARER = "poison-internal-bearer"
POISONED_SLUG = "poison-slug"


def polluted_environment(poison: Path) -> dict[str, str]:
    """A value for every name of ENV_SCRUB that would be loud if a service read it: a state
    directory, bearer files and a slug that are not the lot's own. The directory `poison` must
    stay as it is returned."""
    poison.mkdir(parents=True, exist_ok=True)
    files = {}
    for name in ("internal", "admin", "worker", "coord", "dispatch", "cuda"):
        files[name] = poison / name
        files[name].write_text(POISONED_BEARER)
    return {
        "CHALLENGE_SLUG": POISONED_SLUG,
        "CHALLENGE_STATE_DIR": str(poison / "state"),
        "CHALLENGE_MASTER_URL": "http://poison.invalid",
        "CHALLENGE_NETUID": "7",
        "CHALLENGE_INTERNAL_TOKEN_FILE": str(files["internal"]),
        "CHALLENGE_ADMIN_TOKEN_FILE": str(files["admin"]),
        "CHALLENGE_WORKER_TOKEN_FILE": str(files["worker"]),
        "HYPERTRAIN_COORD_KEY_FILE": str(files["coord"]),
        "HYPERTRAIN_OWNER_HOTKEY": "poison-owner",
        "HYPERTRAIN_GENESIS_UNIX": "1",
        "HYPERTRAIN_EPOCH_SECONDS": "1",
        "HYPERTRAIN_EPOCHS_PER_ROUND": "9",
        "HYPERTRAIN_VEST_ROUNDS": "9",
        "HYPERTRAIN_ROLE_DISPATCH": str(files["dispatch"]),
        "HYPERTRAIN_CUDA_EXECUTOR": str(files["cuda"]),
    }


def package_sources(src: Path, *packages: str) -> Iterator[tuple[str, str]]:
    """(path relative to src, source) of every module of the given packages."""
    for package in packages:
        for path in sorted((src / package).rglob("*.py")):
            yield str(path.relative_to(src)), path.read_text()


# --- GAP-10: the v1 service, one miner whose audit ended in a MISMATCH ---------------------------

V1_FIXTURE = ("li_v1_conftest", "challenge/conftest.py")


@contextmanager
def v1_mismatch(root: Path) -> Iterator[tuple[ModuleType, Any, Keypair]]:
    """(the v1 fixture module, its Net, the victim): three miners, round 0 committed and audited,
    the victim's verdict a MISMATCH (so it carries a blacklisting forfeit), the others MATCH."""
    fixture = load_fixture_module(*V1_FIXTURE)
    secrets = root / "secrets"
    secrets.mkdir(parents=True)
    for name, value in (
        ("internal.token", fixture.INTERNAL),
        ("admin.token", fixture.ADMIN),
        ("worker.token", fixture.WORKER),
        ("coord.key", fixture.COORD_SEED.hex()),
    ):
        (secrets / name).write_text(value + "\n" if name.endswith(".token") else value)
    master = fixture.Master()
    with fixture.make_client(root / "state", secrets, master, [1_800_000_000.0]) as client:
        net = fixture.Net(client, master)
        net.start(bonded=(True, True, False))
        opened = net.body(0)
        net.push(opened["d_assign"])
        for member in net.miners:
            assert net.accept(member, 0).status_code == 200
        net.push(opened["d_assign"] + 2)
        for member in net.miners:
            assert net.commit(member, 0).json()["status"] == "COMMITTED"
            assert net.delta(member, 0).status_code == 200
        net.push(opened["d_audit"])
        victim = net.miners[2]
        while True:
            job = net.c.post("/v1/worker/lease", headers=fixture.bearer(fixture.WORKER)).json()
            if job["target"] == victim.ss58:
                break
            assert net.verdict(job, "MATCH").status_code == 200
        assert net.verdict(job, "MISMATCH").status_code == 200
        yield fixture, net, victim


# --- the real audited and aggregated round, put back to the same moment for every test ---------


@dataclass
class Frozen:
    """An `AuditedRound` with the clock and the v1 ledger journal it started with.

    `AuditedRound.restore()` puts the SQLite database back and nothing else. A test that pushed
    the beacon would leave every later test on a store whose clock is in the future (503 'beacon
    infrastructure paused'), and a test that asked `get_weights` would leave the epoch it asked
    answered in the v1 ledger (kept in memory and in `state/ledger`), so that the same epoch asked
    by the next test is answered from that memory and not from the restored database.
    """

    round: AuditedRound
    now: int
    clock: Any
    journal: Path

    @classmethod
    def take(cls, round_: AuditedRound, journal: Path) -> Frozen:
        """Freeze `round_` now; `journal` is a directory that does not exist yet."""
        shutil.copytree(round_.store.state_dir / "ledger", journal)
        return cls(round_, round_.network.now, round_.store.clock, journal)

    def fresh(self) -> AuditedRound:
        store = self.round.store
        self.round.network.now = self.now
        store.clock = self.clock
        live = store.state_dir / "ledger"
        shutil.rmtree(live)
        shutil.copytree(self.journal, live)
        store.ledger = Ledger(live, store.params)
        self.round.restore()
        return self.round


def post_finalize(round_: AuditedRound, envelope: dict[str, Any]) -> httpx.Response:
    """The Finalize envelope of round 0, posted to the coordinator's route."""
    return round_.network.client.post(
        round_.network.url + "/admin/rounds/0/finalize",
        json=envelope,
        headers=round_.fixtures.admin(),
    )


def round_body(round_: AuditedRound, w: int = 0) -> dict[str, Any]:
    """The coordinator-signed opening of round `w` (d_audit, d_upload, d_final...)."""
    body: dict[str, Any] = round_.store._record_v2(round_.run_id, "round", str(w))["body"]
    return body


def finality(round_: AuditedRound, w: int = 0) -> dict[str, Any]:
    status: dict[str, Any] = round_.store._record_v2(round_.run_id, "finality", str(w))
    return status


def weights_answer(round_: AuditedRound, epoch: int, epoch_at: int | None = None) -> dict[str, Any]:
    """What the route asks the store: the service clock unless the test names a time."""
    store = round_.store
    raw = store.weights(epoch, int(store.clock()) if epoch_at is None else epoch_at, 1)
    answer: dict[str, Any] = json.loads(raw)
    return answer


def live_origins(round_: AuditedRound) -> list[dict[str, Any]]:
    """The reward origins of the store's ledger (genesis collateral has no hotkey), by hotkey."""
    rows = round_.store._db.execute(
        "SELECT origin,issued,mature_at,reward_round,hotkey FROM escrow_origins "
        "WHERE hotkey IS NOT NULL ORDER BY hotkey"
    ).fetchall()
    return [dict(row) for row in rows]


def retire_like_a_rollback(round_: AuditedRound, w: int, cause: str) -> None:
    """Leave round `w` as `rollback_v2` leaves a repaired round: its applied record is the repair
    (the original is kept aside), its finality EXCLUDED by `cause` with the rollback complete, and
    no reward. A test-owned stand-in: the repair itself (tape replay, signed Rollback, referee) is
    not played, the transition is the real `finality_v2.exclude`, and the state hashes of the
    round are those of the original work."""
    store, run = round_.store, round_.run_id
    applied = store._record_v2(run, "applied", str(w))
    status = RoundStatus(**store._record_v2(run, "finality", str(w)))
    with store._tx():
        store._put_record_v2(run, "original-applied", str(w), applied)
        store._put_record_v2(run, "applied", str(w), {**applied, "kind": "repair"})
        repaired = replace(exclude(status, cause), rollback_complete=True)
        store._put_record_v2(run, "finality", str(w), asdict(repaired))


def execute_round(round_: AuditedRound, w: int) -> dict[str, Any]:
    """One more real round (four islands, relay uploads, four audits, aggregation) for the four
    miners of the audited round; about a minute."""
    rollback = load_fixture_module(*ROLLBACK_FIXTURE)
    applied: dict[str, Any] = rollback.execute_round(
        round_.network, w, hotkeys=list(round_.fixtures.HOT[:4])
    )
    return applied
