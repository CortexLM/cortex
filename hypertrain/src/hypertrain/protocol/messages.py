"""Wire message bodies (ultrabrain advisory section 1.1). Floats travel as little-endian f32 hex."""

from __future__ import annotations

import math
import struct
from fractions import Fraction
from typing import Annotated, Literal

from pydantic import (
    BaseModel,
    ConfigDict,
    Field,
    StrictBool,
    StrictInt,
    StringConstraints,
    model_validator,
)

from hypertrain.protocol.hashing import MerkleTree, sha256_hex
from hypertrain.protocol.jcs import canonicalize

Hex64 = Annotated[str, StringConstraints(pattern=r"^[0-9a-f]{64}$")]
F32 = Annotated[str, StringConstraints(pattern=r"^[0-9a-f]{8}$")]
SS58 = Annotated[str, StringConstraints(pattern=r"^[1-9A-HJ-NP-Za-km-z]{46,50}$")]
ImageDigest = Annotated[str, StringConstraints(pattern=r"^sha256:[0-9a-f]{64}$")]
U = Annotated[StrictInt, Field(ge=0, le=2**53 - 1)]
Pos = Annotated[StrictInt, Field(ge=1, le=2**53 - 1)]

QUICKNET_CHAIN_HASH = "52db9ba70e0cc0f6eaf7803dd07447a1f5477735fd3f661792ba94600c84e971"
QUICKNET_GENESIS = 1692803367
QUICKNET_PERIOD = 3


def f32hex(x: float) -> str:
    return struct.pack("<f", x).hex()


def f32val(h: str) -> float:
    return float(struct.unpack("<f", bytes.fromhex(h))[0])


class _M(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True)


def _omit_none(v: object) -> bool:
    return v is None


OD_PRESETS: dict[str, tuple[int, int, int, int, int]] = {
    # preset: (d_model, n_layers, n_heads, head_layers, vocab)
    "od-tiny": (64, 2, 4, 1, 259),
    "od-base": (768, 12, 12, 2, 50432),
    "od-large": (1024, 24, 16, 4, 50432),
}


class ODRecordSpec(_M):
    state_len: Pos
    n_questions: Pos
    n_options: Pos
    opt_len: Pos
    instr_len: Pos

    def record_len(self) -> int:
        """Flat u16 layout: state | instr | opts | qtype | y | teacher_q (A11)."""
        q, k = self.n_questions, self.n_options
        return self.state_len + q * self.instr_len + q * k * self.opt_len + 2 * q + q * (k + 1)


class ODSpec(_M):
    preset: Literal["od-tiny", "od-base", "od-large"]
    head_layers: Pos
    objective: Literal["mlm", "distill", "decision"]
    mask_ratio: F32
    mask_seed: U
    tokenizer_offset: Literal[3]
    warm_start: StrictBool
    record: ODRecordSpec | None
    decision_rule: Literal["log", "brier"]
    rps_weight: F32
    distill_temp: F32


class ModelSpec(_M):
    arch: Literal["decoder", "od-encoder"]
    n_layers: Pos
    d_model: Pos
    n_heads: Pos
    n_kv_heads: Pos
    d_ff: Pos
    n_experts: Pos
    top_k_experts: Pos
    router_tiebreak: Literal["lowest_index"]
    vocab: Pos
    seq_len: Pos
    rope_theta: Pos
    init_seed: U
    param_count: Pos
    compute_dtype: Literal["bf16", "fp32"]
    master_dtype: Literal["fp32"]
    capacity_factor: F32
    aux_loss_coef: F32
    init_std: F32
    od: ODSpec | None = Field(None, exclude_if=_omit_none)

    @model_validator(mode="after")
    def _od_iff_encoder(self) -> ModelSpec:
        if (self.arch == "od-encoder") != (self.od is not None):
            raise ValueError('od must be set iff arch == "od-encoder"')
        return self


class TokenizerSpec(_M):
    name: str
    sha256: Hex64


class DatasetSource(_M):
    mix_id: str
    registry_sha256: Hex64


class DatasetSpec(_M):
    merkle_root: Hex64
    depth: U
    n_samples: Pos
    sample_format: Literal["u32[seq_len+1] token ids", "u16[seq_len+1] token ids"]
    shard_uri_template: str
    shard_sha256_root: Hex64
    holdout_commit: Hex64
    source: DatasetSource | None = Field(None, exclude_if=_omit_none)
    assign_unit: Pos | None = Field(None, exclude_if=_omit_none)
    unit_sha256_root: Hex64 | None = Field(None, exclude_if=_omit_none)


class LrSchedule(_M):
    type: Literal["wsd"]
    peak_lr: F32
    warmup: U
    stable: U
    decay: U


class InnerSpec(_M):
    opt: Literal["adamw", "muon"]
    betas: tuple[F32, F32]
    eps: F32
    wd: F32
    grad_clip: F32
    micro_batch: Pos
    grad_accum: Pos
    H: Pos
    J: Pos
    lr_schedule: LrSchedule
    state_policy: Literal["reset", "derived", "carry"]
    rewarmup_steps: U
    muon_momentum: F32
    ns_steps: Pos

    @model_validator(mode="after")
    def _j_divides_h(self) -> InnerSpec:
        if self.H % self.J:
            raise ValueError("J must divide H (n_leaves = H/J + 1)")
        return self


class OuterSpec(_M):
    opt: Literal["nesterov", "sparseloco"]
    lr: F32
    momentum: F32
    topk_frac: F32
    bits: Pos
    ef_beta: F32
    preclip_norm: F32
    cclip_tau: F32
    cclip_iters: Pos
    center: Literal["prev_outer_update"]


class Timeouts(_M):
    assign_after_open: Pos
    audit_after_commit: Pos
    upload_after_commit: Pos
    serve_deadline: Pos
    dispute_per_level: Pos

    @model_validator(mode="after")
    def _skew(self) -> Timeouts:
        if self.audit_after_commit < 40:
            raise ValueError("d_audit - d_commit must be >= 40 drand rounds (skew margin)")
        return self


def vesting_rounds(q: float) -> int:
    """E = ceil(1/q) computed exactly on the f32 value of q (ultrabrain section 4)."""
    if not 0 < q <= 1:
        raise ValueError("q must be in (0, 1]")
    return math.ceil(Fraction(1) / Fraction(q))


class VerifySpec(_M):
    q_base: F32
    k_segments: U
    Q_top: U
    probation_rounds: Pos
    E_vest_rounds: Pos
    s_min_reward_multiple: Pos
    forgive_per_epoch: U
    influence_cap: F32
    cluster_rules_sha256: Hex64
    T: Timeouts

    @model_validator(mode="after")
    def _deterrence(self) -> VerifySpec:
        e_min = vesting_rounds(f32val(self.q_base))
        if self.E_vest_rounds < e_min:
            raise ValueError(f"E_vest_rounds must be >= ceil(1/q_base) = {e_min}")
        return self

    def s_min_units(self, median_round_reward_units: int) -> int:
        return self.s_min_reward_multiple * median_round_reward_units


def default_verify(
    q_base: float, cluster_rules_sha256: str, T: Timeouts, **overrides: object
) -> VerifySpec:
    """probation_rounds = E = ceil(1/q) and S_min = E x median round reward unless overridden."""
    q = f32hex(q_base)
    e = vesting_rounds(f32val(q))
    fields: dict[str, object] = {
        "q_base": q,
        "k_segments": 3,
        "Q_top": 1,
        "probation_rounds": e,
        "E_vest_rounds": e,
        "s_min_reward_multiple": e,
        "forgive_per_epoch": 1,
        "influence_cap": f32hex(0.25),
        "cluster_rules_sha256": cluster_rules_sha256,
        "T": T,
    }
    fields.update(overrides)
    return VerifySpec.model_validate(fields)


class ReferenceEnv(_M):
    CUBLAS_WORKSPACE_CONFIG: Literal[":4096:8"]
    CUDA_DISABLE_PTX_JIT: Literal["1"]
    cpu_threads: Pos


class Layout(_M):
    """Island layout (todo 11): n_gpus ranks = dp_size data-parallel x ep_size expert-parallel."""

    pp: Pos
    n_gpus: Pos
    dp_size: Pos
    ep_size: Pos
    zero1: StrictBool

    @model_validator(mode="after")
    def _island_shape(self) -> Layout:
        if self.n_gpus != self.dp_size * self.ep_size:
            raise ValueError("n_gpus must equal dp_size * ep_size")
        return self


class ReferenceSpec(_M):
    image_digest: ImageDigest
    spec_doc_sha256: Hex64
    driver_allowlist: list[str]
    sm_count: Pos
    env: ReferenceEnv
    layout: Layout
    profile: Literal["od-bf16-det-eager-v1", "od-fp32-ref-v1"] | None = Field(
        None, exclude_if=_omit_none
    )


class BeaconSpec(_M):
    chain_hash: Hex64
    period: Pos
    genesis_time: Pos


class Budget(_M):
    epochs_per_round: Pos


class ComputeBudget(_M):
    instances: U
    gpus_per_instance: U
    usd_micro_per_round: U


class HoneypotBudget(ComputeBudget):
    rate: F32


class OperatorBudget(_M):
    relay: ComputeBudget
    auditor: ComputeBudget
    honeypot: HoneypotBudget


class RunManifest(_M):
    model: ModelSpec
    tokenizer: TokenizerSpec
    dataset: DatasetSpec
    inner: InnerSpec
    outer: OuterSpec
    verify: VerifySpec
    reference_spec: ReferenceSpec
    init_state_hash: Hex64
    beacon: BeaconSpec
    auditors: list[SS58]
    coord_pubkey: SS58
    budget: Budget
    operator_budget: OperatorBudget

    @model_validator(mode="after")
    def _assign_unit_and_od(self) -> RunManifest:
        ds, inn = self.dataset, self.inner
        if ds.assign_unit is not None:
            u = ds.assign_unit
            if inn.micro_batch * inn.grad_accum * inn.H % u:
                raise ValueError("micro_batch*grad_accum*H must be divisible by assign_unit")
            if ds.n_samples % u:
                raise ValueError("n_samples must be divisible by assign_unit")
            if ds.unit_sha256_root is None:
                raise ValueError("assign_unit requires unit_sha256_root")
        elif ds.unit_sha256_root is not None:
            raise ValueError("unit_sha256_root requires assign_unit")
        if self.model.arch == "od-encoder":
            _check_od(self)
        elif self.reference_spec.profile is not None:
            raise ValueError("profile is only valid for arch == od-encoder")
        return self

    def body(self) -> dict[str, object]:
        return self.model_dump(mode="json")

    def run_id(self) -> str:
        return sha256_hex(canonicalize(self.body(), allow_float=False))

    def drand_round_at(self, epoch_at: int) -> int:
        return drand_round_at(epoch_at, self.beacon.genesis_time, self.beacon.period)


def _check_od(m: RunManifest) -> None:
    """OD rules (design B2 check_manifest, A6/A11/A16/A17); torch-free by design."""
    ms, od, lay = m.model, m.model.od, m.reference_spec.layout
    prof = m.reference_spec.profile
    assert od is not None
    if (lay.pp, lay.n_gpus, lay.dp_size, lay.ep_size, lay.zero1) != (1, 1, 1, 1, False):
        raise ValueError("OD layout must be pp=1,n_gpus=1,dp_size=1,ep_size=1,zero1=false")
    if ms.n_kv_heads != ms.n_heads:
        raise ValueError("OD requires n_kv_heads == n_heads")
    if ms.d_ff != 4 * ms.d_model:
        raise ValueError("OD requires d_ff == 4*d_model")
    if ms.n_experts != 1 or ms.top_k_experts != 1:
        raise ValueError("OD requires n_experts == top_k_experts == 1")
    if f32val(ms.capacity_factor) != 1.0:
        raise ValueError("OD requires capacity_factor == 1.0")
    if f32val(ms.aux_loss_coef) != 0.0:
        raise ValueError("OD requires aux_loss_coef == 0")
    if prof is None:
        raise ValueError("OD requires reference_spec.profile")
    if ms.compute_dtype != ("fp32" if prof == "od-fp32-ref-v1" else "bf16"):
        raise ValueError("compute_dtype does not match reference_spec.profile")
    d, n_layers, n_heads, head_layers, vocab = OD_PRESETS[od.preset]
    if (ms.d_model, ms.n_layers, ms.n_heads, ms.vocab) != (d, n_layers, n_heads, vocab):
        raise ValueError(f"model dims do not match preset {od.preset}")
    if od.head_layers != head_layers:
        raise ValueError(f"head_layers must be {head_layers} for preset {od.preset}")
    if (od.record is None) != (od.objective == "mlm"):
        raise ValueError("od.record must be set iff objective != mlm")
    if od.record is not None and ms.seq_len != od.record.record_len() - 1:
        raise ValueError("seq_len must equal record length - 1")


def drand_round_at(epoch_at: int, genesis: int, period: int = QUICKNET_PERIOD) -> int:
    """Latest drand round emitted at chain time epoch_at: (epoch_at - genesis)//period + 1."""
    if epoch_at < genesis:
        raise ValueError("epoch_at precedes beacon genesis")
    return (epoch_at - genesis) // period + 1


class RosterEntry(_M):
    hotkey: SS58
    slot: U
    q_i: F32


class RoundOpen(_M):
    w: U
    prev_final_hash: Hex64
    theta_hash: Hex64
    outer_state_hash: Hex64
    center_hash: Hex64
    roster: list[RosterEntry]
    roster_hash: Hex64
    honeypot_commit: Hex64
    d_open: Pos
    d_assign: Pos
    d_commit: Pos
    d_audit: Pos
    d_upload: Pos
    d_final: Pos

    @model_validator(mode="after")
    def _order(self) -> RoundOpen:
        chain_ok = self.d_open < self.d_assign < self.d_commit < self.d_audit < self.d_final
        upload_ok = self.d_commit < self.d_upload <= self.d_final
        if not (chain_ok and upload_ok):
            raise ValueError(
                "deadlines must satisfy d_open < d_assign < d_commit < d_audit < d_final "
                "and d_commit < d_upload <= d_final"
            )
        return self


class Accept(_M):
    w: U
    hotkey: SS58
    assignment_hash: Hex64
    image_digest: ImageDigest
    driver_version: str
    n_gpus: Pos


class Commit(_M):
    w: U
    hotkey: SS58
    leaf_scheme: Literal["ht-leaf-v1"]
    n_leaves: Pos
    leaves_root: Hex64
    metrics_root: Hex64
    final_theta_hash: Hex64
    ef_in_hash: Hex64
    ef_out_hash: Hex64
    delta_hash: Hex64
    delta_bytes: U
    tokens: U


class Receipt(_M):
    w: U
    commit_hash: Hex64
    received_round: Pos


class StageState(_M):
    theta: Hex64
    m: Hex64
    v: Hex64


class LeafPreimage(_M):
    run_id: Hex64
    w: U
    t: U
    stages: Annotated[list[StageState], Field(min_length=1)]
    batch_ids_sha256: Hex64
    rng_ctr: U
    loss_f32: F32
    norm_f32: F32

    def stage_root(self) -> bytes:
        leaves = [bytes.fromhex(s.theta + s.m + s.v) for s in self.stages]
        return MerkleTree(leaves).root

    def digest(self) -> str:
        """leaf_t: fixed-width 'ht-leaf-v1'|run_id|w u64|t u64|stage_root|batch|rng u64|f32|f32."""
        return sha256_hex(
            b"ht-leaf-v1"
            + bytes.fromhex(self.run_id)
            + struct.pack("<QQ", self.w, self.t)
            + self.stage_root()
            + bytes.fromhex(self.batch_ids_sha256)
            + struct.pack("<Q", self.rng_ctr)
            + bytes.fromhex(self.loss_f32 + self.norm_f32)
        )


class Chunk(_M):
    off: U
    len: Pos
    sha256: Hex64


class DeltaManifest(_M):
    w: U
    hotkey: SS58
    delta_hash: Hex64
    uri: str
    size: U
    format: Literal["ht-sparse-v1", "ht-dense-int8-v1"]
    chunks: list[Chunk]

    @model_validator(mode="after")
    def _contiguous(self) -> DeltaManifest:
        pos = 0
        for c in self.chunks:
            if c.off != pos:
                raise ValueError("chunks must be contiguous from offset 0")
            pos += c.len
        if pos != self.size:
            raise ValueError("chunk lengths must sum to size")
        return self


AuditReason = Literal["random", "final", "topQ", "probation", "outlier", "cluster", "honeypot"]


class AuditChallenge(_M):
    w: U
    target: SS58
    beacon_round: Pos
    beacon_sig_sha256: Hex64
    mode: Literal["full", "segments"]
    segments: list[tuple[U, U]]
    reasons: Annotated[list[AuditReason], Field(min_length=1)]
    serve_deadline: Pos


class StateServe(_M):
    hotkey: SS58
    challenge_hash: Hex64
    t: U
    uri: str
    tensor_root: Hex64
    merkle_proof_leaf_in_leaves_root: list[Hex64]


class ReplayEnv(_M):
    image_digest: ImageDigest
    driver: str
    gpu_uuid_sha256: Hex64
    sm_count: Pos


class ReplayVerdict(_M):
    challenge_hash: Hex64
    first_bad_leaf: U | None
    result: Literal["MATCH", "MISMATCH", "WITHHELD", "BAD_PROOF", "ASSIGNMENT_VIOLATION"]
    recomputed_leaves_root: Hex64
    replay_env: ReplayEnv


class Dispute(_M):
    hotkey: SS58
    verdict_hash: Hex64
    action: Literal["accept", "contest"]


class Bisect(_M):
    dispute_id: Hex64
    level: Literal["step", "layer", "op"]
    interval: tuple[U, U]
    N: Annotated[StrictInt, Field(ge=2, le=1024)]
    hashes: list[Hex64]
    party: SS58

    @model_validator(mode="after")
    def _n_plus_one(self) -> Bisect:
        if len(self.hashes) != self.N + 1:
            raise ValueError("hashes must have N+1 entries")
        if self.interval[0] >= self.interval[1]:
            raise ValueError("interval must be [a, b] with a < b")
        return self


class Resolution(_M):
    dispute_id: Hex64
    op_spec: str
    inputs_hash: Hex64
    output_hash: Hex64
    loser: SS58


class Forfeit(_M):
    w: U
    hotkey: SS58
    cause: Literal[
        "MISMATCH", "WITHHELD", "DISPUTE_LOST", "ASSIGNMENT_VIOLATION", "TRANSIENT", "NO_UPLOAD"
    ]
    evidence: list[Hex64]
    round_reward_burned: U
    escrow_burned_units: U
    blacklist: StrictBool
    debt_units: U


class Rollback(_M):
    w: U
    excluded: Annotated[list[SS58], Field(min_length=1)]
    old_theta_hash_w2: Hex64
    new_theta_hash_w2: Hex64
    new_outer_state_hash: Hex64
    recomputed: list[Literal["agg_w", "step_w", "agg_w1", "step_w1"]]
    cause_hashes: list[Hex64]


class Finalize(_M):
    w: U
    final_theta_hash_w1: Hex64
    included: list[SS58]
    entitlements_root: Hex64


MESSAGE_TYPES: dict[str, type[_M]] = {
    "RunManifest": RunManifest,
    "RoundOpen": RoundOpen,
    "Accept": Accept,
    "Commit": Commit,
    "Receipt": Receipt,
    "DeltaManifest": DeltaManifest,
    "AuditChallenge": AuditChallenge,
    "StateServe": StateServe,
    "ReplayVerdict": ReplayVerdict,
    "Dispute": Dispute,
    "Bisect": Bisect,
    "Resolution": Resolution,
    "Forfeit": Forfeit,
    "Rollback": Rollback,
    "Finalize": Finalize,
}

SCHEMA_MODELS: dict[str, type[_M]] = {**MESSAGE_TYPES, "LeafPreimage": LeafPreimage}
