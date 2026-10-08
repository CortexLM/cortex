# Validator quickstart

A Cortex validator downloads the sealed epoch bundle from the gateway, verifies it and submits the weights on Bittensor subnet 100. `scripts/run-validator.sh` does the setup. The master (gateway, Proof, challenge containers) runs only on the master operator's machine, and this script never starts it.

For the underlying CLI and trust files, see the [validator guide](external-miner/validators.md).

## Prerequisites

- Linux, Python 3.12 or 3.13, [uv](https://docs.astral.sh/uv/), `curl` and libsodium 1.0.18 or newer.
- A clone of this repository.
- A Bittensor wallet with a hotkey registered on netuid 100.
- The gateway's public hotkey, pinned from an independent, trusted source. Don't copy it from the gateway response.

```bash
sudo apt-get install libsodium23
uv sync --locked --extra chain
```

## Three steps

1. Set the wallet and the pinned gateway key.

   ```bash
   export WALLET_NAME=validator WALLET_HOTKEY=default
   export GATEWAY_PUBLIC=<gateway hotkey pinned from an independent source>
   ```

2. Run one verification. It never submits.

   ```bash
   scripts/run-validator.sh --verify-only
   ```

3. Run for real. It verifies and submits every epoch.

   ```bash
   scripts/run-validator.sh
   ```

## Options

Environment variables:

| Variable | Default | Meaning |
| --- | --- | --- |
| `WALLET_NAME` | required | Validator wallet name |
| `WALLET_HOTKEY` | required | Registered validator hotkey |
| `GATEWAY_PUBLIC` | required | Independently pinned gateway hotkey |
| `GATEWAY` | `https://chain.joinbase.ai` | Gateway URL |
| `NETUID` | `100` | Subnet |
| `NETWORK` | `finney` | Bittensor network |
| `STATE_DB` | `$HOME/.cortex/validator.sqlite3` | Local state database |
| `WALLET_PATH` | `$HOME/.bittensor/wallets` | Wallet directory |
| `VERSION_KEY` | read from the chain | Override the subnet `weights_version` |

Flags:

| Flag | Effect |
| --- | --- |
| `--verify-only` | Verify once, never submit |
| `--once` | Run one tick, then exit |
| `--dry-run` | Print the masked command without running it |
| `--help` | Show usage |

## Verify-only mode

`--verify-only` runs one verification and exits without submitting. Use it first, to confirm your trust files, pinned key and network work. `--dry-run` goes one step less: it prints the `cortex validator` command, with the wallet name and hotkey masked, and starts nothing.

## What the validator checks

Before starting, the script:

- fetches `GET $GATEWAY/v1/weights/latest` and requires HTTP 200;
- requires the answer to be JSON with the expected `netuid`;
- reads the minimum trust-file versions from `config/challenges.toml` and `config/measurements.toml`;
- reads the subnet `weights_version` from the chain, unless you set `VERSION_KEY`.

Then `cortex validator` takes over. It uses the owner public key and the signed trust files in `config/`, verifies the sealed bundle against them and the chain, and recomputes the weights from the signed leaves. It doesn't trust the display fields in the gateway answer. See the [validator guide](external-miner/validators.md) for the full rules.

## Safety notes

- The script refuses `master`, `challenge-supervisor` and `vm-host` as arguments, and exits with code 2.
- It also refuses to run if `CORTEX_MASTER=1`, `BASE_MASTER_SECRETS_HOST_DIR` or `BASE_CHALLENGE_SECRETS_HOST_DIR` is set. Unset them first.
- Pin `GATEWAY_PUBLIC` yourself. A key taken from the gateway proves nothing.
- Keep wallet files private. `--dry-run` masks the wallet name and hotkey in its output.
- Run `--verify-only` before the first real run.

## FAQ

**Do I need to run challenge containers or Docker?**
No. Validators never run challenge containers or Proof evaluation, and they need no registry or challenge token.

**Do I need a GPU or a KVM host?**
No. Those are for the master operator.

**The script says `GATEWAY_PUBLIC is not set`.**
Export the gateway hotkey you pinned from an independent source.

**The script says the netuid doesn't match.**
The gateway answered for a different subnet than `NETUID`. Check `GATEWAY` and `NETUID`.

**It can't read `weights_version`.**
Set `VERSION_KEY` yourself, or check that `NETWORK` is reachable.

**Where do the trust files come from?**
From `config/` in your clone. See the [trust-root ceremony](how-to/trust-root.md).

**Does the validator CLI need anything else?**
The [validator guide](external-miner/validators.md) lists the current requirements for the underlying CLI, such as the consensus seed file.
