# S6a / D21: challenge image fix, `sm_counts` per arch/GPU: REPORT

## Status

DONE: code and ADR, 2026-10-26. Branch `v3-sm-counts`, based on `90660c31`
(`origin/v3-b300-qualif`). `8a90d9bc` was already an ancestor, so `origin/v3-b300-run` was not
merged; its only extra commit is `95dbc63e`, which is docs only. Not pushed (PUSH-ME below).
This session had no s100 access and no GPU host access.

Labels: [V] verified by a command or run in this session, [A] reported by a cited document,
[H] hypothesis, [?] unknown.

## Result

- **Table** [V]: `network_qualification.SM_COUNTS` maps a CUDA capability to the SM counts it
  may record: `{(12, 0): {170}, (10, 3): {148}}`, i.e. RTX 5090 sm_120 and B300 sm_103.
- **Qualification record** [V]: `Environment.capabilities` holds one `[major, minor]` per
  device, next to `sm_counts`.
  - `inspect_environment` reads both values from the same `get_device_properties` object.
  - A device is accepted only when its pair is a row of the table and its count equals the
    manifest's `reference_spec.sm_count`.
- **Refusal messages** [V]:
  - Off the manifest count: `qualification_sm_profile_mismatch` (unchanged).
  - Pair outside the table: `qualification_sm_profile_unqualified:device 0: capability 12.0
    qualifies [170] SMs, not 148`, or `... capability 9.0 is not a qualified device class`.
  - The schema raises the same text as a `ValueError`.
- **Store** [V]: `store.py` is unchanged and consistent with the rule.
  - `_bootstrap_backend_v2` parses `engine.Result`, so the table applies at l.2419.
  - The driver allowlist (l.2439) and the manifest `sm_count` equality (l.2440) still apply.
  - Both are now tested with the B300 driver 595.91.07.
- **Legacy results** [V]: the founder chose option A through the relay question,
  2026-10-26 ("A — compatible 5090").
  - A `result.json` without `capabilities` is accepted only if every count is 170, which is
    what `Literal[170]` accepted. 148 or any other count is refused.
  - The field is omitted from JSON when absent, so old files keep their bytes.
- **Manifest** [V]: `ReferenceSpec` is unchanged, so no run id moves.

Commits:

| Commit | Subject |
| --- | --- |
| `8c50b04e` | fix(hypertrain): qualify cuda devices by (capability, sm count) pair |
| `20cd0303` | test(challenge): bootstrap rtx 5090 and b300 cuda qualifications |
| `5c2ecc29` | docs(adr): record the per-arch sm count table for cuda qualification |
| `d54b4cbf` | docs(adr): state which s5 test-stack blockers adr 0007 removes |
| (this commit) | docs(sm-counts): report |

Files:

- `hypertrain/src/hypertrain/gpu_ops/network_qualification.py`: `SM_COUNTS`,
  `unqualified_device()`, the `Environment.capabilities` field and validator, and the
  `inspect_environment` capture and check.
- `hypertrain/tests/gpu_ops/test_network_gpu_qualification.py`:
  - `results()` now writes `capabilities`;
  - schema, legacy and producer tests;
  - `fake_cuda_host` reports `major`/`minor`;
  - the `8a90d9bc` "any positive count" test is replaced.
- `hypertrain/tests/challenge/test_backend_authority_v2.py`: `authority` fixture takes an
  optional host; 7 bootstrap cases.
- `hypertrain/CHANGELOG.md`, `hypertrain/docs/protocol.md` (§7.5): the behaviour change.
- `docs/adr/0007-sm-counts-per-arch.md`, plus its line in `docs/index.md`.

## Evidence

Commands run from `hypertrain/` unless noted. Temp dirs were under `/tmp/smcounts-main/` and
are deleted.

**Before the fix**: the final tests, run with `network_qualification.py` from `90660c31`
swapped in [V]:

```
uv run --no-sync pytest -q -p no:cacheprovider --basetemp=… \
  tests/gpu_ops/test_network_gpu_qualification.py::{test_result_admits_each_qualified_capability_sm_pair,\
  test_result_refuses_pair_outside_the_table,test_result_without_capabilities_is_an_rtx_5090_record_only,\
  test_inspect_environment_records_capability_with_manifest_sm_count,\
  test_inspect_environment_refuses_manifest_count_on_unqualified_device} \
  tests/challenge/test_backend_authority_v2.py::{test_qualified_device_class_installs_cuda_backend,\
  test_unqualified_or_off_reference_device_installs_nothing}
9 failed, 15 warnings, 7 errors in 1.14s
```

- Many of these fail at the format level: the old schema rejects `capabilities` with
  `extra_forbidden`.
- The behavioural gap, from a probe of the old `Environment` [V]:
  `ACCEPTED B300 148`, `ACCEPTED unknown count 132`, `ACCEPTED unknown count 7`.
- The store reader probe showed the same at the store: an unknown 9.0/132 pair with a 132
  manifest installs at HEAD [V, read-only probe, scratch deleted].

**After the fix** [V]:

```
uv run --no-sync ruff format --check src tests scripts      -> 363 files already formatted
uv run --no-sync ruff check src tests scripts               -> All checks passed!
MYPYPATH=src:scripts:. uv run --no-sync mypy --explicit-package-bases --package hypertrain \
  --module experiments.gpu_network_v2.orchestrate --module scripts.network_service_proof \
  --module scripts.network_gpu_operation                    -> Success: no issues found in 131 source files
uv run --no-sync pytest -q -p no:cacheprovider --basetemp=… tests/gpu_ops/test_network_gpu_qualification.py \
  tests/challenge/test_backend_authority_v2.py tests/gpu_ops/test_network_admit_cli.py \
  tests/gpu_ops/test_network_lifecycle.py                   -> 279 passed, 7 skipped in 225.06s
```

- The 7 skips need `HT_GENUINE_CUDA_INDEX`, which is not set.
- Commit `8c50b04e` passes on its own: the base store tests against the new code gave
  64 passed, 7 skipped.

**Golden and real fixtures** [V]. These hold the v1 baseline bytes, the example run_id
`ad8beb30…`, the island golden roots, the schema docs, the TEST run_id `16bc75dd…` and the
implementation binding:

```
uv run --no-sync pytest -q … tests/trainer/test_golden_replay.py tests/protocol/test_od_fields.py \
  tests/trainer/test_config_v2.py tests/protocol/test_network_v2.py tests/protocol/test_messages.py \
  tests/challenge/test_role_dispatch_lazy.py \
  "tests/challenge/test_service_capacity_v2.py::test_source_binding_ignores_unrelated_methods_but_rejects_capacity_changes"
194 passed in 2.72s
```

**Mutants: 8 of 8 killed** [V]. Each was applied to the committed tree and restored with
`git checkout`; the tree was clean afterwards.

| Mutant | Killed by (examples) |
| --- | --- |
| M1 frozen list restored (`list[Literal[170]]`) | `…admits…[b300]`, `…installs_cuda_backend[b300]`, 12 tests in total |
| M2 table check removed (the `8a90d9bc` behaviour) | `…refuses_pair…[b300_count_on_rtx5090,unknown_pair]`, store `[unknown_pair]` |
| M3 legacy result accepts any count | `…without_capabilities…`, store `[b300_count_without_capabilities]` |
| M4 capabilities required (rejected option B) | store `[rtx5090_without_capabilities]`, 7 `test_network_lifecycle` tests |
| M5 no device-count check, non-strict zip | `…refuses_pair…[unlisted_device]` |
| M6 `store.py:2440` equality deleted (survived the old suite) | store `[b300_devices_under_170_manifest]` |
| M7 producer skips the table | `test_inspect_environment_refuses_manifest_count_on_unqualified_device` |
| M8 table row B300 10.3 set to 170 | `…[b300]` (4 tests) |

**Full hypertrain suite**, the CI file selection run in one process [V]:

```
pytest -m "not od" + the CI deselects, all tests/**/test_*.py except sim/ and e2e/, plus
tests/e2e/test_network_gpu_prepare.py
49 failed, 2768 passed, 17 skipped, 113 deselected in 2874.85s
```

None of the 49 failures comes from this change:

- **46 in `tests/gpu_ops/test_network_admit_cli.py`.** They are an ordering artifact.
  - `role_dispatch._module` caches a stub `network_service_proof` from
    `tests/challenge/test_role_dispatch_lazy.py`.
  - Run alone, the module gives 104 passed.
  - `test_role_dispatch_lazy.py` followed by `test_network_admit_cli.py` gives **46 failed,
    64 passed** on my tree and **the same 46 failed, 64 passed** with the three changed files
    put back to `90660c31`.
  - CI shards the files, so it does not hit this.
- **2 in `tests/test_publish_exclusions.py`.** These are the entropy hits in
  `docs/examples/dispatch.test.json`, which already fail at HEAD.
- **1 in `tests/trainer/test_engine_flags.py::test_compile_flag_repeatable`.** This host has
  no `g++` for torch inductor.

**Repo contract**, from the monorepo root [V]:

```
uv run --no-sync python scripts/check_repo.py --final
Repository contracts passed (final Python-only gate).   (exit 0)
```

The command creates an empty root `.venv`; I deleted it afterwards.

**Independent review** [V]:

- Three lenses (gate, tests, docs) each produced findings, and each finding was checked by an
  adversarial verifier. The verifiers refuted all 4 findings.
- I still applied two of them:
  - a store case for a result without capabilities, restoring coverage that my `results()`
    change had removed, plus a legacy 148-SM refusal case;
  - `--source-digest` in the ADR's attestation step.

## Hash consequences

Measured at `90660c31` and at this branch [V]:

| Value | Before | After |
| --- | --- | --- |
| `network_qualification.py` sha256 | `25d62b3eb678…a0989f0` | `dab7a6ab6926…eb7bda15` |
| `sources()` map (135 files) | `5c6544a660e0…7f606ddf` | `cbb0d711cfd7…2790e27f` |
| `bundle-hash` (2,191,360 bytes) | `41a7ebe02998…6d55bb0d` | `6a7ab8ca9b4a…0468943a` |
| `implementation_hash` | `5a0b23740734…70190a578a` | unchanged |
| `store.py` sha256 | `d75972aec0de…be4ed747db3` | unchanged |
| example v1 run_id | `ad8beb30084a…1b2ba2ff` | unchanged |
| example `reference_hash` | `3a2862cabe99…680d72f323` | unchanged |

- **Older qualifications are invalidated.** `sources()` hashes all of `src/hypertrain/**/*.py`,
  so every source change, this one included, invalidates them. A qualification config built
  from a tree before `8c50b04e` fails `check_inputs` against a tree at this branch with
  `qualification_source_or_profile_changed`. The role-dispatch source pins
  (`role_dispatch.py:112`) and reviewed bundles change too. A config still validates against
  its own frozen tree.
- **Installed records are untouched.** Qualification records already installed in a store
  stay valid: `_backend_v2` does not re-parse `result.json`.
- **No pinned run id, golden root or `reference_hash` changes.** The TEST run `16bc75dd…`
  keeps its id, because a run id hashes the manifest only.
- **New-format results hash differently.** A `result.json` that carries `capabilities` has
  different bytes, so a new qualification's evidence and authority hashes differ from those of
  an old-format file.
- **New image digests.** The v3 build gives new server and GPU digests. A manifest that pins
  the new GPU digest gets a new run id; none exists yet.

## Risks

- **Legacy 170-SM results.** A 170-SM result without capabilities is still accepted. This is
  the old behaviour, kept on purpose by the founder's choice.
- **`compare()` has no `sm_count` binding.** It does not bind `sm_count` to the manifest. This
  predates the change; the store and `inspect_environment` enforce it. The reviewer's finding
  was refuted and left out of scope.
- **Unqualified-pair error type.** At bootstrap this error is a pydantic `ValidationError`
  carrying the message, not a `ChallengeError` 403. `scripts/ops/backend_bootstrap.py`
  therefore stops with a traceback.

## Not verified

- **No GPU in this session.** `inspect_environment` on a real B300 or RTX 5090 (torch
  `major`/`minor`) was only tested through a fake. The B300 reading of capability `[10, 3]`
  with 148 SMs comes from S1/S3 (`attn_qualify_sm103.json`) [A].
- **Genuine RTX 5090 index.** These cases are skipped because `HT_GENUINE_CUDA_INDEX` is not
  set, and no real old-format `result.json` is in Git. The legacy path was tested on a
  synthetic file with the old format.
- **Image build.** Not checked: the image build, its ~22 min duration, the new digests, and
  whether the staged tree shadows the GPU image's copy of `network_qualification.py` [H]. Also
  not checked on s100: `compose.test.yml`, and the beacon and relay images [?].
- **Root-level checks not run.** Root Cortex `pytest`, `check_deploy.py` and `uv build` were
  not run, because the change touches only `hypertrain/` and `docs/`. The CI shard layout was
  not run either; the suite ran in one process instead.
- **No Lium B300 qualification config.** No tool in this repository writes a qualification
  `config.json` for a Lium B300 host [?].

## Next

1. Forge: push `v3-sm-counts`, gate it, and fast-forward `v3`. That triggers
   `hypertrain-images.yml`; note the server and GPU digests from the job summary.
2. Tomorrow, s100 TEST stack only (plan part b): ADR 0007, "Rebuild and the s100 TEST stack".
   - Verify the attestation with `--source-digest`.
   - Swap the `challenge` service image only, with a snapshot and rollback.
   - Re-freeze the staged tree.
   - Build a B300 manifest with 148 and driver 595.91.07.
   - Bootstrap a two-host qualification written by this code before the run exists.
3. Parts c and d remain blocked as S5 reported: no GPU executor reachable from the TEST
   challenge, and no TEST auditors.

PUSH-ME v3-sm-counts
STEP-DONE
