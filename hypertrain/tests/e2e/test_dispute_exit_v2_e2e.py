"""A forged-delta contest used to freeze the run; the beacon now closes it and the run goes on."""

from __future__ import annotations

import json
from pathlib import Path

import pytest
import test_rollback_service_v2 as rollback

from challenge.five_identity_network_v2 import five_identity_network
from hypertrain.aggregator.core import FinalityError
from hypertrain.auditor.island_bisect import IslandParty
from hypertrain.challenge.finality_v2 import require_open
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages import Finalize
from hypertrain.protocol.messages_v2 import (
    BisectV2,
    DisputeV2,
    EscrowLock,
    IslandJobV1,
)

fixtures = rollback.fixtures


@pytest.fixture(scope="module")
def activated_network(tmp_path_factory):
    """Five ACTIVE identities by SQL: the 12-trial ladder has its own e2e and its own cache."""
    yield from five_identity_network(tmp_path_factory, fixtures)


def contest_and_pause(n, hot, cold) -> str:
    """Fund and open the contest of the audit's MISMATCH, then let both parties tell the truth.

    The first half of rollback.exclude_miner without its tamper branch: the transcript pauses
    off the final op, where referee_v2 can only answer 409.
    """
    run = n.manifest.run_id()
    contest = json.loads(
        n.store._db.execute(
            "SELECT data FROM records_v2 WHERE kind='contest' AND json_extract(data,'$.miner')=? "
            "AND json_extract(data,'$.w')=0",
            (hot.ss58,),
        ).fetchone()[0]
    )
    dispute_id = sha256_hex(canonicalize([run, contest["verdict_hash"], hot.ss58]))
    origin = n.store._db.execute(
        "SELECT origin FROM escrow_units WHERE owner=? AND bucket='available' AND units>=100",
        (cold.ss58,),
    ).fetchone()[0]
    lock = EscrowLock(
        operation_id="df" * 32,
        owner=cold.ss58,
        units=100,
        origin_ids=[origin],
        admission_id=None,
        dispute_id=dispute_id,
        kind="LOCK_CONTEST",
    )
    response = n.client.post(n.url + "/escrow/lock", json=n.signed(cold, "EscrowLock", lock))
    assert response.status_code == 200, response.text
    body = DisputeV2(
        hotkey=hot.ss58,
        verdict_hash=contest["verdict_hash"],
        action="contest",
        bond_lock=lock.operation_id,
    )
    response = n.client.post(n.url + "/dispute", json=n.signed(hot, "DisputeV2", body))
    assert response.status_code == 200, response.text
    disputes = n.store._services(run)[2]
    geometry = n.store._record_v2(run, "trace-geometry", contest["verdict_hash"])
    party = IslandParty.published(
        hot.ss58, IslandJobV1.model_validate(geometry["job"]), Path(geometry["directory"])
    )
    for _ in range(64):
        turn = disputes.get(dispute_id)
        if turn.paused:
            break
        key = (
            hot
            if turn.expected_party == hot.ss58
            else next(k for k in fixtures.AUDITORS if k.ss58 == turn.expected_party)
        )
        at = [turn.interval[0], -(-sum(turn.interval) // 2), turn.interval[1]]
        answer = BisectV2(
            dispute_id=dispute_id,
            seq=turn.seq,
            level=turn.level,
            ctx=turn.ctx,
            interval=turn.interval,
            N=2,
            hashes=party.hashes(turn.level, tuple(turn.ctx), at),
            party=key.ss58,
            previous_transcript_hash=turn.transcript_hash,
        )
        response = n.client.post(n.url + "/bisect", json=n.signed(key, "BisectV2", answer))
        assert response.status_code == 200, response.text
    assert disputes.get(dispute_id).paused
    return dispute_id


def test_round_continues_after_forged_delta_contest(activated_network):
    # Given: five identities, one of which commits a forged delta_hash over honest leaves. The
    # audit says MISMATCH (excluded/UNVERIFIED); the other four are MATCH.
    n = activated_network
    run = n.manifest.run_id()
    forger, others = fixtures.HOT[4], fixtures.HOT[:4]
    first = rollback.execute_round(n, 0, forge={forger.ss58: {"delta_hash": "aa" * 32}})
    rollback.execute_round(n, 1, hotkeys=others)
    escrow, _, disputes = n.store._services(run)
    # When: the forger contests that verdict and both parties answer honestly, so the transcript
    # pauses where the referee cannot score it.
    dispute_id = contest_and_pause(n, forger, fixtures.COLD[4])
    # Then, before the beacon: the dispute pins round 0 and, through it, round 2.
    assert n.store._record_v2(run, "finality", "0")["unresolved_disputes"] == 1
    with pytest.raises(FinalityError, match=r"w\+2 blocked"):
        require_open(run, 2, n.store._history_v2(run))
    finalize = Finalize(
        w=0,
        final_theta_hash_w1=first["theta_hash"],
        included=sorted(k.ss58 for k in others),
        entitlements_root="0" * 64,
    )
    finalize_request = {
        "url": n.url + "/admin/rounds/0/finalize",
        "json": n.signed(fixtures.COORD, "Finalize", finalize),
        "headers": fixtures.admin(),
    }
    refused = n.client.post(**finalize_request)  # a round finality refuses is a 409, not a 500
    assert refused.status_code == 409 and "ROUND_BLOCKED: round cannot settle" in refused.text
    # When: the next verified beacon round arrives.
    n.push(n.now + 1)
    # Then: the dispute is closed without a loser and the audit's exclusion stands.
    closed = disputes.get(dispute_id)
    assert closed.resolution is not None
    assert (closed.resolution.reason, closed.resolution.loser) == ("INFRASTRUCTURE", None)
    assert n.store._record_v2(run, "finality", "0")["unresolved_disputes"] == 0
    assert n.store._record_v2(run, "excluded", f"0:{forger.ss58}")["reason"] == "UNVERIFIED"
    # And the coordinator settles round 0, which opens round 2.
    response = n.client.post(**finalize_request)
    assert response.status_code == 200, response.text
    assert n.store._record_v2(run, "finality", "0")["disposition"] == "SETTLED"
    require_open(run, 2, n.store._history_v2(run))
    assert rollback.execute_round(n, 2, open_only=True).status_code == 200
    # The contest bond came back in full and nothing was burned.
    assert escrow.balances(fixtures.COLD[4].ss58).dispute_locked == 0
    assert escrow.balances().burned == 0
