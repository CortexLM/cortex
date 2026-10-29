# ADR 0016: the D94 dense TEST models that fit the admission window (31,462,656 and 44,045,568 parameters)

- Status: draft, 2026-10-29 (Principal SN100 D94, option A). Forge decides promotion to `v3`. Written for branch
  `v3-model-50m` on origin/v3 `667695be`. Scripts, tests and docs only: no file under `hypertrain/src/` changes
  [V], so the attested images of the brief (server `sha256:8fac7038…`, GPU `sha256:cb52fe37…`) [A, brief] stay
  usable and every hash of "Hash consequences" is unchanged [V]. Nothing in this lot ran on a GPU host, on s100,
  on the nodes, against R2 or on-chain.
- Order: brief `v3-model-50m`. D94 gives the rule (miner side plus trusted reference within 250 s, a 15 %
  margin under the 297 s window); Forge (CTO) chose the rows through the relay tool in the night of
  2026-10-28 to 29: first the 44 M row, then the 31 M row besides it, then (after the first full T_ref) the
  31 M row as the run profile and the 44 M row as a documented larger option.
- Labels: as ADR 0015. **[V]** verified in code or by a command at `667695be` plus this lot, **[A]** reported
  by a cited document or session, **[H]** hypothesis or arithmetic, **[?]** unknown.
- Related: ADR 0015 (the 101 M model whose mechanism this copies; its runbook), 0011 Part 9 (re-audit window,
  gap cap), 0010 (screen length and strike window), 0013 (executor results through R2).

## Context

The admission trial window is `challenge_rounds` = 100 beacon rounds, a protocol literal
(`protocol/messages_v2.py:645`), so 297 s from challenge to the end of the reference [V by reading]. The
executor reference of a trial starts only at the proof POST (`challenge/app.py:679-681`) [V by reading], so
the miner side and the reference run in series. S17b measured both at 100,676,352 parameters (`dense-0.1b`,
ADR 0015) on the s100 TEST stack:

| Phase at 101 M | Seconds | Source |
| --- | --- | --- |
| miner side, trial 1 (cold, 2026-10-28 22:47-22:52Z): challenge to job 29, input GETs about 50, GPU compute 70, post-compute 33, 1640 object POSTs 90 | 282.4 (`probe-v2`) | [A, brief; S17b driver stages] |
| miner side, trial 2 (warm, 23:43:47-23:47:24Z): challenge to job 39, 5 input GETs 51, compute and post-compute 58, 1640 object POSTs 64, then the proof POST | 217 | [A, S17b `trial2-timeline.txt`] |
| trusted reference, first full one (measure 1, 2026-10-29 00:46-00:52Z, outside an admission window, one sample): launch -6.4, input push over ssh 2.5 to 98.1 (805,519,360 B, about 8.4 MB/s), compute to the offer 136.5, R2 upload 98.9 and s100 download 42.6 (done 278.7), validation and publication 331.4 (7 files, 4,934,880,048 B), R2 sweep 0 objects left, cleaned 333.2 | **338.0** | [A, S17b `measure-1-phases.txt`, file read] |

Trial 2's reference (started early through the admin override) offered its 4,129,392,640 B result at
+188.3 s and was closed at +266.6 s (409 `REFERENCE_TRIAL_CLOSED`) [A]. 338 s replaces every estimate of
T_ref(101 M) in this record: the 123-235 s of ADR 0015 and the lower bound of 265 s from trial 2. At 101 M
the miner side alone takes 217-282 s of the 297 s, so no admission trial can pass at that size.

## Decision

### Sizing

Forge's model (2026-10-29): every phase but the 30 s from challenge to job and the compute is proportional to
the parameter bytes (the input is 8.0 P bytes and the result 41.016 P bytes at 101 M [V, sizes above]), and
the compute is small. With r = P / 100,676,352:

- T_miner(P) = 30 s + 187 s x r warm (217 s at r = 1), 30 s + 252 s x r cold (282 s) [H, linear scaling];
- T_ref(P) = 338 s x r [H, one sample scaled];
- **a row is allowed iff T_miner(P) + T_ref(P) <= 250 s** (D94).

| Row (geometry) | P [V] | r | T_ref | T_miner warm | total warm | T_miner cold | total cold | Verdict |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `mfu-dense-0.03b` (`dense-0.03b`) | 31,462,656 | 0.31251 | 105.6 | 88.4 | **194.0** | 108.8 | 214.4 | allowed: the run profile |
| `mfu-dense-0.05b` (`dense-0.05b`) | 44,045,568 | 0.43750 | 147.9 | 111.8 | 259.7 | 140.2 | 288.1 | over 250 s: built, documented, not run |
| 4 layers, vocab 16384 (the brief's candidate) | 50,331,648 | 0.49994 | 169.0 | 123.5 | 292.5 | 156.0 | 325.0 | refused, no row |

All [H] but P. Read backwards, a row fits while T_ref(101 M) stays under (250 s - T_miner) / r: 517 s warm and
452 s cold for `dense-0.03b`, 316 s and 251 s for `dense-0.05b`, 253 s and 188 s for 4 layers. If the compute
and the fixed launch and clean-up (48.9 s at 101 M) did not shrink at all, `dense-0.03b` would still fit: T_ref
139 s, 228 s warm, 248 s cold [H]. `dense-0.05b` is for when the miner side shrinks (a miner fast path,
inputs through R2; Forge); by the formula it would also fit at a T_ref(101 M) of 316 s or less warm [H]. It
is not the default anywhere.

| Row | Inputs (8 P) | Miner PUT (17.016 P) | 1 MiB parts | Result (41.016 P) | State (12 P) | Delta |
| --- | --- | --- | --- | --- | --- | --- |
| `dense-0.03b` | 252 MB | 535 MB | 511 | 1.290 GB | 378 MB | 32.0 MB |
| `dense-0.05b` | 352 MB | 749 MB | 715 | 1.807 GB | 529 MB | 44.7 MB |

[H, the 101 M ratios applied; the 101 M input and result sizes are measured.]

Label: **non-toy dense models of 31 M (run profile) and 44 M (larger option), below the 100-300 M band D68
aimed at, bounded by `challenge_rounds` = 100.** They prove the network path at a real size, not the D68 size.

### Rows and geometries

1. **Rows** in `hypertrain/scripts/mfu_profiles.json`, both `decoder`, 3 layers, d_model 768, 12 heads, 4 kv
   heads, d_ff 2048, one expert, seq_len 2048, bf16: `mfu-dense-0.03b` at vocab 8192 (**31,462,656**) and
   `mfu-dense-0.05b` at vocab 16384 (**44,045,568**) [V, `param_shapes`, printed by the builder as
   `param_count`]. Untied embeddings are 12.6 M and 25.2 M of them; the three layers are 18.9 M.
2. **Geometries** `HT_D20_GEOMETRY=dense-0.03b` and `dense-0.05b` in
   `hypertrain/scripts/b300/d20/build_test_manifest.sh` reuse the `dense-0.1b` branch: one `MEASURED` tuple
   (`:232`) and a table of row, band and ADR per geometry (`:301-318`). Everything but the row and the band is
   `dense-0.1b`'s (ADR 0015 Decision 2): H = J = 4, micro_batch 2, `state_policy = reset`, layout `n_gpus 1`,
   the 4096-sample fixture with token ids under the row's vocab, `work_screen_epoch_rounds` 20000 (`:409`),
   `HT_D20_REPLAY_SECONDS` required (`:137`), `HT_D20_WARMUP_OUT` optional, the re-audit window and gap cap
   from the replay (`:448`), the dry-run `audit` block and checks (`:507`) and the warm-up refusal. The
   qualification profile is `<geometry>-bf16` and the geometry source names the row and this ADR (`:327-328`).
3. **Own inclusive bands**, checked on the shape-only count before any manifest file is written:
   25,000,000..40,000,000 for `dense-0.03b`, 40,000,000..70,000,000 for `dense-0.05b`; `dense-0.1b` keeps
   100,000,000..300,000,000. The exact counts are pinned in the tests: the 25-40 M band also holds 2 layers at
   vocab 16384 (37.8 M), 4 layers at vocab 8192 (37.8 M), d_ff 2816 (36.8 M), 12 kv heads (33.8 M) and one
   layer at vocab 16384 (31,459,584).
4. **Init** is the decoder init of ADR 0015 Decision 3. `init_state_hash`
   `e372a68edb6485fce5f3df4efa9d72705fb04bbf7fd130dc3805d84075bf0aa6` for `dense-0.03b` and
   `9cbe3ee93068dbf7cf08791f02bd3bd07e5181fb3bc13efa4396b4ade5eaf897` for `dense-0.05b` [V]; two inits are
   bitwise equal under the AVX2-capped dispatch CI uses.
5. The other geometries are byte-identical (see "Hash consequences"). The runbook of ADR 0015 now runs
   `dense-0.03b` (its section 0); its warm-up, replay measurement, rebuild and stop at 298 s are unchanged.

## Hash consequences

Measured with `network_qualification.sources()`, `bundle()` and `ChallengeStore._service_implementation_v2`
on a `git archive` of `667695be` and on this branch's tree, the tree's own code in a child `python -I -B`
(2026-10-29) [V]. Every value is equal; they are the tip column of ADR 0015 and ADR 0011 Part 3.

| Value | `667695be` and this branch |
| --- | --- |
| `implementation_hash` | `7af135ca…70be62` (`avx2`/none); `c509dfd1…83a920` (`avx512`/none); `24fbe36d…c185ce` (`avx2`/`sm_103`/math); `9bcf9bdd…2c2dd5` (`avx2`/`sm_103`/`sdpa-flash`); `c8af2ae7…68891f` (`avx2`/`sm_120`/`sdpa-flash`) |
| `sources()` | 158 files; JCS map `97e3c592…5f1be3`, `json.dumps(sort_keys)` map `7a721848…b6b4bb` |
| `bundle()` | 2,775,040 bytes, `2c064435…0b8196` |
| example `RunManifestV2` run id | `aa39197d…ec8abd` |
| `network_qualification.py` | `2206486f…319688` |

Builds with fixed seeds (`0x1001`..`0x1007`), exp round 34310319, image `sha256:` + `cd` x 32, driver
595.91.07, 148 SM, 2 coldkeys and relay key `bytes(range(1, 33))`, on both trees [V]:

| Geometry | `667695be` | this branch |
| --- | --- | --- |
| `tiny-cpu` / `dense-1b` | `f3c12327…dcde3588` / `bbbff4bb…35e4a542` | the same run: every input and output file equal but the randomised envelope signature |
| `dense-0.1b`, replay 144 s | `0ea43133…82da2af3` | the same run |
| `dense-0.05b`, replay 144 s | refused (unknown geometry) | `118f29ee…7f365b`, 5.7 s |
| `dense-0.03b`, replay 144 s | refused (unknown geometry) | `b07a732e…873fa75`, 5.3 s |

The D94 builds sign the admission policy `7a881925…` and, at 144 s, the audit policy `1dd433fe…` of
`dense-0.1b`: over the same seeds they differ from it in their training body only (tested).

## Tests and mutants

All on CPU in default CI. Red: the new tests over the scripts of `667695be` gave 23 failed, 63 passed and 1
collection error (`test_strike_window_v2.py`, which reads the builder's new screen rule) [V]. Green: 101 passed
in 312 s for the lot's files plus ADR 0015's model, round and layout tests [V].

| Behaviour | Test |
| --- | --- |
| the rows are the pinned shapes | `test_mfu_profiles.py::test_the_50m_row_has_the_pinned_parameter_count`, `::test_the_d94_run_row_has_the_pinned_parameter_count` |
| deterministic init at full size, pinned | `models/test_dense_50m_golden.py` (both rows) |
| a full build signs its row, the 101 M loop, fixture, screen, window, cap and pin, read back through the real manifest model | `test_b300_d20_builder.py::test_a_d94_build_writes_its_own_row_with_the_dense_0_1b_loop_and_windows` (44 M at 120 s: T 100, gap 175, no cap; at 200 s: T 134, cap 226; 31 M at 144 s) |
| the run ids differ from `dense-0.1b`'s over the same seeds, only by the training | `::test_the_d94_runs_differ_from_the_101m_run_built_from_the_same_seeds_only_by_their_training` |
| the warm-up guard (298 s through, 299-300 s refused) | `::test_the_50m_warm_up_guard_lets_298_s_through_and_refuses_the_warm_up_window` |
| replay and warm-up variables required and checked for both geometries | `::test_a_dense_0_1b_build_without_a_measured_replay_is_refused_before_anything_runs` (new cases) |
| own inclusive bands, ±2 at each edge | `::test_a_d94_row_must_stay_inside_its_own_inclusive_band` (8 cases) |
| unknown geometry refused, the five known ones named | `::test_an_unknown_geometry_is_refused_and_the_known_ones_are_named` |
| the strike window still bounds the TEST screens | `challenge/test_strike_window_v2.py` (reads `screen_rounds = 20000 if geometry in MEASURED else 6000`) |

**49/49 hand mutants killed** [V], each in its own copy of `hypertrain/` under the scratch directory, a kill
counted only on a failed test: band bounds and exclusivity of both rows, the guard strict, off or printing
instead of exiting, a row reading another row, the wrong ADR or qualification profile, token ids reaching the
vocab, each geometry dropped from `MEASURED`, the dry run, the case list or the replay gate, the screen
widened for `dense-0.1b` only, the init index shared or its std doubled, and the row fields of both rows
(4 or 2 layers, the other vocab, d_ff 2816, 12 kv heads, 24 heads, seq 1024, fp32).

## Rejected

- **`dense-0.05b` as the run profile** (Forge's first pick, 2026-10-28): 259.7 s warm at the measured T_ref;
  kept as a documented larger option.
- **4 layers at vocab 16384 (50,331,648)**, the brief's default candidate: 292.5 s warm; no row.
- **2 layers at vocab 16384 (37.8 M)**: 227 s warm but 251 s cold [H]; not asked for. Forge named the three
  layers of the 44 M row at the smaller vocab.
- **A shorter sequence, H, micro_batch or tied embeddings**: the brief keeps the 101 M loop; tied embeddings
  are a new architecture variant under `src`.
- **A registry module or any `src` change**: it moves `sources()`, the bundle or `implementation_hash` and
  needs new images.

## Not verified

- Every time of the sizing table: linear scaling of each phase with the parameter bytes, from one T_ref sample
  and two miner-side trials at one size. The cold miner side and a cold reference at these sizes, the link
  rates of the night of the run and the compute at a smaller vocab are not measured.
- The audit replay these runs sign: none measured, so no real build exists; the warm-up flow of ADR 0015 is
  unchanged and measures it. At about 106 s [H] it would sign T 100 and no cap. Whether five audits in series
  (about 528 s, 176 rounds, if T_audit is near T_ref [H]) end before `absolute` with one executor host is not
  established [?].
- That the attested images accept these manifests: the dry run applies this repository's validators, not the
  image's; no `src` change makes it likely [H].
- No row was trained, replayed or qualified on a GPU in this lot.
