"""Network-v2 bodies. Unchanged training field definitions remain owned by v1."""

from __future__ import annotations

import math
from typing import Annotated, Literal, get_args, get_origin

from pydantic import (
    BaseModel,
    ConfigDict,
    Field,
    JsonValue,
    StrictBool,
    StrictInt,
    StringConstraints,
    model_validator,
)

from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages import (
    F32,
    OD_PRESETS,
    SS58,
    Accept,
    AuditChallenge,
    Chunk,
    Commit,
    DeltaManifest,
    Finalize,
    Hex64,
    ImageDigest,
    Layout,
    LeafPreimage,
    Receipt,
    ReplayVerdict,
    Rollback,
    RunManifest,
    StateServe,
    f32val,
)

# I-JSON is narrower than u64: reuse the frozen canonicalizer's exact integer ceiling.
U64 = Annotated[StrictInt, Field(ge=0, le=2**53 - 1)]
Positive = Annotated[StrictInt, Field(ge=1, le=2**53 - 1)]
PPM = Annotated[StrictInt, Field(ge=0, le=1_000_000)]
Text = Annotated[str, StringConstraints(min_length=1, max_length=4096)]
Identifier = Annotated[str, StringConstraints(pattern=r"^[A-Za-z0-9_.:-]{1,128}$")]
Hashes = Annotated[list[Hex64], Field(max_length=512)]
AdmissionState = Literal["APPLIED", "PROBATION", "ACTIVE", "SUSPENDED", "BANNED"]
WEIGHT_QUANTUM = 1 << 24
MAX_OBJECT_BYTES = 2 << 30
MAX_BODY_BYTES = 64 << 10
MAX_MANIFEST_BYTES = 1 << 20


class WireModel(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True)

    @model_validator(mode="before")
    @classmethod
    def _strict_literals(cls, data: JsonValue) -> JsonValue:
        if isinstance(data, dict):
            for name, info in cls.model_fields.items():
                args = get_args(info.annotation)
                if (
                    get_origin(info.annotation) is Literal
                    and args
                    and type(args[0]) is int
                    and name in data
                    and type(data[name]) is not int
                ):
                    raise ValueError("integer literal fields require strict integers")
        return data

    def body(self) -> dict[str, JsonValue]:
        return self.model_dump(mode="json")

    def digest(self) -> str:
        return sha256_hex(canonicalize(self.body(), allow_float=False))


class TrainingManifestV2(RunManifest):
    """Replace only the version-dependent validator; never invoke the v1 OD layout gate."""

    @model_validator(mode="after")
    def _assign_unit_and_od(self) -> TrainingManifestV2:
        ds, inn, ms, ref = self.dataset, self.inner, self.model, self.reference_spec
        lay = ref.layout
        if lay.pp != 1:
            raise ValueError("v2 supports pp1 only")
        if ds.assign_unit is not None:
            if self.batch_samples() % ds.assign_unit or ds.n_samples % ds.assign_unit:
                raise ValueError("assignment unit must divide island batch and dataset")
            if ds.unit_sha256_root is None:
                raise ValueError("assign_unit requires unit_sha256_root")
        elif ds.unit_sha256_root is not None:
            raise ValueError("unit_sha256_root requires assign_unit")
        if self.batch_samples() > ds.n_samples:
            raise ValueError("dataset cannot supply one island assignment")
        if ms.d_model % ms.n_heads or (ms.d_model // ms.n_heads) % 2:
            raise ValueError("model must split into even-sized attention heads")
        if ms.n_heads % ms.n_kv_heads or ms.top_k_experts > ms.n_experts:
            raise ValueError("invalid attention or expert geometry")
        if ms.n_experts == 1 and lay.ep_size != 1:
            raise ValueError("dense models require EP1")
        if ms.n_experts % lay.ep_size:
            raise ValueError("EP must divide expert count")
        match ms.arch:
            case "decoder":
                if ref.profile is not None:
                    raise ValueError("profile is only valid for OD")
                d, kv = ms.d_model, ms.n_kv_heads * (ms.d_model // ms.n_heads)
                ff = 3 * d * ms.d_ff * ms.n_experts
                router = d * ms.n_experts if ms.n_experts > 1 else 0
                count = (
                    2 * ms.vocab * d
                    + d
                    + ms.n_layers * (2 * d + 2 * d * d + 2 * d * kv + ff + router)
                )
            case "od-encoder":
                od = ms.od
                assert od is not None  # ModelSpec already enforces arch iff OD.
                if lay.ep_size != 1 or lay.dp_size != lay.n_gpus:
                    raise ValueError("OD requires dense DP=N/EP1")
                if ms.n_kv_heads != ms.n_heads or ms.d_ff != 4 * ms.d_model:
                    raise ValueError("OD attention/FF geometry mismatch")
                if ms.n_experts != 1 or ms.top_k_experts != 1:
                    raise ValueError("OD requires one expert")
                if f32val(ms.capacity_factor) != 1 or f32val(ms.aux_loss_coef) != 0:
                    raise ValueError("OD requires capacity=1 and aux_loss=0")
                if ref.profile is None:
                    raise ValueError("OD requires a reference profile")
                dtype = "fp32" if ref.profile == "od-fp32-ref-v1" else "bf16"
                if ms.compute_dtype != dtype:
                    raise ValueError("OD dtype/profile mismatch")
                d, layers, heads, head_layers, vocab = OD_PRESETS[od.preset]
                if (ms.d_model, ms.n_layers, ms.n_heads, ms.vocab, od.head_layers) != (
                    d,
                    layers,
                    heads,
                    vocab,
                    head_layers,
                ):
                    raise ValueError("OD dimensions differ from pinned preset")
                if (od.record is None) != (od.objective == "mlm"):
                    raise ValueError("OD record required iff objective is not MLM")
                if od.record is not None and ms.seq_len != od.record.record_len() - 1:
                    raise ValueError("OD record length must equal seq_len+1")
                if od.objective == "mlm" and (
                    (od.preset == "od-tiny" and ms.seq_len > 64)
                    or (od.preset != "od-tiny" and ms.seq_len != 1024)
                ):
                    raise ValueError("OD MLM sequence length differs from pinned preset")
                if ds.sample_format != "u16[seq_len+1] token ids":
                    raise ValueError("OD requires u16 records")
                if not all(
                    math.isfinite(f32val(v))
                    for v in (od.mask_ratio, od.distill_temp, od.rps_weight)
                ):
                    raise ValueError("nonfinite OD objective quantity")
                if not 0 < f32val(od.mask_ratio) <= 1 or f32val(od.distill_temp) <= 0:
                    raise ValueError("OD mask ratio/temperature must be positive")
                if f32val(od.rps_weight) < 0:
                    raise ValueError("OD RPS weight must be nonnegative")
                # Shape-only count of the pinned flat OD state_dict (no tensors allocated).
                count = vocab * d + layers * (12 * d * d + 9 * d + 4 * (d // heads)) + 2 * d
                if od.objective != "mlm":
                    count += 4 * d + head_layers * (16 * d * d + 11 * d)
                    count += d * d + 4 * d + 4
            case unreachable:
                raise ValueError(f"unsupported architecture {unreachable}")
        if ms.param_count != count:
            raise ValueError(f"param_count {ms.param_count} != shape count {count}")
        if len(self.auditors) > 128 or len(set(self.auditors)) != len(self.auditors):
            raise ValueError("auditor list must be bounded and unique")
        if not all(
            math.isfinite(f32val(value))
            for value in (
                ms.capacity_factor,
                ms.aux_loss_coef,
                ms.init_std,
                inn.eps,
                inn.wd,
                inn.grad_clip,
                inn.muon_momentum,
                *inn.betas,
                inn.lr_schedule.peak_lr,
                self.outer.lr,
                self.outer.momentum,
                self.outer.topk_frac,
                self.outer.ef_beta,
                self.outer.preclip_norm,
                self.outer.cclip_tau,
                self.verify.q_base,
                self.verify.influence_cap,
                self.operator_budget.honeypot.rate,
            )
        ):
            raise ValueError("nonfinite training quantity")
        return self

    def batch_samples(self) -> int:
        return (
            self.inner.H
            * self.inner.micro_batch
            * self.inner.grad_accum
            * self.reference_spec.layout.n_gpus
        )

    def run_id(self) -> str:
        raise ValueError("training hash is not a run ID; use RunManifestV2.run_id()")

    def training_hash(self) -> str:
        return sha256_hex(canonicalize(self.body(), allow_float=False))


class PolicyHashes(WireModel):
    admission_policy_hash: Hex64
    economics_policy_hash: Hex64
    aggregation_policy_hash: Hex64
    dispute_policy_hash: Hex64
    audit_policy_hash: Hex64


class NetworkSpec(PolicyHashes):
    relay_registry_hash: Hex64
    audit_mode: Literal["anchored-full"]
    full_anchor_version: Literal[1]
    capabilities: Annotated[
        list[Literal["island-replay", "all-level-disputes", "transport-receipts"]],
        Field(min_length=3, max_length=3),
    ]

    @model_validator(mode="after")
    def _capabilities(self) -> NetworkSpec:
        if len(set(self.capabilities)) != 3:
            raise ValueError("all three capabilities required exactly once")
        return self


class RunManifestV2(WireModel):
    manifest_version: Literal[2]
    training: TrainingManifestV2
    network: NetworkSpec

    def run_id(self) -> str:
        return self.digest()

    def validate_run_id(self, run_id: str) -> None:
        if run_id != self.run_id():
            raise ValueError("wrapper hash differs from explicit run ID")


class RosterEntryV2(WireModel):
    hotkey: SS58
    slot: U64
    q_i: F32
    admission_id: Hex64
    coldkey_group: Identifier
    state: AdmissionState
    eligible_weight: Annotated[StrictInt, Field(ge=0, le=WEIGHT_QUANTUM)]

    @model_validator(mode="after")
    def _finite(self) -> RosterEntryV2:
        if not math.isfinite(f32val(self.q_i)):
            raise ValueError("nonfinite roster audit probability")
        return self


class RoundOpenV2(WireModel):
    w: U64
    prev_final_hash: Hex64
    theta_hash: Hex64
    outer_state_hash: Hex64
    center_hash: Hex64
    roster_hash: Hex64
    honeypot_commit: Hex64
    d_open: Positive
    d_assign: Positive
    d_commit: Positive
    d_audit: Positive
    d_upload: Positive
    d_final: Positive
    contract_version: Literal[2]
    policy_hashes: PolicyHashes
    registry_epoch: U64
    start_state_index_hash: Hex64
    audit_mode: Literal["anchored-full"]
    roster: Annotated[list[RosterEntryV2], Field(max_length=1024)]

    @model_validator(mode="after")
    def _unique_roster(self) -> RoundOpenV2:
        if not (
            self.d_open < self.d_assign < self.d_commit < self.d_audit < self.d_final
            and self.d_commit < self.d_upload <= self.d_final
        ):
            raise ValueError("invalid round deadline ordering")
        if len({r.slot for r in self.roster}) != len(self.roster):
            raise ValueError("duplicate roster slot")
        if len({r.hotkey for r in self.roster}) != len(self.roster):
            raise ValueError("duplicate roster hotkey")
        return self


class StartStateV2(WireModel):
    run_id: Hex64
    w: U64
    hotkey: SS58
    theta_hash: Hex64
    state_object_sha256: Hex64
    opt_state_hash: Hex64
    ef_object_sha256: Hex64
    ef_hash: Hex64
    parent_anchor_hash: Hex64
    global_step0: U64
    anchor_verdict_hash: Hex64


class AcceptV2(Accept):
    work_screen_hash: Hex64

    def validate_manifest(self, manifest: RunManifestV2) -> None:
        ref = manifest.training.reference_spec
        if (
            self.n_gpus != ref.layout.n_gpus
            or self.image_digest != ref.image_digest
            or self.driver_version not in ref.driver_allowlist
        ):
            raise ValueError("Accept differs from pinned logical layout/reference environment")


class CommitV2(Commit):
    delta_bytes: Annotated[StrictInt, Field(ge=1, le=MAX_OBJECT_BYTES)]

    def validate_assignment(self, manifest: RunManifestV2, sample_count: int) -> None:
        if sample_count != manifest.training.batch_samples():
            raise ValueError("assignment sample count differs from manifest")
        if self.tokens != sample_count * manifest.training.model.seq_len:
            raise ValueError("tokens differ from server-derived entitlement")
        inner = manifest.training.inner
        if self.n_leaves != inner.H // inner.J + 1:
            raise ValueError("leaf count differs from manifest")


class DeltaManifestV2(DeltaManifest):
    grant_hash: Hex64
    master_acceptance_hash: Hex64
    size: Annotated[StrictInt, Field(ge=1, le=MAX_OBJECT_BYTES)]
    chunks: Annotated[list[Chunk], Field(min_length=1, max_length=512)]


class AuditChallengeV2(AuditChallenge):
    anchor_hash: Hex64
    audit_mode: Literal["anchored-full"]
    mode: Literal["full"]
    segments: Annotated[list[tuple[U64, U64]], Field(max_length=256)]


class ArtifactRef(WireModel):
    sha256: Hex64
    size: Annotated[StrictInt, Field(ge=1, le=MAX_OBJECT_BYTES)]


class AuditJobV2(WireModel):
    run_id: Hex64
    job_id: Hex64
    auditor_id: SS58
    attempt: Annotated[StrictInt, Field(ge=1, le=2)]
    lease_nonce: Hex64
    lease_expires: Positive
    absolute_deadline: Positive
    reservation_id: Hex64
    replay_step_budget: Positive
    anchor_age: Annotated[StrictInt, Field(ge=0, le=1)]
    manifest: RunManifestV2
    challenge_envelope: dict[str, JsonValue]
    commit_envelope: dict[str, JsonValue]
    sample_ids: Annotated[list[U64], Field(min_length=1, max_length=1_000_000)]
    start_state: StartStateV2
    preimages: Annotated[list[LeafPreimage], Field(min_length=1, max_length=4096)]
    ef_in: ArtifactRef
    v0: ArtifactRef
    created_beacon: Positive

    @model_validator(mode="after")
    def _bounds(self) -> AuditJobV2:
        self.manifest.validate_run_id(self.run_id)
        h = self.manifest.training.inner.H
        if not h <= self.replay_step_budget <= 2 * h:
            raise ValueError("replay budget must cover H and not exceed 2H")
        if not self.created_beacon < self.lease_expires <= self.absolute_deadline:
            raise ValueError("invalid audit lease deadline")
        if self.lease_expires > self.created_beacon + 100:
            raise ValueError("lease exceeds 100 beacon rounds")
        if self.absolute_deadline > self.created_beacon + 200:
            raise ValueError("absolute audit deadline exceeds 200 beacon rounds")
        if len(self.sample_ids) != self.manifest.training.batch_samples():
            raise ValueError("job assignment length mismatch")
        if self.start_state.run_id != self.run_id:
            raise ValueError("anchor run mismatch")
        self.validate_embedded(self.created_beacon)
        commit = CommitV2.model_validate(self.commit_envelope["body"])
        challenge = AuditChallengeV2.model_validate(self.challenge_envelope["body"])
        if (
            commit.w != challenge.w
            or commit.hotkey != challenge.target
            or self.start_state.w != commit.w
            or self.start_state.hotkey != commit.hotkey
            or challenge.anchor_hash != self.start_state.digest()
            or self.absolute_deadline > challenge.serve_deadline
        ):
            raise ValueError("audit job assignment/anchor/deadline binding mismatch")
        commit.validate_assignment(self.manifest, len(self.sample_ids))
        if any(p.run_id != self.run_id or p.w != commit.w for p in self.preimages):
            raise ValueError("audit preimage run/round mismatch")
        if not all(
            math.isfinite(f32val(v)) for p in self.preimages for v in (p.loss_f32, p.norm_f32)
        ):
            raise ValueError("nonfinite audit metric quantity")
        return self

    def validate_embedded(self, now_round: int) -> None:
        """Live-expiry path; never waive expiry using an unauthenticated history assertion."""
        from hypertrain.protocol.envelope_v2 import parse_envelope, verify_envelope

        if type(now_round) is not int or not self.created_beacon <= now_round < self.lease_expires:
            raise ValueError("embedded intake requires a live authenticated job lease")
        for raw, expected in (
            (self.challenge_envelope, "AuditChallengeV2"),
            (self.commit_envelope, "CommitV2"),
        ):
            env = parse_envelope(raw)
            if env.type != expected or env.run_id != self.run_id or not verify_envelope(raw):
                raise ValueError("audit job embeds invalid authenticated envelope")
            # L0 owns authenticated original acceptance evidence for historical records.
            # This callable supports only live expiry; no unsigned flag bypasses that rule.
            if env.exp_drand < now_round:
                raise ValueError("embedded envelope expired at verified intake beacon")
            if expected == "CommitV2" and env.signer != env.body["hotkey"]:
                raise ValueError("embedded commit hotkey differs from signer")


class RankWorkResult(WireModel):
    rank: U64
    fingerprint: Hex64
    elapsed_ns: U64
    allocated_bytes: U64
    reserved_bytes: U64
    total_bytes: Positive

    @model_validator(mode="after")
    def _memory(self) -> RankWorkResult:
        if max(self.allocated_bytes, self.reserved_bytes) * 100 > self.total_bytes * 85:
            raise ValueError("full-round peak exceeds 85%")
        return self


class WorkScreenV2(WireModel):
    evidence_kind: Literal["work-proof-v1"]
    admission_id: Hex64
    nonce: Hex64
    policy_hash: Hex64
    image_digest: ImageDigest
    layout: Layout
    challenge_hash: Hex64
    rank_results: Annotated[list[RankWorkResult], Field(min_length=1, max_length=1024)]
    artifact_hashes: Hashes

    @model_validator(mode="after")
    def _ranks(self) -> WorkScreenV2:
        if sorted(r.rank for r in self.rank_results) != list(range(self.layout.n_gpus)):
            raise ValueError("work screen needs every logical rank exactly once")
        return self


class HardwareHint(WireModel):
    device_name: Text
    device_count: Positive
    driver: Text


class JoinRequest(WireModel):
    run_id: Hex64
    request_id: Hex64
    hotkey: SS58
    coldkey: SS58
    expires_beacon: Positive
    policy_hash: Hex64
    hardware_hint: HardwareHint
    hot_sig: Annotated[str, StringConstraints(pattern=r"^[0-9a-f]{128}$")]
    cold_sig: Annotated[str, StringConstraints(pattern=r"^[0-9a-f]{128}$")]


class ArtifactLimits(WireModel):
    max_object_bytes: Annotated[StrictInt, Field(ge=1, le=MAX_OBJECT_BYTES)]
    max_body_bytes: Literal[65536]
    max_chunk_manifest_bytes: Literal[1048576]


class JoinChallenge(WireModel):
    admission_id: Hex64
    nonce: Hex64
    seed_beacon: Positive
    deadline_beacon: Positive
    manifest_hash: Hex64
    theta_hash: Hex64
    assignment_hash: Hex64
    layout: Layout
    artifact_limits: ArtifactLimits
    policy_hash: Hex64

    @model_validator(mode="after")
    def _deadline(self) -> JoinChallenge:
        if self.deadline_beacon != self.seed_beacon + 100:
            raise ValueError("join deadline must be seed+100")
        return self


class WorkProof(WireModel):
    admission_id: Hex64
    challenge_hash: Hex64
    leaves_root: Hex64
    delta_hash: Hex64
    artifact_refs: Annotated[list[ArtifactRef], Field(min_length=1, max_length=512)]


class RotateRequest(WireModel):
    hotkey: SS58
    new_hotkey: SS58
    operation_id: Hex64


class AdmissionPolicyV2(WireModel):
    q_base: Literal["0000803e"]  # f32 0.25
    E: Literal[4]
    clean_finalizations: Literal[12]
    work_screen_epoch_rounds: Positive
    max_pending: Literal[32]
    max_trial_replays: Literal[2]
    max_trials_per_beacon: Literal[2]
    join_per_beacon: Literal[2]
    join_burst: Literal[4]
    ip_prefix_per_beacon: Literal[32]
    ip_prefix_burst: Literal[64]
    challenge_rounds: Literal[100]
    artifact_limits: ArtifactLimits


class EconomicsPolicyV2(WireModel):
    ledger_mode: Literal["test", "production"]
    G_max_units: U64
    R_collectible_units: U64
    S_min_units: U64
    beta_ppm: Positive
    gammaV_units: Literal[0]
    s_lower_ppm: PPM
    q_floor: Literal[1_000_000]
    genesis_allocation_hash: Hex64 | None
    reward_authority: SS58
    round_reward_units: U64
    max_total_issuance: U64
    shadow_bootstrap_rounds: Literal[12]

    @model_validator(mode="after")
    def _mode(self) -> EconomicsPolicyV2:
        if (self.ledger_mode == "test") != (self.genesis_allocation_hash is not None):
            raise ValueError("only test mode requires pinned genesis allocation")
        return self


class AggregationPolicyV2(WireModel):
    arithmetic: Literal["flat-cclip-cap-v1"]
    order: Literal["utf8"]
    center: Literal["prev_outer_update"]
    trust_mode: Literal["uniform-verified"]
    weight_quantum: Literal[16777216]
    miner_cap_units: Literal[4194304]
    probation_cap_units: Literal[4194304]
    owner_group_caps: Annotated[dict[Identifier, Positive], Field(max_length=1024)]
    preclip_norm: F32
    cclip_tau: F32
    cclip_iters: Positive

    @model_validator(mode="after")
    def _finite(self) -> AggregationPolicyV2:
        if not all(math.isfinite(f32val(v)) for v in (self.preclip_norm, self.cclip_tau)):
            raise ValueError("nonfinite aggregation quantity")
        return self


class DisputePolicyV2(WireModel):
    referees: Annotated[list[SS58], Field(min_length=1, max_length=128)]
    max_open: Literal[32]
    max_transcript_entries: Literal[256]
    max_entry_bytes: Literal[65536]
    fanout: Literal[2]
    max_referee_reassignments: Literal[1]
    max_referee_cost_units: U64


class AuditPolicyV2(WireModel):
    lease_rounds: Literal[100]
    max_attempts: Literal[2]
    max_concurrent_per_auditor: Literal[1]
    max_running_jobs: Literal[2]
    max_queued_jobs: Literal[32]
    max_anchor_age_rounds: Literal[1]
    max_steps_per_attempt: Positive

    def validate_training(self, training: TrainingManifestV2) -> None:
        if self.max_steps_per_attempt != 2 * training.inner.H:
            raise ValueError("audit policy budget must equal 2H")


class IslandJobV1(WireModel):
    job_version: Literal[1]
    run_id: Hex64
    w: U64
    manifest: RunManifestV2
    sample_ids: Annotated[list[U64], Field(min_length=1, max_length=1_000_000)]
    global_step0: U64
    start_state_sha256: Hex64
    ef_in_sha256: Hex64
    v0_sha256: Hex64
    object_paths: Annotated[dict[Identifier, Text], Field(min_length=1, max_length=512)]
    deadline: Positive

    @model_validator(mode="after")
    def _binding(self) -> IslandJobV1:
        self.manifest.validate_run_id(self.run_id)
        if len(self.sample_ids) != self.manifest.training.batch_samples():
            raise ValueError("island job assignment length mismatch")
        for path in self.object_paths.values():
            if path.startswith("/") or ".." in path.split("/") or "\\" in path:
                raise ValueError("local object paths must be relative and confined")
        return self


class WeightedEntryV2(WireModel):
    hotkey: SS58
    admission_id: Hex64
    probation: StrictBool
    weight_units: Annotated[StrictInt, Field(ge=0, le=WEIGHT_QUANTUM // 4)]
    commit_hash: Hex64
    delta_manifest_hash: Hex64


class WeightAllocationV2(WireModel):
    policy_hash: Hex64
    entries: Annotated[list[WeightedEntryV2], Field(min_length=4, max_length=1024)]

    @model_validator(mode="after")
    def _caps(self) -> WeightAllocationV2:
        ids = [e.hotkey for e in self.entries]
        if ids != sorted(set(ids), key=lambda value: value.encode("utf-8")):
            raise ValueError("weights must use unique UTF-8 miner order")
        if sum(e.weight_units for e in self.entries) != WEIGHT_QUANTUM:
            raise ValueError("weights must sum exactly to Q; no renormalization")
        if sum(e.weight_units for e in self.entries if e.probation) > WEIGHT_QUANTUM // 4:
            raise ValueError("probation cohort exceeds Q/4")
        return self


class OriginAllocation(WireModel):
    origin_id: Hex64
    owner: SS58
    units: Positive
    mature_at: U64


class TestGenesis(WireModel):
    run_id: Hex64
    allocation_hash: Hex64
    total_units: Positive
    origins: Annotated[list[OriginAllocation], Field(min_length=1, max_length=1024)]
    authority_sig: Annotated[str, StringConstraints(pattern=r"^[0-9a-f]{128}$")]

    @model_validator(mode="after")
    def _conservation(self) -> TestGenesis:
        if len({o.origin_id for o in self.origins}) != len(self.origins):
            raise ValueError("duplicate genesis origin")
        if sum(o.units for o in self.origins) != self.total_units:
            raise ValueError("genesis allocations do not conserve total")
        return self


class RewardFinalize(WireModel):
    run_id: Hex64
    w: U64
    finalize_hash: Hex64
    tape_hash: Hex64
    verdict_root: Hex64
    allocation_hash: Hex64
    budget_units: Positive
    origin_ids: Annotated[list[Hex64], Field(min_length=1, max_length=1024)]
    mature_at: U64
    authority_sig: Annotated[str, StringConstraints(pattern=r"^[0-9a-f]{128}$")]

    @model_validator(mode="after")
    def _origins(self) -> RewardFinalize:
        if len(set(self.origin_ids)) != len(self.origin_ids):
            raise ValueError("duplicate reward origin")
        return self


class ShadowRewardFinalizeV1(RewardFinalize):
    shadow: Literal[True]
    shadow_ordinal: Annotated[StrictInt, Field(ge=0, lt=12)]
    reservation_hash: Hex64

    @model_validator(mode="before")
    @classmethod
    def _signed_domain(cls, data: JsonValue) -> JsonValue:
        if isinstance(data, dict) and data.get("shadow") is not True:
            raise ValueError("shadow domain requires JSON true")
        return data


class ShadowReservationRequestV1(WireModel):
    """Trusted store intake binds original custody before external signing."""

    run_id: Hex64
    admission_id: Hex64
    hotkey: SS58
    coldkey: SS58
    trial_epoch: U64
    commit_hash: Hex64
    finalize_hash: Hex64
    custody_hash: Hex64
    graph_hash: Hex64
    economic_admission_hash: Hex64
    nonce: Hex64
    assignment_hash: Hex64
    finalized_beacon: Positive
    mature_at: Positive


class ShadowReservationV1(ShadowReservationRequestV1):
    shadow_ordinal: Annotated[StrictInt, Field(ge=0, lt=12)]


class ShadowReplayReceiptV1(WireModel):
    run_id: Hex64
    admission_id: Hex64
    hotkey: SS58
    coldkey: SS58
    trial_epoch: U64
    shadow_ordinal: Annotated[StrictInt, Field(ge=0, lt=12)]
    reservation_hash: Hex64
    challenge_hash: Hex64
    signed_challenge_hash: Hex64
    assignment_hash: Hex64
    sample_ids_hash: Hex64
    job_hash: Hex64
    reference_publication_hash: Hex64
    miner_publication_hash: Hex64
    reference_result_hash: Hex64
    miner_result_hash: Hex64
    reference_operation_hash: Hex64
    miner_operation_hash: Hex64
    reference_work_proof_hash: Hex64
    miner_work_proof_hash: Hex64
    commit_hash: Hex64
    audit_challenge_hash: Hex64
    verdict_hash: Hex64
    trial_finalize_hash: Hex64
    final_state_hash: Hex64
    backend_qualification_authority_hash: Hex64
    completed_beacon: Positive


class EscrowOperation(WireModel):
    operation_id: Hex64
    owner: SS58
    units: Positive
    origin_ids: Annotated[list[Hex64], Field(min_length=1, max_length=1024)]
    admission_id: Hex64 | None
    dispute_id: Hex64 | None

    @model_validator(mode="after")
    def _origins(self) -> EscrowOperation:
        if len(set(self.origin_ids)) != len(self.origin_ids):
            raise ValueError("duplicate escrow origin")
        return self


class EscrowLock(EscrowOperation):
    kind: Literal["LOCK_ADMISSION", "LOCK_CONTEST"]

    @model_validator(mode="after")
    def _reference(self) -> EscrowLock:
        if (self.kind == "LOCK_ADMISSION") != (self.admission_id is not None):
            raise ValueError("admission lock/reference mismatch")
        if (self.kind == "LOCK_CONTEST") != (self.dispute_id is not None):
            raise ValueError("contest lock/reference mismatch")
        return self


class EscrowRelease(EscrowOperation):
    lock_id: Hex64
    finality_hash: Hex64
    closed_dispute_root: Hex64


class EscrowTransfer(EscrowOperation):
    recipient: SS58


class EscrowReceipt(WireModel):
    operation_id: Hex64
    event_hash: Hex64
    ledger_mode: Literal["test", "production"]
    available_units: U64
    admission_locked_units: U64
    dispute_locked_units: U64


class DisputeV2(WireModel):
    hotkey: SS58
    verdict_hash: Hex64
    action: Literal["accept", "contest"]
    bond_lock: Hex64 | None

    @model_validator(mode="after")
    def _funded(self) -> DisputeV2:
        if (self.action == "contest") != (self.bond_lock is not None):
            raise ValueError("contest requires funded bond_lock; accept must omit it")
        return self


class BisectV2(WireModel):
    dispute_id: Hex64
    seq: Annotated[StrictInt, Field(ge=0, le=255)]
    level: Literal["step", "layer", "op"]
    ctx: Annotated[list[U64], Field(max_length=2)]
    interval: tuple[U64, U64]
    N: Literal[2]
    hashes: Annotated[list[Hex64], Field(min_length=3, max_length=3)]
    party: SS58
    previous_transcript_hash: Hex64

    @model_validator(mode="after")
    def _context(self) -> BisectV2:
        if len(self.ctx) != {"step": 0, "layer": 1, "op": 2}[self.level]:
            raise ValueError("bisection context does not match level")
        if self.interval[0] >= self.interval[1]:
            raise ValueError("bisection interval must be nonempty")
        return self


class ResolutionV2(WireModel):
    dispute_id: Hex64
    transcript_hash: Hex64
    reason: Literal[
        "MATCH",
        "FRAUD",
        "WITHHELD",
        "PARTY_TIMEOUT",
        "INFRASTRUCTURE",
        "AUDITOR_FAULT",
        "REFEREE_FAILURE",
    ]
    loser: SS58 | None
    evidence_hash: Hex64

    @model_validator(mode="after")
    def _loser(self) -> ResolutionV2:
        no_loser = self.reason in {"MATCH", "INFRASTRUCTURE", "REFEREE_FAILURE"}
        if no_loser != (self.loser is None):
            raise ValueError("resolution reason/loser mismatch")
        return self


MESSAGE_TYPES_V2: dict[str, type[BaseModel]] = {
    cls.__name__: cls
    for cls in (
        RunManifestV2,
        RoundOpenV2,
        StartStateV2,
        AcceptV2,
        CommitV2,
        DeltaManifestV2,
        AuditChallengeV2,
        AuditJobV2,
        WorkScreenV2,
        JoinRequest,
        JoinChallenge,
        WorkProof,
        RotateRequest,
        AdmissionPolicyV2,
        EconomicsPolicyV2,
        AggregationPolicyV2,
        DisputePolicyV2,
        AuditPolicyV2,
        TestGenesis,
        RewardFinalize,
        EscrowLock,
        ShadowRewardFinalizeV1,
        ShadowReplayReceiptV1,
        ShadowReservationRequestV1,
        ShadowReservationV1,
        EscrowRelease,
        EscrowTransfer,
        EscrowReceipt,
        DisputeV2,
        BisectV2,
        ResolutionV2,
        Receipt,
        StateServe,
        ReplayVerdict,
        Rollback,
        Finalize,
        IslandJobV1,
        WeightAllocationV2,
    )
}
