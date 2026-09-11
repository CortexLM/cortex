# Runbook — Proof experiment VMs (one Firecracker VM per paid job, in-guest runner)

Operator procedure for topics whose **signed params select an in-guest
runner**: their `Baseline` and `Evaluate` jobs run **inside a dedicated
Firecracker microVM created for that one job and stopped after it** —
destroyed when the job succeeded, **retained** on the KVM host when it failed
(§ Retained jails) — by an operator adaptor baked into the guest image,
against an experiment pack the topic pins by digest. Parallel experiments
are parallel VMs — never several harness trials sharing one VM. This extends
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
| `baseline_runner` (synonym `in_guest_benchmark_runner`) | The operator adaptor id the guest resolves under `/opt/proof/runners/<id>/` — e.g. `baseline_runner=operator_adaptor_v0` selects the adaptor the operator baked under that id. Selecting it switches the topic's paid jobs to dedicated experiment VMs | `[a-z0-9][a-z0-9_-]{1,63}` |
| `experiment_pack_digest` | `sha256:` of the pack tar the KVM host stages into the VM. **Required** with a runner — a topic that names a runner without a pack fails closed, and a digest is never invented | `sha256:<64 hex>` |
| `experiment_pack_path` | Optional relative locator of that tar under the host pack dir (default `sha256-<hex>.tar`) | plain relative path, no `..` |
| `experiment_vcpus`, `experiment_mem_mib`, `experiment_disk_mib` | What the topic asks for; silent = the operator defaults (lock 16 vCPU / 32 GiB / 32 GiB disk), an ask is held under the ceilings (lock 16 vCPU / 32 GiB — a **hard** maximum no operator ceiling may exceed; disk ≥ 16 GiB) — a topic may ask for less, never for more. Over a ceiling = **503**, never a clamp | positive integers |
| `model_pin` (top-level `constraints.model_pin`) | Exported to the adaptor as `PROOF_MODEL_PIN` | `vendor/model[:tag]` |
| `miner_byok`, `miner_env_allowlist` | Environment variable names the **miner** must / may supply on the submit body's `env`. The miner's values are exported into this VM for the paid job under those names, written to `$PROOF_MINER_ENV_DIR/<NAME>` (0600), and redacted from everything the guest ships back. The operator's own key is never substituted for a missing one ([`docs/PROOF.md`](../PROOF.md) § Miner BYOK) | comma-separated `[A-Z][A-Z0-9_]{0,63}`, never `PROOF_…` |
| `inject_miner_env_sister` | `"true"` also forwards that env to a sister guest running the miner's entrypoint | `"true"` / `"false"` |
| any other key | Reaches the adaptor as `PROOF_PARAM_<KEY>` (upper-cased, `-` → `_`; two signed names that collide after that are refused before anything runs) — this is how a topic names its tasks sub-directory, harness agent, concurrency, key file, … | ≤32 printable params |

A topic that names none of these keeps its registered custom runner's
ordinary path (topic VM + sister guest). The topic content the operator
staged (the pack, the harness CLI and agent the adaptor drives, how a trial
becomes a number) lives **outside git** — on the KVM host, in the baked
image, and in the signed document — and is recognised by nothing here.
`deploy/guest/runners/` ships the adaptor **contract**, a fail-closed
skeleton, and the versioned Harbor reference adaptor
[`rlm_fc_in_guest_harbor/`](../../deploy/guest/runners/rlm_fc_in_guest_harbor/)
(bake with `--runner rlm_fc_in_guest_harbor=<that dir>`). Harbor CLI, venv,
and task packs stay operator artefacts. Proof binaries still compile none of
that in.

## Resource caps (Architecte lock)

| Knob | Lock | Where |
|------|------|-------|
| Default per-experiment VM (topic silent) | **16 vCPU / 32 GiB RAM** (`PROOF_EXPERIMENT_VM_VCPUS=16`, `PROOF_EXPERIMENT_VM_MEM_MIB=32768`) | CP |
| Ceiling a topic's ask is held under | **16 vCPU / 32 GiB** (`PROOF_EXPERIMENT_VM_MAX_VCPUS=16`, `…_MAX_MEM_MIB=32768`; host `PROOF_VM_AGENT_EXPERIMENT_MAX_*`) — the default **is** the ceiling; a topic may ask for less | CP + KVM host |
| **Hard maximum** (`LOCK_MAX_EXPERIMENT_VCPUS` / `LOCK_MAX_EXPERIMENT_MEM_MIB`) | **16 vCPU / 32768 MiB** — not a knob. A ceiling set above it (`PROOF_EXPERIMENT_VM_MAX_VCPUS=32`, `PROOF_VM_AGENT_EXPERIMENT_MAX_MEM_MIB=65536`, …) fails `ExperimentCeilings::validate` and the process **refuses to boot**; a spec shaped above it is refused on both sides whatever the ceilings say. A smaller host may **lower** a ceiling. The disk ceiling is not locked | CP + KVM host (compiled in) |
| Writable disk per experiment VM | **≥ 16 GiB** floor; **32 GiB** default and default max (`PROOF_EXPERIMENT_VM_DISK_MIB` / `…_MAX_DISK_MIB`; raise the max when the metal has more) | CP (default), CP + host (max) |
| Experiment VMs at once | `PROOF_VM_AGENT_MAX_EXPERIMENT_VMS` (default **2**; `0` disables) | KVM host |
| Image | `PROOF_EXPERIMENT_VM_IMAGE_DIGEST`, unset = `PROOF_RLM_VM_IMAGE_DIGEST` | CP |
| Pack directory | `PROOF_VM_AGENT_EXPERIMENT_PACK_DIR=/var/lib/proof-vm/packs` | KVM host |

A silent topic gets the defaults (the whole lock: one experiment, one
16 / 32 machine); a topic may ask for less (`experiment_vcpus: 8`,
`experiment_mem_mib: 16384`), never for more than a ceiling. Both sides
enforce their own copy: the control
plane sizes the spec under its ceilings and refuses an ask above them before
any request; the KVM host refuses a spec above its ceilings with `400
bad_spec` before any jail. Keep the two in step. A smaller host lowers a
ceiling by setting **only** the `MAX` knob: an **unset** default follows the
lowered ceiling on both sides (`PROOF_EXPERIMENT_VM_MAX_VCPUS=8` alone boots
with an 8-vCPU default), while a default you **set** above its ceiling is
refused at boot, never clamped. The topic VM keeps its own locked
4 vCPU / 8192 MiB.

**Size the host** for `max_experiment_vms × (ceiling)` on top of the topic
VMs: one experiment VM at the lock wants 16 vCPU / 32 GiB RAM / 32 GiB of
disk beside the 4/8 topic VM — already more than a `g-8vcpu-32gb`; two want
32 vCPU / 64 GiB / 64 GiB. On a droplet that small keep the count at 1 and
lower the ceilings on both sides (or have the topic ask for less); the
dedicated production host must carry the lock. The writable disk is a fresh
ext4
file per boot under `/srv/jailer` (reflink filesystems make the rootfs copy
free; the scratch is still allocated), so budget `max_experiment_vms × disk`
there; set `PROOF_EXPERIMENT_VM_DISK_MIB=16384` when the metal disk cannot
carry 32 GiB per VM.

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
  (job failed: … retain)         ──▶  jail moved under PROOF_VM_AGENT_RETAIN_DIR
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
- The VM is torn down whatever the outcome; the **policy follows the
  outcome**. A successful job is `destroy`, and its outcome is returned
  **only when the orchestrator confirms the VM destroyed**: a `DELETE` that
  fails or answers anything but a confirmed destroy makes the job
  `VmError::TeardownUnconfirmed` (503, no row, no baseline) even when the
  run itself succeeded — a result is never scored while its VM may still
  hold host capacity. The error names the VM; reconcile it on the KVM host
  (`GET /v1/health` `experiment_vms`, `/srv/jailer/firecracker/<topic>-x<n>`)
  before treating the topic as scoring again. The agent frees the capacity
  slot when the host confirms.
- A **failed** job (guest `Failed`, deadline cut, missing attestation, …) is
  `retain`: the CP returns the job's own error (503, no row — the miner sees
  the real failure, never a teardown error in its place) and the host moves
  the jail under `PROOF_VM_AGENT_RETAIN_DIR` (§ Retained jails). A retain
  the orchestrator did not confirm is logged on the CP with the VM id
  (`experiment vm job failed and the vm was not confirmed retained`) for
  the operator to reconcile; nothing is scored either way.

## Retained jails (failed paid jobs)

The experiment VM of a paid job that failed is **kept, stopped**, for
root-cause analysis: `DELETE /v1/vms/{id}` with `retain` kills the process,
tears the TAP + nftables table down, and moves
`/srv/jailer/firecracker/<topic>-x<n>` to
`PROOF_VM_AGENT_RETAIN_DIR/<topic>-x<n>` (default
`/var/lib/proof-vm/retained`) when that name is free. If a prior retain
already occupies that path (a restarted agent reissues deterministic ids
from 1), the jail lands at `<topic>-x<n>-<stamp>` — GNU `mv` into an
existing directory would nest it as `<id>/<id>` and still report success,
mixing old and new evidence. Success is confirmed only when the source
is gone, that unique destination exists, and it is not nested. What is there:

| Path under the retained jail | What |
|------------------------------|------|
| `console.log` | Guest console: init, `proof-vm-guest-agent`, pack staging, the adaptor's redacted tail |
| `root/scratch.ext4` | The VM's writable disk — the fetched artefact tree, harness jobs, `report.json` if the adaptor wrote one (`mount -o ro,loop` to read) |
| `harvest-work/` | Host `debugfs` dump of guest `/work`. If this tree lags the guest overlay (missing trial `result.json` / `verifier/reward.txt`, or Harbor `n_running>0` / `finished_at=null` on the dump while the guest finished), harvest refuses rather than publishing `report.json`. Guest writes those files; do not treat a lagging dump as a missing guest `reward.txt`. |
| `root/vm-config.json`, `net.nft` | What the VM was booted with (the `root/` copies of the kernel and the pinned rootfs sit beside them) |

Pair it with the CP journal line `evaluate refused; no row` (topic, frozen
digest, the same error string the miner's 503 body carried) to walk from a
miner's failed submission to the guest evidence. Key material is not in the
jail: owner keys and miner BYOK files are staged on the guest's tmpfs
(`/run/proof/secrets`) and die with the VM, and the console tail is the
redacted one — but the scratch holds whatever the harness and the miner's
code wrote, so treat a retained jail as operator-only evidence, never
something to publish or hand back. The agent keeps the record
as `retained` (not running: it holds no capacity slot and never answers
`attach`); `GET /v1/health` `vms` counts it until the agent restarts. Scratch
files are sparse, so a retained jail costs what the run wrote plus the rootfs
copy, but nothing prunes the directory: after the RCA, `rm -rf` the retained
jail (or the whole directory) on the KVM host, and budget the disk for a few
failed runs at `experiment_disk_mib`. A VM whose **process died** outside a
teardown is still reaped per the spec's create-time policy (`destroy` for
experiment VMs), like a topic VM.

## The guest image (bake)

`deploy/guest/bake-rootfs.sh` builds one ext4 image that serves both topic
VMs and experiment VMs:

| Component | What | Why |
|-----------|------|-----|
| Debian minbase (`--suite trixie`) | coreutils, iproute2, e2fsprogs, python3, curl, jq | init + adaptors |
| `proof-vm-guest-agent` (musl) | vsock `:5000`, `proof_vm_proto::guest` | the protocol half |
| `/sbin/init` (`deploy/guest/init.sh`) + `catatonit` | mounts, cgroup v2, scratch on `/dev/vdb`, run-as user, agent loop | no systemd in the guest |
| rootless podman + crun + fuse-overlayfs + pasta/slirp4netns + podman-compose | **fallback** container runtime as an unprivileged user when no rootful engine is overlaid (`--run-as-uid 1000`, subuid `100000:65536`), `cgroup_manager = cgroupfs`, store on paths `init.sh` creates and chowns to that user — `graphroot = /var/lib/proof/containers/storage` (scratch disk), `runroot = /run/user/<uid>/containers` (its `XDG_RUNTIME_DIR` tmpfs); never `/var/lib/containers` / `/run/containers` (root-owned, read-only rootfs) | fallback when Docker is absent; no nested KVM |
| rootful `dockerd` (operator overlay / host-tools) | `init.sh` bind-mounts `$SCRATCH/docker` onto `/var/lib/docker` and starts `dockerd` when it is on PATH; adaptors prefer `/var/run/docker.sock` and Harbor `--env docker`; native overlay, no fuse. `docker-compose` is **not** aliased to `podman-compose` | the agent eval path; Compose v2 is `docker compose` |
| `--extra-pkgs a,b,c` · `--overlay DIR` · `--chroot-hook SCRIPT` | the operator's harness tooling: Debian packages; a tree copied over the rootfs (a prebuilt venv, a CLI); a script run inside the chroot (build a venv, `pip install <tool>==<pinned>`) | generic hooks — this repo names no harness; pin every version the hook installs, record it in your own manifest |
| `--runner <id>=<dir>` | adaptor under `/opt/proof/runners/<id>/` | operator capability; contract + skeleton in [`../../deploy/guest/runners/README.md`](../../deploy/guest/runners/README.md); bake the Harbor reference from [`../../deploy/guest/runners/rlm_fc_in_guest_harbor/`](../../deploy/guest/runners/rlm_fc_in_guest_harbor/) when the topic names that id |

**Size budget.** The baked tree must fit `--budget-mib` (default 2560 MiB;
the 1.5–2.5 GiB target — minbase + podman stack ≈ 0.6–0.9 GiB, + whatever
the operator's hooks add (a Python venv with a harness CLI is typically
0.3–0.6 GiB), + adaptors), the image file is `--size-mib` (default 3072 MiB)
and is mounted **read-only**. Everything a run writes — pulled container
images, harness jobs, the artefact tree, `report.json` — lands on the per-VM
writable disk (`experiment_disk_mib`: ≥ 16 GiB, 32 GiB by default). Pulls
are **not** pre-baked: they need the registry hosts (and a resolver,
`--resolver` + `:53/udp`) on the host egress allowlist.

```bash
# on a build box, as root (chroot + mkfs -d); network to the mirror (+ whatever your hook fetches)
rustup target add x86_64-unknown-linux-musl
CC_x86_64_unknown_linux_musl=musl-gcc cargo build --release -p proof-vm-guest-agent-bin --target x86_64-unknown-linux-musl
deploy/guest/bake-rootfs.sh \
  --guest-agent target/x86_64-unknown-linux-musl/release/proof-vm-guest-agent \
  --runner rlm_fc_in_guest_harbor=deploy/guest/runners/rlm_fc_in_guest_harbor \
  --extra-pkgs python3-venv,python3-pip,git \             # what your adaptor's harness needs
  --chroot-hook /path/outside/git/install-harbor.sh \    # pins Harbor inside the chroot
  --resolver <resolver ip on the allowlist> \
  --check-kernel-config <the guest kernel's .config> \
  --out-dir ./out
# prints: image=./out/sha256-<hex>.ext4  digest=sha256:<hex>  (+ bake-manifest.txt)
```

Build the guest agent as static musl (above) so it runs on the Debian
minbase regardless of the build box; a glibc build only works when the
build box's glibc is not newer than the guest suite's. `--dry-run` prints
the plan without root or network. The kernel check
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
   your bake installed (`baseline_runner: rlm_fc_in_guest_harbor` from the
   `--runner` line above when using the in-repo Harbor adaptor), `experiment_pack_digest: sha256:<pack>`, the adaptor's
   `PROOF_PARAM_*` inputs (names that stay distinct after upper-casing and
   `-` → `_`), and any size ask under the ceilings
   (`experiment_vcpus` ≤ 16, `experiment_mem_mib` ≤ 32768; omit them for
   the 16 / 32768 default); `metric.custom_id` is in
   `PROOF_VM_RUNNER_CUSTOM_IDS`.
   Publish, run `TopicSetup` (the baseline is the first experiment VM),
   seal, open.
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
| `PROOF_EXPERIMENT_VM_MAX_VCPUS=32` or `PROOF_VM_AGENT_EXPERIMENT_MAX_MEM_MIB=65536` (above the lock) | the process **does not boot**: `experiment max_vcpus 32 is above the lock 16 (… the lock is not an operator knob …)`; `proof-vm-wire-check.sh all` fails the knob | — |
| `experiment_disk_mib: 8192` (under the 16 GiB floor) | 503 `experiment disk_mib 8192 is below the minimum 16384` | nothing created |
| host ceiling lower than the CP's | 503 `orchestrator 400 … BadSpec: experiment … exceeds the ceiling` | no jail |
| two params that collide as env names (`foo-bar` + `foo_bar`) | 503 `constraints.params "foo-bar" and "foo_bar" both map to PROOF_PARAM_FOO_BAR` | `experiment vm booted` → guest `Failed` before the adaptor runs → retained |
| `artifact_uri` streams past 64 MiB (no / wrong `Content-Length`) | 503 `artifact at … is larger than 67108864 bytes (aborted after …)` | fetch cut mid-stream inside the VM; retained |
| `DELETE /v1/vms/{id}` fails or is not confirmed after a **successful** run | 503 `experiment vm <topic>-x<n> not confirmed destroyed after its job (…); the outcome is withheld, not scored` — no row, no baseline | the VM is still listed by the agent (`experiment_vms` ≥ 1); reconcile it by hand |
| `DELETE /v1/vms/{id}` (`retain`) fails or is not confirmed after a **failed** run | 503 with the job's own error — no row | CP journal `experiment vm job failed and the vm was not confirmed retained (…); reconcile it on the kvm host`; the VM is still listed by the agent |
| `PROOF_VM_AGENT_MAX_EXPERIMENT_VMS` reached | 503 `orchestrator 503 … Capacity: this host runs N of at most N experiment vms` | no boot |
| runner id not baked (`/opt/proof/runners/<id>/run` missing) | 503 `runner … is not installed in this guest image` | `experiment vm booted` → guest `Failed` → retained; **no value reported** |
| run report `sandboxed=false` on a `firecracker_required` topic | 503 `run report says miner code ran outside the Firecracker guest` | retained (final verification is part of the job outcome used for teardown policy) |
| adaptor writes no `report.json` / non-finite value / outlives the deadline | 503 with the adaptor's exit + redacted tail / `cut at the deadline of Ns` | retained (read `console.log` and `root/scratch.ext4` under `PROOF_VM_AGENT_RETAIN_DIR/<topic>-x<n>`) |
| `artifact_uri` unreachable from the VM or bytes ≠ `artifact_digest` (evaluate) | 503 `artifact fetch … refusing to run a substitute` | retained; no sister, no attestation |
| guest agent absent from the image (old RLM image) | 503 `pack staging answered …` / boot timeout | boot fails, jail released (nothing to retain: the VM never existed) |

Every 503 row above that reached the scorer also leaves a CP journal line
`evaluate refused; no row` (topic, frozen digest, the same error string the
miner's 503 body carried); the rows marked **retained** add `experiment vm
job failed; vm retained on the kvm host for root-cause analysis` naming the
VM. A failed-run jail is **kept** until the operator removes it (§ Retained
jails).

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

- **Docker first, then rootless podman.** Firecracker guests have no nested
  KVM. Prefer a rootful `dockerd` from the operator overlay (native overlay,
  Harbor `--env docker`, agents have network for the pinned model / BYOK).
  `init.sh` starts that daemon when present. `docker-compose` is not aliased
  to `podman-compose`. Rootless podman + fuse-overlayfs remains the fallback
  when Docker is absent; harnesses that need Docker-daemon-only features or
  `--network host` semantics may still differ — smoke the harness on the
  baked image before signing a topic on it.
- **Agent network is on.** Harbor `network_mode=no-network` is rewritten to
  `public` on the filtered task copy so Docker env can start and agents can
  reach the allowlisted TAP (OpenRouter). The host nftables allowlist on the
  Firecracker TAP is unchanged. Do not treat guest-internal `public` as open
  host egress.
- **Task pack duration filter.** The Harbor adaptor copies only the Dev
  **default short-task allowlist** from retained n15 x0017 before
  `n_concurrent` baselines or miner evals (`max_task_duration_s`, default
  3600). INCLUDE: `cargo-flight-dispatch`, `embedding-drift-monitor`,
  `bun-sourcemap-leak`, `fin-saccr-rwa`, `foodstuff-beta-activity`,
  `atrx-vep-crispr`. EXCLUDE >1h: `biped-contact-dynamics` (~5.2h),
  `formal-crypto` (~2.1h), `cad-model` (~1.2h), `data-anonymization`
  (~1.1h). EXCLUDE broken until fixed: `batched-eval-parity` (no-network),
  `ctr-optimization` / `cumulative-layout-shift` (EnvStartTimeout),
  `distributed-dedup` (tmux), `coq-block-bound` (wall cut);
  `biped-contact-dynamics` / `cad-model` also stay out until verifier pytest
  is proven. Pack `filter.json` may only **intersect** that allow-list
  (further restrict) and may only **lower** the duration ceiling. Example:
  `harness/pack_filter.example.json`. An empty filtered set fails closed.
  This does not reseal a stub baseline.
- **Verifier pytest.** Harbor execs `pytest` inside the task environment /
  verifier container. Filtered-copy **environment / verifier / tests**
  Dockerfiles are patched even when FROM is CUDA / MuJoCo / FreeCAD (the
  n15 `biped-contact-dynamics` and `cad-model` hole). `requirements.txt` in
  those dirs also gets pytest. Guest-host pytest does not fix that hole.
  `FROM scratch` / distroless last stages are skipped.
- **Guest kernel.** The stock microVM kernel config lacks user namespaces /
  overlayfs / fuse / veth / tun; the bake's `--check-kernel-config` names
  what is missing. Until the guest kernel is rebuilt and re-pinned, rootless
  podman does not start. A rootful overlay that ships `dockerd` does not
  depend on fuse-overlayfs.
- **`flops_used` is guest-agent-authored.** For a sister run the host relays
  a measurement from a guest it fully controls; for an experiment VM the
  figure comes from the adaptor's `report.json` through the pinned guest
  agent, relayed by the host. An agentic harness has no FLOP counter, and
  the control plane does not reject on missing or over-budget `flops_used`.
- **Inspection needs an adaptor.** The anti-cheat checklist is ticked by the
  adaptor's `inspect` entrypoint (topic RLM work); an adaptor that ships
  none leaves its topic unable to reach `Evaluate` until the operator
  provides one — by design, no spend without a green checklist.
- **Reference adaptor in git, harness CLI not.** `deploy/guest/runners/` is
  the contract, a fail-closed skeleton, and the Harbor evaluate reference
  adaptor (`rlm_fc_in_guest_harbor`). The Harbor CLI, venv, and task pack are
  still operator artefacts baked with the generic hooks. A trial without a
  measurement is never scored from some other value.
- **Incomplete harvest-work dump.** Host reconstruction (`proof-fc-harvest`)
  refuses when `{jail}/harvest-work` is a stale snapshot of guest `/work`:
  missing trial `result.json` / `verifier/reward.txt`, or Harbor job
  `n_running>0` / `finished_at=null` on the artefact used for scoring
  (retained `tbench-x0002`: guest atrx finished with `reward.txt=0` and
  `n_completed=6`; the dump still had `n_running=1` and an empty verifier).
  Do not publish that copy. Guest already writes `reward.txt`; this is not
  an adaptor always-write. Overlay-only (no dump) still harvests.
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
  evidence before treating an experiment topic as scoring. On that first
  boot, check as the run-as user that `podman info` reports
  `graphRoot: /var/lib/proof/containers/storage` and
  `runRoot: /run/user/<uid>/containers` — the paths `init.sh` created — and
  that `podman system service` starts.
