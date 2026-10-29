# Replay of the dense TEST models (`dense-0.03b`, D94; `dense-0.1b`) on the s100 TEST stack

**D94 (2026-10-29): the run uses `HT_D20_GEOMETRY=dense-0.03b`**, 31,462,656 parameters, the row
`mfu-dense-0.03b` ([ADR 0016](../adr/0016-d94-dense-test-model.md)): at 100,676,352 parameters the miner side
of a trial took 217-282 s of the 297 s window and the first full reference 338 s [A, S17b], so no admission
trial passes at 101 M. `dense-0.05b` (44,045,568) is built and tested but over 250 s by ADR 0016's rule; use
it only once that rule holds. The geometry changes the model only: every step below (two builds, warm-up,
measured replay, stop above 298 s, dry-run comparison) is the same for the three geometries. Sizes and times
below are the 101 M ones unless `dense-0.03b` is named; section 6 gives the scaled ones (about 0.31 of each).

100,676,352 parameters (`dense-0.1b`), `decoder`, H = J = 4, one GPU. Decision, budget and prerequisites a-f:
[ADR 0015](../adr/0015-intermediate-dense-test-model.md). Base procedure: [the TEST stack](../s100-test-stack.md)
section 6 and its Rollback section, and the S15 operator tools (`lib.sh`, `mkenv.sh`, `mk_exec_config.sh`,
`bgpu.sh`, `watchdog.sh`, `mk_origins.py`, `stage_seeds.sh`, `mk_miner_seeds.sh`; not in the repository, they sit in
Forge's `2026-10-27_s15-replay-s100-gpu/tools` and hard-code its worktree, ss58s, node addresses and key paths).
Anchored, like ADR 0015, on the `v3-last-merges` tip (D89, 2026-10-28; code read at `faa6abb4`, the builder at
`f84dd57c`). Labels: [V]
verified at the tip, [A] reported, [H] estimate, [?] unknown. **Nothing below was run.** Env vars appear by name
only; never print or store a value.

## 0. Go / no-go before anything is touched

The run was **not possible on the code this runbook was written on** (`2cfc2a2b`). On the tip, ADR 0015
prerequisites **(a)** lock-free reference and **(b)** `copy_flags` out of the store lock are merged [V by
reading], but no image is built from such a commit yet. **(e)** is in part: a TEST manifest may raise
`gap_cap_rounds` above 200 and `live_operator open` takes its cap from the audit policy [V], but every audit must
still end before `absolute = min(d_final, d_audit + 2 x lease_rounds)`, 600 s at H = 4 [V by reading], so five
audits in series (718-1279 s [H]) still need the second executor host (config only). **(d)** is needed only if
H >= 31. The **(f)** tooling is merged (`challenge_image.sh executor --r2`, `r2-check`) [V by reading]; decide
(f) from section 2. Start only when the images come from a commit that has (a) and (b), the second executor host
is ready for the real run's executor config, Forge has answered the warm-up host question of section 1 step 3,
and (f) is decided. (c) is not needed for the driver. The warm-up run of section 4
measures the replay and the window; it does not replace the prerequisites.

Server, miner and `publish_run.py` come from one commit. After `e0969448` (`13ec158c`, ADR 0010) the owner routes
`/challenge` and `/job` became signed `POST`s (they were `GET` with headers there), so a tip miner cannot talk to
the S15 server image `a513821d` [V by reading], and the S15 GPU image `331cec82`, whose miner CLI the CUDA driver
runs, cannot talk to a tip server. Since `5c6e02ee` and `fe5ce134` (merged from origin/v3 `2cfc2a2b`) the genesis
origin ids are keyed with `escrow_id_key`: the tip's `publish_run.py` runs inside the server container and imports
it, so it cannot run in an image built from `e0969448`, and a tip server refuses an unkeyed genesis
(`GENESIS_ORIGIN_DOMAIN`) [V by reading; the S15 build commit is [A]]. Every tip manifest also carries
`reference_spec.env.pinned` (DET-01), which the `ReferenceEnv` of `e0969448` refuses, so neither S15 image can
serve a tip manifest [V by reading]. A new commit gives new image digests, hence a new run id and a cold compute
cache.

Guards (S15, unchanged): TEST stack only; never touch `cortex-challenge-hypertrain`, `cortex-master-*`,
`hypertrain-beacon`, `hypertrain-auditor-go-*`, `hypertrain-auditor-0/1`, `/data/role-dispatch`, the host
`/data/runs` (production), nor `executor-*` containers on the rented nodes; nodes get the image and `--rm`
containers only, no checkout and no R2 token; no `HT_GATE_MAX_AGE_S` override (since D67 it can only tighten the
4500 s default; keep the default), no second pusher and no manual backfill; no challenge or executor restart while
a trial or round is open; one driver, never two; never `pkill -f` in an ssh command; miners start only after
`open` returned 200.

## 1. Prepare offline (no s100 change)

1. `gh attestation verify` of the server and the GPU digest; pre-pull the 17.35 GB GPU image on nodes A and B (a
   pull inside a reference is aborted); on s100 pull the server image, `snap.sh pre`, `challenge_image.sh backup`,
   `paper` (the backup directory is the rollback source and every gated command refuses without it).
2. Seeds: `stage_seeds.sh $WARM` **before** the warm-up build (otherwise the builder draws fresh
   owner/coord/auditor seeds and `relay_config.py` and `install` refuse the run); the real build copies them from
   `$WARM` (section 4).
   Six identities: five miners and one sacrificial (index 5). S15 stages five: generate the sixth hot and cold
   seeds, `NMINERS=6`, stage `cold-0..5`, and add **two** hotkeys (miners 4 and 5) to the stub metagraph (the join
   and `get_weights` need the hotkey registered; restart `master-stub`). Retarget `lib.sh` (worktree, `PY`,
   `OWNER`, `AUD0/AUD1`, `RELAY_K1`, node addresses, ssh key).
3. Build twice (Forge, 2026-10-28). First the throw-away warm-up run, into its own `$WARM` (a directory named
   `warmup`):

```bash
UV_OFFLINE=1 HT_D20_GEOMETRY=dense-0.03b HT_D20_COLDKEYS=6 HT_D20_RELAY_PUBKEY=<relay k1 ss58> \
  HT_D20_REPLAY_SECONDS=300 \
  hypertrain/scripts/b300/d20/build_test_manifest.sh <GPU image digest> 595.91.07 148 $WARM
```

`HT_D20_REPLAY_SECONDS` has no default (the build exits 2 without it) [V]. 300 is the ceiling `validate_training`
accepts: T 200, `gap_cap_rounds` 305. The warm-up run serves the four trials of section 4 and nothing else. The
real run is built only after section 4 (step 6 there), into a new `$OUT` with
`HT_D20_REPLAY_SECONDS=<max warm replay, whole seconds, rounded up>`: the max of at least three measured warm
replays on the B300, never an ADR estimate, and with `HT_D20_WARMUP_OUT=$WARM`. The audit policy signs
`reaudit_rounds` from it and, above 174 s, `gap_cap_rounds` (ADR 0011 Part 9, D84/D88); above 300 s the manifest
stage refuses the build [V]. Given `HT_D20_WARMUP_OUT`, the builder refuses (exit 2, "the manifest is the warm-up
build's") a real build whose manifest is the warm-up's, which a max of 299 or 300 s gives, and its dry run checks
that only the audit policy differs (`warmup_same_training`, `warmup_differs_only_by_the_audit_policy`) and prints
the warm-up's run id (`warmup_run_id`) [V]. The builder refuses an `OUT_DIR` whose `inputs`, `out` or `state`
already exists [V].

About 5 s for `dense-0.03b` [V] (11 s for `dense-0.1b` [V], 20 s on a host at load 338 [V]). Each build prints
`"param_count": 31462656` and `"init_state_hash": "e372a68e…0aa6"` for `dense-0.03b` (44045568 and
`9cbe3ee9…f897` for `dense-0.05b`, 100676352 and `be9b7bd6…28f8` for `dense-0.1b`); anything else means the row
or the init changed. It also prints an `audit`
block (`lease_rounds` 100, then `reaudit_rounds` / `gap_needed` / `gap_cap`: 200 / 305 / 305 at 300 s, 116 / 199 /
200 at 174 s, 100 / 175 / 200 at 144 s; `replay_seconds`, `warmup_replay_seconds` 300 and `stop_above_seconds`
298, the STOP of section 4 step 4), a `pin` block (`cpu` avx2, `gpu` sm_103 for 148 SM, `attention` math)
and `checks`, all `true`; if one is false the build exits with `DRY-RUN VALIDATION FAILED` [V]. Then, for the
warm-up run (and again for the real run, section 4), as in S15: `cuda_qualification.py build`, `relay_config.py`
(check the as-found `relay.json` hash first), `mk_exec_config.sh` on the run id of `out/index.json`,
`mk_origins.py` (the S15 copy calls `genesis_origin_id` with three arguments and raises `TypeError` at the tip,
where it takes a fourth, `id_key`: make it pass `escrow_id_key(<coordinator key>._secret)` as
`publish_run.py genesis` does with `coord.key`, so that the list equals the origin ids the genesis step submits [V
by reading]), the flat stage directory. Executor config: S15 values (`sudo -n docker`, `sweeps` 20, pause 15 s);
the real run's config adds the second executor host that (e) needs as a second `hosts` entry with its own label,
address and `known_hosts` file (`hosts` takes 1..8) [V by reading `ExecutorConfig`]; `max_result_bytes` stays
16 GiB (the result is 4.13 GB). **The warm-up's host list waits on Forge.** The executor picks a job's first host
from the job digest (`cuda_executor.py:633-636`) and the compute cache is per host and image digest
(`:365,771-773`) [V by reading]. With both hosts listed, the four serial trials of section 4 give three warm
replays only if all four land on one host (about 2 in 16 if the digests spread evenly [H]); otherwise each
host's first job is cold and at most two warm replays remain. Before writing the warm-up's config, ask Forge
through the relay tool how to warm and measure the hosts (for example a one-host warm-up config, or more trials)
[?]. Validate it the way the challenge loads it (`ExecutorConfig.model_validate_json`,
`hypertrain/docs/cuda-executor.md`): a bad file is a 503 `ROLE_DISPATCH_UNAVAILABLE` at the first CUDA job. The
file serves one run id (a CUDA job of another run raises before any ssh call) and the challenge loads it once per
process, so the real run gets its own file under another path (section 4) [V by reading].
4. R2 only if (f) says so, with the tooling of the tip (`docs/s100-test-stack.md` 6.4a). Put the token on s100 by
   stdin as `/data/executor/r2.env`: 0600, uid 65532, in the 0700 directory of 65532, holding only
   `R2_ACCOUNT_ID`, `R2_ACCESS_KEY_ID`, `R2_SECRET_ACCESS_KEY`, `R2_BUCKET` and optionally `R2_ENDPOINT`. Run
   `r2-check --file /data/executor/r2.env`. In the gated window,
   `executor $B <executor config> --r2 /data/executor/r2.env [--operations LIST] --dry-run`, then the same
   without `--dry-run`; then `r2-check` and `r2-check --node <label> $GPU`. The script sets `HYPERTRAIN_CUDA_EXECUTOR_ARTIFACTS` and writes
   `r2-artifacts.json` with `credentials_file` and `operations` only (default `audit,referee`); it has no
   `--fallback` and refuses a settings file it did not write, so `fallback` stays `classic`: watch
   `exec_artifact_unavailable` and `exec_artifact_skipped` (section 7, STOP 4) [V by reading]. If `fail` is still
   wanted, the script needs a `--fallback` option: a Forge decision. The challenge's own loader checks the settings
   and the token before and after the recreate, and nothing prints the bucket or a value. `reference` may go in
   `--operations`, since the reference no longer holds the store lock (ADR 0013 status); it is opt-in and
   unmeasured. The bucket needs the ADR 0013 rules.

## 2. Measure first (replaces the estimates; commands in ADR 0015)

1. After `p2_key` and `p2_stage_exec` (key and `known_hosts` only): ssh round trip and rate, both directions,
   challenge container to node A. Target `>= 60 MiB/s` as a necessary floor (`T_trial` is 267-311 s there), 100
   or more for margin; below about 43 the 297 s window cannot hold. A first measurement exists [A,
   `livrables/forge/2026-10-28_transfer-measure`, `docs/s100-test-stack.md` 6.4a]: ssh between s100 and a node
   carried 7 MB/s, far under that floor; R2 from a node 20 MB/s on one stream and 100-144 MB/s on eight.
   Re-measure to nodes A and B.
2. Node B to the relay: rate of a 1.7 GB PUT in 1 MiB parts and a 0.8 GB GET (`T_miner` is 93-137 s on it).
3. `df` on s100 (about 390 GB for 52 trials plus about 60 GB for a round [H]: nothing prunes `trials-v2/`), node A
   docker root (about 10 GB per job plus the image), node B `/mnt/nvme`; RAM on s100 (a reference reads a 1.2 GB
   state; the aggregation peaks at about 10-13 GB [H]).
4. Pusher cost per round `c` from the beacon log (2.4-2.7 s in S15) and the Lium cycle schedule on node A.
5. Optional, from a host of ours and **not** from the rented nodes: R2 and the profile's step time (ADR 0015). The
   rate from s100 to R2 with several parts at once is not measured; `r2-check` moves 1 MiB and measures the first
   byte only, and `r2-check --node` checks the GPU image's TLS and presigned GETs from a node without a token.

## 3. Gated window and publication

As S15 P1 and P2: `swap`, `beacon` (0.2/60/100), `executor` (with `--r2` if (f) chose R2) in one gated window.
The gate needs `readyz` 200 and the master's seal of the epoch before its current one, sealed and younger than
`HT_GATE_MAX_AGE_S`, 4500 s by default since D67 [V by reading `challenge_image.sh` `gate`]; the S15
`gate_status` copy still hard-codes 1800 s [A, commit `2a43dc30`]: retarget it with `lib.sh`. Then the ephemeral
ssh key on node A, `cuda_executor_check` (`status ok`, driver 595.91.07, capability `[10, 3]`, one idle GPU,
`image_present`), `install` (must print `"test_only": true` before `run`), `publish_run.py` steps
`objects run dataset genesis status`, relay re-bind, auditors on the run, baseline `beacon_gaps.py 1800 <run id>`
(a run in a child store needs its id). These steps run for the warm-up run first and again for the real run
(section 4). The model is initialised by the builder (2.5-3.9 s), at each trial staging, at `open` and in the
aggregate, not by `run`.

## 4. Warm-up run, replay measurement and the window gate (G0), before any real identity

The first reference after a new GPU digest is cold (57 s on the tiny model [V, S15]). Since `25f58756` (ADR 0012
Part B) a reference holds the store lock only to claim and to settle, not while it stages, launches and puts its
objects (`store.py:3550-3682`) [V by reading], so it should no longer close the gate [A, fake executor, not
replayed on s100]; a cold one can still miss its 297 s window. The compute cache is one directory per executor
host and image digest (`cuda_executor.py:365,771-773`) [V by reading]: a new image starts cold, and a later run on
the same digest and host finds the cache an earlier run left. The executor serves one run only
(`ExecutorConfig.run_id`; `hypertrain/docs/cuda-executor.md`). Hence two builds (Forge, 2026-10-28):

1. **Warm-up run, throw-away.** The `$WARM` build of section 1 step 3 (`HT_D20_REPLAY_SECONDS=300`). Record its run
   id in the report as the warm-up run and label it `warmup`; never reuse it (no round, no real identity). Publish
   it with the per-run steps of sections 1 and 3. A trial puts its objects on the challenge's `/objects/` route,
   not through the relay (`miner/core.py`) [V by reading], and the warm-up opens no round, so the relay re-bind and
   the auditors can wait for the real run [H].
2. **Four trials.** Alone on the stack (no other driver, no operator call, no open Lium cycle on A or B):
   `admit "5:4"` on the warm-up run for the sacrificial identity, with `HT_MINER_DEVICE=cuda` (`grep '^device'` on
   every `miner.toml`) and `HT_IMAGE_DIGEST` equal to the manifest digest. Four trials: the first cold, then three
   warm. If a trial lapses (409 `CHALLENGE_EXPIRED`; `probe-v2` exits 3 after 10 minutes), run
   `live_operator.py reseed --admission-id <id>` on the warm-up run (once per lapse), start the driver again and
   report it. The four trials keep about 30 GB on s100 (7.45 GB a trial [H], ADR 0015 "Disk").
3. **Measured replay.** In the executor journal (`journal.jsonl` in the config's `state_dir`), a replay is the
   `unix` of an `exec_published` line minus the `unix` of the `exec_attempt` line with the same `name`
   (`cuda_executor.py:1068-1075,1143-1145`) [V by reading]. The first one is `T_cold`: report it separately and
   never count it. A warm replay runs on a host that already published a job of this image, with `compute_cache`
   true (the cache was mounted). `HT_D20_REPLAY_SECONDS` of the real run is the **max of the three (or more) warm
   replays**, rounded up to whole seconds. Also read `t_probe` (the `probe-v2` seconds of
   `stages.tsv` plus its waits), the first `proof-v2` POST, the 503 gate-closed time
   (`beacon_gaps.py 1800 <warm-up run id>`), and check that `exec_cache_unavailable` is 0.

```bash
python3 -I -c 'import json,sys
rows=[json.loads(l) for l in open(sys.argv[1]) if l.strip()]
start={r["name"]:r for r in rows if r["kind"]=="exec_attempt"}
for r in rows:
    a=start.get(r.get("name")) if r["kind"]=="exec_published" else None
    if a: print(a["label"],a["operation"],a["compute_cache"],round(r["unix"]-a["unix"],1))' journal.jsonl
```

4. **Thresholds.** Up to 174 s the real run keeps the default cap: its `d_final - d_audit` lies between
   `reaudit_gap_needed` (175 rounds at 144 s, 199 at 174 s) and 200. From 175 to 300 s the builder signs
   `gap_cap_rounds` equal to that gap (226 at 200 s, 276 at 256 s, 305 at 300 s; TEST ledger only), and the real
   run is opened with exactly that gap, the `gap_needed` its dry run printed (`build_test_manifest.sh:432-439`,
   `live_operator.py:158-191`) [V by reading]. Above **298 s: STOP** and report to Forge; do not raise the ceiling
   (Forge, 2026-10-28). At 299 and 300 s the window is the warm-up's own (T 200, cap 305, at the
   `validate_training` ceiling with no margin), so the rebuild would be the warm-up run: the builder refuses it
   given `HT_D20_WARMUP_OUT` (step 6) [V]. Above 300 s the manifest stage refuses the build (`re-audit window must
   be between one and two leases`) [V].
5. **G0 go/no-go (the window).** On the three warm trials `t_probe + T_ref + 17 s <= 270 s` must hold (the window
   is 297 s, 270 keeps a 27 s margin; estimate 233-277 s at 105 MiB/s [H]). Otherwise **NO-GO**: stop, clean up,
   report. An image built from a commit without Part B is NO-GO too: there a trial survives a lock of only 27-54 s
   at `t_probe` 0 (24-48 s at 30 s), against 123-235 s [H].
6. **Rebuild.** A new `$OUT` whose `secrets/` (0700) holds a copy of every seed of `$WARM/secrets/` (0600), the
   same image digest, driver, SM count and `HT_D20_COLDKEYS`, `$(cat $WARM/inputs/exp-round.txt)` as the fifth
   argument, `HT_D20_REPLAY_SECONDS` set to the measured max (at most 298 s, step 4) and `HT_D20_WARMUP_OUT=$WARM`.
   The run id follows the audit window `reaudit_rounds_for(replay, 100)`, not the seconds
   (`build_test_manifest.sh:432-439`) [V by reading]: every max up to 298 s asks another window than the
   warm-up's, so another run id (`test_every_replay_up_to_298_s_asks_another_window_than_the_warm_up`, and the
   full builds of `test_the_rebuild_with_the_measured_replay_changes_only_the_audit_window` at 298 s) [V]. A max of
   299 or 300 s would rebuild the warm-up's manifest and run id: the builder exits 2 and removes what it made
   (`test_a_real_build_that_would_be_the_warm_up_is_refused`) [V]; that is the STOP of step 4. A new `EXP_ROUND`
   would not tell the two apart: it enters only the envelope (`ops/manifest_v2.py:128`), not the manifest. The
   compute cache directory is keyed by executor host and image digest
   (`cuda_executor.py:365,771-773`) [V by reading], so the real run mounts the warm-up's directory on the hosts
   the warm-up used; whether those entries serve the real run is [?] (note at the end of this section).
7. **Dry-run comparison, before the real run is published.** The real build's dry run already compares the two
   manifests (its `warmup_*` checks and `warmup_run_id`, step 6). The two dry runs print the same `param_count`,
   `init_state_hash` and `pin`; only `run_id`, `envelope_sha256`, `audit` and `warmup_run_id` differ. Then, on the
   build host, the same comparison on the input files:

```bash
python3 -I - "$WARM" "$OUT" <<'PY'
import json, sys
from pathlib import Path
w, r = (Path(p) for p in sys.argv[1:3])
read = lambda d, n: json.loads((d / n).read_bytes())
assert (w / "inputs/training.json").read_bytes() == (r / "inputs/training.json").read_bytes()
p = [read(d, "inputs/policies.json") for d in (w, r)]
print([{k: x["audit"].pop(k, None) for k in ("reaudit_rounds", "gap_cap_rounds")} for x in p])
assert p[0] == p[1], "policies differ outside the audit window"
m = [read(d, "out/manifest.json") for d in (w, r)]
assert m[0]["training"] == m[1]["training"], "training differs"
diff = sorted(k for k in m[0]["network"].keys() | m[1]["network"].keys()
              if m[0]["network"].get(k) != m[1]["network"].get(k))
assert diff == ["audit_policy_hash"], diff
print("same training and policies; only the audit window and its hash differ")
PY
```

   It prints the two windows (`reaudit_rounds` 200 and `gap_cap_rounds` 305 for the warm-up) and exits 0 only if
   `training.json` is the same (image digest, model, init state), the policies differ only in
   `audit.reaudit_rounds` and `gap_cap_rounds`, and the manifests differ only in `network.audit_policy_hash`: the
   checks of `test_the_rebuild_with_the_measured_replay_changes_only_the_audit_window` [V]. The command itself was
   only checked on hand-written files.
8. **Publish the real run for the 5 real identities.** Its per-run steps of sections 1 and 3, in a gated window
   with no trial open: `cuda_qualification.py`, `relay_config.py`, `mk_exec_config.sh` on its run id,
   `mk_origins.py`; `challenge_image.sh executor $B <new container path> [--r2 /data/executor/r2.env
   [--operations LIST]]` with a **new container path** (compose recreates the challenge only when the variable
   changes, and the challenge loads the file once per process) [V by reading]. If (f) chose R2, give the same
   `--r2` and `--operations` as in section 1 step 4 again: without `--r2` the script drops
   `HYPERTRAIN_CUDA_EXECUTOR_ARTIFACTS`, checks that no settings remain and removes `r2-artifacts.json`
   (`challenge_image.sh:692-695`) [V by reading], so the real run's results go back over ssh. Then rerun
   `r2-check`. Then the ephemeral key, `cuda_executor_check`, `install`, `publish_run.py`, relay re-bind, auditors
   on the run and a new driver directory; `beacon_gaps.py 1800 <run id>` must exit 0 before its first trial. A
   run POSTed while the challenge hosts another lives in a child store: pass `--state-dir /data/runs/<run id>` to
   `live_operator.py` (`docs/s100-test-stack.md` section 2).
9. **Clean the warm-up run** as S15 did, with the proof (listings before and after) in the report: no `ht-exec-*`
   container left on node A; the sacrificial identity's driver directory and seed copies on node B shredded; its
   stage directory on s100 removed; its ephemeral ssh key revoked once the real run's executor config no longer
   names it, and `authorized_keys` compared with the as-found copy; its executor config file removed once the real
   one is loaded; `build_test_manifest.sh --clean $WARM` once `$OUT` holds the seed copies. Leave its run store in
   place (a child store `/data/runs/<warm-up run id>/` when the challenge already hosts a run): no documented step
   removes a run from a live challenge [?]. Report its size.
10. The manual W2 path (a `docker run` of `train_round` on node A outside the executor) is **not a substitute**
   (Forge, 2026-10-28): it is not the executor path of the references and audits, and its time is not a replay.

[?] The cache and the measurement are per host; the warm-up's host list is Forge's answer to the question of
section 1 step 3. The first real reference must show a replay near the measured max with `compute_cache` true; a
replay near `T_cold` means the cache did not carry.

## 5. Admission and the round

- `admit "1:12,2:12,3:12,4:12,0:4"` with the watchdog (poll every 2 min or less; a lapsed identity (409
  `CHALLENGE_EXPIRED`) gets one new window per lapse with `live_operator.py reseed --admission-id <id>`; `probe-v2`
  waits 10 minutes for it, then exits 3; start the driver again). 52 serial trials, about 3.4-5.6 h at 233-389 s
  [H], inside the 20000-round screen (16.7 h). After a partial `proof-v2` re-send the saved proof file, never
  relaunch `admit`. Check `df` after trial 1 and every 10.
- **Round offsets are computed, not copied.** A round opened once cannot be opened again with other offsets (409
  `conflicting round opening`), and `open` reads `now` before it builds the five roster anchors. Per component
  [H]: client build about 10 s a hotkey (54 s); the server open holds the lock about 14 s a roster entry (77 s).
  With the head-first pusher (`a52644d8`, ADR 0012 section 3) the gate should reopen within about 8 s of a lock of
  up to 150 s [H, model; not run on s100]; the factor 7.3 (about 560 s) was S15's ascending pusher. Until the stack
  shows otherwise, keep it as the margin of `d_assign` below: it only delays the round. Then five sequential
  `run-v2` of about 90 s; `d_audit` = `d_commit` + 2; `d_upload` after the last relay receipt; `d_final` >=
  `d_audit` + the audit batches: an audit has to complete before `absolute` = `min(d_final,
  d_audit + 2 x lease_rounds)`. The aggregate and the finalize may follow `d_final` (since `5bc7524d`) and compute
  without the store lock (since `e2f47116`), but call the aggregate before it: it reads each audited member's
  admission at the call and leaves out one whose screen has ended (`SCREEN_LAPSED`), or answers 409
  `ROUND_INFEASIBLE` if too few remain. In rounds of 3 s: `d_open >= ceil(build/3) + 30`,
  `d_assign >= d_open + ceil(8.3 x open_lock/3) + 10`, `d_commit >= d_assign + ceil(5 x T_run/3) + 20`.
  `d_final - d_audit` must lie between `reaudit_gap_needed(100, reaudit_rounds)` and the audit policy's cap:
  `live_operator.py open` refuses a shorter gap (`check_reaudit_gap`) and a wider one (`check_open_shape` with
  `gap_cap()`, 200 unless `gap_cap_rounds` raises it on a TEST manifest; `live_operator.py:130-191,213-215`) [V by
  reading]. Above a 174 s replay the builder sets the cap to that gap, so the gap is exactly it (section 4). The
  first window of every audit still ends at `absolute`, at most 200 rounds after `d_audit` whatever the gap; a
  wider gap holds the re-audit window of a withheld MISMATCH (`exit_v2.py:162-167`), not more first-window audits.
  The audits must fit `ceil(batches x T_audit/3)` in that window: at H = 4 on one executor host this is 718-1279 s,
  240-427 rounds [H], over 200; that is still the second executor host of prerequisite (e). Do not copy the
  offsets of `launch_cuda_run.sh` (`OFFSETS` default, gap 100) or the tiny-run example of
  `docs/s100-test-stack.md` section 5: `open` refuses a gap below `reaudit_gap_needed` for this policy. `open` also
  refuses a roster hotkey whose `screen_until` is before `d_final` (`open_round_v2`, `live_operator.open_round`);
  the 20000-round screen covers the 4,800-7,200 rounds of the campaign (ADR 0015, Decision 2). Open only with a
  vector that `live_operator.py open` accepts (`check_gap_cap`, `check_open_shape` with the policy's cap,
  `check_reaudit_gap`) and that the measurements support.
- Before `audits`: `round_status.py` shows commit, leaves, accept, assignment and start equal to 5, no lease, and
  the rounds `d_assign` and `d_audit` present in `beacon_gaps.py`. Then run `audits` once, at `d_audit` (it is
  refused from `absolute - 1`): the minimum gap of the re-audit window does not count the rounds before the first
  lease or the second auditor's wait (D88, `docs/s100-test-stack.md` section 5), so a late call eats that slack.
  Start `finalize` once the last MATCH is in and before `d_final`: it calls the aggregate first, which leaves out a
  member whose admission no longer qualifies at that call, and the Finalize itself may follow `d_final` (it still
  settles for the members that qualify then, because the ledger judges the stored commit and verdicts at the beacon
  of their `accepted_v2` row, the audit challenge at the `created` beacon of its lease, and `live_operator finalize`
  seals its Finalize until `max(d_final, now) + 1000`). The aggregate and the finalize's replay no longer hold the
  store lock while they compute (`e2f47116`) [V by reading], so they should not close the gate [H, not replayed on
  s100]. If a Finalize is still refused (503), or the aggregate answers 409 `AGGREGATE_STALE` (the round changed
  while it computed, nothing written), run `finalize` again: an aggregated round returns its saved tape and a fresh
  Finalize is sealed [V by reading]. Wait 60 s for vesting; `weights_once.py` once. Expected: 200, five hotkeys
  equal (4 x 250000 if one miner was excluded).

## 6. Expected timings ([H] unless marked; replace with the section 2 and 4 measurements)

The `dense-0.03b` column is the 101 M measurement scaled by 0.3125, the ratio of the parameter counts (ADR 0016
"Sizing").

| Step | `dense-0.03b` (run profile) | 101 M | Tiny, measured |
| --- | --- | --- | --- |
| build of the manifest | 5 s [V] | 11 s [V] | 8 s [V] |
| warm reference | about 106 s | 338 s [A, S17b, first full one] | 13 s [V] |
| miner side of a trial (warm / cold) | about 88 s / 109 s | 217 s / 282 s [A, S17b] | |
| trial (miner + reference, D94 limit 250 s) | about 194 s warm, 214 s cold | 555-620 s, over the 297 s window | 145 s [V] |
| `run-v2` per miner | about 30 s | about 90 s | 24 s [V] |
| audit | about 106 s each, if near the reference | 144-256 s each | 14 s [V] |
| aggregate (two passes) + finalize, five miners | about a third of 101 M | 327-423 s + 177-223 s [A, measured at this profile, `hypertrain/CHANGELOG.md`], no longer under the store lock | seconds |
| result / inputs / delta | 1.29 GB / 252 MB / 32 MB | 4.13 GB / 805 MB / 102 MB [V sizes] | kilobytes |

## 7. STOP conditions

The tiny-run rules stand: no beacon or gate intervention during a run, a lag over 2 rounds for 60 s, the same
stage failing three times, `exec_host_refused`, a driver exit not understood (read its `SystemExit`; relaunch the
identical `admit` only if at least the measured `T_trial` in rounds, about 90-130, remains in the stored
challenge). A `probe-v2` exit 3 (window lapsed, 409 `CHALLENGE_EXPIRED`) is understood: run
`live_operator.py reseed --admission-id <id>` (once per lapse), then relaunch the identical `admit`. Added for this
size:

1. **Lock** over 150 s (the `beacon` command writes only catch-up 60) or any `missing`/`stranded` gap: stop.
2. **Window**: two consecutive trials over 270 s, or any `409 envelope expired`: stop.
3. **Audits and tail**: a go/no-go at `open`, not at `audits`. The vector must satisfy
   `reaudit_gap_needed <= d_final - d_audit <= gap_cap()` (`live_operator.py open` refuses it otherwise), and the
   first-window audits, `ceil(batches x T_audit / 3)` rounds, must end before `absolute` =
   `min(d_final, d_audit + 200)`, with `T_audit` from the warm-up trials scaled by section 6. The aggregate and the
   finalize have no lease or `d_final` bound (`5bc7524d`) and may run after `d_final`; the aggregate leaves out a
   member whose admission does not qualify at the call and answers 409 `ROUND_INFEASIBLE` if too few remain. With
   two auditors on one executor host a lease sees about `2 x T_audit`, which must stay under `lease_rounds x 3`
   seconds.
4. **R2** (if used): `exec_artifact_unavailable`, `_skipped`, a 403 on a part, or an upload rate under the plan;
   leftover objects under `exec/` after clean-up.
5. **Disk**: free space on s100 under 60 GB or growth of `trials-v2/` over 10 GB per trial; node A docker root
   under 25 GB over the image.
6. Before any multi-minute step, no Lium cycle open on node A or B (`cycle_gate.sh`).
7. Any `PENDING_REPLAY_MISMATCH`, `SUSPENDED`, `UNFUNDED` in a driver log: stop that identity.
8. **Replay**: a max warm replay of the warm-up run over 298 s: stop and report to Forge (Forge, 2026-10-28). At
   299 and 300 s the builder refuses the rebuild, which would be the warm-up run; above 300 s the manifest stage
   refuses it (section 4 steps 4 and 6). From 175 to 298 s is not a stop: the real run is opened with the gap its
   policy signs.
9. **Lost leaf**: a 503 `LEAF_OBJECT_LOST` to an auditor, or a `leaf-object-lost` record in `round_status.py`: the
   round is retired with no reward and nobody blamed (`_park_lost_leaves_v2`, `store.py:8423-8441`;
   `_retire_round_v2`, `:6949-6962`). Stop and report.

## 8. Clean-up

As S15 P5: stop the containers on node B, no `ht-exec-*` leftovers on node A, stop the auditors, `executor-off`,
revoke the ephemeral key and compare `authorized_keys` with the as-found copy, shred the seeds on node B, and
restore the as-found `relay.json`, the as-found six-hotkey `metagraph.json` and the auditors' as-found run id. Plus:
the warm-up run, if section 4 step 9 did not already clean it (its `ht-exec-*` leftovers, stage directory, ssh key
and executor config, with the listings in the report); shred the R2 token file (`r2.env`; `executor-off` removes
only the settings file that `executor --r2` wrote, `/data/executor/r2-artifacts.json`), list and delete objects
under `exec/` in the TEST bucket, revoke or rotate the token, `build_test_manifest.sh --clean $OUT` (and
`--clean $WARM` if still there), remove the scratch copies of the seeds. Leave the compute caches (image-bound) and
the warm-up's run store [?], and report their size.
