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

- `python3`, `curl`, `flock`, `nvidia-smi`, and Docker with the nvidia runtime. The script uses
  `docker` when `docker info` works, otherwise `sudo -n docker`. On the AWS hosts `ubuntu` is not
  in the docker group, so it takes the `sudo -n` path.
- The hosts also run Lium `executor-*` containers. Never touch them. The script only removes or
  stops containers named `<prefix>-<step>` (`HT_B300_CONTAINER_PREFIX`, default `ht-b300`) that
  carry its own run label. Start GPU steps only after the AWS agent reports the host free.
- The monorepo at branch `v3-b300-run` (public repository):
  `git clone --depth 1 --branch v3-b300-run https://github.com/CortexLM/cortex.git ~/cortex`.
  Its `hypertrain/` directory is mounted read-only into every container.
- A results root (default `/results`) writable by the invoking user. Put it on the NVMe RAID:
  `sudo install -d -o "$USER" /mnt/nvme/results && sudo ln -sfn /mnt/nvme/results /results`.
  It and the Docker root each need 100 GiB free.
- For `--upload s3://<bucket>/<prefix>` only: the `aws` CLI and an instance role that can write
  to that prefix. A `systemd-run` unit gets the default system PATH, which lacks `/snap/bin`;
  without `aws` on it the log says `upload skipped: aws CLI not found`.
- Outbound HTTPS to ghcr.io and to PyPI (`miner_test` installs `pytest==9.1.1`).

The default image
`ghcr.io/cortexlm/cortex/hypertrain-gpu@sha256:f4b2885f2dc89478da645b9f5e6f02891cd3309ef0a1f2aa0de04328e11cba0f`
is public: an anonymous `docker pull` works without `docker login`.

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
- **Re-run:** run the same command. A step whose `steps/<name>.rc` is `0 <image>` for the same
  image is skipped; a new `--image` or `--force` re-runs it. A second run on the same results
  directory is refused (exit 2, `.lock`). `host_probe` always re-runs, and so does a step that a
  stop interrupted. `--steps` selects a subset, still in the fixed order. Each invocation starts a
  new `--duration` deadline.

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
`143` stopped by a signal, `--duration` or a Spot notice. Containers are named `ht-b300-<step>`
and labelled `io.cortex.b300-run=<run_id>`.

## Results and PASS criteria

Every run writes `run.log` (appended across runs), `steps.tsv` (one row per executed step: name,
rc, seconds, run_id), `steps/<name>.rc` (the latest rc of each step and its image) and `stop.json` under
`/results/<host>/`. `stop.json` holds `run_id`, `reason`, `rc`, `image`, `steps` (rc per step),
`ran` (steps executed by this invocation) and `spot_notice`. `spot-instance-action.json` is
written only after a Spot notice.

- **host_probe** writes `host_probe.json`, `nvidia-smi.csv`, `nvidia-smi.txt`, `efa-devices.txt`,
  `df.txt`, and `fi_info.txt` when `fi_info` exists. PASS: `ok: true` and every `checks` entry is
  true: `gpu_count` (8), `driver` (all 595.91.07), `compute_cap` (all 10.3), `efa` (at least 1
  device), `disk` (at least 100 GiB free). `HT_B300_EXPECT_GPUS`, `HT_B300_EXPECT_DRIVER`,
  `HT_B300_EXPECT_CC`, `HT_B300_MIN_EFA` and `HT_B300_MIN_DISK_GB` override these values.
- **pull** writes `pull.log` and `pull-digests.json`. PASS: the list contains the requested digest.
- **gpu_probe** writes `gpu_probe.json`. PASS: `ok: true`, with `checks` `torch_cu130`,
  `gpu_count`, `capability`, `same_sm_count` and `bitwise_smoke` all true. `bitwise_smoke` means
  that an fp32 matmul, a bf16 matmul and a flash-SDPA forward and backward pass gave the same
  digests on every GPU and every repeat. Also read:
  - `arch_list`: expect `["sm_75","sm_80","sm_86","sm_90","sm_100","sm_120"]`. There is no
    `sm_103`; the B300 runs the `sm_100` cubins.
  - `devices[].sm_count`: the B300 SM count. The repository does not state it; report it.
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
  mode 0600 and belong to the invoking user.
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
- **New rounds** after a passed `miner_prod` need `--prod --steps miner_prod --force`. A round that
  would start after the deadline fails the step.
- **Manifest pins (owner/Principal action):** the signed run manifest must pin
  `reference_spec.sm_count` to the B300 SM count and include 595.91.07 in `driver_allowlist`
  (`miner/core.py` refuses otherwise). `reference_spec.image_digest` must equal the image digest
  (`miner/island_worker.py` refuses otherwise; the script passes it as `HT_IMAGE_DIGEST`).

## Auto-stop

A stop comes from `--duration` (default `3h`; `3h`, `90m` or seconds), from SIGTERM or SIGINT to
the script's pid, or from a Spot notice: a watchdog polls IMDSv2
`latest/meta-data/spot/instance-action` every `HT_B300_POLL_SECONDS` (default 5). The script then
runs `docker stop --time ${HT_B300_STOP_GRACE:-60}` on this run's containers (label
`io.cortex.b300-run`) and writes `stop.json` with `reason` `duration`, `spot-interruption` or
`signal`. With `--upload`, it copies the results to `s3://<bucket>/<prefix>/<host>/` through
`aws s3 cp --recursive`; it also does this after a completed or a failed run. Local NVMe does not
survive Spot termination: use `--upload`, or collect with `scp` or `aws s3` before you stop.

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

## Not proven yet

`hypertrain/tests/test_b300_run_host.py` drives the script against fake docker, nvidia-smi, aws
and IMDS; that does not prove a B300 run. Still open: sm_103 execution of the `sm_100` cubins
(step `gpu_probe` settles it), the B300 SM count, and production admission with the B300
manifest pins.
