<!-- protocol_version: 1 -->

# Relearn Image — miners

Challenge id: **`relearn-image`**.

Fine-tune the pinned text-to-image generator and beat the champion on prompts
you cannot see. Long guide, eval image, and harness:
[CortexLM/relearn](https://github.com/CortexLM/relearn).
Cortex pin: [`config/relearn-t2i-pin.toml`](../../config/relearn-t2i-pin.toml)
(the file keeps the pre-launch `t2i` spelling; the challenge id is `relearn-image`).

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

## How you are scored

`GET /challenge/relearn-image/v1/prompts` publishes the **public split**: the
frozen prompt strings, the sampler recipe, and the exact seed for every cell.
Every miner generates the same prompt ids at the same seeds, so images are
directly comparable. A cell is keyed `p{prompt_id}#v{variation_index}` and its
seed is `sha256(domain ‖ pin_salt ‖ prompt_id ‖ variation_index)`.

The prompts are frozen on purpose. NVIDIA recommends upsampling a prompt into a
JSON document before generation; do that all you like in your own training, but
the scored split uses the exact strings in the pin. You do not bring an
upsampler to the eval.

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
| **No contamination** | If an eval prompt id shows up in your training metadata, the submission is rejected. **Declaring nothing fails too**: an empty manifest leaves the gate with nothing to check, which is a failure, not a clean bill of health |
| **Capability canary** | A fixed general-prompt slice, scored on the same run and **kept out of the number you are paid on**. Dropping more than ~2 paper points below the champion on it is a hard zero — buying holdout points by wrecking everything the generator could already do is not an improvement |
| **Public–holdout gap** | A public score far above your holdout reads as memorization and fails. An empty public split fails too |
| **Judge N/A rate** | If Q-Judger declines most items, the run is void |

## Submit

The manifest is the license attestation. `base` and `base_license` must name the
pinned checkpoint; anything else is a `400`.

```bash
curl -sS -X POST https://<gateway>/challenge/relearn-image/v1/submissions \
  -H 'content-type: application/json' \
  -H "X-Lium-Api-Key: $LIUM_API_KEY" \
  -d '{
    "miner_hotkey": "<64-hex hotkey>",
    "artifact_digest": "<sha256 of your artifact>",
    "artifact_uri": "optional-url",
    "manifest": {
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
      "claimed_output_hashes": {
        "p1#v0": "<sha256 of your image for that cell>"
      }
    }
  }'
```

`train_prompt_ids` is the bench ids your training mix touched, and
`train_dataset_ids` is the corpora you trained on. Declare at least one of
them: the contamination gate only rejects *overlap* with the scored split, but
a manifest that declares nothing gives it nothing to check and fails as
`contamination_evidence_missing`. Declaring honestly is strictly better than
declaring nothing.

`claimed_output_hashes` is what the seed-replay gate checks. Include at least
the cells the eval asks for; more is fine.

Poll `GET /challenge/relearn-image/v1/submissions/{id}`. Eligible runs sit at
`awaiting_admin` until an operator promotes. You do not promote.

`GET /challenge/relearn-image/v1/status` reports the live pins, the judge
inference parameters, the sampler recipe, and the holdout commitment and size.
It never reveals holdout prompt ids or text.

It also reports whether this host can score at all: `judge_backend`,
`force_sim`, `live_harvest_wired`, `champion_baseline_recorded`, and
`can_score`. When `can_score` is false every submit answers **503** and nothing
is stored — that is the host being unready, not your artifact being rejected,
so retry rather than resubmitting variants. A run scored offline is reported as
`judge_backend: sim` and is never a real verdict.

Never commit the Lium key. If something fails, see [troubleshoot.md](./troubleshoot.md).
