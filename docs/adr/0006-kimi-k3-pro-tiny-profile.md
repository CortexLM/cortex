# ADR 0006: `kimi-k3-pro-tiny` test model profile

- Status: draft for branch `v3-kimi-tiny`, 2026-10-26 (mining sprint S2). Forge decides
  promotion; nothing here changes a production run, profile or emission.
- Labels: **[V]** verified in code or by a run on the date above, **[A]** reported by a cited
  public document, **[H]** hypothesis, **[?]** unknown.

## Context

The founder wants mining on Subnet 100 tested with the Kimi K2/K3 architecture family
(DeepSeek-V3-like: Multi-head Latent Attention, fine-grained MoE with shared experts, top-k
routing, auxiliary-loss-free balancing), scaled to tens of millions of parameters. This means
architecture only: random init, no weights, no tokenizer download.

What hypertrain supports today [V]:

- `decoder` (`hypertrain/src/hypertrain/trainer/model.py`): GQA attention with RoPE, SwiGLU,
  dense or MoE. The MoE router uses a softmax over the selected logits, a stable-sort top-k
  (ties go to the lowest expert index) and capacity-factor drops. Dispatch is a pure
  permutation (`permute_rows`), with a fixed-order k-sum and a Switch-style aux loss.
  `HT_MOE_GROUPED=1` selects a bmm path that is not the replay reference.
- `od-encoder` (`hypertrain/src/hypertrain/models/opendecision/`), loaded through
  `ARCH_REGISTRY` in `trainer/model.py`, with `param_shapes / stage_of / init_params /
  forward / traced_forward`.
- Run manifest: protocol `RunManifest.model` (`protocol/messages.py:85`) has
  `arch: Literal["decoder", "od-encoder"]`. `ReferenceSpec.profile` exists only for OD.
  `TrainConfig._convert` maps the manifest to `ModelConfig`. MFU profiles
  (`scripts/mfu_profiles.json`) are all dense (`n_experts = 1`).
- Tests: the dense/moe x fp32/bf16 x codec matrix (`tests/trainer/test_trainer_core.py:55`)
  runs two child processes and compares `leaves_root`/`delta_hash`. The grouped-MoE tolerance
  test is `tests/trainer/test_moe_grouped.py`.
- `implementation_hash` (`challenge/store.py:4248-4320`) covers nine files under
  `src/hypertrain`: `aggregator/{core,tape_v2,rollback_v2,capacity_worker}.py`,
  `trainer/{compress,config,model}.py`, `miner/island_launch.py` and `auditor/replay.py`. It
  also covers the AST of the `owned` store methods. ADR 0001 also freezes
  `src/hypertrain/protocol/`.

## Gap table

Public sources were fetched on 2026-10-26: Kimi K2 report arXiv 2507.20534, `moonshotai/Kimi-K2-Instruct`
`config.json` and `modeling_deepseek.py`, Kimi K3 report arXiv 2607.24653 and
`moonshotai/Kimi-K3` `config.json`, DeepSeek-V3 report arXiv 2412.19437v2 and the Kimi API
model list. Dates are those of the sources; where the page shows none, the source is "unknown".

**Kimi K3 Pro: [?].** No first-party source uses that name. `huggingface.co/moonshotai/Kimi-K3-Pro`
returned HTTP 401. The first-party model list (platform.kimi.ai/docs/models) has `kimi-k3`,
`kimi-k3.1-pro` and `kimi-k3.1-pro-highspeed` [A]. Moonshot describes K3.1 Pro only as "a
post-trained version of Kimi K3" [A]. This profile therefore follows the **K2 family**
(DeepSeek-V3-like), as the brief requires when K3 Pro is unknown.

| Component | Kimi K2 [A] | Kimi K3 [A] | hypertrain before | `kimi-k3-pro-tiny` |
| --- | --- | --- | --- | --- |
| Attention | MLA, q_lora 1536, kv_lora 512, nope 128, rope 64, v 128, 64 heads | hybrid 3:1 KDA linear + Gated MLA, NoPE, 96 heads | GQA + RoPE only | MLA with K2 ratios per head dim (q_lora 12hd, kv_lora 4hd, rope hd/2) |
| Position | RoPE theta 50000 + YaRN x32 | NoPE in MLA | RoPE | RoPE on the decoupled rope part, no YaRN |
| MoE experts | 384 routed, top-8, 1 shared, ff 2048 | 896 routed, top-16, 2 shared, latent 3584 (LatentMoE) | routed only, no shared | 32 routed, top-4, 1 shared |
| Dense layers | first 1 | first 1 | all or none | first 1, ff = (top_k + shared) x expert ff (K2 ratio 18432 = 9 x 2048) |
| Router | sigmoid, noaux_tc, 1 group, normalised top-k, scale 2.827 | sigmoid, noaux_tc, renormalise, scale 1.0 | softmax over selected, capacity drops | sigmoid, bias for selection only, normalised, scale 2.827, no drops |
| Balancing | aux-free bias (`noaux_tc`); the K2 update rule and gamma are **unknown** | Quantile Balancing bias | Switch aux loss | DeepSeek-V3 sign rule, gamma 0.001 [A DeepSeek-V3] |
| Seq aux loss | config `aux_loss_alpha` 0.001, `seq_aux` true; training value **unknown** | **unknown** | Switch aux | DeepSeek-V3 seq-wise loss, coefficient 1e-4 [A DeepSeek-V3] |
| Activation | SwiGLU | SiTU-GLU | SwiGLU | SwiGLU |
| Depth mixing | residual | Block Attention Residuals | residual | residual |
| MTP | 1 layer in K3 report, 0 in released config (contradiction) | same contradiction | none | none |
| Optimizer | MuonClip, QK-Clip tau 100 | Per-Head Muon + QK-Clip, tau **unknown** | AdamW, Muon (no QK-Clip) | existing AdamW/Muon, no QK-Clip |
| Vocab / tokenizer | 163840 | 163840 | manifest | 32768 ids, synthetic tokens, no tokenizer |

Not implemented: KDA, Gated MLA output gate, NoPE layout, LatentMoE, Quantile Balancing,
SiTU-GLU, Attention Residuals, YaRN, MTP and QK-Clip. Their numerics are known only from the
sources above.

## Decision

1. New module `hypertrain/src/hypertrain/models/kimi_tiny.py` with arch name
   `kimi-k3-pro-tiny`. It implements the same interface as `od-encoder`. MLA, the shared
   expert, the sigmoid router with selection-only bias, the sign-rule bias update and the
   sequence-wise aux loss are plain PyTorch.
2. Determinism: top-k is a stable descending sort (ties go to the lowest index) through
   `trainer.model.route`, with capacity equal to the token count, so no drops. Dispatch and
   combine reuse `permute_rows`. Attention is the eager math path. No `index_add`,
   `scatter_add` or `topk`.
3. The balancing bias is a 1-D tensor `layers.NNN.router_bias` in the parameter dict. Leaves,
   deltas and replay therefore bind it. The forward reads it detached, so its gradient is
   `None` and AdamW leaves it unchanged (1-D, no decay, zero moments). A custom autograd node
   on the router logits applies `b += gamma * sign(mean_load - load)` during backward.
   The current batch is never routed with a bias derived from itself. `loop._train_step`,
   `loop.replay` and `auditor/bisect.traced_step` all run the same update without a code
   change. The update runs once per backward pass, that is per micro-batch; with
   `grad_accum = 1` this equals the DeepSeek-V3 per-step rule.
4. Registration is an explicit opt-in: `kimi_tiny.register()` adds the arch to
   `ARCH_REGISTRY` at runtime. **No `implementation_hash` file changes.** Permanent
   registration needs one line in `trainer/model.py` (hash-covered) and is left to Forge.
5. The TEST run manifest is an unsigned module-level document (`test_run_manifest()`), not a
   protocol `RunManifest`, because `ModelSpec.arch` is a frozen protocol Literal (ADR 0001).
   A network run with this profile needs a protocol change that Forge must decide.

## Limits

- Single device only. The island trainer passes its expert-parallel MoE to every MoE arch
  (`trainer/island.py:600`), and this arch's `forward` rejects it with `ValueError`. Islands
  therefore fail closed instead of running a different MoE [V]. Island DP would also need an
  all-reduce of the expert loads, which is not implemented.
- `trainer/unit_order.train_unit_order` calls `forward` without `moe`, so it is **not**
  refused. With world > 1, each rank updates its own bias, and the leaves then depend on the
  world size (reproduced by review, 2026-10-26 [V]). No production caller exists today (only
  `tests/layout`). A guard there is outside this change's scope and is left to Forge.
- `grad_accum > 1` would apply the rule once per micro-batch. TEST run documents therefore
  require `grad_accum == 1` (`train_config`).
- `HT_COMPILE`, `HT_ACT_CKPT`, `HT_MOE_GROUPED` and FP8 apply to `decoder` only; this arch
  ignores them.
- GPU determinism on B300 is [?] until the qualification below runs.

## Tests

`hypertrain/tests/models/test_kimi_tiny.py` has one named test per behaviour. Each test
fails under the mutation listed below (CPU run, 2026-10-26 [V]):

| Behaviour | Test | Mutation that it catches |
| --- | --- | --- |
| routing | `test_topk_selects_on_biased_scores_and_gates_unbiased` | bias ignored in top-k selection |
| shared expert | `test_shared_expert_is_added_to_every_token` | shared expert dropped from the output |
| bias update | `test_bias_update_sign_rule_applies_after_backward_only` | sign of the rule flipped |
| determinism | `test_two_passes_same_seed_are_bitwise_identical` | unseeded noise on the router logits |
| selection-only bias | `test_uniform_bias_shift_keeps_selection_and_mixture_weights` | gate computed from biased scores |
| seq-wise balance loss | `test_sequence_balance_loss_matches_deepseek_v3_formula` | wrong sequence axis; no score normalisation; batch-wise loss |
| MLA RoPE | `test_mla_matches_independent_reference` | no RoPE on the key; no RoPE on the query; theta doubled |

The last three rows were added after an independent review found that those mutations passed the
earlier suite. The file also checks the profile size and K2 layout, forward/backward shapes on
the full profile, and the unsigned TEST run document (strict keys, `grad_accum == 1`, one round
trains the bias). The dense/moe matrix in
`tests/trainer/test_trainer_core.py` (two processes, identical roots; replay of every leaf)
now includes `kimi` (fp32/sparseloco, bf16/dense-int8). `test_profile_two_rounds_bitwise_on_cuda`
is marked `gpu` and is skipped without CUDA.

## B300 qualification

Run on each B300 host from `hypertrain/`, inside the GPU image or an env synced with
`uv sync --frozen --extra trainer` and a CUDA torch. No key, no network and no dataset are
needed: tokens are Philox ids. Keep outputs in `/tmp/<agent>-<run>/`.

```bash
OUT=/tmp/<agent>-<run>; mkdir -p "$OUT"; HOST=$(hostname -s)
# 1. CUDA test: two rounds in one process must be bitwise equal.
uv run --no-sync pytest -m gpu -p no:cacheprovider --basetemp="$OUT/pt" -s \
  tests/models/test_kimi_tiny.py
# 2. Two qualification rounds per process, two processes per host.
for i in 1 2; do
  uv run --no-sync python -m hypertrain.models.kimi_tiny --device cuda --rounds 2 \
    > "$OUT/kimi-$HOST-$i.json" || echo "round mismatch in process $i"
done
# 3. Same host: both processes must print the same run_id, leaves_root, delta_hash, state_root.
diff <(jq -S .rounds "$OUT/kimi-$HOST-1.json") <(jq -S .rounds "$OUT/kimi-$HOST-2.json")
# 4. Across hosts: compare .rounds[0] of each host's file (same GPU model and torch build).
jq -c '.rounds[0] | {leaves_root, delta_hash, state_root}' "$OUT"/kimi-*-1.json
```

`state_root` here is `final_theta_hash`, the state hash of the final parameters. The network
`state_root` of the v2 capacity genesis cannot be computed for this arch, because no protocol
manifest can carry it [V]. Agreement across hosts on the same GPU model is [H] until the run
above; agreement across GPU models or torch builds is not expected.

## CPU evidence (2026-10-26, torch 2.14.0+cpu)

- The profile has 77,001,376 parameters. Init takes 1.3 s and one fp32 forward/backward over
  1 x 32 tokens takes 0.3 s on the CI host [V].
- `python -m hypertrain.models.kimi_tiny --device cpu --dtype fp32 --rounds 2` was run in two
  processes. All four rounds printed `run_id ca9598d7…`, `leaves_root 0e7f7ad8…41b5`,
  `delta_hash cd441fff…77ab` and `state_root 9e94b577…55d8` [V]. These are CPU fp32 values
  only. The B300 run uses bf16 and will print different hashes.
- `implementation_hash` of the v2 capacity profile is `5a0b2374…0190a578a` on both base
  `4e0a6457` and this branch [V].
