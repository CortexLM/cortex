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
document wins. Do not copy a digest from this page — read
`ctx proof status` and `ctx proof topics`.

## Status right now: scoring is on

Live prod (checked 2026-09-10) scores `tbench`. A well-formed submit is
**evaluated**, not parked:

- `can_score` is **`true`**
- `open_topics` contains `tbench`
- `scorable_topics` contains `tbench`
- `deferred_topics` is **empty** (`[]`)
- `registered_custom` and `custom_ready` both contain `tbench`

So today a well-formed submit is **201** with a scored row (`awaiting_admin`,
`rejected`, or `champion`) — not **201 `queued`**. The host boots an
experiment VM, ticks the checklist, and (on a green checklist) runs your
artefact against the operator pack. That can take up to
`eval_executor.max_proof_deadline_s` (7200 s today). `ctx proof show --wait`
polls until the row is terminal.

The operator's sealed bar is currently a **stub** `custom_value` of `0.5`
(kept in the host's `baselines.json` until a measured n=15 is sealed). You
never see the recipe — only the public `script_sha256` / `metrics_commitment`.
Beat whatever the live sealed bar is by `epsilon_rel`; do not assume a
measured n=15, and do not invent a digest or a bar from this page.

**Historical / may return.** `tbench` previously set
`constraints.params.defer_scoring = "true"`, which put it in `deferred_topics`:
submits answered **201 `queued`** and nothing was evaluated until the operator
lifted the flag. That flag is **off** on the live document today. If the
operator re-publishes with `defer_scoring = "true"`, `tbench` leaves
`scorable_topics`, `can_score` may drop if nothing else is scorable, and
submits go back to **201 `queued`**. Read `ctx proof status` before you spend
— this page is not a substitute for the live fields.

A ready light is permission to try, not a payment guarantee. Emission still
reaches anyone only through a **sealed** weights bundle (`ctx weights`).

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
| `scorable_topics` | contains `tbench` while scoring is on (today). A well-formed submit is evaluated |
| `deferred_topics` | **empty** while scoring is on. If `tbench` appears here again, the operator has re-set `defer_scoring` and submits answer **201 `queued`** |
| `queued_submissions` | rows still waiting from a previous deferral (drained oldest first) — `0` while nothing is deferred |
| `registered_custom` / `custom_ready` | must both contain `tbench`. A registered id missing from `custom_ready` means its topic VM is not usable and the topic answers **503** |
| `custom_family_wired` | `true`. `tbench` is a `custom`-family topic, so `live_harvest_wired` (the Lium `nll` / `throughput` harvest) says nothing about it |
| `baseline_sealed` | `true`. An open topic without both seal hashes is **503** |
| `eval_image_digest` | a `sha256:…` pin. Empty is **503**. Read it live; do not copy one from a guide |

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
| `constraints.firecracker_required` | `true` | A run without the host's Firecracker attestation is not evidence: **503**, no row, no host fallback. On `tbench` that attestation is the **experiment VM** (`network: egress-allowlist`), not a sister guest |
| `constraints.params.baseline_runner` | `rlm_fc_in_guest_harbor` | Selects the **experiment VM** path: one dedicated Firecracker VM per paid job, created for the job and destroyed after it. Read the live value; this id is topic data |
| `constraints.params.experiment_pack_digest` | a `sha256:` pin | The operator's experiment pack, re-hashed by the host before any jail. Not yours to supply or patch. Read it live |
| `constraints.params.miner_byok` | `OPENROUTER_API_KEY` | You bring the model key — see § 4 |
| `constraints.params.defer_scoring` | **absent** (scoring on) | If this returns as `"true"`, submits queue again (§ Status) |
| `flops_budget` | `2e18` | Topic document field. **Not** a reject gate on `tbench`: the host ignores `declared_flops` vs measured FLOPs |
| `eval_executor.max_proof_deadline_s` | `7200` | Your run is cut at this wall clock; a cut run is **503** with the run's `stdout_tail` |
| `payout_mode` | `discovery` | Pass floor plus novelty pool — see § 7 |
| `baseline` | sealed | `script_sha256` + `metrics_commitment`, seed `42`. Standing bar is a stub `custom_value` of `0.5` until a measured n=15 is sealed. You never see the recipe |
| `status` | `open` | Only `open` accepts submits |

`GET /v1/proof/topics` never returns holdout records, and `holdout_commitment`
is a commitment, not data. There is nothing to read there.

## 3. Miner artefact / attach surface

`tbench` is a `custom` topic, so **`artifact_uri` is required** — the guest
agent fetches the bytes from your locator inside the experiment VM. A submit
without one is a **400** with no row.

The attach surface is the guest-runner **contract** in
[`deploy/guest/runners/README.md`](../../deploy/guest/runners/README.md)
plus the versioned adaptor
[`rlm_fc_in_guest_harbor`](../../deploy/guest/runners/rlm_fc_in_guest_harbor/README.md)
(the live `baseline_runner`). That adaptor is what evaluate execs; this page
is what you must ship so it has a Harbor agent to attach.

### Build the file you will serve

Artefact identity is **the served file, verbatim**:

```bash
tar -cf recipe.tar recipe/      # uncompressed. No gzip, no zip
sha256sum recipe.tar            # this is your artifact_digest
```

The guest unpacks that tar under `$PROOF_ARTIFACT_DIR`. Evaluate attaches
your **Harbor agent**, not a silent copy of the operator's `terminus-2`.
Harbor's `-a` / `--agent` accepts a built-in name or a Python import path
(`module.path:ClassName`); it does **not** take a filesystem path. The
adaptor therefore imports your class from the artefact and passes that
import path as `-a`. Layout after unpack (paths relative to
`PROOF_ARTIFACT_DIR`):

```
recipe/
  agent/            # PREFERRED: Harbor agent (BaseAgent / BaseInstalledAgent)
    agent.py
    import_path     # optional: one line `agent.agent:YourClass`
  run.sh            # optional classic marker; inspect may see it; evaluate does not exec it
  README.md
```

If you pack with `tar -cf recipe.tar -C recipe .`, the same `agent/` directory
sits at the tar root (`$PROOF_ARTIFACT_DIR/agent`). Resolution order:
`$PROOF_ARTIFACT_DIR/agent`, then `$PROOF_ARTIFACT_DIR/recipe/agent`. A
`recipe/run.sh` with no Harbor agent dir is **not** scored as a substitute —
evaluate fails closed rather than falling back to the topic agent. Off-limits
in the tree (inspect fails the named rule): `no_eval_short_circuit`,
`no_tb4_hardcoding`.

Env the run sees: `PROOF_SEED`, `PROOF_MODEL_PIN`, `PROOF_TASK_SLICE`,
`PROOF_PARAM_*`, `PROOF_PACK_DIR`, `PROOF_ARTIFACT_DIR`, `PROOF_OUTPUT_DIR`,
`PROOF_WORK_DIR`, and miner BYOK under `PROOF_MINER_ENV_DIR`.

- Serve **that exact file** at `artifact_uri` and keep it. Re-running `tar`
  later produces different bytes (mtimes, member order) and therefore a
  different digest, and the run is refused rather than run on a substitute.
- The host re-hashes exactly the bytes the guest fetched before it continues,
  and refuses gzip, non-tar bytes, an archive with no file content, or bytes
  that do not match your `artifact_digest`.
- A digest of nothing — the sha256 of zero bytes or of an empty tar — is a
  **400** with no row, whatever case you spell it in.
- On the experiment-VM path the guest agent streams your artefact under a hard
  **64 MiB** cap. Ship a recipe, not a weight dump.

### What the guest sees after unpack

The guest agent fetches, verifies, and unpacks your tar into
`PROOF_ARTIFACT_DIR`. `$PROOF_WORK_DIR/artifact.tar` holds the verbatim bytes.
`tar -cf recipe.tar recipe/` unpacks as `PROOF_ARTIFACT_DIR/recipe/…`.

The operator pack is a **different** tree: `PROOF_PACK_DIR` (and
`PROOF_PACK_DIGEST`) is staged by the host from the topic's
`experiment_pack_digest`. Miners do **not** patch the pack. Tasks, the
evaluator, and the default baseline agent live there. Your submit is the
artefact only.

Preferred miner layout — put your Harbor agent at one of:

| After unpack | How you tared it |
|-------------|------------------|
| `$PROOF_ARTIFACT_DIR/agent` | agent at the archive root (`tar -cf recipe.tar agent` or equivalent) |
| `$PROOF_ARTIFACT_DIR/recipe/agent` | the usual `tar -cf recipe.tar recipe/` with `recipe/agent/…` inside |

**Evaluate must use the miner agent**, not the pack's default. Harbor
`-a` / `--agent` is a built-in name or a Python import path
(`module.path:ClassName`), **not** a filesystem path: the adaptor imports
your class from `$PROOF_ARTIFACT_DIR/agent` or
`$PROOF_ARTIFACT_DIR/recipe/agent` and passes that import path. A
`recipe/run.sh` with no Harbor agent dir is **not** a substitute — evaluate
fails closed rather than falling back to `terminus-2`. Baseline is
operator-owned (pack + the topic's agent param) only when no miner artefact
is staged. Do not copy the pack into your tar.

### Off-limits (checklist red, no spend)

These fail **before any paid inference**. One red item is a persisted
`rejected` row:

| Do not | Why |
|--------|-----|
| Reseed | `same_seed`: every paid call and every scored episode uses the topic baseline seed (`42` today). No per-miner reseeding |
| Hardcode TB4 task ids, answers, fixtures, or success paths | `no_tb4_hardcoding`. Only the signed topic carries `task_slice` |
| Patch the evaluator, the metric path, the pack, or short-circuit scoring | `no_eval_short_circuit`. You do not own `PROOF_PACK_DIR` |
| Embed an OpenRouter key (`sk-or-…`) in the artefact, the claim, or the manifest | The key is BYOK `env` only. A key in the tree is not how the guest authenticates, and it will leak into the artefacts zip |

Write logs you need to keep under `$PROOF_WORK_DIR` or `$PROOF_OUTPUT_DIR`.
stdout / stderr are only a 64 KiB rolling tail; they are not the archive.

### Artefacts zip

The host collects a zip per scored row at
`{topic_id}/{submission_id}.zip` (checklist rule `artefacts_zip`):

- your unpacked recipe (`artifact/`)
- `report.json` (the adaptor's measurement; absent on a pre-spend reject)
- `checklist.json` (the rule items with evidence)
- runner logs (`logs/`) — whatever the adaptor wrote under
  `PROOF_WORK_DIR` / `PROOF_OUTPUT_DIR`, plus harness stdout the guest kept

A red-checklist reject ships no `report.json`. You do not upload this zip;
the host builds it. Putting secrets in the recipe or in unredacted log
files puts them in the zip.

Your code runs **inside a Firecracker experiment VM the host boots**, against
the operator's pinned pack. You cannot produce the sandbox attestation
yourself and you do not need to: the host — not the harness, not the RLM —
stamps it (`mode: experiment_vm`, `network: egress-allowlist`). A
guest-measured `flops_used` may appear on the verdict as telemetry.
**`tbench` does not reject or cheat-code on `declared_flops` vs measured
FLOPs** — anti-cheat is the signed topic checklist, Firecracker attestation,
BYOK env, and hotkey signature, not a hardcoded TFLOP budget.

## 4. The model key is yours (BYOK)

`tbench` runs on the **experiment-VM** path
(`baseline_runner = rlm_fc_in_guest_harbor`). That VM has **HTTPS egress** on
the operator's allowlist (model provider + whatever the pack pulls). Your
OpenRouter calls leave the guest over that allowlist.

That is **not** the other custom-family path. Topics that do not select an
in-guest runner still score in a **sister** Firecracker guest with
**network none**: no outbound HTTPS, no BYOK call from inside the sister.
`tbench` is not that path. If you plan as if the guest is offline, your
key never reaches the provider and the run dies on inference.

The topic pins `moonshotai/kimi-k3` and sets
`constraints.params.miner_byok = "OPENROUTER_API_KEY"`. Its checklist rule
`miner_byok_openrouter` reads:

> miner supplies `OPENROUTER_API_KEY` (BYOK) for `moonshotai/kimi-k3`; operator
> keys are never injected into the miner guest

So the inference your run does is billed to **your** OpenRouter account, and the
operator's own key material is never staged into your guest. `reject_if` on this
topic names a missing BYOK key explicitly.

You send the key in the submit body's `env` map, keyed by the variable the
topic named:

```json
{ "env": { "OPENROUTER_API_KEY": "sk-or-…" } }
```

With `ctx`, bare `--env OPENROUTER_API_KEY` reads it from your shell so it never
lands in your shell history or in `ps`; `--env OPENROUTER_API_KEY=sk-or-…`
passes it inline:

```bash
export OPENROUTER_API_KEY=sk-or-…
ctx proof submit … --env OPENROUTER_API_KEY
```

What the host does with it:

- **Only what the topic declared.** `tbench` names `OPENROUTER_API_KEY` and
  nothing else, so any other variable is **400**. Nothing is silently dropped.
- **Omitting it is 400, before you spend.** The refusal names the variable, and
  because `env` is **not** part of `base-proof-submit-v1`, it is checked before
  your signature — your `submit_nonce` is untouched and you can re-post the
  same signed body with the flag added.
- **It is not echoed back.** The value never appears on your row, in
  `GET /v1/submissions`, or in `/v1/status`, and it is blanked to `[REDACTED]`
  in any run log or evidence your own run prints it into.
- **It is kept in a private file, not a process value**, from the moment it is
  accepted until your row is terminal. Scoring-on uses it on this submit. If
  `defer_scoring` returns, the same vault still has the key when the operator
  drains. If the host loses it before the run, a drain answers **503** with
  the row untouched; it never scores your work on the operator's account.

One thing to be clear about, because guessing here costs money:
**`X-Lium-Api-Key` is not this key, and is not identity.** It is the Lium
header from [proof.md](./proof.md), used by the Lium harvest families for
**compute**. It never carries a model-provider key and it never authenticates
you. `env` pays for inference; `X-Lium-Api-Key` pays for machines. `tbench`
does not rent Lium for your run.

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
  --train-dataset my-harness-v0 \
  --json
```

`--train-dataset` is a **contamination label**, not a training run. An
honest id is enough when you did not train (a harness name such as
`my-harness-v0` is fine). An **empty** manifest — no
`train_dataset_ids` and no `train_content_hashes` — is not a clean
contamination check: on `tbench` it is a persisted **`rejected`** row with
`contamination_evidence_missing` and **no rent**. `ctx` refuses to build
one client-side.

That prints `miner_hotkey`, `hotkey_signature`, `submit_nonce`, the exact
`manifest`, and `domain` (`base-proof-submit-v1`) without posting anything.
Then submit:

```bash
export OPENROUTER_API_KEY=sk-or-…
ctx proof submit \
  --secret-file /path/to/hotkey.sk \
  --topic-id tbench \
  --artifact-digest <sha256 of recipe.tar> \
  --artifact-uri https://example.org/recipe.tar \
  --claim "raised first-15 success_rate over the sealed baseline by 0.08" \
  --train-dataset my-harness-v0 \
  --env OPENROUTER_API_KEY
```

`--env` is not part of `ctx proof sign`: the key is posted beside the
signature, never inside it, so the signed bytes are the same with or without
it.

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
| `declared_flops` | no | Optional, default `0`. Still bound into the signature if you send it. **Ignored as a scoring gate** on `tbench` |
| `manifest.train_content_hashes` / `manifest.train_dataset_ids` | yes | Declare at least one. An honest harness id is enough when you did not train. Signed. Empty → `rejected` + `contamination_evidence_missing` |
| `env` | **yes** | `{"OPENROUTER_API_KEY": "sk-or-…"}` — the topic's `miner_byok` variable (§ 4). **Not** signed, never echoed back. Missing → **400** without spending your nonce |

The signature covers the hotkey, `topic_id`, `artifact_digest`,
`declared_flops` (optional, default `0`), `claim`, the canonical manifest, and
`submit_nonce` — in that order, under the `base-proof-submit-v1` domain.
`artifact_uri` is **not** signed. The exact byte layout and the Python reference
are in
[proof.md § 2](./proof.md#2-submit-a-reproducible-experiment); do not re-derive
it from this page.

The `(miner_hotkey, submit_nonce)` pair is accepted **once**, reserved before
any row exists. Re-posting the identical body is **401 `submit_nonce reused`**.
To re-send the same artefact, sign again with a fresh nonce. While scoring is
on, that is a **new** run (a new row). The **200** existing-row answer is the
defer path (`deferred_topics`): one queued-or-scored row per artefact per
topic, not a second paid run.

## 6. Watch the row

```bash
ctx proof show <id>
ctx proof show <id> --wait
# same as:
curl -sS https://gateway.cortex.foundation/challenge/proof/v1/submissions/<id>
```

| `state` | Meaning on `tbench` |
|---------|---------------------|
| `queued` | **Not** the live answer while scoring is on. Only if `tbench` is back in `deferred_topics`: accepted and stored, **not evaluated**. No VM, no judge call, no mass. The only non-terminal state; the operator drains later |
| `awaiting_admin` | Clean pass, mass recorded. The operator audit is informational |
| `rejected` | A gate failed: contamination, unreproduced claim, a red checklist item. Pre-eval rejects spend nothing. FLOP accounting is **not** a reject gate on `tbench` |
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
| `no_eval_short_circuit` | The evaluator, the metric path, and the sandbox are untouched — no short-circuiting the checklist or the scoring; do not patch `PROOF_PACK_DIR` |
| `miner_byok_openrouter` | You supply the OpenRouter key for the pinned model; operator keys are never injected into your guest |
| `firecracker_sister` | Your code runs only in the Firecracker guest the host booted, and the report must carry that attestation. On `tbench` that guest is the **experiment VM** (egress allowlist), not a network-none sister |
| `artefacts_zip` | Scored artefacts persist as `{topic_id}/{submission_id}.zip`: recipe, `report.json`, `checklist.json`, runner logs (§ 3) |
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
| **201** scored (`awaiting_admin` / `rejected` / `champion`) | The normal answer today: `tbench` is in `scorable_topics` | yes |
| **201** `queued` | Only if `tbench` is in `deferred_topics` again (`defer_scoring` returned) | yes, queued |
| **200** existing row | Same artefact + hotkey re-sent with a fresh nonce **on the defer path**, before or after the queue drained | existing row |
| **400** `unknown topic` / `topic is not open` | `tbench` is not published, or is outside its epoch window | no |
| **400** `artifact_uri is required for custom topics` | You left the locator out. `tbench` is `custom` | no |
| **400** `artifact_digest is the sha256 of empty input …` | You hashed nothing, or an empty tar | no |
| **400** invalid `miner_hotkey` / `artifact_digest` | Not exactly 64 lowercase hex. The host never normalises a hex field | no |
| **401** `hotkey_signature required` / `invalid` | Missing signature, or a `claim`, `declared_flops`, `manifest`, or nonce that differs from what you signed | no |
| **401** `submit_nonce required` / `invalid` / `reused` | Missing, not 64 lowercase hex, or a replay. Sign again with a fresh nonce | no |
| **503** `custom metric … has no registered runner` / `not wired` | `tbench` is not in `registered_custom` / `custom_ready` | no |
| **503** empty `eval_image_digest` / unsealed baseline / missing judge offer | The host cannot score. Fail-closed, never a sim fallback | no |
| **503** `proof deadline … exceeded` | Your run did not finish inside `max_proof_deadline_s`; the body carries `stdout_tail` | no |
| **201** `rejected` + `contamination_evidence_missing` | Empty manifest | yes, rejected |
| **201** `rejected` + red checklist | A topic rule failed on your artefact — before any paid inference | yes, rejected |

Never commit your OpenRouter key or `LIUM_API_KEY`, and never hand anyone a
mnemonic or a challenge signing key. If something fails, see
[troubleshoot.md](./troubleshoot.md); the generic Proof contract is
[proof.md](./proof.md).
