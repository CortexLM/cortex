"""The exit reaper on `push_beacon` leaves out a silent roster member at the bound of its wait,
books whose fault it is, and sanctions the miner."""
# ruff: noqa: F811  (the `network` fixture parameter shadows the imported fixture)

from __future__ import annotations

import json

import pytest

from hypertrain.challenge.store import ChallengeStore
from hypertrain.protocol import relay_envelope
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.messages_v2 import AuditPolicyV2
from hypertrain.protocol.relay_messages import MasterAcceptanceV2, RelayReceipt

from . import test_service_network_v2 as fixtures
from .audit_lease_fixture_v2 import D_AUDIT, network, seed_round  # noqa: F401  (fixture import)
from .five_identity_network_v2 import five_identity_network, push_late, push_through
from .test_retire_round_v2 import second_round, seeded
from .test_round_refusals_v2 import open_round, round_zero


@pytest.fixture
def six_network(tmp_path_factory):
    """Six ACTIVE identities, rebuilt per test: a round consumes the network."""
    yield from five_identity_network(tmp_path_factory, fixtures, size=6)


class FakeHook:
    """What applies a sanction when the round is seeded: its admission ids are synthetic, so
    `Admission.seize_bond` would not know them."""

    def __init__(self) -> None:
        self.calls: list[tuple[str, str, str]] = []
        self.failing = False

    def seize_bond(self, admission_id: str, *, reason: str, evidence_hash: str, now: int) -> int:
        if self.failing:
            raise RuntimeError("poisoned")
        self.calls.append((admission_id, reason, evidence_hash))
        return 0


@pytest.fixture
def hook(network):
    network.store.sanction_hook = FakeHook()
    return network.store.sanction_hook


def admission_id(slot: int) -> str:
    return sha256_hex(f"adm-{slot}".encode())  # the seeded roster's


def records(net, kind: str) -> dict[str, dict]:
    rows = net.store._db.execute(
        "SELECT id,data FROM records_v2 WHERE run_id=? AND kind=?", (net.manifest.run_id(), kind)
    )
    return {r[0]: json.loads(r[1]) for r in rows}


def every_record(net) -> list[tuple]:
    rows = net.store._db.execute(
        "SELECT kind,id,data FROM records_v2 WHERE run_id=? ORDER BY kind,id",
        (net.manifest.run_id(),),
    )
    return [tuple(r) for r in rows]


def excluded_as(net) -> dict[str, tuple[str, str, str]]:
    return {
        k: (v["reason"], v["stage"], v["evidence_hash"])
        for k, v in records(net, "excluded").items()
    }


def rows_of(net) -> dict[str, tuple[str, str]]:
    return {k: (v["blame"], v["state"]) for k, v in records(net, "exit-sanction").items()}


def standing(net, hotkey: str) -> tuple:
    row = net.store._db.execute(
        "SELECT state,reason,strikes FROM admissions_v2 WHERE hotkey=?", (hotkey,)
    ).fetchone()
    return tuple(row)


def no_show(run: str, hotkey: str, stage: str) -> tuple[str, str, str]:
    evidence = sha256_hex(f"ht-no-show/1|{run}|0|{hotkey}|{stage}".encode())
    return ("NO_SHOW", stage, evidence)


def upload_evidence(net, kind: str, hotkey: str, w: int) -> None:
    """What the master keeps of an upload that reached the relay: the relay's receipt, or the
    acceptance the master signed after fetching the bytes. Both bodies name round and hotkey."""
    run = net.manifest.run_id()
    if kind == "master-acceptance":
        body = MasterAcceptanceV2(
            w=w, hotkey=hotkey, delta_hash="ff" * 32, receipt_hash="aa" * 32, received_drand=10
        )
        signed = relay_envelope.seal(fixtures.COORD, "MasterAcceptanceV2", run, body, 10000)
    else:
        body = RelayReceipt(
            w=w,
            hotkey=hotkey,
            delta_hash="ff" * 32,
            size=10,
            grant_hash="bb" * 32,
            chunk_manifest_hash="cc" * 32,
            durable_object_key="object",
            key_id="k1",
            received_drand=10,
            retain_until=1000,
        )
        signed = relay_envelope.seal(fixtures.RELAY, "RelayReceipt", run, body, 10000)
    with net.store._tx():
        net.store._put_record_v2(run, kind, body.digest(), signed)


def commit_records(net, hotkeys) -> None:
    with net.store._tx():
        for hot in hotkeys:  # the reaper reads who committed, not what
            net.store._put_record_v2(net.manifest.run_id(), "commit", f"0:{hot.ss58}", {"s": 1})


def test_the_push_that_closes_the_commit_window_excludes_the_silent_members(six_network):
    n = six_network
    run = n.manifest.run_id()
    opening = round_zero(n, fixtures.HOT)
    assert open_round(n, opening, n.client).status_code == 200
    commit_records(n, fixtures.HOT[:4])
    push_through(n, opening.d_commit - 1)
    assert records(n, "excluded") == {}

    push_through(n, opening.d_commit)

    assert excluded_as(n) == {
        f"0:{h.ss58}": no_show(run, h.ss58, "commit") for h in fixtures.HOT[4:]
    }
    assert records(n, "finality")["0"]["unresolved_audits"] == 4
    settled = every_record(n)
    push_through(n, opening.d_commit + 2)
    assert every_record(n) == settled


@pytest.mark.parametrize("case", ["served", "paused", "nobody-committed"])
def test_a_silent_member_is_sanctioned_only_when_the_master_could_have_heard_it(six_network, case):
    n = six_network
    run = n.manifest.run_id()
    escrow = n.store._services(run)[0]
    opening = round_zero(n, fixtures.HOT)
    assert open_round(n, opening, n.client).status_code == 200
    committed = [] if case == "nobody-committed" else fixtures.HOT[:4]
    commit_records(n, committed)
    silent = [h.ss58 for h in fixtures.HOT if h not in committed]
    before = {h: standing(n, h) for h in silent}
    burned = escrow.balances().burned
    push_through(n, opening.d_commit - 1)

    if case == "paused":  # the master learns of d_commit six rounds late: it refused meanwhile
        push_late(n, fixtures, opening.d_commit, wall=opening.d_commit + 6)
    else:
        push_through(n, opening.d_commit)

    miner = case == "served"
    assert rows_of(n) == {
        f"0:{h}": ("MINER", "APPLIED") if miner else ("INFRASTRUCTURE", "WAIVED") for h in silent
    }
    after = {h: standing(n, h) for h in silent}
    assert after == ({h: ("PROBATION", "NO_SHOW", 1) for h in silent} if miner else before)
    assert escrow.balances().burned - burned == (len(silent) * 1000 if miner else 0)
    assert escrow.balances().conserved()


@pytest.mark.parametrize(
    ("deltas", "undelivered", "late", "evidence", "excluded", "marker", "booked", "applied"),
    [
        (True, [5], False, None, {5}, {}, {5: ("MINER", "APPLIED")}, [5]),
        (True, [5], True, None, {5}, {}, {5: ("INFRASTRUCTURE", "WAIVED")}, []),
        (True, [5], False, ("relay-receipt", 5, 0), {5}, {}, {5: ("INFRASTRUCTURE", "WAIVED")}, []),
        (
            True,
            [5],
            False,
            ("master-acceptance", 5, 0),
            {5},
            {},
            {5: ("INFRASTRUCTURE", "WAIVED")},
            [],
        ),
        (True, [5], False, ("master-acceptance", 0, 0), {5}, {}, {5: ("MINER", "APPLIED")}, [5]),
        (True, [5], False, ("master-acceptance", 5, 1), {5}, {}, {5: ("MINER", "APPLIED")}, [5]),
        (
            False,
            [],
            False,
            None,
            set(),
            {"0": {"state": "INFEASIBLE"}},
            dict.fromkeys(range(6), ("INFRASTRUCTURE", "WAIVED")),
            [],
        ),
    ],
    ids=[
        "one-member-short",
        "upload-window-paused",
        "the-relay-lost-the-upload",
        "the-master-lost-the-upload",
        "the-upload-of-another-member",
        "the-upload-of-another-round",
        "nobody-delivered",
    ],
)
def test_the_push_at_d_upload_leaves_out_an_audited_member_without_a_delta(
    network, hook, deltas, undelivered, late, evidence, excluded, marker, booked, applied
):
    # A round nobody delivered a delta to is an upload path that failed for everybody: no member
    # is left out for it, nobody is blamed, and the round that no survivor can carry is retired.
    store, run = network.store, network.manifest.run_id()
    keys = seed_round(network, n=6, deltas=deltas, executable=not deltas)
    push_through(network, D_AUDIT)
    store.schedule_audits_v2(run, 0)
    with store._tx():
        store._db.execute("UPDATE audit_leases_v2 SET state='completed'")
        for i in undelivered:
            store._db.execute(
                "DELETE FROM records_v2 WHERE run_id=? AND kind='delta' AND id=?",
                (run, f"0:{keys[i].ss58}"),
            )
    if evidence:
        kind, slot, w = evidence
        upload_evidence(network, kind, keys[slot].ss58, w)
    network.push(D_AUDIT)  # the bound is d_upload, one round later
    assert records(network, "excluded") == {}

    if late:  # the master learns of d_upload six rounds late: it refused meanwhile
        push_late(network, fixtures, D_AUDIT + 1, wall=D_AUDIT + 7)
    else:
        network.push(D_AUDIT + 1)

    assert excluded_as(network) == {
        f"0:{keys[i].ss58}": no_show(run, keys[i].ss58, "delta") for i in excluded
    }
    assert records(network, "round-exit") == marker
    status = records(network, "finality")["0"]
    assert (status["disposition"], status["unresolved_audits"]) == (
        ("RETIRED", 0) if marker else ("PENDING", 6)
    )
    assert set(records(network, "retired")) == ({"0"} if marker else set())
    assert rows_of(network) == {f"0:{keys[i].ss58}": row for i, row in booked.items()}
    assert hook.calls == [
        (admission_id(i), "NO_SHOW", no_show(run, keys[i].ss58, "delta")[2]) for i in applied
    ]


def test_a_round_the_survivors_cannot_carry_is_retired_and_still_sanctions_its_silent_members(
    network, hook
):
    store, run = network.store, network.manifest.run_id()
    keys = seeded(network, 5, silent=(3, 4))
    push_through(network, D_AUDIT)
    push_through(network, D_AUDIT + 3)

    assert records(network, "excluded") == {}
    assert records(network, "round-exit") == {"0": {"state": "INFEASIBLE"}}
    retired = records(network, "retired")["0"]
    assert (retired["cause"], retired["survivors"]) == (
        "INFEASIBLE",
        sorted(k.ss58 for k in keys[:3]),
    )
    assert records(network, "finality")["0"]["disposition"] == "RETIRED"
    assert set(records(network, "anchor")) >= {f"0:{k.ss58}" for k in keys}  # carried, all five
    # the route that asks for new work on the round says why it takes none
    late = network.client.post(network.url + "/admin/rounds/0/audits", headers=fixtures.admin())
    assert (late.status_code, "ROUND_RETIRED" in late.text) == (409, True)
    assert rows_of(network) == {f"0:{keys[i].ss58}": ("MINER", "APPLIED") for i in (3, 4)}
    assert sorted(hook.calls) == sorted(
        (admission_id(i), "NO_SHOW", no_show(run, keys[i].ss58, "commit")[2]) for i in (3, 4)
    )
    assert (
        store._record_v2(run, "finality", "0")["unresolved_audits"] == 0
    )  # retired: no audit left
    # a later push finds the round closed and touches nothing
    settled = every_record(network)
    push_through(network, D_AUDIT + 6)
    assert every_record(network) == settled


def test_a_sanction_that_fails_never_removes_the_exclusion_or_stops_the_beacon(network, hook):
    store, run = network.store, network.manifest.run_id()
    keys = seed_round(network, n=6, silent=(4, 5))
    silent = {f"0:{keys[i].ss58}" for i in (4, 5)}
    hook.failing = True
    push_through(network, D_AUDIT - 4)  # the push of d_commit answers 200 all the same

    failed = records(network, "exit-failed")
    push_through(network, D_AUDIT + 2)

    assert set(excluded_as(network)) == silent
    assert set(rows_of(network).values()) == {("MINER", "PENDING")}
    assert set(failed) == {f"{name}:sanction" for name in silent}
    assert records(network, "exit-failed") == failed
    assert hook.calls == []
    hook.failing = False

    push_through(network, D_AUDIT + 3)

    assert set(rows_of(network).values()) == {("MINER", "APPLIED")}
    assert sorted(c[0] for c in hook.calls) == sorted(admission_id(i) for i in (4, 5))
    assert store._record_v2(run, "finality", "0")["unresolved_audits"] == 4

    push_through(network, D_AUDIT + 6)

    assert len(hook.calls) == 2


@pytest.mark.parametrize(
    ("closed", "forged", "sanctioned"),
    [(False, False, (4, 5)), (False, True, (4,)), (True, False, ())],
    ids=["open-round", "evidence-that-does-not-re-derive", "closed-round"],
)
def test_a_no_show_a_lazy_writer_recorded_is_adopted_while_its_round_is_open(
    network, hook, monkeypatch, closed, forged, sanctioned
):
    store, run = network.store, network.manifest.run_id()
    keys = seed_round(network, n=6, silent=(4, 5))
    with monkeypatch.context() as restarted:  # the beacon route did not see the round
        restarted.setattr(ChallengeStore, "_reap_exits_v2", lambda *_a, **_k: None)
        push_through(network, D_AUDIT)
        store.schedule_audits_v2(run, 0)  # the lazy exclusion of Part 5
        assert len(excluded_as(network)) == 2
        assert rows_of(network) == {}
    with store._tx():
        if forged:
            lie = {**records(network, "excluded")[f"0:{keys[5].ss58}"], "evidence_hash": "ab" * 32}
            store._put_record_v2(run, "excluded", f"0:{keys[5].ss58}", lie)
        if closed:
            finality = store._record_v2(run, "finality", "0")
            done = {**finality, "applied": True, "disposition": "SETTLED", "unresolved_audits": 0}
            store._put_record_v2(run, "finality", "0", done)

    push_through(network, D_AUDIT + 1)

    assert rows_of(network) == {f"0:{keys[i].ss58}": ("MINER", "APPLIED") for i in sanctioned}
    assert sorted(c[2] for c in hook.calls) == sorted(
        no_show(run, keys[i].ss58, "commit")[2] for i in sanctioned
    )


@pytest.mark.parametrize(
    ("evidence", "row"),
    [(None, ("MINER", "APPLIED")), ("relay-receipt", ("INFRASTRUCTURE", "WAIVED"))],
    ids=["no-evidence", "the-relay-holds-a-receipt"],
)
def test_a_delta_a_lazy_writer_left_out_is_adopted_with_the_upload_evidence(
    network, hook, monkeypatch, evidence, row
):
    store, run = network.store, network.manifest.run_id()
    keys = seed_round(network, n=6)
    with monkeypatch.context() as restarted:  # the beacon route did not see the round
        restarted.setattr(ChallengeStore, "_reap_exits_v2", lambda *_a, **_k: None)
        push_through(network, D_AUDIT)
        store.schedule_audits_v2(run, 0)
        with store._tx():
            store._db.execute("UPDATE audit_leases_v2 SET state='completed'")
            store._db.execute(
                "DELETE FROM records_v2 WHERE run_id=? AND kind='delta' AND id=?",
                (run, f"0:{keys[5].ss58}"),
            )
        push_through(network, D_AUDIT + 1)
        with store._tx():
            store._exclude_missing_deltas_v2(run, 0)  # what aggregate does
        assert set(excluded_as(network)) == {f"0:{keys[5].ss58}"}
    if evidence:
        upload_evidence(network, evidence, keys[5].ss58, 0)

    push_through(network, D_AUDIT + 2)

    assert rows_of(network) == {f"0:{keys[5].ss58}": row}
    assert len(hook.calls) == (row[1] == "APPLIED")


def test_a_job_without_leaves_is_dropped_at_the_first_absolute_and_nobody_is_sanctioned(
    network, hook
):
    # The commits outlive d_final, so a second audit window is possible: it must not be opened for
    # a job nobody can lease.
    store, run = network.store, network.manifest.run_id()
    keys = seed_round(network, n=5)
    lost = keys[4].ss58
    push_through(network, D_AUDIT)
    store.schedule_audits_v2(run, 0)
    with store._tx():
        store._db.execute("UPDATE audit_leases_v2 SET state='completed' WHERE hotkey!=?", (lost,))
        store._db.execute(
            "DELETE FROM records_v2 WHERE run_id=? AND kind='leaves' AND id=?", (run, f"0:{lost}")
        )
    absolute = store._db.execute(
        "SELECT absolute FROM audit_leases_v2 WHERE hotkey=?", (lost,)
    ).fetchone()[0]
    network.push(absolute - 1)
    assert records(network, "excluded") == {}

    network.push(absolute)

    assert excluded_as(network) == {f"0:{lost}": no_show(run, lost, "leaves")}
    lease = store._db.execute("SELECT state,result FROM audit_leases_v2 WHERE hotkey=?", (lost,))
    assert tuple(lease.fetchone()) == ("exhausted", "NO_LEAVES")
    assert records(network, "audit-rearm") == {}
    assert records(network, "finality")["0"]["unresolved_audits"] == 4
    assert rows_of(network) == {f"0:{lost}": ("INFRASTRUCTURE", "WAIVED")}
    assert hook.calls == []


def test_a_member_whose_leaves_the_master_lost_keeps_its_bond_and_its_standing(six_network):
    n = six_network
    run = n.manifest.run_id()
    escrow = n.store._services(run)[0]
    opening = round_zero(n, fixtures.HOT)
    assert open_round(n, opening, n.client).status_code == 200
    lost = fixtures.HOT[5].ss58
    with n.store._tx():
        for hot in fixtures.HOT:  # all committed and delivered; only the lost member has no leaves
            for kind in ("commit", "delta") + (() if hot.ss58 == lost else ("leaves",)):
                n.store._put_record_v2(run, kind, f"0:{hot.ss58}", {"s": 1})
        n.store._db.execute(
            "INSERT INTO audit_leases_v2(id,run_id,w,hotkey,challenge,created,absolute,state) "
            "VALUES(?,?,0,?,'{}',?,?,'queued')",
            (sha256_hex(b"job"), run, lost, opening.d_audit, opening.d_audit + 100),
        )
    before, burned = standing(n, lost), escrow.balances().burned

    n.push(opening.d_audit + 100)

    assert excluded_as(n) == {f"0:{lost}": no_show(run, lost, "leaves")}
    assert rows_of(n) == {f"0:{lost}": ("INFRASTRUCTURE", "WAIVED")}
    assert standing(n, lost) == before
    assert escrow.balances().burned == burned and escrow.balances().conserved()


def test_a_lagging_pusher_leaves_one_witness_per_pause(network):
    push_through(network, 10)
    for number in range(11, 20):  # the wall clock is at round 20 for the whole catch-up
        push_late(network, fixtures, number, wall=20)

    assert records(network, "beacon-pause") == {"10": {"to": 17}}


def poison(monkeypatch, name: str) -> None:
    def raiser(*_args, **_kwargs):
        raise RuntimeError("poisoned")

    monkeypatch.setattr(ChallengeStore, name, raiser, raising=False)


@pytest.mark.parametrize(
    ("method", "journal", "left_out"),
    [
        ("_reap_round_v2", "0:reap", False),
        ("_open_rounds_v2", "run:exit", False),
        ("_close_blocked_disputes_v2", "run:dispute", True),
    ],
    ids=["round", "reaper", "dispute-reaper"],
)
def test_a_reaper_that_fails_never_stops_the_beacon_or_the_other_reaper(
    network, hook, monkeypatch, method, journal, left_out
):
    store, run = network.store, network.manifest.run_id()
    keys = seed_round(network, n=6, silent=(4, 5))
    silent = {f"0:{keys[i].ss58}" for i in (4, 5)}
    with monkeypatch.context() as broken:
        poison(broken, method)

        network.push(D_AUDIT)  # network.push asserts that the beacon route answered 200
        first = records(network, "exit-failed")
        network.push(D_AUDIT + 1)
        network.push(D_AUDIT + 2)

        assert set(first) == {journal}
        assert records(network, "exit-failed") == first
        assert set(excluded_as(network)) == (silent if left_out else set())

    network.push(D_AUDIT + 3)

    assert set(excluded_as(network)) == silent
    assert store._record_v2(run, "finality", "0")["unresolved_audits"] == 4


def test_a_round_nobody_scheduled_audits_for_is_retired_without_blaming_anyone(network, hook):
    # Given: five members that all committed and delivered, and an audit window nobody used
    store, run = network.store, network.manifest.run_id()
    keys = seeded(network, 5)
    opening = store._record_v2(run, "round", "0")["body"]
    audit = AuditPolicyV2.model_validate_json(
        store.objects.get(network.manifest.network.audit_policy_hash)
    )
    absolute = min(opening["d_final"], opening["d_audit"] + audit.absolute_rounds())
    push_through(network, absolute - 2)
    assert records(network, "retired") == {}

    # When: the push that ends the window
    push_through(network, absolute - 1)

    # Then: the round is retired for the window it missed; the master could not hear anybody, so
    # nobody is excluded, sanctioned or struck
    retired = records(network, "retired")["0"]
    assert (retired["cause"], len(retired["survivors"])) == ("AUDIT_WINDOW_MISSED", 5)
    assert records(network, "finality")["0"]["disposition"] == "RETIRED"
    assert records(network, "excluded") == {} and rows_of(network) == {} and hook.calls == []
    assert set(records(network, "anchor")) >= {f"0:{k.ss58}" for k in keys}


def test_a_retirement_the_run_is_not_ready_for_is_the_reaper_waiting(network, hook):
    # Given: round 0 applied and not finalized, and round 1 opened with a roster nobody committed to
    store, run = network.store, network.manifest.run_id()
    keys = seeded(network, 5)
    with store._tx():
        status = store._record_v2(run, "finality", "0")
        store._put_record_v2(run, "finality", "0", {**status, "applied": True})
    second_round(network)

    # When: the push that ends the commit window of round 1
    push_through(network, D_AUDIT + 1)

    # Then: round 1 cannot go on, but an earlier round is open, so it is marked and not retired;
    # the member blame is booked all the same and the beacon went on
    waiting = records(network, "round-exit")["1"]
    assert waiting == {
        "state": "INFEASIBLE",
        "waiting": "ROUND_RETIRE_REFUSED: an earlier round is not closed",
    }
    assert records(network, "retired") == {}
    assert {k for k in rows_of(network) if k.startswith("1:")} == {f"1:{k.ss58}" for k in keys}


def test_a_retirement_that_fails_keeps_the_blame_and_is_journaled(network, hook, monkeypatch):
    # Given: a round the survivors cannot carry, and a disk that is full when anchors are carried
    keys = seeded(network, 5, silent=(3, 4))

    def full_disk(self, *args):
        raise OSError("no space left on device")

    monkeypatch.setattr(ChallengeStore, "_carry_anchor_v2", full_disk)

    # When
    push_through(network, D_AUDIT)
    push_through(network, D_AUDIT + 3)

    # Then: the round stays as it was, the failure is journaled once, and the silent members are
    # sanctioned all the same
    assert records(network, "retired") == {}
    assert records(network, "finality")["0"]["disposition"] == "PENDING"
    assert records(network, "exit-failed") == {"0:retire": {"error": "OSError", "stage": "retire"}}
    assert rows_of(network) == {f"0:{keys[i].ss58}": ("MINER", "APPLIED") for i in (3, 4)}
    assert len(hook.calls) == 2


def test_an_unverified_exclusion_that_no_mismatch_verdict_backs_is_waived(network, hook):
    # Given: a member excluded as UNVERIFIED with no completed MISMATCH audit behind it (a dropped
    # job, an executor fault): nothing the auditor or its host did may reach the hook
    store, run = network.store, network.manifest.run_id()
    keys = seed_round(network, n=5)
    identity = f"0:{keys[0].ss58}"
    with store._tx():
        store._put_record_v2(
            run, "excluded", identity, {"reason": "UNVERIFIED", "evidence_hash": "ab" * 32}
        )

    # When: the beacon goes on
    push_through(network, D_AUDIT)
    push_through(network, D_AUDIT + 2)

    # Then: the exclusion is booked once, as the infrastructure's, and never applied
    row = records(network, "exit-sanction")[identity]
    assert (row["reason"], row["blame"], row["state"], row["grade"]) == (
        "AUDIT_MISMATCH",
        "INFRASTRUCTURE",
        "WAIVED",
        "NONE",
    )
    assert hook.calls == []
