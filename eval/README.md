# Proof eval image

Digest-pinned scorer for challenge `proof`. The control plane boots
`ghcr.io/cortexlm/proof-eval@sha256:…` on a Lium pod the miner pays for,
stages `request.json` over stdin, and runs:

```
proof-eval score --request request.json --out metrics.json
```

Harvest wrappers print `PROOF_METRICS=<document>` and `PROOF_EVAL_OK`.
`/usr/bin/proof-eval` is a regular file, not a symlink. Failures exit
non-zero with no marker.

Pin the **scoring** image (`eval/Dockerfile.scoring`, CUDA + torch), never
the contract-only digest. The RLM **judge** is the live `InferenceOffer`
(OpenAI-compatible HTTP). Auth is `teacher.env` (`OPENAI_API_KEY` /
`PROOF_INFERENCE_API_KEY`) staged by harvest — never request.json, never
git. No HF bake into the judge path. Fabric: no InfiniBand, no NVLink, no
NCCL fast path, 12.5 Gbit/s cap.

No secrets, holdout text, teacher hosts, or Modal references are baked in.
Shard bytes arrive via `PROOF_HOLDOUT_STORE/<content_sha256>`. Optional
local measurement weights via `PROOF_PROXY_MODEL_DIR` or
`PROOF_ALLOW_MODEL_DOWNLOAD=1`.
