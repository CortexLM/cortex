## Summary

<!-- What does this PR change, and why? -->

## Rules attestation (required — `pr-gate` fails a ready PR without all four)

Tick only what you actually did. See [`.rules/30-pr.md`](../.rules/30-pr.md).

- [ ] I read all of `.rules/` before opening this PR and before marking it ready
- [ ] `AGENTS.md`, `README.md` and `.rules/` are accurate for this change, or N/A with a reason below
- [ ] Local pre-prod gates in `.rules/20-pre-prod-local.md` all passed
- [ ] Version bumped per `.rules/50-versioning.md`

Version before → after: <!-- e.g. 0.2.0 → 0.2.1 -->

N/A reasons (if any): <!-- which box, and why it does not apply -->

## Local gates run

<!-- Paste or trim; these are the commands from .rules/20-pre-prod-local.md -->

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check
cargo run -p xtask -- loc-cap
cargo run -p xtask -- consensus-lint
cargo run -p xtask -- spec-check
cargo run -p xtask -- design-check
cargo run -p xtask -- external-docs-check
cargo run -p xtask -- rules-check
cargo run -p xtask -- version check
```

## Challenge verification (if this PR touches a challenge)

<!-- Healthz is not proof. Which submission did you simulate: baseline, cheat,
     admin winners, edges, leaf → seal → sealed: true? -->

## Risk

<!-- Deploy, miner CVM measurement, signature domain, or emission impact. -->

## Naming

I did **not** rename `BASE_*` environment variables, deployed host paths
(`/opt/base`, `/run/base`, …), GHCR `baseintelligence/base` package names, or
`base-*-v1` cryptographic domain tags, unless this PR’s purpose is a coordinated
cutover documented in [`.rules/60-naming.md`](../.rules/60-naming.md).
