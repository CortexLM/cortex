"""CUDA-free stand-in for a non-CPU device, used to prove the trainer is device-correct.

DevTensor reports device ``meta`` but carries real CPU data, so the math actually runs.
Like CUDA: ``.numpy()`` on it raises, ``.cpu()`` returns a plain host copy, and any aten op
that mixes it with a non-scalar host tensor raises DeviceMixError. Factories that request the
proxy device produce DevTensors. ``FakeDeviceMode(default_on_device=True)`` calls the real
``torch.set_default_device(PROXY)``, so un-pinned factories (zeros/ones/arange/tensor/randn...)
land on the proxy exactly as under ``set_default_device("cuda")``; ``torch.from_numpy`` and
``device="cpu"`` factories stay on the host.
"""

from __future__ import annotations

from typing import Any

import torch
from torch.utils._python_dispatch import TorchDispatchMode
from torch.utils._pytree import tree_flatten, tree_map, tree_unflatten

aten = torch.ops.aten
PROXY = torch.device("meta")
REAL_SET_DEFAULT_DEVICE = torch.set_default_device


class DeviceMixError(RuntimeError):
    pass


class DevTensor(torch.Tensor):
    inner: torch.Tensor

    @staticmethod
    def __new__(cls, inner: torch.Tensor) -> DevTensor:
        assert inner.device.type == "cpu" and not isinstance(inner, DevTensor)
        r = torch.Tensor._make_wrapper_subclass(  # type: ignore[attr-defined]
            cls,
            inner.size(),
            strides=inner.stride(),
            storage_offset=inner.storage_offset(),
            dtype=inner.dtype,
            layout=inner.layout,
            device=PROXY,
            requires_grad=False,
        )
        r.inner = inner
        return r  # type: ignore[no-any-return]

    def __repr__(self) -> str:  # type: ignore[override]
        return f"DevTensor({self.inner!r})"

    def tolist(self) -> Any:
        return self.inner.tolist()

    @classmethod
    def __torch_dispatch__(cls, func: Any, types: Any, args: Any = (), kwargs: Any = None) -> Any:
        return _handle(func, args, kwargs or {})


def to_device(t: torch.Tensor) -> torch.Tensor:
    return DevTensor(t.detach().clone())


def _is_proxy(dev: Any) -> bool:
    return dev is not None and torch.device(dev).type == "meta"


def _handle(func: Any, args: Any, kwargs: dict[str, Any]) -> Any:
    flat, _ = tree_flatten((args, kwargs))
    tensors = [a for a in flat if isinstance(a, torch.Tensor)]
    on_dev = [a for a in tensors if isinstance(a, DevTensor)]
    host = [a for a in tensors if not isinstance(a, DevTensor)]
    for h in host:
        if h.device.type == "meta":
            raise DeviceMixError(f"{func}: real meta tensor leaked (data-less factory)")
    if on_dev and any(h.dim() > 0 for h in host):
        shapes = [tuple(h.shape) for h in host if h.dim() > 0]
        raise DeviceMixError(f"{func}: device tensor mixed with host tensor(s) {shapes}")

    want = kwargs.get("device", None)
    wrap_out = bool(on_dev)
    if func in (aten._to_copy.default, aten.to.dtype_layout, aten.to.device):
        if want is not None:
            wrap_out = _is_proxy(want)
            kwargs = {**kwargs, "device": torch.device("cpu")}
    elif not tensors and "device" in {a.name for a in func._schema.arguments}:
        if _is_proxy(want):
            wrap_out = True
            kwargs = {**kwargs, "device": torch.device("cpu")}

    by_inner = {id(d.inner): d for d in on_dev}

    def unwrap(x: Any) -> Any:
        return x.inner if isinstance(x, DevTensor) else x

    out = func(*tree_map(unwrap, args), **tree_map(unwrap, kwargs))
    if not wrap_out:
        return out

    def wrap(x: Any) -> Any:
        if isinstance(x, torch.Tensor) and not isinstance(x, DevTensor):
            same = by_inner.get(id(x))
            return same if same is not None else DevTensor(x)
        return x

    leaves, spec = tree_flatten(out)
    return tree_unflatten([wrap(x) for x in leaves], spec)


class FakeDeviceMode(TorchDispatchMode):
    """Activate the proxy; ``default_on_device`` = real torch.set_default_device(PROXY)."""

    def __init__(self, default_on_device: bool) -> None:
        super().__init__()
        self.default_on_device = default_on_device

    def __enter__(self) -> FakeDeviceMode:
        self._prev = torch.get_default_device()
        if self.default_on_device:
            REAL_SET_DEFAULT_DEVICE(PROXY)
        return super().__enter__()  # type: ignore[no-any-return]

    def __exit__(self, *exc: object) -> None:
        super().__exit__(*exc)
        REAL_SET_DEFAULT_DEVICE(self._prev)

    def __torch_dispatch__(self, func: Any, types: Any, args: Any = (), kwargs: Any = None) -> Any:
        return _handle(func, args, kwargs or {})


class CudaRedirect(torch.Tensor):
    """Host tensor whose ``.to("cuda")`` lands on the proxy (CPU torch builds cannot init CUDA)."""

    def to(self, *args: Any, **kwargs: Any) -> torch.Tensor:  # type: ignore[override]
        dev = kwargs.get("device", args[0] if args else None)
        plain = self.as_subclass(torch.Tensor)
        if dev is not None and torch.device(dev).type == "cuda":
            return to_device(plain)
        return plain.to(*args, **kwargs)
