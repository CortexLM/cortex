"""A relay releases custody on the master's public signed settlement view, with no credential."""

from __future__ import annotations

from collections.abc import Callable
from pathlib import Path

import anyio
import httpx
import pytest
from test_beacon_cache import hold_a_receipted_upload, serving
from test_transport_v2 import MASTER, NEXT, RUN, H, Rig

from hypertrain.beacon.drand import QUICKNET_GENESIS, QUICKNET_PERIOD
from hypertrain.protocol import relay_envelope
from hypertrain.protocol.relay_envelope import RelayEnvelope
from hypertrain.protocol.relay_messages import (
    CustodyRelease,
    RelaySettlementView,
    UploadGrant,
)
from hypertrain.relay import app as relay_app
from hypertrain.relay.client import fetch_settlement_view
from hypertrain.relay.core import RelayError

# The facts the master reports for the settled round; closed_beacon is the latest of the three
# so that a release one beacon earlier than closed_beacon + 100 breaks the epoch grace.
FINALITY, VESTING, CLOSED = 30, 40, 45
GRACE = 100
# A relay at this beacon is past the grace; the pinned retention horizon of the Rig is 200.
SERVED_AT = QUICKNET_GENESIS + QUICKNET_PERIOD * (CLOSED + GRACE + 5 - 1) + 1


def sealed_view(view: RelaySettlementView, signer=MASTER, exp: int = 10_000) -> RelayEnvelope:
    return relay_envelope.parse_envelope(
        relay_envelope.seal(signer, "RelaySettlementView", RUN, view, exp)
    )


def view_of(grant_hash: str, **override: object) -> RelaySettlementView:
    facts: dict[str, object] = dict(
        grant_hash=grant_hash,
        finality_hash=H,
        finality_beacon=FINALITY,
        closed_dispute_root=H,
        closed_beacon=CLOSED,
        vesting_beacon=VESTING,
    )
    return RelaySettlementView.model_validate({**facts, **override})


class Master:
    """The master's public route: records every request, answers with a prepared reply."""

    def __init__(self, reply: Callable[[str], httpx.Response]) -> None:
        self.reply, self.seen = reply, []

    def __call__(self, request: httpx.Request) -> httpx.Response:
        self.seen.append((request.method, request.url.path, dict(request.headers)))
        return self.reply(request.url.path.rsplit("/", 1)[-1])


def release_of(grant: UploadGrant, receipt: str, beacon: int) -> bytes:
    release = CustodyRelease(
        grant_hash=grant.digest(),
        custody_hashes=[receipt],
        retention_hash=receipt,
        finality_hash=H,
        vesting_beacon=VESTING,
        closed_dispute_root=H,
        release_beacon=beacon,
    )
    return (
        relay_envelope.parse_envelope(
            relay_envelope.seal(MASTER, "CustodyRelease", RUN, release, 10_000)
        )
        .model_dump_json()
        .encode()
    )


async def released(tmp_path: Path, grant: UploadGrant, receipt: str) -> int | None:
    async with httpx.AsyncClient() as client:
        state = await Rig(tmp_path / "store", client).relay.state()
        return state.uploads[grant.digest()].pins[receipt].delete_after


def test_release_over_http_uses_the_master_settlement_view_and_no_credential(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    async def scenario() -> None:
        grant, receipt = await hold_a_receipted_upload(tmp_path)
        master = Master(
            lambda _: httpx.Response(200, json=sealed_view(view_of(grant.digest())).model_dump())
        )
        monkeypatch.setattr(
            relay_app,
            "http_client",
            lambda: httpx.AsyncClient(transport=httpx.MockTransport(master)),
        )
        async with serving(tmp_path, monkeypatch, SERVED_AT) as (client, _, _):
            answer = await client.post(
                f"/v1/uploads/{grant.digest()}/release",
                content=release_of(grant, receipt, CLOSED + GRACE),
            )
        assert answer.status_code == 200, answer.text
        assert answer.json()["type"] == "CustodyRelease"
        assert await released(tmp_path, grant, receipt) == grant.retain_until
        # Startup recovery also posts the pending receipt; only the GET is the settlement.
        views = [(m, path) for m, path, _ in master.seen if m == "GET"]
        assert views == [("GET", f"/v2/runs/{RUN}/relay-settlements/{grant.digest()}")]
        assert all(not {"authorization", "cookie"} & set(h) for _, _, h in master.seen)

    anyio.run(scenario)


@pytest.mark.parametrize(
    ("fault", "status", "answer"),
    [
        ("wrong-signer", 400, {"error": "SignatureError"}),
        ("expired", 400, {"error": "SignatureError"}),
        (
            "other-grant",
            400,
            {"error": "RelayError", "detail": "settlement view names another grant"},
        ),
        *[
            (
                fact,
                400,
                {
                    "error": "RelayError",
                    "detail": "release lacks matching authenticated finality/closure evidence",
                },
            )
            for fact in ("other-finality", "other-closure", "other-vesting")
        ],
        ("not-settled", 503, {"error": "HTTPStatusError"}),
    ],
)
def test_release_over_http_refused_when_the_view_is_not_the_masters_for_this_grant(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, fault: str, status: int, answer: dict
) -> None:
    async def scenario() -> None:
        grant, receipt = await hold_a_receipted_upload(tmp_path)

        def reply(grant_hash: str) -> httpx.Response:
            view = view_of(grant_hash)
            if fault == "wrong-signer":
                return httpx.Response(200, json=sealed_view(view, signer=NEXT).model_dump())
            if fault == "expired":
                return httpx.Response(200, json=sealed_view(view, exp=CLOSED).model_dump())
            if fault == "other-grant":
                return httpx.Response(200, json=sealed_view(view_of(H)).model_dump())
            if fault.startswith("other-") and fault != "other-grant":
                other = {
                    "other-finality": dict(finality_hash="34" * 32),
                    "other-closure": dict(closed_dispute_root="56" * 32),
                    "other-vesting": dict(vesting_beacon=VESTING + 1),
                }[fault]
                return httpx.Response(
                    200, json=sealed_view(view_of(grant_hash, **other)).model_dump()
                )
            return httpx.Response(409, json={"detail": "relay custody remains pinned"})

        monkeypatch.setattr(
            relay_app,
            "http_client",
            lambda: httpx.AsyncClient(transport=httpx.MockTransport(Master(reply))),
        )
        async with serving(tmp_path, monkeypatch, SERVED_AT) as (client, _, _):
            refused = await client.post(
                f"/v1/uploads/{grant.digest()}/release",
                content=release_of(grant, receipt, CLOSED + GRACE),
            )
        assert (refused.status_code, refused.json()) == (status, answer)
        assert await released(tmp_path, grant, receipt) is None

    anyio.run(scenario)


def test_release_over_http_refused_before_every_settled_fact_has_aged_an_epoch(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # The grace runs from the latest of finality, vesting and the closed disputes: one beacon
    # short of closed_beacon + 100 is refused, so the view's closed_beacon is what decides.
    async def scenario() -> None:
        grant, receipt = await hold_a_receipted_upload(tmp_path)
        view = sealed_view(view_of(grant.digest())).model_dump()
        master = Master(lambda _: httpx.Response(200, json=view))
        monkeypatch.setattr(
            relay_app,
            "http_client",
            lambda: httpx.AsyncClient(transport=httpx.MockTransport(master)),
        )
        async with serving(tmp_path, monkeypatch, SERVED_AT) as (client, _, _):
            with pytest.raises(ValueError, match="epoch grace"):
                await client.post(
                    f"/v1/uploads/{grant.digest()}/release",
                    content=release_of(grant, receipt, CLOSED + GRACE - 1),
                )
        assert await released(tmp_path, grant, receipt) is None

    anyio.run(scenario)


@pytest.mark.parametrize(
    "master_url",
    ["http://master.test", "https://user:secret@master.test", "https:///no-host", "master.test"],
)
def test_fetch_settlement_view_refuses_a_master_endpoint_that_is_not_credential_free_https(
    master_url: str,
) -> None:
    async def scenario() -> None:
        sent = []
        transport = httpx.MockTransport(lambda request: sent.append(request) or httpx.Response(500))
        async with httpx.AsyncClient(transport=transport) as client:
            with pytest.raises(RelayError, match="credential-free HTTPS"):
                await fetch_settlement_view(client, master_url, RUN, H)
        assert sent == []

    anyio.run(scenario)


def test_fetch_settlement_view_sends_a_bare_get_and_returns_the_signed_envelope() -> None:
    async def scenario() -> None:
        signed = sealed_view(view_of(H))
        master = Master(lambda _: httpx.Response(200, json=signed.model_dump()))
        async with httpx.AsyncClient(transport=httpx.MockTransport(master)) as client:
            got = await fetch_settlement_view(client, "https://master.test/prefix/", RUN, H)
        assert got == signed
        [(method, path, headers)] = master.seen
        assert (method, path) == ("GET", f"/prefix/v2/runs/{RUN}/relay-settlements/{H}")
        assert not {"authorization", "cookie"} & set(headers)

    anyio.run(scenario)
