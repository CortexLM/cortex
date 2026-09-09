# Runbook — Proof experiment VMs (one Firecracker VM per paid job, in-guest runner)

Operator procedure for topics whose **signed params select an in-guest
runner**: their `Baseline` and `Evaluate` jobs run **inside a dedicated
Firecracker microVM created for that one job and destroyed after it**, by an
operator adaptor baked into the guest image, against an experiment pack the
topic pins by digest. Parallel experiments are parallel VMs — never several
harness trials sharing one VM. This extends
[`proof-vm-orchestrator.md`](proof-vm-orchestrator.md) (the KVM host, the
topic VM, the sister path, TLS, egress — all of which still apply); read that
first. Product spec: [`../PROOF.md`](../PROOF.md) § Isolation boundary.

Code: `crates/proof-experiment` (the generic binding + ceilings),
`crates/proof-rlm` (`run_paid_job`), `crates/proof-fc-experiment` (host
layer: pack resolve / stage / attest), `crates/proof-vm-guest` +
`bins/proof-vm-guest-agent` (the guest), `deploy/guest/` (bake, init,
adaptor contract).

## Generalist by construction

Proof runs whatever a signed topic describes. Nothing about a benchmark, a
dataset, a task list, a model, or a harness is compiled into any binary or
committed to this repository; the words that select the in-guest path are
**generic knobs** whose values are topic data:

| `constraints.params` key | Meaning | Shape |
|--------------------------|---------|-------|
| `in_guest_benchmark_runner` (alias `baseline_runner`) | The operator adaptor id the guest resolves under `/opt/proof/runners/<id>/`. Selecting it switches the topic's paid jobs to dedicated experiment VMs | `[a-z0-9][a-z0-9_-]{1,63}` |
| `experiment_pack_digest` | `sha256:` of the pack tar the KVM host stages into the VM. **Required** with a runner — a topic that names a runner without a pack fails closed, and a digest is never invented | `sha256:<64 hex>` |
| `experiment_pack_path` | Optional relative locator of that tar under the host pack dir (default `sha256-<hex>.tar`) | plain relative path, no `..` |
| `experiment_vcpus`, `experiment_mem_mib`, `experiment_disk_mib` | What the topic asks for; held under the ceilings (silent = the ceiling for CPU / memory, the operator default for disk). Over a ceiling = **503**, never a clamp | positive integers |
| `model_pin` (top-level `constraints.model_pin`) | Exported to the adaptor as `PROOF_MODEL_PIN` | `vendor/model[:tag]` |
| any other key | Reaches the adaptor as `PROOF_PARAM_<KEY>` — this is how a topic names its tasks sub-directory, harness agent, concurrency, key file, … | ≤32 printable params |

A topic that names none of these keeps its registered custom runner's
ordinary path (topic VM + sister guest). The example topic content the
operator staged (a "TB4" pack, a Harbor agent, …) lives **outside git** on
the KVM host and in the signed document, and is recognised by nothing here.

## Ceilings (Architecte lock)

| Knob | Default | Where |
|------|---------|-------|
| vCPUs per experiment VM | **16** max (`PROOF_EXPERIMENT_VM_MAX_VCPUS` / `PROOF_VM_AGENT_EXPERIMENT_MAX_VCPUS`) | CP + KVM host |
| Memory per experiment VM | **32 GiB** max (`…_MAX_MEM_MIB=32768`) | CP + KVM host |
| Writable disk per experiment VM | **32 GiB** default (`PROOF_EXPERIMENT_VM_DISK_MIB`), 128 GiB max (`…_MAX_DISK_MIB`) | CP (default), CP + host (max) |
| Experiment VMs at once | `PROOF_VM_AGENT_MAX_EXPERIMENT_VMS` (default **2**; `0` disables) | KVM host |
| Image | `PROOF_EXPERIMENT_VM_IMAGE_DIGEST`, unset = `PROOF_RLM_VM_IMAGE_DIGEST` | CP |
| Pack directory | `PROOF_VM_AGENT_EXPERIMENT_PACK_DIR=/var/lib/proof-vm/packs` | KVM host |

Both sides enforce their own copy: the control plane sizes the spec under
its ceilings and refuses an ask above them before any request; the KVM host
refuses a spec above its ceilings with `400 bad_spec` before any jail. Keep
the two in step. The topic VM keeps its own locked 4 vCPU / 8192 MiB.

**Size the host** for `max_experiment_vms × (ceiling)` on top of the topic
VMs: two experiment VMs at the lock want 32 vCPU / 64 GiB / 64 GiB of disk
beside the 4/8 topic VM — more than a `g-8vcpu-32gb`. On that droplet lower
the ceilings (both sides) or the topic's ask, and keep the count at 1. The
writable disk is a fresh ext4 file per boot under `/srv/jailer` (reflink
filesystems make the rootfs copy free; the scratch is still allocated), so
budget `max_experiment_vms × disk` there.

## What happens on a paid job

```text
control plane                         KVM host (proof-vm-orchestrator)              experiment VM
────────────                          ────────────────────────────────              ─────────────
request.params select a runner  ──▶  POST /v1/vms {spec.experiment}                 
  size under CP ceilings              ceilings ✓ · pack resolved + re-hashed ✓        
                                      jail: rootfs (pinned) + <disk_mib> scratch     boot → hello
                                      StageSecrets (owner keys)              ──────▶ tmpfs
                                      StagePack (verified tar)               ──────▶ verify + unpack
POST /v1/vms/{id}/jobs {Baseline|Evaluate} ──▶ bind: job runner/pack == vm ──▶ exec /opt/proof/runners/<id>/run
                                                                                       ├ fetch artifact_uri, verify, unpack (evaluate)
                                                                                       └ report.json
                                      attest experiment_vm (this vm, this job) ◀────── Done
  bind_evidence ✓ · sandboxed ✓ ◀──   stamp sandboxed / flops_used
DELETE /v1/vms/{id} destroy      ──▶  jail + scratch gone
```

- The spec is public data: runner id, pack digest, sizes. No host path, no
  key, no origin crosses the wire.
- The host re-hashes the pack (`proof_vm_proto::tar::verify_artifact`:
  uncompressed tar with content whose sha256 is the pin) **before** any jail;
  the guest runs the same check on what it received before unpacking.
  Missing, mis-hashed, gzip, empty, or > 160 MiB packs never boot anything.
- The job must name the runner and pack the VM was created for (host bind,
  `400 bad_spec` otherwise) and the VM's topic (agent bind, `409`).
- The attestation is `mode: experiment_vm`, names the VM, the job's topic /
  submission / artefact, `network: egress-allowlist`; `flops_used` is what
  the guest agent reported (see Limitations). The CP refuses an attestation
  for another VM than the one it dispatched to, an unbound one, and a
  `firecracker_required` run without one.
- Destroy runs whatever the outcome; the agent frees the capacity slot when
  the host confirms.

## The guest image (bake)

`deploy/guest/bake-rootfs.sh` builds one ext4 image that serves both topic
VMs and experiment VMs:

| Component | What | Why |
|-----------|------|-----|
| Debian minbase (`--suite trixie`) | coreutils, iproute2, e2fsprogs, python3, curl, jq | init + adaptors |
| `proof-vm-guest-agent` (musl) | vsock `:5000`, `proof_vm_proto::guest` | the protocol half |
| `/sbin/init` (`deploy/guest/init.sh`) + `catatonit` | mounts, cgroup v2, scratch on `/dev/vdb`, run-as user, agent loop | no systemd in the guest |
| rootless podman + crun + fuse-overlayfs + pasta/slirp4netns + podman-compose | containers **inside** the VM as an unprivileged user (`--run-as-uid 1000`, subuid `100000:65536`), store on the scratch disk, `cgroup_manager = cgroupfs` | the harness's container runtime; no nested KVM |
| `--with-harbor --harbor-version X.Y.Z` | Harbor CLI in `/opt/harbor` (venv, pinned) | one example harness; the bake refuses a Harbor whose `harbor run --help` lacks a flag the example adaptor uses |
| `--runner <id>=<dir>` | adaptor under `/opt/proof/runners/<id>/` | operator capability; contract in [`../../deploy/guest/runners/README.md`](../../deploy/guest/runners/README.md) |

**Size budget.** The baked tree must fit `--budget-mib` (default 2560 MiB;
the 1.5–2.5 GiB target — minbase + podman stack ≈ 0.6–0.9 GiB, + Python +
Harbor venv ≈ 0.3–0.6 GiB, + adaptors), the image file is `--size-mib`
(default 3072 MiB) and is mounted **read-only**. Everything a run writes —
pulled container images, harness jobs, the artefact tree, `report.json` —
lands on the per-VM writable disk (`experiment_disk_mib`, ≥ 32 GiB by
default). Pulls are **not** pre-baked: they need the registry hosts (and a
resolver, `--resolver` + `:53/udp`) on the host egress allowlist.

```bash
# on a build box, as root (chroot + mkfs -d); network to the mirror (+ PyPI with --with-harbor)
rustup target add x86_64-unknown-linux-musl
CC_x86_64_unknown_linux_musl=musl-gcc cargo build --release -p proof-vm-guest-agent-bin --target x86_64-unknown-linux-musl
deploy/guest/bake-rootfs.sh \
  --guest-agent target/x86_64-unknown-linux-musl/release/proof-vm-guest-agent \
  --runner <runner id your topic names>=deploy/guest/runners/harbor-podman \
  --with-harbor --harbor-version <exact version you tested> \
  --resolver <resolver ip on the allowlist> \
  --check-kernel-config <the guest kernel's .config> \
  --out-dir ./out
# prints: image=./out/sha256-<hex>.ext4  digest=sha256:<hex>  (+ bake-manifest.txt)
```

`--dry-run` prints the plan without root or network. The kernel check
lists what rootless podman needs (`CONFIG_USER_NS`, `CONFIG_OVERLAY_FS`,
`CONFIG_FUSE_FS`, `CONFIG_VETH`, `CONFIG_TUN`, cgroups, seccomp, netfilter,
vsock, virtio-blk/net, ext4); a stock Firecracker microVM kernel config
usually lacks several — rebuild the guest kernel with them and re-pin
`PROOF_VM_AGENT_KERNEL_DIGEST`.

## RE-LOCK (metal)

Every digest below is `sha256sum` of a file you staged. Nothing here is
typed from a document.

1. **Bake** (above) → `out/sha256-<img>.ext4`, `bake-manifest.txt`.
2. **KVM host — image:** `install -m 0644 out/sha256-<img>.ext4 /var/lib/proof-vm/images/`;
   `sha256sum /var/lib/proof-vm/images/sha256-<img>.ext4` must print `<img>`.
3. **KVM host — pack:** build the pack tar the topic pins from the operator
   content (e.g. `tar -cf pack.tar -C /var/lib/proof <pack dir>` — an
   **uncompressed** tar; keep the file you hash), `sha256sum pack.tar` →
   `<pack>`, `install -m 0644 pack.tar /var/lib/proof-vm/packs/sha256-<pack>.tar`.
   That `<pack>` is the topic's `constraints.params.experiment_pack_digest`
   (a topic pinned to another tar of the "same" tree does not match: tar
   bytes change with mtimes and order — the digest is of **this file**).
4. **KVM host — env:** `PROOF_VM_AGENT_EXPERIMENT_PACK_DIR`,
   `PROOF_VM_AGENT_EXPERIMENT_MAX_*`, `PROOF_VM_AGENT_MAX_EXPERIMENT_VMS` in
   `/etc/proof-vm/orchestrator.env`; the owner key files the adaptor reads
   (`inference_key_file` param) in `PROOF_VM_AGENT_OWNER_KEY_DIR`; registry
   hosts + resolver in `PROOF_VM_AGENT_EGRESS_ALLOW` (+ § Egress of the
   orchestrator runbook for ufw / Docker forward chains);
   `systemctl restart proof-vm-orchestrator`. Boot log:
   `experiment vm layer (one dedicated vm per paid job; pack staged over vsock)`
   with `pack_dir_present=true` and the ceilings.
5. **CP env:** `PROOF_RLM_VM_IMAGE_DIGEST=sha256:<img>` (or
   `PROOF_EXPERIMENT_VM_IMAGE_DIGEST` when topic VMs keep another image),
   `PROOF_EXPERIMENT_VM_*` ceilings agreeing with the host; restart
   `proof-challenge`. Boot log: `firecracker topic-vm orchestrator wired` now
   carries `experiment_max_vcpus`, `experiment_max_mem_mib`,
   `experiment_disk_mib`, `experiment_image`.
6. **Topic:** the signed document's `constraints.params` carry the runner id
   your bake installed, `experiment_pack_digest: sha256:<pack>`, the
   adaptor's `PROOF_PARAM_*` inputs, and any size ask under the ceilings;
   `metric.custom_id` is in `PROOF_VM_RUNNER_CUSTOM_IDS`. Publish, run
   `TopicSetup` (the baseline is the first experiment VM), seal, open.
7. **Verify** (below); record the evidence with dates and commands.

Running VMs keep the image they booted; a new image applies from the next
create. Rotating the pack = re-signing the topic (its digest is signed).

## Verify

Fail-closed rows — each **503 (or 400), no row, no experiment VM, no
spend**. Use `proof-vm-wire-check.sh submit-probe --topic <id> --expect
<code> --reason <text>` where applicable and read the agent journal:

| Flip | Expect | Journal / probe shows |
|------|--------|-----------------------|
| topic selects a runner, no `experiment_pack_digest` | 503 `experiment_pack_digest is required` | nothing created (refused on the CP before any request) |
| pack file absent on the host | 503 `experiment pack sha256:… (no …/packs/… on this host)` | no jail (`Image` before any boot) |
| pack file present but re-tarred / wrong bytes | 503 `experiment pack …: … hashes to …` | no jail |
| `experiment_vcpus: 32` with the 16 ceiling | 503 `experiment vcpus 32 exceeds the ceiling 16` | nothing created |
| host ceiling lower than the CP's | 503 `orchestrator 400 … BadSpec: experiment … exceeds the ceiling` | no jail |
| `PROOF_VM_AGENT_MAX_EXPERIMENT_VMS` reached | 503 `orchestrator 503 … Capacity: this host runs N of at most N experiment vms` | no boot |
| runner id not baked (`/opt/proof/runners/<id>/run` missing) | 503 `runner … is not installed in this guest image` | `experiment vm booted` → guest `Failed` → destroyed; **no value reported** |
| adaptor writes no `report.json` / non-finite value / outlives the deadline | 503 with the adaptor's exit + redacted tail / `cut at the deadline of Ns` | destroyed |
| `artifact_uri` unreachable from the VM or bytes ≠ `artifact_digest` (evaluate) | 503 `artifact fetch … refusing to run a substitute` | destroyed; no sister, no attestation |
| guest agent absent from the image (old RLM image) | 503 `pack staging answered …` / boot timeout | boot fails, jail released |

Happy path evidence (one baseline or one submission):

| Step | Where | Must show |
|------|-------|-----------|
| VM created for the job | CP log · agent journal | `experiment vm created for one paid job` (runner, pack, vcpus, mem, disk) · `experiment vm booted` with the same, `experiment pack staged` |
| pack verified in the guest | guest console (`/srv/jailer/firecracker/<vm>/console.log`) | `experiment pack staged` from `proof-vm-guest-agent` |
| run attested | agent journal | `experiment vm run attested` with `flops_used` from the guest and the VM id; no `sister guest` line |
| host-stamped facts on the row | `GET /v1/submissions/<id>` · artefact `report.json` | `sandboxed: true`, `verdict.agent.flops_used` = the guest's figure, evidence `runner` / `pack_digest` |
| VM destroyed | agent journal · KVM host | `jail released`; `/srv/jailer/firecracker/` has no `<topic>-x<n>` after the job |
| capacity freed | `GET /v1/health` (agent) | `experiment_vms` back to 0 |

`GET /v1/health` on the agent now reports `experiment_vms` /
`max_experiment_vms`; `GET /v1/status` and `GET /v1/proof/topics` on the CP
still leak no path, key, or origin (the wire check's `cp` step).

## Limitations (v1, stated plainly)

- **Rootless podman, not Docker-in-VM.** Firecracker guests have no nested
  KVM; containers inside the VM are namespaces + cgroups run by an
  unprivileged user. Harnesses that need Docker-daemon-only features,
  privileged containers, or `--network host` semantics may behave
  differently under podman's API socket and pasta/slirp4netns networking.
  Smoke the harness on the baked image before signing a topic on it.
- **Guest kernel.** The stock microVM kernel config lacks user namespaces /
  overlayfs / fuse / veth / tun; the bake's `--check-kernel-config` names
  what is missing. Until the guest kernel is rebuilt and re-pinned, rootless
  podman does not start and every in-guest run fails closed.
- **`flops_used` is guest-agent-authored.** For a sister run the host relays
  a measurement from a guest it fully controls; for an experiment VM the
  figure comes from the adaptor's `report.json` through the pinned guest
  agent, relayed by the host. An agentic harness has no FLOP counter: the
  topic either sets `flops_budget: 0` or supplies an accounting param the
  adaptor uses — the adaptor never invents one, and a budgeted topic with no
  figure is `503` (`FlopsMissing`).
- **Inspection needs an adaptor.** The anti-cheat checklist is ticked by the
  adaptor's `inspect` entrypoint (topic RLM work); the example adaptor ships
  none, so such a topic cannot reach `Evaluate` until the operator provides
  one — by design, no spend without a green checklist.
- **Pack size.** Packs travel in one vsock frame: ≤ 160 MiB uncompressed
  tar. Larger packs need a block-device staging path this protocol version
  does not have; the host refuses them by name.
- **Sister path unchanged.** Topics without in-guest params keep the topic
  VM + sister guest; a VM running this guest agent answers their paid jobs
  `Failed` (no adaptor), so keep the RLM image for such topics or give them
  an adaptor.
- **Not yet run on metal.** Everything here is tested against the fake
  hypervisor, a recording stager, and shell-script adaptors; the bake was
  planned (`--dry-run`) but no image has been built or booted from this
  change. Record the first bake's `bake-manifest.txt` and the § Verify
  evidence before treating an experiment topic as scoring.
