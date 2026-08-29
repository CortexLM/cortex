# Relearn challenge (live)

One-challenge Cortex subnet. Challenge artifacts (eval image, harness,
generators, teacher) live in [`CortexLM/relearn`](https://github.com/CortexLM/relearn).
This repo is the control plane and pins `config/relearn-pin.toml`.

## Identifiers

| Field | Value |
|-------|--------|
| `challenge_id` | `relearn` |
| `challenge_scoring_version` | `1` |
| Base model | `Qwen/Qwen3.8-Flash-Next` (verified Hugging Face id) |
| Teacher / judge | `zai-org/GLM-5.3` (verified Hugging Face id) |
| Teacher NVFP4 | `Inferact/GLM-5.3-NVFP4` (preferred Lium serve when practical) |
| Port | `8095` |
| `SCORE_MAX` | `1_000_000` |
| Emission | `10000` bps |

Miner pays Lium (`LIUM_API_KEY` / `X-Lium-Api-Key`). The control plane
rents a digest-pinned eval image and operator-SSH harvests receipts.

## Submit / promote

1. `POST /v1/submissions` `{ miner_hotkey, artifact_digest, artifact_uri? }`
2. Freeze digest + nonce; unseal holdout
3. Paired displacement vs champion + public-private / perturbation / canary /
   agent-trace gates
4. Never crown a regression
5. `POST /v1/admin/promote` (operator bearer) only when eligible
6. D24 exact-E leaf set via `challenge-common`

Teacher HTTP API is **judge-only**. Miner weights are never the served model
on that API. NVFP4-on-Lium is the preferred teacher path when an 8×
Blackwell-class host can be rented; v0 defaults to HTTP API / Sim.

## Official benches

No official-benchmark contamination. Train and eval generators are disjoint;
decontam vs official benches lives in CortexLM/relearn.
