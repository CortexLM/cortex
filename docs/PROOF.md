# Proof challenge

Proof is Cortex's research contribution path: the intended output is a
reproducible finding that others can reuse, not just a finished model checkpoint.
Read the [overview](OVERVIEW.md) for the purpose and the
[whitepaper comparison](WHITEPAPER.md) for the proposed shared-research loop.

**Implementation limits:** the Python judge currently performs an authenticated
acknowledgement request and static checks, not the paper's autonomous investigation
and arbitrary recipe reproduction. The service stores submissions in memory and
does not run an automatic reward-leaf emitter. Payout/signing helpers exist, but
readiness checks alone do not establish a complete research-to-payment path.
The rules below describe the current interfaces and scoring functions, not a
claim that these gaps are closed.

Live challenge id: **`proof`**. Emission **8000 bps** (80% of the subnet;
bounty is 2000). This 20%/80% lock is independent of eval digest. Eval
digest `sha256:78b614a1…` is pinned (`ghcr.io/cortexlm/proof-eval`). The
RLM judge backend is the live `InferenceOffer` (not a baked HF proxy). Live
submits still **503** until harvest is wired, a baseline is sealed, and ≥1
topic is open. Do not invent a sha256.
Sum across the two live rows stays 10000. Port **8100** (local probe
**28100**).

Proof is the research-problem challenge. The unit of work is an
**operator-published signed topic**, not a prompt and not an episode. Git
carries global floors in [`config/proof-pin.toml`](../config/proof-pin.toml)
and no topic catalog. Miners submit against `topic_id`. The RLM judge lives
in a digest-pinned eval image (`ghcr.io/cortexlm/proof-eval`). The digest
is pinned; live submits still answer **503** until harvest + sealed
baseline + an open topic are on the host.

## Product rules (do not weaken)

- Topics are dynamic. Muon / token superposition / “decentralized training
  without InfiniBand” are *example solutions or example topics*, never a
  frozen catalog in git.
- A topic may tighten a floor, never loosen it. Floors live in the pin.
- The RLM **judge** lives in a digest-pinned `proof-eval` image. Harvest
  boots that image and both call the live master `InferenceOffer` as the
  **judge backend** (not a miner training proxy, not an HF bake). Miners
  do **not** bind submit to an `offer_id` as a train target; they still
  post **claim + code + FLOPs + artifact** against a topic. No baked Qwen;
  architecture ≠ HF id stays retired. Pin ceilings / modes / commitment
  bound the **judge** offer; missing / closed / judge down → **503**. Topic
  optional field: `require_judge_offer_commitment` (not a miner-facing
  bind). The pin also carries complete `[inference]` judge defaults
  (`provider`, `base_url` empty = secret-backed, `model`, `mode`,
  `max_input_tokens`, `max_output_tokens`) plus schema v1, `allowed_modes`,
  token ceilings, and `inference_offer_commitment_alg = sha256`. A topic's
  signed `inference{…}` may **override** provider/model/mode and may **only
  tighten** token caps vs those pin defaults. It must **not** redirect
  origin: `config_commitment` hashes config knobs **and** `provider.base_url`;
  a topic that spoofs `inference.base_url` fails closed (**503**) before
  lattice. `require_judge_offer_commitment` (64-hex) pins the live judge
  offer's `config_commitment` — mismatch → **503**. Missing or misconfigured
  judge resolve → publish **400** (open topic) / score **503**. Empty pin
  `model` / `base_url` is pre-launch fail-closed (like an empty digest).
  Auth is `PROOF_INFERENCE_API_KEY_FILE`, staged into the harvest pod as
  `teacher.env` (`OPENAI_API_KEY`) — never git, never `/v1/status`. Missing
  key on a live open offer → **503**. `proxy_model` stays empty.
  Live score has **no HF bake**: harvest must stage operator-provided
  local weights (`PROOF_PROXY_MODEL_DIR`) and holdout shard bytes
  (`PROOF_HOLDOUT_STORE/<content_sha256>`). The pinned image does not
  contain `/opt/proof-eval/holdout` or a valid default proxy id. Missing
  either → `can_score=false` / submit **503** (do not rent). After this
  source fix, republish `proof-eval` and re-pin the new digest before the
  next 1× GPU rent — do not invent a sha256.
- **Where** the image runs is the live `EvalExecutorOffer` — a sibling of
  the judge `InferenceOffer`, never the same document. The pin carries only
  ceilings: `eval_executor_schema_version = 1`, `gpu_class = "1x"`,
  `max_proof_deadline_s_ceiling = 7200`, optional
  `allowed_lium_template_prefixes` (`["proof-eval-"]`, the digest-scoped
  harvest template name `proof-eval-<12 hex>`), and
  `eval_executor_commitment_alg = sha256`. The live offer
  (`PROOF_EVAL_EXECUTOR_OFFER_FILE`; `POST /v1/admin/proof/executor` to
  rotate or close) names `offer_id`, `lium_template_id`, `machine_shape`,
  `max_proof_deadline_s` (≤ ceiling), `eval_image_digest` (must equal the pin
  when non-empty), `config_commitment` = sha256 of the canonical public knobs,
  and `status`. `lium_template_id` is the **digest-scoped template name**
  (it must carry the pinned digest's 12-hex prefix); harvest resolves it
  through the digest-bound resolver, which reuses a listed template only when
  its image is `eval_image@digest` and otherwise creates one bound to it. A
  raw Lium template UUID is **refused** (offer or override, under any
  allowlist) because the provider would rent it verbatim with no image
  check; a pin with no digest binds no executor at all. Every field is
  public: `GET /v1/status` (`eval_executor`, pin `executor`) and
  `GET /v1/proof/executor` show it whole. Missing / closed /
  `machine_shape ≠ 1x` → `can_score=false` → **503** on the Lium path (sim
  rents nothing and does not consult it). Harvest rents exactly that
  template at exactly `1x` — a rent that would upsize to a whole host
  (`rent_gpu_count ≠ 1`) aborts before the rent — and holds the run to the
  resolved deadline: the deadline **is** the pod-side `timeout` (never
  clamped below it by the host's `PROOF_EVAL_TIMEOUT_SECS` fallback, whose
  default equals the ceiling) and the harvest wait is deadline + grace. A run
  the wrapper cut (`exit=124`, or `137` after the full budget) is a **503**
  carrying the pod's `stdout_tail`, never a zero; a `137` before the deadline
  is reported as an external SIGKILL (e.g. OOM), not as the deadline. A
  topic may only tighten: `eval_executor.max_proof_deadline_s` (shorter) and
  `eval_executor.require_offer_commitment` (64-hex pin of the live offer,
  not a miner bind). There is **no per-topic `machine_id`** (publish
  reject). Operator hot-swap without a rebuild: `PROOF_HARVEST_TEMPLATE_ID`
  / `PROOF_HARVEST_GPU_COUNT` / `PROOF_HARVEST_DEADLINE_SECS` replace the
  offer's values; the pin ceilings still bind, an unparseable or
  out-of-ceiling value refuses the rent rather than clamping, and a topic
  that pins `require_offer_commitment` **refuses** any override that changes
  the template or deadline (it approved the offer's configuration, not the
  operator's). The run request and the scored row stamp the commitment of
  the configuration that actually ran (`executor_commitment`) next to the
  offer's (`executor_offer_commitment` on the request); they differ only when
  a topic tighten or an override changed the offer's knobs. Ceremony:
  `cargo run -p xtask -- proof-executor-offer --offer-id <slug> --max-proof-deadline-s <s> --out <off-git path>`.
  Isolation: the control-plane host runs neither the eval image nor the RLM
  judge; the harvest is the **only** path to the rented GPU, and the executor
  offer names a remote machine class, never a host process. The contract is
  challenge-agnostic — no topic ids, benchmark names, or model names are
  compiled in; the rent plan carries the topic id only as scope for a
  topic-scoped attach.
- A baseline must be sealed (`script_sha256` + `metrics_commitment`) to
  open. Nobody is paid for beating a number nobody measured.
- 8000 bps is split equally across currently `open` topics. Each topic then
  pays under its `payout_mode`:
  - **`wta`:** among miners with `pass=true` this epoch, the best primary
    (direction-aware) takes 100% of that topic's mass. Exact ties split
    equally. Non-winners get 0 on that topic.
  - **`discovery`:** `pass_floor_share_bps` (default 3000) of the topic pool
    is split equally among verified passes (research-cost reimbursement).
    `novelty_pool_share_bps` (default 7000; the two shares must sum to 10000)
    is weighted by improvement delta vs the sealed baseline and vs a
    previous accepted champion if any. A near-duplicate of another accepted
    artifact keeps the floor and gets 0 novelty weight.
- Global miner proof score = **sum** of per-topic masses, not a mean of
  binary lattices. Skipped topic = 0 on that topic. Empty open set →
  `NoScore(ChallengeInternal)`, not a paid 0.
- `custom` metric family: the `custom_id` is **topic data**
  (`[a-z0-9][a-z0-9_-]{1,63}`). There is no compiled-in list of metrics.
  Any well-formed id may **draft**; a custom topic may **open** only when a
  runner is registered under its id on the host (`400` otherwise), and the
  runner registry is **empty by default** — an unregistered or unwired id is
  **503 at score**, never a harvest fallback. No benchmark, model, rule
  list, or repository is compiled into this repository; the first live
  topic is a signed document its RLM sets up, not a code branch.
- Anti-cheat rules are a **vector carried by the signed topic**
  (`checklist: [{id, text}]`), re-versioned by the topic's RLM into the
  database. Every rule of the current version is ticked with evidence
  **before any paid inference**; one red, missing, duplicated, or
  evidence-less item is a persisted reject with no spend.
- The RLM runs **inside a VM attributed to its topic**, reached only through
  the orchestrator boundary (`TopicVmOrchestrator`). Miner code runs in a
  Firecracker guest under that VM when the topic says
  `constraints.firecracker_required`. The control plane never runs RLM
  logic and never hands the VM a host path or a secret; an unwired
  orchestrator is a **503**, not a host-local fallback.
- `PROOF_FORCE_SIM` is CI/local opt-in only. Never a fallback. Forbidden on
  droplet overlays. Under sim, a sealed topic scores with harness numbers
  relative to the seal (`sim_win_document`); skill-only `sim_document`
  cannot beat a real ~0.29 NLL baseline. `PROOF_SIM_STUB_WIN` is a leftover
  no-op. Resealing staging to `BASELINE_SKILL=0.40` (NLL ≈ 2.94) is an
  operator lane, not this binary.
- No Modal. No secrets, hosts, holdout records, or teacher endpoints in git.

## Publish a research topic

1. Write a YAML or JSON draft with `id`, English `statement`, `payout_mode`
   (`wta` | `discovery`), `validation.{score_on,accept_if,reject_if}`, and
   `metric`. Do not put it in git.
2. Fill a holdout and sign with the `proof` row key (never commit the secret):

```bash
cargo run -p xtask -- proof-topic \
  --input /root/.base-secrets/proof/agent-harness-improve-v0.yaml \
  --secret ~/.base-secrets/proof.sk \
  --synthetic \
  --out /root/.base-secrets/proof/agent-harness-improve-v0.signed.json
```

`--synthetic` is for local/dev. Production fills `--holdout` from the
operator holdout file (`xtask proof-holdout --topic-id <id> …`) so the
commitment matches records the host will unseal.

3. Seal the baseline (`script_sha256` + `metrics_commitment`) before setting
   `status: open`. A draft may be unsealed; an open topic may not.
4. `POST /v1/admin/proof/topics` with the signed document and the operator
   bearer. `GET /v1/proof/topics` lists open ids (never holdout records).

Ship order: control plane (payout schema) → proof-eval image + digest pin
(this pin) → holdout/baseline files on the host → open topics. Operator
path: [`deploy/scripts/proof-operator-path.sh`](../deploy/scripts/proof-operator-path.sh).
Empty digest stays 503 (never invent a sha256).

## Metric families

| Family | Primary | Win |
|--------|---------|-----|
| `nll` | `holdout_nll` (min) | Beat sealed AdamW by `epsilon_nll >= 0.02`. Per-split NLL regress `<= epsilon_topic_max_regress >= 0.05` |
| `throughput` | `tokens_per_sec` (max) or `step_latency_ms` (min) | Requires `flops_budget` **and** `wall_budget_s`. `epsilon_rel >= 0.05`. Quality floor: `holdout_nll <= sealed_nll + quality_floor_nll` (≤ pin 0.02). Eval image enforces comms (e.g. 12.5 Gbit/s); it does not trust the claim |
| `custom` | `custom_id` minted by the topic | `primary` (topic name, `max` or `min`), `epsilon_rel > 0` relative to the sealed value. Scored by the runner registered under `custom_id` (empty registry by default → **503**); the checklist gate runs first |

Holdout: 120 records, stratified 24 each across `web_ood`, `code_ood`,
`math_ood`, `longctx` (8k–32k), `multilingual_ood`. `canary_offpath` is
off-score and never in the 120.

Ceremony: `cargo run -p xtask -- proof-holdout --topic-id <id> …` and
`cargo run -p xtask -- proof-topic --input … --secret …`. Operator path:
[`deploy/scripts/proof-operator-path.sh`](../deploy/scripts/proof-operator-path.sh).
Trust-root keygen is the throwaway owner path in
[`config/CEREMONY.md`](../config/CEREMONY.md).

## HTTP

- `GET /health`, `GET /v1/status` — `can_score`, `eval_backend`, `force_sim`,
  `live_harvest_wired`, `baseline_sealed`, `open_topics`, `scorable_topics`
  (open topics whose scorer is wired on this host; `can_score` is true when
  it is non-empty), `registered_custom` (custom ids with a runner), public
  pin `inference` judge defaults (no origin), public `inference_offer` (RLM
  judge backend), public `eval_executor` (live `1x` executor offer) and pin
  `executor` ceilings. Never leak origins, keys, or holdout records.
- `GET /v1/proof/topics`, `GET /v1/proof/topics/{id}`
- `GET /v1/proof/executor` — always **200**: `eval_executor` (public offer or
  `null`), `ready`, `reason` when not ready, and the pin ceilings.
- `POST /v1/admin/proof/topics` — operator bearer; verify sig/schema/floors/seal before `open`
- `POST /v1/admin/proof/executor` — operator bearer; body is the offer
  document. Pin-validated (**400** keeps the previous offer); `status: closed`
  takes the executor down live. In-memory until restart, like submissions —
  update `PROOF_EVAL_EXECUTOR_OFFER_FILE` to persist.
- `POST /v1/submissions` **requires** `topic_id`. Missing/unknown/not-open →
  **400**. Miners do **not** bind the judge offer or the executor offer. Zero
  open / unsealed baseline / empty digest / missing or closed RLM judge
  backend / missing, closed, or non-`1x` executor / agent down / run cut at
  the proof deadline / no registered or wired runner for the topic's
  `custom_id` → **503**. Refusals must **not** persist rows. Scored rows
  stamp `executor_offer_id` + `executor_commitment` next to the judge
  `inference_offer_id` + `config_commitment`.
- A pass that the family scorer crowns (custom: green checklist and
  `primary >= bar * (1 + epsilon_rel)` direction-aware, bar = sealed value or
  reigning best) persists as `champion`; other passes stay `awaiting_admin`.
- Submit fields miners must send: `claim` (what the recipe achieved),
  `declared_flops` (≤ topic budget), `artifact_digest` of a **reproducible
  train/eval recipe** (code under budget, not weights-only), plus `manifest`;
  on custom topics also `artifact_uri` (the runner fetches from it).
  The agent verdict (`reproduced`, `claim_holds_public`, cheat codes) is
  filled by the eval image, not the miner.
- Contamination / empty manifest: persist **rejected** without renting.

Miner-facing: [`external-miner/proof.md`](./external-miner/proof.md).

## Example topics (not live)

### `dt-no-ib-v0` — throughput **wta**

Operator **example**, not in the pin and not published until the operator
seals a baseline on the pinned image. Throughput family, no InfiniBand / NVLink / NCCL fast fabric,
12.5 Gbit/s cap, beat sealed AdamW/comms reference, 2e18 FLOPs. Winner
takes the topic.

```json
{
  "schema_version": 1,
  "id": "dt-no-ib-v0",
  "statement": "Beat the sealed AdamW + comms reference on a 12.5 Gbit/s fabric with no InfiniBand, NVLink, or NCCL fast path. Quality may not regress past the floor.",
  "payout_mode": "wta",
  "validation": {
    "score_on": "tokens_per_sec vs sealed comms reference under the fabric cap",
    "accept_if": "reproduced under FLOP/wall budget; quality floor held; beat reference by epsilon_rel",
    "reject_if": "unreproduced claim; fabric cheat; FLOP or wall over budget"
  },
  "constraints": {
    "no_infiniband": true,
    "no_nvlink": true,
    "no_nccl_fast_fabric": true,
    "max_inter_node_gbps": 12.5
  },
  "metric": {
    "family": "throughput",
    "primary": "tokens_per_sec",
    "direction": "max",
    "unit": "tok/s",
    "epsilon_rel": 0.05,
    "quality_floor_nll": 0.02,
    "wall_budget_s": 14400
  },
  "flops_budget": 2000000000000000000,
  "inference": {
    "provider": "openai_compatible",
    "model": "master-proxy-v0",
    "mode": "chat",
    "max_input_tokens": 4096,
    "max_output_tokens": 2048
  },
  "status": "draft"
}
```

### `muon-vs-adamw-10m-v0` — NLL **wta**

Operator **example**, not in the pin. Beat sealed AdamW holdout NLL with Muon
at ~10M params under the same FLOP budget. Staging may publish this id next
to `dt-no-ib-v0`; miners still discover it from `GET /v1/proof/topics`.

### `agent-harness-improve-v0` — custom **discovery**

Operator POST, not in git. The `custom_id` is the topic's own name for its
metric; nothing in this repository knows it. The document drafts as-is and
may open once a runner is registered under that id on the host (until then
an `open` publish is **400** and the registry is empty by default).

```json
{
  "id": "agent-harness-improve-v0",
  "statement": "Improve the agent harness: raise success rate on the sealed holdout episodes without contaminating holdout or short-circuiting eval.",
  "payout_mode": "discovery",
  "validation": {
    "score_on": "Holdout harness success rate (and secondary latency) vs sealed baseline",
    "accept_if": "Reproduced under FLOP/wall budget; no contamination; success rate >= baseline + epsilon",
    "reject_if": "Unreproduced claim; eval short-circuit; FLOP over budget; near-duplicate of an accepted artifact"
  },
  "metric": { "family": "custom", "custom_id": "harness_success_rate", "primary": "success_rate", "direction": "max", "epsilon_rel": 0.05 },
  "constraints": { "firecracker_required": true, "model_pin": "vendor/model", "task_slice": "operator-label", "params": {} },
  "checklist": [
    { "id": "same_seed", "text": "every paid call uses the topic baseline seed" },
    { "id": "no_eval_short_circuit", "text": "the evaluator and the metric path are untouched" }
  ],
  "eval_executor": { "require_offer_commitment": null, "max_proof_deadline_s": 3600 },
  "flops_budget": 2000000000000000000,
  "status": "draft"
}
```

These JSON bodies are documentation. Publishing requires a holdout
commitment, a sealed baseline, a signed `inference{…}` that does not loosen
pin **judge** defaults, and an sr25519 signature under the `proof`
trust-root key. Omitted inference fields inherit the pin; `open` requires a
complete resolved judge config (provider + model + mode + tokens). Empty pin
model with no topic `model` is **400** at publish. The signing payload is
the whole document, but the generic `constraints.*` knobs, `checklist`, and
`eval_executor` are omitted from it when unset, so a topic that sets none of
them signs to the exact bytes it signed before they existed and older
signatures keep verifying.

## Dynamic agentic engine (RLM)

Proof is a **dynamic agentic challenge system**. Every research problem is a
signed topic; each topic's RLM (research lifecycle manager) runs **inside a
VM attributed to that topic**, where it writes the anti-cheat rules, runs the
baseline, inspects and runs miner submissions, and promotes the best
artefact. The binary is the orchestrator: schema, DB, isolation boundary,
artefact store, runner registry. Nothing about a challenge — no benchmark,
metric, model, rule list, or repository — is compiled in. Crates:
`proof-rlm` (core), `proof-rlm-store` (Postgres / memory), `proof-rlm-scorer`
(`LiveScorer` + artefacts + setup driver), `proof-canon` (canonical JSON +
id shapes shared with `proof-task`).

### Topic-carried, generic bindings (signed)

| Field | Meaning |
|-------|---------|
| `metric.custom_id` | Topic-minted metric id, `[a-z0-9][a-z0-9_-]{1,63}`. Draft with any; open needs a registered runner |
| `constraints.firecracker_required` | Miner code runs only inside a Firecracker guest under the topic VM |
| `constraints.model_pin` | `vendor/model[:tag]` every paid call must name (shape-checked only) |
| `constraints.task_slice` | Opaque label the runner interprets; the control plane does not |
| `constraints.params` | ≤32 opaque `slug → printable` runner params |
| `checklist` | ≤64 `{id, text}` anti-cheat rules (unique slug ids), version 1 of the rule set |
| `eval_executor.require_offer_commitment` | 64-hex pin against the live `1x` `EvalExecutorOffer` (`proof-executor`) |
| `eval_executor.max_proof_deadline_s` | Tighten-only against pin `max_proof_deadline_s_ceiling` (7200 s; the live offer may be shorter) |

### Rules → DB, not logs

Rule sets are versioned per topic in `proof_rule_version` (migration
`0020_proof_rlm.sql`): v1 is the signed vector, later versions are what the
RLM writes (`source = rlm`) or the operator edits. A checklist binds to a
rule version **and** its digest, so it cannot be replayed against edited
rules; it is green only when every rule of that version is ticked with
evidence and passes. Every checklist (red or green), every lifecycle
transition, the baseline measurement, artefact metadata, and every promotion
event land in the DB (`proof_checklist`, `proof_lifecycle_event`,
`proof_baseline_measurement`, `proof_artefact`, `proof_promotion_event`,
`proof_topic_version`). Tables are append-only for `base_app`; "current
best" is the newest promotion row. `BASE_DATABASE_URL` selects Postgres; a
configured-but-unreachable database is fatal, an unset one falls back to the
in-memory store with a warning.

### Lifecycle

`draft → owner_presend → awaiting_owner_keys → provisioning → baselining →
open ⇄ evaluating → promoting → open … → closed`. `owner_presend` is an
`askUser`-style hook (no hook = cannot advance; decline = back to draft);
`awaiting_owner_keys` probes `PROOF_RLM_OWNER_INFERENCE_KEY_FILE` for
presence only. `TopicSetup` drives the ceremony over the VM boundary
(provision → RLM `ProposeRules` → rules vN in DB → `Baseline` job →
measurement in DB; a baseline measured over the topic `flops_budget` or
without a measurement is refused) and `mark_sealed` moves `baselining →
open` after the operator seals `custom_value` and re-signs. `mark_sealed`
is fail-closed: the document must be `status: open`, validate as an open
topic on this host (sealed baseline, registered `custom_id`, tighten-only
floors), verify under the pin's topic key, and the sealed
`BaselineMeasurement` must bind to it **and** carry the `custom_value` the
RLM measured — otherwise nothing moves and no version is stored. A re-run
resumes from the persisted state.

### Isolation boundary

`TopicVmOrchestrator` (create / attach / run / teardown-or-retain) is the only
way RLM work happens. `VmJob`s carry public data (signed topic, digests,
rule set, request) — never a host path, a key, or a judge origin. The shipped
orchestrator is `UnwiredVmOrchestrator` (refuses, names
`PROOF_VM_ORCHESTRATOR_URL` / `PROOF_VM_ORCHESTRATOR_TOKEN_FILE`;
`PROOF_RLM_VM_IMAGE_DIGEST` pins the RLM VM image). The generic
`VmBackedRunner` turns inspect / evaluate into VM jobs; registering it under
a `custom_id` is an operator / RLM action. The Lium harvest for
`nll` / `throughput` and the live `1x` `EvalExecutorOffer` (`proof-executor`)
govern the harvest rent; on the custom path each run request records the
resolved executor plan's deadline (tighter of topic and plan) and
`config_commitment` as provenance, and the row stamps `executor_commitment`
like every other scored row. The run request also carries the miner's
`artifact_uri` (the runner fetches it inside the VM and checks
`artifact_digest`; a custom submission without one is a **400** at intake,
no row, and the scorer refuses a request without it), the topic's
`flops_budget`, and the miner's `declared_flops` (the runner may enforce it
as a hard cap). The runner's report must carry its measured `flops_used`,
which becomes the verdict's usage — a report without one is not evidence
(**503**, no row); a measurement over the budget (`flops_over_budget`) or
over the miner's declaration (`flops_under_declared`) is a persisted reject.
The miner's `declared_flops` is never the enforced usage figure; it is the
cap the measurement is held to.

### Runner registry

`custom_id → CustomRunner`, **empty by default**. `GET /v1/status` lists
`registered_custom`. An open custom topic whose id is not registered (or
whose runner reports its backend unwired) is open but not in
`scorable_topics`; a submit is **503** with the root cause and no row.
Publishing an `open` custom topic without a registered runner is **400**;
the same document drafts fine.

### Artefacts and promotion

Every scored row leaves `$PROOF_ARTEFACT_ROOT/{topic_id}/{submission_id}.zip`
(`manifest.json`, `artifact/`, `report.json`, `checklist.json`,
`baseline_ref.json`, `logs/`; a red-checklist reject ships no report), plus
`best.json` (current best pointer) and `events.jsonl` (public `scored` /
`promoted` events). Default root `/artefacts`; compose sets
`/var/lib/proof/artefacts` on the `proof-artifacts` volume. **Promote:** a
pass with a green checklist whose primary beats the bar (sealed value or
reigning best) by `epsilon_rel`, direction-aware, persists as `champion`,
gets a promotion row (with the displaced best), and moves the pointer.
Runs of one topic are serialised by a **lease** held from scoring until the
row is persisted, so the promotion is decided against the store's current
best (never a bar computed before an earlier crown) and written under the
same lease with a compare-and-swap on the best pointer: a crown whose
previous best moved, or that is not strictly better than the incumbent, is
refused (`promotion_refused` in the lifecycle, manifest `promoted: false`).
A run whose row never lands releases its lease after
`DEFAULT_LEASE_TTL` (5 min).

`GET /v1/status` exposes pin `inference` public judge defaults (`provider`,
`model`, `mode`, token caps) and `inference_offer` **public fields only**
(`offer_id`, `provider_kind`, `mode`, `model_ref`, token caps,
`config_commitment`, `status`). It never leaks `base_url`, API keys, or file
paths. Miners do not call this backend.

It also exposes `eval_executor` (`offer_id`, `lium_template_id`,
`machine_shape`, `gpu_count`, `max_proof_deadline_s`, `eval_image_digest`,
`config_commitment`, `status`) and the pin `executor` ceilings
(`gpu_class`, `max_proof_deadline_s_ceiling`, `allowed_lium_template_prefixes`,
`schema_version`, `commitment_alg`). The executor offer holds no secret, so it
is shown whole. Miners do not rent it and do not pass it.
