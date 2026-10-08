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

## 5. Results of the CPU grid

<!-- RESULTS:todo13 -->
**Status: full-scale parity NOT demonstrated.** The full grid was launched on 2026-10-07 and stopped by the user on a CPU and energy limit before any job finished.

- **Jobs.** 150{ev:experiments/results/summary.json#full_scale_status.jobs_censored} full-scale jobs were censored and 0{ev:experiments/results/summary.json#full_scale_status.jobs_done} finished. There is no gap, no confidence interval, no TOST result and no promotion at the pre-registered scale of 1116032{ev:experiments/results/summary.json#model_params} parameters. Source: [experiments/results/summary.json](../experiments/results/summary.json).
- **Hypotheses H1 to H4 are untested.** Not one of them has data at full scale.
- **Why this is not a pass or a fail.** A censored run says nothing about the margin. Reading "no result" as "no difference" would be the mistake the protocol exists to prevent.

**Smoke run (non-informative scale).** The same 48 cells and 3 seeds ran at a 37k-parameter plumbing scale (600 steps, sequence length 64) to exercise the harness: 150{ev:experiments/results/summary.json#smoke_results.jobs_done} jobs finished, on 37216{ev:experiments/results/summary.json#smoke_results.model_params} parameters. The file labels it "non-informative scale", and that label stands: these numbers support no parity and no mechanism claim. Two hints are recorded only so the next full run knows what to look at.

- Hierarchical cells came out worse than the flat run with `M = R`, not equivalent to it. For 2 regions x 4 islands and 4 x 4 the paired gap ranged from 2.72%{ev:experiments/results/summary_smoke.json#hierarchy_test.tests.3.vs_flat_M_eq_R.mean*100} to 3.67%{ev:experiments/results/summary_smoke.json#hierarchy_test.tests.2.vs_flat_M_eq_R.mean*100} (loss relative to flat), and the four 2 x 8 cells (three plain, one with a single-island anneal) ranged from 5.16%{ev:experiments/results/summary_smoke.json#hierarchy_test.tests.6.vs_flat_M_eq_R.mean*100} to 5.22%{ev:experiments/results/summary_smoke.json#hierarchy_test.tests.11.vs_flat_M_eq_R.mean*100}. That contradicts H1 at this scale, but a 37k-parameter model with 64-token sequences cannot confirm or kill it.
- The best outer learning rate was 1.0 at `M` of 1, 2, 4, 8 and 16 (1.0{ev:experiments/results/summary_smoke.json#eta_trend.16.best_eta} at `M = 16`) and 0.8 at `M = 32` (0.8{ev:experiments/results/summary_smoke.json#eta_trend.32.best_eta}). The grid's top value was best almost everywhere, so the grid probably did not bracket the optimum.

**Underpowered even if it had finished.** The smoke seed spread of paired differences was 0.0042{ev:experiments/results/summary_smoke.json#power.sd_diff} (relative), giving a 90% interval half-width of 0.0071{ev:experiments/results/summary_smoke.json#power.ci90_half_width} against the margin of 0.005{ev:experiments/results/summary_smoke.json#power.delta} with n = 3{ev:experiments/results/summary_smoke.json#power.n} seeds. The power check requires at least 8{ev:experiments/results/summary_smoke.json#power.n_required} seeds to reach the 0.5% margin. Three seeds can reject equivalence but can almost never demonstrate it, so a rerun must raise the seed count before spending compute.

**How to re-run.** Get explicit approval for the CPU time first. The runner measures its 12 hour cap from the start time stored in `experiments/results/parity_state.json`, and that cap has already passed, so delete that file to restart the clock. Finished rows in `parity_runs.jsonl` are kept and skipped. Then start `uv run --frozen python experiments/sim/parity_grid.py --full`. To raise the seed count, edit `SEEDS` in `experiments/sim/parity_grid.py` (it is part of the design hash), then archive the old `parity_prereg.json`, write a new pre-registration with `--write-prereg`, and keep the old one next to it: the runner refuses to start if the design hash differs from the registered one. Evidence for this closure: `.omo/evidence/hypertrain-challenge/task-13-hypertrain-challenge.json`.
<!-- /RESULTS:todo13 -->

## 6. What the CPU grid can and cannot say

From the file itself (`limits`): it is a tiny CPU proxy of about 1M parameters. It can rank mechanisms by sign and order, test whether the hierarchy behaves like a flat run with `M = R`, find divergence regions, and show how the best outer learning rate moves with `M` and `H`. It cannot give absolute gaps at 1B or more (published gaps there are 4 to 10 times smaller than at 35M), cannot give the right learning rate below about 335M parameters, and says nothing about benchmarks, MoE routing drift, BF16 numerics on RTX 5090 or network behavior.

## 7. Claims we make and claims we don't

- We claim a pre-registered, falsifiable protocol, with the decision rule fixed before data, and a negative control that makes the harness able to fail.
- We do not claim 100B parity. It is an extrapolation from a CPU proxy, from published results at 2B to 15B for small `M`, and from the audit and aggregation design. A matched 100B control run will not be done.
- If the GPU ladder shows equivalence only up to some `M_eff` and model size, we report that point and the confidence interval, and nothing beyond it.

## 8. Verification interaction

A hierarchical round is not replayable from the global state alone, so each regional aggregate is committed and recomputable and the auditor substitutes it at the boundary. Optimizer state resets only at the global round boundary. Merges are pinned to a step (`sync_step + tau`), never wall-clock, and any asynchronous merge is recorded in a signed event tape that replay consumes. Details of the audit math are in [mechanism.md](mechanism.md).
