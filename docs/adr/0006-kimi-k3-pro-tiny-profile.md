# ADR 0006: `kimi-k3-pro-tiny` test model profile

- Status: draft for branch `v3-kimi-tiny`, 2026-10-26 (mining sprint S2). Forge decides
  promotion; nothing here changes a production run, profile or emission.
- Amended 2026-10-26 by batch `v3-kimi-registry` (Principal SN100 decision D18): permanent
  registry entry, `ModelSpec.arch` taken from the registry, one-rank island and per-arch
  reduction order. See [D18 amendment](#d18-amendment-v3-kimi-registry-2026-10-26).
- Labels: **[V]** verified in code or by a run on the date above, **[A]** reported by a cited
  public document, **[H]** hypothesis, **[?]** unknown.

## Context

The founder wants mining on Subnet 100 tested with the Kimi K2/K3 architecture family
(DeepSeek-V3-like: Multi-head Latent Attention, fine-grained MoE with shared experts, top-k
routing, auxiliary-loss-free balancing), scaled to tens of millions of parameters. This means
architecture only: random init, no weights, no tokenizer download.

What hypertrain supported at `4e0a6457` [V] (D18 changed the registry, `ModelSpec.arch` and
`implementation_hash`; see the D18 amendment):

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
4. Registration (D18): the arch has a permanent entry in the read-only `ARCH_REGISTRY` of
   `hypertrain/src/hypertrain/models/registry.py`, which `implementation_hash` covers. The S2
   opt-in `kimi_tiny.register()` is removed: a runtime entry never reached a torchrun worker.
5. The TEST run document (`run_manifest()`) stays unsigned and serves the B300 qualification
   CLI only. A network run uses a signed `RunManifestV2` with `model.arch =
   "kimi-k3-pro-tiny"` (D18).

## Limits

- One rank only (registry `max_ranks=1`, D18). `TrainingManifestV2` rejects `n_gpus > 1` and
  `train_island` refuses a second rank before training. On one rank the island runs this
  arch's own MoE unsharded (expert parallelism is decoder-only) and equals `loop.train_round`
  bit for bit for every reduction mode [V]. Island DP would need an all-reduce of the expert
  loads, which is not implemented: with dp 2, rank 1's bias was never committed and a window
  re-run diverged (reader probe, 2026-10-26 [V]).
- Reduction order "rank" only (registry `reduction_orders`, D18). `check_reduction_order` is
  the first statement of `train_unit_order` and is applied by `hardware_class.admit`, so
  unit-order training and a mixed-class island (which needs "unit") are refused. Before D18,
  unit order with world 2 gave two ranks with different roots and nothing raised [V].
- `grad_accum > 1` would apply the rule once per micro-batch. TEST run documents and v2
  manifests therefore require `grad_accum == 1`.
- `inner.opt == "adamw"` only (S10 K4): `optim.MUON_SUFFIXES` names the decoder tensors
  (`.wq .wk .wv .wo .w1 .w2 .w3`), so Muon on this arch would orthogonalise only `wo`, `w1`, `w2`
  and `w3` (34,013,184 of 77,001,376 parameters on PROFILE) and leave `wq_a`, `wq_b`, `wkv_a`,
  `wkv_b` and the shared expert on AdamW at the Muon learning rate. Both parsers refuse it. Mapping
  the names was rejected: which tensors K2's MuonClip orthogonalises, and QK-Clip, are not
  implemented here (see the gap table).
- `top_k_experts >= 2` (S10 M-K4): with one routed expert the normalised gate is the constant
  `ROUTED_SCALE`, so the router gets no gradient from the LM loss (6.8e-11 against 3.1e-4 at
  k=2 [V]). K2 normalises only when k > 1 [A]. `dims()` and the v2 rule refuse k=1.
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

Target (Forge, 2026-10-26): 2 hosts, each with 8 x B300, compute capability 10.3 (sm_103).
The profile is single-device: one process uses one GPU (77M parameters, under 2 GB of fp32
weights, grads and AdamW state [H estimate]), so "1-2 GPUs" means one or two independent
processes on GPU 0 and GPU 1. Run from `hypertrain/` on each host, inside the GPU image
(`torch==2.14.0+cu130`; whether this build ships sm_103 kernels is [?], so check `arch_list`
in the output) or in an env synced with `uv sync --frozen --extra trainer` and a CUDA torch.
No key, no network and no dataset are needed: tokens are Philox ids. Keep outputs in
`/tmp/<agent>-<run>/`.

```bash
OUT=/tmp/<agent>-<run>; mkdir -p "$OUT"; HOST=$(hostname -s)
# 1. CUDA test on GPU 0: two rounds in one process must be bitwise equal.
CUDA_VISIBLE_DEVICES=0 uv run --no-sync pytest -m gpu -p no:cacheprovider \
  --basetemp="$OUT/pt" -s tests/models/test_kimi_tiny.py
# 2. Two processes per host (GPU 0 and GPU 1, in parallel), two rounds each.
for g in 0 1; do
  CUDA_VISIBLE_DEVICES=$g uv run --no-sync python -m hypertrain.models.kimi_tiny \
    --device cuda --rounds 2 > "$OUT/kimi-$HOST-gpu$g.json" &
done; wait   # a process exits non-zero ("MISMATCH") if its two rounds differ
# 3. Same host: capability must be [10, 3], and both processes must print identical rounds.
jq -c '{gpu, capability, torch}' "$OUT"/kimi-$HOST-gpu*.json
diff <(jq -S .rounds "$OUT/kimi-$HOST-gpu0.json") <(jq -S .rounds "$OUT/kimi-$HOST-gpu1.json")
# 4. Across the 2 hosts: copy one file per host and compare the three roots.
jq -c '.rounds[0] | {leaves_root, delta_hash, state_root}' "$OUT"/kimi-*-gpu0.json
```

`state_root` here is `final_theta_hash`, the state hash of the final parameters. The network
`state_root` of the v2 capacity genesis cannot be computed for this arch: the capacity profile
`tiny-cpu-service-32-v1` admits only its 728-parameter decoder geometry [V]. Agreement across hosts on the same GPU model is [H] until the run
above; agreement across GPU models or torch builds is not expected.

## CPU evidence (2026-10-26, torch 2.14.0+cpu)

- The profile has 77,001,376 parameters. Init takes 1.3 s and one fp32 forward/backward over
  1 x 32 tokens takes 0.3 s on the CI host [V].
- `python -m hypertrain.models.kimi_tiny --device cpu --dtype fp32 --rounds 2` was run in two
  processes. All four rounds printed `run_id ca9598d7…`, `leaves_root 0e7f7ad8…41b5`,
  `delta_hash cd441fff…77ab` and `state_root 9e94b577…55d8` [V]. These are CPU fp32 values
  only. The B300 run uses bf16 and will print different hashes.
- `implementation_hash` of the v2 capacity profile was `5a0b2374…0190a578a` on base
  `4e0a6457` and on `v3-kimi-tiny` `f45064de` [V]. D18 moves it (see the amendment).

## D18 amendment (v3-kimi-registry, 2026-10-26)

Principal SN100 decision D18 settles the three items S2 left to Forge. All of it lands on the
monorepo `v3` line only. Production does not move: it changes only with a future pin switch
under the D2/D14 guards (provenance proof, canary, next seal accepted).

### Decision

1. **Permanent registry entry.** `hypertrain/src/hypertrain/models/registry.py` is a
   stdlib-only, read-only `ARCH_REGISTRY` (`MappingProxyType` of frozen `Arch` records) with
   `decoder`, `kimi-k3-pro-tiny` and `od-encoder`. `trainer/model.py` re-exports it and
   dispatches on `ARCH_REGISTRY[arch].module`. It cannot live in `trainer/model.py`: importing
   `hypertrain.trainer` loads torch, and protocol validation must stay torch-free
   (`tests/ops/test_manifest_v2.py` blocks torch imports). The file joins the
   `implementation_hash` paths.
2. **`ModelSpec.arch` from the registry.** It is a `StrictStr` that must name a registry
   entry; an unknown name is rejected. An omitted arch means `decoder` and is always dumped,
   so the bytes, `run_id` and training hash of every existing manifest are unchanged. Signed
   intake still needs the canonical body, which carries `arch`. v1 is frozen (ADR 0001):
   `V1_ARCHS = ("decoder", "od-encoder")` and v1 refuses a later arch ("needs
   manifest_version 2"); `docs/schemas/RunManifest.json` keeps its v1 bytes. The v2 rule for
   `kimi-k3-pro-tiny` (`protocol/messages_v2.py`) requires: no reference profile,
   `grad_accum == 1`, `n_kv_heads == n_heads`, head dim divisible by 4, `n_experts > 1`,
   `n_layers > 1`, capacity 1, default precision, `inner.opt == "adamw"`, `top_k_experts >= 2`
   (both added by S10, see the [S10 amendment](#s10-amendment-review-fixes-2026-10-26)), and
   `param_count` equal to the shape-only count of `kimi_tiny.param_shapes`.
3. **One rank, rank order.** Each `Arch` carries `max_ranks` (kimi: 1) and `reduction_orders`
   (kimi and `od-encoder`: `("rank",)`; `decoder`: `("rank", "unit")`). `max_ranks` is checked
   by `TrainingManifestV2` and by `train_island` before training. `check_reduction_order` is
   the one reduction-order guard: the first statement of `train_unit_order`, and applied by
   `hardware_class.admit` to the run's arch, which is a required argument. "unit" is opt-in
   with evidence: only the decoder has a world-invariance test.
   `od-encoder` fails closed until one exists [?].

The v2 island runs kimi's own MoE unsharded: expert parallelism (`_moe_ep`,
`Geometry.is_expert`) applies to the decoder MoE only, and the overlap reductions treat the
router bias, which has no gradient, like an unused parameter. kimi's `forward` still refuses an
EP override.

### `implementation_hash` (v2 capacity profile)

| | value |
| --- | --- |
| before, `f45064de` (also `4e0a6457`) | `5a0b23740734c596914ed305fbcc4a4c0865292d7edb01599a9aaf70190a578a` |
| after, this batch | `02e208a35cba5703fef6c2adc59df133fa80c561b04bab5cd04b11eabc0189cc` |

Changed covered inputs [V, `git diff --stat f45064de..HEAD`]:

- `models/registry.py`: new, the tenth covered path;
- `trainer/model.py`: imports the registry, `_arch_impl` reads `.module`;
- `challenge/store.py`: the `paths` tuple inside the owned `_service_profile_v2`, whose AST is
  hashed;
- `aggregator/capacity_worker.py`: its copy of the path tuple.

`tests/challenge/protected_opening_fixture_static.py` lists the same modules; it is a fixture,
not a hash input. The other covered files are unchanged: `aggregator/{core,tape_v2,rollback_v2}.py`,
`trainer/{compress,config}.py`, `miner/island_launch.py` and `auditor/replay.py`.

No test or CI job pins `implementation_hash`: every consumer recomputes it from the source at
run time (`store.py`, `capacity_worker.py:82`). There is therefore no golden root to regenerate
in CI. An external pin or a live `service-admission` record that still holds `5a0b2374…`
refuses the new code, which is the intended binding. These values are pinned and do not move:

- the published v1 baseline `run_id` `ad8beb30…`;
- the v1 OD body `0a32bc44…`;
- the v2 wrapper `run_id` `d0725eb4…` and training hash `d7a864ce…`
  (`tests/protocol/test_model_arch.py`, captured at `f45064de`);
- `docs/schemas/*.json`, byte-identical;
- the golden leaf roots of `tests/trainer/test_golden_replay.py`.

### Tests and mutants (D18)

Each mutant was applied to the source, its named tests were run, and the file was restored
(`git diff` clean afterwards). **29/29 were killed** on CPU on 2026-10-26 [V]. Paths are under
`hypertrain/tests/`.

| Behaviour | Test | Mutants killed |
| --- | --- | --- |
| permanent entry | `models/test_model_registry.py::test_kimi_registered_without_runtime_registration` | kimi line removed |
| read-only registry | `models/test_model_registry.py::test_registry_cannot_be_changed_at_runtime` | plain dict; mutable `Arch` |
| hash binds the registry | `challenge/test_service_capacity_v2.py::test_capacity_profile_binds_the_arch_registry`; `challenge/test_capacity_outer_v2.py` | path dropped from `store.py`; from `capacity_worker.py` |
| arch is a registry name | `protocol/test_model_arch.py::test_unknown_arch_is_rejected_by_v1_and_v2`, `::test_arch_must_be_a_json_string` | name unchecked; lax `str` |
| same hashes, default `decoder` | `protocol/test_model_arch.py::test_existing_manifests_hash_as_before_and_omitted_arch_is_the_decoder` | default left out of the dump; no default |
| frozen v1 | `protocol/test_model_arch.py::test_v1_manifest_refuses_an_arch_registered_after_v1`; `protocol/test_messages.py::test_schema_files_match_and_regenerate_byte_identically` | v1 accepts kimi; v1 schema without the v1 contract |
| v2 kimi rule | `protocol/test_model_arch.py::test_kimi_v2_param_count_rule_matches_the_implementation` (micro, profile, two-layer), `::test_kimi_v2_rule_rejects` (11 cases) | `max_ranks` ignored; kimi `max_ranks` unset; grad_accum, capacity, precision, profile and geometry checks removed; shared expert left out of the count |
| unit-order guard | `layout/test_unit_order_canonical.py::test_kimi_unit_order_is_refused_before_training` (world 1, 2), `::test_unit_order_root_is_world_invariant` | guard removed; kimi allows "unit"; decoder loses "unit" |
| admission guard | `miner/test_hardware_class.py::test_unit_order_refused_for_a_rank_only_arch`, `::test_admission_requires_the_run_arch` | check removed; od-encoder allows "unit"; arch defaults to `decoder` |
| one-rank island | `models/test_kimi_tiny.py::test_one_rank_island_equals_the_reference_loop` (7 reductions), `::test_island_refuses_a_second_rank_before_training` | EP `moe` for every MoE arch; non-decoder MoE sharded; `max_ranks` ignored; overlap hook asserts a gradient |
| join-v2 -> run-v2 | `miner/test_client_network_v2.py::test_kimi_manifest_joins_and_runs_v2_through_the_cli` | EP `moe` for every MoE arch |

The join-v2 -> run-v2 test runs both commands through `cli.main` against a mock master that
serves an owner-signed v2 kimi manifest. The torchrun worker is a fresh interpreter. Its
`leaves_root`, `final_theta_hash`, `delta_hash` and delta bytes equal an in-process
`loop.train_round` on the same samples, and the router bias trains. It does not cover
`run-v2 --round` (four-member roster, relay) or a dispute on this arch [?].

### Rejected

- A `Literal` built from the registry: mypy rejects a computed `Literal`.
- The table in `trainer/model.py` plus a torch-free copy of its names: two sources of truth.
- A lazy `trainer.model` import in the validator: it loads torch inside protocol validation.
- The S2 runtime `register()`: it never reaches a torchrun worker and is not bound by the
  hash.
- Kimi on v1: frozen v1 has no per-arch rule.
- Island DP for kimi: it would need an all-reduce of the expert loads.
