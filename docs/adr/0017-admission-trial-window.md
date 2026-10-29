# ADR 0017: the admission trial window as a policy value (D94 lot C)

- Status: **prepared, not merged**, 2026-10-29 (Principal SN100 D94 lot C). The window is a protocol value,
  so the founder decides; Principal SN100 relays that decision, and nothing here is merged before it.
  Branch `v3-trial-rounds` on origin/v3 `667695be`, with origin/v3 `708414d8` merged in. No
  production run, master, profile, pin or emission changes. Nothing in this lot ran on a GPU host, on
  s100, against R2 or on-chain. The one-page sheet for the founder is the last section but one.
- Order: brief `v3-trial-rounds` ("make admission trial window MANIFEST parameter, PREPARE ONLY"). Written
  as 0016, renumbered 0017 by Forge (CTO), 2026-10-29: `v3-model-50m` (`708414d8`, now on origin/v3) took
  0016 for `docs/adr/0016-d94-dense-test-model.md` [V].
- Labels: **[V]** verified in code, by a test or by a command at `667695be` or on this branch,
  2026-10-29; **[A]** reported by a cited document; **[H]** hypothesis or arithmetic; **[?]** unknown.
  Paths are relative to `hypertrain/src/hypertrain/` unless prefixed.
- Related: ADR 0002 (E1/E2, the proof window), 0003 (the 297-300 s window of a trial upload), 0010
  (trials expire on the clock, dead applications, the operator's reseed, signed owner requests), 0011
  Part 9 (`reaudit_rounds`, the pattern this follows), 0012 (`REFERENCE_WAIT_SECONDS`), 0015 (the 101 M
  budget).

## Context

A trial is a join challenge: the coordinator signs `JoinChallenge{seed_beacon, deadline_beacon}`, and
the proof must be accepted by `deadline_beacon` (the deadline round itself still accepts one). At
`667695be` the window is fixed at 100 beacon rounds, about 297-300 s at 3 s a round [V]:

| Where | At `667695be` |
| --- | --- |
| `protocol/messages_v2.py:645` | `AdmissionPolicyV2.challenge_rounds: Literal[100]` |
| `protocol/messages_v2.py:586-587` | `JoinChallenge` refuses `deadline_beacon != seed_beacon + 100` |
| `challenge/admission.py:437` | `deadline_beacon=seed + 100`, the only hard-coded runtime use |
| `ops/manifest_v2.py:197`, `scripts/b300/d20/build_test_manifest.sh:399` | the example and the TEST builder sign 100 |

Every other user reads the policy or the signed challenge, so it follows whatever window a run signs [V
by reading, each pinned by a test or a mutant below]: `CHALLENGE_EXPIRED` (`admission.py:420`), the
dead-application reaper `APPLIED_EXPIRED` (`:353`), the trial reaper `reap_expired_trials` (`:383`,
`deadline_beacon < now`), `RESEED_NOT_NEEDED` (`:1391`), the coordinator's seal of the challenge (expiry
`deadline_beacon`, `:462`), the proof route (`:967`), the reference claim (`:618`), the miner's signed
proof and screen (expiry `challenge.deadline_beacon`, `miner/core.py:944-947`), the wall-clock deadlines
of the trial jobs (`challenge/store.py:3293`, `:3817`) and the work screen (`gpu_ops/work_screen.py:93`).

ADR 0015 puts the 101 M TEST model at `T_trial` 233-277 s at 105 MiB/s, 282-326 s at 50 MiB/s and
345-389 s at 30 MiB/s, against the 297 s window [H, ADR 0015 "Budget", unmeasured]. The one link
measured from s100 carried 7 MB/s [A, `docs/s100-test-stack.md` 6.4a]. The miner side alone takes
about 282 s [A, brief]. 126 M needs 283-338 s at the best link, 209 M 435-526 s, and dense-1B cannot
finish a trial at all (a 10.67 GB state; ADR 0013) [A]. D94 asks for the window to become a manifest
value, by analogy with `reaudit_rounds`, without moving the digests and run ids that exist.

The full inventory (`rg "challenge_rounds|CHALLENGE_ROUNDS|deadline_beacon|\+ 100|OWNER_REQUEST|exp_drand"`
over `src`, `tests`, `scripts` and `docs`, 238 lines) is in the scratch evidence of this lot and in
REPORT.md. The other `100`s it finds are not the trial window and stay as they are [V by reading]:
`lease_rounds` 100 and the audit `absolute_rounds()` `+ 100` (`messages_v2.py:718`), the lineage
horizon (`store.py:6110`), relay `retain_until` (`store.py:12526`) and the auditor receipt
(`auditor/__main__.py:95`).

## Decision

1. **`challenge_rounds` becomes a bounded integer**, `Annotated[StrictInt, Field(ge=100,
   le=CHALLENGE_ROUNDS_MAX)]`, with `CHALLENGE_ROUNDS_MAX = 400` (20 minutes at 3 s, about 1.8 times the
   measured warm 101 M trial, 566 s). It is **still serialised at 100**, not omitted. The field already exists, and
   `"challenge_rounds": 100` is in the bytes of every admission policy ever signed. So keeping it
   serialised is what leaves those digests unchanged. Omitting it at 100, the `reaudit_rounds` pattern
   the brief names, would move every one of them: the `tiny-cpu` builder policy digest would go from
   `d0a5538d…14a499` to `8cad96ce…b6bf7b` [V, computed]. That pattern works for `reaudit_rounds`
   only because that field was new. A `bool`, a float or a string is refused (`StrictInt`), as are 99
   and 401 [V].
2. **`JoinChallenge` takes any window a policy may set; the run's policy fixes the one it gets.** The
   schema accepts `100 <= deadline - seed <= CHALLENGE_ROUNDS_MAX`, because a wire-model validator
   sees the message alone, never the policy it names. The tie to the policy is a separate,
   explicit check, `JoinChallenge.validate_policy(policy)`: the challenge names the policy by
   digest and lasts exactly its `challenge_rounds`. Each side enforces it where it can:
   - **Service.** It builds every challenge from its own policy (`admission.py`, `seed +
     self.policy.challenge_rounds`), and `Admission.__init__` refuses a policy whose digest is not
     the manifest's `admission_policy_hash` (`ADMISSION_POLICY_HASH`). No other window can be
     issued, so the service does not re-check its own output.
   - **Miner.** `probe-v2` (no `--job`) reads the run's admission policy by the digest the
     manifest signs, checks its sha256, and refuses a challenge that fails the tie with `trial
     challenge refused: …` (exit 2). It does this after the signer and binding checks and before
     any trial input is fetched. A miss on the policy object fails as a trial input miss does
     (`ChallengeHTTPError: 404: trial input object unavailable`), so the s100 driver retries it
     the same way. This catches a coordinator-signed challenge that names another policy or
     stretches the window, for example from a coordinator key reused across runs or a buggy
     service. Before this change the miner would have proved such a challenge to its deadline.
   - **Not in `probe()` itself.** `probe-v2 --job --challenge` takes a folder from
     `trial_export`, which reads the challenge from the coordinator's own store. The direct
     `probe()` callers (`scripts/network_gpu_operation.py`, the admission fixtures) pass fixed or
     fake policy hashes and have no policy object to read.
3. **TEST ledger only, until the founder rules.** `AdmissionPolicyV2.validate_ledger(ledger_mode)`
   refuses a window above 100 unless the economics policy is `test`. Every way in calls it:
   `ops/manifest_v2 build` (exit 2) and `POST /v2/admin/runs` (`_create_run_v2`, 422) next to
   `validate_training`, the D20 builder's dry run, and `Admission.__init__`
   (`CHALLENGE_ROUNDS_TEST_ONLY`) next to its policy hash binding. The last one covers a service
   built over a ledger that never went through run creation. This is the D88 `gap_cap_rounds` rule
   applied to a second bound.
4. **A derived value, not a guessed one.** `challenge_rounds_for(trial_seconds)` returns
   `max(100, ceil(1.15 x trial_seconds / 3))` and refuses a trial the ceiling cannot hold. Its argument
   is the measured `T_miner + T_ref` of a trial, in whole seconds. Examples: 282 s gives 109, 297 s
   gives 114, 389 s gives 150, 521 s gives 200, 566 s (the measured warm 101 M trial) gives 217,
   1043 s gives 400, and 1044 s is refused [V]. The policy carries the
   number, not the seconds. Like `reaudit_rounds_for`, the helper is how the operator chooses it.
5. **TEST builder knob** `HT_D20_CHALLENGE_ROUNDS` (100..400, default 100) in
   `hypertrain/scripts/b300/d20/build_test_manifest.sh`. Values are digits only, with no sign, space,
   leading zero or exponent; anything else exits 2 before anything is written. The dry run checks the
   signed window and prints `challenge_rounds`. Unset, the builder signs the bytes it signed before [V].
6. **Left unchanged on purpose** [V by reading, test below]: `OWNER_REQUEST_ROUNDS` = 20
   (`miner/core.py:62`), `OWNER_REQUEST_MAX_ROUNDS` = 100 (`challenge/app.py:54`, `:605`) and
   `REQUEST_HORIZON` = 20 (`app.py:647`, `:847`, `:1119`). They bound how long a captured *request*
   (`ChallengeRequest`, `JobRequest`, bond and dispute polls) can be replayed, not how long a trial
   lasts. The miner signs a fresh request for every ask. The CLI test below proves a trial 150 rounds
   after its seed with those bounds untouched.

## Consequences

**Hashes** (measured on full `hypertrain/` trees of `667695be`, `708414d8` and this branch's tip
with `network_qualification.sources()`, `bundle()` and `ChallengeStore._service_implementation_v2`,
2026-10-29 [V]; `667695be` and `708414d8` give the same value for every row):

| Value | `667695be` = `708414d8` | This branch |
| --- | --- | --- |
| `implementation_hash`, all five pins (`avx2`/`none` `7af135ca…70be62`, `avx512`/`none` `c509dfd1…83a920`, `avx2`/`sm_103`/`math` `24fbe36d…c185ce`, `avx2`/`sm_103`/`sdpa-flash` `9bcf9bdd…2c2dd5`, `avx2`/`sm_120`/`sdpa-flash` `c8af2ae7…68891f`) | as listed | **unchanged** |
| `challenge/store.py` / `challenge/admission.py` / `protocol/messages_v2.py` sha256 | `22fb5dcd…c56676` / `0d7fa656…d67ead` / `8e03a8a0…42bb04` | **moved**: `3edf2faa…0ab5bd` / `482d1f1a…41951c` / `bc9d1635…a270f9` |
| `miner/core.py` / `ops/manifest_v2.py` sha256 | `0a5a6f30…3d1618` / `d9616083…a4c989` | **moved**: `4f11cd62…896bc4` / `ad88b68e…888ffd` |
| `challenge/app.py` sha256 | `0e6ec421…e07d48` | unchanged |
| `sources()` canonical sha256 (158 files) | `97e3c592…5f1be3` | **moved**: `7414f4c4…96479d` (`json.dumps(sort_keys)`: `7a721848…b6b4bb` to `0f5cf259…1cf0bb`) |
| `bundle-hash` (2,775,040 B) | `2c064435…0b8196` | **moved**: `a314b987…7a2193` |
| example admission digest (both ledger modes), example `RunManifestV2` run id | `c3fd6d4d…b3e3e3`, `aa39197d…ec8abd` | unchanged; `manifest_v2 example` writes the same three files byte for byte |
| D20 builder, fixed seeds (ADR 0015 inputs): `tiny-cpu` / `dense-1b` / `dense-0.1b` at 144, 256, 300 s | `f3c12327…dcde3588` / `bbbff4bb…35e4a542` / `0ea43133…82da2af3`, `944b69ae…5bb46da6`, `b6bebaf7…ca41283d` | unchanged; `manifest.json`, every `policy-*.json` and `relay-registry.json` byte identical; the envelope differs only in its randomised `sig`, and `index.json` only in the envelope's hash |
| the same, ADR 0016 geometries: `dense-0.03b` / `dense-0.05b` at 144, 256 s | `b07a732e…1873fa75`, `fdef6c21…978681ea` / `118f29ee…ff7f365b`, `88106cac…fd4f2d36` | unchanged, with the same byte identity |
| the same `tiny-cpu` admission policy at 114 / 200 / 400 rounds | (refused) | `759066dc…e071e1` / `0be8078b…da33fd` / `2be2ca04…8214ec`, so a new run id |

The brief expected `implementation_hash` to move because "admission policy covered". It does not. That
hash covers 48 named definitions of `store.py` and 12 files, and none of them is touched. The new 422 is
in `_create_run_v2`, which is not one of the names [V]. A run binds the admission policy through
`NetworkSpec.admission_policy_hash` and so through its run id. No golden value is regenerated: the
pinned admission digest and run id (`tests/protocol/test_network_v2.py:140-141`,
`tests/ops/test_manifest_v2.py:339`) still hold. The moved bundle and sources mean that a qualification
config, a dispatch source pin or an image made at `667695be` does not match this branch.

**Images ship first.** An image built before this change refuses any policy whose window is not 100
(`Literal[100]`, `extra='forbid'`). Its miner CLI also refuses any challenge whose deadline is not
seed + 100 (`messages_v2.py:586` at `667695be`) [V by reading]. A run with a wider window therefore
needs server and miner images from this branch. Default runs need nothing new.

**Costs of a wider window** [V by reading the rules; durations are H at 3 s a round]:

- A dead unfunded application keeps its pending place (`max_pending` 32) and its coldkey until `seed +
  challenge_rounds`: 10 min at 200 instead of 5.
- A vanished miner's trial holds one of the run's two trial places (`max_trial_replays` is `Literal[2]`)
  until its deadline. Two such miners block every newcomer for the whole window. This is ADR 0010's
  residual, now as long as the window.
- The coordinator-signed challenge stays valid until its deadline. So does a capture of it, which
  ADR 0010 already allows.
- A stranded identity waits longer for its reseed: `RESEED_NOT_NEEDED` holds until `seed +
  challenge_rounds`. `probe-v2` asks for 10 minutes and then exits 3. At 200 rounds an identity that
  lapses early can exhaust those 10 minutes before a reseed is possible, so the operator restarts the
  driver after the reseed, as the runbook already says.
- The screen bound of a serial campaign grows. `network-v2.md` bounds a trial's span by about
  `challenge_rounds + 1`. At 200, 36 trials can take up to 7,236 rounds, which is more than the 6,000 of
  `manifest_v2 example` and the `tiny-cpu` build, and less than the 20,000 that `dense-0.1b` signs. The
  measured span is about 64 (joinbase-6) [A].
- `REFERENCE_WAIT_SECONDS` (330 s, `store.py:313`) is shorter than a window above 110 rounds, and
  shorter than the measured 101 M reference (347 s, sheet below). A second request for a reference
  that is still running then gets 503 after 330 s, and the client retries a 503 (ADR 0012). That is a
  retry, not a lost trial, and the leader request is not bounded by it. It is left unchanged; only its
  comment changes.

**Status codes.** A production manifest with a window above 100 is refused with 422 (`validate_ledger`).
A window outside 100..400 fails the policy schema and answers 409, like every other malformed policy,
because the v2 `ValidationError` handler answers 409 (`challenge/app.py:512-522`) [V]. The brief asked
for 422 there. Changing it would change how every policy refusal is reported, so it is left to Forge.

## Tests and mutants

All on CPU in default CI. Each test fails on the code before its fix (red logs in REPORT.md) and kills
at least one mutant. 22 mutants were applied to the real files and run with `pytest -x`, then the files
restored. **22/22 killed**, each by a failing test, never by an error [V, 2026-10-29].

| Behaviour | Test | Mutants killed |
| --- | --- | --- |
| a 200-round policy serves deadline seed + 200, the trial reaper waits for seed + 201, and `CHALLENGE_EXPIRED` comes at seed + 201 (real `Admission`) | `challenge/test_admission_v2.py::test_a_200_round_policy_serves_reaps_and_expires_on_its_own_window` | deadline stays seed + 100; `CHALLENGE_EXPIRED` at 100; reaper at 100 |
| a proof at seed + 150 is accepted | `::test_a_200_round_policy_accepts_a_proof_150_rounds_after_the_seed` | deadline stays seed + 100 |
| a dead application is reaped at seed + 201, not 101 | `::test_a_200_round_policy_moves_the_dead_application_round` | reaper at 100 |
| reseed is refused until the window ends and the new trial gets the policy's window | `::test_a_200_round_policy_moves_the_reseed_rounds` | reseed at 100 |
| the CLI miner proves a trial 150 rounds after its seed and signs proof and screen at seed + 200; the service accepts the pair | `miner/test_probe_trial_v2.py::test_probe_v2_proves_a_trial_150_rounds_after_its_seed_under_a_200_round_window` | miner expiry not following the challenge |
| round trip, new digest and new run id at 101, 200 and 400 | `protocol/test_network_v2.py::test_a_wider_trial_window_round_trips_and_moves_the_digest_and_run_id` | (see the golden test below) |
| 100 keeps the pinned digest and run id | the existing `EXAMPLE_ADMISSION_DIGEST` / `RUN_ID` goldens | 100 dropped from serialisation |
| 99, 0, -1, 401, `True`, `200.0`, `2.5`, `"200"` refused | `::test_a_trial_window_outside_100_to_the_ceiling_is_refused` | ceiling not bounded; floor not bounded |
| above 100 is for a TEST ledger only | `::test_a_trial_window_above_100_rounds_is_for_a_test_ledger_only`; `ops/test_manifest_v2.py::test_the_builder_refuses_a_production_manifest_that_lifts_a_test_only_bound[trial_window]` (exit 2); `::test_the_service_refuses_a_production_manifest_that_lifts_a_test_only_bound[trial_window]` (422, then the TEST twin accepted) | `validate_ledger` allows production; builder skips it; store skips it |
| `challenge_rounds_for` table and ceiling | `::test_challenge_rounds_for_covers_the_trial_with_a_margin_and_never_below_100`, `::test_challenge_rounds_for_refuses_a_trial_the_ceiling_cannot_hold` | margin 1.10; floor division; no ceiling |
| `JoinChallenge` takes 100, 200 and 400 and refuses 99, 0 and 401 | `::test_a_join_challenge_carries_any_window_the_policy_may_sign` | validator kept at seed + 100 |
| a challenge must name its policy and last exactly its window, for a 100- and a 200-round policy | `::test_a_join_challenge_must_last_exactly_its_own_policys_window` | tie skips the window; tie skips the hash |
| the CLI miner refuses a coordinator-signed trial stretched to 300 rounds, naming another policy, or naming a 300-round policy served in place of the run's; nothing fetched or uploaded | `miner/test_probe_trial_v2.py::test_probe_v2_refuses_a_signed_trial_that_is_not_its_policys[window, policy, swapped]` | miner skips the tie; miner skips the policy sha256; tie skips the window; tie skips the hash |
| `Admission` refuses a production ledger bound to a 200-round policy and takes the 100 twin | `challenge/test_admission_v2.py::test_admission_refuses_a_production_ledger_with_a_window_above_100_rounds` | `Admission.__init__` skips `validate_ledger` |
| the builder signs 100 unset and 200 on request, and the dry run checks and prints the window | `test_b300_d20_builder.py::test_the_trial_window_is_100_rounds_unless_the_operator_asks_for_another[default_100, opt_in_200]` | knob ignored |
| `99`, `401`, `0`, `2e2`, `200 `, `+200`, `0200` exit 2 with nothing written | `::test_a_trial_window_outside_100_to_400_rounds_is_refused_before_anything_runs` | knob unchecked |

The `[gap_cap]` cases of the two parametrised `ops` tests are the D88 tests they replace, unchanged in
substance. `test_service_network_v2.py` gains an `ADMISSION_POLICY_EXTRA` hook next to
`AUDIT_POLICY_EXTRA`, used by the CLI test. In `test_probe_v2_refuses_unverified_trial_without_probing`,
the `corrupt` and `gzip` faults now skip the admission policy object, which the miner reads first, so
they still reach a trial input. Their expected messages now name that input.

## Rejected

- **`Literal[200]`**: moves every digest and run id, production included, and leaves no way to keep a
  100-round run.
- **Omit the field at 100**: moves every existing digest (Decision 1).
- **An unbounded integer**: a stalled application would hold its place, and a captured challenge stay
  valid, without limit.
- **A window derived in code from the manifest**: `T_miner + T_ref` depends on the link, the host and
  the cache, none of which is in the manifest. A code formula from the model size would sign an estimate.
  The operator passes the measured value through `challenge_rounds_for`.
- **Widening the owner-request bounds with the window**: not needed (Decision 6). It would also let a
  captured request replay for longer.
- **Production now**: the costs above fall on every newcomer of a production run. That is the founder's
  call.

## One-page sheet for the founder (relayed by Principal SN100)

**New measurements** [A, Forge (CTO), 2026-10-29]: the trusted reference of a 101 M trial takes **347 s**
(the larger of two warm samples, hosts A and B). The miner side takes **282 s cold** and **219 s warm**.
So a 101 M trial needs about 566 s from challenge to reference, and the current 297 s window cannot
hold even the reference alone. The window therefore has to follow the run's size. The table applies
`challenge_rounds_for` (Decision 4, 15 % margin, rounded up, at least 100) [V, the helper run on these
seconds]:

| Row | Parameters | T_miner + T_ref | Window (`challenge_rounds_for`) | 36-trial screen span (36 x (rounds + 1)) |
| --- | --- | --- | --- | --- |
| `dense-0.03b`, the run profile (ADR 0016) | 31.5 M | ~194 s est. warm [H, ADR 0016]; 198 s warm, 217 s cold with T_ref scaled from 347 s [H] | **100**: today's window, no merge needed | 3,636 |
| `dense-0.05b` (ADR 0016, built, not run) | 44 M | ~260 s est. warm [H, ADR 0016]; 265 s warm, 292 s cold scaled from 347 s [H] | **100** at 260 s; 102 warm and 113 cold when scaled | 3,636 to 4,104 |
| `dense-0.1b` (ADR 0015) | 101 M | 566 s warm (347 + 219), 629 s cold (347 + 282) [A] | **217** warm (651 s); 242 cold | **7,848** warm; 8,748 cold |
| `dense-1b` | 889 M | about 4,765 s by the same linear scaling [H] | out of reach: 1,827 rounds, above the 400 ceiling. It cannot complete a trial at any window either (a 10.67 GB state, ADR 0013) | - |

Forge's "~215 rounds" for 565 s is 216.6 before rounding, so the helper returns 217. A cold 101 M trial
(629 s) still fits a 217-round window (651 s), with 22 s to spare instead of the 15 % margin [H].

**What the numbers change** [V by reading, durations H at 3 s a round]:

- **The "stop above 200" recommendation is withdrawn.** It would stop every 101 M run, since the
  measured 101 M trial needs 217. Each run's window is derived from its own size. Stop and report
  only when a measured trial needs more than its row above plus 15 %: at 101 M, above 250 rounds (a
  trial over 652 s). The hard ceiling stays 400.
- **`REFERENCE_WAIT_SECONDS` (330 s) is below the 347 s reference.** A second request for a 101 M
  reference still running gets 503 after 330 s and retries (ADR 0012). Only that waiter is affected:
  the leader request, which starts the reference, does not wait on this bound. No change is proposed.
- **Screen span.** At 217 rounds, 36 serial trials can span up to 7,848 rounds. That is above the 6,000
  of `manifest_v2 example` and the `tiny-cpu` build. It is below the 20,000 the D20 builder signs for
  `dense-0.1b`, `dense-0.05b` and `dense-0.03b` (`build_test_manifest.sh`, `MEASURED`). The worst
  case assumes every trial runs to its deadline; joinbase-6 measured about 64 rounds a trial
  (`hypertrain/docs/network-v2.md`) [A].

**Decisions** (each is the founder's; the recommendation is Forge's to relay):

1. **Value.**
   - (a) Keep 100 and do not merge: the 31.5 M profile runs, 101 M cannot.
   - (b) A fixed 200 on TEST: too short for the measured 101 M trial.
   - (c) **Recommended**: on TEST, build the throw-away warm-up run, then build the real run at
     `challenge_rounds_for(max of the warm T_miner + T_ref measured for its size)`. With today's
     numbers that is 217 at 101 M and 100 at 31.5 M and 44 M. Stop as in the first point above.
2. **Production.** Refused for now (`validate_ledger`, in the builder, at run creation and in
   `Admission`). Recommended: keep it refused until a TEST run has measured trials at the chosen
   window; a production window is a separate ruling.
3. **Ceiling.** 400 (recommended; it holds a 1,043 s trial, 1.8 times the measured 101 M one), or
   another value. The ceiling caps the costs above for any run.
4. **Accept the costs** of a wider window: slower reaping of dead applications, longer-held pending and
   trial places, a later reseed, and a larger worst-case screen span. At 217 rounds each lasts about
   11 min instead of 5.
5. **Out-of-bounds status**: 409, like every other schema refusal (shipped), or a dedicated 422 (Forge).

## Not verified

- No trial ran at a wider window on a GPU host, on s100 or with a real link. The 101 M reference and
  miner times are Forge's measurements [A], not re-measured in this lot. The 31.5 M, 44 M and 1B rows
  are linear scalings [H].
- The waiter-503 path of `REFERENCE_WAIT_SECONDS` under a window above 110 rounds is reasoned, not
  tested [H].
- `probe-v2` reading the admission policy object over a real coordinator (s100, R2) has not run; the
  in-process service and the HTTP fixture serve it [V by test].
- GitHub CI ran on `04518b5c` only: `hypertrain-ci` `test (7)` failed on the publish secret scan of
  the production-ledger test fixture, which the next commit allowlists. The tip after it has passed
  the local CI mirror, not GitHub CI.
