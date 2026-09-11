<!-- protocol_version: 1 -->

# Proof topic `tbench` — miners

`tbench` is a live **Proof** topic. It is not a separate challenge: you submit
to `proof` (**8000 bps**) exactly as [proof.md](./proof.md) describes, with
`topic_id = tbench`. This page is the topic-specific part — what the signed
document asks for, what its anti-cheat checklist ticks, and what a submit
answers today.

**`tbench` is Proof Firecracker, not Lium.** Inspect and evaluate run inside
the host's dedicated experiment VMs (one Firecracker guest per paid job).
The Lium harvest (`nll` / `throughput`, `X-Lium-Api-Key`,
`live_harvest_wired`) does not score this topic. A red checklist with
`flops_used = 0` / `custom_value = null` is Proof FC inspect refusing
evaluate — not a Lium rent failure.

**Gateway:** [https://gateway.cortex.foundation](https://gateway.cortex.foundation)  
**Live topic:** `GET /challenge/proof/v1/proof/topics/tbench` (or `ctx proof topics`)  
**Generic submit contract:** [proof.md](./proof.md) — signing payload, manifest,
hex rules, and the full status/error tables live there and are not repeated here.

Every number, digest, and rule below is **topic data**: it comes from a signed
document an operator publishes and can re-publish at any time. Nothing about
`tbench` is compiled into the network binaries. Read the live document before
you spend anything; if this page and the live document disagree, the live
document wins.

## Status right now: scoring is on

Live metal scores `tbench`. Read `ctx proof status` (or
`GET /challenge/proof/v1/status`) before you spend — those fields move when the
operator republishes the topic or the host changes, and this page does not pin
them.

What that endpoint has been reading while scoring is on:

- `can_score`: **`true`**
- `open_topics` / `scorable_topics` / `registered_custom` / `custom_ready`:
  contain `tbench`
- `deferred_topics`: **`[]`** — `constraints.params.defer_scoring` is **not**
  set on the live document. A topic listed there (`defer_scoring = "true"`)
  would accept submits as **201 `queued`** and score later; that is not the
  live `tbench` state
- `custom_family_wired` / `baseline_sealed`: **`true`**
- The sealed bar is a **stub baseline 0.5** (operator-noted). Beat it by
  `epsilon_rel` on `success_rate`. Do not copy a digest from this page.

The short-task allowlist is **operator-side** (pack filter / adaptor hints).
You do not choose the task list; `constraints.task_slice` stays an opaque
runner input. A ready light is permission to try, not a payment guarantee.

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
| `can_score` | `true` while this host can evaluate. `false` is **503** except for a topic listed in `deferred_topics` |
| `open_topics` | contains `tbench` — otherwise the topic is not published and a submit is **400** `unknown topic` / `topic is not open` |
| `scorable_topics` | contains `tbench` while the topic is being scored (live metal: yes) |
| `deferred_topics` | empty while scoring is on. If `tbench` appears here, submits answer **201 `queued`** and are not scored yet |
| `queued_submissions` | how many rows are already waiting across topics |
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
| `constraints.firecracker_required` | `true` | A run without the host's sister-guest attestation is not evidence: **503**, no row, no host fallback |
| `constraints.params.baseline_runner` | an in-guest runner id | Selects the **experiment VM** path: one dedicated Firecracker VM per paid job, created for the job and stopped after it (destroyed once it scored; kept stopped on the operator's host when the run failed, never reused) |
| `constraints.params.experiment_pack_digest` | a `sha256:` pin | The operator's experiment pack, re-hashed by the host before any jail. Not yours to supply |
| `constraints.params.miner_byok` | `OPENROUTER_API_KEY` | You bring the model key — see § 4 |
| `constraints.params.defer_scoring` | unset today | When `"true"`, the topic is in `deferred_topics` and submits stay **`queued`**. Absent on the live document while scoring is on — confirm with `ctx proof topics` |
| `flops_budget` | `2e18` | Topic document field. **Not** a reject gate on `tbench`: the host ignores `declared_flops` vs measured FLOPs |
| `eval_executor.max_proof_deadline_s` | `7200` | Your run is cut at this wall clock; a cut run is **503** with the run's `stdout_tail` |
| `payout_mode` | `discovery` | Pass floor plus novelty pool — see § 7 |
| `baseline` | sealed | `script_sha256` + `metrics_commitment`, seed `42`. You never see the recipe, only the commitments |
| `status` | `open` | Only `open` accepts submits |

`GET /v1/proof/topics` never returns holdout records, and `holdout_commitment`
is a commitment, not data. There is nothing to read there.

## 3. Build and upload the artefact

`tbench` is a `custom` topic: **upload the uncompressed tar** (≤5 MiB) with
`ctx proof submit --artifact recipe.tar`. `artifact_uri` is optional compat
— a miner-hosted locator the runner can still fetch. A submit with neither
an upload nor a URI is a **400** `artifact required` with no row. When both
are sent, the uploaded bytes win.

Artefact identity is **the served file's sha256**, verbatim. Uncompressed
only — no gzip, no zip. Both of these packs work; pick one, hash **that**
file, and serve **that** file. Re-running `tar` later (mtimes, member
order) is a different digest.

Working tree on disk:

```
recipe/
  harness.json      # optional: {"kind":"python","import_path":"agent.agent:Agent"}
  agent/
    agent.py        # class Agent
    __init__.py     # optional
```

**Both layouts** (adaptor looks at `$PROOF_ARTIFACT_DIR/agent` then
`$PROOF_ARTIFACT_DIR/recipe/agent`):

```bash
# A — prefix `recipe/` in the archive (README preferred)
tar -cf recipe.tar recipe/
# unpack: $PROOF_ARTIFACT_DIR/recipe/agent/agent.py

# B — contents at tar root
tar -cf recipe.tar -C recipe .
# unpack: $PROOF_ARTIFACT_DIR/agent/agent.py

sha256sum recipe.tar
# serve that exact file at https://…/recipe.tar
```

A double wrap (`recipe/recipe/agent`), `agent.py` sitting beside
`harness.json` with no `agent/` package, or `recipe/run.sh` with no agent
dir, does not resolve as this Python Agent (the last is a **script
harness**). Evaluate then fails closed — no Terminus-2 fallback.

The guest unpacks that tar under `$PROOF_ARTIFACT_DIR`. Evaluate attaches
your **custom Python agent** (primary), a Harbor `BaseAgent` subclass, a
`harness.json` kind, or a `run.sh` script — not a silent copy of the
operator's `terminus-2`. You are not required to ship Terminus-2. A
**prompt-only subclass of `terminus-2`** (same agent, your prompt) is not
an eval short-circuit: inspect does not fail it. That path only **runs**
when the guest Harbor overlay exposes Terminus-2 as an importable class
you can subclass (Harbor is an operator overlay, not in this repo). Name
that class in `harness.json` / `import_path`. Custom Python `class Agent`
does not depend on that overlay and is the primary path.

Harbor's `-a` / `--agent` accepts a built-in name or a Python import path
(`module.path:ClassName`); it does **not** take a filesystem path. The
adaptor therefore imports your class from the artefact (custom Python is
wrapped as `proof_python_agent:ProofPythonAgent`). Resolution order:
`harness.json`, then `$PROOF_ARTIFACT_DIR/agent`, then
`$PROOF_ARTIFACT_DIR/recipe/agent`, then `run.sh`. A `recipe/run.sh` with no
agent dir is scored as a **script harness**, not as the topic agent. The
script must leave Harbor jobs with measured `verifier_result.rewards.reward`;
a self-written `$PROOF_OUTPUT_DIR/report.json` is **not** a score. Inspect
ticks `no_eval_short_circuit` / `no_tb4_hardcoding` on **cheat markers**, not
on the rule ids. Naming those ids in a README or comment is not a fail.
What fails: `skip_eval`, `skip_verifier`, `always_pass_eval`,
`short_circuit_eval` (short-circuit) and `tb4_answers`, `hardcoded_tb4`
(tb4 hardcoding).

### Minimal Agent example

This is the constructor / `run` miners ask for. It is **custom Python** —
you are **not** required to subclass Harbor `BaseAgent` (or Terminus). Name
the class `Agent`. If you do subclass Terminus — including a prompt-only
`terminus-2` subclass — import it from the **guest Harbor overlay** (not
from this repo) and point `import_path` at that class
(`…:ImprovedTerminus`). If that overlay does not expose a subclassable
Terminus-2, evaluate fails at import rather than scoring; use custom
Python `class Agent` instead. Keep one primary example here.

`harness.json`:

```json
{"kind":"python","import_path":"agent.agent:Agent"}
```

The fixture-minimal class (same shape as
`deploy/guest/runners/rlm_fc_in_guest_harbor/tests/fixtures/python_agent/agent/agent.py`).
The constructor may be omitted, empty, or `*args, **kwargs` (Harbor may pass
`logs_dir` and other kwargs; the adaptor binds Harbor's form or falls back
to no-args):

```python
class Agent:
    def __init__(self, *args, **kwargs):
        pass

    def run(self, instruction, environment=None, **kwargs):
        return "ok"
```

When you run a **real terminal command**, prefer **async** `run`. Harbor's
environment API is `await environment.exec(<str>)` → `ExecResult` (stdout,
stderr, return_code) — the command is a positional string, not a keyword:

```python
class Agent:
    def __init__(self, *args, **kwargs):
        pass

    async def run(self, instruction, environment=None, context=None):
        if environment is None:
            return None
        result = await environment.exec("pwd && ls -la")
        # Score is Harbor verifier rewards under $PROOF_WORK_DIR/harbor-jobs.
        # Do not write $PROOF_OUTPUT_DIR/report.json — that path is refused.
        return getattr(result, "stdout", None)
```

`run` may be sync or async. The return value is **not** the Proof score:
leave measured rewards under harbor-jobs. Returning `"ok"`, `None`, or the
command's stdout is honest for this minimal example.

Pack, hash, and serve **that exact file** (either layout):

```bash
tar -cf recipe.tar recipe/
# or: tar -cf recipe.tar -C recipe .
sha256sum recipe.tar
# upload that exact file (preferred), or serve it at artifact_uri
```

Submit sketch. Pass `--openrouter-api-key` (never printed) or
`--env OPENROUTER_API_KEY`. Hotkey signing, nonce, and hex rules:
[proof.md § 2](./proof.md#2-submit-a-reproducible-experiment).

```bash
export OPENROUTER_API_KEY=sk-or-…
ctx proof submit \
  --secret-file /path/to/hotkey.sk \
  --topic-id tbench \
  --artifact recipe.tar \
  --claim "raised first-15 success_rate over the sealed baseline" \
  --env OPENROUTER_API_KEY
# or: --openrouter-api-key "$OPENROUTER_API_KEY"
# compat: --artifact-uri https://example.org/recipe.tar --artifact-digest <sha256>
```

Custom Python `run(instruction, …)` need not subclass Harbor `BaseAgent`.
Agents run with **network on** (OpenRouter / the topic's BYOK). The guest
eval path does not apply Harbor `network_mode=no-network` to Docker — that
mode is unsupported on this runtime and blocked model calls. Task containers
use the default Docker bridge; the Firecracker TAP is still allowlisted on
the host.

The scored task slice is the operator's **default short-task allowlist**
(tasks that finished under one hour on retained n15 x0017):
`cargo-flight-dispatch`, `embedding-drift-monitor`, `bun-sourcemap-leak`,
`fin-saccr-rwa`, `foodstuff-beta-activity`, `atrx-vep-crispr`. Hour-plus
Harbor ids are excluded (`biped-contact-dynamics` ~5.2h, `formal-crypto`
~2.1h, `cad-model` ~1.2h, `data-anonymization` ~1.1h), as are tasks that
broke that run until they are fixed (`batched-eval-parity` no-network,
`ctr-optimization` / `cumulative-layout-shift` EnvStartTimeout,
`distributed-dedup` tmux, `coq-block-bound` wall cut). You do not choose
the task list; `constraints.task_slice` remains an opaque runner input. A
verifier image that lacks `pytest` on PATH scores 0 rather than failing the
trial — that is an operator image hole, not a miner contract
(`biped-contact-dynamics` and `cad-model` hit this on n15 and stay out of
the default pack until that image is proven).

Env the run sees: `PROOF_SEED`, `PROOF_MODEL_PIN`, `PROOF_TASK_SLICE`,
`PROOF_PARAM_*`, `PROOF_PACK_DIR`, `PROOF_ARTIFACT_DIR`, `PROOF_OUTPUT_DIR`,
`PROOF_WORK_DIR`, and miner BYOK under `PROOF_MINER_ENV_DIR`.

- Prefer `ctx proof submit --artifact recipe.tar` (gateway intake cap **5 MiB**).
  Re-running `tar` later produces different bytes (mtimes, member order) and
  therefore a different digest.
- URI-only compat: serve **that exact file** at `artifact_uri` and keep it.
  The guest streams it under a hard **64 MiB** cap (not the gateway upload
  cap). The host re-hashes exactly the bytes the runner fetched before it
  boots your guest, and refuses gzip, non-tar bytes, an archive with no
  file content, or bytes that do not match your `artifact_digest`.
- A digest of nothing — the sha256 of zero bytes or of an empty tar — is a
  **400** with no row, whatever case you spell it in. Ship a recipe, not a
  weight dump.

Your code runs **inside a Firecracker guest the host boots**, against the
operator's pinned experiment pack. You cannot produce the sandbox attestation
yourself and you do not need to: the host — not the harness, not the RLM —
stamps it. A guest-measured `flops_used` may appear on the verdict as
telemetry. **`tbench` does not reject or cheat-code on `declared_flops` vs
measured FLOPs** — anti-cheat is the signed topic checklist, sister
attestation, BYOK env, and hotkey signature, not a hardcoded TFLOP budget.

## 4. The model key is yours (BYOK)

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

With `ctx`, pass `--openrouter-api-key` (never printed) or bare
`--env OPENROUTER_API_KEY` to read it from your shell so it never lands in
your history. Exporting the variable alone does **not** attach it — that
would send a leftover key to every topic and every `--gateway`.

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
  accepted until your row is terminal — so a topic that queues your row today
  still has your key when the operator drains it. If the host loses it before
  the run, your row stays `queued` and the drain answers **503**; it never
  scores your work on the operator's account.

One thing to be clear about, because guessing here costs money:
**`X-Lium-Api-Key` is not this key, and is not identity.** It is the Lium
header from [proof.md](./proof.md), used by the Lium harvest families for
**compute**. It never carries a model-provider key and it never authenticates
you. `env` pays for inference; `X-Lium-Api-Key` pays for machines.

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
  --json
```

That prints `miner_hotkey`, `hotkey_signature`, `submit_nonce`, the exact
`manifest` (empty training lists — `tbench` has no training step), and
`domain` (`base-proof-submit-v1`) without posting anything.
Then submit:

```bash
export OPENROUTER_API_KEY=sk-or-…
ctx proof submit \
  --secret-file /path/to/hotkey.sk \
  --topic-id tbench \
  --artifact-digest <sha256 of recipe.tar> \
  --artifact-uri https://example.org/recipe.tar \
  --claim "raised first-15 success_rate over the sealed baseline by 0.08" \
  --env OPENROUTER_API_KEY
```

`--openrouter-api-key` / `--env` is not part of `ctx proof sign`: the key is
posted beside the signature, never inside it, so the signed bytes are the
same with or without it.

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
| `artifact_digest` | yes | sha256 of the exact file you upload (or serve); not the digest of nothing |
| `artifact` (multipart) | **preferred** | Uncompressed tar ≤5 MiB. `ctx proof submit --artifact` |
| `artifact_uri` | optional | Compat locator; omit when you upload. Neither upload nor URI → **400** `artifact required` |
| `claim` | yes | One English sentence of what improved. Signed |
| `declared_flops` | no | Optional, default `0`. Still bound into the signature if you send it. **Ignored as a scoring gate** on `tbench` |
| `manifest.train_content_hashes` / `manifest.train_dataset_ids` | **no** | `tbench` is a custom agent topic with no training step. Omit `--train-dataset` / `--train-hash`. Do not invent a harness id as a fake corpus. Signed (empty lists are fine) |
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
Sign again with a fresh nonce only when you intend a **new** run: while
scoring is on, that is a second **201** and a second paid evaluation, not
the existing row. The **200** “already queued / already submitted” answer
is the deferred path only (`tbench` in `deferred_topics`).

`tbench` does not require training evidence. An empty manifest is a clean
submit: `ctx` will not ask you for `--train-dataset`, and the host will not
reject you for omitting it. Do not invent a harness id as a fake dataset.
Holdout overlap in a *declared* manifest is still contamination: a persisted
**`rejected`** row with no rent (immediate while scoring is on, at drain time
only if the topic is back in `deferred_topics`).

## 6. Watch the row

```bash
ctx proof show <id>
ctx proof show <id> --wait
# same as:
curl -sS https://gateway.cortex.foundation/challenge/proof/v1/submissions/<id>
```

| `state` | Meaning on `tbench` |
|---------|---------------------|
| `queued` | Accepted and stored, **not evaluated**. Only when the topic is in `deferred_topics` (`defer_scoring = "true"`). No rent, no VM, no judge call, no mass. Not an in-progress score: a live (non-deferred) submit waits for scoring and the **201** is already `awaiting_admin`, `rejected`, or `champion`. `ctx proof show --wait` is for a deferred row |
| `awaiting_admin` | Clean pass, mass recorded. The operator audit is informational |
| `rejected` | A gate failed: contamination, unreproduced claim, a red checklist item. Pre-eval rejects spend nothing: **evaluate is skipped**, so `flops_used = 0` / `custom_value = null` is expected. FLOP accounting is **not** a reject gate on `tbench` |
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
evidence-less item is a persisted `rejected` row with **no spend**. Evaluate
never runs in that case — `flops_used = 0` and `custom_value = null` are
expected, not a missing Harbor trial. This inspect is Proof Firecracker
(the in-guest Harbor adaptor), not Lium. The rule texts in the live topic
are the contract; the ids published today are:

| Rule id | What it holds you to |
|---------|----------------------|
| `same_seed` | Every paid call and every scored episode uses the topic baseline seed. No per-miner reseeding |
| `no_tb4_hardcoding` | Your harness must not hardcode task ids, answers, fixtures, or success paths. Only the signed topic carries `task_slice`. Inspect fails on cheat markers `tb4_answers` / `hardcoded_tb4`, not because a README names this rule id |
| `no_eval_short_circuit` | The evaluator, the metric path, and the sandbox are untouched — no short-circuiting the checklist or the scoring. Inspect fails on `skip_eval` / `skip_verifier` / `always_pass_eval` / `short_circuit_eval`, not because a README names this rule id. A prompt-only `terminus-2` subclass is not a short-circuit; whether it **imports** depends on the guest Harbor overlay (not in this repo) |
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
| **201** (scored) | Well-formed submit while scoring is on: the host scores before it answers, so the row is already `awaiting_admin`, `rejected`, or `champion`. **201 `queued`** only if `tbench` is back in `deferred_topics` | yes |
| **200** existing row | Same artefact + hotkey, freshly signed, **only** while `tbench` is in `deferred_topics` (queued or already drained). While scoring is on, a fresh nonce is a new **201** and a second paid run | existing row |
| **400** `unknown topic` / `topic is not open` | `tbench` is not published, or is outside its epoch window | no |
| **400** `artifact required` | You left both the upload and the locator out. `tbench` is `custom` | no |
| **400** `artifact is not a tar archive` / gzip / no file content | The upload is not an uncompressed tar with file bytes | no |
| **400** `artifact_digest is the sha256 of empty input …` | You hashed nothing, or an empty tar | no |
| **400** invalid `miner_hotkey` / `artifact_digest` | Not exactly 64 lowercase hex. The host never normalises a hex field | no |
| **401** `hotkey_signature required` / `invalid` | Missing signature, or a `claim`, `declared_flops`, `manifest`, or nonce that differs from what you signed | no |
| **401** `submit_nonce required` / `invalid` / `reused` | Missing, not 64 lowercase hex, or a replay. Sign again with a fresh nonce | no |
| **503** `custom metric … has no registered runner` / `not wired` | `tbench` is not in `registered_custom` / `custom_ready` | no |
| **503** empty `eval_image_digest` / unsealed baseline / missing judge offer | The host cannot score. Fail-closed, never a sim fallback | no |
| **503** `proof deadline … exceeded` | Your run did not finish inside `max_proof_deadline_s`; the body carries `stdout_tail` | no |
| **201** `rejected` + contamination | You declared a holdout shard / corpus id in `manifest` (immediate while scoring is on; at drain time only if the topic is back in `deferred_topics`) | yes, rejected |
| **201** `rejected` + red checklist | A topic rule failed on your artefact — **before any paid inference**. Evaluate is skipped on purpose: `flops_used = 0`, `custom_value = null`, and `cheat_codes` such as `other` are the expected shape of that skip, not a Lium/harvest miss | yes, rejected |

Never commit your OpenRouter key or `LIUM_API_KEY`, and never hand anyone a
mnemonic or a challenge signing key. If something fails, see
[troubleshoot.md](./troubleshoot.md); the generic Proof contract is
[proof.md](./proof.md).
