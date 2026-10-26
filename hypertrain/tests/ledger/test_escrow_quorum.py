"""k-of-n auditor quorum on reward items (plan 5.1); v1 single-verdict path stays the default."""

from __future__ import annotations

import importlib.util
import sys
from dataclasses import replace
from pathlib import Path

import pytest

from hypertrain.ledger.escrow_v2 import EscrowError, EscrowV2, digest
from hypertrain.protocol.envelope_v2 import EnvelopeV2, seal
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages import ReplayVerdict
from hypertrain.protocol.messages_v2 import RewardFinalize

_spec = importlib.util.spec_from_file_location(
    "escrow_v2_fixtures", Path(__file__).with_name("test_escrow_v2.py")
)
assert _spec is not None and _spec.loader is not None
base = importlib.util.module_from_spec(_spec)
sys.modules[_spec.name] = base
_spec.loader.exec_module(base)
AUDITOR, COORD, ledger, verified_shadow = (
    base.AUDITOR,
    base.COORD,
    base.ledger,
    base.verified_shadow,
)
authority_message, actual_reward = base.authority_message, base.actual_reward
__all__ = ["actual_reward"]
AUDITOR2 = Keypair(b"\x0a" * 32)
AUDITOR3 = Keypair(b"\x0b" * 32)


def _verdict(s, evidence, signer, result="MATCH"):
    v = ReplayVerdict.model_validate(evidence.works[0].verdict.body)
    v = ReplayVerdict.model_validate({**v.model_dump(mode="json"), "result": result})
    return EnvelopeV2.model_validate(seal(signer, "ReplayVerdict", s.manifest.run_id(), v, 200))


def _quorum(s, tmp_path: Path, k: int, cold: dict[str, str] | None = None) -> EscrowV2:
    e = ledger(s, tmp_path / "ledger.db", replay=verified_shadow)
    e.auditors = frozenset({AUDITOR.ss58, AUDITOR2.ss58, AUDITOR3.ss58})
    e.auditor_quorum = (k, 3)
    base = e.owner_of
    e.owner_of = lambda h: (cold or {}).get(h, "cold-" + h) if h in e.auditors else base(h)
    return e


def _with(evidence, extra):
    item = evidence.works[0]
    return replace(evidence, works=(replace(item, extra_verdicts=tuple(extra)),))


def _resign(reward, evidence):
    item = evidence.works[0]
    root = sha256_hex(
        canonicalize(
            [
                digest(ReplayVerdict.model_validate(e.body))
                for e in (item.verdict, *item.extra_verdicts)
            ]
        )
    )
    unsigned = RewardFinalize.model_validate({**reward.body(), "verdict_root": root})
    return RewardFinalize.model_validate(
        {**unsigned.body(), "authority_sig": COORD.sign(authority_message(unsigned)).hex()}
    )


def test_quorum_config_rejects_when_k_exceeds_auditors(actual_reward, tmp_path):
    s, _, _ = actual_reward
    with pytest.raises(EscrowError, match="AUDITOR_QUORUM"):
        EscrowV2(
            tmp_path / "l.db",
            s.manifest,
            s.policy,
            auditors=frozenset({AUDITOR.ss58}),
            replay_finality=verified_shadow,
            settlement=lambda _: None,  # type: ignore[arg-type, return-value]
            owner_of=str,
            assignment_of=lambda w, h: (0,),
            auditor_quorum=(2, 2),
        )


def test_extra_verdicts_reject_when_quorum_not_enabled(actual_reward, tmp_path):
    s, evidence, reward = actual_reward
    e = ledger(s, tmp_path / "ledger.db", replay=verified_shadow)
    with pytest.raises(EscrowError, match="QUORUM_NOT_ENABLED"):
        e.reward_finalize(reward, _with(evidence, [_verdict(s, evidence, AUDITOR2)]))
    assert e.balances().issued == 0


def test_k_minus_one_verdicts_rejected(actual_reward, tmp_path):
    s, evidence, reward = actual_reward
    e = _quorum(s, tmp_path, 2)
    with pytest.raises(EscrowError, match="QUORUM_UNMET"):
        e.reward_finalize(reward, evidence)
    assert e.balances().issued == 0


def test_same_coldkey_twice_rejected(actual_reward, tmp_path):
    s, evidence, reward = actual_reward
    e = _quorum(s, tmp_path, 2, cold={AUDITOR.ss58: "c", AUDITOR2.ss58: "c"})
    ev = _with(evidence, [_verdict(s, evidence, AUDITOR2)])
    with pytest.raises(EscrowError, match="QUORUM_UNMET"):
        e.reward_finalize(_resign(reward, ev), ev)
    assert e.balances().issued == 0


def test_mixed_match_mismatch_escalates(actual_reward, tmp_path):
    s, evidence, reward = actual_reward
    e = _quorum(s, tmp_path, 2)
    ev = _with(evidence, [_verdict(s, evidence, AUDITOR2, "MISMATCH")])
    with pytest.raises(EscrowError, match="QUORUM_DISPUTE"):
        e.reward_finalize(_resign(reward, ev), ev)
    assert e.balances().issued == 0


def test_k_distinct_matches_mint_and_conserve(actual_reward, tmp_path):
    s, evidence, reward = actual_reward
    e = _quorum(s, tmp_path, 2)
    ev = _with(evidence, [_verdict(s, evidence, AUDITOR2)])
    e.reward_finalize(_resign(reward, ev), ev)
    b = e.balances()
    assert b.issued == reward.budget_units and b.conserved()
