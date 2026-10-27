"""The dispute watch is authenticated by a signed ht/2 body, never by an HTTP header (D52 I1)."""

from __future__ import annotations

import importlib.util
import sys
import time
from pathlib import Path

import pytest

from hypertrain.challenge.disputes_v2 import Contest, Turn
from hypertrain.protocol.envelope_v2 import signing_message
from hypertrain.protocol.keys import Keypair

spec = importlib.util.spec_from_file_location(
    "dispute_watch_envelope_fixture", Path(__file__).parent / "test_service_network_v2.py"
)
assert spec is not None and spec.loader is not None
fixtures = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = fixtures
spec.loader.exec_module(fixtures)
network = fixtures.network
HOT = fixtures.HOT
HORIZON = 20  # rounds a request may be signed ahead: the bound documented in network-v2.md


def request(n, signer=None, *, party=None, cursor=0, wait=0, exp=None, run_id=None, kind=None):
    """POST a poll sealed by `signer` (default: HOT[0] polling for itself)."""
    signer = signer or HOT[0]
    body = {"party": party or signer.ss58, "cursor": cursor, "wait": wait}
    expiry = n.now + HORIZON if exp is None else exp
    run = run_id or n.manifest.run_id()
    if kind is not None:
        body = {"w": 0, "commit_hash": "ab" * 32, "received_round": 1}
    envelope = fixtures.seal(signer, kind or "DisputeWatchRequest", run, body, expiry)
    return n.client.post(n.url + "/disputes/watch", json=envelope)


def test_a_signed_poll_is_answered_again_and_again_and_the_header_route_is_gone(network):
    network.join()
    before = network.store._db.total_changes
    first = request(network)
    assert first.status_code == 200, first.text
    assert first.json() == []
    # A poll reserves nothing: the identical bytes are good until they expire.
    envelope = fixtures.seal(
        HOT[0],
        "DisputeWatchRequest",
        network.manifest.run_id(),
        {"party": HOT[0].ss58, "cursor": 0, "wait": 0},
        network.now + HORIZON,
    )
    for _ in range(2):
        again = network.client.post(network.url + "/disputes/watch", json=envelope)
        assert again.status_code == 200, again.text
    assert network.store._db.total_changes == before
    # The replayable static header signature no longer opens anything.
    static = HOT[0].sign(f"hypertrain/watch/2|{network.manifest.run_id()}|{HOT[0].ss58}".encode())
    legacy = network.client.get(
        network.url + "/disputes",
        params={"party": HOT[0].ss58},
        headers={"X-Dispute-Signature": static.hex()},
    )
    assert legacy.status_code == 404, legacy.text


def forged_wait(n):
    """Signed with the right key and domain, but a wait beyond the 30 s long-poll bound."""
    body = {"party": HOT[0].ss58, "cursor": 0, "wait": 31}
    run_id = n.manifest.run_id()
    exp = n.now + HORIZON
    return {
        "v": "ht/2",
        "type": "DisputeWatchRequest",
        "run_id": run_id,
        "body": body,
        "signer": HOT[0].ss58,
        "exp_drand": exp,
        "sig": HOT[0].sign(signing_message("DisputeWatchRequest", body, exp, run_id)).hex(),
    }


@pytest.mark.parametrize(
    ("name", "call", "status"),
    [
        ("another key signs for the party", lambda n: request(n, HOT[1], party=HOT[0].ss58), 403),
        ("another run", lambda n: request(n, run_id="ff" * 32), 403),
        ("not a watch request", lambda n: request(n, kind="Receipt"), 403),
        ("further ahead than the bound", lambda n: request(n, exp=n.now + HORIZON + 1), 403),
        ("past its expiry", lambda n: request(n, exp=n.now - 1), 403),
        (
            "body changed after signing",
            lambda n: n.client.post(
                n.url + "/disputes/watch",
                json={
                    **fixtures.seal(
                        HOT[0],
                        "DisputeWatchRequest",
                        n.manifest.run_id(),
                        {"party": HOT[0].ss58, "cursor": 0, "wait": 0},
                        n.now + HORIZON,
                    ),
                    "body": {"party": HOT[0].ss58, "cursor": 7, "wait": 0},
                },
            ),
            403,
        ),
        (
            "wait beyond 30 s",
            lambda n: n.client.post(n.url + "/disputes/watch", json=forged_wait(n)),
            403,
        ),
        ("the old header with no body", lambda n: n.client.post(n.url + "/disputes/watch"), 403),
    ],
)
def test_a_refused_poll_answers_nothing_and_changes_nothing(network, name, call, status):
    network.join()
    network.join(1)
    network.push(network.now + 5)  # so that "past its expiry" is a real past
    before = network.store._db.total_changes
    refused = call(network)
    assert refused.status_code == status, (name, refused.text)
    assert network.store._db.total_changes == before


def test_a_captured_poll_is_good_for_the_bound_and_not_one_round_more(network):
    network.join()
    envelope = fixtures.seal(
        HOT[0],
        "DisputeWatchRequest",
        network.manifest.run_id(),
        {"party": HOT[0].ss58, "cursor": 0, "wait": 0},
        network.now + HORIZON,  # the furthest the master accepts
    )
    url = network.url + "/disputes/watch"
    assert network.client.post(url, json=envelope).status_code == 200
    network.push(network.now + HORIZON)  # the last round it is good for
    assert network.client.post(url, json=envelope).status_code == 200
    network.push(network.now + 1)
    replayed = network.client.post(url, json=envelope)
    assert replayed.status_code == 403


def test_an_unknown_key_signing_for_itself_is_still_not_a_party(network):
    # Signs correctly, but is neither a pinned role nor registered on the subnet.
    stranger = Keypair(bytes([201]) * 32)
    denied = request(network, stranger)
    assert denied.status_code == 403, denied.text
    assert "not registered on the subnet" in denied.json()["detail"]


def test_a_poll_before_any_beacon_is_a_503_not_an_envelope_error(network):
    network.join()
    network.store._db.execute("DELETE FROM beacon")  # a master that has verified no round yet
    waiting = request(network)
    assert waiting.status_code == 503, waiting.text
    assert waiting.json()["detail"] == "no verified beacon"


def seeded_event(n, party):
    """A real signed event for `party`, written the way the dispute service writes one."""
    run_id = n.manifest.run_id()
    disputes = n.store.for_run(run_id)._services(run_id)[2]
    contest = Contest(
        verdict_hash="aa" * 32,
        challenge_hash="bb" * 32,
        w=0,
        miner=HOT[0].ss58,
        auditor=fixtures.AUDITORS[0].ss58,
        referee=fixtures.REFEREE.ss58,
        step_span=1,
        layer_span=1,
        op_span=1,
    )
    turn = Turn(
        run_id=run_id,
        dispute_id="cc" * 32,
        contest=contest,
        seq=0,
        level="step",
        ctx=[],
        interval=(0, 2),
        expected_party=party,
        transcript_hash="dd" * 32,
        turn_deadline=1000,
        absolute_deadline=2000,
        lock_id="ee" * 32,
    )
    with n.store._tx():
        return disputes._event(turn, n.now)


def test_a_poll_returns_the_events_after_its_cursor_for_its_own_party_only(network):
    network.join()
    network.join(1)
    event = seeded_event(network, HOT[0].ss58)
    assert [e["cursor"] for e in request(network, cursor=0).json()] == [event.cursor]
    assert request(network, cursor=event.cursor).json() == []
    assert request(network, HOT[1]).json() == []  # another party sees nothing of it


def test_the_wait_of_a_poll_is_honoured(network):
    network.join()
    started = time.monotonic()
    assert request(network, wait=1).json() == []
    assert time.monotonic() - started >= 0.9
