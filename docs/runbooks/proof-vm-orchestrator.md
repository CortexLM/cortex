# Runbook — Proof topic-VM orchestrator (Firecracker + jailer agent on a KVM host)

Operator procedure for the `proof-vm-orchestrator` agent that boots one
Firecracker RLM microVM per Proof topic and a **sister** Firecracker guest
for every miner run. Product spec: [`../PROOF.md`](../PROOF.md) § Isolation
boundary. Code: `crates/proof-vm-proto` (wire), `crates/proof-vm-fc`
(control-plane client), `crates/proof-vm-agent` (agent API),
`crates/proof-fc-host` (Firecracker backend), `bins/proof-vm-orchestrator`.
Staging wire + probes: § DigitalOcean staging and
[`deploy/scripts/proof-vm-wire-check.sh`](../../deploy/scripts/proof-vm-wire-check.sh).

## What runs where

```text
control plane (proof-challenge, master)      KVM host = wherever /dev/kvm works (see below)
┌─────────────────────────────────┐          ┌────────────────────────────────────────────┐
│ proof-challenge                 │  HTTPS   │ proof-vm-orchestrator (systemd, :8200)     │
│  RlmScorer → RunnerRegistry     │ bearer   │  bearer file · one running VM per topic_id  │
│   [custom_id] → VmBackedRunner  │ ───────▶ │  jailer ─ firecracker  RLM VM (4 vCPU/8 GiB)│
│  FirecrackerOrchestrator        │          │     ▲ vsock: jobs, owner-key staging        │
│   PROOF_VM_ORCHESTRATOR_URL     │          │     │ sister request (paid jobs only)        │
│   PROOF_VM_ORCHESTRATOR_TOKEN_  │          │  jailer ─ firecracker  sister guest          │
│     FILE (never logged)         │          │     no NIC · artefact over vsock · destroyed │
│   PROOF_RLM_VM_IMAGE_DIGEST     │          │  host stamps sandboxed + flops_used          │
│  VmJob = public data only       │          │  nftables per-VM egress allowlist            │
└─────────────────────────────────┘          └────────────────────────────────────────────┘
```

**Where the agent runs.** Only where `/dev/kvm` works and the agent's
`ready()` is green — the isolation boundary is the RLM microVM plus the
sister guest, not the machine they sit on:

- **Production: dedicated DigitalOcean metal preferred — never colocated on
  the CP.** A bare-metal / dedicated KVM host with its own `/dev/kvm`,
  reachable from the master over the VPC or a private network.
- **Staging: colocating the agent on the CP droplet with nested `/dev/kvm`
  is an allowed exception, proven.** On `cortex-staging` nested DO
  virtualisation booted Firecracker and the § 4 fail-closed matrix came back
  green. Nested virtualisation stays **fragile** (it depends on what the
  hypervisor underneath exposes and can change with a resize or a
  migration): if the boot fails or `/dev/kvm` disappears, do not patch
  around it — provision dedicated metal and point the CP at it.
- Never a Lium pod, never a software emulator, never anything without
  `/dev/kvm` (the unit's `ConditionPathExists` refuses).

Locked by design (do not move any of it):

| Rule | Where it is enforced |
|------|----------------------|
| Firecracker microVMs, **sisters** (RLM VM + miner guest) run only where `/dev/kvm` works — production on dedicated DO metal (never colocated on the CP); staging colocated on the CP droplet with nested `/dev/kvm` as the allowed, proven exception; never Lium, never emulated | agent runs only where `/dev/kvm` exists (`ConditionPathExists`); nothing in `proof-challenge` can exec |
| The RLM never sees the host filesystem or secrets — only `VmJob` payloads | `proof-vm-proto` types; jobs are the signed topic, digests, rule versions; tests assert no path / key / origin in any body |
| RLM image pin `PROOF_RLM_VM_IMAGE_DIGEST=sha256:…`, empty = fail-closed | `FirecrackerOrchestrator::ready()` → `NotWired` naming the var; agent re-hashes `images/sha256-<hex>.ext4` before boot |
| Auth: `PROOF_VM_ORCHESTRATOR_URL` + `PROOF_VM_ORCHESTRATOR_TOKEN_FILE`, bearer file first, never logged | client reads the file per request; agent compares SHA-256 digests in constant time, re-reads its file per request |
| RLM VM 4 vCPU / 8192 MiB | `proof_vm_fc::DEFAULT_RLM_VCPUS` / `DEFAULT_RLM_MEM_MIB` |
| Sister sized by the host / topic deadline only, never by the RLM | `PROOF_VM_AGENT_SISTER_VCPUS` / `_MEM_MIB`; the RLM's request carries no size |
| `retain` default **destroy** on topic close | `TopicVmSpec::for_topic` → `RetainPolicy::Destroy` |
| Hard `topic_id ↔ VM` bind | agent: request topic **and** job topic must equal the VM's (409 `topic_mismatch`); client refuses a job for another topic before any request and checks every echo |
| Zero live Firecracker in CI | every test uses `FakeHypervisor` / `RecordingShell`; `FirecrackerHypervisor::ready()` refuses without `firecracker`, `jailer`, `/dev/kvm` and the test asserts nothing was spawned |

## Host prerequisites

- Bare-metal (or nested-virt-capable) Linux with `/dev/kvm`, `CONFIG_VHOST_VSOCK`, nftables, `iproute2`, `e2fsprogs` (`mkfs.ext4`), `coreutils` (`cp --reflink`, `truncate`).
- `firecracker` and `jailer` of the **same** release, statically linked (musl), at `/usr/local/bin/`. Record the release you installed in your change log.
- A filesystem that supports reflinks under `/srv/jailer` and `/var/lib/proof-vm` (XFS with `reflink=1` or btrfs) so per-VM rootfs copies are instant. ext4 works but copies whole images per boot.
- Layout:

```text
/etc/proof-vm/orchestrator.env      # from deploy/env/proof-vm-orchestrator.env.example (0600 root)
/etc/proof-vm/token                 # bearer, 0400 root; same bytes as the CP's PROOF_VM_ORCHESTRATOR_TOKEN_FILE
/etc/proof-vm/tls.crt + tls.key     # agent certificate; CP pins the CA via PROOF_VM_ORCHESTRATOR_CA_FILE if private
/etc/proof-vm/owner-keys/           # optional; files staged into the RLM VM over vsock (0400 root)
/var/lib/proof-vm/vmlinux           # guest kernel; pin = sha256sum → PROOF_VM_AGENT_KERNEL_DIGEST
/var/lib/proof-vm/images/sha256-<hex>.ext4   # RLM rootfs (CP pin) and sister rootfs (agent pin)
/var/lib/proof-vm/retained/         # `retain` teardowns land here
/srv/jailer/firecracker/<vm_id>/    # live jails (root/, console.log, net.nft)
```

Digests: `sha256sum vmlinux`, `sha256sum <rootfs>.ext4`, then name the rootfs
file `sha256-<hex>.ext4`. **Never write a digest you did not compute from the
file you staged.** An unpinned or mis-pinned image never boots; that is the
intended failure.

Guest images are **not** built by this repository. The RLM image must ship a
guest agent listening on vsock port `5000` and speaking `HostToRlm` /
`RlmToHost`; the sister image must ship one listening on port `5002` speaking
`HostToMiner` / `MinerToHost` (both in `crates/proof-vm-proto/src/guest.rs`;
frames are 4-byte big-endian length + JSON, `api_version: 1`). The RLM guest
asks for a sister by connecting to host port `5001` with a `SisterRequest`
carrying the artefact tarball it already fetched and inspected.

## Install

```bash
install -m 0755 target/release/proof-vm-orchestrator /usr/local/bin/proof-vm-orchestrator
install -m 0644 deploy/systemd/proof-vm-orchestrator.service /etc/systemd/system/
install -d -m 0750 /etc/proof-vm /var/lib/proof-vm/images /var/lib/proof-vm/retained /srv/jailer
install -m 0600 deploy/env/proof-vm-orchestrator.env.example /etc/proof-vm/orchestrator.env
# edit: PROOF_VM_AGENT_KERNEL_DIGEST, PROOF_VM_AGENT_SISTER_IMAGE_DIGEST,
#       PROOF_VM_AGENT_EGRESS_ALLOW, PROOF_VM_AGENT_UPLINK, TLS paths
head -c 32 /dev/urandom | base64 -w0 > /etc/proof-vm/token && chmod 0400 /etc/proof-vm/token
systemctl daemon-reload && systemctl enable --now proof-vm-orchestrator
journalctl -u proof-vm-orchestrator -n 50
```

Boot log must show `firecracker + jailer + /dev/kvm present; agent ready` and
`bearer token file present (contents not logged)`. A malformed pin exits 1; a
non-loopback bind without TLS exits 1.

Verify from the host (the bearer is required even for health):

```bash
curl -fsS --cacert /etc/proof-vm/tls.crt -H "Authorization: Bearer $(cat /etc/proof-vm/token)" \
  https://127.0.0.1:8200/v1/health
# {"api_version":1,"ready":true,"reason":"","hypervisor":"firecracker","vms":0}
```

## Wire the control plane (master droplet)

In `deploy/env/proof-challenge.env` (age-materialized, never git):

```bash
PROOF_VM_ORCHESTRATOR_URL=https://<kvm-host>:8200
PROOF_VM_ORCHESTRATOR_TOKEN_FILE=/run/base/proof/vm_orchestrator_token   # same bytes as /etc/proof-vm/token
PROOF_VM_ORCHESTRATOR_CA_FILE=/run/base/proof/vm_orchestrator_ca.pem     # only for a private CA
PROOF_RLM_VM_IMAGE_DIGEST=sha256:<hex of the RLM rootfs staged on the KVM host>
PROOF_VM_RUNNER_CUSTOM_IDS=<custom ids from the signed topics this host serves>
```

Put the token under `deploy/secrets/proof/` (mounted at `/run/base/proof`,
mode 0400, uid 65532). Restart `proof-challenge`; its boot log must show
`firecracker topic-vm orchestrator wired` and one `vm-backed runner
registered` line per id. `GET /v1/status` → `custom_family_wired: true`,
`registered_custom` lists the ids, `custom_ready` lists the ones whose runner
can run now; an open custom topic with a ready id appears in
`scorable_topics`.

The Lium harvest is **not** a prerequisite. With these four variables set
and no `LIUM_API_KEY` / `LIUM_SSH_PUBLIC_KEY_FILE`, the boot log shows
`live harvest not wired: custom-family topics route to the rlm scorer over
the topic-vm orchestrator`, custom topics open and score, and every `nll` /
`throughput` topic stays out of `scorable_topics` (submit **503**, no row).
`GET /v1/status` then reads `live_harvest_wired: false` (that flag is Lium
only — expected here, not a fault), `custom_family_wired: true`,
`registered_custom` = your ids, and `custom_ready` = the ids whose runner can
run now; an id in `registered_custom` but not in `custom_ready` means the
bearer file or image pin is missing on this host. Do not stage a placeholder
Lium key to open custom topics. If the log shows `live harvest not wired;
every submission will 503` instead, the orchestrator URL is unset or refused
(plain `http://` off loopback) or no id registered — fix that, not Lium.

The RLM VM shape is 4 vCPU / 8192 MiB. `PROOF_RLM_VM_VCPUS` /
`PROOF_RLM_VM_MEM_MIB` exist for a deliberate change only.

**Probe the wire from inside the CP** (operator bearer, read-only, no VM,
no spend): `GET /v1/admin/proof/vm-orchestrator` runs the client's own
`ready()` (bearer file + pin, re-read now) and one agent health call through
the very client the runner uses — same bearer file, same CA, same rustls —
and reports the host gates next to it. A broken wire is data, not an error:

```bash
TOKEN=$(head -n1 deploy/secrets/proof/admin_tokens)
curl -sS -H "Authorization: Bearer $TOKEN" http://127.0.0.1:8080/challenge/proof/v1/admin/proof/vm-orchestrator
# {"orchestrator":"firecracker","ready":true,"reason":"","image_digest":"sha256:…","vcpus":4,"mem_mib":8192,
#  "agent":{"api_version":1,"ready":true,"reason":"","hypervisor":"firecracker","vms":0},"agent_error":null,
#  "live_harvest_wired":false,"custom_family_wired":true,"registered_custom":["<custom-id>"]}
unset TOKEN
```

| Field | Root cause it names |
|-------|---------------------|
| `orchestrator: "unwired"` + `reason` | `PROOF_VM_ORCHESTRATOR_URL` unset, or set but refused (plain http off loopback, no `PROOF_VM_ORCHESTRATOR_TOKEN_FILE`) — restart after fixing |
| `ready: false` + `reason` | bearer file missing / empty, or `PROOF_RLM_VM_IMAGE_DIGEST` unpinned — fix the file / env; the token needs no restart |
| `agent: null` + `agent_error` | `orchestrator unreachable` (agent down, firewall, VPC route), `orchestrator refused the bearer` (bytes differ from `/etc/proof-vm/token`), TLS refused (CA / SAN) |
| `agent.ready: false` + `agent.reason` | the KVM host: `firecracker` / `jailer` / `/dev/kvm` / image missing |
| `custom_family_wired: false` (ids set) | the custom family is not routed: orchestrator env unset / refused, or no id registered — `registered_custom` says which |
| `live_harvest_wired` | **Lium only** (`nll` / `throughput`); informational for the custom family — `false` on a custom-only host is expected, not a fault |

Run it over SSH + loopback (staging's public API is cleartext; never send
the operator bearer over it). The reason strings name env vars and
container paths — operator data — never the bearer.
[`deploy/scripts/proof-vm-wire-check.sh cp`](../../deploy/scripts/proof-vm-wire-check.sh)
wraps this route with the status / leak checks (§ DigitalOcean staging).

## DigitalOcean staging (wire + e2e probes)

Flip staging from `UnwiredVmOrchestrator` to `FirecrackerOrchestrator` and
prove both the fail-closed matrix and one happy path, with evidence. The
operator harness is
[`deploy/scripts/proof-vm-wire-check.sh`](../../deploy/scripts/proof-vm-wire-check.sh)
(bash + curl + python3; runs on the droplet, no cargo; never prints the
bearer; refuses production hosts). Env overlays with placeholders only:
[`deploy/env/proof-challenge.staging-vm.example`](../../deploy/env/proof-challenge.staging-vm.example)
(CP side) and
[`deploy/env/proof-vm-orchestrator.staging.example`](../../deploy/env/proof-vm-orchestrator.staging.example)
(KVM host side).

### Where things run

| Piece | Host | Notes |
|-------|------|-------|
| `proof-challenge` (client, `FirecrackerOrchestrator`) | the **existing staging master droplet** (`cortex-staging`; topology in [`staging-testnet-e2e.md`](staging-testnet-e2e.md)), compose `role-master` + `env-staging` | holds the bearer file, the CA PEM, the RLM image pin, and the custom ids; it is only the HTTPS client — nothing in the container can exec Firecracker |
| `proof-vm-orchestrator` (agent, Firecracker + jailer) | **colocated on that same droplet** — the allowed, proven staging exception — or dedicated DO metal when nested KVM does not boot | `systemd/proof-vm-orchestrator.service` on the host, `ConditionPathExists=/dev/kvm`; bind on the droplet's private address, HTTPS + bearer file |

**Colocated staging (allowed exception, proven).** `cortex-staging` exposes
a working `/dev/kvm` through nested DigitalOcean virtualisation; the agent
booted Firecracker there and § 4 came back green. This is the staging
default — and staging only: production never colocates the agent on the CP.
One droplet runs the compose stack **and** the agent as a host systemd unit.
Two things follow from the CP living in a container on the same machine:

- The CP must reach the agent on the **droplet's private (VPC) address**,
  never on loopback: `127.0.0.1` inside the `proof-challenge` container is
  the container, and `FcConfig` only accepts plain `http://` on loopback
  anyway. Bind the agent on the VPC IP with TLS (`PROOF_VM_AGENT_BIND=<vpc-ip>:8200`,
  a certificate with that IP or name as SAN) and set
  `PROOF_VM_ORCHESTRATOR_URL=https://<vpc-ip>:8200` on the CP. Open `:8200`
  on the host firewall to the compose network only (`docker network ls` →
  the stack's `base` network → `docker network inspect <name>` for its
  subnet); the wire check runs on the host itself and needs nothing more.
- The RLM VM (4 vCPU / 8 GiB) and each sister (2 vCPU / 4 GiB, no NIC)
  share CPU, RAM, and disk with gateway, Postgres, and both challenges. Size
  the droplet for it and keep `/srv/jailer` + `/var/lib/proof-vm` on a disk
  with room for the images plus ~10 GiB scratch per open topic.

**Nested virtualisation is fragile.** What the droplet's hypervisor exposes
is not under our control and can change with a resize or a live migration.
Treat any of these as "provision metal", not as something to patch around
(no nested-FC redesign, no `--no-kvm` anything — a software emulator is not
the isolation boundary):

- `/dev/kvm` missing or `kvm-ok` reporting KVM cannot be used → the unit
  does not start (`ConditionPathExists`), agent health `ready: false`.
- `topic vm boot failed; releasing its jail` with a KVM ioctl error from
  Firecracker in the agent journal / the jail's `console.log`, or the guest
  never saying hello inside `PROOF_VM_AGENT_BOOT_TIMEOUT_SECS`.
- Sisters cut at the deadline on a run that fits comfortably on metal.

**Dedicated DO metal (production — preferred, never colocated on the CP;
staging fallback when nested does not boot).** A DigitalOcean bare-metal /
dedicated-hardware host, or any bare-metal KVM host attached to the VPC's
private network (WireGuard from the master, or VPC peering when both sides
are DO). Same unit, same env file; the agent listens on the private address
only and TLS + bearer stay mandatory (the bearer never crosses a network in
clear).

Check whichever host you pick before installing anything:

```bash
ls -l /dev/kvm                                  # must exist (crw-rw---- root kvm)
grep -cE '(vmx|svm)' /proc/cpuinfo              # > 0
kvm-ok                                          # "KVM acceleration can be used" (apt install cpu-checker)
modprobe vhost_vsock && ls -l /dev/vhost-vsock  # vsock for the guest channels
nft list ruleset >/dev/null && ip -br link      # nftables + the uplink you will name
df -T /srv /var/lib | grep -E 'xfs|btrfs'       # reflink FS preferred (see Host prerequisites)
```

Budget per open topic: RLM VM 4 vCPU / 8192 MiB + sister 2 vCPU / 4096 MiB
+ ~10 GiB scratch. Size the host for the number of custom topics staging
will keep open at once, plus one probe VM — on the colocated droplet, on
top of the compose stack.

### 0. Preconditions on the CP

The custom family scores only when the rest of the live stack is up; check
`GET /v1/status` on the master **before** touching the wire:

| Gate | Where | Must read |
|------|-------|-----------|
| eval backend | `/v1/status` `eval_backend` | `lium` (`PROOF_FORCE_SIM` off — sim never hosts staging scoring) |
| custom family | `/v1/status` `custom_family_wired`, `registered_custom`, `custom_ready` | `true`, your ids, your ids — wired from the topic-VM env alone (§ Wire the control plane); an id registered but not ready = bearer file / image pin on this host |
| harvest (Lium, informational here) | `/v1/status` `live_harvest_wired` | Lium only (`nll` / `throughput`): `true` with `LIUM_API_KEY` + `LIUM_SSH_PUBLIC_KEY_FILE`, `false` on a custom-only host — expected, not a fault; never stage a placeholder Lium key to open custom topics |
| judge | `/v1/status` `inference_offer.status` | `open`, plus `PROOF_INFERENCE_API_KEY_FILE` present |
| executor | `GET /v1/proof/executor` | `ready: true` (open `1x` offer) |
| topic | a **signed custom topic** (`metric.family: custom`, `metric.custom_id: <id>`) with a **sealed baseline**; the RLM of that topic sets it up per [`../PROOF.md`](../PROOF.md) § Dynamic agentic engine | its `custom_id` is what goes into `PROOF_VM_RUNNER_CUSTOM_IDS`; the topic can only **open** once that id is registered |

Nothing here is challenge content in git: the topic, its rules, and its
images are operator-published documents and staged files.

### 1. KVM host (the staging droplet itself, or the dedicated host)

Follow § Host prerequisites and § Install with
`deploy/env/proof-vm-orchestrator.staging.example` as `/etc/proof-vm/orchestrator.env`:

1. `PROOF_VM_AGENT_BIND=<private-ip>:8200` — the droplet's VPC address when
   colocated (the CP container cannot use loopback), the private address of
   the dedicated host otherwise; a certificate for that address (a private CA
   is fine — the CP pins its root) **with a SAN**: the CP's rustls client
   refuses CN-only certificates even when curl accepts them.
2. Bearer: `head -c 32 /dev/urandom | base64 -w0 > /etc/proof-vm/token; chmod 0400 /etc/proof-vm/token`.
3. Stage `vmlinux`, the RLM rootfs, and the sister rootfs; `sha256sum` each;
   name the rootfs files `images/sha256-<hex>.ext4`; put the kernel and
   sister digests in the env. The RLM digest goes to the CP. Never copy a
   digest from a document; only from `sha256sum` of the file you staged.
4. `PROOF_VM_AGENT_EGRESS_ALLOW`: the judge `InferenceOffer` origin, the
   artefact hosts miners use, a resolver — IPv4 CIDRs, nothing else. On a
   host with ufw or Docker also accept the TAPs in their forward chain
   (§ Egress) — the agent warns `host forward chains drop by default` at
   every VM boot until you do, and the RLM VM reaches nothing before that.
5. `systemctl enable --now proof-vm-orchestrator`; the boot log shows
   `firecracker + jailer + /dev/kvm present; agent ready` and
   `bearer token file present (contents not logged)`.
6. Open `:8200` on the host firewall to the CP only: the compose network's
   subnet when colocated, the staging master's private address when the
   host is dedicated.

### 2. Control plane (staging master)

```bash
ssh root@<staging-master> ; cd /opt/base ; umask 077
# The bearer: a second copy of the agent's file — never pasted on a command line.
install -m 0400 -o 65532 -g 65532 /etc/proof-vm/token deploy/secrets/proof/vm_orchestrator_token         # colocated
install -m 0400 -o 65532 -g 65532 /etc/proof-vm/ca.pem deploy/secrets/proof/vm_orchestrator_ca.pem       # private CA only
# Dedicated host instead: scp root@<kvm-host-private-ip>:/etc/proof-vm/{token,ca.pem} over the private
# network to the same paths, then chown 65532:65532 + chmod 0400 both files.
# Append the keys of deploy/env/proof-challenge.staging-vm.example (values filled) to the age source
# of proof-challenge.env (deploy/scripts/age-encrypt-env.sh / age-push-env.sh), then:
./deploy/scripts/materialize-env.sh
docker compose -f docker-compose.yml -f deploy/compose/role-master.yml -f deploy/compose/env-staging.yml --profile master up -d proof-challenge
docker compose logs proof-challenge | grep -E 'topic-vm orchestrator|vm-backed runner'
# firecracker topic-vm orchestrator wired (bearer file present, contents not logged)
# vm-backed runner registered custom_id=<id>
```

### 3. Wire check (on the droplet, no cargo)

```bash
cd /opt/base
./deploy/scripts/proof-vm-wire-check.sh all          # env + agent + cp; exit 0 = every check PASS
./deploy/scripts/proof-vm-wire-check.sh boot-probe   # one RLM VM: create → attach → 409 → 409 topic_mismatch → destroy → 404
```

| Subcommand | Proves |
|------------|--------|
| `env` | `PROOF_VM_ORCHESTRATOR_URL` is `https://`; the bearer file (container path mapped through the compose bind mount, `--path-map`) exists and is non-empty, mode 0400 / uid 65532; `PROOF_RLM_VM_IMAGE_DIGEST` is `sha256:<64 hex>` (empty or a placeholder = FAIL — never invented); the CA file is PEM when set; every custom id is well-formed; the shape is the locked 4 / 8192; `PROOF_FORCE_SIM` is off |
| `agent` | `GET /v1/health` with the bearer → `ready: true`, `hypervisor: firecracker`; no bearer → 401; wrong bearer → 401 |
| `cp` | `/v1/status`: `lium`, `custom_family_wired`, `registered_custom` ⊇ ids, `custom_ready` ⊇ ids (`live_harvest_wired` is logged, Lium-only, never a FAIL), no URL / token / path in the body; `/v1/proof/topics` leaks no holdout; `/v1/proof/executor` readiness; then the admin probe above — `orchestrator: firecracker`, `ready: true`, `agent.ready: true` through the CP's own rustls client |
| `boot-probe` | the agent boots the **pinned** image for a probe topic, one topic ↔ one VM, a teardown naming another topic is refused, destroy is confirmed, nothing is left for the topic. Opt-in: it boots a real 4 vCPU / 8 GiB RLM VM on the KVM host (up to 10 min, the RLM guest must say hello); no job runs, nothing is spent. Nothing outlives it: Ctrl-C, a lost `201` (timeout, dropped connection), or an unconfirmed teardown all end in a by-topic attach + destroy before the script exits, so a retry on the same probe topic is never blocked by a stranded VM |

Every check re-reads the files it names, so a fix to the bearer or the CA
needs no restart; URL / digest / ids are read at boot.

### 4. Fail-closed matrix (every row: 503, no row, no VM, no rent)

`./deploy/scripts/proof-vm-wire-check.sh matrix --topic <custom-topic-id>`
prints these as ready-to-paste steps. Flip one knob, probe, restore, re-run
`cp`. `submit-probe` POSTs a probe submission (64×`a` hotkey, random
artefact digest, `https://example.invalid/…` locator — never fetchable) and
asserts the status **and** the reason text; a 2xx expectation is refused
without `--allow-live-run`.

| Flip | Restart? | `submit-probe --expect 503 --reason …` | Admin probe shows |
|------|----------|----------------------------------------|-------------------|
| comment out `PROOF_VM_ORCHESTRATOR_URL` | yes | `PROOF_VM_ORCHESTRATOR_URL` (from `runner not wired: no orchestrator configured (…)`) | `orchestrator: unwired` |
| empty the CP bearer file (`: > deploy/secrets/proof/vm_orchestrator_token`) | no | `PROOF_VM_ORCHESTRATOR_TOKEN_FILE` … `missing or empty` | `ready: false` |
| write other bytes into the CP bearer file | no | `refused the bearer` (agent 401 → CP 503) | `agent_error: … refused the bearer` |
| `PROOF_RLM_VM_IMAGE_DIGEST=` | yes | `PROOF_RLM_VM_IMAGE_DIGEST` … `image_digest is missing or out of range` | `ready: false`, `image_digest: ""` |
| `systemctl stop proof-vm-orchestrator` on the KVM host | no | `orchestrator unreachable` (route named, agent address never) | `agent_error: … unreachable` |
| unknown / closed topic → `--expect 400 --reason 'unknown topic'`; custom topic without a locator → `--expect 400 --no-artifact-uri --reason artifact_uri` | no | 400, explicit error, no row | — |
| digest of nothing → `--expect 400 --artifact-digest $(sha256sum </dev/null | cut -d' ' -f1) --reason 'sha256 of empty input'` (the sha256 of zero bytes; the empty-tar digest, `head -c 10240 /dev/zero | sha256sum`, is refused the same way) | no | 400, explicit error, no row — the CP never scores a digest of nothing | — |

After each row `docker compose logs proof-challenge` must show no
`topic vm created`, and the KVM host journal no `topic vm booted`; the
`/v1/submissions` list gains no row. Restore the knob and run
`proof-vm-wire-check.sh cp` before the next flip.

### 5. Happy path (one real run, real artefact)

The live run is judged on **bytes**: the RLM VM fetches `artifact_uri`,
tars the tree, and the KVM host re-hashes that tar against the submission's
`artifact_digest` before it boots a sister — then checks it *is* an artefact
(an uncompressed tar with at least one byte of file content). So the probe
needs a real recipe, served where the RLM VM can reach it. A random digest
can never pass this path honestly; the staging happy path that once went
green matched the digest of an **empty** tree because the RLM VM could not
reach the artefact host and its guest agent substituted an empty artefact.
That stub is staging history, not a path: the CP now answers 400 to a digest
of nothing, the host refuses a content-less tar by name, and the harness
refuses to start a live run without the real digest.

1. Build the recipe and hash it — the bytes the sister will unpack, as an
   **uncompressed** tar:

   ```bash
   tar -cf recipe.tar -C <dir> recipe        # run.sh + code, no gzip
   sha256sum recipe.tar                      # this is artifact_digest; never type it by hand
   ```

2. Serve exactly that file at an `https://` (or `http://`) URL the RLM VM
   can reach: an object-store URL, or a web server on an address the VM
   routes to. Never the CP host's loopback (`127.0.0.1` inside the VM is the
   VM), never `example.invalid`. The artefact host's IP must be in the
   agent's `PROOF_VM_AGENT_EGRESS_ALLOW` (`<ip>/32:443`), and on a KVM host
   with ufw / Docker the TAP forward must be accepted (§ Egress below).

3. Run the probe with the file; it re-hashes the URL from the CP host before
   anything is spent and stops on a mismatch or a fetch failure:

   ```bash
   ./deploy/scripts/proof-vm-wire-check.sh submit-probe --topic <custom-topic-id> \
     --expect 201 --allow-live-run --artifact-file recipe.tar --artifact-uri https://<artefact host>/recipe.tar
   # PASS  the URL serves the declared bytes (sha256 … fetched + re-hashed here)
   # … PASS  row carries the real artefact digest … (the sister ran these bytes, not a stub)
   ```

   `--artifact-digest HEX` replaces `--artifact-file` when the file is not
   on the CP host (the URL is still fetched and compared); `--no-fetch-check`
   only when the URL is reachable from the RLM VM but not from the CP host.
   An empty file, an empty tar, gzip, or the digest of nothing are refused
   before any request (exit 2).

The POST is synchronous (the RLM job runs before the 201). The live probe
declares the topic's whole `flops_budget` (read from `GET
/v1/proof/topics/<id>`; `--declared-flops N` overrides) so the sister's
measurement is judged against the budget, not against the token `1` the
fail-closed probes send — a run over its own declaration is a
`flops_under_declared` reject, which is the miner rule, not a wire fault.
Evidence to collect, in order:

| Step | Where | Must show |
|------|-------|-----------|
| artefact reachable from the VM | agent journal | no `sister request names …`, no `artifact_tar …` refusal; a guest that could not fetch must answer the job with `Failed` (the CP logs the error, 503, no row) — never a substitute tree |
| topic VM created (first job) | CP log · agent journal | `topic vm created` · `topic vm booted`, `rlm guest ready`, `owner key material staged` when a key dir is set; **no** `host forward chains drop by default` warning (§ Egress) |
| inspection (`Inspect` job, no miner code, no sister) | agent journal | the job, no `sister guest` line |
| paid run (`Evaluate`) in the sister | agent journal | `sister guest booting (no network)` → `sister guest run attested` with `sandboxed=true` and the guest's `flops_used` |
| evidence bound to the job | agent + CP | no `evidence_mismatch` (agent 502) and no `orchestrator evidence is not this job's` (CP): the attestation named this job's topic / submission / artefact |
| host-stamped facts on the row | `submit-probe` output · `GET /v1/submissions/<pf_id>` | `state: awaiting_admin`, `verdict.agent.flops_used` > 0 (the sister's measurement, never the RLM's), `artifact_digest` = `sha256sum recipe.tar` |
| `sandboxed: true` in the artefact | CP volume | `docker compose cp proof-challenge:/var/lib/proof/artefacts/<topic>/<pf_id>.zip /tmp/ && unzip -p /tmp/<pf_id>.zip report.json` |
| sister destroyed | agent journal · KVM host | `jail released`; `/srv/jailer/firecracker/` has no `<vm_id>-s<n>` |
| topic VM teardown | close the topic → CP log · agent journal · KVM host | `topic vm teardown` with `state: Destroyed`, `confirmed: true` · `topic vm torn down` · `/srv/jailer/firecracker/<vm_id>` gone, `nft list tables` has no `proof_vm_pfc<n>` |

Then run the § Verify cleanup probes (failed `ip tuntap`, deadline cut,
`kill -9`) at least once on the staging KVM host.

### 6. Sign-off

Staging is "wired and tested" when all of these are in the change log with
dates and the exact commands:

- [ ] placement recorded: colocated on `cortex-staging` (the allowed,
      proven nested-KVM exception — staging only) or dedicated DO metal, with
      `ls -l /dev/kvm` + `kvm-ok` output from that host; none of the
      fragility signs above appeared during the run.
- [ ] `proof-vm-wire-check.sh all` → all PASS on the staging master.
- [ ] `proof-vm-wire-check.sh boot-probe` → all PASS; KVM host left clean.
- [ ] every row of § 4 → the expected 503 (or 400) with the expected reason,
      no row, no VM, no rent; knob restored; `cp` PASS again.
- [ ] § 5 evidence table complete for one submission, including
      `flops_used` on the row and `sandboxed: true` in `report.json`,
      with the row's `artifact_digest` = `sha256sum` of the served
      `recipe.tar` (never a random or empty digest, never a guest that
      substituted bytes); no `host forward chains drop by default` warning
      in the agent journal for that VM.
- [ ] topic close → VM destroyed, host clean.
- [ ] `GET /v1/status` and `GET /v1/proof/topics` still leak nothing
      (`cp` checks both).

Where an item cannot be run yet (no custom topic sealed, no RLM image
built), write **unknown / not run** with the blocker — never a green box
without the evidence.

### Rollback

Comment out `PROOF_VM_ORCHESTRATOR_URL` (or all four keys) in the age
source, re-materialize, restart `proof-challenge`: the host logs
`no topic-vm orchestrator (…)` and every custom topic answers 503 with that
reason — the same state as before the flip. `systemctl stop
proof-vm-orchestrator` on the KVM host; live VMs die with the agent (no
`--daemonize`); remove `/srv/jailer/firecracker/*` by hand before the next
start (§ Limitations).

## Verify a submission end to end (mandatory, see root `AGENTS.md`)

1. `POST /v1/submissions` on a custom topic with a listed id → the first job
   creates the topic's VM (`journalctl -u proof-vm-orchestrator`: `topic vm
   booted`, `rlm guest ready`, `owner key material staged` when a key dir is
   set), inspection runs (`Inspect` job), then `Evaluate` → `sister guest
   booting (no network)` → `sister guest run attested` with `sandboxed=true`
   and the guest's `flops_used`.
2. The persisted row's verdict carries that `flops_used`; the artefact zip's
   `report.json` has `sandboxed: true`.
3. Failure probes, each **503 with no row and no rent**
   (`proof-vm-wire-check.sh submit-probe --topic <id> --expect 503 --reason <text>`
   asserts the status and the reason; § DigitalOcean staging has the matrix):
   - stop the agent → `orchestrator unreachable`;
   - empty `/etc/proof-vm/token` → `orchestrator refused the bearer`;
   - remove `PROOF_RLM_VM_IMAGE_DIGEST` → `PROOF_RLM_VM_IMAGE_DIGEST … missing or out of range`;
   - delete the RLM image file → agent `503 not_ready: image … no … on this host`;
   - `firecracker_required` topic whose RLM never asked for a sister →
     `firecracker_required run came back without the host's sister-guest attestation`;
   - an RLM whose `SisterRequest` names another submission or artefact than
     the job → agent log `sister request names submission_digest … the paid
     job names …`, no sister jail is built, and the run comes back without
     an attestation (same 503 as above). A hypervisor that ever presented
     evidence for another identity would be a `502 evidence_mismatch` from
     the agent and `orchestrator evidence is not this job's` on the CP.
4. `GET /v1/proof/topics` still leaks no holdout; `GET /v1/status` shows no
   URL, token, or path.
5. Cleanup probes on the KVM host (nothing a run started may outlive it):
   - make `ip tuntap add` fail once (e.g. a stale `pfc<n>` device) → the
     agent logs `topic vm boot failed; releasing its jail` and
     `/srv/jailer/firecracker/<vm_id>` is gone, along with the
     `proof_vm_pfc<n>` table;
   - a paid run whose deadline passes while its sister is still up → the
     sister is killed and `/srv/jailer/firecracker/<vm_id>-s<n>` removed
     before the job answers (log `jail released`);
   - `kill -9` a topic VM's Firecracker → the next `attach` / `create` /
     job / health logs `topic vm process exited outside teardown; reaping`,
     the record becomes `crashed`, the jail is destroyed or retained per the
     topic's policy, and the CP's next job creates a fresh VM (no 409).

## Operate

| Task | How |
|------|-----|
| Is the wire up? | on the master: `./deploy/scripts/proof-vm-wire-check.sh all` (env + agent + admin probe); or `GET /v1/admin/proof/vm-orchestrator` with the operator bearer over loopback |
| Rotate the bearer | write the new token to `/etc/proof-vm/token` and to the CP's token file; no restart on either side (both re-read per request); confirm with `proof-vm-wire-check.sh agent` |
| Rotate the RLM image | stage `images/sha256-<new>.ext4`, set `PROOF_RLM_VM_IMAGE_DIGEST` on the CP, restart `proof-challenge`; running VMs keep the old image until torn down |
| Close a topic | the CP tears the VM down with the topic's `retain` policy (default destroy). `retain` moves `/srv/jailer/firecracker/<vm_id>` to `/var/lib/proof-vm/retained/<vm_id>` (scratch, console log, config) |
| Agent restart | live VMs die with the agent (no `--daemonize`); `attach` then answers 404 and the CP's next job creates a fresh VM. Rules, checklists, and promotions live in the CP's RLM store, not in the VM |
| A topic VM crashed | nothing to do: the agent probes the process on every attach / create / job / health, reaps a dead VM per the topic's `retain` policy (destroy removes the jail; retain moves it for audit — read `console.log` there), records it `crashed`, and the CP's next job creates a fresh VM. A crashed record answers `DELETE` with `state: crashed, confirmed: true` |
| Egress change | edit `PROOF_VM_AGENT_EGRESS_ALLOW`, restart the agent; existing VMs keep their table until torn down |
| Inspect a VM | `nft list table inet proof_vm_pfc<n>`, `cat /srv/jailer/firecracker/<vm_id>/console.log`, `ls /srv/jailer/firecracker/<vm_id>/root/` |

## Egress: the host firewall must let the TAPs forward

The per-VM table (`proof_vm_pfc<n>`) *allows* the egress list, but every
base chain on the netfilter **forward** hook sees the packet and one `drop`
verdict wins. ufw (`DEFAULT_FORWARD_POLICY="DROP"`, the prod droplets run
ufw) and Docker (it sets the iptables `FORWARD` policy to `DROP` — the
colocated staging droplet) both install such a chain, so with them in place
the RLM VM reaches **nothing** — not the judge origin, not the artefact host
— whatever the allowlist says. This is what stranded staging's artefact
fetch ("guest cannot reach the host HTTP"): the nftables `FORWARD` drop, and
a server bound to an address the VM never sees (a container's netns,
loopback).

The agent tells you at every VM boot: `host forward chains drop by default:
guest egress (judge, artefact host) is blocked until the TAPs are accepted
there` with `chains = ["ip filter FORWARD", …]` (it lists `nft -j list
chains` and names foreign forward chains with `policy drop`). Accept the TAPs
in those chains once per host — the allowlist table still drops everything
the list does not name, so this opens nothing beyond it:

```bash
# ufw: /etc/ufw/before.rules, inside the *filter section, before COMMIT
-A ufw-before-forward -i pfc+ -j ACCEPT
-A ufw-before-forward -o pfc+ -m conntrack --ctstate RELATED,ESTABLISHED -j ACCEPT
# then: ufw reload

# Docker (colocated staging): DOCKER-USER is evaluated first and survives docker restarts
# only if re-added at boot — put both lines in a oneshot unit or iptables-persistent
iptables -I DOCKER-USER -i pfc+ -j ACCEPT
iptables -I DOCKER-USER -o pfc+ -m conntrack --ctstate RELATED,ESTABLISHED -j ACCEPT
```

Then verify from inside the path the guest takes, not from the host:

```bash
nft -j list chains | python3 -c 'import json,sys; [print(c["chain"]["family"],c["chain"]["table"],c["chain"]["name"],c["chain"].get("policy")) for c in json.load(sys.stdin)["nftables"] if "chain" in c and c["chain"].get("hook")=="forward"]'
journalctl -u proof-vm-orchestrator -n 200 | grep -c 'host forward chains drop by default'   # 0 after the fix, on a fresh VM boot
```

Serve artefacts on an address the VM routes to and the allowlist names
(object store, or the host's VPC / public address with `<ip>/32:443` in
`PROOF_VM_AGENT_EGRESS_ALLOW`); a web server on the host's loopback or inside
a container's network namespace is unreachable from the VM by construction.

**No fetch fallback, ever.** A guest whose artefact fetch fails must answer
its job with `RlmToHost::Failed` (the CP logs it and answers 503 with no
row). The staging RLM image used to substitute an empty tree instead, and
the happy path matched that empty-file digest; that behaviour is
**staging-only history and forbidden in the production image**. It is also
no longer able to pass: the CP answers 400 to a digest of nothing, the host
refuses a content-less / compressed / non-tar `artifact_tar` before any
sister jail, and the harness will not start a live run without the real
digest of the real file.

## Security model

- **CP never mounts host paths into the RLM VM.** The only bytes that cross
  the wire are `VmJob`s (signed topic, digests, rule versions, run request)
  and their outputs; every test on both sides asserts no path, key, or origin
  in a body.
- **Keys: presence-probe on the CP, material staged by the agent only.**
  `PROOF_RLM_OWNER_INFERENCE_KEY_FILE` is probed for presence at
  `awaiting_owner_keys`; the bytes the RLM uses come from the KVM host's
  `PROOF_VM_AGENT_OWNER_KEY_DIR` over vsock and are never logged.
- **Egress allowlist.** Each RLM VM gets its own nftables table: forward from
  its TAP only to `PROOF_VM_AGENT_EGRESS_ALLOW`, established replies back,
  masquerade out the uplink, drop the rest. Empty list = no egress. The
  sister guest has **no network interface**; the artefact arrives over vsock.
- **The artefact is bytes, never a stub.** Before a sister jail is built the
  host re-hashes `artifact_tar` against the paid job's digest **and** walks
  it: an uncompressed tar with at least one byte of file content
  (`proof_vm_proto::tar`, the guest contract crate). gzip, non-tar bytes, an empty archive, or a
  tree of empty files are refused by name — a digest that matches an empty
  tree is a guest whose fetch failed, not a miner's work. The CP refuses the
  digest of nothing (sha256 of zero bytes / of an empty tar) at submit, 400,
  no row.
- **Hard `topic_id ↔ VM` bind.** Agent: every job / teardown names the topic
  twice (envelope + job) and both must match the VM's. Client: a job for
  another topic never leaves the process; every echo is checked; a created VM
  must report the pinned digest.
- **Host-stamped facts.** `sandboxed` and `flops_used` on paid outputs are
  overwritten by the agent from the sister it booted. An RLM claiming a
  sandbox without a sister is corrected to `false` and the CP refuses the
  report for a `firecracker_required` topic; a sister that measured nothing
  yields `flops_used: null` → 503, never a substituted number.
- **Evidence is bound to its job.** The `SisterAttestation` names the
  `topic_id`, `submission_digest`, and `artifact_digest` the host verified
  against the paid job before it built the sister jail (a `SisterRequest`
  for anything else is refused with no jail). Before stamping, the agent
  checks the attestation **and** the RLM's report against the job
  (`proof_vm_proto::bind_evidence`; mismatch → `502 evidence_mismatch`,
  nothing stamped); the CP runs the same check before accepting. Sister
  evidence for artefact A is never evidence for artefact B.
- **No orphaned host state.** A jail is owned by a guard from `prepare`
  until the VM is registered (or the sister run ends): a failed TAP / rules /
  spawn / handshake step, a cancelled or timed-out sister, or a request the
  CP gave up on releases the process, the TAP, the nftables table, and the
  directory. Sisters are cancelled cooperatively (killed + destroyed before
  the job answers), never aborted mid-flight. A VM whose process died is
  reaped per its retain policy and recorded `crashed`; its topic is free to
  create a fresh one.
- **Jailer.** Firecracker runs chrooted under `/srv/jailer/firecracker/<vm_id>/root`
  as `PROOF_VM_AGENT_JAIL_UID`, with a read-only rootfs copy, a fresh scratch
  drive, and `/dev/kvm` + `/dev/net/tun` mknod'ed by the jailer. No
  `--daemonize` / `--new-pid-ns`, so the agent's child handle is the VM.

## Limitations (v1)

- Agent restarts drop live VMs (state is in the CP's RLM store; the next job re-creates). Jails of VMs that died with the agent are not swept at the next start; remove `/srv/jailer/firecracker/*` by hand before restarting.
- Dead VMs are detected on the next attach / create / job / health call, not by a background reaper; an idle host with a crashed VM reaps it when something asks.
- One sister per paid job; a second `SisterRequest` in the same job is refused.
- Allowlist entries are IPv4 CIDRs; hostnames must be resolved by the operator (allow the resolver's `:53/udp` if the RLM needs DNS).
- mTLS between CP and agent is a follow-up; today the bearer file over TLS is the auth.
- Guest images (RLM, sister) and their vsock agents are built outside this repository against `proof-vm-proto::guest`.
