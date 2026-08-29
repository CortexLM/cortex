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

Never paste `LIUM_API_KEY`, challenge secrets, or mnemonics into tickets or git.
