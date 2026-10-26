"""Host probe verdict for scripts/b300/run_host.sh (step a). Standard library only (host python3).

Reads what run_host.sh captured: ``nvidia-smi --query-gpu=index,name,driver_version,compute_cap,
memory.total,pci.bus_id --format=csv,noheader``, the ``/sys/class/infiniband`` listing (EFA
devices) and ``df -P -B1G`` of the results and Docker roots. Writes host_probe.json with each
check and exits 0 only when all pass.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path
from typing import Any


def gpus(csv: str) -> list[dict[str, str]]:
    keys = ("index", "name", "driver_version", "compute_cap", "memory_total", "pci_bus_id")
    rows = [[c.strip() for c in line.split(",")] for line in csv.splitlines() if line.strip()]
    return [dict(zip(keys, r, strict=False)) for r in rows]


def free_gb(df: str) -> dict[str, int]:
    """Available GiB per mount point from ``df -P -B1G`` (header skipped)."""
    out = {}
    for line in df.splitlines()[1:]:
        cols = line.split()
        if len(cols) >= 6 and cols[3].rstrip("G").isdigit():
            out[cols[5]] = int(cols[3].rstrip("G"))
    return out


def verdict(a: argparse.Namespace) -> dict[str, Any]:
    g = gpus(a.nvidia_smi.read_text())
    efa = [x for x in a.efa.read_text().split() if x]
    disk = free_gb(a.df.read_text())
    checks = {
        "gpu_count": len(g) == a.gpus,
        "driver": bool(g) and {x.get("driver_version") for x in g} == {a.driver},
        "compute_cap": bool(g) and {x.get("compute_cap") for x in g} == {a.cc},
        "efa": len(efa) >= a.min_efa,
        "disk": bool(disk) and min(disk.values()) >= a.min_disk_gb,
    }
    return {
        "kind": "b300-host-probe",
        "expected": {
            "gpus": a.gpus,
            "driver": a.driver,
            "compute_cap": a.cc,
            "min_efa": a.min_efa,
            "min_disk_gb": a.min_disk_gb,
        },
        "gpus": g,
        "efa_devices": efa,
        "free_gb": disk,
        "checks": checks,
        "ok": all(checks.values()),
    }


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", type=Path, required=True)
    ap.add_argument("--nvidia-smi", type=Path, required=True)
    ap.add_argument("--efa", type=Path, required=True)
    ap.add_argument("--df", type=Path, required=True)
    ap.add_argument("--driver", required=True)
    ap.add_argument("--gpus", type=int, required=True)
    ap.add_argument("--cc", required=True)
    ap.add_argument("--min-efa", type=int, default=1)
    ap.add_argument("--min-disk-gb", type=int, default=100)
    a = ap.parse_args()
    v = verdict(a)
    a.out.write_text(json.dumps(v, indent=1, sort_keys=True))
    print(json.dumps({"ok": v["ok"], "checks": v["checks"]}, sort_keys=True))
    return 0 if v["ok"] else 1


if __name__ == "__main__":
    sys.exit(main())
