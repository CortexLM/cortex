# B300 host run

`hypertrain/scripts/b300/run_host.sh` runs hypertrain on one AWS p6-b300.48xlarge host (8x B300,
sm_103, driver 595.91.07, EFA). Results go under `<results-root>/<host>/`, where `<host>` is
`hostname -s` unless `--host` is given. Run it on each host, then compare the hosts offline.

```text
run_host.sh [--image REPO@sha256:<64 hex>] [--results-root DIR] [--host NAME]
            [--duration 3h|90m|SECONDS] [--steps "host_probe pull ..."] [--force]
            [--upload s3://bucket/prefix] [--prod]
```

## Prerequisites

- bash 5.1 or newer (Ubuntu 22.04 and 24.04 qualify; the script exits 2 on an older one),
  `python3`, `curl`, `flock`, `git` (the commit of the checkout is recorded), `nvidia-smi`, and
  Docker with the nvidia runtime. The script uses `docker` when `docker info` works, otherwise
  `sudo -n docker`. On the AWS hosts `ubuntu` is not in the docker group, so it takes the `sudo -n`
  path.
- The hosts also run Lium `executor-*` containers. Never touch them. The script never removes a
  container by name. It names its containers `<prefix>-<step>` (`HT_B300_CONTAINER_PREFIX`,
  default `ht-b300`, see the input rules below) and, when a run ends, stops only those that carry
  its own run label. A name already in use makes `docker run` fail (exit 5 to 8). After a hard kill
  (SIGKILL, OOM) the previous run's container may still hold the name and the GPUs: list it with
  `docker ps --filter label=io.cortex.b300-run` (prefix `sudo -n` where the script needs it), check
  that it is yours, and remove it by id.
  Start GPU steps only after the AWS agent reports the host free.
- The monorepo at branch `v3-b300-run` (public repository):
  `git clone --depth 1 --branch v3-b300-run https://github.com/CortexLM/cortex.git ~/cortex`.
  Its `hypertrain/` directory is mounted read-only into every container.
- A results root (default `/results`) writable by the invoking user, as an absolute path of
  letters, digits and `._/+@=,-` (the upload filters and the bind mount cannot take more). Put it
  on the NVMe RAID:
  `sudo install -d -o "$USER" /mnt/nvme/results && sudo ln -sfn /mnt/nvme/results /results`.
  It and the Docker root each need 100 GiB free.
- For `--upload s3://<bucket>/<prefix>` only: the `aws` CLI and an instance role that can write
  to that prefix. A `systemd-run` unit gets the default system PATH, which lacks `/snap/bin`;
  without `aws` on it the log says `upload skipped: aws CLI not found`.
- Outbound HTTPS to ghcr.io and to PyPI (`miner_test` installs `pytest==9.1.1`).

The default image
`ghcr.io/cortexlm/cortex/hypertrain-gpu@sha256:f4b2885f2dc89478da645b9f5e6f02891cd3309ef0a1f2aa0de04328e11cba0f`
is public: an anonymous `docker pull` works without `docker login`. It was built before
`hypertrain.models.kimi_tiny` existed and `gpu_probe` runs a kimi MICRO round from the image's own
package: with this pin the step fails with `ImportError: cannot import name 'kimi_tiny'` (exit 5).
Pass `--image` with a build that contains the module.

## Start, stop, re-run

Start the script detached so that a dropped SSH session does not kill it. It writes its own log
to `<results-root>/<host>/run.log`.

```bash
ssh -i <ssh-key-path> ubuntu@<addr> 'nohup ~/cortex/hypertrain/scripts/b300/run_host.sh --duration 3h --upload s3://<bucket>/b300 </dev/null >/dev/null 2>&1 &'
ssh -i <ssh-key-path> ubuntu@<addr> 'tail -n 40 /results/$(hostname -s)/run.log'
# or as a transient unit
ssh -i <ssh-key-path> ubuntu@<addr> 'sudo systemd-run --unit ht-b300 --collect --uid "$USER" -p KillMode=mixed -p TimeoutStopSec=300 "$HOME/cortex/hypertrain/scripts/b300/run_host.sh" --duration 3h --upload s3://<bucket>/b300'
```

Over SSM, commands run as root and the unit runs as `ubuntu`. Run the clone first:

```bash
aws ssm send-command --region ap-southeast-3 --instance-ids <instance-id> \
  --document-name AWS-RunShellScript --query Command.CommandId --output text \
  --parameters 'commands=["sudo -u ubuntu git clone --depth 1 --branch v3-b300-run https://github.com/CortexLM/cortex.git /home/ubuntu/cortex","install -d -o ubuntu /results"]'
aws ssm send-command --region ap-southeast-3 --instance-ids <instance-id> \
  --document-name AWS-RunShellScript --query Command.CommandId --output text \
  --parameters 'commands=["systemd-run --unit ht-b300 --collect --uid ubuntu -p KillMode=mixed -p TimeoutStopSec=300 /home/ubuntu/cortex/hypertrain/scripts/b300/run_host.sh --duration 3h --upload s3://<bucket>/b300"]'
aws ssm get-command-invocation --region ap-southeast-3 --command-id <command-id> \
  --instance-id <instance-id> --query '[Status,StandardOutputContent,StandardErrorContent]' --output text
```

Send `commands=["tail -n 40 /results/$(hostname -s)/run.log"]` to read progress, and
`commands=["systemctl stop ht-b300"]` to stop.

- **Stop early:** `kill -TERM "$(pgrep -o -f b300/run_host.sh)"` for a `nohup` run, or
  `sudo systemctl stop ht-b300` for a unit. The pid is also the suffix of `run_id` on the latest
  `run ...` line of `run.log`. A stop runs the same drain as a Spot notice.
- **Group signals drain too.** A terminal Ctrl-C or a process-group kill also kills the `tee`
  that writes `run.log`. The drain then logs straight to `run.log`. The unit uses
  `KillMode=mixed` so that systemd signals the script first. `TimeoutStopSec` must exceed
  `HT_B300_STOP_GRACE` plus the upload time.
- **Re-run:** run the same command. A step whose `steps/<name>.rc` is `0 <image> <commit>` for the
  same image and the same clean git commit of the checkout is skipped. A new `--image`, a new
  commit or `--force` re-runs it, and so does a modified tree (untracked files count) or a tree
  without git: code nobody can name is never skipped. A step that starts again first removes its
  own evidence files and its `.rc`, so a step that a stop interrupted has no `.rc` and re-runs.
  A second run on the same results directory is refused (exit 2, `.lock`). `host_probe` always
  re-runs. `--steps` selects a subset, still in the fixed order. Each invocation starts a new
  `--duration` deadline.

## Steps and exit codes

The steps run in this order. The run stops at the first step that fails.

| Step | What it runs | Exit on failure |
| --- | --- | --- |
| `host_probe` | `nvidia-smi`, the `/sys/class/infiniband` listing, `fi_info` if present, `df`, then `b300/host_check.py` | 3 `host-probe-failed` |
| `pull` | `docker pull` of the digest, then a check that `RepoDigests` contains it | 4 `pull-failed` |
| `gpu_probe` | `b300/gpu_probe.py` in the image, `--network none` | 5 `gpu-probe-failed` |
| `mfu` | `torch.distributed.run` on 8 ranks: `run_mfu_dist.py --profile mfu-dense-1b-dp8 --reduction zero2o --act-ckpt --warmup 1` on the default Docker network (torchrun's store uses the container hostname), then `check_gpu_evidence.py --kind mfu` | 6 `mfu-failed` |
| `miner_test` | `scripts/mine_smoke_v2.py` under `timeout ${HT_B300_SMOKE_BUDGET:-900}` | 7 `miner-test-failed` |
| `miner_prod` | only with `--prod`: `run-v2 --round <W>` for each round | 8 `miner-prod-failed` |

Other exit codes: `0` completed; `2` bad arguments, including `--prod` without its variables;
`9` `watchdog-died`; `143` stopped by a signal, `--duration` or a Spot notice. Once the start-up
checks have passed, any other death of the script drains the same way with reason
`unexpected-exit` and its exit status (1 if it had none). Before that, for example when the results
directory cannot be created, the script exits 1 with nothing to drain and no `stop.json`. A run
whose `stop.json` cannot be written exits 1 even if its steps passed. Containers are named `ht-b300-<step>` and labelled `io.cortex.b300-run=<run_id>`.

Inputs are checked as text, in the C locale, before they are used (exit 2): `--duration` is `N`,
`Nm` or `Nh` with at most 6 digits and not zero (leading zeros are decimal); `--host` starts with a
letter or digit and has only letters, digits and `._-`; `HT_B300_POLL_SECONDS` is a plain decimal
from 0.001 to 59.999 (at most 3 decimals, no leading zero); `HT_B300_STOP_GRACE` has at most 4
digits; `HT_B300_EXPECT_GPUS` is 1 to 99; `HT_B300_CONTAINER_PREFIX` is `ht-`, then a lowercase
letter or digit, then lowercase letters, digits and `_.-`; `--results-root` is as described above.

## Results and PASS criteria

Every run writes `run.log` (appended across runs), `steps.tsv` (one row per executed step: name,
rc, seconds, run_id), `steps/<name>.rc` (the latest rc of each step, with its image and commit) and
`stop.json` under `/results/<host>/`. `stop.json` holds `run_id`, `reason`, `rc`, `image`,
`git_rev` and `git_dirty` (the checkout's HEAD and whether the tree was modified; `unknown` and
`null` without git), `steps` (rc per step), `ran` (steps executed by this invocation),
`spot_notice` and `errors` (an rc file, `steps.tsv` or Spot answer that could not be read; a Spot
answer that is not JSON is kept as `{"unparsed": <first 200 characters>}`).
`spot-instance-action.json` is written only after a Spot notice of this run.

- **host_probe** writes `host_probe.json`, `nvidia-smi.csv`, `nvidia-smi.txt`, `efa-devices.txt`,
  `df.txt`, and `fi_info.txt` when `fi_info` exists. PASS: `ok: true` and every `checks` entry is
  true: `gpu_count` (8), `driver` (all 595.91.07), `compute_cap` (all 10.3), `efa` (at least 1
  device), `disk` (at least 100 GiB free). `HT_B300_EXPECT_GPUS`, `HT_B300_EXPECT_DRIVER`,
  `HT_B300_EXPECT_CC`, `HT_B300_MIN_EFA` and `HT_B300_MIN_DISK_GB` override these values.
- **pull** writes `pull.log` and `pull-digests.json`. PASS: the list contains the requested digest.
- **gpu_probe** writes `gpu_probe.json`. PASS: `ok: true`, with `checks` `torch_cu130`,
  `gpu_count`, `capability`, `same_sm_count` and `bitwise_smoke` all true. `bitwise_smoke` means
  that an fp32 matmul, a bf16 matmul, the attention forward and backward pass of the backend that
  `select_backend` picks for the device (`HT_ATTN_BACKEND` included: `math` on sm_103) and one
  kimi-k3-pro-tiny MICRO round (bf16; the three roots are the `kimi_micro_*` digests) gave the same
  digests on every GPU and every repeat. If the probe raises, `gpu_probe.json` holds
  `{"ok": false, "error": ...}`; an earlier run's file is removed before the step. Also read:
  - `arch_list`: expect `["sm_75","sm_80","sm_86","sm_90","sm_100","sm_120"]`. There is no
    `sm_103`; the B300 runs the `sm_100` cubins.
  - `devices[].sm_count`: the B300 SM count, measured as 148 on host B on 2026-10-26. Production
    manifests must pin that value.
  - `devices[].select_backend`: `math` on sm_103, because `sdpa-flash` is qualified only on
    sm_120 (`QUALIFIED_BACKENDS` in `hypertrain/src/hypertrain/trainer/attention.py`).
  - `smoke.errors`: must be empty. An entry such as `no kernel image is available` means the wheel
    cannot run that kernel on sm_103. That is a blocker; report it.
- **mfu** writes `mfu-dp8-flash.json`; per-rank output is teed into `run.log`. The bench forces
  `HT_ATTN_BACKEND=sdpa-flash`. PASS: `check_gpu_evidence.py` prints `VALID`, `replay_match` is
  `true`, `attention_backend` is `sdpa-flash` and `attention_paths` is `["flash"]`. Compare
  `tokens_per_s` between hosts. `mfu` and `vendor.mfu` use the RTX 5090 bf16 peak (209.5e12,
  `MFU_PEAK_RTX5090_BF16` in `hypertrain/src/hypertrain/trainer/perf.py`), so they are not a B300
  MFU. `HT_B300_PEAK_FLOPS=<B300 dense bf16 peak>` adds a `measured` block. The repository
  states no B300 peak; do not invent one.
- **miner_test** writes `mine-smoke-v2.json` and `mine-smoke-v2.stdout`. PASS: `result: PASS`.
  This is the repository's only test challenge: the real challenge app and a TLS relay on
  127.0.0.1 with the test ledger, driven by join-v2 -> probe-v2 -> proof-v2 -> run-v2 ->
  `UPLOADED` through the real CLI on the CPU device. It uses neither the GPUs nor the production
  master. On failure, `failed` and `stages` name the failing stage.
- **miner_prod** writes `miner-prod/round-<W>.json` and `miner-prod/round-<W>-commitments.json`.

## Production rounds (`--prod`)

`--prod` needs three variables. With `systemd-run`, pass them as `--setenv=NAME=VALUE`.

- `HT_B300_PROD_ENDPOINT`: the master challenge URL, e.g. `https://<master>/challenge/hypertrain`.
- `HT_B300_MINER_CONFIG`: the path to `miner.toml`, which must set `device = "cuda"` (the default
  is `cpu`). Its directory is mounted at the same path, so the `keyfile` and `workdir` it names
  must be inside it. The container runs as `--user $(id -u):$(id -g)`, so the keyfile must have
  mode 0600 and belong to the invoking user. That directory must be neither inside nor above the
  results directory (exit 2): the results directory is mounted at `/out` and uploaded.
- `HT_B300_PROD_ROUNDS`: the round numbers the operator opened, separated by spaces.

```bash
HT_B300_PROD_ENDPOINT=https://<master>/challenge/hypertrain HT_B300_MINER_CONFIG=<miner-dir>/miner.toml \
HT_B300_PROD_ROUNDS="<W1> <W2>" nohup ~/cortex/hypertrain/scripts/b300/run_host.sh --prod \
  --upload s3://<bucket>/b300 </dev/null >/dev/null 2>&1 &
```

- **Admission first:** the operator does join-v2 `--prepare`, the coldkey cosign and escrow
  beforehand. No owner key is on the host.
- **PASS:** every `round-<W>.json` is `{"status": "UPLOADED"}`. `UPLOADED` means the upload was
  accepted, not that the audit succeeded or that payment was made.
- **New rounds** after a passed `miner_prod` need `--prod --steps miner_prod --force`. The step
  removes the files of the rounds in `HT_B300_PROD_ROUNDS`, including any it never reaches, before
  it starts; the files of other rounds stay. A round that would start after the deadline fails the
  step.
- **Manifest pins (owner/Principal action):** the signed run manifest must pin
  `reference_spec.sm_count` to the B300 SM count and include 595.91.07 in `driver_allowlist`
  (`miner/core.py` refuses otherwise). `reference_spec.image_digest` must equal the image digest
  (`miner/island_worker.py` refuses otherwise; the script passes it as `HT_IMAGE_DIGEST`).

## Auto-stop

A stop comes from `--duration` (default `3h`; `3h`, `90m` or seconds), from SIGTERM, SIGINT or
SIGHUP to the script's pid (the documented `nohup ... &` start ignores SIGHUP, because of `nohup`,
and SIGINT, because it is a background job of a non-interactive shell: use SIGTERM), or from a Spot
notice: a watchdog polls IMDSv2 `latest/meta-data/spot/instance-action` every
`HT_B300_POLL_SECONDS` (default 5). The script then runs `docker stop --time
${HT_B300_STOP_GRACE:-60}` on this run's containers (label `io.cortex.b300-run=<run_id>`) and
writes `stop.json` with `reason` `duration`, `spot-interruption`, `signal`, `unexpected-exit`,
`watchdog-died` (the watchdog is waited on during `pull` and every container step and checked
before each step; without it neither `--duration` nor a Spot notice could stop the run) or the
`*-failed` reason of a step. With `--upload`, it copies the
evidence files to `s3://<bucket>/<prefix>/<host>/` through `aws s3 cp --recursive` with
`--exclude '*'` and one `--include` per file: `run.log`, `steps.tsv`, `stop.json`,
`spot-instance-action.json`, `steps/*.rc`, the files listed under each step above and
`miner-prod/round-*.json`. The lock, the IMDS scratch file and anything else in the directory stay
on the host. It does this after a completed or a failed run; `aws s3 cp` never deletes, so a file
of an earlier run can stay under the prefix beside this run's `stop.json`: read `stop.json` first.
Local NVMe does not survive Spot termination: use `--upload`, or collect with `scp` or `aws s3`
before you stop.

## Collect and compare the two hosts

```bash
scp -r -i <ssh-key-path> ubuntu@<addrA>:/results/<hostA> ./b300-results/   # same for B
aws s3 sync s3://<bucket>/b300 ./b300-results                               # or this
q='{leaves_root, replay_leaves_root, rank_leaves_roots}'
diff <(jq -S "$q" b300-results/<hostA>/mfu-dp8-flash.json) <(jq -S "$q" b300-results/<hostB>/mfu-dp8-flash.json) && echo SAME
diff <(jq -S .smoke.reference b300-results/<hostA>/gpu_probe.json) <(jq -S .smoke.reference b300-results/<hostB>/gpu_probe.json) && echo SAME
jq '{gpu_name, tokens_per_s}' b300-results/*/mfu-dp8-flash.json
jq '[.devices[].sm_count] | unique' b300-results/*/gpu_probe.json
```

PASS: both `diff` commands print `SAME`. Inside each MFU file, `leaves_root`,
`replay_leaves_root` and every `rank_leaves_roots` entry are equal. Both hosts must use the same
profile, image and flags.

- **Commitments:** `miner-prod/round-<W>-commitments.json` holds `leaves_root`, `delta_hash`,
  `state_root`, `final_theta_hash` and `ef_out_hash`, copied from `.commitments` in
  `<workdir>/<run_id>/<hotkey>/<W>/published/rank-0/summary.json` without `leaves`. In a live
  round each hotkey gets its own assignment, so two miners' roots legitimately differ.
- **Bitwise comparison:** both hosts run the same `job.json` with
  `hypertrain-miner run-v2 --job <job.json>`, using the container flags of `miner_prod`. This
  writes `published/rank-0/summary.json` beside the job. Copy the job directory without
  `published/` (an existing one is returned as it is), run before the job's `deadline`, and
  compare with `jq -S '.commitments|del(.leaves)'`.
- B300 results are a replay domain of their own. Never compare them with RTX 5090 roots.

## Lium validator cycle on the GPUs (D19, 2026-10-26)

The Lium validator (13.223.237.157) opens SSH sessions on both hosts about every 15 minutes (at :08, :23, :38 and :53 on host B, removed about 7 minutes later) and runs a matmul challenge inside the `executor` container. That challenge holds most of the VRAM of GPU 0 for 2 to 5 minutes, and the executor process sometimes keeps a resident 810 MiB. It happens whether or not rentals are paused. Observed on host B on 2026-10-26: two dp8 attempts failed at 14:27:44Z and about 14:30Z with a rank 0 `torch.OutOfMemoryError` (about 265 GiB of the 267.68 GiB held by another process). [V by the AWS agent: executor docker logs, docker events, journalctl ssh]

The executor stays up: stopping it would take the nodes OFFLINE at Lium (SN51 decision D19). So:

- The :08/:23/:38/:53 schedule is NOT reliable (observed on 2026-10-26: a challenge started about 5 minutes after the announced slot). The validator checks can take all 8 GPUs, not only GPU 0 (host A, 15:14Z: 8 foreign `python` processes from the executor container). `watchtower` can also update and recreate `executor-executor-1` without notice (host A, 15:24Z).
- Rule before every run and every step: `nvidia-smi --query-compute-apps=pid,process_name --format=csv`. If a `/root/app/.venv/bin/python` process is listed, wait. A cycle starts at `docker logs executor-executor-1 2>&1 | grep upload_ssh_key` and ends at `remove_ssh_key` (30 s to 8 min on hosts A and B; 4 to 12.5 minutes on a third host, below). A short-lived Lium pod seen on a paused node (44 s, killed by the executor, exit 137) was not a served rental.
- For a run with at most 7 GPUs, leaving GPU 0 out (`CUDA_VISIBLE_DEVICES=1,2,3,4,5,6,7`) helps only against the GPU 0 challenge, not against a full-host check.
- Tolerate an OOM at start-up (any rank) and re-run the step after the cycle ends. Never stop the executor.
- Before each step also record `nvidia-smi --query-gpu=index,memory.used --format=csv`.
- A throughput figure measured with 8 GPUs must say whether a validator cycle overlapped the timed steps (compare the run window with the `upload_ssh_key` and `remove_ssh_key` lines of the executor log). Bitwise results (`leaves_root`, `delta_hash`, `state_root`) do not depend on the cycle: the dp8 qualification already obtained stays valid.

### Gate a step and record the overlap

`hypertrain/scripts/b300/cycle_gate.sh` applies the two checks above and prints one JSON line per
poll. It exits 0 only when all of these hold:

- No compute app is on the selected GPUs (`--gpus 2,3`; default all) except processes whose name
  starts with `--ours` (default `/opt/hypertrain/`). This is a name match, not an owner check: our
  own short-lived processes also show as `[No data]` or a bare `python`, and those count as foreign, so
  another job of ours can block the gate for a moment.
- The executor log (`docker logs -t --since 30m` of `HT_GATE_EXECUTOR`, default
  `executor-executor-1`) shows no open cycle. Over the `POST /upload_ssh_key` and
  `POST /remove_ssh_key` lines with a 2xx status younger than `--cycle-max` (default 900 s, above the
  783 s of the longest cycle observed), each upload opens a cycle and each removal closes one. An
  older unmatched upload is reported as `stale_upload` and does not block.
- The log holds at least one marker. A recreated executor container (watchtower) has an empty log
  while the cycles go on, so with no marker the gate waits; `--allow-no-marker` is for a host without
  a validator.
- `nvidia-smi`, the executor log and the marker times (GNU `date`) could be read. Otherwise the gate
  does not clear.

Exit codes: 0 clear; 1 not clear before `--timeout` (default 1800 s); 2 usage; 4 `nvidia-smi` or the
executor log was unreadable at the last poll. It only reads `nvidia-smi` and the executor's logs.

`hypertrain/scripts/b300/gpu_watch.sh OUT_PREFIX [INTERVAL]` truncates and then appends the compute
apps to `OUT_PREFIX.apps.csv` and the memory used per GPU to `OUT_PREFIX.mem.csv` every `INTERVAL`
seconds (default 2), until it is killed. A failed or hung (10 s) `nvidia-smi` call writes a
`nvidia-smi-failed` row. Start it before a timed step and compare its rows with the executor markers
of the step's window.

Observed on a third host (Hyderabad, 2026-10-26): the executor logged an `upload_ssh_key` every
15 minutes (at about :02, :17, :32 and :47 of the hour, 8 consecutive cycles from 20:02Z to 21:47Z)
and the matching `remove_ssh_key` 4 to 12.5 minutes later; in 2 of the 8 cycles a second upload came
about 7 minutes after the first, before the removals. An upload/removal pair 8 s apart at 21:15
coincides with the pause of the node and is unexplained. In the one cycle watched in detail the
validator's `/root/app/.venv/bin/python` appeared on GPU 0 5 min 38 s after the upload, held
271,534 MiB for about 59 s and was present for at least 65 s. Rows named `[No data]` (about 1 GiB)
appeared on the GPU a step was using; each such pid is also listed there under our own process name,
so they are our short-lived processes seen before the name resolved. From about 22:30Z the uploads
were irregular (22:29:59, 22:38:06, 22:44:07, 22:53:07). These are observations, not a schedule: keep
gating, and keep the check on the whole host (an earlier cycle used all 8 GPUs).

## Local CUDA run-v2 (d20 harness)

`hypertrain/scripts/b300/d20/run_container.sh` runs the repository's two CPU end-to-end scripts
(`scripts/mine_smoke_v2.py`, `scripts/mine_multi_round_v2.py`) with a CUDA miner and CUDA audits on
one GPU of a B300 host:

```text
run_container.sh --image REPO@sha256:<64 hex> --gpu N --out DIR [--mode MODE]
                 [--driver 595.91.07] [--sm-count 148] [--no-gate] [--gate-timeout SECONDS]
```

`MODE` is `smoke` or `multi` (the fixture decoder) or `smoke-kimi`, `multi-kimi`, `job-kimi` (below).
It waits for `cycle_gate.sh` (unless `--no-gate`; exit 3 if the gate does not clear within
`--gate-timeout`), starts `gpu_watch.sh`, then runs one `--rm` container named `ht-s9-<mode>-gpu<N>`
on `--gpus device=N` with the checkout mounted read-only at `/opt/ht`, first on `PYTHONPATH` (and
`pytest==9.1.1` installed from PyPI inside the container). The gate looks at the whole host whatever
GPU you pick; choosing one other than 0 only avoids the GPU the validator's challenge was seen on
first. `--out` must not exist. On SIGINT or SIGTERM only that container is removed and the exit code is
143 (a shell ignores SIGINT in a job it starts with `&`, `nohup` or not, so use SIGTERM there);
otherwise the exit code is the container's. It writes `gate.jsonl`, `gpu-watch.{apps,mem}.csv`,
`stdout.log`, `stderr.log`, `events.jsonl`, `executor-markers.txt`, the script's evidence
(`multi.json`, `mine-smoke-v2.json` or `job-kimi.json`) and `run.json`: the window, the exit code,
`interrupted`, the last gate line, `executor_log_ok`, the last SSH-key marker before the window
(`last_marker_before_window`) and every marker inside it (`executor_markers_in_window`). An empty list
with `executor_log_ok: false` means the log could not be read, not that nothing happened.

Real: the challenge app, the TLS relay, the `hypertrain-miner` processes, admission, the probation
trials, the CUDA training inside `run-v2`, the audits (replayed in the challenge process on the same
GPU), aggregation and `get_weights`. The miner's CUDA worker gates (torch `2.14.0+cu130`,
`HT_IMAGE_DIGEST`, driver allowlist, SM count) apply. Substituted, as `d20/gpu_run_v2.py` states:

- the fixture manifest's `reference_spec` is pinned to the B300 image, driver and SM count, which
  gives a new `run_id`;
- the CUDA qualification record is a fixture-owner-signed **stand-in**, installed before the run is
  created. The real record needs two results from two distinct machines, which one host cannot
  give, so a PASS here does not exercise `_bootstrap_backend_v2`;
- `role-dispatch.json` is written from the challenge process's own GPU and is not a dispatcher's
  record; its image digest is `HT_IMAGE_DIGEST`, the same variable that pins the manifest;
- the subnet transport and the beacon are the pytest network fixture's.

The harness also logs a `device_class` event: the capability and SM count torch reports for the
visible GPUs, and the verdict of the repository's own `unqualified_device` (ADR 0007), `null` when
the pair is in the table. It stops when no GPU is visible or when the verdict is not `null`, but the
verdict does not enter the run's qualification record, which stays the stand-in above.

`--mode smoke-kimi`, `multi-kimi` and `job-kimi` run kimi-k3-pro-tiny (ADR 0006) instead of the fixture
decoder, with its MICRO geometry (73,200 parameters) and `HT_D20_KIMI_DTYPE` (`bf16`, the default, or
`fp32`). `smoke-kimi` and `multi-kimi` are the same chains as `smoke` and `multi`; `job-kimi` is the CLI
test `tests/miner/test_client_network_v2.py::test_kimi_manifest_joins_and_runs_v2_through_the_cli` on
CUDA: `join-v2`, then `run-v2 --job`, and the miner's torchrun worker must equal an in-process CUDA
`train_round` (same `leaves_root`, `final_theta_hash`, `delta_hash` and delta bytes; the router bias
must move). It writes `job-kimi.json` (a failing join or run is recorded as `FAIL`, with the exit
codes). The fixture stages the job with a 120 s deadline. The 77M-parameter PROFILE cannot ride this challenge: the
fixture admission policy caps one object at 1,000,000 bytes and the PROFILE state is about 0.9 GB.
Disputes and bisection on kimi are not exercised.

## Not proven yet

`hypertrain/tests/test_b300_run_host.py` drives the script against fake docker, nvidia-smi, aws
and IMDS; that does not prove a B300 run. Still open: sm_103 execution of the `sm_100` cubins
(step `gpu_probe` settles it), the B300 SM count, and production admission with the B300
manifest pins.
