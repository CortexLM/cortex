<!-- protocol_version: 1 -->

# Proof miner guide

Proof (`proof`, 7000 bps under algorithm 2) rewards reproducible submissions
against signed, operator-published research topics. Install the [Python CLI](README.md) and use
your Bittensor hotkey. The operator supplies the gateway URL and an
independently pinned Proof public key.

## Discover a topic

```bash
curl --fail-with-body "$GATEWAY/challenge/proof/v1/status"
curl --fail-with-body "$GATEWAY/challenge/proof/v1/proof/topics"
curl --fail-with-body "$GATEWAY/challenge/proof/v1/proof/topics/$TOPIC_ID"
```

The list response is `{"topics": [...]}` and may include draft or closed
topics. Choose a topic with `status: "open"` whose epoch window is active. The
CLI verifies the selected document's signature and identity before submission.
An empty list is not an invitation to use a historical topic id.

Python topics use `schema_version: 2`. Read the document's `statement`,
`metric`, `params`, `checklist`, `baseline`, `documentation`, `endpoints` and
epoch window. Its `eval_image_digest` and `inference_offer_commitment` bind the
execution image and RLM offer. Miners do not choose either offer.

The operator gives the RLM an objective. Topic setup runs in an isolated topic
VM, prepares the experiment and proposes rules, documentation and endpoints.
The control plane publishes the signed topic only after validating the
baseline execution evidence. A topic id, runner id or benchmark name in an old
guide does not establish a currently open topic.

Topic-specific paths live below `/v1/proof/topics/{topic_id}`. An endpoint with
`path: "/instructions"` therefore resolves below that topic, with the public
`/challenge/proof` prefix. Declared purposes are documentation, submission and
results. Submission endpoints pass through the same signature, artifact and
quota checks as `/v1/submissions`; they do not execute arbitrary HTTP services
on the control-plane host.

## Submit an artifact

Prepare a nonempty, uncompressed tar containing the code and inputs requested
by the topic. The uploaded tar and its expanded regular-file content must fit
within 5 MiB. Links, special files, traversal paths, duplicate members and
compressed archives are refused. `artifact_digest` is SHA-256 of the exact tar
bytes, not a directory hash or a newly packed substitute.

```bash
tar -cf artifact.tar research
uv run cortex miner --gateway "$GATEWAY" \
  --wallet-name research --wallet-hotkey miner \
  --proof-public "$PROOF_PUBLIC_KEY" \
  proof-submit --topic "$TOPIC_ID" \
  --artifact artifact.tar --claim-file claim.txt \
  --receipt submission.receipt.json
```

For an encrypted hotkey add `--wallet-password-file /private/hotkey-password`
before `proof-submit`. The command hashes the artifact, generates a fresh
nonce and signs locally. Before the first POST it exclusively creates the
`--receipt` file with mode `0600`, flushes it to disk, then sends a multipart
request with an `artifact` part and a `json` metadata part to
`/challenge/proof/v1/submissions`. Refusing an existing receipt path prevents
an accidental overwrite or resubmission.

Optional action arguments:

| Argument | Meaning |
|----------|---------|
| `--manifest PATH` | JSON containing `train_content_hashes` and/or `train_dataset_ids` |
| `--env-file PATH` | Private JSON string-to-string map of topic-declared miner credentials |
| `--receipt PATH` | New private recovery receipt; required and never overwritten |
| `--declared-flops N` | Signed unsigned integer; default `0`, unused as a rejection gate for custom topics |
| `--submit-timeout-secs N` | POST response wait; default `7200`, `0` waits without a read timeout |

`CTX_PROOF_SUBMIT_TIMEOUT_SECS` sets the default POST wait. Topic GET and
receipt lookup requests use a 60-second timeout. The CLI has no `proof sign`,
`proof status` or `proof topics` subcommand; use the HTTP reads above.

The API also accepts JSON with an HTTPS `artifact_uri` without embedded
credentials or a fragment. Malformed authorities, invalid ports and any userinfo
are rejected with `400` before signature verification, nonce reservation or
execution. Correcting only this unsigned URI lets you reuse the original signed
envelope and unspent nonce. The fetched file must match `artifact_digest`
verbatim. Upload bytes take precedence when both paths are supplied; the ignored
URI is discarded before storing any accepted or rejected result. The CLI uses
uploads and does not expose a URI-only option.

## Submission signature

The JSON fields are `topic_id`, `miner_hotkey` (64 lowercase hex),
`artifact_digest` (64 lowercase hex), `claim`, `declared_flops`, `manifest`,
`submit_nonce` (64 lowercase hex), `hotkey_signature` (128 lowercase hex),
optional `artifact_uri`, and optional `env`.

Proof signatures use sr25519 with the frozen `base-sr25519-v1` signing context
and domain `base-proof-submit-v1`. A generic `wallet.sign()` uses the Substrate
context and is not interchangeable. The Python miner's wallet signer preserves
the Proof context.

The signed payload joins these UTF-8 byte strings with the single byte `0xff`,
in order: miner hotkey hex, topic id, artifact digest hex, decimal declared
FLOPs, claim, canonical manifest, nonce hex. Each manifest list is encoded as
its decimal length followed by its sorted strings, joined with `0xff`; the
hash list and dataset-id list are joined with the same separator. The signing
message is SCALE `Vec<u8>(domain) || Vec<u8>(payload)`. Use
`Submission.signing_payload()` and `HotkeySigner.sign()` from the Python
package when implementing a client; do not invent a JSON serialization.

Topic signatures use domain `base-proof-topic-v1` in the same signing context,
over Python topic-v2 canonical JSON excluding `signature`. This is a versioned
document format, not a promise that legacy topic documents remain compatible.

The `(miner_hotkey, submit_nonce)` pair is single-use and reserved before
enqueueing or evaluation. Replay returns `401 submit_nonce reused`.
`X-Lium-Api-Key` is not identity and cannot replace the hotkey signature.
`env` and `artifact_uri` are outside the signed payload; the artifact digest
still binds the bytes, and transport must be authenticated HTTPS.

## BYOK and evidence

Only variables declared by the signed topic are accepted. In Python topic v2,
`params.miner_byok` names one required variable and
`params.miner_env_allowlist` is a comma-separated optional allowlist. The
required name is also allowed and needs no duplicate allowlist entry. Both
submission intake and the VM host validate it before research begins; preflight
receives no miner credentials. `env` is a JSON object, not an HTTP header.

Use `--env-file` with a private file containing the names actually declared by
your topic. Undeclared, malformed or missing required names return `400`
before signature verification, nonce reservation or paid execution. There
may be at most eight variables; each value must be nonempty printable text of
at most 4096 characters. Reserved process variables and `PROOF_*` names are
refused.

Credentials are held in private files: `0700` directories and `0600` files.
Values do not enter submission rows, status responses or public results. A
vault failure is `503`; a deferred evaluation cannot substitute an owner key
for a missing miner credential. Credentials are passed only to the paid guest
job and included in output redaction.

An interrupted internal VM response can be recovered only from a completed,
matching execution record with confirmed VM destruction. Recovery preserves the
job's original budgets and does not rerun an unknown experiment. It does not
change the miner's nonce or signed submission receipt.

Training evidence is required by default for harvest families. A custom topic
requires it only when `params.require_training_evidence` is `"true"`. Missing
required evidence or overlap with the private holdout persists a `rejected`
result before paid evaluation. Public topic documents contain the holdout
commitment, never holdout records. Do not invent dataset identifiers to pass a
training-evidence check.

## Results and retry behavior

For ordinary topics, successful POST processing returns `201` **after scoring**,
with `id`, `topic_id`, `miner_hotkey`, `epoch`, `status`, `artifact_digest`,
`reason`, `metrics` and `evidence_digest`. `status` is `accepted` or `rejected`;
a `201` alone does not mean a winning submission. A signed topic with
`params.defer_scoring: "true"` instead returns `201` with `status: "queued"`.

If the POST times out, the process exits, or its response is lost, recover the
same accepted attempt with the original wallet and receipt:

```bash
uv run cortex miner --gateway "$GATEWAY" \
  --wallet-name research --wallet-hotkey miner \
  proof-lookup --receipt submission.receipt.json
```

The command sends `POST /challenge/proof/v1/submissions/lookup` with the
original envelope signed under `base-proof-submit-v1`. The receipt contains
the claim, manifest, nonce, hotkey, artifact digest and original signature. It
contains neither `env` values nor `artifact_uri`; it must remain private
because possession permits reading that submission's state. The server only
answers when the signature is valid and the exact signed-payload digest matches
the durable `(miner_hotkey, submit_nonce)` binding. A different valid payload,
nonce or miner receives the same `404` as an unknown submission, and lookup
does not reserve or consume a nonce.

Accepted request bodies survive client disconnects through a durable job
journal. Lookup returns `pending` for queued or running work and returns the
durable `accepted`, `rejected`, or `failed` terminal outcome after restart. An
infrastructure failure can occur after the nonce has been consumed and work
has been queued. A timeout or `503` does not prove that nothing ran: use the
receipt instead of starting a second paid attempt.

| Outcome | Meaning |
|---------|---------|
| `400` | Invalid topic or body, missing/unsafe artifact, or invalid BYOK; no accepted submission |
| `401` | Missing/invalid identity signature or nonce, or nonce replay |
| `429` | Per-miner pending-job quota reached; the current default is four |
| `503` | Scoring prerequisites or infrastructure unavailable; never a fabricated score |
| `201`, `rejected` | A recorded rejection, including contamination or failed scientific checks |
| `201`, `accepted` | Evaluation passed; payout still depends on topic competition and sealed weights |

## Scoring availability and payout

An open topic needs a sealed, reproducible baseline, registered runner, pinned
image and open, authenticated inference offer. Unwired VMs, unavailable
credentials, mismatched pins or unconfirmed experiment teardown fail closed.
Missing, malformed or incompatible VM resource ceilings also return `503` before
submission acceptance; the service does not shrink a requested VM to fit.
There is no control-plane-host evaluation fallback. Paid experiments run in
isolated Firecracker guests; a successful result is usable only after their
teardown is confirmed.

`GET /v1/status` reports `can_score`, a bounded `reason`, the number of
`open_topics`, `scorable_topics` split into custom and harvest families,
`live_harvest_wired`, `custom_family_wired`, `registered_custom`,
`custom_ready`, and the public image, inference-offer and executor commitments
under `pins`. `can_score` is true only when at least one currently open topic
passes the same baseline, holdout, runner, image, inference and executor-offer
checks as intake. The other fields remain wiring diagnostics, not proof of
payment. `GET /v1/proof/executor` returns `200` with `ready` and `reason`,
including when the harvest executor is unavailable. An empty open-topic set
cannot produce a paid Proof score. Never invent an image pin to bypass a
refusal.

The `nll` and `throughput` Lium families remain unavailable in the master:
there is no configured compatible GPU-isolated guest or published evaluator
image. The injected adapter requires the complete signed topic, verified private
setup pack/holdout material and exact verified artifact bytes before rent. A
URI alone is insufficient for that boundary. Operators must also provision
trusted SSH host keys; miners cannot replace this trust with a provider API key.
Returned results must bind the request and signed topic's image, environment,
holdout and inference commitments, with confirmed teardown of the dedicated
experiment VM. Provider pod deletion by itself does not establish VM isolation
or a paid Proof result. These requirements do not change the submission signature
or expose private holdout records in topic discovery.

The Lium boundary revalidates the request and private material after rent, before
execution, and after confirmed teardown. Raw guest output and provider error
text may contain private holdout data or credentials, so public failure reasons
and stored report rationales use fixed diagnostics instead of output excerpts.

Each open topic carries its own `wta` or `discovery` policy. `wta` assigns its
mass to the best eligible result, sharing exact ties. `discovery` divides a
pass-floor pool among eligible miners and a novelty pool according to new
improvement; duplicates receive no novelty pool. A miner's Proof score is the
**sum of per-topic masses**, not a mean of binary passes. The fixed subnet
split is Bounty 3000 / Proof 7000 bps under algorithm 2; algorithm 3 takes any
owner-signed split. Legacy owner-signed 2000/8000 deployments retain algorithm 1
until
[activation](../how-to/trust-root.md#activate-proportional-bounty).

Actual payment additionally needs signed leaves, a valid gateway seal and
validator submission on Bittensor. See [validators](validators.md) and
[troubleshooting](troubleshoot.md).
