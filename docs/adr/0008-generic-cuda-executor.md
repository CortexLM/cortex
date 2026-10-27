# ADR 0008: generic CUDA executor for challenge references and audit replays

- Status: draft for branch `v3-cuda-executor`, 2026-10-26 (Principal SN100 D26). Forge decides
  promotion to `v3`. Nothing here changes a production run, master, profile, pin or emission,
  and no code in this batch was run on a GPU host (see "Not verified").
- Order: Principal SN100 D26, brief `v3-cuda-executor`. Founder rule of 2026-10-26: rented or
  listed machines are never servers; the challenge stays on the operator host.
- Labels: **[V]** verified in code or by a command on the date above, **[A]** reported by a
  cited document or session, **[H]** hypothesis, **[?]** unknown.

## Context

- The challenge replays a reference, an audit or a referee run itself unless a dispatcher is
  installed. `ChallengeStore._role_launch` is `Callable[[operation, IslandJobV1, Path, Mapping],
  IslandLaunch]`; it is called for `reference` (`challenge/store.py:3000`), `audit` (`:7243`)
  and `referee` (`:8832`), and is `None` unless `HYPERTRAIN_ROLE_DISPATCH` names a file
  (`challenge/app.py:168,282-295,329-333`). [V]
- The only dispatcher is Vast-shaped. `gpu_ops/role_dispatch.load` builds a `Provider` (Vast
  HTTP adapter, `gpu_ops/provider.py`) and a `NetworkRuntime` (`experiments/gpu_network_v2/
  orchestrate.py`, 3,925 lines) over a lifecycle run directory: rental journal, debit
  reservations, an owner `Receipt` per operation, source attestation, tar rescue custody, and a
  canonical publication. It serves exactly one authorized `run_id`, for every backend
  (`role_dispatch.py:34-141`, `store.py:540-551`). [V]
- A CUDA audit also needs `role-dispatch.json` next to the publication. `_dispatch_env_v2`
  (`store.py:7338-7377`) reads `environment` and requires `backend == "cuda"`, the manifest's
  `image_digest`, one identical driver per GPU inside `driver_allowlist`, `sm_counts ==
  [reference_spec.sm_count] * n_gpus`, and `n_gpus` distinct valid GPU UUIDs. Missing or
  malformed: 503 `ROLE_DISPATCH_PROVENANCE_UNAVAILABLE`; mismatch: 409
  `ROLE_DISPATCH_PROVENANCE_MISMATCH`. Only the Vast dispatcher writes the file
  (`role_dispatch.provenance`, `:55-79`). [V]
- S7 (s100 TEST stack) could not run a GPU run: no GPU on s100, host B reclaimed, qualification
  needs two distinct hosts (`network_qualification.compare`, `:507-532`), and there is no
  dispatch target that is not a Vast lifecycle host [A, S7 `REPORT.md`; the three code facts
  re-read here].
- Reusable typed contracts: `IslandJobV1`, `validate_artifacts` (recomputes state, leaf, EF and
  delta commitments and the job binding from the rank files, `miner/island_launch.py:418-558`),
  `gpu_uuid`, `launch_argv`, `network_qualification.SM_COUNTS` and `unqualified_device`
  (ADR 0007), `gpu_ops.journal`. [V]
- The GPU image already carries everything the island needs: `hypertrain.miner.island_worker`,
  torch 2.14.0+cu130 and `/opt/hypertrain/venv/bin/python` (`docker/Dockerfile.gpu`). The worker
  itself refuses a wrong `HT_IMAGE_DIGEST`, driver, SM count or device count
  (`miner/island_worker.py:45-70`). [V]
- Lium B300 hosts: docker through `sudo -n`, `executor-*` containers untouchable, the validator
  cycle can hold the GPUs (OOM at start-up is re-run after the cycle), Spot hosts vanish
  (`docs/b300-run.md`). [V]

## Decision

### 1. Placement: a new transport next to the Vast one

- New modules `gpu_ops/cuda_executor.py` (config, SSH transport, host pool, provenance,
  `load`) and `gpu_ops/cuda_executor_runner.py` (stdlib-only program that runs inside the
  container). `role_dispatch.py`, `provider.py`, `NetworkRuntime` and `Orchestrator` are not
  edited, so the Vast path keeps its behaviour and its `DispatchConfig` schema.
- The challenge selects the executor with `HYPERTRAIN_CUDA_EXECUTOR=<config path>`. It is
  mutually exclusive with `HYPERTRAIN_ROLE_DISPATCH`; setting both fails at startup. The file
  is loaded lazily on first use like the Vast file, so a missing or invalid file answers 503
  `ROLE_DISPATCH_UNAVAILABLE` and is retried (`challenge/app.py` change only).
- `load(path)` returns the same `RoleLaunch` signature. The returned launch serves
  `backend="cuda"` for the one authorized `run_id` only and runs `backend="cpu"` in-process
  through `launch_island`. A CPU run on the same challenge therefore keeps working, which the
  Vast dispatcher does not allow.

### 2. Host contract (what an operator must provide)

A host is any machine reachable by SSH that has an NVIDIA driver, `nvidia-smi`, Docker with the
NVIDIA runtime, and a POSIX login shell. It needs no Python, no checkout, no provider API, no
daemon and no open port.

| Item | Contract |
| --- | --- |
| SSH | `address`, `port`, `user`, `identity_file` (a path), `known_hosts_file` (a path). The config never holds a key. The challenge never reads the key: it passes the path to `ssh -i`. Load refuses an identity file that is not a regular file owned by the process uid with mode 0600. |
| Host key | Pinned by the operator in `known_hosts_file` (verify the fingerprint out of band, for example in the Lium panel). `StrictHostKeyChecking=yes`, `GlobalKnownHostsFile=/dev/null`, `-F /dev/null`, `IdentitiesOnly`, `BatchMode`, no agent, no forwarding, no TTY, `ServerAliveInterval` 15 s, `ServerAliveCountMax` 3, `ConnectTimeout`. No trust-on-first-use: a replaced machine fails the key check and is a lost host. |
| Docker | `docker` argv prefix per host (`["docker"]`, or `["sudo","-n","docker"]` on the Lium and AWS hosts). Only these verbs are used: `image inspect`, `pull`, `run`, `rm -f <own name>`. Containers are named `ht-exec-<16 hex>`, labelled `io.cortex.cuda-executor`, and never touch an `executor-*` or any other container. |
| Image | `image_repository` in the config plus the digest of the signed manifest: the container is started as `repo@sha256:<manifest reference_spec.image_digest>`. A tag is never accepted. If the image is absent it is pulled by digest. |
| Container | `docker run --rm -i --init --name ht-exec-<16 hex> --label io.cortex.cuda-executor=<job> --gpus "device=<uuids>" --ipc=host --ulimit memlock=-1 --ulimit stack=67108864 --entrypoint /opt/hypertrain/venv/bin/python <image> -I -c <runner>`. No volume, no bind mount, no host path, no `-e`: the determinism pins, `HT_IMAGE_DIGEST`, `HT_ISLAND_TRACE`, `HT_GPU_UUIDS` and `CUDA_VISIBLE_DEVICES` travel in `request.json` and the runner applies an allowlisted set of them to the island's environment. These flags are the ones `scripts/b300/run_host.sh` already uses plus `-i`, `--name`, `--label` and the device list. `network` is `default` or `none` per config (see Not verified). |
| Data in and out | Inputs travel as a tar on the SSH standard input; results come back as a tar on the standard output. Nothing is written on the host outside the container's own layer, which `--rm` deletes. The only persistent thing is the pulled image layer cache. |
| GPU probe | One SSH call before each attempt reads `nvidia-smi --query-gpu=uuid,name,driver_version,compute_cap,memory.used`, `--query-compute-apps=gpu_uuid,pid,process_name`, the host epoch and `docker image inspect` (`RepoDigests`). A GPU is idle when no compute app is listed on it and `memory.used` is at most `idle_gpu_memory_mib` (default 2048, the Lium executor keeps about 810 MiB resident). The executor takes the `n_gpus` idle GPUs with the lowest UUIDs. Fewer: the host is busy for this job. |

### 3. Provenance record

For every launch the executor writes `directory/role-dispatch.json` (canonical JSON, no
floats) before it publishes `directory/published`. The file is a superset of what
`_dispatch_env_v2` reads, so the store needs no change.

```text
executor            "cuda-executor-v1"
operation           reference | audit | referee
binding             body_digest(context), same construction as the Vast operation binding
run_id, job_sha256  the job the result is bound to
host                label, machine_id, instance_id, known_hosts_sha256
image               requested "repo@sha256:...", repo_digests reported by the daemon
runner_sha256       sha256 of the runner source that ran in the container
result_tar_sha256   sha256 of the bytes received from the host
publication_sha256  sha256 of the canonical {relative path: sha256} map of published/
environment         backend, image_digest, python, torch, torch_path, cuda,
                    drivers[n], sm_counts[n], capabilities[n][2], gpu_uuids[n], gpu_names[n]
```

- `machine_id` is a positive 52-bit integer (JSON-safe) derived from the sorted UUIDs of **all** GPUs on the
  host (physical identity: two jobs on different GPU subsets of one box, or one box listed twice
  under two labels, share it). `instance_id` is derived from the pinned `known_hosts_file` bytes
  and `address:port` (the OS or rental instance). Both are integers so a future qualification
  `Result` can carry them, and `compare` (`network_qualification.py:518-523`) refuses equal
  values as it does today. [H that a generic-host qualification will reuse them; no such tool
  exists, see S9]
- Idempotence follows `launch_island`: if `published/` exists, `role-dispatch.json` must exist
  and its `publication_sha256` must match the files; otherwise the launch refuses. If
  `published/` is absent, a leftover `role-dispatch.json` is stale (it was written before the
  rename) and is replaced. Write order: validated staging directory, then
  `role-dispatch.json`, then one `rename` to `published/`.
- The record carries `capabilities`, which `_dispatch_env_v2` ignores; the executor checks each
  `(capability, sm_count)` pair with `unqualified_device` (ADR 0007) before it writes.

### 4. Failure model

| Event | Class | Action |
| --- | --- | --- |
| SSH exit 255, connect or key failure, `ServerAlive` timeout, stream cut, a host probe or image pull that outlives its own limit, a failing `nvidia-smi` query | host lost | `docker rm -f` its container best effort and skip the host for the rest of this pass over the host list (the next pass tries it again). The host is not quarantined. Bytes already received are never accepted. |
| Host busy (foreign compute app, memory used, too few idle GPUs) | busy | Try the next host; when all are busy wait `sweep_pause_seconds` and sweep again (`sweeps`, default 3). |
| Clock skew above 5 s, GPU set changed for a label, duplicate `machine_id`, wrong driver, capability or SM pair, image digest not in `RepoDigests`, container sees other UUIDs than selected (runner exit 4), host-reported fields that do not parse or exceed their bounds | host refused | The host is quarantined until the challenge restarts and the next host is tried. A refusal is not a job failure, and the pass over the host list ends as soon as every host is quarantined. |
| Container exits non-zero (OOM during a validator cycle, island refusal, a device probe that could not run (runner exit 9), `docker run` error), even if it wrote a complete tar first | attempt failed | Count one attempt (`max_attempts`, default 3); retry on another host, or on the same host after a pause when it is the only one. |
| Exit 0 but no `COMPLETE` member, hash mismatch, size over `max_result_bytes` (tar or the sum of its members), unsafe tar member (link, device, sparse, duplicate, foreign name), any failure while validating the received bytes, `backend != cuda`, missing or malformed `trace.json` when traced, a record the store could not read back | attempt failed | Nothing is published. |
| Job deadline reached or `cancel` set | abort | Kill the SSH process group, `docker rm -f`, raise `IslandFailure`. No retry. |
| All hosts and attempts exhausted | no result | Raise `IslandFailure`; the lease or reference fails as it does today. No partial result exists to accept. |

A result is accepted only when all of these hold: SSH exit 0, the final tar member `COMPLETE`
lists the sha256 of every other member and of the received `request.json`, every file hash
matches, `validate_artifacts(job, staging)` passes, all rank summaries say `cuda`, the
environment checks of section 5 pass, and the job deadline is not over. The publication is
one `rename`.

### 5. Security: what the challenge does not take from the host

The host is operator-owned and qualified by the existing two-host bitwise qualification, so
this executor is as trusted as the Vast hosts it replaces. It cannot be stronger than that:
anyone with root on the host can forge every observation below [V by reasoning, same as S9 on
`compare`]. What the challenge still binds itself:

- The container image is requested by the manifest's digest; the daemon pulls and runs that
  content. The challenge records `RepoDigests` and refuses an answer that lacks the digest. The
  `HT_IMAGE_DIGEST` that the worker compares is set by the challenge, so the worker's own check
  is circular, as the S5 harness already noted.
- The result is bound to the job: `validate_artifacts` recomputes the leaf, state, EF and delta
  commitments from the received files and compares `job_hash` with the canonical job the
  challenge itself wrote, in the challenge process on CPU. The audit verdict (MATCH or MISMATCH)
  is still computed by the challenge from the miner's committed values.
- The environment is read from the same container that trained (torch properties and
  `nvidia-smi -i <uuid>`), cross-checked against the host-level probe (same UUIDs, same
  driver), then checked against the signed manifest and the `SM_COUNTS` table.
- Transport integrity: the `COMPLETE` manifest, a byte cap, and a whitelist tar reader (regular
  files under `rank-<n>/` plus `environment.json`, no link, device, absolute or `..` name, no
  duplicate) written by the challenge, never `extractall`.
- Secrets: the SSH environment is rebuilt (`PATH`, `HOME` and `LC_ALL` only); no token, seed, bearer or key content
  reaches a command line, the container, the journal or `role-dispatch.json`. The identity file
  is a path. Remote stderr is bounded (16 MiB), stored 0600 under a 0700 `logs/` directory of the executor state directory, and shown in errors only as a printable 600-byte tail.
- Residual risk, unchanged from the Vast path: a compromised host can return a self-consistent
  but wrong trajectory, which `validate_artifacts` cannot detect. A false MISMATCH against an
  honest miner is the harmful direction; a false MATCH needs the miner's outputs.

### 6. What stays unchanged or Vast-only

Vast rental lifecycle, debit, rescue custody, `NetworkRuntime`, `Provider`, `DispatchConfig`,
`HYPERTRAIN_ROLE_DISPATCH`, the two-host qualification bootstrap (`_bootstrap_backend_v2`
still takes two Vast-shaped `Qualification` and `Result` files), `launch_window` (the dispute
window re-run still runs where `island_bisect` runs it), and the miner `probe` and `live`
operations. No protocol constant, manifest field, wire format or `implementation_hash` input
changes. `scripts/network_gpu_prepare.py:554` keeps the relay-manifest construction S7 found
wrong (outside this batch).

## Hash consequences

Measured with `network_qualification.sources()`/`bundle()` and
`ChallengeStore._service_implementation_v2`, before (`a5b60804`, the S7 tip, 136 files) and
after this batch [V; tool: `measure_hashes.py` over a `git archive` of each commit]:

| Value | Before | After |
| --- | --- | --- |
| `implementation_hash` | `02e208a3…0189cc` | unchanged |
| `store.py` sha256 | `b5bc9904…aaec88` | unchanged |
| `network_qualification.py` sha256 | `dab7a6ab…7bda15` | unchanged |
| `sources()` map, canonical sha256 | `a9f091ad…64e274` (136 files) | `9616cce4…2661fb` (138 files) |
| `bundle-hash` | 2,201,600 bytes, `8340ae6d…73c7d7` | 2,252,800 bytes, `6eb0fe03…e9572d` |

Consequences, each the same kind as ADR 0007's:

- `sources()` hashes every `src/hypertrain/**/*.py`, so the two new modules, the relay change
  (S7 defect b) and the `app.py` change alter the map and the reviewed bundle. Older
  qualification configs no longer validate against a tree at this commit
  (`qualification_source_or_profile_changed`) and the dispatch source pins
  (`role_dispatch.py:112`) must be recomputed. A config validates against its own frozen tree.
- Installed qualification records stay valid (`_backend_v2` never re-parses `result.json`).
- No run id, golden root or `reference_hash` changes: no manifest, schema or constant is edited.
- The v3 build gives new server and GPU image digests; a manifest that pins a new GPU digest has
  a new run id. The runner is shipped per call, not baked in, so **the existing GPU image
  `4e5f4b7e` works unchanged** [H: it holds `hypertrain.miner.island_worker`; not run].
- The server image already contains `openssh-client` and a passwd entry for uid 65532
  (`docker/Dockerfile.server`), so no Dockerfile change is needed.

## Consequences

- A challenge without a GPU can obtain CUDA references and audit replays from any operator-owned
  host, and CUDA audits carry provenance the existing store checks accept.
- Concurrency is one job per host at a time (an in-process lock per host); jobs spread over the
  configured hosts. Throughput on a single host is serial.
- Transfer volume equals the publication size (checkpoints included), bounded by
  `max_result_bytes` (default 16 GiB). Models whose publication exceeds that, or whose transfer
  does not fit the lease, need a different transport [?].
- The key and `known_hosts` files must be readable by uid 65532 inside the challenge container.
  Installing them on s100 is an operator step on the TEST stack only and is not part of this
  batch (`hypertrain/docs/cuda-executor.md`).

## Rejected alternatives

1. A Lium, AWS or other provider API client next to `Provider`: the brief forbids a provider API,
   each provider differs, and the generic part is SSH plus a digest.
2. Generalizing `NetworkRuntime` and `Orchestrator` to static hosts: they own rentals, debit and
   rescue, and 7.9k lines of Vast lifecycle would be touched; any edit changes the dispatch
   behaviour that production relies on.
3. Running the challenge on the GPU host: breaks the founder rule that rented machines are never
   servers.
4. A resident agent or HTTP worker on the host: a persistent service and a new attack surface on
   a machine the validator cycles.
5. `scp` or bind-mounted staging on the host: leaves data on a rented machine, needs cleanup, and
   adds path races. SSH streams need none.
6. A checkout and host Python (the `run_host.sh` style): the code would not be pinned by the
   image digest and would need a persistent tree.
7. Trust-on-first-use host keys (`Ssh.trust`): a re-rented machine behind the same address would
   be silently adopted.
8. Trusting `docker inspect .Id` as the image digest: it is the config digest, not the registry
   manifest digest that the manifest pins.
9. Dual execution on two machines by default (accept only bitwise-equal results): it closes the
   forged-trajectory risk at twice the GPU cost and needs a decision rule for disagreement. Left
   as a later option; the `machine_id` rule is what would make it safe.
10. Adding a capability check to `_dispatch_env_v2`: that edits `store.py` for no gain, since the
    executor refuses a bad pair before it writes the record.

## Sources

- `hypertrain/src/hypertrain/gpu_ops/{role_dispatch,provider,network_qualification,remote}.py`,
  `challenge/store.py:3000,7243,7338-7377,8832`, `challenge/app.py:168,282-333`,
  `miner/island_launch.py:386-558,561-640`, `miner/island_worker.py:45-70`,
  `experiments/gpu_network_v2/orchestrate.py:810-1282,1712-1898`,
  `scripts/network_gpu_operation.py:161-202`, `scripts/b300/run_host.sh`, 2026-10-26.
- `docs/adr/0007-sm-counts-per-arch.md`, `docs/b300-run.md`, `hypertrain/docs/role-dispatch.md`,
  2026-10-26.
- S7 `REPORT.md` (`v3-s100-test-stack-bcd--02f5798b`) and the S9 notes, 2026-10-26 [A].

## Not verified

- No run on a GPU host: the container flags, `--gpus "device=<uuids>"` with
  `CUDA_VISIBLE_DEVICES`, NCCL and torchrun rendezvous under `--ipc=host` (and under
  `network none`, which `docs/b300-run.md` suggests a multi-rank torchrun does not accept),
  large-result transfer time and the Lium validator cycle interaction are untested [?].
- The tests use a fake `ssh`, a fake `docker` and a fake `nvidia-smi` that execute the real
  transport, the real runner framing and the real artifact checks, but replace the training
  step with a canned valid publication.
- Dispute window re-runs (`launch_window`) stay local and are not dispatched.
