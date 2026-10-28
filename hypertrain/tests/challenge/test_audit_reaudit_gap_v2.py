"""The re-audit in the round the 101 M TEST run opens, at the minimum gap its window needs (D88).

The round is opened by `live_operator.open_round`, the code the operator runs, against the
in-process service; what the miners deliver is seeded. The master's replay of a 101 M model lasts
144 to 256 s (48 to 86 rounds [A], ADR 0015): the executor boundary pushes that many beacon rounds,
one by one as the pusher does, before it returns the outcome, so every round count below is the
store's own, not an assumption of the test. The gap is the one `reaudit_gap_needed` of
`audit_cooldown_v2` asks for the window the policy pins (175, 226 and 276 rounds for 144, 200 and
256 s; a TEST manifest raises the cap of 200 for the last two): the whole window fits before
`d_final`. A second auditor then decides the miner iff `x + p` (the rounds before the first lease
and before the second auditor asks) stays inside the slack of the replay, and otherwise the member
is dropped as an infrastructure fault without a sanction (D87). The boundaries are the ones of
ADR 0011 Part 9 decision 12.
"""
# ruff: noqa: F811  (the `network` fixture parameter shadows the imported fixture, as in test_multirun.py)

from __future__ import annotations

import argparse
import math

import pytest
from scripts.ops import live_operator

from hypertrain.auditor import worker
from hypertrain.auditor.replay import AuditInputError, Outcome
from hypertrain.challenge import audit_cooldown_v2 as cooldown
from hypertrain.challenge.audit_cooldown_v2 import reaudit_gap_needed
from hypertrain.protocol import envelope_v2
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.messages_v2 import CommitV2, RoundOpenV2, reaudit_rounds_for

from . import test_service_network_v2 as fixtures
from .audit_lease_fixture_v2 import (
    AUDITOR_A,
    AUDITOR_B,
    lease,
    lease_rounds,
    miners,
    network,  # noqa: F401  (pytest fixture import)
    seed_round,
)
from .five_identity_network_v2 import activate_identities
from .test_audit_reaudit_v2 import genuine, receipt, standing
from .test_audit_withheld_signature_v2 import lease_row, records

A, B = AUDITOR_A, AUDITOR_B
BEACON_SECONDS = 3
LEASE = 100  # the lease of an H <= 30 run


def rounds_of(seconds: int) -> int:
    return math.ceil(seconds / BEACON_SECONDS)


def minimum_gap(seconds: int) -> int:
    """The gap the policy of a replay of `seconds` asks for: 175, 226, 276 for 144, 200, 256 s."""
    return reaudit_gap_needed(LEASE, reaudit_rounds_for(seconds, LEASE))


@pytest.fixture(autouse=True)
def reaudit_policy(monkeypatch, request):
    """The run is signed over the window `reaudit_rounds_for` publishes for the replay, and over the
    cap of 200 rounds lifted to the gap the window needs when that is wider (a TEST manifest)."""
    seconds = request.node.callspec.params["replay_s"]
    monkeypatch.setitem(
        fixtures.AUDIT_POLICY_EXTRA, "reaudit_rounds", reaudit_rounds_for(seconds, LEASE)
    )
    if minimum_gap(seconds) > 200:
        monkeypatch.setitem(fixtures.AUDIT_POLICY_EXTRA, "gap_cap_rounds", minimum_gap(seconds))


def walk(network, to):
    """Push every beacon round up to `to`, in process, as the real pusher does."""
    store, genesis = network.store, network.manifest.training.beacon.genesis_time
    for number in range(network.now + 1, to + 1):
        network.now = number
        store.clock = lambda number=number: genesis + (number - 1) * 3
        store.push_beacon(fixtures.fixture_beacon(number))


def open_with_live_operator(network, tmp_path, gap):
    coord, token = tmp_path / "coord.key", tmp_path / "admin.token"
    coord.write_bytes(bytes(range(32)))
    coord.chmod(0o600)
    token.write_text("integration-admin")
    master = live_operator.Master(
        argparse.Namespace(
            master="http://testserver",
            run_id=network.manifest.run_id(),
            state_dir=network.store.state_dir,
            coord_key=coord,
            admin_token_file=token,
        )
    )
    master.client = network.client  # the same routes, in process
    hotkeys = [key.ss58 for key in miners(4)]
    offsets = [3, 6, 8, 10, 12, 10 + gap]
    live_operator.check_roster_floor(hotkeys, 0)
    live_operator.open_round(master, 0, hotkeys, offsets)
    body = network.store._record_v2(network.manifest.run_id(), "round", "0")["body"]
    return RoundOpenV2.model_validate(body)


@pytest.fixture
def four_identities(network):
    activate_identities(network, fixtures, range(4))
    return network


def post(network, key, path, job, body_type, body):
    return network.client.post(
        network.url + f"/worker/jobs/{job['job_id']}/{path}",
        json=network.signed(key, body_type, body),
    )


class Replay:
    """The duration of the master's replay, in beacon rounds, for the next `execute`; `forge` turns
    the first outcome into a MISMATCH, the master's one-off glitch."""

    def __init__(self, network, monkeypatch, *, forge=False):
        self.rounds, self.forge = 0, forge
        real = worker.execute_island_audit

        def replaying(*args, **kwargs):
            outcome, artifacts = real(*args, **kwargs)
            if self.forge:
                outcome, self.forge = Outcome("MISMATCH", 0, "ab" * 32), False
            walk(network, network.now + self.rounds)
            return outcome, artifacts

        monkeypatch.setattr(worker, "execute_island_audit", replaying)


def run_replay(network, replay, key, job, rounds):
    """Ask for the execution of `job`; None when the lease ran out under the replay."""
    replay.rounds = rounds
    try:
        answer = network.client.post(
            network.url + f"/worker/jobs/{job['job_id']}/execute",
            json=receipt(network, key, job["lease_nonce"]),
        )
    except AuditInputError:
        return None  # the store's guard cancelled the executor at the lease's last round
    assert answer.status_code == 200, answer.text
    return answer.json()["verdict"]


def honest(network, d_final):
    """Every member delivers what its island run produced, sealed until d_final."""
    store, run = network.store, network.manifest.run_id()
    for key in miners(4):
        genuine(network, key)
        ident = f"0:{key.ss58}"
        body = envelope_v2.parse_envelope(store._record_v2(run, "commit", ident)).body
        with store._tx():
            sealed = network.signed(key, "CommitV2", CommitV2.model_validate(body), expiry=d_final)
            store._put_record_v2(run, "commit", ident, sealed)


def standings(network):
    return [standing(network, key.ss58) for key in miners(4)]


def reaudit(network, tmp_path, monkeypatch, *, replay_s, x, poll, busy=False):
    gap = minimum_gap(replay_s)
    """A leases `x` rounds after d_audit, the master replays for `replay_s` seconds, A never signs;
    B polls `poll` rounds after the detection (`busy`: B first replays the job behind A's, on its
    own host, as the auditor client does) and replays for as long. Returns the timeline."""
    store, run = network.store, network.manifest.run_id()
    opening = open_with_live_operator(network, tmp_path, gap)
    assert opening.d_final - opening.d_audit == gap
    seed_round(network, n=4, executable=True, opened=True)  # an honest commit seals d_final
    if busy:
        honest(network, opening.d_final)  # nobody is excluded by a MISMATCH of its own
    before = standings(network), store._services(run)[0].balances().burned
    network.push(opening.d_audit)
    scheduled = network.client.post(
        network.url + "/admin/rounds/0/audits", headers=fixtures.admin()
    )
    assert scheduled.status_code == 200, scheduled.text
    if not busy:  # a test-owned stand-in: the other three members were audited and matched
        _, *others = store._db.execute(
            "SELECT id FROM audit_leases_v2 WHERE run_id=? ORDER BY created,id", (run,)
        ).fetchall()
        with store._tx():
            for (job_id,) in others:
                store._db.execute(
                    "UPDATE audit_leases_v2 SET state='completed',result='MATCH' WHERE id=?",
                    (job_id,),
                )
    walk(network, opening.d_audit + x)
    replay, clock = rounds_of(replay_s), Replay(network, monkeypatch, forge=busy)
    first = lease(network, A, 1).json()["body"]
    t = {"d_audit": opening.d_audit, "d_final": opening.d_final, "replay": replay}
    t["r0"], t["job"] = network.now, first["job_id"]
    assert first["lease_expires"] == min(t["r0"] + lease_rounds(network), opening.d_final)
    beside = lease(network, B, 2).json()["body"] if busy else None
    assert run_replay(network, clock, A, first, replay)["result"] == "MISMATCH"  # forged if busy
    stored = records(network, "audit-execution")[f"{first['job_id']}:{first['lease_nonce']}"]["at"]
    if busy:  # B's host replayed the other job meanwhile: it signs it, then the auditor asks again
        verdict = run_replay(network, clock, B, beside, 0)
        signed = post(network, B, "complete", beside, "ReplayVerdict", verdict)
        assert signed.status_code == 200, signed.text
        following = lease(network, B, 3).json()["body"]
        assert following["job_id"] not in (first["job_id"], beside["job_id"])
        assert run_replay(network, clock, B, following, replay) is not None
        signed = post(
            network, B, "complete", following, "ReplayVerdict", stored_verdict(network, following)
        )
        assert signed.status_code == 200, signed.text
    ends = min(first["lease_expires"], stored + cooldown.SIGN_WINDOW_ROUNDS)
    walk(network, max(network.now, ends))
    miner = lease_row(network, first["job_id"])["hotkey"]
    window = records(network, "reaudit")[f"0:{miner}"]
    # the window opens at the lease's end: 20 rounds after the storing, or its expiry if sooner
    ends_at = t["r0"] + min(lease_rounds(network), replay + cooldown.SIGN_WINDOW_ROUNDS)
    assert window["detected"] == ends_at
    # the window is T after the detection, never sooner than the window the job already had
    # (`absolute`, which an honest commit makes d_final up to two leases), and the commit ends it
    window_rounds = reaudit_rounds_for(replay_s, LEASE)
    absolute = min(opening.d_final, opening.d_audit + 2 * LEASE)
    assert window["until"] == min(
        opening.d_final, max(window["detected"] + window_rounds, absolute)
    )
    t["detected"] = window["detected"]
    walk(network, max(network.now, t["detected"] + poll))
    taken = lease(network, B, 4)
    t["b_lease"] = network.now
    if taken.status_code != 200:  # the window had no round left for a lease
        return finish(network, t, None, miner, before, busy)
    second = taken.json()["body"]
    assert second["job_id"] == first["job_id"]
    verdict = run_replay(network, clock, B, second, replay)
    t["b_stored"] = network.now
    if verdict is None:
        return finish(network, t, None, miner, before, busy)
    done = post(network, B, "complete", second, "ReplayVerdict", verdict)
    if done.status_code != 200:  # stored in the last round of the window: too late to sign
        return finish(network, t, None, miner, before, busy)
    walk(network, network.now + 1)  # the next round books a completed MISMATCH
    return finish(network, t, done.json()["result"], miner, before, busy)


def stored_verdict(network, job):
    """The verdict the master stored for `job`, as its holder would sign it."""
    return network.store._record_v2(network.manifest.run_id(), "executed-verdict", job["job_id"])


def finish(network, t, result, miner, before, busy):
    """Assert what the end of the re-audit leaves, and name it."""
    store, run = network.store, network.manifest.run_id()
    if result is None:
        walk(network, t["d_final"] + 1)
        # the window failed (D87): its job has no verdict, and its member alone is dropped as an
        # infrastructure fault with the evidence of a failed re-audit; nobody is sanctioned and
        # the standings and the bond are untouched
        assert lease_row(network, t["job"])["result"] == "REAUDIT_FAILED"
        assert records(network, "excluded")[f"0:{miner}"] == {
            "reason": "INFRASTRUCTURE",
            "evidence_hash": sha256_hex(f"ht-reaudit-failed/1|{t['job']}".encode()),
        }
        assert records(network, "exit-sanction") == {}
        assert (standings(network), store._services(run)[0].balances().burned) == before
        # a roster of four cannot absorb an exclusion, so the planner may retire the round as
        # INFEASIBLE, as it always did; the failed re-audit is never the cause of a retirement
        assert {r["cause"] for r in records(network, "retired").values()} <= {"INFEASIBLE"}
        t["outcome"] = "dropped"
        return t
    if busy:  # the miner is genuine: the second replay clears it
        assert result == "MATCH"
        assert records(network, "exit-sanction") == {} and f"0:{miner}" not in records(
            network, "excluded"
        )
        assert records(network, "retired") == {}
    else:
        # the forger is left out of the round as a fraud (a roster of four absorbs no exclusion,
        # so the round itself is infeasible) and sanctioned through D47 by the second auditor
        assert result == "MISMATCH"
        assert records(network, "excluded")[f"0:{miner}"]["reason"] == "UNVERIFIED"
        sanction = records(network, "exit-sanction")[f"0:{miner}"]
        assert (sanction["state"], sanction["auditor"]) == ("PENDING", B.ss58)
    t["outcome"] = "decided"
    return t


CASES = [
    # (replay seconds, x = A's lease after d_audit, poll = B's wait after the detection, outcome):
    # the last round of slack of each replay and the one past it. The slack at the minimum gap is
    # `gap - 1 - min(L, R + W) - R`: 175 - 1 - 68 - 48 = 58, 226 - 1 - 87 - 67 = 71 and
    # 276 - 1 - 100 - 86 = 89 rounds of x + p; the 20 s poll of scripts/ops/auditors.sh spends up to
    # 7 rounds for each of x and p, which the 256 s gap holds with 75 rounds to spare
    pytest.param(144, 58, 0, "decided", id="144s-last-round-of-slack"),
    pytest.param(144, 59, 0, "dropped", id="144s-one-past"),
    pytest.param(200, 71, 0, "decided", id="200s-last-round-of-slack"),
    pytest.param(200, 72, 0, "dropped", id="200s-one-past"),
    pytest.param(256, 82, 7, "decided", id="256s-last-round-of-slack"),
    pytest.param(256, 83, 7, "dropped", id="256s-one-past"),
    pytest.param(256, 7, 7, "decided", id="256s-a-20-second-poll"),
]


@pytest.mark.parametrize(("replay_s", "x", "poll", "outcome"), CASES)
def test_the_reaudit_at_the_minimum_gap_is_decided_inside_the_slack_and_drops_the_member_past_it(
    four_identities, tmp_path, monkeypatch, replay_s, x, poll, outcome
):
    t = reaudit(four_identities, tmp_path, monkeypatch, replay_s=replay_s, x=x, poll=poll)
    assert t["outcome"] == outcome, t


@pytest.mark.parametrize(
    ("replay_s", "outcome"),
    [pytest.param(144, "decided", id="144s"), pytest.param(256, "decided", id="256s")],
)
def test_the_other_auditor_is_busy_with_the_job_behind(
    four_identities, tmp_path, monkeypatch, replay_s, outcome
):
    # four miners, two auditors: the other auditor replays the job behind first, so the re-audit
    # needs (waves + 1) * R = 3 * R rounds after the detection's start; 144 s (3R = 144) and 256 s
    # (258) both fit the minimum gap of their window (174 and 275 rounds after the first round)
    t = reaudit(four_identities, tmp_path, monkeypatch, replay_s=replay_s, x=0, poll=0, busy=True)
    assert t["outcome"] == outcome, t
