<!-- protocol_version: 1 -->

# Miner and validator troubleshooting

Use the [Python CLI](README.md) and the gateway selected by the operator. Read
the actual HTTP error and signed topic before retrying. Do not include wallet
files, provider keys, Bounty sessions or private artifacts in diagnostics.

## Installation and identity

| Symptom | Check |
|---------|-------|
| `ctx` examples do not match the CLI | Use `uv run cortex miner --help`; the Python client is `cortex` |
| Bittensor wallet support unavailable | Install the existing `chain` extra with `uv sync --locked --extra chain` |
| `Bittensor hotkey unavailable` | Check wallet name, hotkey name, wallet root and the password file; loading does not create keys |
| Encrypted hotkey password file required | Add miner `--wallet-password-file` before the action; the miner does not prompt interactively |
| Credential file unavailable or not private | Use a regular, non-symlink file with no group/other permissions, such as `0600` |
| Proof owner public key required | Add miner `--proof-public` with the independently pinned Proof key, not the gateway key |
| Invalid Proof signature from `wallet.sign()` | Proof needs Cortex context `base-sr25519-v1`, domain `base-proof-submit-v1`; use the Python wallet signer |
| HTTP connection failure | Verify the selected gateway URL and TLS; there is no implicit deployment URL |

The miner's `--dev-seed-file` is for local development fixtures. Bittensor
wallets are the normal path. Validator identity currently has a separate
consensus-seed requirement; see the [validator guide](validators.md).

## Bounty

Bounty runs as the [CortexLM/bounty](https://github.com/CortexLM/bounty) container;
its routes are under `/challenge/bounty/`.

| Symptom | Meaning and next action |
|---------|-------------------------|
| `403 terms_required` | Read the terms and explicitly pass `--accept-terms` when pairing |
| `403 pairing not authorized by account operator` | Ask the operator to verify the Cortex Chat account and issue a fresh grant for this exact account and hotkey |
| `401 signature verification failed` | Sign `cortex-bounty-v1\|{account_id}\|{nonce}\|{exp}` in the Substrate sr25519 context |
| `400 invalid or expired pairing window` | Correct the local clock and pair with a future expiry |
| `409 nonce reused` | The nonce was already accepted, even for an identical request; a lost session requires a new operator grant and nonce, which revoke the old session on successful pairing |
| `401 invalid_session` | Check the private session file and deployment; pairing the same account again revokes its previous session |
| `403 hotkey_mismatch` | The selected wallet hotkey differs from the session's paired hotkey |
| CLI cannot write the session file | Select a new writable file; the CLI never overwrites an existing session file |
| `400` thin or repetitive report | Supply a distinct title, at least 80 body characters and 20 reproduction characters with concrete evidence |
| `429` | Wait for the 60-second interval or for pending adjudications to fall below five |
| `422` | Match the API JSON field names and types; unknown fields are refused |
| `can_score: false` | Status could not read and validate the backend feed; inspect `reason` and wait for operator recovery |
| `503` on reports after `can_score: true` | Submit rechecks the feed; availability or publication consistency changed after the successful status probe |
| Locally adjudicated report has no paid score | Scoring reads CortexLM/backend; local adjudications are not automatically exported there |
| Public report GET is denied | Reports require operator access; the gateway does not expose private report reads |

A Bounty feed outage stores no new report and emits no positive bounty score.
Any challenge-container outage covers participants with
`NoScore(ChallengeInternal)` so the share can burn through a valid seal. Do not enable a substitute local scorer.

## Proof intake

| Symptom | Meaning and next action |
|---------|-------------------------|
| `400 topic missing, unknown or not open` | Rediscover `/challenge/proof/v1/proof/topics` and check status and epoch window |
| `401` missing/invalid signature, hotkey or nonce | Use the exact signed fields; hotkey and nonce are 64 lowercase hex, signature is 128 lowercase hex |
| `401 submit_nonce reused` | The hotkey/nonce pair was already reserved; reconcile the earlier attempt before paying for a new one |
| `400` undeclared/missing/invalid `env` | Follow the signed topic's `params.miner_byok` and `miner_env_allowlist`; send a body map using private `--env-file` |
| `400 artifact required` | Upload the `artifact` tar part or supply a valid HTTPS artifact URI |
| `400` size, digest, tar or member error | Send the exact nonempty uncompressed tar, no links/traversal/special entries, within the 5 MiB upload limit |
| `413` | The full HTTP request exceeds the bounded intake limit |
| `429` | Too many pending jobs for this miner; default limit is four |
| `201` with `rejected` and contamination reason | Holdout overlap or required training evidence failed before paid evaluation; do not invent training ids |

`env` validation precedes cryptographic signature checking and nonce
reservation. A malformed environment is not silently dropped. The artifact
digest covers exact bytes; uploading a differently packed tar changes the
digest even when its extracted files look identical.

## Proof execution

| Symptom | Meaning and next action |
|---------|-------------------------|
| `503 UnwiredVmOrchestrator` | No live VM execution path is configured; operator action is required |
| `503` image pin or inference offer mismatch | The signed topic and live host configuration disagree; do not invent or substitute a digest |
| `503 custom runner unavailable` | This signed custom runner is not registered on the live host |
| `503 harvest executor unavailable` | Python Lium harvest execution is unavailable; a custom runner does not make `nll` or `throughput` ready |
| `503` baseline evidence error | A baseline needs verified execution evidence and a sealed comparison; a model claim alone cannot open scoring |
| `503` credential vault or stored artifact error | Required private inputs are unavailable; the operator must restore or reconcile the accepted job |
| `503` teardown or evaluation infrastructure failure | No score may be fabricated; failed work may still have a durable job and consumed nonce |
| `201` with `queued` | The topic explicitly sets `params.defer_scoring: "true"`; the response is not a completed score |
| `201` with `accepted` but no payment | Topic payout, signed leaves, sealing and chain submission are additional stages |
| POST times out or client disconnects | Accepted work continues; check its returned id or reconcile with the operator before resubmitting |

Normal Proof POST responses are **201-after-score**. The CLI waits 7200 seconds
by default; `--submit-timeout-secs` or `CTX_PROOF_SUBMIT_TIMEOUT_SECS` overrides
that wait and `0` disables the read timeout. GET requests keep their separate
60-second timeout. A `404` from a result lookup can mean no result row exists
yet; it is not proof that a job or nonce was never accepted.

Status family flags describe wiring. They do not establish successful paid
execution, scientific validity or end-to-end payment. There is no fixed topic
catalog; follow the documentation and checklist in the current signed topic.

## Validators

| Symptom | Meaning and next action |
|---------|-------------------------|
| Latest is `sealed: false` with UID 0 weight 1.0 | Fail-closed fallback; do not submit it or a persisted last-known-good seal |
| Verified sealed burn has UID 0 weight 1.0 | Submit this real seal; `burn_outcome: true` is not itself a refusal reason |
| Sealed vector pays only a nonzero owner or permitted validator | Refuse; this is not the burn UID |
| Consensus seed mismatch | The required consensus seed must identify the same hotkey as the Bittensor wallet |
| Missing/invalid peer sample | Configure independently selected HTTPS peers with metagraph `validator_permit`; do not disable verification |
| Conflicting signed roots or equivocation | Preserve the journal and signed evidence; reconcile the network state |
| Stale block, future epoch, reorg or changed latest | Wait for a current valid seal; do not force-submit the old one |
| Ambiguous chain dispatch remains pending | Reconcile inclusion/finalization before retrying; retain the SQLite journal |
| CRv4 or drand failure | Do not fall back to public weights while chain commit-reveal is enabled |
| `/v1/attest/*` returns `503` and `verified: false` | DCAP verification is not implemented in the Python validator |

Validators fetch and verify sealed results; evaluation runs on master and the
dedicated VM host. See [the validator guide](validators.md) for peer consensus,
quarantine and Class A handling.
