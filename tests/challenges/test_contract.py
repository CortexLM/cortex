"""Registry validation, exact leaf conversion and public proxy refusals."""

from fractions import Fraction

import httpx
import pytest
from fastapi import FastAPI
from fastapi.testclient import TestClient

from cortex.challenges.client import ChallengeWeights, leaf_scores, parse_weights
from cortex.challenges.proxy import create_router
from cortex.challenges.registry import parse_registry
from cortex.protocol.models import FULL_SHARE_SCORE, NoScore, NoScoreReason, Score

ROW = {
    "id": "opentype",
    "image": "ghcr.io/opentypeai/challenge",
    "source": "https://github.com/OpentypeAI/challenge",
}


@pytest.mark.parametrize(
    "change",
    [
        {"id": "proof"},
        {"id": "Bad"},
        {"image": "docker.io/opentypeai/challenge"},
        {"image": "ghcr.io/opentypeai/challenge:latest"},
        {"source": "http://github.com/OpentypeAI/challenge"},
        {"channel": "latest"},
        {"pin": "sha256:abc"},
        {"memory_mib": 1},
        {"env": {"CHALLENGE_SLUG": "bounty"}},
        {"unknown": 1},
    ],
)
def test_registry_refuses_unsafe_rows(change):
    with pytest.raises((ValueError, TypeError)):
        parse_registry({"version": 1, "challenge": [{**ROW, **change}]})


def test_registry_refuses_duplicate_ids():
    with pytest.raises(ValueError, match="duplicate"):
        parse_registry({"version": 1, "challenge": [ROW, ROW]})


def test_algorithm_three_scores_are_exact_and_ignore_hotkeys_outside_the_expected_set():
    a, b, outsider, idle = (bytes([n]) * 32 for n in (1, 2, 3, 4))
    answer = ChallengeWeights(
        {a: Fraction(3), b: Fraction(1), outsider: Fraction(1000)}, Fraction(10)
    )

    scores = leaf_scores(answer, {a, b, idle}, algorithm_version=3)

    assert scores == {
        a: Score(3 * FULL_SHARE_SCORE // 10),
        b: Score(FULL_SHARE_SCORE // 10),
        idle: NoScore(NoScoreReason.NOT_ATTEMPTED),
    }
    normalized = leaf_scores(
        ChallengeWeights({a: Fraction(1), b: Fraction(2)}), {a, b}, algorithm_version=3
    )
    assert sum(score.value for score in normalized.values()) == FULL_SHARE_SCORE - 1


@pytest.mark.parametrize("weight", [Fraction(1, 2), Fraction(2**64)])
def test_algorithm_two_refuses_the_whole_answer_on_one_non_u64_weight(weight):
    good, bad = b"\1" * 32, b"\2" * 32
    answer = ChallengeWeights({good: Fraction(3), bad: weight})
    with pytest.raises(ValueError, match="u64"):
        leaf_scores(answer, {good, bad}, algorithm_version=2)


def test_algorithm_one_never_signs_a_container_weight():
    hotkey = b"\1" * 32
    with pytest.raises(ValueError, match="algorithm 1"):
        leaf_scores(ChallengeWeights({hotkey: Fraction(3)}), {hotkey}, algorithm_version=1)


@pytest.mark.parametrize(
    "body",
    [
        b'{"challenge_slug":"bounty","epoch":7,"weights":{}}',
        b'{"challenge_slug":"opentype","epoch":8,"weights":{}}',
        b'{"challenge_slug":"opentype","epoch":7,"weights":{"' + b"a" * 64 + b'":-1}}',
        b'{"challenge_slug":"opentype","epoch":7,"weights":{"' + b"a" * 64 + b'":NaN}}',
        b'{"challenge_slug":"opentype","epoch":7,"weights":{"not-a-key":1}}',
    ],
)
def test_weights_for_another_challenge_epoch_or_with_invalid_values_are_rejected(body):
    with pytest.raises(ValueError):
        parse_weights(body, slug="opentype", epoch=7)


def test_proxy_forwards_public_routes_only():
    seen = []

    def upstream(request: httpx.Request) -> httpx.Response:
        seen.append(request)
        return httpx.Response(200, json={"ok": True})

    entry = parse_registry({"version": 1, "challenge": [ROW]})
    app = FastAPI()
    http = httpx.AsyncClient(transport=httpx.MockTransport(upstream))
    app.include_router(create_router(lambda: entry, http))
    client = TestClient(app)

    response = client.get(
        "/challenge/opentype/v1/status?x=1",
        headers={"authorization": "Bearer miner", "cookie": "secret", "x-forwarded-for": "1.2.3.4"},
    )
    assert response.status_code == 200
    forwarded = seen[-1]
    assert str(forwarded.url) == "http://cortex-challenge-opentype:8000/v1/status?x=1"
    assert forwarded.headers["authorization"] == "Bearer miner"
    assert "cookie" not in forwarded.headers
    assert forwarded.headers["x-forwarded-for"] == "testclient"

    count = len(seen)
    for path in (
        "/challenge/opentype/internal/v1/get_weights?epoch=1",
        "/challenge/opentype/v1/%2e%2e/internal/v1/get_weights",
        "/challenge/opentype/v1//status",
        "/challenge/unknown/v1/status",
    ):
        assert client.get(path).status_code == 404
    assert (
        client.post("/challenge/opentype/v1/x", content=b"x" * (1024 * 1024 + 1)).status_code == 413
    )
    assert len(seen) == count
