"""Controller-checkable compute traces.

This is not a universal FLOP formula and not a hostile-proof of every kernel.
Listed ops have published costs; unlisted compute-shaped ops refuse rather
than undercount. `torch.profiler` and `FlopCounterMode` are not the source
of truth: the retained op list is, and the controller recomputes the same
total. A passing tiny matmul does not attest every training recipe.
"""

from __future__ import annotations

from collections import Counter
from contextlib import contextmanager, nullcontext
from typing import Any, Iterator

from .contract import ContractError

TRACE_SCHEMA = 1
_U64 = (1 << 64) - 1

# Canonical names after stripping `.default` / `.out`.
_COST = frozenset(
    {
        "aten::mm",
        "aten::addmm",
        "aten::bmm",
        "aten::baddbmm",
        "aten::scaled_dot_product_attention",
        "aten::_scaled_dot_product_attention_math",
    }
)

_UNLISTED_MARKERS = (
    "matmul",
    "addmm",
    "baddbmm",
    "addbmm",
    "convolution",
    "conv2d",
    "conv1d",
    "conv3d",
    "conv_transpose",
    "scaled_dot_product",
    "flash_attention",
    "cudnn_convolution",
    "miopen",
)


def canonical_op(name: str) -> str:
    raw = name.strip()
    if raw.startswith("aten."):
        raw = "aten::" + raw[5:]
    if raw.endswith(".default") or raw.endswith(".out"):
        raw = raw.rsplit(".", 1)[0]
    return raw


def _looks_compute(name: str) -> bool:
    n = name.lower()
    if n in _COST or n.endswith("::mm") or n.endswith(".mm"):
        return True
    return any(marker in n for marker in _UNLISTED_MARKERS)


def _checked_mul(*values: int) -> int:
    total = 1
    for value in values:
        if value < 0:
            raise ContractError("negative trace shape")
        total *= value
        if total > _U64:
            raise ContractError("trace flop overflow")
    return total


def _checked_add(left: int, right: int) -> int:
    total = left + right
    if total > _U64:
        raise ContractError("trace flop overflow")
    return total


def _matmul(a: list[int], b: list[int]) -> int:
    if len(a) < 2 or len(b) < 2 or a[-1] != b[-2]:
        raise ContractError("matmul shape mismatch")
    a_batch, b_batch = a[:-2], b[:-2]
    n = max(len(a_batch), len(b_batch))
    a_batch = [1] * (n - len(a_batch)) + a_batch
    b_batch = [1] * (n - len(b_batch)) + b_batch
    batch = 1
    for left, right in zip(a_batch, b_batch, strict=True):
        if left != right and left != 1 and right != 1:
            raise ContractError("matmul batch mismatch")
        batch = _checked_mul(batch, max(left, right))
    return _checked_mul(2, batch, a[-2], a[-1], b[-1])


def op_flops(op: str, shapes: list[list[int]]) -> int:
    name = canonical_op(op)
    if name == "aten::mm":
        if len(shapes) < 2:
            raise ContractError("mm missing operands")
        return _matmul(shapes[-2], shapes[-1])
    if name == "aten::addmm":
        if len(shapes) < 3:
            raise ContractError("addmm missing operands")
        return _matmul(shapes[-2], shapes[-1])
    if name == "aten::bmm":
        if len(shapes) < 2:
            raise ContractError("bmm missing operands")
        return _matmul(shapes[-2], shapes[-1])
    if name == "aten::baddbmm":
        if len(shapes) < 3:
            raise ContractError("baddbmm missing operands")
        return _matmul(shapes[-2], shapes[-1])
    if name in {
        "aten::scaled_dot_product_attention",
        "aten::_scaled_dot_product_attention_math",
    }:
        if len(shapes) < 3 or any(len(item) < 2 for item in shapes[:3]):
            raise ContractError("sdpa missing operands")
        q, k, v = shapes[0], shapes[1], shapes[2]
        # Math-backend equivalent: QK^T + AV. Flash/efficient kernels refuse.
        qk = _matmul(q, k[:-2] + [k[-1], k[-2]])
        av = _matmul(q[:-2] + [q[-2], k[-2]], v)
        return _checked_add(qk, av)
    if _looks_compute(name):
        raise ContractError(f"unlisted compute op: {name}")
    return 0


def verify_trace(trace: dict[str, Any]) -> int:
    """Recompute FLOPs from a retained op list. Never trust a supplied total."""
    if not isinstance(trace, dict) or trace.get("schema_version") != TRACE_SCHEMA:
        raise ContractError("compute trace schema is not 1")
    ops = trace.get("ops")
    if not isinstance(ops, list) or not ops:
        raise ContractError("compute trace is empty")
    total = 0
    saw_compute = False
    for item in ops:
        if not isinstance(item, dict):
            raise ContractError("compute trace op is not an object")
        op = item.get("op")
        shapes = item.get("shapes")
        count = item.get("count")
        dtype = item.get("dtype")
        if not isinstance(op, str) or not isinstance(dtype, str) or not dtype:
            raise ContractError("compute trace op is malformed")
        if not isinstance(shapes, list) or not all(
            isinstance(shape, list) and all(isinstance(dim, int) for dim in shape)
            for shape in shapes
        ):
            raise ContractError("compute trace shapes are malformed")
        if not isinstance(count, int) or count < 1:
            raise ContractError("compute trace count is malformed")
        name = canonical_op(op)
        if _looks_compute(name) and name not in _COST:
            raise ContractError(f"unlisted compute op: {name}")
        flops = _checked_mul(op_flops(name, shapes), count)
        if flops:
            saw_compute = True
        total = _checked_add(total, flops)
    if not saw_compute:
        raise ContractError("compute trace has no listed matmul/attention work")
    return total


class ComputeTrace:
    def __init__(self) -> None:
        self._ops: Counter[tuple[str, tuple[tuple[int, ...], ...], str]] = Counter()

    def add(self, op: str, shapes: list[list[int]], dtype: str) -> None:
        key = (canonical_op(op), tuple(tuple(shape) for shape in shapes), dtype)
        self._ops[key] += 1

    def to_dict(self) -> dict[str, Any]:
        ops = [
            {
                "op": op,
                "shapes": [list(shape) for shape in shapes],
                "dtype": dtype,
                "count": count,
            }
            for (op, shapes, dtype), count in sorted(self._ops.items())
        ]
        return {"schema_version": TRACE_SCHEMA, "ops": ops}


@contextmanager
def collect_trace() -> Iterator[ComputeTrace]:
    try:
        import torch
        from torch.utils._python_dispatch import TorchDispatchMode
    except ImportError as exc:
        raise ContractError(f"no compute-trace runtime: {exc}") from exc

    trace = ComputeTrace()

    class _Mode(TorchDispatchMode):
        def __torch_dispatch__(self, func, types, args=(), kwargs=None):  # type: ignore[no-untyped-def]
            kwargs = kwargs or {}
            result = func(*args, **kwargs)
            name = getattr(func, "_name", None) or str(func)
            shapes: list[list[int]] = []
            dtype = ""
            for arg in args:
                if hasattr(arg, "shape") and hasattr(arg, "dtype"):
                    shapes.append([int(dim) for dim in arg.shape])
                    if not dtype:
                        dtype = str(arg.dtype).replace("torch.", "")
            if dtype:
                trace.add(str(name), shapes, dtype)
            return result

    attention = nullcontext()
    try:
        from torch.nn.attention import SDPBackend, sdpa_kernel

        attention = sdpa_kernel(SDPBackend.MATH)
    except Exception:  # noqa: BLE001
        try:
            attention = torch.backends.cuda.sdp_kernel(
                enable_flash=False, enable_mem_efficient=False, enable_math=True
            )
        except Exception:  # noqa: BLE001
            attention = nullcontext()
    with attention, _Mode():
        yield trace
