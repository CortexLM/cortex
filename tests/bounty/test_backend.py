"""Reject inconsistent publications before they can produce any paid leaf."""

import asyncio
import hashlib

import httpx
import pytest

from cortex.bounty import BackendUnavailable, PublicBackend
from cortex.protocol.crypto import decode_hotkey, public_key

HOTKEY = "ab" * 32
CURRENT_HOTKEY = public_key(bytes([7]) * 32).hex()
HISTORICAL_HOTKEY = public_key(bytes([8]) * 32).hex()


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


def feed_body(route, leaderboard, reports, *, revision="1"):
    if route == "status":
        return {
            "api_version": 1,
            "revision": revision,
            "adjudication_available": True,
            "published": len(reports),
            "valid": sum(row["status"] == "valid" for row in reports),
            "duplicate": sum(row["status"] == "duplicate" for row in reports),
            "already_fixed_not_prod": sum(
                row["status"] == "already_fixed_not_prod" for row in reports
            ),
            "invalid_malicious": sum(row["status"] == "invalid_malicious" for row in reports),
            "hotkeys": len({row["hotkey"] for row in reports}),
            "awaiting_adjudication": 0,
            "unpriced_valid": 0,
        }
    if route == "leaderboard":
        return {
            "api_version": 1,
            "revision": revision,
            "items": leaderboard,
            "has_more": False,
        }
    return {
        "api_version": 1,
        "revision": revision,
        "items": reports,
        "count": len(reports),
        "has_more": False,
        "next_cursor": None,
    }


def backend_for(leaderboard, reports, /, **tokens):
    def handle(request):
        route = request.url.path.rsplit("/", 1)[-1]
        body = feed_body(
            route,
            leaderboard,
            reports,
            revision=tokens.get(route, tokens.get("status", "1")),
        )
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
            {"leaderboard": "2", "reports": "3"},
        ),
        ([{"hotkey": "invalid", "valid_count": 1}], [published_report(hotkey="invalid")], {}),
    ],
)
async def test_stable_but_incoherent_feed_is_refused(leaderboard, reports, tokens):
    backend = backend_for(leaderboard, reports, **tokens)

    with pytest.raises(BackendUnavailable):
        await backend.fetch()


async def test_moving_feed_cannot_be_mistaken_for_stable_scores():
    def handle(request):
        route = request.url.path.rsplit("/", 1)[-1]
        revision = "1" if route == "status" else "2"
        return httpx.Response(200, json=feed_body(route, [], [], revision=revision))

    backend = PublicBackend("https://backend.invalid", transport=httpx.MockTransport(handle))

    with pytest.raises(BackendUnavailable, match="revision changed"):
        await backend.fetch()


async def test_transient_json_and_revision_rollout_errors_are_retried():
    status_reads = 0

    def handle(request):
        nonlocal status_reads
        route = request.url.path.rsplit("/", 1)[-1]
        if route == "status":
            status_reads += 1
            if status_reads == 1:
                return httpx.Response(200, content=b"{")
        revision = "2" if route == "leaderboard" and status_reads == 2 else "1"
        return httpx.Response(200, json=feed_body(route, [], [], revision=revision))

    backend = PublicBackend("https://backend.invalid", transport=httpx.MockTransport(handle))
    backend._SNAPSHOT_RETRY_DELAY_SECONDS = 0

    snapshot = await backend.fetch()

    assert snapshot.reports == ()
    assert status_reads == 3


async def test_transient_failures_stop_after_the_bounded_attempt_count():
    requests = 0

    def unavailable(request):
        nonlocal requests
        requests += 1
        return httpx.Response(503)

    backend = PublicBackend("https://backend.invalid", transport=httpx.MockTransport(unavailable))
    backend._SNAPSHOT_RETRY_DELAY_SECONDS = 0

    with pytest.raises(BackendUnavailable, match="HTTP 503"):
        await backend.fetch()

    assert requests == backend._SNAPSHOT_ATTEMPTS


async def test_ignored_metadata_does_not_make_a_stable_feed_unreadable():
    request_number = 0

    def handle(request):
        nonlocal request_number
        request_number += 1
        route = request.url.path.rsplit("/", 1)[-1]
        body = feed_body(route, [], [])
        body["generated_at"] = request_number
        return httpx.Response(200, json=body)

    backend = PublicBackend("https://backend.invalid", transport=httpx.MockTransport(handle))

    snapshot = await backend.fetch()

    assert snapshot.score([HOTKEY])[HOTKEY].reason == "NotAttempted"


async def test_backend_public_leaderboard_valid_field_is_supported():
    backend = backend_for([{"hotkey": HOTKEY, "valid": 1}], [published_report()])

    snapshot = await backend.fetch()

    assert snapshot.leaderboard[0].valid_count == 1


async def test_backend_rejects_conflicting_leaderboard_count_aliases():
    backend = backend_for(
        [{"hotkey": HOTKEY, "valid": 1, "valid_count": 2}],
        [published_report()],
    )

    with pytest.raises(BackendUnavailable):
        await backend.fetch()


@pytest.mark.parametrize(
    "revision",
    ["-1", "01", "x1", "1x", str(2**63)],
)
async def test_backend_rejects_noncanonical_or_out_of_range_revisions(revision):
    backend = backend_for([], [], status=revision)

    with pytest.raises(BackendUnavailable):
        await backend.fetch()


async def test_backend_report_read_uses_the_largest_supported_page():
    seen = []

    def handle(request):
        seen.append(request.url)
        route = request.url.path.rsplit("/", 1)[-1]
        return httpx.Response(200, json=feed_body(route, [], []))

    backend = PublicBackend("https://backend.invalid", transport=httpx.MockTransport(handle))

    await backend.fetch()

    report_reads = [url for url in seen if url.path.endswith("/reports")]
    assert report_reads and all(url.params.get("limit") == "100" for url in report_reads)


async def test_backend_reads_every_report_page_at_the_status_revision():
    seen = []

    def handle(request):
        seen.append(request.url)
        route = request.url.path.rsplit("/", 1)[-1]
        if route == "status":
            return httpx.Response(
                200,
                json={
                    "api_version": 1,
                    "revision": "7",
                    "adjudication_available": True,
                    "published": 2,
                    "valid": 2,
                    "duplicate": 0,
                    "already_fixed_not_prod": 0,
                    "invalid_malicious": 0,
                    "hotkeys": 1,
                    "awaiting_adjudication": 0,
                    "unpriced_valid": 0,
                },
            )
        if route == "leaderboard":
            return httpx.Response(
                200,
                json={
                    "api_version": 1,
                    "revision": "7",
                    "items": [{"hotkey": HOTKEY, "valid": 2}],
                    "has_more": False,
                },
            )
        cursor = request.url.params.get("cursor")
        item = published_report("r2" if cursor else "r1")
        return httpx.Response(
            200,
            json={
                "api_version": 1,
                "revision": "7",
                "items": [item],
                "count": 1,
                "has_more": cursor is None,
                "next_cursor": "next" if cursor is None else None,
            },
        )

    backend = PublicBackend("https://backend.invalid", transport=httpx.MockTransport(handle))

    snapshot = await backend.fetch()

    assert [report.id for report in snapshot.reports] == ["r1", "r2"]
    report_reads = [url for url in seen if url.path.endswith("/reports")]
    assert [url.params.get("revision") for url in report_reads] == ["7", "7"]
    assert [url.params.get("cursor") for url in report_reads] == [None, "next"]


@pytest.mark.parametrize(
    "page",
    [
        {"items": [], "count": 1, "has_more": False, "next_cursor": None},
        {"items": [], "count": 0, "has_more": False, "next_cursor": "unexpected"},
        {"items": [], "count": 0, "has_more": True, "next_cursor": "next"},
        {
            "items": [published_report()],
            "count": 1,
            "has_more": True,
            "next_cursor": None,
        },
    ],
)
async def test_backend_rejects_malformed_report_pagination(page):
    def handle(request):
        route = request.url.path.rsplit("/", 1)[-1]
        body = feed_body(route, [], [])
        if route == "reports":
            body.update(page)
        return httpx.Response(200, json=body)

    backend = PublicBackend("https://backend.invalid", transport=httpx.MockTransport(handle))

    with pytest.raises(BackendUnavailable, match="page count|pagination|terminal"):
        await backend.fetch()


async def test_backend_rejects_a_repeated_report_cursor():
    def handle(request):
        route = request.url.path.rsplit("/", 1)[-1]
        if route != "reports":
            return httpx.Response(200, json=feed_body(route, [], []))
        report_id = "r2" if request.url.params.get("cursor") else "r1"
        return httpx.Response(
            200,
            json={
                "api_version": 1,
                "revision": "1",
                "items": [published_report(report_id)],
                "count": 1,
                "has_more": True,
                "next_cursor": "repeated",
            },
        )

    backend = PublicBackend("https://backend.invalid", transport=httpx.MockTransport(handle))

    with pytest.raises(BackendUnavailable, match="pagination"):
        await backend.fetch()


async def test_backend_reconstructs_truncated_leaderboard_from_complete_reports():
    hotkeys = [hashlib.sha256(str(index).encode()).hexdigest() for index in range(1001)]
    reports = [published_report(f"r{index}", hotkey=hotkey) for index, hotkey in enumerate(hotkeys)]
    ordered = sorted(hotkeys)

    def handle(request):
        route = request.url.path.rsplit("/", 1)[-1]
        body = feed_body(route, [], reports)
        if route == "leaderboard":
            body["items"] = [{"hotkey": hotkey, "valid": 1} for hotkey in ordered[:1000]]
            body["has_more"] = True
        elif route == "reports":
            cursor = request.url.params.get("cursor")
            page = int(cursor.removeprefix("page-")) if cursor else 0
            items = reports[page * 100 : (page + 1) * 100]
            has_more = (page + 1) * 100 < len(reports)
            body.update(
                items=items,
                count=len(items),
                has_more=has_more,
                next_cursor=f"page-{page + 1}" if has_more else None,
            )
        return httpx.Response(200, json=body)

    backend = PublicBackend("https://backend.invalid", transport=httpx.MockTransport(handle))

    snapshot = await backend.fetch()

    assert len(snapshot.leaderboard) == 1001
    assert {decode_hotkey(row.hotkey).hex() for row in snapshot.leaderboard} == set(hotkeys)
    assert all(row.valid_count == 1 for row in snapshot.leaderboard)


async def test_backend_rejects_an_inconsistent_truncated_leaderboard_prefix():
    reports = [published_report("r1"), published_report("r2", hotkey=CURRENT_HOTKEY)]

    def handle(request):
        route = request.url.path.rsplit("/", 1)[-1]
        body = feed_body(route, [], reports)
        if route == "leaderboard":
            body["items"] = [{"hotkey": HOTKEY, "valid": 2}]
            body["has_more"] = True
        return httpx.Response(200, json=body)

    backend = PublicBackend("https://backend.invalid", transport=httpx.MockTransport(handle))

    with pytest.raises(BackendUnavailable, match="truncated leaderboard"):
        await backend.fetch()


async def test_backend_enforces_per_response_and_complete_report_size_limits():
    backend = backend_for([], [])
    backend._MAX_RESPONSE_BYTES = 32
    with pytest.raises(BackendUnavailable, match="response too large"):
        await backend.fetch()

    backend = backend_for([], [])
    backend._MAX_REPORT_BYTES = 1
    with pytest.raises(BackendUnavailable, match="snapshot is too large"):
        await backend.fetch()


@pytest.mark.parametrize(
    "change,reason",
    [
        ({"adjudication_available": False}, "adjudication"),
        ({"unpriced_valid": 1}, "unpriced"),
    ],
)
async def test_backend_status_must_be_ready_and_fully_priced(change, reason):
    requests = 0

    def handle(request):
        nonlocal requests
        requests += 1
        route = request.url.path.rsplit("/", 1)[-1]
        if route == "status":
            body = {
                "api_version": 1,
                "revision": "0",
                "adjudication_available": True,
                "published": 0,
                "valid": 0,
                "duplicate": 0,
                "already_fixed_not_prod": 0,
                "invalid_malicious": 0,
                "hotkeys": 0,
                "awaiting_adjudication": 0,
                "unpriced_valid": 0,
                **change,
            }
            return httpx.Response(200, json=body)
        return httpx.Response(
            200,
            json={
                "api_version": 1,
                "revision": "0",
                "items": [],
                "has_more": False,
                **({"count": 0, "next_cursor": None} if route == "reports" else {}),
            },
        )

    backend = PublicBackend("https://backend.invalid", transport=httpx.MockTransport(handle))

    with pytest.raises(BackendUnavailable, match=reason):
        await backend.fetch()

    assert requests == 1, "a stable gate is not retried"


async def test_backend_refuses_an_empty_publication_with_a_waiting_backlog():
    def handle(request):
        route = request.url.path.rsplit("/", 1)[-1]
        body = feed_body(route, [], [])
        if route == "status":
            body["awaiting_adjudication"] = 1
        return httpx.Response(200, json=body)

    backend = PublicBackend("https://backend.invalid", transport=httpx.MockTransport(handle))

    with pytest.raises(BackendUnavailable, match="backlog"):
        await backend.fetch()


async def test_backend_snapshot_has_one_global_deadline():
    entered = asyncio.Event()

    async def stalled(request):
        entered.set()
        await asyncio.Event().wait()

    backend = PublicBackend("https://backend.invalid", transport=httpx.MockTransport(stalled))
    backend._SNAPSHOT_TIMEOUT_SECONDS = 0.01

    with pytest.raises(BackendUnavailable, match="deadline"):
        await backend.fetch()
    assert entered.is_set()


async def test_public_probe_reuses_a_short_cache_but_intake_fetch_does_not():
    requests = 0
    available = True

    def handle(request):
        nonlocal requests
        requests += 1
        if not available:
            return httpx.Response(503)
        route = request.url.path.rsplit("/", 1)[-1]
        return httpx.Response(200, json=feed_body(route, [], []))

    backend = PublicBackend("https://backend.invalid", transport=httpx.MockTransport(handle))

    await backend.probe()
    await backend.probe()
    assert requests == 3

    available = False
    with pytest.raises(BackendUnavailable, match="HTTP 503"):
        await backend.fetch()


async def test_public_probe_refuses_parallel_refresh_without_duplicate_upstream_reads():
    entered = asyncio.Event()
    release = asyncio.Event()
    requests = 0

    async def handle(request):
        nonlocal requests
        requests += 1
        route = request.url.path.rsplit("/", 1)[-1]
        if route == "status":
            entered.set()
            await release.wait()
        return httpx.Response(200, json=feed_body(route, [], []))

    backend = PublicBackend("https://backend.invalid", transport=httpx.MockTransport(handle))
    first = asyncio.create_task(backend.probe())
    await entered.wait()

    with pytest.raises(BackendUnavailable, match="refresh already in progress"):
        await backend.probe()

    assert requests == 1
    release.set()
    await first


async def test_public_probe_caches_failures_briefly():
    requests = 0

    def unavailable(request):
        nonlocal requests
        requests += 1
        return httpx.Response(503)

    backend = PublicBackend("https://backend.invalid", transport=httpx.MockTransport(unavailable))

    for _ in range(2):
        with pytest.raises(BackendUnavailable, match="HTTP 503"):
            await backend.probe()

    assert requests == backend._SNAPSHOT_ATTEMPTS, "a failure is cached after bounded retries"


async def test_unpriced_valid_is_a_gate_even_after_three_priced_reports():
    reports = [published_report(f"r{index}") for index in range(3)]
    reports.append(published_report("unpriced", severity=None))
    backend = backend_for([{"hotkey": HOTKEY, "valid_count": 4}], reports)

    with pytest.raises(BackendUnavailable, match="severity"):
        await backend.fetch()


async def test_nonvalid_public_report_cannot_publish_a_severity():
    backend = backend_for(
        [],
        [published_report(status="duplicate", severity="critical")],
    )

    with pytest.raises(BackendUnavailable, match="severity"):
        await backend.fetch()


@pytest.mark.parametrize(
    "report",
    [
        published_report(status="duplicate", severity=None),
        published_report(status="duplicate", severity=None, related_report_id="missing"),
        published_report(status="duplicate", severity=None, related_report_id="r1"),
        published_report(related_report_id="other"),
    ],
)
async def test_backend_rejects_invalid_duplicate_references(report):
    backend = backend_for([], [report])

    with pytest.raises(BackendUnavailable, match="duplicate reference"):
        await backend.fetch()


async def test_backend_rejects_a_duplicate_cycle_without_a_root_report():
    reports = [
        published_report("r1", status="duplicate", severity=None, related_report_id="r2"),
        published_report("r2", status="duplicate", severity=None, related_report_id="r1"),
    ]
    backend = backend_for([], reports)

    with pytest.raises(BackendUnavailable, match="non-duplicate root"):
        await backend.fetch()


async def test_backend_accepts_a_duplicate_chain_with_a_root_report():
    reports = [
        published_report("r1", status="duplicate", severity=None, related_report_id="r2"),
        published_report("r2", status="duplicate", severity=None, related_report_id="r3"),
        published_report("r3"),
    ]
    backend = backend_for([{"hotkey": HOTKEY, "valid": 1}], reports)

    snapshot = await backend.fetch()

    assert [report.id for report in snapshot.reports] == ["r1", "r2", "r3"]


async def test_malicious_published_row_without_severity_burns_the_hotkey():
    backend = backend_for([], [published_report(status="invalid_malicious", severity=None)])

    scores = (await backend.fetch()).score([HOTKEY])

    assert scores[HOTKEY].value == 0
    assert scores[HOTKEY].reason == "InvalidResponse"


async def test_leaderboard_weight_cannot_invent_evidence():
    reports = [published_report(f"r{index}", justification="") for index in range(3)]
    backend = backend_for([{"hotkey": HOTKEY, "valid_count": 3, "weight": 1_000_000}], reports)

    with pytest.raises(BackendUnavailable, match="evidence"):
        await backend.fetch()


@pytest.mark.parametrize("field", ["problem_found", "justification", "adjudicator"])
async def test_blank_public_evidence_makes_the_feed_unavailable(field):
    backend = backend_for(
        [{"hotkey": HOTKEY, "valid": 1}],
        [published_report(**{field: ""})],
    )

    with pytest.raises(BackendUnavailable, match="evidence"):
        await backend.fetch()


async def test_historical_hotkey_cannot_block_the_current_champion():
    reports = [
        *[published_report(f"old-{index}", hotkey=HISTORICAL_HOTKEY) for index in range(4)],
        *[published_report(f"current-{index}", hotkey=CURRENT_HOTKEY) for index in range(3)],
    ]
    backend = backend_for(
        [
            {"hotkey": HISTORICAL_HOTKEY, "valid": 4},
            {"hotkey": CURRENT_HOTKEY, "valid": 3},
        ],
        reports,
    )

    scores = (await backend.fetch()).score([CURRENT_HOTKEY])

    assert scores[CURRENT_HOTKEY].value == 1_000_000


@pytest.mark.parametrize("body", [b"not json", b'{"items":{}}', b'{"items":[{}]}'])
async def test_unparseable_feed_is_a_scoring_outage(body):
    backend = PublicBackend(
        "https://backend.invalid",
        transport=httpx.MockTransport(lambda request: httpx.Response(200, content=body)),
    )

    with pytest.raises(BackendUnavailable):
        await backend.fetch()
