# Proof eval image

Evaluation image for challenge `proof`. The network's harvest service boots
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
The pin ships **no HF bake** and this image does not download a default
Hugging Face id. Harvest must stage:

- local measurement weights at `PROOF_PROXY_MODEL_DIR` (a directory of
  operator-provided weights; missing dir → refuse, not a silent HF fetch)
- shard bytes at `PROOF_HOLDOUT_STORE/<content_sha256>` (the image has no
  `/opt/proof-eval/holdout` tree)

## Implementation limits

This is not yet the autonomous research judge described in the whitepaper.
`judge.py` requests an authenticated acknowledgement and does not parse a
research verdict. `agent.py` performs static text checks; `cli.py` inspects the
claim and a local baseline script, then measures a configured model directory.
General reproduction of a committed miner recipe is not implemented.

`harness.py` measures model loss and optional throughput. It leaves custom and
canary metrics unset. A digest pin or successful judge HTTP request is not proof
that arbitrary research was reproduced. See the
[paper-to-code comparison](../docs/WHITEPAPER.md).
