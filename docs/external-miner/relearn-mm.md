<!-- protocol_version: 1 -->

# Relearn Multimodal — miners

> **This challenge is off.** `relearn-mm` has no row in
> [`config/challenges.toml`](../../config/challenges.toml), so it has **no
> emission** and no leaf signed by its key can verify. Submitting to it earns
> nothing. The vision work miners came here for now lives in
> [Relearn](./relearn.md) — `Qwen/Qwen3.8-27B` is a native VLM and the holdout
> already carries captioning, VQA, OCR, and spatial families with a
> pixel-shuffle control. The agentic side lives in
> [Relearn Agent](./relearn-agent.md).
>
> The page stays for the archived encoder pins and the licence rules, which
> still apply if the challenge is turned back on with a trust-root ceremony.

Give the champion Relearn LLM eyes, without breaking its language ability. Long
guide, eval image, and harness:
[CortexLM/relearn](https://github.com/CortexLM/relearn).
Cortex pin: [`config/relearn-mm-pin.toml`](../../config/relearn-mm-pin.toml).

Miner pays Lium (`LIUM_API_KEY` / `X-Lium-Api-Key`).

## What you start from

| Thing | Value |
|-------|-------|
| Language side | the champion Relearn LLM (base `Qwen/Qwen3.8-27B`) |
| Vision encoder pin | `google/siglip2-so400m-patch14-384` (Apache-2.0) |
| Accepted encoder licenses | **Apache-2.0, MIT, BSD-2/3-Clause, ISC** |

You train the vision encoder and the projector. You may also improve the LLM. You
may substitute a different encoder — the pin is a default, not a lock — as long
as its model card license is OSI-permissive. OpenRAIL, `cc-by-nc-*`, and
bespoke community licenses are refused when you submit, because the artifact has
to stay redistributable. Verified Apache-2.0 alternatives include
`google/siglip-so400m-patch14-384` and the SigLIP tower inside
`HuggingFaceM4/idefics2-8b`.

## How you are scored — two gates, both mandatory

**Gate 1: the LLM must survive.** Your submitted language model is rerun on the
Relearn text holdout with the vision modules ignored. If it scores below
`champion − 0.01` you get **zero on this challenge** — not a reduced score —
regardless of how good your vision numbers are. You are paid to add sight, not
to trade language ability for it.

If you submit `"kind": "encoder_only"`, your LM weights must hash-match the
champion exactly. `GET /challenge/relearn-mm/v1/status` publishes
`champion_lm_weights_hash`; put that value in `lm_weights_hash`. If you also
changed the LM, submit `"kind": "encoder_and_lm"` and the hash is free to
differ — but then the text score alone has to hold up.

**Gate 2: the vision side must actually improve.** A frozen image holdout across
four families — captioning, VQA, OCR / text-in-image, spatial relations — plus
agentic traces where the model has to look at a screenshot, diagram, or UI and
then call a tool or answer. Every family must have items.

The traces carry a control: each one is replayed with the image pixels shuffled.
A model that really reads the image gets materially worse; one that guesses from
the text prompt does not move. If your shuffled score barely drops, the run
fails on `ignores_the_image` no matter what the clean score was.

Both comparisons are champion-versus-challenger on the **private** holdout. The
public split is informational.

| Gate | What it means for you |
|------|----------------------|
| **LM intact** | Text holdout ≥ champion − ε, or the whole submission is zero |
| **LM weights** | `encoder_only` must hash-match the champion LM |
| **Vision win** | Paired win on the private image holdout |
| **Task coverage** | All four families must be scored |
| **Agentic win** | Paired win on the tool-using traces |
| **Pixel shuffle** | Shuffled-image score must fall materially |
| **Encoder license** | OSI-permissive only |

## Submit

```bash
curl -sS -X POST https://<gateway>/challenge/relearn-mm/v1/submissions \
  -H 'content-type: application/json' \
  -H "X-Lium-Api-Key: $LIUM_API_KEY" \
  -d '{
    "miner_hotkey": "<64-hex hotkey>",
    "artifact_digest": "<sha256 of your artifact>",
    "artifact_uri": "optional-url",
    "manifest": {
      "encoder_model": "google/siglip2-so400m-patch14-384",
      "encoder_license": "apache-2.0",
      "projector": "2-layer MLP, 1152 -> 4096",
      "kind": "encoder_only",
      "lm_weights_hash": "<champion_lm_weights_hash from /v1/status>"
    }
  }'
```

Poll `GET /challenge/relearn-mm/v1/submissions/{id}`. Eligible runs sit at
`awaiting_admin` until an operator promotes. You do not promote.

Never commit the Lium key. If something fails, see [troubleshoot.md](./troubleshoot.md).
