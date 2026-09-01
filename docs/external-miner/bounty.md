<!-- protocol_version: 1 -->

# Bounty Challenge — miners

Report real Cortex product/backend bugs. Valid unique reports earn subnet
weight. Every report is tagged with your Bittensor hotkey so operators can
patch in real time and pay (or penalize) the right miner.

Validators do **not** re-run your reports. They verify the sealed weight
bundle. HTTP submit goes through the gateway:

```text
https://<gateway>/challenge/bounty/v1/pair
https://<gateway>/challenge/bounty/v1/reports
```

## Dedicated account (required)

Create a **dedicated** Cortex Chat mining account. Do **not** pair a private
personal account. Pairing is confidential to that account: operators see the
bound hotkey and the reports you file from it.

A private personal account is for your own chats. A mining account is for
bounty work and is used to fix bugs and to remunerate (or penalize) the
bound hotkey.

## Terms (blocking)

You must accept these terms before pairing. The pair API rejects
`terms_accepted: false`.

> By pairing a Bittensor hotkey to a Cortex Chat account for Bounty
> Challenge, you accept that this dedicated mining account, its logs, and
> its conversations may be used for research, to fix product and backend
> bugs, and to remunerate (or penalize) the bound miner hotkey. Do not pair
> a private personal account.

## Pair (CLI)

Never paste a mnemonic into Chat. Sign locally.

```bash
# Print the Chat inject command + one-time pairing code.
# BOUNTY_CHAT_COMMAND is operator-configured and unguessable.
# Examples below use the public placeholder only.
export BOUNTY_CHAT_COMMAND="${BOUNTY_CHAT_COMMAND:-<BOUNTY_CHAT_COMMAND>}"

cortex-bounty pair --hotkey <ss58> --account-id <dedicated-chat-account-id>
```

If you have a local hotkey mini-secret file (32 bytes or 64-hex — not a
mnemonic):

```bash
cortex-bounty pair \
  --hotkey <ss58> \
  --account-id <dedicated-chat-account-id> \
  --secret-file /path/to/hotkey.sk
```

If you sign offline, the CLI prints a challenge string
`cortex-bounty-v1|{account_id}|{nonce}|{exp}`. Sign that exact string with
the hotkey (sr25519, Substrate context), then:

```bash
cortex-bounty pair \
  --hotkey <ss58> \
  --account-id <dedicated-chat-account-id> \
  --signature <128-hex>
```

The CLI prints:

1. The Chat inject command from `BOUNTY_CHAT_COMMAND` (placeholder
   `<BOUNTY_CHAT_COMMAND>` when unset — never a live production token).
2. The one-time pairing code.
3. How to pick/switch hotkey if several are linked:

```bash
# Switch which linked hotkey you are pairing
cortex-bounty pair --hotkey <other-ss58> --account-id <id> \
  --wallet-name <wallet> --wallet-hotkey <name>
```

In Cortex Chat, paste the inject command, then the pairing code. That binds
the account to the hotkey and marks the session as bounty-miner.

## File a report

After pairing, Chat (or you) posts to the gateway. Optional miner-pays-Lium
header is accepted and never logged; omit it if you are not using Lium.

```bash
curl -sS -X POST https://<gateway>/challenge/bounty/v1/reports \
  -H 'content-type: application/json' \
  -H "X-Lium-Api-Key: $LIUM_API_KEY" \
  -d '{
    "session": "<session claim from pair>",
    "hotkey": "<ss58>",
    "title": "<short bug title>",
    "body": "<what broke, in at least 80 characters>",
    "repro_steps": "<how to reproduce, in at least 20 characters>"
  }'
```

Poll `GET /challenge/bounty/v1/reports/{id}`. That path is internal ingest,
not a public leaderboard.

**Public consumers** (leaderboard + published reports) hit
**CortexLM/backend** — not this subnet. Cortex **reads**
`/v1/bounty/public/leaderboard` and `/v1/bounty/public/reports` from
`BOUNTY_BACKEND_PUBLIC_URL` (operator env). Scoring uses `problem_found` +
`justification` + `severity` + hotkey counts on those payloads.

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
a report either. `GET /challenge/bounty/v1/status` publishes the live numbers.

If the host has no scoring backend configured, `POST /v1/reports` answers
**503** rather than accepting work it could never pay for. Check
`GET /challenge/bounty/v1/status` → `can_score` before you go hunting.

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
them away — and above 5000 bps they are a hard zero. Re-filing the same finding
is free in your visible score and fatal in your actual one.

Unmatched emission burns to uid 0.

Never commit `LIUM_API_KEY`, pairing secrets, or the live Chat inject token.
If something fails, see [troubleshoot.md](./troubleshoot.md).
