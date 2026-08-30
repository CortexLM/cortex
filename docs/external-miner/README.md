<!-- protocol_version: 1 -->

# How to mine

**Bundle `protocol_version`:** `1`  
**Miner pays Lium** (`LIUM_API_KEY` / `X-Lium-Api-Key`).

This badge must match `bundle::PROTOCOL_VERSION` in crate `bundle`.
CI gate: `cargo run -p xtask -- external-docs-check`.

Live challenges: **Relearn** (`relearn`) and **Bounty** (`bounty`). HTTP submit.

| Challenge | Guide (this repo) | Notes |
|-----------|-------------------|-------|
| Relearn | [relearn.md](./relearn.md) | Long guide + eval image: [CortexLM/relearn](https://github.com/CortexLM/relearn) |
| Bounty | [bounty.md](./bounty.md) | Pair via `cortex-bounty`; Chat inject is `BOUNTY_CHAT_COMMAND` (env-only). Public leaderboard/reports live in CortexLM/backend; Cortex reads `BOUNTY_BACKEND_PUBLIC_URL` |

Pinned models: `Qwen/Qwen3.8-Flash-Next`, teacher `kimi-k3`.
Bundle bytes: [`BUNDLE_SPEC.md`](../BUNDLE_SPEC.md).

```text
https://<gateway>/challenge/relearn/...
https://<gateway>/challenge/bounty/...
```

Never put mnemonics or challenge signing keys in miner clients.
Read `LIUM_API_KEY` from the environment. Do not commit it.
