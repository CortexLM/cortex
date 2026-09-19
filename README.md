# Cortex

Cortex research subnet: Bounty and agentic Proof on Bittensor.

The Python implementation runs the gateway and both challenges on a master,
verifies sealed rewards in independent validators, and isolates research work
in Firecracker guests. Bounty receives 20% of emission; Proof receives 80%.
Proof uses Cortex's own recursive language-model engine with persistent memory,
context compaction and bounded tool execution.

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

Tests cover signatures and Rust wire vectors, replay protection, feed outages,
artifact validation, topic setup, rejected submissions, VM lifecycle failures,
RLM recursion/compaction, reward allocation and sealed-weight submission.
CI runs offline and never rents a GPU or boots Firecracker.

## Contributing

Keep research content out of source control. Add tests for observable behavior
at service boundaries, and update the matching miner documentation when an API
changes. Follow [the agent contract](AGENTS.md) and the PR review requirements.

## License

[Apache-2.0](LICENSE).
