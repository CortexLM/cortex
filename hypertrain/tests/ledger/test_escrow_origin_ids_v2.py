"""Origin ids are keyed: a third party who knows every public input cannot compute one.

ADR 0014 part 1.9: genesis, bootstrap, reward and shadow origin ids are the HMAC of their preimage
under the id key of the master. The unkeyed preimages stay readable only to verify origins that were
issued before the key existed.
"""

from __future__ import annotations

import importlib.util
import sys
from dataclasses import replace
from pathlib import Path

import pytest

import hypertrain.ledger.escrow_v2 as esc
from hypertrain.ledger.escrow_v2 import (
    EscrowError,
    authority_message,
    bootstrap_origin_id,
    escrow_id_key,
    genesis_allocation_hash,
    genesis_origin_id,
    reward_origin_id,
    shadow_origin_id,
)
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages_v2 import (
    OriginAllocation,
    ProductionBootstrap,
    RewardFinalize,
    TestGenesis,
)


def load(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


HERE = Path(__file__).parent
E = load("origin_ids_ledger_fixture", HERE / "test_escrow_v2.py")
P = load("origin_ids_bootstrap_fixture", HERE / "test_production_bootstrap_v2.py")
actual_reward = E.actual_reward
OTHER_KEY = escrow_id_key(bytes(32))


def legacy(preimage: str) -> str:
    """What every origin id was before the id key: sha256 of public values."""
    return sha256_hex(preimage.encode())


def test_no_public_input_gives_an_origin_id() -> None:
    run, ah, hot, fh = "aa" * 32, "bb" * 32, E.HOT.ss58, "cc" * 32
    cases = {
        "genesis": (genesis_origin_id(run, ah, 3, E.ID_KEY), f"ht-origin-genesis-v2|{run}|{ah}|3"),
        "bootstrap": (
            bootstrap_origin_id(run, ah, 3, E.ID_KEY),
            f"ht-origin-bootstrap-v1|{run}|{ah}|3",
        ),
        "reward": (
            reward_origin_id(run, 7, hot, fh, E.ID_KEY),
            f"ht-origin-reward-v2|{run}|7|{hot}|{fh}",
        ),
    }
    for name, (origin, preimage) in cases.items():
        assert len(origin) == 64, name
        assert origin != legacy(preimage), name  # not a function of the public values alone
    assert genesis_origin_id(run, ah, 3, OTHER_KEY) != cases["genesis"][0]  # a function of the key
    assert reward_origin_id(run, 7, hot, fh, E.ID_KEY) == cases["reward"][0]  # deterministic


def test_the_chain_issues_keyed_origin_ids_for_shadow_and_live_rewards(
    tmp_path: Path, actual_reward
) -> None:
    s, evidence, live = actual_reward
    e = E.shadow_ledger(s, tmp_path / "ledger.db", live.finalize_hash)
    binding = E.reserve_actual_shadow(e, evidence)
    shadow = E.sign_actual_shadow(live, binding)
    e.reward_finalize(shadow, replace(evidence, shadow=True))
    e.mature(E.H, tuple(shadow.origin_ids), 20)
    e.reward_finalize(live, evidence)
    origins = {row["origin"] for row in e.db.execute("SELECT origin FROM escrow_origins")}
    run, finalize_hash = e.run_id, live.finalize_hash
    public = {
        legacy(f"ht-origin-reward-v2|{run}|0|{E.HOT.ss58}|{finalize_hash}"),
        sha256_hex(esc.shadow_origin_preimage(binding).encode()),
    }
    assert len(origins) == 2 and not (origins & public)  # the shadow and the live reward
    assert shadow_origin_id(binding, E.ID_KEY) in origins
    assert e.reward_origin(0, E.HOT.ss58, finalize_hash) in origins


def test_a_reward_whose_origin_ids_are_the_public_function_is_refused(
    tmp_path: Path, actual_reward
) -> None:
    s, evidence, live = actual_reward
    e = E.shadow_ledger(s, tmp_path / "ledger.db", live.finalize_hash)
    shadow = E.sign_actual_shadow(live, E.reserve_actual_shadow(e, evidence))
    e.reward_finalize(shadow, replace(evidence, shadow=True))
    e.mature(E.H, tuple(shadow.origin_ids), 20)
    unkeyed = legacy(f"ht-origin-reward-v2|{e.run_id}|0|{E.HOT.ss58}|{live.finalize_hash}")
    origin = OriginAllocation(origin_id=unkeyed, owner=E.COLD.ss58, units=1_000_000, mature_at=20)
    reward = RewardFinalize.model_validate(
        {
            **live.body(),
            "origin_ids": [unkeyed],
            "allocation_hash": sha256_hex(canonicalize([origin.body()])),
            "authority_sig": "0" * 128,
        }
    )
    reward = reward.model_copy(
        update={"authority_sig": E.COORD.sign(authority_message(reward)).hex()}
    )
    with pytest.raises(EscrowError, match="REWARD_ALLOCATION"):
        e.reward_finalize(reward, evidence)
    assert e.balances().issued == 1_000_000  # only the shadow reward exists


def test_a_genesis_whose_origin_ids_are_the_public_function_is_refused(tmp_path: Path) -> None:
    e = E.ledger(E.setup(), tmp_path / "ledger.db")
    alloc = [(E.COLD.ss58, 20000, 1), (E.OTHER.ss58, 10000, 1)]
    ah = genesis_allocation_hash(alloc)
    genesis = TestGenesis(
        run_id=e.run_id,
        allocation_hash=ah,
        total_units=30000,
        origins=[
            OriginAllocation(
                origin_id=legacy(f"ht-origin-genesis-v2|{e.run_id}|{ah}|{i}"),
                owner=h,
                units=u,
                mature_at=m,
            )
            for i, (h, u, m) in enumerate(alloc)
        ],
        authority_sig="0" * 128,
    )
    genesis = genesis.model_copy(
        update={"authority_sig": E.COORD.sign(authority_message(genesis)).hex()}
    )
    with pytest.raises(EscrowError, match="GENESIS_ORIGIN_DOMAIN"):
        e.test_genesis(genesis)
    assert e.balances().issued == 0


def test_a_bootstrap_whose_origin_ids_are_the_public_function_is_refused(tmp_path: Path) -> None:
    s = P.pinned(E.setup("production"))
    e = E.ledger(s, tmp_path / "ledger.db")
    good = P.bootstrap(e.run_id)
    origins = [
        o.model_copy(
            update={
                "origin_id": legacy(f"ht-origin-bootstrap-v1|{e.run_id}|{good.allocation_hash}|{i}")
            }
        )
        for i, o in enumerate(good.origins)
    ]
    unsigned = ProductionBootstrap.model_validate(
        {**good.body(), "origins": [o.body() for o in origins], "authority_sig": "0" * 128}
    )
    bad = unsigned.model_copy(
        update={"authority_sig": E.COORD.sign(authority_message(unsigned)).hex()}
    )
    with pytest.raises(EscrowError, match="BOOTSTRAP_ORIGIN_DOMAIN"):
        e.production_bootstrap(bad)
    assert e.balances().issued == 0


def test_a_shadow_origin_issued_before_the_key_still_verifies(
    tmp_path: Path, actual_reward, monkeypatch
) -> None:
    s, evidence, live = actual_reward
    path = tmp_path / "ledger.db"
    # Given: a chain whose origins were issued by the code that had no id key.
    monkeypatch.setattr(esc, "derived_id", lambda key, preimage: sha256_hex(preimage.encode()))
    old = E.shadow_ledger(s, path, live.finalize_hash)
    shadow = E.sign_actual_shadow(live, E.reserve_actual_shadow(old, evidence))
    old.reward_finalize(shadow, replace(evidence, shadow=True))
    old.db.close()
    monkeypatch.undo()
    # When / Then: the keyed ledger, and a verifier without the key, open and verify it.
    reopened = E.shadow_ledger(s, path, live.finalize_hash)
    reopened.verify()
    reopened.id_key = None
    reopened.verify()
    row = reopened.db.execute("SELECT origin FROM escrow_origins").fetchone()
    assert row["origin"] == shadow.origin_ids[0]


def test_a_persisted_shadow_origin_that_is_neither_keyed_nor_unkeyed_is_refused(
    tmp_path: Path, actual_reward
) -> None:
    s, evidence, live = actual_reward
    e = E.shadow_ledger(s, tmp_path / "ledger.db", live.finalize_hash)
    shadow = E.sign_actual_shadow(live, E.reserve_actual_shadow(e, evidence))
    e.reward_finalize(shadow, replace(evidence, shadow=True))
    e.verify()
    e.db.execute("UPDATE escrow_shadow_finalized SET origin=?", ("dd" * 32,))
    with pytest.raises(EscrowError, match="SHADOW_PERSISTED_BINDING"):
        e.verify()
