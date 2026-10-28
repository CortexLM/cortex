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
- `beacon`: some of `services.beacon.entrypoint.9`, `.11` and `.13` (interval, catch-up, failure budget), and then exactly the cadence below.

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

On s100 each round costs about 2.4 s against the 3 s drand period (S7 measurement [A]): the pusher
verifies each round with py_ecc BLS (about 1.2 s) and the challenge verifies it again on ingest.
With the original `--interval 3 --catch-up 20` the lag held at 8-9 rounds; with `--interval 0.2` it
is 1-2 rounds when idle (measured 2026-10-26).

**A store lock stops the beacon.** A remote or in-process reference holds the lock of the store that
hosts the run for its whole duration (13-19 s with the CUDA cache, 43-64 s without). The first run
of a state directory lives in the root store, where `GET /v1/beacon/latest` and
`POST /v1/admin/beacon` both wait for that lock. A run created while the root already hosts one
(S13's) lives in a child store with a lock of its own: the root still answers `latest` and keeps the
round a POST brings, but the POST then waits for the child, so the pusher times out all the same and
the child, whose table the gate and the seeds read, gets no round. The pusher gets 10 s timeouts (a
failure each), and when the lock ends it refills the missing rounds oldest first at one round per
2.4 s while the clock adds one per 3 s. Two things can go wrong, and the cadence decides which:

- `--catch-up N` pushes at most the last N rounds after the highest one the service holds. Rounds
  older than that are skipped for good, because the service reports only its highest round. The
  challenge reads exact rounds: after a trial is finalized in round `n` (and after a join or a
  resume), the identity's seed is round `n + 1`. A seed in a skipped round answers 500
  `BeaconUnavailable: round N not pushed` until the window is over (100 rounds), then 409
  `CHALLENGE_EXPIRED`; the identity stays in its state and gets no new seed. Pushing that one round
  before the window closes recovers it; a range does not (S13's backfill of 36 rounds held the lock
  for two minutes). This is code reading by a reviewer, not a run.
- `--max-failures N` consecutive failures end the pusher (the container restarts, and the count
  restarts from zero). A lock costs one failure per 11.4 s (10 s timeout plus the relay fetch); a
  refused connection (the challenge restarting) one per 1.4 s.

`beacon` therefore sets `--interval 0.2 --catch-up 60 --max-failures 100`. Sizing, from the model of
the real pusher: the catch-up must cover `lock / 3 + 10` rounds (60 covers a lock of 150 s; the old
`3` skipped rounds after any lock over about 9 s, 20 after one over about 50 s), and the budget must
cover `lock / 11.4` failures for a lock and `outage / 1.4` for a refused connection (100: 140 s). The
script takes the stock cadence and the one the previous version left on this stack (`0.2 / 3`), raises
a budget below 100 and keeps a higher one, and refuses (exit 5, nothing changed) any compose file
where the result is not exactly that. Never start a second pusher, and never backfill a range of
rounds by hand during trials: S13 lost two identities that way.

The price is time. The model (`tests/ops/s100/test_beacon_lock_tolerance.py`, both store layouts,
checked against two S13 observations) says the gate stays closed after the lock for up to about
`4 x lock + 10` s with the run in the root store, about `3 x lock` in a child: 30-40 s after a 13 s
reference, 1 minute after 19 s, **3 to 4 minutes after a 60 s reference**. That figure is very
sensitive to the cost of one round: at 2.6 s it doubles, at 2.8 s it is five times larger, and at
2.9 s the pusher never catches up. Measure the idle lag with a child run present before trusting it.
Expect 503s in that window and retry them. Hence two rules: keep the CUDA cache (the executor does
by default), and run the first reference after a new image (43-64 s, cold) alone, before a second
driver starts. Releasing the lock during the reference (ADR 0012 section 2) or a pusher that pushes
the newest round first (same ADR, a proposal) is what removes the wait.

Proving it instead of inferring it: `beacon_gaps.py` lists rounds missing from the `beacon` table
of the store that hosts the run, the identities whose seed is one of them, and whether the pusher
is behind or dead. It is read-only and takes the run id.

```bash
ssh root@s100 "docker exec -i hypertrain-test-challenge python -I - 1800 $RID" < $T/beacon_gaps.py
```

Exit 0: nothing missing in the last 30 minutes. 3: a hole (`missing`, and `stranded` names who waits
for it: stop the run and report it) or a table that is dead (`stale`). 4: bad arguments or a run the
stack does not host. A pusher that is only behind exits 0: read `lag` and `gate_open` in the JSON.
Take a baseline before the retune (without `$RID` until the run is published: the tool reads the
store that hosts the run and refuses an id it cannot find) and run it again after every ten trials.
After a challenge restart during a lock, run it on the child store too: rounds the root kept and the
child never got are invisible to the pusher.

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

A manifest that sets `reference_spec.env.pinned` (`hypertrain/docs/protocol.md` section 7, item 4a)
makes the hand-set `ATEN_CPU_CAPABILITY=avx2` below unnecessary: the launcher gives every rank the
pins, and `bootstrap_service_capacity_v2` answers 409 `SERVICE_PROFILE_ENV_PIN` when the challenge
host cannot honor them. Unpinned runs keep the manual pin (`cpu_miners.py` is unchanged). The pin
removes ambient-induced divergence only; `docs/adr/0011-audit-integrity.md` part 3 says what was
and was not measured.

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
  can no longer progress (`admission.py` `challenge`). If it holds no stake, the next `join`
  suspends it as `APPLIED_EXPIRED` and its coldkey may apply again with a new hotkey (ADR 0010);
  a funded one stays APPLIED and blocks its coldkey. This is why `admit` joins a miner only
  when its turn starts.

A round opens on a roster of four funded, eligible miners with at most one in PROBATION, each with
a screen that lasts to the round's `d_final` (`open` refuses a hotkey whose screen ends earlier and
names the round). The operator opens it, the miners upload, and the operator settles it. Open it with five when an
eviction is plausible: a miner that never commits is left out (`excluded`, reason `NO_SHOW`) and
the others are audited and paid, while four miners cannot lose anyone (`audits` answers 409
`ROUND_INFEASIBLE`). `finalize` lists the roster minus the `excluded` records, read after the
aggregate. The operator runs `scripts/ops/live_operator.py` inside the TEST challenge:

```bash
L="docker exec -i hypertrain-test-challenge python -I - --master http://127.0.0.1:8000 \
  --run-id $RID --state-dir /data --coord-key /run/secrets/coord.key \
  --admin-token-file /run/secrets/admin.token"
O=hypertrain/scripts/ops/live_operator.py
ssh root@s100 "$L open --w 0 --hotkey H0 --hotkey H1 --hotkey H2 --hotkey H3 \
  --offsets 3,6,80,82,150,180" < $O     # gap 98: one lease window; end with 282 (gap 200) to fit a replacement lease
ATEN_CPU_CAPABILITY=avx2 $PY -I $T/cpu_miners.py round DIR 0 0 1 2 3   # one miner at a time
ssh root@s100 "docker exec -i hypertrain-test-challenge python -I - $RID 0" < $T/round_status.py
ssh root@s100 "$L audits --w 0" < $O     # once now >= d_audit
ssh root@s100 "$L finalize --w 0" < $O   # once every audit lease is complete with MATCH (a late one settles too)
ssh root@s100 "docker exec -i hypertrain-test-challenge python -I - $RID" < $T/weights_once.py
```

**`d_final - d_audit` decides how many audit leases fit in a round.** The store dates every lease
by the round it starts in (`created_beacon = now`), so a lease validates at any round before
`absolute = min(d_final, d_audit + 2 * lease_rounds)`; `lease_rounds` is 100 for H <= 30. With a
gap of 100 rounds or less the first lease runs to `d_final`: if its auditor goes silent the job is
not replaced, because an honest miner seals its commit until `d_final` and an audit job embeds it
live, so no window can start after that round; the miner is left out of the round (`excluded`,
`INFRASTRUCTURE`, no sanction). With a gap of 101 to 200 rounds a second auditor can take the job
inside the same window, and a job whose attempts fail early gets a second window that ends at
`d_final` (`audit-rearm`).
`live_operator.py open` accepts any gap from 2 to 200 rounds for every H; 200 is the service
horizon (`store.py:5109`). A round with a wider gap closes later: 100 more rounds (5 minutes) per
extra 100.

Run `audits` as soon as `now >= d_audit`: it must be called before beacon round `absolute - 1`
(`absolute = min(d_final, d_audit + 2 * lease_rounds)`), and a call after that answers 503 for every
roster.

Run `finalize` (it aggregates first) before `d_final`. The aggregate reads every audited member's
admission at the call and leaves out one that no longer qualifies (`excluded`, reason
`INELIGIBLE`, with its `cause`: rotated away, banned, suspended, back on probation, screen
ended, bond burned); nobody is sanctioned for it and the others are paid. A roster of four then
answers 409 `ROUND_INFEASIBLE` and writes nothing: repair the member's standing, or open the
next round with five.

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
the TEST container names, and the resolved compose config must differ only in the variables the
command sets):

```bash
ssh root@s100 "bash -s -- executor $B /data/executor/cuda-executor.json" < $T/challenge_image.sh
```

The challenge must run an image that contains the executor (`$NEW`, section 1). Setting it together with
`HYPERTRAIN_ROLE_DISPATCH` stops the challenge at start. A later
`swap` or `rollback` uses `compose.test.yml` alone and drops the variable: rerun `executor`. Undo
with `bash -s -- executor-off $B`. The challenge loads the executor on the first CUDA job, not at
start: a configuration it cannot load is a 503 `ROLE_DISPATCH_UNAVAILABLE` there, and `live` and
`check` do not see it. `executor` therefore asks the challenge itself, before and after (6.4a).

### 6.4a Results through R2 instead of ssh

Measured on 2026-10-28 (`livrables/forge/2026-10-28_transfer-measure`): ssh between s100 and a node
carried 7 MB/s, R2 from a node 20 MB/s on one stream and 100-144 MB/s on eight, so the 4.13 GB result
of the 101 M profile does not fit a lease over ssh and does over R2. Nothing below has been run on
s100 or on a node yet. The mechanism, its limits and the settings are in
[ADR 0013](adr/0013-executor-artifacts-via-r2.md) and
[cuda-executor.md](../hypertrain/docs/cuda-executor.md#results-through-r2-opt-in); this section is
the command.

The token is a file of the challenge's user in the executor directory, named like any other file
there, written the way the key was in 6.1 (stdin, so that the value is neither on s100's command line
nor in a shell history) and never shown, `cat`-ed or passed as an argument. It must be a bucket-scoped
token for a `cortex-v3-*` bucket (never the Cloudflare account token) with these names and no other:
`R2_ACCOUNT_ID`, `R2_ACCESS_KEY_ID`, `R2_SECRET_ACCESS_KEY`, `R2_BUCKET`, and `R2_ENDPOINT` if the
account's default endpoint is not the one to use.

```bash
ssh root@s100 "umask 077; cat > $X/r2.env; chown 65532:65532 $X/r2.env" < ./r2.env   # mode 0600
```

It is owned by uid 65532, not by root: the challenge's loader refuses a token file of any other
user, and the script refuses it before the loader can. `$X` is already `0700` and owned by 65532
(6.1). Then, inside the seal window (the first half, as for `swap`):

```bash
ssh root@s100 "bash -s -- executor $B /data/executor/cuda-executor.json --r2 /data/executor/r2.env --dry-run" < $T/challenge_image.sh
ssh root@s100 "bash -s -- executor $B /data/executor/cuda-executor.json --r2 /data/executor/r2.env" < $T/challenge_image.sh
```

The path of the token is the one the container sees. `--operations` is a comma list of `audit`,
`referee` (the default) and `reference`; keep `reference` out until the reference no longer holds the
store lock for its whole launch (ADR 0013, decision 1). Every other setting keeps the loader's
default; `fallback` is `classic`, so a broken R2 setup is not an error but the 7 MB/s path again:
look for `exec_artifact_unavailable` and `exec_artifact_skipped` in the journal
(`$X/state/journal.jsonl`) after the first job.

What the command does, in this order, and what stops it:

| Step | Refusal |
| --- | --- |
| backup, seal window, TEST container names | exit 3 (window), 4 |
| the token: one plain name in `/data/executor`, a plain file (not a link), mode exactly 0600, uid 65532, in a directory `0700` of 65532 | exit 4, with the mode or uid that was found |
| `r2-artifacts.json` is absent or one this script wrote (only the keys `credentials_file` and `operations`) | exit 4 |
| the running challenge is on the image `compose.test.yml` resolves, and its own loader accepts the settings (on stdin, nothing is written yet): the settings model, the token file, `network` `default`, `max_artifact_bytes` at most `max_result_bytes`, an R2 client, an `ssh` client | exit 4, with the loader's reason |
| `compose config` of the file and of the file with the override differ in the two variables and in nothing else | exit 5 |
| writes `r2-artifacts.json` (0600, 65532), then the override; `up` recreates the challenge, with `--force-recreate` when the settings changed but the variables did not | exit 7 |
| `live`, then the recreated challenge's own look at its environment and at the files as its user | exit 7, with the `executor-off` command |

`--dry-run` takes every step but the writes and the `up`, in a temporary directory, and prints
variable and key names only. It reports a closed seal window instead of stopping at it and exits
with the window's code, so a dry run that exits 0 means the real run would be accepted now.
Running the command again with the same arguments recreates nothing. The challenge reads the settings
and the token once per process: new settings are picked up by the recreate the script forces, a
rotated token at the same path is not. To rotate a token run `executor-off`, replace the file, run
`executor --r2` again.

`executor` without `--r2` goes back to the ssh path and removes `r2-artifacts.json`; `executor-off`
removes both variables and that file (only a file with the keys the script writes), after the
recreate, and leaves the token where it is: delete it and revoke the token yourself when the run is
over. `swap` and `rollback` drop both variables like the executor one: rerun the same `executor`
command. A failed `up` or look prints the `executor-off` command and leaves the files for it.

`r2-check` looks at the bucket from the challenge, with the token the challenge is configured with
(or, before `executor --r2`, with `--file`, which is checked like `--r2`'s). It is not gated, takes no
backup and changes nothing in the stack:

```bash
ssh root@s100 "bash -s -- r2-check" < $T/challenge_image.sh
ssh root@s100 "bash -s -- r2-check --file /data/executor/r2.env" < $T/challenge_image.sh
```

It puts 1 MiB of random bytes under `exec/_r2-check/<random>`, reads it back, compares and deletes it,
through the same client settings the transfer uses (no proxy or CA variable from the environment),
and prints one line: `r2-check challenge put=200 get=200 delete=204 bytes=1048576 put_s=… get_s=…
delete_s=…`. A step that failed shows its HTTP status or the class name of the error instead
(`put=403`, `put=ConnectError`), a read that returns other bytes `get=mismatch`, a read that did not
run `get=skipped`. Never a URL, a key, the bucket or any value of the token. Exit 8 when a step failed
or the token does not load, 4 when the script refuses `--file`. One mebibyte measures the first byte
(about 0.36 s from a node) and not the bulk rate: the rate from s100 to R2 with several parts at once is
still to be measured before the run, and `put_s` says nothing about it.

`--node LABEL sha256:<image digest>` adds the look from an executor host (`LABEL` as in
`cuda-executor.json`, the digest the `$GPU` above), between the read and the delete:

```bash
ssh root@s100 "bash -s -- r2-check --node x $GPU" < $T/challenge_image.sh
```

The challenge hands two presigned URLs of that object (HEAD and GET, 120 s) to a `--rm` container
of the GPU image that the executor's own ssh transport starts on the host (its key, its pinned
`known_hosts`, its `docker` words), with the URLs on the container's stdin: they are in no argument,
no file and no log. The container prints the paths Python's `ssl` looks for CA certificates in, then
makes a HEAD and a one-byte ranged GET to the endpoint with the default TLS context, the one the
runner uses. The image must already be on the host: the command passes `--pull never`, and exit 125
(`r2-check node x exit_125`) means docker refused, usually because it is not there
(`cuda_executor_check.py` reports `image_present`). A host is a rented machine and may print
anything: only fields of a fixed shape are shown, rebuilt by the challenge, and never its stderr.

```
r2-check node x python=3.12.4 openssl=OpenSSL_3.0.13_30_Jan_2024 cafile=1 openssl_cafile=0 capath_entries=146 ssl_env=0 head=200 head_bytes=1048576 head_s=0.41 get=206 get_bytes=1 get_s=0.38
r2-check node x … head=SSLCertVerificationError head_verify=unknown_issuer get=SSLCertVerificationError …
```

The two statuses are the verdict; `cafile`, `openssl_cafile` and `capath_entries` say where the
interpreter looked. A `head_verify` of `unknown_issuer` or `self_signed` is the open question of
ADR 0013 (does the image carry a CA bundle that its Python can see): the cure is in the image, never
in turning verification off. HEAD is only checked here: the runner itself sends GET and PUT, and R2's
answer to a presigned HEAD has not been seen, so a 403 on `head` with a `get` of 206 is not a
network problem. None of this has run on a node.

Exit codes: 2 usage (and `readyz`), 3 seal window, 4 refusal, 5 unexpected compose change, 6 database,
7 the challenge is not up, or not configured as asked, after a change, 8 `r2-check` did not complete.

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
  `docker` field of `cuda-executor.json` that added only `-v <host dir>:/root/.nv` to `docker run`.
  The executor now does this itself (`compute_cache`, default on, ADR 0012): one directory per image
  digest under `~/.cortex-cuda-executor/compute-cache` of the ssh user, mounted with `--mount`. **Remove
  the wrapper** from the `docker` field (`["sudo","-n","docker"]` again): with both, the container would
  get two mounts on `/root/.nv`. The first reference after a new image digest is still cold (43-64 s):
  run it alone, before a second driver starts.
- **The reference holds the store lock**, so the beacon pusher times out meanwhile and every trial
  waits for it to catch up (503 "beacon infrastructure paused" for 10-70 s). A second driver or a
  second executor host does not shorten the references; two drivers only overlap the miner side.
- **Do not push beacon rounds out of order.** The challenge reads exact rounds (a trial's seed round,
  `d_audit`). A second pusher with `--catch-up 1` moved `known` past rounds the first one would have
  filled; the challenge request (then an unsigned `GET /join/<id>/challenge`, an owner-signed
  `POST` since ADR 0010) answered 500 `BeaconUnavailable: round N not pushed` and
  that trial could not continue. Backfilling the gap through `/v1/admin/beacon` took 2 minutes of
  store lock, a pause of 5 minutes, and two trials passed their 100-round window: the identity is
  SUSPENDED `CHALLENGE_EXPIRED` at its next challenge request, and `resume` resets `clean_count` to 0.
  Since ADR 0010 such a trial is closed `EXPIRED` at the next beacon push and the identity keeps
  its state; its next request answers 409 `CHALLENGE_EXPIRED`.
- **A validator cycle on A** shows as `exec_host_unavailable` ("0 idle GPUs, 1 needed") and the
  executor waits (`sweeps`); it cost one trial 40 s.

Plan 40 trials (3 ACTIVE x 12 + 1 PROBATION x 4) at 40-50 s each with the cache, so about 30 minutes,
then the round (about 6 minutes). Without the cache, plan 85 minutes.
