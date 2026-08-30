<!-- protocol_version: 1 -->

# External miner — troubleshoot (HTTP)

**Path:** HTTP submit to **relearn**. Miner pays Lium (`LIUM_API_KEY` / `X-Lium-Api-Key`).

## Relearn

| Symptom | Likely cause | What to check |
|---------|--------------|---------------|
| `400` on `POST /v1/submissions` | Invalid hotkey or artifact digest | Both must be 64 hex chars |
| `awaiting_admin` but no weights | Operator has not promoted | `POST /v1/admin/promote` is operator-only |
| `rejected` with `Regression` | Challenger did not displace the champion | Improve the artifact; regressions are never crowned |
| `rejected` with `PublicPrivateGap` | Overfit / contamination | Public-private gap exceeded the gate |
| `rejected` with `Canaries` | Catastrophic forgetting | Base-model canaries must stay ≥ 0.95 |
| Live Lium skipped | No `eval_image_digest` pin or no `X-Lium-Api-Key` | Cortex refuses rent until relearn CI publishes a digest; sim still scores |
| Teacher 4xx | Miner weights sent to the judge API | Teacher is judge-only; never the scored artifact |

## Bounty

| Symptom | Likely cause | What to check |
|---------|--------------|---------------|
| `403 terms_required` on `POST /v1/pair` | Terms not accepted | Pairing is blocking; accept the dedicated-account terms |
| `401` on pair | Bad hotkey signature | Sign `cortex-bounty-v1\|{account_id}\|{nonce}\|{exp}` with that hotkey |
| `401 invalid_session` on reports | Session claim expired or wrong | Re-run `cortex-bounty pair` and paste the new code in Chat |
| `already_fixed_not_prod` | Bug already patched, not in prod | Ack only — no reward, no penalty |
| `invalid_malicious` | Fabricated / does not exist | Penalty (burn toward uid 0) |
| `duplicate` | Same fingerprint as an open report | No extra reward, no penalty |
| Chat inject unknown | Guessing a slash command | The inject token is unguessable and comes from `BOUNTY_CHAT_COMMAND` |

Never paste `LIUM_API_KEY`, challenge secrets, Chat inject tokens, or mnemonics into tickets or git.
