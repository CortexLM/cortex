"""Builders private to the weights gap tests (test_gaps_weights*.py). Not collected.

Everything here builds a world or reads a counter a scenario needs; the claims live in the tests.
`held` is the assertion of a hole: it raises `DefenceMissing`, so a strict xfail that names that
type cannot swallow a broken scenario (a failed `control` is a `ScenarioError`).
"""

from __future__ import annotations

from collections.abc import Iterator, Sequence
from contextlib import contextmanager
from types import ModuleType
from typing import Any

import httpx

from hypertrain.challenge.store import ChallengeError
from hypertrain.ledger.escrow_v2 import EscrowV2

from .support import World
from .support_exit_roster import control
from .support_opids import DefenceMissing
from .support_weights import Metagraph, ModeStore, mode_store

__all__ = [
    "ChallengeError",
    "DefenceMissing",
    "control",
    "held",
    "register_hotkeys",
    "root_and_child",
    "seed_v1_entitlement",
    "watermarks",
]


def held(condition: object, what: str) -> None:
    """Require the defence under test; its failure is the hole a strict xfail records."""
    if not condition:
        raise DefenceMissing(what)


@contextmanager
def root_and_child(
    li_escrow: ModuleType,
    root: Sequence[tuple[str, str, int, int]] = (),
    child: Sequence[tuple[str, str, int, int]] = (),
    *,
    attached: bool = True,
) -> Iterator[tuple[ModeStore, ModeStore]]:
    """A root store and one hosted child, each on its own database and test-mode escrow.

    An origin is (id, hotkey, units, mature_at). The child is registered the way
    `_create_child` leaves it, in `_children`, where `weights` asks it inside the root
    transaction; `attached=False` leaves that to the test, so that the child can be fresh when it
    joins a root that already answered.
    """
    with mode_store(li_escrow) as first, mode_store(li_escrow) as second:
        for origin in root:
            first.origin(*origin)
        for origin in child:
            second.origin(*origin)
        if attached:
            first.store._children["child"] = second.store
        try:
            yield first, second
        finally:
            first.store._children.clear()


def register_hotkeys(world: World, hotkeys: set[str]) -> Metagraph:
    """Replace the master the route asks for the metagraph by one that lists `hotkeys`."""
    metagraph = world.app.state.metagraph
    master = Metagraph(metagraph.netuid, hotkeys)
    metagraph.client = httpx.AsyncClient(transport=master.transport())
    return master


def seed_v1_entitlement(world: World, hotkey: str, tokens: int = 1000) -> None:
    """One finalized v1 entitlement of `hotkey`, a hundred seconds before the service clock.

    Written through the ledger the store owns, the way the finalize path leaves it; it vests
    `vest_rounds` rounds after that time, so it is unvested for every ordinary current epoch.
    """
    at = int(world.store.clock()) - 100
    ledger = world.store.ledger
    ledger.commit(0, hotkey, at)
    ledger.verdict(0, hotkey, "UNSAMPLED", tokens, 0, at)
    ledger.finalize(0, at)


def watermarks(world: World) -> tuple[int, int, int]:
    """(records of the v1 journal, stored epoch answers, meta last_answer_at): the state a refused
    get_weights request must leave exactly as it found it."""
    store = world.store
    with store._lock:
        last = store._meta(store._db, "last_answer_at", -1)
    return len(store.ledger.journal.records), world.count("weights_runs"), last


def owed(escrow: EscrowV2) -> dict[str, int]:
    """Units still free to pay, by hotkey of the origin that issued them."""
    return {
        row["hotkey"]: int(row["units"])
        for row in escrow.db.execute(
            "SELECT o.hotkey,SUM(u.units) units FROM escrow_units u JOIN escrow_origins o "
            "ON o.origin=u.origin WHERE u.bucket='available' AND u.units>0 AND o.hotkey "
            "IS NOT NULL GROUP BY o.hotkey"
        )
    }


def answer_of(response: httpx.Response) -> dict[str, Any]:
    body: dict[str, Any] = response.json()
    return body
