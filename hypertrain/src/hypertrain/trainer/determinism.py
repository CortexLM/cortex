"""Determinism setup. Must run before ``import torch`` in every process that trains or replays.

Importing ``hypertrain.trainer`` calls :func:`setup_determinism` from the package ``__init__``,
so every trainer module gets it before its own ``import torch``.
"""

from __future__ import annotations

import os
import sys
from collections.abc import Mapping
from typing import Any

CUBLAS_WORKSPACE_CONFIG = ":4096:8"


def apply_reference_env(env: Mapping[str, Any]) -> dict[str, Any]:
    """Pin determinism from manifest ``reference_spec.env``; refuse values we cannot honor."""
    if env.get("CUBLAS_WORKSPACE_CONFIG") != CUBLAS_WORKSPACE_CONFIG:
        raise RuntimeError("manifest CUBLAS_WORKSPACE_CONFIG differs from the pinned value")
    threads = env.get("cpu_threads")
    if type(threads) is not int or threads < 1:
        raise RuntimeError("manifest reference_spec.env.cpu_threads must be an int >= 1")
    return setup_determinism(threads)


def require_threads(expected: int) -> None:
    """Refuse to train/replay when the live CPU thread pin differs from the manifest's."""
    import torch

    if torch.get_num_threads() != expected:
        raise RuntimeError(
            f"cpu thread pin mismatch: torch uses {torch.get_num_threads()}, manifest {expected}"
        )


def setup_determinism(num_threads: int = 1) -> dict[str, Any]:
    """Set env + torch flags for bitwise-reproducible training; return the applied settings.

    Raises RuntimeError if torch was imported before the cuBLAS workspace env was pinned
    (cuBLAS reads it at CUDA init, so a late setting cannot be trusted).
    """
    if num_threads < 1:
        raise ValueError("num_threads must be >= 1")
    torch_preloaded = "torch" in sys.modules
    if torch_preloaded and os.environ.get("CUBLAS_WORKSPACE_CONFIG") != CUBLAS_WORKSPACE_CONFIG:
        raise RuntimeError("torch imported before hypertrain determinism setup")
    os.environ["CUBLAS_WORKSPACE_CONFIG"] = CUBLAS_WORKSPACE_CONFIG
    import torch

    torch.use_deterministic_algorithms(True)  # no warn_only: nondeterministic ops raise
    torch.backends.cudnn.benchmark = False
    torch.backends.cudnn.deterministic = True
    torch.backends.cudnn.allow_tf32 = False
    torch.backends.cuda.matmul.allow_tf32 = False
    torch.backends.cuda.matmul.allow_bf16_reduced_precision_reduction = False
    torch.backends.cuda.matmul.allow_fp16_reduced_precision_reduction = False
    torch.set_float32_matmul_precision("highest")
    # CPU reductions depend on thread partitioning; pin it (auditor must use the same value).
    torch.set_num_threads(num_threads)
    return {
        "torch_preloaded": torch_preloaded,
        "CUBLAS_WORKSPACE_CONFIG": os.environ["CUBLAS_WORKSPACE_CONFIG"],
        "deterministic_algorithms": torch.are_deterministic_algorithms_enabled(),
        "deterministic_warn_only": torch.is_deterministic_algorithms_warn_only_enabled(),
        "cudnn_benchmark": torch.backends.cudnn.benchmark,
        "cudnn_deterministic": torch.backends.cudnn.deterministic,
        "cudnn_allow_tf32": torch.backends.cudnn.allow_tf32,
        "matmul_allow_tf32": torch.backends.cuda.matmul.allow_tf32,
        "bf16_reduced_precision_reduction": (
            torch.backends.cuda.matmul.allow_bf16_reduced_precision_reduction
        ),
        "num_threads": torch.get_num_threads(),
    }
