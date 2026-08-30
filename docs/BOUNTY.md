# Bounty Challenge (live challenge)

Control-plane notes. Miners start at [`external-miner/bounty.md`](./external-miner/bounty.md).
Validators start at [`external-miner/validators.md`](./external-miner/validators.md).

**Public transparency lives in CortexLM/backend.** This subnet **reads**
`GET {BOUNTY_BACKEND_PUBLIC_URL}/v1/bounty/public/leaderboard` and
`GET {BOUNTY_BACKEND_PUBLIC_URL}/v1/bounty/public/reports`. It does not
serve `/v1/public/*` (or any unauthenticated public leaderboard). Public
consumers hit the backend. Empty `BOUNTY_BACKEND_PUBLIC_URL` → skip / sim
(CI stays green). Never bake a host into git.

Internal ingest (pair / reports / adjudicate) stays on this service so
Chat can bind a hotkey. Scoring maps hotkey → lattice from published
backend rows that include both `problem_found` and `justification`.

| Field | Value |
|-------|--------|
| `challenge_id` | `bounty` |
| `challenge_scoring_version` | `1` |
| Port | `8096` (local host `28096`) |
| Emission | `3000` bps (default; Relearn keeps `7000`) |

Miners pair a Bittensor hotkey to a dedicated Cortex Chat account, then file
bug reports. Every report is tagged with that hotkey. Operators adjudicate:

| Verdict | Weight |
|---------|--------|
| `valid` | reward (precision credit) |
| `already_fixed_not_prod` | ack only — no reward, no penalty |
| `invalid_malicious` | penalty (burn toward uid 0) |
| `duplicate` | no extra reward, no penalty |

Champion is displacement vs the previous bounty champion on a holdout of
adjudicated reports (precision, not spam volume). Validators do not evaluate
reports; they verify sealed bundles. Unmatched emission burns to uid 0.

Chat inject is env-only (`BOUNTY_CHAT_COMMAND`). Docs and examples use the
placeholder `<BOUNTY_CHAT_COMMAND>` only. Never commit the live token.
Optional `X-Lium-Api-Key` is accepted and never logged; live Lium is skipped
when no key is present.
