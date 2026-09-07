# Proof challenge

Proof is Cortex's research contribution path: the intended output is a
reproducible finding that others can reuse, not just a finished model checkpoint.
Read the [overview](OVERVIEW.md) for the purpose and the
[whitepaper comparison](WHITEPAPER.md) for the proposed shared-research loop.

**Implementation limits:** the Python judge currently performs an authenticated
acknowledgement request and static checks, not the paper's autonomous investigation
and arbitrary recipe reproduction. Clean static inspection now raises `ContractError`
without agent reproduction and verified FLOP evidence; forbidden fabric rejects.
The CLI gates before judge/model calls and produces no successful metrics.
Agent-led accounting is not implemented: the agent must determine experiment-appropriate accounting, retain reproducible evidence, and have that evidence verified by the controller. Neither a universal formula nor an arbitrary model assertion is sufficient.
The default v1 store is in memory. The optional SQL journal now passes fresh workspace durability tests (2/2) after fixing embedded-migration tracking. Async writes persist SQL before memory and reject NaN/infinity. Production durability remains unproven: cross-process ID collisions/upserts, separate submission/score transactions, synchronous bypass and cancellation-induced memory lag remain.
The v1 service does not run an automatic reward-leaf emitter. Payout/signing helpers exist, but
readiness checks alone do not establish a complete research-to-payment path.
The rules below describe the current interfaces and scoring functions, not a
claim that these gaps are closed.

The separate [Atlas orchestration path](../crates/proof-autonomy-pg/README.md)
persists experiment commands, consent, controller ownership, resources and cleanup
in Postgres using canonical migrations **0020–0028**. `proof-challenge` can
explicitly mount its signed v2 API with `PROOF_AUTONOMY_DATABASE_URL_FILE`, but
starts no experiment worker. The `proof-worker` library implements quote refresh,
strict resource adoption, durable runtime identity/deadlines, same-fence
invocation refusal and an independent cleanup lane. It enforces the original DB
runtime deadline even if an agent ignores stop; shutdown aborts lease acquisition
and drops suspended operation/heartbeat futures before DB bookkeeping. Live strict Lium,
credential and quote adapters remain unwired. One split-GPU rent returned **400**; one whole-host **$0.25/hr** pod remained `PENDING` before termination. Two DELETEs returned **200**; GET returned **404** and the list was empty after **6 s**, with balance unchanged. This does not establish side-effect-free refusals generally, client-id support or its absence, stopped running-pod billing, a whole-hour tariff, or expiry enforcement. Existing `prism-lium` custom templates accept `docker_image=repo@digest`; a null separate digest field does not make digest pinning impossible. Live custom-image enforcement remains untested. See the
[measured semantics](runbooks/proof-autonomy-local.md#measured-lium-api-semantics-live-probe-2026-09-07).

The local Docker executor runs actual paired CPU scripts and retains outputs,
exit status, wall time and failures. Metrics and FLOPs come only from a trusted
observer (`proof-measure`), optionally wired in `proof-experiment`, that runs a
pinned evaluation image with no network by default,
a read-only rootfs and a read-only holdout bind; `collect` still fails closed
with `UnobservedMeasurements` whenever FLOPs are unmeasured, and the default
`NoObserver` never produces science. Only a test observer image has been
exercised: the real `proof-eval` image does not yet report independent FLOPs,
and optional judge egress has only demonstrated a synthetic probe with a real
completion and four sampled blocked escapes. The eval helper supports explicit `PROOF_JUDGE_PROXY=1` without Authorization only for the exact alias `http://proof-judge:8080/v1` and `chat/completions`; direct mode still requires a key. The proxy does not yet prove confidentiality or integrity:
allowed arbitrary payloads and artifact code remain risks.
The shared headless launcher drives the real `CortexRuntime`; one authorized
Astra → isolated Python → private controller synthetic IPC test passed, not a
reproduction or verified-cost test. W&B v0.28.0 automatic runtime/environment
telemetry exceeds the seven-field public allowlist, so the stock SDK stays
unusable; `proof-wandb` uploads the allowlisted record over direct GraphQL
instead, but it is wired into no binary and has never contacted W&B.

The separate `proof-atlas` binary is opt-in via `PROOF_ATLAS_CONFIG_FILE` /
`--config`, with a bounded `--check` mode. It can run the finalized-round
scheduler/publisher, never experiments, rentals, sealing or chain submission.
The strict round receiver and exact-receipt plus byte-readback transport are
implemented and tested locally; no deployed receiver or live round publication
is established. The ignored headless-delivery regression passed the real scheduler /
Postgres → unmodified `CortexRuntime`/Docker Python → private decision → strict
HTTP publication/readback → production `seal_epoch` → served router/independent
Python vector `[0.4, 0.4, 0.2]` path, including lost-ack byte replay without model
rerun. Chain/science/inference are synthetic; the admin seal HTTP route is not
covered by that direct-helper test.

Three binary startup/shutdown regressions passed, including SIGTERM while
waiting for finality; Atlas cancellation prevents a late blocking RPC result
from freezing a round after shutdown. Seventeen process-supervision tests passed;
the optional `headless.pid_namespace` closes `setsid` escapes by running the
launcher as PID 1 of a private PID namespace (PID containment only).
See the [operator contract and verification limits](runbooks/proof-autonomy-local.md),
including what `--check` does not validate and the complete local validation checkpoint.
Existing v1 submission/scoring and the Proof/Bounty **8000/2000 bps** split are
unchanged.

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
- `custom` metric family: unknown id is **400 at publish**, **503 at score**.
  v0 `supported_custom()` lists `harness_success_rate` so the operator can
  publish that topic; scoring fail-closes until the real harness fills
  `custom_value`.
- `PROOF_FORCE_SIM` is CI/local opt-in only. Never a fallback. Forbidden on
  droplet overlays.
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
| `custom` | named inside `proof-eval` | Unknown id refuses. `harness_success_rate` is listed and fail-closes until the harness exists |

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
  `live_harvest_wired`, `baseline_sealed`, public pin `inference` judge
  defaults (no origin), public `inference_offer` (RLM judge backend). Never
  leak origins, keys, or holdout records.
- `GET /v1/proof/topics`, `GET /v1/proof/topics/{id}`
- `POST /v1/admin/proof/topics` — operator bearer; verify sig/schema/floors/seal before `open`
- `POST /v1/submissions` **requires** `topic_id`. Missing/unknown/not-open →
  **400**. Miners do **not** bind the judge offer. Zero open / unsealed
  baseline / empty digest / missing or closed RLM judge backend / agent down
  → **503**. Refusals must **not** persist rows.
- Submit fields miners must send: `claim` (what the recipe achieved),
  `declared_flops` (≤ topic budget), `artifact_digest` of a **reproducible
  train/eval recipe** (code under budget, not weights-only), plus `manifest`.
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

### `agent-harness-improve-v0` — custom **discovery**

Operator POST, not in git. `custom_id = harness_success_rate` is listed so
this document publishes; scoring fail-closes until the harness exists.

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
  "flops_budget": 2000000000000000000,
  "status": "draft"
}
```

These JSON bodies are documentation. Publishing requires a holdout
commitment, a sealed baseline, a signed `inference{…}` that does not loosen
pin **judge** defaults, and an sr25519 signature under the `proof`
trust-root key. Omitted inference fields inherit the pin; `open` requires a
complete resolved judge config (provider + model + mode + tokens). Empty pin
model with no topic `model` is **400** at publish.

`GET /v1/status` exposes pin `inference` public judge defaults (`provider`,
`model`, `mode`, token caps) and `inference_offer` **public fields only**
(`offer_id`, `provider_kind`, `mode`, `model_ref`, token caps,
`config_commitment`, `status`). It never leaks `base_url`, API keys, or file
paths. Miners do not call this backend.
