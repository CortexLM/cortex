"""The miner's poll is a signed ht/2 body with a bounded expiry, never a header (D52 I1)."""

from __future__ import annotations

import json

import httpx
import pytest

from hypertrain.challenge.disputes_v2 import DisputeError
from hypertrain.miner.dispute_watch import HttpDisputeTransport
from hypertrain.protocol.envelope_v2 import Intake
from hypertrain.protocol.keys import Keypair

KEY = Keypair(b"\x42" * 32)
RUN = "ab" * 32


def transport(seen: list[httpx.Request], now: int = 40) -> HttpDisputeTransport:
    def answer(request: httpx.Request) -> httpx.Response:
        seen.append(request)
        return httpx.Response(200, json=[])

    client = httpx.Client(transport=httpx.MockTransport(answer))
    return HttpDisputeTransport(
        client, "https://service.test/v2/runs/" + RUN, KEY, RUN, lambda: now
    )


def test_the_poll_is_a_post_signed_in_its_body_and_expires_20_rounds_ahead() -> None:
    seen: list[httpx.Request] = []
    assert transport(seen).events(KEY.ss58, 7, timeout=5) == []
    (request,) = seen
    assert (request.method, request.url.path) == ("POST", "/v2/runs/" + RUN + "/disputes/watch")
    assert not [h for h in request.headers if h.lower().startswith("x-")]
    envelope = json.loads(request.content)
    # The master's own intake accepts it at the round the miner read, and for 20 rounds more.
    poll = Intake(RUN, {"DisputeWatchRequest": KEY.ss58.__eq__}).accept(request.content, 40)
    assert poll.model_dump() == {"party": KEY.ss58, "cursor": 7, "wait": 5}
    assert envelope["exp_drand"] == 60 and envelope["signer"] == KEY.ss58
    assert Intake(RUN, {}).accept(request.content, 60)  # still good on its last round


def test_a_fractional_wait_is_rounded_up_and_the_bound_is_kept() -> None:
    seen: list[httpx.Request] = []
    transport(seen).events(KEY.ss58, 0, timeout=0.2)
    assert json.loads(seen[0].content)["body"]["wait"] == 1
    for timeout in (-1, 31):
        with pytest.raises(DisputeError, match="LONG_POLL_BOUND"):
            transport(seen).events(KEY.ss58, 0, timeout=timeout)
    assert len(seen) == 1
