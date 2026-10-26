"""FLOPs/token and MFU accounting (pure Python; no torch import).

FLOPs/token = 6 * N_active + 12 * L * d * T (PaLM appendix B convention, no causal halving).
N_active counts every parameter a token touches: embedding, norms, attention, head, dense MLP,
the router, and top_k / n_experts of the expert weights. This reproduces the audit's 201.9M active
parameters for the Phase B profile.
Island MFU = tokens/s * FLOPs/token / (n_gpus * per-GPU peak).
"""

from __future__ import annotations

from hypertrain.trainer.config import ModelConfig

# ASSUMED: vendor dense BF16 tensor throughput for one RTX 5090 (no sparsity). Always report the
# measured GEMM peak (experiments/mfu/peak_matmul.py) next to it.
MFU_PEAK_RTX5090_BF16 = 209.5e12


def active_params(cfg: ModelConfig) -> int:
    """Parameters touched per token (decoder only; the shapes follow model.param_shapes)."""
    if cfg.arch != "decoder":
        raise ValueError(f"no FLOPs model for arch {cfg.arch!r}")
    d, v = cfg.d_model, cfg.vocab
    kv = cfg.n_kv_heads * (d // cfg.n_heads)
    layer = 2 * d + 2 * d * d + 2 * d * kv
    if cfg.is_moe:
        layer += d * cfg.n_experts + cfg.top_k_experts * 3 * d * cfg.d_ff
    else:
        layer += 3 * d * cfg.d_ff
    return 2 * v * d + d + cfg.n_layers * layer


def flops_per_token(cfg: ModelConfig) -> int:
    return 6 * active_params(cfg) + 12 * cfg.n_layers * cfg.d_model * cfg.seq_len


def mfu(tokens_per_s: float, flops_token: int, peak_flops: float, n_gpus: int = 1) -> float:
    """Model FLOPs utilisation in [0, inf); callers report it as a fraction, not a percent."""
    if not (tokens_per_s > 0 and flops_token > 0 and peak_flops > 0 and n_gpus >= 1):
        raise ValueError("tokens_per_s, flops_token, peak_flops must be > 0 and n_gpus >= 1")
    return tokens_per_s * flops_token / (n_gpus * peak_flops)
