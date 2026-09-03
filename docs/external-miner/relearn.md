<!-- protocol_version: 1 -->

# Relearn — miners

Challenge id is `relearn`. One of four live challenges; the others are
[Relearn Image](./relearn-image.md), [Relearn Agent](./relearn-agent.md), and
[Bounty](./bounty.md). Relearn Agent post-trains the **same** checkpoint, so
read both before you pick: this page pays for answers, that one pays for
grounded tool use. Long guide, eval image, and harness:
[CortexLM/relearn](https://github.com/CortexLM/relearn).
Cortex pin: [`config/relearn-pin.toml`](../../config/relearn-pin.toml).

**Gateway:** `https://network.cortex.foundation`  
**CLI:** `ctx relearn submit` (install: [README](./README.md#1-install-ctx))  
Miner pays Lium (`LIUM_API_KEY` / `X-Lium-Api-Key`).

You post-train `Qwen/Qwen3.8-27B` (Apache-2.0, native VLM). There is no
separate encoder-attach challenge and no SigLIP to glue on.

The teacher (`incoai/GLM-5.3-NVFP4`, wire id `glm-5.3`) is an HTTP API the
operator runs on the eval host. You never configure it, never reach it, and
never submit weights to it: it is judge-side only.

## How you are scored

You are judged against the live champion on a **private** holdout. The public
ids on `GET /challenge/relearn/v1/status` are the only split you may train on.
The holdout commitment and size are published; the items are not.

| Gate | What it means for you |
|------|----------------------|
| **Holdout win** | Paired win on the private split. This is the only number that can become lattice |
| **Public–holdout gap** | A huge public score with a flat holdout is rejected as overfitting |
| **No contamination** | If a holdout item id or image hash shows up in `manifest.train_*`, the run is rejected. Submitting an **empty** `manifest` does not skip this gate — it fails it (`contamination_evidence_missing`), so declare what you trained on |
| **Pixel shuffle** | Vision families (caption / VQA / OCR / spatial) must get worse when pixels are shuffled |
| **General benches** | MMLU / MMMU-style canaries are **not** in the visible score. A drop past ε vs the champion zeros the run |
| **Retention floors** | The perturbed rerun and the known-answer canaries still have to hold up |

No gate can be skipped by leaving its evidence out. A run measured without a
public split, a perturbed rerun, known-answer canaries, general benches, or the
shuffle control on a vision family the champion measured is rejected
(`*_evidence_missing`) rather than passing the gate it did not take.

## Before you submit

```bash
ctx relearn status
```

Read `can_score`. While it is `false` every submit answers **503**, nothing is
stored, and no pod is rented. `eval_backend` tells you what would score the
run: `lium` is a real eval on the pinned eval image, `sim` is the operator's
offline harness (CI / local only) and is never a live verdict. `base_weights`
reports whether the host has the base checkpoint primed.

## Submit

```bash
export LIUM_API_KEY=...      # your key, if you want a live eval

ctx relearn submit \
  --hotkey 64-hex-hotkey \
  --artifact-digest sha256-of-your-artifact \
  --artifact-uri https://huggingface.co/you/your-model \
  --train-id 1 --train-id 2 \
  --train-dataset my-sft-mix-v3 \
  --wait
```

`--train-id`, `--train-hash`, and `--train-dataset` are repeatable and become
`manifest.train_item_ids`, `train_image_hashes`, and `train_dataset_ids`. At
least one of them is required: `ctx` refuses an empty manifest locally rather
than letting the contamination gate reject it after you have paid for a run.
Pass a hand-written manifest with `--manifest-file manifest.json`.

The same thing with `curl`:

```bash
curl -sS -X POST https://network.cortex.foundation/challenge/relearn/v1/submissions \
  -H 'content-type: application/json' \
  -H "X-Lium-Api-Key: $LIUM_API_KEY" \
  -d '{
    "miner_hotkey": "64-hex hotkey",
    "artifact_digest": "sha256 of your artifact",
    "artifact_uri": "optional-url",
    "manifest": {
      "train_item_ids": [1, 2, 3],
      "train_image_hashes": ["sha256 of each training image"],
      "train_dataset_ids": ["my-sft-mix-v3"]
    }
  }'
```

## Read the verdict

```bash
ctx relearn show rl_0123456789abcdef --wait
```

`--wait` polls until the submission stops moving. Eligible runs sit at
`awaiting_admin` until an operator promotes; you do not promote, and a
regression is never crowned. `rejected` carries the reason —
[troubleshoot.md](./troubleshoot.md) maps each one to the fix.

Never commit the Lium key. If something fails, see [troubleshoot.md](./troubleshoot.md).
