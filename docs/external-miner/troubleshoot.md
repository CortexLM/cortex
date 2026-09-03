<!-- protocol_version: 1 -->

# Miner troubleshooting

**Gateway:** `https://network.cortex.foundation`. Every challenge takes HTTP
submits through that host, and `ctx` is the CLI in front of them (install:
[README](./README.md#1-install-ctx)). Miner pays Lium (`LIUM_API_KEY` /
`X-Lium-Api-Key`).

Start here, always:

```bash
ctx status      # can_score per challenge, plus whether the epoch is sealed
ctx weights     # sealed: true is a real seal; false is the burn vector
```

## What a `503` means

A challenge that cannot score answers **503** instead of accepting a
submission. Nothing is stored, no submission id exists, no pod is rented, and
nothing is recorded against your hotkey. It is the host being unready, not
your artifact being rejected — retry later rather than resubmitting variants.
`ctx status` shows `can_score: false` for exactly those challenges.

The mirror image is `sealed: false` on `ctx weights`: with no sealed bundle the
gateway serves a burn vector (uid 0 = 100%) rather than a stale or invented
one. Nothing is being paid that epoch.

## CLI and connectivity

| Symptom | Likely cause | What to do |
|---------|--------------|------------|
| `install-ctx: no SHA256SUMS.txt in that release` | The release has no checksum file | Refusing to install unverified is the intended behaviour. Pin a release that has one: `CTX_VERSION=v0.1.0` |
| `install-ctx: checksum mismatch` | Corrupt or tampered download | Do not run it. Re-download; if it repeats, open a Bounty report |
| `ctx: command not found` after install | `~/.local/bin` is not on `PATH` | `export PATH="$HOME/.local/bin:$PATH"` |
| `request to … failed` | Network, DNS, or proxy between you and the gateway | Check `curl -sS https://network.cortex.foundation/v1/weights/latest` |
| `gateway must be an http(s) URL` | `--gateway` without a scheme | Pass the full URL. You only need this flag for your own stack |
| `declare what you trained on` | Empty manifest | Pass `--train-id` / `--train-hash` / `--train-dataset`, or `--manifest-file`. The CLI refuses locally so the contamination gate does not reject you after you have paid for a run |

## Relearn, Relearn Image, Relearn Agent

| Symptom | Likely cause | What to do |
|---------|--------------|------------|
| `400` on submit | Invalid hotkey or artifact digest | Both must be 64 hex chars |
| `400` naming a Flux base (Image) | Flux-family checkpoint declared | Flux is rejected on submit, before scoring. Fine-tune the pinned Cosmos3 seed instead |
| `awaiting_admin` but no weights | Operator has not promoted | Promotion is operator-only. You do not promote |
| `rejected` with `Regression` | Challenger did not displace the champion | Improve the artifact; regressions are never crowned |
| `rejected` with `PublicPrivateGap` | Overfit / memorization | A public score far above the holdout fails as overfitting |
| `rejected` with `Contamination` | Training metadata overlapped the holdout | Drop holdout ids / hashes from the manifest |
| `rejected` with `ContaminationEvidenceMissing` | The manifest declared nothing | Fill it in. An empty manifest fails the gate instead of skipping it |
| `rejected` with `CanaryRegression` | General-bench drop past ε | Off-score capability canary; it is not in the number you are paid on |
| `rejected` with `IgnoresTheImage` | Pixel-shuffle control | A vision family scored the same on shuffled pixels |
| `rejected` with `ShuffleEvidenceMissing` | The champion took the shuffle control on that vision family and this run did not | Not a text-only holdout: the family has images. Missing evidence fails the gate instead of skipping it |
| `rejected` with `PerturbationEvidenceMissing` | No perturbed rerun in the eval document | Fail-closed, like an empty manifest. The brittleness floor is not skipped by omitting the series |
| `rejected` with `BaseCanaryEvidenceMissing` | No known-answer canaries in the eval document | Fail-closed. The base-competence floor is not skipped by omitting the series |
| `rejected` with `Canaries` | Catastrophic forgetting | Base-model canaries must stay ≥ 0.95 |
| `rejected` on a pillar drop (Image) | An L1 pillar collapsed versus the champion | A big Alignment gain does not buy a Quality collapse |
| `rejected` on seed replay (Image) | Regenerated cells did not match your claimed hashes | Ship the weights you generated with, and generate deterministically |
| `rejected` on an ablation floor (Agent) | Tool ablation or observation shuffle barely moved your success | The tools have to be load-bearing. A missing arm fails closed the same way |
| `503` on submit, `can_score: false` | The host is not ready to score: judge or teacher API not up, base weights not primed, no champion baseline recorded, or the live harvest not wired | Operator-side. `ctx status` names which field is false. Nothing was rented or stored; retry later |
| `503` mentioning the eval pod | The pod could not be rented, reached, or torn down | Transient. Retry; the run is not banked and no verdict was recorded |
| `503` with an eval log tail | The eval image ran and exited without scoring | The tail is the image's own log (truncated, secrets redacted). Usually an operator-side judge or model-loading failure, not your artifact |
| `503 recorded baseline: …` | The eval returned a document bound to another run, image, or holdout | Operator issue; a mismatched document is never accepted as a score |
| Repeated `503`, no submission id | Nothing was scored, so nothing was stored | Expected. A refused attempt is not a submission and does not consume anything |
| `eval_backend: sim` / `judge_backend: sim` on your row | The host scored offline (CI / local only) | Not a live verdict. Prod and staging report `lium` |
| Teacher API `4xx` | Miner weights sent to the judge API | The teacher is judge-only, never the scored artifact, and you never call it |

## Bounty

| Symptom | Likely cause | What to do |
|---------|--------------|------------|
| `ctx: terms not accepted` | Pairing is blocking | Read the printed terms, then re-run with `--accept-terms` |
| `403 terms_required` on pair | Same, via `curl` | Send `terms_accepted: true` only if you accept them |
| `401` on pair | Bad hotkey signature | Sign `cortex-bounty-v1\|{account_id}\|{nonce}\|{exp}` with that exact hotkey (sr25519, substrate context) |
| `secret file holds a different hotkey` | Signing key is not the `--hotkey` you passed | Pick the matching key, or switch with `--wallet-hotkey` |
| `401 invalid_session` on a report | Session claim expired or wrong | Run `ctx bounty pair … --accept-terms` again; the CLI re-caches the claim |
| `no cached pairing session` | You have not paired on this machine | Pair first, or pass `--session` |
| `cached session was paired against …` | The cached claim belongs to another gateway | Pair again for the gateway you are using |
| `429` on a report | Pending cap (5) or the 60s interval | Wait. Nothing is recorded against you, but nothing is filed either |
| `400 title_and_body_must_differ` / `body_lacks_distinct_evidence` | Title pasted as the body, or a repeated-token farm | Write a real report: distinct title, body, and repro steps |
| `503` on a report | The adjudication feed is unreadable, so this host cannot pay for reports | Fail-closed by design: nothing was stored and that epoch pays nobody. Check `ctx bounty status` |
| `already_fixed_not_prod` | Bug already patched, not in prod | Ack only — no reward, no penalty, but it counts as triage noise |
| `invalid_malicious` | Fabricated / does not exist | Penalty (burn toward uid 0) |
| `duplicate` | Same fingerprint as a prior report, including closed ones | No extra reward, no penalty. Whitespace-only edits of the same title and body still match |
| High `valid_count`, zero weight | Nothing adjudicated, or no severity assigned | The published leaderboard is informational. Only adjudicated, priced reports are scored |

Never paste `LIUM_API_KEY`, challenge secrets, pairing codes, or mnemonics into
tickets, screenshots, or git.
