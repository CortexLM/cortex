<!-- protocol_version: 1 -->

# Relearn LLM — miners

The live challenge. Challenge id is `relearn`. Long guide, eval image, and harness:
[CortexLM/relearn](https://github.com/CortexLM/relearn).
Cortex pin: [`config/relearn-pin.toml`](../../config/relearn-pin.toml).

Miner pays Lium (`LIUM_API_KEY` / `X-Lium-Api-Key`).

You post-train `Qwen/Qwen3.8-27B` (Apache-2.0, native VLM). There is no
separate encoder-attach challenge and no SigLIP to glue on.

Teacher is an HTTP API served from an operator local directory. The operator
sets `RELEARN_TEACHER_API_URL`, `RELEARN_TEACHER_MODEL`,
`RELEARN_TEACHER_API_KEY`, and `RELEARN_TEACHER_LOCAL_DIR` on the host. You
do not.

## How you are scored

You are judged against the live champion on a **private** holdout. The public
ids on `GET /challenge/relearn/v1/status` are the only split you may train on.
The holdout commitment and size are published; the items are not.

| Gate | What it means for you |
|------|----------------------|
| **Holdout win** | Paired win on the private split. This is the only number that can become lattice |
| **Public–holdout gap** | A huge public score with a flat holdout is rejected as overfitting |
| **No contamination** | If a holdout item id or image hash shows up in `manifest.train_*`, the run is rejected |
| **Pixel shuffle** | Vision families (caption / VQA / OCR / spatial) must get worse when pixels are shuffled |
| **General benches** | MMLU / MMMU-style canaries are **not** in the visible score. A drop past ε vs the champion zeros the run |

## Submit

```bash
curl -sS -X POST https://<gateway>/challenge/relearn/v1/submissions \
  -H 'content-type: application/json' \
  -H "X-Lium-Api-Key: $LIUM_API_KEY" \
  -d '{
    "miner_hotkey": "<64-hex hotkey>",
    "artifact_digest": "<sha256 of your artifact>",
    "artifact_uri": "optional-url",
    "manifest": {
      "train_item_ids": [],
      "train_image_hashes": [],
      "train_dataset_ids": []
    }
  }'
```

Poll `GET /challenge/relearn/v1/submissions/{id}`. Eligible runs sit at
`awaiting_admin` until an operator promotes. You do not promote.

Never commit the Lium key. If something fails, see [troubleshoot.md](./troubleshoot.md).
