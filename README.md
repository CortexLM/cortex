# Cortex

Cortex research subnet on Bittensor: agentic Proof plus Docker challenge containers.

The master runs the gateway, Proof and every challenge container, then signs one
leaf per miner and seals each epoch. Validators only verify the sealed bundle
from the gateway API and submit the weights.

| Role | Runs | Command |
| --- | --- | --- |
| validator master | gateway, Proof, [challenge containers](docs/CHALLENGES.md), auto-updater | `cortex master` + `cortex challenge-supervisor` |
| validator | verification and weight submission, nothing else | `cortex validator` |

Challenges live in their own repositories and are loaded automatically:
[CortexLM/bounty](https://github.com/CortexLM/bounty) (vulnerability reports) and
[OpentypeAI/challenge](https://github.com/OpentypeAI/challenge) (exact-gold
DiffusionGemma duels). The owner-signed trust root sets each challenge's emission
share. The shortfall of a challenge burns to UID 0 and never moves to another one.

Cortex is experimental research software. Offline tests exercise submission, scoring,
sealing and validator dispatch through fake external boundaries. A live model
smoke is distinct from live KVM execution or confirmed on-chain payment.

## Installation

Linux, Python 3.12 or 3.13, `uv`, and libsodium 1.0.18 or newer are required.
The VM host additionally requires working `/dev/kvm`, Firecracker and jailer.

```bash
sudo apt-get install libsodium23
uv sync --locked --extra chain --group build
uv run cortex --help
```

## Usage

```bash
uv run cortex master --help
uv run cortex challenge-supervisor --help
uv run cortex validator --help
uv run cortex vm-host --help
uv run cortex miner --help
uv run cortex topic-create --help
```

Configure the master using private credential files and signed trust roots.
An operator gives Proof a research objective; its topic agent prepares an
experiment, measures a baseline and proposes rules and miner documentation.
A topic opens only after the control plane verifies the host's execution
receipts and signs the resulting document. No research task catalog is built in.

- [Operator configuration](docs/reference/configuration.md)
- [Architecture and trust boundaries](docs/ARCHITECTURE.md)
- [Challenge container contract](docs/CHALLENGES.md)
- [Deployment](deploy/README.md)
- [Proof miner guide](docs/external-miner/proof.md)
- [Bounty miner guide](docs/external-miner/bounty.md)
- [Validator guide](docs/external-miner/validators.md)
- [Documentation index](docs/index.md)

## Development

```bash
uv run ruff format --check src tests scripts
uv run ruff check src tests scripts
uv run mypy
uv run pytest -m 'not live'
uv run python scripts/check_deploy.py --check-examples
uv build --no-build-isolation
```

Tests cover signatures and Rust wire vectors, replay protection, challenge
container outages, auto-update rollback, artifact validation, topic setup, rejected submissions, VM lifecycle failures,
RLM recursion/compaction, reward allocation and sealed-weight submission.
CI runs offline and never rents a GPU or boots Firecracker.

## Contributing

Keep research content out of source control. Add tests for observable behavior
at service boundaries, and update the matching miner documentation when an API
changes. Follow [the agent contract](AGENTS.md) and the PR review requirements.

## License

[Apache-2.0](LICENSE).
