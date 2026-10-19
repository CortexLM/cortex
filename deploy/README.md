# Deploy Cortex

Cortex uses three independently operated roles:

| Role | Runtime | Responsibility |
| --- | --- | --- |
| master | Docker Compose | public gateway, Proof, challenge containers and durable epoch emission |
| validator | Docker Compose | independent bundle verification, peer roots and Bittensor submission |
| Proof VM host | systemd on a KVM machine | Firecracker topic and experiment VMs |

The gateway exists only on the master. A validator never receives challenge
signing keys and never executes miner code. Production runs the VM host on a
dedicated machine reachable from the master over a private network and HTTPS.

- [Validate the deployment source](#validate-the-deployment-source)
- [Build and publish the Python image](#build-and-publish-the-python-image)
- [Master](#master)
- [Challenge containers](#challenge-containers)
- [Validator](#validator)
- [Proof VM host](#proof-vm-host)
- [Promotion and rollback](#promotion-and-rollback)

## Validate the deployment source

```bash
uv run python scripts/check_deploy.py --check-examples
```

The contract check requires digest-pinned images, read-only non-root application
containers, dropped capabilities, explicit bind addresses, private file mounts,
durable state and strict separation between master and validator credentials. It
does not start a service.

## Build and publish the Python image

`deploy/Dockerfile` has two final targets:

- `runtime` includes the chain dependency and the `cortex` entry point for
  master and validator;
- `guest` excludes Bittensor and boots `proof-guest-init` for conversion into a
  Firecracker rootfs.

The base image is an exact Python 3.12 Bookworm digest. On every push to `main`,
`.github/workflows/images.yml` waits for CI, builds the runtime once, exercises
it without network access, generates a CycloneDX SBOM, rejects fixable high or
critical vulnerabilities, publishes the tested bytes to GHCR under the source
commit and records GitHub build provenance. Annotated release tags only alias
that existing attested digest; they never rebuild it. The privileged workflow
has no manual entry point. It does not build or publish the unwired Proof guest,
boot init, Firecracker or KVM. Do not deploy a local tag or copy a digest from
another build.

```bash
docker build --file deploy/Dockerfile --target runtime \
  --build-arg PYTHON_IMAGE='python:3.12-slim-bookworm@sha256:<verified-digest>' \
  --tag cortex-python:test .
docker run --rm --network none --read-only --cap-drop ALL cortex-python:test --help
```

The guest rootfs and kernel are operator artifacts. Build the Python `guest`
stage and convert it to ext4 with `deploy/guest/bake-rootfs.sh`. The builder
normalizes filesystem time and ext4 metadata, verifies the result with
`e2fsck`, names it from the SHA-256 of the final bytes and never writes a live
pin. Follow the [guest rootfs procedure](../docs/how-to/build-guest-rootfs.md),
install the measured image under `/var/lib/proof/images`, and put only that
value in `host.toml`. The repository contains no fabricated rootfs or kernel
digest.

## Master

Create `deploy/env/master.env` from the example and set the primary Bittensor
network plus optional bounded `wss://` fallback RPC origins, then set:

- `CORTEX_IMAGE` to the published `repository@sha256:<64 hex>`;
- the master VPC bind address and netuid;
- `BASE_CHALLENGE_SECRETS_HOST_DIR`, the absolute host directory of challenge
  bearers, and `BASE_DOCKER_GID`, the group owning `/var/run/docker.sock`;
- the Proof VM URL, CA, exact rootfs digest, signed inference-offer commitment
  and registered custom IDs when Proof custom topics should open;
- explicit `PROOF_RLM_VM_VCPUS`, `PROOF_RLM_VM_MEM_MIB` and
  `PROOF_RLM_VM_DISK_MIB` values that fit the VM host ceilings and capacity.

Prepare a private directory, owned so container UID 65532 can read it:

```text
deploy/secrets/master/
  proof.key           # signs Proof topics (miners pin its public key)
  operator.token      # admin routes and sealing
  intake.token        # raw leaf intake only; a different value
  proof-vm.token
  proof-vm-ca.pem
```

The seed is raw 32 bytes or 64 hexadecimal characters. Nothing else on the master signs: the gateway seals unsigned wire-v2 bundles, so there is no gateway key and no per-challenge leaf key. Bearers are nonempty opaque values. Files are regular,
not hardlinks or symlinks, and mode 0400 or 0600. The Proof VM token and CA are
needed only when `PROOF_VM_ORCHESTRATOR_URL` is set.

The config bind defaults to `config/` and holds the unsigned `challenges.toml`
and `measurements.toml`. The master reads them and the gateway serves them as the
challenge profile at `GET /v1/trust`; validators build their view from that route.

```bash
docker compose --project-directory . --env-file deploy/env/master.env \
  -f deploy/compose/role-master.yml --profile master up -d
```

The state volume contains gateway seals, Proof jobs/topics and the epoch
journal. Each challenge keeps its own state in the `cortex-challenge-<id>-data`
volume. Back it up as SQLite state, including WAL, through a quiesced copy
or SQLite's backup API. A filesystem copy of only the main database while the
service writes is not a valid backup.

## Challenge containers

The master role runs a second service, `challenge-supervisor`, which is the only
Cortex process holding the Docker socket. It reads
`deploy/challenges/registry.toml`, and for each entry it:

1. pulls the channel or pin;
2. checks the slug, contract and source labels and the GitHub build provenance;
3. runs a secretless canary;
4. replaces the container `cortex-challenge-<id>` on the private
   `cortex-challenges` network;
5. rolls back when the new digest does not answer `/version`.

The gateway joins that network as `cortex-master`, polls `get_weights` once per
completed epoch and proxies public routes under `/challenge/<id>/`. The full
contract is in [docs/CHALLENGES.md](../docs/CHALLENGES.md).

Per challenge, on the master host:

```bash
dir="$BASE_CHALLENGE_SECRETS_HOST_DIR/bounty"
install -d -m 0700 -o 65532 -g 65532 "$dir"
(umask 077; openssl rand -hex 32 >"$dir/internal.token"; openssl rand -hex 32 >"$dir/admin.token")
chown 65532:65532 "$dir"/*.token
```

Put any challenge settings in the registry `env` table,
for example `BOUNTY_BACKEND_PUBLIC_URL`. Challenge-specific secrets, such as
Bounty's `session.key`, go next to the tokens; see each challenge's operator
guide ([Bounty](https://github.com/CortexLM/bounty/blob/main/docs/operator.md),
[OpenType](https://github.com/OpentypeAI/challenge/blob/main/docs/operator.md)).

A registered id that the challenge profile does not list runs without emission. Use
this burn-in to check `GET /challenge/<id>/version` and the logs. Then activate it
with an algorithm 3 profile (`config/challenges-v3.example.toml`), see
[Changing emission shares](#changing-emission-shares).
A trusted id that is missing, unhealthy or invalid burns its share.

Operator switches:

- `channel = "edge"` follows `main`;
- `pin = "sha256:..."` freezes a digest;
- `attestation = false` is for local images only;
- deleting a registry row stops that container and keeps its volume.

The master re-reads the registry when the file changes, and the supervisor
re-reads it every 15 seconds.

## Validator

Create `deploy/env/python-validator.env` and set the exact master HTTPS origin,
netuid, chain network, live subnet `version_key`, VPC peer bind address and private
host paths. `role-validator.yml` is verify-only and needs no wallet. Weight setting
is a separate role, `role-weight-setter.yml`, the only one that also takes the
validator wallet name and hotkey. The HTTPS gateway is the only anchor for the challenge profile and the bundle; nothing it serves is signed. Optional fallback RPCs are a JSON list of at most eight
credential-free `wss://` URLs.

The weight-setter mounts the wallet directory read-only at `/run/wallets`; the
verify-only role mounts none. The validator identity directory contains:

```text
consensus.key
tls.crt
tls.key
```

The config directory additionally contains `peers.json`, a JSON object mapping
each validator public-key hex string to one SAN-valid HTTPS origin. Do not run
two validators with the same wallet: their commit/reveal or rate-limit state can
conflict.

```bash
# verify-only: no hotkey, no consensus seed
docker compose --project-directory . --env-file deploy/env/python-validator.env \
  -f deploy/compose/role-validator.yml up -d
# weight setting: the only role with the validator hotkey and consensus seed
docker compose --project-directory . --env-file deploy/env/python-validator.env \
  -f deploy/compose/role-weight-setter.yml up -d
```

Enable chain submission only after the master returns a current `sealed: true`
bundle and the [validator `--verify-only --once` preflight](../docs/external-miner/validators.md#run-the-validator)
reports `validator outcome=verified` for the same root/vector from historical
chain state. The unsealed UID0 fallback is a readiness failure, not a weight to
submit.

`outcome=verified` from the hotkey-less role says nothing about the hotkey: it
logs `preflight=skipped` and does not check hotkey registration, validator permit
or the weight rate limit. There is no submission-off switch on the weight-setter,
so check the hotkey before starting it, on the weight-setter host:

1. Run `WALLET_NAME=... WALLET_HOTKEY=... scripts/run-validator.sh --verify-only`
   (or `cortex validator --verify-only --once` with `--wallet-name`,
   `--wallet-hotkey` and `--wallet-path`). With a hotkey, verify-only runs the same
   chain preflight as a submission and never submits: the hotkey must be
   registered on the netuid, hold a validator permit, have its weight rate limit
   elapsed (`blocks_since_last_update > weights_rate_limit`) and the chain limits
   must accept the sealed vector. A refusal prints `chain_preflight_failed`.
2. Only then start `role-weight-setter.yml`.

### Upgrading from the single validator role

Before this release `role-validator.yml` held the wallet and submitted. It is now
verify-only: after the upgrade it keeps running, stays healthy and sets no weights.
It ignores `BASE_WALLET_NAME`, `BASE_WALLET_HOTKEY`, `BASE_VALIDATOR_WALLETS_DIR`
and the `consensus.key` in its identity directory (`BASE_GATEWAY_HOTKEY` is removed).
To keep setting weights, move to `role-weight-setter.yml`, whose state volume
`base-weight-setter-state` is new. Started empty, the weight-setter has no
submission journal and no consensus watermarks: rollback protection restarts
from the first profile `/v1/trust` serves it, and it does not know the epochs the
old role already submitted (the chain rate-limit preflight still refuses a second
set inside the limit). Reconcile any `dispatch_pending` attempt of the old role
(`cortex validator-reconcile`) before switching, or keep its state by copying the
volume while both roles are stopped:

```bash
docker compose --project-directory . --env-file deploy/env/python-validator.env \
  -f deploy/compose/role-validator.yml down
docker run --rm --entrypoint cp \
  -v cortex-validator_base-validator-state:/from:ro \
  -v cortex-weight-setter_base-weight-setter-state:/var/lib/base \
  "$CORTEX_IMAGE" -a /from/. /var/lib/base/
docker compose --project-directory . --env-file deploy/env/python-validator.env \
  -f deploy/compose/role-weight-setter.yml up -d
```

The volume names are Compose's `<project>_<volume>` defaults for the `name:` in
each role file; check yours with `docker volume ls` first. Docker creates the
target volume on that `docker run` and Compose then warns it was not created by
Compose; that warning is expected. Copy only an old volume that you no longer
start: two processes must not share one journal.

To run a validator without Compose, use `scripts/run-validator.sh` from a
repository clone. It needs `WALLET_NAME` and `WALLET_HOTKEY` only to submit weights
(`--verify-only` needs neither), reads the
challenge profile from the gateway, and refuses to run master-only commands; see the
[quick start](../docs/external-miner/validators.md#quick-start-one-script).

## Changing emission shares

The challenge profile is the unsigned `config/challenges.toml` (plus
`config/measurements.toml`). The master reads it; the gateway is the single source
of truth and serves it at `GET /v1/trust`.

1. Edit the `[[challenges]]` shares (basis points, sum 10000) in `config/challenges.toml`.
2. Bump `version`. Adding a third challenge or arbitrary shares needs
   `challenges_version >= 3` (1..64 unique ids summing to 10000); versions 1-2 only
   accept the 2000/8000 and 3000/7000 splits. Do NOT set `introduced_epoch` in the
   future: the profile is global, so the master then cannot seal epochs before it
   and the gateway cannot decode stored seals older than it (`/v1/weights/latest`
   answers "unsealed" and the emission burns). Keep the existing value, or the epoch
   of the first bundle the new profile must govern, never above the current epoch.
   The gateway store refuses a version rollback.
3. Run `python scripts/check_repo.py`, then restart the master between two epochs
   (just after a seal), because the new shares apply to the next bundle it builds.
4. Validators pick up the new profile from `/v1/trust` at the next epoch; no
   validator restart or file copy is needed. Check `GET /v1/trust` shows the new
   `challenges_version`.

A registered container id still needs a row in `deploy/challenges/registry.toml` to run.

## Proof VM host

The host requires Linux, `/dev/kvm`, Firecracker, jailer, nftables/network
namespace support, a reviewed kernel and one or more measured ext4 images. Install
the locked Cortex wheel in `/opt/base/venv`, copy
`deploy/env/proof-vm-host.toml.example` to `/etc/proof-vm/host.toml`, and fill
every blank pin from exact local bytes or a signed offer.

Private `/etc/proof-vm` files include the rotating master bearer, TLS private key,
OpenRouter key, signed inference offer and optional knowledge-approval public
key. The TLS certificate must cover every hostname or IP used in the master's
URL. The host binds one explicit address; the example loopback value is not a
production wire.

The `[images]` table maps each raw 64-hex rootfs digest to its absolute installed
path. `[host].kernel_digest` binds the kernel. `custom_ids` is empty by default;
only listed IDs can open custom topics. `[egress]` permits exact topic-setup
IPv4/port pairs. Experiment VMs receive no network regardless of that list.

### Small-host sizing

For the local KVM smoke, plan for a dedicated host with at least 4 vCPU, 8 GiB RAM
and 80 GiB storage, with working KVM support. This is a sizing starting point,
not evidence that a particular host can boot the guest or run a research topic.
In the local copy of `host.toml`, set:

```toml
[host]
# Keep the other required host fields from the example.
max_topics = 1
max_experiments = 1

[caps]
vcpus = 1
mem_mib = 1024
disk_mib = 16384
```

Both master environment examples request this same per-VM shape. The resource
settings apply to the persistent topic VM and each fresh experiment VM, so leave
capacity for both at once. Reserve additional space for the OS, installed images
and retained failed experiments. Larger topics need explicit capacity planning;
the general host template retains its larger ceilings and topic count.

Requests above host ceilings fail rather than being clamped. A restarted master
cannot attach to an existing topic VM with a different image or resource shape;
changing environment values does not resize it. Resource ranges and omitted-value
defaults are in the [configuration reference](../docs/reference/configuration.md#master).

### Local checks and KVM smoke

Run the local preflight before enabling the host service:

```bash
sudo /opt/base/venv/bin/cortex vm-host --config /etc/proof-vm/host.toml --check
```

The JSON report exits successfully only when all local prerequisite checks pass.
It requires effective UID 0, matching the service and jailer, and checks that
`cp`, `chown` and `mkfs.ext4` resolve to safe executable files. Only when topic
egress is configured does it also check `ip`, `nft` and `sysctl`, and verify that
`/dev/net/tun` opens read/write as a character device. It creates no TAP interface.
It does not create state, boot VMs, execute binaries or contact inference/provider
services. A passing preflight is not live execution evidence.

The separate smoke explicitly starts real Firecracker guests on that host:

```bash
sudo /opt/base/venv/bin/python -m cortex.vm.smoke --run-live \
  --config /etc/proof-vm/host.toml \
  --state-dir /var/lib/proof/smoke-first \
  --vcpus 1 --mem-mib 1024 --disk-mib 16384
```

Choose a new private state directory for each run; its parent must already exist.
Keep its absolute path short: generated UNIX socket paths must fit Linux's
107-byte limit. An excessive path length is rejected before any VM is created.
If several images are configured, select the installed rootfs with
`--image-digest sha256:<actual-digest>`. From a source checkout, the equivalent
entry point is `uv run --no-sync python scripts/vm_smoke.py` with the same flags.
The smoke reads local artifact and hypervisor settings without reading TLS or
inference credentials. It leaves the running orchestrator configuration untouched.

The smoke checks two separate guest identities and boot IDs, a read-only root,
a separate writable workspace, loopback-only network interfaces and confirmed
teardown. It records private evidence in `smoke.json`. It does not run inference,
reproduce a scientific result, score a submission or submit chain weights. CI
tests this command with an injected hypervisor; only a successful real host run
provides KVM lifecycle evidence.

After preflight and the separate smoke, install and start the service:

```bash
sudo install -m 0644 deploy/systemd/proof-vm-orchestrator.service \
  /etc/systemd/system/proof-vm-orchestrator.service
sudo systemctl daemon-reload
sudo systemctl enable --now proof-vm-orchestrator.service
```

Before opening a topic, additionally test authenticated orchestrator health and
the topic/experiment API lifecycle, including retention after failure and
destruction after success. The isolated smoke does not exercise those HTTPS
routes or prove a challenge reward path.

## Promotion and rollback

CI builds distributions and tests both supported Python versions. Image
publication is automatic for `main`, but it does not edit deployment pins or
touch a host. Run `.github/workflows/approve-image-update.yml` from `main` with
the candidate, source commit, expected digest and the exact approval phrase.
The protected `production` environment verifies provenance and emits
`update.json`; it still changes no host. Promotion consists of reviewing that
evidence, changing `CORTEX_IMAGE` to its immutable digest, rendering Compose,
backing up state, pulling that digest and recreating one role.

The Cortex image itself has deliberately no `git pull`, Docker-socket watcher,
CI SSH deployment or unattended host updater; only challenge containers
auto-update, through the supervisor gates above, and they never hold a signing
key. Safe automation of the Cortex image must preserve the same operator gate,
take a consistent SQLite backup, verify role health plus a fresh sealed bundle,
and restore both the prior digest and schema-compatible state on failure.

Rollback uses the prior reviewed digest and a schema-compatible state backup.
Never roll a database backward by deleting rows or replacing the latest seal.
Challenge profile changes are separate edits of `config/challenges.toml` and do not
happen as a side effect of an image rollback.
