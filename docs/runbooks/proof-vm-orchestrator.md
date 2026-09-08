# Runbook — Proof topic-VM orchestrator (Firecracker on a dedicated KVM host)

Operator procedure for the `proof-vm-orchestrator` agent that boots one
Firecracker RLM microVM per Proof topic and a **sister** Firecracker guest
for every miner run. Product spec: [`../PROOF.md`](../PROOF.md) § Isolation
boundary. Code: `crates/proof-vm-proto` (wire), `crates/proof-vm-fc`
(control-plane client), `crates/proof-vm-agent` (agent API),
`crates/proof-fc-host` (Firecracker backend), `bins/proof-vm-orchestrator`.

## What runs where

```text
master droplet (DO)                          dedicated KVM host (bare metal, /dev/kvm)
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

Locked by design (do not move any of it):

| Rule | Where it is enforced |
|------|----------------------|
| Firecracker microVMs, **sisters** (RLM VM + miner guest) on one dedicated KVM host — not the control-plane droplet, not Lium, not nested | agent runs only where `/dev/kvm` exists (`ConditionPathExists`); nothing in `proof-challenge` can exec |
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
registered` line per id. `GET /v1/status` → `registered_custom` lists the
ids; an open custom topic with a listed id appears in `scorable_topics`.

The RLM VM shape is 4 vCPU / 8192 MiB. `PROOF_RLM_VM_VCPUS` /
`PROOF_RLM_VM_MEM_MIB` exist for a deliberate change only.

## Verify a submission end to end (mandatory, see root `AGENTS.md`)

1. `POST /v1/submissions` on a custom topic with a listed id → the first job
   creates the topic's VM (`journalctl -u proof-vm-orchestrator`: `topic vm
   booted`, `rlm guest ready`, `owner key material staged` when a key dir is
   set), inspection runs (`Inspect` job), then `Evaluate` → `sister guest
   booting (no network)` → `sister guest run attested` with `sandboxed=true`
   and the guest's `flops_used`.
2. The persisted row's verdict carries that `flops_used`; the artefact zip's
   `report.json` has `sandboxed: true`.
3. Failure probes, each **503 with no row and no rent**:
   - stop the agent → `orchestrator unreachable`;
   - empty `/etc/proof-vm/token` → `orchestrator refused the bearer`;
   - remove `PROOF_RLM_VM_IMAGE_DIGEST` → `PROOF_RLM_VM_IMAGE_DIGEST … missing or out of range`;
   - delete the RLM image file → agent `503 not_ready: image … no … on this host`;
   - `firecracker_required` topic whose RLM never asked for a sister →
     `firecracker_required run came back without the host's sister-guest attestation`.
4. `GET /v1/proof/topics` still leaks no holdout; `GET /v1/status` shows no
   URL, token, or path.

## Operate

| Task | How |
|------|-----|
| Rotate the bearer | write the new token to `/etc/proof-vm/token` and to the CP's token file; no restart on either side (both re-read per request) |
| Rotate the RLM image | stage `images/sha256-<new>.ext4`, set `PROOF_RLM_VM_IMAGE_DIGEST` on the CP, restart `proof-challenge`; running VMs keep the old image until torn down |
| Close a topic | the CP tears the VM down with the topic's `retain` policy (default destroy). `retain` moves `/srv/jailer/firecracker/<vm_id>` to `/var/lib/proof-vm/retained/<vm_id>` (scratch, console log, config) |
| Agent restart | live VMs die with the agent (no `--daemonize`); `attach` then answers 404 and the CP's next job creates a fresh VM. Rules, checklists, and promotions live in the CP's RLM store, not in the VM |
| Egress change | edit `PROOF_VM_AGENT_EGRESS_ALLOW`, restart the agent; existing VMs keep their table until torn down |
| Inspect a VM | `nft list table inet proof_vm_pfc<n>`, `cat /srv/jailer/firecracker/<vm_id>/console.log`, `ls /srv/jailer/firecracker/<vm_id>/root/` |

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
- **Hard `topic_id ↔ VM` bind.** Agent: every job / teardown names the topic
  twice (envelope + job) and both must match the VM's. Client: a job for
  another topic never leaves the process; every echo is checked; a created VM
  must report the pinned digest.
- **Host-stamped facts.** `sandboxed` and `flops_used` on paid outputs are
  overwritten by the agent from the sister it booted. An RLM claiming a
  sandbox without a sister is corrected to `false` and the CP refuses the
  report for a `firecracker_required` topic; a sister that measured nothing
  yields `flops_used: null` → 503, never a substituted number.
- **Jailer.** Firecracker runs chrooted under `/srv/jailer/firecracker/<vm_id>/root`
  as `PROOF_VM_AGENT_JAIL_UID`, with a read-only rootfs copy, a fresh scratch
  drive, and `/dev/kvm` + `/dev/net/tun` mknod'ed by the jailer. No
  `--daemonize` / `--new-pid-ns`, so the agent's child handle is the VM.

## Limitations (v1)

- Agent restarts drop live VMs (state is in the CP's RLM store; the next job re-creates).
- One sister per paid job; a second `SisterRequest` in the same job is refused.
- Allowlist entries are IPv4 CIDRs; hostnames must be resolved by the operator (allow the resolver's `:53/udp` if the RLM needs DNS).
- mTLS between CP and agent is a follow-up; today the bearer file over TLS is the auth.
- Guest images (RLM, sister) and their vsock agents are built outside this repository against `proof-vm-proto::guest`.
