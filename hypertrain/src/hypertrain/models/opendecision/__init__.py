"""OpenDecision v1 adapter: the trainer's architecture interface over the OD encoder (design B2).

`forward` (training) and `traced_forward` (dispute replay) are both the one eager function `_run`,
so a miner's committed leaves are reproducible op by op (A16). Stage A is MLM over packed rows with
static deterministic masking (A1, A2); stages B/C read the fixed-shape decision record (A11).
"""

from __future__ import annotations

import hypertrain.trainer  # noqa: F401  (pins determinism before torch and opendecision load)

# isort: split
import math
import threading
from collections.abc import Callable, Sequence
from functools import lru_cache
from typing import Any

import numpy as np
import torch
import torch.nn.functional as F
from torch import Tensor, nn
from torch.nn.attention import SDPBackend, sdpa_kernel

from hypertrain.protocol.messages import ODSpec, RunManifest, f32val
from hypertrain.protocol.messages_v2 import RunManifestV2
from hypertrain.trainer.config import ModelConfig, TrainConfig
from hypertrain.trainer.loop import SampleFn
from hypertrain.trainer.model import Params, embed, permute_rows
from hypertrain.trainer.rng import rng_ctr, uniform_f64

try:
    from opendecision.masking import mask_positions, n_mask_for
    from opendecision.model import ModelConfig as ODConfig
    from opendecision.model import OpenDecisionModel, apply_rope, param_names, rope_table
    from opendecision.presets import PRESETS, STAGE_A_PREFIXES
    from opendecision.records import RecordShape, qmax_for, unpack_records
    from opendecision.tokenizer import MASK
    from opendecision.train import decision_loss, distill_loss
except ImportError as e:
    raise ImportError("set PYTHONPATH to opendecision-arch-20261008/src") from e

Hook = Callable[[int, str, Tensor], Tensor]
PROFILE_DTYPE = {"od-bf16-det-eager-v1": "bf16", "od-fp32-ref-v1": "fp32"}
_NORM_WEIGHTS = (".qn.weight", ".kn.weight", ".n1.weight", ".n2.weight", ".n3.weight")
_SDPA_LOCK = threading.RLock()


def _identity(layer: int, op: str, x: Tensor) -> Tensor:
    return x


def _od(cfg: ModelConfig) -> ODSpec:
    if cfg.arch != "od-encoder" or cfg.od is None:
        raise ValueError("not an OpenDecision model config")
    return cfg.od


@lru_cache(maxsize=8)
def od_config(cfg: ModelConfig) -> Any:
    """The opendecision ModelConfig for a hypertrain ModelConfig (dims from the signed manifest)."""
    od = _od(cfg)
    return ODConfig(
        d=cfg.d_model,
        layers=cfg.n_layers,
        heads=cfg.n_heads,
        head_layers=od.head_layers,
        max_len=PRESETS[od.preset].max_len,
        vocab=cfg.vocab,
        rope_theta=float(cfg.rope_theta),
        chunk=0,
    )


def check_manifest(m: RunManifest | RunManifestV2) -> None:
    """Adapter-side rules on top of the L2 RunManifest validator (which already ran on ``m``)."""
    match m:
        case RunManifestV2():
            cfg = TrainConfig.from_manifest_v2(m).model
            ms = m.training.model
        case RunManifest():
            cfg = TrainConfig.from_manifest(m.body()).model
            ms = m.model
    od = ms.od
    if ms.arch != "od-encoder" or od is None:
        raise ValueError("not an OpenDecision manifest")
    want = sum(math.prod(s) for s in param_shapes(cfg).values())
    if ms.param_count != want:
        raise ValueError(f"param_count {ms.param_count} != {want} trainable for {od.objective}")
    n = PRESETS[od.preset].max_len
    # A6 pins N = max_len (1024) for real presets; od-tiny (tests only) may use any N <= max_len.
    tiny_ok = od.preset == "od-tiny" and ms.seq_len <= n
    if od.objective == "mlm" and ms.seq_len != n and not tiny_ok:
        raise ValueError(f"mlm seq_len must be {n} for {od.preset} (A6)")


def param_shapes(cfg: ModelConfig) -> dict[str, tuple[int, ...]]:
    """OD state_dict names; stage A (objective mlm) trains tok.* and encoder.* only (A4)."""
    full: dict[str, tuple[int, ...]] = dict(param_names(od_config(cfg)))
    if _od(cfg).objective == "mlm":
        return {n: s for n, s in full.items() if n.startswith(STAGE_A_PREFIXES)}
    return full


def stage_of(name: str, cfg: ModelConfig, n_stages: int) -> int:
    if name.startswith("tok."):
        return 0
    if name.startswith("encoder.blocks."):
        return int(name.split(".")[2]) * n_stages // cfg.n_layers
    return n_stages - 1


def _init(cfg: ModelConfig, names: set[str]) -> Params:
    """Philox uniform(+-std*sqrt(3)) keyed by the index in the FULL sorted name list, so a head
    tensor gets the same init whether it is built at stage B or by ``extend_params``."""
    a = cfg.init_std * math.sqrt(3.0)
    out: Params = {}
    for idx, (name, shape) in enumerate(sorted(param_names(od_config(cfg)).items())):
        if name not in names:
            continue
        if name.endswith(_NORM_WEIGHTS) or name in ("encoder.norm.weight", "scorer.0.weight"):
            out[name] = torch.ones(shape, dtype=torch.float32)
        elif name.endswith(".bias") or name.startswith("log_temp."):
            out[name] = torch.zeros(shape, dtype=torch.float32)
        else:
            u = uniform_f64(math.prod(shape), rng_ctr(f"od-init:{cfg.init_seed}", 0, 0, idx))
            out[name] = torch.from_numpy((u * 2.0 - 1.0) * a).to(torch.float32).reshape(shape)
    return out


def init_params(cfg: ModelConfig) -> Params:
    return _init(cfg, set(param_shapes(cfg)))


def extend_params(theta: Params, cfg: ModelConfig) -> Params:
    """A19 warm start: keep every tensor of ``theta``, add the missing names of ``cfg`` (init)."""
    want = param_shapes(cfg)
    if extra := sorted(set(theta) - set(want)):
        raise ValueError(f"theta has names outside the target model: {extra[:3]}")
    for n, x in theta.items():
        if tuple(x.shape) != tuple(want[n]) or x.dtype != torch.float32:
            raise ValueError(f"{n}: expected fp32 {want[n]}, got {x.dtype} {tuple(x.shape)}")
    dev = next(iter(theta.values())).device if theta else torch.device("cpu")
    new = _init(cfg, set(want) - set(theta))
    return {n: theta[n] if n in theta else new[n].to(dev) for n in sorted(want)}


def take_rows(flat: Tensor, rows: Tensor) -> Tensor:
    """flat[rows] through a FULL permutation (rows first, then the sorted complement), so the
    backward is ``_Permute`` (index_select), never index_put/scatter_add (review defect 7)."""
    n = flat.shape[0]
    r = rows.detach().to("cpu", torch.int64).reshape(-1)
    rest = torch.ones(n, dtype=torch.bool, device="cpu")
    rest[r] = False
    perm = torch.cat([r, torch.arange(n, device="cpu")[rest]])
    if perm.numel() != n:
        raise ValueError("rows must be distinct")
    return permute_rows(flat, perm.to(flat.device))[: r.numel()]


def _det_gather(h: Tensor, positions: Tensor) -> Tensor:
    """h [B,N,d], positions [B,n_mask] -> rows b*N + positions[b,k] in (b, k) order."""
    b, n = h.shape[0], h.shape[1]
    rows = torch.arange(b, device=positions.device)[:, None] * n + positions
    return take_rows(h.reshape(b * n, h.shape[-1]), rows)


@lru_cache(maxsize=16)
def _rope_cpu(n: int, head_dim: int, theta: float) -> tuple[Tensor, Tensor]:
    with torch.device("cpu"):
        cos, sin = rope_table(n, head_dim, theta)
    return cos, sin


def _ln(x: Tensor, p: Params, pre: str) -> Tensor:
    return F.layer_norm(x, (x.shape[-1],), p[pre + ".weight"], p[pre + ".bias"])


def _blocks(m: ModelConfig, p: Params, x: Tensor, mask: Tensor | None, hook: Hook) -> Tensor:
    """Op-for-op copy of opendecision ``Block.forward`` over the pinned names, with op hooks."""
    b, n, d = x.shape
    h, hd = m.n_heads, m.d_model // m.n_heads
    cos, sin = (v.to(x.device) for v in _rope_cpu(n, hd, float(m.rope_theta)))
    am = None if mask is None else mask[:, None, None, :]
    for i in range(m.n_layers):
        pre = f"encoder.blocks.{i}"
        y = hook(i, "attn_norm", _ln(x, p, pre + ".n1"))
        q, k, v = F.linear(y, p[pre + ".qkv.weight"]).view(b, n, 3, h, hd).permute(2, 0, 3, 1, 4)
        q = apply_rope(_ln(q, p, pre + ".qn"), cos, sin)
        k = apply_rope(_ln(k, p, pre + ".kn"), cos, sin)
        a = F.scaled_dot_product_attention(q, k, v, attn_mask=am)
        a = hook(i, "attn", F.linear(a.transpose(1, 2).reshape(b, n, d), p[pre + ".o.weight"]))
        x = hook(i, "attn_residual", x + a)
        y = hook(i, "mlp_norm", _ln(x, p, pre + ".n2"))
        f = F.gelu(F.linear(y, p[pre + ".ff.0.weight"], p[pre + ".ff.0.bias"]))
        f = hook(i, "mlp", F.linear(f, p[pre + ".ff.2.weight"], p[pre + ".ff.2.bias"]))
        x = hook(i, "mlp_residual", x + f)
    return x


def _mlm(m: ModelConfig, od: ODSpec, p: Params, tokens: Tensor, hook: Hook) -> Tensor:
    # A6: only tokens[:, :seq_len] are used; the +1 column of u*[seq_len+1] is ignored.
    # Mask, masked input and targets are built on the host (numpy), then moved: no device scatter.
    ids = tokens[:, : m.seq_len].cpu().numpy()
    pos = mask_positions(ids, od.mask_seed, n_mask_for(m.seq_len, f32val(od.mask_ratio)))
    tgt = np.take_along_axis(ids, pos, 1).reshape(-1)
    inp = ids.copy()
    np.put_along_axis(inp, pos, MASK, 1)
    dev = tokens.device
    pos_t = torch.from_numpy(pos).to(dev)
    x = hook(0, "embed", embed(p["tok.weight"], torch.from_numpy(inp).to(dev)))
    x = _blocks(m, p, x, None, hook)
    L = m.n_layers
    z = hook(L, "final_norm", _ln(x, p, "encoder.norm"))
    logits = hook(L, "head", (_det_gather(z, pos_t) @ p["tok.weight"].T).float())
    return hook(L, "loss", F.cross_entropy(logits, torch.from_numpy(tgt).to(dev)))


class _Decide(nn.Module):
    """Calls ``decide`` of a meta-device OpenDecisionModel under functional_call."""

    def __init__(self, model: Any) -> None:
        super().__init__()
        self.m = model

    def forward(self, *args: Tensor) -> Any:
        return self.m.decide(*args)


_DECIDERS = threading.local()


def _decider(cfg: ModelConfig) -> _Decide:
    # functional_call temporarily swaps module tensors: never share its module across ranks.
    if not hasattr(_DECIDERS, "models"):
        _DECIDERS.models = {}
    if cfg in _DECIDERS.models:
        return _DECIDERS.models[cfg]
    with torch.device("meta"):
        module = _Decide(OpenDecisionModel(od_config(cfg), embed_fn=embed))
    _DECIDERS.models[cfg] = module
    return module


def _decision(m: ModelConfig, od: ODSpec, p: Params, tokens: Tensor, hook: Hook) -> Tensor:
    assert od.record is not None
    d = unpack_records(tokens, RecordShape(**od.record.model_dump()), qmax_for(m.vocab))
    x = hook(0, "embed", embed(p["tok.weight"], d["ids"]))
    x = _blocks(m, p, x, d["mask"], hook)
    L = m.n_layers
    z = hook(L, "final_norm", _ln(x, p, "encoder.norm"))
    args = (z, d["mask"], d["opt_ids"], d["opt_mask"], d["instr_ids"], d["qtype"])
    cos, sin = _rope_cpu(PRESETS[od.preset].max_len, m.d_model // m.n_heads, float(m.rope_theta))
    tensors = {f"m.{k}": v for k, v in p.items()} | {
        "m.rope_cos": cos.to(tokens.device),
        "m.rope_sin": sin.to(tokens.device),
    }
    logits, ext = torch.func.functional_call(_decider(m), tensors, args)
    logits = hook(L, "head", logits)
    if od.objective == "distill":
        loss = distill_loss(logits, ext, d["teacher"], f32val(od.distill_temp))
    else:
        loss = decision_loss(
            logits, ext, d["y"], d["qtype"], od.decision_rule, f32val(od.rps_weight)
        )
    return hook(L, "loss", loss)


def _run(m: ModelConfig, p: Params, tokens: Tensor, hook: Hook) -> tuple[Tensor, Tensor]:
    """The one eager implementation (A16): profile kernels, then the objective's loss."""
    od = _od(m)
    dt = PROFILE_DTYPE.get(m.profile)
    if dt is None:
        raise ValueError(f"OD profile {m.profile!r} is not accepted in v1")
    if dt != m.compute_dtype:
        raise ValueError(f"compute_dtype {m.compute_dtype} does not match profile {m.profile}")
    hook(-1, "input", tokens)
    mlm = od.objective == "mlm"
    # FLASH only on the unmasked stage A path of the bf16 profile; masked B/C and fp32-ref: MATH.
    backend = SDPBackend.FLASH_ATTENTION if mlm and dt == "bf16" else SDPBackend.MATH
    with (
        _SDPA_LOCK,
        sdpa_kernel([backend]),
        torch.autocast(tokens.device.type, dtype=torch.bfloat16, enabled=dt == "bf16"),
    ):
        loss = (_mlm if mlm else _decision)(m, od, p, tokens, hook)
    return loss.float(), torch.zeros((), dtype=torch.float32, device=tokens.device)


def forward(
    cfg: ModelConfig,
    params: Params,
    tokens: Tensor,
    moe: object | None = None,
    hook: Hook | None = None,
) -> tuple[Tensor, Tensor]:
    """Same signature as ``trainer.model.forward``; OD is dense, so ``moe`` must be None."""
    if moe is not None:
        raise ValueError("OpenDecision is dense: moe must be None")
    return _run(cfg, params, tokens, hook or _identity)


def traced_forward(
    cfg: TrainConfig, p: Params, tokens: Tensor, hook: Hook
) -> tuple[Tensor, Tensor]:
    """Signature of ``auditor.bisect.traced_forward``; same ops/names (OPS, TAIL_OPS[:3])."""
    return _run(cfg.model, p, tokens, hook)


def _rows(cfg: ModelConfig, get_sample: SampleFn, ids: Sequence[int]) -> Tensor:
    out = []
    for i in ids:
        s = np.asarray(get_sample(i))
        if s.dtype != np.uint32 or s.shape != (cfg.seq_len + 1,):
            raise ValueError(f"sample {i} must be u32[seq_len+1]")
        if int(s.max()) >= cfg.vocab:
            raise ValueError(f"sample {i} has token id >= vocab")
        out.append(s.astype(np.int64))
    return torch.from_numpy(np.stack(out))


def eval_holdout(
    cfg: ModelConfig, theta: Params, get_sample: SampleFn, ids: Sequence[int], batch: int = 8
) -> float:
    """Mean objective loss over ``ids`` (sample-weighted over fixed batches), no grad."""
    if not len(ids):
        raise ValueError("no holdout samples")
    dev = next(iter(theta.values())).device
    total = 0.0
    with torch.no_grad():
        for k in range(0, len(ids), batch):
            chunk = ids[k : k + batch]
            ce, _ = forward(cfg, theta, _rows(cfg, get_sample, chunk).to(dev))
            total += float(ce) * len(chunk)
    return total / len(ids)
