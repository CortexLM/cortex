"""Example CPU RunManifest; values trace to advisories (never test fixtures), re-tune at todo 13."""

from __future__ import annotations

from typing import Any

from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages import (
    QUICKNET_CHAIN_HASH,
    QUICKNET_GENESIS,
    QUICKNET_PERIOD,
    RunManifest,
    Timeouts,
    default_verify,
    f32hex,
)

PROVENANCE: dict[str, str] = {
    "verify.q_base=0.1": "ultrabrain section 3a default q",
    "verify.E/probation/S_min=ceil(1/q)=10": "ultrabrain section 4 E=ceil(1/q)",
    "verify.forgive_per_epoch=1": "ultrabrain section 4 (verif SYNTH section 6)",
    "verify.T": "ultrabrain section 1.2 timeouts in drand rounds (3 s): 20/40/300/900/600",
    "inner.H=30,J=5": "plan todo 16 Phase B round shape; ultrabrain section 1.1 n_leaves=H/J+1",
    "inner.state_policy=carry": (
        "todo 7 decision (experiments/results/decision.json: A1/A2/A3 FAIL) -> carry + segment "
        "audits; switched from reset by todo 13"
    ),
    "inner.rewarmup_steps=0": "carry has no per-round re-warmup (segment replay requires 0)",
    "outer.lr=0.4,momentum=0.9": "parity section 2 starting grid eta_g in {0.4..1.0}, mom 0.9",
    "outer.center=prev_outer_update": "ultrabrain section 3c",
    "operator_budget.honeypot.rate=0.03": "ultrabrain section 4 honeypots 1-5%",
    "model.vocab=259": "plan todo 4 byte-level 259-token CPU tokenizer",
    "UNSOURCED (provisional, todo 13)": "inner betas/eps/wd/clip/lr, preclip/cclip, model dims",
    "UNSOURCED verify (provisional, todo 13)": "k_segments=3, Q_top=1, influence_cap=0.25",
    "UNSOURCED outer (provisional, todo 13)": "topk_frac=1.0, bits=8, ef_beta=0.0",
    "UNSOURCED inner (provisional, todo 13)": "lr warmup/stable/decay, micro_batch=8",
    "UNSOURCED model (provisional, todo 13)": (
        "capacity_factor=1.25, aux_loss_coef=0.01, init_std=0.02"
    ),
    "UNSOURCED inner muon (provisional, todo 13)": "muon_momentum=0.95, ns_steps=5",
    "reference_spec.env.cpu_threads=1, layout.pp=1": "todo 6 CPU determinism pin; provisional",
    "reference_spec.layout n_gpus=dp_size=ep_size=1, zero1=false": (
        "todo 11 single-rank CPU island; GPU runs pin their own layout"
    ),
}

_EXAMPLE_SEED = bytes(range(32))


def _h(label: str) -> str:
    return sha256_hex(b"hypertrain-example|" + label.encode())


def example_manifest() -> RunManifest:
    coord = Keypair(_EXAMPLE_SEED).ss58
    auditor = Keypair(bytes(31) + b"\x01").ss58
    body: dict[str, Any] = {
        "model": {
            "arch": "decoder",
            "n_layers": 4,
            "d_model": 256,
            "n_heads": 4,
            "n_kv_heads": 4,
            "d_ff": 1024,
            "n_experts": 1,
            "top_k_experts": 1,
            "router_tiebreak": "lowest_index",
            "vocab": 259,
            "seq_len": 256,
            "rope_theta": 10000,
            "init_seed": 0,
            "param_count": 3_279_104,
            "compute_dtype": "fp32",
            "master_dtype": "fp32",
            "capacity_factor": f32hex(1.25),
            "aux_loss_coef": f32hex(0.01),
            "init_std": f32hex(0.02),
        },
        "tokenizer": {"name": "ht-byte-259", "sha256": _h("tokenizer")},
        "dataset": {
            "merkle_root": _h("dataset-root"),
            "depth": 20,
            "n_samples": 781_250,
            "sample_format": "u32[seq_len+1] token ids",
            "shard_uri_template": "data/shards/{index:05d}.u32",
            "shard_sha256_root": _h("shard-root"),
            "holdout_commit": _h("holdout"),
        },
        "inner": {
            "opt": "adamw",
            "betas": [f32hex(0.9), f32hex(0.95)],
            "eps": f32hex(1e-8),
            "wd": f32hex(0.1),
            "grad_clip": f32hex(1.0),
            "micro_batch": 8,
            "grad_accum": 1,
            "H": 30,
            "J": 5,
            "lr_schedule": {
                "type": "wsd",
                "peak_lr": f32hex(3e-4),
                "warmup": 100,
                "stable": 8000,
                "decay": 1900,
            },
            "state_policy": "carry",
            "rewarmup_steps": 0,
            "muon_momentum": f32hex(0.95),
            "ns_steps": 5,
        },
        "outer": {
            "opt": "nesterov",
            "lr": f32hex(0.4),
            "momentum": f32hex(0.9),
            "topk_frac": f32hex(1.0),
            "bits": 8,
            "ef_beta": f32hex(0.0),
            "preclip_norm": f32hex(1.0),
            "cclip_tau": f32hex(1.0),
            "cclip_iters": 1,
            "center": "prev_outer_update",
        },
        "verify": default_verify(
            0.1,
            _h("cluster-rules"),
            Timeouts(
                assign_after_open=20,
                audit_after_commit=40,
                upload_after_commit=300,
                serve_deadline=900,
                dispute_per_level=600,
            ),
        ),
        "reference_spec": {
            "image_digest": "sha256:" + _h("image"),
            "spec_doc_sha256": _h("spec-doc"),
            "driver_allowlist": [],
            "sm_count": 170,
            "env": {
                "CUBLAS_WORKSPACE_CONFIG": ":4096:8",
                "CUDA_DISABLE_PTX_JIT": "1",
                "cpu_threads": 1,
            },
            "layout": {"pp": 1, "n_gpus": 1, "dp_size": 1, "ep_size": 1, "zero1": False},
        },
        "init_state_hash": _h("init-state"),
        "beacon": {
            "chain_hash": QUICKNET_CHAIN_HASH,
            "period": QUICKNET_PERIOD,
            "genesis_time": QUICKNET_GENESIS,
        },
        "auditors": [auditor],
        "coord_pubkey": coord,
        "budget": {"epochs_per_round": 1},
        "operator_budget": {
            "relay": {"instances": 1, "gpus_per_instance": 0, "usd_micro_per_round": 0},
            "auditor": {"instances": 1, "gpus_per_instance": 1, "usd_micro_per_round": 0},
            "honeypot": {
                "instances": 0,
                "gpus_per_instance": 1,
                "usd_micro_per_round": 0,
                "rate": f32hex(0.03),
            },
        },
    }
    body["verify"] = body["verify"].model_dump(mode="json")
    return RunManifest.model_validate(body)


def example_document() -> dict[str, Any]:
    m = example_manifest()
    return {"run_id": m.run_id(), "manifest": m.body(), "provenance": PROVENANCE}
