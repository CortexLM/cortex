"""CPU vs CUDA equivalence of the parity simulator on shortened full-scale cells.

Result: docs/parity.md section 5.

  python experiments/sim/parity_equiv.py --device cpu  --out equiv_cpu.json
  python experiments/sim/parity_equiv.py --device cuda --out equiv_cuda.json
  python experiments/sim/parity_equiv.py --compare equiv_cpu.json equiv_cuda.json

Each job is the real grid job (run_job) at full model scale with ``steps`` shortened to STEPS
(the WSD schedule rescales as in build_cfg). Tolerance on held-out CE: |dL| / L <= TOL, a tenth
of the pre-registered 0.5% TOST margin, so device rounding cannot move a verdict by itself.
"""

from __future__ import annotations

import argparse
import json
import sys
import time
from concurrent.futures import ProcessPoolExecutor
from multiprocessing import get_context
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import parity_grid as pg  # noqa: E402

STEPS = 300
TOL = 0.1 * 0.005
JOBS = [
    ("A_flat_M8_H30_eta0.6", 0.6),
    ("B_sparseloco_M8_H30", 1.0),
    ("B_hier_R2x4_Hr5_Hg30", 0.6),
    ("DP_lr0.0003", None),
]


def job(cid: str, eta: float | None, device: str) -> dict:
    sc = pg.SCALES["full"] | {"steps": STEPS}
    base = {"key": f"{cid}|s0", "cell": cid, "seed": 0, "deadline": time.time() + 86400}
    base |= {"scale": sc, "mu": pg._outer_mu(), "device": device, "eta": eta}
    if cid.startswith("DP_"):
        return base | {"kind": "dp", "inner_lr": 3e-4, "spec": None}
    return base | {"kind": "diloco", "spec": pg.cell_design()[cid]}


def compare(a: dict, b: dict) -> dict:
    rows, ok = [], True
    for k, ra in a["rows"].items():
        rb = b["rows"][k]
        row = {"job": k}
        for m in ("heldout_loss", "heldout_loss_lawa", "init_loss"):
            if ra.get(m) is None or rb.get(m) is None:
                continue
            rel = abs(rb[m] - ra[m]) / ra[m]
            row[m] = {a["device"]: ra[m], b["device"]: rb[m], "rel_diff": rel}
            ok &= rel <= TOL and ra["status"] == rb["status"] == "DONE"
        rows.append(row)
    return {"tolerance_rel": TOL, "steps": STEPS, "pass": ok, "rows": rows}


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--device", default="cpu")
    ap.add_argument("--out", type=Path)
    ap.add_argument("--workers", type=int, default=4)
    ap.add_argument("--compare", nargs=2, type=Path)
    a = ap.parse_args()
    if a.compare:
        res = compare(*(json.loads(p.read_text()) for p in a.compare))
        print(json.dumps(res, indent=2))
        return 0 if res["pass"] else 1
    t0 = time.time()
    jobs = [job(c, e, a.device) for c, e in JOBS]
    with ProcessPoolExecutor(a.workers, mp_context=get_context("spawn")) as ex:
        rows = list(ex.map(pg.run_job, jobs))
    doc = {"device": a.device, "steps": STEPS, "wall_s": round(time.time() - t0, 1)}
    doc["rows"] = {r["key"]: r for r in rows}
    a.out.write_text(json.dumps(doc, indent=2, default=str) + "\n")
    print(json.dumps({k: (r["status"], r.get("heldout_loss")) for k, r in doc["rows"].items()}))
    return 0 if all(r["status"] == "DONE" for r in rows) else 1


if __name__ == "__main__":
    sys.exit(main())
