# Miner guide

A hypertrain miner is one island: one hotkey, one escrow account, training the shared model on assigned data from a public start state, then committing and uploading a compressed delta. Blocks tagged `doctest` are run by `bash scripts/doctest_docs.sh` from the tree root. Blocks tagged `doctest-skip:prod` need a live run and are only listed.

Contents:

1. [Hardware](#1-hardware)
2. [Install and configure](#2-install-and-configure)
3. [Admission, probation and bond](#3-admission-probation-and-bond)
4. [Determinism checklist](#4-determinism-checklist)
5. [Joining from any datacenter](#5-joining-from-any-datacenter)
6. [What gets audited and how faults are paid](#6-what-gets-audited-and-how-faults-are-paid)
7. [Troubleshooting](#7-troubleshooting)

## 1. Hardware

- GPU: what the run's `reference_spec` says. The reference is an RTX 5090 (170 SMs). The client checks the SM count and refuses to start on anything else, and checks the driver too when the manifest has a `driver_allowlist`.
- One GPU per miner process today. The client accepts only layouts with `n_gpus == 1`; multi-GPU islands are not wired yet.
- Disk for the dataset shards (about 770 MB for the current dataset) plus your work dir.
- A stable outbound HTTPS path to the challenge server and to at least one drand relay. No inbound ports are needed.

Why the hardware is pinned: auditors replay your segments bit for bit on the reference hardware and image. Phase A (todo 12) showed this works across hosts: 6 honest runs on 3 RTX 5090 hosts with drivers 580.105.08, 580.178.04 and 595.84 produced identical leaves, deltas and final states, and the negative control diverged at leaf 11.

## 2. Install and configure

Use the GPU image (`ghcr.io/cortexlm/hypertrain-gpu`, entry point `hypertrain-miner`), pinned by digest to the run's `reference_spec.image_digest`. From a checkout, the same CLI is:

```bash doctest
uv run --frozen hypertrain-miner --help
uv run --frozen hypertrain-miner run --help | grep -q -- '--rounds'
uv run --frozen hypertrain-miner join --help | grep -q -- '--config'
```

Get the dataset and check it against the run's Merkle root. A contract-2 `run-v2` job needs no local shards: the service serves each job's `samples` and `sample_proofs` objects, and the island checks every row against the signed `merkle_root`. For a u16 `run`, download `manifest.json` from the operator's base URL into `data_dir`. Save shard `i` as `shard-{i:05d}.u16`; its URL key is `shard_uri_template` with `manifest.shard_sha256s[i]`. Then verify (this covers u16 sets only):

```bash doctest
uv run --frozen python -m hypertrain.datasets verify --help >/dev/null
```

```bash doctest-skip:prod
uv run --frozen python -m hypertrain.datasets verify data
```

It prints `OK`, or one line per bad shard and exits 1. If `data_dir` holds no shards, `run` fetches each unit it needs by HTTP Range from `HYPERTRAIN_DATA_BASES` through `hypertrain.datasets.fetch.prefetch`, and checks each unit against the signed unit hashes.

The config is a TOML file. Every key can be overridden with `HYPERTRAIN_MINER_<KEY>` in the environment, and unknown keys are rejected. Relative paths resolve against the config file's directory.

| Key | Required | Meaning |
| --- | --- | --- |
| `api` | yes | challenge base URL, for example `https://<master>/challenge/hypertrain` |
| `keyfile` | yes | hotkey seed, 32 raw bytes or 64 hex chars, mode 0600, owned by you |
| `workdir` | yes | journal, states and deltas |
| `state_source` | yes | where round start states come from (a dir or an https base), checked by hash |
| `image_digest` | yes | the image you run, `sha256:...` |
| `data_dir` | yes | the verified shards |
| `run_id` | no | defaults to the server's run |
| `owner_hotkey` | no | pin the run owner's ss58 |
| `device` | no | `cpu` (default) or `cuda` |
| `poll_seconds` | no | default 2.0 |
| `chunk_bytes`, `allow_file_upload` | no | upload tuning |

The keyfile check is strict. A key that other users can read is refused with exit code 2:

```bash doctest
d=$(mktemp -d)
(umask 022; openssl rand -hex 32 >"$d/miner.key")
cat >"$d/miner.toml" <<EOF
api = "http://127.0.0.1:9"
keyfile = "miner.key"
workdir = "work"
state_source = "states"
data_dir = "data"
image_digest = "sha256:$(printf '1%.0s' $(seq 64))"
EOF
rc=0; uv run --frozen hypertrain-miner join --config "$d/miner.toml" 2>"$d/err" || rc=$?
grep -q 'keyfile must be mode 0600' "$d/err"
rm -rf "$d"
test "$rc" -eq 2
```

## 3. Admission, probation and bond

Contract v1 has no public join route. Run `join`; it checks your hardware against the manifest and prints a request signed by your hotkey:

```bash doctest-skip:prod
uv run --frozen hypertrain-miner join --config miner.toml
```

Send that JSON to the operator. They check it and admit you with a roster row (`PUT /v1/admin/runs/{id}/roster/{hotkey}`), setting your `cluster` and `region`.

New miners start on probation: every round you train is audited before your delta is applied (q = 1). Probation lasts `probation_rounds` rounds (10 in the example manifest, `ceil(1/q)` for q = 0.1) and continues after that until your bond is set. The bond is your escrow: rewards you earned but that haven't vested yet. The operator sets `bond` once your escrow reaches S_min (`s_min_reward_multiple` times the median round reward, 10 by default). After that you're audited at the base rate `q_base` (0.1 in the example). The `outlier` or `copy` flags, or a blacklisted member in your cluster, put you back to q = 1.

Then train:

```bash doctest-skip:prod
uv run --frozen --extra trainer hypertrain-miner run --config miner.toml --rounds 100
```

`run` returns 0 only when every round ends `UPLOADED`, 1 otherwise, and 2 on a config, key, hardware or protocol error. Its journal in `workdir` lets you restart it; a round already committed isn't sent twice.

The whole flow, against a real local server with a fixture beacon, runs in about a minute: two rounds trained and uploaded, then a resend of round 0 with a fresh journal that the server refuses with 409.

```bash doctest
uv run --frozen --extra trainer python tests/miner/qa_cli.py 2>/dev/null \
  | grep '^{"happy"' | tail -n 1 | jq -e '.happy and .duplicate_commit_409'
rm -rf .qa-miner
```

The script prints its verdict as a JSON line and exits 0 only when both checks pass.

## 4. Determinism checklist

Replay is bitwise, so any of these will turn an honest run into a MISMATCH:

- Import `hypertrain.trainer` before anything imports torch. It sets `CUBLAS_WORKSPACE_CONFIG=:4096:8`, deterministic algorithms with no warn-only fallback, cuDNN deterministic and no benchmark, TF32 off for matmul and cuDNN, no reduced-precision BF16 reductions, and one CPU thread. The image also sets `CUDA_DISABLE_PTX_JIT=1`.
- Run the pinned image (`reference_spec.image_digest`), not your own build.
- Same GPU model and SM count as the reference; driver in the allowlist when there is one.
- Verified shards (`python -m hypertrain.datasets verify <data_dir>` must print `OK`).
- Don't change batch size, accumulation, threads or anything else the manifest fixes.
- Every manifest pins its process environment (`reference_spec.env.pinned`, protocol.md section 7, item 4a): a CPU class (`avx2` or `avx512`) and a GPU class (`none`, `sm_103` or `sm_120`). Check this host before you join. `python -m hypertrain.trainer.env_probe --manifest manifest.json` (a v1 or v2 manifest, or a training body) prints `{"honorable": true, "cpu": "avx2", "gpu": "none", "cpu_capability": "AVX2", "env": {...}, "unset": [...]}`: the variables to export and to leave unset. It exits 1 with the reason when a process started that way is refused, for example `ATen CPU capability: manifest 'AVX2', live 'DEFAULT'` on a CPU without AVX2, or `GPU class: manifest 'sm_103', live 'none'` when no such device is visible. The island workers you start through the launcher get the pin whatever your own environment is, and a host that cannot honor it is refused before capacity is charged (`EnvPinRefused`). `join` and the trial probe tell you the same, before any request, with `this host cannot honor the manifest's environment pin: ...`. A process that runs training code in-process (the v1 miner and auditor, the honeypot) must be started with exactly that environment and refuses otherwise.

The server compares the environment you declare with the manifest's pin and refuses a difference before it spends anything, at `join` and at the trial, with 409 `ENV_PIN_DIVERGES: NAME, NAME`. Nothing is consumed: no admission, strike or suspension is recorded and the trial stays open until its deadline, so fix the host and post the same proof again.

| 409 detail | Meaning | What to do |
| --- | --- | --- |
| `ENV_PIN_DIVERGES: MKL_CBWR` (or any pinned variable) | the process that ran your round saw another value | start the miner under the variables `env_probe --manifest` prints |
| `ENV_PIN_DIVERGES: cpu_capability` | ATen resolved another instruction set than the manifest's CPU class | run on a CPU that reaches the class, or on another run |
| `ENV_PIN_DIVERGES: gpu` | your device is not the manifest's GPU class (`none` means no GPU) | use the device class the manifest names |

Check the pins in your environment:

```bash doctest
uv run --frozen --extra trainer python -c '
import hypertrain.trainer as t
d = t.DETERMINISM
assert d["CUBLAS_WORKSPACE_CONFIG"] == ":4096:8" and d["deterministic_algorithms"]
assert not d["matmul_allow_tf32"] and not d["cudnn_allow_tf32"] and not d["torch_preloaded"]
print(d)'
```

## 5. Joining from any datacenter

Miners can be in any datacenter or country. Everything is pull-based over HTTPS: the round schedule comes from the drand beacon (public, verifiable), the start state from `state_source` checked against its hash, and you post your commit and delta. Deadlines are drand rounds (3 s each), so clock skew doesn't matter, but latency does: you must commit before `d_commit` and upload before `d_upload` (300 drand rounds, about 15 minutes after the commit deadline, in the example manifest). Pick a `state_source` and drand relay near you. Tell the operator your real region and cluster; miners that share hardware, network or payout should share a cluster, since hiding that is treated as collusion.

## 6. What gets audited and how faults are paid

Each round, after you commit a Merkle root of your training leaves, the beacon picks which miners and which segments get replayed. It's chosen after your commit, so you can't guess it. You're picked with probability q (1 on probation), and the last transition is always among the checked segments. An auditor replays them from the public start state and compares bits.

| Outcome | What happens |
| --- | --- |
| MATCH or not sampled, and uploaded | round reward is credited to escrow and vests `E_vest_rounds` rounds after the round's final round (10 in the example), paid FIFO |
| NO_UPLOAD (committed, no valid upload in time) | round reward forfeited, escrow kept |
| TRANSIENT (your own rerun reproduces the auditor's bits) | round reward burned, escrow kept; forgiven `forgive_per_epoch` (1) times per epoch, the next one is a fault |
| FAULT (MISMATCH, withheld data, bad proof, assignment violation) | round reward and all unvested escrow burn, the hotkey is blacklisted, your cluster goes to q = 1 |

Burned mass is never given to other miners or the operator, so nobody gains from framing you. After a MISMATCH you can submit your own rerun (`POST /v1/runs/{id}/rerun`) before finalize; if you dispute, the dispute is bisected by step and settled by an auditor other than the first. If the auditor was wrong you're restored, and a round already finalized as NO_UPLOAD is credited back. Paid emission can't be taken back; your exposure is your unvested escrow.

## Seedless install and auto-update

The coldkey (mnemonic) stays on the operator's machine. A GPU host only ever holds a server key plus a coldkey-signed grant (scope `mine`: commit, upload, heartbeat).

On the operator machine:

```bash
hypertrain-miner key create --coldkey ~/cold.key --server vast-1 --api "$API" --out ./bundle-vast-1
hypertrain-miner key list --coldkey-ss58 "$COLDKEY_SS58" --api "$API"
hypertrain-miner key revoke KEY_ID --coldkey ~/cold.key --api "$API"
```

Copy only the bundle (`server.key` + `grant.json`, mode 0600) to the host. On the GPU host:

```bash
bash scripts/install-miner.sh "$MANIFEST_URL" ./bundle-vast-1   # idempotent, never prompts for a seed
hypertrain-miner install --bundle ./bundle-vast-1 --config miner.toml  # refuses coldkey/mnemonic files
hypertrain-miner register --config miner.toml   # posts the grant; no coldkey needed
hypertrain-miner run --config miner.toml --rounds 100 --auto-update --manifest-url "$MANIFEST_URL"
hypertrain-miner status --config miner.toml
hypertrain-miner update --check --manifest-url "$MANIFEST_URL"
```

`register` (no `--coldkey`) posts the server-key grant (`grant.json` next to the keyfile) and submits no contract-2 admission. `register --coldkey COLDFILE --request-id ID --expiry ROUND` posts no grant; instead it signs a join v2 with the configured keyfile (the server key) and the coldkey and posts it to `/v2/runs/<run_id>/join`, so it needs the coldkey file and belongs on the operator machine. For contract 2, prepare the hotkey-signed request on the GPU host using its configured hotkey (not a delegated server key):

```bash
hypertrain-miner join-v2 --config miner.toml --request-id "$REQUEST_ID" --expiry "$EXPIRY" \
  --coldkey-ss58 "$COLDKEY_SS58" --prepare join.json
```

Copy `join.json` to the operator machine. It contains only public fields and the hotkey signature; no coldkey is needed on the GPU host. Co-sign and submit without `miner.toml`, then lock collateral from explicit existing origins:

```bash
hypertrain-miner join-v2-cosign --request join.json --coldkey ~/cold.key --api "$API"
hypertrain-miner escrow-lock --api "$API" --run-id "$RUN_ID" --coldkey ~/cold.key \
  --hotkey-ss58 "$HOTKEY_SS58" --units "$UNITS" --origin "$ORIGIN_ID"
```

`REQUEST_ID`, `RUN_ID` and origin IDs are 64 lowercase hex characters; `EXPIRY` is a beacon round. Inspect the public request before co-signing. Use a new output filename for each preparation. Repeat `--origin` for multiple origins; the CLI never creates origins. `escrow-lock --dry-run` prints only the unsigned operation without posting. Its default expiry is the run's current beacon round plus 1000; `--expiry` overrides it. Identical inputs (including reordered origins) reuse the operation ID. Success reports the event hash, current locked units, total required units and server eligibility; funding alone does not guarantee eligibility. A new lock is refused with 409 `LOCK_BELOW_MINIMUM` when it holds less than a tenth of the run's funding floor (`S_min_units`), with `OPEN_LOCK_CAP` when the coldkey already holds 2,048 open locks, and with `LEDGER_LOCK_CAP` when the ledger holds 4,096; a retry of a lock that was accepted is never refused for any of them. The existing `join-v2 --coldkey ...` path remains available. A revoked key is rejected on every authenticated miner route.

Only sr25519-signed release manifests are accepted; unsigned tags and downgrades are rejected. During the announce window (at least 600 s) `run --auto-update` stages the new wheel while training continues. At the first round boundary at or after `changeover_at`, it swaps `~/.hypertrain/miner/current` atomically and re-executes the new version, which resumes from the round journal. A miner still on the old version 60 s after changeover refuses new rounds. `update --apply` does the stage and swap by hand. `HYPERTRAIN_UPDATE_ROOT` and `HYPERTRAIN_UPDATE_MANIFEST_URL` override the defaults.

## 7. Troubleshooting

| Symptom | Cause and fix |
| --- | --- |
| `KeyfileError: ... must be mode 0600` | `chmod 600` the keyfile and own it as the miner user |
| `FileNotFoundError` traceback on start | the keyfile or config path doesn't exist (paths are relative to the config file) |
| traceback on a bad number in the config | a non-numeric `poll_seconds` or similar isn't caught nicely yet; fix the value |
| `unknown config keys [...]` | typo in the TOML; only the keys above are allowed |
| `HardwareMismatch: SM count ... != reference 170` | wrong GPU model for this run |
| `driver ... not in the manifest allowlist` | install an allowed driver |
| `this client trains single-GPU layouts only` | the run uses a multi-GPU layout, not supported by this client yet |
| `local shards are not the run's dataset` or `shard verification failed` | re-fetch the data and run `python -m hypertrain.datasets verify <data_dir>` |
| 409 on commit | that round is already committed; keep the journal in `workdir` between restarts |
| status `EXCLUDED` on commit | committed after `d_audit`; move closer to the server or start earlier |
| MISMATCH on a run you believe is honest | check section 4, then submit your rerun before finalize |
