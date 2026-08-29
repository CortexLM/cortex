<!-- protocol_version: 1 -->

# How to mine

**Bundle `protocol_version`:** `1`  
**Miner pays Lium** (`LIUM_API_KEY` / `X-Lium-Api-Key`).

This badge must match `bundle::PROTOCOL_VERSION` in crate `bundle`.
CI gate: `cargo run -p xtask -- external-docs-check`.

Live challenge is **Relearn** (`challenge_id` `relearn`). HTTP submit.

| Challenge | Guide (this repo) | Long guide + eval image |
|-----------|-------------------|-------------------------|
| Relearn | [relearn.md](./relearn.md) | [CortexLM/relearn](https://github.com/CortexLM/relearn) |

Pinned models: `Qwen/Qwen3.8-Flash-Next`, `zai-org/GLM-5.3`.
Bundle bytes: [`BUNDLE_SPEC.md`](../BUNDLE_SPEC.md).

```text
https://<gateway>/challenge/relearn/...
```

Never put mnemonics or challenge signing keys in miner clients.
Read `LIUM_API_KEY` from the environment. Do not commit it.
