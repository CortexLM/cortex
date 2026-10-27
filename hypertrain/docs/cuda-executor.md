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
  `image_repository@<manifest reference_spec.image_digest>` if it is absent (the only thing that
  stays on the host is that image) and runs it with `docker run --rm`. Inputs go in on the SSH
  standard input and the rank artifacts come back on the standard output; no file is written on
  the host outside the container.
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
| `hosts[]` | `label` (unique), `address`, `port`, `user`, `identity_file`, `known_hosts_file`, `docker` (argv prefix, default `["docker"]`). 1 to 8 hosts. |
| `network` | `default`, or `none` for `--network none`. Whether a multi-rank torchrun starts under `none` is unverified; keep `default` until a GPU run shows otherwise. |
| `max_attempts`, `sweeps`, `sweep_pause_seconds` | Container runs per job (default 3), passes over the host list (3) and the pause between passes (15 s). |
| `idle_gpu_memory_mib`, `max_clock_skew_seconds` | Idle threshold per GPU (2048 MiB) and the largest accepted difference between host and challenge clocks (5 s). |
| `max_result_bytes`, `pull_timeout_seconds`, `ssh_connect_timeout_seconds` | Result size cap (16 GiB), image pull bound, SSH connect bound. |

The key is a path, never a value. `identity_file` must be a regular file owned by the challenge
process user with mode 0600 (uid 65532 in the server image, which already ships `openssh-client`);
the challenge never reads it, `ssh -i` does. `known_hosts_file` holds the host keys you pinned
yourself; there is no trust on first use, so a replaced machine fails the key check:

```bash
ssh-keyscan -p 22 -t ed25519 <host> > known_hosts.<label>   # then compare the fingerprint
ssh-keygen -lf known_hosts.<label>                          # with the provider's console
```

## What a job produces

Next to the usual `published/` directory the executor writes `role-dispatch.json`, which
`ChallengeStore._dispatch_env_v2` reads for a CUDA audit. Besides `environment` (backend, image
digest, drivers, SM counts, capabilities, GPU UUIDs and names) it records the host label, a
`machine_id` derived from every GPU UUID of the host and an `instance_id` derived from the pinned
host keys, the requested image reference and the daemon's `RepoDigests`, the hash of the container
program, the hash of the received tar and the hash of the published files. Two labels that
resolve to one machine, or a label whose GPU set changes, are refused. The challenge recomputes
the artifact commitments itself (`validate_artifacts`) and checks the environment against the
signed manifest and the ADR 0007 table before it publishes; a host that returns a partial,
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
  For the 889M-parameter `dense-1b` geometry that is about 60 GB per job by arithmetic [H, not
  measured], more than an audit lease can move over an ordinary link: start with a small model.
- Dispute window re-runs (`launch_window`) still run where the dispute code runs them.
- The host is trusted like a Vast host: it can return a self-consistent but wrong trajectory, and
  the executor cannot tell. Dual execution on two machines is a later option (ADR 0008).
- Qualification (`_bootstrap_backend_v2`) still takes the two Vast-shaped `Qualification` and
  `Result` files; this executor does not write them.
