# hypertrain challenge routes (contract v1)

Source: `src/hypertrain/challenge/app.py` (routes) and `store.py` (state).
Behind the Cortex proxy, public paths are served at `/challenge/hypertrain/<path>`.
Every request body is at most 1 MiB (`413` above that).
Bodies are parsed raw, so duplicate JSON keys and `NaN`/`Infinity` give `400`.
Tensors never travel through these routes; they go out-of-band via presigned URLs.

The only clock is the latest drand round D that the operator has pushed and the container has verified.

## Errors

Errors have the shape `{"detail": str}`.

| Status | Meaning |
| --- | --- |
| 400 | Malformed, expired or wrong-run envelope |
| 401 | Bad bearer token or bad signature |
| 403 | Signer not allowed: unregistered hotkey, not on the roster, not an auditor or K_coord |
| 404 | Unknown run, round or job |
| 409 | Wrong state, replay, lease not current, or window closed |
| 413 | Body too large |
| 422 | Schema or contract mismatch |
| 503 | Route not configured (secret file missing) or metagraph unavailable |

## Environment

| Variable | Meaning |
| --- | --- |
| `CHALLENGE_SLUG` | Default `hypertrain` |
| `CHALLENGE_STATE_DIR` | Default `/data`. Holds the SQLite WAL `challenge.db`, the hash-chained ledger journal `ledger/journal.jsonl`, and `objects/` |
| `CHALLENGE_INTERNAL_TOKEN_FILE` | Bearer for `get_weights` |
| `CHALLENGE_ADMIN_TOKEN_FILE` | Bearer for admin, beacon push and aggregator routes |
| `CHALLENGE_WORKER_TOKEN_FILE` | Bearer for auditors and referees |
| `CHALLENGE_MASTER_URL` | Used for `GET /v1/metagraph/latest`, which returns `{"hotkeys": {ss58: uid}}` and is cached 30 s |
| `HYPERTRAIN_COORD_KEY_FILE` | K_coord sr25519 seed: 32 raw bytes or 64 hex. It is never the Cortex leaf seed |
| `HYPERTRAIN_OWNER_HOTKEY` | ss58 of K_owner, which signs RunManifest |
| `HYPERTRAIN_GENESIS_UNIX` | Ledger `Params`. Default: quicknet genesis |
| `HYPERTRAIN_EPOCH_SECONDS` | Ledger `Params`. Default 4320 |
| `HYPERTRAIN_EPOCHS_PER_ROUND` | Ledger `Params`. Default 1 |
| `HYPERTRAIN_VEST_ROUNDS` | Ledger `Params`. Default `ceil(1/0.1)` = 10 |

Token files are read on every request.
`/health` returns 200 only when the state DB is writable, the internal token is readable and K_coord loads. Otherwise it returns 503.
`/version` always works, which is what the supervisor canary needs.

## Envelopes

Signed messages use the protocol envelope from `hypertrain.protocol.envelope.seal`:

```
{"v": "ht/1", "type": T, "run_id": hex64, "body": {...}, "signer": ss58, "exp_drand": int, "sig": hex128}
```

- The signed preimage is `signing_message(T, body, exp_drand, run_id)`.
- Intake runs `Intake.accept` with D as `now_round`.
- The replay set (`Intake.seen`) is persisted in SQLite table `seen`, so it survives restarts.
- A handler rejection releases the replay key, so a corrected resend is not blocked.

Allowed signers:

| Message | Allowed signer |
| --- | --- |
| Accept, Commit, DeltaManifest, StateServe, Dispute | The miner; body `hotkey` must equal `signer`, and the signer must be registered on the metagraph |
| Bisect | The disputing miner or the disputed auditor (`party`) |
| ReplayVerdict, Resolution | `manifest.auditors`. A Resolution must not come from the disputed auditor |
| RoundOpen, Rollback, Finalize | `manifest.coord_pubkey` (K_coord) |
| RunManifest | `HYPERTRAIN_OWNER_HOTKEY`, with `run_id = sha256(JCS(body))` |

Container-issued records (Receipt, AuditChallenge, Forfeit, round-0 RoundOpen) are sealed by K_coord with `exp_drand = 2^53-1`.

## Contract routes

### `GET /health`
Returns `{"ok": bool}` with status 200 or 503.

### `GET /version`
Returns:

```
{"slug": "hypertrain", "version": "0.1.0", "contract": 1, "capabilities": ["get_weights", "proxy_routes"]}
```

### `GET /internal/v1/get_weights?epoch=E[&epoch_at=T]`
- **Headers:** `Authorization: Bearer <internal>` and `X-Platform-Challenge-Slug: hypertrain`.
- **Answer:** the canonical JSON bytes from `Ledger.get_weights`:

  ```
  {challenge_slug, epoch, weights{hotkey: float}, full_share_mass: 1000000, metadata, computed_at}
  ```

- The first answer per epoch is persisted in the journal and replayed byte-identical.
- It returns 200 with empty weights while nothing has vested; it never returns 503 for "pending".
- When `epoch_at` is omitted it defaults to `genesis + (E+1)*epoch_seconds`.

## Admin routes (Bearer admin)

| Route | Body | Result |
| --- | --- | --- |
| `POST /v1/admin/beacon` | drand JSON `{round, signature, randomness}` | Verified (quicknet BLS in production) and stored. Conflicting payload for the same round: 409. Returns `{round, latest}`. Audit selection runs when the `d_audit` round arrives |
| `POST /v1/admin/runs` | RunManifest envelope signed by K_owner | 201 `{run_id, status:"created"}`. Requires `coord_pubkey` to be this container's K_coord, and `budget.epochs_per_round`, `verify.E_vest_rounds` and `forgive_per_epoch` to equal the ledger Params. One run per state dir (409) |
| `PUT /v1/admin/runs/{run_id}/config` | `{train_rounds: int>=1, final_after_upload: int>=1}` in drand rounds | `d_commit = d_assign + train_rounds`, `d_final = max(d_audit, d_upload) + final_after_upload`. 409 while running |
| `PUT /v1/admin/runs/{run_id}/paused` | `{paused: bool}` | Unpausing needs a config, at least one rostered miner and a pushed beacon. The first unpause opens round 0 at `d_open = D+1` |
| `PUT /v1/admin/runs/{run_id}/roster/{hotkey}` | `{bond: bool, probation: bool, cluster: str?, region: str?, flags: [str]}` | Admission. Flags `outlier` or `copy` force q=1 |
| `DELETE /v1/admin/runs/{run_id}/roster/{hotkey}` | none | Removes the miner from the next rounds |
| `PUT /v1/admin/runs/{run_id}/honeypot` | `{commitment: hex64}` | Copied into the next RoundOpen's `honeypot_commit` |
| `POST /v1/admin/runs/{run_id}/honeypot/reveal` | `{commitment, members:[{hotkey, mode:"honest"\|"fabricate"\|"last_step"\|"noise"}], salt: hex64}` | Checked with `hypertrain.auditor.honeypot.verify_reveal`, the same commitment as `honeypot.commit`. 422 on mismatch. Every non-honest mode counts as adversarial in auditor-stats |

## Public routes (miners; no bearer)

| Route | Body | Result |
| --- | --- | --- |
| `GET /v1/runs` | none | `{runs:[{run_id, status}]}` |
| `GET /v1/beacon/latest` | none | `{round, signature, randomness}` |
| `GET /v1/runs/{run_id}` | none | `{run_id, status, config, manifest_envelope, now_round, rounds:[{w,state}], roster:[...]}` |
| `GET /v1/runs/{run_id}/rounds/{w}` | none | `{state, now_round, round_open (signed), base, assignment:[{hotkey, slot, samples, assignment_hash}] (after d_assign), audit_beacon_round, audit_beacon_signature, selected, miners, jobs, forfeits, rollback, finalize}` |
| `GET /run/{run_id}/auditor-stats` (alias `/v1/runs/{run_id}/auditor-stats`) | none | `{epochs:[{epoch, audits, catches, hp_bad, hp_bad_caught, hp_honest, hp_honest_flagged, honeypot_catch_rate, honeypot_false_positive_rate}]}`. Honeypot rates count only revealed commitments |
| `POST /v1/runs/{run_id}/accept` | Accept envelope | Allowed for `d_assign <= D < d_commit`. Checks reference `image_digest`, the driver allowlist (when it is non-empty) and `assignment_hash`. Returns `{w, hotkey, status:"ACCEPTED", slot}` |
| `POST /v1/runs/{run_id}/commit` | Commit envelope | `{status:"COMMITTED"\|"EXCLUDED", receipt}`. The receipt is a K_coord-signed Receipt `{w, commit_hash: body_digest(commit body), received_round: D}`. When `received_round >= d_audit` the status is EXCLUDED: no ledger commit and no entitlement |
| `POST /v1/runs/{run_id}/uploads` | `{w, hotkey, sha256}` | `{method:"PUT", sha256, url}`. A presigned URL from the object-store interface, only for the committed `delta_hash` before `d_upload` |
| `POST /v1/runs/{run_id}/leaves` | `{w, hotkey (registered), preimages:[LeafPreimage], ef_in_sha256?}` | Requires COMMITTED/UPLOADED before `d_upload`. The `n_leaves` preimages must belong to this run/round and their Merkle root (over `LeafPreimage.digest()`) must equal the committed `leaves_root`. They are self-authenticating, so no signature is needed. A job is leasable only after its leaves are stored |
| `POST /v1/runs/{run_id}/rerun` | `{w, hotkey, leaves_root, sig}`, with `sig = sr25519(hotkey, "hypertrain/1\|Rerun\|run_id\|w\|hotkey\|leaves_root")` | The miner's own rerun after a MISMATCH, before finalize, once per round. Classified with `hypertrain.auditor.replay.classify_mismatch`. TRANSIENT clears the Forfeit and blacklist, and the round is written to the ledger as TRANSIENT (round reward burned, escrow kept, forgiven `forgive_per_epoch` times). Returns `{classification: TRANSIENT\|CONTEST\|FAULT}` |
| `POST /v1/runs/{run_id}/delta` | DeltaManifest envelope | Must come before `d_upload`. `delta_hash` and `size` must equal the Commit's. Status becomes UPLOADED |
| `POST /v1/runs/{run_id}/state` | StateServe envelope | Only for `segments` mode, before `serve_deadline`. `uri` ends with the blob sha256 of a `replay.pack_state(theta, st)` object already uploaded. `tensor_root` must equal `hypertrain.auditor.replay.tensor_root(theta, st)` of that blob (422 otherwise) |
| `POST /v1/runs/{run_id}/dispute` | Dispute envelope (`verdict_hash = body_digest(verdict body)`) | `{dispute_id = body_digest(dispute body), state:"open"\|"accepted"}` |
| `POST /v1/runs/{run_id}/bisect` | Bisect envelope | `{dispute_id, seq}`. Bisects are stored in order for the referee |
| `POST /v1/runs/{run_id}/resolution` (Bearer worker) | Resolution envelope from an auditor other than the disputed one | If the loser is the auditor, the miner is restored to MATCH and un-blacklisted. If round w is already finalized, the ledger `vindicate` event credits the forgone NO_UPLOAD slice. It is idempotent per (run_id, w, hotkey), vests from the credit's round, and moves units from burned to pending. The response carries `credited: bool` (pay on vindication, ultrabrain section 1.2). If the loser is the miner and round w is already finalized (it was carried as NO_UPLOAD), the ledger `fault` event is applied once per (run_id, w, hotkey). The forgone round slice stays burned, all outstanding entitlements burn, and the hotkey is blacklisted with no further payments. The response carries `faulted: bool`, and the Forfeit is resealed with cause DISPUTE_LOST and the burned units |

### Round states
Round states are derived purely from D:

- `OPEN` while `D < d_assign`.
- `ASSIGNED` at `D == d_assign`.
- `TRAINING` while `D < d_commit`.
- `COMMIT_CLOSED` while `D < d_upload`.
- `UPLOAD_CLOSED` until the aggregate is posted.
- `APPLIED` once round w+1 is opened, until round w+1's `d_assign`.
- `AUDIT`, or `DISPUTE` while any dispute is open.
- `FINAL` after Finalize.
- `VESTING` once the ledger round passes the finalize round.
- `RELEASED` at `final_round + vest_rounds`.

Per-miner statuses are `ASSIGNED`, `ACCEPTED`, `COMMITTED`, `UPLOADED`, `EXCLUDED`, `MATCH`, `MISMATCH`, `WITHHELD`, `BAD_PROOF` and `ASSIGNMENT_VIOLATION`.

### Assignment
The assignment is `hypertrain.data.assignment.assign_round` with these inputs:

- `run_id`, `w`, and `drand_sig[d_assign]`;
- `n_samples` = `dataset.n_samples`;
- `n_slots` = roster size;
- `batch` = `micro_batch * grad_accum * H`;
- `base` = previous base + previous slots × batch, moved to the next data-pass boundary when the round would straddle it.

The assignment hash is:

```
assignment_hash = sha256("ht-assignment-v1" || run_id(32B) || u64be(w) || u64be(slot) || u64le(sample)*)
```

### Audit selection
Selection runs when `drand[d_audit]` is pushed, over COMMITTED and UPLOADED miners:

```
sel = sha256("ht-audit" || run_id(32B) || u64be(w) || drand_sig[d_audit] || hotkey utf8) < q_i * 2^256
```

- The comparison is exact on the f32 value of `q_i` from RoundOpen roster.
- `q_i` is 1.0 for probation (`probation` flag, or not bonded within `verify.probation_rounds` of admission), for `outlier`/`copy` flags, and for members of a cluster with a blacklisted member. Otherwise it is `verify.q_base`.
- The mode is `full` unless `inner.state_policy == "carry"`. Then it is `segments`. Segments are leaf windows `[a, a+1]` with `0 <= a < H/J`, in leaf units rather than inner steps. They come from `hypertrain.auditor.replay.select_segments(run_id, w, beacon_sig_sha256, target, committed_norms(preimages), verify.k_segments, verify.Q_top)`, the same call the auditor uses to check them: the final window always, Q_top windows by largest committed `norm_f32`, and k windows drawn from `sha256("ht-segments|"...)`.
- In segments mode, the challenge for a selected miner is issued once its leaf preimages are stored, or at `d_upload` if they are missing. Missing preimages give zero norms.
- StateServe for window `[a, b]` carries `t = a*J`, the state after inner step `a*J`, plus the Merkle proof of leaf `a`.
- A fault verdict maps to a Forfeit cause through `auditor.replay.forfeit_for`. For example, `BAD_PROOF` is sealed as cause `MISMATCH`.

## Auditor routes (Bearer worker; OpenType lease pattern)

### `POST /v1/worker/lease`
Returns 204 when there is no job. Otherwise 200:

```
{id, lease, lease_expires_round, run_id, w, target,
 challenge (K_coord-signed AuditChallenge envelope), challenge_hash (= body_digest(challenge body)),
 round_open (signed RoundOpen envelope), commit (miner Commit envelope),
 delta_manifest (envelope|null), state_serves ([StateServe envelopes])}
```

The job object also carries the replay inputs that `hypertrain.auditor.worker.Job.parse` reads:

```
manifest (RunManifest body), challenge (AuditChallenge body), commit (Commit body),
leaves [hex LeafPreimage.digest()], preimages [LeafPreimage body],
assignment {sample_ids (this slot's derived samples), global_step0 = w*H},
theta_start_sha256, ef_in_sha256|null, v0_sha256|null,
challenge_envelope / commit_envelope (the signed originals)
```

Blobs are `replay.pack_state` safetensors in the object store.

- A lease lasts 600 drand rounds (30 min).
- An expired lease is re-queued on the next lease call, with a new lease token.

### Other job routes

| Route | Body | Result |
| --- | --- | --- |
| `GET /v1/worker/jobs/{id}/serves?lease=` | none | `{now_round, serves:[{serve: StateServe body, blob_sha256}]}`. 409 when the lease is not current |
| `GET /v1/objects/{sha256}` | none | Raw bytes. Needs the worker bearer. 404 if unknown, 400 for a bad key |
| `POST /v1/worker/jobs/{id}/heartbeat` | `{lease}` | `{id, lease_expires_round}`. 409 when the lease is not current |
| `POST /v1/worker/jobs/{id}/complete` | `{lease, verdict: ReplayVerdict envelope signed by a manifest auditor}` | `{id, state:"done", result, forfeit?}`. For MISMATCH, WITHHELD, BAD_PROOF or ASSIGNMENT_VIOLATION a K_coord-signed Forfeit is recorded and the hotkey is blacklisted. The provisional Forfeit claims `round_reward_burned = escrow_burned_units = 0`, because the ledger burns nothing before finalize. At finalize (FAULT verdict), and after a post-finality lost dispute, it is resealed with the units the ledger actually burned (`Ledger.burned_for_fault`) |
| `POST /v1/worker/jobs/{id}/fail` | `{lease, reason, retry: bool}` | `{id, state:"queued"\|"failed"}` |

## Aggregator routes (Bearer admin; outputs signed by K_coord)

| Route | Body | Result |
| --- | --- | --- |
| `GET /v1/aggregator/runs/{run_id}/rounds/{w}/inputs[?d_open=]` | none | `{state, now_round, round_open, base, miners:[{hotkey, status, selected, received_round, commit, receipt, delta_manifest, verdict}], selected, rollback, next_round_template}`. The template holds every RoundOpen field except the four state hashes |
| `PUT .../rounds/{w}/state` | `{theta_start_sha256, v0_sha256?}` | Publishes round w's public start state (the blobs must already be in the object store). Write-once. Audit jobs for round w are leased only after it is published |
| `POST .../rounds/{w}/aggregate` | RoundOpen envelope for w+1 | Allowed when D >= `d_upload(w)`. Must equal `next_round_template` plus `prev_final_hash`, `theta_hash`, `outer_state_hash` and `center_hash`. Round w becomes APPLIED |
| `POST .../rounds/{w}/rollback` | Rollback envelope | Only before Finalize. `excluded` must be committed miners of round w |
| `POST .../rounds/{w}/finalize` | Finalize envelope | Allowed when D >= `d_final` and rounds finalize in order. `included` must equal the sorted hotkeys with MATCH or unsampled+uploaded. Writes ledger verdicts: MATCH, UNSAMPLED, FAULT, or NO_UPLOAD (no delta, or rolled-back unresolved audit). Then `Ledger.finalize`. Open disputes must be rolled back first |
