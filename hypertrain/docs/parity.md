# Parity specification

The question this document answers: if many replicas train in many datacenters, can the result match a centralized run on loss and on benchmarks (MMLU and the rest)? What we can claim today, what we measure, how we decide, and what stays an extrapolation.

**Read this first.** No public measurement shows flat DiLoCo-style training with many replicas reaching loss parity at 1B parameters or more. Parity is measured only for two replicas (M up to 2) at 2B to 10B parameters, and roughly at M=4 for 10B (+0.29% loss). A 100B parity claim is an extrapolation. A matched 100B centralized control costs as much as the run itself, so it will not exist. Everything below is written to make that extrapolation as honest as it can be.

Numbers tagged `value{ev:path#key}` are checked by `scripts/check_doc_numbers.py` against the pre-registration file [experiments/sim/parity_prereg.json](../experiments/sim/parity_prereg.json).

## 1. Topology: many replicas across datacenters

Each **island** is a full replica inside one datacenter (the example manifest layout is data-parallel by expert-parallel ranks in one box, see [protocol.md](protocol.md)). Islands are grouped into **regions**: a metro or continent cluster with several Gbit/s and under about 30 ms between datacenters.

```
  island  island  island        island  island  island
      \     |     /                 \     |     /
     regional relay (CPU)        regional relay (CPU)       every H_r inner steps
              \                         /
               +---- global aggregator ----+                 every H_g = K * H_r steps
```

- **Regional level.** `n_r = N/R` islands per region sync every `H_r` inner steps through a CPU relay inside the region. Dense int8 deltas, or SparseLoCo if links are weak. Plain averaging (`eta_r = 1`, no momentum) when `H_r <= 10`: small-H momentum hurt in an earlier toy ablation.
- **Global level.** Every `H_g` steps the `R` regional states are merged with the outer optimizer (Nesterov, or SparseLoCo with error feedback), with its learning rate tuned for `M = R`.
- **Relays forward, they do not sum.** For verification the regional aggregate is committed and recomputable (fixed-order fp32 sum, canonical codec, published inputs), and the global level sees per-region deltas.
- **Effective top-level replica count** `M_eff ~ R` if `H_r` is small enough that a region behaves like one data-parallel replica. This is a hypothesis, tested in section 4 as H1.

## 2. Formulas

All of these are derived, not measured, unless a tag says otherwise.

- **Batch and steps.** `B_glob = N * b`, steps `T = D / (B_glob * seq_len)`, global outer steps `S_g = T / H_g`. Keep each island's batch `b` at or above the value tuned on the proxy. Keep `S_g` at roughly 1 to 3 thousand or more (assumed): too many islands at fixed tokens starve the outer optimizer, so extra islands should add tokens rather than split them.
- **Gap estimate.** If the two levels add, `gap ~ g(R, H_g, N) + g(n_r, H_r, N)`, where `g` is read from published tables at matching model size.
- **Outer learning rate per level.** Each level's `eta` depends on its fan-in (`n_r` or `R`) and its `H`, not on `N` (Charles et al., arXiv 2503.09799, Finding 4). Starting grid for `eta_g`: 0.4, 0.6, 0.8, 1.0 with momentum 0.9.
- **Bandwidth per island per sync.** `bytes = P * rho * b_v / 8`, required rate `= bytes / (H * t_step)` with full overlap. Dense int8 at P = 100B is 100 GB per sync, about 80 Gbit/s at `H_r = 10` and a 10 s step. SparseLoCo at 1.56% density and about 20 bits per value is about 3.9 GB, about 3 Gbit/s. Consequence: near-data-parallel regional sync at 100B only works over a private metro fabric, not over internet links.

## 3. Schedule

1. **Stable phase** on the full hierarchy, WSD (warmup, stable, decay) learning rate.
2. **Decay phase**, the last 10 to 20% of tokens, on one or two islands or with all islands at `H_r = 1`, so the anneal has `M_eff ~ 1`. WSD with a short decay is known to match cosine; "many replicas, then single-replica anneal" has not been tested against data-parallel anywhere, and is assumed to recover part of the gap.
3. **Checkpoint averaging** over the last checkpoints of the decay.
4. **Optimizer state** carried across rounds (the CPU proxy decided `carry`; see [mechanism.md](mechanism.md)), which is why audits are segment audits.

Where parity is plausible: `R <= 2` (maybe 4), regional links fast enough for `H_r <= 10`, several billion parameters or more, outer learning rate tuned at the exact `M`, `H` and compression, a low-`M` anneal. Where it is not: flat `M >= 8` at `H >= 30` with AdamW (published: +1.1% loss at 2.4B, a 3-point MMLU drop at 15B), any setup on consumer-grade links, and MoE beyond about 4B active parameters, which has no DiLoCo evidence at all.

## 4. Pre-registered protocol

The design was frozen before any run: file [experiments/sim/parity_prereg.json](../experiments/sim/parity_prereg.json), registered at 2026-10-07T22:57:44Z, design hash `c14c4a5e1d9ebc8737cff9e8748d5b2b254806617c49578e82a1b3e04a886635`. The hash is stored in the file itself.

**Hypotheses**

- **H1 (key).** A hierarchy `(R, n_r)` with `H_r <= 10` is equivalent to a flat run with `M = R` at the same `H_g`.
- **H2.** The best outer learning rate grows with `M`.
- **H3.** Mechanisms rank by paired loss improvement over flat DiLoCo at the same `M`.
- **H4.** The gap to data-parallel at equal tokens grows with `M`.

**Grid.** 48{ev:experiments/sim/parity_prereg.json#n_cells} cells, seeds 0, 1 and 2, flat `M` from 1 to 32 and hierarchies (2 regions x 4 islands, 4 x 4, 2 x 8), `H_g` of 10, 30 and 100, `H_r` of 1, 5 and 10, Nesterov or SparseLoCo outer, AdamW or Muon inner, streaming fragments, anneal at full `M` or at `M = 1`, and final checkpoint averaging. Every cell is paired with a data-parallel control by seed: the seed fixes the initialization and the sample stream, so arms see identical data.

**Scale.** Full scale is 3000{ev:experiments/sim/parity_prereg.json#scales.full.steps} steps of a 4-layer model with `d_model` 128 and sequence length 256, global batch 32{ev:experiments/sim/parity_prereg.json#global_batch_seqs} sequences. Holdout is 512 fixed windows. The runner is capped at 12{ev:experiments/sim/parity_prereg.json#wall_clock_cap_hours} hours on at most 64{ev:experiments/sim/parity_prereg.json#max_processes} processes.

**Decision rule (TOST).**

- Primary metric: held-out cross-entropy on `data/holdout`.
- Gap per seed `(L_cell - L_dp) / L_dp`, with the data-parallel control at the matched inner learning rate (primary) and the best control rate (sensitivity).
- Equivalence: the paired t 90% confidence interval of the gap lies inside plus or minus 0.5%{ev:experiments/sim/parity_prereg.json#margins.loss_rel*100}. Significance level 0.05{ev:experiments/sim/parity_prereg.json#alpha} per one-sided test.
- Margins fixed for benchmark metrics at the GPU stage: MMLU 1.0{ev:experiments/sim/parity_prereg.json#margins.mmlu} point, benchmark mean 1.0{ev:experiments/sim/parity_prereg.json#margins.bench_mean} point, GSM8K 2.0{ev:experiments/sim/parity_prereg.json#margins.gsm8k} points.
- If the interval is not inside the margin, the verdict is "equivalence not demonstrated at delta = 0.005". It is never relabeled as "within noise".
- Outer learning rate per cell is chosen on the three seeds with the lowest held-out loss among non-divergent runs (ties go to the smaller value). The reuse of the same seeds for selection and testing biases the result in favor of the cell (winner's curse), and is acknowledged here.
- A mechanism is promoted to the GPU proxy only if it beats flat DiLoCo at the same `M` by a clear margin across seeds.
- With only three seeds the 90% interval half-width is about 1.74 times the seed standard deviation of the difference, so equivalence at 0.5% requires that spread to be small. The seed spread is measured first and reported next to every verdict.

**Scale ladder after the CPU grid** (needs a spend approval and is not part of this document's evidence): about 0.5B, then 1 to 3B, then one rung of 7B or more, each against a matched control, with the same TOST rule. MMLU and GSM8K sit near chance below about 3B, so use cloze-style MMLU at proxy scale and keep GSM8K for 7B and above.

## 5. Results of the full grid

<!-- RESULTS:todo13 -->
**Status: the full pre-registered grid ran, and parity with data-parallel training is not demonstrated for any cell.** All 150{ev:experiments/results/parity_vast_attempts.json#grid.jobs_done} jobs finished on 2026-10-12 at the registered scale of 1116032{ev:experiments/results/summary.json#model_params} parameters, with no errors, no censored jobs and no divergence. The design hash is unchanged. Source: [experiments/results/summary.json](../experiments/results/summary.json), rows in [parity_runs.jsonl](../experiments/results/parity_runs.jsonl) and [parity_grid.csv](../experiments/results/parity_grid.csv).

**How it ran.** The CPU hosts measured too slow (see the rented-host attempt below), so the simulator gained a CUDA path (`HYPERTRAIN_SIM_DEVICE=cuda`) and a batched path (`HYPERTRAIN_SIM_BATCHED=1`, [experiments/sim/batched.py](../experiments/sim/batched.py)) that trains all replicas of a sub-round as one stacked model. The device is an execution detail outside the design hash. The grid ran on one rented RTX 5090 (driver 595.71.05) with 8 concurrent jobs under CUDA MPS. The first batched rental used 16 jobs, ran out of GPU memory on the `M >= 16` cells, and its 64 finished rows were resumed on the second rental. Estimated spend for all four GPU rentals: USD 0.80{ev:experiments/results/parity_vast_attempts.json#gpu_spend.usd_est_sum}.

**CPU and CUDA give the same results within tolerance.** Four real grid jobs (flat `M = 8`, SparseLoCo `M = 8`, hierarchy 2 x 4 and the data-parallel control), shortened to 300{ev:experiments/results/parity_gpu_equiv.json#steps} steps at full model size, ran on the local CPU and on each rented GPU. The tolerance is a relative held-out loss difference of 0.05%{ev:experiments/results/parity_gpu_equiv.json#tolerance_rel*100}, one tenth of the TOST margin. The largest difference was 0.014%{ev:experiments/results/parity_gpu_equiv.json#runs.r1_per_replica.max_rel_diff_heldout*100} on the per-replica path and 0.004%{ev:experiments/results/parity_gpu_equiv.json#runs.r4_batched_grid_host.max_rel_diff_heldout*100} on the batched path that produced the grid, both from the SparseLoCo cell. Its top-k selection amplifies rounding; the other cells agree to about 1e-7. Muon, streaming and anneal cells were not compared on CUDA. On CPU the batched path matches the per-replica path within 1e-5 for AdamW, Muon and streaming (test `test_batched_replicas_match_per_replica`). Source: [parity_gpu_equiv.json](../experiments/results/parity_gpu_equiv.json).

**H1 (hierarchy behaves like flat `M = R`): rejected.** Not one of the 9 hierarchical cells at `H_g = 30` is equivalent to flat `M = R`. All of them are clearly worse, and the gap grows with `H_r` and with the number of islands per region:

- 2 x 4: +2.97{ev:experiments/results/summary.json#hierarchy_test.tests.0.vs_flat_M_eq_R.mean*100}% at `H_r = 1`, +4.20{ev:experiments/results/summary.json#hierarchy_test.tests.1.vs_flat_M_eq_R.mean*100}% at 5, +5.03{ev:experiments/results/summary.json#hierarchy_test.tests.2.vs_flat_M_eq_R.mean*100}% at 10.
- 4 x 4: +2.73{ev:experiments/results/summary.json#hierarchy_test.tests.3.vs_flat_M_eq_R.mean*100}%, +3.99{ev:experiments/results/summary.json#hierarchy_test.tests.4.vs_flat_M_eq_R.mean*100}%, +4.96{ev:experiments/results/summary.json#hierarchy_test.tests.5.vs_flat_M_eq_R.mean*100}%.
- 2 x 8: +4.69{ev:experiments/results/summary.json#hierarchy_test.tests.6.vs_flat_M_eq_R.mean*100}%, +7.19{ev:experiments/results/summary.json#hierarchy_test.tests.7.vs_flat_M_eq_R.mean*100}%, +9.07{ev:experiments/results/summary.json#hierarchy_test.tests.8.vs_flat_M_eq_R.mean*100}%.

Compared with flat DiLoCo at the full replica count `M = R x n_r`, a hierarchy with `H_r = 1` is better: -1.64{ev:experiments/results/summary.json#hierarchy_test.tests.0.vs_flat_M_eq_N.mean*100}% for 2 x 4, -1.93{ev:experiments/results/summary.json#hierarchy_test.tests.3.vs_flat_M_eq_N.mean*100}% for 4 x 4 and -2.27{ev:experiments/results/summary.json#hierarchy_test.tests.6.vs_flat_M_eq_N.mean*100}% for 2 x 8. At `H_r = 10` the advantage is gone. One cell (2 x 8, `H_r = 5`) passes TOST against flat `M = 16` (0.06{ev:experiments/results/summary.json#hierarchy_test.tests.7.vs_flat_M_eq_N.mean*100}%, 90% CI -0.37{ev:experiments/results/summary.json#hierarchy_test.tests.7.vs_flat_M_eq_N.ci90.0*100}% to 0.49{ev:experiments/results/summary.json#hierarchy_test.tests.7.vs_flat_M_eq_N.ci90.1*100}%). So at this scale a hierarchy is worth roughly as much as its total replica count, not its region count.

**H2 (best outer learning rate grows with `M`): weakly supported.** The best rate was 0.6{ev:experiments/results/summary.json#eta_trend.1.best_eta} at `M = 1`, 0.4{ev:experiments/results/summary.json#eta_trend.2.best_eta} at `M = 2`, 0.6{ev:experiments/results/summary.json#eta_trend.4.best_eta} at `M = 4`, and 1.0{ev:experiments/results/summary.json#eta_trend.8.best_eta}, 1.0{ev:experiments/results/summary.json#eta_trend.16.best_eta} and 1.0{ev:experiments/results/summary.json#eta_trend.32.best_eta} at `M` = 8, 16 and 32. The trend has the predicted sign. For `M >= 8` the best rate sits at the top of the grid, so the optimum is not bracketed and may lie higher.

**H3 (mechanism ranking at the same `M`).** Of the paired improvements over the best flat cell at the same `M`, the three that clear the two-sigma promotion rule by the largest margin are all Muon as the inner optimizer:

- Muon: 10.77{ev:experiments/results/summary.json#mechanism_ranking.1.mean_rel_improvement*100}% lower loss at `M = 32`, 8.03{ev:experiments/results/summary.json#mechanism_ranking.3.mean_rel_improvement*100}% at `M = 8`, 4.89{ev:experiments/results/summary.json#mechanism_ranking.5.mean_rel_improvement*100}% at `M = 2`.
- Hierarchy with frequent regional syncs: 2.25{ev:experiments/results/summary.json#mechanism_ranking.9.mean_rel_improvement*100}% for 2 x 4 at `H_r = 5`, `H_g = 10`; 2.27{ev:experiments/results/summary.json#mechanism_ranking.8.mean_rel_improvement*100}% for 2 x 8 at `H_r = 1`.
- Streaming with one chunk of overlap: 1.20{ev:experiments/results/summary.json#mechanism_ranking.14.mean_rel_improvement*100}% at `M = 8`.
- Final checkpoint averaging (LAWA): a small, consistent gain, for example 0.32{ev:experiments/results/summary.json#mechanism_ranking.23.mean_rel_improvement*100}% at `M = 8`.
- No gain: anneal at `M = 1` (-0.03{ev:experiments/results/summary.json#mechanism_ranking.29.mean_rel_improvement*100}% at `M = 8`).
- Worse: streaming without overlap (-3.65{ev:experiments/results/summary.json#mechanism_ranking.53.mean_rel_improvement*100}%), `H = 100` (-4.92{ev:experiments/results/summary.json#mechanism_ranking.55.mean_rel_improvement*100}%), and SparseLoCo, at -6.95{ev:experiments/results/summary.json#mechanism_ranking.57.mean_rel_improvement*100}% at `M = 2`, -14.67{ev:experiments/results/summary.json#mechanism_ranking.62.mean_rel_improvement*100}% at `M = 8` and -27.74{ev:experiments/results/summary.json#mechanism_ranking.65.mean_rel_improvement*100}% at `M = 32`.

The promotion list is `B_muon_M32_H30` (with and without LAWA) and `B_muon_M8_H30` with LAWA. Muon was not part of the CPU/CUDA equivalence comparison.

**H4 (gap to data-parallel grows with `M`): supported.** Gap of the best flat cell against the data-parallel control at the matched inner learning rate: +3.14{ev:experiments/results/summary.json#cells.2.gap_vs_dp*100}% at `M = 1`, +4.33{ev:experiments/results/summary.json#cells.8.gap_vs_dp*100}% at 2, +6.69{ev:experiments/results/summary.json#cells.18.gap_vs_dp*100}% at 4, +9.23{ev:experiments/results/summary.json#cells.30.gap_vs_dp*100}% at 8, +11.76{ev:experiments/results/summary.json#cells.36.gap_vs_dp*100}% at 16 and +15.62{ev:experiments/results/summary.json#cells.46.gap_vs_dp*100}% at 32. It grows monotonically. Against the better control learning rate (1e-3) every gap is about 3 points larger. Muon comes closest to the control: -0.77{ev:experiments/results/summary.json#cells.76.gap_vs_dp*100}% at `M = 2` (90% CI -1.62{ev:experiments/results/summary.json#cells.76.ci90.0*100}% to 0.07{ev:experiments/results/summary.json#cells.76.ci90.1*100}%) and 0.45{ev:experiments/results/summary.json#cells.80.gap_vs_dp*100}% at `M = 8` (90% CI -0.67{ev:experiments/results/summary.json#cells.80.ci90.0*100}% to 1.57{ev:experiments/results/summary.json#cells.80.ci90.1*100}%). Neither passes TOST. No cell is equivalent to data-parallel at the 0.5% margin.

**Power.** The seed spread of the paired differences was 0.0056{ev:experiments/results/summary.json#power.sd_diff}, so the 90% interval half-width at n = 3{ev:experiments/results/summary.json#power.n} seeds is 0.0094{ev:experiments/results/summary.json#power.ci90_half_width}, wider than the 0.005{ev:experiments/results/summary.json#power.delta} margin. Reaching the margin needs 12{ev:experiments/results/summary.json#power.n_required} seeds. Most negative verdicts above are not affected, because their intervals lie entirely outside the margin. A "not demonstrated" verdict whose interval straddles zero (for example Muon at `M = 2` or `M = 8` against data-parallel) stays inconclusive.

**Scope.** These are 1.1M-parameter results with one dataset and no benchmarks. They rank mechanisms and show directions. They make no claim about 1B+ models, where published gaps are smaller (section 6).

**Rented-host attempt on CPU, 2026-10-11/12: too slow.** Four many-core Vast CPU hosts measured 11.6{ev:experiments/results/parity_vast_attempts.json#rentals.2.s_per_step_under_64_workers} to 34.59{ev:experiments/results/parity_vast_attempts.json#rentals.0.s_per_step_under_64_workers} s per inner step under 64-process load, against 1.86{ev:experiments/results/parity_vast_attempts.json#local_idle_s_per_step} s idle locally, and aggregate throughput stayed flat as processes were added. Source: [parity_vast_attempts.json](../experiments/results/parity_vast_attempts.json).

**How to re-run.** `python scripts/vast_parity.py --gpu 1 --gpu-workers 8 --equiv-cpu <cpu equiv json> --key-file KEY --max-usd 4.4 --max-minutes 420` rents a 1x RTX 5090 host. It reruns the CUDA equivalence jobs and compares them with the CPU file (refusing the host on a mismatch), probes throughput, refuses the host if the projection exceeds the budget, and runs the grid with `--resume-dir` support. Locally: `HYPERTRAIN_SIM_DEVICE=cuda HYPERTRAIN_SIM_BATCHED=1 uv run --frozen python experiments/sim/parity_grid.py --full --workers 8`. Raising the seed count changes the design hash and needs a new pre-registration (`--write-prereg`). Evidence: `.omo/evidence/hypertrain-complete-20261010/parity-gpu.md`.
<!-- /RESULTS:todo13 -->

## 6. What the CPU grid can and cannot say

From the file itself (`limits`): it is a tiny CPU proxy of about 1M parameters. It can rank mechanisms by sign and order, test whether the hierarchy behaves like a flat run with `M = R`, find divergence regions, and show how the best outer learning rate moves with `M` and `H`. It cannot give absolute gaps at 1B or more (published gaps there are 4 to 10 times smaller than at 35M), cannot give the right learning rate below about 335M parameters, and says nothing about benchmarks, MoE routing drift, BF16 numerics on RTX 5090 or network behavior.

## 7. Claims we make and claims we don't

- We claim a pre-registered, falsifiable protocol, with the decision rule fixed before data, and a negative control that makes the harness able to fail.
- We do not claim 100B parity. It is an extrapolation from a CPU proxy, from published results at 2B to 15B for small `M`, and from the audit and aggregation design. A matched 100B control run will not be done.
- If the GPU ladder shows equivalence only up to some `M_eff` and model size, we report that point and the confidence interval, and nothing beyond it.

## 8. Verification interaction

A hierarchical round is not replayable from the global state alone, so each regional aggregate is committed and recomputable and the auditor substitutes it at the boundary. Optimizer state resets only at the global round boundary. Merges are pinned to a step (`sync_step + tau`), never wall-clock, and any asynchronous merge is recorded in a signed event tape that replay consumes. Details of the audit math are in [mechanism.md](mechanism.md).
