import hashlib
import json
from pathlib import Path

import httpx
import pytest

from hypertrain.beacon import (
    QUICKNET_CHAIN_HASH,
    QUICKNET_GENESIS,
    QUICKNET_PERIOD,
    QUICKNET_PUBLIC_KEY,
    BeaconUnavailable,
    BeaconVerificationError,
    DrandQuicknet,
    FixtureBeacon,
    parse_round,
    verify_quicknet,
)
from hypertrain.beacon.drand import quicknet_push_beacon

VECTORS = json.loads((Path(__file__).parent / "vectors.json").read_text())["rounds"]
V0 = VECTORS[0]


def flip(hexstr: str, i: int = 20) -> str:
    b = bytearray(bytes.fromhex(hexstr))
    b[i] ^= 0x01
    return b.hex()


@pytest.mark.parametrize("v", VECTORS, ids=lambda v: str(v["round"]))
def test_recorded_vectors_verify(v: dict) -> None:
    br = parse_round(v)
    assert (br.round, br.randomness, br.bls_verified) == (v["round"], v["randomness"], True)


def test_signature_bound_to_round_and_key() -> None:
    assert not verify_quicknet(V0["round"] + 1, V0["signature"])
    other_sig = VECTORS[1]["signature"]
    assert not verify_quicknet(V0["round"], other_sig)
    assert not verify_quicknet(V0["round"], V0["signature"], flip(QUICKNET_PUBLIC_KEY, 50))


def test_flipped_signature_byte_rejected_by_verify_and_get() -> None:
    bad_sig = flip(V0["signature"])
    assert verify_quicknet(V0["round"], bad_sig) is False
    # Recompute randomness so only the BLS check can catch it.
    forged = {
        "round": V0["round"],
        "signature": bad_sig,
        "randomness": hashlib.sha256(bytes.fromhex(bad_sig)).hexdigest(),
    }
    push = quicknet_push_beacon()
    with pytest.raises(BeaconVerificationError, match="BLS"):
        push.ingest(forged)
    with pytest.raises(BeaconUnavailable):
        push.get(V0["round"])
    client = DrandQuicknet(("https://a.test", "https://b.test"), client=_mock({"x": forged}))
    with pytest.raises(BeaconVerificationError, match="BLS"):
        client.get(V0["round"])


@pytest.mark.parametrize(
    "payload",
    [
        {},
        {"round": "1000000", "signature": V0["signature"], "randomness": V0["randomness"]},
        {"round": 0, "signature": V0["signature"], "randomness": V0["randomness"]},
        {"round": True, "signature": V0["signature"], "randomness": V0["randomness"]},
        {"round": V0["round"], "signature": V0["signature"][:-2], "randomness": V0["randomness"]},
        {"round": V0["round"], "signature": "zz" * 48, "randomness": V0["randomness"]},
        {"round": V0["round"], "signature": V0["signature"], "randomness": "00" * 32},
        {"round": V0["round"], "signature": None, "randomness": V0["randomness"]},
    ],
)
def test_malformed_payloads_rejected(payload: dict) -> None:
    with pytest.raises(BeaconVerificationError):
        parse_round(payload)


# verify-5.json repro: real round-1000000 signature + a G1 cofactor-torsion point.
TORSION_FORGED_SIG = (
    "b2fcf4f7d2b5bbabc7bc8bab08d4fadbb341292f830beca0"
    "85bc05a28e8403c46d20c1665628d579c30f6c3a2ce5495f"
)


def test_torsion_malleated_signature_rejected() -> None:
    assert TORSION_FORGED_SIG != V0["signature"]
    assert verify_quicknet(V0["round"], TORSION_FORGED_SIG) is False
    forged = {
        "round": V0["round"],
        "signature": TORSION_FORGED_SIG,
        "randomness": hashlib.sha256(bytes.fromhex(TORSION_FORGED_SIG)).hexdigest(),
    }
    with pytest.raises(BeaconVerificationError, match="BLS"):
        parse_round(forged)
    with pytest.raises(BeaconVerificationError, match="BLS"):
        quicknet_push_beacon().ingest(forged)


def test_identity_and_noncanonical_encodings_rejected() -> None:
    assert verify_quicknet(V0["round"], "c0" + "00" * 47) is False
    from py_ecc.optimized_bls12_381 import field_modulus as q

    s = int(V0["signature"], 16)
    x = s & ((1 << 381) - 1)
    noncanon = ((s >> 381) << 381) | (x + q)
    assert verify_quicknet(V0["round"], noncanon.to_bytes(48, "big").hex()) is False


def test_cli_network_down_reports_censored(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    import hypertrain.beacon.__main__ as cli

    def down(relays: tuple[str, ...]) -> DrandQuicknet:
        return DrandQuicknet(relays, client=_mock({}, status={"a.test": 503, "b.test": 503}))

    monkeypatch.setattr(cli, "DrandQuicknet", down)
    rc = cli.main(["check", "--live", "--relay", "https://a.test", "--relay", "https://b.test"])
    assert rc == 3
    assert json.loads(capsys.readouterr().out)["live"] == "CENSORED(network)"


def test_non_curve_point_returns_false() -> None:
    assert verify_quicknet(V0["round"], "ff" * 48) is False
    assert verify_quicknet(V0["round"], "00" * 48) is False


def test_push_beacon_stores_and_rejects_conflicts() -> None:
    push = quicknet_push_beacon()
    br = push.ingest(V0)
    assert push.get(V0["round"]) == br
    assert push.ingest(dict(V0)) == br
    assert push.round_at_or_after(QUICKNET_GENESIS + 3 * (V0["round"] - 1)) == V0["round"]


def test_round_at_or_after_integer_math() -> None:
    p = quicknet_push_beacon()
    g = QUICKNET_GENESIS
    assert [p.round_at_or_after(t) for t in (g - 5, g, g + 1, g + 3, g + 4)] == [1, 1, 2, 2, 3]
    assert QUICKNET_PERIOD == 3


def test_fixture_beacon_explicit_advance() -> None:
    b = FixtureBeacon(seed=b"s", current=5)
    r5 = b.get(5)
    with pytest.raises(BeaconUnavailable):
        b.get(6)
    assert b.advance() == 6
    assert b.get(6).randomness != r5.randomness
    assert FixtureBeacon(seed=b"s", current=9).get(5) == r5
    assert FixtureBeacon(seed=b"t", current=9).get(5) != r5
    assert r5.bls_verified is False
    with pytest.raises(ValueError):
        b.advance(-1)


def _mock(per_host: dict[str, dict], status: dict[str, int] | None = None) -> httpx.Client:
    def handler(req: httpx.Request) -> httpx.Response:
        host = req.url.host
        assert req.url.path.startswith(f"/{QUICKNET_CHAIN_HASH}/")
        if status and host in status:
            return httpx.Response(status[host])
        body = per_host.get(host, per_host.get("x"))
        return httpx.Response(200, json=body)

    return httpx.Client(transport=httpx.MockTransport(handler))


def test_pull_client_requires_two_relays_agreeing() -> None:
    relays = ("https://a.test", "https://b.test")
    ok = DrandQuicknet(relays, client=_mock({"x": V0}))
    assert ok.get(V0["round"]).bls_verified
    down = DrandQuicknet(relays, client=_mock({"x": V0}, status={"b.test": 503}))
    with pytest.raises(BeaconUnavailable):
        down.get(V0["round"])
    split = DrandQuicknet(relays, client=_mock({"a.test": V0, "b.test": VECTORS[1]}))
    with pytest.raises(BeaconVerificationError):
        split.get(V0["round"])
    with pytest.raises(ValueError):
        DrandQuicknet(relays[:1])


def test_latest_straddling_relays_refetch_lowest_round() -> None:
    def handler(req: httpx.Request) -> httpx.Response:
        if req.url.path.endswith("/public/latest"):
            return httpx.Response(200, json=VECTORS[req.url.host == "b.test"])
        assert req.url.path.endswith(f"/public/{V0['round']}")
        return httpx.Response(200, json=V0)

    c = DrandQuicknet(
        ("https://a.test", "https://b.test"),
        client=httpx.Client(transport=httpx.MockTransport(handler)),
    )
    assert c.latest().round == V0["round"]
