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
Shard bytes arrive via `PROOF_HOLDOUT_STORE/<content_sha256>`. Optional
local measurement weights via `PROOF_PROXY_MODEL_DIR` or
`PROOF_ALLOW_MODEL_DOWNLOAD=1`.

## Implementation limits

This is not yet the autonomous research judge described in the whitepaper.
`judge.py` requests an authenticated acknowledgement and does not parse a
research verdict. `agent.py` performs static text checks: clean inspection raises
`ContractError` absent agent reproduction and verified FLOP evidence; forbidden
fabric yields a rejection. `cli.py` still refuses success unless
`PROOF_TRAINING_EVIDENCE_FILE` carries a retained compute trace whose FLOPs
independently recompute. General recipe reproduction is not implemented.
`adamw.py` is a parameter lock, not executable training.

Holdout measurement records a listed-op compute trace (`mm` / `addmm` / `bmm` /
math SDPA). Unlisted compute-shaped ops refuse. This is not a universal formula
and not hostile-proof of flash or custom kernels. `artifact_fingerprint` hashes
file bytes. Custom and canary metrics remain unset. Optional observer/judge-egress wiring exists in
`proof-experiment`, with explicit `PROOF_JUDGE_PROXY=1` helper transport without Authorization only for the exact alias `http://proof-judge:8080/v1` and `chat/completions`; direct mode still requires a key. A
synthetic probe obtained a real completion and blocked four sampled escapes;
arbitrary allowed payloads and artifact code leave confidentiality/integrity
unproven. A digest pin or successful judge HTTP request is not proof
that arbitrary research was reproduced. See the
[paper-to-code comparison](../docs/WHITEPAPER.md).

Public candidate: `ghcr.io/cortexlm/proof-eval@sha256:9e32451e178a2e04f33be592ea73f7f89acf723631d53291dbab016c4690ee92`
(tag `3e92bc28-flop-correction-candidate`); anonymous tag/digest HTTP 200 and
hash match verified. Not a production repin; this image predates the latest
proxy-helper corrections and has not been run on a real GPU. Python **31/31**
and proxy adversarial **3/3** checks passed; rebuilt test-image Astra egress
**2/2** passed, not a real evaluation recipe. DNS-based enforcement and arbitrary
successful-payload exfiltration/integrity remain unproven.
