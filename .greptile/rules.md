# Cortex review rules

Live challenge is **Relearn LLM** (`relearn`) only. T2I, encoder-attach Multimodal, and Bounty are not live.

Locked ids: base `Qwen/Qwen3.8-27B`, teacher NVFP4 `LibertAIDAI/GLM-5.3-Flash-NVFP4`. Serve teacher from `RELEARN_TEACHER_LOCAL_DIR` — never pass a Hugging Face repo id to vLLM.

- Do not introduce Modal, Modal tokens, Modal deploy files, or Modal profile names.
- Do not commit secrets, teacher/judge hostnames, API keys, or endpoints. Env var names only.
- Holdout records stay off git. Pins carry `public_ids`, `holdout_commitment`, `holdout_size` — not items.
- General benches are off-path: they may hard-zero a submission and must not appear in miner-visible scores.
- Do not rename `BASE_*` env vars, deployed paths, or `base-*-v1` crypto domain tags.
- `unsafe_code = forbid`. No `unwrap` / `expect` in non-test code.
- Digest-only images in deploy paths. `evil-gateway` is test-only.
