# ADR 0015: the intermediate dense TEST model (100,676,352 parameters)

- Status: draft for branch `v3-model-intermediate`, 2026-10-28 (Principal SN100 D68). Forge decides
  promotion to `v3`. Nothing here changes a production run, master, profile, pin or emission, and no
  file under `hypertrain/src/` changes. Nothing in this batch ran on a GPU host, on s100, against R2
  or on-chain. Merged with origin/v3 `2cfc2a2b` (merge `350e50b1`, 2026-10-28): the code anchors and the
  statements about the verdict lease below are those of that tip; the hash table gives the values
  before and after the merge.
- Order: brief `v3-model-intermediate`. Forge (CTO) chose the size on 2026-10-28, within the D68
  band (see "Decision").
  The number 0015 may collide with another open batch; rename at merge.
- Labels: **[V]** verified in code or by a command on the date above, **[A]** reported by a cited
  document or session, **[H]** hypothesis or arithmetic, **[?]** unknown.
- Related: ADR 0006 (kimi-tiny, per-arch registry), 0007 (SM counts, hash tables), 0008 (generic
  CUDA executor), 0010 (signed owner routes), 0011 (audit integrity, environment pins), 0012
  (executor performance, Part B lock-free reference), 0013 (executor results through R2), 0014
  (escrow operation ids, keyed origin ids).

## Context

The tiny-cpu replay (S15, 2026-10-27) took 824 parameters through join-v2, probation, run-v2, CUDA
audits and `get_weights` on the s100 TEST stack. D68 asks for the next proof to be a dense model of
100-300 M parameters (target about 200 M), with its artifacts through the R2 TEST bucket, and the
1 B model only if the audit window holds.

What exists at `2cfc2a2b` (unchanged since `8c2b24e8`):

| Model | Parameters | Where | Used for |
| --- | --- | --- | --- |
| tiny-cpu | 824 | builder geometry `tiny-cpu` | S15 replay on B300 [A, S15 report] |
| kimi-k3-pro-tiny | 77,001,376 [V] | `models/kimi_tiny.py:66` `PROFILE`; `models/registry.py` names the arch | CPU unit tests only. The GPU runs of S3, S9 and S12 used `MICRO` (73,200 parameters): the PROFILE cannot ride the local harness (`gpu_run_v2.py:37-41`) [V by reading] |
| od-base TEST | 123,753,984 [V] | `od-encoder` arch, 2 ranks | joinbase-6, one host with 4 x RTX 5090 (2 per island), 63 s reference [A] |
| mfu-dense-0.35b | 350,274,560 [V] | `scripts/mfu_profiles.json` | MFU only |
| dense-1b D20 | 889,260,032 [V] | builder geometry `dense-1b`, row `mfu-dense-1b` | S3 qualification, 2 rounds in 320 s [A]; a state object of 10.67 GB cannot complete [A, ADR 0013] |

The dense configs of the local "substitute qualification" runs (S12) are the one-layer fixture of
`mine_multi_round_v2` and S3's dense-1b triples; neither is a 100-300 M network run. There is no
model-profile registry in `src`: `ARCH_REGISTRY` maps an architecture name to a module, and a dense
config is a geometry of the `decoder` architecture. The only registry of dense geometries is the
profile file of the MFU tools (`hypertrain/scripts/mfu_profiles.json`, read by `run_mfu.py`; a stale
duplicate, `hypertrain/experiments/mfu/profiles.json`, is read by nothing and left alone), and the D20
builder already takes `dense-1b` from it.

## Decision

D68 (Principal SN100) gave the 100-300 M band and "intermediate first", not the exact size. Forge
(CTO) chose the size on 2026-10-28, within that band, in answer to the question this batch asked
through the relay tool: freeze **100,676,352 parameters** as the single profile, not the 209 M
target, because a run that fails for code reasons proves nothing. The budget below shows that even
this size fails today and passes only once prerequisites (a), (b) and (e) land.

1. **Profile row `mfu-dense-0.1b`** in `hypertrain/scripts/mfu_profiles.json`: arch `decoder`, 8
   layers, d_model 768, 12 heads, 4 kv heads (head dim 64), d_ff 2048, one expert, vocab 32768,
   seq_len 2048, bf16. Shape-only count **100,676,352** [V, `param_shapes`, printed by the builder
   as `param_count`]. It reuses the dense decoder path unchanged: no new architecture and no
   `ARCH_REGISTRY` row. "Registry entry" is read as this row; the builder takes the model block from
   it. Another rung (126 M, 209 M) is a new row plus a small builder edit (the geometry name in the
   case list, the row key, the epoch rule, the dry-run expression) and its pinned count in the tests.
2. **Builder geometry `HT_D20_GEOMETRY=dense-0.1b`** in `hypertrain/scripts/b300/d20/
   build_test_manifest.sh` (the brief names `scripts/ops/s100_test/build_test_manifest.sh`, which does
   not exist). Inner loop H = J = 4, micro_batch 2, grad_accum 1, `state_policy = reset`, AdamW,
   peak lr 3e-3 with warmup 2, codec dense-int8 at full density (the example manifest's outer block),
   layout `n_gpus 1`, the 4096-sample u32 fixture of `dense-1b` (33.6 MB); the B300 arguments (148 SM,
   driver 595.91.07) are the builder's. It refuses a row whose shape-only count leaves
   100,000,000..300,000,000 (inclusive) before any manifest file is written (the failed-run cleanup
   removes the seeds and directories the run created).
   - **J = H.** A result keeps `H/J + 1` checkpoints of 12 P bytes each, and `validate_artifacts`
     requires checkpoints 0 and H; J = H gives the fewest (two), so S = 41.016 P instead of 53.016 P
     at H/J = 2. Compute is seconds, not milliseconds: about 1 s a step in math attention [H], 3.9 s
     at H = 4, small against the transfers. H = 4 stays under the 30 steps of the 100-round audit
     lease (`lease_rounds` 100 is a literal of the builder's audit policy). Prerequisite (e) changes
     H and with it the budget; see "Budget at H >= 31".
   - **`work_screen_epoch_rounds` 20000** for this geometry only (the others keep 6000, so the
     tiny-cpu and dense-1b run ids are unchanged). `docs/network-v2.md` requires the first screen to
     cover the whole serial campaign: 52 trials of 83-130 rounds (250-390 s) plus about 470 rounds to
     the `d_final` of round 0 (the document's worked example, 4106 rounds, minus its 36 trials) is
     4,800-7,200 rounds against 6,000, so 6,000 holds only on the fastest link. The value is part of
     the admission policy hash, hence of the run id, and it is also the window of the rotation budget
     and the IP-prefix quota epoch (`now // work_screen_epoch_rounds`), which refill every 16.7 h
     instead of 5 h.
3. **Deterministic init** is the existing decoder init (Philox per tensor index, seed 0, std 0.02).
   The state hash of the initial parameters, which the builder signs as `init_state_hash`, is
   `be9b7bd63f79e4d7a3e640f51e9cfcc03e445a9de9ab249bcf77d938b8ee28f8` [V]; two inits are bitwise
   equal (2.5-3.9 s each, 1.65 GB peak RSS with the torch import) and the pin holds under the
   AVX2-capped dispatch CI uses.
4. **No `src/hypertrain` change.** The run needs code that does not exist yet ("Prerequisites"); this
   lot documents it instead of widening the scope.

### What the builder writes (for the later operator)

`OUT/out/`: `index.json` (`run_id`), `manifest-envelope.json`, `manifest.json` (its sha256 is the run
id), five `policy-*.json`, `relay-registry.json`. `OUT/inputs/`: `training.json`, `policies.json`,
`relays.json`, `reference-spec.json`, `public-keys.json`, `genesis-allocation.json`, `exp-round.txt`,
`samples.u32`, `proofs.json`; plus `secrets/` and `state/`. They are the files of the `tiny-cpu`
build, with the same consumers (see the runbook). A run id depends on every field of the manifest and
the builder draws random seeds, so every fresh build has a new id; the stable facts of this geometry
are `param_count` 100,676,352, `init_state_hash` above and the policy hashes, which differ from the
tiny build's in the admission epoch (20000) and in the audit policy's `max_steps_per_attempt` (2 H = 8).

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
| first reference after a new GPU image digest | 57.3 s, holds the store lock | V (S15) |
| B300 math-attention cost, b4 h16 T2048 hd128, one layer | 243-260 ms; flash 1.75 ms | V (S3); hd 64 not measured |
| CPU init of the profile | 2.5-3.9 s; real-size round (2 steps, 1 x 32 tokens, replay included) 19-27 s, 5.8 GB, one thread | V (this host, load 90-200) |
| gate stays closed after a store lock of T seconds | about 7.3 T (63-147 s after 13 s) | V (S15), extrapolated H |
| `validate_artifacts` 125-133 ns per parameter; challenge CPU 8.9 s/GiB (reference), 10.9 s/GiB (audit) | 13 s per pass; 34 / 42 s on S | A (ADR 0013, EPYC 9575F; s100 is a Zen 3) |
| `copy_flags` per delta / per pair at 208.7 M (P log P), scaled | 28-34 s / 4.6-5.8 s | H (measured at 4-32 M; 54 s for two deltas and a pair at 100.7 M in review) |
| ssh link A to s100 | 105 / 30 / 10 MiB/s at 10 / 50 / 150 ms RTT; 50 MiB/s interpolated | A (ADR 0013, emulated); RTT never measured |
| miner PUT of 1 MiB parts / GET of an object | 29-114 MB/s / 88 MB/s | A (joinbase-6 bandwidth, other hosts) |
| R2 from a GPU host | not measured; 18.8 up / 36.7 down MB/s from one non-GPU host | A (ADR 0003) |

Formulas ([H], `k = P / 100,676,352`, link `r`, `S` and the inputs `I = 8 P` in bytes):

- reference under the lock: `T_ref = stage 10k + I/r + (13 + gpu + 14k) + S/r + 8.9 S[GiB] + 3`, where
  `gpu = H x layers x (mb/4) x 0.245 s` (math attention, 3.9 s here);
- miner side of a trial: `T_miner = 10 + I/88 MB/s + 15 + gpu + 14k + 2 x 13k + 17.016 P/(29..114 MB/s)`;
  `T_trial = T_miner + T_ref + 17k` against the fixed 297 s window (`challenge_rounds` 100);
- audit: `T_audit = 12k + I/r + (13 + gpu + 14k + 8k) + S/r + 10.9 S[GiB] + 6k`; five audits in series on
  one executor host against 600 s (`d_final - d_audit` <= 200 rounds, `live_operator`) and 300 s per
  lease;
- aggregation: `copy_flags` passes under the store lock, `n x delta + n(n-1)/2 x pair` each.

The constants carry two known errors in opposite directions: the audit path has six
`validate_artifacts` passes at HEAD (call sites in `cuda_executor.py:949,970`, `auditor/worker.py:191`,
`island_bisect.py:182`, `store.py:7781,8129`), ADR 0013 counted four (up to 25 s more per audit); and
the 125-133 ns per parameter was measured on a result of 70 bytes per parameter, so a result of 41
bytes per parameter validates faster (7.7 s instead of 13 s a pass).

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

Reading [H]:

- **Lock and gate.** With the lock-holding reference of today's code, the gate closes for about
  7.3 T after a lock of T: 900-1700 s for the profile. A trial survives a lock of at most
  `(297 - 26)/(1 + 7.3)` = 33 s (23 s at the worst factor of 11); the S15 model gives 27-54 s at a
  zero probe. The reference reaches 33 s at about 16 M parameters on a 105 MiB/s link (10-36 M for
  27-54 s). Every identity would strand, as one did at 57 s in S15. Part B of ADR 0012 is needed at
  any size of this lot.
- **Trial window.** With Part B, the profile fits 297 s only on a fast link: `T_trial` crosses 297 s
  at 43 MiB/s (fast miner PUT) or 73 MiB/s (slow PUT), and 270 s, the runbook's go/no-go, at 58 or
  125 MiB/s. 101 M is the largest row of the band whose best-link, lower-bound case fits at all
  (233-277 s); 126 M has 283-338 s and 209 M fails everywhere. The size is a hypothesis to confirm
  with the first warm trials.
- **Audits.** Five in series need 718 s at best against 600 s, and a verdict must be accepted inside
  its own 300 s lease. A second executor host relieves the 600 s cap and the 2 x T_audit use of a
  lease on a shared host; a roster of four relieves only the 600 s cap, and only on a fast link
  (4 x 144 s = 576 s at 105 MiB/s, 4 x 193 s = 772 s at 50 MiB/s). The lease no longer bounds the
  rest of the chain on an ordinary run (no `service-admission` record, the only kind `live_operator`
  serves): since `5bc7524d` the ledger judges the stored commit and verdicts at the beacon of their
  `accepted_v2` row and the coordinator's audit challenge at the `created` beacon of its audit
  lease, and `live_operator finalize` seals the Finalize until `max(d_final, now) + 1000`, so the
  aggregate and the finalize may follow the last verdict [V by reading, `escrow_v2.py:486-500`,
  `store.py:2353`, `live_operator.py:253-259`]. At `8c2b24e8` they had to end inside the lease (about
  285 s at H <= 30).
- **Aggregation holds the lock too.** `aggregate_v2` and `finalize_v2` take the store lock, so the
  7.3 T rule applies to them: a 372-454 s aggregate (two passes) closes the gate for 2,700-3,300 s,
  one pass of 186-227 s for 1,360-1,660 s, and the finalize is a 503 meanwhile. (b) must take
  `copy_flags` out of the lock or make it about six times faster, not only compute it once. The
  aggregate call stays under `live_operator`'s 600 s client timeout (`live_operator.py:59`), and no
  verdict horizon bounds it or the finalize on an ordinary run (previous bullet); a late aggregate
  adds one exclusion to the missing-delta one: a member whose admission no longer qualifies at the
  call (`_exclude_unfit_members_v2`, `store.py:6583`).
- **R2.** For an audit result the transfer legs take 334 s at 18.8/36.7 MB/s, 90 s at 100/88, 18 s at
  500/500, against 131 s over ssh at 30 MiB/s. R2 beats ssh above 231 / 107 / 64 / 21 MB/s each way
  against ssh at 105 / 50 / 30 / 10 MiB/s. Fitting the 297 s window at 30 MiB/s ssh needs 102-221
  MB/s each way, and the reference over R2 needs Part B and `reference` in `operations`; only
  scenario C of ADR 0013 (500/500 [?]) does.
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
Until (e) lands, `live_operator` caps `d_final - d_audit` at 200 rounds (600 s). Math attention still
pushes the trial to 288-385 s, over the 270 s go/no-go everywhere and over the 297 s window at J = 16
and 8, so (d) is still needed at H >= 31. Whether a second executor host is still needed at H >= 31
is no longer shown by the lease arithmetic, which was the only reason given here; Principal decides
it, see (e). Without (d) the profile can only be run at H = 4 for the trial window, and there it
needs a second executor host (or the roster of four at 105 MiB/s), because the 600 s `absolute`
cannot hold five audits on one host. This is the honest state of the profile: it is the band floor
and it sits at the edge of the pipeline.

## Prerequisites

The profile cannot pass a run on the current code. Each item is separate, with an owner proposal, a
size estimate [H] and whether it is needed at 101 M. None is in this lot (no `src/hypertrain` change).

| # | What | Where | Owner / size | Needed at 101 M |
| --- | --- | --- | --- | --- |
| a | Lock-free remote reference (ADR 0012 Part B): release `store._lock` during the executor job | `challenge/store.py:3046-3116`, `admission.py` | Forge, session `v3-executor-lock-release` (in progress); M | **Yes.** T_ref 123-235 s of lock close the gate for 900-1700 s; a trial tolerates about 33 s. |
| b | `copy_flags` three times under the lock in aggregation and finalize | `aggregator/core.py:341-365` (called at `weighted_v2.py:225` through `tape_v2.compute_body`), `store.py:8762,8775`, `ledger/escrow_v2.py:542` | aggregator session; M (60-100 lines plus a bitwise-equivalence test); moves `implementation_hash` | **Yes.** 9.3-11.3 min under the lock against a gate that closes 7.3 times as long. The fix must keep the replay a recomputation (the tape stores `copy_suspicion` and `replay_tape` compares), so "exactly faster" or out of the lock, not a cached result. |
| c | Miner HTTP client timeout 120 s while the proof POST holds the whole reference | `miner/core.py:309`, `miner/cli.py:237,406,460` | miner session; S (about 10 lines plus a test) | **No for this run**: `cpu_miners.py` treats `ReadTimeout` as transient and re-posts (`:47-54`), costing one or two wasted POSTs per trial (T_ref + 17 s = 140-250 s against 120 s; the re-post waits behind the first). Yes for the CLI that a real miner uses. |
| d | Attention backend table: no `(10, 3)` entry, so B300 replays in `math` | `trainer/attention.py:42-46`; the executor forwards no `HT_ATTN_BACKEND` | Forge decision, needs a B300 qualification at hd 64; moves `sources()` | **No at H = 4** (about 4 s per island, 3 % of an audit). **Yes if (e) takes H >= 31**: math attention adds 30 s at H = 32 and puts the trial over the 270 s go/no-go (288-385 s) and, at J = 16 and 8, over the 297 s window. Do not set the variable on the miner side only: math and flash are different replay domains. |
| e | Lease and offsets for H >= 31: `lease_rounds` derived from H and `step_seconds_bound`, a round opener whose `d_final - d_audit` exceeds 200, **and a second executor host** | `build_test_manifest.sh` (literal 100, about 10 lines), `scripts/ops/live_operator.py:108-118` (cap 200, about 5 lines); the second host is config only | Forge; S | **Yes.** At H <= 30, five audits take 718-1279 s against 600 s and a 300 s lease; a second host relieves the 600 s cap and the shared-host lease, four miners only the 600 s cap and only on a fast link. At H >= 31 the lease arithmetic no longer shows that a second host is needed (previous section): Principal decides it. |
| f | Result transfer path, ssh or R2, and the tooling to switch | `challenge_image.sh executor` can set only `HYPERTRAIN_CUDA_EXECUTOR`, not `HYPERTRAIN_CUDA_EXECUTOR_ARTIFACTS` (`:204-232`); R2 never measured from a GPU host | Forge decides after the measurement below; S script extension plus its test | **Depends on the measured link.** ssh is enough at 105 MiB/s; at 30 MiB/s the reference does not fit and R2 would need Part B plus `reference` in `operations` and 100-220 MB/s each way. |

Other gaps found, not on the list above: other holders of the store lock close the gate for about
7 times their duration too (`open_round_v2` about 14 s a roster entry at 101 M, `complete_audit_v2`
with a validate pass inside the transaction, `export_trial_v2` staging); the client side of `open`
builds five roster anchors first (about 10 s a hotkey [H]) and `d_open` must be ahead of that;
`challenge_image.sh beacon` refuses a catch-up other than 60 (enough for a lock of 150 s);
`cpu_miners.py` stops on one 404 of a trial object (S15 D2) and needs a watchdog; any rebuild gives
new image digests (the build label carries the commit) and a cold compute cache (S15 D1).

**Measurements that settle (f) and the window, to run in the later session, not here** (commands
only; none was run):

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
  `role-dispatch.json`. Also check the CA bundle of the GPU image for https
  (`ssl.get_default_verify_paths()` in a `--rm` container; ADR 0013 lists it as [?]). [ADR 0013:
  written, never run; no bucket-scoped token existed.]
- The profile's step time on B300: `scripts/run_mfu.py --profile mfu-dense-0.1b --device cuda --out
  <file>` from a checkout on a host of ours, not on the rented nodes (the GPU image has no `scripts/`).
  The row carries the MFU loop (micro_batch 4, H = J = 60), not the TEST loop (micro_batch 2, H = 4):
  halve the step time for micro_batch 2.

## R2 artifact configuration for the TEST bucket

Names only; no value is stored or printed here. The settings file named by
`HYPERTRAIN_CUDA_EXECUTOR_ARTIFACTS` is `hypertrain/docs/examples/cuda-executor-artifacts.example.json`
with two changes for this model: `credentials_file` = the container path of the credentials file
(proposal: `/data/executor/r2.env`, mode 0600, owner uid 65532, put on s100 by stdin) and `fallback` =
`fail`, so that an unreachable R2 is visible instead of becoming a silent 4 GB ssh transfer.
`operations` stays `["audit", "referee"]` (a reference holds the store lock), `workers` 8,
`part_bytes` 16 MiB gives about 247 parts for S, and `max_artifact_bytes` 16 GiB is above S = 4.13 GB.
The credentials file holds only `R2_ACCOUNT_ID`, `R2_ACCESS_KEY_ID`, `R2_SECRET_ACCESS_KEY`,
`R2_BUCKET` (a `cortex-v3-test-*` bucket) and optionally `R2_ENDPOINT`; the process environment is
ignored. `reserve_seconds` 60 plus the upload must fit the lease: R2 is skipped
(`exec_artifact_skipped`) when a lease has less than `reserve + 30` s left. The bucket needs the Forge
prerequisites of ADR 0013 (a bucket-scoped token, lifecycle rules on the literal prefix `exec/`, a
ledger line, at most 50 USD).

## Hash consequences

Measured with `network_qualification.sources()` and `bundle()` and `ChallengeStore._service_implementation_v2`
on `git archive` of each commit (Python 3.12 of the project venv), 2026-10-28 [V]: `8c2b24e8` (the
origin/v3 this lot was built on), `3036f8e7` (its code commit), `2cfc2a2b` (origin/v3 now) and `350e50b1`
(the merge of the two). The values at `3036f8e7` equal those at `8c2b24e8`, and those at `350e50b1` equal
those at `2cfc2a2b`: this lot moves none of them, and every move below comes from the 31 origin/v3
commits (`git log 8c2b24e8..2cfc2a2b`). `sources()` is given in the canonical JCS form of ADRs 0007,
0008, 0010 and 0012; ADR 0013 used `json.dumps(sort_keys)`, whose value is `84b17675…c285f4` at
`8c2b24e8` and `3623f281…dce0a4` at `350e50b1`.

| Value | At `8c2b24e8` and `3036f8e7` | At `2cfc2a2b` and `350e50b1` |
| --- | --- | --- |
| `implementation_hash` | `941247f8…a688d1` | **moved**: `fc9e87b0…e4c8fe` |
| `challenge/store.py` sha256 | `6eadf5bb…1e3972` | **moved**: `e9c9fb36…58acde` |
| `network_qualification.py` sha256 | `dab7a6ab…7bda15` | unchanged |
| `cuda_executor.py` sha256 | `f342234d…7db619` | unchanged |
| `cuda_executor_runner.py` sha256 (`runner_sha256`) | `5930be17…4dff90` | unchanged |
| `sources()` canonical sha256 | `0d989cd1…987bf9`, 153 files, 2,391,442 B | **moved**: `82d9a332…e2081b`, 155 files, 2,427,686 B |
| `bundle-hash` | 2,519,040 B, `0fbf1c09…280721` | **moved**: 2,549,760 B, `50e26ecd…96345e` |
| run id of `tiny-cpu` / `dense-1b`, fixed seeds (`0x1001`..`0x1007` for owner, coord, auditor-0, auditor-1, referee, cold-0, cold-1), image `sha256:` + `cd` x 32, driver 595.91.07, 148 SM, 2 coldkeys, exp round 34310319, relay key of `bytes(range(1, 33))` (`baseline/run_baseline.sh` in the evidence folder) | `a55da95e…74ab8e` / `ef0fc003…83ebfe` | unchanged |
| `dense-0.1b`, added by `3036f8e7` (absent at `8c2b24e8` and `2cfc2a2b`) | at `3036f8e7`: `param_count` 100,676,352; `init_state_hash` `be9b7bd6…ee28f8`; run id `1cf7d790…71a3fb` on the same inputs | at `350e50b1`: the same three values |

What moved and why [V]: `implementation_hash` covers ten files and 35 owned names of `store.py`; three
of the files (`aggregator/rollback_v2.py`, `miner/island_launch.py`, `trainer/config.py`) and seven
of the names changed (`open_round_v2`, `aggregate_v2`, `finalize_v2`, `bootstrap_service_capacity_v2`,
`_service_implementation_v2`, and the two new `_unfit_cause_v2` and `_exclude_unfit_members_v2`).
`sources()` and the bundle moved because 16 `src/hypertrain` files changed and two were added
(`protocol/env_pins.py`, `trainer/env_probe.py`); the seven fixed files (`Dockerfile.gpu`,
`pyproject.toml`, `uv.lock`, `experiments/gpu_network_v2/*`, `scripts/network_gpu_qualification.py`)
did not. The run ids and the policy, manifest and registry files of the three geometries were rebuilt
at `350e50b1` (and, for `tiny-cpu` and `dense-1b`, with the builder of `2cfc2a2b`): the manifest bytes
are identical, so v3's `reference_spec.env.pinned` (opt-in, omitted when absent) changes no run id.

This lot touches `scripts/mfu_profiles.json`, `scripts/b300/d20/build_test_manifest.sh`, tests and
docs only, none of which feeds `sources()`, the bundle or `implementation_hash`. The envelope and
`index.json` differ between two builds of the same inputs (the signature is randomised), and the
builder's printed dry-run JSON renames its check `work_screen_epoch_rounds_6000` to
`work_screen_epoch_rounds` for every geometry; nothing reads it. Consequences:

- This lot invalidates no qualification config, dispatch source pin or reviewed bundle. The merged
  origin/v3 commits do: a record, pin or review made against the `8c2b24e8` values above does not match
  `2cfc2a2b`.
- The images of S15 (server `a513821d`, GPU `331cec82`, built from `e0969448` [A, S15 report]) are
  not the code of HEAD: 25 `src` files changed since (10 at `8c2b24e8`; `git diff --name-only
  e0969448 350e50b1 -- hypertrain/src`), the owner routes `/challenge` and `/job` went from `GET` with
  headers to `POST` with a signed body after it (`13ec158c`, ADR 0010), so a HEAD miner cannot talk to
  the S15 server image [V by reading], and the genesis origin ids are now keyed (ADR 0014:
  `genesis_origin_id` takes `escrow_id_key` of the coordinator secret as a fourth argument;
  `publish_run.py genesis` runs inside the server container and imports it, so it cannot run in the
  S15 server image) [V by reading]. A replay must run server, miner and operator tools from the same
  side of both changes.
- The GPU container path is unaffected for a manifest without `reference_spec.env.pinned`. Its import
  closure (a static walk from `hypertrain.miner.island_worker`, the hand-off of
  `gpu_ops/cuda_executor_runner.py:206`) meets ten of those 25 files (under `src/hypertrain/`): in
  `protocol/` `env_pins`, `envelope_v2`, `messages` and `messages_v2`; in `trainer/` `config`,
  `determinism`, `island` and `loop`; in `miner/` `island_launch` and `island_worker`. Two
  (`envelope_v2`, `messages_v2`) are the additive request types of ADR 0010; the other eight are the
  env-pin edits of ADR 0011, which do nothing when the pin is `None`, so the manifest bytes, the run
  id and the rank summary stay the same [V by reading]. `trainer/env_probe.py` is outside the
  closure. The S15 image refuses a manifest with `env.pinned` (its `ReferenceEnv` has three fields and
  forbids extras). So the island worker inside `331cec82` would run an unpinned manifest; but the
  miners run their CLI from the manifest's image (`docs/s100-test-stack.md` section 6, `cpu_miners.py`
  with `HT_MINER_DEVICE=cuda`), and the CLI of an image built from `e0969448` sends `GET` with
  headers to `/challenge` and `/job` (`miner/core.py` at that commit), which a HEAD server refuses
  (`challenge/app.py` registers `POST` only): against a HEAD server a manifest cannot pin `331cec82`
  [V by reading; the build commit of the image is [A]]. Any rebuild gives new digests (the build
  label carries the commit), hence a new run id and a cold compute cache (S15 D1).
- A new run id per build and per image digest, as always.

## Tests and mutants

`hypertrain/tests/`, all on CPU in default CI, about 50-70 s added with 5.8 GB at the peak of one
test (below the 8.8 GB od-base round, `test_model_registry.py::test_from_scratch_round_bitwise_and_replays`,
which sits in one shard). Three new files shift the round-robin split of later files by a shard
or two:

| Behaviour | Test | Mutants killed |
| --- | --- | --- |
| the row is the pinned shape | `test_mfu_profiles.py` | 12, 24 or 2 layers; d_ff 2816; vocab 65536; 12 kv heads; 24 heads |
| deterministic init at full size, pinned | `models/test_dense_intermediate_golden.py` | init index shared; std doubled |
| the band is inclusive and applies to the row | `test_b300_d20_builder.py::test_the_dense_0_1b_row_must_stay_inside_the_inclusive_100_to_300_million_band` (99,999,998 refused, 100,000,000 and 300,000,000 accepted, 300,000,002 refused, by builds that stop after the first stage; only even counts exist, so ±2 is the tightest edge) | bounds 0, 10 M, 10^12, 3 G; either edge exclusive; guard inverted or not exiting |
| the build reads the row and signs the loop, the fixture, the layout and the window | `test_a_dense_0_1b_build_writes_the_registry_row_and_a_wide_screen_window`, `test_a_good_build_...` (tiny keeps 6000) | count + 1; 2048 samples; token id equal to the vocab; shard root over truncated bytes; window always or never 20000; J = 2; state policy carry; rewarmup 5; peak lr 3e-4; warmup 0; the 0.35b row; two ranks; geometry not accepted; dry run expecting 6000 |
| unknown geometry refused, known ones named | `test_an_unknown_geometry_is_refused_and_the_known_ones_are_named` | any geometry accepted |
| serialised sizes and replay at real size | `models/test_dense_intermediate_round.py` | int8 block 256 -> 128 |

Each mutant was applied to the real file, its tests run with `pytest -x`, and the file restored (the
unstaged `git diff` was empty afterwards); a kill counts only on a failed test, never on an error
(an earlier run counted four transient errors as kills and was rerun). **34/34 killed** on CPU,
2026-10-28 [V]; the driver and its output are in the evidence folder. The real-size round is a scale
proof of existing engine code, not a regression test of this lot: a reduced-size round would pass on
the old code as well. A local end to end (join, probation, run-v2, audits, `get_weights`) at this size
is not possible on CPU: the harness (`mine_multi_round_v2.py`) is wired to the one-layer fixture and
its 1 MB object caps, and runs about 70 train/replay units; the largest local proof is the builder's
dry run (the real validators of `POST /v2/admin/runs`) plus the round test above.

## Rejected

- **209 M (12 layers, d_model 1024, 208,692,224)**, the D68 target: statically valid, but every
  time model fails it (trial 435-526 s even at 105 MiB/s against 297 s; five audits 1408 s; an 8.56 GB
  result). Measured here: init 6.3 s, state 2.50 GB, one real CPU step 223 s at seq 2048 [V,
  contended host]. Kept as a follow-up row.
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
  start with this model's kernels (the compute cache may not carry over from the tiny model [?]),
  s100 CPU, RAM and free disk, math-attention time at head dim 64, trace-hook cost and its linear
  scaling with J, the number of validation passes at runtime (six by reading).
- That a run id built here is accepted by the TEST challenge image: the dry run applies the repo's
  validators, not the image's. `POST`ing a run before its CUDA qualification record freezes its
  backend to `cpu`.
- That the server keeps working on a proof POST after the client gave up at 120 s (assumed by (c)),
  and whether the GPU image carries a CA bundle for https (ADR 0013 [?]).
- The provenance of the S15 images (`a513821d`, `331cec82`, built from `e0969448`) comes from the S15
  report only; the import-closure claim was checked by reading against `e0969448`. At `8c2b24e8` the
  closure met two files (`protocol/envelope_v2.py`, `protocol/messages_v2.py`); at `350e50b1` it meets
  ten, eight of them env-pin edits read as inert for a manifest without `reference_spec.env.pinned`.
  The case with `pinned` was not examined.
- The prerequisites' sizes and owners are estimates; (a) is in progress in another session and was
  not read here.
- Whether the sacrificial-identity warm-up of the runbook populates the cache entries the island
  needs (ADR 0012: the mechanism is not established).
