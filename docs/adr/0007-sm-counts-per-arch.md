# ADR 0007: accepted SM counts per CUDA architecture

- Status: draft for branch `v3-sm-counts`, 2026-10-26 (S6a, Principal SN100 D21). Forge
  decides promotion to `v3`; nothing here changes a production run, master, profile or
  emission.
- Order: Principal SN100 D21 (plan `v3-s100-test-stack`, part a). The legacy-result rule in
  Decision 3 was chosen by the founder on 2026-10-26 (option A, "compatible 5090").
- Labels: **[V]** verified in code or by a run on the date above, **[A]** reported by a cited
  document or brief, **[H]** hypothesis, **[?]** unknown.

## Context

- Before `8a90d9bc`, `Environment.sm_counts` in
  `hypertrain/src/hypertrain/gpu_ops/network_qualification.py` was `list[Literal[170]]`. The
  s100 TEST challenge image (`a543239e`, rev `dc163190`) still has that schema, so it refuses
  every B300 (148 SM) CUDA qualification [A, S5 report on `v3-d20-gpu-test`].
- `8a90d9bc` widened the schema to any positive count and bound it to the manifest only:
  `inspect_environment` refuses a device whose count differs from `reference_spec.sm_count`,
  and the store requires the same equality (`challenge/store.py:2440`). Any positive count
  then passed the schema, 7 and 132 included [V, probe at `90660c31`]. The record did not say
  which GPU produced the count.
- Measured device classes: RTX 5090, compute capability 12.0 (sm_120), 170 SMs (gpu_network_v2
  hosts) [A]; B300 SXM6 AC, capability 10.3 (sm_103), 148 SMs on hosts A and B
  (`docs/b300-run.md`, `hypertrain/tests/trainer/data/attn_qualify_sm103.json`) [V on hardware
  by S1/S3, A here].

## Decision

1. `network_qualification.SM_COUNTS` maps a capability `(major, minor)` to the SM counts it
   may record: `(12, 0) -> {170}` and `(10, 3) -> {148}`. Add a row only from a measured host.
2. `Environment.capabilities` holds one `[major, minor]` per device, beside `sm_counts`.
   `inspect_environment` reads both from the same `get_device_properties` object. A device is
   qualified only when its pair is a row of the table and its count equals the manifest's
   `reference_spec.sm_count`.
   - Off the manifest count: `qualification_sm_profile_mismatch` (unchanged).
   - Pair outside the table: `qualification_sm_profile_unqualified:device N: capability X.Y
     is not a qualified device class` or `... qualifies [170] SMs, not 148`.
   - The schema enforces the table, so the challenge refuses an unqualified pair when it
     parses `result.json` in `_bootstrap_backend_v2`. It still refuses an off-manifest count
     at `store.py:2440`. `store.py` is unchanged.
3. Legacy results. A `result.json` written before this change has no `capabilities`. It is
   accepted only if every count is 170, which is exactly what `Literal[170]` accepted; 148 or
   any other count without capabilities is refused. The field is omitted from JSON when absent,
   so such a file keeps its bytes. Every result written by this code carries capabilities.
4. `ReferenceSpec` (the signed manifest) is not changed. The capability lives in the
   qualification record, not in the run id.

## Hash consequences

Measured with `network_qualification.sources()`/`bundle()` and
`ChallengeStore._service_implementation_v2`, before (`90660c31`) and after this change [V]:

| Value | Before | After |
| --- | --- | --- |
| `network_qualification.py` sha256 | `25d62b3e…0989f0` | `dab7a6ab…7bda15` |
| `sources()` map (135 files), canonical sha256 | `5c6544a6…606ddf` | `cbb0d711…90e27f` |
| `bundle-hash` (2,191,360 bytes) | `41a7ebe0…55bb0d` | `6a7ab8ca…68943a` |
| `implementation_hash` | `5a0b2374…0190a578a` | unchanged |
| `store.py` sha256 | `d75972ae…ed747db3` | unchanged |
| example v1 run_id / `reference_hash` | `ad8beb30…2ba2ff` / `3a2862ca…72f323` | unchanged |

- `sources()` hashes every `src/hypertrain/**/*.py`, so this change, like any source change,
  invalidates older qualification configs against a tree at this commit
  (`qualification_source_or_profile_changed`), the dispatch source pins
  (`gpu_ops/role_dispatch.py:112`) and reviewed bundles. A config still validates against its
  own frozen tree.
- Installed qualification records stay valid: `_backend_v2` never re-parses `result.json`.
- No run id, golden root or `reference_hash` changes: the manifest schema is untouched and
  the golden tests pass [V]. A run id hashes the manifest only, so the TEST run `16bc75dd…`
  keeps its id [V by reading; its manifest is not in Git].
- A new-format `result.json` carries `capabilities`, so its sha256, and the evidence and
  authority hashes of a new qualification built from it, differ from an old-format file.
- The v3 build gives new server and GPU image digests. A manifest that pins the new GPU digest
  has a new run id.

## Rebuild and the s100 TEST stack

- Rebuild [V, `.github/workflows/hypertrain-images.yml:9-16,42-49`]: Forge pushes this branch
  into `v3`; the push touches `hypertrain/**` and triggers `hypertrain-images.yml`. It runs
  `hypertrain-ci`, then builds and attests `ghcr.io/cortexlm/cortex/hypertrain` (server, the
  challenge image) and `.../hypertrain-gpu`, tagged `v3-sha-<sha>`, and prints the digests in
  the job summary. It takes about 22 minutes [A, brief]. A push to `v3-sm-counts` itself runs
  `hypertrain-ci` only and builds no image (`hypertrain-ci.yml:7-12`).
- Tomorrow on s100, TEST stack only (part b of the plan, not done here) [H]:
  1. Verify the server digest against the `v3` commit that contains this change:
     `gh attestation verify oci://ghcr.io/cortexlm/cortex/hypertrain@sha256:<d> --repo
     CortexLM/cortex --source-ref refs/heads/v3 --source-digest <v3 commit sha>
     --signer-workflow CortexLM/cortex/.github/workflows/hypertrain-images.yml
     --deny-self-hosted-runners --bundle-from-oci`. `/version` shows no revision, so it is a
     liveness check only.
  2. Outside a seal window, snapshot `compose.test.yml` and the TEST state. Change only the
     `challenge` service image (`hypertrain-test-challenge`) to that digest, `up -d`, and check
     `/version`. Rollback is the snapshot plus the same `up -d`. Never touch
     `cortex-challenge-hypertrain`, the master or `/data/role-dispatch/dispatch.json`.
  3. Re-freeze the staged qualification tree at the new commit (`scripts/ops/prepare_runtime.py
     freeze`): the sources hash changed.
  4. B300 TEST manifest: `sm_count` 148, `driver_allowlist` with `595.91.07`, and the GPU
     digest the qualification runs on. Produce the two-host CUDA qualification with this code,
     so `result.json` carries `[10, 3]`. Bootstrap it before the run is created (S5).

## Not changed

These still assume RTX 5090 and are outside this change: `select_inventory` (Vast name check),
`experiments/gpu_network_v2/orchestrate.py:3766` (Vast `qualify_role`, asserts 170),
`gpu_ops/verdict.py:44`, `scripts/check_gpu_evidence.py`, `scripts/network_gpu_seed.py:200`,
`scripts/ops/manifest/od_large_manifest.py`, `scripts/launch_cuda_run.sh:96`,
`trainer/attention.py` (no `(10, 3)` backend row) and `public_api/views.py:329` (shows
`sm148`). The miner, auditor and repair paths compare `sm_count` with the manifest only.
They need no change for a 148 manifest [V by reading]. No non-Vast tool in this repository
writes the qualification `config.json` for a Lium B300 host [?].
