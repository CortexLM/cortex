# ADR 0017: the admission trial window as a policy value (D94 lot C)

- Status: **prepared, not merged**, 2026-10-29 (Principal SN100 D94 lot C). The window is a protocol value,
  so the founder decides; Principal SN100 relays that decision, and nothing here is merged before it.
  Branch `v3-trial-rounds` on origin/v3 `667695be`. No production run, master, profile, pin or emission
  changes. Nothing in this lot ran on a GPU host, on s100, against R2 or on-chain.
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
   le=CHALLENGE_ROUNDS_MAX)]`, with `CHALLENGE_ROUNDS_MAX = 400` (20 minutes at 3 s, about 3.5 times the
   101 M trial). It is **still serialised at 100**, not omitted. The field already exists, and
   `"challenge_rounds": 100` is in the bytes of every admission policy ever signed. So keeping it
   serialised is what leaves those digests unchanged. Omitting it at 100, the `reaudit_rounds` pattern
   the brief names, would move every one of them: the `tiny-cpu` builder policy digest would go from
   `d0a5538d…14a499` to `8cad96ce…b6bf7b` [V, computed]. That pattern works for `reaudit_rounds`
   only because that field was new. A `bool`, a float or a string is refused (`StrictInt`), as are 99
   and 401 [V].
2. **`JoinChallenge` accepts any window a policy may set**: `100 <= deadline - seed <=
   CHALLENGE_ROUNDS_MAX`. A miner parses the challenge before it reads the policy. The service builds
   the deadline from its own policy (`admission.py:437`, `seed + self.policy.challenge_rounds`), so a
   run gets exactly the window it signed.
3. **TEST ledger only, until the founder rules.** `AdmissionPolicyV2.validate_ledger(ledger_mode)`
   refuses a window above 100 unless the economics policy is `test`. `ops/manifest_v2 build` (exit 2)
   and `POST /v2/admin/runs` (`_create_run_v2`, 422) call it next to `validate_training`, and so does
   the D20 builder's dry run. This is the D88 `gap_cap_rounds` rule applied to a second bound.
4. **A derived value, not a guessed one.** `challenge_rounds_for(trial_seconds)` returns
   `max(100, ceil(1.15 x trial_seconds / 3))` and refuses a trial the ceiling cannot hold. Its argument
   is the measured `T_miner + T_ref` of a trial. Examples: 282 s gives 109, 297 s gives 114, 389 s
   gives 150, 521 s gives 200, 1043 s gives 400, and 1044 s is refused [V]. The policy carries the
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

**Hashes** (measured on `git archive` of base and branch with `network_qualification.sources()`,
`bundle()` and `ChallengeStore._service_implementation_v2`, 2026-10-29 [V]):

| Value | `667695be` | This branch |
| --- | --- | --- |
| `implementation_hash`, all five pins (`avx2`/`none` `7af135ca…70be62`, `avx512`/`none` `c509dfd1…83a920`, `avx2`/`sm_103`/`math` `24fbe36d…c185ce`, `avx2`/`sm_103`/`sdpa-flash` `9bcf9bdd…2c2dd5`, `avx2`/`sm_120`/`sdpa-flash` `c8af2ae7…68891f`) | as listed | **unchanged** |
| `challenge/store.py` / `challenge/admission.py` / `protocol/messages_v2.py` sha256 | `22fb5dcd…c56676` / `0d7fa656…d67ead` / `8e03a8a0…42bb04` | **moved**: `3edf2faa…0ab5bd` / `0f5edb64…bf1276` / `e9905489…41be6c` |
| `challenge/app.py` sha256 | `0e6ec421…e07d48` | unchanged |
| `sources()` canonical sha256 (158 files) | `97e3c592…5f1be3` | **moved**: `14d25faf…ed0cb9` (`json.dumps(sort_keys)`: `7a721848…b6b4bb` to `7b1ad647…fb4a34`) |
| `bundle-hash` (2,775,040 B) | `2c064435…0b8196` | **moved**: `4c98877c…cc4a69` |
| example admission digest (both ledger modes), example `RunManifestV2` run id | `c3fd6d4d…b3e3e3`, `aa39197d…ec8abd` | unchanged; `manifest_v2 example` writes the same three files byte for byte |
| D20 builder, fixed seeds (ADR 0015 inputs): `tiny-cpu` / `dense-1b` / `dense-0.1b` at 144, 256, 300 s | `f3c12327…dcde3588` / `bbbff4bb…35e4a542` / `0ea43133…82da2af3`, `944b69ae…5bb46da6`, `b6bebaf7…ca41283d` | unchanged; `manifest.json`, every `policy-*.json` and `relay-registry.json` byte identical; the envelope differs only in its randomised `sig`, and `index.json` only in the envelope's hash |
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
- `REFERENCE_WAIT_SECONDS` (330 s, `store.py:309`) is shorter than a window above 110 rounds. A second
  request for a reference that is still running then gets 503 after 330 s, and the client retries a 503
  (ADR 0012). That is a retry, not a lost trial. It is left unchanged; only its comment changes.

**Status codes.** A production manifest with a window above 100 is refused with 422 (`validate_ledger`).
A window outside 100..400 fails the policy schema and answers 409, like every other malformed policy,
because the v2 `ValidationError` handler answers 409 (`challenge/app.py:512-522`) [V]. The brief asked
for 422 there. Changing it would change how every policy refusal is reported, so it is left to Forge.

## Tests and mutants

All on CPU in default CI. Each test fails at `667695be` (red logs in REPORT.md) and kills at least one
mutant. 17 mutants were applied to the real files and run with `pytest -x`, then the files restored.
**17/17 killed**, each by a failing test, never by an error [V, 2026-10-29].

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
| the builder signs 100 unset and 200 on request, and the dry run checks and prints the window | `test_b300_d20_builder.py::test_the_trial_window_is_100_rounds_unless_the_operator_asks_for_another[default_100, opt_in_200]` | knob ignored |
| `99`, `401`, `0`, `2e2`, `200 `, `+200`, `0200` exit 2 with nothing written | `::test_a_trial_window_outside_100_to_400_rounds_is_refused_before_anything_runs` | knob unchecked |

The `[gap_cap]` cases of the two parametrised `ops` tests are the D88 tests they replace, unchanged in
substance. `test_service_network_v2.py` gains an `ADMISSION_POLICY_EXTRA` hook next to
`AUDIT_POLICY_EXTRA`, used by the CLI test.

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

## Decisions for the founder (relayed by Principal SN100)

1. **Value.** (a) Keep 100: no merge, and 101 M is possible only on a link of at least about 43-73 MiB/s
   [H]. (b) A fixed 200 on TEST. (c) **Recommended**: on TEST, build the throw-away warm-up run at 200,
   then build the real run at `challenge_rounds_for(max of three warm T_miner + T_ref)`. Above 200
   (a trial over 521 s), stop and report, as the replay rule stops above 298 s.
2. **Production.** Refused for now (`validate_ledger`). Recommended: keep it refused until a TEST run has
   measured trials at the chosen window; a production window is a separate ruling.
3. **Ceiling.** 400 (recommended; it holds a 1,043 s trial) or another value. The ceiling caps the costs
   above for any run.
4. **Accept the costs** of a wider window: slower reaping of dead applications, longer-held pending and
   trial places, a later reseed, and a larger worst-case screen span.
5. **Out-of-bounds status**: 409, like every other schema refusal (shipped), or a dedicated 422 (Forge).

## Not verified

- No trial ran at a wider window on a GPU host, on s100 or with a real link. The 101 M trial times are
  ADR 0015 estimates [H].
- The waiter-503 path of `REFERENCE_WAIT_SECONDS` under a window above 110 rounds is reasoned, not
  tested [H].
- The CI of this branch has not run (the session cannot push).
