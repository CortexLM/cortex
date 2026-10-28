"""A failed re-audit drops its member and nothing else (ADR 0011 Part 9, D87).

Six ACTIVE identities, one real round: four honest members and two forgers whose commits lie
about the delta hash over honest leaves, so that the real worker says MISMATCH for both. The first
auditor has the master replay the MISMATCH of the forger first in the queue (A) and never signs;
the re-audit window opens, the second auditor takes it, has the master replay it again and never
signs either. The re-audit failed: A is dropped as an infrastructure fault, with no sanction, and
the round goes on. The second auditor then audits the rest, signs the MISMATCH of the other forger
(D) and the four MATCHes. D is sanctioned as ever (D47), the four honest members are paid, and
the ledger balances. Retiring the round for A's failure would have cancelled the audit of D and
left the honest four unpaid.
"""

from __future__ import annotations

import json

import pytest
import test_rollback_service_v2 as rollback

from challenge.five_identity_network_v2 import five_identity_network, push_through
from hypertrain.challenge import audit_cooldown_v2 as cooldown
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.messages import Finalize

fixtures = rollback.fixtures
LIE = {"delta_hash": "aa" * 32}  # honest leaves under another delta hash: COMMIT_CONTRADICTS_LEAVES
STANDING = ("ACTIVE", "JOIN_ACCEPTED", 0)


@pytest.fixture
def n(tmp_path_factory):
    """Six ACTIVE identities, rebuilt per test: a round consumes the network."""
    yield from five_identity_network(tmp_path_factory, fixtures, size=6)


class Halt(Exception):
    """Raised by `Halting` to stop `execute_round` where the audits would be scheduled."""


class Halting:
    """Stands in for the client of a network: every request passes except the first POST whose URL
    ends with `suffix`, which stops the round exactly there."""

    def __init__(self, client, suffix):
        self.client, self.suffix = client, suffix

    def __getattr__(self, name):
        return getattr(self.client, name)

    def post(self, url, *args, **kwargs):
        if url.endswith(self.suffix):
            raise Halt
        return self.client.post(url, *args, **kwargs)


def committed_round(n, *forgers):
    """Round 0 of every identity up to the request that would schedule the audits."""
    real = n.client
    n.client = Halting(real, "/audits")
    try:
        rollback.execute_round(n, 0, forge={k.ss58: LIE for k in forgers})
    except Halt:
        pass
    else:
        raise AssertionError("execute_round did not reach the audit scheduling request")
    finally:
        n.client = real


def job_id(run, hotkey):
    return sha256_hex(f"ht-audit-job/2|{run}|0|{hotkey}".encode())


def receipt(n, key, nonce):
    return n.signed(key, "Receipt", {"w": 0, "commit_hash": nonce, "received_round": n.now})


def lease(n, key, tag):
    answer = n.client.post(n.url + "/worker/lease", json=receipt(n, key, f"{tag:064x}"))
    assert answer.status_code == 200, answer.text
    return answer.json()["body"]


def execute(n, key, job):
    path = f"/worker/jobs/{job['job_id']}/execute"
    answer = n.client.post(n.url + path, json=receipt(n, key, job["lease_nonce"]))
    assert answer.status_code == 200, answer.text
    return answer.json()["verdict"]


def sign(n, key, job, verdict):
    path = f"/worker/jobs/{job['job_id']}/complete"
    answer = n.client.post(n.url + path, json=n.signed(key, "ReplayVerdict", verdict))
    assert answer.status_code == 200, answer.text
    return answer.json()["result"]


def records(n, kind):
    return {
        row[0]: json.loads(row[1])
        for row in n.store._db.execute(
            "SELECT id,data FROM records_v2 WHERE run_id=? AND kind=?",
            (n.manifest.run_id(), kind),
        )
    }


def standing(n, hotkey):
    return tuple(
        n.store._db.execute(
            "SELECT state,reason,strikes FROM admissions_v2 WHERE hotkey=?", (hotkey,)
        ).fetchone()
    )


def lease_state(n, hotkey):
    row = n.store._db.execute(
        "SELECT state,result FROM audit_leases_v2 WHERE hotkey=?", (hotkey,)
    ).fetchone()
    return tuple(row)


def test_a_failed_reaudit_drops_its_member_pays_the_honest_and_sanctions_the_next_forger(n):
    store, run = n.store, n.manifest.run_id()
    store.enforce_audit_mismatch = True
    escrow = store._services(run)[0]
    burned = escrow.balances().burned
    # A is first in the queue, then D, then the four honest members (one job id per hotkey)
    a, d, *honest = sorted(fixtures.HOT, key=lambda key: job_id(run, key.ss58))
    first, second = fixtures.AUDITORS
    committed_round(n, a, d)
    scheduled = n.client.post(n.url + "/admin/rounds/0/audits", headers=fixtures.admin())
    assert scheduled.status_code == 200, scheduled.text
    started = n.now

    # Given: the first auditor has the master replay the MISMATCH of A and never signs it
    job = lease(n, first, 1)
    assert job["start_state"]["hotkey"] == a.ss58
    assert execute(n, first, job)["result"] == "MISMATCH"
    push_through(n, started + cooldown.SIGN_WINDOW_ROUNDS)
    assert records(n, "reaudit")[f"0:{a.ss58}"]["detected"] == n.now
    assert records(n, "excluded") == {}

    # When: the second auditor takes the re-audit, has the master replay it again and does not
    # sign either
    again = lease(n, second, 2)
    assert (again["job_id"], again["attempt"]) == (job["job_id"], 1)
    assert execute(n, second, again)["result"] == "MISMATCH"
    push_through(n, n.now + cooldown.SIGN_WINDOW_ROUNDS)

    # Then: A alone is out of the round, as an infrastructure fault of its own evidence; it is
    # neither sanctioned, nor struck, nor retired with the others
    assert lease_state(n, a.ss58) == ("exhausted", "REAUDIT_FAILED")
    assert records(n, "excluded") == {
        f"0:{a.ss58}": {
            "reason": "INFRASTRUCTURE",
            "evidence_hash": sha256_hex(f"ht-reaudit-failed/1|{job['job_id']}".encode()),
        }
    }
    assert (records(n, "exit-sanction"), records(n, "exit-failed")) == ({}, {})
    assert standing(n, a.ss58) == STANDING
    assert records(n, "retired") == {}
    status = store._record_v2(run, "finality", "0")
    assert (status["disposition"], status["unresolved_audits"]) == ("PENDING", 5)
    assert {lease_state(n, k.ss58)[0] for k in (d, *honest)} == {"queued"}

    # When: the second auditor audits the rest, D first
    for tag, key in enumerate((d, *honest), start=3):
        queued = lease(n, second, tag)
        assert queued["start_state"]["hotkey"] == key.ss58
        verdict = execute(n, second, queued)
        assert verdict["result"] == ("MISMATCH" if key.ss58 == d.ss58 else "MATCH")
        assert sign(n, second, queued, verdict) == verdict["result"]
    aggregated = n.client.post(n.url + "/admin/rounds/0/aggregate", headers=fixtures.admin())
    assert aggregated.status_code == 200, aggregated.text
    applied = aggregated.json()
    assert {i["roster"]["hotkey"] for i in applied["tape"]["body"]["inputs"]} == {
        k.ss58 for k in honest
    }
    n.push(n.now + 1)  # books the sanction of D

    # Then: D is sanctioned as ever, once its contest window is over; A never is
    row = records(n, "exit-sanction")[f"0:{d.ss58}"]
    assert (row["state"], row["grade"], row["auditor"]) == (
        "PENDING",
        "COMMIT_CONTRADICTS_LEAVES",
        second.ss58,
    )
    assert f"0:{a.ss58}" not in records(n, "exit-sanction")
    final = Finalize(
        w=0,
        final_theta_hash_w1=applied["theta_hash"],
        included=sorted(k.ss58 for k in honest),
        entitlements_root="0" * 64,
    )
    settled = n.client.post(
        n.url + "/admin/rounds/0/finalize",
        json=n.signed(fixtures.COORD, "Finalize", final),
        headers=fixtures.admin(),
    )
    assert settled.status_code == 200, settled.text
    n.push(row["first_seen"] + n.manifest.training.verify.T.dispute_per_level)
    assert records(n, "exit-sanction")[f"0:{d.ss58}"]["state"] == "APPLIED"
    assert standing(n, d.ss58) == ("SUSPENDED", "AUDIT_MISMATCH", 1)
    assert standing(n, a.ss58) == STANDING
    assert escrow.balances().burned - burned == 1000  # D's bond, and nothing of A's
    # the four honest members share the round's reward; the two forgers are not paid
    paid = dict(
        store._db.execute(
            "SELECT hotkey,issued FROM escrow_origins WHERE reward_round=0"
        ).fetchall()
    )
    assert set(paid) == {k.ss58 for k in honest}
    assert sum(paid.values()) == escrow.policy.round_reward_units
    assert escrow.balances().conserved()
    escrow.verify()
