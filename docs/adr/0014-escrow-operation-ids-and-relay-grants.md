# ADR 0014: escrow key spaces, typed settlement references, finalize at acceptance, ledger growth, relay grants

- Status: draft for branch `v3-escrow-relay-fixes`, 2026-10-27 (Principal SN100 D45, D48, D49,
  D50, D51). Forge decides promotion to `v3`. Nothing here changes a production run, master,
  profile, emission, pin or digest, and no wire message that a manifest or `docs/BUNDLE_SPEC.md`
  covers.
- Order: brief `v3-escrow-relay-fixes`. Findings come from the Lens C1 report (families F1, F3,
  F4, F6, F7) and the Lens C2 report (TRN-02 and the IMPORTANT tail), tree `33b50656`
  (`origin/v3`, 2026-10-27). This ADR is written before any code. Order of work (D50, D51 5):
  F1 + F4, then M2 (the finalize half of F3), then F7a, then TRN-02, then F6, F7b, ESC-02 and
  the rest.
- Labels: **[V]** verified in this tree by reading or by a command (2026-10-27), **[A]** reported
  by a cited document or sibling branch and not re-read, **[H]** hypothesis, **[?]** unknown.
  Paths are relative to `hypertrain/src/hypertrain/` unless prefixed `tests/`, `docs/`,
  `scripts/`.
- Rules of the lot: no manifest-covered field changes; no schema migration (`CREATE INDEX IF NOT
  EXISTS` is allowed, there is no `ALTER TABLE` precedent [V]); every behaviour has a test that is
  red on `33b50656` and a mutant it kills; one fix per commit. The Lens' own conclusion frames the
  lot: "the risk is liveness and missing exits, not unit creation", no conservation flaw was found
  across 246 invariants [A: Lens C1 FINAL]. Every item below is a liveness or availability defect.

## Context

All findings were reproduced in-process on `33b50656` by at least one design analyst of this lot
[V]; the notes are in the lot report.

| Id | Defect | Where |
| --- | --- | --- |
| F1 (= ESC-01, ESC-D1..D4, BND-D01) | `escrow_events.operation_id` is `UNIQUE` and shared by client-chosen ids (`lock`, `transfer`, `release`) and the ids the service derives from public values (reward finalize, maturity, weights payout, dispute settlement). A coldkey holding one available unit signs any of them under a server id; the server's own operation then fails `OPERATION_CONFLICT` for ever and its transaction rolls back (finalize, the beacon route, `get_weights`, `settle_lock`). | `ledger/escrow_v2.py:233,246,281-289`; `challenge/store.py:737,2122,8379`; `challenge/disputes_v2.py:881` |
| F4 (= ESC-04, ESC-D6, DIA-D3) | A lock id, a dispute id and a reward-origin id are one string with three meanings: `settlement()` resolves it with `id=? OR json_extract(turn,'$.lock_id')=?` (no order, no type) and `release`/`slash`/`mature` never compare the answer with the object they act on. A bond named after an older dispute id makes the new dispute unresolvable; a lock named after a reward-origin id is released with that origin's settlement. | `challenge/disputes_v2.py:817-861`; `challenge/store.py:2318-2333`; `ledger/escrow_v2.py:907-1017` |
| M2 (F3, CRD-D02, A2W-D10) | `reward_allocations` re-validates the stored commit, verdict and audit challenge with `Intake.accept(..., finalized_beacon)`, which refuses an envelope whose `exp_drand` is below the finalize beacon. Honest envelopes expire at or before `d_final`, so any delayed finalize (an open contest, a beacon pause, operator lateness) fails for ever and `FINALIZE_ORDER` blocks the next rounds. | `ledger/escrow_v2.py:458-472`; `protocol/envelope_v2.py:347-348` |
| F7 (CL-18) | `EscrowV2.locked()` scans every `LOCK_ADMISSION` event and runs one unindexed `SUM` per lock, under the store lock, for the unauthenticated `GET /v2/runs/{run}/admission/{hotkey}` and for round open, accept and finalize. Measured on the real code: 1.08 s at 5,000 open locks, 5.4 s at 10,000, 96 s at 40,000 [V]; the beacon freshness gate pauses every v2 operation after 6 s. Every event also re-serialises the whole state in pure Python (94% of an event), so a restart `verify()` of 5,000 events takes 106 s. | `ledger/escrow_v2.py:291-315,1073-1086,1098-1164,1166-1179` |
| TRN-02 | The relay refuses every upload after 64 grants and `/readyz` turns false: `accept` and `ready()` count every record ever accepted, only a master-signed `replaces` deletes one, `release()` needs a `settlement` callback that `runtime()` never builds, and every miner is given the same relay. | `relay/core.py:144,275,779`; `relay/app.py:336-349`; `challenge/store.py:8458` |
| F6 (CL-14, SHD-D3) | Shadow maturity runs inside `push_beacon`'s per-run transaction. One origin whose custody check raises something other than three tolerated codes aborts the maturity of every origin of the run, the lease expiry and the notifications; the pusher exits on the 409. | `challenge/store.py:727-754` |
| ESC-02 | `escrow/lock|transfer|release` have no gate and no rate limit; each event holds the store lock and writes an O(state) snapshot. | `challenge/app.py:577-602` |

## Decisions

### Part 1. Server events get their own key space (F1)

1. Every event written by a server-only path is stored under the key `"srv:" + h`, where `h` is
   the hex HMAC-SHA256 of the derived id under an id key held by the master (D51 3: derived
   from the coordinator secret, `hmac(coord secret, "hypertrain/escrow-id/2")`, the same
   construction `_prefix_secret_v2` already uses [V `store.py:2536`]). A client id is a `Hex64`
   (`^[0-9a-f]{64}$`), so it can never start with `srv:`: the two key spaces are disjoint **by
   construction**, and a third party cannot compute the stored key of a server event even from
   the public inputs of its id. The guarantee is the disjointness; the keyed hash is depth, as
   D51 asks ("not choosable and not enumerable").
2. Client events (`lock`, `transfer`, client `release`) stay exactly as they are: bare `Hex64` key,
   same `UNIQUE` column. No consumer of a client lock id changes: `release`, `slash`, `locked()`,
   the dispute lookups, `admission.py:1100` and the `JOIN` of `verified_inputs_v2`
   (`store.py:7788`, one of the 28 hashed functions) keep reading bare ids.
3. `EscrowReceipt.operation_id` and every request body keep the raw id (`Hex64`, signed by
   clients); only the stored key and the event-hash preimage of **new server events** carry the
   prefixed key. No persisted byte is rewritten; no event hash, receipt or snapshot of an existing
   chain changes; no run id, golden or vector changes (no test or document pins an escrow event
   hash [V grep]).
4. The server entry points are `test_genesis`, `production_bootstrap`, `reward_finalize`
   (live and shadow), `mature`, `slash` and `pay`: they apply the server key themselves. `release`
   is the only method reached from both a client route and a server path
   (`DisputesV2.settle_lock`), so it takes a keyword `server: bool = False`; the **default is the
   client key space** (a forgotten flag on a client route cannot write into the server space).
   `settle_lock` passes `server=True`. Sibling code that releases or slashes with a server-derived
   id (`Admission.release_bond/seize_bond` of ADR 0010 Part 6) must pass it too.
5. Legacy rows. A chain written before this change holds its server events under bare ids.
   `_existing` for a server key falls back to the bare id **restricted to the server kinds and an
   identical request** (never to a `RELEASE`, which is a client kind), so replays of old server
   events return the stored receipt, `verify()` still opens a ledger that holds a finalized
   shadow reward, and a chain that was **already squatted heals**: the squatter's bare row is
   ignored and the server writes under its own key [V on a base-code chain, `f1db` notes].
6. `verify()` checks `receipt.operation_id` against the stored key (with the id key when the
   ledger has one) and finds shadow events by their request instead of by id, so the read-only
   authority snapshot (`scripts/network_authority_snapshot.py`, no coordinator secret) still
   verifies a chain.
7. Hardening in the same lot, not part of the guarantee: `EscrowV2.transfer` refuses
   `recipient == owner` (`SELF_TRANSFER`, after the replay lookup, so old self-transfers replay);
   the quota of Part 5 meters new client events. They remove the free vehicle, not the class.
8. **Residual, stated plainly (ESC-D5).** A client can still occupy another client's id, because
   the client space stays global: the miner CLI derives its admission lock id from public values
   (`miner/coldkey_ops.py:139-149`), a third party that signs any event under it makes the
   victim's identical retry fail `OPERATION_CONFLICT`. It is per victim and recoverable (another
   unit count or origin list changes the id), cannot block a server operation, and costs the
   squatter a funded key plus a quota slot. Closing it needs owner-scoped client keys, which
   changes lookups in `disputes_v2.py`, `admission.py`, the outbox primary key and the hashed
   `verified_inputs_v2`; it is **not** done here (see "Decisions for Forge").

Rejected: a `scope` column with `UNIQUE(scope, operation_id)` (needs a rebuild of two tables
that SQLite cannot `ALTER`, an old image then fails with `OperationalError`, per-owner lock ids
re-open settlement aliasing and change `verified_inputs_v2`, so `implementation_hash`); a
composite client key (changes the hash and every lock lookup); ids derived from `(owner, nonce)`
(a wire change of `EscrowOperation` and of the deployed CLI); refusing client ids that equal a
derivable server id (the set is unbounded); making server operations tolerate
`OPERATION_CONFLICT` (the id is the idempotency key and the occupant cannot be removed); a rate
limit alone (one unit funds unlimited squats).

### Part 2. Typed settlement references (F4)

1. A reference is typed by the call site or by the kind of the lock event, never by guessing from
   the string. `DisputesV2.settlement(dispute_id)` becomes a primary-key lookup; the new
   `DisputesV2.contest_settlement(lock)` finds the dispute through the lock's own signed
   `dispute_id` and requires `turn.lock_id == lock.operation_id`; `settle_lock` asks
   `settlement(turn.dispute_id)`; `ChallengeStore._settlement_v2` matches a dispute by id only
   and a reward origin by `records_v2`.
2. `EscrowV2.release` and `slash` ask `_lock_status(lock_id)`, which picks the resolver by the
   kind of the lock event (`LOCK_CONTEST` -> `contest_settlement`, fail closed
   `UNACCEPTED_SETTLEMENT` without one; every other kind -> the generic `settlement`) and requires
   `status.dispute_id == lock.dispute_id`. The name and shape are those of ADR 0010 Part 6, which
   adds the `LOCK_ADMISSION` branch; the two merge by inserting one branch.
3. No query can return two rows (dispute by primary key, lock event by its unique id, origin by
   `(run_id, kind, id)`), so there is nothing to order. This is the intent of D49 ("settlement
   query filtered by type with a deterministic ORDER BY"): the multi-row query is deleted instead
   of ordered. `DisputesV2.open` additionally refuses a bond whose id is an existing dispute id or
   reward-origin id (two primary-key lookups, `CONTEST_LOCK_ID_COLLISION`), as the brief asks; it
   is depth, because it is point-in-time and the typed lookups are what protect.
4. `Turn` gains no field: it is embedded in the coordinator-signed `WatchEvent` and `WireModel` is
   `extra="forbid"`, so a new field breaks the signature of every issued event and every old
   miner [V: `turn_field_check`].

"Non client-choosable lock ids" (D49) is met in the sense that matters: a client may still choose
the name of its own lock, but no other key space ever consults that name. Server-derived ids (dispute
ids, origin ids, operation ids) are not client-choosable and, for operation ids, not enumerable
(Part 1.1). Rejected: bond ids derived by the server (`sha256(["ht-contest-bond/2", run,
dispute_id])`): a public function in the shared client space is squattable like ESC-D5 and changes
nine test literals and every hand-built client.

Origin ids stay a public function of `(run, w, hotkey, finalize_hash)`: D51 3 asks that they are
never learnable. Keying them changes the derivation that the coordinator signs into
`RewardFinalize.origin_ids` and edits `finalize_v2` (hashed), and typed references plus the
disjoint operation space remove what a learned origin id could be used for. This is a decision
for Forge below, not done here.

### Part 3. Reward evidence is judged at its acceptance beacon (M2, D48)

Rule (D48): an envelope that was valid at its acceptance stays valid for the finalisation of its
round; `finalize` never redoes a temporal check on a commitment it already accepted. M2 is fixed in
**this** lot, in the same zone (`ledger/escrow_v2.py`) as F1, so that `v3-audit-integrity` (ADR 0011
owns M1, the lease queue and the DSP-01 exit) does not duplicate it. Merge order: F1 + F4 + M2
(this lot) before `v3-audit-integrity`.

1. `EscrowV2` gains a callback `accepted_at(envelope) -> beacon`, next to `owner_of` and
   `assignment_of`. `reward_allocations` calls `Intake.accept(envelope, accepted_at(envelope))` for
   the stored commit, each verdict and the audit challenge, instead of `finalized_beacon`. The ledger
   enforces `1 <= accepted <= finalized_beacon`; signature, signer role, run and "not expired at
   acceptance" are unchanged. A ledger without the callback fails closed
   (`ACCEPTANCE_AUTHORITY_UNAVAILABLE`, the idiom of `SHADOW_AUTHORITY_UNAVAILABLE`). Shadow evidence
   is already pinned to the trial's own finalization beacon and is not changed [V].
2. The store supplies the beacon from records it already keeps: `accepted_v2.accepted_beacon` for
   the commit and the verdict (the `auth:` row written by `_authenticated_v2`; the helper compares
   the whole stored envelope, not only the body digest), and `audit_leases_v2.created` for the
   coordinator's challenge (a lower bound of its sealing beacon). The helper is a new method beside
   `_settlement_v2` wired in `_services`; **no function of the 28 in `implementation_hash`
   changes** (measured: adding it leaves the hash at `02e208a3…89cc`, one statement in `finalize_v2`
   changes it) [V].
3. Not done, and why: bounding `exp_drand` at intake (hardening only: there is no single right
   minimum, honest clients seal `d_final`, `d_upload`, `lease_expires` and `deadline_beacon`, and it
   edits `training_v2`/`complete_audit_v2`, both hashed and edited by ADR 0011); sealing the challenge
   beyond `d_final + vesting` (not needed once evidence is judged at acceptance).
4. The operator tool seals the Finalize envelope with `exp = d_final` (`scripts/ops/live_operator.py`),
   so a finalize command after `d_final` is refused by the **Finalize** envelope itself even with the
   ledger fixed. The tool seals it with `max(d_final, now) + 1000` (the margin the trial Finalize
   already uses). The Finalize is a fresh request, not stored evidence, and keeps its check.
5. The mandatory test (D48): a round finalized at `d_final + N` after realistic expiries (commit
   `d_final`, verdict `lease_expires`, challenge `absolute`) settles, `FINALIZE_ORDER` advances, and a
   repeat returns the stored result; a ledger test with `exp` below the finalize beacon and above the
   acceptance beacon settles; an envelope that expired **before** its acceptance beacon is still
   refused. "After a contest" is exercised through the existing MATCH-verdict contest closed by
   `referee_v2`; ADR 0011's DSP-01 closure adds the assertion after both lots merge.

### Part 4. Third-party growth of ledger state (F7)

F7a (liveness, conditional blocker):

1. `locked()` is rewritten on one statement over a **partial index** created with `CREATE INDEX IF
   NOT EXISTS escrow_open_admission_locks ON escrow_units(owner, ref) WHERE bucket='admission_locked'
   AND units>0`. Its cost is the number of **open** locks of the owner, not the ledger. It returns
   the same `(units, hash)` bit for bit (checked on real 1,000- and 2,000-lock chains and by a
   differential test against the legacy statement), because the value feeds `lock_receipt_hash`
   in the signed tape. The index is derived data kept by SQLite in the same transaction, an older
   image on a database that has it is harmless, and `network_authority_snapshot` builds it in
   memory [V]. No side table or running aggregate: a missed writer would make `locked()` silently
   wrong and it decides `eligible`. A general `(owner, bucket, ref)` index must not be added later
   without re-checking the plan (a test pins `EXPLAIN QUERY PLAN`).
2. A cap of **64 open `LOCK_ADMISSION` locks per coldkey** (2 × `max_pending`), enforced in
   `EscrowV2.lock` after the replay lookup (a coldkey at the cap still gets its old receipt back;
   the miner CLI and the protected-opening fixture re-post saved locks) and pre-checked in
   `Admission.lock` so no outbox row is left that `reconcile_lock` can never apply. Refusal:
   `OPEN_LOCK_CAP` (HTTP 409). Contest locks are not capped (33 contests by one owner is a pinned
   case). A minimum lock amount is not added: the cap bounds the count regardless of amount.
3. `GET /admission/{hotkey}` stays public and gets cheap, not authenticated or cached: four
   in-repo clients (the miner, the coldkey CLI, the operator tool, the proof script) call it unsigned
   and ADR 0010's `admission_owner()` relies on admission ids being public. After the index it costs
   2.4 ms at 100 locks and 0.17 s at 40,006 locks that the cap makes unreachable [V]. A flood of
   cheap GETs is the generic ingress gap (no limiter exists), not F7a.

F7b (IMPORTANT):

4. A local encoder in `escrow_v2.py` replaces the pure-Python `canonicalize` in `_snapshot`,
   `_event` and `verify()`: `json.dumps(value, separators=(",", ":"), ensure_ascii=False,
   allow_nan=False)` is byte-identical to JCS for lists of strings, integers within ±(2^53−1) and
   `None` (all 5,006 stored event hashes of a real chain recompute identically; 3,000 Hypothesis
   examples agree [V]) and 21× faster for events and `verify()`; a range guard keeps the errors
   `canonicalize` raised. `protocol/jcs.py` is not touched.
5. `transfer` skips a zero credit; `lock`, `release` and `slash` do **not**: a lock that lists two
   origins leaves a zero-unit row for the second, and `settle_lock` rebuilds the release from the
   original origin list, which `_move` requires a row for [V reproduced]. `SELF_TRANSFER` (Part 1.7).
6. **Not done: a digest in the event-hash preimage with periodic full checkpoints** (storage and
   `verify()` become linear, prototype 3,000 locks in 7.6 s against 92 s). It changes the hash format
   of all new events and makes the chain forward-only (an older image refuses to start); the escrow
   chain format is documented nowhere. It needs maintainer approval; see "Decisions for Forge". Row
   deltas are not small and are rejected.

### Part 5. Escrow ingress (ESC-02)

1. New client events (never a replay, never a service-driven call) are metered by the quota that
   `join` already uses: identity `escrow:<owner>:<now // work_screen_epoch_rounds>`, rate 0, burst
   `join_burst`. It is charged after the signature is verified, before any ledger work, in the
   same transaction (a refusal consumes neither the token nor the replay nonce). Refusal
   `ESCROW_QUOTA` (HTTP 409). No new policy field. The cleanup uses a key-range predicate that can use
   the primary-key index (a default `LIKE` cannot).
2. A transfer must go to a recipient the ledger already knows (`UNKNOWN_RECIPIENT`): the quota alone
   is bypassed by hops to fresh keys (79 signers accepted once each, reproduced over HTTP [V]). No
   in-repo flow transfers to an unknown wallet. This is a product rule; see "Decisions for Forge".

### Part 6. Relay grants (TRN-02, D51 5)

1. The quota counts **held** uploads: a reservation whose service deadline has not passed and has no
   receipt, or custody whose pins are neither released nor past their horizon. Receipt and deadline
   release a reservation; a lapsed horizon stops counting custody. Not counting is not deleting: the
   bytes stay pinned until the master-signed release, no signed promise is dropped.
2. The default is **no count cap**; the physical limit is the journal, one JSON blob of at most 16 MiB
   rewritten whole on every mutation. `accept` refuses with `relay journal capacity exhausted` before a
   mutation would take the journal above a 12 MiB budget (4 MiB headroom for chunk acks, receipts,
   retrievals, extensions and releases). `RelayConfig.max_uploads` is an optional operator knob.
   `ready()` no longer depends on the count: the exit path (release, extension, retrievals, chunk PUTs)
   goes through the same Service, so a quota that turns `/readyz` false removes the means of clearing it.
3. The reclaim path is wired: a public, read-only master route returns a coordinator-signed
   `RelaySettlementView` (built from `relay_settlement_v2`), the relay verifies it with
   `master_message` and feeds the existing `settlement` callback; no admin credential enters the relay.
   Spent records (all pins released and past `delete_after`, or a reservation with no custody past its
   deadline) are pruned at the end of `collect()`, which a periodic task now calls. Who releases a
   grant is still an operator action (`POST .../admin/relay/{grant}/release`).
4. Rejected: renewal of the 64 grants (a new wire message that refreshes a number with no physical
   meaning); spreading assignments (production has one relay [A], and `assignment_policy` is a literal
   pinned in nine places); embedding the settlement fields in `CustodyRelease` (smaller, but removes the
   cross-check against the master's current record); deleting lapsed unreleased records (their bytes
   would leak with no record to release them). A second relay in the signed registry is needed before
   any run of 16 miners or more, not before the off-chain proof (D51 5).

### Part 7. Shadow maturity is failure-isolated (F6, CL-14)

`push_beacon` isolates each origin: the three "waiting" codes stay silent, ledger conservation and
`sqlite3.Error` stay fatal, anything else is journaled as `records_v2 kind='maturity-failed'` (id =
origin, rewritten only on change and without a beacon field, because `_rollback_snapshot_v2` hashes
every record) and the loop goes on. Each `mature` already runs under a savepoint. The failure is
not silent (the record is the alarm) and retries every round. After Part 1, `OPERATION_CONFLICT` on a
server id can no longer come from a client; it is isolated and journaled, not tolerated.

### Part 8. No signature in HTTP headers (D51 2)

The master proxy is the only entry path and forwards only `content-type`, `accept` and
`authorization` [A: `src/cortex/challenges/proxy.py:14`]. Rule: whatever must be signed is signed in
the **body**, as a signed envelope with an expiry. The two static header signatures that SIG-01 and
DSP-09 flag (the dispute watch `X-Dispute-Signature` and the trial job `X-Trial-Signature`, replayable
for ever) move to body envelopes; the dispute watch also gets a per-party and a total bound on
concurrent long polls, on its own thread pool. The `PUT /objects` header signature
(`X-Object-Signature`, content-bound) and the signed challenge GET that ADR 0010 decision 3 adds are
in the same situation and are listed under "Decisions for Forge". Other IMPORTANT items follow in
their own commits and are recorded in the report with their sibling status: HTP-07 (the observer
challenge is handed back until answered; the pin id carries the round), ADM-04 (no store lock on
the event loop before the signature check; the anonymous relay-receipt route too), ADM-06,
ESC-05, TRN-03, TRN-05. ADM-03 belongs to `v3-executor-perf` (ADR 0012 Part B); ADM-02 waits for
`v3-audit-integrity` because it edits `lease_v2`.

## Consequences

- `implementation_hash`: Parts 1 to 7 edit no function of the 28 and no file of the 10; the value
  stays `02e208a3…89cc` and the capacity profile `tiny-cpu-service-32-v1`. Items that would change it
  (ADM-06 `rollback_v2`, ADM-02 `lease_v2`, TRN-03 `training_v2`) are separate, last, and listed in the
  report with the value after them.
- Schema: no table, column or key changes. One partial index is created if absent.
- Compatibility: new server events carry a new key and new hashes; old chains verify unchanged and
  are forward-only for the first new server event (an older image then fails closed with an
  `EscrowError` and cannot write: take a database backup before the digest moves, as
  `deploy/README.md` and `docs/OPERATOR_SECURITY.md` already say). HTTP gains `OPEN_LOCK_CAP`,
  `SELF_TRANSFER`, `ESCROW_QUOTA`, `UNKNOWN_RECIPIENT`, `CONTEST_LOCK_ID_COLLISION` (all 409) and 429 on
  the dispute watch. The relay gains one message type and one public GET; an old relay still fails
  `release` closed.
- `sources()` of `network_qualification` changes like for any source edit; stored qualification
  configs validate against their own frozen tree.
- A chain that already holds an early release of an open dispute's bond (the F4 evasion happened)
  stays wedged after the fix (`settle_lock` fails `UNFUNDED_OR_UNKNOWN_ORIGIN`) [V]. No such chain is
  known.

## Decisions for Forge

1. **Keyed origin ids (D51 3).** Done: operation ids are keyed and disjoint. Not done: origin ids.
   Keying them changes a derivation the coordinator signs and edits `finalize_v2`. Say if the
   learnability of an origin id is a requirement on its own after Part 2.
2. **ESC-D5.** Accept the residual of Part 1.8, or approve owner-scoped client keys (a bigger change in
   four consumers, a hashed function and the outbox key) and a CLI lock id derived with a secret.
3. **F7b structural.** Approve (or refuse) the digest-in-preimage scheme with 1,000-event checkpoints.
4. **Caps and quota.** 64 open admission locks per coldkey; escrow burst 4 per window (the analyst
   suggests 8 if top-ups and retries matter); the `UNKNOWN_RECIPIENT` rule.
5. **Header signatures.** `PUT /objects` and the ADR 0010 challenge GET against D51 2: move them too?
   ADR 0010 decision 3 is a header signature and must change before it merges.
6. **Early-release chain** (Context): is a repair needed for a chain that already released a bond?

## Not verified

No run on the production master or behind the real Cortex proxy; real SQLite 3.40 (the image's) was
not available, plans were checked on 3.46 and 3.53; the F7 numbers are CPU/SQLite cost on tmpfs
(no `fsync`); production roster, round cadence and relay count; the gateway's behaviour when all relay
pods are unready; a threaded reproduction of DSP-03 (lot `v3-audit-integrity`); shadow reward delay
at store level. The contest-then-finalize chain with the DSP-01 closure needs that lot.
