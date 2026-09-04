<!-- protocol_version: 1 -->

# Proof — miners

Challenge id is `proof`. One of two live challenges (`bounty` 2000 bps,
`proof` 8000 bps). Topics are
**operator-published** research problems, not a frozen catalog in git.
You `POST` with a `topic_id` against whatever is currently `open`. Muon,
token superposition, and “decentralized training without InfiniBand” are
*examples* of solutions or of topics — they are not the product.

Cortex pin: [`config/proof-pin.toml`](../../config/proof-pin.toml).
Public docs: [https://network.cortex.foundation](https://network.cortex.foundation).
CLI: `ctx proof submit` (install: [README](./README.md)).

Miner pays Lium (`LIUM_API_KEY` / `X-Lium-Api-Key`).

## How you are scored

List open topics with `ctx proof topics` or
`GET /challenge/proof/v1/proof/topics`. Each topic
carries a signed English `statement`, `validation` (what to score, when to
accept, when to reject), `payout_mode` (`wta` or `discovery`), constraints
the eval image enforces, a metric family (`nll` | `throughput` | `custom`),
FLOP (and for throughput, wall) budgets, and a **sealed** baseline.

Pass gates (reproduced, no contamination, under budget, beat epsilon) are
fail-closed and unchanged. What you are **paid** after a pass depends on the
topic:

| `payout_mode` | Who is paid on that topic |
|---------------|--------------------------|
| **`wta`** | Winner-take-all. Best primary metric among `pass=true` this epoch takes 100% of the topic's emission mass. Exact ties split equally. Everyone else on that topic gets 0 |
| **`discovery`** | Pass floor (default 30% of the topic pool) split equally among verified passes — this reimburses compute even for a small win. The rest is a novelty pool weighted by how much you improved on the sealed baseline (and the current champion, if any). A near-duplicate of an accepted artifact keeps the floor and gets 0 novelty |

Your paid score is the **sum of per-topic masses** over currently `open`
ids, not a mean of binary lattices. A skipped topic is 0 on that topic.
Zero open topics → the host cannot score (`503`), not a paid 0.

| Gate | What it means for you |
|------|----------------------|
| **`topic_id` required** | Missing, unknown, or not-`open` → **400**. The refusal is not a submission |
| **Architecture lock** | The proxy you train must be the one the pinned image bakes. Anything else is **400** |
| **No contamination** | If a holdout shard hash or corpus id shows up in `manifest`, the run is **rejected** without renting. An empty manifest fails `contamination_evidence_missing` |
| **Empty digest** | While `eval_image_digest` is empty the host answers **503**. There is no sim fallback |
| **Throughput quality floor** | Speed is not free: holdout NLL may not trade past the topic's quality floor |

The holdout is 120 records, 24 per scored split (`web_ood`, `code_ood`,
`math_ood`, `longctx`, `multilingual_ood`). The canary is off the number you
are paid on. You never see the records.

`GET /challenge/proof/v1/status` shows `can_score`, `eval_backend`,
`force_sim`, `live_harvest_wired`, `baseline_sealed`. It never leaks
holdout records or teacher hosts.

## Submit

You submit a **reproducible artifact recipe** — train/eval code that the
digest-pinned RLM can re-run under the topic's FLOP/wall budget. Weights-only
tarballs are not a recipe. The agent verdict (`reproduced`,
`claim_holds_public`, cheat codes, rationale) is written by the judge, not
by you.

Required fields:

| Field | What it is |
|-------|----------------|
| `claim` | What you say the recipe achieved (public-split / task claim the RLM re-runs) |
| `declared_flops` | FLOPs you spent. Must be ≤ the topic `flops_budget` |
| `artifact_digest` | SHA-256 of the recipe artifact (reproducible code, not weights-only) |
| `architecture` | Proxy id baked by the pin |
| `manifest` | Training fingerprints the contamination gate checks |

```bash
ctx proof submit \
  --hotkey <64-hex hotkey> \
  --topic-id <open topic id> \
  --artifact-digest <sha256 of your artifact> \
  --architecture <proxy id baked by the pin> \
  --claim "beat sealed baseline holdout NLL by 0.04 at 1.2e18 FLOPs" \
  --declared-flops 1500000000000000000 \
  --train-dataset my-mix-v0
```

The same thing with `curl`:

```bash
curl -sS -X POST https://network.cortex.foundation/challenge/proof/v1/submissions \
  -H 'content-type: application/json' \
  -H "X-Lium-Api-Key: $LIUM_API_KEY" \
  -d '{
    "miner_hotkey": "<64-hex hotkey>",
    "topic_id": "<open topic id>",
    "artifact_digest": "<sha256 of your artifact>",
    "architecture": "<proxy id baked by the pin>",
    "claim": "beat sealed baseline holdout NLL by 0.04 at 1.2e18 FLOPs",
    "declared_flops": 1500000000000000000,
    "manifest": {
      "train_content_hashes": [],
      "train_dataset_ids": ["my-mix-v0"]
    }
  }'
```

Poll `GET /challenge/proof/v1/submissions/{id}`. The row echoes state and,
once judged, the **verdict** (`pass`, agent envelope, failed gates). While
`can_score` is `false`, submissions answer **503**.

The first live *example* topic (not in the pin, not a catalog): **`dt-no-ib-v0`**
is a **throughput `wta`** problem — beat a sealed AdamW/comms reference with no
InfiniBand / NVLink / NCCL fast fabric, under a **12.5 Gbit/s** cap and a
2e18 FLOP budget. Operators publish it when the eval image exists. Until
then, `GET /v1/proof/topics` is empty and submits 503.

Never commit the Lium key. If something fails, see [troubleshoot.md](./troubleshoot.md).
