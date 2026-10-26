# Mechanism specification

How Hypertrain makes cheating unprofitable and pays honest miners: audit sampling and its detection math, adaptive audit rates, the reward formula, escrow, vesting and clawback, how burned mass is accounted, why the auditors can be trusted, and what is still unproven. The wire formats are in [protocol.md](protocol.md).

Numbers carry a tag of the form `value{ev:path#key}`. `scripts/check_doc_numbers.py` reads the JSON at `path` and fails if the document disagrees. Closed-form tables (keys `q005`, `q010`, `q025` are q = 0.05, 0.10, 0.25) come from `scripts/gen_mechanism_tables.py` ([experiments/results/mechanism_tables.json](../experiments/results/mechanism_tables.json)); measured numbers come from the experiment results named in each tag.

## 1. Threat model in one paragraph

A miner is paid for training a delta. Cheating means submitting a delta that was not produced by the assigned data, the published start state and the pinned optimizer: fabricated weights, skipped steps, interpolation, a last-step-only fix, or a copied delta. The bits of an honest run are reproducible (section 8), so a verifier can recompute any leaf and compare. The mechanism only has to make the expected cost of being caught larger than the saving from cheating.

## 2. Audit sampling and detection

A round commits `n_leaves = H/J + 1` Merkle leaves. After the last commit is in, the drand signature of round `d_audit` is the seed:

```
selected  <=>  sha256("ht-audit" | run_id | w | drand_sig[d_audit] | hotkey) < q_i * 2^256
```

A committer cannot predict the seed. If it could, the adaptive cheater would pass every audit (probability of detection 0 in the simulation of the commit-then-sample scheme).

**Full replay** (state policy `reset` or `derived`). The auditor replays all `H` steps from the public round-start state and compares every leaf. If the miner is selected, detection is certain for any deviation, including a single step or only the last step. Detection per round equals `q_i`, so the chance a cheater survives `n` rounds is `(1-q)^n`:

| q | 1 round | 5 rounds | 10 rounds | 20 rounds |
|---|---|---|---|---|
| 0.05 | 5.00%{ev:experiments/results/mechanism_tables.json#cumulative_detection.q005.1*100} | 22.6%{ev:experiments/results/mechanism_tables.json#cumulative_detection.q005.5*100} | 40.1%{ev:experiments/results/mechanism_tables.json#cumulative_detection.q005.10*100} | 64.2%{ev:experiments/results/mechanism_tables.json#cumulative_detection.q005.20*100} |
| 0.10 | 10.0%{ev:experiments/results/mechanism_tables.json#cumulative_detection.q010.1*100} | 41.0%{ev:experiments/results/mechanism_tables.json#cumulative_detection.q010.5*100} | 65.1%{ev:experiments/results/mechanism_tables.json#cumulative_detection.q010.10*100} | 87.8%{ev:experiments/results/mechanism_tables.json#cumulative_detection.q010.20*100} |
| 0.25 | 25.0%{ev:experiments/results/mechanism_tables.json#cumulative_detection.q025.1*100} | 76.3%{ev:experiments/results/mechanism_tables.json#cumulative_detection.q025.5*100} | 94.4%{ev:experiments/results/mechanism_tables.json#cumulative_detection.q025.10*100} | 99.7%{ev:experiments/results/mechanism_tables.json#cumulative_detection.q025.20*100} |

**Segment audits** (state policy `carry`). Carrying AdamW state across rounds means a replay cannot start from the public state, so the miner serves its optimizer state at the start of a few leaf windows (`StateServe`, with a Merkle proof) and the auditor replays only those windows. The auditor checks `k_segments` random windows, the final window always, and the `Q_top` windows with the largest committed norm.

With `U = 30` windows and `k = 3` random draws and the final rule off, a cheat confined to `c` windows is drawn with probability `1 - C(U-c,k)/C(U,k)`:

| cheated windows c | probability drawn |
|---|---|
| 1 | 10.0%{ev:experiments/results/mechanism_tables.json#segments_U30_k3.no_final_involved.1*100} |
| 3 | 28.0%{ev:experiments/results/mechanism_tables.json#segments_U30_k3.no_final_involved.3*100} |
| 10 | 71.9%{ev:experiments/results/mechanism_tables.json#segments_U30_k3.no_final_involved.10*100} |
| 29 | 100%{ev:experiments/results/mechanism_tables.json#segments_U30_k3.no_final_involved.29*100} |

A miner that only fakes the last step escapes with 90% without the final rule (caught with probability 10.0%{ev:experiments/results/mechanism_tables.json#segments_U30_k3.last_step_only_without_final_rule*100}). With the final window always audited, it is caught with certainty (experiments/results/mechanism_tables.json: `last_step_only_with_final_rule` = 1{ev:experiments/results/mechanism_tables.json#segments_U30_k3.last_step_only_with_final_rule}). That is why the final window is never optional. The auditor tests include an ablation of the rule: 17 catches in 200 trials without it, inside the 99% binomial band for 3/30 (evidence: `.omo/evidence/hypertrain-challenge/task-10-hypertrain-challenge.json`).

Top-Q selection by committed norm is a heuristic: committed metrics can lie, and a lie is only caught if the window is actually drawn.

## 3. Adaptive audit rate q_i

`q_i` is written into the signed RoundOpen roster as an exact f32, and the selection comparison is integer-exact on that value.

| Condition | q_i |
|---|---|
| Default | `verify.q_base` (example manifest 0.1) |
| Probation: new hotkey, or not bonded within `probation_rounds` | 1.0 (verify before apply) |
| `outlier` or `copy` flag from the soft layer | 1.0 |
| Cluster member (shared coldkey, ASN, payout or timing) | drawn at `q_base` with influence capped as one identity; one fault in the cluster puts every member at 1.0 next round |

Copy suspicion is only a hint. Honest deltas overlap on top-k indices at 0.07 to 0.12, but a rescaled average of several deltas can stay below the 0.4 threshold and still score well on loss, so replay is the defence, not the detector.

## 4. Rewards

Everything is integer units. A training round has a budget `B_w = epochs_per_round * 10^6`; `full_share_mass` is `10^6` per chain epoch.

```
contrib_i = tokens_i * (800000 + clamp(soft_score_ppm_i, 0, 10^6) / 5)       # f_i in [0.8, 1.0], in ppm
e_i       = floor(B_w * contrib_i / sum_j contrib_j)    for MATCH or UNSAMPLED
e_i       = 0                                           for FAULT, NO_UPLOAD, or blacklisted
```

The denominator includes every committer with a verdict, so the slice of a faulty miner burns. It is never redistributed to the others. Redistribution would let an attacker profit from framing a competitor; burning removes that incentive. The soft score can move a payout by at most 20%, because loss-based ranking alone rewarded the copy attacker in simulation.

Example with `epochs_per_round = 1` and three equal honest miners: each entitlement is 333333{ev:experiments/results/mechanism_tables.json#reward_example.three_equal_miners_entitlement} units and 1 unit burns to rounding. If one of three is faulty, 333334{ev:experiments/results/mechanism_tables.json#reward_example.burn_with_one_faulty_of_three} units burn.

## 5. Escrow, vesting and clawback

Entitlements do not pay immediately. An entitlement of round `w` finalized in ledger round `r` is released at `r + E`. Until then it is escrow, and that escrow is the stake: Cortex has no slashing, so unvested reward is the only collateral.

**Sizing.** A cheater who saves a fraction `g <= 1` of one round's reward `R` gains `G = g*R`. If caught it loses the escrow `S = E*R` and the round reward, so deterrence needs `q*(S + R) >= G`, which gives `E >= g/q - 1`. The ledger uses `E = ceil(1/q)`, computed exactly on the f32 value of `q`, and the manifest validator rejects a smaller `E_vest_rounds`.

| q | E (rounds) |
|---|---|
| 0.05 | 20{ev:experiments/results/mechanism_tables.json#vesting_rounds_E.q005} |
| 0.10 | 10{ev:experiments/results/mechanism_tables.json#vesting_rounds_E.q010} |
| 0.25 | 4{ev:experiments/results/mechanism_tables.json#vesting_rounds_E.q025} |

**Release.** `get_weights` pays vested entitlements oldest first, capped at `full_share_mass` per epoch. The excess carries over, so a backlog cannot exceed the cap in one epoch. A hotkey with a journaled post-finality fault is frozen.

**Fault.** FAULT burns every outstanding entitlement of the hotkey plus the round reward and blacklists it. A TRANSIENT verdict (the miner's own rerun reproduces the auditor's bits, as can happen without ECC) burns only that round's reward, and is forgiven `forgive_per_epoch` times per chain epoch; more than that escalates to FAULT. NO_UPLOAD forfeits the round only.

**Clawback.** Only unvested escrow and future entitlements can be taken. A clawback burns outstanding escrow first. The remainder becomes `debt_units`, repaid from future entitlements (repaid units burn). Emission already paid out on chain cannot be recalled.

**Vindication and late fault.** If a dispute is later decided for the miner after a NO_UPLOAD finalize, the forgone slice is credited once and vests from the credit round (units move from burned to pending). If the miner loses, the forgone slice stays burned, outstanding escrow burns, and the hotkey is blacklisted.

## 6. Burn accounting via full_share_mass

Cortex computes a leaf weight as `floor(1e12 * w / max(W, full_share_mass))`, where `W` is the sum of listed weights. The ledger always reports `full_share_mass = 10^6`, so any epoch in which `W < 10^6` leaves mass unclaimed, and unclaimed mass burns. That covers three cases without extra code: the start of a run (nothing has vested yet), a round whose miners were faulty, and capped excess. The answer metadata exposes `units_paid`, `units_burned_this_epoch` and the running `ledger_minted`, `ledger_burned`, `ledger_paid`, `ledger_pending`, with `minted = burned + paid + pending` at all times.

The first answer per epoch is persisted and returned byte for byte afterwards. An answer is a pure function of journal events at or before `epoch_at`, so anyone can recompute it. While a round sits in AUDIT the endpoint answers 200 with whatever has vested, never 503, because Cortex burns the whole share on an error.

## 7. Auditor honesty

The operator runs the auditors, so the mechanism must stay honest when the auditor is the weak point.

1. Every verdict carries `recomputed_leaves_root`, and every replay input is public. Anyone with the pinned image and a matching GPU can redo it.
2. A miner can contest. A referee on a different host and driver then runs N-ary bisection (step, layer, op) to a single operator that either side can recompute, and a Resolution punishes the loser. A lying auditor is named by the bisection.
3. Honeypot miners run at a secret rate. The list is committed (`sha256(list | salt)`) in RoundOpen and revealed per epoch. Planted bad runs measure the catch rate, planted honest runs measure the false positive rate, and both are published per epoch (`auditor-stats`).
4. Fraud does not pay the operator: burned mass returns to nobody. The remaining risk is favoritism toward operator-owned miners, which items 1 to 3 expose rather than prevent.

## 8. Evidence from experiments

### State policy: carry (CPU proxy)

A 4.3M-parameter CPU proxy ({ev:experiments/results/decision.json#model_params} parameters, 600 steps, 3 seeds, M and H grid) compared four ways of handling the inner optimizer state between rounds against carrying it: A1 reset, A2 re-warmup, A3 derived. A policy passes when its relative loss gap to carry is inside the pre-registered margin in every cell.

| Arm | M=2, H=30 gap | M=4, H=100 gap | Verdict |
|---|---|---|---|
| A1 reset | 2.23%{ev:experiments/results/decision.json#cells.M2_H30.arms.A1.mean_gap*100} | 0.83%{ev:experiments/results/decision.json#cells.M4_H100.arms.A1.mean_gap*100} | FAIL |
| A2 re-warmup | 0.74%{ev:experiments/results/decision.json#cells.M2_H30.arms.A2.mean_gap*100} | 0.47%{ev:experiments/results/decision.json#cells.M4_H100.arms.A2.mean_gap*100} | FAIL |
| A3 derived | 10.5%{ev:experiments/results/decision.json#cells.M2_H30.arms.A3.mean_gap*100} | 7.31%{ev:experiments/results/decision.json#cells.M4_H100.arms.A3.mean_gap*100} | FAIL |
| NEG (sign-flipped momentum) | 5.22%{ev:experiments/results/decision.json#cells.M2_H30.arms.NEG.mean_gap*100} | 11.5%{ev:experiments/results/decision.json#cells.M4_H100.arms.NEG.mean_gap*100} | worse, as it must be |

No arm other than the control passed, so the decision is `state_policy = carry` with segment audits (`experiments/results/decision.json`, key `decision.state_policy`). The negative control gate held in every cell, which shows the harness can see a degradation. Caveat from the same file: this is a tiny CPU proxy and says nothing about GPU, BF16 numerics or quality at scale.

### Cross-host bitwise result

Phase A rented three RTX 5090 hosts with distinct machine ids ({ev:experiments/results/phase_a_summary.json#distinct_machine_ids}) and drivers 580.105.08, 580.178.04 and 595.84. A 59,010,560-parameter model ({ev:experiments/results/phase_a_summary.json#runs.0.result.param_count}) trained one round on each host, twice, in the pinned image. Every honest run produced the same leaves root, delta hash and final theta hash. A negative control with one injected perturbation diverged at leaf {ev:experiments/results/phase_a_summary.json#negative_control.observed}, the leaf the injection predicts, so the comparison can fail. The whole phase cost USD 0.54{ev:experiments/results/phase_a_summary.json#cost.total_usd} against a cap of USD 10{ev:experiments/results/phase_a_summary.json#cost.cap_usd}, and no instance was left running ({ev:experiments/results/phase_a_summary.json#inventory.owned_instances} owned at the end). Source: `experiments/results/phase_a_summary.json` (copy of `.omo/evidence/hypertrain-challenge/task-12-hypertrain-challenge.json`).

Limits: one model size, one round, 21 leaves, a single image. This is evidence that bitwise equality across hosts is achievable with the pinned stack, not a proof for every kernel or every future driver. Phase B, which covers multi-GPU islands, is in the next subsection.

### Cross-host bitwise result on 8-GPU islands (Phase B)

Two rented hosts with eight RTX 5090 each (Quebec, driver 580.95.05, and Taiwan, driver 580.159.03, machine ids distinct: 2{ev:experiments/results/phase_b_summary.json#distinct_machine_ids} hosts) ran one round of a 654965760{ev:experiments/results/phase_b_summary.json#runs.0.result.param_count}-parameter MoE with H=30, J=5, expert-parallel over 8 ranks and ZeRO-1. Both hosts produced the same leaves root, delta hash, final theta hash and top-k index and value hashes, and all eight ranks on each host agreed. Outcome: PASS. Source: `experiments/results/phase_b_summary.json` (trimmed copy of `.omo/evidence/hypertrain-challenge/task-16-hypertrain-challenge.json`).

- **Memory.** Peak allocation was 23.2{ev:experiments/results/phase_b_summary.json#b2.per_host.h0.probe_peak_alloc_gib} GiB, 78%{ev:experiments/results/phase_b_summary.json#b2.per_host.h0.probe_peak_mem_frac*100} of a 32 GB card, below the 85% limit.
- **Determinism overhead is above plan.** Throughput of the baseline arm over the deterministic arm was 1.97{ev:experiments/results/phase_b_summary.json#overhead.per_host_ratio.h0} on the Quebec host and 2.06{ev:experiments/results/phase_b_summary.json#overhead.per_host_ratio.h1} on the Taiwan host. The plan flags a re-plan above 1.6; the verdict is REPLAN. The baseline also enables TF32, bf16 reduced-precision reductions, cuDNN autotune and NCCL all-reduce, so the ratio mixes the cost of determinism with precision and kernel differences. The split is not measured.
- **Network.** Inter-host TCP with cubic congestion control reached 0.03{ev:experiments/results/phase_b_summary.json#network.iperf.iperf-h0-cubic-P1.received_Gbps}, 0.15{ev:experiments/results/phase_b_summary.json#network.iperf.iperf-h0-cubic-P8.received_Gbps} and 0.47{ev:experiments/results/phase_b_summary.json#network.iperf.iperf-h0-cubic-P32.received_Gbps} Gbit/s with 1, 8 and 32 streams. The BBR runs are censored: the container kernel lacks `tcp_bbr`.
- **Cost.** USD 11.57{ev:experiments/results/phase_b_summary.json#cost.total_usd} for the whole phase, including three failed attempts before the run (a provider fault and two host faults), one code fault (an out-of-memory in the ZeRO-1 gather) and the passing run.

Limits: two hosts, one round, one model, one image; no instance was left running (0{ev:experiments/results/phase_b_summary.json#inventory.owned_instances} owned at the end).

## 9. Residual risks

- **Cross-host bitwise result.** Section 8 covers three hosts and one configuration. A driver or kernel outside the allow-list can make an honest miner mismatch. The TRANSIENT path (rerun) and the driver allow-list limit the damage, and any new driver needs a new cross-host check.
- **Stealthy low-rate poisoning.** A small, consistent bias in a delta that is computed honestly from honest data is not a replay failure. The influence cap, CClip and the soft layer bound its effect but do not remove it. A backdoor found after finality is handled only with money.
- **Validator concentration.** One operator runs the auditors. Public replay inputs, bisection to a recomputable operator and published honeypot rates make misbehavior visible, but they do not make it impossible.
- **Deregister and leave.** A miner can abandon its hotkey. The loss is limited to escrow that has not vested, and fraud proven after the escrow is paid is not recoverable.
- **Clusters.** Identity is inferred from signals (coldkey, network, timing). A well-hidden Sybil spreads across the sampling and is only bounded by the influence cap.
- **100B quality.** All mechanism evidence is at toy or small scale; see [parity.md](parity.md) for what that does and does not say about quality.
