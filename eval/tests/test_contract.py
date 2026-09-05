"""Unit tests for the image contract (no torch required)."""

from __future__ import annotations

import json
from http.server import BaseHTTPRequestHandler, HTTPServer
from threading import Thread

import pytest

from proof_eval.baked import baked_proxies, require_baked
from proof_eval.contract import DEFAULT_PROXY, METRICS_MARKER, OK_MARKER
from proof_eval.fabric import DT_NO_IB_GBPS, enforce
from proof_eval.judge import call_judge, load_judge_api_key, require_judge
from proof_eval.request import Constraints, HarvestRequest, canonical_json, holdout_commitment


def test_markers_match_harvest_pod() -> None:
    assert METRICS_MARKER == "PROOF_METRICS="
    assert OK_MARKER == "PROOF_EVAL_OK"


def test_default_proxy_is_baked() -> None:
    assert DEFAULT_PROXY == "Qwen/Qwen3.8-0.6B"
    assert DEFAULT_PROXY in baked_proxies()
    require_baked(DEFAULT_PROXY)


def test_unknown_proxy_is_refused() -> None:
    with pytest.raises(Exception, match="not baked"):
        require_baked("Qwen/Qwen3.8-27B")


def test_fabric_applies_the_dt_no_ib_cap(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr("proof_eval.fabric._data_ifaces", lambda: [])
    applied = enforce(
        Constraints(
            no_infiniband=True,
            no_nvlink=True,
            no_nccl_fast_fabric=True,
            max_inter_node_gbps=DT_NO_IB_GBPS,
        )
    )
    assert applied["NCCL_IB_DISABLE"] == "1"
    assert applied["NCCL_P2P_DISABLE"] == "1"
    assert applied["NCCL_NET"] == "Socket"
    assert applied["PROOF_MAX_INTER_NODE_GBPS"] == "12.5"


def test_loosening_the_gbps_cap_is_refused() -> None:
    with pytest.raises(Exception, match="loosens"):
        enforce(Constraints(max_inter_node_gbps=25.0))


def test_unknown_constraint_key_is_refused() -> None:
    with pytest.raises(Exception, match="allow_secret_fabric"):
        Constraints.from_dict({"no_infiniband": True, "allow_secret_fabric": True})


def test_canonical_json_sorts_keys() -> None:
    assert canonical_json({"b": 1, "a": {"d": [1, 2], "c": "x"}}) == '{"a":{"c":"x","d":[1,2]},"b":1}'


def _holdout() -> list[dict]:
    return [
        {
            "id": 1,
            "split": "web_ood",
            "dataset_id": "synthetic-dev",
            "content_sha256": "aa" * 32,
            "token_count": 2048,
        }
    ]


def _raw(**over: object) -> dict:
    holdout = over.pop("holdout", _holdout())
    raw = {
        "schema_version": 1,
        "challenge_id": "proof",
        "submission_digest": "d",
        "artifact_digest": "a",
        "topic_id": "dt-no-ib-v0",
        "family": "throughput",
        "inference_offer_id": "master-v0",
        "provider_kind": "openai_compatible",
        "base_url": "http://127.0.0.1:8000/v1",
        "mode": "chat",
        "model_ref": "master-proxy-v0",
        "max_input_tokens": 4096,
        "max_output_tokens": 256,
        "config_commitment": "ab" * 32,
        "eval_image_digest": "sha256:" + "ab" * 32,
        "holdout_commitment": holdout_commitment(holdout),
        "constraints": {},
        "flops_budget": 1,
        "wall_budget_s": 1,
        "claim": "beats the sealed reference under the cap",
        "holdout": holdout,
    }
    raw.update(over)
    return raw


def test_request_refuses_empty_claim() -> None:
    with pytest.raises(Exception, match="claim"):
        HarvestRequest.from_dict(_raw(claim=""))


def test_request_requires_judge_origin_and_ignores_no_hf_default() -> None:
    req = HarvestRequest.from_dict(_raw())
    assert req.base_url == "http://127.0.0.1:8000/v1"
    assert req.model_ref == "master-proxy-v0"
    assert req.proxy_model == ""
    with pytest.raises(Exception, match="http"):
        HarvestRequest.from_dict(_raw(base_url="ftp://evil"))
    with pytest.raises(Exception, match="api_key"):
        HarvestRequest.from_dict(_raw(api_key="sk-should-never-be-here"))


def test_judge_key_comes_from_env_not_request(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.delenv("OPENAI_API_KEY", raising=False)
    monkeypatch.delenv("PROOF_INFERENCE_API_KEY", raising=False)
    with pytest.raises(Exception, match="API key"):
        load_judge_api_key()
    monkeypatch.setenv("OPENAI_API_KEY", "sk-from-teacher-env")
    assert load_judge_api_key() == "sk-from-teacher-env"


class _Judge(BaseHTTPRequestHandler):
    last_auth = ""
    last_path = ""
    last_body = b""

    def do_POST(self) -> None:  # noqa: N802
        length = int(self.headers.get("Content-Length") or 0)
        _Judge.last_auth = self.headers.get("Authorization") or ""
        _Judge.last_path = self.path
        _Judge.last_body = self.rfile.read(length)
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(b'{"id":"x","choices":[{"message":{"content":"ok"}}]}')

    def log_message(self, fmt: str, *args: object) -> None:
        return


def test_judge_sends_bearer_from_teacher_env(monkeypatch: pytest.MonkeyPatch) -> None:
    server = HTTPServer(("127.0.0.1", 0), _Judge)
    port = server.server_address[1]
    thread = Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        raw = _raw(base_url=f"http://127.0.0.1:{port}/v1")
        req = HarvestRequest.from_dict(raw)
        monkeypatch.setenv("PROOF_INFERENCE_API_KEY", "sk-live-not-a-real-secret")
        require_judge(req)
        assert _Judge.last_auth == "Bearer sk-live-not-a-real-secret"
        assert _Judge.last_path.endswith("/chat/completions")
        body = json.loads(_Judge.last_body.decode())
        assert body["model"] == "master-proxy-v0"
        assert "sk-live" not in json.dumps(body)
    finally:
        server.shutdown()


def test_judge_refuses_without_a_key(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.delenv("OPENAI_API_KEY", raising=False)
    monkeypatch.delenv("PROOF_INFERENCE_API_KEY", raising=False)
    req = HarvestRequest.from_dict(_raw())
    with pytest.raises(Exception, match="API key"):
        require_judge(req)
    with pytest.raises(Exception, match="API key"):
        call_judge(req, "   ")
