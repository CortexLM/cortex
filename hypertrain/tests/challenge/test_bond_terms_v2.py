"""The terms of an admission bond are the holder's: no anonymous answer names a lock or an origin.

D51 and D52 I2 (ADR 0014 point 9): origin ids are never readable by a third party. The public
status of an admission keeps the amounts and the state of the bond; the lock id, the origin ids and
the two hashes a release names come from `POST .../join/{admission_id}/bond`, a `BondRequest` signed
in the body by the admission's coldkey, or from the admin route behind the operator bearer.
"""

# ruff: noqa: F811  (imported fixtures are asked for by name)
from __future__ import annotations

import pytest

from hypertrain.protocol.messages_v2 import REQUEST_HORIZON, EscrowRelease, EscrowTransfer

from .test_admission_hardening_v2 import (  # noqa: F401  (`network` is a pytest fixture)
    balances,
    lock_bond,
    network,
    release_request,
    service,
)
from .test_subnet_gate_v2 import HOT, gated, refresh, subnet_master  # noqa: F401
from .test_subnet_gate_v2 import lock as gated_lock

PUBLIC = {"units", "outcome", "releasable", "release_beacon"}
HOLDER_ONLY = {"lock_id", "origin_ids", "finality_hash", "closed_dispute_root"}


def public_status(n, hotkey=None):
    return n.client.get(n.url + f"/admission/{(hotkey or service.HOT[0]).ss58}")


def terms_of(n, identity):
    answer = n.bond(identity)
    assert answer.status_code == 200, answer.text
    (entry,) = answer.json()["locks"]
    return entry


def test_the_public_status_of_a_bonded_admission_keeps_the_amounts_and_names_no_identifier(network):
    # Given: an admission that holds a bond of 1000 units
    identity, lock = lock_bond(network)
    entry = terms_of(network, identity)

    # When: anybody reads its status
    status = public_status(network)

    # Then: the amounts and the settlement state are there, none of the identifiers a release
    # names, and none of their values anywhere in the answer
    assert status.status_code == 200
    (public,) = status.json()["bond"]["locks"]
    assert set(public) == PUBLIC
    assert (public["units"], public["outcome"], public["releasable"]) == (1000, "MATCH", True)
    for secret in (lock.operation_id, *lock.origin_ids, entry["finality_hash"]):
        assert secret not in status.text
    assert entry["closed_dispute_root"] not in status.text


def test_the_holder_reads_the_terms_the_operator_reads(network):
    identity, lock = lock_bond(network)

    held = terms_of(network, identity)
    admin = network.client.get(
        network.url + f"/admin/join/{identity}/bond", headers=service.admin()
    )

    assert (held["lock_id"], held["origin_ids"], held["units"]) == (
        lock.operation_id,
        lock.origin_ids,
        1000,
    )
    assert set(held) == PUBLIC | HOLDER_ONLY
    assert admin.status_code == 200 and admin.json() == {"locks": [held]}


@pytest.mark.parametrize("cut", [True, False], ids=["bearer-missing", "bearer-wrong"])
def test_the_operator_route_wants_the_operator_bearer(network, cut):
    identity, _ = lock_bond(network)
    headers = {} if cut else {"Authorization": "Bearer " + "x" * 32}

    answer = network.client.get(network.url + f"/admin/join/{identity}/bond", headers=headers)

    assert answer.status_code == 401
    assert "finality_hash" not in answer.text


def forgeries(n, identity):
    """Bodies that are not the coldkey's unexpired BondRequest for `identity`."""
    cold, rival, hot = service.COLD[0], service.COLD[1], service.HOT[0]
    now = n.store._now(n.store._db)
    mine = n.bond_request(identity)
    return {
        "no body": b"",
        "a body that is not json": b"not json",
        "a rival coldkey signing as itself": n.bond_request(identity, rival),
        "the admission's hotkey signing": n.bond_request(identity, hot),
        "the coldkey's request for another admission": n.signed(
            cold, "BondRequest", {"admission_id": "ee" * 32}, now + 5
        ),
        "the coldkey's request under another run": service.seal(
            cold, "BondRequest", "ff" * 32, {"admission_id": identity}, now + 5
        ),
        "the hotkey's challenge request": n.owner_request("ChallengeRequest", identity),
        "the hotkey's job request": n.owner_request("JobRequest", identity),
        "another message of the coldkey": n.signed(
            cold, "Receipt", {"w": 0, "commit_hash": "11" * 32, "received_round": 1}, now + 5
        ),
        "an expired request": n.bond_request(identity, expires=now - 1),
        "a request signed further ahead than the horizon": n.bond_request(
            identity, expires=now + REQUEST_HORIZON + 1
        ),
        "a request with the signature cut": {**mine, "sig": "00" * 64},
        "a request with a rival as signer": {**mine, "signer": rival.ss58},
    }


def test_the_bond_route_refuses_everyone_but_the_coldkey_of_the_admission(network):
    identity, _ = lock_bond(network)
    network.push(50)
    url = network.url + f"/join/{identity}/bond"
    changes = network.store._db.total_changes

    for name, body in forgeries(network, identity).items():
        answer = (
            network.client.post(url, content=body)
            if isinstance(body, bytes)
            else network.client.post(url, json=body)
        )
        assert answer.status_code == 403, name
        assert "finality_hash" not in answer.text, name

    # the operator bearer is not a signature, a GET carries no signature at all
    assert network.client.post(url, headers=service.admin()).status_code == 403
    assert network.client.get(url).status_code == 405
    assert network.store._db.total_changes == changes  # nothing was written
    assert network.bond(identity).status_code == 200  # and the holder is still served


def test_an_admission_nobody_knows_is_a_409_and_an_unknown_run_a_404(network):
    unknown = "ee" * 32

    assert network.bond(unknown).status_code == 409
    assert (
        network.client.post(f"/v2/runs/{unknown}/join/{unknown}/bond", json={}).status_code == 404
    )


def test_the_holder_releases_the_bond_with_the_terms_of_the_route(network):
    identity, lock = lock_bond(network)
    entry = terms_of(network, identity)

    released = network.client.post(
        network.url + "/escrow/release", json=release_request(network, identity, lock, entry)
    )

    assert released.status_code == 200, released.text
    assert balances(network).admission_locked == 0 and balances(network).conserved()
    assert network.bond(identity).json() == {"locks": []}


def test_a_lock_id_learned_elsewhere_grants_nothing_without_the_coldkey_signature(network):
    # Given: the full terms of a bond, as a rival who read them somewhere else would have them
    identity, lock = lock_bond(network)
    entry = terms_of(network, identity)
    rival = service.COLD[1]
    before = balances(network)
    events = network.store._db.execute("SELECT COUNT(*) FROM escrow_events").fetchone()[0]
    stolen_release = EscrowRelease(
        operation_id="ce" * 32,
        owner=lock.owner,
        units=1000,
        origin_ids=entry["origin_ids"],
        admission_id=identity,
        dispute_id=None,
        lock_id=lock.operation_id,
        finality_hash=entry["finality_hash"],
        closed_dispute_root=entry["closed_dispute_root"],
    )
    stolen_transfer = EscrowTransfer(
        operation_id="cf" * 32,
        owner=lock.owner,
        recipient=rival.ss58,
        units=1,
        origin_ids=entry["origin_ids"],
        admission_id=None,
        dispute_id=None,
    )

    # When: the rival signs a release and a transfer of the holder's lock and origins
    attempts = [
        network.client.post(
            network.url + "/escrow/release",
            json=network.signed(rival, "EscrowRelease", stolen_release),
        ),
        network.client.post(
            network.url + "/escrow/release",
            json=network.signed(
                rival,
                "EscrowRelease",
                stolen_release.model_copy(update={"owner": rival.ss58, "operation_id": "d0" * 32}),
            ),
        ),
        network.client.post(
            network.url + "/escrow/transfer",
            json=network.signed(rival, "EscrowTransfer", stolen_transfer),
        ),
    ]

    # Then: every one is refused and the ledger did not move
    assert [a.status_code for a in attempts] == [409, 409, 409], [a.text for a in attempts]
    assert balances(network) == before
    assert network.store._db.execute("SELECT COUNT(*) FROM escrow_events").fetchone()[0] == events


def test_a_hotkey_that_left_the_subnet_does_not_strand_its_bond(gated, subnet_master):
    # Given: an admitted hotkey with a bond, then deregistered
    n = gated
    subnet_master.registered.add(HOT[0].ss58)
    identity = n.join(0).json()["admission_id"]
    gated_lock(n, 0, identity)
    subnet_master.registered.clear()
    refresh(n)

    # When / Then: its trial routes are closed to it, the holder's bond terms are not: the coldkey
    # owns the lock and signs the release, and neither depends on the hotkey's registration
    assert n.challenge(identity).status_code == 403
    answer = n.bond(identity)
    assert answer.status_code == 200, answer.text
    assert answer.json()["locks"][0]["units"] == 1000
