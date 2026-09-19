# Contributing to Cortex

Cortex is a Python Bittensor subnet with two live challenges: Bounty and Proof.
Read [AGENTS.md](AGENTS.md), the [architecture](docs/ARCHITECTURE.md), and the
[naming contract](docs/NAMING.md) before changing protocol or deployment code.

## Development setup

Linux, Python 3.12 or 3.13, `uv`, Docker Compose, and libsodium are required.

```bash
sudo apt-get install libsodium23
uv sync --locked --extra chain --group build
```

Run the same offline gates as CI:

```bash
uv run --no-sync ruff format --check src tests scripts
uv run --no-sync ruff check src tests scripts
uv run --no-sync mypy
uv run --no-sync pytest -m 'not live'
uv run --no-sync python scripts/check_repo.py --final
uv run --no-sync python scripts/check_deploy.py --check-examples
uv build --no-build-isolation
```

CI never rents a GPU, contacts OpenRouter, boots Firecracker, or submits chain
weights. Tests substitute those external boundaries while exercising real HTTP,
SQLite, signatures, RLM state, scoring, sealing, and validator verification.

## Behavioral contracts

- Do not change the frozen files named by `scripts/check_repo.py`.
- Preserve the `BASE_*` aliases and `base-*-v1` signature domains.
- Keep challenge content out of Git. Proof topics are signed operator data.
- A product/API change must update the matching `docs/external-miner/` page.
- Never weaken a fail-closed path to make a smoke test pass.
- Every bug fix needs a regression test at the public boundary it affected.

## Pull requests

Target `main`, fill in the pull request template, and request a Greptile review.
Use Conventional Commit subjects no longer than 72 characters:

```text
type(scope): lowercase summary
```

Supported types are `feat`, `fix`, `refactor`, `perf`, `test`, `docs`, `chore`,
`build`, `ci`, `style`, and `revert`.

## Security

Do not publish vulnerability details or credentials in issues, logs, fixtures, or
test output. Use the process in [SECURITY.md](SECURITY.md).

## Code of conduct

Participation is governed by [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md).
