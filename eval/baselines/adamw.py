"""Locked AdamW recipe the image seals.

script_sha256 on an open topic must be SHA-256 of these exact bytes.
A topic that claims `optimizer = adamw` with a different script is a
strawman and a publish reject on the control plane.

lr=3e-4, betas=(0.9, 0.95), eps=1e-8, weight_decay=0.1, warmup_ratio=0.02,
cosine schedule, bf16, seed=42 — matching `proof_task::default_adamw`.
"""

from __future__ import annotations

ADAMW = {
    "optimizer": "adamw",
    "lr": 3e-4,
    "betas": (0.9, 0.95),
    "eps": 1e-8,
    "weight_decay": 0.1,
    "warmup_ratio": 0.02,
    "schedule": "cosine",
    "dtype": "bf16",
    "seed": 42,
}


def recipe() -> dict:
    return dict(ADAMW)
