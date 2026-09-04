<!-- protocol_version: 1 -->

# Proof — miners

Challenge id is `proof`. One of two live challenges (`bounty` 5000 bps,
`proof` 5000 bps). Topics are
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
carries a signed statement, constraints the eval image enforces, a metric
family (`nll` | `throughput` | `custom`), FLOP (and for throughput, wall)
budgets, and a **sealed** baseline. Your score on one topic is a lattice
against that sealed baseline. Your paid score is the **mean of per-topic**
lattices over currently `open` ids. A skipped topic is 0 and pulls the mean
down. Zero open topics → the host cannot score (`503`), not a paid 0.

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

```bash
ctx proof submit \
  --hotkey <64-hex hotkey> \
  --topic-id <open topic id> \
  --artifact-digest <sha256 of your artifact> \
  --architecture <proxy id baked by the pin> \
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
    "manifest": {
      "train_content_hashes": [],
      "train_dataset_ids": ["my-mix-v0"]
    }
  }'
```

Poll `GET /challenge/proof/v1/submissions/{id}`. While `can_score` is
`false`, submissions answer **503**.

The first live *example* topic (not in the pin, not a catalog): **`dt-no-ib-v0`**
is a **throughput** problem — beat a sealed AdamW/comms reference with no
InfiniBand / NVLink / NCCL fast fabric, under a **12.5 Gbit/s** cap and a
2e18 FLOP budget. Operators publish it when the eval image exists. Until
then, `GET /v1/proof/topics` is empty and submits 503.

Never commit the Lium key. If something fails, see [troubleshoot.md](./troubleshoot.md).
