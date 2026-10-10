"""Client-side signatures and the local GPU compute self-check.

The service decides funding and eligibility; :func:`compute_selfcheck` is a miner-side gate that
proves each listed device actually computes (bitwise, under the repo determinism pins) and can
allocate ~90% of the memory it reports, instead of trusting NVML/driver identity.
"""

from __future__ import annotations

import argparse
import hashlib
import importlib
import json
import sys
import time
from collections.abc import Sequence
from typing import Any

from pydantic import JsonValue

from hypertrain.protocol.envelope_v2 import join_signing_message, seal
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages_v2 import HardwareHint, JoinRequest, RotateRequest, WorkProof


def sign_join(
    hotkey: Keypair,
    coldkey: Keypair,
    *,
    run_id: str,
    request_id: str,
    expires_beacon: int,
    policy_hash: str,
    hardware_hint: HardwareHint,
) -> JoinRequest:
    request = JoinRequest(
        run_id=run_id,
        request_id=request_id,
        hotkey=hotkey.ss58,
        coldkey=coldkey.ss58,
        expires_beacon=expires_beacon,
        policy_hash=policy_hash,
        hardware_hint=hardware_hint,
        hot_sig="0" * 128,
        cold_sig="0" * 128,
    )
    message = join_signing_message(request)
    return JoinRequest.model_validate(
        {
            **request.body(),
            "hot_sig": hotkey.sign(message).hex(),
            "cold_sig": coldkey.sign(message).hex(),
        }
    )


def sign_proof(
    hotkey: Keypair, run_id: str, proof: WorkProof, expires_beacon: int
) -> dict[str, JsonValue]:
    return seal(hotkey, "WorkProof", run_id, proof, expires_beacon)


def sign_rotation(
    coldkey: Keypair, run_id: str, request: RotateRequest, expires_beacon: int
) -> dict[str, JsonValue]:
    return seal(coldkey, "RotateRequest", run_id, request, expires_beacon)


SELFCHECK_SEED = 20261010
VRAM_FRACTION = 0.9
CHUNK_BYTES = 256 << 20
# ponytail: host RAM is not probed; CPU "reported" memory is a fixed small budget so the
# self-check stays cheap in tests. Probe real RAM if CPU miners ever need a capacity proof.
CPU_REPORTED_BYTES = 64 << 20


def _device_name(index: int | str) -> str:
    if index == "cpu":
        return "cpu"
    if type(index) is int and index >= 0:
        return f"cuda:{index}"
    raise ValueError(f"device index must be a non-negative int or 'cpu', got {index!r}")


def _tensor_bytes(t: Any) -> bytes:
    return bytes(t.detach().cpu().contiguous().view(-1).view(_torch().uint8).numpy().tobytes())


def _torch() -> Any:
    importlib.import_module("hypertrain.trainer")  # pins determinism before torch is imported
    import torch

    return torch


def _workload(device: str) -> list[Any]:
    """Fixed-seed FP32 + BF16 matmuls and one transformer block forward/backward."""
    torch = _torch()
    # Inputs come from a CPU generator so every device sees identical bytes.
    g = torch.Generator().manual_seed(SELFCHECK_SEED)

    def rand(*shape: int) -> Any:
        return torch.randn(*shape, generator=g, dtype=torch.float32).to(device)

    a, b = rand(256, 256), rand(256, 256)
    out: list[Any] = [a @ b, a.bfloat16() @ b.bfloat16()]

    seq, dim, heads, hidden = 32, 64, 4, 256
    x = rand(2, seq, dim).requires_grad_(True)
    params = {
        "ln1": rand(dim),
        "wqkv": rand(dim, 3 * dim) * dim**-0.5,
        "wo": rand(dim, dim) * dim**-0.5,
        "ln2": rand(dim),
        "w1": rand(dim, hidden) * dim**-0.5,
        "w2": rand(hidden, dim) * hidden**-0.5,
    }
    for p in params.values():
        p.requires_grad_(True)
    f = torch.nn.functional
    h = f.layer_norm(x, (dim,), weight=params["ln1"])
    q, k, v = (h @ params["wqkv"]).split(dim, dim=-1)
    q, k, v = (t.reshape(2, seq, heads, dim // heads).transpose(1, 2) for t in (q, k, v))
    mask = torch.ones(seq, seq, dtype=torch.bool, device=device).tril()
    scores = (q @ k.transpose(-1, -2)) * (dim // heads) ** -0.5
    attn = scores.masked_fill(~mask, float("-inf")).softmax(dim=-1) @ v
    y = x + attn.transpose(1, 2).reshape(2, seq, dim) @ params["wo"]
    y = y + f.gelu(f.layer_norm(y, (dim,), weight=params["ln2"]) @ params["w1"]) @ params["w2"]
    y.square().mean().backward()
    out.append(y)
    out.append(x.grad)
    out.extend(params[n].grad for n in sorted(params))
    return out


def _reported_bytes(device: str) -> int:
    if device == "cpu":
        return CPU_REPORTED_BYTES
    return int(_torch().cuda.get_device_properties(device).total_memory)


def _allocate_chunk(device: str, nbytes: int) -> Any:
    return _torch().empty(nbytes, dtype=_torch().uint8, device=device)


def _probe_memory(device: str, target: int) -> int:
    """Allocate ``target`` bytes in chunks, write+read a pattern per chunk; return bytes proven."""
    torch = _torch()
    chunks: list[Any] = []
    proven = 0
    try:
        while proven < target:
            n = min(CHUNK_BYTES, target - proven)
            try:
                chunk = _allocate_chunk(device, n)
            except torch.OutOfMemoryError:
                break
            pattern = len(chunks) % 251 + 1
            chunk.fill_(pattern)
            if not bool((chunk == pattern).all()):
                break
            chunks.append(chunk)
            proven += n
    finally:
        del chunks
        if device != "cpu":
            torch.cuda.empty_cache()
    return proven


def _sync(device: str) -> None:
    if device != "cpu":
        _torch().cuda.synchronize(device)


def compute_selfcheck(device_indices: Sequence[int | str]) -> dict[str, Any]:
    """Run the deterministic workload + VRAM probe on every device; all hashes must match.

    ``device_indices`` holds CUDA ordinals or ``"cpu"``. Hashes are only comparable within one
    host/device type (CPU and CUDA kernels differ), which is exactly the cross-GPU check.
    """
    devices = [_device_name(i) for i in device_indices]
    if not devices:
        raise ValueError("compute_selfcheck needs at least one device")
    torch = _torch()
    failures: list[str] = []
    rows: list[dict[str, Any]] = []
    for device in devices:
        row: dict[str, Any] = {"device": device}
        rows.append(row)
        try:
            _sync(device)
            t0 = time.perf_counter()
            tensors = _workload(device)
            _sync(device)
            row["workload_seconds"] = time.perf_counter() - t0
            digest = hashlib.sha256()
            for t in tensors:
                digest.update(_tensor_bytes(t))
            row["sha256"] = digest.hexdigest()
            del tensors
            reported = _reported_bytes(device)
            target = int(reported * VRAM_FRACTION)
            proven = _probe_memory(device, target)
            row.update(reported_bytes=reported, target_bytes=target, allocatable_bytes=proven)
            if proven < target:
                failures.append(f"{device}: allocatable {proven} < target {target} bytes")
        except (RuntimeError, torch.OutOfMemoryError) as exc:
            row["error"] = f"{type(exc).__name__}: {exc}"
            failures.append(f"{device}: {row['error']}")
    hashes = {row.get("sha256") for row in rows}
    hashes_equal = len(hashes) == 1 and None not in hashes
    if not hashes_equal:
        failures.append("output hashes differ across devices")
    return {
        "ok": not failures,
        "hashes_equal": hashes_equal,
        "sha256": rows[0].get("sha256") if hashes_equal else None,
        "determinism": _determinism(),
        "devices": rows,
        "failures": failures,
    }


def _determinism() -> dict[str, Any]:
    from hypertrain.trainer import DETERMINISM

    return dict(DETERMINISM)


def _parse_devices(spec: str | None) -> list[int | str]:
    if spec is None:
        n = _torch().cuda.device_count()
        return list(range(n)) if n else ["cpu"]
    return [p if p == "cpu" else int(p) for p in spec.split(",")]


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="python -m hypertrain.miner.admission")
    sub = parser.add_subparsers(dest="command", required=True)
    sc = sub.add_parser("selfcheck", help="prove GPU compute + allocatable memory")
    sc.add_argument("--devices", help="comma list of CUDA ordinals or 'cpu' (default: all GPUs)")
    args = parser.parse_args(argv)
    result = compute_selfcheck(_parse_devices(args.devices))
    print(json.dumps(result, indent=2, sort_keys=True))
    return 0 if result["ok"] else 1


if __name__ == "__main__":
    sys.exit(main())
