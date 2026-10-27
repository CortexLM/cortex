"""Pin bodies for tests of ``reference_spec.env.pinned`` (ADR 0011, part 3).

The presets are measured per compute dtype on kimi MICRO (docs/protocol.md section 7): fp32 roots
depend on ``ATEN_CPU_CAPABILITY`` and ``MKL_CBWR``, bf16 roots on ``ONEDNN_MAX_CPU_ISA``.
Production code reads only the manifest, so the presets live here and in the docs.
"""

from __future__ import annotations

from typing import Any

CPU_FP32: dict[str, Any] = {
    "present": {
        "ATEN_CPU_CAPABILITY": "avx2",
        "HT_ATTN_BACKEND": "math",
        "MKL_CBWR": "AVX2",
        "OMP_DYNAMIC": "FALSE",
    },
    "absent": ["HT_COMPILE", "HT_FP8", "HT_MOE_GROUPED", "OMP_THREAD_LIMIT"],
}
CPU_BF16: dict[str, Any] = {
    "present": {**CPU_FP32["present"], "ONEDNN_MAX_CPU_ISA": "AVX2"},
    "absent": list(CPU_FP32["absent"]),
}
