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

**Gateway:** [https://network.cortex.foundation](https://network.cortex.foundation)  
**CLI:** `ctx proof topics`, then `ctx proof submit` (install:
[README](./README.md))  
**Pin:** [`config/proof-pin.toml`](../../config/proof-pin.toml)  
**Eval image:** `ghcr.io/cortexlm/proof-eval@sha256:78b614a1f51ce5dd80076c4e343a2b31b85d6c36025e02836cb83929867e7009`

You submit **claim + code + FLOPs + artifact** against a topic. You do
not bind an offer id. The digest-pinned `proof-eval` image (harvest boots
it) calls the master's `InferenceOffer` as the RLM **judge** backend.
No baked Qwen; architecture is not an HF id check.

Miner pays Lium (`LIUM_API_KEY` / `X-Lium-Api-Key`).

If `eval_image_digest` is empty the host answers **503**. That is fail-closed,
not a sim fallback. The pin currently carries the digest above. Do not invent
a different one.

## How Proof works

The **unit of work is a signed topic**, not a frozen catalog in git. An
operator publishes a research problem (admin `POST` can inject one at any
time). You submit **against that `topic_id`**:

1. a **claim** (natural language: what improved, under which constraints)
2. a **code artifact** (reproducible recipe — code + lockfile / entrypoint)
3. **declared FLOPs** (must be `≤ topic.flops_budget`)

The artifact contract requires a recipe reproducible under the topic's FLOP /
wall budget. **A weight dump alone is not an artifact.** The intended judge
re-runs that recipe and a separate harness measures holdout loss or throughput.
The current Python image does not yet implement arbitrary recipe reproduction;
its static checks and model measurements are only part of that design.
Holdout records are not included in public topic responses.

`GET /challenge/proof/v1/status` shows `can_score`, `eval_backend`,
`force_sim`, `live_harvest_wired`, `baseline_sealed`, public pin `inference`
judge defaults (provider, model, mode, token caps — never the origin), the
public RLM judge `inference_offer` (id, kind, mode, model_ref, token caps,
commitment, status), and the public `eval_executor` — the `1x` Lium machine
class your recipe is re-run on (template, shape, proof deadline, commitment,
status). It never leaks holdout records, teacher hosts, origins, or keys.

Muon, token superposition, and “decentralized training without InfiniBand”
are *examples* of solutions or of topics — they are not the product.

Pass gates (reproduced, no contamination, under budget, beat epsilon) are
fail-closed. The implemented **payout calculation** after a pass depends on the
topic's `payout_mode`. The score is the **sum of per-topic** masses over
currently `open` ids, not a mean of binary lattices. A skipped topic is 0 on
that topic. Zero open topics → the host cannot score (`503`), not a paid 0.

## 0. Can this host score right now?

```bash
ctx proof status
# same as:
curl -sS https://network.cortex.foundation/challenge/proof/v1/status
```

`GET /challenge/proof/v1/status` shows `can_score`, `eval_backend`,
`force_sim`, `live_harvest_wired`, `baseline_sealed`, `eval_image_digest`,
public pin `inference` (no origin), public RLM judge `inference_offer`,
public `eval_executor` (plus the pin `executor` ceilings), and
`open_topics`. It never leaks holdout records, teacher hosts, origins, or
keys. `GET /challenge/proof/v1/proof/executor` shows the executor alone with
`ready` and a `reason` when it cannot rent.

`can_score: false` means submits **503**. Nothing is stored and nothing is
rented.

| Status field | What it means |
|--------------|----------------|
| `eval_image_digest` | Must be a `sha256:…` pin (live pin is `sha256:78b614a1…`). Empty → **503** |
| `inference_offer` | Public RLM **judge** backend (id, kind, mode, model_ref, token caps, commitment, status). Missing/closed/misconfigured → **503**. You do not pass an offer id |
| `eval_executor` | Public `1x` **executor**: the Lium machine class your recipe is re-run on (`lium_template_id`, `machine_shape`, `max_proof_deadline_s`, commitment, status). Your recipe must finish inside `max_proof_deadline_s` (≤ pin ceiling 7200 s; a topic may name a shorter one) on **one** GPU — the host never rents more. Missing/closed/any shape but `1x` → **503**. You do not pass or rent it |
| `open_topics` empty | No currently `open` signed topic with a sealed baseline → **503** |
| `scorable_topics` | Open topics whose scorer is wired on this host. An open topic **not** listed here (a `custom` topic whose runner is not registered or not wired) answers **503** |
| `registered_custom` | Custom metric ids with a registered runner. Nothing is compiled in; ids come from signed topics |
| `baseline_sealed: false` | An open topic without `script_sha256` + `metrics_commitment` → **503** |
| `live_harvest_wired: false` | Live RLM harvest is not connected → **503** |

## 1. List open topics

```bash
ctx proof topics
curl -sS https://network.cortex.foundation/challenge/proof/v1/proof/topics
curl -sS https://network.cortex.foundation/challenge/proof/v1/proof/topics/dt-no-ib-v0
```

Topics are **operator-published** and can be **injected at any time** (operator
`POST /challenge/proof/v1/admin/proof/topics`). There is no catalog in git.
`ctx proof topics` is the live list.

Each topic is a signed document. Read at least:

| Field | What it means for you |
|-------|------------------------|
| `id` | The `topic_id` you submit against |
| `statement` | The research problem in English |
| `constraints` | Fabric / comms caps the eval image enforces (it never trusts the claim). Custom topics may add `firecracker_required`, `model_pin`, an opaque `task_slice`, and `params` |
| `checklist` | Anti-cheat rules `[{id, text}]` the topic's RLM ticks over your artefact **before any paid inference** |
| `eval_executor` | Executor commitment (`require_offer_commitment`, tighten-only `max_proof_deadline_s`) |
| `metric.family` | `nll` \| `throughput` \| `custom` (`metric.custom_id` names the metric; it is topic data) |
| `flops_budget` | Hard cap. `declared_flops` must be `≤` this |
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
FLOP / wall / contamination gates.

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

Build a recipe the judge can re-run: code, lockfile, and entrypoint, under
the topic's FLOP (and for throughput, wall) budget. Hash that tree. That hash
is `artifact_digest`. `artifact_uri` is a locator (git URL, object URL) for
the same bytes: optional on `nll` / `throughput` (the image fetches by
digest), **required on custom topics** (the topic's runner fetches from it
inside the topic VM and checks the digest; without one the submit is a 400).

The **claim** is one English sentence of what improved. The RLM re-runs the
code against the public split and checks the claim against those public
numbers. A claim the code cannot support is `unreproduced_claim` / reject.

```bash
ctx proof status          # can_score, inference_offer, eval_executor, eval_image_digest
ctx proof topics          # pick an open topic_id; read flops_budget + payout_mode

ctx proof submit \
  --hotkey <64-hex hotkey> \
  --topic-id <open topic id> \
  --artifact-digest <sha256 of the recipe> \
  --claim "beat sealed baseline holdout NLL by 0.04 at 1.2e18 FLOPs" \
  --declared-flops 1500000000000000000 \
  --train-dataset my-mix-v0
```

`--wait` keeps polling until the row is terminal (`awaiting_admin`,
`rejected`, or `champion`).

The same submit with `curl`:

```bash
curl -sS -X POST https://network.cortex.foundation/challenge/proof/v1/submissions \
  -H 'content-type: application/json' \
  -H "X-Lium-Api-Key: $LIUM_API_KEY" \
  -d '{
    "miner_hotkey": "<64-hex hotkey>",
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

`claim` and `declared_flops` are **required**. The control plane scores a
claim against public numbers and checks FLOPs against the topic budget.

Poll `GET /challenge/proof/v1/submissions/{id}`. While `can_score` is
`false` (empty digest, missing/closed RLM judge backend, incomplete pin+topic
judge config, no open sealed topic), submissions answer **503**.

### Required POST JSON

`POST https://network.cortex.foundation/challenge/proof/v1/submissions`

| Field | Required | Shape |
|-------|----------|-------|
| `miner_hotkey` | yes | 64 hex characters (no `0x`) |
| `topic_id` | yes | Open topic id from `ctx proof topics` |
| `artifact_digest` | yes | SHA-256 hex of the recipe bytes |
| `claim` | yes | Non-empty string: NL of what improved |
| `declared_flops` | yes | `u64`, must be `≤ topic.flops_budget` |
| `manifest.train_content_hashes` | yes (array) | Shard hashes you trained on (may be `[]` if you declare dataset ids) |
| `manifest.train_dataset_ids` | yes (array) | Corpus ids you trained on (may be `[]` if you declare hashes) |
| `artifact_uri` | custom topics: yes | Locator for the same bytes as `artifact_digest`; optional on `nll` / `throughput` |

An empty `manifest` (both arrays empty / omitted) is **not** a clean
contamination check. It is `contamination_evidence_missing`: the row is
**rejected** and **no pod is rented**.

## 3. See status

```bash
ctx proof show <id>
# same as:
curl -sS https://network.cortex.foundation/challenge/proof/v1/submissions/<id>
```

| `state` | Meaning |
|---------|---------|
| `awaiting_admin` | Clean pass; mass recorded. Operator audit is informational. |
| `rejected` | Gates failed (contamination, unreproduced claim, NLL miss, red anti-cheat checklist, …). No rent and no paid inference on pre-eval rejects. |
| `champion` | Promoted: operator crown, or automatic on custom topics when a pass beats the current best by `epsilon_rel` with a green checklist. Proof pays on pass, not on a crown. |

Poll `GET /challenge/proof/v1/submissions/{id}` for the verdict envelope
below. While `can_score` is `false`, the POST itself answers **503** and
there is no row to show.

## HTTP 400 vs 503

A **400** is your request. A **503** is the host. Neither rents a pod.
Refusals (**400** / **503**) do **not** persist a submission row.

| Status | When | Stored? | Rented? |
|--------|------|---------|---------|
| **400** `topic_id is required` | Missing `topic_id` | no | no |
| **400** `unknown topic` | `topic_id` not published | no | no |
| **400** `topic is not open` | Draft / closed / outside epoch window | no | no |
| **400** `declared_flops exceeds the topic budget` | `declared_flops > topic.flops_budget` | no | no |
| **400** `artifact_uri is required for custom topics` | Custom topic, no locator | no | no |
| **400** invalid `miner_hotkey` / `artifact_digest` | Not 64 hex | no | no |
| **503** empty `eval_image_digest` | Digest not pinned | no | no |
| **503** zero open sealed topics | Nothing to score against | no | no |
| **503** unsealed baseline | Topic open without both seal hashes | no | no |
| **503** live harvest down / unparseable agent verdict | Host cannot judge | no | no |
| **503** missing / closed RLM judge backend | Live `InferenceOffer` not scoring | no | no |
| **503** missing / closed / non-`1x` executor | Live `eval_executor` cannot rent the `1x` machine | no | no |
| **503** `proof deadline … exceeded` | Your recipe did not finish inside `max_proof_deadline_s`; the body carries the run's `stdout_tail` | no | no (pod torn down) |
| **503** `custom metric … has no registered runner` / `not wired` | The topic's `custom_id` has no runner on this host, or its topic VM is not configured | no | no |
| **201** `rejected` + `contamination_evidence_missing` | Empty manifest | **yes** (rejected) | **no** |
| **201** `rejected` + contamination | Holdout shard / corpus id in `manifest` | **yes** (rejected) | **no** |
| **201** `rejected` + `anti-cheat checklist red` | A topic rule failed on your artefact | **yes** (rejected) | **no** (no paid inference) |

Contamination (including empty evidence) is a **reject, no rent**. It is
not a 400 and not a 503. A red anti-cheat checklist is the same shape: a
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
| `flops_used` / `flops_budget` | u64 | Measured by the judge / runner vs the topic budget. Your `declared_flops` is never the enforced usage figure; on custom topics the runner's measurement is the verdict's usage, over budget is `flops_over_budget`, and over your own declaration is `flops_under_declared` |
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
| `flops_over_budget` | Run spent more FLOPs than the topic budget |
| `flops_under_declared` | Run spent more FLOPs than your `declared_flops` (custom topics: the runner's measurement is held to your declaration) |
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
about it is compiled into the network. If the topic sets
`constraints.firecracker_required`, your code runs only inside a Firecracker
guest under the topic's own VM; if it sets `constraints.model_pin`, every
paid call must name exactly that model; `task_slice` / `params` are opaque
runner inputs the topic defines.

**Anti-cheat checklist — every rule in the topic's `checklist` (current
version) must pass before a single paid inference call is made.** Read the
rule texts in `ctx proof topics`; they are the contract. One red, missing,
duplicated, or evidence-less item is a persisted `rejected` row with no
spend. The rules may be re-versioned by the topic's RLM; the version you were
ticked against is recorded with your row.

`artifact_uri` is required: the runner fetches the bytes from it inside the
topic VM and checks the digest, so a submission the runner cannot retrieve is
a **400** with no row. The runner also measures your run's FLOPs; that
measurement (not `declared_flops`) is what the verdict carries, and it must
stay within both the topic budget (`flops_over_budget`) and your own
`declared_flops` (`flops_under_declared`) — declare what you will use, up to
the budget. The runner may enforce your declaration as a hard cap.

A clean pass that beats the current best (sealed value or reigning best) by
`epsilon_rel` is promoted automatically: the row is `champion` and the
operator archive keeps your artefact, `report.json`, and `checklist.json`
under `{topic_id}/{submission_id}.zip`. Runs on one topic are scored and
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
