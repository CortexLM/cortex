# Generic CUDA executor (`HYPERTRAIN_CUDA_EXECUTOR`)

The challenge replays references, audits and referee runs itself. A challenge without a GPU (the
s100 hosts) can instead send CUDA jobs to any SSH + Docker GPU host, with no provider API and
nothing installed on the host. Decision record: [ADR 0008](../../docs/adr/0008-generic-cuda-executor.md).
The Vast lifecycle dispatcher ([role dispatch](role-dispatch.md)) is unchanged and stays the
other option; the two are mutually exclusive.

> **Status: not run on a GPU host.** The tests use a fake `ssh`, `docker` and `nvidia-smi` that
> run the real transport, the real container program and the real artifact checks, with a canned
> publication instead of training. Nothing below has been run on a B300 or any GPU.

## What the host needs

- SSH reachable, a user that can run Docker (directly, or through `sudo -n docker`), an NVIDIA
  driver with `nvidia-smi`, Docker with the NVIDIA runtime, a POSIX login shell, a clock within a
  few seconds of the challenge's.
- No Python, no checkout, no listening port. The challenge pulls
  `image_repository@<manifest reference_spec.image_digest>` if it is absent and runs it with
  `docker run --rm`. Inputs go in on the SSH standard input and the rank artifacts come back on
  the standard output; no job file is written on the host. What stays on the host is that image
  and one compute cache directory per image digest ([below](#compute-cache)).
- An x86-64 CPU that reaches the manifest's CPU class (`reference_spec.env.pinned.cpu`, `avx2`
  or `avx512`; [the pin](#the-environment-pin)). ATen clamps the capability to what the CPU
  supports (inferred from the clamping seen on a CPU host; no AVX2-less or ARM GPU host was
  available), so a CPU without AVX2, or an ARM host such as a Grace machine that reports 10.3 and
  148 SMs like a B300, cannot honor the pin: the container program sees it and the host is refused
  and quarantined before any island starts.
- On shared hosts (Lium) never use the `executor-*` containers; the executor only touches
  containers named `ht-exec-<16 hex>`. It takes the `n_gpus` idle GPUs with the lowest UUIDs (no
  compute app on them and at most `idle_gpu_memory_mib` used) and treats the host as busy
  otherwise, so a validator cycle makes the job wait or move to another host.

## Config file

[`examples/cuda-executor.example.json`](examples/cuda-executor.example.json). The challenge
reads it from `HYPERTRAIN_CUDA_EXECUTOR` on first use; a missing or invalid file answers 503
`ROLE_DISPATCH_UNAVAILABLE` and is retried on the next call. Setting both
`HYPERTRAIN_CUDA_EXECUTOR` and `HYPERTRAIN_ROLE_DISPATCH` stops the challenge at startup.

| Field | Meaning |
| --- | --- |
| `run_id` | The one run whose CUDA jobs are served. A CUDA job of another run raises before any SSH call. CPU jobs (any run) still run in-process. |
| `image_repository` | Repository part of the image reference; the digest always comes from the signed manifest. |
| `state_dir` | Journal (`journal.jsonl`) and remote stderr logs (`logs/`), mode 0700. Evidence only. |
| `hosts[]` | `label` (unique), `address`, `port`, `user`, `identity_file`, `known_hosts_file`, `docker` (argv prefix, default `["docker"]`), `compute_cache_dir` (optional absolute root of the cache on that host). 1 to 8 hosts. Two hosts may not share an address and port, nor pin a host key in common: a key line in two `known_hosts_file`s refuses the config (`@cert-authority` and `@revoked` lines are not host keys), so give each host a file of its own. |
| `network` | `default`, or `none` for `--network none`. Whether a multi-rank torchrun starts under `none` is unverified; keep `default` until a GPU run shows otherwise. |
| `compute_cache` | `true` (default): bind-mount a persistent CUDA compute cache into every container ([below](#compute-cache)). `false`: no mount, the ADR 0008 command. |
| `max_attempts`, `sweeps`, `sweep_pause_seconds` | Container runs per job (default 3), passes over the host list (3) and the pause between passes (15 s). |
| `idle_gpu_memory_mib`, `max_clock_skew_seconds` | Idle threshold per GPU (2048 MiB) and the largest accepted difference between host and challenge clocks (5 s). |
| `max_result_bytes`, `pull_timeout_seconds`, `ssh_connect_timeout_seconds` | Result size cap (16 GiB), image pull bound, SSH connect bound. |
| `image_sources_sha256` | Optional pin (`null` by default) of the package files the GPU image holds: the digest a qualification records as `environment.image_sources_sha256`. A container that reports another one, or none, refuses its host like any other contradiction, so reference and referee runs, which the store does not compare, are covered. |

The key is a path, never a value. `identity_file` must be a regular file owned by the challenge
process user with mode 0600 (uid 65532 in the server image, which already ships `openssh-client`);
the challenge never reads it, `ssh -i` does. `known_hosts_file` holds the host keys you pinned
yourself; there is no trust on first use, so a replaced machine fails the key check:

```bash
ssh-keyscan -p 22 -t ed25519 <host> > known_hosts.<label>   # then compare the fingerprint
ssh-keygen -lf known_hosts.<label>                          # with the provider's console
```

The executor does not run or write a qualification. `scripts/ops/qualification_attest.py
--executor-config <this file> --host <label> --result <result.json>` reads the same file (address,
user, key, pinned `known_hosts`) to have a host's own SSH key sign a statement about the
`result.json` of its qualification, which a production run needs
([ADR 0011](../../docs/adr/0011-audit-integrity.md), part 4). It needs the single `ssh-ed25519` key
of that host's `known_hosts_file`.

## Compute cache

A fresh `--rm` container pays 25-45 s of CPU for the first use of the CUDA kernels; the driver's
compute cache (`/root/.nv` in the image, which runs as root) removes most of it. S13 measured a
reference at 57-64 s without a persistent cache and 12-19 s with one, on a B300 host, through a
`docker` wrapper that added a `-v` mount (the first reference after a new image still took 43 s).
The executor now does it by default ([ADR 0012](../../docs/adr/0012-executor-performance.md)). It
has not run on a GPU host: the tests model the mount and the warm/cold difference on the fake host.

- Before each container it runs, as the ssh user and never through `sudo`, `umask 077`, `mkdir -p`,
  a refusal of a link or of a directory it does not own, and `chmod 0700` on `<root>/<first 32 hex of the manifest image digest>`, and
  reads back the path. The root is `$HOME/.cortex-cuda-executor/compute-cache`, or
  `hosts[].compute_cache_dir`, an absolute path of letters, digits and `._-` only (use it when the
  home directory is small, or its path holds a space or a comma; `/mnt/nvme/...` on Lium hosts).
  `mkdir -p` also creates the parents of a named root, with the same umask. A report that is not
  the path asked for is refused. The call is remembered per host and image until a container run
  fails.
- If the directory cannot be prepared (read-only home, odd `$HOME`, a link in the way) the attempt
  goes on **without** the cache: `compute_cache` is false in its provenance and journal entry, and
  the journal gets an `exec_cache_unavailable` line with the host's reason. A lost host (ssh down,
  timeout) is still a lost host. Look for `exec_cache_unavailable` after the first job: a job that
  is slow and has no cache is not an error.
- The container gets exactly one more flag, `--mount type=bind,src=<that directory>,dst=/root/.nv`
  (`--mount`, because `-v` would create a missing source as root). Nothing else changes: no
  `--privileged`, no capability, no device, no `-e`.
- One directory per image digest, so a cache made by another image never serves this one (it does
  not make results independent of the driver and the history of the host: whether the cache can
  change a result is not proven, S13's 28 matches ran with and without it). A new image pays one
  cold start. The driver bounds what it writes (`CUDA_CACHE_MAXSIZE`, its default, not set by the
  executor); nothing prunes old image directories.
- `role-dispatch.json` records `compute_cache: true|false` and the journal's `exec_attempt` the same:
  it says the mount was requested for that attempt, not that it was warm or effective (the first job
  in an empty directory records true).
- Files made by the container belong to root, in a directory owned by the ssh user. Removal is the
  operator's: `sudo rm -rf ~/.cortex-cuda-executor` (or the named root). Do it while no job runs: a
  directory removed under a running executor costs one failed attempt (and the sweep pause).
  `compute_cache: false` restores the previous command exactly.
- **An old `docker` wrapper that adds `-v ...:/root/.nv` (S13) must go**: with the default two
  mounts target `/root/.nv`, which docker is expected to refuse, and every container fails. Put
  `["sudo","-n","docker"]` back in `hosts[].docker`. `cuda_executor_check.py` runs nothing and does
  not exercise this step.
- On a shared host the ssh user is root-equivalent anyway (`docker`, `sudo -n docker`); on a host
  where it is not, a container root can leave root-owned files, including setuid ones, in the cache
  directory.

## The environment pin

Every manifest pins its process environment ([protocol.md](protocol.md), section 7, item 4a;
ADR 0011, part 3), so the container must run the island under exactly that environment whatever
the image's `ENV` or the host's daemon add. The pin travels in `request.json`, under the
`request_sha256` that `COMPLETE` binds:

| Field | Content |
| --- | --- |
| `env` | The fixed names (`PYTHONHASHSEED`, `OMP_NUM_THREADS`, `MKL_NUM_THREADS`, `CUBLAS_WORKSPACE_CONFIG`, `CUDA_DISABLE_PTX_JIT`, `NVIDIA_TF32_OVERRIDE`, `HT_IMAGE_DIGEST`, `HT_ISLAND_TRACE`, `HT_GPU_UUIDS`, `CUDA_VISIBLE_DEVICES`) and every name the pin sets (`ATEN_CPU_CAPABILITY`, `MKL_CBWR`, `OMP_DYNAMIC`, `ONEDNN_MAX_CPU_ISA`, `HT_ATTN_BACKEND`). |
| `unset` | The names the pin leaves unset, as the sorted list of the table (`HT_COMPILE`, `HT_FP8`, `HT_MOE_GROUPED`, `OMP_THREAD_LIMIT`). |
| `gpu` | The pin's GPU class (`sm_103` or `sm_120`). |

The container program (stdlib only; it keeps its own copy of the names, which a test holds equal
to the tables) refuses with exit 2 a request that does not carry the pin whole: a pinned name
missing from `env`, a name outside the tables, an `unset` other than the table's list, an unknown
class. Then, in this order, it sets `env`, removes `unset`, probes and launches. The probe, a
child started under that environment, adds to what it reports `pinned_env` (the value, or null,
of every name of the tables as the child sees it), `cpu_capability` (what
`torch.backends.cpu.get_cpu_capability()` resolved) and the devices' `capabilities`.

Before it starts the island the program compares the report with the request and exits 4 ("container
refused the environment") when a name has another value or is still set, when ATen did not resolve
the capability the pin names (`ATEN_CPU_CAPABILITY` upper-cased), or when the devices are not of the
request's class. The executor maps exit 4 to a refused host, so an honest host that cannot honor
the pin is quarantined at once and not retried, after a cold start, as a failed attempt.

The executor does not take the program's word for it: `_check_environment` judges the same report
against the manifest's pin with the comparison the island worker uses, and quarantines the host
when it diverges (`container environment diverges from the manifest pin: 'HT_FP8', ...`; names the
host chose are shown through `repr`, cut at 40 characters). A report that lacks the two fields or
has the wrong shape is a malformed result, a failed attempt and not a quarantine. Every rank
summary a host returns must show the pin in its `env` observation (`validate_artifacts`): a
summary that does not is a failed attempt, nothing is published, and the host is not quarantined
for what a file says. A manifest whose GPU class is `none` is refused by `launch` before any ssh
call (it describes a CPU run; judging it per host would quarantine them all).

`role-dispatch.json` records `pinned_env` and `cpu_capability` in `environment`. Nothing here ran on a GPU: the fake host replaces the island and the device probe.

The qualification record a CUDA run needs before its run is created follows the same rule:
`network_qualification run` starts its island through the launcher's `island_env`, so it runs
under the manifest's pin whatever the host exports, and `_bootstrap_backend_v2` refuses (403)
reviewed evidence whose rank summaries do not declare that pin in `env`. The challenge also
answers 409 `ENV_CLASS_BACKEND: gpu class none` when a manifest that names no device resolves to
the CUDA backend: a CUDA run needs a manifest of class `sm_103` or `sm_120` (a manifest built from
the example, class `none`, is a CPU run).

## What a job produces

Next to the usual `published/` directory the executor writes `role-dispatch.json`, which
`ChallengeStore._dispatch_env_v2` reads for a CUDA audit. Besides `environment` (backend, image
digest, drivers, SM counts, capabilities, GPU UUIDs and names, the manifest's environment pin as
the container's probe saw it (`pinned_env`, `cpu_capability`), and `image_sources_sha256`, the
digest of the `src/hypertrain/**/*.py` files the container's own image holds, listed by the runner
with `python -I` before the island starts: a measurement failure is exit 9) it records the host label, a
`machine_id` derived from every GPU UUID of the host and an `instance_id` derived from the pinned
host keys, the requested image reference and the daemon's `RepoDigests`, the hash of the container
program, the hash of the received tar and the hash of the published files. Two labels that
resolve to one machine, a label whose GPU set changes, or a label that lists a GPU of
another label (the machine id follows the whole set, so a partial overlap is a second chance to
list the same hardware) are refused. The challenge recomputes
the artifact commitments itself (`validate_artifacts`) and checks the environment against the
signed manifest, the ADR 0007 table and, when the qualification record has one, its
`image_sources_sha256` (409 on another build, 503 when the container reported none) before it
publishes; a host that returns a partial,
tampered or unverifiable result produces nothing, and the job moves to another host or fails.

## A hostile or broken host

Everything a host sends is bytes to check, never a statement to believe
([ADR 0011](../../docs/adr/0011-audit-integrity.md), which amends section 4 of ADR 0008).

- A probe whose `RepoDigests` line is not a plain list of at most 32 references (printable ASCII
  without space, quote or backslash, at most 256 characters each) refuses the host: it is
  quarantined until the challenge restarts and the next host serves the job, with no container
  run on it. The line is read by a grammar, not by a JSON parser.
- A probe whose clock is not a `time_t` (a 400-digit `date +%s` would overflow the comparison with
  the challenge clock) refuses the host the same way.
- A result whose `COMPLETE` or `environment.json` nests deeper than 8 levels (the container
  program writes 2 and 3) refuses the host, after one container run: no honest container program
  writes it. Both documents are read as UTF-8 only.
- A result tar whose extended (PAX or GNU long name) header asks for more than 1 MiB refuses the
  host the same way: the size is the host's claim, and `tarfile` would allocate it in one call.
- Whatever else a parser of ours raises on a host's bytes (`RecursionError`, `MemoryError`,
  `OverflowError`) fails the attempt and the next host runs the job again; nobody is quarantined for
  a cause that may be ours.
- A probe or an image pull that prints more than 1 MiB refuses the host the same way.
- A `trace.json` that the audit reader would refuse (not a list of trace entries, nested too deep, or
  an entry that names another rank) is no trace: the attempt fails and nothing is published.
- A `summary.json`, `leaves.json` or `trace.json` of a rank over 64 MiB (the largest JSON the audit
  reader accepts) is refused before it is staged: the attempt fails.
- When no host can serve the job, `launch` raises a plain `IslandFailure`: "cuda executor found
  no host able to run the job: ..." when every host is quarantined or unavailable, "cuda executor
  gave up: ..." after `max_attempts` failed attempts, and `Aborted` (a subclass) when the job
  deadline passed or the lease was cancelled. The audit route must book all three as the
  executor's failure, not as a verdict about the miner or the auditor.
## Results through R2 (opt-in)

By default the rank artifacts come back on the SSH standard output. For results of several GB that
cannot fit a lease on one SSH stream, point `HYPERTRAIN_CUDA_EXECUTOR_ARTIFACTS` at a settings file
([`examples/cuda-executor-artifacts.example.json`](examples/cuda-executor-artifacts.example.json)):
the container hashes the same tar, declares the hashes in R2, uploads it to presigned part URLs the
challenge hands out, and the challenge fetches it, checking every 4 MiB block and the whole sha256,
before the unchanged checks run. The host never holds a credential. Decision record:
[ADR 0013](../../docs/adr/0013-executor-artifacts-via-r2.md), which also says which result sizes fit
which windows. Nothing here has run against a GPU host or the real R2 service.

| Field | Meaning |
| --- | --- |
| `credentials_file` | The R2 token: a regular file of the challenge's user, mode 0600, holding only `R2_ACCOUNT_ID`, `R2_ACCESS_KEY_ID`, `R2_SECRET_ACCESS_KEY`, `R2_BUCKET` (must start `cortex-v3-`) and optionally `R2_ENDPOINT` (https; plain http only to a literal loopback address). The process environment is ignored. Use a bucket-scoped token, never the Cloudflare account token. |
| `operations` | Which of `reference`, `audit`, `referee` use R2 (default `audit`, `referee`: the default was chosen while a reference held the store lock for the whole launch, which it no longer does since ADR 0012 Part B, and has not been revisited). Others run the SSH way. |
| `part_bytes`, `max_artifact_bytes` | Part size (a multiple of 4 MiB, 8 to 64 MiB; the challenge chooses, the host cannot) and the largest result accepted (at most 10,000 parts). Default 16 MiB and 16 GiB. Raise `max_artifact_bytes` per profile **together with `max_result_bytes`** of the executor config, which still bounds the tar after the download: the executor refuses to load with `max_artifact_bytes` above it. |
| `workers` | Parallel parts: uploads in the container, ranged downloads on the challenge. |
| `reserve_seconds` | Taken from the island's time budget so the transfer has time left (default 60). A lease with less than this plus 30 s left does not use R2 (`exec_artifact_skipped`). |
| `url_margin_seconds`, `url_ttl_cap_seconds` | The two mailbox URLs live the time left to the job deadline plus the margin (at most 7 days); the part and list URLs, made after the island ran, live the same but at most the cap (default 3600). The container is told a shorter life than they have left. |
| `poll_ms`, `offer_wait_seconds`, `store_blip_seconds` | How often each side looks for the other's mailbox object, how long the container waits for the offer, and how long the challenge waits out a store that fails while the island runs (default 30 s) before it ends the attempt. |
| `request_timeout_seconds`, `retry_attempts`, `retry_base_ms` | Per request and backoff, on both sides. |
| `fallback` | `classic` (default): if R2 is unreachable when an attempt starts, that attempt uses the SSH way; `fail`: it fails. |

Requirements: `network` must be `default` (the container reaches R2 over HTTPS; the image needs a CA
bundle). The attempt's objects live under the bucket prefix `exec/<run>/<job>/<nonce>/`; give the
bucket the rules "abort incomplete uploads after one day" and "expire after one day" on the literal
prefix `exec/`. The code deletes its own uploads, objects and mailbox, but a host that still holds a
URL can write a mailbox object again until it expires. A host that cannot reach R2 is skipped without
using an attempt. The journal gets `exec_artifact_begin`, `_offered`, `_done`, `_end`, `_unavailable`,
`_skipped`, `_salvaged` and `_cleanup_failed`; `role-dispatch.json` gets an `artifact` block
(transport, sha256, size, part size, parts, upload and download milliseconds). A host lost after its
last part is salvaged; one lost earlier costs the upload, and the next attempt starts again.

## Install on the `hypertrain-test` stack only (not run)

Preconditions: a TEST manifest for the hardware (`sm_count`, `driver_allowlist`, GPU image digest)
with its CUDA qualification record installed before the run is created, the config validated as
below, and the host change approved. The paths come from the S7 report, not from the host.

```bash
# Validate the file the way the challenge loads it (checks the key mode and the known_hosts file):
docker exec hypertrain-test-challenge python -I -c 'import os, sys
from pathlib import Path
from hypertrain.gpu_ops.cuda_executor import ExecutorConfig
c = ExecutorConfig.model_validate_json(Path(os.environ["HYPERTRAIN_CUDA_EXECUTOR"]).read_bytes())
print(c.run_id, [h.label for h in c.hosts])'
```

Mount the config, the key and the `known_hosts` files read-only, readable by uid 65532, add
`HYPERTRAIN_CUDA_EXECUTOR` to the `challenge` service of `compose.test.yml`, back the file up
first, and run `up -d challenge` like [the role dispatch install](role-dispatch.md). Rollback:
restore the compose backup and run the same `up -d challenge`. Never touch
`cortex-challenge-hypertrain` or `/data/role-dispatch/dispatch.json`.

## Limits

- One job per host at a time; throughput on one host is serial.
- Transfer volume is the whole publication (checkpoints included), capped by `max_result_bytes`.
  Over SSH one stream carries about 2 MiB per round trip; for results of more than a few GB use the
  R2 transfer above. The job inputs still go over SSH. The `dense-1b` geometry (889M parameters)
  moves about 62 GB per job and, with AdamW, has a 10.7 GB state that the part-index ceiling of 8 GiB
  refuses, whatever the transport; start with a small model (ADR 0013, "Windows").
- Dispute window re-runs (`launch_window`) still run where the dispute code runs them.
- The host is trusted like a Vast host: it can return a self-consistent but wrong trajectory, and
  the executor cannot tell. Dual execution on two machines is a later option (ADR 0008).
- Qualification (`_bootstrap_backend_v2`) still takes the two Vast-shaped `Qualification` and
  `Result` files; this executor does not write them.
