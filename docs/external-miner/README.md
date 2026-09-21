<!-- protocol_version: 1 -->

# Cortex mining

Submit research to Proof or report product bugs to Bounty using a Bittensor hotkey.

The Python implementation is under integration. A healthy service, a successful
model call or an accepted submission does not prove deployment, scientific
reproduction or on-chain payment.

| Challenge | Emission share | Guide |
|-----------|----------------|-------|
| `bounty` | up to 3000 bps (30%), algorithm 2 | [Pair an account and report bugs](bounty.md) |
| `proof` | 7000 bps (70%), algorithm 2 | [Discover topics and submit research](proof.md) |

The legacy owner-signed profile remains 2000/8000 until
[algorithm 2 activation](../how-to/trust-root.md#activate-proportional-bounty).
These are the only live challenge ids. Proof topics are operator-published,
signed documents discovered through the API, never a built-in catalog. No
particular benchmark, runner, model or topic is promised by this repository.

## Installation

Use Linux, Python 3.12 or 3.13, `uv` and libsodium 1.0.18 or newer. From this
repository:

```bash
sudo apt-get install libsodium23
uv sync --locked --extra chain
uv run cortex miner --help
```

`cortex` is the Python CLI. The historical `ctx` installer and command examples
do not describe this implementation. See the [repository README](../../README.md)
for installation and development commands.

## Usage

Get the gateway URL and independently pinned Proof public key from the subnet
operator. `--gateway` is required; the CLI does not select a deployment for you.
The public challenge route prefixes are `/challenge/bounty` and
`/challenge/proof`.

```bash
uv run cortex miner --gateway "$GATEWAY" \
  --wallet-name research --wallet-hotkey miner proof-submit --help
```

The miner uses a standard Bittensor wallet. `--wallet-name` selects the coldkey
wallet directory; `--wallet-hotkey` selects its signing hotkey. The coldkey
secret is not needed for research submissions.

| Option | Meaning |
|--------|---------|
| `--wallet-name NAME` | Existing Bittensor wallet directory name |
| `--wallet-hotkey NAME` | Existing hotkey name; default `default` |
| `--wallet-path PATH` | Wallet root; default `~/.bittensor/wallets` |
| `--wallet-password-file PATH` | Private password file required for an encrypted hotkey; no interactive prompt |
| `--proof-public KEY` | Independently pinned Proof signing public key, SS58 or hex; required for `proof-submit` |
| `--dev-seed-file PATH` | Development-only 32-byte hexadecimal seed in a private file; mutually exclusive with `--wallet-name` |

Place these options before the miner action. Password, development seed, BYOK
and Bounty session files must be private (for example mode `0600`) and must not
be symlinks. Wallet loading never creates a wallet or rewrites its key files.
Never put a mnemonic, private key, session token or provider credential in Git
or a support ticket. Public challenge signing keys are verification material,
not miner secrets.

## Support

- [Proof guide](proof.md): signed topics, artifacts, BYOK and submission outcomes.
- [Bounty guide](bounty.md): terms, pairing, reports and the external scoring feed.
- [Validator guide](validators.md): independently verify seals and peer roots.
- [Troubleshooting](troubleshoot.md): refusal codes and safe retry behavior.

Bundle `protocol_version` remains `1`; Python Proof topic documents use
`schema_version: 2`. The [bundle specification](../BUNDLE_SPEC.md) defines the
frozen consensus wire format. The public documentation contract is checked by
`uv run python scripts/check_repo.py`.

`relearn`, `relearn-image`, `relearn-agent`, `relearn-mm`, `design` and `prism`
are retired products with no trust-root row or emission. Their historical
[Relearn](relearn.md), [image](relearn-image.md), [agent](relearn-agent.md) and
[multimodal](relearn-mm.md) pointers remain for old links only.

## License

[Apache-2.0](../../LICENSE).
