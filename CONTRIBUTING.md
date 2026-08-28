# Contributing to Cortex

This is the Rust control-plane monorepo for the Cortex Bittensor subnet
([`CortexLM/cortex`](https://github.com/CortexLM/cortex)).

## Before you start

1. Read [AGENTS.md](AGENTS.md) (repo map, gates, what must not break).
2. Read [docs/NAMING.md](docs/NAMING.md) before renaming anything that looks
   like `base` / `BASE_*`.
3. Frozen specs (`docs/BUNDLE_SPEC.md`, `docs/DESIGN_CHALLENGE.md`) are
   pinned by xtask. Do not change incentive, scoring, or consensus semantics
   unless that is the explicit task.

## Development setup

- Rust **1.96.0** via `rust-toolchain.toml` (`rustfmt`, `clippy`).
- Optional: `./scripts/install-githooks.sh` so `commit-msg` / `pre-commit`
  match CI.

```bash
cargo test --workspace
```

That is the core gate. Before opening a PR, also run what CI runs:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo deny check
cargo run -p xtask -- loc-cap
cargo run -p xtask -- consensus-lint
cargo run -p xtask -- spec-check
cargo run -p xtask -- design-check
cargo run -p xtask -- external-docs-check
```

Local subnet stack (Docker Compose, secrets via age): see
[deploy/README.md](deploy/README.md) and
[docs/runbooks/local-testnet-e2e.md](docs/runbooks/local-testnet-e2e.md).

```bash
./deploy/scripts/materialize-env.sh
./deploy/scripts/local-e2e.sh --smoke
```

## Pull requests

- Target **`main`**.
- Use a [pull request template](.github/PULL_REQUEST_TEMPLATE.md).
- Keep diffs scoped. Branding and docs PRs must not rewrite protocol bytes.
- `unsafe_code` is forbidden. No `unwrap` / `expect` outside tests.

## Commit messages

A `commit-msg` hook enforces Conventional Commits:

```text
type(scope): summary
```

- `type` is one of: `feat`, `fix`, `refactor`, `perf`, `test`, `docs`,
  `chore`, `build`, `ci`, `style`, `revert`.
- Subject starts with a **lowercase** letter.
- Entire subject ≤ **72** characters.
- `Merge` and `Revert` subjects are allowed as-is.

Examples: `docs(readme): describe cortex miner http path`,
`feat(config): accept CORTEX_* env aliases`.

## Issues

Use the GitHub issue templates (bug / feature). GitHub Discussions are
**not** enabled on this repository.

## Security

Do not file public issues for vulnerabilities. See [SECURITY.md](SECURITY.md).

## Code of conduct

[CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md). Contact owners listed in
[CODEOWNERS](CODEOWNERS) via GitHub.
