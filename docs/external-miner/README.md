<!-- protocol_version: 1 -->

# How to mine

**Bundle `protocol_version`:** `1`  
**Miner pays Lium** (`LIUM_API_KEY` / `X-Lium-Api-Key`).

This badge must match `bundle::PROTOCOL_VERSION` in crate `bundle`.
CI gate: `cargo run -p xtask -- external-docs-check`.

Live challenge: **Relearn LLM** (`relearn`). T2I, encoder-attach Multimodal,
and Bounty are **not live**. HTTP submit.

| Challenge | Guide (this repo) | Notes |
|-----------|-------------------|-------|
| Relearn LLM | [relearn.md](./relearn.md) | Live. Post-train `Qwen/Qwen3.8-27B`. Long guide + eval image: [CortexLM/relearn](https://github.com/CortexLM/relearn) |
| Relearn T2I | [relearn-t2i.md](./relearn-t2i.md) | Not live. Archived: `nvidia/Cosmos3-Super-Text2Image` (OpenMDW 1.1). Judge is **Q-Judger** (`Qwen/Qwen-Image-Bench`). **Flux is rejected** |
| Relearn Multimodal | [relearn-mm.md](./relearn-mm.md) | Not live — Qwen3.8 is a native VLM; no SigLIP encoder-attach. Archived encoder pin `google/siglip2-so400m-patch14-384` |
| Bounty | [bounty.md](./bounty.md) | Not live. Pair via `cortex-bounty`; Chat inject is `BOUNTY_CHAT_COMMAND` (env-only). Cortex reads CortexLM/backend at `BOUNTY_BACKEND_PUBLIC_URL` |

Pinned live model: `Qwen/Qwen3.8-27B`. Teacher weights
`LibertAIDAI/GLM-5.3-Flash-NVFP4` (served from a local dir, never passed as a
Hugging Face repo id to vLLM). Wire id `glm-5.3-flash`.
Bundle bytes: [`BUNDLE_SPEC.md`](../BUNDLE_SPEC.md).

Every Relearn challenge promotes champion-versus-challenger on a **private
holdout**. Winning the published split is informational; it is not a promotion.

```text
https://<gateway>/challenge/relearn/...
https://<gateway>/challenge/relearn-t2i/...
https://<gateway>/challenge/relearn-mm/...
https://<gateway>/challenge/bounty/...
```

Never put mnemonics or challenge signing keys in miner clients.
Read `LIUM_API_KEY` from the environment. Do not commit it.

Control-plane PRs on `CortexLM/cortex` need a Greptile review before merge
(`.greptile/`; comment `@greptileai review` if the bot is silent). That is
an operator gate, not a miner submit step.
