"""A service-admitted run's Finalize lists the roster less the convicted coldkeys (D64-m3).

The service path checks the Finalize against the roster of the opening before it aggregates. It used
to compare with the whole roster, so one convicted member made the round unfinalizable. The service
boundary and the aggregate are replaced by stand-ins: the comparison is the unit under test.
"""
# ruff: noqa: F811  (the `network` fixture parameter shadows the imported fixture)

from __future__ import annotations

import pytest

from hypertrain.challenge.store import ChallengeError
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages import Finalize

from . import test_service_network_v2 as fixtures
from .audit_lease_fixture_v2 import network, seed_round  # noqa: F401  (pytest fixture import)

THETA = "bb" * 32


class Reached(Exception):
    """The comparison passed and the store went on to the aggregate."""


def admit(store, run, hotkey, coldkey, state):
    with store._tx():
        store._db.execute(
            "INSERT INTO admissions_v2(admission_id,run_id,hotkey,coldkey,state) VALUES(?,?,?,?,?)",
            (f"adm:{hotkey}", run, hotkey, coldkey, state),
        )


@pytest.mark.parametrize("lists", ["the-whole-roster", "the-unconvicted-ones"])
def test_the_service_finalize_lists_the_roster_less_the_convicted_coldkeys(
    network, monkeypatch, lists
):
    # Given: a roster of four, the first member convicted, the second run by the same owner, the
    # third an admission of another owner, the fourth no admission of this run at all
    store, run = network.store, network.manifest.run_id()
    keys = seed_round(network, n=4)
    owner, other = fixtures.COLD[0].ss58, fixtures.COLD[1].ss58
    admit(store, run, keys[0].ss58, owner, "BANNED")
    admit(store, run, keys[1].ss58, owner, "ACTIVE")
    admit(store, run, keys[2].ss58, other, "ACTIVE")
    with store._tx():
        store._put_record_v2(run, "applied", "0", {"theta_hash": THETA, "kind": "tape"})
    monkeypatch.setattr(store, "_service_boundary_v2", lambda *a, **k: {"service": True})

    def aggregate(*_args, **_kwargs):
        raise Reached

    monkeypatch.setattr(store, "aggregate_v2", aggregate)
    included = keys if lists == "the-whole-roster" else keys[2:]
    final = Finalize(
        w=0,
        final_theta_hash_w1=THETA,
        included=sorted(k.ss58 for k in included),
        entitlements_root="0" * 64,
    )
    raw = canonicalize(network.signed(fixtures.COORD, "Finalize", final))

    # When / Then: the whole roster is a stale list; the roster less the convicted coldkey goes on
    if lists == "the-whole-roster":
        with pytest.raises(ChallengeError, match="SERVICE_FINALIZE_AUTHORITY") as refused:
            store.finalize_v2(run, 0, raw)
        assert refused.value.status == 409
    else:
        with pytest.raises(Reached):
            store.finalize_v2(run, 0, raw)
