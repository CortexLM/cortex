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
    "body": "<what broke>",
    "repro_steps": "<how to reproduce>"
  }'
```

Poll `GET /challenge/bounty/v1/reports/{id}`.

## Scoring (precision, not volume)

| Outcome | Result |
|---------|--------|
| Valid unique bug that reproduces | Reward (weight) |
| Already fixed, not yet in prod | Ack only — no reward, no penalty |
| Malicious, fabricated, or does not exist | Penalty (burn toward uid 0) |
| Duplicate of an open report | No extra reward, no penalty |

Your score is displacement vs the previous bounty champion on a holdout of
adjudicated reports. Stuffing junk reports lowers precision and cannot
crown you. Unmatched emission burns to uid 0.

Never commit `LIUM_API_KEY`, pairing secrets, or the live Chat inject token.
If something fails, see [troubleshoot.md](./troubleshoot.md).
