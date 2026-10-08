"""Todo 13: CPU parity grid (many replicas across regions) on the todo-7 simulator.

  uv run --frozen python experiments/sim/parity_grid.py --write-prereg   # before any run
  uv run --frozen python experiments/sim/parity_grid.py --smoke
  uv run --frozen python experiments/sim/parity_grid.py --full            # resumable
  uv run --frozen python experiments/sim/parity_grid.py --full --summarize-only \
      [--inject-shift CELL:0.01]

Design (pre-registered in experiments/sim/parity_prereg.json, sha-checked at start):
stage A = outer Nesterov eta sweep on flat DiLoCo (H=30) per M; stage B = mechanisms, each using
the stage-A best eta of its reference M (hierarchy: M=R). Global batch 32 sequences per step for
every arm (per-replica batch 32/M), equal tokens, DP control at the same tokens and samples.
Inner state policy carry everywhere (todo-7 decision). LAWA is a free evaluation variant of each
cell (no extra training). Scope: tiny CPU proxy; ranks mechanisms, never claims 100B or GPU parity.
"""

from __future__ import annotations

import argparse
import csv
import hashlib
import json
import math
import os
import statistics
import sys
import time
import traceback
from concurrent.futures import FIRST_COMPLETED, Future, ProcessPoolExecutor, wait
from dataclasses import replace
from multiprocessing import get_context
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
# HYPERTRAIN_DATA_DIR: corpus root holding train/ and holdout/ (default: <repo>/data, unpublished)
DATA = Path(os.environ.get("HYPERTRAIN_DATA_DIR", ROOT / "data"))
HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
PREREG = HERE / "parity_prereg.json"

SEEDS = (0, 1, 2)
ETA5 = (0.2, 0.4, 0.6, 0.8, 1.0)
ETA3 = (0.2, 0.6, 1.0)
GLOBAL_BATCH = 32
CAP_HOURS = 12.0
WORKERS = 64
ANNEAL_FRAC = 0.15
SPARSE = {"topk_frac": 0.0156, "bits": 2, "ef_beta": 0.95, "outer_lr": 1.0}
DP_INNER_LRS = (3e-4, 1e-3)
PROMOTE_SIGMA = 2.0

SCALES: dict[str, dict[str, Any]] = {
    "full": {
        "steps": 3000,
        "d_model": 128,
        "d_ff": 512,
        "n_layers": 4,
        "n_heads": 4,
        "seq_len": 256,
        "n_heldout": 512,
        "lawa": (75, 4),
    },
    "smoke": {
        "steps": 600,
        "d_model": 32,
        "d_ff": 64,
        "n_layers": 2,
        "n_heads": 4,
        "seq_len": 64,
        "n_heldout": 64,
        "lawa": (15, 4),
    },
}

PROVENANCE = {
    "flat M {1,2,4,8,16,32}, hier (R,n_r) {(2,4),(4,4),(2,8)}, H_g {10,30,100}, H_r {1,5,10}": (
        "plan todo 13 / parity draft section 4"
    ),
    "eta sweep {0.2,0.4,0.6,0.8,1.0} (3-level {0.2,0.6,1.0} at M 1/4/16)": (
        "plan todo 13 (fractional: full sweep at M 2/8/32)"
    ),
    "regional sync plain average (eta_r=1, no momentum)": "parity draft section 2 (H_r <= 10)",
    "outer momentum 0.9, dense-int8 codec": "example manifest outer.momentum / outer.bits=8",
    "SparseLoCo topk 1.56%, 2-bit, EF beta 0.95, outer lr 1": (
        "research-compression-a2 (2508.15706 / Covenant: chunk top-k 64/4096, EF .95, lr 1); "
        "codec is per-tensor top-k (ponytail: not chunked)"
    ),
    "inner AdamW lr 3e-4, betas, eps, wd, clip; muon momentum 0.95, ns 5": (
        "example manifest (UNSOURCED provisional values, shared by every arm)"
    ),
    "WSD warmup=min(100,S/10), decay=19% of S": "example manifest shape rescaled (as todo 7)",
    "state_policy carry": "todo 7 decision.json (A1/A2/A3 all FAIL)",
    "anneal final 15% of tokens at M_eff=1": "plan todo 13 / parity draft section 2 schedule",
    "model d128 x4 layers (~1.1M params), 3000 steps x 32 seq x 256 tok (~22 tok/param)": (
        "parity draft section 4 (2-10M params, 20-50 tok/param) bounded by the 12 h cap at "
        "<=64 processes (measured ~4.4k tok/s/process)"
    ),
    "margins loss +-0.5% rel, MMLU +-1.0, bench mean +-1.0, GSM8K +-2.0, alpha 0.05": (
        "plan todo 13 / parity draft section 3"
    ),
    "LAWA last 4 snapshots every 2.5% of steps": "UNSOURCED (pre-registered choice)",
    "DP inner lr {3e-4 (matched), 1e-3 (sensitivity)}": "equal-effort caveat, parity section 3",
}


def cell_design() -> dict[str, dict[str, Any]]:
    """All training cells (<= 48). eta=None -> best stage-A eta of flat M=eta_ref at H=30."""
    c: dict[str, dict[str, Any]] = {}

    def add(cid: str, **kw: Any) -> None:
        base = {
            "stage": "B",
            "topo": "flat",
            "M": 1,
            "R": None,
            "H_r": 30,
            "H_g": 30,
            "outer": "nesterov",
            "eta": None,
            "eta_ref": None,
            "inner": "adamw",
            "fragments": 1,
            "tau": 0,
            "anneal": False,
            "family": "",
            "baseline": None,
        }
        c[cid] = base | kw

    for M in (1, 2, 4, 8, 16, 32):
        for eta in ETA5 if M in (2, 8, 32) else ETA3:
            add(f"A_flat_M{M}_H30_eta{eta}", stage="A", M=M, eta=eta, family="eta_sweep")
    for R, nr in ((2, 4), (4, 4), (2, 8)):
        for hr in (1, 5, 10):
            add(
                f"B_hier_R{R}x{nr}_Hr{hr}_Hg30",
                topo="hier",
                M=R * nr,
                R=R,
                H_r=hr,
                eta_ref=R,
                family="hierarchy",
                baseline=R * nr,
            )
    for hg in (10, 100):
        add(
            f"B_hier_R2x4_Hr5_Hg{hg}",
            topo="hier",
            M=8,
            R=2,
            H_r=5,
            H_g=hg,
            eta_ref=2,
            family="hierarchy_Hg",
            baseline=8,
        )
        add(f"B_flat_M8_H{hg}", M=8, H_r=hg, H_g=hg, eta_ref=8, family="H_g", baseline=8)
    for M in (2, 8, 32):
        add(
            f"B_sparseloco_M{M}_H30",
            M=M,
            outer="sparseloco",
            eta=SPARSE["outer_lr"],
            family="sparseloco",
            baseline=M,
        )
        add(f"B_muon_M{M}_H30", M=M, inner="muon", eta_ref=M, family="muon", baseline=M)
    add("B_stream_M8_P3_tau0", M=8, fragments=3, eta_ref=8, family="streaming", baseline=8)
    add("B_stream_M8_P3_tau1", M=8, fragments=3, tau=1, eta_ref=8, family="streaming", baseline=8)
    for M in (8, 32):
        add(f"B_anneal1_M{M}_H30", M=M, anneal=True, eta_ref=M, family="anneal", baseline=M)
    add(
        "B_anneal1_hier_R2x8_Hr5_Hg30",
        topo="hier",
        M=16,
        R=2,
        H_r=5,
        anneal=True,
        eta_ref=2,
        family="anneal",
        baseline=16,
    )
    assert len(c) <= 48, len(c)
    return c


def design_doc() -> dict[str, Any]:
    cells = cell_design()
    doc = {
        "format": "hypertrain-parity-prereg-v1",
        "title": "Todo 13 CPU parity grid: many replicas across regions vs DP control",
        "hypotheses": [
            "H1 (key): hierarchical (R, n_r) with H_r <= 10 is equivalent to flat M=R at H_g "
            "(TOST, loss margin 0.5% rel)",
            "H2: best outer eta grows with M (2503.09799 Finding 4)",
            "H3: mechanisms ranked by paired loss improvement over flat DiLoCo at the same M",
            "H4: gap vs DP at equal tokens grows with M",
        ],
        "cells": cells,
        "n_cells": len(cells),
        "seeds": list(SEEDS),
        "controls": {
            "dp": {"inner_lr": list(DP_INNER_LRS), "primary": DP_INNER_LRS[0]},
            "pairing": "seed s fixes init (init_seed=s) and the sample stream (assign_round keyed "
            "by s); for H=30 every arm sees the same samples per 30-step window",
        },
        "scales": SCALES,
        "global_batch_seqs": GLOBAL_BATCH,
        "wall_clock_cap_hours": CAP_HOURS,
        "max_processes": WORKERS,
        "margins": {"loss_rel": 0.005, "mmlu": 1.0, "bench_mean": 1.0, "gsm8k": 2.0},
        "alpha": 0.05,
        "analysis": {
            "primary_metric": "held-out CE on data/holdout (fixed evenly spaced windows)",
            "gap": "per seed (L_cell - L_dp) / L_dp, DP at matched inner lr 3e-4 (primary); "
            "sensitivity vs best DP inner lr",
            "tost": "paired t 90% CI of the gap inside (-0.005, +0.005) -> equivalent; else "
            "'equivalence not demonstrated at delta=0.005'",
            "eta_selection": "stage A: lowest mean held-out loss over all 3 seeds among etas "
            "with every seed DONE and none divergent (tie -> smaller eta); stage-B cells use the "
            "best eta of flat M=eta_ref (hierarchy: M=R). Winner's-curse bias acknowledged",
            "hierarchy_test": "TOST of hier cell vs flat M=R (H=H_g) and vs flat M=R*n_r, paired",
            "promotion": "cell beats flat DiLoCo at the same M (best eta, H=30) when mean paired "
            "relative improvement > 2 * sd/sqrt(n); top 3 by improvement -> GPU proxy list",
            "lawa": "every cell also evaluated on the mean of its last 4 global snapshots",
            "divergence": "non-finite or held-out >= init loss -> divergent, excluded from eta "
            "choice, reported",
            "censoring": "jobs not finished by the cap are CENSORED and listed; no imputation",
        },
        "limits": "1.1M-param CPU proxy: ranks/signs only; absolute gaps at >=1B are 4-10x "
        "smaller, eta below 335M is unstable, no benchmark/MoE/BF16/network behavior; no 100B "
        "parity claim (extrapolation only)",
        "provenance": PROVENANCE,
    }
    body = json.dumps(doc, sort_keys=True, separators=(",", ":"))
    return doc | {"design_sha256": hashlib.sha256(body.encode()).hexdigest()}


def build_cfg(scale: dict[str, Any], seed: int, micro_batch: int, opt: str = "adamw") -> Any:
    from hypertrain.protocol.example import example_manifest
    from hypertrain.trainer.config import TrainConfig

    cfg = TrainConfig.from_manifest(example_manifest().body())
    model = replace(
        cfg.model,
        init_seed=seed,
        d_model=scale["d_model"],
        d_ff=scale["d_ff"],
        n_layers=scale["n_layers"],
        n_heads=scale["n_heads"],
        n_kv_heads=scale["n_heads"],
        seq_len=scale["seq_len"],
    )
    S = scale["steps"]
    warmup, decay = min(100, S // 10), round(0.19 * S)
    inner = replace(
        cfg.inner,
        warmup=warmup,
        stable=S - warmup - decay,
        decay=decay,
        micro_batch=micro_batch,
        opt=opt,
        state_policy="carry",
        rewarmup_steps=0,
    )
    return replace(cfg, model=model, inner=inner)


def _outer_mu() -> float:
    from hypertrain.protocol.example import example_manifest
    from hypertrain.protocol.messages import f32val

    return f32val(example_manifest().body()["outer"]["momentum"])  # type: ignore[index]


def run_job(job: dict[str, Any]) -> dict[str, Any]:
    """Process-pool entry: one cell x seed (or DP control) -> held-out losses."""
    from diloco import (
        ARMS,
        ChunkedShards,
        SimSpec,
        anneal_single,
        data_parallel,
        heldout_loss,
        simulate,
        simulate_streaming,
    )

    import hypertrain.trainer  # noqa: F401  (determinism pin before torch)
    from hypertrain.trainer.config import CompressConfig
    from hypertrain.trainer.model import init_params

    t0 = time.time()
    row: dict[str, Any] = {"key": job["key"], "cell": job["cell"], "seed": job["seed"]}
    try:
        if time.time() > job["deadline"]:
            return row | {"status": "CENSORED", "reason": "cap reached before start"}
        sc, c = job["scale"], job["spec"]
        S, s = sc["steps"], job["seed"]
        tr = ChunkedShards(DATA / "train", sc["seq_len"])
        ho = ChunkedShards(DATA / "holdout", sc["seq_len"])
        ho_ids = list(range(0, ho.n, ho.n // sc["n_heldout"]))[: sc["n_heldout"]]
        if job["kind"] == "dp":
            cfg = build_cfg(sc, s, GLOBAL_BATCH)
            cfg = replace(cfg, inner=replace(cfg.inner, lr=job["inner_lr"]))
            spec = SimSpec(
                M=1,
                H=30,
                steps=S,
                outer="average",
                outer_lr=1.0,
                outer_momentum=0.0,
                arm=ARMS["A0"],
                seed=s,
            )
            res = data_parallel(spec, cfg, tr, tr.n)
        else:
            M = c["M"]
            cfg = build_cfg(sc, s, GLOBAL_BATCH // M, c["inner"])
            sparse = None
            if c["outer"] == "sparseloco":
                sparse = CompressConfig("sparseloco", SPARSE["topk_frac"], 2, SPARSE["ef_beta"])
            hier = c["topo"] == "hier"
            steps = S - round(ANNEAL_FRAC * S) if c["anneal"] else S
            spec = SimSpec(
                M=M,
                H=c["H_r"],
                K=c["H_g"] // c["H_r"],
                R=c["R"] if hier else None,
                steps=steps,
                outer=c["outer"],
                outer_lr=job["eta"],
                outer_momentum=job["mu"],
                arm=ARMS["A0"],
                seed=s,
                sparse=sparse,
            )
            if c["fragments"] > 1:
                res = simulate_streaming(
                    spec, cfg, tr, tr.n, c["fragments"], c["tau"], deadline=job["deadline"]
                )
            else:
                lawa = None if c["anneal"] else tuple(sc["lawa"])
                res = simulate(spec, cfg, tr, tr.n, deadline=job["deadline"], lawa=lawa)
                if c["anneal"] and not res.get("censored") and not res["diverged"]:
                    res = anneal_single(spec, cfg, tr, tr.n, res, S)
        if res.get("censored"):
            return row | {"status": "CENSORED", "reason": "cap reached mid-run"}
        loss = heldout_loss(cfg, res["theta"], ho_ids, ho)
        init_loss = heldout_loss(cfg, init_params(cfg.model), ho_ids, ho)
        lawa_loss = None
        if res.get("lawa_theta") is not None and res.get("lawa_k", 0) >= sc["lawa"][1]:
            lawa_loss = heldout_loss(cfg, res["lawa_theta"], ho_ids, ho)
        diverged = bool(res["diverged"]) or not math.isfinite(loss) or loss >= init_loss
        return row | {
            "status": "DONE",
            "heldout_loss": loss,
            "heldout_loss_lawa": lawa_loss,
            "init_loss": init_loss,
            "diverged": diverged,
            "reason": res["reason"] or ("held-out >= init" if loss >= init_loss else ""),
            "eta": job.get("eta"),
            "inner_lr": cfg.inner.lr,
            "final_train_loss": res["trace"][-1] if res["trace"] else None,
            "theta_hash": res["theta_hash"],
            "wall_s": round(time.time() - t0, 1),
            "pid": os.getpid(),
        }
    except Exception:  # keep the failure verbatim in the results; never drop the row
        return row | {"status": "ERROR", "reason": traceback.format_exc()[-3000:]}


def _done(rows: dict[str, dict[str, Any]], key: str) -> dict[str, Any] | None:
    r = rows.get(key)
    return r if r is not None and r["status"] == "DONE" else None


def best_eta(cells: dict[str, Any], rows: dict[str, Any], M: int) -> tuple[float | None, dict]:
    tune: dict[str, Any] = {}
    for cid, c in cells.items():
        if c["stage"] != "A" or c["M"] != M:
            continue
        rs = [_done(rows, f"{cid}|s{s}") for s in SEEDS]
        ok = all(r is not None for r in rs) and not any(r["diverged"] for r in rs if r)
        tune[str(c["eta"])] = statistics.fmean(r["heldout_loss"] for r in rs) if ok else None  # type: ignore[index]
    fin = {float(k): v for k, v in tune.items() if v is not None}
    if not fin:
        return None, tune
    return min(fin, key=lambda e: (fin[e], e)), tune


def stage_a_finished(cells: dict[str, Any], rows: dict[str, Any], M: int) -> bool:
    return all(
        f"{cid}|s{s}" in rows and rows[f"{cid}|s{s}"]["status"] != "CENSORED"
        for cid, c in cells.items()
        if c["stage"] == "A" and c["M"] == M
        for s in SEEDS
    )


def cell_losses(rows: dict[str, Any], cid: str, lawa: bool = False) -> dict[int, float] | None:
    out = {}
    for s in SEEDS:
        r = _done(rows, f"{cid}|s{s}")
        if r is None or r["diverged"]:
            return None
        v = r["heldout_loss_lawa"] if lawa else r["heldout_loss"]
        if v is None:
            return None
        out[s] = float(v)
    return out


def summarize(
    design: dict[str, Any], rows: dict[str, Any], inject: tuple[str, float] | None = None
) -> dict[str, Any]:
    from hypertrain.parity import MARGINS, power_check, tost_paired

    cells = design["cells"]
    d = MARGINS["loss_rel"]
    dp = {lr: cell_losses(rows, f"DP_lr{lr:g}") for lr in DP_INNER_LRS}
    dp_main = dp[DP_INNER_LRS[0]]
    dp_means = {lr: statistics.fmean(v.values()) for lr, v in dp.items() if v}
    dp_best_lr = min(dp_means, key=lambda k: dp_means[k]) if dp_means else None
    etas = {M: best_eta(cells, rows, M) for M in (1, 2, 4, 8, 16, 32)}
    losses: dict[str, dict[int, float]] = {}
    for cid in cells:
        v = cell_losses(rows, cid)
        if v is not None:
            losses[cid] = v
        vl = cell_losses(rows, cid, lawa=True)
        if vl is not None:
            losses[cid + "+lawa"] = vl
    if inject is not None:
        src, shift = inject
        base = dp_main if src == "DP" else losses.get(src)
        if base is None:
            raise SystemExit(f"--inject-shift source {src} has no complete losses")
        losses[f"INJECT[{src}+{shift:g}]"] = {s: x * (1 + shift) for s, x in base.items()}
        if src == "DP":
            losses["INJECT[DP+0]"] = dict(base)

    def rel(a: dict[int, float], b: dict[int, float]) -> list[float]:
        return [(a[s] - b[s]) / b[s] for s in SEEDS]

    gap, ci90, tost, cell_rows = {}, {}, {}, []
    for cid, v in losses.items():
        base_id = cid.split("+lawa")[0]
        c = cells.get(base_id, {})
        entry: dict[str, Any] = {
            "cell": cid,
            **{
                k: c.get(k)
                for k in (
                    "family",
                    "M",
                    "R",
                    "H_r",
                    "H_g",
                    "outer",
                    "inner",
                    "fragments",
                    "tau",
                    "anneal",
                )
            },
        }
        entry["mean_loss"] = statistics.fmean(v.values())
        if c.get("stage") == "A":
            entry["eta"] = c["eta"]
        elif base_id in cells:
            entry["eta"] = c["eta"] if c["eta"] is not None else etas[c["eta_ref"]][0]
        if dp_main:
            t = tost_paired(rel(v, dp_main), d, "loss_rel")
            gap[cid], ci90[cid], tost[cid] = t.mean, list(t.ci90), t.as_dict()
            entry |= {"gap_vs_dp": t.mean, "ci90": list(t.ci90), "tost": t.verdict}
            if dp_best_lr is not None:
                tb = tost_paired(rel(v, dp[dp_best_lr]), d)  # type: ignore[arg-type]
                entry["gap_vs_best_dp"] = {
                    "inner_lr": dp_best_lr,
                    "mean": tb.mean,
                    "ci90": list(tb.ci90),
                }
        cell_rows.append(entry)

    def flat_best(M: int) -> str | None:
        e = etas.get(M, (None, {}))[0]
        cid = f"A_flat_M{M}_H30_eta{e}"
        return cid if e is not None and cid in losses else None

    hierarchy = []
    for cid, c in cells.items():
        if c["topo"] != "hier" or cid not in losses:
            continue
        h: dict[str, Any] = {
            "cell": cid,
            "R": c["R"],
            "n_r": c["M"] // c["R"],
            "H_r": c["H_r"],
            "H_g": c["H_g"],
            "anneal": c["anneal"],
        }
        for tag, M in (("vs_flat_M_eq_R", c["R"]), ("vs_flat_M_eq_N", c["M"])):
            ref = flat_best(M)
            if c["H_g"] != 30:
                ref = (
                    f"B_flat_M8_H{c['H_g']}"
                    if M == 8 and f"B_flat_M8_H{c['H_g']}" in losses
                    else None
                )
            if ref is None:
                h[tag] = {
                    "ref": None,
                    "result": "no complete reference cell (not in design at "
                    "this H_g, or censored/divergent)",
                }
                continue
            t = tost_paired(rel(losses[cid], losses[ref]), d)
            h[tag] = {
                "ref": ref,
                "mean": t.mean,
                "ci90": list(t.ci90),
                "equivalent": t.equivalent,
                "verdict": t.verdict,
            }
        hierarchy.append(h)
    holds = [h for h in hierarchy if not h["anneal"] and "equivalent" in h["vs_flat_M_eq_R"]]
    hier_summary = {
        "question": "does hierarchy (R, n_r) behave like flat M=R (TOST, delta 0.5% rel)?",
        "equivalent_cells": [h["cell"] for h in holds if h["vs_flat_M_eq_R"]["equivalent"]],
        "not_demonstrated_cells": [
            h["cell"] for h in holds if not h["vs_flat_M_eq_R"]["equivalent"]
        ],
        "tests": hierarchy,
    }

    promotion, comparisons = [], []
    for cid, v in losses.items():
        base_id = cid.split("+lawa")[0]
        c = cells.get(base_id)
        if c is None or (c["stage"] == "A" and not cid.endswith("+lawa")):
            continue
        ref = flat_best(c["M"])
        if ref is None or ref == cid:
            continue
        imp = [-x for x in rel(v, losses[ref])]
        m, sd = statistics.fmean(imp), statistics.stdev(imp)
        se = sd / math.sqrt(len(imp))
        beats = m > PROMOTE_SIGMA * se and m > 0
        comparisons.append(
            {
                "cell": cid,
                "flat_ref": ref,
                "mean_rel_improvement": m,
                "se": se,
                "beats_flat_2sigma": beats,
            }
        )
    comparisons.sort(key=lambda x: -x["mean_rel_improvement"])
    promotion = [x for x in comparisons if x["beats_flat_2sigma"]][:3]

    eta_trend = {str(M): {"best_eta": e, "mean_loss_by_eta": t} for M, (e, t) in etas.items()}
    sds = [statistics.stdev(rel(v, dp_main)) for v in losses.values() if dp_main]
    pw = power_check(statistics.median(sds), len(SEEDS), d) if sds else None
    return {
        "cells": cell_rows,
        "gap": gap,
        "ci90": ci90,
        "tost_result": tost,
        "promotion_list": promotion,
        "mechanism_ranking": comparisons,
        "hierarchy_test": hier_summary,
        "eta_trend": eta_trend,
        "dp_control": {str(k): v for k, v in dp.items()} | {"best_inner_lr": dp_best_lr},
        "power": None if pw is None else pw.__dict__,
        "injected_shift": None if inject is None else {"source": inject[0], "shift": inject[1]},
    }


def load_rows(path: Path) -> dict[str, dict[str, Any]]:
    rows: dict[str, dict[str, Any]] = {}
    if path.exists():
        for line in path.read_text().splitlines():
            if line.strip():
                r = json.loads(line)
                if r["status"] == "DONE" or r["key"] not in rows:
                    rows[r["key"]] = r
    return rows


def main() -> int:
    ap = argparse.ArgumentParser()
    g = ap.add_mutually_exclusive_group(required=True)
    g.add_argument("--full", action="store_true")
    g.add_argument("--smoke", action="store_true")
    g.add_argument("--write-prereg", action="store_true")
    ap.add_argument("--summarize-only", action="store_true")
    ap.add_argument("--inject-shift", default=None, help="CELL:frac, e.g. DP:0.01")
    ap.add_argument("--workers", type=int, default=WORKERS)
    ap.add_argument("--out-dir", type=Path, default=ROOT / "experiments" / "results")
    args = ap.parse_args()
    doc = design_doc()
    if args.write_prereg:
        if PREREG.exists():
            print(f"{PREREG} exists; refusing to overwrite a pre-registration")
            return 2
        PREREG.write_text(
            json.dumps(
                doc | {"registered_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())},
                indent=2,
            )
            + "\n"
        )
        print(f"wrote {PREREG} sha={doc['design_sha256']}")
        return 0
    reg = json.loads(PREREG.read_text())
    if reg["design_sha256"] != doc["design_sha256"]:
        print("design differs from the pre-registration; refusing to run")
        return 2
    if args.workers > WORKERS:
        ap.error(f"--workers must be <= {WORKERS} (pre-registered)")
    mode = "full" if args.full else "smoke"
    scale = SCALES[mode]
    sfx = "" if args.full else "_smoke"
    out = args.out_dir
    out.mkdir(parents=True, exist_ok=True)
    runs_path = out / f"parity_runs{sfx}.jsonl"
    state_path = out / f"parity_state{sfx}.json"
    cells = doc["cells"]
    inject = None
    if args.inject_shift:
        src, frac = args.inject_shift.rsplit(":", 1)
        inject = (src, float(frac))
    if not state_path.exists():
        state_path.write_text(json.dumps({"t_start": time.time(), "mode": mode}) + "\n")
    t_start = json.loads(state_path.read_text())["t_start"]
    deadline = t_start + CAP_HOURS * 3600
    log = (out / f"parity_progress{sfx}.log").open("a")

    def say(msg: str) -> None:
        line = f"[{time.strftime('%H:%M:%S')}] {msg}"
        print(line, flush=True)
        log.write(line + "\n")
        log.flush()

    rows = load_rows(runs_path)
    if not args.summarize_only:
        mu = _outer_mu()
        todo: dict[str, dict[str, Any]] = {}
        for lr in DP_INNER_LRS:
            for s in SEEDS:
                k = f"DP_lr{lr:g}|s{s}"
                todo[k] = {
                    "key": k,
                    "cell": f"DP_lr{lr:g}",
                    "seed": s,
                    "kind": "dp",
                    "inner_lr": lr,
                    "spec": None,
                }
        for cid, c in cells.items():
            for s in SEEDS:
                k = f"{cid}|s{s}"
                todo[k] = {"key": k, "cell": cid, "seed": s, "kind": "diloco", "spec": c}
        for k in [
            k for k in todo if _done(rows, k) or (k in rows and rows[k]["status"] == "ERROR")
        ]:
            del todo[k]
        order = sorted(todo, key=lambda k: (todo[k]["spec"] or {}).get("stage", "A") != "A")
        say(
            f"{mode}: {len(todo)} jobs to run ({len(rows)} rows already on disk), "
            f"workers={args.workers}, deadline in {(deadline - time.time()) / 3600:.2f} h"
        )
        fh = runs_path.open("a")

        def record(r: dict[str, Any]) -> None:
            rows[r["key"]] = r
            fh.write(json.dumps(r, default=str) + "\n")
            fh.flush()
            os.fsync(fh.fileno())

        ctx = get_context("spawn")
        running: dict[Future[dict[str, Any]], str] = {}
        with ProcessPoolExecutor(max_workers=args.workers, mp_context=ctx) as ex:
            while order or running:
                for k in list(order):
                    if len(running) >= args.workers:
                        break
                    j = todo[k]
                    c = j["spec"]
                    if c is not None and c["eta"] is None:
                        if not stage_a_finished(cells, rows, c["eta_ref"]):
                            continue
                        e, _ = best_eta(cells, rows, c["eta_ref"])
                        if e is None:
                            order.remove(k)
                            record(
                                {
                                    "key": k,
                                    "cell": j["cell"],
                                    "seed": j["seed"],
                                    "status": "CENSORED",
                                    "reason": f"no usable stage-A eta for M={c['eta_ref']}",
                                }
                            )
                            continue
                        j["eta"] = e
                    elif c is not None:
                        j["eta"] = c["eta"]
                    j |= {"deadline": deadline, "scale": scale, "mu": mu}
                    running[ex.submit(run_job, j)] = k
                    order.remove(k)
                if not running:
                    if order:
                        say(f"deadlock: {len(order)} jobs wait on unfinished stage A")
                        for k in order:
                            record(
                                {
                                    "key": k,
                                    "cell": todo[k]["cell"],
                                    "seed": todo[k]["seed"],
                                    "status": "CENSORED",
                                    "reason": "dependency unresolved",
                                }
                            )
                    break
                fin, _ = wait(list(running), return_when=FIRST_COMPLETED)
                for f in fin:
                    k = running.pop(f)
                    try:
                        r = f.result()
                    except Exception:  # worker crash: record, never drop
                        r = {
                            "key": k,
                            "cell": todo[k]["cell"],
                            "seed": todo[k]["seed"],
                            "status": "ERROR",
                            "reason": traceback.format_exc()[-3000:],
                        }
                    record(r)
                    say(
                        f"{sum(1 for x in rows.values() if x['status'] == 'DONE')} done | {k} -> "
                        f"{r['status']} loss={r.get('heldout_loss')} lawa="
                        f"{r.get('heldout_loss_lawa')} eta={r.get('eta')} div={r.get('diverged')}"
                        f" wall={r.get('wall_s')} {(r.get('reason') or '')[:160]}"
                    )
        fh.close()
    cols = [
        "key",
        "cell",
        "seed",
        "status",
        "eta",
        "inner_lr",
        "heldout_loss",
        "heldout_loss_lawa",
        "init_loss",
        "final_train_loss",
        "diverged",
        "reason",
        "theta_hash",
        "wall_s",
        "pid",
    ]
    with (out / f"parity_grid{sfx}.csv").open("w", newline="") as f:
        w = csv.DictWriter(f, fieldnames=cols, extrasaction="ignore")
        w.writeheader()
        w.writerows(sorted(rows.values(), key=lambda r: r["key"]))
    summ = summarize(doc, rows, inject)
    from diloco import n_params

    summ |= {
        "mode": mode,
        "prereg": {"path": str(PREREG.relative_to(ROOT)), "design_sha256": doc["design_sha256"]},
        "model_params": n_params(build_cfg(scale, 0, GLOBAL_BATCH)),
        "scale": scale,
        "censored": sorted(k for k, r in rows.items() if r["status"] == "CENSORED"),
        "errors": {k: r["reason"] for k, r in rows.items() if r["status"] == "ERROR"},
        "divergent": sorted(k for k, r in rows.items() if r.get("diverged") is True),
        "wall_s": round(time.time() - t_start, 1),
        "caveat": design_doc()["limits"],
    }
    name = f"summary{sfx}" + ("_inject" if inject else "")
    (out / f"{name}.json").write_text(json.dumps(summ, indent=2, default=str) + "\n")
    say(
        f"summary -> {name}.json; promotion={[p['cell'] for p in summ['promotion_list']]}; "
        f"censored={len(summ['censored'])} errors={len(summ['errors'])}"
    )
    if inject:
        for k, v in summ["tost_result"].items():
            if k.startswith("INJECT"):
                say(f"{k}: {v['verdict']}")
    return 1 if summ["errors"] else 0


if __name__ == "__main__":
    sys.exit(main())
