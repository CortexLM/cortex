# Proof challenge

Proof is Cortex's research contribution path: the intended output is a
reproducible finding that others can reuse, not just a finished model checkpoint.
Read the [overview](OVERVIEW.md) for the purpose and the
[whitepaper comparison](WHITEPAPER.md) for the proposed shared-research loop.

**Implementation limits:** the Python judge currently performs an authenticated
acknowledgement request and static checks, not the paper's autonomous investigation
and arbitrary recipe reproduction. The service stores submissions in memory.
A `ProofEmitter` loop signs exact-`E` leaves (or covers `E` with
`ChallengeInternal`) so D24 can seal; the scored-epoch watermark is
persisted so a restart does not burn a paid allocation, and the gateway
refuses a burn from replacing a positive leaf. That is still not the
paper's automatic research-to-payment path. Readiness checks alone do not establish a complete
path from a scored run to on-chain payment.
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
- The RLM runs **inside a Firecracker microVM attributed to its topic** on a
  dedicated KVM host, reached only through the orchestrator boundary
  (`TopicVmOrchestrator` → `FirecrackerOrchestrator` → `proof-vm-orchestrator`
  agent). Miner code runs in a **sister** Firecracker guest with no network
  beside that VM; the host, not the RLM, stamps `sandboxed` and the
  guest-measured `flops_used` on the report. The control plane never runs
  RLM logic and never hands the VM a host path or a secret; an unwired
  orchestrator, a missing bearer file, or an unpinned RLM image is a
  **503**, not a host-local fallback.
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
  `live_harvest_wired` (**Lium harvest only** — the `nll` / `throughput`
  scorer; never true because a custom-family scorer is present),
  `custom_family_wired` (a custom-family scorer with ≥1 registered runner is
  on this host, independent of Lium), `baseline_sealed`, `open_topics`,
  `scorable_topics` (open topics whose family's scorer is wired on this
  host **and** that do not defer scoring; `can_score` is true when it is
  non-empty), `deferred_topics` (open topics whose signed document sets
  `constraints.params.defer_scoring = "true"`: submits there are **201**
  `queued`, nothing is evaluated), `queued_submissions` (rows waiting for a
  drain), `registered_custom`
  (custom ids with a runner), `custom_ready` (registered ids whose runner
  could run right now: topic-VM orchestrator bearer file present, image
  pinned — independent of which topics are open), public
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
- `GET /v1/admin/proof/vm-orchestrator` — operator bearer; read-only probe
  of the topic-VM orchestrator through the host's own client: `orchestrator`
  (`firecracker` / `unwired`), `ready` + `reason` (bearer file, RLM image
  pin), the locked template, one agent health call (`agent` /
  `agent_error`), plus the host gates `custom_family_wired`,
  `registered_custom`, and `live_harvest_wired` (Lium only — informational
  for the custom family). Always **200** once authorised — a broken wire is
  data. Names env vars and container
  paths, never the bearer. Run over loopback; wrapped by
  [`deploy/scripts/proof-vm-wire-check.sh`](../deploy/scripts/proof-vm-wire-check.sh).
- `POST /v1/admin/proof/queue/drain` — operator bearer; body
  `{"topic_id": "<id>", "limit": 1}`. Scores that topic's `queued` rows
  oldest first through the live submit path (`limit` rows per call, default
  1) and answers a report (`drained[]`, `remaining`, `stopped`). **409**
  while the topic still defers scoring or is not open (nothing touched);
  **400** unknown topic; **503** carrying the report when the host refused
  before one row scored — every row stays `queued`, nothing was rented. See
  § Deferred scoring.
- `POST /v1/admin/proof/submissions/{id}/score` — operator bearer; scores
  one `queued` row now under the same rules. The row must be the **head**
  of its topic's queue (**404** unknown, **409** not queued / not the head —
  the error names the head — / topic already has a row in flight / topic
  still deferring, **503** host refusal with the row released back to the
  queue).
- `POST /v1/submissions` **requires** `topic_id`, a miner
  `hotkey_signature` (sr25519 over `base-proof-submit-v1`: hotkey + topic_id +
  artifact_digest + declared_flops + claim + canonical manifest + submit_nonce,
  `0xff`-separated), and a 64-hex `submit_nonce`. The signature is checked
  right after the topic checks; the `(hotkey, nonce)` pair is then reserved
  in the submission store **before** any budget check, rent, or row, so an
  identical replay is **401** `submit_nonce reused` and never evaluates
  twice. Missing/unknown/not-open → **400**. Missing/invalid signature or
  nonce → **401** (no row). `X-Lium-Api-Key` is not identity. An `env` map
  that does not match the signed topic's `miner_byok` /
  `miner_env_allowlist` is **400** — checked *before* the signature, so it
  never spends the nonce (§ Miner BYOK). Miners do
  **not** bind the judge offer or the executor offer. Zero
  open / unsealed baseline / empty digest / missing or closed RLM judge
  backend / missing, closed, or non-`1x` executor / agent down / run cut at
  the proof deadline / no registered or wired runner for the topic's
  `custom_id` / `nll` or `throughput` topic on a host with no Lium harvest
  → **503**. Refusals must **not** persist rows. Scored rows
  stamp `executor_offer_id` + `executor_commitment` next to the judge
  `inference_offer_id` + `config_commitment`. On a topic listed in
  `deferred_topics` the same intake gates apply (**400**s unchanged) but
  the host gates are not consulted: the row persists as **`queued`**
  (**201**, `eligible: false`, `detail` names the deferral) with no eval,
  no rent, no judge call, no stamps, no mass; the same artefact from the
  same hotkey again is **200** with the existing row — still queued, or
  already scored after a drain (one row per frozen digest per topic, decided
  in one atomic store step).
- `GET /v1/submissions?state=<queued|awaiting_admin|rejected|champion>&topic_id=<id>`
  — both filters optional; newest first. `GET /v1/submissions/{id}` shows a
  `queued` row with `verdict: null` until it is drained.
- A pass that the family scorer crowns (custom: green checklist and
  `primary >= bar * (1 + epsilon_rel)` direction-aware, bar = sealed value or
  reigning best) persists as `champion`; other passes stay `awaiting_admin`.
- Submit fields miners must send: `claim` (what the recipe achieved),
  `declared_flops` (optional, default `0`, still signed; ignored as a gate
  on custom / agent topics; harvest `nll` / `throughput` still refuse a
  declaration over the topic budget), `artifact_digest` of a **reproducible
  train/eval recipe** (code, not weights-only), `manifest`
  (signed), `submit_nonce` (64 lowercase hex, single use), and
  `hotkey_signature` (exactly 128 lowercase hex sr25519 over
  `base-proof-submit-v1`); on custom topics also `artifact_uri` (the runner
  fetches from it).
  The agent verdict (`reproduced`, `claim_holds_public`, cheat codes) is
  filled by the eval image, not the miner.
- Contamination (holdout overlap in a declared manifest) persists **rejected**
  without renting. An empty training manifest is the same reject **only** on
  topics that require training evidence (harvest `nll` / `throughput` by
  default, or any topic with `constraints.params.require_training_evidence =
  "true"`). Custom / agent topics skip that empty-manifest gate unless they
  tighten; do not invent fake dataset ids.

Miner-facing: [`external-miner/proof.md`](./external-miner/proof.md).

## Deferred scoring (`queued` rows)

An open topic may **accept artefacts before its scoring path is ready** —
the live case is a topic whose in-guest baseline / harness is still being
installed on the KVM host while miners already have recipes to file. The
switch is topic data, signed like every other binding:

```json
"constraints": { "params": { "defer_scoring": "true" } }
```

(`params` values are strings — quote it in YAML too: `defer_scoring: "true"`.
`"false"` and an absent key are the same thing; any other spelling is a
publish **400** naming `constraints.params.defer_scoring`.)

A sibling boolean word, `require_training_evidence`, is the empty-manifest
gate. Absent: harvest (`nll` / `throughput`) require declared training
hashes or dataset ids; `custom` / agent topics skip (no training step).
`"true"` tightens — even a custom topic then rejects an empty manifest.
`"false"` skips on any family (a topic that does not train). Holdout
overlap in a *declared* manifest is always contamination. Any other spelling
is a publish **400** naming `constraints.params.require_training_evidence`.
Absent and `"false"` are **not** the same: absent follows the family default.

Semantics, none of which weaken a product rule:

| Rule | With `defer_scoring = "true"` |
|------|-------------------------------|
| Topic status | Stays **`open`**: it needs a sealed baseline to publish, it is listed in `open_topics`, and it is **not** `draft` (a draft is still a submit **400**). |
| Intake gates | Unchanged: hotkey / digest shape, digest-of-nothing, unknown / not-open topic, missing `artifact_uri` on a custom topic, missing/undeclared miner `env` are the same **400**s with no row. Harvest `nll` / `throughput` still 400 `declared_flops` over budget; custom / agent topics ignore that gate. |
| Host gates | **Not consulted.** The row persists as **`queued`** (**201**) whether or not the host could score it right now — no readiness check, no harvest rent, no topic VM, no judge call, no verdict, no stamps, no topic mass, no emission. |
| Status | The topic is in `deferred_topics`, **not** in `scorable_topics`; `can_score` keeps its meaning (something is scored right now). `queued_submissions` counts the waiting rows. |
| Duplicates | One row per frozen digest per topic, for the row's whole life, decided in one atomic store step: the same artefact from the same hotkey again is **200** with the existing row (`detail: already queued …`), and after a drain it is **200** with the *scored* row (`already submitted … and scored`) — two identical submits racing each other yield one row, and a retry never buys a second paid run. |
| Drain | A drain of a topic that still defers is **409**, nothing touched. |

**Lifting the flag** is a re-publish: sign the same document without the
param (or with `"false"`) and `POST /v1/admin/proof/topics`. The queue
survives the re-publish (rows bind `topic_id`, not a signature). From then
on the rows score **oldest first, one at a time, through the exact path a
live submit takes** (readiness → offers → sealed baseline → holdout unseal →
contamination gate → eval → judge → persist with the host stamps and the
family scorer's promotion / persist hooks), each row keeping its `pf_…` id:

- automatically, by the binary's poll loop (`PROOF_QUEUE_DRAIN_POLL_SECS`,
  default 60; `0` disables it) — lifting the flag *is* "score now";
- on demand, with `POST /v1/admin/proof/queue/drain {"topic_id": …, "limit": n}`
  (default one row per call; the report says what is `remaining`), or the
  head of the queue with `POST /v1/admin/proof/submissions/{id}/score`.

Fail-closed on the drain: a host refusal (unwired runner, closed executor,
no sealed baseline recorded, agent down, …) leaves that row **`queued`** —
never a reject — and stops the pass (**503** with the reason when nothing
scored); a contamination row, or an empty-manifest row on a topic that
requires training evidence, persists **`rejected`** with no
rent, exactly as a live submit would. **A topic holds one claim at a time:**
while its head is mid-eval, a second drain of that topic (admin route,
single-row route, or the poll pass) is a **409** / a `stopped` report that
names the row in flight and scores nothing — two rows of one topic never run
side by side, so promotion always compares against the best at that moment;
other topics are independent. The claim is released on every path that does
not land the row — host refusal, panic, or a drain interrupted mid-eval (an
operator HTTP call cut, the poll task cancelled) — so an interruption never
leaves a topic busy until a restart; a late release can never drop a claim a
later drain took. Re-publishing the topic with the flag back on **pauses**
the queue mid-pass. `queued` is the only non-terminal state;
`ctx proof show --wait` keeps polling through it.

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
| `metric.custom_id` | Topic-minted metric id, `[a-z0-9][a-z0-9_-]{1,63}` (underscores allowed — a separate namespace from the topic `id`, which is a hyphen slug `[a-z0-9][a-z0-9-]{1,62}`; nothing maps one onto the other). Draft with any; open needs a runner registered under **exactly** this id (`PROOF_VM_RUNNER_CUSTOM_IDS`, byte-for-byte: `_` ≠ `-`). A miss names the registered hyphen / underscore twin when there is one |
| `constraints.firecracker_required` | Miner code runs only inside a Firecracker guest under the topic VM |
| `constraints.model_pin` | `vendor/model[:tag]` every paid call must name (shape-checked only) |
| `constraints.task_slice` | Opaque label the runner interprets; the control plane does not |
| `constraints.params` | ≤32 opaque `slug → printable` runner params |
| `constraints.params.miner_byok` | Comma-separated environment variable names (`[A-Z][A-Z0-9_]{0,63}`) a miner **must** send in the submit body's `env`. A submission missing one is **400** before any row, rent, or paid inference. The topic carries the *name*; the miner carries the value. See § Miner BYOK |
| `constraints.params.miner_env_allowlist` | Additional variable names a miner **may** send (accepted, never demanded). `miner_byok` is always allowed on top of it |
| `constraints.params.inject_miner_env_sister` | `"true"` also forwards the miner env into the **sister** guest that runs the miner's entrypoint. Absent / `"false"` keeps it in the runner guest only |
| `constraints.params.baseline_runner` (synonym `in_guest_benchmark_runner`) | Generic knob (`proof-experiment`): selects an **operator adaptor id** baked into the guest image (e.g. `baseline_runner: rlm_fc_in_guest_harbor`); the topic's paid jobs then run in **one dedicated experiment VM per job**, sized under a hard 16 vCPU / 32 GiB lock, and score only once the orchestrator confirms that VM destroyed. Values are topic data. A versioned Harbor reference adaptor ships under `deploy/guest/runners/rlm_fc_in_guest_harbor/` for operators to bake; the Harbor CLI and task pack do not |
| `constraints.params.experiment_pack_digest` / `experiment_pack_path` | `sha256:` of the pack tar the KVM host stages into that VM (required with a runner; never defaulted) / optional relative locator under the host pack dir |
| `constraints.params.experiment_vcpus` / `experiment_mem_mib` / `experiment_disk_mib` | The topic's size ask: silent = the operator defaults (lock 16 vCPU / 32 GiB / 32 GiB disk), an ask may go up to the ceilings (lock 16 vCPU / 32 GiB — the default is the ceiling; disk ≥ 16 GiB); over = 503, never a clamp |
| `checklist` | ≤64 `{id, text}` anti-cheat rules (unique slug ids), version 1 of the rule set |
| `eval_executor.require_offer_commitment` | 64-hex pin against the live `1x` `EvalExecutorOffer` (`proof-executor`) |
| `eval_executor.max_proof_deadline_s` | Tighten-only against pin `max_proof_deadline_s_ceiling` (7200 s; the live offer may be shorter) |

### Miner BYOK (`env` on the submit body)

A topic whose harness calls a paid third-party API is paid for by the
**miner**, never by the operator. The signed topic names the environment
variables (`miner_byok` / `miner_env_allowlist`); the miner posts the values
as `env: {"<NAME>": "<value>"}` on `POST /v1/submissions`.

| Rule | Where |
|------|-------|
| The allowlist is the signed topic's. An undeclared name is **400**, never a silent drop; a topic that declares nothing accepts no `env` at all | `Constraints::miner_env_allowlist`, `MinerEnv::accept` (`proof-canon`) |
| Names are `[A-Z][A-Z0-9_]{0,63}`, never `PROOF_…` and never one the guest contract sets (`PATH`, `HOME`, `LANG`, `XDG_RUNTIME_DIR`). Enforced at publish, at intake, and again in the guest | `is_env_name` |
| `env` is **not** in `base-proof-submit-v1`. It is checked *before* the signature, so a rejected `env` does not spend the miner's single-use `submit_nonce` | `submit` (`proof-http`) |
| The value never reaches a public answer: not on the `Submission` row, so not in `GET /v1/submissions`, `/v1/status`, or a drain report. `MinerEnv`'s `Debug` prints names and `[REDACTED]`, so it cannot reach a log line by being nested in something formatted | `MinerEnv`, `MemoryStore::stash_miner_env` |
| **A secure file, not a process value.** At intake the key goes into the vault: `<PROOF_MINER_BYOK_DIR>/<submission_digest>/<NAME>`, directories `0700`, files `0600`, written temp-then-rename, keyed by frozen digest. The scoring path reads it back from there — the same step whether the row is scored now or drained days later, so a control plane that restarted in between still reaches the paid run with the miner's key. Removed the moment the row is terminal | `MinerEnvVault` (`proof-store`) |
| A vault that cannot hold the key is a **503 with no row**: a key the host did not keep must not be accepted | `submit` (`proof-http`) |
| A topic that **requires** BYOK and a host that no longer holds it is a **503 with the row untouched** (a drain leaves it `queued`, nothing rented). The operator key is never substituted for a miner's missing one | `score_intake` (`proof-http`) |
| In the guest it is exported under the declared name for the **paid** job only (`Baseline` / `Evaluate`), written to a 0600 file at `$PROOF_MINER_ENV_DIR/<NAME>` — the same shape as the vault, so the adaptor reads a file on both sides of the VM boundary — listed by name in `$PROOF_MINER_ENV_NAMES`, and added to the redaction set so a run that prints it gets `[REDACTED]` back. Inspection ticks rules without spending and is handed nothing | `inject_miner_env` (`proof-vm-guest`) |
| Owner key material is a different path entirely: the KVM host reads `PROOF_VM_AGENT_OWNER_KEY_DIR` from its own disk and stages it into the RLM guest at boot. It never travels on a job, and never into a sister or experiment guest as a miner variable | `proof-fc-host` |

Operator knob: `PROOF_MINER_BYOK_DIR` (default `/run/proof/miner-byok` — a
runtime path, so a reboot never leaves a miner's key on disk). Set it to the
empty string to keep material in the process; the host warns at boot that a
restart then loses it and a deferred topic's queue will 503 on drain.

The sister leg is opt-in per topic (`inject_miner_env_sister`) and
name-checked host-side (`SisterRequest::check_env`) before the relay, so a
compromised RLM image cannot use it to rewrite a sister's `PATH`.

`X-Lium-Api-Key` is unrelated and unchanged: it pays for **compute** on the
Lium executor. `env` pays for whatever the topic's own harness calls.

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
measurement in DB; custom / agent baselines do not refuse on
`flops_budget` or a missing `flops_used`) and `mark_sealed` moves `baselining →
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
rule set, request) — never a host path, a key, or a judge origin. Two
orchestrators exist: `UnwiredVmOrchestrator` (the default; refuses, names
`PROOF_VM_ORCHESTRATOR_URL` / `PROOF_VM_ORCHESTRATOR_TOKEN_FILE`) and the
live `FirecrackerOrchestrator` (`crates/proof-vm-fc`), a thin HTTPS client of
the `proof-vm-orchestrator` agent on a host with a working `/dev/kvm` —
production: **a dedicated DO droplet (`g-8vcpu-32gb`, nyc1, nested
`/dev/kvm`) on the VPC, never colocated on the CP**; staging: colocating the
agent on the control-plane droplet with nested `/dev/kvm` is an **allowed
exception, proven** on `cortex-staging` (fragile — if the boot fails,
provision the dedicated droplet); never a Lium pod, never an emulator.
The host prefers it
when `PROOF_VM_ORCHESTRATOR_URL` (https; plain http only on loopback) and
`PROOF_VM_ORCHESTRATOR_TOKEN_FILE` are set; the bearer is a file re-read per
request and never logged; `PROOF_RLM_VM_IMAGE_DIGEST` pins the RLM VM rootfs
(4 vCPU / 8192 MiB by default; unpinned = `ready()` fails, 503 naming the
var). On the KVM host, jailer boots **one Firecracker RLM microVM per
`topic_id`** from that digest (re-hashed before boot), hands it jobs over
vsock, stages the owner key material from the host's own directory (the
control plane only probes its copy for presence), and gives it an nftables
egress allowlist (empty = no egress). Every paid run (`Baseline`,
`Evaluate`) that the RLM asks for happens in a **sister** Firecracker guest
with **no network**: the RLM ships the artefact bytes it already inspected
over vsock — **the exact bytes it fetched from `artifact_uri`, verbatim**
(`artifact_digest` is the sha256 of that served file; the guest runs
`proof_vm_proto::tar::verify_artifact` on what it received and never re-tars
the tree, never substitutes one — a fetch that fails or does not verify is
`RlmToHost::Failed`, 503, no row), the host runs the same check and boots
the sister from its own pinned image, holds it to the topic deadline,
destroys it, and writes the `SisterAttestation`. The
agent then **stamps** the report: `sandboxed` is `true` only when a sister
ran, `flops_used` is the sister guest's measurement — an RLM cannot claim a
sandbox the host did not boot, and a sister that measured nothing yields no
usage (503, never a substituted number). The attestation is evidence for
**one job**: it names the topic, submission, and artefact the host verified
before booting the sister (a `SisterRequest` for any other identity is
refused before a jail exists), the agent refuses to stamp — 502
`evidence_mismatch` — when the attestation or the report names another
identity than the job, and the client runs the same `bind_evidence` check
before accepting the stamps, so a sister that ran artefact A can never score
artefact B. Hard binds on both sides: a job
must name the VM's topic (envelope **and** job) or the agent answers 409; the
client refuses a job for another topic before any request, checks every
echo, refuses a created VM on another digest, and refuses a
`firecracker_required` run that came back without the sister attestation.
Teardown honours the topic's `retain` policy (default **destroy**; `retain`
keeps the jail for audit). Nothing a boot started outlives its failure: a
boot that fails after the jail is prepared (TAP, rules, spawn, guest
handshake) releases the process, the TAP, the nftables table, and the jail;
a sister whose job ends first is cancelled cooperatively and destroyed
before the job answers; and a VM whose process exits outside a teardown is
reaped per its retain policy, recorded as `crashed`, and never advertised
as running — its topic gets a fresh VM on the next job.
**Experiment VMs (in-guest runner topics).** A topic whose signed
`constraints.params` select an in-guest runner (`baseline_runner` /
`in_guest_benchmark_runner` + `experiment_pack_digest`, `proof-experiment`)
gets **one dedicated experiment microVM per paid job** instead of a sister:
`run_paid_job` sizes it from the topic's ask under the operator caps (lock:
16 vCPU / 32 GiB RAM — the default a silent topic gets and the most a topic
may ask for — writable disk ≥ 16 GiB, 32 GiB by default; over = 503, never
clamped), the agent creates it
beside the topic's RLM VM (no one-per-topic rule for experiments, a host
capacity cap instead — parallel experiments are parallel VMs, never
containers sharing one), the host resolves the pinned pack under its pack
directory, re-hashes it with the same artefact check, and stages it over
vsock before any job, the guest agent (`proof-vm-guest-agent`, this
repository) execs the operator adaptor for that runner id with the
experiment pack, the fetched-and-verified artefact, the model pin, and every
topic param, and the host attests the run as `experiment_vm` for that VM,
topic, submission, and artefact; the VM is destroyed after the job. No
adaptor, pack, or value is defaulted anywhere: no runner selected, no
adaptor baked, no pack staged, no report, or a non-finite value is a failed
job. Runbook [`runbooks/proof-experiment-vms.md`](runbooks/proof-experiment-vms.md).
Deploy: `deploy/systemd/proof-vm-orchestrator.service`,
runbook [`runbooks/proof-vm-orchestrator.md`](runbooks/proof-vm-orchestrator.md).
CI runs the fake hypervisor only; no GitHub runner ever boots Firecracker.
The generic `VmBackedRunner` turns inspect / evaluate into VM jobs;
registering it under a `custom_id` is an operator action
(`PROOF_VM_RUNNER_CUSTOM_IDS`, comma-separated; unset = empty registry). The
Lium harvest for
`nll` / `throughput` and the live `1x` `EvalExecutorOffer` (`proof-executor`)
govern the harvest rent; on the custom path each run request records the
resolved executor plan's deadline (tighter of topic and plan) and
`config_commitment` as provenance, and the row stamps `executor_commitment`
like every other scored row. The run request also carries the miner's
`artifact_uri` (`artifact_digest` is the sha256 of the **file** served
there — an uncompressed tar of the recipe tree; the runner fetches it inside
the VM, verifies the bytes as received against `artifact_digest`, and
forwards them verbatim, and the KVM host runs the same check
(`proof_vm_proto::tar::verify_artifact`) before it boots a sister: one
identity, never a re-tar of the tree, which would hash differently. A
custom submission without a locator is a **400** at intake, no row, and the
scorer refuses a request without it; an `artifact_digest` that is the sha256
of nothing — zero bytes, an empty tar — is a **400** too; the host refuses a
content-less, compressed, non-tar, or mis-hashed `artifact_tar`, so a guest
that substitutes an empty tree when its fetch fails can never produce a
scored run). The topic's `flops_budget` and the miner's `declared_flops`
travel with the request for signature / harvest compat; custom / agent
topics do **not** reject on measured `flops_used` vs either figure, and a
report without a measurement is still evidence. Anti-cheat is the signed
topic checklist, sister attestation, and miner BYOK env.

### Runner registry

`custom_id → CustomRunner`, **empty by default**. `GET /v1/status` lists
`registered_custom`. An open custom topic whose id is not registered (or
whose runner reports its backend unwired) is open but not in
`scorable_topics`; a submit is **503** with the root cause and no row.
Publishing an `open` custom topic without a registered runner is **400**;
the same document drafts fine.

The registry is wired from the topic-VM orchestrator env alone, **not**
from the Lium harvest. `proof-challenge` builds the RLM scorer over the
registry whenever the env selects the live `FirecrackerOrchestrator`
(`PROOF_VM_ORCHESTRATOR_URL` + `PROOF_VM_ORCHESTRATOR_TOKEN_FILE`, https)
and `PROOF_VM_RUNNER_CUSTOM_IDS` registers at least one id; with no Lium
credentials the mux is `FamilyMux::custom_only` — custom topics score over
the topic VMs while every `nll` / `throughput` topic is open but not in
`scorable_topics` and a submit there is **503** (`LiveHarvestUnavailable`,
no row, no rent, never an in-process sim). No placeholder harvest is needed
to open a custom topic. `GET /v1/status` reports the two families apart:
such a host shows `live_harvest_wired: false` (that flag is the Lium harvest
and nothing else) next to `custom_family_wired: true`, `registered_custom`,
and `custom_ready`. Token file and image digest are still checked per
request (**503** naming the variable; the id then drops out of
`custom_ready` while staying in `registered_custom`). URL unset or refused
(plain `http://` off loopback) keeps `UnwiredVmOrchestrator`, and with no
Lium harvest either the host has no live scorer at all (both flags false,
every submission **503**); the unwired stub never carries a mux, and ids
listed over it register nothing.

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
