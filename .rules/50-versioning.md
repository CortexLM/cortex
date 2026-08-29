# 50 — Automatic versioning

## Single source of truth

`[workspace.package] version` in the root `Cargo.toml`. Nothing else holds a
version by hand:

- all 88 workspace members inherit it with `version.workspace = true`;
- `Cargo.lock` is rewritten from it by the bump command;
- prod releases are annotated git tags `vX.Y.Z` cut on `main`, and the tag must
  match the workspace version.

There is no `VERSION` file, no `package.json`, and no second packaging system.

## Level comes from Conventional Commits

| Commit on the branch | Required bump |
|----------------------|---------------|
| `feat…` | **minor** |
| `!` before the colon, or a `BREAKING CHANGE:` footer | **major** |
| anything else (`fix`, `refactor`, `perf`, `test`, `docs`, `chore`, `build`, `ci`, `style`, `revert`) | **patch** |

**Zero-major rule:** while the version is `0.y.z`, a breaking change maps to
**minor**, because cargo treats `0.y` as the compatibility unit. The repo does
not promote itself to `1.0.0` by accident — that is a deliberate, human call.

The gate requires the bump to be **at least** the derived level. Bumping higher
than required is always allowed.

## Commands

```bash
cargo run -p xtask -- version                                # print current version
cargo run -p xtask -- version check                          # members inherit + Cargo.lock in sync
cargo run -p xtask -- version bump                           # auto level from commits vs origin/main
cargo run -p xtask -- version bump --level patch             # or minor / major
cargo run -p xtask -- version verify-bump --base origin/main # the CI gate, run locally
```

`version bump` edits `Cargo.toml` and rewrites the workspace-member entries in
`Cargo.lock` in place — no network, no registry refresh. Commit both files.

## CI enforcement

`.github/workflows/pr-gate.yml` runs `version verify-bump --base origin/main`
on every pull request. It fails when:

- the version is unchanged versus the merge base;
- the version went backwards;
- the bump is smaller than the commit subjects require.

Narrow exemption: a PR whose changed files are **only** under `deploy/pins/`,
`deploy/digests/`, or `metadata/` needs no bump. Those are machine-written pin
artifacts, normally committed straight to `main` by `images.yml`.

`ci.yml` additionally runs `version check`, so a hand-edited manifest or a
stale `Cargo.lock` fails on push as well as on PRs.

## Releasing

```bash
git switch main && git pull
cargo run -p xtask -- version            # e.g. 0.2.0
git tag -a v0.2.0 -m 'v0.2.0'
git push origin v0.2.0                   # triggers deploy-prod.yml
```

Tags are append-only. Never rewrite or move a released tag, and never rewrite
git history to "fix" a version.
