"""Builders private to the weights package of the ledger-invariants lot. Not collected.

Two hosts of `ChallengeStore.weights`:

* `reward_store` re-opens a store that hosts the real signed w=0 reward, so a restart can be
  exercised. It is `support.hosted_reward` with a restart.
* `ModeStore` hosts a test-mode EscrowV2 whose live origins are written the way issuance and
  maturity leave them, so a property can quantify over origins without the 8 s real reward.
"""

from __future__ import annotations

import json
import sqlite3
import tempfile
from collections.abc import Iterator
from contextlib import contextmanager
from dataclasses import dataclass
from pathlib import Path
from types import ModuleType
from typing import Any

import httpx

from hypertrain.challenge.store import ChallengeStore
from hypertrain.data.store import LocalFSStore
from hypertrain.ledger import FULL_SHARE_MASS, Params
from hypertrain.ledger.escrow_v2 import EscrowV2
from hypertrain.protocol.hashing import sha256_hex

from .support import SLUG, Hosted, Real, at, known_replay, table_names

EXACT = 2**53


def parse(text: bytes | str) -> dict[str, Any]:
    """A get_weights answer; a hotkey listed twice is an error, not a silent overwrite."""

    def unique(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
        keys = [key for key, _ in pairs]
        assert len(keys) == len(set(keys)), f"duplicate keys in {keys}"
        return dict(pairs)

    answer: dict[str, Any] = json.loads(text, object_pairs_hook=unique)
    return answer


def pay_events(escrow: EscrowV2) -> list[dict[str, Any]]:
    """Every PAY event, oldest first: the operation id the service named, origin, owner, units.

    The id is read from the stored request, which carries the id the caller named. The key the
    event is stored under may be a keyed derivative of it, so it is not the id.
    """
    found = []
    for row in escrow.db.execute("SELECT request FROM escrow_events WHERE kind='PAY' ORDER BY seq"):
        request = json.loads(row["request"])
        found.append(
            {
                "id": request["operation_id"],
                "origin": request["origin_ids"][0],
                "owner": request["owner"],
                "units": int(request["units"]),
            }
        )
    return found


def events(escrow: EscrowV2) -> int:
    return int(escrow.db.execute("SELECT COUNT(*) FROM escrow_events").fetchone()[0])


def assert_answer_shape(answer: dict[str, Any], *, mass: int = FULL_SHARE_MASS) -> None:
    """The contract of every stored answer: sums that add up and weights that are exact."""
    meta = answer["metadata"]
    weights = answer["weights"]
    assert list(weights) == sorted(weights)
    assert answer["full_share_mass"] == mass
    for hotkey, weight in weights.items():
        assert isinstance(weight, float) and weight == int(weight), (hotkey, weight)
        assert 0 < weight < EXACT, (hotkey, weight)
    assert sum(int(w) for w in weights.values()) == meta["units_paid"]
    assert meta["units_paid"] + meta["units_burned_this_epoch"] == mass
    assert 0 <= meta["units_paid"] <= mass


def stored_bytes(db: sqlite3.Connection) -> int:
    """Bytes held by every column of every table: the storage cost, whichever table keeps it."""
    total = 0
    for table in table_names(db):
        columns = [row[1] for row in db.execute(f'PRAGMA table_info("{table}")')]
        size = " + ".join(f'IFNULL(LENGTH(CAST("{column}" AS BLOB)), 0)' for column in columns)
        total += int(db.execute(f'SELECT IFNULL(SUM({size}), 0) FROM "{table}"').fetchone()[0])
    return total


def pay_id(run_id: str, epoch: int, origin: str, owner: str) -> str:
    """The id the service gives the payout of `origin` to `owner` in `epoch`."""
    return sha256_hex(f"ht-pay/2|{run_id}|{epoch}|{origin}|{owner}".encode())


def fail_persist(db: sqlite3.Connection) -> None:
    """The next insert of an epoch answer aborts: a crash between pay and persistence."""
    db.execute(
        "CREATE TRIGGER fail_persist BEFORE INSERT ON weights_runs "
        "BEGIN SELECT RAISE(ABORT, 'crash'); END"
    )


def recover_persist(db: sqlite3.Connection) -> None:
    db.execute("DROP TRIGGER fail_persist")


def open_store(root: Path, genesis_time: int) -> ChallengeStore:
    return ChallengeStore(
        root / "state",
        # The v1 genesis deliberately differs from the drand genesis the maturity round uses.
        Params(SLUG, genesis_time - 600, 300, 1, 1),
        None,
        None,
        lambda payload: None,
        LocalFSStore(root / "objects"),
    )


@contextmanager
def reward_store(root: Path, fixture: ModuleType, real: Real, hosted: Hosted) -> Iterator[Hosted]:
    """The store of `hosted` closed and opened again on the same state directory.

    The accepted reward and its settlement records are in the database; only the escrow object
    is built again over the new connection, with the store's real settlement callback.
    """
    setup, evidence, reward = real
    hosted.store.close()
    run_id = hosted.run_id
    store = open_store(root, setup.manifest.training.beacon.genesis_time)
    try:
        escrow: EscrowV2 = fixture.ledger(
            setup,
            store._db,
            replay=known_replay(fixture),
            settlement=lambda ref: store._settlement_v2(run_id, ref),
        )
        escrow.mutex = store._lock
        store._services = lambda _run_id: (escrow, None, None)
        yield Hosted(store, escrow, reward, evidence, run_id, set(), True)
    finally:
        store.close()


def live_gate(db: sqlite3.Connection) -> None:
    """The row finalize_v2 leaves in escrow_finalized: the store pays v2 units only after it."""
    db.execute("INSERT OR IGNORE INTO escrow_finalized VALUES(0, ?)", ("ab" * 32,))


def add_origin(
    escrow: EscrowV2, origin_id: str, hotkey: str | None, units: int, mature_at: int, owner: str
) -> None:
    """A matured origin held by its issued owner, as issuance plus maturity leave it."""
    escrow.db.execute(
        "INSERT INTO escrow_origins VALUES(?,?,?,0,?)", (origin_id, units, mature_at, hotkey)
    )
    for bucket, held in (("reward_pending", 0), ("available", units)):
        escrow.db.execute(
            "INSERT INTO escrow_units VALUES(?,?,?,'',?)", (origin_id, owner, bucket, held)
        )


@dataclass
class ModeStore:
    store: ChallengeStore
    escrow: EscrowV2
    fixture: ModuleType

    def origin(
        self, origin_id: str, hotkey: str | None, units: int, mature_at: int, *, owner: str = ""
    ) -> None:
        add_origin(
            self.escrow, origin_id, hotkey, units, mature_at, owner or self.fixture.COLD.ss58
        )

    def when(self, rnd: int) -> int:
        """Chain time of drand round `rnd`."""
        return at(self.escrow, rnd)

    def answer(self, epoch: int, rnd: int, computed_at: int = 1) -> dict[str, Any]:
        return parse(self.store.weights(epoch, self.when(rnd), computed_at))


@contextmanager
def mode_store(
    fixture: ModuleType, root: Path | None = None, *, gate: bool = True
) -> Iterator[ModeStore]:
    """A store on a test-mode escrow; `gate` opens the live gate; `root` defaults to a tmp dir."""
    with tempfile.TemporaryDirectory(prefix="li-weights-") as scratch:
        base = root or Path(scratch)
        base.mkdir(parents=True, exist_ok=True)
        setup = fixture.setup("test")
        store = open_store(base, setup.manifest.training.beacon.genesis_time)
        try:
            escrow: EscrowV2 = fixture.ledger(setup, store._db, replay=known_replay(fixture))
            escrow.mutex = store._lock
            store._db.execute(
                "INSERT INTO runs VALUES(?,?,?,'created',NULL)",
                (setup.manifest.run_id(), setup.manifest.model_dump_json(), "{}"),
            )
            store._services = lambda _run_id: (escrow, None, None)
            if gate:
                live_gate(store._db)
            yield ModeStore(store, escrow, fixture)
        finally:
            store.close()


class Metagraph:
    """A master whose metagraph answer the test changes between requests."""

    def __init__(self, netuid: int, registered: set[str]) -> None:
        self.netuid, self.registered, self.up = netuid, set(registered), True

    def transport(self) -> httpx.MockTransport:
        def handler(request: httpx.Request) -> httpx.Response:
            if not self.up:
                return httpx.Response(503)
            hotkeys = {hotkey: uid for uid, hotkey in enumerate(sorted(self.registered))}
            return httpx.Response(200, json={"netuid": self.netuid, "hotkeys": hotkeys})

        return httpx.MockTransport(handler)
