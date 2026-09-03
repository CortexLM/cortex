<!-- protocol_version: 1 -->

# How to mine Cortex

Everything a miner needs, in order: install the CLI, point it at the public
gateway, pick a challenge, submit, and read the verdict.

**Public gateway:** `https://network.cortex.foundation`  
**Bundle `protocol_version`:** `1`  
**Miner pays Lium** (`LIUM_API_KEY` / `X-Lium-Api-Key`).

Every challenge takes **HTTP** submits through that one host, so the CLI and
`curl` are interchangeable — the CLI just knows the routes, the required
manifest fields, and what each failure means.

## 1. Install `ctx`

`ctx` is the subnet CLI: status, submits, and Bounty pairing for all four live
challenges.

```bash
curl -fsSL https://raw.githubusercontent.com/CortexLM/cortex/main/scripts/install-ctx.sh | sh
```

It installs to `~/.local/bin/ctx` after verifying the release checksum, and it
refuses to install anything it could not verify. Pin a version with
`CTX_VERSION=v0.1.0`, or change the target directory with `CTX_INSTALL_DIR`.
Archives for linux-amd64, linux-arm64, darwin-amd64, darwin-arm64, and
windows-amd64 are attached to every
[release](https://github.com/CortexLM/cortex/releases). From a clone:
`cargo build -p ctx --release`.

```bash
ctx --help          # every command
ctx challenges      # the four live challenges and what they pay for
ctx status          # can each challenge score right now, and is the epoch sealed
```

`ctx` talks to `https://network.cortex.foundation` by default. `--gateway`
overrides it, which you only need when running your own stack.

## 2. What you need before you submit

| Thing | Why |
|-------|-----|
| A Bittensor hotkey registered on the subnet | Weight is paid to a hotkey in the metagraph. Unmatched emission burns to uid 0 |
| The 64-hex form of that hotkey | The three Relearn challenges take 64-hex; Bounty pairing also accepts SS58 |
| `sha256` of the artifact you are submitting | The digest is frozen at accept, and the holdout stays sealed until it is |
| A Lium API key, if you want a live eval | You pay for your own eval pod. Export `LIUM_API_KEY`; `ctx` forwards it as `X-Lium-Api-Key` and never prints it |

Never commit the Lium key, and never put a mnemonic into a CLI, a browser, or
a chat window. Bounty pairing signs locally with sr25519.

## 3. Pick a challenge

Four live challenges: **Relearn** (`relearn`), **Relearn Image**
(`relearn-image`), **Relearn Agent** (`relearn-agent`), and **Bounty**
(`bounty`). Encoder-attach Multimodal (`relearn-mm`) is **off** — it has no
trust-root row, so it earns nothing.

| Challenge | Id | Start here | Guide |
|-----------|----|-----------|-------|
| Relearn | `relearn` | `ctx relearn submit` | [relearn.md](./relearn.md) — post-train `Qwen/Qwen3.8-27B`. Teacher `incoai/GLM-5.3-NVFP4`, wire id `glm-5.3`. Long guide + eval image: [CortexLM/relearn](https://github.com/CortexLM/relearn) |
| Relearn Image | `relearn-image` | `ctx image submit` | [relearn-image.md](./relearn-image.md) — fine-tune `nvidia/Cosmos3-Super-Text2Image` (OpenMDW 1.1). Judge is **Q-Judger** (`Qwen/Qwen-Image-Bench`). **Flux is rejected** |
| Relearn Agent | `relearn-agent` | `ctx agent submit` | [relearn-agent.md](./relearn-agent.md) — post-train the same `Qwen/Qwen3.8-27B` into a tool-using agent. Scored on **replayed tool traces**, not prompts |
| Bounty | `bounty` | `ctx bounty pair` | [bounty.md](./bounty.md) — real bug reports, adjudicated by operators. Published rows live in **CortexLM/backend** |
| Relearn Multimodal | `relearn-mm` | — | [relearn-mm.md](./relearn-mm.md) — **off.** Qwen3.8 is a native VLM, so there is no SigLIP encoder-attach product. Archived encoder pin `google/siglip2-so400m-patch14-384` |

Emission: `relearn` 4000 bps, `relearn-image` 1500, `relearn-agent` 1500,
`bounty` 3000. `relearn-mm` has no row and earns 0.
Bundle bytes: [`BUNDLE_SPEC.md`](../BUNDLE_SPEC.md).

## 4. What every challenge pays for

Each of the four promotes champion-versus-challenger on evidence that is **not
in git**, and none of them pays for the published split:

- The three Relearn challenges score on a **private holdout** whose only
  public trace is a commitment in `config/*-pin.toml`. Winning the published
  split is informational; it is not a promotion, and a public score far above
  the holdout is itself a gate failure.
- Every challenge runs a measurement kept **off the number you are paid on**:
  a general-capability canary for `relearn` and `relearn-agent`, faithfulness
  plus seed-replay for `relearn-image` (the published image does not emit a
  canary series), and a triage-noise ratio for Bounty. You cannot tune what
  you cannot see — regressing one past its epsilon is a hard zero, not a
  discount.
- **Missing evidence fails closed.** An empty training manifest is not a clean
  contamination check, an eval that skipped an arm is not a passing run, and a
  host that cannot score answers `503` instead of inventing a verdict.

## 5. Read `can_score` before you spend anything

```bash
ctx status
```

`can_score: false` means that challenge cannot turn a submission into weight
right now, so every submit answers **503** and nothing is stored. That is the
host being unready, not your artifact being rejected: retry later instead of
resubmitting variants. A `503` costs you nothing — no pod is rented, no
submission id exists, and nothing is recorded against your hotkey.

`ctx weights` shows the epoch vector. `sealed: true` is a real seal;
`sealed: false` is the fail-closed burn vector (uid 0 = 100%), which means
nothing is being paid this epoch.

## 6. The routes, if you prefer `curl`

```text
https://network.cortex.foundation/challenge/relearn/v1/submissions
https://network.cortex.foundation/challenge/relearn-image/v1/submissions
https://network.cortex.foundation/challenge/relearn-agent/v1/submissions
https://network.cortex.foundation/challenge/bounty/v1/reports
https://network.cortex.foundation/v1/weights/latest
```

Each challenge also serves `GET /v1/status`, and the Relearn family serves
`GET /v1/submissions/{id}`. Relearn Image publishes its frozen public split at
`GET /challenge/relearn-image/v1/prompts`.

Never put mnemonics or challenge signing keys in miner clients. The
`/v1/admin/*` routes are operator-only; you never promote your own run.

When something fails, [troubleshoot.md](./troubleshoot.md) maps the status
codes and reject reasons to the fix. Validators run a different job:
[validators.md](./validators.md).

Control-plane PRs on `CortexLM/cortex` need a Greptile review before merge
(`.greptile/`; comment `@greptileai review` if the bot is silent). That is
an operator gate, not a miner submit step.
