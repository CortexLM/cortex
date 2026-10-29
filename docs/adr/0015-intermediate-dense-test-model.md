# ADR 0015: the intermediate dense TEST model (100,676,352 parameters)

- Status: draft, 2026-10-28 (Principal SN100 D68). Forge decides promotion to `v3`. Written for branch
  `v3-model-intermediate` on origin/v3 `2cfc2a2b` (merge `350e50b1`), then merged into `v3-last-merges` at
  `04efdac6`. Now anchored on the `v3-last-merges` tip (D89, 2026-10-28), no longer on `2cfc2a2b`/`350e50b1`:
  the code anchors, hashes and numbers below were read or measured at `faa6abb4`, whose `hypertrain/src` is
  that of `ea0f11fc` [V]; the builder's anchors are those of `f84dd57c`, which adds the warm-up refusal (Forge,
  2026-10-28) and changes no manifest [V]. Nothing here changes a production run, master, profile, pin or emission, and no
  file under `hypertrain/src/` changes in this lot (the merge `04efdac6` touches none) [V]. Nothing in this
  batch ran on a GPU host, on s100, against R2 or on-chain.
- Order: brief `v3-model-intermediate`. Forge (CTO) chose the size on 2026-10-28, within the D68
  band (see "Decision"). The merge `04efdac6` adapted the builder to the tip (D84/D88; Forge, 2026-10-28,
  through the relay tool; see Decision 2). The number 0015 is free on the tip: `docs/adr/` has no other
  0015 and `docs/index.md` links this record [V].
- Labels: **[V]** verified in code or by a command, at the tip unless a commit or a session (S15, S3) is
  named, **[A]** reported by a cited document or session, **[H]** hypothesis or arithmetic, **[?]** unknown.
- Related: ADR 0006 (kimi-tiny, per-arch registry), 0007 (SM counts, hash tables), 0008 (generic
  CUDA executor), 0010 (signed owner routes), 0011 (audit integrity, environment pins; Part 9: the
  re-audit window `reaudit_rounds` and the manifest gap cap `gap_cap_rounds`, D84/D88), 0012
  (executor performance, Part B lock-free reference), 0013 (executor results through R2), 0014
  (escrow operation ids, keyed origin ids).

## Context

The tiny-cpu replay (S15, 2026-10-27) took 824 parameters through join-v2, probation, run-v2, CUDA
audits and `get_weights` on the s100 TEST stack. D68 asks for the next proof to be a dense model of
100-300 M parameters (target about 200 M), with its artifacts through the R2 TEST bucket, and the
1 B model only if the audit window holds.

What exists at the tip (the counts are unchanged since `8c2b24e8`; this lot only added the row
`mfu-dense-0.1b`) [V]:

| Model | Parameters | Where | Used for |
| --- | --- | --- | --- |
| tiny-cpu | 824 | builder geometry `tiny-cpu` | S15 replay on B300 [A, S15 report] |
| kimi-k3-pro-tiny | 77,001,376 [V] | `models/kimi_tiny.py:67` `PROFILE`; `models/registry.py` names the arch | CPU unit tests only. The GPU runs of S3, S9 and S12 used `MICRO` (73,200 parameters): the PROFILE cannot ride the local harness (`gpu_run_v2.py:44-48`) [V by reading] |
| od-base TEST | 123,753,984 [V] | `od-encoder` arch, 2 ranks | joinbase-6, one host with 4 x RTX 5090 (2 per island), 63 s reference [A] |
| mfu-dense-0.35b | 350,274,560 [V] | `scripts/mfu_profiles.json` | MFU only |
| dense-1b D20 | 889,260,032 [V] | builder geometry `dense-1b`, row `mfu-dense-1b` | S3 qualification, 2 rounds in 320 s [A]; a state object of 10.67 GB cannot complete [A, ADR 0013] |

The dense configs of the local "substitute qualification" runs (S12) are the one-layer fixture of
`mine_multi_round_v2` and S3's dense-1b triples; neither is a 100-300 M network run. There is no
model-profile registry in `src`: `ARCH_REGISTRY` maps an architecture name to a module, and a dense
config is a geometry of the `decoder` architecture. The only registry of dense geometries is the
profile file of the MFU tools (`hypertrain/scripts/mfu_profiles.json`, read by `run_mfu.py`; a stale
duplicate, `hypertrain/experiments/mfu/profiles.json`, is read by nothing and left alone). The D20
builder's `dense-1b` geometry is a literal copy of its row `mfu-dense-1b` (`build_test_manifest.sh:267-271`;
the reference spec names the row as its source); `dense-0.1b` is the first geometry that reads the file
(`:295-296`) [V].

## Decision

D68 (Principal SN100) gave the 100-300 M band and "intermediate first", not the exact size. Forge
(CTO) chose the size on 2026-10-28, within that band, in answer to the question this batch asked
through the relay tool: freeze **100,676,352 parameters** as the single profile, not the 209 M
target, because a run that fails for code reasons proves nothing. The budget below was computed at
`2cfc2a2b`: there this size failed, and it could pass only once prerequisites (a), (b) and (e) landed.
On the tip (a) and (b) are merged and (e) is in part (the TEST gap cap of D88) [V by reading]; with one
executor host the five audits still do not fit their 600 s, so a run still needs the second executor host
of (e) and a result path the measured link allows (f) [H] ("Prerequisites"). The budget has not been
recomputed on the lock-free code [?].

1. **Profile row `mfu-dense-0.1b`** in `hypertrain/scripts/mfu_profiles.json`: arch `decoder`, 8
   layers, d_model 768, 12 heads, 4 kv heads (head dim 64), d_ff 2048, one expert, vocab 32768,
   seq_len 2048, bf16. Shape-only count **100,676,352** [V, `param_shapes`, printed by the builder
   as `param_count`]. It reuses the dense decoder path unchanged: no new architecture and no
   `ARCH_REGISTRY` row. "Registry entry" is read as this row; the builder takes the model block from
   it. Another rung (126 M, 209 M) is a new row plus a small builder edit (the geometry name in the
   case list, the row key, the epoch rule, the measured-replay rule and the re-audit block of the audit
   policy, the dry-run expressions) and its pinned count in the tests.
2. **Builder geometry `HT_D20_GEOMETRY=dense-0.1b`** in `hypertrain/scripts/b300/d20/
   build_test_manifest.sh` (the brief names `scripts/ops/s100_test/build_test_manifest.sh`, which does
   not exist). Inner loop H = J = 4, micro_batch 2, grad_accum 1, `state_policy = reset`, AdamW,
   peak lr 3e-3 with warmup 2, codec dense-int8 at full density (the example manifest's outer block),
   layout `n_gpus 1`, the 4096-sample u32 fixture of `dense-1b` (33.6 MB); the B300 arguments (148 SM,
   driver 595.91.07) are the builder's. SM_COUNT also sets the environment pin that every manifest now
   carries (`reference_spec.env.pinned`, required since DET-01, `038c80bc`): CPU class `avx2` and the GPU
   class measured on that count, 148 -> `sm_103` (170 -> `sm_120`; any other count stops the first stage),
   with `HT_ATTN_BACKEND` from `HT_D20_ATTN_BACKEND`, default `math` (`build_test_manifest.sh:120`,
   `:222-233`, `:383`); the dry run checks and prints the pin [V]. It refuses a row whose shape-only
   count leaves 100,000,000..300,000,000 (inclusive) before any manifest file is written (the failed-run
   cleanup removes the seeds and directories the run created).
   - **J = H.** A result keeps `H/J + 1` checkpoints of 12 P bytes each, and `validate_artifacts`
     requires checkpoints 0 and H; J = H gives the fewest (two), so S = 41.016 P instead of 53.016 P
     at H/J = 2. Compute is seconds, not milliseconds: about 1 s a step in math attention [H], 3.9 s
     at H = 4, small against the transfers. H = 4 stays under the 30 steps of the 100-round audit
     lease (`lease_rounds` 100 is a literal of the builder's audit policy). Prerequisite (e) changes
     H and with it the budget; see "Budget at H >= 31".
   - **`work_screen_epoch_rounds` 20000** for this geometry only (the others keep 6000, so the
     tiny-cpu and dense-1b run ids are unchanged). `hypertrain/docs/network-v2.md` requires the first
     screen to cover the whole serial campaign: 52 trials of 83-130 rounds (250-390 s) plus about 470
     rounds to the `d_final` of round 0 (the document's worked example, 4106 rounds, minus its 36 trials)
     is 4,800-7,200 rounds against 6,000, so 6,000 holds only on the fastest link. The value is part of
     the admission policy hash, hence of the run id, and it is also the window of the rotation budget
     and the IP-prefix quota epoch (`now // work_screen_epoch_rounds`), which refill every 16.7 h
     instead of 5 h.
   - **Measured replay, re-audit window and gap cap** (ADR 0011 Part 9, D84/D88; decided by Forge on
     2026-10-28 through the relay tool, merge `04efdac6`). `HT_D20_REPLAY_SECONDS` is required for this
     geometry, with no default, and refused for the others: exit 2 before anything is created
     (`build_test_manifest.sh:122-138`) [V]. It is the measured warm audit replay on the auditing B300 in
     whole seconds, the max of at least three warm replays from the executor journal (runbook section 4),
     never an estimate of this record. The audit policy signs
     `reaudit_rounds = reaudit_rounds_for(replay, 100)` (twice the replay in 3 s rounds, never below the
     lease) and, when `reaudit_gap_needed(100, T)` exceeds `DEFAULT_GAP_CAP_ROUNDS` = 200,
     `gap_cap_rounds` = that gap (`:432-439`; `protocol/messages_v2.py:723-733`) [V]. Computed with the
     tip's functions [V]: 144 s gives T 100 and a gap of 175, which the default cap holds; 174 s gives
     T 116 and 199, the largest replay that fits 200; 200 s gives 134 and 226; 256 s 171 and 276; 300 s
     200 and 305. 300 s is the ceiling: at 301 s the window of 201 rounds passes two leases and
     `validate_training` refuses it in the manifest stage (`messages_v2.py:767-768`) [V]. The cap is the
     manifest's (D88): a cap above 200 is for a TEST manifest only, `validate_training` refuses a
     production manifest that lifts it (`messages_v2.py:774-777`), and the builder's economics policy is
     `test` (`:406`). `live_operator open` takes its cap from the audit policy's `gap_cap()` and refuses a
     gap below `reaudit_gap_needed` (`scripts/ops/live_operator.py:130-191,213-215`) [V]. The dry run
     prints an `audit` block (`lease_rounds`, `reaudit_rounds`, `gap_needed`, `gap_cap`) and checks the
     window and the cap (`:529-537,585-593`) [V]. The tests are listed under "Tests and mutants". No warm
     replay of this model has been measured, so no real build exists yet [?].
3. **Deterministic init** is the existing decoder init (Philox per tensor index, seed 0, std 0.02).
   The state hash of the initial parameters, which the builder signs as `init_state_hash`, is
   `be9b7bd63f79e4d7a3e640f51e9cfcc03e445a9de9ab249bcf77d938b8ee28f8` [V]; two inits are bitwise
   equal (2.5-3.9 s each, 1.65 GB peak RSS with the torch import) and the pin holds under the
   AVX2-capped dispatch CI uses.
4. **No `src/hypertrain` change in this lot** [V: the merge `04efdac6` touches no file under
   `hypertrain/src`]. Part of the code the run needs has since landed on the tip from other lots:
   prerequisites (a) and (b), and the manifest gap cap of (e). The rest is listed under "Prerequisites";
   this lot documents it instead of widening the scope.

### What the builder writes (for the later operator)

`OUT/out/`: `index.json` (`run_id`), `manifest-envelope.json`, `manifest.json` (its sha256 is the run
id), five `policy-*.json`, `relay-registry.json`. `OUT/inputs/`: `training.json`, `policies.json`,
`relays.json`, `reference-spec.json`, `public-keys.json`, `genesis-allocation.json`, `exp-round.txt`,
`samples.u32`, `proofs.json`; plus `secrets/` and `state/`. They are the files of the `tiny-cpu`
build, with the same consumers (see the runbook). A run id depends on every field of the manifest. The
builder draws random seeds unless they are staged, so every fresh build has a new id. For this geometry the
id also depends on the replay, but only through the audit policy: the builder reads `HT_D20_REPLAY_SECONDS`
only to sign `reaudit_rounds = reaudit_rounds_for(replay, 100)` and the cap that window needs
(`build_test_manifest.sh:432-439`) [V by reading]. The stable facts of this geometry are `param_count`
100,676,352, `init_state_hash` above and the admission and aggregation policy hashes. The audit policy hash
is stable for every replay that gives the same `reaudit_rounds`, not for one value only: every replay from 1
to 150 s signs T 100 and no cap, and 299 s and 300 s both sign T 200 and cap 305 [V, computed with the tip's
functions]. The economics and dispute policies name the coord, referee and cold keys, so their hashes move
with the seeds [V by reading `build_test_manifest.sh`]. Against the tiny build, the admission policy differs
in the epoch (20000) and the audit policy in `max_steps_per_attempt` (2 H = 8), in `reaudit_rounds`, which it
always signs, and above 174 s in `gap_cap_rounds`. Two builds over the same staged seeds and `EXP_ROUND`, such
as the warm-up at 300 s and the rebuild at the measured replay, differ at most in the audit policy, hence in
`audit_policy_hash` and the run id (`test_the_rebuild_with_the_measured_replay_changes_only_the_audit_window`,
298 s against the warm-up's 300 s) [V]. When the two replays give the same `reaudit_rounds` (a measured max of
299 or 300 s against the warm-up's 300 s), the two manifests would be identical and so would the run ids: the
manifest holds only `manifest_version`, `training` and `network` (`messages_v2.py:278-284`), and `EXP_ROUND`
enters only the envelope (`ops/manifest_v2.py:128`) [V by reading]. So the runbook stops above 298 s (Forge,
2026-10-28), and the builder, given the warm-up's `OUT_DIR` in `HT_D20_WARMUP_OUT`, refuses (exit 2) a real
build whose manifest is the warm-up's; its dry run then checks that only the audit policy differs and prints the
warm-up's run id and the stop (`stop_above_seconds` 298) [V].

## Budget

Sizes. The first three rows were measured from a real serialization
(`tests/models/test_dense_intermediate_round.py` trains two steps at full size and serialises them)
[V]; the rest follow from the formulas of ADR 0013 [H, validated there at 52.8 M]:

| Object | Bytes | Per parameter | Label |
| --- | --- | --- | --- |
| θ, and the error-feedback file | 402,712,448 | 4 P + 7 KB header | V |
| state: θ, m, v, step | 1,208,136,936 | 12 P + 21 KB | V |
| delta, dense-int8 | 102,253,376 | 1.01565 P (independent of H and J) | V |
| job inputs, `reset` (θ and ef_in) | about 805 MB | 8 P | H |
| what a miner PUTs per trial (state, ef, delta, leaves) | about 1.713 GB in about 1,640 parts of 1 MiB | 17.016 P | H |
| result tar of an executor job, `S = (17.016 g + 12 (H/J + 1)) P`, g = 1 | about 4.13 GB at H/J = 1 | 41.016 P | H (components V) |

`S` is 5.34 GB at H/J = 2 and 7.75 GB at H/J = 4. Every object is far under the protocol ceilings:
the state is 1,153 parts of 8,192 (the ceiling is P <= 715.8 M with AdamW), the delta 25 relay chunks
of 4 MiB, and `max_result_bytes` / `max_artifact_bytes` keep their 16 GiB defaults [V].

Times. Only the rows marked V were measured; every other number is the output of the formulas below
and must be replaced by the measurements of the first GPU run.

| Quantity | Value | Label |
| --- | --- | --- |
| warm executor attempt on the tiny model (fixed part) | 12.8-14.6 s | V (S15) |
| first reference after a new GPU image digest | 57.3 s; in S15 it held the store lock, at the tip only its claim and settlement do (ADR 0012 Part B) | V (S15); lock release V by reading |
| B300 math-attention cost, b4 h16 T2048 hd128, one layer | 243-260 ms; flash 1.75 ms | V (S3); hd 64 not measured |
| CPU init of the profile | 2.5-3.9 s; real-size round (2 steps, 1 x 32 tokens, replay included) 19-27 s, 5.8 GB, one thread | V (this host, load 90-200) |
| gate stays closed after a store lock of T seconds | about 7.3 T (63-147 s after 13 s) with S15's ascending pusher; the tip's head-first pusher (`beacon/push.py`) is modelled at <= 7.7 s after a lock of 13-150 s on the test ledger | V (S15), extrapolated H, for the old pusher; H (ADR 0012 model) for the tip, not measured live |
| `validate_artifacts` 125-133 ns per parameter; challenge CPU 8.9 s/GiB (reference), 10.9 s/GiB (audit) | 13 s per pass; 34 / 42 s on S | A (ADR 0013, EPYC 9575F; s100 is a Zen 3) |
| `copy_flags` per delta / per pair at 208.7 M (P log P), scaled | 28-34 s / 4.6-5.8 s | H (measured at 4-32 M; 54 s for two deltas and a pair at 100.7 M in review) |
| ssh link A to s100 | 105 / 30 / 10 MiB/s at 10 / 50 / 150 ms RTT; 50 MiB/s interpolated; measured between s100 and a node: 7 MB/s | A (ADR 0013, emulated; `docs/s100-test-stack.md` 6.4a, measured); RTT never measured |
| miner PUT of 1 MiB parts / GET of an object | 29-114 MB/s / 88 MB/s | A (joinbase-6 bandwidth, other hosts) |
| R2 | from a node: 20 MB/s on one stream, 100-144 MB/s on eight; s100 to R2 not measured; 18.8 up / 36.7 down MB/s from one non-GPU host | A (`docs/s100-test-stack.md` 6.4a; ADR 0003) |

Formulas ([H], `k = P / 100,676,352`, link `r`, `S` and the inputs `I = 8 P` in bytes):

- reference (since ADR 0012 Part B only its claim and settlement hold the store lock; this is its length,
  which bounds the trial window): `T_ref = stage 10k + I/r + (13 + gpu + 14k) + S/r + 8.9 S[GiB] + 3`, where
  `gpu = H x layers x (mb/4) x 0.245 s` (math attention, 3.9 s here);
- miner side of a trial: `T_miner = 10 + I/88 MB/s + 15 + gpu + 14k + 2 x 13k + 17.016 P/(29..114 MB/s)`;
  `T_trial = T_miner + T_ref + 17k` against the fixed 297 s window (`challenge_rounds` 100; ADR 0017
  prepares a wider TEST window, `challenge_rounds_for(T_trial)`, for the founder to decide);
- audit: `T_audit = 12k + I/r + (13 + gpu + 14k + 8k) + S/r + 10.9 S[GiB] + 6k`; five audits in series on
  one executor host against at most 600 s (the first window of every audit ends at
  `absolute = min(d_final, d_audit + 2 x lease_rounds)`, 200 rounds at H <= 30, `challenge/store.py:8155`;
  `live_operator open` caps `d_final - d_audit` at the audit policy's `gap_cap()`, 200 unless a TEST
  manifest raises `gap_cap_rounds`) and 300 s per lease;
- aggregation: `copy_flags` passes (two in `aggregate`, one in the `finalize` replay), outside the store
  lock at the tip, `n x delta + n(n-1)/2 x pair` each.

The constants carry two known errors in opposite directions: the audit path has six
`validate_artifacts` passes at the tip (call sites in `cuda_executor.py:1012,1033`, `auditor/worker.py:191`,
`island_bisect.py:182`, `store.py:9435,9807`) [V by reading], ADR 0013 counted four (up to 25 s more per
audit); and the 125-133 ns per parameter was measured on a result of 70 bytes per parameter, so a result of
41 bytes per parameter validates faster (7.7 s instead of 13 s a pass).

Result for H = J = 4 (seconds; the arithmetic is `budget.py` in the evidence folder
`livrables/forge/2026-10-28_model-intermediate/`):

| Link (ssh) | Model | T_ref | T_trial | T_audit | 5 audits | gate closed |
| --- | --- | --- | --- | --- | --- | --- |
| 105 MiB/s | **100.7 M (profile)** | 123 | 233-277 | 144 | 718 | 898 |
| | 125.8 M | 151 | 283-338 | 177 | 887 | 1100 |
| | 208.7 M | 235 | 435-526 | 282 | 1408 | 1719 |
| 50 MiB/s | **100.7 M** | 172 | 282-326 | 193 | 965 | 1258 |
| | 125.8 M | 212 | 345-400 | 239 | 1195 | 1550 |
| | 208.7 M | 338 | 537-628 | 384 | 1919 | 2465 |
| 30 MiB/s | **100.7 M** | 235 | 345-389 | 256 | 1279 | 1716 |
| | 125.8 M | 291 | 423-478 | 317 | 1587 | 2123 |
| | 208.7 M | 468 | 667-758 | 514 | 2569 | 3415 |

The last column is `7.3 x T_ref`: the lock-holding reference and the ascending pusher of `2cfc2a2b` and
S15. At the tip the reference holds the store lock only for its claim and settlement (ADR 0012 Part B) [V
by reading], so the column no longer applies; it is kept for comparison with S15. T_ref still counts in
T_trial.

Reading [H]:

- **Lock and gate.** S15 measured, with the lock-holding reference and the ascending pusher of that time,
  a gate closed for about 7.3 T after a lock of T. A trial then survived a lock of at most
  `(297 - 26)/(1 + 7.3)` = 33 s (23 s at the worst factor of 11; the S15 model gives 27-54 s at a zero
  probe), and one identity stranded at 57 s (V, S15); a lock-holding reference of 123-235 s would strand
  every identity of this size. The tip removes both causes. The remote reference holds the store lock only
  for its claim and its settlement (ADR 0012 Part B, `trial_reference_v2`, `challenge/store.py:3550-3682`)
  [V by reading]; the claim held it 3-9 ms at 100,676,352 parameters on a fake executor [A, ADR 0012]. The
  beacon pusher posts the newest round first (`beacon/push.py:101-132`), modelled to reopen the gate within
  7.7 s of a lock of 13-150 s on the test ledger [H, ADR 0012 section 3]. Neither change has run on a GPU
  host or on s100; the replay must show the gate staying open [?].
- **Trial window.** With Part B, now on the tip, the profile fits 297 s only on a fast link: `T_trial`
  crosses 297 s at 43 MiB/s (fast miner PUT) or 73 MiB/s (slow PUT), and 270 s, the runbook's go/no-go, at
  58 or 125 MiB/s. 101 M is the largest row of the band whose best-link, lower-bound case fits at all
  (233-277 s); 126 M has 283-338 s and 209 M fails everywhere. The size is a hypothesis to confirm
  with the first warm trials.
- **Audits.** Five in series need 718 s at best against at most 600 s. The first window of each audit ends
  at `absolute = min(d_final, d_audit + 200)` at H <= 30 (`challenge/store.py:8155`), however wide the gap, and
  a verdict must be accepted inside its own 300 s lease. A second executor host relieves the 600 s and the
  2 x T_audit use of a lease on a shared host; a roster of four relieves only the 600 s, and only on a fast
  link (4 x 144 s = 576 s at 105 MiB/s, 4 x 193 s = 772 s at 50 MiB/s). Since D84/D88 the gap must also hold
  the re-audit window of a withheld MISMATCH: `live_operator open` refuses a gap below
  `reaudit_gap_needed(100, reaudit_rounds)` (`scripts/ops/live_operator.py:170-191`): 175 rounds at a 144 s
  replay, 199 at 174 s, 226 at 200 s, 276 at 256 s, 305 at 300 s (Decision 2) [V]. A gap below 200 also
  shortens `absolute`: at 175 rounds the five audits have 525 s. A raised `gap_cap_rounds` does not lengthen
  the first window; the rounds past it serve only the re-audit window (`reaudit_until`,
  `challenge/exit_v2.py:162-167`) and the one re-arm of an exhausted job (`_rearm_audit_job_v2`,
  `store.py:8771`). A re-audit that fails drops only its member, as INFRASTRUCTURE (`REAUDIT_FAILED`,
  `store.py:8673-8684`) [V by reading]. The lease no longer bounds the rest of the chain on an ordinary
  run (no `service-admission` record, the only kind `live_operator` serves): since `5bc7524d` the ledger
  judges the stored commit and verdicts at the beacon of their `accepted_v2` row and the coordinator's audit
  challenge at the `created` beacon of its audit lease, and `live_operator finalize` seals the Finalize
  until `max(d_final, now) + 1000`, so the aggregate and the finalize may follow the last verdict [V by
  reading, `escrow_v2.py:503-517`, `store.py:2570`, `live_operator.py:342-351`]. At `8c2b24e8` they had
  to end inside the lease (about 285 s at H <= 30).
- **Aggregation no longer holds the lock.** At `2cfc2a2b`, `aggregate_v2` and `finalize_v2` computed under
  the store lock (327-423 s and 177-223 s of one hold at this profile [A, `hypertrain/CHANGELOG.md`]). At
  the tip, `aggregate_v2` claims under the lock, makes and replays the tape without it and settles under it
  (`_aggregate_flight_v2`, `challenge/store.py:10412`; 409 `AGGREGATE_STALE` if the round moved), and
  `finalize_v2` replays the applied tape before its transaction (`_prime_finality_replay_v2`,
  `store.py:2694`); it replays under the lock only when that verdict is missing, over 120 s old or its
  records changed [V by reading]. So (b) is met by taking `copy_flags` out of the lock; it still runs three
  times, two passes in the aggregate (372-454 s) and one in the finalize (186-227 s) [H]. The aggregate call
  stays under `live_operator`'s 600 s client timeout (`live_operator.py:63`), a second caller waits at most
  600 s (`AGGREGATE_WAIT_SECONDS`, `store.py:320`), and no verdict horizon bounds the aggregate or the
  finalize on an ordinary run (previous bullet). A late aggregate adds one exclusion to the missing-delta
  one: a member whose admission no longer qualifies at the call (`_exclude_unfit_members_v2`,
  `store.py:8008`).
- **R2.** For an audit result the transfer legs take 334 s at 18.8/36.7 MB/s, 90 s at 100/88, 18 s at
  500/500, against 131 s over ssh at 30 MiB/s. R2 beats ssh above 231 / 107 / 64 / 21 MB/s each way
  against ssh at 105 / 50 / 30 / 10 MiB/s. Fitting the 297 s window at 30 MiB/s ssh needs 102-221
  MB/s each way, and the reference over R2 needs only `reference` in `operations` (Part B is on the tip;
  the default stays `["audit", "referee"]`, `gpu_ops/artifact_transfer.py:85`, and
  `challenge_image.sh executor BACKUP PATH --r2 FILE --operations LIST` sets it); only scenario C of
  ADR 0013 (500/500 [?]) does.
- **Disk.** A result needs `2.2 S + 1 GiB` free on the challenge (R2); nothing prunes `trials-v2/`:
  `8P` inputs written, `8P` copied into `published/`, `S` and `17 P` of objects, 74 P = 7.45 GB a
  trial, so 52 trials keep about 387 GB on s100 [H].

### Budget at H >= 31

Prerequisite (e) can only lengthen the lease through H (`audit_lease_rounds`: 100 rounds for H <= 30,
else `ceil(2 H b / 3) + 100` with the step bound `b` at most 16 s: 431 rounds for H = 31, 442 for
H = 32). The same formulas at H = 32 on one executor host and 105 MiB/s, trace cost scaled with J
and the hash passes with the checkpoints [H]:

| H, J | attention | S | T_ref | T_trial | T_audit | 5 audits | + aggregate (2 passes, 414 s) | lease (b = 16 s) |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 32, 32 | math | 4.13 GB | 150 | 288 | 227 | 1135 | 1549 | 1326 |
| 32, 32 | flash | 4.13 GB | 119 | 226 | 196 | 980 | 1394 | 1326 |
| 32, 16 | math | 5.34 GB | 173 | 320 | 220 | 1101 | 1515 | 1326 |
| 32, 16 | flash | 5.34 GB | 142 | 258 | 189 | 946 | 1360 | 1326 |
| 32, 8 | math | 7.75 GB | 219 | 385 | 255 | 1273 | 1687 | 1326 |
| 32, 8 | flash | 7.75 GB | 188 | 323 | 224 | 1118 | 1532 | 1326 |

The "+ aggregate" column no longer decides anything, and the lease column bounds one lease only: at
`8c2b24e8` the aggregate and the finalize had to end inside the first verdict's lease, and the chain
of five audits and the aggregate (1,360-1,687 s) did not fit 1,326 s; since `5bc7524d` (see "Audits"
above) only the audits must end, each inside its own lease and all before `absolute` =
`min(d_final, d_audit + 2 x lease_rounds)`, which is 884 rounds (2,652 s) at H = 32 [V by reading,
`messages_v2.py` `audit_lease_rounds`, `store.py` `schedule_audits_v2`]. Five audits in series need
946-1,273 s on one executor host against those 2,652 s, and one lease sees about 2 x T_audit
(378-510 s) against 1,326 s [H]; the aggregate (about 414 s) and the finalize are bound by neither.
`live_operator open` caps `d_final - d_audit` at the audit policy's `gap_cap()`: 200 rounds (600 s) unless
a TEST manifest signs `gap_cap_rounds` (D88). `validate_training` refuses a cap above 200 for a production
ledger, and any cap above 4 x `lease_rounds` plus the re-audit window; `live_operator open` also refuses a gap
too short for the re-audit window of a pinned `reaudit_rounds` (D84) [V, `live_operator.py:130-191,213-215`,
`messages_v2.py:723,752,772-777`]. So the 884-round `absolute` at H = 32 can only be reached on a TEST
manifest with `gap_cap_rounds` >= 884; a pinned `reaudit_rounds` is never below one lease (442 rounds), and
that needs a gap of at least 688 rounds [V, `reaudit_gap_needed(442, 442)` at the tip]. The builder still
signs `lease_rounds` 100, so H >= 31 still needs (e). Math attention still pushes the trial to 288-385 s,
over the 270 s go/no-go everywhere and over the 297 s window at J = 16 and 8, so (d) is still needed at
H >= 31. Whether a second executor host is still needed at H >= 31 is no longer shown by the lease
arithmetic, which was the only reason given here; Principal decides it, see (e). Without (d) the profile
can only be run at H = 4 for the trial window, and there it needs a second executor host (or the roster of
four at 105 MiB/s), because the 600 s `absolute` cannot hold five audits on one host. This is the honest
state of the profile: it is the band floor and it sits at the edge of the pipeline.

## Prerequisites

At the tip, other lots have merged (a) and (b), (f) has its tooling and (e) has its TEST gap cap; each row
gives the status. (a) has not run on a GPU host or on s100 [A, ADR 0012], and nothing here shows that the
others have [?]. With five members on one executor host the profile is still not expected to pass a run:
five audits need 718 s at best against the 600 s `absolute` [H], see (e). Each item is separate, with an
owner proposal, a size estimate [H] and whether it is needed at 101 M. This lot adds none of it (no
`src/hypertrain` change).

| # | What | Where | Owner / size | Needed at 101 M |
| --- | --- | --- | --- | --- |
| a | Lock-free remote reference (ADR 0012 Part B): release `store._lock` during the executor job | **Merged** (`25f58756`, through `v3-integration-final` `389c45b4`). `trial_reference_v2` takes the lock for the claim, then stages, launches and puts with no lock, then takes it again to settle (`challenge/store.py:3550-3682`; `plan_trial_v2` `:3204` plans the staging under the lock and stages after it); `admission.py` `claim_reference`, `run_reference`, `settle_reference` [V by reading] | Forge, `v3-executor-lock-release`; done | **Yes, and done.** Before the fix, a T_ref of 123-235 s under the lock closed the gate for 900-1700 s, and a trial tolerates about 33 s. Now the claim's longest hold at 101 M is 3-9 ms with a fake executor [A, ADR 0012]. Whether the gate stays open on s100 is for the replay to show [?]. |
| b | `copy_flags` three times under the lock in aggregation and finalize | **Merged as an out-of-lock fix** (`e2f47116`, `v3-aggregate-lock-release`). `aggregate_v2` claims under the lock, computes `make_tape` and `replay_tape` with no lock (`_aggregate_compute_v2`, `store.py:10520`) and settles under it; `finalize_v2` replays the tape before its transaction (`_prime_finality_replay_v2`, `:2694`) and uses that verdict only if it is at most 120 s old and its records are unchanged, otherwise it replays in the transaction as before (`ledger/escrow_v2.py:559`). `copy_flags` itself is unchanged (`aggregator/core.py:345-369`, called at `weighted_v2.py:225` through `tape_v2.compute_body`) [V by reading] | aggregator session; done; moved `implementation_hash` | **Yes, and done.** Before the fix, at 101 M, the aggregate's tape held the lock 327-423 s and the finalize's replay 177-223 s, each in one hold [A, `hypertrain/CHANGELOG.md`]. The CPU time is the same as before, and the replay is still a recomputation (the tape stores `copy_suspicion` and `replay_tape` compares), not a cached result. |
| c | Miner HTTP client timeout 120 s while the proof POST holds the whole reference | Still open: `miner/core.py:347`, `miner/cli.py:244,418,476` | miner session; S (about 10 lines plus a test) | **No for this run**: `cpu_miners.py` treats `ReadTimeout` as transient and re-posts (`:50-58`), which wastes one or two POSTs per trial (T_ref + 17 s = 140-250 s against 120 s). Since Part B the re-post joins the reference already running and gets its commitment (`_follow_reference_v2`, `store.py:3684`: at most two waiters, 330 s each) [V by reading]. Yes for the CLI that a real miner uses. |
| d | Attention backend of the B300 replay | The manifest's pin now chooses the backend, not the table. Every manifest pins `HT_ATTN_BACKEND` (`protocol/env_pins.py:102-107`; `sm_103` accepts `math` or `sdpa-flash`, `:69-70`), applied by the miner (`miner/island_launch.py:595`) and by the CUDA executor (`gpu_ops/cuda_executor.py:844`; the runner exits 4 when the environment contradicts the pin). `trainer/attention.py:42-46` still has no `(10, 3)` entry, which matters only to an unpinned call. The builder pins `math` unless `HT_D20_ATTN_BACKEND=sdpa-flash` (`build_test_manifest.sh:120,231`) [V by reading] | Forge decision; needs a B300 bitwise check of `sdpa-flash` at hd 64 (never run with the pin, ADR 0011). A build flag, not code: it changes the run id, not `sources()` | **No at H = 4** (about 4 s per island, 3 % of an audit). **Yes if (e) takes H >= 31**: math attention adds 30 s at H = 32 and puts the trial over the 270 s go/no-go (288-385 s) and, at J = 16 and 8, over the 297 s window. A miner cannot choose the backend alone any more: the pin overrides its environment. |
| e | Lease and offsets for H >= 31: `lease_rounds` derived from H and `step_seconds_bound`, a round opener whose `d_final - d_audit` exceeds 200, **and a second executor host** | **Partly done.** The opener is done for a TEST manifest (D88): `live_operator open` caps the gap at the policy's `gap_cap()` and refuses a gap shorter than the D84 re-audit window (`scripts/ops/live_operator.py:130-191,213-215`); for dense-0.1b the builder signs `reaudit_rounds` from `HT_D20_REPLAY_SECONDS` and, above 174 s, `gap_cap_rounds` (`build_test_manifest.sh:432-439`) [V]. Still open: the builder's literal `lease_rounds` 100 (`:422`, which only H <= 30 accepts) and the second host (config only) | Forge; S | **Yes.** At H <= 30, five audits take 718-1279 s against the 600 s `absolute` (`min(d_final, d_audit + 2 x lease_rounds)`, `store.py:8155`) and a 300 s lease. A `gap_cap_rounds` above 200 does not move that `absolute`; it only holds the re-audit window. A gap below 200 shortens it, to 525 s at the 175-round gap of a 144 s replay [V by reading, H]. A second host relieves the 600 s `absolute` and the shared-host lease, four miners only the 600 s and only on a fast link. At H >= 31 the lease arithmetic no longer shows that a second host is needed (previous section): Principal decides it. |
| f | Result transfer path, ssh or R2, and the tooling to switch | **Tooling merged** (`be243121`). `challenge_image.sh executor BACKUP PATH --r2 FILE [--operations LIST]` writes the settings file and sets `HYPERTRAIN_CUDA_EXECUTOR_ARTIFACTS`; `r2-check [--file FILE] [--node LABEL sha256:DIGEST]` checks the bucket from the challenge and probes an executor host without putting a token on it [V by reading]. Rates measured from nodes: ssh between s100 and a node 7 MB/s, R2 20 MB/s on one stream and 100-144 MB/s on eight; s100 to R2 not measured [A, `docs/s100-test-stack.md` 6.4a] | Forge decides on the measured link | **Depends on the measured link.** ssh is enough at 105 MiB/s. The reported 7 MB/s is below the slowest ssh row of the budget (30 MiB/s), where the reference already does not fit [H]. Over R2 the reference needs `reference` in `operations` (`--operations audit,referee,reference`; Part B is merged) and 100-220 MB/s each way. |

Other gaps found, not on the list above: other holders of the store lock close the gate for about
7 times their duration too: `open_round_v2` (about 14 s a roster entry at 101 M) and `complete_audit_v2`
(a validate pass inside the transaction). `export_trial_v2` now stages after it releases the lock, with a
1-5 ms hold at 101 M [A, ADR 0012], and so do `aggregate` and `finalize` (row b). The client side of `open`
builds five roster anchors first (about 10 s a hotkey [H]) and `d_open` must be ahead of that;
`challenge_image.sh beacon` refuses a catch-up other than 60 (enough for a lock of 150 s). `cpu_miners.py`
no longer stops on one 404 of a trial object: the miner retries an object GET 404 (RELAY-02,
`miner/core.py:61`) and the driver retries the trial on the last 404 (`cpu_miners.py:12-17`); a missed trial
still strands the identity until `live_operator.py reseed`, which `probe-v2` waits 10 minutes for before it
exits 3, so the watchdog is still needed. Any rebuild gives new image digests (the build label carries the
commit) and a cold compute cache (S15 D1).

**Measurements that settle (f), the window and the re-audit window, to run in the later session, not
here** (commands only; none was run here). Rates measured from nodes are already reported: ssh between
s100 and a node 7 MB/s, R2 20 MB/s on one stream and 100-144 MB/s on eight [A, `docs/s100-test-stack.md`
6.4a].

- The warm audit replay that `HT_D20_REPLAY_SECONDS` needs (Forge, 2026-10-28; runbook section 4). It has
  no default and is never an estimate from this record. First a throw-away warm-up run, built at
  `HT_D20_REPLAY_SECONDS=300` (the ceiling: T 200, `gap_cap_rounds` 305), published as a TEST run labelled
  warmup and never reused. A sacrificial identity runs 4 trials on it (1 cold, 3 warm). The replay is the
  max of the 3 or more warm `exec_attempt` -> `exec_published` intervals of the executor journal; the cold
  one is reported separately and never counted. Then a rebuild with that max, published for the 5 real
  identities, built with `HT_D20_WARMUP_OUT` naming the warm-up's `OUT_DIR`. Every max up to 298 s gives a new
  run id; 299 and 300 s sign the warm-up's own window (T 200, `gap_cap_rounds` 305), so the rebuild would be the
  warm-up run: the builder refuses it (exit 2) and the runbook stops above 298 s and reports to Forge (Forge,
  2026-10-28; section 4, steps 4 and 6) [V]. The compute cache directory is keyed by executor host and image digest
  (`cuda_executor.py:365,771-773`) [V by reading], so the real run mounts the warm-up's directory on the hosts
  the warm-up used; whether those entries serve the real run is [?] (see "Not verified"). A dry run compares
  the two builds: same image digest and model, only `reaudit_rounds` / `gap_cap_rounds` differ. The warm-up
  run is then cleaned as in S15 (identity, ssh key, executor config), with proof. Above 174 s the real run
  needs `gap_cap_rounds` > 200 on the TEST ledger and is opened with that gap; above 298 s stop and report to
  Forge, the ceiling (300 s) is not raised. The manual W2 path is not a substitute: it is not the executor path.
- RTT and ssh rate between the challenge and the executor host, with the executor's own key and
  `known_hosts`, from the TEST challenge container, after the ephemeral key and the executor files
  are staged (both directions, 1 GiB):
  `docker exec hypertrain-test-challenge sh -c 'ssh -i /data/executor/id_ed25519 -o UserKnownHostsFile=/data/executor/known_hosts -o IdentitiesOnly=yes ubuntu@<node A> "head -c 1073741824 /dev/zero" | dd of=/dev/null bs=1M'`
  (executor to challenge: the result leg) and
  `... 'dd if=/dev/zero bs=1M count=1024 | ssh <same options> ubuntu@<node A> "cat >/dev/null"'`
  (challenge to executor: the input leg); `dd` prints the rate. Use the executor's other options
  (`-F /dev/null`, `StrictHostKeyChecking=yes`, the port) for a faithful figure. Ten `ssh ... true`
  timings give about five RTTs of handshake each, an upper bound on the RTT. [untested; assumes `dd`
  in the server image]
- R2 from a non-shared host of the same region class, which gives a lower bound of the link:
  `HT_R2_EXEC_TOKEN_FILE=<0600 env file> HT_R2_TEST_BUCKET=cortex-v3-test-<name> HT_R2_ARTIFACT_MIB=1024 HT_R2_ARTIFACT_EVIDENCE=<file outside the repository> uv run --frozen pytest -s -m cloudflare tests/gpu_ops/test_artifact_r2_live.py`.
  It moves a real artifact through the real runner and challenge side, with 16 workers (the settings
  below use 8) and a time that includes generating and hashing the data. **Do not copy the token or a
  checkout to the rented nodes** (S15 guard: image and `--rm` containers only; the host holds URLs
  only): the upload leg from node A is read from the first R2 audit's `artifact.upload_ms` in
  `role-dispatch.json`. Also check the CA bundle of the GPU image for https:
  `challenge_image.sh r2-check --node LABEL sha256:DIGEST` runs `ssl.get_default_verify_paths()` and the
  TLS handshakes in a confined `--rm` container of the image, with presigned URLs on stdin and no token on
  the node [V by reading]. It moves 1 MiB, so it says nothing about the rate. [ADR 0013: written, never
  run; no bucket-scoped token existed.]
- The profile's step time on B300: `scripts/run_mfu.py --profile mfu-dense-0.1b --device cuda --out
  <file>` from a checkout on a host of ours, not on the rented nodes (the GPU image has no `scripts/`).
  The row carries the MFU loop (micro_batch 4, H = J = 60), not the TEST loop (micro_batch 2, H = 4):
  halve the step time for micro_batch 2.

## R2 artifact configuration for the TEST bucket

Names only; no value is stored or printed here.
`challenge_image.sh executor BACKUP PATH --r2 /data/executor/r2.env` writes the settings file named by
`HYPERTRAIN_CUDA_EXECUTOR_ARTIFACTS` (`/data/executor/r2-artifacts.json`) itself, with only
`credentials_file` and `operations`; every other field takes its `ArtifactSettings` default, which is the
value in
`hypertrain/docs/examples/cuda-executor-artifacts.example.json` [V by reading]. The token file must be
`/data/executor/NAME`, a plain file of uid 65532 with mode 0600, in a directory of mode 0700 (proposal:
`r2.env`, put on s100 by stdin). So `fallback` stays `classic`: an unreachable R2 sends the 4 GB result over
ssh, and the journal records `exec_artifact_unavailable`. `fallback` = `fail` would need a script extension,
because the script refuses to replace a settings file it did not write; Forge decides [?]. `operations` is
the script's default `audit,referee`; a reference no longer holds the store lock (row a), so `reference` can
be added with `--operations audit,referee,reference` once the measured link favours R2 (row f). `workers` 8,
`part_bytes` 16 MiB gives about 247 parts for S, and `max_artifact_bytes` 16 GiB is above S = 4.13 GB.
The credentials file holds only `R2_ACCOUNT_ID`, `R2_ACCESS_KEY_ID`, `R2_SECRET_ACCESS_KEY`,
`R2_BUCKET` (a `cortex-v3-test-*` bucket) and optionally `R2_ENDPOINT`; the process environment is
ignored. `reserve_seconds` 60 plus the upload must fit the lease: R2 is skipped
(`exec_artifact_skipped`) when a lease has less than `reserve + 30` s left. The bucket needs the Forge
prerequisites of ADR 0013 (a bucket-scoped token, lifecycle rules on the literal prefix `exec/`, a
ledger line, at most 50 USD).

## Hash consequences

Measured with `network_qualification.sources()` and `bundle()` and `ChallengeStore._service_implementation_v2`
on `git archive` of each commit (Python 3.12 of the project venv), 2026-10-28 [V]. When this record was
written: `8c2b24e8` (the origin/v3 this lot was built on), `3036f8e7` (its code commit), `2cfc2a2b`
(origin/v3 then) and `350e50b1` (the merge of the two); the values at `3036f8e7` equalled those at
`8c2b24e8`, and those at `350e50b1` those at `2cfc2a2b`, so this lot moved none of them. At the tip,
`faa6abb4` (its `hypertrain/src` and the fixed files of `sources()` are those of `ea0f11fc`) [V]: the values
equal the d82-tip column of ADR 0011 Part 3. `sources()` is given in the canonical JCS form of ADRs 0007,
0008, 0010 and 0012; ADR 0013 used `json.dumps(sort_keys)`, whose value is `3623f281…dce0a4` at `350e50b1`
and `7a721848…b6b4bb` at the tip.

| Value | At `2cfc2a2b` and `350e50b1` | At the tip (`faa6abb4`) |
| --- | --- | --- |
| `implementation_hash` | `fc9e87b0…e4c8fe` (no pin input then) | **moved**, a function of the pin: `avx2`/`none` (the example pin) `7af135ca…70be62`; `avx512`/`none` `c509dfd1…83a920`; `avx2`/`sm_103`/`math` `24fbe36d…c185ce`; `avx2`/`sm_103`/`sdpa-flash` `9bcf9bdd…2c2dd5`; `avx2`/`sm_120`/`sdpa-flash` `c8af2ae7…68891f` |
| owned names / hashed files of `implementation_hash` | 35 / 10 | 48 / 12 |
| `challenge/store.py` sha256 | `e9c9fb36…58acde` | **moved**: `22fb5dcd…c56676` |
| `challenge/admission.py` / `challenge/app.py` / `protocol/messages_v2.py` sha256 | not measured | `0d7fa656…d67ead` / `0e6ec421…e07d48` / `8e03a8a0…42bb04` |
| `network_qualification.py` sha256 | `dab7a6ab…7bda15` | **moved**: `2206486f…319688` |
| `cuda_executor.py` sha256 | `f342234d…7db619` | **moved**: `16ed30df…6ac457` |
| `cuda_executor_runner.py` sha256 (`runner_sha256`) | `5930be17…4dff90` | **moved**: `fd43d985…e2eb9f` |
| `sources()` canonical sha256 | `82d9a332…e2081b`, 155 files, 2,427,686 B | **moved**: `97e3c592…5f1be3`, 158 files, 2,641,859 B; `sha256sum` of the CLI listing `94a3f4db…a9f202` |
| `bundle-hash` | 2,549,760 B, `50e26ecd…96345e` | **moved**: 2,775,040 B, `2c064435…0b8196` |
| example `RunManifestV2` run id | not measured | `aa39197d…ec8abd` (unchanged since `389c45b4`) |
| run id of `tiny-cpu` / `dense-1b`, fixed seeds (`0x1001`..`0x1007` for owner, coord, auditor-0, auditor-1, referee, cold-0, cold-1), image `sha256:` + `cd` x 32, driver 595.91.07, 148 SM, 2 coldkeys, exp round 34310319, relay key of `bytes(range(1, 33))` (`baseline/run_baseline.sh` in the evidence folder) | `a55da95e…74ab8e` / `ef0fc003…83ebfe` | **moved**: `f3c12327…dcde3588` / `bbbff4bb…35e4a542`; `init_state_hash` `e07838a2…6133884` / `8dfdacef…c164efe9` |
| `dense-0.1b` on the same inputs | at `350e50b1` (absent at `2cfc2a2b`): `param_count` 100,676,352; `init_state_hash` `be9b7bd6…ee28f8`; run id `1cf7d790…71a3fb` | the same `param_count` and `init_state_hash`; **moved** run id, which now depends on `HT_D20_REPLAY_SECONDS`: 144 s `0ea43133…82da2af3`, 256 s `944b69ae…5bb46da6`, 300 s `b6bebaf7…ca41283d` |

What moved and why. `implementation_hash` now hashes twelve files (the ten of `2cfc2a2b` plus
`protocol/env_pins.py` and `trainer/determinism.py`), 48 owned names of `store.py` and the body of the
manifest's environment pin (`challenge/store.py:4933-4946,4975-5024,5034`) [V by reading]: it is a function
of the pin and is given per pin, as in ADR 0011. Since `2cfc2a2b` eight of the twelve files changed and 31
of the 48 names are new or changed, among them the claim, compute and settle steps of the aggregate and the
finality replay of ADR 0012 [V, blob and AST comparison]. `sources()` and the bundle moved because 39
`src/hypertrain` files changed and three were added since `2cfc2a2b` (`challenge/exit_v2.py`,
`challenge/sanction_v2.py`, `gpu_ops/host_attest.py`), and one fixed file changed,
`experiments/gpu_network_v2/orchestrate.py` (`4a588b63`) [V]. Every run id built on the fixed inputs moved
because DET-01 (`038c80bc`) made `reference_spec.env.pinned` required (`protocol/messages.py:295`): the builder
writes the pin for every geometry (`build_test_manifest.sh:222-233,383`), so every manifest's bytes move [V
by reading]. That no run id moved at `350e50b1` held only while the pin was opt-in. The `dense-0.1b` run id
also moved because its audit policy now signs `reaudit_rounds`, and `gap_cap_rounds` above 174 s; a
`dense-0.1b` run id must name the replay it was built with. The fixed-seed builds at the tip [V]: `tiny-cpu`
pin `avx2`/`none`, 7 s; `dense-1b` pin `avx2`/`sm_103`/`math`, 49 s; `dense-0.1b` pin `avx2`/`sm_103`/`math`,
about 11 s each, with the dry-run `audit` block (`lease_rounds`, `reaudit_rounds`, `gap_needed`, `gap_cap`)
100 / 100 / 175 / 200 at 144 s (no `gap_cap_rounds` field), 100 / 171 / 276 / 276 at 256 s and
100 / 200 / 305 / 305 at 300 s. The lot's builder change moves only `dense-0.1b`: `tiny-cpu` and `dense-1b`
built with the builder before the merge of `v3-model-intermediate` (`62a10e5d`, same `src`) and with the
tip's builder are byte for byte identical (`inputs/training.json`, `policies.json`, `relays.json`,
`reference-spec.json` and every `out/` file except the randomised envelope signature and `index.json`),
with the same run ids [V].

This lot touches `scripts/mfu_profiles.json`, `scripts/b300/d20/build_test_manifest.sh`, tests and
docs only, none of which feeds `sources()`, the bundle or `implementation_hash`. The envelope and
`index.json` differ between two builds of the same inputs (the signature is randomised). The builder's
printed dry-run JSON renames its check `work_screen_epoch_rounds_6000` to `work_screen_epoch_rounds` for
every geometry; since `04efdac6` it also prints the `audit` block and the checks
`reaudit_rounds_of_the_replay` and `gap_cap_holds_the_reaudit_window` (dense-0.1b) or
`no_reaudit_window_or_cap` (the other geometries); only the builder tests read them. One test read the
builder's source: `tests/challenge/test_strike_window_v2.py` (D70) took the TEST screen from the literal
`"work_screen_epoch_rounds": 6000`, which this lot replaced with `screen_rounds`, and failed at collection at
`10a565a1`; `faa6abb4` reads both screens, 6000 and 20000 for `dense-0.1b`, whose no-show strike window is
80000 rounds (ADR 0010) [V]. Consequences:

- This lot invalidates no qualification config, dispatch source pin or reviewed bundle. The commits
  merged since `8c2b24e8` do: a record, pin or review made against the `2cfc2a2b` column above does not
  match the tip.
- The images of S15 (server `a513821d`, GPU `331cec82`, built from `e0969448` [A, S15 report]) are
  not the code of the tip: 51 `src` files changed since (25 at `350e50b1`, 10 at `8c2b24e8`; `git diff
  --name-only e0969448 faa6abb4 -- hypertrain/src`) [V], the owner routes `/challenge` and `/job` went from
  `GET` with headers to `POST` with a signed body after it (`13ec158c`, ADR 0010), so a tip miner cannot
  talk to the S15 server image [V by reading], and the genesis origin ids are now keyed (ADR 0014:
  `genesis_origin_id` takes `escrow_id_key` of the coordinator secret as a fourth argument;
  `publish_run.py genesis` runs inside the server container and imports it, so it cannot run in the
  S15 server image) [V by reading]. A replay must run server, miner and operator tools from the same
  side of all three changes: the signed owner routes, the keyed origin ids and the required environment pin
  (next bullet).
- The GPU container of the S15 image cannot serve the tip. Every manifest the tip accepts carries
  `reference_spec.env.pinned` (`messages.py:295`); the `ReferenceEnv` of `e0969448` has three fields and
  forbids extras, so the island worker inside `331cec82` refuses every such manifest, and the tip refuses a
  manifest without the pin [V by reading]. The unpinned case this bullet examined at `350e50b1` no longer
  exists. At the tip the island worker reads the pin, refuses a device outside its GPU class
  (`miner/island_worker.py:84`) and writes what it observed into `summary.json["env"]` (`:263`). Its import
  closure (a static walk from `hypertrain.miner.island_worker`, the hand-off of
  `gpu_ops/cuda_executor_runner.py:346`) meets 11 of the 51 files: the ten of `350e50b1` plus
  `data/store.py` [V, static walk]. The miners give a second, independent reason: they run their CLI from
  the manifest's image (`docs/s100-test-stack.md` section 6, `cpu_miners.py` with `HT_MINER_DEVICE=cuda`),
  and the CLI of an image built from `e0969448` sends `GET` with headers to `/challenge` and `/job`
  (`miner/core.py` at that commit), which a tip server refuses (`challenge/app.py` registers `POST` only)
  [V by reading; the build commit of the image is [A]]. Server, GPU and miner images must all be rebuilt
  from the tip. Any rebuild gives new digests (the build label carries the commit), hence a new run id and
  a cold compute cache (S15 D1).
- A new run id per fresh build (new seeds), per image digest and, for `dense-0.1b`, per audit window: the
  run id follows `reaudit_rounds_for(replay, 100)` and the cap it implies, not the seconds themselves (1-150 s
  all sign T 100 and no cap; 299 s and 300 s both sign T 200 and cap 305) [V, computed with the tip's
  functions]. The throw-away warm-up build at 300 s and the rebuild at the measured replay share the training
  body, the image and every policy except the audit one; they differ in `audit_policy_hash` and so in run id
  (`test_the_rebuild_with_the_measured_replay_changes_only_the_audit_window`, 300 s then 298 s) [V], unless the
  measured max is 299 or 300 s: then the two manifests would be identical, so the builder refuses the rebuild
  given `HT_D20_WARMUP_OUT` and the runbook stops above 298 s (section 4, steps 4 and 6) [V].

## Tests and mutants

`hypertrain/tests/`, all on CPU in default CI. The lot's tests take about 190 s together on a host at
load 290 [V, 2026-10-28 at `10a565a1`, whose builder and tests are those of the tip]; about 80 s of that is
the tests the merge `04efdac6` added (the two-build rebuild 50 s, a second full `dense-0.1b` build 19-29 s,
the 301 s refusal 11 s). The peak is 5.8 GB in one test, below the 8.8 GB od-base round
(`models/test_model_registry.py::test_from_scratch_round_bitwise_and_replays`), which sits in one shard.
Three new files shift the round-robin split of later files by a shard or two:

| Behaviour | Test | Mutants killed |
| --- | --- | --- |
| the row is the pinned shape | `test_mfu_profiles.py` | 12, 24 or 2 layers; d_ff 2816; vocab 65536; 12 kv heads; 24 heads |
| deterministic init at full size, pinned | `models/test_dense_intermediate_golden.py` | init index shared; std doubled |
| the band is inclusive and applies to the row | `test_b300_d20_builder.py::test_the_dense_0_1b_row_must_stay_inside_the_inclusive_100_to_300_million_band` (99,999,998 refused, 100,000,000 and 300,000,000 accepted, 300,000,002 refused, by builds that stop after the first stage; only even counts exist, so ±2 is the tightest edge) | bounds 0, 10 M, 10^12, 3 G; either edge exclusive; guard inverted or not exiting |
| the build reads the row and signs the loop, the fixture, the layout, the screen window, the re-audit window, the cap and the pin | `test_a_dense_0_1b_build_writes_the_registry_row_and_a_wide_screen_window[replay_144s_fits_the_default_cap, replay_256s_raises_the_cap]` (full builds read back as `POST /v2/admin/runs` does: T 100 and no cap, T 171 and cap 276, the dry-run `audit` block, pin `avx2`/`sm_103` at 148 SM; over a production economics policy the same inputs publish at 144 s and are refused at 256 s), `test_a_good_build_...` (tiny keeps 6000 and signs no window or cap) | count + 1; 2048 samples; token id equal to the vocab; shard root over truncated bytes; window always or never 20000; J = 2; state policy carry; rewarmup 5; peak lr 3e-4; warmup 0; the 0.35b row; two ranks; geometry not accepted; dry run expecting 6000; window = replay rounds instead of `reaudit_rounds_for`; cap always set; cap never set; cap one round wide; the D88 guard of `validate_training` removed; `manifest_v2 build` not passing the ledger mode |
| the replay is required for `dense-0.1b` and only for it | `test_a_dense_0_1b_build_without_a_measured_replay_is_refused_before_anything_runs[missing, zero, not_whole_seconds, another_geometry]` (exit 2, nothing created) | replay not required; replay accepted for other geometries; regex accepting 0 |
| the 300 s ceiling | `test_a_replay_above_the_300_s_ceiling_is_refused_by_the_manifest_stage` (301 s: "re-audit window must be between one and two leases", nothing left) | replay clamped to 300 |
| the two builds differ only in the audit window | `test_the_rebuild_with_the_measured_replay_changes_only_the_audit_window` (300 s then 298 s over the same seeds, the real one given `HT_D20_WARMUP_OUT`: `training.json` byte identical, policies equal but `reaudit_rounds`/`gap_cap_rounds` 200/305 against 199/304, manifest training equal, network equal but `audit_policy_hash`, the real dry run's `warmup_*` checks true and `warmup_run_id` the warm-up's) | replay leaking into the training body; the network compared with its audit policy hash |
| a real build that would be the warm-up is refused (stop above 298 s, Forge 2026-10-28) | `test_a_real_build_that_would_be_the_warm_up_is_refused[299, 300]` (exit 2, "the manifest is the warm-up build's", nothing left but the staged seeds), `test_every_replay_up_to_298_s_asks_another_window_than_the_warm_up` (`reaudit_rounds_for(r, 100) < reaudit_rounds_for(300, 100)` for r = 1..298, equal at 299), the dry-run `stop_above_seconds` 298 in the 144/256 s builds | no refusal; the index compared instead of the manifest; the stop computed at the warm-up's own window |
| the warm-up variable is checked before anything runs | `test_a_dense_0_1b_build_without_a_measured_replay_is_refused_before_anything_runs[warmup_for_another_geometry, warmup_relative, warmup_without_a_manifest]` (exit 2, nothing created) | relative path accepted |
| unknown geometry refused, known ones named | `test_an_unknown_geometry_is_refused_and_the_known_ones_are_named` | any geometry accepted |
| serialised sizes and replay at real size | `models/test_dense_intermediate_round.py` | int8 block 256 -> 128 |

Each mutant was applied to the real file, its tests run with `pytest -x`, and the file restored (the
unstaged `git diff` was empty afterwards); a kill counts only on a failed test, never on an error
(an earlier run counted four transient errors as kills and was rerun). **34/34 killed** on CPU on the
branch before its merge, 2026-10-28 [V then; not rerun at the tip]; the driver and its output are in the
evidence folder. The merge `04efdac6` adds nine hand mutants of its new builder lines, **9/9 killed**, and
two mutants of the D88 guard in `src` (the guard removed, `manifest_v2 build` not passing the ledger mode),
both killed by the 256 s build [V, run on `04efdac6`, whose builder, tests and `src` are those of the tip]. The
warm-up refusal (Forge, 2026-10-28) adds five hand mutants of its builder lines, **5/5 killed** [V].
The two model tests pass the CPU pin to `run_mfu.manifest_body`; the pinned `init_state_hash` is unchanged.
The real-size round is a scale proof of existing engine code, not a regression test of this lot: a
reduced-size round would pass on the old code as well. A local end to end (join, probation, run-v2,
audits, `get_weights`) at this size is not possible on CPU: the harness (`mine_multi_round_v2.py`) is
wired to the one-layer fixture and its 1 MB object caps, and runs about 70 train/replay units; the largest
local proof is the builder's dry run (the real validators of `POST /v2/admin/runs`) plus the round test
above.

## Rejected

- **209 M (12 layers, d_model 1024, 208,692,224)**, the D68 target: statically valid, but every
  time model fails it (trial 435-526 s even at 105 MiB/s against 297 s; five audits 1408 s; an 8.56 GB
  result). Measured here: init 6.3 s, state 2.50 GB, one real CPU step 223 s at seq 2048 [V,
  contended host]. Left for a later rung: no row exists for it in `scripts/mfu_profiles.json` [V]; adding
  it is a new row plus the builder edit of "Decision".
- **126 M (12 layers, d_model 768, 125,848,320)**: 283-338 s at the best link, no margin.
- **A registry module under `src/hypertrain`**: it moves `sources()`, the bundle and (for
  `models/registry.py`) `implementation_hash`, and forces new images for a geometry the `decoder`
  arch already runs.
- **A range-only test**: rows of 114.8 M (d_ff 2816), 125.8 M (12 layers), 151.0 M (vocab 65536) and
  107.0 M (12 kv heads) are in band; the exact count freezes the shape.
- **H/J = 2 (H = 8, J = 4)** as in dense-1b: 5.34 GB instead of 4.13 GB for nothing the proof needs.
- **A smaller sequence length** to cut attention: changes no artifact size and hides the memory
  profile of a 2048-token batch.
- **Tied embeddings** to land the count lower: a new architecture variant.

## Not verified

- Every time other than the V rows: link rates (RTT between s100 and the nodes was never measured),
  R2 from a GPU host, `T_ref`, `T_audit`, `copy_flags` at this size beyond one two-delta run, the cold
  start with this model's kernels, s100 CPU, RAM and free disk, math-attention time at head dim 64,
  trace-hook cost and its linear scaling with J, the number of validation passes at runtime (six by
  reading). The compute cache directory is keyed by image digest (`cuda_executor.py:365,771-773`) [V by
  reading], and this run needs an image built from the tip, so its cache starts empty and nothing carries
  over from the tiny model.
- The audit replay that the real run signs. No warm replay of this model has been measured, so no real
  build exists yet: the builder refuses `dense-0.1b` without `HT_D20_REPLAY_SECONDS` (exit 2). The two-build
  flow (a warm-up run at 300 s, then a rebuild at the max of at least three warm replays, the cold one
  excluded) is tested only on CPU builds with fixed replays (144, 256, 300, 301 s). Whether the measured max
  is at most 174 s (default cap), 175-298 s (`gap_cap_rounds` above 200 on the TEST ledger, round opened with
  that gap) or above 298 s (stop, report to Forge) is unknown [?]. The warm-up run opens no round, so its
  warm replays are references; the executor part of `T_audit` has an `8k` term more than that of `T_ref`
  [H], so whether a reference replay bounds the audit replay the window is sized for is not established [?].
- That a run id built here is accepted by the TEST challenge image: the dry run applies the repo's
  validators, not the image's. `POST`ing a run before its CUDA qualification record freezes its
  backend to `cpu`.
- That the server keeps working on a proof POST after the client gave up at 120 s (assumed by (c)),
  and whether the GPU image carries a CA bundle for https (ADR 0013 [?]; `r2-check --node` reports it).
- The provenance of the S15 images (`a513821d`, `331cec82`, built from `e0969448`) comes from the S15
  report only [A]. It no longer decides anything: every tip manifest carries `reference_spec.env.pinned`,
  which their `ReferenceEnv` refuses, so neither image can run a tip manifest [V by reading]. At the tip the
  island worker's import closure meets 11 of the 51 changed files (the ten of `350e50b1` plus
  `data/store.py`).
- (a) is merged at the tip (ADR 0012 Part B, `25f58756` through `389c45b4`): `trial_reference_v2` claims
  under the store lock and stages, launches and puts without it (`store.py:3550-3682`) [V by reading]. The
  aggregate and the finalize replay also compute off the lock (`e2f47116`, `store.py:10412`, `2694`) [V by
  reading]. Whether the gate stays open with them on s100 is not established [?]. The sizes and owners of
  the other prerequisites are estimates.
- Whether the warm-up run (built at 300 s, 1 cold and 3 warm trials of the sacrificial identity) populates
  the cache entries the real run's island needs. Both runs use one image digest, so the executor mounts the
  same cache directory on a host (`cuda_executor.py:365,771-773`) [V by reading]; whether the entries the
  warm-up trials write serve the real run, a different run id, is not established (ADR 0012) [?]. The cache
  is per host as well: a second executor host's first job is cold unless the warm-up also ran on it [?].
