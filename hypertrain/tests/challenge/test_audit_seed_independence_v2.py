"""D105 proof: no audit seed, auditor selection or sample assignment depends on the lease round.

Hotfix 2 (S19) dates a lease, and the replay it starts, at a round the master has not verified yet
(emitted - MAX_LAG_ROUNDS). D105 allows that only if nothing the audit draws comes from the beacon
of that round. Everything it draws is fixed before any lease: the samples at round open (the
`d_assign` signature, stored as the `assignment` record), the challenge at scheduling (sealed with
`d_audit` and the digest of its signature), the auditor by the queue order `created,id` and the
first auditor to ask, the nonce by `secrets`. The lease round is a date and nothing else.

Two leases of one job at two rounds, every stored beacon signature replaced by bytes no drand round
has: the job differs only by holder, attempt and dates, the master's replay still recomputes what
the miner committed, and no route reads a beacon row other than the verified one it pins. One
admitted member: /complete looks its admission up (store.py:9865), so a job of a seeded member
without one would 409 UNKNOWN_HOTKEY.

Lens (D105 verdict) asked for two more pins. Three jobs and two auditors leased at rounds of either
parity: the queue order stays `created,id`. The one decision a lease round takes, the commit
expiry (COMMIT_EXPIRED), is judged at the round a lag-2 view holds. A lagging view judges there
too, never at its stale round, which would serve up to RESULT_PAUSE_ROUNDS of expired commits.
The service-admission branch of `lease_v2` is out of scope: hotfix 2 does not tolerate admitted
runs.
"""
# ruff: noqa: F811  (the `network` fixture parameter shadows the imported fixture, as in test_multirun.py)

from __future__ import annotations

import json
import re

import pytest

from hypertrain.challenge.exit_v2 import MAX_LAG_ROUNDS, RESULT_PAUSE_ROUNDS
from hypertrain.protocol.hashing import sha256_hex

from . import test_service_network_v2 as fixtures
from .audit_lease_fixture_v2 import (
    AUDITOR_A,
    AUDITOR_B,
    D_AUDIT,
    lease,
    lease_rounds,
    miners,
    network,  # noqa: F401  (pytest fixture import)
    schedule,
    seed_round,
)
from .five_identity_network_v2 import activate_identities
from .test_audit_freshness_pause_v2 import running, wall_at
from .test_audit_reaudit_v2 import complete, execute, genuine

# What a lease adds to the scheduled job: its holder, the attempt and its dates.
LEASE_FIELDS = {"attempt", "auditor_id", "created_beacon", "lease_expires", "lease_nonce"}


def beacon_rounds(reads: list[str]) -> set[int]:
    """The beacon rows a route read, besides the newest-round query of `_now`."""
    rounds = set()
    for sql in reads:
        if re.search(r"\bFROM beacon\b", sql) is None or "MAX(round)" in sql:
            continue
        row = re.fullmatch(r"SELECT (?:\*|signature) FROM beacon WHERE round=(\d+)", sql.strip())
        assert row is not None, sql  # any other read of the beacon is an input to account for
        rounds.add(int(row[1]))
    reads.clear()
    return rounds


def test_d105_no_audit_seed_selection_or_assignment_depends_on_the_lease_round(network):
    activate_identities(network, fixtures, range(1))
    store, run = network.store, network.manifest.run_id()
    key = miners(1)[0]
    admission = store._db.execute(
        "SELECT admission_id FROM admissions_v2 WHERE hotkey=?", (key.ss58,)
    ).fetchone()[0]
    seed_round(network, n=1, executable=True, gap=200, admission_ids={0: admission})
    genuine(network, key)
    (scheduled,) = schedule(network)
    stored = store._db.execute(
        "SELECT challenge FROM audit_leases_v2 WHERE id=?", (scheduled,)
    ).fetchone()[0]
    challenge = json.loads(stored)
    samples = store._record_v2(run, "assignment", f"0:{key.ss58}")["samples"]
    audit_sig = store._known_sig(store._db, D_AUDIT)
    assert audit_sig is not None
    reads: list[str] = []

    first_round = D_AUDIT + 5
    network.push(first_round)
    store._db.set_trace_callback(reads.append)
    first = lease(network, AUDITOR_A, 1)
    assert first.status_code == 200, first.text
    assert beacon_rounds(reads) == {first_round}  # the freshness pin: the verified view
    job_a = first.json()["body"]

    # A never answers; its lease ends at the round below and the job goes to B. Every stored
    # signature, d_audit's included, and the new round carry bytes no drand round has.
    second_round = first_round + lease_rounds(network)
    store._db.execute("UPDATE beacon SET signature=?,randomness=?,bls=0", ("ab" * 48, "cd" * 32))
    store._db.execute("INSERT INTO beacon VALUES(?,?,?,0)", (second_round, "ef" * 48, "01" * 32))
    genesis = network.manifest.training.beacon.genesis_time
    store.clock = lambda: genesis + (second_round - 1) * 3
    network.now = second_round
    reads.clear()
    second = lease(network, AUDITOR_B, 2)
    assert second.status_code == 200, second.text
    assert beacon_rounds(reads) == {second_round}
    job_b = second.json()["body"]

    # Selection: the queued job both times, whatever the round and its beacon.
    assert job_a["job_id"] == job_b["job_id"] == scheduled
    # The round only dates the lease; the seed-bearing bytes are the ones scheduling stored.
    assert {k for k in job_a if job_a[k] != job_b[k]} == LEASE_FIELDS
    for job, at in ((job_a, first_round), (job_b, second_round)):
        assert job["created_beacon"] == at
        assert job["lease_expires"] == min(at + lease_rounds(network), job["absolute_deadline"])
    assert job_b["challenge_envelope"] == challenge
    body = challenge["body"]
    assert (body["beacon_round"], body["beacon_sig_sha256"]) == (
        D_AUDIT,
        sha256_hex(bytes.fromhex(audit_sig)),
    )
    assert job_b["sample_ids"] == samples

    # The master's replay, on the stored samples and challenge alone, recomputes the commitment.
    verdict = execute(network, AUDITOR_B, job_b)
    assert verdict["result"] == "MATCH"
    assert beacon_rounds(reads) == {second_round}
    assert complete(network, AUDITOR_B, job_b, verdict) == "MATCH"
    assert beacon_rounds(reads) == {second_round}


@pytest.mark.parametrize("first_round", [D_AUDIT + 1, D_AUDIT + 2])
def test_d105_the_queue_order_does_not_depend_on_the_lease_round(network, first_round):
    # Three jobs, all created at d_audit; A leases at `first_round`, B one round later, so each
    # parametrization has both an even and an odd lease round. The first to ask takes the head of
    # `created,id`, the next one the job after it, whatever the round.
    seed_round(network, n=3)
    order = sorted(schedule(network))
    assert len(order) == 3
    granted = []
    for number, (auditor, at) in enumerate(
        ((AUDITOR_A, first_round), (AUDITOR_B, first_round + 1)), 1
    ):
        network.push(at)
        answer = lease(network, auditor, number)
        assert answer.status_code == 200, answer.text
        granted.append(answer.json()["body"]["job_id"])
    assert granted == order[:2]


VIEW = D_AUDIT + 5  # the master's newest verified round when the lease is asked
LAG = MAX_LAG_ROUNDS + 1


@pytest.mark.parametrize("lag", [MAX_LAG_ROUNDS, LAG, MAX_LAG_ROUNDS + RESULT_PAUSE_ROUNDS])
@pytest.mark.parametrize(
    ("sealed", "status", "state"),
    [(-1, 204, ("exhausted", "MINER_INPUT_REFUSED")), (0, 200, ("running", None))],
)
def test_d105_a_commit_expiry_is_judged_at_the_round_a_healthy_view_holds(
    network, lag, sealed, status, state
):
    # The miner sealed its commit to expire one round before the judged round (emitted -
    # MAX_LAG_ROUNDS), or at it. Before: the miner's fault, its job dropped and the queue empty.
    # At: served. A lag-2 view judges at its own round; a lagging one exactly there too.
    judged = VIEW + lag - MAX_LAG_ROUNDS
    running(network)
    seed_round(network, n=1, commit_exp=judged + sealed)
    (job,) = schedule(network)
    network.push(VIEW)
    wall_at(network, VIEW + lag)
    answer = lease(network, AUDITOR_A, 1)
    assert answer.status_code == status, answer.text
    row = network.store._db.execute(
        "SELECT state,result FROM audit_leases_v2 WHERE id=?", (job,)
    ).fetchone()
    assert tuple(row) == state
