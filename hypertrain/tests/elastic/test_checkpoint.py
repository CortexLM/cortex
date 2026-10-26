"""Checkpoint chains: a resumer trusts a head only after walking it back to the round's base."""

import hashlib
import json

import pytest

from hypertrain.elastic.checkpoint import (
    MANIFEST_MAGIC,
    CheckpointError,
    CheckpointManager,
    WorkUnit,
)
from hypertrain.elastic.store import MemoryCheckpointStore

POLICY = hashlib.sha256(b"policy").hexdigest()


def unit_for(store: MemoryCheckpointStore, shard: int = 3, steps: int = 10) -> WorkUnit:
    return WorkUnit("run-1", POLICY, 0, shard, store.put(b"base"), steps)


def test_resume_returns_the_head_state_after_walking_the_chain() -> None:
    store = MemoryCheckpointStore()
    cp = CheckpointManager(store)
    unit = unit_for(store)
    k4 = cp.save(unit, None, 4, b"s4", "m1", 1)
    k8 = cp.save(unit, k4, 8, b"s8", "m1", 1)
    k10 = cp.save(unit, k8, 10, b"s10", "m2", 2)
    assert [(m.step, m.parent, m.producer) for m in cp.chain(unit, k10)] == [
        (4, None, "m1"),
        (8, k4, "m1"),
        (10, k8, "m2"),
    ]
    assert cp.resume(unit, k10) == (10, b"s10")
    assert cp.resume(unit, None) == (0, b"base")


def test_resume_refuses_a_chain_spliced_from_another_shard() -> None:
    store = MemoryCheckpointStore()
    cp = CheckpointManager(store)
    mine, other = unit_for(store, shard=3), unit_for(store, shard=4)
    foreign = cp.save(other, None, 4, b"other-s4", "m9", 9)
    head = cp.link(mine, foreign, 8, store.put(b"s8"), "m1", 1)
    with pytest.raises(CheckpointError) as e:
        cp.resume(mine, head)
    assert e.value.code == "WRONG_UNIT"


def test_check_link_accepts_only_a_child_of_the_head() -> None:
    store = MemoryCheckpointStore()
    cp = CheckpointManager(store)
    unit = unit_for(store)
    head = cp.save(unit, None, 4, b"s4", "m1", 1)
    sibling = cp.save(unit, None, 6, b"x6", "m1", 1)
    stale = cp.save(unit, head, 4, b"x4", "m1", 1)
    child = cp.save(unit, head, 6, b"s6", "m1", 1)
    for key, code in ((sibling, "NOT_HEAD_CHILD"), (stale, "STEP_NOT_INCREASING")):
        with pytest.raises(CheckpointError) as e:
            cp.check_link(unit, head, key)
        assert e.value.code == code
    assert cp.check_link(unit, head, child).step == 6


def test_a_manifest_is_never_written_before_its_state() -> None:
    store = MemoryCheckpointStore()
    cp = CheckpointManager(store)
    unit = unit_for(store)
    missing = hashlib.sha256(b"never uploaded").hexdigest()
    with pytest.raises(CheckpointError) as e:
        cp.link(unit, None, 4, missing, "m1", 1)
    assert e.value.code == "MISSING_STATE"
    with pytest.raises(CheckpointError) as e:
        cp.save(unit, None, unit.steps + 1, b"s", "m1", 1)
    assert e.value.code == "STEP_RANGE"


def test_only_canonical_manifest_bytes_are_manifests() -> None:
    store = MemoryCheckpointStore()
    cp = CheckpointManager(store)
    unit = unit_for(store)
    key = cp.save(unit, None, 4, b"s4", "m1", 1)
    body = json.loads(store.get(key)[len(MANIFEST_MAGIC) :])
    respaced = store.put(MANIFEST_MAGIC + json.dumps(body, indent=1).encode())
    for raw_key, code in ((respaced, "NONCANONICAL_MANIFEST"), (unit.base, "NOT_A_MANIFEST")):
        with pytest.raises(CheckpointError) as e:
            cp.manifest(raw_key)
        assert e.value.code == code
