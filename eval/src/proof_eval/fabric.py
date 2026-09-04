"""Enforce topic fabric constraints. The image never trusts the miner's claim.

dt-no-ib-v0 (and any throughput topic that sets these flags) must run with:

* no InfiniBand
* no NVLink
* no NCCL fast-fabric all-reduce
* inter-node (or emulated inter-rank) cap at ``max_inter_node_gbps`` (12.5)

This module sets the NCCL/UCX env the training process inherits, optionally
installs a `tc` rate limit, and refuses if a fast path is already in use.
"""

from __future__ import annotations

import os
import shutil
import subprocess
from pathlib import Path

from .contract import ContractError
from .request import Constraints

# 12.5 Gbit/s is the documented dt-no-ib-v0 cap. A topic may tighten it.
DT_NO_IB_GBPS = 12.5


def enforce(constraints: Constraints) -> dict[str, str]:
    """Apply constraints. Returns the env that was set (for the document)."""
    applied: dict[str, str] = {}
    if constraints.no_infiniband:
        applied.update(_disable_infiniband())
    if constraints.no_nvlink:
        applied.update(_disable_nvlink())
    if constraints.no_nccl_fast_fabric:
        applied.update(_disable_nccl_fast_fabric())
    if constraints.max_inter_node_gbps is not None:
        cap = float(constraints.max_inter_node_gbps)
        if cap <= 0 or cap > DT_NO_IB_GBPS + 1e-9:
            raise ContractError(
                f"max_inter_node_gbps {cap} loosens the image floor {DT_NO_IB_GBPS} Gbit/s"
            )
        applied.update(_cap_bandwidth(cap))
    return applied


def _set(name: str, value: str) -> dict[str, str]:
    os.environ[name] = value
    return {name: value}


def _disable_infiniband() -> dict[str, str]:
    applied = {}
    applied.update(_set("NCCL_IB_DISABLE", "1"))
    applied.update(_set("NCCL_IB_HCA", ""))
    applied.update(_set("UCX_TLS", "tcp"))
    applied.update(_set("UCX_NET_DEVICES", "eth0,enp0s0,ens,eth"))
    ib = Path("/sys/class/infiniband")
    if ib.is_dir() and any(ib.iterdir()):
        # Hardware may exist; using it is the cheat. NCCL_IB_DISABLE=1 is the
        # enforcement. A process that opens /dev/infiniband after this is a
        # later agent check, not a reason to refuse the machine.
        applied["infiniband_devices_present"] = "1"
    return applied


def _disable_nvlink() -> dict[str, str]:
    applied = {}
    applied.update(_set("NCCL_P2P_DISABLE", "1"))
    applied.update(_set("NCCL_NVLS_ENABLE", "0"))
    applied.update(_set("NCCL_P2P_LEVEL", "LOC"))
    applied.update(_set("NCCL_SHM_DISABLE", "0"))
    return applied


def _disable_nccl_fast_fabric() -> dict[str, str]:
    applied = {}
    applied.update(_set("NCCL_NET", "Socket"))
    applied.update(_set("NCCL_ALGO", "Ring"))
    applied.update(_set("NCCL_PROTO", "Simple"))
    applied.update(_set("NCCL_NET_GDR_LEVEL", "0"))
    return applied


def _cap_bandwidth(gbps: float) -> dict[str, str]:
    """Cap inter-node traffic at ``gbps`` Gbit/s.

    Prefer `tc` when the pod can install a qdisc (best-effort). Always set
    NCCL socket env so a run that cannot tc still cannot silently use IB/GDR.
    """
    applied = _set("PROOF_MAX_INTER_NODE_GBPS", f"{gbps:g}")
    # Smaller NCCL buffers make it harder to hide a burst over the cap.
    bytes_per_sec = int(gbps * 1_000_000_000 / 8)
    applied.update(_set("NCCL_BUFFSIZE", str(min(max(bytes_per_sec // 64, 32_768), 1_048_576))))
    applied.update(_set("NCCL_NSOCKS_PERTHREAD", "1"))
    tc = shutil.which("tc")
    if tc:
        kbit = max(int(gbps * 1_000_000), 1)
        for dev in _data_ifaces():
            subprocess.run(  # noqa: S603
                [tc, "qdisc", "replace", "dev", dev, "root", "tbf",
                 "rate", f"{kbit}kbit", "burst", "256kb", "latency", "50ms"],
                check=False,
                capture_output=True,
            )
            applied[f"tc:{dev}"] = f"{kbit}kbit"
    return applied


def _data_ifaces() -> list[str]:
    sys = Path("/sys/class/net")
    if not sys.is_dir():
        return []
    skip = {"lo", "docker0", "cni0"}
    out = []
    for p in sorted(sys.iterdir()):
        name = p.name
        if name in skip or name.startswith("veth"):
            continue
        out.append(name)
    return out


def selftest() -> None:
    """Prove the image can enforce the dt-no-ib-v0 cap without a request."""
    applied = enforce(
        Constraints(
            no_infiniband=True,
            no_nvlink=True,
            no_nccl_fast_fabric=True,
            max_inter_node_gbps=DT_NO_IB_GBPS,
        )
    )
    for key in ("NCCL_IB_DISABLE", "NCCL_P2P_DISABLE", "NCCL_NET", "PROOF_MAX_INTER_NODE_GBPS"):
        if os.environ.get(key) in (None, ""):
            raise ContractError(f"fabric selftest did not set {key}")
    if os.environ.get("NCCL_IB_DISABLE") != "1":
        raise ContractError("NCCL_IB_DISABLE must be 1 under the no-IB cap")
    if os.environ.get("NCCL_P2P_DISABLE") != "1":
        raise ContractError("NCCL_P2P_DISABLE must be 1 under the no-NVLink cap")
    if os.environ.get("NCCL_NET") != "Socket":
        raise ContractError("NCCL_NET must be Socket (no NCCL fast fabric)")
    if applied.get("PROOF_MAX_INTER_NODE_GBPS") != "12.5":
        raise ContractError("12.5 Gbit/s cap was not applied")
