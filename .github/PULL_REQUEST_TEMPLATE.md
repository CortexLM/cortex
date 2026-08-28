## Summary

<!-- What does this PR change, and why? -->

## Test plan

- [ ] `cargo test --workspace` (or note the subset and why)
- [ ] `cargo fmt --all -- --check`
- [ ] Clippy / deny / xtask gates if this PR touches crates they cover

## Risk

<!-- Deploy, miner CVM measurement, signature domain, or emission impact. -->

## Naming

I did **not** rename `BASE_*` environment variables, deployed host paths
(`/opt/base`, `/run/base`, …), GHCR `baseintelligence/base` package names, or
`base-*-v1` cryptographic domain tags, unless this PR’s purpose is a coordinated
cutover documented in `docs/NAMING.md`.
