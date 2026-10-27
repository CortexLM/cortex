"""The dispute event poll is a request signed in its body, never in a header (D51)."""
# ruff: noqa: F811  (the `network` fixture parameter shadows the imported fixture, as in test_multirun.py)

from __future__ import annotations

import pytest

from hypertrain.protocol.keys import Keypair

from .test_service_network_v2 import HOT, OWNER_REQUEST_ROUNDS, network  # noqa: F401


def poll(network, key, *, party=None, cursor=0, wait=0, expires=None, run_id=None):
    """POST the signed poll of `party` (default: the signer), expiring after the current round."""
    body = {"party": (party or key).ss58, "cursor": cursor, "wait_seconds": wait}
    expires = expires or network.store._now(network.store._db) + OWNER_REQUEST_ROUNDS
    request = network.signed(key, "WatchRequest", body, expires)
    if run_id is not None:
        request["run_id"] = run_id
    return network.client.post(network.url + "/disputes", json=request)


def test_the_poll_is_a_signed_body_and_the_old_get_is_gone(network):
    # When: the party asks for its events with a signed envelope in the body
    answered = poll(network, HOT[0])
    # Then: it is answered, and the unsigned or header-signed GET no longer exists
    assert answered.status_code == 200, answered.text
    assert answered.json() == []
    old = network.client.get(
        network.url + "/disputes",
        params={"party": HOT[0].ss58, "cursor": 0, "timeout": 0},
        headers={"X-Dispute-Signature": HOT[0].sign(b"anything").hex()},
    )
    assert old.status_code == 405


@pytest.mark.parametrize(
    "case",
    ["signed by another key", "too far ahead", "expired", "other run", "not a member"],
)
def test_a_poll_that_is_not_the_partys_own_current_request_is_refused(network, case):
    now = network.store._now(network.store._db)
    stranger = Keypair(b"\x5a" * 32)  # a valid key that no role of the run holds
    match case:
        case "signed by another key":
            refused = poll(network, HOT[1], party=HOT[0])
        case "too far ahead":
            refused = poll(network, HOT[0], expires=now + 100_000)
        case "expired":
            network.push(now + 50)
            refused = poll(network, HOT[0], expires=now + 1)
        case "other run":
            refused = poll(network, HOT[0], run_id="ff" * 32)
        case _:
            refused = poll(network, stranger)
    assert refused.status_code == 403, refused.text


def test_a_poll_for_another_cursor_is_not_the_captured_one(network):
    # Given: a captured poll of cursor 0
    now = network.store._now(network.store._db)
    captured = network.signed(
        HOT[0],
        "WatchRequest",
        {"party": HOT[0].ss58, "cursor": 0, "wait_seconds": 0},
        now + OWNER_REQUEST_ROUNDS,
    )
    # When: the body is replayed with another cursor
    forged = {**captured, "body": {**captured["body"], "cursor": 5}}
    refused = network.client.post(network.url + "/disputes", json=forged)
    # Then: the signature no longer covers it
    assert refused.status_code == 403, refused.text
