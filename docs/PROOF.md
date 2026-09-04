# Proof challenge

Live challenge id: **`proof`**. Emission **5000 bps** (equal split with
`bounty`; sum across the two live rows stays 10000). Port **8100** (local
probe **28100**).

Proof is the research-problem challenge. The unit of work is an
**operator-published signed topic**, not a prompt and not an episode. Git
carries global floors in [`config/proof-pin.toml`](../config/proof-pin.toml)
and no topic catalog. Miners submit against `topic_id`. The RLM judge lives
in a digest-pinned eval image (`ghcr.io/cortexlm/proof-eval`). Empty
`eval_image_digest` is the pre-launch state: live submits answer **503**.
Do not invent a sha256.

## Product rules (do not weaken)

- Topics are dynamic. Muon / token superposition / “decentralized training
  without InfiniBand” are *example solutions or example topics*, never a
  frozen catalog in git.
- A topic may tighten a floor, never loosen it. Floors live in the pin.
- A baseline must be sealed (`script_sha256` + `metrics_commitment`) to
  open. Nobody is paid for beating a number nobody measured.
- 5000 bps is split equally across currently `open` topics. Miner score =
  **mean of per-topic lattices**. Skipped topic = 0. Empty open set →
  `NoScore(ChallengeInternal)`, not a paid 0.
- `custom` metric family: unknown id is **400 at publish**, **503 at score**.
  v0 `supported_custom()` is empty.
- `PROOF_FORCE_SIM` is CI/local opt-in only. Never a fallback. Forbidden on
  droplet overlays.
- No Modal. No secrets, hosts, holdout records, or teacher endpoints in git.

## Metric families

| Family | Primary | Win |
|--------|---------|-----|
| `nll` | `holdout_nll` (min) | Beat sealed AdamW by `epsilon_nll >= 0.02`. Per-split NLL regress `<= epsilon_topic_max_regress >= 0.05` |
| `throughput` | `tokens_per_sec` (max) or `step_latency_ms` (min) | Requires `flops_budget` **and** `wall_budget_s`. `epsilon_rel >= 0.05`. Quality floor: `holdout_nll <= sealed_nll + quality_floor_nll` (≤ pin 0.02). Eval image enforces comms (e.g. 12.5 Gbit/s); it does not trust the claim |
| `custom` | named inside `proof-eval` | Unknown id refuses |

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
- Contamination / empty manifest: persist **rejected** without renting.

Miner-facing: [`external-miner/proof.md`](./external-miner/proof.md).

## Example topic (not live)

`dt-no-ib-v0` is an operator **example**, not in the pin and not published
until the eval image exists. Throughput family, no InfiniBand / NVLink /
NCCL fast fabric, 12.5 Gbit/s cap, beat sealed AdamW/comms reference, 2e18
FLOPs.

```json
{
  "schema_version": 1,
  "id": "dt-no-ib-v0",
  "statement": "Beat the sealed AdamW + comms reference on a 12.5 Gbit/s fabric with no InfiniBand, NVLink, or NCCL fast path. Quality may not regress past the floor.",
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

This JSON is documentation. Publishing it requires a holdout commitment, a
sealed baseline, and an sr25519 signature under the `proof` trust-root key.
