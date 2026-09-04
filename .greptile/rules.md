# Cortex review rules

This is a Bittensor subnet control plane (`CortexLM/cortex`), not an app-platform
or SOC2 checklist. Two **live** challenge ids: `bounty` (2000 bps) and
`proof` (8000). Proof-weighted 20%/80% regardless of eval digest. Proof's
eval digest is empty (503); do not invent a sha256. Sum is 10000. `relearn`, `relearn-image`,
`relearn-agent`, `relearn-mm`, `design`, and `prism` are **removed as products** (no trust-root
row, no compose services). Historical miner stubs stay under `docs/external-miner/`. Frozen
specs remain for xtask gates. Leftover `prism-*` crates are the Lium harvest stack used by Proof.

- **Fail-closed.** Missing holdout file, commitment mismatch, unpinned eval
  digest, or unset judge credentials → refuse / 503. Never score the public split as a
  substitute. `*_FORCE_SIM` is CI/local only and must be the *only* way to
  reach a sim scorer: sim is never a fallback for a missing live eval, and the
  resolved backend belongs on `/v1/status` and on the submit row.
- **Absence of evidence is a failed gate.** Empty public split or an undeclared
  miner `manifest` must fail the corresponding gate rather than skip it.
- **A refusal is not a submission.** Fail-closed paths must not persist a row,
  charge a miner, or rent a pod before scoring starts. Report the root cause
  (unpinned digest) rather than a downstream symptom (missing baseline).
- **Champions are measured by the scorer challengers face.** Never seed a live
  host's baseline with sim numbers, and never leave a live host with no
  baseline — the gates are comparisons and cannot run without one.
- **Holdout stays off git.** Pins may carry `public_ids`, `holdout_commitment`,
  `holdout_size`. Do not commit holdout items, prompts, salts, or canary benches.
- **No Modal.** No Modal tokens, deploy files, profile names, or `modal.com`
  hosts. No teacher/judge hostnames, API keys, or endpoints — env var names only.
- **Do not rename** `BASE_*` env vars, deployed paths (`/opt/base`, `/run/base`),
  or `base-*-v1` crypto domain tags.
- `unsafe_code = forbid`. No `unwrap` / `expect` in non-test code.
- Digest-only images in deploy paths. `evil-gateway` is test-only.
- Frozen specs (`BUNDLE_SPEC`, `DESIGN_CHALLENGE`) — do not weaken scoring or
  consensus semantics.
