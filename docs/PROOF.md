# Proof challenge

Live challenge id: **`proof`**. Emission **8000 bps** (80% of the subnet;
bounty is 2000). This 20%/80% lock is independent of eval digest. Proof still
answers **503** on submit until `eval_image_digest` is pinned. Do not invent
a sha256. Sum across the two live rows stays 10000. Port **8100** (local
probe **28100**).

Proof is the research-problem challenge. The unit of work is an
**operator-published signed topic**, not a prompt and not an episode. Git
carries global floors in [`config/proof-pin.toml`](../config/proof-pin.toml)
and no topic catalog. Miners submit against `topic_id`. The RLM judge lives
in a digest-pinned eval image (`ghcr.io/cortexlm/proof-eval`). Empty
`eval_image_digest` is the pre-launch state: live submits answer **503**.

## Product rules (do not weaken)

- Topics are dynamic. Muon / token superposition / “decentralized training
  without InfiniBand” are *example solutions or example topics*, never a
  frozen catalog in git.
- A topic may tighten a floor, never loosen it. Floors live in the pin.
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

## How Mathis injects a challenge (English)

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

Ship order: control plane (this) → proof-eval image + digest pin (separate)
→ holdout/baseline files on the host → open topics. `proxy_model` may stay
empty until the image exists. Empty digest stays 503.

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
`cargo run -p xtask -- proof-topic --input … --secret …`. Trust-root keygen
is the throwaway owner path in [`config/CEREMONY.md`](../config/CEREMONY.md).

## HTTP

- `GET /health`, `GET /v1/status` — `can_score`, `eval_backend`, `force_sim`,
  `live_harvest_wired`, `baseline_sealed`. Never leak endpoints or records.
- `GET /v1/proof/topics`, `GET /v1/proof/topics/{id}`
- `POST /v1/admin/proof/topics` — operator bearer; verify sig/schema/floors/seal before `open`
- `POST /v1/submissions` **requires** `topic_id`. Missing/unknown/not-open →
  **400**. Architecture ≠ proxy → **400**. Zero open / unsealed baseline /
  empty digest / agent down → **503**. Refusals must **not** persist rows.
- Submit fields miners must send: `claim` (what the recipe achieved),
  `declared_flops` (≤ topic budget), `artifact_digest` of a **reproducible
  train/eval recipe** (code under budget, not weights-only), plus `manifest`.
  The agent verdict (`reproduced`, `claim_holds_public`, cheat codes) is
  filled by the eval image, not the miner.
- Contamination / empty manifest: persist **rejected** without renting.

Miner-facing: [`external-miner/proof.md`](./external-miner/proof.md).

## Example topics (not live)

### `dt-no-ib-v0` — throughput **wta**

Operator **example**, not in the pin and not published until the eval image
exists. Throughput family, no InfiniBand / NVLink / NCCL fast fabric,
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
commitment, a sealed baseline (to open), and an sr25519 signature under the
`proof` trust-root key.
