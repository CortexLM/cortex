<!-- protocol_version: 1 -->

# How to mine

**Bundle `protocol_version`:** `1`  
**Miner pays Lium** (`LIUM_API_KEY` / `X-Lium-Api-Key`).

This badge must match `bundle::PROTOCOL_VERSION` in crate `bundle`.
CI gate: `cargo run -p xtask -- external-docs-check`.

Live challenges: **Relearn LLM** (`relearn`), **Relearn T2I** (`relearn-t2i`),
**Relearn Multimodal** (`relearn-mm`), and **Bounty** (`bounty`). HTTP submit.

| Challenge | Guide (this repo) | Notes |
|-----------|-------------------|-------|
| Relearn LLM | [relearn.md](./relearn.md) | Long guide + eval image: [CortexLM/relearn](https://github.com/CortexLM/relearn) |
| Relearn T2I | [relearn-t2i.md](./relearn-t2i.md) | Fine-tune `nvidia/Cosmos3-Super-Text2Image` (OpenMDW 1.1). Judge is **Q-Judger** (`Qwen/Qwen-Image-Bench`) on Qwen-Image-Bench prompts. **Flux is rejected** |
| Relearn Multimodal | [relearn-mm.md](./relearn-mm.md) | Attach an **Apache-2.0 / MIT / BSD / ISC** vision encoder to the champion LLM. Regressing the LLM scores zero |
| Bounty | [bounty.md](./bounty.md) | Pair via `cortex-bounty`; Chat inject is `BOUNTY_CHAT_COMMAND` (env-only). Public leaderboard/reports live in CortexLM/backend; Cortex reads `BOUNTY_BACKEND_PUBLIC_URL` |

Pinned models: `Qwen/Qwen3.8-Flash-Next`, teacher `kimi-k3`,
`nvidia/Cosmos3-Super-Text2Image`, `google/siglip2-so400m-patch14-384`.
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
