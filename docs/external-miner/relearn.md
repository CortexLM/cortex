<!-- protocol_version: 1 -->

# Relearn — miner submit

**Challenge id:** `relearn`  
**Base model (verified):** `Qwen/Qwen3.8-Flash-Next`  
**Teacher / judge (verified):** `zai-org/GLM-5.3`  
**Eval image:** digest-pinned from [CortexLM/relearn](https://github.com/CortexLM/relearn)  
**No Phala/CVM. No TDX.**

Miners improve the pinned base. Score is **displacement vs the previous
champion** on a holdout that is unsealed **only after** the submission digest
freezes. A regression is never crowned.

## Submit

```bash
curl -sS -X POST https://<gateway>/challenge/relearn/v1/submissions \
  -H 'content-type: application/json' \
  -H "X-Lium-Api-Key: $LIUM_API_KEY" \
  -d '{
    "miner_hotkey": "<64-hex hotkey>",
    "artifact_digest": "<sha256 of your improved weights/adapter>",
    "artifact_uri": "optional-hf-or-object-url"
  }'
```

`X-Lium-Api-Key` is miner BYOK. The control plane never logs it and never
writes it to git. The miner pays Lium; Cortex rents the digest-pinned eval
image and operator-SSH harvests the receipt.

## Flow

1. Accept + **freeze** `submission_digest = sha256(hotkey || artifact || nonce)`.
2. **Unseal holdout** (seed was hidden until the digest froze).
3. Eval on the pinned image (sim in CI / no key; live Lium when a key is present
   **and** `config/relearn-pin.toml` has a real `eval_image_digest`).
4. Paired test vs the current champion + retention/overfit gates
   (public-private gap, perturbation, canaries, agent-trace).
5. Eligible runs sit in `awaiting_admin`. Operator `POST /v1/admin/promote`
   with a bearer from `deploy/secrets/relearn/admin_tokens`.
6. Epoch emit: champion lattice; everyone else explicit `NoScore` (D24).

## Teacher

Prefer serving frozen GLM-5.3 **NVFP4** (`Inferact/GLM-5.3-NVFP4`) on Lium
when an 8× Blackwell-class host is available (`RELEARN_TEACHER_BACKEND=lium`).
v0 default is a **teacher-only HTTP API** (`RELEARN_TEACHER_API_URL`) or Sim.
The teacher API is **judge-only** — miner weights are never the served model.

## Official benches

Do not train or score on official public benchmark items. The factory uses
disjoint train/eval generators and decontamination against official benches.
Holdout items are synthetic and unsealed after digest freeze.

## Routes

| Method | Path | Who |
|--------|------|-----|
| GET | `/health` | anyone |
| GET | `/v1/status` | anyone |
| POST | `/v1/submissions` | miner |
| GET | `/v1/submissions` | miner |
| GET | `/v1/submissions/{id}` | miner |
| POST | `/v1/admin/promote` | operator bearer |
