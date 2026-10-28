"""A round finalized long after every stored envelope expired still settles (ADR 0014 part 3, D48).

The envelopes expire as the reference clients seal them: the commit at `d_final`, the verdict at
`lease_expires`, the audit challenge at `absolute`. The default test expiry (10000) hides the
case, so the signing helper of the network fixture is wrapped. The operator finalizes at
`d_final + N`, as late as the miners' work screens stay valid.
"""

from __future__ import annotations

import json

import pytest
from test_rollback_service_v2 import (  # noqa: F401
    active_network,
    execute_round,
    fixtures,
)

from hypertrain.challenge.store import ChallengeError
from hypertrain.protocol import envelope_v2
from hypertrain.protocol.messages import Finalize

LATE = 500  # rounds after d_final at which the operator finalizes, if the work screens last


def realistic_expiries(network, w: int) -> None:
    """Seal commits and verdicts the way the reference miner and auditor do (fixed per round)."""
    original = type(network).signed
    run = network.manifest.run_id()

    def signed(key, type_, body, expiry=10000):
        if type_ in ("CommitV2", "ReplayVerdict"):
            row = network.store._record_v2(run, "round", str(w))["body"]
            absolute = min(row["d_final"], row["d_audit"] + 200)
            expiry = row["d_final"] if type_ == "CommitV2" else min(row["d_audit"] + 100, absolute)
        return original(network, key, type_, body, expiry)

    network.signed = signed


def finalize(network, w: int, applied):
    final = Finalize(
        w=w,
        final_theta_hash_w1=applied["theta_hash"],
        included=sorted(k.ss58 for k in fixtures.HOT),
        entitlements_root="0" * 64,
    )
    return network.client.post(
        network.url + f"/admin/rounds/{w}/finalize",
        json=type(network).signed(network, fixtures.COORD, "Finalize", final, 100000),
        headers=fixtures.admin(),
    )


def test_finalize_settles_after_every_evidence_envelope_expired(
    active_network,  # noqa: F811
    monkeypatch,
) -> None:
    network = active_network
    store, run = network.store, network.manifest.run_id()
    sealed_at: dict[str, int] = {}
    seal = envelope_v2.seal

    def spy(key, type_, run_id, body, expiry):
        if type_ == "AuditChallengeV2":
            sealed_at[body.target] = store._now(store._db)
        return seal(key, type_, run_id, body, expiry)

    monkeypatch.setattr(envelope_v2, "seal", spy)
    realistic_expiries(network, 0)
    applied = execute_round(network, 0)
    row = store._record_v2(run, "round", "0")["body"]

    # The beacon recorded for a challenge is a lower bound of the one it was sealed at.
    leases = store._db.execute("SELECT hotkey, created FROM audit_leases_v2 WHERE w=0").fetchall()
    assert leases and {r["hotkey"] for r in leases} == set(sealed_at)
    assert all(r["created"] <= sealed_at[r["hotkey"]] for r in leases)

    # Every stored envelope expires before the beacon at which the operator finalizes (the miners
    # must still be eligible there: their work screens bound how late a round can be finalized).
    screens = store._db.execute("SELECT MIN(screen_until) FROM admissions_v2").fetchone()[0]
    network.push(min(row["d_final"] + LATE, screens - 1))
    assert network.now > row["d_final"] + 100
    stored = [
        envelope_v2.parse_envelope(store._record_v2(run, kind, f"0:{key.ss58}"))
        for kind in ("commit", "verdict")
        for key in fixtures.HOT
    ]
    assert max(e.exp_drand for e in stored) < network.now
    challenge = store._db.execute("SELECT challenge FROM audit_leases_v2 WHERE w=0").fetchall()
    assert all(
        envelope_v2.parse_envelope(json.loads(r["challenge"])).exp_drand < network.now
        for r in challenge
    )

    # The round settles, the same request replays the stored result, and the ledger moved on.
    response = finalize(network, 0, applied)
    assert response.status_code == 200, response.text
    assert response.json()["status"] == "SETTLED"
    assert finalize(network, 0, applied).json() == response.json()
    # The reward ledger moved past round 0: the next reward finalize is round 1 (FINALIZE_ORDER).
    assert [r[0] for r in store._db.execute("SELECT w FROM escrow_finalized")] == [0]
    escrow = store._services(run)[0]
    assert escrow.balances().reward_pending == escrow.policy.round_reward_units
    escrow.verify()

    # The helper behind it accepts exactly the envelope that was stored, nothing re-sealed.
    commit = stored[0]
    accepted = store._accepted_beacon_v2(run, commit)
    assert row["d_assign"] <= accepted < row["d_commit"]  # the window in which a commit is taken
    assert accepted < commit.exp_drand
    key = next(k for k in fixtures.HOT if k.ss58 == commit.signer)
    model = envelope_v2.MESSAGE_TYPES_V2["CommitV2"].model_validate(commit.body)
    resealed = envelope_v2.parse_envelope(
        envelope_v2.seal(key, "CommitV2", run, model, commit.exp_drand + 1000)
    )
    with pytest.raises(ChallengeError, match="was not accepted by this master"):
        store._accepted_beacon_v2(run, resealed)
    stored_challenge = envelope_v2.parse_envelope(json.loads(challenge[0]["challenge"]))
    lease = store._db.execute("SELECT created, hotkey FROM audit_leases_v2 WHERE w=0").fetchone()
    assert store._accepted_beacon_v2(run, stored_challenge) == lease["created"]
    other = envelope_v2.parse_envelope(
        envelope_v2.seal(
            fixtures.COORD,
            "AuditChallengeV2",
            run,
            envelope_v2.MESSAGE_TYPES_V2["AuditChallengeV2"].model_validate(stored_challenge.body),
            stored_challenge.exp_drand + 1000,
        )
    )
    with pytest.raises(ChallengeError, match="was not accepted by this master"):
        store._accepted_beacon_v2(run, other)
