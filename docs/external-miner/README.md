<!-- protocol_version: 1 -->

# External miner docs

**Bundle `protocol_version`:** `1`  
**Miner path:** HTTP submit. **Miner pays Lium** (`LIUM_API_KEY` / `X-Lium-Api-Key`).

This badge must match `bundle::PROTOCOL_VERSION` in crate `bundle`.  
CI gate: `cargo run -p xtask -- external-docs-check`.

Cortex is a **one-challenge subnet**: **Relearn**. Design and Prism are retired
as products (their crates remain as unused libraries / historical specs).

| Challenge | `challenge_id` | Scoring | Guide | Public miner repo |
|-----------|----------------|---------|-------|-------------------|
| Relearn | `relearn` | `challenge_scoring_version` **1** (paired displacement vs champion) | [relearn.md](./relearn.md) | [CortexLM/relearn](https://github.com/CortexLM/relearn) |

Pinned models (verified Hugging Face ids):

- Base: `Qwen/Qwen3.8-Flash-Next`
- Teacher / judge: `zai-org/GLM-5.3`

Do **not** conflate version axes:

| Axis | Value | Meaning |
|------|-------|---------|
| Bundle `protocol_version` | **1** | Leaf / merkle / weight bytes ([`BUNDLE_SPEC.md`](../BUNDLE_SPEC.md)) |
| Relearn scoring | **1** | Displacement vs previous champion + overfit gates |

| Page | Topic |
|------|-------|
| [relearn.md](./relearn.md) | Artifact digest HTTP submit, Lium BYOK, promote |
| [troubleshoot.md](./troubleshoot.md) | Common HTTP / scoring failures |

Normative contracts:

- Relearn (this repo): [`../ARCHITECTURE.md`](../ARCHITECTURE.md) + `config/relearn-pin.toml`
- Public eval image / harness: [CortexLM/relearn](https://github.com/CortexLM/relearn)
- Bundle bytes: [`../BUNDLE_SPEC.md`](../BUNDLE_SPEC.md)
- Threat claim (D19): [`../THREAT_MODEL.md`](../THREAT_MODEL.md) §1

## Gateway base URL

Production/staging miners call the **gateway** reverse proxy:

```text
https://<gateway>/challenge/relearn/...
```

Local smoke (host ports from `env-local.yml`):

```bash
curl -sS http://127.0.0.1:28095/health   # relearn-challenge
```

Never paste mnemonics, Lium keys, or challenge signing keys into miner clients
or into git. Hotkeys are public 64-hex identifiers only. Read `LIUM_API_KEY`
from the environment (or send `X-Lium-Api-Key` on submit) — never commit it.
