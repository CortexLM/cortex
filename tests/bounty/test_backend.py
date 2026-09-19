"""Reject inconsistent publications before they can produce any paid leaf."""

import httpx
import pytest

from cortex.bounty import BackendUnavailable, PublicBackend

HOTKEY = "ab" * 32


def published_report(report_id="r1", **changes):
    return {
        "id": report_id,
        "hotkey": HOTKEY,
        "status": "valid",
        "severity": "critical",
        "problem_found": "Unauthorized configuration change",
        "adjudicator": "operator",
        "justification": "Reproduced from an unauthenticated client",
        "created_at": "2026-09-16T00:00:00Z",
        "adjudicated_at": "2026-09-16T01:00:00Z",
        **changes,
    }


def backend_for(leaderboard, reports, /, **tokens):
    def handle(request):
        route = request.url.path.rsplit("/", 1)[-1]
        body = {"items": leaderboard if route == "leaderboard" else reports}
        if route in tokens:
            body["revision"] = tokens[route]
        return httpx.Response(200, json=body)

    return PublicBackend("https://backend.invalid", transport=httpx.MockTransport(handle))


@pytest.mark.parametrize(
    "leaderboard,reports,tokens",
    [
        ([{"hotkey": HOTKEY, "valid_count": 2}], [published_report()], {}),
        ([], [published_report()], {}),
        ([{"hotkey": HOTKEY, "valid_count": 1}] * 2, [published_report()], {}),
        ([{"hotkey": HOTKEY, "valid_count": 2}], [published_report()] * 2, {}),
        (
            [{"hotkey": HOTKEY, "valid_count": 1}],
            [published_report()],
            {"leaderboard": "a", "reports": "b"},
        ),
        ([{"hotkey": "invalid", "valid_count": 1}], [published_report(hotkey="invalid")], {}),
    ],
)
async def test_stable_but_incoherent_feed_is_refused(leaderboard, reports, tokens):
    backend = backend_for(leaderboard, reports, **tokens)

    with pytest.raises(BackendUnavailable):
        await backend.fetch()


async def test_moving_feed_cannot_be_mistaken_for_stable_scores():
    request_number = 0

    def handle(request):
        nonlocal request_number
        request_number += 1
        return httpx.Response(200, json={"items": [], "revision": str(request_number)})

    backend = PublicBackend("https://backend.invalid", transport=httpx.MockTransport(handle))

    with pytest.raises(BackendUnavailable, match="changed under every read"):
        await backend.fetch()


async def test_ignored_metadata_does_not_make_a_stable_feed_unreadable():
    request_number = 0

    def handle(request):
        nonlocal request_number
        request_number += 1
        return httpx.Response(200, json={"items": [], "generated_at": request_number})

    backend = PublicBackend("https://backend.invalid", transport=httpx.MockTransport(handle))

    snapshot = await backend.fetch()

    assert snapshot.score([HOTKEY])[HOTKEY].reason == "NotAttempted"


async def test_unpriced_valid_is_a_gate_even_after_three_priced_reports():
    reports = [published_report(f"r{index}") for index in range(3)]
    reports.append(published_report("unpriced", severity=None))
    backend = backend_for([{"hotkey": HOTKEY, "valid_count": 4}], reports)

    scores = (await backend.fetch()).score([HOTKEY])

    assert scores[HOTKEY].value == 0
    assert scores[HOTKEY].reason == "NotAttempted"


async def test_malicious_published_row_without_severity_burns_the_hotkey():
    backend = backend_for([], [published_report(status="invalid_malicious", severity=None)])

    scores = (await backend.fetch()).score([HOTKEY])

    assert scores[HOTKEY].value == 0
    assert scores[HOTKEY].reason == "InvalidResponse"


async def test_leaderboard_weight_cannot_invent_evidence():
    reports = [published_report(f"r{index}", justification="") for index in range(3)]
    backend = backend_for([{"hotkey": HOTKEY, "valid_count": 3, "weight": 1_000_000}], reports)

    scores = (await backend.fetch()).score([HOTKEY])

    assert scores[HOTKEY].value == 0


@pytest.mark.parametrize("body", [b"not json", b'{"items":{}}', b'{"items":[{}]}'])
async def test_unparseable_feed_is_a_scoring_outage(body):
    backend = PublicBackend(
        "https://backend.invalid",
        transport=httpx.MockTransport(lambda request: httpx.Response(200, content=body)),
    )

    with pytest.raises(BackendUnavailable):
        await backend.fetch()
