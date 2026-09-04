<!-- protocol_version: 1 -->

# External miner — troubleshoot (HTTP)

**Path:** HTTP submit through [https://network.cortex.foundation](https://network.cortex.foundation).
Install `ctx` from [README](./README.md). Proof miners pay Lium
(`LIUM_API_KEY` / `X-Lium-Api-Key`).

## Installer and connectivity

| Symptom | Likely cause | What to check |
|---------|--------------|---------------|
| `install-ctx` aborts on checksum | Missing or mismatched `SHA256SUMS.txt` | The installer refuses an unverified binary. Wait for a `v*.*.*` release, or build `ctx` from this repo |
| `request to … failed` | Gateway not reachable | `ctx status --gateway https://network.cortex.foundation`. A local stack needs `--gateway http://127.0.0.1:8080` |
| `can_score: NO` / HTTP 503 | The host cannot score right now | Nothing was stored and nothing was rented. Read the error; do not retry-spend |

## Bounty

| Symptom | Likely cause | What to check |
|---------|--------------|---------------|
| CLI prints terms and stops | `--accept-terms` missing | Pairing is blocking; re-run with `--accept-terms` |
| `403 terms_required` on `POST /v1/pair` | Terms not accepted | Same as above |
| `401` on pair | Bad hotkey signature | Sign `cortex-bounty-v1\|{account_id}\|{nonce}\|{exp}` with that hotkey |
| `401 invalid_session` on reports | Session claim expired or wrong | Re-run `ctx bounty pair` |
| `already_fixed_not_prod` | Bug already patched, not in prod | Ack only — no reward, no penalty |
| `invalid_malicious` | Fabricated / does not exist | Penalty (burn toward uid 0) |
| `duplicate` | Same fingerprint as a prior report | No extra reward, no penalty. Whitespace-only edits of the same title+body still match |
| `400 title_and_body_must_differ` / `body_lacks_distinct_evidence` | Title pasted as the body, or a repeated-token farm | Write a real report: distinct title, body, and repro |
| HTTP 503 on report | Adjudication feed unreadable | `ctx bounty status` → `can_score`. The share burns; there is no offline scorer |

## Proof

| Symptom | Likely cause | What to check |
|---------|--------------|---------------|
| `400` missing / unknown / not-open `topic_id` | Topic is not currently open | `ctx proof topics`. The refusal is not a submission |
| `400` architecture / `proxy not baked` | Proxy is not the one the pin bakes | Copy `proxy_model` from `ctx proof status` |
| `400` `declared_flops exceeds the topic budget` | `declared_flops > topic.flops_budget` | Cap declared FLOPs at the open topic's budget |
| `400` invalid hotkey / `artifact_digest` | Not 64 hex | Both must be 64 hex characters |
| `rejected` with `contamination_evidence_missing` | Empty `manifest` | Declare `train_content_hashes` or `train_dataset_ids` |
| `rejected` with contamination / cheat code | Holdout overlap, unreproduced claim, strawman AdamW, … | Read the verdict `cheat_codes`. Contamination rejects without rent |
| HTTP 503 on submit | Empty `eval_image_digest`, zero open topics, unsealed baseline, or harvest down | `ctx proof status` → `can_score`. Live digest is `sha256:78b614a1…`. Empty digest still 503. Do not invent a digest. Nothing was rented |

## Off challenges

`relearn`, `relearn-image`, `relearn-agent`, `relearn-mm`, `design`, and
`prism` have no trust-root row. Submitting to them earns nothing.
`ctx relearn|image|agent` still talk to a local stack (`--gateway`); they are
not live work.

Never paste `LIUM_API_KEY`, challenge secrets, or mnemonics into tickets or git.
