# Deploy Cortex

Cortex uses three independently operated roles:

| Role | Runtime | Responsibility |
| --- | --- | --- |
| master | Docker Compose | public gateway, Bounty, Proof and durable epoch emission |
| validator | Docker Compose | independent bundle verification, peer roots and Bittensor submission |
| Proof VM host | systemd on a KVM machine | Firecracker topic and experiment VMs |

The gateway exists only on the master. A validator never receives challenge
signing keys and never executes miner code. Production runs the VM host on a
dedicated machine reachable from the master over a private network and HTTPS.

- [Validate the deployment source](#validate-the-deployment-source)
- [Build and publish the Python image](#build-and-publish-the-python-image)
- [Master](#master)
- [Validator](#validator)
- [Proof VM host](#proof-vm-host)
- [Promotion and rollback](#promotion-and-rollback)

## Validate the deployment source

```bash
uv run python scripts/check_deploy.py --check-examples
docker compose --env-file deploy/env/master.env.example \
  -f deploy/compose/role-master.yml --profile master config >/dev/null
docker compose --env-file deploy/env/python-validator.env.example \
  -f deploy/compose/role-validator.yml config >/dev/null
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

The base image is an exact Python 3.12 Bookworm digest. The manual
`.github/workflows/images.yml` workflow builds both targets and checks installed
commands, imports and native dependencies without network access during the
smoke checks. The guest check overrides the entry point; it does not boot init
or KVM. Optional publication pushes only the tested runtime and records its
registry digest. Do not deploy a local tag or copy a digest from another build.

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

Create `deploy/env/master.env` from the example and set:

- `CORTEX_IMAGE` to the published `repository@sha256:<64 hex>`;
- the master VPC bind address and netuid;
- the external Bounty feed when Bounty should accept reports;
- the Proof VM URL, CA, exact rootfs digest, signed inference-offer commitment
  and registered custom IDs when Proof custom topics should open;
- explicit `PROOF_RLM_VM_VCPUS`, `PROOF_RLM_VM_MEM_MIB` and
  `PROOF_RLM_VM_DISK_MIB` values that fit the VM host ceilings and capacity.

Prepare a private directory, owned so container UID 65532 can read it:

```text
deploy/secrets/master/
  gateway.key
  bounty.key
  proof.key
  bounty-session.key
  operator.token
  proof-vm.token
  proof-vm-ca.pem
```

Seeds are raw 32 bytes or 64 hexadecimal characters. `bounty-session.key` has at
least 32 random bytes. Bearers are nonempty opaque values. Files are regular,
not hardlinks or symlinks, and mode 0400 or 0600. The Proof VM token and CA are
needed only when `PROOF_VM_ORCHESTRATOR_URL` is set.

The trust bind defaults to `config/` and contains `owner.pubkey`, both signed
TOML documents and their adjacent `.sig` files. Verify them before startup with
the [offline ceremony](../docs/how-to/trust-root.md).

```bash
docker compose --env-file deploy/env/master.env \
  -f deploy/compose/role-master.yml --profile master up -d
```

The state volume contains gateway seals, Bounty state, Proof jobs/topics and the
epoch journal. Back it up as SQLite state, including WAL, through a quiesced copy
or SQLite's backup API. A filesystem copy of only the main database while the
service writes is not a valid backup.

## Validator

Create `deploy/env/python-validator.env` and set the exact master HTTPS origin,
netuid, independently pinned gateway public key, chain network, wallet name and
hotkey, VPC peer bind address and private host paths.

The wallet directory is mounted read-only at `/run/wallets`. The validator
identity directory contains:

```text
consensus.key
tls.crt
tls.key
```

The trust directory additionally contains `peers.json`, a JSON object mapping
each validator public-key hex string to one SAN-valid HTTPS origin. Do not run
two validators with the same wallet: their commit/reveal or rate-limit state can
conflict.

```bash
docker compose --env-file deploy/env/python-validator.env \
  -f deploy/compose/role-validator.yml up -d
```

Enable chain submission only after the master returns a current `sealed: true`
bundle and a validator dry run recomputes the same root/vector from historical
chain state. The unsealed UID0 fallback is a readiness failure, not a weight to
submit.

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
publication is an explicit workflow and does not edit deployment pins or touch a
host. Promotion consists of changing `CORTEX_IMAGE` to a reviewed digest,
rendering Compose, backing up state, pulling that digest and recreating one role.

Rollback uses the prior reviewed digest and a schema-compatible state backup.
Never roll a database backward by deleting rows or replacing the latest seal.
Trust-root rotations are separate signed ceremonies and do not happen as a side
effect of an image rollback.
