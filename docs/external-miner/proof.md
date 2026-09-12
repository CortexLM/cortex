<!-- protocol_version: 1 -->

# Proof — miners

Challenge id is `proof`. The two configured challenges are `bounty`
(**2000 bps**) and `proof` (**8000 bps**), a 20/80 allocation.

**Implementation warning:** Proof's Python judge is partial, submission records
are in memory, and the service does not yet drive automatic reward-leaf emission.
Do not spend compute on the assumption that `can_score` proves the complete
research-to-payment path. Read the
[paper-to-code comparison](../WHITEPAPER.md#proposal-versus-current-code) and
confirm deployment support with the operator first.

**Gateway:** [https://gateway.cortex.foundation](https://gateway.cortex.foundation)  
**CLI:** `ctx proof topics`, then `ctx proof submit` (install:
[README](./README.md))  
**Per-topic guides:** [`tbench`](./proof-tbench.md)  
**Pin:** [`config/proof-pin.toml`](../../config/proof-pin.toml)  
**Eval image:** `ghcr.io/cortexlm/proof-eval@sha256:78b614a1f51ce5dd80076c4e343a2b31b85d6c36025e02836cb83929867e7009`

You submit **claim + code + artifact** against a topic, signed with
your miner hotkey. `declared_flops` is optional (signature compat). You do
not bind an offer id. The digest-pinned
`proof-eval` image (harvest boots it) calls the master's `InferenceOffer` as
the RLM **judge** backend. No baked Qwen; architecture is not an HF id
check.

Miner pays Lium (`LIUM_API_KEY` / `X-Lium-Api-Key`). That key is **not**
identity: every submit must carry `hotkey_signature` over
`base-proof-submit-v1` and a single-use `submit_nonce`.

If `eval_image_digest` is empty the host answers **503**. That is fail-closed,
not a sim fallback. The pin currently carries the digest above. Do not invent
a different one.

## How Proof works

The **unit of work is a signed topic**, not a frozen catalog in git. An
operator publishes a research problem (admin `POST` can inject one at any
time). You submit **against that `topic_id`**:

1. a **claim** (natural language: what improved, under which constraints)
2. a **code artifact** (reproducible recipe — code + lockfile / entrypoint)
3. **`declared_flops`** (optional; still bound into the signature). Custom /
   agent topics (`tbench`) **ignore** it as a scoring gate. Harvest
   `nll` / `throughput` still refuse a declaration over `topic.flops_budget`

The artifact contract requires a recipe the judge can re-run. Custom / agent
topics (`tbench`) do not apply a hardcoded FLOP budget. Harvest
`nll` / `throughput` still score under the topic's FLOP / wall budget. **A weight dump alone is not an artifact.** The intended judge
re-runs that recipe and a separate harness measures holdout loss or throughput.
The current Python image does not yet implement arbitrary recipe reproduction;
its static checks and model measurements are only part of that design.
Holdout records are not included in public topic responses.

`GET /challenge/proof/v1/status` shows `can_score`, `eval_backend`,
`force_sim`, `live_harvest_wired`, `custom_family_wired`, `baseline_sealed`,
public pin `inference`
judge defaults (provider, model, mode, token caps — never the origin), the
public RLM judge `inference_offer` (id, kind, mode, model_ref, token caps,
commitment, status), and the public `eval_executor` — the `1x` Lium machine
class your recipe is re-run on (template, shape, proof deadline, commitment,
status). It never leaks holdout records, teacher hosts, origins, or keys.

Muon, token superposition, and “decentralized training without InfiniBand”
are *examples* of solutions or of topics — they are not the product.

Pass gates (reproduced, no contamination, signed-topic checklist, beat epsilon)
are fail-closed. Custom / agent topics do not apply a hardcoded FLOP budget. The implemented **payout calculation** after a pass depends on the
topic's `payout_mode`. The score is the **sum of per-topic** masses over
currently `open` ids, not a mean of binary lattices. A skipped topic is 0 on
that topic. Zero open topics → the host cannot score (`503`), not a paid 0.

## 0. Can this host score right now?

```bash
ctx proof status
# same as:
curl -sS https://gateway.cortex.foundation/challenge/proof/v1/status
```

`GET /challenge/proof/v1/status` shows `can_score`, `eval_backend`,
`force_sim`, `live_harvest_wired` (Lium harvest, `nll` / `throughput`),
`custom_family_wired` + `registered_custom` + `custom_ready` (the `custom`
family, reported apart), `baseline_sealed`, `eval_image_digest`,
public pin `inference` (no origin), public RLM judge `inference_offer`,
public `eval_executor` (plus the pin `executor` ceilings), and
`open_topics`. It never leaks holdout records, teacher hosts, origins, or
keys. `GET /challenge/proof/v1/proof/executor` shows the executor alone with
`ready` and a `reason` when it cannot rent.

`can_score: false` means submits **503**. Nothing is stored and nothing is
rented. The one exception is a topic listed in `deferred_topics`: it accepts
your submission and stores it as **`queued`** (see § 3) while the operator
finishes installing its scoring path — nothing is evaluated or rented yet.

| Status field | What it means |
|--------------|----------------|
| `eval_image_digest` | Must be a `sha256:…` pin (live pin is `sha256:78b614a1…`). Empty → **503** |
| `inference_offer` | Public RLM **judge** backend (id, kind, mode, model_ref, token caps, commitment, status). Missing/closed/misconfigured → **503**. You do not pass an offer id |
| `eval_executor` | Public `1x` **executor**: the Lium machine class your recipe is re-run on (`lium_template_id`, `machine_shape`, `max_proof_deadline_s`, commitment, status). Your recipe must finish inside `max_proof_deadline_s` (≤ pin ceiling 7200 s; a topic may name a shorter one) on **one** GPU — the host never rents more. Missing/closed/any shape but `1x` → **503**. You do not pass or rent it |
| `open_topics` empty | No currently `open` signed topic with a sealed baseline → **503** |
| `scorable_topics` | Open topics whose family's scorer is wired on this host and that are scored right now. An open topic **not** listed here (a `custom` topic whose runner is not registered or not wired; an `nll` / `throughput` topic on a host whose Lium harvest is not wired) answers **503** — unless it is in `deferred_topics` |
| `deferred_topics` | Open topics whose signed document sets `constraints.params.defer_scoring = "true"`: the operator is still installing the baseline / harness. Submits are accepted (**201**) and stored as **`queued`**; nothing is evaluated or rented until the operator lifts the flag and drains the queue, oldest first |
| `queued_submissions` | Rows waiting in that queue across topics |
| `registered_custom` | Custom metric ids with a registered runner. Nothing is compiled in; ids come from signed topics |
| `custom_ready` | The subset of `registered_custom` whose runner can run right now (topic-VM orchestrator reachable by config, image pinned). A registered id missing here → its topics answer **503** |
| `baseline_sealed: false` | An open topic without `script_sha256` + `metrics_commitment` → **503** |
| `live_harvest_wired: false` | The Lium harvest — the `nll` / `throughput` scorer — is not connected → **503** on those families. **Lium only**: it says nothing about `custom` topics |
| `custom_family_wired: false` | No custom-family runner is registered on this host → **503** on `custom` topics. Independent of `live_harvest_wired`; both `false` → **503** for everything |

## 1. List open topics

```bash
ctx proof topics
curl -sS https://gateway.cortex.foundation/challenge/proof/v1/proof/topics
curl -sS https://gateway.cortex.foundation/challenge/proof/v1/proof/topics/dt-no-ib-v0
```

Topics are **operator-published** and can be **injected at any time** (operator
`POST /challenge/proof/v1/admin/proof/topics`). There is no catalog in git.
`ctx proof topics` is the live list.

Topics that have a written miner guide in this repo:

| Topic | Guide | Family / payout |
|-------|-------|-----------------|
| `tbench` | [proof-tbench.md](./proof-tbench.md) | `custom` (`success_rate`) / `discovery` |

For a minimal custom-Python `Agent` constructor/`run` on `tbench` (Harbor
`environment.exec`, both pack layouts), see
[proof-tbench.md § Minimal Agent example](./proof-tbench.md#minimal-agent-example).

A guide is a convenience, not the contract: the signed document returned by
`ctx proof topics` wins wherever the two disagree, and a topic without a guide
is submitted to exactly like any other.

Each topic is a signed document. Read at least:

| Field | What it means for you |
|-------|------------------------|
| `id` | The `topic_id` you submit against |
| `statement` | The research problem in English |
| `constraints` | Fabric / comms caps the eval image enforces (it never trusts the claim). Custom topics may add `firecracker_required`, `model_pin`, an opaque `task_slice`, and `params`. `params.defer_scoring: "true"` means the topic accepts submits but scores them later (`queued`) |
| `checklist` | Anti-cheat rules `[{id, text}]` the topic's RLM ticks over your artefact **before any paid inference** |
| `eval_executor` | Executor commitment (`require_offer_commitment`, tighten-only `max_proof_deadline_s`) |
| `metric.family` | `nll` \| `throughput` \| `custom` (`metric.custom_id` names the metric; it is topic data) |
| `flops_budget` | Harvest (`nll` / `throughput`) hard cap: `declared_flops` must be `≤` this. Custom / agent topics ignore FLOP accounting as a reject gate |
| `epsilon_nll` / `epsilon_topic_max_regress` / throughput knobs | Pass-rule epsilons. A topic may **tighten** a pin floor, never loosen it |
| `payout_mode` | `wta` or `discovery` |
| `validation` | English pass contract `{score_on, accept_if, reject_if}` |
| `baseline` | Sealed recipe you have to beat (`script_sha256` + `metrics_commitment`) |
| `status` | Only `open` accepts submits and pays |

### `payout_mode`

| Mode | How that topic pays |
|------|---------------------|
| **`wta`** | Winner-take-all. Best primary metric among `pass=true` this epoch takes 100% of the topic's emission mass. Exact ties split equally. Everyone else on that topic gets 0 |
| **`discovery`** | Pass floor (default **≈30%** of the topic pool) split equally among verified passes — this reimburses compute. The rest (**≈70%**) is a novelty pool weighted by how much you improved on the sealed baseline (and the current champion, if any). A near-duplicate of an accepted artifact keeps the floor and gets 0 novelty |

### `validation` (English)

```json
{
  "score_on": "what the harness measures (holdout_nll, tokens_per_sec, …)",
  "accept_if": "English: when this run is a pass",
  "reject_if": "English: when this run is a reject, including cheat codes"
}
```

Read `statement` + `validation` before you train. English does not override
contamination gates or harvest-family FLOP / wall gates. Custom / agent
topics use the signed checklist, not a hardcoded FLOP budget.

### Pin floors (a topic may tighten only)

From [`config/proof-pin.toml`](../../config/proof-pin.toml):

| Floor | Pin value | Topic rule |
|-------|----------|------------|
| `flops_budget_max` | `2e18` | Topic budget must be `1..=` this |
| `epsilon_nll_min` | `0.02` | NLL win must be at least this large |
| `epsilon_topic_max_regress_min` | `0.05` | Per-split NLL regress cap cannot be looser |
| `epsilon_throughput_rel_min` | `0.05` | Throughput relative win cannot be looser |
| `quality_floor_nll_max` | `0.02` | Throughput may not trade more NLL than this |

The holdout is 120 records, 24 per scored split (`web_ood`, `code_ood`,
`math_ood`, `longctx`, `multilingual_ood`). The canary is **off the number you
are paid on**. You never see the records.

## 2. Submit a reproducible experiment

Build a recipe the judge can re-run: code, lockfile, and entrypoint. Harvest
`nll` / `throughput` topics still score under the topic's FLOP (and for
throughput, wall) budget; custom / agent topics do not. Hash that tree. That hash
is `artifact_digest`. Upload the uncompressed tar (≤5 MiB) as multipart
part `artifact` — preferred on custom / `tbench`. That upload is the
evaluate path: the gateway stages the bytes and the KVM host injects them
into the guest over vsock (no miner HTTPS). `artifact_uri` is an
optional compat locator (git URL, object URL) for the same bytes: optional
on `nll` / `throughput`, and optional on custom topics when you upload.
A custom topic with neither upload nor URI is a **400** `artifact required`.
When both are sent, the uploaded bytes win (the URI is ignored for identity).
A topic in `deferred_topics` still accepts the upload as **201** `queued`.
Gzip, a non-tar body, or a tar with no file content is **400** with no row.
Missing vault / digest mismatch / empty / oversize at evaluate is **503**
with the row untouched — never invented bytes.

The **claim** is one English sentence of what improved. The RLM re-runs the
code against the public split and checks the claim against those public
numbers. A claim the code cannot support is `unreproduced_claim` / reject.

```bash
ctx proof status          # can_score, inference_offer, eval_executor, eval_image_digest
ctx proof topics          # pick an open topic_id; read flops_budget + payout_mode

ctx proof submit \
  --secret-file /path/to/hotkey.sk \
  --topic-id <open topic id> \
  --artifact recipe.tar \
  --claim "beat sealed baseline holdout NLL by 0.04 at 1.2e18 FLOPs" \
  --declared-flops 1500000000000000000 \
  --train-dataset my-mix-v0
```

`--secret-file` is a 32-byte mini-secret (or 64 hex chars), never a mnemonic.
`--wallet-name` / `--wallet-dir` / `--wallet-hotkey` load a Bittensor
wallet the same way `ctx bounty pair` does. `--signature` is a 128-hex
offline signature. Pass **exactly one** signer — `--secret-file`,
`--wallet-name`, or `--signature`; `ctx` refuses combinations rather than
picking one. `--hotkey` is optional when a secret or wallet is loaded
(derived); with `--signature` it is required and must match, and
`--submit-nonce` must be the nonce that was signed.

The signature is sr25519 under `base-proof-submit-v1` over these exact
bytes (`0xff` is a single separator byte; it never occurs inside UTF-8):

```
hotkey_hex || 0xff || topic_id || 0xff || artifact_digest || 0xff
  || declared_flops_decimal || 0xff || claim || 0xff
  || manifest_canonical || 0xff || submit_nonce_hex

manifest_canonical =
  count(hashes)_decimal   || (0xff || hash)*      hashes   = manifest.train_content_hashes, sorted bytewise
  || 0xff ||
  count(datasets)_decimal || (0xff || dataset)*   datasets = manifest.train_dataset_ids,   sorted bytewise
```

- `hotkey_hex` / `artifact_digest`: exactly 64 lowercase hex, no `0x` —
  the only spelling the host accepts, so the bytes you sign are the bytes
  it verifies (any other spelling is a **400**, never a silent rewrite).
- `declared_flops_decimal`: the integer as ASCII digits.
- `topic_id` / `claim`: the exact UTF-8 strings you post (the host trims
  `topic_id` only to look the topic up; it verifies what you sent).
- `manifest_canonical`: each list's entry count as ASCII digits, then every
  entry prefixed by `0xff`, entries as exact UTF-8 (duplicates kept, nothing
  trimmed), sorted bytewise; the two lists joined by `0xff`; a missing list
  is `0`. `{"train_dataset_ids":["my-mix-v0"]}` is `0 ff 1 ff my-mix-v0`.
  Order on the wire does not matter; the **contents** do — a signature over
  one manifest never authorises another.
- `submit_nonce_hex`: 32 random bytes you choose, as 64 lowercase hex, sent
  verbatim as `submit_nonce`. The host accepts each `(miner_hotkey,
  submit_nonce)` pair **once**; a second request with the same pair is
  **401** `submit_nonce reused`, whatever happened to the first. Draw a
  fresh nonce for every request (`ctx` does).

`ctx` signs with schnorrkel: signing context `base-sr25519-v1`, message
`scale(b"base-proof-submit-v1") || scale(payload)` where `scale(x)` is the
SCALE byte-vector encoding (compact length prefix + bytes). Reference:

```python
def canonical(xs):
    xs = sorted(xs)
    return str(len(xs)).encode() + b"".join(b"\xff" + x.encode() for x in xs)

payload = (hotkey_hex.encode() + b"\xff" + topic_id.encode() + b"\xff"
           + artifact_digest.encode() + b"\xff" + str(declared_flops).encode()
           + b"\xff" + claim.encode() + b"\xff"
           + canonical(train_content_hashes) + b"\xff" + canonical(train_dataset_ids)
           + b"\xff" + submit_nonce.encode())
```

`ctx proof sign` prints `miner_hotkey`, `hotkey_signature`, `submit_nonce`,
and the exact `manifest` to post, without posting. Pass the same
`--train-dataset` / `--train-hash` / `--manifest-file` you will submit when
the live topic requires training evidence; omit them on custom / agent
topics (`tbench`).

When you declare nothing, `ctx` reads the topic from the gateway to find out
which of those two cases you are in, so that call needs a reachable
`--gateway` and an open `topic_id` — an unknown topic or an unreachable
gateway is an error there, never a guess. Signing an empty manifest for a
topic that *does* require evidence would spend your single-use `submit_nonce`
on a submission the host then rejects. Declare `--train-dataset` /
`--train-hash` and no lookup happens: the declaration stands on its own, so
that signature can be produced offline.

`--wait` keeps polling until the row is terminal (`awaiting_admin`,
`rejected`, or `champion`). A `queued` row is not terminal: on a topic that
defers scoring, `--wait` keeps polling until the operator drains the queue,
which can take as long as the operator's install does.

Live (non-deferred) evaluate is **synchronous** and can run for minutes
(`tbench`). `ctx proof submit` waits up to **7200 s** for that POST
(`CTX_PROOF_SUBMIT_TIMEOUT_SECS` / `--submit-timeout-secs`; `0` waits until
the host answers). GET routes stay on ~60 s. A client that hangs up after
the body is accepted does not cancel scoring: the row still lands, and a
client that holds still gets **201-after-score**.

The same submit with `curl` (`miner_hotkey`, `hotkey_signature`,
`submit_nonce`, and `manifest` from one `ctx proof sign --json` run — the
manifest is signed, so post the one you signed):

```bash
curl -sS -X POST https://gateway.cortex.foundation/challenge/proof/v1/submissions \
  -H 'content-type: application/json' \
  -H "X-Lium-Api-Key: $LIUM_API_KEY" \
  -d '{
    "miner_hotkey": "<64-hex hotkey>",
    "hotkey_signature": "<128-hex sr25519>",
    "submit_nonce": "<64-hex, fresh per request>",
    "topic_id": "<open topic id>",
    "artifact_digest": "<sha256 of the recipe>",
    "claim": "beat sealed baseline holdout NLL by 0.04 at 1.2e18 FLOPs",
    "declared_flops": 1500000000000000000,
    "manifest": {
      "train_content_hashes": [],
      "train_dataset_ids": ["my-mix-v0"]
    }
  }'
```

`claim` is **required**. `declared_flops` is optional (default `0`, still
bound into the signature). Custom / agent topics ignore it as a scoring
gate; harvest `nll` / `throughput` still refuse a declaration over the topic
budget.

Poll `GET /challenge/proof/v1/submissions/{id}`. While `can_score` is
`false` (empty digest, missing/closed RLM judge backend, incomplete pin+topic
judge config, no open sealed topic), submissions answer **503** — except on
a topic in `deferred_topics`, where they answer **201** `queued`.

### Required POST JSON

`POST https://gateway.cortex.foundation/challenge/proof/v1/submissions`

Preferred: **multipart** with the same fields as form parts plus part
`artifact` = the uncompressed tar (≤5 MiB). `ctx proof submit --artifact`
does this. JSON + `artifact_uri` remains a compat path.

| Field | Required | Shape |
|-------|----------|-------|
| `miner_hotkey` | yes | **Exactly** 64 lowercase hex (no `0x`); the sr25519 public key that verifies `hotkey_signature` |
| `hotkey_signature` | yes | **Exactly** 128 lowercase hex (no `0x`, no uppercase) sr25519 over `base-proof-submit-v1` (payload above). Missing/invalid → **401**. `X-Lium-Api-Key` is not a substitute |
| `submit_nonce` | yes | **Exactly** 64 lowercase hex (32 random bytes), bound into the signature, accepted once per hotkey. Missing/invalid/reused → **401** |
| `topic_id` | yes | Open topic id from `ctx proof topics` |
| `artifact_digest` | yes | SHA-256 of the recipe bytes as **exactly** 64 lowercase hex (no `0x`) |
| `claim` | yes | Non-empty string: NL of what improved (bound into the signature) |
| `declared_flops` | no | `u64`, default `0`, bound into the signature. Ignored as a scoring gate on custom / agent topics. Harvest `nll` / `throughput`: must be `≤ topic.flops_budget` |
| `manifest.train_content_hashes` | when the topic requires training evidence | Shard hashes you trained on (may be `[]` if you declare dataset ids); bound into the signature. Omit both lists on custom / agent topics (`tbench`) |
| `manifest.train_dataset_ids` | when the topic requires training evidence | Corpus ids you trained on (may be `[]` if you declare hashes); bound into the signature. Do not invent a fake id |
| `artifact_uri` | custom: optional | Compat locator; omit when you upload `artifact`. Required on neither: **400** `artifact required`. Optional on `nll` / `throughput` |
| `env` | topics that ask for a key: yes | `{"<NAME>": "<value>"}` — your own API keys for the variables the signed topic declares. See [Bring your own key](#bring-your-own-key-env). **Not** signed |

An empty `manifest` (both arrays empty / omitted) is **not** a clean
contamination check on a topic that **requires training evidence** — harvest
`nll` / `throughput` by default, or any topic whose signed
`constraints.params.require_training_evidence` is `"true"`. That is
`contamination_evidence_missing`: the row is **rejected** and **no pod is
rented**. Custom / agent topics (`tbench`) have no training step: omit
`--train-dataset` / `--train-hash`, send empty arrays, and do not invent a
harness id as a fake corpus. Holdout overlap in a *declared* manifest is
always contamination, on every family. `ctx proof submit` / `ctx proof sign`
read the live topic and only insist on a declaration when that topic needs one.

### Bring your own key (`env`)

Some topics run your recipe against a paid third-party API. You pay for that
call, so you supply the key — the operator's own credentials are never used
for a miner's run, on any topic.

Which variable a topic wants is in its signed document, under
`constraints.params`:

| Param | Meaning |
|-------|---------|
| `miner_byok` | The variable you **must** send. A submission without it is **400** |
| `miner_env_allowlist` | Comma-separated variables you **may** send. Optional |
| `require_training_evidence` | `"true"`: empty manifest is `contamination_evidence_missing`. `"false"`: skip. Absent: harvest (`nll` / `throughput`) require a declaration, custom / agent (`tbench`) skip |

Read them from `ctx proof topics --json` or
`GET /challenge/proof/v1/proof/topics/<id>` before you submit. A topic that
declares neither takes no `env` at all, and sending one is **400**. `tbench`
declares `miner_byok = "OPENROUTER_API_KEY"` — see
[proof-tbench.md § 4](./proof-tbench.md#4-the-model-key-is-yours-byok).

```bash
ctx proof submit \
  --topic-id <open topic id> \
  --artifact-digest <sha256> \
  --artifact-uri https://example.org/recipe.tar \
  --claim "beat the sealed baseline" \
  --wallet-name miner --wallet-hotkey default \
  --env OPENROUTER_API_KEY
```

Pass `--openrouter-api-key` (never printed) or `--env OPENROUTER_API_KEY` to
read it from your shell. Exporting the variable alone does not attach it.
`--env NAME=value` passes any BYOK variable inline. `--env` is repeatable,
and over `curl` it is the body's `env`:

```json
{ "env": { "OPENROUTER_API_KEY": "sk-or-…" } }
```

What the host does with it:

- **Allowlist, not filter.** A name the signed topic does not declare is a
  **400** naming what the topic does accept. Nothing is silently dropped.
- **Checked before your signature.** `env` is not part of
  `base-proof-submit-v1`, so a rejected `env` does **not** spend your
  `submit_nonce`: fix the flag and re-post the same signed body.
- **One way only.** The value is kept in a private file (mode `0600`) from
  the moment it is accepted, and handed to the guest that runs *your* code —
  exported there under the name the topic declared and written to a second
  private file at `$PROOF_MINER_ENV_DIR/<NAME>`, which is what the harness
  reads at eval time. It is never written to your submission row, never in
  `GET /v1/submissions`, never in `/v1/status`, and it is blanked out of any
  run log or evidence your own run prints it into.
- **Kept only as long as the run needs it.** On a topic that defers scoring,
  the key waits for the operator's drain and is deleted as soon as your row
  is terminal. If the host loses it before the run (a restart on an operator
  who did not configure durable storage), your row stays `queued` and the
  submit path answers **503** — it never scores your work on someone else's
  credentials.
- **Never a substitute for a bad key.** If your key is rejected by the
  provider, that is your run failing — the host does not fall back to its own.

`X-Lium-Api-Key` is unchanged and unrelated: that header pays for **compute**
on the Lium executor. `env` pays for whatever the topic's own harness calls.

## 3. See status

```bash
ctx proof show <id>
# same as:
curl -sS https://gateway.cortex.foundation/challenge/proof/v1/submissions/<id>
```

| `state` | Meaning |
|---------|---------|
| `queued` | Accepted and stored, **not yet evaluated**: the topic defers scoring (`constraints.params.defer_scoring`) while the operator finishes its baseline / harness install. No eval, no rent, no judge call, no mass yet; `verdict` is `null` and `detail` says so. The only non-terminal state — the row is scored later, oldest first and one at a time per topic, when the operator lifts the flag and drains the queue, and then becomes one of the three below. Re-sending the same artefact (freshly signed, new `submit_nonce`) returns the same row (**200**) — before *and* after it is scored: one run per artefact per topic. A byte-identical replay of an earlier request is **401** `submit_nonce reused`. |
| `awaiting_admin` | Clean pass; mass recorded. Operator audit is informational. |
| `rejected` | Gates failed (contamination, unreproduced claim, NLL miss, red anti-cheat checklist, …). No rent and no paid inference on pre-eval rejects. |
| `champion` | Promoted: operator crown, or automatic on custom topics when a pass beats the current best by `epsilon_rel` with a green checklist. Proof pays on pass, not on a crown. |

Poll `GET /challenge/proof/v1/submissions/{id}` for the verdict envelope
below. While `can_score` is `false`, the POST itself answers **503** and
there is no row to show.

On a scored **custom** evaluate the same GET also carries `results`: the
topic-defined complete RLM results JSON (not just `primary_value`). The
object is **obligatory** for a pass — missing, invalid, or non-conforming
results JSON is a **503** (no pass row), never a silent drop. Harvest
`nll` / `throughput` omit `results`. A red-checklist reject has no
`results` field (or `null`). The same document is at the artefact zip root
as `results.json` (or the filename in signed
`constraints.params.results_path`). Consensus scoring still reads only
`report.json`; `results.primary_value` / `claim_holds` / identities must
match those scored facts. See [Complete results JSON](#complete-results-json).

## HTTP 400 vs 503

A **400** is your request. A **401** is a missing, invalid, or replayed
hotkey signature / `submit_nonce`. A **503** is the host. None of those
rent a pod. Refusals (**400** / **401** / **503**) do **not** persist a
submission row.

| Status | When | Stored? | Rented? |
|--------|------|---------|---------|
| **400** `topic_id is required` | Missing `topic_id` | no | no |
| **400** `unknown topic` | `topic_id` not published | no | no |
| **400** `topic is not open` | Draft / closed / outside epoch window | no | no |
| **400** `declared_flops exceeds the topic budget` | Harvest `nll` / `throughput` only: `declared_flops > topic.flops_budget`. Custom / agent topics do not 400 on this | no | no |
| **400** `artifact required` | Custom topic, no upload and no `artifact_uri` | no | no |
| **400** `artifact exceeds 5 MiB` / `artifact is empty` / `artifact_digest does not match uploaded bytes` | Upload oversize, empty, or digest mismatch | no | no |
| **400** `artifact is not a tar archive` / `artifact is gzip-compressed; upload an uncompressed tar` / `artifact carries no file content` | Upload is gzip, not a tar, or a tar with no file bytes | no | no |
| **400** `env.<NAME> is required by this topic` | The topic's `miner_byok` variable is missing from `env`. Your `submit_nonce` is **not** spent — re-post the same signed body with `--env <NAME>` | no | no |
| **400** `env name <NAME> is not declared by this topic` | A variable the signed topic's `miner_byok` / `miner_env_allowlist` does not list. The message names what it does accept | no | no |
| **400** `env name <NAME> is not a miner environment variable` | Not `[A-Z][A-Z0-9_]{0,63}`, or a name the guest owns (`PROOF_…`, `PATH`, `HOME`, `LANG`, `XDG_RUNTIME_DIR`) | no | no |
| **400** `env.<NAME> is empty` | The value is blank. Check the variable is exported in the shell you ran `ctx` from | no | no |
| **400** invalid `miner_hotkey` / `artifact_digest` | Not exactly 64 lowercase hex (`0x`, uppercase, or whitespace) — the host verifies what you post and never normalises a hex field | no | no |
| **401** `hotkey_signature required` | Missing / empty `hotkey_signature` | no | no |
| **401** `hotkey_signature invalid` | Not exactly 128 lowercase hex, or does not verify under `miner_hotkey` for `base-proof-submit-v1` — including a `claim`, `declared_flops`, `manifest`, or `submit_nonce` that differs from what was signed | no | no |
| **401** `submit_nonce required` | Missing / empty `submit_nonce` | no | no |
| **401** `submit_nonce invalid` | Not exactly 64 lowercase hex | no | no |
| **401** `submit_nonce reused` | This `(miner_hotkey, submit_nonce)` pair was already presented with a valid signature: a replay. Sign again with a fresh nonce | no | no |
| **400** `artifact_digest is the sha256 of empty input …` | The digest of zero bytes or of an empty tar archive: hash the recipe bytes you actually upload (or serve at `artifact_uri`) | no | no |
| **503** empty `eval_image_digest` | Digest not pinned | no | no |
| **503** zero open sealed topics | Nothing to score against | no | no |
| **503** unsealed baseline | Topic open without both seal hashes | no | no |
| **503** live harvest down / unparseable agent verdict | Host cannot judge | no | no |
| **503** missing / closed RLM judge backend | Live `InferenceOffer` not scoring | no | no |
| **503** missing / closed / non-`1x` executor | Live `eval_executor` cannot rent the `1x` machine | no | no |
| **503** `proof deadline … exceeded` | Your recipe did not finish inside `max_proof_deadline_s`; the body carries the run's `stdout_tail` | no | no (pod torn down) |
| **503** `custom metric … has no registered runner` / `not wired` | The topic's `custom_id` has no runner on this host, or its topic VM is not configured | no | no |
| **503** missing / invalid evaluate `results.json` | Custom evaluate: no topic-defined results JSON, or it does not bind the scored `primary_value` / `claim_holds` / identities | no | no (guest torn down) |
| **503** staged artefact missing / digest mismatch | Upload-only evaluate: the host no longer holds matching vault bytes (missing, empty, oversize, or digest mismatch). The row is untouched — nothing was rented and no bytes are invented | live: no; deferred: queued | no |
| **201** `queued` | Topic in `deferred_topics` (operator still installing its scoring path); every **400** above still applies first | **yes** (queued, scored later) | **no** (not yet) |
| **200** existing row (`already queued …` / `already submitted … and scored`) | Same artefact + hotkey re-sent (freshly signed, new `submit_nonce`) to a deferring topic, before or after its row was drained | existing row | **no** |
| **201** `rejected` + `contamination_evidence_missing` | Empty manifest on a topic that requires training evidence (harvest default; custom / agent only if `require_training_evidence = "true"`) | **yes** (rejected) | **no** |
| **201** `rejected` + contamination | Holdout shard / corpus id in `manifest` | **yes** (rejected) | **no** |
| **201** `rejected` + `anti-cheat checklist red` | A topic rule failed on your artefact | **yes** (rejected) | **no** (no paid inference) |

Holdout overlap is a **reject, no rent**. An empty manifest is the same
shape **only** when the topic requires training evidence. It is not a 400
and not a 503. A red anti-cheat checklist is the same shape: a
persisted reject with no spend.

## Agent verdict (RLM judge)

The RLM lives in `ghcr.io/cortexlm/proof-eval@sha256:78b614a1f51ce5dd80076c4e343a2b31b85d6c36025e02836cb83929867e7009`.
It never sees holdout records. It gets the claim, the code, the public split,
and the constraints, and must emit:

| Field | Values | What it means for you |
|-------|--------|------------------------|
| `verdict` | `clean` \| `suspicious` \| `reject` | Only `clean` can pass. `suspicious` and `reject` are 0 |
| `reproduced` | bool | Recipe re-ran under the topic constraints |
| `claim_holds_public` | bool | Public-split numbers match the claim |
| `contamination` | bool | Holdout fingerprints in the recipe / data |
| `canary_hit` | bool | Off-score. Recorded, never a fail by itself |
| `flops_used` / `flops_budget` | u64 | Telemetry when the runner measured usage. Custom / agent topics do **not** reject on this vs `declared_flops` or the topic budget. Harvest `nll` / `throughput` may still fail `flops_over_budget` |
| `cheat_codes` | list | See below |
| `rationale` | string | Audit text (truncated) |
| `topic_id` / `family` | echo | Must match the submission |

`holdout_nll` is **not** an agent field. If the agent emits one, it is ignored.

### Cheat codes

Any of these except `other` zeros the run even when harness numbers look like
a win:

| Code | Meaning |
|------|---------|
| `unreproduced_claim` | Could not re-run the claimed recipe to the claimed result |
| `flops_over_budget` | Harvest `nll` / `throughput` only: run spent more FLOPs than the topic budget. **Not emitted** on custom / agent topics |
| `flops_under_declared` | Harvest leftover: run spent more FLOPs than `declared_flops`. **Not emitted** on custom / agent topics |
| `strawman_adamw` | Compared against a weaker / different AdamW than the sealed recipe |
| `fake_optimizer` | Optimizer named Muon / TSP (etc.) but the code is AdamW |
| `contamination` | Training data overlapped the holdout |
| `public_metric_mismatch` | Claimed public numbers do not match the harness public split |
| `other` | Named by the agent; does not by itself zero |

## Pass rules and paid score

The harness, not the agent, fills the metric values and decides `pass`.
Promotion is holdout-vs-sealed-baseline only; the public split never enters
the paid number.

### `nll` family

Primary: `holdout_nll` (min). Win: beat the sealed AdamW by
`epsilon_nll >= 0.02`. Per-split NLL regress `<= epsilon_topic_max_regress`
(pin floor 0.05; the topic may tighten).

### `throughput` family

Primary: `tokens_per_sec` (max) or `step_latency_ms` (min). Requires
`flops_budget` **and** `wall_budget_s`. Relative win `epsilon_rel >= 0.05`.
Quality floor: `holdout_nll <= sealed_nll + quality_floor_nll` (pin max
0.02). Speed is not free. The eval image enforces comms (for example
**12.5 Gbit/s**); it does not trust the claim.

### `custom` family (topic-minted metrics)

Primary: `metric.primary` (`max` or `min`, as the topic says). Win:
beat the sealed value by `metric.epsilon_rel` relative
(`primary >= sealed * (1 + epsilon_rel)` for `max`). The metric is computed
by the runner registered on the host under `metric.custom_id`; nothing
about it is compiled into the network. The topic's RLM runs in its own
Firecracker microVM on a dedicated KVM host, and **your code runs in a
separate ("sister") Firecracker guest beside it that has no network
interface**: the RLM obtains the artefact (HTTP `artifact_uri` on the
URI-only path, or a vsock inject of gateway-staged bytes when you uploaded),
checks it against your `artifact_digest`, inspects it, and ships **those
exact bytes** into the sister over vsock. Plan for an offline run —
nothing your code does at run time can reach the internet, the RLM, or the
host. The host (not the RLM) stamps `sandboxed` on your report from the
guest it booted, and the `flops_used` your verdict carries is what that
guest measured. If the topic sets `constraints.firecracker_required`, a run
that did not happen in that sister guest is not evidence; if it sets
`constraints.model_pin`, every paid call must name exactly that model;
`task_slice` / `params` are opaque runner inputs the topic defines (they are
exported to your run's environment).

Some topics select an **in-guest runner** instead: their
`constraints.params` carry `baseline_runner` (or `in_guest_benchmark_runner`)
and `experiment_pack_digest`. For such a topic your submission runs in **one
dedicated Firecracker VM created for that job and stopped after it** (destroyed
once it scored; the VM of a run that failed is kept stopped on the operator's
host so the failure can be traced — it is never reused for another job),
inside the operator's harness (a container runtime and benchmark adaptor
baked into the VM image by the operator — nothing about it lives in the
network repo), against the experiment pack the topic pins by digest —
16 vCPU / 32 GiB RAM unless the topic asks for less (that lock is a hard
maximum on every host), with at least 16 GiB of writable disk (32 GiB by
default). Upload the recipe at the gateway (`--artifact`, ≤5 MiB): evaluate
injects those staged bytes over vsock, so you do not host a fetch URL. A
miner-hosted `artifact_uri` is still fetched inside the VM (streamed, cut
at 64 MiB) when you did not upload. The bytes are checked against
`artifact_digest` before anything runs, the
result is recorded only once that VM is confirmed destroyed (an operator
cleanup failure is a 503 for you, never a score), the VM has only the operator's
egress allowlist (the topic says which registries / model providers), and
the host — not the harness — stamps `sandboxed` on your report. Read the
topic's `params` in `ctx proof topics`: they name the harness inputs
(tasks, agent, model, concurrency). Custom / agent topics do **not** apply
hardcoded `flops_used` accounting; anti-cheat is the signed checklist.

## Complete results JSON

A successful **custom** evaluate must produce a topic-defined complete
results document. The adaptor writes it next to `report.json` (default
name `results.json`; a topic may pin `constraints.params.results_path` to
another single `*.json` segment). The guest, harvest reconstruct, and
RLM scorer all bind it to the scored facts and **refuse a pass** when it
is missing or non-conforming (**503**, no pass row). This is display and
audit for the frontend — not a second score. `primary_value` /
`claim_holds` / identities must match `report.json`.

`GET /challenge/proof/v1/submissions/{id}` exposes the same object as
`results`. The operator artefact zip always embeds it at the zip root as
`results.json` (even when the topic pinned another write name).

Envelope (`schema_version` is `1`):

| Field | Required | Meaning |
|-------|----------|---------|
| `schema_version` | yes | `1` |
| `contract` | yes | Known contract id (below). A topic may pin `constraints.params.results_contract`; the file's `contract` must be the same family |
| `topic_id` / `custom_id` / `submission_digest` / `artifact_digest` | yes | Echo the run |
| `primary_value` | yes | Finite number; must match the scored primary |
| `claim_holds` | yes | Must match the scored report |

Known contracts:

| `contract` | Extra required fields |
|------------|------------------------|
| `generic-custom-v1` | `display`: non-empty JSON object (not `primary_value` alone) |
| `harbor-trials-v1` / `tbench-harbor-v1` | Untruncated `trials[]` (`name`, finite `reward`, `outcome`); `n_scored` = `trials.length`; `n_measured`; `mean_reward` = `primary_value` = mean of trial rewards; non-empty `agent`; `logs.harbor_run_tail` and/or `logs.harbor_run_log` |

Harvest `nll` / `throughput` rows have no `results`. A red-checklist
reject never ships the file. See [`tbench`](./proof-tbench.md) for the
Harbor trial document the frontend renders.

**Anti-cheat checklist — every rule in the topic's `checklist` (current
version) must pass before a single paid inference call is made.** Read the
rule texts in `ctx proof topics`; they are the contract. One red, missing,
duplicated, or evidence-less item is a persisted `rejected` row with no
spend. The rules may be re-versioned by the topic's RLM; the version you were
ticked against is recorded with your row.

Upload the **uncompressed** tar of your recipe tree (`tar -cf recipe.tar
recipe/`, then `sha256sum recipe.tar` is your `artifact_digest`) with
`ctx proof submit --artifact recipe.tar`. The digest is of **that file**,
byte for byte, not of the tree: re-running `tar` later produces a different
file (mtimes, member order) with a different digest, so keep the file you
hashed. A custom topic with neither an upload nor `artifact_uri` is a
**400** `artifact required` with no row. JSON + `artifact_uri` remains a
compat path (the runner fetches that file inside the topic VM); when you
upload, those bytes win and the URI is ignored for identity. The host
refuses gzip, non-tar bytes, a tree with no file content, or bytes that do
not hash to your `artifact_digest` — a run never starts on a substitute or
re-encoded artefact. A guest-measured `flops_used` may appear on the
verdict as telemetry. Custom / agent topics do **not** reject on that
figure vs `declared_flops` or the topic budget.

A clean pass that beats the current best (sealed value or reigning best) by
`epsilon_rel` is promoted automatically: the row is `champion` and the
operator archive keeps your artefact, `report.json`, `results.json`, and
`checklist.json` under `{topic_id}/{submission_id}.zip`. Runs on one topic are scored and
crowned one at a time against the best at that moment, so a run that is not
strictly better than the reigning champion never replaces it.

If the topic's `custom_id` is not in `registered_custom`, the topic is
`open` but not in `scorable_topics`, and submits answer **503** (`no
registered runner`). Nothing is stored and nothing is spent.

### Paid mass

A clean pass is eligible. `wta` / `discovery` then assign that topic's share
of Proof's **8000 bps** as above. Your paid score is the **sum of per-topic**
masses over currently `open` ids.

## Example topic (operator-published, not a git catalog)

`dt-no-ib-v0` is an operator **example**: throughput `wta`, no InfiniBand /
NVLink / NCCL fast fabric, **12.5 Gbit/s** cap, beat a sealed AdamW/comms
reference, 2e18 FLOPs. It pays only once it is signed, sealed, and `open`.
Until at least one topic is `open`, `GET /challenge/proof/v1/proof/topics`
is empty and submits **503**.

Never commit the Lium key. If something fails, see
[troubleshoot.md](./troubleshoot.md).
