# OpenDecision runs

Operator notes for `arch = "od-encoder"` manifests (design: OpenDecision x Hypertrain, A/B sections).

- **Manifest**: `model.od` (ODSpec) is set iff `model.arch == "od-encoder"`. `reference_spec.profile` is required: `od-bf16-det-eager-v1` (bf16) or `od-fp32-ref-v1` (fp32). `od-bf16-det-v1` is reserved and rejected.
- **Validator rules**: layout pp=1,n_gpus=1,dp=1,ep=1,zero1=false; n_kv_heads==n_heads; d_ff==4*d_model; n_experts==top_k_experts==1; capacity_factor 1.0; aux_loss_coef 0; dims match the preset; `od.record` set iff objective != mlm and seq_len == record length - 1.
- **Datasets**: optional `dataset.source {mix_id, registry_sha256}`, `assign_unit`, `unit_sha256_root`. With `assign_unit`: `micro_batch*grad_accum*H % assign_unit == 0`, `n_samples % assign_unit == 0`, `unit_sha256_root` required.
- **Stages**: each stage is a new run. `od.warm_start = true` makes the miner fetch round-0 theta by `RoundOpen.theta_hash` instead of `init_params`; pin the start blob hash in `init_state_hash`.
- **Dispatch**: `trainer.model.param_shapes/stage_of/init_params/forward` route to `hypertrain.models.opendecision` when `cfg.arch != "decoder"`.
- Old (decoder) manifests keep byte-identical bodies and run IDs; new optional fields are omitted when unset.
- Registry and public API sections: see lanes L4 and L5.

## Teacher labels (stage B)

Soft labels for `od-b-v1` records come from an offline, configurable OpenAI-compatible teacher (default OpenRouter, `deepseek/deepseek-v4.1-flash`): see [teacher.md](teacher.md). Tulu-3 rows are not convertible yet (design gap).
