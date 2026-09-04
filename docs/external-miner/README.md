<!-- protocol_version: 1 -->

# How to mine

**Bundle `protocol_version`:** `1`  
**Miner pays Lium** (`LIUM_API_KEY` / `X-Lium-Api-Key`).

This badge must match `bundle::PROTOCOL_VERSION` in crate `bundle`.
CI gate: `cargo run -p xtask -- external-docs-check`.

Five live challenges: **Relearn** (`relearn`), **Relearn Image**
(`relearn-image`), **Relearn Agent** (`relearn-agent`), **Bounty**
(`bounty`), and **Proof** (`proof`). All five take HTTP submits. Encoder-attach Multimodal
(`relearn-mm`) is **off** — it has no trust-root row, so it earns nothing.

| Challenge | Id | Guide (this repo) | Notes |
|-----------|----|-------------------|-------|
| Relearn | `relearn` | [relearn.md](./relearn.md) | Post-train `Qwen/Qwen3.8-27B`. Teacher `incoai/GLM-5.3-NVFP4`, wire id `glm-5.3`. Long guide + eval image: [CortexLM/relearn](https://github.com/CortexLM/relearn) |
| Relearn Image | `relearn-image` | [relearn-image.md](./relearn-image.md) | Fine-tune `nvidia/Cosmos3-Super-Text2Image` (OpenMDW 1.1). Judge is **Q-Judger** (`Qwen/Qwen-Image-Bench`). **Flux is rejected** |
| Relearn Agent | `relearn-agent` | [relearn-agent.md](./relearn-agent.md) | Post-train the same `Qwen/Qwen3.8-27B` into a tool-using agent. Scored on **replayed tool traces**, not prompts |
| Bounty | `bounty` | [bounty.md](./bounty.md) | Real bug reports. Pair via `cortex-bounty`; Chat inject is `BOUNTY_CHAT_COMMAND` (env-only). Cortex reads CortexLM/backend at `BOUNTY_BACKEND_PUBLIC_URL` |
| Proof | `proof` | [proof.md](./proof.md) | Reproducible experiments against **operator-published** topics. Digest-pinned RLM judge. Empty eval digest → 503 |
| Relearn Multimodal | `relearn-mm` | [relearn-mm.md](./relearn-mm.md) | **Off.** Qwen3.8 is a native VLM, so there is no SigLIP encoder-attach product. Archived encoder pin `google/siglip2-so400m-patch14-384` |

Emission: `relearn` 3000 bps, `relearn-image` 1000, `relearn-agent` 1000,
`bounty` 3000, `proof` 2000. `relearn-mm` has no row and earns 0.
Bundle bytes: [`BUNDLE_SPEC.md`](../BUNDLE_SPEC.md).

## What every challenge pays for

Each of the five promotes champion-versus-challenger on evidence that is **not
in git**, and none of them pays for the published split:

- The three Relearn challenges score on a **private holdout** whose only
  public trace is a commitment in `config/*-pin.toml`. Winning the published
  split is informational; it is not a promotion, and a public score far above
  the holdout is itself a gate failure.
- Proof scores operator-published topics against a **private per-topic holdout**.
  The pin has no catalog; `GET /v1/proof/topics` is the live list.
- Every challenge runs a measurement kept **off the number you are paid on**:
  a general-capability canary for `relearn` and `relearn-agent`, faithfulness
  plus seed-replay for `relearn-image` (the published image does not emit a
  canary series), and a triage-noise ratio for Bounty. You cannot tune what
  you cannot see — regressing one past its epsilon is a hard zero, not a
  discount.
- **Missing evidence fails closed.** An empty training manifest is not a clean
  contamination check, an eval that skipped an arm is not a passing run, and a
  host that cannot score answers `503` instead of inventing a verdict. Check
  `GET /v1/status` → `can_score` before you spend anything.

```text
https://<gateway>/challenge/relearn/...
https://<gateway>/challenge/relearn-image/...
https://<gateway>/challenge/relearn-agent/...
https://<gateway>/challenge/bounty/...
https://<gateway>/challenge/proof/...
```

Never put mnemonics or challenge signing keys in miner clients.
Read `LIUM_API_KEY` from the environment. Do not commit it.

Control-plane PRs on `CortexLM/cortex` need a Greptile review before merge
(`.greptile/`; comment `@greptileai review` if the bot is silent). That is
an operator gate, not a miner submit step.
