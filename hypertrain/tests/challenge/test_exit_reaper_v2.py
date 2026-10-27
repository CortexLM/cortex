"""The exit reaper on `push_beacon` leaves out a silent roster member at the bound of its wait,
books whose fault it is, and sanctions the miner."""
# ruff: noqa: F811  (the `network` fixture parameter shadows the imported fixture)

from __future__ import annotations

import json

import pytest

from hypertrain.challenge.store import ChallengeStore
from hypertrain.protocol import relay_envelope
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.relay_messages import MasterAcceptanceV2, RelayReceipt

from . import test_service_network_v2 as fixtures
from .audit_lease_fixture_v2 import D_AUDIT, network, seed_round  # noqa: F401  (fixture import)
from .five_identity_network_v2 import five_identity_network, push_late, push_through
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
    # is left out for it, and the survivors that cannot carry the round are only marked.
    store, run = network.store, network.manifest.run_id()
    keys = seed_round(network, n=6, deltas=deltas)
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
    assert records(network, "finality")["0"]["unresolved_audits"] == 6
    assert rows_of(network) == {f"0:{keys[i].ss58}": row for i, row in booked.items()}
    assert hook.calls == [
        (admission_id(i), "NO_SHOW", no_show(run, keys[i].ss58, "delta")[2]) for i in applied
    ]


def test_a_round_the_survivors_cannot_carry_still_sanctions_its_silent_members(network, hook):
    store, run = network.store, network.manifest.run_id()
    keys = seed_round(network, n=5, silent=(3, 4))
    push_through(network, D_AUDIT)
    push_through(network, D_AUDIT + 3)

    assert records(network, "excluded") == {}
    assert records(network, "round-exit") == {"0": {"state": "INFEASIBLE"}}
    assert rows_of(network) == {f"0:{keys[i].ss58}": ("MINER", "APPLIED") for i in (3, 4)}
    assert sorted(hook.calls) == sorted(
        (admission_id(i), "NO_SHOW", no_show(run, keys[i].ss58, "commit")[2]) for i in (3, 4)
    )
    assert store._record_v2(run, "finality", "0")["unresolved_audits"] == 5


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
