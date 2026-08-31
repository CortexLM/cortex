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
| `rejected` with `Contamination` | Training metadata overlapped the holdout | Drop holdout ids / image hashes from `manifest.train_*` |
| `rejected` with `ContaminationEvidenceMissing` | `manifest` declared nothing | Fill `manifest.train_item_ids` / `train_image_hashes` / `train_dataset_ids`. An empty manifest fails the gate instead of skipping it |
| `rejected` with `CanaryRegression` | General-bench drop past ε | Off-path MMLU/MMMU canary; not in the visible score |
| `rejected` with `IgnoresTheImage` | Pixel-shuffle control | Vision family scored the same on shuffled pixels |
| `503` on submit | Holdout file missing or mismatched | Operator: `RELEARN_HOLDOUT_FILE` must match the pin commitment |
| `503 eval image digest not pinned` | Host has no `sha256:` eval image and did not opt into sim | Wait for relearn CI to publish a digest. `GET /v1/status` shows `can_score: false` |
| `503 … no in-process sim` | Digest is pinned but the live harvest is not wired on that host | Operator issue; `/v1/status` shows `live_harvest_wired: false`. The control plane refuses to substitute sim numbers |
| `503 backend: lium …` | The eval pod could not be rented, reached, or torn down | Transient. Retry; the run is not banked and no verdict was recorded |
| `503 recorded baseline: …` | The eval image returned a document bound to another run, image, or holdout | Operator issue; a mismatched document is never accepted as a score |
| `503 no champion baseline recorded` | The host has not measured the base model, so there is nothing to compare against | Operator issue; `/v1/status` shows `champion_baseline_recorded: false` |
| Repeated `503`, no submission id | Nothing was scored, so nothing was stored | Expected. A refused attempt is not a submission and does not consume anything |
| `rejected` with `Canaries` | Catastrophic forgetting | Base-model canaries must stay ≥ 0.95 |
| `eval_backend: "sim"` on your row | Operator set `RELEARN_FORCE_SIM=1` | CI / local only. Not a live verdict; prod and staging set it to `false` |
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
