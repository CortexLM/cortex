# Cortex review rules

This is a Bittensor subnet control plane (`CortexLM/cortex`), not an app-platform
or SOC2 checklist. Two **live** challenge ids: `bounty` (5000 bps) and
`proof` (5000). Equal split; sum is 10000. `relearn`, `relearn-image`,
`relearn-agent`, `relearn-mm`, `design`, and `prism` are **off** (no trust-root
row). Relearn* code stays behind the `relearn` / `mm` compose profiles.

- **Fail-closed.** Missing holdout file, commitment mismatch, unpinned eval
  digest, or unset teacher → refuse / 503. Never score the public split as a
  substitute. `*_FORCE_SIM` is CI/local only and must be the *only* way to
  reach a sim scorer: sim is never a fallback for a missing live eval, and the
  resolved backend belongs on `/v1/status` and on the submit row.
- **Absence of evidence is a failed gate.** Empty public split or an undeclared
  miner `manifest` must fail the corresponding gate rather than skip it. A
  one-sided canary fails; both-empty is a skip only on `relearn-image`, whose
  published eval image does not emit that series.
- **A refusal is not a submission.** Fail-closed paths must not persist a row,
  charge a miner, or rent a pod before scoring starts. Report the root cause
  (unpinned digest) rather than a downstream symptom (missing baseline).
- **Champions are measured by the scorer challengers face.** Never seed a live
  host's baseline with sim numbers, and never leave a live host with no
  baseline — the gates are comparisons and cannot run without one.
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
