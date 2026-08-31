# Cortex review rules

This is a Bittensor subnet control plane (`CortexLM/cortex`), not an app-platform
or SOC2 checklist. Challenge ids in-tree: `relearn`, `relearn-t2i`,
`relearn-mm`, `bounty`. The **live** path is Relearn LLM (`relearn`). T2I,
encoder-attach Multimodal, and Bounty stay off but their contracts still apply
when those files are touched.

- **Fail-closed.** Missing holdout file, commitment mismatch, unpinned eval
  digest, or unset teacher → refuse / 503. Never score the public split as a
  substitute. `*_FORCE_SIM` is CI/local only and must be the *only* way to
  reach a sim scorer: sim is never a fallback for a missing live eval, and the
  resolved backend belongs on `/v1/status` and on the submit row.
- **Absence of evidence is a failed gate.** Empty public split, missing general
  canary, or an undeclared miner `manifest` must fail the corresponding gate
  rather than skip it.
- **Holdout stays off git.** Pins may carry `public_ids`, `holdout_commitment`,
  `holdout_size`. Do not commit holdout items, prompts, salts, or canary benches.
- **No Modal.** No Modal tokens, deploy files, profile names, or `modal.com`
  hosts. No teacher/judge hostnames, API keys, or endpoints — env var names only
  (`RELEARN_TEACHER_*`, `RELEARN_HOLDOUT_FILE`, `RELEARN_CANARY_FILE`).
- **No Flux.** T2I generator is `nvidia/Cosmos3-Super-Text2Image` (OpenMDW 1.1).
  `FLUX.1-*` and Flux derivatives are refused, not low-scored.
- **OSI-permissive encoders only** on the archived MM path (Apache-2.0 / MIT /
  BSD / ISC). OpenRAIL and NC licenses reject at submit. Live Relearn LLM is a
  native VLM (`Qwen/Qwen3.8-27B`); do not reintroduce a SigLIP encoder-attach
  product as live.
- **Do not rename** `BASE_*` env vars, deployed paths (`/opt/base`, `/run/base`),
  or `base-*-v1` crypto domain tags.
- `unsafe_code = forbid`. No `unwrap` / `expect` in non-test code.
- Digest-only images in deploy paths. `evil-gateway` is test-only.
- Frozen specs (`BUNDLE_SPEC`, `DESIGN_CHALLENGE`) — do not weaken scoring or
  consensus semantics.
