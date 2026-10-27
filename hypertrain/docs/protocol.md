# Protocol specification

This is the wire-level contract of a Hypertrain run: who signs what, which bytes get hashed, how a round moves through its states, and what a miner must pin so an auditor can reproduce its bits. Every statement here comes from `src/hypertrain/protocol`, `challenge`, `auditor`, `ledger` and `trainer/determinism.py`. The HTTP surface is in [challenge-routes.md](challenge-routes.md), the incentive math in [mechanism.md](mechanism.md).

Tagged numbers like `40{ev:docs/schemas/example-run-manifest.json#manifest.verify.T.audit_after_commit}` are checked against the example manifest by `scripts/check_doc_numbers.py`.

## 1. Messages and schemas

Fifteen message types are signed. The JSON Schema of each is generated from the pydantic model (`uv run --frozen python -m hypertrain.protocol.schema docs/schemas`) and lives in `docs/schemas/`.

| Message | Sender | Schema | Purpose |
|---|---|---|---|
| RunManifest | run owner (K_owner) | [RunManifest.json](schemas/RunManifest.json) | Everything that defines a run. `run_id = sha256(JCS(body))` |
| RoundOpen | K_coord / aggregator | [RoundOpen.json](schemas/RoundOpen.json) | Roster, state hashes and the six drand deadlines of round `w` |
| Accept | miner | [Accept.json](schemas/Accept.json) | Takes the assignment, reports image digest, driver and GPU count |
| Commit | miner | [Commit.json](schemas/Commit.json) | Merkle root of the leaf hashes plus hashes of the final state and delta |
| Receipt | K_coord | [Receipt.json](schemas/Receipt.json) | Proof of the drand round in which a commit arrived |
| DeltaManifest | miner | [DeltaManifest.json](schemas/DeltaManifest.json) | Where the delta lives, its size and chunk hashes |
| AuditChallenge | K_coord | [AuditChallenge.json](schemas/AuditChallenge.json) | Which miner, which beacon round, full replay or segments |
| StateServe | miner | [StateServe.json](schemas/StateServe.json) | Optimizer state at a segment start, with a Merkle proof |
| ReplayVerdict | an auditor | [ReplayVerdict.json](schemas/ReplayVerdict.json) | MATCH, MISMATCH, WITHHELD, BAD_PROOF or ASSIGNMENT_VIOLATION |
| Dispute | miner | [Dispute.json](schemas/Dispute.json) | Accept or contest a verdict |
| Bisect | miner or auditor | [Bisect.json](schemas/Bisect.json) | One level of N-ary bisection |
| Resolution | referee auditor | [Resolution.json](schemas/Resolution.json) | Single operator that decides a dispute |
| Forfeit | K_coord | [Forfeit.json](schemas/Forfeit.json) | Burn and blacklist record |
| Rollback | aggregator | [Rollback.json](schemas/Rollback.json) | Excluded miners and recomputed state hashes |
| Finalize | aggregator | [Finalize.json](schemas/Finalize.json) | Makes round `w` final and fixes the paid set |

`LeafPreimage` ([LeafPreimage.json](schemas/LeafPreimage.json)) is not sent as a message. It is the pre-image of one Merkle leaf and is revealed on audit. A fully populated manifest is in [example-run-manifest.json](schemas/example-run-manifest.json).

All bodies reject unknown fields. Integers are `StrictInt` in `[0, 2^53-1]` (JSON `true` is not `1`). Floats never travel as JSON numbers: they are 4-byte little-endian `f32` rendered as 8 hex characters, so canonicalization never sees a float.

## 2. Envelope, canonical bytes and signatures

Every message is wrapped as

```
{v: "ht/1", type, run_id, body, signer, exp_drand, sig}
```

and nothing else is allowed in it. `run_id` is 64 lowercase hex characters, `exp_drand` a positive integer drand round, `sig` 64 bytes as 128 hex characters.

- **Canonical JSON.** Bodies are serialized with RFC 8785 JCS (`protocol/jcs.py`). Lone surrogates, NaN and Infinity are refused. The intake additionally requires that the received body equals the canonical dump of the validated model, so a body with defaults filled in or fields reordered is rejected.
- **Signing message.** `"hypertrain/1|" + type + "|" + sha256_hex(JCS(body)) + "|" + exp_drand + "|" + run_id`. Appending `run_id` is a deliberate addition to the advisory preimage: a body signed for one run can never verify in another.
- **Scheme.** sr25519 over the signing message. Signers are SS58 hotkeys; K_coord and K_owner are also SS58 keys named in the manifest (`coord_pubkey`) or configured on the container.
- **Expiry.** An envelope is refused once `now_round > exp_drand`. Container-issued records (Receipt, AuditChallenge, Forfeit, round-0 RoundOpen) use `exp_drand = 2^53-1`.
- **Signer binding.** For Commit, Accept, DeltaManifest, StateServe and Dispute the body `hotkey` must equal the envelope signer. Other types are checked against an allow-list per type (auditors for ReplayVerdict and Resolution, K_coord for container records).
- **Replay protection.** The key is `(run_id, w, type, signer, subject)`, where `subject` is the first present of `hotkey`, `target`, `commit_hash`. A Dispute is keyed by its `verdict_hash` instead, so a miner gets one decision per verdict. The challenge container persists the seen set in SQLite, so replays stay refused after a restart.
- **Order of checks** in `Intake.accept`: shape, run id, signature, expiry, signer allow-list, model validation, canonical form, signer binding, replay.

Failures map to HTTP codes in [challenge-routes.md](challenge-routes.md): malformed 400 or 422, bad signature 400, not allowed 403, replay or wrong state 409.

## 3. Hashing and the leaf scheme

- **Tensor hash TH(state).** For each tensor, `sha256(lp(name) | lp(dtype) | lp(shape) | lp(raw little-endian bytes))` where `lp(x)` is a u64-LE length prefix and `shape` is `u32 ndim | u64 dims`. TH is the sha256 of the concatenated entry digests sorted by the UTF-8 bytes of the name. Duplicate names and negative dimensions raise.
- **Merkle tree.** Leaf = `sha256(0x00 | x)`, node = `sha256(0x01 | left | right)`, an odd last node is promoted unchanged.
- **Leaf scheme `ht-leaf-v1`.** One leaf per `J` inner steps plus the initial state, so `n_leaves = H/J + 1` and `J` must divide `H`. A leaf digest is

```
sha256( "ht-leaf-v1" | run_id(32B) | w u64le | t u64le | stage_root
        | batch_ids_sha256(32B) | rng_ctr u64le | loss_f32(4B) | norm_f32(4B) )
```

  with `stage_root` the Merkle root over `theta | m | v` hashes per pipeline stage. `Commit.leaves_root` is the Merkle root of the leaf digests and `metrics_root` commits the per-leaf loss and norm.
- **Assignment.** `assignment_hash = sha256("ht-assignment-v1" | run_id | u64be(w) | u64be(slot) | u64le(sample)*)`. The sample slices are a pure function of `run_id`, `w`, `drand_sig[d_assign]` and the manifest, so the auditor recomputes them and raises ASSIGNMENT_VIOLATION if the miner trained on other data.
- **Audit selection.** `sha256("ht-audit" | run_id | u64be(w) | drand_sig[d_audit] | hotkey) < q_i * 2^256`, exact on the f32 value of `q_i`.
- **Segment selection** (carry policy). Fisher-Yates over leaf windows seeded with `sha256("ht-segments|" + run_id|w|beacon_sig_sha256|target)`, `k` random windows, the final window always, and the `Q_top` windows with the largest committed `norm_f32`.

## 4. Round state machine

All times are drand rounds `D` from the manifest beacon (quicknet, period 3 s). `drand_round_at(t) = (t - genesis)//period + 1`.

`RoundOpen` carries `d_open < d_assign < d_commit < d_audit < d_final` and `d_commit < d_upload <= d_final`; any other order fails validation. The manifest timeouts are offsets from these anchors (example manifest values):

| Timeout | Rounds | Meaning |
|---|---|---|
| `assign_after_open` | 20{ev:docs/schemas/example-run-manifest.json#manifest.verify.T.assign_after_open} | `d_assign - d_open` |
| `upload_after_commit` | 300{ev:docs/schemas/example-run-manifest.json#manifest.verify.T.upload_after_commit} | `d_upload - d_commit` |
| `audit_after_commit` | 40{ev:docs/schemas/example-run-manifest.json#manifest.verify.T.audit_after_commit} | `d_audit - d_commit`, validated `>= 40` as clock-skew margin |
| `serve_deadline` | 900{ev:docs/schemas/example-run-manifest.json#manifest.verify.T.serve_deadline} | miner must publish StateServe |
| `dispute_per_level` | 600{ev:docs/schemas/example-run-manifest.json#manifest.verify.T.dispute_per_level} | per bisection level |

Round states are derived from `D` and stored facts, never from a mutable flag:

```
OPEN -> ASSIGNED -> TRAINING -> COMMIT_CLOSED -> UPLOAD_CLOSED -> APPLIED -> AUDIT
                                                        AUDIT <-> DISPUTE (while any dispute is open)
AUDIT -> FINAL -> VESTING -> RELEASED
```

| State | Condition |
|---|---|
| OPEN | `D < d_assign` |
| ASSIGNED | `D == d_assign` |
| TRAINING | `d_assign < D < d_commit` |
| COMMIT_CLOSED | `D >= d_commit` |
| UPLOAD_CLOSED | `D >= d_upload`, aggregate not yet posted |
| APPLIED | round `w+1` opened (optimistic apply), until its `d_assign` |
| AUDIT / DISPUTE | after that; DISPUTE while a dispute row is open |
| FINAL | Finalize stored |
| VESTING | ledger round is past the finalize round |
| RELEASED | ledger round `>= final_round + vest_rounds` |

A commit received at `D >= d_audit` gets status EXCLUDED: no ledger commit, no entitlement. The audit selection runs once `drand[d_audit]` is known, which is after every accepted commit, so the seed cannot be predicted by the committer.

**Miner status:** `ASSIGNED, ACCEPTED, COMMITTED, UPLOADED, EXCLUDED, MATCH, MISMATCH, WITHHELD, BAD_PROOF, ASSIGNMENT_VIOLATION`.

**Run lifecycle.** The owner posts a signed RunManifest (`created`), the operator sets the round timing with `PUT .../config`, then unpauses. The challenge container requires `coord_pubkey` to be its own K_coord and the ledger parameters to equal the manifest (`epochs_per_round`, `E_vest_rounds`, `forgive_per_epoch`). One run per state directory.

**Dispute.** `Dispute{accept|contest}` on a verdict, then `Bisect` envelopes (`level` step, layer, op; `N` between 2 and 1024; `N+1` hashes; `interval` with `a < b`) until a `Resolution` names a single operator and a `loser`. A Resolution may not come from the disputed auditor. If the loser is the auditor the miner is restored to MATCH, and if the round was already finalized the ledger `vindicate` event pays the forgone slice. If the loser is the miner the ledger `fault` event burns the outstanding escrow.

**Rollback.** Only before Finalize, and only for committed miners of round `w`. The aggregator recomputes `agg_w, step_w, agg_w1, step_w1` from stored artifacts, reports the old and new `theta` hashes of round `w+2`, and opens round `w+2` from the corrected state. After Finalize there is no rollback: later fraud is handled with money only (clawback, see the mechanism document).

## 5. Verdicts and forfeits

| Verdict | Forfeit cause | Effect at finalize |
|---|---|---|
| MATCH | none | paid |
| MISMATCH, BAD_PROOF | MISMATCH | FAULT, blacklist |
| WITHHELD | WITHHELD | FAULT, blacklist |
| ASSIGNMENT_VIOLATION | ASSIGNMENT_VIOLATION | FAULT, blacklist |
| lost dispute | DISPUTE_LOST | FAULT, blacklist |
| MISMATCH reproduced by the miner's own rerun | TRANSIENT | round reward burns, escrow kept, forgiven `forgive_per_epoch` times per chain epoch |
| no upload | NO_UPLOAD | slice burns, escrow kept, payable on vindication |

## 6. Ledger and get_weights

The ledger is a hash-chained journal of integer events: `commit, verdict, finalize, clawback, vindicate, fault`, plus the `weights` answers. State is a pure replay of the journal, and on open every stored answer is recomputed byte for byte. A torn tail is quarantined, a flipped byte raises `ChainBreak`. Event times must be non-decreasing and strictly after the last answered `epoch_at`.

`get_weights(epoch, epoch_at)` follows Cortex algorithm 3: `full_share_mass = 10^6` per epoch, unpaid mass burns, the first answer per epoch is persisted and returned unchanged, at most 65,536 hotkeys are listed (largest kept, ties by hotkey, rest carried). While a round is in AUDIT the endpoint answers 200 with the vested mass only, never 503.

## 7. Determinism reference specification (checklist)

An auditor and a miner must produce identical bits on any allowed host. A manifest fixes `reference_spec`: `image_digest`, `spec_doc_sha256`, `driver_allowlist`, `sm_count`, `env` and the island `layout`. The checklist:

1. **Image pinned by digest.** torch, CUDA, cuBLAS, cuDNN and NCCL come from that image. `CUDA_DISABLE_PTX_JIT=1`, so only precompiled SASS runs.
2. **Environment before `import torch`.** `CUBLAS_WORKSPACE_CONFIG=:4096:8` (the only accepted value). `setup_determinism` raises if torch was imported first without it.
3. **Torch flags.** `use_deterministic_algorithms(True)` without `warn_only`, `cudnn.benchmark=False`, `cudnn.deterministic=True`, TF32 off for matmul and cuDNN, reduced-precision reductions off for bf16 and fp16, `float32_matmul_precision("highest")`.
4. **CPU threads.** `torch.set_num_threads(cpu_threads)` from the manifest; the trainer refuses to start if the live value differs.
4a. **Process environment (required).** Every manifest pins its process environment: `reference_spec.env.pinned = {"cpu": CLASS, "gpu": CLASS, "present": {NAME: VALUE}, "absent": [NAME]}` is a required field, and a manifest without it (or with `null`) is refused wherever it is parsed (`create_run` and `create_run_v2` answer 422, `TrainConfig`, the miner client, `env_probe --manifest`). ATen, MKL and oneDNN read their variables at first use, so the environment must be right before the process starts. The pin names one CPU class and one GPU class from closed tables (`hypertrain.protocol.env_pins`) and carries the environment those two rows imply; parsing accepts exactly the rows, so a table edit can never change what a signed manifest means. A run that uses no GPU says `"gpu": "none"`: that is a class, not an omission, and a pin with only a CPU part or only a GPU part is refused. The pin removes ambient-induced divergence only: it does not make two CPU vendors bitwise equal. The JSON schema lists the class names; the rows are here:

   | CPU class | `ATEN_CPU_CAPABILITY` | `MKL_CBWR` | `ONEDNN_MAX_CPU_ISA` | `OMP_DYNAMIC` | ATen must report | absent |
   | --- | --- | --- | --- | --- | --- | --- |
   | `avx2` | `avx2` | `AVX2` | `AVX2` | `FALSE` | `AVX2` | `OMP_THREAD_LIMIT` |
   | `avx512` | `avx512` | `AVX512` | `AVX512_CORE` | `FALSE` | `AVX512` | `OMP_THREAD_LIMIT` |

   | GPU class | Device (compute capability, SMs) | `HT_ATTN_BACKEND` | absent |
   | --- | --- | --- | --- |
   | `none` | no GPU: the run uses the CPU backend | `math` | `HT_COMPILE`, `HT_FP8`, `HT_MOE_GROUPED` |
   | `sm_103` | 10.3 (B300), 148 SMs | `math`, `sdpa-flash` | `HT_COMPILE`, `HT_FP8`, `HT_MOE_GROUPED` |
   | `sm_120` | 12.0 (RTX 5090), 170 SMs | `math`, `sdpa-flash` | `HT_COMPILE`, `HT_FP8`, `HT_MOE_GROUPED` |

   - **Why each name is pinned** (AMD EPYC 9575F, torch 2.14.0+cpu, MKL 2024.2, oneDNN 3.12). `ATEN_CPU_CAPABILITY` changes fp32 roots (torch ignores a value set after its first op). `MKL_CBWR` changes fp32 roots; on AMD every set value gives the machine's best branch, only unset and `COMPATIBLE` differ. `ONEDNN_MAX_CPU_ISA` changes bf16 roots (`AVX2` costs a bf16 matmul up to 358 times against the ambient, `AVX512_CORE` about 5.6 times), not fp32, and it is pinned in every class so that the class does not depend on the compute dtype. `HT_ATTN_BACKEND` is the attention domain: `sdpa-flash` changes roots, `math` equals unset on CPU, and on a GPU the code default depended on the architecture, so the manifest now says which. `OMP_DYNAMIC=TRUE` changed training-loop bf16 roots with 2 and 4 threads on one core. `HT_COMPILE`, `HT_FP8` and `HT_MOE_GROUPED` are read as `== "1"` in `trainer/model.py`, and `OMP_THREAD_LIMIT` changes the OpenMP team size while `torch.get_num_threads()` still reports `cpu_threads`: they can only be pinned unset.
   - **Measured under a hostile ambient** (`kimi_tiny.qualify`, one MICRO round, class pin over `ATEN_CPU_CAPABILITY=default MKL_CBWR=COMPATIBLE ONEDNN_MAX_CPU_ISA=ALL OMP_DYNAMIC=TRUE HT_ATTN_BACKEND=sdpa-flash HT_FP8=1`): `avx2` gives the deployed goldens (fp32 leaves root `7f01e9f9...`, bf16 `684efad6...`) and `avx512` gives fp32 `ef95c128...`, bf16 `b4465e3c...`; the two classes are two replay domains. `tests/trainer/test_env_pin_classes.py` pins this relationally (the hostile roots equal the clean class roots) because the roots are per CPU vendor.
   - **The GPU class is hardware, not an environment variable.** `sm_count` must be a count measured on the class (148 for `sm_103`, 170 for `sm_120`; the manifest is refused otherwise), and the worker refuses a device whose compute capability is not the class's. `HT_FP8` is an environment switch pinned absent in every class.
   - **Append-only, measured values only.** A row is never edited (a different environment is a new row) and never removed, so a signed manifest keeps its meaning; appending never invalidates one. A name or value enters with a recorded measurement that it changes round bytes. `MKL_CBWR=COMPATIBLE`, `HT_ATTN_BACKEND=flash-attn` and `NVIDIA_TF32_OVERRIDE` are not in the tables: no measurement yet. `tests/protocol/test_env_pins.py` keeps the digest of every first row and checks that every `HT_*` variable read under `trainer/` and `models/` is pinned or waived with a reason.
   - **Enforcement.** `TrainConfig.env_pin` carries the pin (required) and `require_reference_env` (threads, then pin) runs at the start of `train_round`, `train_island` and `train_unit_order`; `apply_reference_env` runs the same check before `init_process_group` in the island worker and in the repair and history workers, and the island worker also refuses a device outside the GPU class. It compares the pinned names with `os.environ` and the capability torch actually resolved (torch silently ignores a bogus value, or one set after its first op). MKL's code branch and oneDNN's ISA have no public getter: only their environment strings are checked, at check time. The in-process users (the v1 miner and auditor, the honeypot, the tests) refuse under a process that does not honor the pin and must be started under it.
   - **Launch.** `launch_island`, `launch_window` and `_launch_unbounded` start every island rank under `island_env(job, ...)`: the launcher's environment with the pin applied (present set, absent removed) plus the fixed keys, so a miner or auditor with a default or hostile ambient still runs the pinned environment. Before any capacity is charged or any process started, the parent asks `require_honorable` whether this host can honor the pin and raises `EnvPinRefused` (an `IslandFailure`) if not: a fault of the host, never evidence about the miner's work. The torchrun repair and history workers of the rollback path (`aggregator/rollback_v2.py`) are started under the same pin; they have no parent preflight, so a host that cannot honor it fails in the worker.
   - **Disputes.** The ranks of a publication ran in a child under the pin, which the miner's daemon environment need not honor, so every dispute is answered from the publication's actual-path trace, as CUDA publications already were; the referee re-runs windows through the same launcher.
   - **Admission.** A miner whose host cannot honor the pin is told at `join` and at the trial (`NetworkMiner.preflight_env_pin`), before any request, so no bond, admission or trial deadline is spent. `ChallengeStore.bootstrap_service_capacity_v2`, the CPU service profile the challenge host runs itself, always asks whether its host can honor the pin and answers 409 `SERVICE_PROFILE_ENV_PIN` when it cannot; the creating host of a run is not gated, because it may not be the executing host.
   - **The declared environment and the server's refusal.** Hardware hints stay advisory, but the environment is not: `JoinRequest.env` and `WorkScreenV2.env` (an `EnvObservation`: `env`, one entry per name the pin speaks about with the value the process saw or `null`; `cpu_capability`, what ATen resolved; `gpu`, the class of the device) are required, and they sit inside the bodies the party already signs (the join by hot and cold key, the screen by the hot key; no header). The miner's client fills the join from `require_honorable` and the screen from the rank summaries, and refuses a publication in which a rank declared another environment (`EnvPinRefused`). The server compares the declaration with the manifest's pin and answers 409 `ENV_PIN_DIVERGES: NAME, NAME` (the names that differ, sorted: the pinned variables, then `cpu_capability`, then `gpu`) at `POST /v2/runs/{run}/join`, in `POST /v2/runs/{run}/join/proof` before the reference is paid, and in `Admission.proof` before the roots are compared. Nothing is written first: no quota, admission, strike, suspension or nonce is consumed and the trial stays open, so the party fixes its host and posts again. The declaration is self-reported and is not authority: a divergence the process cannot see (MKL's branch or oneDNN's ISA after init, `LD_PRELOAD`) still ends as a MISMATCH of the trial or of a later audit.
   - **Binding.** The pin is part of the CPU service profile's `implementation_hash` (`ChallengeStore._service_implementation_v2`): the hash covers the whole pin body of the manifest (both classes, every `present` value and every `absent` name) and the files `protocol/env_pins.py` and `trainer/determinism.py`. The same code under another pin, or the same pin under edited tables or checks, is another implementation, and a profile signed for one does not authorize the other. The capacity worker's genesis check recomputes the hash under the pin of the manifest it is given.
   - **Honor system.** Anything outside the tables (`LD_PRELOAD`, `sitecustomize`, `PYTHONPATH`) is neither pinned nor checked, and a miner running modified code is not stopped by an environment check (its roots still decide).
5. **Hardware gate.** `sm_count` must equal the manifest (170 for RTX 5090, 148 for B300) and the device name must match. A CUDA network qualification also records each device's compute capability; the (capability, SM count) pair must be a row of `network_qualification.SM_COUNTS` (12.0 = 170, 10.3 = 148). The driver is set by the host and cannot live in the image: the miner reports it in Accept and the auditor replays on an allow-listed driver.
6. **Shapes.** Fixed micro-batch and sequence length, pre-tokenized u32 shards, no dynamic padding.
7a. **Aggregator norms.** `ht-norm-v1` (default; every existing run): the exactly rounded sum of float64 products (`math.fsum` semantics), computed vectorised by exact integer binning per float64 exponent. `ht-norm-v2` (opt-in, new runs): per tensor, float32 values are squared (or multiplied) in float64 (exact), zero-padded to a power of two, then reduced by the fixed pairwise tree `x = x[0::2] + x[1::2]` until one value remains; only elementwise IEEE adds are used, never `np.sum`/`np.add.reduce`, whose blocking is build-dependent. Per-tensor results are combined with `math.fsum` in UTF-8 tensor-name order. Implemented in `hypertrain.aggregator.core` (`NORMS`).
7. **Kernels.** Deterministic attention backward, MoE router ties broken by the lowest index, fixed-order reductions. Cross-rank sums inside an island are fixed-order `all_gather` plus local sum (`forbid_reductions` guards against NCCL `all_reduce`).
8. **Master state.** fp32 master weights and optimizer state; `compute_dtype` is `bf16` or `fp32`.
9. **Evidence.** Phase A on three rented hosts is summarized in [mechanism.md](mechanism.md#cross-host-bitwise-result) (section "Cross-host bitwise result"). The 8-GPU island result (Phase B) is in the same document, subsection "Cross-host bitwise result on 8-GPU islands".
