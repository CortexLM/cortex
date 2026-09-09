<!-- protocol_version: 1 -->

# How to mine

Cortex aims to reward reproducible research that can improve shared methods,
rather than only selecting a finished model. New to the project? Read the
[overview](../OVERVIEW.md).

**Before spending compute on Proof:** review the
[implementation limits](../WHITEPAPER.md#proposal-versus-current-code). The judge
is partial, submission state is in memory, and automatic Proof reward emission
is not wired into the service. A ready status is not an end-to-end payment guarantee.

**Bundle `protocol_version`:** `1`  
**Miner pays Lium** (`LIUM_API_KEY` / `X-Lium-Api-Key`) on Proof.

This badge must match `bundle::PROTOCOL_VERSION` in crate `bundle`.
CI gate: `cargo run -p xtask -- external-docs-check`.

Two live challenges: **Bounty** (`bounty`) and **Proof** (`proof`). Both take
HTTP submits through the public gateway. Emission is **bounty 2000 bps /
proof 8000 bps** (20/80). `relearn`, `relearn-image`, `relearn-agent`,
`relearn-mm`, `design`, and `prism` are **off** — they have no trust-root
row, so they earn nothing.

Install the CLI:

```bash
curl -fsSL https://raw.githubusercontent.com/CortexLM/cortex/main/scripts/install-ctx.sh | sh
ctx --help
ctx challenges
ctx status
```

The installer takes the newest [release](https://github.com/CortexLM/cortex/releases)
and verifies `ctx-<os>-<arch>.tar.gz` against that release's `SHA256SUMS.txt`.
It refuses anything unverified: a release published without those files
stops with a message naming it and installs nothing. To install a specific
release instead, pin the tag with `CTX_VERSION`:

```bash
curl -fsSL https://raw.githubusercontent.com/CortexLM/cortex/main/scripts/install-ctx.sh | CTX_VERSION=vX.Y.Z sh
```

Default gateway is [https://gateway.cortex.foundation](https://gateway.cortex.foundation).
`--gateway` overrides it for a local stack. `LIUM_API_KEY` is forwarded as
`X-Lium-Api-Key` and never printed.

| Challenge | Id | Guide | Notes |
|-----------|----|-------|-------|
| Bounty | `bounty` | [bounty.md](./bounty.md) | Real bug reports. Pair with `ctx bounty pair`, then `ctx bounty report`. Cortex reads CortexLM/backend for scoring. **2000 bps** |
| Proof | `proof` | [proof.md](./proof.md) | Reproducible experiments (claim + code + FLOPs) against **operator-published** topics. Digest-pinned RLM judge (`sha256:78b614a1…`). Empty eval digest → 503. **8000 bps** |

Emission: `bounty` 2000 bps, `proof` 8000 bps (sum 10000). Off challenges have
no row and earn 0.
Bundle bytes: [`BUNDLE_SPEC.md`](../BUNDLE_SPEC.md).

## What every challenge pays for

Neither live challenge pays for a published split you can grind:

- Proof scores operator-published topics against a **private per-topic holdout**.
  You submit a claim + reproducible recipe + `declared_flops` vs `topic_id`.
  The pin has no catalog; `GET /challenge/proof/v1/proof/topics` is the live
  list (operators inject topics at any time). The canary stays
  **off the number you are paid on**. Paid score is the **sum of per-topic**
  masses (`wta` or `discovery`). Empty `eval_image_digest` still **503**;
  the live pin is
  `sha256:78b614a1f51ce5dd80076c4e343a2b31b85d6c36025e02836cb83929867e7009`.
- Bounty pays precision times severity. The triage-noise ratio stays off the
  visible score. An unpriced `valid` row is not creditable.
- **Missing evidence fails closed.** An empty training manifest is not a clean
  contamination check, and a host that cannot score answers `503` instead of
  inventing a verdict. Check `GET /challenge/proof/v1/status` (or
  `GET /challenge/bounty/v1/status`, or `ctx status`) before you spend
  anything. Bare `GET /v1/status` on the public gateway is not the Proof
  status path.

```text
https://gateway.cortex.foundation/challenge/bounty/...
https://gateway.cortex.foundation/challenge/proof/...
```

Never put mnemonics or challenge signing keys in miner clients.
Read `LIUM_API_KEY` from the environment. Do not commit it.

Network software PRs on `CortexLM/cortex` need a Greptile review before merge
(`.greptile/`; comment `@greptileai review` if the bot is silent). That is
an operator gate, not a miner submit step.
