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

`rollback` and `restore-db` are not gated: they are the way back.

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
`relay.json` and `metagraph.json`. Every step that takes a backup verifies its `SHA256SUMS`.

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
ssh root@s100 "$L finalize --w 0" < $O   # once every audit lease is complete with MATCH
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
