# Private deployment files

This directory contains documentation only. Secret bytes are never committed.
Application containers run as UID/GID 65532; mounted files must be private
regular files, owned/readable by that identity, with mode 0400 or 0600.

## Master mount

`deploy/secrets/master/` is mounted read-only at `/run/secrets`:

| File | Purpose |
| --- | --- |
| `gateway.key` | 32-byte sr25519 seed for bundle seals |
| `<id>.key` | leaf seed of each trusted container challenge, e.g. `bounty.key` |
| `proof.key` | Proof topic/leaf seed matching the trust root |
| `operator.token` | bearer for master administrative routes |
| `proof-vm.token` | bearer shared with the dedicated VM host |
| `proof-vm-ca.pem` | CA used to verify the VM-host TLS certificate |

The final two files are required only when Proof VM orchestration is configured.
The CA certificate is public material but remains operator-managed because it
controls the authenticated host boundary.

## Challenge secrets

`$BASE_CHALLENGE_SECRETS_HOST_DIR/<id>/` is outside this repository. The gateway
mounts the whole directory read-only at `/run/challenge-secrets` and reads only
`<id>/internal.token`. The supervisor bind-mounts `<id>/` read-only at
`/run/secrets` inside that challenge container, without reading it:

| File | Purpose |
| --- | --- |
| `internal.token` | master bearer for `get_weights` |
| `admin.token` | optional operator bearer for the challenge's admin routes |
| challenge-specific | e.g. Bounty `session.key`; see the challenge's operator guide |

## Validator mounts

The wallet tree is mounted read-only at `/run/wallets`. A separate validator
identity directory is mounted at `/run/validator` and contains
`consensus.key`, `tls.crt` and `tls.key`. The validator never receives gateway,
challenge, Proof, operator or provider credentials.

## VM-host files

Private host files live under `/etc/proof-vm`, outside this repository:

| File | Purpose |
| --- | --- |
| `token` | counterpart of the master's `proof-vm.token` |
| `tls.key` / `tls.crt` | SAN-valid HTTPS identity |
| `openrouter.key` | owner-funded RLM inference credential |
| `inference-offer.json` | signed model/provider offer; contains no API key |
| `owner.pubkey` | optional Ed25519 key for shared-knowledge approvals |

Runtime state, miner BYOK, evaluator offers, topic evidence, experiment packs,
rootfs images, kernels and retained consoles also stay outside Git. Compute every
pin from the final installed bytes; never create a digest-shaped placeholder.
