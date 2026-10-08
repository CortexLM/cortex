"""Todo 7: inner-optimizer state-policy decision experiment (CPU, todo-4 shards).

  uv run --frozen --extra trainer python experiments/sim/state_policy.py --full
  uv run --frozen --extra trainer python experiments/sim/state_policy.py --smoke \
      --sabotage-outer-lr 10 --out-dir /tmp/x

Arms A0 carry, A1 reset, A2 reset + re-warmup (10% of H), A3 derived v0, NEG (outer momentum
-0.9). Each arm's outer lr is tuned on {0.2, 0.4, 0.7, 1.0} per (M, H) cell (lowest mean held-out
loss over seeds). Paired seeds (same init + data across arms). Gap_s = (L_arm - L_A0) / L_A0.
PASS(arm): every cell mean gap <= +0.25%, 95% t-CI upper <= +0.5%, no divergent seed at the
chosen lr, and the NEG gate holds (every cell: NEG mean gap > +0.25% and CI lower > 0).
Decision: first PASS in A1, A2, A3 -> manifest state_policy; else carry + segment audits.
Scope: tiny CPU proxy only; says nothing about GPU numerics or quality at scale.
"""

from __future__ import annotations

import argparse
import csv
import json
import math
import os
import statistics
import sys
import time
import traceback
from concurrent.futures import FIRST_COMPLETED, ProcessPoolExecutor, wait
from dataclasses import asdict, replace
from multiprocessing import get_context
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(Path(__file__).resolve().parent))

OUTER_LR_GRID = (0.2, 0.4, 0.7, 1.0)
ARM_ORDER = ("A0", "A1", "A2", "A3", "NEG")
T975 = {1: 12.706, 2: 4.303, 3: 3.182, 4: 2.776, 5: 2.571}
GAP_MEAN_MAX, GAP_CI_MAX = 0.0025, 0.005
N_HELDOUT = 256

PROVENANCE = {
    "model.*, inner.lr/betas/eps/wd/grad_clip/micro_batch, outer.momentum=0.9, outer.bits=8": (
        "hypertrain.protocol.example.example_manifest (docs/schemas/example-run-manifest.json)"
    ),
    "inner.lr_schedule WSD": (
        "example manifest shape (warmup 100, decay 19% of 10000 steps) rescaled to the run "
        "length: warmup=min(100, steps//10), decay=round(0.19*steps), stable=rest"
    ),
    "outer.lr": "tuned per arm per (M,H) cell on grid {0.2,0.4,0.7,1.0} (plan todo 7)",
    "arms": "ultrabrain section 2 (A0 carry, A1 reset, A2 re-warmup 10% H, A3 derived, NEG)",
    "NEG outer momentum=-0.9": "sign-flipped manifest momentum (deliberately wrong)",
    "grid M{2,4} x H{30,100} x seeds{0,1,2}": "plan todo 7",
    "PASS thresholds 0.25% / 0.5% CI95": "plan todo 7 / ultrabrain section 2",
    "cpu_threads": "example manifest reference_spec.env.cpu_threads",
    "held-out": f"{N_HELDOUT} fixed windows of data/holdout (evenly spaced ids)",
}


def wsd(steps: int) -> dict[str, int]:
    warmup = min(100, steps // 10)
    decay = round(0.19 * steps)
    return {"warmup": warmup, "stable": steps - warmup - decay, "decay": decay}


def base_cfg(seed: int, steps: int, seq_len: int | None = None, tiny: bool = False) -> Any:
    from hypertrain.protocol.example import example_manifest
    from hypertrain.trainer.config import TrainConfig

    cfg = TrainConfig.from_manifest(example_manifest().body())
    model = replace(cfg.model, init_seed=seed)
    if tiny:  # unit-test size only (tests/sim); never used by --full
        model = replace(model, n_layers=2, d_model=32, n_heads=4, n_kv_heads=4, d_ff=64)
    if seq_len is not None:
        model = replace(model, seq_len=seq_len)
    s = wsd(steps)
    inner = replace(cfg.inner, warmup=s["warmup"], stable=s["stable"], decay=s["decay"])
    return replace(cfg, model=model, inner=inner)


def outer_momentum() -> float:
    from hypertrain.protocol.example import example_manifest
    from hypertrain.protocol.messages import f32val

    return f32val(example_manifest().body()["outer"]["momentum"])  # type: ignore[index]


def run_job(job: dict[str, Any]) -> dict[str, Any]:
    """Process-pool entry: one simulated run (or DP control) + held-out eval."""
    from diloco import ARMS, ChunkedShards, SimSpec, data_parallel, heldout_loss, simulate

    import hypertrain.trainer  # noqa: F401  (determinism before torch in the child)

    t0 = time.time()
    row = {k: job[k] for k in ("kind", "arm", "M", "H", "seed", "outer_lr", "steps")}
    try:
        if time.time() > job["deadline"]:
            return row | {"status": "CENSORED", "heldout_loss": "", "diverged": ""}
        cfg = base_cfg(job["seed"], job["steps"], job.get("seq_len"), job.get("tiny", False))
        tr = ChunkedShards(ROOT / "data" / "train", cfg.model.seq_len)
        ho = ChunkedShards(ROOT / "data" / "holdout", cfg.model.seq_len)
        ho_ids = list(range(0, ho.n, ho.n // job.get("n_heldout", N_HELDOUT)))
        ho_ids = ho_ids[: job.get("n_heldout", N_HELDOUT)]
        arm = ARMS[job["arm"] if job["kind"] == "diloco" else "A0"]
        spec = SimSpec(
            M=job["M"],
            H=job["H"],
            steps=job["steps"],
            outer="nesterov",
            outer_lr=job["outer_lr"] or 1.0,
            outer_momentum=job["mu"],
            arm=arm,
            seed=job["seed"],
        )
        if job["kind"] == "dp":
            res = data_parallel(spec, cfg, tr, tr.n)
        else:
            res = simulate(spec, cfg, tr, tr.n, deadline=job["deadline"])
        if res.get("censored"):
            return row | {"status": "CENSORED", "heldout_loss": "", "diverged": ""}
        loss = heldout_loss(cfg, res["theta"], ho_ids, ho)
        init_loss = job.get("init_loss")
        diverged = bool(res["diverged"]) or not math.isfinite(loss)
        reason = res["reason"]
        if init_loss is not None and math.isfinite(loss) and loss >= init_loss:
            diverged, reason = True, reason or f"held-out {loss:.4f} >= init {init_loss:.4f}"
        return row | {
            "status": "DONE",
            "heldout_loss": loss,
            "diverged": diverged,
            "reason": reason,
            "outer_momentum": arm.outer_momentum if arm.outer_momentum is not None else job["mu"],
            "state_policy": arm.state_policy,
            "rewarmup_steps": round(arm.rewarmup_frac * job["H"]),
            "final_train_loss": res["trace"][-1] if res["trace"] else "",
            "theta_hash": res["theta_hash"],
            "wall_s": round(time.time() - t0, 1),
            "pid": os.getpid(),
        }
    except Exception:  # keep the failure verbatim in the CSV; never drop the row
        return row | {"status": "ERROR", "reason": traceback.format_exc()[-2000:]}


def init_eval(seed: int, steps: int, seq_len: int | None, tiny: bool, n_heldout: int) -> float:
    from diloco import ChunkedShards, heldout_loss

    import hypertrain.trainer  # noqa: F401
    from hypertrain.trainer.model import init_params

    cfg = base_cfg(seed, steps, seq_len, tiny)
    ho = ChunkedShards(ROOT / "data" / "holdout", cfg.model.seq_len)
    ids = list(range(0, ho.n, ho.n // n_heldout))[:n_heldout]
    return heldout_loss(cfg, init_params(cfg.model), ids, ho)


def ci95(xs: list[float]) -> tuple[float, float, float]:
    m = statistics.fmean(xs)
    if len(xs) < 2:
        return m, -math.inf, math.inf
    h = T975.get(len(xs) - 1, 1.96) * statistics.stdev(xs) / math.sqrt(len(xs))
    return m, m - h, m + h


def decide(
    rows: list[dict[str, Any]],
    seeds: list[int],
    cells: list[tuple[int, int]],
    lr_grid: tuple[float, ...] = OUTER_LR_GRID,
) -> Any:
    done = [r for r in rows if r["kind"] == "diloco" and r["status"] == "DONE"]
    out: dict[str, Any] = {"cells": {}, "arms": {}}
    best: dict[tuple[str, int, int], dict[str, Any]] = {}
    for M, H in cells:
        key = f"M{M}_H{H}"
        out["cells"][key] = {"tuning": {}, "arms": {}}
        for arm in ARM_ORDER:
            tune = {}
            for lr in lr_grid:
                rs = [
                    r for r in done if (r["arm"], r["M"], r["H"], r["outer_lr"]) == (arm, M, H, lr)
                ]
                losses = {r["seed"]: r["heldout_loss"] for r in rs}
                div = any(r["diverged"] for r in rs)
                complete = sorted(losses) == sorted(seeds)
                score = statistics.fmean(losses.values()) if complete and not div else math.inf
                tune[str(lr)] = {"mean_loss": score, "complete": complete, "any_divergent": div}
            out["cells"][key]["tuning"][arm] = tune
            finite = {lr: v["mean_loss"] for lr, v in tune.items() if math.isfinite(v["mean_loss"])}
            if finite:
                best_lr = min(finite, key=lambda k: (finite[k], float(k)))
                best[(arm, M, H)] = {"lr": float(best_lr), "censored_or_divergent": False}
            else:
                any_done = any(r for r in done if (r["arm"], r["M"], r["H"]) == (arm, M, H))
                best[(arm, M, H)] = {"lr": None, "censored_or_divergent": True, "any": any_done}
        a0 = best[("A0", M, H)]
        for arm in ARM_ORDER:
            b = best[(arm, M, H)]
            cell: dict[str, Any] = {"outer_lr": b["lr"]}
            if b["lr"] is None or a0["lr"] is None:
                cell |= {"status": "CENSORED_OR_DIVERGENT", "pass": False}
                out["cells"][key]["arms"][arm] = cell
                continue

            def loss_of(a: str, s: int, lr: float, M: int = M, H: int = H) -> float:
                return next(
                    r["heldout_loss"]
                    for r in done
                    if (r["arm"], r["M"], r["H"], r["outer_lr"], r["seed"]) == (a, M, H, lr, s)
                )

            gaps = [
                (loss_of(arm, s, b["lr"]) - loss_of("A0", s, a0["lr"])) / loss_of("A0", s, a0["lr"])
                for s in seeds
            ]
            m, lo, hi = ci95(gaps)
            cell |= {
                "status": "MEASURED",
                "gaps": gaps,
                "mean_gap": m,
                "ci95": [lo, hi],
                "losses": {str(s): loss_of(arm, s, b["lr"]) for s in seeds},
                "pass": m <= GAP_MEAN_MAX and hi <= GAP_CI_MAX,
            }
            dp = [r for r in rows if r["kind"] == "dp" and (r["M"], r["H"]) == (M, H)]
            if dp and all(r["status"] == "DONE" for r in dp):
                dpl = {r["seed"]: r["heldout_loss"] for r in dp}
                dg = [(loss_of(arm, s, b["lr"]) - dpl[s]) / dpl[s] for s in seeds if s in dpl]
                cell["gap_vs_dp"] = {"mean": statistics.fmean(dg), "ci95": list(ci95(dg)[1:])}
            out["cells"][key]["arms"][arm] = cell
    neg_ok = all(
        c["arms"]["NEG"].get("status") == "MEASURED"
        and c["arms"]["NEG"]["mean_gap"] > GAP_MEAN_MAX
        and c["arms"]["NEG"]["ci95"][0] > 0
        for c in out["cells"].values()
    )
    out["neg_gate"] = {
        "rule": "every cell: NEG mean gap > +0.25% and CI95 lower bound > 0",
        "pass": neg_ok,
    }
    for arm in ARM_ORDER[1:4]:
        cs = [c["arms"][arm] for c in out["cells"].values()]
        out["arms"][arm] = {
            "pass": neg_ok and all(c["pass"] for c in cs),
            "cells_pass": [c["pass"] for c in cs],
            "verdict": "PASS" if neg_ok and all(c["pass"] for c in cs) else "FAIL",
        }
    out["arms"]["A0"] = {"verdict": "CONTROL"}
    out["arms"]["NEG"] = {"verdict": "WORSE (gate holds)" if neg_ok else "GATE FAILED"}
    policy = {"A1": "reset", "A2": "reset", "A3": "derived"}
    chosen = next((a for a in ("A1", "A2", "A3") if out["arms"][a]["pass"]), None)
    if chosen is None:
        out["decision"] = {
            "chosen_arm": "A0",
            "state_policy": "carry",
            "rewarmup_steps_frac_H": 0.0,
            "audit_mode": "carry + segment audits (ultrabrain 3b)",
            "why": "no arm of A1/A2/A3 passed"
            + ("" if neg_ok else " (and the NEG gate failed: experiment cannot discriminate)"),
        }
    else:
        out["decision"] = {
            "chosen_arm": chosen,
            "state_policy": policy[chosen],
            "rewarmup_steps_frac_H": 0.1 if chosen == "A2" else 0.0,
            "audit_mode": "full-round replay (ultrabrain 3a)",
            "why": f"first passing arm in order A1, A2, A3 is {chosen}",
        }
    return out


def main() -> int:
    ap = argparse.ArgumentParser()
    g = ap.add_mutually_exclusive_group(required=True)
    g.add_argument("--full", action="store_true")
    g.add_argument("--smoke", action="store_true")
    ap.add_argument("--sabotage-outer-lr", type=float, default=None)
    ap.add_argument("--steps", type=int, default=None)
    ap.add_argument("--workers", type=int, default=min(64, os.cpu_count() or 1))
    ap.add_argument("--cap-hours", type=float, default=6.0)
    ap.add_argument("--out-dir", type=Path, default=ROOT / "experiments" / "results")
    args = ap.parse_args()
    if args.sabotage_outer_lr is not None and not (args.sabotage_outer_lr > 0):
        ap.error("--sabotage-outer-lr must be > 0")
    if args.full:
        Ms, Hs, seeds, steps = [2, 4], [30, 100], [0, 1, 2], args.steps or 600
        seq_len, tiny, n_ho = None, False, N_HELDOUT
    else:
        Ms, Hs, seeds, steps = [2], [5], [0, 1, 2], args.steps or 60
        seq_len, tiny, n_ho = 64, False, 64
    for H in Hs:
        if steps % H:
            ap.error(f"steps={steps} must be a multiple of every H")
    lr_grid: tuple[float, ...] = OUTER_LR_GRID
    if args.sabotage_outer_lr is not None:
        lr_grid = (args.sabotage_outer_lr,)
    mu = outer_momentum()
    t_start = time.time()
    deadline = t_start + args.cap_hours * 3600
    args.out_dir.mkdir(parents=True, exist_ok=True)
    log = (args.out_dir / "progress.log").open("a")

    def say(msg: str) -> None:
        line = f"[{time.strftime('%H:%M:%S')}] {msg}"
        print(line, flush=True)
        log.write(line + "\n")
        log.flush()

    ctx = get_context("spawn")
    with ProcessPoolExecutor(max_workers=min(args.workers, len(seeds)), mp_context=ctx) as ex:
        init_losses = dict(
            zip(
                seeds,
                ex.map(init_eval, seeds, *([x] * len(seeds) for x in (steps, seq_len, tiny, n_ho))),
                strict=True,
            )
        )
    say(f"init held-out losses {init_losses}")
    jobs: list[dict[str, Any]] = []
    common: dict[str, Any] = {
        "steps": steps,
        "seq_len": seq_len,
        "tiny": tiny,
        "n_heldout": n_ho,
        "mu": mu,
    }
    common["deadline"] = deadline
    for M in Ms:
        for H in Hs:
            for s in seeds:
                base = common | {"M": M, "H": H, "seed": s, "init_loss": init_losses[s]}
                jobs.append(base | {"kind": "dp", "arm": "DP", "outer_lr": None})
                for arm in ARM_ORDER:
                    for lr in lr_grid:
                        jobs.append(base | {"kind": "diloco", "arm": arm, "outer_lr": lr})
    jobs.sort(key=lambda j: -int(j["M"]))  # longest first
    say(f"{len(jobs)} jobs, steps={steps}, workers={args.workers}, cap={args.cap_hours}h")
    rows: list[dict[str, Any]] = []
    with ProcessPoolExecutor(max_workers=args.workers, mp_context=ctx) as ex:
        pending = {ex.submit(run_job, j) for j in jobs}
        while pending:
            fin, pending = wait(pending, return_when=FIRST_COMPLETED)
            for f in fin:
                r = f.result()
                rows.append(r)
                say(
                    f"{len(rows)}/{len(jobs)} {r['kind']} {r['arm']} M{r['M']} H{r['H']} "
                    f"s{r['seed']} lr{r['outer_lr']} -> {r['status']} "
                    f"loss={r.get('heldout_loss')} div={r.get('diverged')} "
                    f"{(r.get('reason') or '')[:120]}"
                )
    rows.sort(key=lambda r: (r["kind"], r["arm"], r["M"], r["H"], str(r["outer_lr"]), r["seed"]))
    cols = [
        "kind", "arm", "M", "H", "seed", "outer_lr", "outer_momentum", "state_policy",
        "rewarmup_steps", "steps", "status", "heldout_loss", "final_train_loss", "diverged",
        "reason", "theta_hash", "wall_s", "pid",
    ]  # fmt: skip
    name = "state_policy" if args.full else "state_policy_smoke"
    with (args.out_dir / f"{name}.csv").open("w", newline="") as f:
        w = csv.DictWriter(f, fieldnames=cols, extrasaction="ignore")
        w.writeheader()
        w.writerows(rows)
    errors = [r for r in rows if r["status"] == "ERROR"]
    cells = [(M, H) for M in Ms for H in Hs]
    dec = decide(rows, seeds, cells, lr_grid)
    from diloco import n_params

    dec |= {
        "mode": "full" if args.full else "smoke",
        "sabotage_outer_lr": args.sabotage_outer_lr,
        "grid": {"M": Ms, "H": Hs, "seeds": seeds, "outer_lr": list(lr_grid), "steps": steps},
        "model_params": n_params(base_cfg(0, steps, seq_len, tiny)),
        "base_config": json.loads(json.dumps(asdict(base_cfg(0, steps, seq_len, tiny)))),
        "outer": {"opt": "nesterov", "momentum": mu, "codec": "dense-int8 (manifest bits=8)"},
        "init_heldout_loss": init_losses,
        "divergent_runs": [
            {k: r.get(k) for k in ("arm", "M", "H", "seed", "outer_lr", "reason")}
            for r in rows
            if r.get("diverged") is True
        ],
        "censored": [
            {k: r[k] for k in ("kind", "arm", "M", "H", "seed", "outer_lr")}
            for r in rows
            if r["status"] == "CENSORED"
        ],
        "errors": [
            {"job": {k: r[k] for k in ("kind", "arm", "M", "H")}, "trace": r["reason"]}
            for r in errors
        ],
        "wall_s": round(time.time() - t_start, 1),
        "provenance": PROVENANCE,
        "caveat": (
            "tiny CPU proxy (~4M params, few tokens/param); proves nothing about GPU/BF16 "
            "numerics or quality at scale; confirm on a 0.5-1B GPU proxy (needs approval)"
        ),
    }
    dname = "decision.json" if args.full else "decision_smoke.json"
    (args.out_dir / dname).write_text(json.dumps(dec, indent=2, default=str) + "\n")
    say(f"decision: {dec['decision']} neg_gate={dec['neg_gate']['pass']} errors={len(errors)}")
    say("arms: " + json.dumps({a: v["verdict"] for a, v in dec["arms"].items()}))
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
