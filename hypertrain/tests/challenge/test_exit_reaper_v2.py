"""The exit reaper on `push_beacon` leaves out a silent roster member at the bound of its wait."""
# ruff: noqa: F811  (the `network` fixture parameter shadows the imported fixture)

from __future__ import annotations

import json

import pytest

from hypertrain.challenge.store import ChallengeStore
from hypertrain.protocol.hashing import sha256_hex

from . import test_service_network_v2 as fixtures
from .audit_lease_fixture_v2 import D_AUDIT, network, seed_round  # noqa: F401  (fixture import)
from .five_identity_network_v2 import five_identity_network, push_through
from .test_round_refusals_v2 import open_round, round_zero


@pytest.fixture
def six_network(tmp_path_factory):
    """Six ACTIVE identities, rebuilt per test: a round consumes the network."""
    yield from five_identity_network(tmp_path_factory, fixtures, size=6)


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


def no_show(run: str, hotkey: str, stage: str) -> tuple[str, str, str]:
    evidence = sha256_hex(f"ht-no-show/1|{run}|0|{hotkey}|{stage}".encode())
    return ("NO_SHOW", stage, evidence)


def test_the_push_that_closes_the_commit_window_excludes_the_silent_members(six_network):
    n = six_network
    run = n.manifest.run_id()
    opening = round_zero(n, fixtures.HOT)
    assert open_round(n, opening, n.client).status_code == 200
    with n.store._tx():
        for hot in fixtures.HOT[:4]:  # the reaper reads who committed, not what
            n.store._put_record_v2(run, "commit", f"0:{hot.ss58}", {"seeded": True})
    push_through(n, opening.d_commit - 1)
    assert records(n, "excluded") == {}

    push_through(n, opening.d_commit)

    seen = {
        k: (v["reason"], v["stage"], v["evidence_hash"]) for k, v in records(n, "excluded").items()
    }
    assert seen == {f"0:{h.ss58}": no_show(run, h.ss58, "commit") for h in fixtures.HOT[4:]}
    assert records(n, "finality")["0"]["unresolved_audits"] == 4
    settled = every_record(n)
    push_through(n, opening.d_commit + 2)
    assert every_record(n) == settled


@pytest.mark.parametrize(
    ("deltas", "undelivered", "excluded", "marker"),
    [
        (True, [5], {5}, {}),
        (False, [], set(), {"0": {"state": "INFEASIBLE"}}),
    ],
    ids=["one-member-short", "nobody-delivered"],
)
def test_the_push_at_d_upload_leaves_out_an_audited_member_without_a_delta(
    network, deltas, undelivered, excluded, marker
):
    # A round nobody delivered a delta to is an upload path that failed for everybody: no member
    # is left out for it, and the survivors that cannot carry the round are only marked.
    store, run = network.store, network.manifest.run_id()
    keys = seed_round(network, n=6, deltas=deltas)
    network.push(D_AUDIT)
    store.schedule_audits_v2(run, 0)
    with store._tx():
        store._db.execute("UPDATE audit_leases_v2 SET state='completed'")
        for i in undelivered:
            store._db.execute(
                "DELETE FROM records_v2 WHERE run_id=? AND kind='delta' AND id=?",
                (run, f"0:{keys[i].ss58}"),
            )
    network.push(D_AUDIT)  # the bound is d_upload, one round later
    assert records(network, "excluded") == {}

    network.push(D_AUDIT + 1)

    seen = {
        k: (v["reason"], v["stage"], v["evidence_hash"])
        for k, v in records(network, "excluded").items()
    }
    assert seen == {f"0:{keys[i].ss58}": no_show(run, keys[i].ss58, "delta") for i in excluded}
    assert records(network, "round-exit") == marker
    assert records(network, "finality")["0"]["unresolved_audits"] == 6


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
    network, monkeypatch, method, journal, left_out
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
        assert set(records(network, "excluded")) == (silent if left_out else set())

    network.push(D_AUDIT + 3)

    assert set(records(network, "excluded")) == silent
    assert store._record_v2(run, "finality", "0")["unresolved_audits"] == 4
