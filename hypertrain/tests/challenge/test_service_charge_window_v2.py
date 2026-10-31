"""A service charge on a hosted store must wait for another thread's transaction, not be refused.

`_service_charge_v2` refuses with 409 SERVICE_CHARGE_REQUIRES_COMMITTED_TRANSACTION when the
store connection is in a transaction, and reads that without the store lock: a transaction any
other thread holds on the connection refuses the charge. Since hotfix 4 the root's weights answer
keeps every hosted (child) store inside its answer transaction until the root commits
(`ChallengeStore._epoch_answer`), so a capacity outer or genesis charge landing in that window is
refused, and its attempt 1 is burned: `request.json`, or the genesis-resource record, is written
before the charge, and the retry is refused as ATTEMPT_RECORDED.

The window is not specific to hotfix 4: any transaction another thread holds on the connection
(a beacon push, any request) refuses the charge. The server cannot reach the outer charge
(`aggregate_v2` answers 503 SERVICE_RUNTIME_NOT_ENFORCED before `_capacity_outer_v2`), but the
operator continuation `scripts/network_service_proof.py` `decoder_continue` runs the genesis
charge on a production store it also serves over HTTPS, where the pusher's transactions land.
The fix (read the guard under the store lock, or drop it) is in an owned body of the
implementation hash, so it belongs to the hash train (founder, 2026-10-31).
"""

from __future__ import annotations

import importlib.util
import json
import sys
import threading
from collections.abc import Callable
from pathlib import Path
from typing import Any

import pytest

from hypertrain.challenge import store as store_module
from hypertrain.challenge.store import ChallengeError, ChallengeStore
from hypertrain.protocol import envelope_v2
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages_v2 import RunManifestV2

TREE = Path(__file__).parents[2]
spec = importlib.util.spec_from_file_location(
    "charge_capacity_fixture", TREE / "tests/challenge/test_service_capacity_v2.py"
)
assert spec is not None and spec.loader is not None
fixture = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = fixture
spec.loader.exec_module(fixture)
network = fixture.network
tiny = fixture.tiny

WAIT = 10  # seconds: far above a charge that does not wait, far below a test timeout
# Both window tests stay strict xfails until the hash moves (Principal SN100 D128).
WINDOW_IS_A_REFUSAL = pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="_service_charge_v2 reads in_transaction without the store lock: a transaction of "
    "another thread refuses the charge and burns attempt 1; the fix is an owned body "
    "[hash train F14, ADR 0017 finding 5]",
)


@pytest.fixture
def hosted(tiny):
    """(root, child, manifest): the run of `tiny` stays in the root, and a second run with the
    same policies is hosted by a CHILD store, its capacity profile admitted and round 0 reserved
    as `reserved` does for the root."""
    root, occupant = tiny
    body = occupant.body()
    body["network"]["capabilities"].reverse()  # a distinct run id, nothing else differs
    manifest = RunManifestV2.model_validate(body)
    root.create_run_v2(
        canonicalize(
            envelope_v2.seal(
                fixture.service.OWNER, "RunManifestV2", manifest.run_id(), manifest, 100
            )
        )
    )
    child = root.for_run(manifest.run_id())
    assert child is not root, "the root hosts the first run: the second must be a child"
    fixture.reserved((child, manifest))
    yield root, child, manifest
    child._db.close()


def in_thread(
    function: Callable[..., Any], *args: Any, **kwargs: Any
) -> tuple[threading.Thread, list[Any]]:
    """`function` started in a thread; its answer, or the error it raised, goes to the list."""
    out: list[Any] = []

    def call() -> None:
        try:
            out.append(function(*args, **kwargs))
        except Exception as error:
            out.append(error)

    thread = threading.Thread(target=call)
    thread.start()
    return thread, out


def outer_units(store: ChallengeStore, run_id: str) -> int:
    """What `_capacity_outer_v2` charges for round 0: the reservation's outer work units."""
    estimate = store._service_boundary_v2(run_id, 0)
    if estimate is None:  # not an AssertionError: the strict xfail must not absorb it
        pytest.fail("round 0 has no service reservation")
    return int(estimate["outer_work_units"])


def charges(store: ChallengeStore, run_id: str) -> list[tuple[str, dict[str, Any]]]:
    return [
        (row[0], json.loads(row[1]))
        for row in store._db.execute(
            "SELECT id, data FROM records_v2 WHERE run_id=? AND kind=?",
            (run_id, "service-charge"),
        )
    ]


def outer_charge(store: ChallengeStore, run_id: str, units: int) -> tuple[str, dict[str, Any]]:
    """The record an uncontended round-0 outer charge of `units` writes."""
    identity = sha256_hex(canonicalize({"w": 0, "operation": "outer", "attempt": 1}))
    return identity, {
        "w": 0,
        "operation": "outer",
        "attempt": 1,
        "kind": "outer",
        "units": units,
        "profile_hash": store._service_admission_v2(run_id)[1],
    }


@WINDOW_IS_A_REFUSAL
def test_a_charge_in_the_weights_answer_window_of_its_store_waits_and_is_granted(
    hosted, monkeypatch
):
    # Given: the root's real weights answer, held where hotfix 4 keeps the child inside its
    # answer transaction: the child has answered, the root has not merged nor committed.
    root, child, manifest = hosted
    run_id = manifest.run_id()
    units = outer_units(child, run_id)
    in_window, release = threading.Event(), threading.Event()
    merge = store_module._merge_answers

    def held_merge(answers: list[bytes]) -> bytes:
        in_window.set()
        release.wait(WAIT)
        return merge(answers)

    monkeypatch.setattr(store_module, "_merge_answers", held_merge)
    beacon = manifest.training.beacon
    weights, answered = in_thread(root.weights, 0, beacon.genesis_time + 19 * beacon.period, 1)
    charger: threading.Thread | None = None
    try:
        # Controls fail as such, not as the expected AssertionError of the strict xfail.
        if not in_window.wait(WAIT):
            pytest.fail(f"the weights answer never reached the merge: {answered!r}")
        if not child._db.in_transaction:
            pytest.fail("the child's answer transaction is not open")
        # When: the capacity charge of round 0 lands in that window, from another thread.
        charger, charged = in_thread(
            child._service_charge_v2, run_id, 0, "outer", 1, kind="outer", units=units
        )
        charger.join(WAIT / 5)
        # Then: it is not refused; it waits for the transaction it does not own.
        assert charged == [] and charger.is_alive(), f"the charge did not wait: {charged!r}"
    finally:
        release.set()
        weights.join(WAIT)
        if charger is not None:
            charger.join(WAIT)
    assert not weights.is_alive() and not charger.is_alive()
    assert [type(answer) for answer in answered] == [bytes]
    assert child._db.execute("SELECT 1 FROM weights_runs WHERE epoch=0").fetchone()
    # Once the answer committed, the attempt is granted and debited once, as uncontended.
    assert charged == [True]
    assert charges(child, run_id) == [outer_charge(child, run_id, units)]


@pytest.fixture
def genesis(tiny):
    """(store, manifest, units): the run `tiny` keeps in the ROOT store, its capacity profile
    admitted and its genesis attempt reserved, as `_prepare_capacity_genesis_v2` commits the
    genesis-resource record before it charges (so a refused charge burns the attempt)."""
    store, manifest = tiny
    run_id = manifest.run_id()
    store.bootstrap_service_capacity_v2(run_id, *fixture.signed_profile(store, manifest))
    entries = fixture.roster(32)
    estimate = store.require_roster_v2(run_id, roster=entries)
    with store._tx():
        store._put_record_v2(
            run_id,
            "genesis-resource",
            "run",
            {**estimate, "roster": entries, "deadline": 100, "request_hash": "0" * 64},
        )
    return store, manifest, int(estimate["outer_work_units"])


@WINDOW_IS_A_REFUSAL
def test_a_genesis_charge_during_a_beacon_push_of_its_store_waits_and_is_granted(
    genesis, monkeypatch
):
    """The operator continuation (`decoder_continue`) charges genesis on a store it serves while
    the pusher posts a round every 3 s: no child, no weights window, one push transaction."""
    # Given: the root store inside the transaction of a beacon push, held in `_advance`.
    store, manifest, units = genesis
    run_id = manifest.run_id()
    in_push, release = threading.Event(), threading.Event()
    advance = store._advance

    def held_advance(db: Any) -> Any:
        in_push.set()
        release.wait(WAIT)
        return advance(db)

    monkeypatch.setattr(store, "_advance", held_advance)
    number = store._now(store._db) + 1
    pusher, pushed = in_thread(store.push_beacon, fixture.service.fixture_beacon(number))
    charger: threading.Thread | None = None
    try:
        # Controls fail as such, not as the expected AssertionError of the strict xfail.
        if not in_push.wait(WAIT):
            pytest.fail(f"the push never reached its transaction: {pushed!r}")
        if not store._db.in_transaction:
            pytest.fail("the push transaction is not open")
        # When: the genesis charge lands in it, from another thread.
        charger, charged = in_thread(
            store._service_charge_v2, run_id, -1, "genesis", 1, kind="outer", units=units
        )
        charger.join(WAIT / 5)
        # Then: it is not refused (a 409 here burns the genesis attempt); it waits.
        assert charged == [] and charger.is_alive(), f"the charge did not wait: {charged!r}"
    finally:
        release.set()
        pusher.join(WAIT)
        if charger is not None:
            charger.join(WAIT)
    assert not pusher.is_alive() and not charger.is_alive()
    assert pushed == [{"round": number, "latest": number}]
    # Once the push committed, the attempt is granted and debited once.
    assert charged == [True]
    identity = sha256_hex(canonicalize({"w": -1, "operation": "genesis", "attempt": 1}))
    assert [row[0] for row in charges(store, run_id)] == [identity]


def test_a_charge_made_inside_its_own_open_transaction_is_still_refused(hosted):
    # Given: the calling thread holds a transaction on the store that hosts the run.
    _, child, manifest = hosted
    run_id = manifest.run_id()
    units = outer_units(child, run_id)
    # When: it charges from inside that transaction.
    with child._tx():
        with pytest.raises(ChallengeError) as refused:
            child._service_charge_v2(run_id, 0, "outer", 1, kind="outer", units=units)
    # Then: it is refused with the mapped 409 (without the guard the nested BEGIN raises a bare
    # sqlite error), nothing is charged, and the same charge outside it is granted.
    assert (refused.value.status, refused.value.detail) == (
        409,
        "SERVICE_CHARGE_REQUIRES_COMMITTED_TRANSACTION",
    )
    assert charges(child, run_id) == []
    assert child._service_charge_v2(run_id, 0, "outer", 1, kind="outer", units=units)
    assert charges(child, run_id) == [outer_charge(child, run_id, units)]
