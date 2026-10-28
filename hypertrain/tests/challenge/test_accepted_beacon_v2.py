"""The beacon at which stored reward evidence was accepted is read from what the master stored.

ADR 0014 part 3: the ledger judges a stored envelope at its acceptance beacon, and
`ChallengeStore._accepted_beacon_v2` only vouches for the exact envelope it holds. A lease that
was recreated holds another challenge, so the old one is not accepted any more.
"""

from __future__ import annotations

import pytest

from hypertrain.challenge.store import ChallengeError
from hypertrain.protocol import envelope_v2
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.messages_v2 import AuditChallengeV2

from . import test_service_network_v2 as N
from .test_service_network_v2 import network  # noqa: F401  (pytest fixture import)


def challenge(n, hotkey: str, serve_deadline: int, expiry: int):
    body = AuditChallengeV2(
        w=0,
        target=hotkey,
        beacon_round=1,
        beacon_sig_sha256=sha256_hex(b"beacon"),
        mode="full",
        segments=[],
        reasons=["probation"],
        serve_deadline=serve_deadline,
        anchor_hash=sha256_hex(b"anchor"),
        audit_mode="anchored-full",
    )
    return envelope_v2.parse_envelope(
        envelope_v2.seal(N.COORD, "AuditChallengeV2", n.manifest.run_id(), body, expiry)
    )


def put_lease(n, envelope, created: int) -> None:
    body = envelope.body
    n.store._db.execute(
        "INSERT OR REPLACE INTO audit_leases_v2(id,run_id,w,hotkey,challenge,created,absolute,"
        "state,attempts,auditor,nonce,expires,tried,reservation,step_budget,result) "
        "VALUES('lease-1',?,0,?,?,?,?,'queued',0,NULL,NULL,NULL,'[]',NULL,1,NULL)",
        (
            n.manifest.run_id(),
            body["target"],
            envelope.model_dump_json(),
            created,
            10**6,
        ),
    )


def test_a_recreated_lease_does_not_vouch_for_the_challenge_it_replaced(
    network,  # noqa: F811
) -> None:
    # Given: a challenge the master leased at beacon 7, then the lease was recreated at 11 with
    # another challenge for the same round and miner.
    n = network
    run, hotkey = n.manifest.run_id(), N.HOT[0].ss58
    old = challenge(n, hotkey, serve_deadline=200, expiry=100)
    new = challenge(n, hotkey, serve_deadline=300, expiry=100)
    put_lease(n, old, created=7)
    assert n.store._accepted_beacon_v2(run, old) == 7
    put_lease(n, new, created=11)
    # When / Then: the new one is accepted where it was leased; the old one is a 409, not a
    # silent acceptance at the beacon of the lease that replaced it.
    assert n.store._accepted_beacon_v2(run, new) == 11
    with pytest.raises(ChallengeError) as refused:
        n.store._accepted_beacon_v2(run, old)
    assert refused.value.status == 409
    assert "was not accepted by this master" in str(refused.value)
