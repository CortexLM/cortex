"""A roster member that never commits is left out of its round; the others are audited."""
# ruff: noqa: F811  (the `network` fixture parameter shadows the imported fixture)

from __future__ import annotations

import json

import pytest

from hypertrain.challenge.store import ChallengeError
from hypertrain.protocol.hashing import sha256_hex

from .audit_lease_fixture_v2 import (
    D_AUDIT,
    network,  # noqa: F401  (pytest fixture import)
    seed_round,
)


def job_id(run: str, hotkey: str) -> str:
    return sha256_hex(f"ht-audit-job/2|{run}|0|{hotkey}".encode())


def excluded(store, run: str) -> dict[str, dict]:
    rows = store._db.execute(
        "SELECT id,data FROM records_v2 WHERE run_id=? AND kind='excluded'", (run,)
    )
    return {r[0]: json.loads(r[1]) for r in rows}


@pytest.mark.parametrize(("size", "silent"), [(5, (4,)), (6, (1, 4))])
def test_a_member_that_never_commits_no_longer_stalls_the_round(network, size, silent):
    store, run = network.store, network.manifest.run_id()
    keys = seed_round(network, n=size, silent=silent)
    present = [k.ss58 for i, k in enumerate(keys) if i not in silent]
    absent = [k.ss58 for i, k in enumerate(keys) if i in silent]
    network.push(D_AUDIT)

    jobs = store.schedule_audits_v2(run, 0)

    assert sorted(jobs) == sorted(job_id(run, h) for h in present)
    leased = {r[0] for r in store._db.execute("SELECT hotkey FROM audit_leases_v2")}
    assert leased == set(present)
    seen = excluded(store, run)
    assert {k: (v["reason"], v["stage"]) for k, v in seen.items()} == {
        f"0:{h}": ("NO_SHOW", "commit") for h in absent
    }
    assert seen[f"0:{absent[0]}"]["evidence_hash"] == sha256_hex(
        f"ht-no-show/1|{run}|0|{absent[0]}|commit".encode()
    )
    finality = store._record_v2(run, "finality", "0")
    assert (finality["disposition"], finality["applied"], finality["rollback_complete"]) == (
        "PENDING",
        False,
        False,
    )
    assert finality["unresolved_audits"] == len(jobs)


def test_scheduling_twice_counts_each_no_show_once(network):
    store, run = network.store, network.manifest.run_id()
    keys = seed_round(network, n=5, silent=(4,))
    network.push(D_AUDIT)

    first = store.schedule_audits_v2(run, 0)
    second = store.schedule_audits_v2(run, 0)

    assert len(first) == 4 and second == []
    assert store._record_v2(run, "finality", "0")["unresolved_audits"] == 4
    assert list(excluded(store, run)) == [f"0:{keys[4].ss58}"]


@pytest.mark.parametrize(
    ("size", "silent"), [(4, (3,)), (5, (3, 4)), (5, (0, 1, 2, 3, 4))], ids=["4-1", "5-2", "5-all"]
)
def test_a_round_the_survivors_cannot_carry_excludes_nobody(network, size, silent):
    store, run = network.store, network.manifest.run_id()
    seed_round(network, n=size, silent=silent)
    network.push(D_AUDIT)
    before = store._record_v2(run, "finality", "0")

    with pytest.raises(ChallengeError, match="ROUND_INFEASIBLE") as refused:
        store.schedule_audits_v2(run, 0)

    assert refused.value.status == 409
    assert excluded(store, run) == {}
    assert store._db.execute("SELECT COUNT(*) FROM audit_leases_v2").fetchone()[0] == 0
    assert store._record_v2(run, "finality", "0") == before


def test_the_delta_stage_leaves_out_only_audited_members_that_are_not_excluded(network):
    # Kills two mutants the end-to-end delta test cannot tell apart from the fix: dropping the
    # "audit completed" condition (every member whose audit is still queued would be left out
    # and its unresolved audit stranded) and overwriting an earlier exclusion (a failed audit
    # would be renamed NO_SHOW). The end-to-end round has only completed audits and no cheater.
    store, run = network.store, network.manifest.run_id()
    keys = seed_round(network, n=5)
    network.push(D_AUDIT)
    store.schedule_audits_v2(run, 0)
    unverified = {"reason": "UNVERIFIED", "evidence_hash": "ab" * 32}
    with store._tx():
        store._db.execute(
            "UPDATE audit_leases_v2 SET state='completed' WHERE hotkey IN (?,?)",
            (keys[0].ss58, keys[1].ss58),
        )
        store._put_record_v2(run, "excluded", f"0:{keys[1].ss58}", unverified)
    network.push(D_AUDIT + 1)

    with store._tx():
        store._exclude_missing_deltas_v2(run, 0)

    seen = excluded(store, run)
    assert {k: v["reason"] for k, v in seen.items()} == {
        f"0:{keys[0].ss58}": "NO_SHOW",
        f"0:{keys[1].ss58}": "UNVERIFIED",
    }
    assert seen[f"0:{keys[0].ss58}"]["stage"] == "delta"
    assert seen[f"0:{keys[1].ss58}"] == unverified
    assert store._record_v2(run, "finality", "0")["unresolved_audits"] == 5
