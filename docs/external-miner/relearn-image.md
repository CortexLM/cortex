<!-- protocol_version: 1 -->

# Relearn Image — miners

Challenge id: **`relearn-image`**.

Fine-tune the pinned text-to-image generator and beat the champion on prompts
you cannot see. Long guide, eval image, and harness:
[CortexLM/relearn](https://github.com/CortexLM/relearn).
Cortex pin: [`config/relearn-t2i-pin.toml`](../../config/relearn-t2i-pin.toml)
(the file keeps the pre-launch `t2i` spelling; the challenge id is `relearn-image`).
Eval image: `ghcr.io/cortexlm/relearn-image-eval@sha256:81c40dc6…` (same
manifest as `relearn-t2i-eval`; [`CortexLM/relearn`](https://github.com/CortexLM/relearn) PR #3).

**Gateway:** `https://network.cortex.foundation`  
**CLI:** `ctx image submit` (install: [README](./README.md#1-install-ctx))  
Miner pays Lium (`LIUM_API_KEY` / `X-Lium-Api-Key`).

## What you start from

| Thing | Value |
|-------|-------|
| Generator seed | `nvidia/Cosmos3-Super-Text2Image` |
| Base license you inherit | **NVIDIA OpenMDW 1.1** — <https://openmdw.ai/license/1-1/> |
| Judge | **Q-Judger** — `Qwen/Qwen-Image-Bench` (Apache-2.0) |
| Prompt set | `Qwen/Qwen-Image-Bench` dataset, ids 1..=1000 |

Your artifact must be a fine-tune or post-train of that Cosmos3 checkpoint. The
card documents it as ready for commercial and non-commercial use, 65B params,
BF16 only, on NVIDIA Ampere / Hopper / Blackwell. Serve it with vLLM-Omni
(`vllm serve nvidia/Cosmos3-Super-Text2Image --omni …`) or the Diffusers
`Cosmos3OmniPipeline`.

**Flux is rejected.** `black-forest-labs/FLUX.1-*` and any Flux derivative is
refused when you submit, before anything is scored — its weights are
non-commercial, which does not work for a subnet that pays for redistributable
artifacts. Submitting a Flux fine-tune returns `400`, not a low score.

**Q-Judger is the only judge.** There is no alternate VLM, and no way to request
one. Its inference is fixed by the model card so your run is comparable with the
champion's: seed 42, temperature 0, `top_k` 1, `top_p` 1.0, repetition penalty
1.05, thinking on, 4096 max new tokens.

## The public split is frozen

```bash
ctx image prompts          # summary: cell count, dataset, holdout commitment
ctx image prompts --json   # every public cell: prompt text, seed, cell key
```

That endpoint publishes the **public split**: the frozen prompt strings, the
sampler recipe, and the exact seed for every cell. Every miner generates the
same prompt ids at the same seeds, so images are directly comparable. A cell is
keyed `p{prompt_id}#v{variation_index}` and its seed is
`sha256(domain ‖ pin_salt ‖ prompt_id ‖ variation_index)`.

The prompts are frozen on purpose. NVIDIA recommends upsampling a prompt into a
JSON document before generation; do that all you like in your own training, but
the scored split uses the exact strings in the pin. You do not bring an
upsampler to the eval. The holdout prompts are never published — only their
commitment and size.

## How you are scored

Q-Judger scores each image across five L1 pillars (Quality, Aesthetics,
Alignment, Real-world Fidelity, Creative Generation). Raw `0|1|2` map to
`0|60|100`, `N/A` is excluded rather than zeroed, and the paper's aggregation
runs level 3 → level 2 → level 1 → total.

Promotion needs all of this, not just a higher total:

| Gate | What it means for you |
|------|----------------------|
| **Holdout win** | You are compared on a **private** prompt split you never see. Winning the public split proves nothing |
| **Paired A/B** | Same prompt, same seed, champion versus you; you must win the head-to-head |
| **No pillar collapse** | No L1 pillar may drop more than ~2 paper points versus the champion. A big Alignment gain will not buy a Quality collapse |
| **Seed replay** | Three pinned `(prompt_id, seed)` cells are regenerated and compared with the outputs you claimed. Non-determinism or different weights than the ones you shipped fails |
| **Prompt faithfulness** | Small agentic checks (count the objects, read the rendered text, check the spatial relation) must agree with Q-Judger's Alignment pillar |
| **No contamination** | If an eval prompt id shows up in your training metadata, the submission is rejected. **Declaring nothing fails too**: an empty manifest leaves the gate with nothing to check, which is a failure, not a clean bill of health (`contamination_evidence_missing`) |
| **Capability canary** | Off the paid number when the eval image emits it. The published image does not; faithfulness and seed-replay are its off-score controls. If a later image measures the slice on **both** sides, dropping more than ~2 paper points below the champion is a hard zero. One-sided absence still fails closed |
| **Public–holdout gap** | A public score far above your holdout reads as memorization and fails. An empty public split fails too |
| **Judge N/A rate** | If Q-Judger declines most items, the run is void |

## Before you submit

```bash
ctx image status
```

`can_score` is the field to read: while it is `false` every submit answers
**503**, nothing is stored, and no pod is rented. The same reply carries
`judge_backend`, `live_harvest_wired`, `champion_baseline_recorded`, and
`base_weights`, plus the live pins, the judge inference parameters, the sampler
recipe, and the holdout commitment and size. It never reveals holdout prompt ids
or text. A run scored offline is reported as `judge_backend: sim` and is never
a real verdict.

## Submit

The manifest is the license attestation. `base` and `base_license` must name the
pinned checkpoint; anything else is a `400`.

```bash
ctx image submit \
  --hotkey 64-hex-hotkey \
  --artifact-digest sha256-of-your-artifact \
  --train-dataset your-training-corpus-id \
  --claimed-output 'p1#v0=sha256-of-your-image-for-that-cell' \
  --wait
```

`ctx` fills `base` with `nvidia/Cosmos3-Super-Text2Image` and `base_license`
with `OpenMDW-1.1`, and omits the sampler so the pinned recipe applies. If you
generated with a different recipe, declare the whole manifest yourself with
`--manifest-file manifest.json`:

```json
{
  "base": "nvidia/Cosmos3-Super-Text2Image",
  "base_license": "OpenMDW-1.1",
  "sampler": {
    "width": 1024,
    "height": 1024,
    "num_inference_steps": 50,
    "guidance_scale": 4.0,
    "flow_shift": 3.0,
    "num_frames": 1,
    "dtype": "bfloat16",
    "scheduler": "UniPCMultistepScheduler"
  },
  "train_prompt_ids": [],
  "train_dataset_ids": ["your-training-corpus-id"],
  "claimed_output_hashes": { "p1#v0": "sha256 of your image for that cell" }
}
```

`train_prompt_ids` is the bench ids your training mix touched (`--train-id`),
and `train_dataset_ids` is the corpora you trained on (`--train-dataset`).
Declare at least one of them: the contamination gate only rejects *overlap*
with the scored split, but a manifest that declares nothing gives it nothing to
check and fails as `contamination_evidence_missing`. Declaring honestly is
strictly better than declaring nothing.

`claimed_output_hashes` is what the seed-replay gate checks. Include at least
the cells the eval asks for; more is fine.

The same submit with `curl`:

```bash
curl -sS -X POST https://network.cortex.foundation/challenge/relearn-image/v1/submissions \
  -H 'content-type: application/json' \
  -H "X-Lium-Api-Key: $LIUM_API_KEY" \
  -d @submission.json
```

## Read the verdict

```bash
ctx image show t2i_0123456789abcdef --wait
```

Eligible runs sit at `awaiting_admin` until an operator promotes. You do not
promote. A contaminated or empty-evidence manifest is rejected **without**
renting anything, so junk never spends a pod.

Never commit the Lium key. If something fails, see [troubleshoot.md](./troubleshoot.md).
