"""Production bootstrap issuance: pinned, signed, once per run, collateral never emission."""

from __future__ import annotations

import importlib.util
import sys
from dataclasses import replace
from pathlib import Path

import pytest
from pydantic import ValidationError

from hypertrain.ledger.escrow_v2 import (
    Balances,
    EscrowError,
    Settlement,
    authority_message,
    bootstrap_origin_id,
    digest,
    genesis_allocation_hash,
)
from hypertrain.protocol.messages_v2 import (
    EconomicsPolicyV2,
    EscrowTransfer,
    OriginAllocation,
    ProductionBootstrap,
    RunManifestV2,
)

_spec = importlib.util.spec_from_file_location(
    "bootstrap_escrow_fixture", Path(__file__).parent / "test_escrow_v2.py"
)
assert _spec is not None and _spec.loader is not None
E = importlib.util.module_from_spec(_spec)
sys.modules[_spec.name] = E
_spec.loader.exec_module(E)

ALLOC = [(E.COLD.ss58, 20000, 1), (E.OTHER.ss58, 10000, 1)]
OP = "44" * 32


def pinned(s, alloc=ALLOC, **update):
    """Production setup whose economics pins `alloc`; the run ID follows the new policy."""
    policy = EconomicsPolicyV2.model_validate(
        {**s.policy.body(), "bootstrap_allocation_hash": genesis_allocation_hash(alloc), **update}
    )
    body = s.manifest.body()
    body["network"]["economics_policy_hash"] = digest(policy)
    return replace(s, policy=policy, manifest=RunManifestV2.model_validate(body))


def bootstrap(run_id, alloc=ALLOC, operation=OP, signer=E.COORD) -> ProductionBootstrap:
    ah = genesis_allocation_hash(alloc)
    unsigned = ProductionBootstrap(
        run_id=run_id,
        operation_id=operation,
        allocation_hash=ah,
        total_units=sum(u for _, u, _ in alloc),
        origins=[
            OriginAllocation(
                origin_id=bootstrap_origin_id(run_id, ah, i), owner=h, units=u, mature_at=m
            )
            for i, (h, u, m) in enumerate(alloc)
        ],
        authority_sig="0" * 128,
    )
    return unsigned.model_copy(
        update={"authority_sig": signer.sign(authority_message(unsigned)).hex()}
    )


def test_bootstrap_conserves_and_survives_restart(tmp_path: Path) -> None:
    # Given
    s = pinned(E.setup("production"))
    e = E.ledger(s, tmp_path / "ledger.db")
    b = bootstrap(e.run_id)
    # When
    e.production_bootstrap(b)
    e.mature(E.H, tuple(o.origin_id for o in b.origins), 1)
    # Then: per-origin and global conservation hold, also after the chain is re-verified.
    assert e.balances() == Balances(30000, 30000, 0, 0, 0, 0, 0)
    assert e.balances(E.COLD.ss58).available == 20000
    reopened = E.ledger(s, tmp_path / "ledger.db")
    assert reopened.balances().issued == 30000 and reopened.balances().conserved()
    kinds = [r["kind"] for r in e.db.execute("SELECT kind FROM escrow_events ORDER BY seq")]
    assert kinds == ["PRODUCTION_BOOTSTRAP", "MATURE"]


def test_bootstrap_above_total_issuance_rejected(tmp_path: Path) -> None:
    s = pinned(E.setup("production"), max_total_issuance=29_999)
    e = E.ledger(s, tmp_path / "ledger.db")
    with pytest.raises(EscrowError, match="TOTAL_ISSUANCE_BUDGET"):
        e.production_bootstrap(bootstrap(e.run_id))
    assert e.balances().issued == 0
    assert e.db.execute("SELECT COUNT(*) FROM escrow_events").fetchone()[0] == 0


def test_bootstrap_idempotent_per_operation_and_once_per_run(tmp_path: Path) -> None:
    # Given
    s = pinned(E.setup("production"))
    e = E.ledger(s, tmp_path / "ledger.db")
    b = bootstrap(e.run_id)
    first = e.production_bootstrap(b)
    # When / Then: exact replay returns the stored receipt and issues nothing more.
    assert e.production_bootstrap(bootstrap(e.run_id)) == first
    assert e.balances().issued == 30000
    # Same operation_id, different content.
    other = [(E.COLD.ss58, 30000, 1)]
    with pytest.raises(EscrowError, match="OPERATION_CONFLICT"):
        e.production_bootstrap(bootstrap(e.run_id, other))
    # A second bootstrap under a fresh operation_id.
    with pytest.raises(EscrowError, match="PRODUCTION_BOOTSTRAP_FORBIDDEN"):
        e.production_bootstrap(bootstrap(e.run_id, operation="55" * 32))
    assert e.balances().issued == 30000
    assert e.db.execute("SELECT COUNT(*) FROM escrow_events").fetchone()[0] == 1


@pytest.mark.parametrize("fault", ["signer", "run", "allocation", "unpinned", "test_mode"])
def test_bootstrap_rejects_unpinned_or_unauthorized(tmp_path: Path, fault: str) -> None:
    # Given
    s = E.setup("test" if fault == "test_mode" else "production")
    if fault != "unpinned" and fault != "test_mode":
        s = pinned(s)
    e = E.ledger(s, tmp_path / "ledger.db")
    run_id = "ab" * 32 if fault == "run" else e.run_id
    alloc = [(E.COLD.ss58, 30000, 1)] if fault == "allocation" else ALLOC
    b = bootstrap(run_id, alloc, signer=E.OTHER if fault == "signer" else E.COORD)
    # When / Then
    with pytest.raises(EscrowError):
        e.production_bootstrap(b)
    assert e.balances().issued == 0


def test_bootstrap_policy_pins_reward_authority_and_mode(tmp_path: Path) -> None:
    s = E.setup("production")
    # reward_authority must stay the coordinator (escrow_v2 PINNED_AUTHORITIES).
    wrong = pinned(s, reward_authority=E.OTHER.ss58)
    with pytest.raises(EscrowError, match="PINNED_AUTHORITIES"):
        E.ledger(wrong, tmp_path / "ledger.db")
    # A test-mode policy cannot carry a bootstrap pin.
    test = E.setup("test")
    with pytest.raises(ValidationError, match="only production mode pins"):
        EconomicsPolicyV2.model_validate(
            {**test.policy.body(), "bootstrap_allocation_hash": genesis_allocation_hash(ALLOC)}
        )
    # An unpinned policy keeps its pre-existing digest (no new key serialized).
    assert "bootstrap_allocation_hash" not in s.policy.body()


# One unit over half the fixture's 2e6 cap: bootstrap plus one live award cannot both fit.
BIG = [(E.COLD.ss58, 1_000_001, 1)]


@pytest.fixture(scope="module")
def bootstrapped_reward(tmp_path_factory):
    """Real signed live w=0 reward (1e6 units) for a run whose policy pins BIG."""
    original = E.setup
    try:
        E.setup = lambda mode="production": pinned(original(mode), BIG)
        return E.actual_reward.__wrapped__(tmp_path_factory)
    finally:
        E.setup = original


def reward_ledger(s, path, reward):
    return E.ledger(
        s,
        path,
        replay=E.verified_shadow,
        settlement=lambda _: Settlement(
            finality_hash=reward.finalize_hash,
            closed_dispute_root=E.H,
            release_beacon=20,
            unresolved=False,
            outcome="MATCH",
        ),
    )


def test_reward_after_bootstrap_cannot_exceed_cap(tmp_path: Path, bootstrapped_reward) -> None:
    s, evidence, reward = bootstrapped_reward
    assert s.policy.max_total_issuance == 2_000_000
    e = reward_ledger(s, tmp_path / "ledger.db", reward)
    e.production_bootstrap(bootstrap(e.run_id, BIG))
    with pytest.raises(EscrowError, match="TOTAL_ISSUANCE_BUDGET"):
        e.reward_finalize(reward, evidence)
    assert e.balances().issued == 1_000_001 and e.balances().conserved()
    assert e.db.execute("SELECT COUNT(*) FROM escrow_finalized").fetchone()[0] == 0


def test_bootstrap_after_reward_cannot_exceed_cap(tmp_path: Path, bootstrapped_reward) -> None:
    s, evidence, reward = bootstrapped_reward
    e = reward_ledger(s, tmp_path / "ledger.db", reward)
    e.reward_finalize(reward, evidence)
    with pytest.raises(EscrowError, match="TOTAL_ISSUANCE_BUDGET"):
        e.production_bootstrap(bootstrap(e.run_id, BIG))
    assert e.balances().issued == 1_000_000 and e.balances().conserved()
    # The matured live award is payable; nothing else was issued.
    e.mature(E.H, tuple(reward.origin_ids), 20)
    assert [r["origin"] for r in e.payable(10**9)] == reward.origin_ids


def test_bootstrap_units_never_payable(tmp_path: Path) -> None:
    # Given: matured bootstrap units, some transferred to another owner.
    s = pinned(E.setup("production"))
    e = E.ledger(s, tmp_path / "ledger.db")
    b = bootstrap(e.run_id)
    e.production_bootstrap(b)
    e.mature(E.H, tuple(o.origin_id for o in b.origins), 1)
    e.transfer(
        EscrowTransfer(
            operation_id="33" * 32,
            owner=E.COLD.ss58,
            recipient=E.HOT.ss58,
            units=500,
            origin_ids=[b.origins[0].origin_id],
            admission_id=None,
            dispute_id=None,
        ),
        signer=E.COLD.ss58,
    )
    # When / Then: free, matured units exist, yet get_weights has nothing to emit.
    assert e.balances().available == 30000
    assert e.payable(10**9) == []
