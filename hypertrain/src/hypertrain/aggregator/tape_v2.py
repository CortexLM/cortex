"""Signed versioned flat tapes replay original objects and trusted input facts."""

from __future__ import annotations

from collections.abc import Sequence
from dataclasses import dataclass
from typing import Annotated, Literal

import numpy as np
from pydantic import BaseModel, ConfigDict, Field, JsonValue

from hypertrain.aggregator.core import (
    OuterParams,
    OuterState,
    TapeError,
    get_object,
    load_delta,
    load_state,
)
from hypertrain.aggregator.weighted_v2 import Candidate, aggregate_weighted, allocate_weights
from hypertrain.challenge.trust_v2 import (
    FundedStatus,
    ReplayEvidence,
    SettlementStatus,
    audit_floor,
)
from hypertrain.data.store import Store
from hypertrain.protocol.envelope_v2 import load_json, tape_signing_message
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import Keypair, decode_hotkey, verify
from hypertrain.protocol.messages import SS58, Hex64
from hypertrain.protocol.messages_v2 import (
    U64,
    AggregationPolicyV2,
    CommitV2,
    DeltaManifestV2,
    EconomicsPolicyV2,
    PolicyHashes,
    RosterEntryV2,
    RunManifestV2,
    WeightAllocationV2,
    WireModel,
)


@dataclass(frozen=True, slots=True)
class VerifiedInput:
    """Only authorized L0 intake may assemble these from L1/L2/L5 records."""

    roster: RosterEntryV2
    commit: CommitV2
    delta_manifest: DeltaManifestV2
    replay: ReplayEvidence
    funding: FundedStatus
    settlement: SettlementStatus
    assignment_hash: str
    clean_finalizations: int


class TapeInput(WireModel):
    roster: RosterEntryV2
    commit_hash: Hex64
    delta_manifest_hash: Hex64
    assignment_hash: Hex64
    anchor_hash: Hex64
    verdict_hash: Hex64
    lock_receipt_hash: Hex64
    settlement_hash: Hex64


class Exclusion(WireModel):
    hotkey: SS58
    reason: Literal["SHADOW", "UNVERIFIED", "DISPUTE", "UNFUNDED", "INFRASTRUCTURE", "FRAUD"]
    evidence_hash: Hex64


class TapeBodyV2(WireModel):
    v: Literal["ht-tape-v2"]
    arithmetic: Literal["flat-cclip-cap-v1"]
    run_id: Hex64
    w: U64
    policy_hash: Hex64
    policy_hashes: PolicyHashes
    predecessor_tape_hash: Hex64
    prev_state: Hex64
    prev_hashes: dict[str, Hex64]
    inputs: Annotated[list[TapeInput], Field(min_length=4, max_length=1024)]
    input_root: Hex64
    excluded: Annotated[list[Exclusion], Field(max_length=1024)]
    allocation: WeightAllocationV2
    preclipped: list[SS58]
    copy_suspicion: list[dict[str, str]]
    out_state: Hex64
    out_hashes: dict[str, Hex64]


class TapeV2(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True)

    body: TapeBodyV2
    signer: SS58
    sig: Annotated[str, Field(pattern=r"^[0-9a-f]{128}$")]

    def to_bytes(self) -> bytes:
        return canonicalize(self.body_json(), allow_float=False)

    def body_json(self) -> dict[str, JsonValue]:
        return {"body": self.body.body(), "signer": self.signer, "sig": self.sig}

    @classmethod
    def from_bytes(cls, raw: bytes) -> TapeV2:
        data = load_json(raw, max_bytes=1 << 20)
        tape = cls.model_validate(data)
        if canonicalize(data, allow_float=False) != tape.to_bytes():
            raise TapeError("noncanonical tape body")
        return tape


def _input(work: VerifiedInput, manifest: RunManifestV2, w: int) -> TapeInput:
    c, d, r, s = work.commit, work.delta_manifest, work.replay, work.settlement
    if (
        (c.w, d.w, r.w, s.w) != (w, w, w, w)
        or (c.hotkey, d.hotkey, r.hotkey, s.hotkey) != (work.roster.hotkey,) * 4
        or (r.run_id, s.run_id) != (manifest.run_id(),) * 2
    ):
        raise TapeError("input identity/run/round mismatch")
    c.validate_assignment(manifest, manifest.training.batch_samples())
    if (d.delta_hash, d.size) != (c.delta_hash, c.delta_bytes):
        raise TapeError("delta manifest differs from commit")
    if (
        r.result != "MATCH"
        or r.audit_mode != "anchored-full"
        or (
            r.assignment_hash,
            r.leaves_root,
            r.delta_hash,
            r.final_theta_hash,
            r.ef_in_hash,
            r.ef_out_hash,
        )
        != (
            work.assignment_hash,
            c.leaves_root,
            c.delta_hash,
            c.final_theta_hash,
            c.ef_in_hash,
            c.ef_out_hash,
        )
    ):
        raise TapeError("own-assignment full replay not VERIFIED")
    if s.unresolved_dispute or s.excluded:
        raise TapeError("input unsettled/excluded")
    return TapeInput(
        roster=work.roster,
        commit_hash=sha256_hex(canonicalize(c.model_dump(mode="json"))),
        delta_manifest_hash=sha256_hex(canonicalize(d.model_dump(mode="json"))),
        assignment_hash=work.assignment_hash,
        anchor_hash=r.anchor_hash,
        verdict_hash=r.verdict_hash,
        lock_receipt_hash=work.funding.lock_receipt_hash,
        settlement_hash=s.status_hash,
    )


def compute_body(
    store: Store,
    manifest: RunManifestV2,
    policy: AggregationPolicyV2,
    economics: bytes,
    *,
    w: int,
    prev_state: str,
    predecessor_tape_hash: str,
    inputs: Sequence[VerifiedInput],
    excluded: Sequence[Exclusion] = (),
    reference_reward_units: int,
) -> tuple[TapeBodyV2, OuterState]:
    """Fetch every original object, allocate exact weights, emit reproducible arithmetic."""
    from hypertrain.protocol.messages_v2 import EconomicsPolicyV2

    if policy.digest() != manifest.network.aggregation_policy_hash:
        raise TapeError("policy not pinned by run")
    econ = EconomicsPolicyV2.model_validate(load_json(economics))
    if econ.digest() != manifest.network.economics_policy_hash:
        raise TapeError("economics not pinned by run")
    ordered = sorted(inputs, key=lambda x: x.roster.hotkey.encode("utf-8"))
    records = [_input(x, manifest, w) for x in ordered]
    if len({x.roster.hotkey for x in ordered}) != len(ordered):
        raise TapeError("duplicate input")
    exclusions = sorted(excluded, key=lambda x: x.hotkey.encode("utf-8"))
    if len({x.hotkey for x in exclusions}) != len(exclusions) or (
        {x.hotkey for x in exclusions} & {x.roster.hotkey for x in ordered}
    ):
        raise TapeError("conflicting exclusion set")
    for x, record in zip(ordered, records, strict=True):
        _verify_original(store, manifest, econ, x, record, reference_reward_units)
    previous = load_state(store, prev_state)
    return _aggregate_body(
        manifest,
        policy,
        previous,
        w=w,
        prev_state=prev_state,
        predecessor_tape_hash=predecessor_tape_hash,
        records=records,
        exclusions=exclusions,
        deltas={
            x.roster.hotkey: load_delta(store, x.commit.delta_hash, previous.theta) for x in ordered
        },
    )


def _verify_original(
    store: Store,
    manifest: RunManifestV2,
    econ: EconomicsPolicyV2,
    work: VerifiedInput,
    record: TapeInput,
    reference_reward_units: int,
) -> None:
    floor = audit_floor(
        econ,
        manifest.training.verify,
        work.roster,
        work.funding,
        run_id=manifest.run_id(),
        clean_finalizations=work.clean_finalizations,
        reference_reward_units=reference_reward_units,
    )
    if not floor.influence_allowed:
        raise TapeError("funded influence gate failed")
    for key, original_body in (
        (record.commit_hash, work.commit.model_dump(mode="json")),
        (record.delta_manifest_hash, work.delta_manifest.model_dump(mode="json")),
    ):
        if get_object(store, key) != canonicalize(original_body, allow_float=False):
            raise TapeError("original input body differs")
    payload = get_object(store, work.commit.delta_hash)
    if len(payload) != work.delta_manifest.size or any(
        sha256_hex(payload[c.off : c.off + c.len]) != c.sha256 for c in work.delta_manifest.chunks
    ):
        raise TapeError("original delta/chunks differ")


def _aggregate_body(
    manifest: RunManifestV2,
    policy: AggregationPolicyV2,
    previous: OuterState,
    *,
    w: int,
    prev_state: str,
    predecessor_tape_hash: str,
    records: list[TapeInput],
    exclusions: list[Exclusion],
    deltas: dict[str, dict[str, np.ndarray]],
) -> tuple[TapeBodyV2, OuterState]:
    """Shared arithmetic; provenance is checked by each version's input boundary."""
    allocation = allocate_weights(
        [Candidate(r.roster, r.commit_hash, r.delta_manifest_hash) for r in records], policy
    )
    o = manifest.training.outer
    params = OuterParams(
        o.opt, o.lr, o.momentum, policy.preclip_norm, policy.cclip_tau, policy.cclip_iters
    )
    result = aggregate_weighted(
        previous,
        deltas,
        allocation,
        policy,
        params,
    )
    hashes = PolicyHashes.model_validate(
        {
            name: getattr(manifest.network, name)
            for name in (
                "admission_policy_hash",
                "economics_policy_hash",
                "aggregation_policy_hash",
                "dispute_policy_hash",
                "audit_policy_hash",
            )
        }
    )
    body = TapeBodyV2(
        v="ht-tape-v2",
        arithmetic=policy.arithmetic,
        run_id=manifest.run_id(),
        w=w,
        policy_hash=policy.digest(),
        policy_hashes=hashes,
        predecessor_tape_hash=predecessor_tape_hash,
        prev_state=prev_state,
        prev_hashes=previous.hashes(),
        inputs=records,
        input_root=sha256_hex(canonicalize([r.body() for r in records])),
        excluded=exclusions,
        allocation=allocation,
        preclipped=list(result.preclipped),
        copy_suspicion=list(result.copy_suspicion),
        out_state=sha256_hex(result.state.to_bytes()),
        out_hashes=result.state.hashes(),
    )
    return body, result.state


def make_tape(
    store: Store,
    manifest: RunManifestV2,
    policy: AggregationPolicyV2,
    economics: bytes,
    key: Keypair,
    *,
    w: int,
    prev_state: str,
    predecessor_tape_hash: str,
    inputs: Sequence[VerifiedInput],
    excluded: Sequence[Exclusion] = (),
    reference_reward_units: int,
) -> TapeV2:
    body, state = compute_body(
        store,
        manifest,
        policy,
        economics,
        w=w,
        prev_state=prev_state,
        predecessor_tape_hash=predecessor_tape_hash,
        inputs=inputs,
        excluded=excluded,
        reference_reward_units=reference_reward_units,
    )
    store.put(state.to_bytes())
    return TapeV2(
        body=body,
        signer=key.ss58,
        sig=key.sign(tape_signing_message(body.body(), manifest.run_id())).hex(),
    )


def replay_tape(
    store: Store,
    tape: TapeV2,
    manifest: RunManifestV2,
    policy: AggregationPolicyV2,
    economics: bytes,
    *,
    signer: str,
    w: int,
    prev_state: str,
    predecessor_tape_hash: str,
    inputs: Sequence[VerifiedInput],
    excluded: Sequence[Exclusion] = (),
    reference_reward_units: int,
) -> OuterState:
    """Caller pins expected predecessor/input set; a signer cannot omit accepted work."""
    if (
        tape.signer != signer
        or tape.body.run_id != manifest.run_id()
        or not verify(
            decode_hotkey(signer),
            tape_signing_message(tape.body.body(), manifest.run_id()),
            bytes.fromhex(tape.sig),
        )
    ):
        raise TapeError("tape signature/run/signer mismatch")
    body, state = compute_body(
        store,
        manifest,
        policy,
        economics,
        w=w,
        prev_state=prev_state,
        predecessor_tape_hash=predecessor_tape_hash,
        inputs=inputs,
        excluded=excluded,
        reference_reward_units=reference_reward_units,
    )
    if body.body() != tape.body.body():
        raise TapeError("tape replay differs from original inputs/weights/policy/predecessor")
    if get_object(store, body.out_state) != state.to_bytes():
        raise TapeError("output object differs")
    return state
