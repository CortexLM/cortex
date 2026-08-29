<!-- protocol_version: 1 -->

# Relearn — miners

> **This challenge is off.** `relearn` has no row in
> [`config/challenges.toml`](../../../config/challenges.toml), so it has **no
> emission** and no leaf signed by its key can verify. Submitting to it earns
> nothing. Live work is [Bounty](./bounty.md) and [Proof](./proof.md).

Challenge id is `relearn`. Off; the live challenges are
[Bounty](./bounty.md) and [Proof](./proof.md). Long guide, eval image, and harness:
[CortexLM/relearn](https://github.com/CortexLM/relearn).
Cortex pin: [`config/relearn-pin.toml`](../../../config/relearn-pin.toml).

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
| **No contamination** | If a holdout item id or image hash shows up in `manifest.train_*`, the run is rejected. Submitting an **empty** `manifest` does not skip this gate — it fails it (`contamination_evidence_missing`), so declare what you trained on |
| **Pixel shuffle** | Vision families (caption / VQA / OCR / spatial) must get worse when pixels are shuffled |
| **General benches** | MMLU / MMMU-style canaries are **not** in the visible score. A drop past ε vs the champion zeros the run |
| **Retention floors** | The perturbed rerun and the known-answer canaries still have to hold up |

No gate can be skipped by leaving its evidence out. A run measured without a
public split, a perturbed rerun, known-answer canaries, general benches, or the
shuffle control on a vision family the champion measured is rejected
(`*_evidence_missing`) rather than passing the gate it did not take.

## Submit

```bash
curl -sS -X POST https://network.cortex.foundation/challenge/relearn/v1/submissions \
  -H 'content-type: application/json' \
  -H "X-Lium-Api-Key: $LIUM_API_KEY" \
  -d '{
    "miner_hotkey": "<64-hex hotkey>",
    "artifact_digest": "<sha256 of your artifact>",
    "artifact_uri": "optional-url",
    "manifest": {
      "train_item_ids": [1, 2, 3],
      "train_image_hashes": ["<sha256 of each training image>"],
      "train_dataset_ids": ["my-sft-mix-v3"]
    }
  }'
```

`manifest` is required evidence, not decoration. Declare the public item ids,
image hashes, and dataset ids you trained on. All three arrays empty (or no
`manifest` at all) is rejected — the contamination gate cannot clear a run it
has nothing to check.

Poll `GET /challenge/relearn/v1/submissions/{id}`. Eligible runs sit at
`awaiting_admin` until an operator promotes. You do not promote.

The response carries `eval_backend`. `lium` is a real eval on the pinned eval
image; `sim` is the operator's offline harness (CI / local only) and is not a
live verdict. `GET /challenge/relearn/v1/status` shows the same field plus
`can_score` and `base_weights` (`primed` + which var, never a path), so you
can tell before submitting whether the host will score at all. While
`can_score` is `false`, submissions answer **503**.

Never commit the Lium key. If something fails, see [troubleshoot.md](./troubleshoot.md).
