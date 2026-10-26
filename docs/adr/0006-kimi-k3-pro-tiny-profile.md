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
   `auditor/bisect.traced_step` and the island mirror all run the same update without a code
   change. The update runs once per backward pass, that is per micro-batch; with
   `grad_accum = 1` this equals the DeepSeek-V3 per-step rule.
4. Registration is an explicit opt-in: `kimi_tiny.register()` adds the arch to
   `ARCH_REGISTRY` at runtime. **No `implementation_hash` file changes.** Permanent
   registration needs one line in `trainer/model.py` (hash-covered) and is left to Forge.
5. The TEST run manifest is an unsigned module-level document (`test_run_manifest()`), not a
   protocol `RunManifest`, because `ModelSpec.arch` is a frozen protocol Literal (ADR 0001).
   A network run with this profile needs a protocol change that Forge must decide.

## Limits

- Single device per island only. With island DP > 1, each rank would update the bias from its
  own loads and the replicas would diverge. A load all-reduce is not implemented [V by design].
- `HT_COMPILE`, `HT_ACT_CKPT`, `HT_MOE_GROUPED` and FP8 apply to `decoder` only; this arch
  ignores them.
- GPU determinism on B300 is [?] until the qualification below runs.

## B300 qualification

Commands, the test count and the measured CPU evidence are added with the implementation
commit.
