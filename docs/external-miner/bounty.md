<!-- protocol_version: 1 -->

# Bounty Challenge — miners

Challenge id is `bounty`. One of two live challenges (`bounty` 7000 bps,
`proof` 3000 bps). Report real Cortex product and backend bugs. Valid unique
reports earn subnet weight. Every report is tagged with your Bittensor hotkey
so operators can patch in real time and pay (or penalize) the right miner.

**Gateway:** `https://network.cortex.foundation`  
**CLI:** `ctx bounty pair`, then `ctx bounty report` (install:
[README](./README.md))

Validators do **not** re-run your reports. They verify the sealed weight
bundle. Ingest goes through the gateway:

```text
https://network.cortex.foundation/challenge/bounty/v1/pair
https://network.cortex.foundation/challenge/bounty/v1/reports
https://network.cortex.foundation/challenge/bounty/v1/status
```

## Dedicated account (required)

Create a **dedicated** Cortex Chat mining account. Do **not** pair a private
personal account. Pairing is confidential to that account: operators see the
bound hotkey and the reports you file from it.

A private personal account is for your own chats. A mining account is for
bounty work and is used to fix bugs and to remunerate (or penalize) the
bound hotkey.

## Terms (blocking)

You must accept these terms before pairing. `ctx bounty pair` prints them and
refuses to pair until you pass `--accept-terms`; the pair API rejects
`terms_accepted: false`.

> By pairing a Bittensor hotkey to a Cortex Chat account for Bounty
> Challenge, you accept that this dedicated mining account, its logs, and
> its conversations may be used for research, to fix product and backend
> bugs, and to remunerate (or penalize) the bound miner hotkey. Do not pair
> a private personal account.

## 1. Pair your hotkey

Never paste a mnemonic anywhere. `ctx` signs the pairing challenge locally
with sr25519 and sends only the signature.

```bash
# Read the terms first (no signing, no network write):
ctx bounty pair --hotkey your-ss58-hotkey --account-id your-chat-account-id

# Then pair, signing from a Bittensor wallet on this machine:
ctx bounty pair \
  --hotkey your-ss58-hotkey \
  --account-id your-chat-account-id \
  --wallet-name your-wallet \
  --wallet-hotkey default \
  --accept-terms
```

Other ways to sign the same challenge:

| You have | Flag |
|----------|------|
| A 32-byte hotkey mini-secret file (not a mnemonic) | `--secret-file /path/to/hotkey.sk` |
| An offline signer | run pair without a signing flag, sign the printed `cortex-bounty-v1\|…` string, then re-run with `--signature` and the 128-hex result |
| Several linked hotkeys | pick with `--hotkey` and `--wallet-hotkey`; pair again to switch |

On success the CLI prints the bound hotkey, the session id, and the Chat
activation step, and caches the session claim under
`~/.config/cortex/bounty-session.json` (mode 0600). There is no token for you
to export and no environment variable for you to set: activation is whatever
the CLI prints after pairing. Paste the pairing code it shows into your
dedicated mining account in Cortex Chat and Chat confirms the binding.

Session claims expire. When a report answers `401`, pair again.

## 2. File a report

```bash
ctx bounty report \
  --title "gateway 500 on artifact_uri with a query string" \
  --body-file report.md \
  --repro-file repro.md
```

`--body` and `--repro` take inline text instead of files, and `--session`
overrides the cached claim. `ctx` checks the shape locally — a real title, at
least 80 characters of body, at least 20 characters of repro steps, and a body
that is not just the title pasted twice — so a thin report does not burn a
rate-limit window.

The same thing with `curl` (the Lium header is optional and never logged):

```bash
curl -sS -X POST https://network.cortex.foundation/challenge/bounty/v1/reports \
  -H 'content-type: application/json' \
  -H "X-Lium-Api-Key: $LIUM_API_KEY" \
  -d '{
    "session": "session claim from pair",
    "title": "short bug title",
    "body": "what broke, in at least 80 characters",
    "repro_steps": "how to reproduce, in at least 20 characters"
  }'
```

Then follow it:

```bash
ctx bounty show by_0123456789abcdef
```

That path is internal ingest, not a public leaderboard.

**Public consumers** (leaderboard and published reports) hit
**CortexLM/backend** — not this subnet. Cortex **reads**
`/v1/bounty/public/leaderboard` and `/v1/bounty/public/reports` from that
backend, and scoring uses `problem_found` + `justification` + `severity` +
hotkey counts on those payloads.

Those published rows are the whole path from a bug to weight: the challenge
service fetches them each tick, signs one leaf per metagraph hotkey for the
current epoch, and posts them to the gateway. Validators then verify the
sealed bundle — they never read the bounty feed and never re-run your report.

The published leaderboard is **informational**. Topping it is worth nothing on
its own: only adjudicated, justified reports are scored, so a hotkey with a
high `valid_count` and no adjudications is paid zero.

## Report quotas

Adjudication is the scarce resource here: every pending report costs a human or
an agent a triage pass. The service enforces, per hotkey:

| Limit | Value |
|-------|-------|
| Reports awaiting adjudication | 5 |
| Interval between reports | 60s |
| Minimum body | 80 characters |
| Minimum `repro_steps` | 20 characters |

Over the pending cap or inside the interval returns `429`; too thin returns
`400`. Neither is a penalty — nothing is recorded against you — but neither is
a report either. `ctx bounty status` publishes the live numbers.

## The scorer fails closed

```bash
ctx bounty status
```

The adjudication feed published by CortexLM/backend is the **only** scorer. If
the host cannot read it, `POST /v1/reports` answers **503** rather than
accepting work it could never pay for, and that epoch pays nobody — every
bounty leaf is an explicit no-score, so the share burns to uid 0. There is no
offline stand-in, so a 503 here is the honest answer rather than a temporary
degradation you can submit through. Check `scoring_backend` and `can_score`
before you go hunting.

## Scoring (precision × severity, not volume)

| Outcome | Result |
|---------|--------|
| Valid unique bug that reproduces, with an operator severity | Reward (weight), scaled by that severity |
| Valid bug the operator did not assign a severity to | Not creditable — an unpriced bug cannot be paid for, and it blocks your crown until it is priced |
| Already fixed, not yet in prod | Ack only — no reward, no penalty, but it counts as triage noise |
| Malicious, fabricated, or does not exist | Penalty (burn toward uid 0) |
| Duplicate of an open report | No reward, no penalty, counts as triage noise |

Your score is displacement vs the previous bounty champion on adjudicated
reports, and it is the **product of two things**:

- **precision** — `valid / (valid + malicious)`. Junk is subtracted, not
  ignored, and a net-negative miner burns toward uid 0. A champion also needs
  at least 6000 bps of precision outright, so beating a sloppy incumbent while
  still filing mostly junk does not crown you.
- **impact** — the mean severity operators assigned (`trivial`, `minor`,
  `major`, `critical`). This is what stops the volume play: forty cosmetic
  findings at perfect precision are worth a fraction of four critical ones, and
  the arithmetic says so.

One measurement is deliberately **not** in the number you are paid on: the
share of your adjudications that were duplicates or re-files of already-fixed
issues. Precision cannot see those, so a miner tuning precision cannot tune
them away — and above 5000 bps of triage noise they are a hard zero. Re-filing
the same finding is free in your visible score and fatal in your actual one.

Unmatched emission burns to uid 0.

Never commit `LIUM_API_KEY`, pairing secrets, or a mnemonic. If something
fails, see [troubleshoot.md](./troubleshoot.md).
