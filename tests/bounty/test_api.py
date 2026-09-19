"""Real sr25519 signatures, SQLite and ASGI; only upstream I/O and clock vary."""

from dataclasses import dataclass

import httpx
import pytest
import sr25519
from fastapi import FastAPI

from cortex.bounty import BountyService, BountyStore, PublicBackend, create_router


@dataclass
class Clock:
    now: int = 1_800_000_000

    def __call__(self):
        return self.now


def signed_pair(account="account-a", nonce="12" * 16, *, seed_byte=7):
    public, secret = sr25519.pair_from_seed(bytes([seed_byte]) * 32)
    expiry = 1_800_000_900
    challenge = f"cortex-bounty-v1|{account}|{nonce}|{expiry}".encode()
    return {
        "account_id": account,
        "hotkey": public.hex(),
        "nonce": nonce,
        "exp": expiry,
        "signature": sr25519.sign((public, secret), challenge).hex(),
        "terms_accepted": True,
    }


def report_body(session, number=0):
    return {
        "session": session,
        "title": f"Gateway accepts unauthorized request {number}",
        "body": f"Request {number} to the operator endpoint succeeds without credentials. "
        "An anonymous caller can change the backend configuration "
        "and invalidate the current bundle.",
        "repro_steps": "Call the operator endpoint without an Authorization header.",
    }


@pytest.fixture
def service(tmp_path):
    clock = Clock()
    upstream = {"status": 200, "leaderboard": [], "reports": []}

    def transport(request):
        return httpx.Response(
            upstream["status"], json={"items": upstream[request.url.path.rsplit("/", 1)[-1]]}
        )

    backend = PublicBackend("https://backend.invalid", transport=httpx.MockTransport(transport))
    store = BountyStore(tmp_path / "bounty.sqlite3")
    svc = BountyService(
        store,
        backend,
        session_secret=b"session-secret-for-tests" * 2,
        admin_tokens=["operator-token"],
        clock=clock,
    )
    yield svc, clock, upstream
    store.close()


@pytest.fixture
async def client(service):
    app = FastAPI()
    app.include_router(create_router(service[0]))
    async with httpx.AsyncClient(
        transport=httpx.ASGITransport(app=app), base_url="http://test"
    ) as client:
        yield client


async def pair(client):
    payload = signed_pair()
    await grant_pair(client, payload)
    response = await client.post("/v1/pair", json=payload)
    assert response.status_code == 201, response.text
    return response.json()["session"]


async def grant_pair(client, payload):
    response = await client.post(
        "/v1/admin/pair-grants",
        headers={"Authorization": "Bearer operator-token"},
        json={
            "account_id": payload["account_id"],
            "hotkey": payload["hotkey"],
            "expires_at": 1_800_000_300,
        },
    )
    assert response.status_code == 201, response.text
    return response


async def test_adjudication_is_durable_but_only_published_backend_rows_are_paid(service, client):
    svc, clock, upstream = service
    session = await pair(client)
    reports = []
    for number in range(3):
        response = await client.post("/v1/reports", json=report_body(session, number))
        assert response.status_code == 201, response.text
        report_id = response.json()["id"]
        verdict = await client.post(
            "/v1/admin/adjudicate",
            headers={"Authorization": "Bearer operator-token"},
            json={"report_id": report_id, "verdict": "valid", "severity": "major"},
        )
        assert verdict.status_code == 200, verdict.text
        reports.append(verdict.json())
        clock.now += 60

    assert svc.store.get_report(reports[0]["id"])["severity"] == "major"
    hotkey = signed_pair()["hotkey"]
    assert (await svc.score([hotkey]))[hotkey].value == 0
    upstream["leaderboard"] = [{"hotkey": hotkey, "valid_count": 3}]
    upstream["reports"] = [
        {
            "id": row["id"],
            "hotkey": hotkey,
            "status": "valid",
            "severity": "major",
            "problem_found": row["title"],
            "justification": "Reproduced with an unauthorized request",
            "adjudicator": "operator",
            "adjudicated_at": "2026-09-16T00:00:00Z",
            "created_at": "2026-09-16T00:00:00Z",
        }
        for row in reports
    ]

    scores = await svc.score([hotkey, "ab" * 32])

    assert scores[hotkey].value == 500_000
    assert scores["ab" * 32].reason == "NotAttempted"


@pytest.mark.parametrize("configured,status", [(False, 200), (True, 503)])
async def test_unavailable_scoring_refuses_report_before_any_row(
    service, client, configured, status
):
    svc, _, upstream = service
    session = await pair(client)
    upstream["status"] = status
    if not configured:
        svc.backend = PublicBackend(None)

    response = await client.post("/v1/reports", json=report_body(session))

    assert response.status_code == 503
    assert svc.store.list_reports() == []


@pytest.mark.parametrize("mutation,status", [("signature", 401), ("terms", 403), ("expiry", 400)])
async def test_pairing_rejects_forgery_missing_terms_and_expired_challenge(
    service, client, mutation, status
):
    payload = signed_pair()
    await grant_pair(client, payload)
    if mutation == "signature":
        payload["signature"] = "00" * 64
    elif mutation == "terms":
        payload["terms_accepted"] = False
    else:
        payload["exp"] = 1

    response = await client.post("/v1/pair", json=payload)

    assert response.status_code == status


async def test_first_pairing_requires_an_operator_grant_without_burning_the_nonce(client):
    payload = signed_pair()

    refused = await client.post("/v1/pair", json=payload)
    await grant_pair(client, payload)
    accepted = await client.post("/v1/pair", json=payload)

    assert refused.status_code == 403
    assert refused.json() == {"error": "pairing not authorized by account operator"}
    assert accepted.status_code == 201


async def test_pair_grant_requires_operator_authentication(client):
    payload = signed_pair()

    response = await client.post(
        "/v1/admin/pair-grants",
        headers={"Authorization": "Bearer wrong"},
        json={
            "account_id": payload["account_id"],
            "hotkey": payload["hotkey"],
            "expires_at": 1_800_000_300,
        },
    )

    assert response.status_code == 401


async def test_pair_grant_authentication_precedes_body_parsing(client):
    response = await client.post(
        "/v1/admin/pair-grants",
        headers={"Authorization": "Bearer wrong", "Content-Type": "application/json"},
        content=b"{not-json",
    )

    assert response.status_code == 401
    assert response.json() == {"error": "unauthorized"}


async def test_pair_grant_body_has_a_hard_size_limit(client):
    response = await client.post(
        "/v1/admin/pair-grants",
        headers={"Authorization": "Bearer operator-token", "Content-Type": "application/json"},
        content=b" " * 4097,
    )

    assert response.status_code == 413
    assert response.json() == {"error": "pair grant request too large"}


async def test_expired_pair_grant_does_not_authorize_or_burn_nonce(service, client):
    _, clock, _ = service
    payload = signed_pair()
    grant = await client.post(
        "/v1/admin/pair-grants",
        headers={"Authorization": "Bearer operator-token"},
        json={
            "account_id": payload["account_id"],
            "hotkey": payload["hotkey"],
            "expires_at": clock.now + 1,
        },
    )
    assert grant.status_code == 201
    clock.now += 1

    refused = await client.post("/v1/pair", json=payload)
    renewed = await client.post(
        "/v1/admin/pair-grants",
        headers={"Authorization": "Bearer operator-token"},
        json={
            "account_id": payload["account_id"],
            "hotkey": payload["hotkey"],
            "expires_at": clock.now + 300,
        },
    )
    accepted = await client.post("/v1/pair", json=payload)

    assert refused.status_code == 403
    assert renewed.status_code == 201
    assert accepted.status_code == 201


async def test_successful_pairing_consumes_its_operator_grant(client):
    payload = signed_pair()
    await grant_pair(client, payload)
    assert (await client.post("/v1/pair", json=payload)).status_code == 201
    payload = signed_pair(nonce="34" * 16)

    response = await client.post("/v1/pair", json=payload)

    assert response.status_code == 403
    assert response.json() == {"error": "pairing not authorized by account operator"}


async def test_pair_retry_refuses_used_nonce_without_returning_session_data(client):
    payload = signed_pair()
    await grant_pair(client, payload)

    first = await client.post("/v1/pair", json=payload)
    retry = await client.post("/v1/pair", json=payload)

    assert first.status_code == 201
    assert retry.status_code == 409
    assert retry.json() == {"error": "nonce reused"}


@pytest.mark.parametrize(
    "account,seed_byte", [("account-b", 7), ("account-b", 8), ("account-a", 8)]
)
async def test_used_nonce_refuses_different_identity_without_consuming_its_grant(
    client, account, seed_byte
):
    original_session = await pair(client)
    payload = signed_pair(account, seed_byte=seed_byte)
    await grant_pair(client, payload)

    reused = await client.post("/v1/pair", json=payload)
    fresh = signed_pair(account, nonce="34" * 16, seed_byte=seed_byte)
    retry = await client.post("/v1/pair", json=fresh)

    assert reused.status_code == 409
    assert reused.json() == {"error": "nonce reused"}
    if account == "account-a":
        assert retry.status_code == 409
        assert retry.json() == {"error": "account already paired to another hotkey"}
        # A conflicting account must also roll back grant deletion and nonce reservation.
        repeated_conflict = await client.post("/v1/pair", json=fresh)
        assert repeated_conflict.status_code == 409
        assert repeated_conflict.json() == retry.json()
    else:
        assert retry.status_code == 201
    retained = await client.post("/v1/reports", json=report_body(original_session))
    assert retained.status_code == 201


async def test_pair_grant_expiry_is_bounded_to_five_minutes(service, client):
    _, clock, _ = service
    payload = signed_pair()

    response = await client.post(
        "/v1/admin/pair-grants",
        headers={"Authorization": "Bearer operator-token"},
        json={
            "account_id": payload["account_id"],
            "hotkey": payload["hotkey"],
            "expires_at": clock.now + 301,
        },
    )

    assert response.status_code == 400
    assert response.json() == {"error": "pair grant must expire within 300 seconds"}


async def test_restart_preserves_sessions_nonces_reports_and_quotas(service, client):
    svc, clock, _ = service
    session = await pair(client)
    created = (await client.post("/v1/reports", json=report_body(session))).json()
    path = svc.store.path
    svc.store.close()
    svc.store = BountyStore(path)

    repeated_pair = await client.post("/v1/pair", json=signed_pair())
    repeated_report = await client.post("/v1/reports", json=report_body(session, 1))

    assert repeated_pair.status_code == 409
    assert repeated_pair.json() == {"error": "nonce reused"}
    assert repeated_report.status_code == 429
    assert svc.store.get_report(created["id"])["state"] == "pending"
    clock.now += 60
    assert (await client.post("/v1/reports", json=report_body(session, 1))).status_code == 201
    svc.store.close()


async def test_repairing_an_account_revokes_its_previous_session(service, client):
    first = await pair(client)
    payload = signed_pair(nonce="34" * 16)
    await grant_pair(client, payload)
    reused = await client.post("/v1/pair", json=signed_pair())
    replacement = await client.post("/v1/pair", json=payload)

    old_report = await client.post("/v1/reports", json=report_body(first))
    new_report = await client.post(
        "/v1/reports", json=report_body(replacement.json()["session"], 1)
    )

    assert reused.status_code == 409
    assert reused.json() == {"error": "nonce reused"}
    assert replacement.status_code == 201
    assert old_report.status_code == 401
    assert new_report.status_code == 201


async def test_different_hotkey_cannot_replace_an_existing_account_pairing(service, client):
    original = await pair(client)

    payload = signed_pair(nonce="34" * 16, seed_byte=8)
    await grant_pair(client, payload)
    replacement = await client.post("/v1/pair", json=payload)
    retained = await client.post("/v1/reports", json=report_body(original))

    assert replacement.status_code == 409
    assert replacement.json() == {"error": "account already paired to another hotkey"}
    assert retained.status_code == 201


async def test_status_probes_the_external_feed_instead_of_claiming_configured_is_ready(
    service, client
):
    _, _, upstream = service
    upstream["status"] = 503

    status = await client.get("/v1/status")

    assert status.status_code == 200
    assert status.json()["backend_public_configured"] is True
    assert status.json()["can_score"] is False
    assert "fetch failed" in status.json()["reason"]
    assert status.json()["pairing"] == {
        "requires_operator_grant": True,
        "grant_max_ttl_secs": 300,
    }


async def test_duplicate_of_closed_report_never_reopens_triage(service, client):
    svc, clock, _ = service
    session = await pair(client)
    original = (await client.post("/v1/reports", json=report_body(session))).json()
    await client.post(
        "/v1/admin/adjudicate",
        headers={"Authorization": "Bearer operator-token"},
        json={"report_id": original["id"], "verdict": "invalid_malicious"},
    )
    clock.now += 60
    payload = report_body(session)
    payload["title"] = "  " + payload["title"].upper() + "  "

    duplicate = await client.post("/v1/reports", json=payload)

    assert duplicate.status_code == 201
    assert duplicate.json()["state"] == "duplicate"
    assert svc.store.get_report(duplicate.json()["id"])["duplicate_of"] == original["id"]


async def test_report_reads_require_operator_and_public_routes_do_not_exist(service, client):
    session = await pair(client)
    report = (await client.post("/v1/reports", json=report_body(session))).json()

    responses = [
        await client.get("/v1/reports"),
        await client.get(f"/v1/reports/{report['id']}"),
        await client.get("/v1/public/reports"),
    ]

    assert [response.status_code for response in responses] == [401, 401, 404]
    assert all("repro_steps" not in response.text for response in responses)


async def test_pending_quota_and_substance_rejections_do_not_create_rows(service, client):
    svc, clock, _ = service
    session = await pair(client)
    thin = report_body(session)
    thin["body"] = "fabricated " * 10
    assert (await client.post("/v1/reports", json=thin)).status_code == 400
    for index in range(5):
        assert (
            await client.post("/v1/reports", json=report_body(session, index))
        ).status_code == 201
        clock.now += 60

    response = await client.post("/v1/reports", json=report_body(session, 6))

    assert response.status_code == 429
    assert len(svc.store.list_reports()) == 5


@pytest.mark.parametrize("mutation,status", [("session", 401), ("hotkey", 403)])
async def test_report_cannot_impersonate_another_hotkey(service, client, mutation, status):
    session = await pair(client)
    body = report_body(session)
    if mutation == "session":
        body["session"] = "00" * 32
    else:
        body["hotkey"] = "ab" * 32

    response = await client.post("/v1/reports", json=body)

    assert response.status_code == status
    assert service[0].store.list_reports() == []


async def test_empty_admin_configuration_never_exposes_private_reports(service):
    svc, _, _ = service
    locked = BountyService(svc.store, svc.backend, session_secret=b"s" * 32)
    app = FastAPI()
    app.include_router(create_router(locked))

    async with httpx.AsyncClient(
        transport=httpx.ASGITransport(app=app), base_url="http://test"
    ) as client:
        response = await client.get("/v1/reports")

    assert response.status_code == 503
    assert response.json() == {"error": "auth_unconfigured"}


async def test_scoring_outage_covers_every_participant_without_payment(service):
    svc, _, upstream = service
    upstream["status"] = 503
    hotkeys = ["ab" * 32, "cd" * 32]

    scores = await svc.score(hotkeys)

    assert set(scores) == set(hotkeys)
    assert all(
        score.value == 0 and score.reason == "ChallengeInternal" for score in scores.values()
    )
