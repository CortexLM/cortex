# Results report

Final results of the hypertrain challenge build, with the budget reconciliation and an honest qualification status. Numbers tagged `value{ev:path#key}` are checked by `scripts/check_doc_numbers.py` against [experiments/results/final_results.json](../experiments/results/final_results.json). Evidence paths below are in the workspace evidence directory `.omo/evidence/hypertrain-challenge/` (not part of the published tree).

## 1. Status

**Equivalence not demonstrated at >=0.5B; GPU proxy ladder gated.** The determinism machinery works on real hardware. Training-quality parity with a centralized run is unproven at every scale that matters.

| Gate | Status | Evidence |
| --- | --- | --- |
| Todos 1 to 20 built and independently verified | PASS | verify-1-r2.json, verify-2-r2.json, verify-3-r2.json, verify-4-r2.json, verify-5-r2.json, verify-6-r2.json, verify-7.json, verify-8-r4.json, verify-9-r2.json, verify-10-r2.json, verify-11-r3.json, verify-12.json, verify-14.json, verify-15.json, verify-16.json, verify-17.json, verify-18.json, verify-19.json, verify-20.json |
| State policy decision (todo 7) | PASS: CARRY plus segment audits | task-7-hypertrain-challenge.json, verify-7.json |
| Phase A: cross-host bitwise determinism, 3 hosts (todo 12) | PASS | task-12-hypertrain-challenge.json, verify-12-phaseA.json |
| Phase B: cross-host bitwise determinism, 2 x 8 RTX 5090 (todo 16) | PASS | task-16-hypertrain-challenge.json, verify-16.json |
| Determinism overhead base/det at most 1.6 | FAIL (RE-PLAN flag) | task-16-hypertrain-challenge.json |
| BBR network measurement | CENSORED | task-16-hypertrain-challenge.json |
| CPU parity grid, many replicas (todo 13) | CENSORED, 0 of 150 jobs | task-13-hypertrain-challenge.json, verify-13.json |
| End-to-end network scenarios (todo 17) | PASS, with one limit (section 5) | task-17-hypertrain-challenge.json, verify-17.json |
| Publication (todo 20) | DONE | push-main-r2.log, verify-20.json |

## 2. What was built

A challenge network with a miner, an aggregator, auditors and a ledger. Honest miners are paid after a vesting delay. Cheaters are caught by beacon-sampled bitwise replay of committed training segments, with dispute bisection and rollback. Docs: [protocol](protocol.md), [mechanism](mechanism.md), [miner](miner.md), [operator](operator.md), [parity](parity.md). Every one of todos 1 to 20 was confirmed by an independent reviewer (the verify files above; earlier needs-fix rounds are kept next to the final ones).

## 3. State-policy decision (todo 7)

On the 4.3M-parameter CPU proxy, none of the reset, re-warmup or derived-state arms passed the gate, and the negative control held. Decision: carry the inner optimizer state across rounds and audit segments. The proxy is tiny, so this is a decision for the mechanism, not a quality claim. Evidence: task-7-hypertrain-challenge.json.

## 4. Determinism on real GPUs

**Phase A (todo 12).** 3{ev:experiments/results/final_results.json#phase_a.hosts} distinct RTX 5090 hosts with three driver versions gave 6{ev:experiments/results/final_results.json#phase_a.identical_honest_runs} honest runs with identical leaves root, delta and theta. The negative control diverged at leaf 11{ev:experiments/results/final_results.json#phase_a.neg_control_divergent_leaf}, as designed. Verdict PASS. Cost USD0.63{ev:experiments/results/final_results.json#budget.phase_a_usd} settled.

**Phase B (todo 16).** A 654965760{ev:experiments/results/final_results.json#phase_b.params}-parameter MoE ran on 2{ev:experiments/results/final_results.json#phase_b.hosts} hosts of 8{ev:experiments/results/final_results.json#phase_b.gpus_per_host} RTX 5090 each (Quebec, driver 580.95.05; Taiwan, driver 580.159.03). Leaves, delta hash, top-k and final theta were bitwise equal across the two hosts. Verdict PASS.

**Overhead: FAIL against the plan.** Baseline over deterministic throughput was 1.97{ev:experiments/results/final_results.json#phase_b.overhead_h0} on the Quebec host and 2.06{ev:experiments/results/final_results.json#phase_b.overhead_h1} on the Taiwan host, above the plan's 1.6{ev:experiments/results/final_results.json#phase_b.replan_threshold}x RE-PLAN flag. The Quebec host was PCIe limited (NCCL about 2.7 GB/s against about 24.5 GB/s on Taiwan). The baseline arm also used TF32, bf16 reduced-precision reductions and cuDNN autotune, so the ratio mixes determinism cost with precision and kernel choices. Recommended follow-up: a baseline that changes only the determinism flags, one flag at a time, to split the gap between TF32, bf16 reductions, cuDNN autotune and determinism itself.

**Network.** Cubic iperf numbers are recorded in task-16-hypertrain-challenge.json. BBR is **CENSORED**: Vast containers don't expose `tcp_bbr`.

## 5. End-to-end

Todo 17 runs 14{ev:experiments/results/final_results.json#e2e.tests} tests over real separate processes, all passing (task-17-hypertrain-challenge.json, verify-17.json). With replay disabled (`HT_E2E_NO_REPLAY=1`) 7{ev:experiments/results/final_results.json#e2e.no_replay_failed} fail and 7{ev:experiments/results/final_results.json#e2e.no_replay_passed} pass, so the suite does notice a missing audit. **Limit:** after a late refactor, one full run plus the NO_REPLAY run were not repeated, on the user's CPU instruction. The last full run before the refactor is the evidence.

## 6. Parity: CENSORED

The pre-registered CPU grid (many replicas, many regions) finished 0{ev:experiments/results/final_results.json#parity.jobs_finished} of 150{ev:experiments/results/final_results.json#parity.jobs_total} jobs before the user's CPU and energy stop. No gap, no confidence interval, no TOST result, no promotion. Smoke runs exist but carry no parity claim. **Full-scale parity is NOT demonstrated** (task-13-hypertrain-challenge.json, verify-13.json, experiments/sim/parity_prereg.json). Published results elsewhere show parity only up to two replicas, so a many-replica 100B claim stays an extrapolation (see [parity](parity.md)).

## 7. Budget reconciliation

Source: GET-only reads of the Vast account on 2026-10-08 (raw: task-21-vast-acct-raw, task-21-vast-inv-raw, task-21-vast-vols-raw, task-21-vast-charges-raw). No create or PUT call was made. Cap: min(USD50{ev:experiments/results/final_results.json#budget.nominal_cap_usd}, prepaid credit) = USD33.48{ev:experiments/results/final_results.json#budget.cap_usd}.

| Item | USD |
| --- | --- |
| Opening credit (budget-snapshot-0.json) | 33.48{ev:experiments/results/final_results.json#budget.opening_credit_usd} |
| Phase A, settled (verify-12-phaseA.json) | 0.63{ev:experiments/results/final_results.json#budget.phase_a_usd} |
| Phase B attempts a1 to a3 (host and provider faults) | 1.832{ev:experiments/results/final_results.json#budget.phase_b_round1_a1_a3_usd} |
| Phase B b1 (code fault, OOM) | 4.809{ev:experiments/results/final_results.json#budget.phase_b_b1_usd} |
| Phase B b2 (PASS) | 4.928{ev:experiments/results/final_results.json#budget.phase_b_b2_usd} |
| Phase B total (task-16-hypertrain-challenge.json) | 11.569{ev:experiments/results/final_results.json#budget.phase_b_usd} |
| Phases A and B attributed | 12.199{ev:experiments/results/final_results.json#budget.attributed_usd} |
| Billing after the last Phase B read (not attributed to a run) | 0.331{ev:experiments/results/final_results.json#budget.post_final_get_residual_usd} |
| **Total debit** (opening credit minus current credit) | **12.53{ev:experiments/results/final_results.json#budget.total_debit_usd}** |
| Current credit | 20.95{ev:experiments/results/final_results.json#budget.current_credit_usd} |
| Headroom under cap | 20.95{ev:experiments/results/final_results.json#budget.headroom_usd} |

Owned instances: 0{ev:experiments/results/final_results.json#budget.owned_instances}. Volumes: 0{ev:experiments/results/final_results.json#budget.volumes}. The account shows one payment (USD 50, 2026-09-01) and no pending charge. The residual of about USD0.33 appeared after the final Phase B read, with nothing running; it's most likely late billing settlement of Phase B or storage, not a new rental. The total stays far below the cap either way.

## 8. Publication

The code is at github.com/CortexLM/hypertrain, pushed by echobt after the user's go (push-main-r2.log, verify-20.json). GitHub Actions are disabled, by the user's choice, so CI does not run there.

## 9. Gated next steps and residual decisions

All need explicit spend approval.

1. GPU proxy ladder: 0.5B, then 1 to 3B, then at least 7B, with a TOST equivalence gate at each rung.
2. Determinism-only baseline for the overhead split (section 4).
3. Re-run the full e2e suite and the NO_REPLAY run on the current tree.
4. Re-run the CPU parity grid at full scale or an amended reduced scale (pre-registered).

Residual decisions left open: deferred FLTrust (root-dataset defence against poisoned deltas) and the checkpoint and weights license.

## 10. Preservation

`scripts/preservation_check.py --compare` against `preservation-before.json`: 1065 files compared, 0 changed (task-21-hypertrain-challenge.log).

## 11. Resume en francais

Le determinisme bit a bit est prouve sur 3 hotes (phase A) et sur 2 x 8 RTX 5090 (phase B, MoE 655M). Le surcout du mode deterministe (1,97 et 2,06) depasse le seuil de 1,6 : RE-PLAN. BBR non mesure. La grille de parite CPU est CENSUREE (0 sur 150) : parite a pleine echelle non demontree. Cout total 12,53 USD sur un plafond de 33,48 USD, 0 instance, 0 volume.
