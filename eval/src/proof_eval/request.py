"""HarvestRequest the control plane stages as request.json."""

from __future__ import annotations

import hashlib
import json
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from .contract import ContractError, PROOF_METRICS_SCHEMA

CHALLENGE_ID = "proof"
HOLDOUT_DOMAIN = b"base-proof-holdout-v1"


def _is_http_origin(url: str) -> bool:
    u = url.strip()
    return u.startswith("http://") or u.startswith("https://")


@dataclass(frozen=True)
class Constraints:
    no_infiniband: bool = False
    no_nvlink: bool = False
    no_nccl_fast_fabric: bool = False
    max_inter_node_gbps: float | None = None

    @classmethod
    def from_dict(cls, raw: dict[str, Any] | None) -> Constraints:
        data = raw or {}
        unknown = set(data) - {
            "no_infiniband",
            "no_nvlink",
            "no_nccl_fast_fabric",
            "max_inter_node_gbps",
        }
        if unknown:
            raise ContractError(f"unknown constraint keys: {sorted(unknown)}")
        cap = data.get("max_inter_node_gbps")
        return cls(
            no_infiniband=bool(data.get("no_infiniband", False)),
            no_nvlink=bool(data.get("no_nvlink", False)),
            no_nccl_fast_fabric=bool(data.get("no_nccl_fast_fabric", False)),
            max_inter_node_gbps=float(cap) if cap is not None else None,
        )


@dataclass
class HarvestRequest:
    schema_version: int
    challenge_id: str
    submission_digest: str
    artifact_digest: str
    topic_id: str
    family: str
    inference_offer_id: str
    provider_kind: str
    base_url: str
    mode: str
    model_ref: str
    max_input_tokens: int
    max_output_tokens: int
    config_commitment: str
    proxy_model: str
    eval_image_digest: str
    holdout_commitment: str
    constraints: Constraints
    flops_budget: int
    wall_budget_s: int
    claim: str
    holdout: list[dict[str, Any]]

    @classmethod
    def from_dict(cls, raw: dict[str, Any]) -> HarvestRequest:
        if int(raw.get("schema_version", -1)) != PROOF_METRICS_SCHEMA:
            raise ContractError(
                f"schema_version {raw.get('schema_version')}, image reads {PROOF_METRICS_SCHEMA}"
            )
        challenge = str(raw.get("challenge_id", "")).strip()
        if challenge != CHALLENGE_ID:
            raise ContractError(f"challenge_id {challenge!r} is not {CHALLENGE_ID}")
        submission = str(raw.get("submission_digest", "")).strip()
        artifact = str(raw.get("artifact_digest", "")).strip()
        topic = str(raw.get("topic_id", "")).strip()
        family = str(raw.get("family", "")).strip()
        if "api_key" in raw or "openai_api_key" in raw:
            raise ContractError("api_key must not appear in request.json; use teacher.env")
        offer_id = str(raw.get("inference_offer_id", "")).strip()
        provider_kind = str(raw.get("provider_kind", "")).strip()
        base_url = str(raw.get("base_url", "")).strip()
        mode = str(raw.get("mode", "")).strip() or "chat"
        model_ref = str(raw.get("model_ref", "")).strip()
        commitment = str(raw.get("config_commitment", "")).strip()
        if not offer_id:
            raise ContractError("inference_offer_id is required")
        if not _is_http_origin(base_url):
            raise ContractError("base_url must be an http(s) origin")
        if not model_ref:
            raise ContractError("model_ref is required")
        if len(commitment) != 64 or any(c not in "0123456789abcdefABCDEF" for c in commitment):
            raise ContractError("config_commitment must be 64 hex chars")
        proxy = str(raw.get("proxy_model", "")).strip()
        digest = str(raw.get("eval_image_digest", "")).strip()
        holdout_commit = str(raw.get("holdout_commitment", "")).strip()
        claim = str(raw.get("claim", "")).strip()
        if not submission or not artifact or not topic:
            raise ContractError("submission_digest, artifact_digest, and topic_id are required")
        if not digest.startswith("sha256:") or len(digest) < 71:
            raise ContractError("eval_image_digest is not a sha256 pin")
        if len(holdout_commit) != 64 or any(
            c not in "0123456789abcdefABCDEF" for c in holdout_commit
        ):
            raise ContractError("holdout_commitment must be 64 hex chars")
        if not claim:
            raise ContractError("claim is required (recipe, not weights alone)")
        holdout = raw.get("holdout")
        if not isinstance(holdout, list) or not holdout:
            raise ContractError("holdout is empty")
        req = cls(
            schema_version=PROOF_METRICS_SCHEMA,
            challenge_id=challenge,
            submission_digest=submission,
            artifact_digest=artifact,
            topic_id=topic,
            family=family,
            inference_offer_id=offer_id,
            provider_kind=provider_kind or "openai_compatible",
            base_url=base_url,
            mode=mode,
            model_ref=model_ref,
            max_input_tokens=int(raw.get("max_input_tokens") or 0),
            max_output_tokens=int(raw.get("max_output_tokens") or 0),
            config_commitment=commitment,
            proxy_model=proxy,
            eval_image_digest=digest,
            holdout_commitment=holdout_commit,
            constraints=Constraints.from_dict(raw.get("constraints") or {}),
            flops_budget=int(raw.get("flops_budget") or 0),
            wall_budget_s=int(raw.get("wall_budget_s") or 0),
            claim=claim,
            holdout=holdout,
        )
        got = holdout_commitment(req.holdout)
        if got.lower() != holdout_commit.lower():
            raise ContractError("holdout records do not hash to holdout_commitment")
        return req


def _u64(n: int) -> bytes:
    return int(n).to_bytes(8, "little", signed=False)


def _u32(n: int) -> bytes:
    return int(n).to_bytes(4, "little", signed=False)


def _field(h: "hashlib._Hash", value: str) -> None:
    body = value.encode("utf-8")
    h.update(_u64(len(body)))
    h.update(body)


def holdout_commitment(records: list[dict[str, Any]]) -> str:
    """Mirror `proof_task::holdout_commitment` byte-for-byte."""
    sorted_recs = sorted(records, key=lambda r: int(r.get("id") or 0))
    h = hashlib.sha256()
    h.update(HOLDOUT_DOMAIN)
    h.update(b"\xff")
    h.update(_u64(len(sorted_recs)))
    for rec in sorted_recs:
        split = rec.get("split") or rec.get("task") or ""
        if hasattr(split, "value"):
            split = split.value
        h.update(_u32(int(rec.get("id") or 0)))
        h.update(_u32(int(rec.get("token_count") or 0)))
        _field(h, str(split))
        _field(h, str(rec.get("dataset_id") or ""))
        _field(h, str(rec.get("content_sha256") or "").lower())
    return h.hexdigest()


def canonical_json(value: Any) -> str:
    if value is None:
        return "null"
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, (int, float)) and not isinstance(value, bool):
        if isinstance(value, float) and value.is_integer():
            return str(int(value))
        return json.dumps(value)
    if isinstance(value, str):
        return json.dumps(value, ensure_ascii=True)
    if isinstance(value, list):
        return "[" + ",".join(canonical_json(v) for v in value) + "]"
    if isinstance(value, dict):
        parts = [
            json.dumps(k, ensure_ascii=True) + ":" + canonical_json(value[k])
            for k in sorted(value)
        ]
        return "{" + ",".join(parts) + "}"
    raise ContractError(f"cannot canonicalize {type(value).__name__}")


def read_request(path: Path) -> HarvestRequest:
    raw = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(raw, dict):
        raise ContractError("request.json must be an object")
    return HarvestRequest.from_dict(raw)
