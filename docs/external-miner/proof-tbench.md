<!-- protocol_version: 1 -->

# Proof topic `tbench` — miners

`tbench` is a live **Proof** topic. It is not a separate challenge: you submit
to `proof` (**8000 bps**) exactly as [proof.md](./proof.md) describes, with
`topic_id = tbench`. This page is the topic-specific part — what the signed
document asks for, what its anti-cheat checklist ticks, and what a submit
answers today.

**Gateway:** [https://gateway.cortex.foundation](https://gateway.cortex.foundation)  
**Live topic:** `GET /challenge/proof/v1/proof/topics/tbench` (or `ctx proof topics`)  
**Generic submit contract:** [proof.md](./proof.md) — signing payload, manifest,
hex rules, and the full status/error tables live there and are not repeated here.

Every number, digest, and rule below is **topic data**: it comes from a signed
document an operator publishes and can re-publish at any time. Nothing about
`tbench` is compiled into the network binaries. Read the live document before
you spend anything; if this page and the live document disagree, the live
document wins.

## Status right now: submits are accepted but not scored

`tbench` sets `constraints.params.defer_scoring = "true"`. That puts it in
`deferred_topics` on `GET /challenge/proof/v1/status`: the host **accepts** your
submission and stores it as **`queued`** (**201**), and then evaluates nothing.
No pod is rented, no VM boots, no judge call is made, and no mass is recorded
until the operator lifts the flag and drains the queue, oldest first and one at
a time.

So today:

- A well-formed submit is **201 `queued`**, not a score.
- `can_score` on `/challenge/proof/v1/status` is **`false`** and
  `scorable_topics` does **not** list `tbench`. That is expected for a deferring
  topic and is not an outage — a topic in `deferred_topics` is the one case where
  `can_score: false` does not mean **503**.
- A queued row is the only non-terminal state. `ctx proof show --wait` keeps
  polling through it for as long as the operator's install takes.

Queue for a topic that is not yet scoring only if you accept that a `queued`
row is a place in line, not a payment. A ready light is permission to try.

## 0. Install `ctx`

```bash
curl -fsSL https://raw.githubusercontent.com/CortexLM/cortex/main/scripts/install-ctx.sh | sh
ctx --version
```

Use **v3.3.31 or newer**. Older builds predate the miner hotkey signature and
the single-use `submit_nonce`, so they post bodies the host answers **401**.
Pin a version with `CTX_VERSION=v3.3.31` if you need to. The download is
checksum-verified against the release's `SHA256SUMS.txt`.

## 1. Check the host before you build anything

```bash
ctx proof status
# same as:
curl -sS https://gateway.cortex.foundation/challenge/proof/v1/status
```

Read these fields for `tbench`:

| Field | What you want to see |
|-------|----------------------|
| `open_topics` | contains `tbench` — otherwise the topic is not published and a submit is **400** `unknown topic` / `topic is not open` |
| `deferred_topics` | contains `tbench` while scoring is deferred: submits answer **201 `queued`** |
| `scorable_topics` | contains `tbench` once the topic is actually being scored. While it is deferred, it is not here |
| `queued_submissions` | how many rows are already waiting across topics — your place in line |
| `registered_custom` / `custom_ready` | must both contain `tbench`. A registered id missing from `custom_ready` means its topic VM is not usable and the topic answers **503** once it stops deferring |
| `custom_family_wired` | `true`. `tbench` is a `custom`-family topic, so `live_harvest_wired` (the Lium `nll` / `throughput` harvest) says nothing about it |
| `baseline_sealed` | `true`. An open topic without both seal hashes is **503** |
| `eval_image_digest` | a `sha256:…` pin. Empty is **503** |

Read the digests, the judge `inference_offer`, and the `eval_executor` from
this endpoint rather than from any document. They are re-pinned by the
operator; a digest copied into a guide goes stale, and this page deliberately
does not carry one.

## 2. Read the topic document

```bash
ctx proof topics
curl -sS https://gateway.cortex.foundation/challenge/proof/v1/proof/topics/tbench
```

What the signed document carries today, and what each field means for you:

| Field | Value today | What it means |
|-------|-------------|---------------|
| `metric.family` | `custom` | Scored by the runner registered under `metric.custom_id`, inside a topic VM — not by the Lium `nll` / `throughput` harvest |
| `metric.custom_id` | `tbench` | Must be in `registered_custom` **and** `custom_ready` |
| `metric.primary` / `direction` | `success_rate`, `max` | Higher is better. `success_rate` is what the operator's harness reports for the scored task slice |
| `metric.epsilon_rel` | `0.05` | A pass must beat the bar (sealed baseline, or the reigning champion) by this **relative** margin |
| `constraints.task_slice` | `tb4-first-15` | The scored slice. It is an opaque runner input: the task content is not in this repository and must not be in your code |
| `constraints.model_pin` | `moonshotai/kimi-k3` | Every paid model call in your run must name exactly this model |
| `constraints.firecracker_required` | `true` | A run without the host's sister-guest attestation is not evidence: **503**, no row, no host fallback |
| `constraints.params.baseline_runner` | an in-guest runner id | Selects the **experiment VM** path: one dedicated Firecracker VM per paid job, created for the job and destroyed after it |
| `constraints.params.experiment_pack_digest` | a `sha256:` pin | The operator's experiment pack, re-hashed by the host before any jail. Not yours to supply |
| `constraints.params.miner_byok` | `OPENROUTER_API_KEY` | You bring the model key — see § 4 |
| `constraints.params.defer_scoring` | `"true"` | Submits are queued, not scored (§ Status) |
| `flops_budget` | `2e18` | `declared_flops` must be `<=` this |
| `eval_executor.max_proof_deadline_s` | `7200` | Your run is cut at this wall clock; a cut run is **503** with the run's `stdout_tail` |
| `payout_mode` | `discovery` | Pass floor plus novelty pool — see § 7 |
| `baseline` | sealed | `script_sha256` + `metrics_commitment`, seed `42`. You never see the recipe, only the commitments |
| `status` | `open` | Only `open` accepts submits |

`GET /v1/proof/topics` never returns holdout records, and `holdout_commitment`
is a commitment, not data. There is nothing to read there.

## 3. Build and serve the artefact

`tbench` is a `custom` topic, so **`artifact_uri` is required** — the runner
fetches the bytes from your locator inside the topic VM. A submit without one
is a **400** with no row.

Artefact identity is **the served file, verbatim**:

```bash
tar -cf recipe.tar recipe/      # uncompressed. No gzip, no zip
sha256sum recipe.tar            # this is your artifact_digest
```

- Serve **that exact file** at `artifact_uri` and keep it. Re-running `tar`
  later produces different bytes (mtimes, member order) and therefore a
  different digest, and the run is refused rather than run on a substitute.
- The host re-hashes exactly the bytes the runner fetched before it boots your
  guest, and refuses gzip, non-tar bytes, an archive with no file content, or
  bytes that do not match your `artifact_digest`.
- A digest of nothing — the sha256 of zero bytes or of an empty tar — is a
  **400** with no row, whatever case you spell it in.
- On the experiment-VM path the guest agent streams your artefact under a hard
  **64 MiB** cap. Ship a recipe, not a weight dump.

Your code runs **inside a Firecracker guest the host boots**, against the
operator's pinned experiment pack. You cannot produce the sandbox attestation
yourself and you do not need to: the host — not the harness, not the RLM —
stamps it, along with the `flops_used` the guest measured. That measured figure,
not your `declared_flops`, is what the verdict carries, and it must stay within
both the topic budget (`flops_over_budget`) and your own declaration
(`flops_under_declared`). Declare what you will actually use.

## 4. The model key is yours (BYOK)

The topic pins `moonshotai/kimi-k3` and sets
`constraints.params.miner_byok = "OPENROUTER_API_KEY"`. Its checklist rule
`miner_byok_openrouter` reads:

> miner supplies `OPENROUTER_API_KEY` (BYOK) for `moonshotai/kimi-k3`; operator
> keys are never injected into the miner guest

So the inference your run does is billed to **your** OpenRouter account, and the
operator's own key material is never staged into your guest. `reject_if` on this
topic names a missing BYOK key explicitly.

Two things to be clear about, because guessing here costs money:

- **`X-Lium-Api-Key` is not this key, and is not identity.** It is the Lium
  header from [proof.md](./proof.md), used by the Lium harvest families. It
  never carries a model-provider key and it never authenticates you.
- **The submit body has no field for a provider key.** `POST /v1/submissions`
  takes the fields in § 5 and nothing else; there is no header or parameter on
  the public API that hands the gateway an OpenRouter key. The delivery path is
  defined by the topic and the operator's guest image, not by the submit API.
  Read `constraints.params` on the live topic and confirm the arrangement with
  the operator **before** you spend — do not invent a channel and do not paste a
  key into a field that is signed and echoed back.

Never commit an OpenRouter key, and never put one in your claim, your manifest,
or a repository you publish as `artifact_uri`.

## 5. Sign and submit

Proof identity is an sr25519 signature by your miner hotkey. Sign first if you
want to see exactly what will be posted:

```bash
ctx proof sign \
  --secret-file /path/to/hotkey.sk \
  --topic-id tbench \
  --artifact-digest <sha256 of recipe.tar> \
  --claim "raised first-15 success_rate over the sealed baseline by 0.08" \
  --declared-flops 1500000000000000000 \
  --train-dataset my-harness-v0 \
  --json
```

That prints `miner_hotkey`, `hotkey_signature`, `submit_nonce`, the exact
`manifest`, and `domain` (`base-proof-submit-v1`) without posting anything.
Then submit:

```bash
ctx proof submit \
  --secret-file /path/to/hotkey.sk \
  --topic-id tbench \
  --artifact-digest <sha256 of recipe.tar> \
  --artifact-uri https://example.org/recipe.tar \
  --claim "raised first-15 success_rate over the sealed baseline by 0.08" \
  --declared-flops 1500000000000000000 \
  --train-dataset my-harness-v0
```

Pass **exactly one** signer: `--secret-file` (a 32-byte mini-secret, never a
mnemonic), `--wallet-name` (a Bittensor wallet), or an offline `--signature`
together with `--hotkey` and the `--submit-nonce` that was signed. `ctx` refuses
combinations rather than picking one for you.

Fields the host reads on `POST /challenge/proof/v1/submissions`:

| Field | Required | Notes for `tbench` |
|-------|----------|--------------------|
| `miner_hotkey` | yes | Exactly 64 lowercase hex, no `0x` |
| `hotkey_signature` | yes | Exactly 128 lowercase hex, sr25519 over `base-proof-submit-v1` |
| `submit_nonce` | yes | Exactly 64 lowercase hex, 32 fresh random bytes, **single-use per hotkey** |
| `topic_id` | yes | `tbench` |
| `artifact_digest` | yes | sha256 of the exact file you serve; not the digest of nothing |
| `artifact_uri` | **yes** | Required because `tbench` is a `custom` topic |
| `claim` | yes | One English sentence of what improved. Signed |
| `declared_flops` | yes | `<= 2e18`. Signed, and enforced against the guest's measurement |
| `manifest.train_content_hashes` / `manifest.train_dataset_ids` | yes | Declare at least one. Signed |

The signature covers the hotkey, `topic_id`, `artifact_digest`,
`declared_flops`, `claim`, the canonical manifest, and `submit_nonce` — in that
order, under the `base-proof-submit-v1` domain. `artifact_uri` is **not** signed.
The exact byte layout and the Python reference are in
[proof.md § 2](./proof.md#2-submit-a-reproducible-experiment); do not re-derive
it from this page.

The `(miner_hotkey, submit_nonce)` pair is accepted **once**, reserved before
any row exists. Re-posting the identical body is **401 `submit_nonce reused`**.
To re-send the same artefact, sign again with a fresh nonce — on a deferring
topic that returns the **existing** row (**200**), not a second one: one run per
artefact per topic.

An empty manifest is not a clean contamination check. On `tbench` it is queued
now and becomes a persisted **`rejected`** row with `contamination_evidence_missing`
and no rent when the queue drains. `ctx` refuses to build one client-side.

## 6. Watch the row

```bash
ctx proof show <id>
ctx proof show <id> --wait
# same as:
curl -sS https://gateway.cortex.foundation/challenge/proof/v1/submissions/<id>
```

| `state` | Meaning on `tbench` |
|---------|---------------------|
| `queued` | Accepted and stored, **not evaluated**. No rent, no VM, no judge call, no mass. The only non-terminal state; the operator drains the queue later and the row becomes one of the three below |
| `awaiting_admin` | Clean pass, mass recorded. The operator audit is informational |
| `rejected` | A gate failed: contamination, unreproduced claim, a red checklist item, FLOPs over budget or over your declaration. Pre-eval rejects spend nothing |
| `champion` | Promoted. On this topic promotion is automatic: a passing run with a green checklist that beats the sealed bar or the reigning best by `epsilon_rel`. Proof pays on pass, not on the crown |

Runs on one topic are scored and crowned one at a time against the best at that
moment, so a run that is not strictly better than the reigning champion never
replaces it.

## 7. What `tbench` pays

`payout_mode` is `discovery`, so the topic's mass splits two ways:

- a **pass floor** (`discovery.pass_floor_share_bps`, `3000` today) split equally
  among verified passes — this reimburses compute;
- a **novelty pool** (`novelty_pool_share_bps`, `7000` today) weighted by how
  much you improved on the sealed baseline and the current champion. A
  near-duplicate of an already-accepted artefact keeps the floor and earns zero
  novelty.

Your paid Proof score is the **sum of per-topic** masses over currently `open`
ids, not a mean. `tbench` is one topic inside Proof's **8000 bps**; skipping a
topic is a 0 on that topic, not a penalty elsewhere.

Emission only reaches anyone through a **sealed** weights bundle. Check
`ctx weights`: an unsealed burn vector (uid 0 = 100%) is the fail-closed answer
and is not a payment path.

## 8. The anti-cheat checklist

The topic carries a `checklist` of rules `[{id, text}]`. They are ticked over
your artefact **before any paid inference**: one red, missing, duplicated, or
evidence-less item is a persisted `rejected` row with **no spend**. The rule
texts in the live topic are the contract; the ids published today are:

| Rule id | What it holds you to |
|---------|----------------------|
| `same_seed` | Every paid call and every scored episode uses the topic baseline seed. No per-miner reseeding |
| `no_tb4_hardcoding` | Your harness must not hardcode task ids, answers, fixtures, or success paths. Only the signed topic carries `task_slice` |
| `no_eval_short_circuit` | The evaluator, the metric path, and the sandbox are untouched — no short-circuiting the checklist or the scoring |
| `miner_byok_openrouter` | You supply the OpenRouter key for the pinned model; operator keys are never injected into your guest |
| `firecracker_sister` | Your code runs only in the Firecracker guest the host booted, and the report must carry that attestation |
| `artefacts_zip` | Scored artefacts persist as a zip under the topic's artefact root, per submission |
| `auto_promote_best` | Only a passing, green-checklist run that beats the bar by `epsilon_rel` is promoted |
| `rlm_topic_setup_autonomous` | The topic's RLM drives setup, baseline, evaluation, and promotion from the signed document alone |

Rules may be **re-versioned** by the topic's RLM. The version you were ticked
against is recorded with your row, so read the live texts before each submit
rather than trusting a copy.

## 9. What an answer means

A **400** is your request, a **401** is your signature or nonce, a **503** is the
host, and a **201** means a row exists. None of the refusals rent anything, and
none of them persist a row. The generic table is in
[proof.md § HTTP 400 vs 503](./proof.md#http-400-vs-503); these are the answers
you will actually meet on `tbench`:

| Answer | When | Row? |
|--------|------|------|
| **201** `queued` | The normal answer today: `tbench` is in `deferred_topics` | yes, queued |
| **200** existing row | Same artefact + hotkey re-sent with a fresh nonce, before or after the queue drained | existing row |
| **400** `unknown topic` / `topic is not open` | `tbench` is not published, or is outside its epoch window | no |
| **400** `artifact_uri is required for custom topics` | You left the locator out. `tbench` is `custom` | no |
| **400** `artifact_digest is the sha256 of empty input …` | You hashed nothing, or an empty tar | no |
| **400** invalid `miner_hotkey` / `artifact_digest` | Not exactly 64 lowercase hex. The host never normalises a hex field | no |
| **400** `declared_flops exceeds the topic budget` | Over `2e18` | no |
| **401** `hotkey_signature required` / `invalid` | Missing signature, or a `claim`, `declared_flops`, `manifest`, or nonce that differs from what you signed | no |
| **401** `submit_nonce required` / `invalid` / `reused` | Missing, not 64 lowercase hex, or a replay. Sign again with a fresh nonce | no |
| **503** `custom metric … has no registered runner` / `not wired` | `tbench` is not in `registered_custom` / `custom_ready`. Only reachable once the topic stops deferring | no |
| **503** empty `eval_image_digest` / unsealed baseline / missing judge offer | The host cannot score. Fail-closed, never a sim fallback | no |
| **503** `proof deadline … exceeded` | Your run did not finish inside `max_proof_deadline_s`; the body carries `stdout_tail` | no |
| **201** `rejected` + `contamination_evidence_missing` | Empty manifest, at drain time | yes, rejected |
| **201** `rejected` + red checklist | A topic rule failed on your artefact — before any paid inference | yes, rejected |

Never commit your OpenRouter key or `LIUM_API_KEY`, and never hand anyone a
mnemonic or a challenge signing key. If something fails, see
[troubleshoot.md](./troubleshoot.md); the generic Proof contract is
[proof.md](./proof.md).
