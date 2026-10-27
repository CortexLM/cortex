# s100 TEST stack: image swap, TEST auditors and a CPU TEST run

The TEST stack on s100 is compose project `hypertrain-test`
(`/root/backups/hypertrain-test/compose.test.yml` and `compose.relay.yml`), with these parts:
- containers `hypertrain-test-challenge`, `-beacon`, `-master-stub` and `-relay`;
- network `hypertrain-test_test`;
- state in `/data/hypertrain-test/`.

`https://relay.joinbase.ai/test-master/*` reaches the TEST challenge (127.0.0.1:18996).
Everything else on that host reaches the TEST relay (127.0.0.1:18995). Nothing here touches
`cortex-challenge-hypertrain`, the master, `hypertrain-beacon`, `hypertrain-auditor-go-*` or
`/data/role-dispatch`.

The tools are in `hypertrain/scripts/ops/s100_test/`. Variables used below:

```bash
T=hypertrain/scripts/ops/s100_test; PY=hypertrain/.venv/bin/python   # run from the repo root
NEW=ghcr.io/cortexlm/cortex/hypertrain@sha256:<attested server digest>
RID=<run id>; TS=$(date -u +%Y%m%dT%H%M%SZ); B=/root/backups/hypertrain-test-s7-$TS
```

The shell tools run on s100 over stdin (`ssh root@s100 'bash -s -- CMD ARGS' < tool.sh`), so
nothing is written loose there. The in-container Python tools take the same path through
`docker exec -i hypertrain-test-challenge python -I - ARGS < tool.py`.

## Never restart `hypertrain-auditor-0/1`

These are the old production auditors:
- they poll `cortex-challenge-hypertrain`;
- they hold the same hotkeys as `hypertrain-auditor-go-0/1`, with seeds in
  `/data/role-dispatch/auditors/auditor-N`.

The TEST auditors are `hypertrain-test-auditor-0/1`. Their disposable seeds are in
`/data/hypertrain-test/auditors/auditor-N`.

## 1. Swap the TEST challenge image (and tune the TEST beacon)

Only the challenge service changes image. The beacon and relay code is the same in the deployed
images and in `e435ee8f`, and the store schema did not change.

Do not change anything while a seal is due. `swap` and `beacon` refuse to run unless all of
these hold:
- master `readyz` answers 200;
- the latest seal is sealed and less than 30 minutes old (the first half of the 72-minute window);
- the latest seal is for the epoch just before the master's current one.

`HT_GATE_MAX_AGE_S` replaces only the 30-minute limit (1..4320 seconds, one epoch), for a change the
founder approved outside the first half of the window. Readyz 200, a sealed seal and the previous
epoch are still required, the script prints the value whenever it is set, and a malformed value
exits 4 before any call: `ssh root@s100 "HT_GATE_MAX_AGE_S=4000 bash -s -- swap $NEW $B" < $T/challenge_image.sh`.

`rollback` and `restore-db` are not gated: they are the way back.

`swap`, `beacon`, `paper`, `rollback`, `executor` and `executor-off` also refuse (exit 4, before any
container or live file is touched) a compose file in which any service has no `container_name` or
one that is not a TEST container (`hypertrain-test-challenge`, `-beacon`, `-master-stub`, `-relay`,
`-auditor-0`, `-auditor-1`). `docker compose up` acts on the names the file declares, so a file
naming `hypertrain-beacon` would otherwise recreate the production twin. `swap` and `beacon` check
the live file and the edited copy; `executor` checks the live file and the live file merged with its
override; `executor-off` checks the live file and leaves the override in place when it refuses;
`paper` and `rollback` check the backup's `compose.test.yml` before it can replace the live one. The
services `up` recreates, `challenge` and `beacon`, must be in that config under exactly
`hypertrain-test-challenge` and `hypertrain-test-beacon`: `compose config` leaves out a service
behind a profile, which `up` would still start, and a TEST name on the wrong service is refused.
`restore-db` runs no compose and is not checked.

```bash
ssh root@s100 "docker pull -q $NEW"
ssh root@s100 "bash -s -- pre" < $T/snap.sh
ssh root@s100 "bash -s -- backup $TS" < $T/challenge_image.sh
ssh root@s100 "bash -s -- paper $B" < $T/challenge_image.sh
ssh root@s100 "bash -s -- swap $NEW $B" < $T/challenge_image.sh
ssh root@s100 "bash -s -- beacon $B" < $T/challenge_image.sh
ssh root@s100 "bash -s -- post" < $T/snap.sh
```

Each change is refused unless the resolved compose config differs in exactly the expected fields:
- `swap`: `services.challenge.image`;
- `beacon`: `services.beacon.entrypoint.9` and `.11`.

`swap` fails unless `/version` answers and the challenge runs the new image ID.

`backup` refuses to finish unless both SQLite copies pass `integrity_check`. It also keeps
`relay.json` and `metagraph.json`.

Every command that takes a backup directory (`swap`, `beacon`, `paper`, `rollback`, `restore-db`)
refuses it with exit 4 unless:
- `compose.test.yml`, `challenge.db`, `server_keys.db` and `SHA256SUMS` exist;
- `sha256sum -c --strict` passes, so a malformed or cut line fails;
- `SHA256SUMS` lists `compose.test.yml`, `challenge.db` and `server_keys.db`, and also
  `compose.relay.yml`, `relay.json`, `metagraph.json` and `inspect-before.json` when they exist
  (`rollback` writes the two JSON files back over live ones and reads `inspect-before.json`), so a
  `SHA256SUMS` cut short cannot hide a corrupt file.

Backups from the first versions of the script list five files and have no `relay.json` or
`metagraph.json`; they still pass. The files `swap` and `beacon` add (`config-*.json`,
`compose.test.yml.before-*`) and `restore-db-aside-*` are never used, so they are not checked.

### Beacon lag

The test ledger refuses joins, trials and leases with 503 "beacon infrastructure paused" when the
beacon lags more than 2 rounds (`store.py` `_snapshot_current_v2`).

On s100 each round costs about 2.4 s against the 3 s drand period. The pusher verifies each round
with py_ecc BLS (about 1.2 s) and the challenge verifies it again on ingest. With the original
`--interval 3 --catch-up 20` the lag held at 8-9 rounds. `beacon` applies the production twin's
`--interval 0.2 --catch-up 3`, which gave 1-2 rounds when idle (measured 2026-10-26).

Under trial load the lag reaches 3 again. Each in-process CPU reference holds the store while the
pusher waits, and the pusher recovers only 0.6 s per 3 s drand period. So expect 503 "beacon
infrastructure paused" for 30-40 s after each reference, and retry.

### Rollback

1. `rollback B` puts back the compose file and recreates the challenge and the beacon. It fails
   unless the challenge runs the old image ID again.
2. When `B` holds `relay.json` or `metagraph.json` that differ from the live ones, `rollback`
   writes them back and restarts `hypertrain-test-relay` or `hypertrain-test-master-stub`.
3. If the run must go too, `restore-db B` restores `challenge.db` and `server_keys.db`. It first
   keeps the live copies in `B/restore-db-aside-*`.
   - It does not restore the ledger journal, child run stores or `trials-v2/`. Use it before
     `get_weights` has answered for the run.
4. Remove the TEST auditors with `test_auditors.sh remove`.

## 2. Publish a CPU TEST run

A cpu-backend TEST run needs no qualification. With no qualification record, the test ledger
freezes the backend to `cpu` when the run is POSTed (`store._backend_v2`).

Never POST a run that should be CUDA before its qualification is bootstrapped. The cpu freeze
cannot be undone for that run_id.

Keys:
- The run must be signed by the TEST owner (`HYPERTRAIN_OWNER_HOTKEY` of the TEST challenge).
- It must name the TEST coordinator (`/data/hypertrain-test/secrets/coord.key`).
- Before the build, put their disposable seeds in `OUT/secrets/owner.seed` and `coord.seed`, and
  the TEST miner coldkeys in `cold-0..N-1.seed`. The builder reuses seeds that already exist.
- The auditor and referee seeds are created fresh.
- The relay k1 SS58 is the `pubkey` of the `relay-joinbase` spec in
  `/data/relay/config/relay.json`.

```bash
HT_D20_GEOMETRY=tiny-cpu HT_D20_COLDKEYS=4 HT_D20_RELAY_PUBKEY=<relay k1 ss58> \
  hypertrain/scripts/b300/d20/build_test_manifest.sh sha256:<gpu image digest> cpu-test 170 $OUT
```

Copy the public outputs to `/data/hypertrain-test/state/<stage>/` (uid 65532):
- `out/manifest-envelope.json`, `out/policy-*.json`, `out/relay-registry.json`;
- `inputs/samples.u32`, `inputs/proofs.json`, `inputs/genesis-allocation.json`.

Then run, in order:

```bash
for s in objects run dataset genesis status; do
  ssh root@s100 "docker exec -i hypertrain-test-challenge python -I - /data/<stage> $s" \
    < $T/publish_run.py
done
```

`status` must show the `execution-backend` record `{"authority_hash":null,"backend":"cpu"}`.

The `genesis` step prints `origins`: one escrow origin per coldkey, in allocation order. Save that
list, alone, as `origins.json`.

A run POSTed while the TEST challenge already hosts one goes to the child store `/data/runs/<run id>/`:
- `publish_run.py` (after `run`), `round_status.py` and `weights_once.py` follow it;
- use `--state-dir /data/runs/<run id>` for `live_operator.py`.

A build that fails or is interrupted removes what it created (seeds you pre-staged stay). When the
campaign is over, clean `$OUT` with `hypertrain/scripts/b300/d20/build_test_manifest.sh --clean $OUT`:
it deletes `secrets`, `inputs`, `out` and `state` under `$OUT`, then `$OUT` if it is empty. It
refuses a relative path, `/`, a symlink (also with a trailing slash) and any directory it cannot
prove is a builder output: `inputs/training.json` with `inputs/public-keys.json`, or a seed in
`secrets/`, and `secrets/` may hold only `*.seed` files and `state/` only `objects/`. The TEST
stack's own `/data/hypertrain-test` is refused (its `secrets/` holds `coord.key`).

## 3. Re-bind the TEST relay to the run

The relay serves one run (`/data/relay/config/relay.json`). `relay_config.py` re-seals its registry
and network manifest for the new run with the TEST coordinator key. It also makes the run's
auditors the observers, and keeps every other field.

The network manifest must be the one the challenge signs relay assignments against:
`base_manifest_hash` is `manifest.training.training_hash()` (`store.relay_assignment_v2`), not the
run_id. Otherwise the relay refuses every upload. `/readyz` does not check this field, so the tool
rebuilds the challenge's manifest and refuses to write if the two differ.

```bash
$PY -I $T/relay_config.py relay.json.current $OUT/out $OUT/secrets/coord.seed relay.json.new
```

Back up the live file and write the new content over it (same inode, mode 0400, uid 65532). Then
run `docker restart hypertrain-test-relay`. `https://relay.joinbase.ai/readyz` must answer 200.

## 4. TEST auditors

Stage each disposable seed without printing it:

```bash
ssh root@s100 'D=/data/hypertrain-test/auditors/auditor-0;
  install -d -o 65532 -g 65532 -m 0700 /data/hypertrain-test/auditors $D; umask 077;
  cat > $D/auditor.seed; chown 65532:65532 $D/auditor.seed' < auditor-0.seed
ssh root@s100 "bash -s -- check $NEW <auditor-0 ss58> <auditor-1 ss58>" < $T/test_auditors.sh
ssh root@s100 "bash -s -- start $NEW $RID" < $T/test_auditors.sh
```

They are contract-2 clients: they lease, ask the challenge to execute the replay (on CPU for a cpu
run), and sign the verdict the challenge computed. Stop with `stop`; remove with `remove`.

## 5. Mine from a remote host

`cpu_miners.py` runs every miner stage through the real `hypertrain-miner` CLI against
`https://relay.joinbase.ai/test-master`.

The miner hotkeys must be in the TEST stub metagraph (`/data/hypertrain-test/config/metagraph.json`,
`{"hotkeys": {ss58: uid}, "netuid": 100}`):
- The four joinbase-3 TEST hotkeys are already in it.
- The stub reads the file once at start. To add a hotkey, back the file up, write the new content
  over it, then run `docker restart hypertrain-test-master-stub`.

**Pin the miner's ATen CPU capability to the challenge host's.** A trial or audit scores only if
the miner's state is bitwise equal to the challenge's own CPU replay.
- s100 is a Zen 3 (AVX2). On a Zen 5 miner, the default AVX-512 kernels changed the optimizer state
  of one trial in six: `PENDING_REPLAY_MISMATCH`, and the identity is suspended.
- The same job matched under `ATEN_CPU_CAPABILITY=avx2`.
- `cpu_parity_probe.py` compares one exported job on both hosts.

`setup` reads `SEEDS_DIR/miner<i>-hot.seed` and `miner<i>-cold.seed`. `miner<i>-cold` must be the
builder's `cold-<i>`: origin i belongs to allocation entry i.

```bash
$PY -I $T/cpu_miners.py setup DIR https://relay.joinbase.ai/test-master $RID \
  <owner ss58> sha256:<gpu image digest> <seeds dir> origins.json 4
ATEN_CPU_CAPABILITY=avx2 $PY -I $T/cpu_miners.py admit DIR "1:12,2:12,3:12,0:4"
```

That gives 3 ACTIVE and 1 PROBATION. Rules that decide how identities are spent:
- **One non-ACTIVE identity per coldkey** (`COLDKEY_PROBATION_EXISTS`). A coldkey funds a second
  hotkey only after its first one is ACTIVE.
- **The first challenge is due 100 rounds (5 min) after the join.** An identity that misses it
  stays APPLIED and blocks its coldkey for good (`admission.py` `challenge`). This is why `admit`
  joins a miner only when its turn starts.

A round opens on a roster of four funded, eligible miners with at most one in PROBATION. The
operator opens it, the miners upload, and the operator settles it. The operator runs
`scripts/ops/live_operator.py` inside the TEST challenge:

```bash
L="docker exec -i hypertrain-test-challenge python -I - --master http://127.0.0.1:8000 \
  --run-id $RID --state-dir /data --coord-key /run/secrets/coord.key \
  --admin-token-file /run/secrets/admin.token"
O=hypertrain/scripts/ops/live_operator.py
ssh root@s100 "$L open --w 0 --hotkey H0 --hotkey H1 --hotkey H2 --hotkey H3 \
  --offsets 3,6,80,82,150,180" < $O     # d_final - d_audit = 98 <= 100
ATEN_CPU_CAPABILITY=avx2 $PY -I $T/cpu_miners.py round DIR 0 0 1 2 3   # one miner at a time
ssh root@s100 "docker exec -i hypertrain-test-challenge python -I - $RID 0" < $T/round_status.py
ssh root@s100 "$L audits --w 0" < $O     # once now >= d_audit
ssh root@s100 "$L finalize --w 0" < $O   # once every audit lease is complete with MATCH (a late one settles too)
ssh root@s100 "docker exec -i hypertrain-test-challenge python -I - $RID" < $T/weights_once.py
```

**Keep `d_final - d_audit <= 100` beacon rounds.** Audit jobs are created with
`created = d_audit` (`store.py:6427`), and each lease gets `expires = min(now + lease_rounds,
d_final)` (`store.py:6586-6589`). `AuditJobV2` refuses `expires > created + 100` when H <= 30
(`messages_v2.py:441-444`, `:659-663`). Audits can only be created once `now >= d_audit`, so with
a wider gap every lease answers 409 "lease exceeds 100 beacon rounds" and the round is never
audited. `live_operator.py open` refuses a gap above `audit_lease_rounds(H, 16.0)` for the
manifest's H (100 rounds when H <= 30) and above 200 rounds for any H.

**Upload one miner at a time.** A relay image built before `BeaconCache` (`relay/app.py`)
BLS-verifies drand in `now()` on every capability and chunk call; later images share one verified
beacon per drand period and put the `RelayError` text in the 400 body as `detail`. Four concurrent
uploads kept the old relay past its own 2-round bound, and every chunk got 400. Each sequential
upload takes about 12 s. Every miner must finish before `d_commit`: `launch_island` checks the job
deadline before it reuses a publication.

Ask `get_weights` once, and only through `weights_once.py`. It refuses (exit 3) until the run has
a payable reward at `epoch_at` and no matured reward still vesting. The first answer per epoch is
final, so an early call freezes that epoch unpaid.

## 6. CUDA TEST run: references and audits on one ssh GPU host

The challenge on s100 has no GPU. With `HYPERTRAIN_CUDA_EXECUTOR` it sends every CUDA reference and
audit replay of ONE run to an ssh-reachable GPU host X and starts `repo@<manifest image digest>`
there with `docker run --rm --gpus ...` (ADR 0008, `hypertrain/docs/cuda-executor.md`). Nothing is
installed on X; X is never a server. First run: S13, 2026-10-27, with the measured numbers in 6.8.

What is **TEST-only** here, exactly:
- `cuda_qualification.py` installs the CUDA backend record with stand-in evidence signed by the
  TEST owner. It replaces the two-host Vast qualification (`_bootstrap_backend_v2`, `compare`) and
  proves nothing about bitwise reproduction on X. It refuses a non-`test` ledger.
- Production still needs the two-host qualification, which stays Vast-shaped.

The paths below use the state volume that is `/data` in the container:

```bash
X=/data/hypertrain-test/state/executor        # /data/executor inside hypertrain-test-challenge
HOST=<ssh host of X>; PORT=22; GUSER=ubuntu; KEY=<path of the private key for X on YOUR machine>
GPU=sha256:<gpu image digest the TEST manifest pins>     # the image digest, not a tag
```

### 6.1 Host files (key by path, nothing printed)

```bash
ssh root@s100 "install -d -o 65532 -g 65532 -m 0700 $X $X/state"
ssh root@s100 "umask 077; cat > $X/id_ed25519; chown 65532:65532 $X/id_ed25519" < $KEY  # stdin: the key is not on s100
ssh root@s100 "ssh-keyscan -p $PORT -t ed25519 $HOST > $X/known_hosts && \
  chown 65532:65532 $X/known_hosts && chmod 0600 $X/known_hosts && ssh-keygen -lf $X/known_hosts"
```

Compare the fingerprint with the provider's console before going on: the file is the only host
key the challenge will accept (no trust on first use). Then the config (use `HOST` exactly as in
`known_hosts`; `docker` is `["sudo","-n","docker"]` on Lium and AWS hosts, `["docker"]` otherwise;
raise `max_clock_skew_seconds` if the probe below says the clocks differ):

```bash
ssh root@s100 "umask 077; cat > $X/cuda-executor.json; chown 65532:65532 $X/cuda-executor.json" <<EOF
{"run_id": "$RID", "image_repository": "ghcr.io/cortexlm/cortex/hypertrain-gpu",
 "state_dir": "/data/executor/state", "max_clock_skew_seconds": 15,
 "hosts": [{"label": "x", "address": "$HOST", "port": $PORT, "user": "$GUSER",
            "identity_file": "/data/executor/id_ed25519",
            "known_hosts_file": "/data/executor/known_hosts",
            "docker": ["sudo", "-n", "docker"]}]}
EOF
```

`RID` is the run_id of the manifest built in 6.3 (it is in `OUT/out/index.json`), so build first and
write this file afterwards; the executor serves CUDA jobs of that run only, every other job still
runs in the challenge. Only the probe in 6.2 can run before: it never reads `run_id`, any 64 hex will do.

### 6.2 Probe X before touching the challenge

A throwaway container of the new server image `$NEW` (it already contains the executor) opens the
same ssh transport, with the same key, `known_hosts` and `sudo -n docker`, and prints what a job
would see: GPUs, the ones a compute app holds, the idle ones, whether the image is on X, the
machine id. It runs nothing on X.

```bash
ssh root@s100 "docker run --rm -i --user 65532:65532 --network hypertrain-test_test \
  -v $X:/data/executor --entrypoint python $NEW -I - /data/executor/cuda-executor.json $GPU" \
  < $T/cuda_executor_check.py
```

Expect `"status": "ok"`, `"driver": "595.91.07"`, `"capability": [10, 3]`, as many idle GPUs as the
manifest's layout (`n_gpus`, 1 for `tiny-cpu`) and `"image_present": true` (the first job pulls
the image by digest if it is absent). `unreachable` carries the ssh error: wrong key, wrong host
key, port, user or `sudo -n docker`. `refused` with "unparseable host probe" usually means the
ssh user's login shell prints to stdout in a non-interactive session (an unguarded `~/.bashrc`):
the result tar travels on stdout too, so fix that on X first. Nothing secret is printed.

### 6.3 Build the CUDA manifest and its TEST backend record

```bash
HT_D20_GEOMETRY=tiny-cpu HT_D20_COLDKEYS=4 HT_D20_RELAY_PUBKEY=<relay k1 ss58> \
  hypertrain/scripts/b300/d20/build_test_manifest.sh $GPU 595.91.07 148 $OUT
$PY -I $T/cuda_qualification.py build $OUT/out $OUT/secrets/owner.seed
```

Use the `tiny-cpu` geometry (824 parameters, `n_gpus=1`, H=1) for the first CUDA run, the model S5
already ran on a B300: its publication is a few kilobytes, so a reference or audit fits in its lease
whatever the link to X. The `dense-1b` geometry (889M parameters, `n_gpus=2`) returns every
checkpoint, final state and error-feedback tensor of both ranks through ssh: by arithmetic from the
tensor sizes about 60 GB per job [H, not measured], which a 100-round (5 min) audit lease cannot
carry over an ordinary link. Do not start there.

Copy the public outputs to `/data/hypertrain-test/state/<stage>/` as in section 2 and add
`qualification-test.json` (it holds the stand-in evidence and the owner's Receipt, no secret).

### 6.4 Point the TEST challenge at X

Back up first (section 1: `snap.sh pre`, `challenge_image.sh backup $TS`). The new variable goes
in through a compose override, gated like the image swap (master `readyz`, a young sealed seal,
the TEST container names, and the resolved compose config must differ only in that variable):

```bash
ssh root@s100 "bash -s -- executor $B /data/executor/cuda-executor.json" < $T/challenge_image.sh
```

The challenge must run an image that contains the executor (`$NEW`, section 1). Setting it together with
`HYPERTRAIN_ROLE_DISPATCH` stops the challenge at start. A later
`swap` or `rollback` uses `compose.test.yml` alone and drops the variable: rerun `executor`. Undo
with `bash -s -- executor-off $B`.

### 6.5 Publish the run with the backend record in place, then run it

```bash
ssh root@s100 "docker exec -i hypertrain-test-challenge python -I - install /data/<stage>" \
  < $T/cuda_qualification.py
for s in objects run dataset genesis status; do
  ssh root@s100 "docker exec -i hypertrain-test-challenge python -I - /data/<stage> $s" \
    < $T/publish_run.py
done
```

`install` must print `"test_only": true` BEFORE `run`; `status` must then show the
`execution-backend` record `{"backend": "cuda", "authority_hash": "<64 hex>"}`. A `cpu` record
means the run was POSTed first: it is frozen, build a new manifest.

Then re-bind the relay (section 3) and start the TEST auditors on `$RID` (section 4). Auditors
lease and ask the challenge to execute; the challenge sends the replay to X. Miners for a CUDA run
run on a GPU host in a `--rm` container of the GPU image, with `cpu_miners.py` in CUDA mode (6.7).

### 6.6 What to look at

- `status` of `round_status.py`, as in section 5, and the audit verdicts `MATCH`.
- `/data/hypertrain-test/state/executor/state/journal.jsonl`: `exec_attempt`, `exec_published`,
  `exec_host_unavailable`, `exec_host_refused`, `exec_attempt_failed` (no secret in it).
- `logs/<container name>.stderr` in the same directory: the island's stderr, 0600.
- The `audit-dispatch` record of each audit job (`records_v2`): the full `role-dispatch.json` with
  host label, `machine_id`, image reference and `RepoDigests`, driver, capability, SM count, GPU
  UUIDs, tar and publication hashes.
- On X: `docker ps` shows `ht-exec-*` only while a job runs; `docker ps -a` shows none after.

Failure behaviour: a lost host is retried (other hosts first, then the same one after a pause), a
host whose driver, capability, SM count, image digest or GPU set contradicts the manifest is
quarantined until the challenge restarts, a deadline or a cancelled lease is never retried, and a
partial or tampered result is never published. With one host, one failure costs one attempt of
`max_attempts` (3).

### 6.7 Miners on the GPU host

`cpu_miners.py` with `HT_MINER_DEVICE=cuda` drives the same real miner CLI for a CUDA run, from the GPU
image's own python (`/opt/hypertrain/venv/bin/python`, no checkout). In that mode it does not need the
ATen pin (the reference runs on a GPU host), it needs `HT_IMAGE_DIGEST` (the manifest's image digest,
which `island_worker.py` compares), `setup` writes `device = "cuda"`, a `probe-v2` or `run-v2` call
first waits up to 240 s for the GPU to hold under 2 GiB (a Lium validator cycle holds about 265 GiB),
and a 500 from the challenge (its remote reference failed) is retried like a 503.

```bash
sudo -n docker run --rm --init --gpus device=5 --ipc=host --ulimit memlock=-1 --ulimit stack=67108864 \
  --user "$(id -u):$(id -g)" -e HOME=/tmp -e HT_IMAGE_DIGEST=$GPU -e HT_MINER_DEVICE=cuda \
  -v $D/scripts:/s:ro -v $D:$D -w $D --entrypoint /opt/hypertrain/venv/bin/python \
  ghcr.io/cortexlm/cortex/hypertrain-gpu@$GPU -I /s/cpu_miners.py admit $D/mine "1:12,2:12"
```

`setup` takes no `--gpus`; `$D` holds `scripts/cpu_miners.py`, `seeds/` and the miner directories. Two
drivers on different GPUs with disjoint plans overlap one miner's probe with the other's reference, but
the challenge computes references one at a time under its store lock. Gate the node with
`hypertrain/scripts/b300/cycle_gate.sh --gpus 5` before each driver.

### 6.8 Measured on 2026-10-27 (S13, one executor host, tiny-cpu)

Node A (8x B300, `sudo -n docker`) ran the references; the miners ran on node B in the GPU image. 28
trials of 3 miners were `FULL_REFERENCE_MATCH` (0 mismatch): the miner's GPU result on B equals the
executor's on A bit for bit, and one miner reached ACTIVE (`GRADUATED_FULL_REPLAY`). The run did not
reach a round: no `run-v2`, no audit, no `get_weights`.

| Step | Seconds |
| --- | --- |
| `probe-v2` on the miner GPU, fresh container / same container | 60-80 / 15-19 |
| remote reference, fresh `--rm` container on A | 57-64 |
| same, with the CUDA compute cache kept between containers | 13 (first one 43) |
| one trial, probe to accepted proof, one driver | about 125 without the cache, 70-90 with it |
| two drivers, with the cache | 40-50 per trial |

- **Cold CUDA start.** Each fresh container pays 25-45 s of CPU for the first use of the CUDA kernels
  (on A: first fp32 matmul 6 s, bf16 matmul 10 s, AdamW 5 s; the second process of the same container
  takes 0.7-2 s). The cache lives in `/root/.nv` of the container. S13 kept it with a wrapper in the
  `docker` field of `cuda-executor.json` that adds only `-v <host dir>:/root/.nv` to `docker run`; the
  config is read once, so a change needs `docker restart hypertrain-test-challenge` between two trials.
  The executor has no option for it yet.
- **The reference holds the store lock**, so the beacon pusher times out meanwhile and every trial
  waits for it to catch up (503 "beacon infrastructure paused" for 10-70 s). A second driver or a
  second executor host does not shorten the references; two drivers only overlap the miner side.
- **Do not push beacon rounds out of order.** The challenge reads exact rounds (a trial's seed round,
  `d_audit`). A second pusher with `--catch-up 1` moved `known` past rounds the first one would have
  filled; `GET /join/<id>/challenge` then answered 500 `BeaconUnavailable: round N not pushed` and
  that trial could not continue. Backfilling the gap through `/v1/admin/beacon` took 2 minutes of
  store lock, a pause of 5 minutes, and two trials passed their 100-round window: the identity is
  SUSPENDED `CHALLENGE_EXPIRED` at its next challenge request, and `resume` resets `clean_count` to 0.
- **A validator cycle on A** shows as `exec_host_unavailable` ("0 idle GPUs, 1 needed") and the
  executor waits (`sweeps`); it cost one trial 40 s.

Plan 40 trials (3 ACTIVE x 12 + 1 PROBATION x 4) at 40-50 s each with the cache, so about 30 minutes,
then the round (about 6 minutes). Without the cache, plan 85 minutes.
