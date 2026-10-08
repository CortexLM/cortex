"""Trainer configuration derived from a validated protocol RunManifest body."""

from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass
from typing import Any

from hypertrain.protocol.messages import RunManifest, f32val

DTYPES = ("fp32", "bf16")
STATE_POLICIES = ("reset", "derived", "carry")
INNER_OPTS = ("adamw", "muon")
CODECS = ("dense-int8", "sparseloco")


@dataclass(frozen=True)
class ModelConfig:
    n_layers: int
    d_model: int
    n_heads: int
    n_kv_heads: int
    d_ff: int
    vocab: int
    seq_len: int
    n_experts: int = 0
    top_k_experts: int = 1
    capacity_factor: float = 1.25
    rope_theta: float = 10000.0
    init_seed: int = 0
    init_std: float = 0.02
    aux_loss_coef: float = 0.01
    compute_dtype: str = "fp32"

    @property
    def is_moe(self) -> bool:
        return self.n_experts > 1

    def __post_init__(self) -> None:
        for name in ("n_layers", "d_model", "n_heads", "n_kv_heads", "d_ff", "vocab", "seq_len"):
            if getattr(self, name) < 1:
                raise ValueError(f"{name} must be >= 1")
        if self.d_model % self.n_heads or (self.d_model // self.n_heads) % 2:
            raise ValueError("d_model must split into even-sized heads")
        if self.n_heads % self.n_kv_heads:
            raise ValueError("n_heads must be a multiple of n_kv_heads")
        if self.n_experts < 0 or (self.is_moe and not 1 <= self.top_k_experts <= self.n_experts):
            raise ValueError("invalid expert configuration")
        if self.aux_loss_coef < 0:
            raise ValueError("aux_loss_coef must be >= 0")
        if self.capacity_factor <= 0 or self.init_std <= 0 or self.rope_theta <= 0:
            raise ValueError("capacity_factor, init_std, rope_theta must be > 0")
        if self.compute_dtype not in DTYPES:
            raise ValueError(f"compute_dtype must be one of {DTYPES}")


@dataclass(frozen=True)
class InnerConfig:
    lr: float
    H: int
    J: int
    micro_batch: int
    opt: str = "adamw"
    beta1: float = 0.9
    beta2: float = 0.95
    eps: float = 1e-8
    wd: float = 0.1
    grad_clip: float = 1.0
    grad_accum: int = 1
    warmup: int = 0
    stable: int = 1 << 62
    decay: int = 0
    state_policy: str = "reset"
    rewarmup_steps: int = 0
    muon_momentum: float = 0.95
    ns_steps: int = 5

    def __post_init__(self) -> None:
        if self.opt not in INNER_OPTS:
            raise ValueError(f"inner.opt must be one of {INNER_OPTS}")
        if self.state_policy not in STATE_POLICIES:
            raise ValueError(f"state_policy must be one of {STATE_POLICIES}")
        if self.H < 1 or self.J < 1 or self.H % self.J:
            raise ValueError("H and J must be >= 1 and J must divide H")
        if self.micro_batch < 1 or self.grad_accum < 1 or self.ns_steps < 1:
            raise ValueError("micro_batch, grad_accum, ns_steps must be >= 1")
        if min(self.warmup, self.stable, self.decay, self.rewarmup_steps) < 0:
            raise ValueError("schedule lengths must be >= 0")
        if not (self.lr > 0 and 0 <= self.beta1 < 1 and 0 <= self.beta2 < 1 and self.eps > 0):
            raise ValueError("invalid optimizer hyperparameters")
        if self.wd < 0 or self.grad_clip <= 0 or not 0 <= self.muon_momentum < 1:
            raise ValueError("invalid wd / grad_clip / muon_momentum")

    @property
    def n_leaves(self) -> int:
        return self.H // self.J + 1


@dataclass(frozen=True)
class CompressConfig:
    codec: str = "dense-int8"
    topk_frac: float = 1.0
    bits: int = 8
    ef_beta: float = 1.0

    def __post_init__(self) -> None:
        if self.codec not in CODECS:
            raise ValueError(f"codec must be one of {CODECS}")
        if self.codec == "dense-int8" and self.bits != 8:
            raise ValueError("dense-int8 requires bits=8")
        if self.codec == "sparseloco" and (self.bits != 2 or not 0 < self.topk_frac <= 1):
            raise ValueError("sparseloco requires bits=2 and 0 < topk_frac <= 1")
        if not 0 <= self.ef_beta <= 1:
            raise ValueError("ef_beta must be in [0, 1]")


@dataclass(frozen=True)
class TrainConfig:
    model: ModelConfig
    inner: InnerConfig
    compress: CompressConfig
    n_stages: int = 1
    cpu_threads: int = 1

    def __post_init__(self) -> None:
        if not 1 <= self.n_stages <= self.model.n_layers:
            raise ValueError("n_stages must be in [1, n_layers]")
        if self.cpu_threads < 1:
            raise ValueError("cpu_threads must be >= 1")

    @classmethod
    def from_manifest(cls, body: Mapping[str, Any]) -> TrainConfig:
        """Validate ``body`` as a protocol RunManifest (strict, f32-hex reals) and convert."""
        m = RunManifest.model_validate(body)
        ms, inn, out, ref = m.model, m.inner, m.outer, m.reference_spec
        model = ModelConfig(
            n_layers=ms.n_layers,
            d_model=ms.d_model,
            n_heads=ms.n_heads,
            n_kv_heads=ms.n_kv_heads,
            d_ff=ms.d_ff,
            vocab=ms.vocab,
            seq_len=ms.seq_len,
            n_experts=ms.n_experts,
            top_k_experts=ms.top_k_experts,
            capacity_factor=f32val(ms.capacity_factor),
            rope_theta=float(ms.rope_theta),
            init_seed=ms.init_seed,
            init_std=f32val(ms.init_std),
            aux_loss_coef=f32val(ms.aux_loss_coef),
            compute_dtype=ms.compute_dtype,
        )
        sched = inn.lr_schedule
        inner = InnerConfig(
            lr=f32val(sched.peak_lr),
            H=inn.H,
            J=inn.J,
            micro_batch=inn.micro_batch,
            opt=inn.opt,
            beta1=f32val(inn.betas[0]),
            beta2=f32val(inn.betas[1]),
            eps=f32val(inn.eps),
            wd=f32val(inn.wd),
            grad_clip=f32val(inn.grad_clip),
            grad_accum=inn.grad_accum,
            warmup=sched.warmup,
            stable=sched.stable,
            decay=sched.decay,
            state_policy=inn.state_policy,
            rewarmup_steps=inn.rewarmup_steps,
            muon_momentum=f32val(inn.muon_momentum),
            ns_steps=inn.ns_steps,
        )
        compress = CompressConfig(
            codec="sparseloco" if out.opt == "sparseloco" else "dense-int8",
            topk_frac=f32val(out.topk_frac),
            bits=out.bits,
            ef_beta=f32val(out.ef_beta),
        )
        return cls(
            model=model,
            inner=inner,
            compress=compress,
            n_stages=ref.layout.pp,
            cpu_threads=ref.env.cpu_threads,
        )
