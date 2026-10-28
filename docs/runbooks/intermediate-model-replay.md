# Replay of the intermediate model (`dense-0.1b`) on the s100 TEST stack

100,676,352 parameters, `decoder`, H = J = 4, one GPU. Decision, budget and prerequisites a-f:
[ADR 0015](../adr/0015-intermediate-dense-test-model.md). Base procedure: [the TEST stack](../s100-test-stack.md)
section 6 and its Rollback section, and the S15 operator tools (`lib.sh`, `mkenv.sh`, `mk_exec_config.sh`,
`bgpu.sh`, `watchdog.sh`, `mk_origins.py`, `stage_seeds.sh`, `mk_miner_seeds.sh`; not in the repository, they sit in
Forge's `2026-10-27_s15-replay-s100-gpu/tools` and hard-code its worktree, ss58s, node addresses and key paths).
Labels: [V] verified, [A] reported, [H] estimate. **Nothing below was run.** Env vars appear by name only; never
print or store a value.

## 0. Go / no-go before anything is touched

The run is **not possible on the current code**. Start only when ADR 0015 prerequisites **(a)** lock-free
reference, **(b)** `copy_flags`, **(e)** lease/offsets with a second executor host, and **(d)** if (e) uses
H >= 31, are merged and in the images, and **(f)** is decided from section 2. (c) is not needed for the driver.
The sacrificial trials of section 4 settle whether the rest holds; they do not replace the prerequisites.

Server and miner come from one commit: since `e0969448` the owner routes `/challenge` and `/job` are signed
`POST`s, so a HEAD miner cannot talk to the S15 server image `a513821d` [V by reading]. A new commit gives new
image digests, hence a new run id and a cold compute cache.

Guards (S15, unchanged): TEST stack only; never touch `cortex-challenge-hypertrain`, `cortex-master-*`,
`hypertrain-beacon`, `hypertrain-auditor-go-*`, `hypertrain-auditor-0/1`, `/data/role-dispatch`, the host
`/data/runs` (production), nor `executor-*` containers on the rented nodes; nodes get the image and `--rm`
containers only, no checkout and no R2 token; no `HT_GATE_MAX_AGE_S`, second pusher or manual backfill; no
challenge or executor restart while a trial or round is open; one driver, never two; never `pkill -f` in an
ssh command; miners start only after `open` returned 200.

## 1. Prepare offline (no s100 change)

1. `gh attestation verify` of the server and the GPU digest; pre-pull the 17.35 GB GPU image on nodes A and B (a
   pull inside a reference is aborted); on s100 pull the server image, `snap.sh pre`, `challenge_image.sh backup`,
   `paper` (the backup directory is the rollback source and every gated command refuses without it).
2. Seeds: `stage_seeds.sh $OUT` **before** the build (otherwise the builder draws fresh owner/coord/auditor seeds
   and `relay_config.py` and `install` refuse the run). Six identities: five miners and one sacrificial (index 5).
   S15 stages five: generate the sixth hot and cold seeds, `NMINERS=6`, stage `cold-0..5`, and add **two** hotkeys
   (miners 4 and 5) to the stub metagraph (the join and `get_weights` need the hotkey registered; restart
   `master-stub`). Retarget `lib.sh` (worktree, `PY`, `OWNER`, `AUD0/AUD1`, `RELAY_K1`, node addresses, ssh key).
3. Build:

```bash
UV_OFFLINE=1 HT_D20_GEOMETRY=dense-0.1b HT_D20_COLDKEYS=6 HT_D20_RELAY_PUBKEY=<relay k1 ss58> \
  hypertrain/scripts/b300/d20/build_test_manifest.sh <GPU image digest> 595.91.07 148 $OUT
```

About 11 s [V]. It prints `"param_count": 100676352` and `"init_state_hash": "be9b7bd6…28f8"`; anything else means
the row or the init changed. Then, as in S15: `cuda_qualification.py build`, `relay_config.py` (check the as-found
`relay.json` hash first), `mk_exec_config.sh` on the run id of `$OUT/out/index.json`, `mk_origins.py`, the flat
stage directory. Executor config: S15 values (one host, `sudo -n docker`, `sweeps` 20, pause 15 s);
`max_result_bytes` stays 16 GiB (the result is 4.13 GB). Validate it the way the challenge loads it
(`ExecutorConfig.model_validate_json`, `docs/cuda-executor.md`): a bad file is a silent 503.
4. R2 only if (f) says so: settings = `hypertrain/docs/examples/cuda-executor-artifacts.example.json` with
   `credentials_file` = `/data/executor/r2.env` (0600, uid 65532, put by stdin; only `R2_ACCOUNT_ID`,
   `R2_ACCESS_KEY_ID`, `R2_SECRET_ACCESS_KEY`, `R2_BUCKET`, optionally `R2_ENDPOINT`) and `fallback` `fail`. The
   challenge reads the file named by `HYPERTRAIN_CUDA_EXECUTOR_ARTIFACTS`, which `challenge_image.sh executor`
   cannot set yet (prerequisite f). Validate with `ArtifactSettings.model_validate_json` and
   `load_credentials(...).bucket`, printing only the bucket name. The bucket needs the ADR 0013 rules.

## 2. Measure first (replaces the estimates; commands in ADR 0015)

1. After `p2_key` and `p2_stage_exec` (key and `known_hosts` only): ssh round trip and rate, both directions,
   challenge container to node A. Target `>= 60 MiB/s` as a necessary floor (`T_trial` is 267-311 s there), 100
   or more for margin; below about 43 the 297 s window cannot hold.
2. Node B to the relay: rate of a 1.7 GB PUT in 1 MiB parts and a 0.8 GB GET (`T_miner` is 93-137 s on it).
3. `df` on s100 (about 390 GB for 52 trials plus about 60 GB for a round [H]: nothing prunes `trials-v2/`), node A
   docker root (about 10 GB per job plus the image), node B `/mnt/nvme`; RAM on s100 (a reference reads a 1.2 GB
   state; the aggregation peaks at about 10-13 GB [H]).
4. Pusher cost per round `c` from the beacon log (2.4-2.7 s in S15) and the Lium cycle schedule on node A.
5. Optional, from a host of ours and **not** from the rented nodes: R2 and the profile's step time (ADR 0015).

## 3. Gated window and publication

As S15 P1 and P2: `swap`, `beacon` (0.2/60/100), `executor` in one gated window (a sealed seal younger than
30 min), then the ephemeral ssh key on node A, `cuda_executor_check` (`status ok`, driver 595.91.07, capability
`[10, 3]`, one idle GPU, `image_present`), `install` (must print `"test_only": true` before `run`),
`publish_run.py` steps `objects run dataset genesis status`, relay re-bind, auditors on the run, baseline
`beacon_gaps.py 1800`. The model is initialised by the builder (2.5-3.9 s), at each trial staging, at `open` and
in the aggregate, not by `run`.

## 4. Cache warm-up and the lock gate (G0), before any real identity

The first reference after a new GPU digest is cold (57 s on the tiny model [V]); it holds the store lock and, today,
costs the identity (S15 D1). The 101 M kernels may not be in the tiny model's cache [?].

1. Alone on the stack (no driver, no operator call, no open Lium cycle on A or B): `admit "5:3"` for the
   sacrificial identity with `HT_MINER_DEVICE=cuda` (`grep '^device'` on every `miner.toml`) and `HT_IMAGE_DIGEST`
   equal to the manifest digest. Three trials: the first cold, two warm.
2. Read `exec_attempt` to `exec_published` in the executor journal (`T_cold`, `T_warm`), `t_probe` (the `probe-v2`
   seconds of `stages.tsv` plus its waits), the first `proof-v2` POST, the 503 gate-closed time (`beacon_gaps.py`),
   and that `exec_cache_unavailable` is 0.
3. **G0 go/no-go.** With Part B the reference does not close the gate and the condition is the window:
   `t_probe + T_ref + 17 s <= 270 s` on the warm trials (the window is 297 s, 270 keeps a 27 s margin; estimate
   233-277 s at 105 MiB/s [H]). Without Part B the condition is `T_lock <= (270 - t_probe)(3 - c)/3`: 27-54 s at
   `t_probe` 0 and 24-48 s at 30 s, against 123-235 s [H]: **NO-GO**, stop, clean up, report.
4. Manual alternative without an identity (W2, untested): on node A as the ssh user, create
   `$HOME/.cortex-cuda-executor/compute-cache/<first 32 hex of the digest>` exactly as the executor does (`umask 077`,
   `mkdir -p`, owned by the ssh user, 0700), pass `cycle_gate.sh` for the chosen idle GPU, and run once
   `sudo -n docker run --rm --init --name ht-w2-<random> --gpus device=<idle> --ipc=host --mount
   type=bind,src=<dir>,dst=/root/.nv --entrypoint /opt/hypertrain/venv/bin/python <GPU image> -I -` with a short
   `train_round` of the profile on stdin (the image has no `scripts/`). Whether that fills the entries the island
   needs is not established (ADR 0012); the first real reference must show `T` near `T_warm`.

## 5. Admission and the round

- `admit "1:12,2:12,3:12,4:12,0:4"` with the watchdog (poll every 2 min or less; a lapsed identity cannot be
  re-seeded). 52 serial trials, about 3.4-5.6 h at 233-389 s [H], inside the 20000-round screen (16.7 h). After a
  partial `proof-v2` re-send the saved proof file, never relaunch `admit`. Check `df` after trial 1 and every 10.
- **Round offsets are computed, not copied.** A round opened once cannot be opened again with other offsets (409
  `conflicting round opening`), and `open` reads `now` before it builds the five roster anchors. Per component
  [H]: client build about 10 s a hotkey (54 s); the server open holds the lock about 14 s a roster entry (77 s),
  so the gate stays closed about 7.3 times that (about 560 s) and accept, commit and assignment wait for it;
  then five sequential `run-v2` of about 90 s; `d_audit` = `d_commit` + 2; `d_upload` after the last relay receipt;
  `d_final` = `d_audit` + the audit batches + the aggregate and its gate closure + finalize. In rounds of 3 s:
  `d_open >= ceil(build/3) + 30`, `d_assign >= d_open + ceil(8.3 x open_lock/3) + 10`, `d_commit >= d_assign +
  ceil(5 x T_run/3) + 20`. `d_final - d_audit` must be at most 200 (`live_operator` cap) and hold
  `ceil((batches x T_audit + aggregate + closure + finalize)/3)`, which at H = 4 exceeds both 200 and the lease;
  that is prerequisite (e). Open only with a vector that `check_open_shape` accepts and the measurements support.
- Before `audits`: `round_status.py` shows commit, leaves, accept, assignment and start equal to 5, no lease, and
  the rounds `d_assign` and `d_audit` present in `beacon_gaps.py`. Then `audits` once between `d_audit` and
  absolute - 2; `finalize` immediately after the last MATCH (verdicts expire with their lease); wait 60 s for
  vesting; `weights_once.py` once. Expected: 200, five hotkeys equal (4 x 250000 if one miner was excluded).

## 6. Expected timings (101 M, [H] unless marked; replace with the section 2 and 4 measurements)

| Step | Estimate | Tiny, measured |
| --- | --- | --- |
| build of the manifest | 11 s [V] | 8 s [V] |
| cold first reference / warm reference | 123-235 s ssh-bound, plus about 44 s cold | 57 s / 13 s [V] |
| trial (miner + reference) | 233-389 s | 145 s [V] |
| `run-v2` per miner | about 90 s | 24 s [V] |
| audit | 144-256 s each | 14 s [V] |
| aggregate (two passes) + finalize, five miners | 372-454 s + 186-227 s | seconds |
| result / inputs / delta | 4.13 GB / 805 MB / 102 MB [V sizes] | kilobytes |

## 7. STOP conditions

The tiny-run rules stand: no beacon or gate intervention during a run, a lag over 2 rounds for 60 s, the same
stage failing three times, `exec_host_refused`, a driver exit not understood (read its `SystemExit`; relaunch the
identical `admit` only if at least the measured `T_trial` in rounds, about 90-130, remains in the stored
challenge). Added for this size:

1. **Lock** over 150 s (the `beacon` command writes only catch-up 60) or any `missing`/`stranded` gap: stop.
2. **Window**: two consecutive trials over 270 s, or any `409 envelope expired`: stop.
3. **Audits and tail**: a go/no-go at `open`, not at `audits`: the vector must satisfy `d_final - d_audit >=
   ceil((batches x T_audit + aggregate + closure + finalize)/3)` in rounds, and `batches x T_audit + aggregate +
   finalize <= lease_rounds x 3` seconds from the first lease, with `T_audit` from the sacrificial trials scaled
   by section 6. With two auditors on one executor host a lease sees about `2 x T_audit`.
4. **R2** (if used): `exec_artifact_unavailable`, `_skipped`, a 403 on a part, or an upload rate under the plan;
   leftover objects under `exec/` after clean-up.
5. **Disk**: free space on s100 under 60 GB or growth of `trials-v2/` over 10 GB per trial; node A docker root
   under 25 GB over the image.
6. Before any multi-minute step, no Lium cycle open on node A or B (`cycle_gate.sh`).
7. Any `PENDING_REPLAY_MISMATCH`, `SUSPENDED`, `UNFUNDED` in a driver log: stop that identity.

## 8. Clean-up

As S15 P5: stop the containers on node B, no `ht-exec-*` leftovers on node A, stop the auditors, `executor-off`,
revoke the ephemeral key and compare `authorized_keys` with the as-found copy, shred the seeds on node B, and
restore the as-found `relay.json`, the as-found six-hotkey `metagraph.json` and the auditors' as-found run id. Plus:
shred `r2.env` and delete the artifacts settings file on s100, list and delete objects under `exec/` in the TEST
bucket, revoke or rotate the token, `build_test_manifest.sh --clean $OUT`, remove the scratch copies of the seeds.
Leave the compute caches (image-bound) and report their size.
