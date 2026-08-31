# Relearn LLM (live challenge)

Sibling challenges: [`RELEARN-T2I.md`](./RELEARN-T2I.md) (image generation,
judged by Q-Judger) and [`RELEARN-MM.md`](./RELEARN-MM.md) (vision encoder on
this challenge's champion). They share the champion-versus-challenger holdout
shape and the Lium payment model, and each signs leaves under its own key.

Control-plane notes. Miners start at [`external-miner/relearn.md`](./external-miner/relearn.md).
Validators start at [`external-miner/validators.md`](./external-miner/validators.md).

Eval image and harness live in [`CortexLM/relearn`](https://github.com/CortexLM/relearn).
This repo pins them in `config/relearn-pin.toml`.

| Field | Value |
|-------|--------|
| `challenge_id` | `relearn` |
| `challenge_scoring_version` | `1` |
| Base model | `Qwen/Qwen3.8-27B` (Apache-2.0, native VLM). Not Flash-Next. |
| Teacher weights | `incoai/GLM-5.3-NVFP4` (full GLM-5.3 NVFP4) — download, then serve from `RELEARN_TEACHER_LOCAL_DIR`. Never pass the Hugging Face repo id to vLLM. Not Flash. Never DFlash2 (CC BY-NC-ND). |
| Teacher / judge | HTTP API (operator sets `RELEARN_TEACHER_*`; wire id `glm-5.3`) |
| Operator GPUs | 2×B300, tensor-parallel 2 (docs only). If OOM, raise tp on those 2 GPUs — do not add an 8-GPU layout. |
| Port | `8095` (local host `28095`) |
| Emission | `4000` bps (default) |

Miner pays Lium (`LIUM_API_KEY` / `X-Lium-Api-Key`). Operator promote is
`POST /v1/admin/promote`. Epoch emit is champion lattice; others `NoScore` (D24).

## Holdout and anti-overfit gates

The holdout is **not** in git. `config/relearn-pin.toml` carries only
`holdout_commitment` and `holdout_size`. Records come from
`RELEARN_HOLDOUT_FILE` and are verified at boot; a missing or mismatched file
means submissions answer **503** rather than scoring a reconstructable seed
or the public split.

Promotion requires every gate:

| Gate | Rule |
|------|------|
| Holdout displacement | Bootstrap paired test on the private split (the only series that may enter the lattice) |
| Public–holdout gap | Public far above holdout signals memorization; empty public is fail-closed |
| Contamination | Any holdout id / image hash in submitted training metadata rejects the run |
| Pixel shuffle | Every vision family present in the holdout (caption / VQA / OCR / spatial) must drop ≥ `MIN_SHUFFLE_DROP` when pixels are shuffled |
| General-bench canary | MMLU / MMMU-style slice is **off** the visible score. Regression past `CANARY_EPSILON` vs the champion is a hard zero |
| Perturbation / base canaries / agent-trace | Existing retention floors still apply |

```bash
# Rotate the holdout (records never enter git). Never reuse the T2I/dev salt.
cargo run -p xtask -- relearn-holdout \
  --catalog ~/.base-secrets/relearn-catalog.json \
  --salt "$RELEARN_HOLDOUT_SALT" --size 120 \
  --exclude 1 --exclude 2 … \
  --out deploy/secrets/relearn/holdout.json
```

Paste the printed `holdout_commitment` into `config/relearn-pin.toml`, then
re-sign the trust root ([`../config/CEREMONY.md`](../config/CEREMONY.md)).
