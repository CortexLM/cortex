# ADR 0014: escrow key spaces, typed settlement references, finalize at acceptance, ledger growth, relay grants

- Status: draft for branch `v3-escrow-relay-fixes`, 2026-10-27 (Principal SN100 D45, D48, D49,
  D50, D51, D52 and D53, relayed by Forge; D52 and D53 amend the first text). Forge decides
  promotion to `v3`. Nothing here changes a production run, master, profile, emission, pin or
  digest, and no wire message that a manifest or `docs/BUNDLE_SPEC.md` covers.
- Order: brief `v3-escrow-relay-fixes`. Findings come from the Lens C1 report (families F1, F3,
  F4, F6, F7) and the Lens C2 report (TRN-02 and the IMPORTANT tail), tree `33b50656`
  (`origin/v3`, 2026-10-27). This ADR is written before the code. Order of work (D50, D52, D53):
  F1 + F4, then M2 (the finalize half of F3), then F7a, then TRN-02, then F6, F7b, ESC-02 and
  the rest; one commit per finding, each with tests that are red on its parent and green on it.
- Labels: **[V]** verified in this tree by reading or by a command (2026-10-27), **[A]** reported
  by a cited document or sibling branch and not re-read, **[H]** hypothesis, **[?]** unknown.
  Paths are relative to `hypertrain/src/hypertrain/` unless prefixed `tests/`, `docs/`,
  `scripts/`.
- Rules of the lot: no manifest-covered field changes; no schema migration (`CREATE INDEX IF NOT
  EXISTS` is allowed, there is no `ALTER TABLE` precedent [V]); every behaviour has a test that is
  red on its parent commit and a mutant it kills. The Lens' own conclusion frames the lot: "the risk
  is liveness and missing exits, not unit creation", no conservation flaw was found across 246
  invariants [A: Lens C1 FINAL]. Every item below is a liveness or availability defect.
- Single owners of shared rules (D52): the rule for a round that closes without a reward
  (`FINALIZE_ORDER`) is stated once, in ADR 0011 (D51 4); this ADR refers to it and does not treat it.

## Context

All findings were reproduced in-process on `33b50656` by at least one design analyst of this lot
[V]; the notes are in the lot report.

| Id | Defect | Where |
| --- | --- | --- |
| F1 (= ESC-01, ESC-D1..D4, BND-D01) | `escrow_events.operation_id` is `UNIQUE` and shared by client-chosen ids (`lock`, `transfer`, `release`) and the ids the service derives from public values (reward finalize, maturity, weights payout, dispute settlement). A coldkey holding one available unit signs any of them under a server id; the server's own operation then fails `OPERATION_CONFLICT` for ever and its transaction rolls back (finalize, the beacon route, `get_weights`, `settle_lock`). Origin ids are also a public function of `(run, w, hotkey, finalize_hash)` (D51 3). | `ledger/escrow_v2.py:233,246,281-289`; `challenge/store.py:737,2122,8354,8379`; `challenge/disputes_v2.py:881` |
| F4 (= ESC-04, ESC-D6, DIA-D3) | A lock id, a dispute id and a reward-origin id are one string with three meanings: `settlement()` resolves it with `id=? OR json_extract(turn,'$.lock_id')=?` (no order, no type) and `release`/`slash`/`mature` never compare the answer with the object they act on. A bond named after an older dispute id makes the new dispute unresolvable; a lock named after a reward-origin id is released with that origin's settlement. | `challenge/disputes_v2.py:817-861`; `challenge/store.py:2318-2333`; `ledger/escrow_v2.py:907-1017` |
| M2 (F3, CRD-D02, A2W-D10) | `reward_allocations` re-validates the stored commit, verdict and audit challenge with `Intake.accept(..., finalized_beacon)`, which refuses an envelope whose `exp_drand` is below the finalize beacon. Honest envelopes expire at or before `d_final`, so any delayed finalize (an open contest, a beacon pause, operator lateness) fails for ever. | `ledger/escrow_v2.py:458-472`; `protocol/envelope_v2.py:347-348` |
| F7 (CL-18) | `EscrowV2.locked()` scans every `LOCK_ADMISSION` event and runs one unindexed `SUM` per lock, under the store lock, for the unauthenticated `GET /v2/runs/{run}/admission/{hotkey}` and for round open, accept and finalize. Measured on the real code: 1.08 s at 5,000 open locks, 5.4 s at 10,000, 96 s at 40,000 [V]; the beacon freshness gate pauses every v2 operation after 6 s. Every event also re-serialises the whole state in pure Python (94% of an event), so a restart `verify()` of 5,000 events takes 106 s. | `ledger/escrow_v2.py:291-315,1073-1086,1098-1164,1166-1179` |
| TRN-02 | The relay refuses every upload after 64 grants and `/readyz` turns false: `accept` and `ready()` count every record ever accepted, only a master-signed `replaces` deletes one, `release()` needs a `settlement` callback that `runtime()` never builds, and every miner is given the same relay. | `relay/core.py:144,275,779`; `relay/app.py:336-349`; `challenge/store.py:8458` |
| F6 (CL-14, SHD-D3) | Shadow maturity runs inside `push_beacon`'s per-run transaction. One origin whose custody check raises something other than three tolerated codes aborts the maturity of every origin of the run, the lease expiry and the notifications; the pusher exits on the 409. | `challenge/store.py:727-754` |
| ESC-02 | `escrow/lock|transfer|release` have no gate and no rate limit; each event holds the store lock and writes an O(state) snapshot. | `challenge/app.py:577-602` |

## Decisions

### Part 1. Server events and origin ids live in a keyed, disjoint space (F1, D51 3, D52 I2-I4)

1. **Id key.** The master derives one id key from the coordinator secret,
   `escrow_id_key = hmac(coordinator secret, "hypertrain/escrow-id/2")`, with the construction
   `_prefix_secret_v2` already uses [V `store.py:2536`]. Every id the service derives from public
   values is the HMAC of its preimage under that key: operation ids (reward finalize, shadow
   finalize, maturity, weights payout, dispute settlement) and origin ids (reward, shadow, genesis,
   bootstrap). The preimages are today's strings, which already carry the type tag, the run, the
   round and the subject (D52: `HMAC(secret, (run, round, type, subject))`). **There is no unkeyed
   mode (D53).** A ledger built without a key is a read-only verifier: every server operation
   (`test_genesis`, `production_bootstrap`, `reward_finalize`, `mature`, `slash`, `pay`,
   `release_settlement`) refuses with `ID_KEY_REQUIRED`, so no run can silently keep predictable ids;
   the unit-test helper passes a fixed test key, the store always passes the coordinator key, and a
   test builds the production store and asserts the key reaches the ledger. The unkeyed derivation
   survives only to **verify** origin rows that were issued before the key existed (point 6).
2. **Continuity pin (I3).** `ChallengeStore` stores `sha256(id key)` in `meta`
   (`escrow-id-namespace-v2`) at first use and refuses with 503 `escrow id namespace differs` when
   the secret later yields another key, exactly as `_prefix_secret_v2` does with
   `admission-prefix-namespace-v2`. A changed coordinator secret therefore fails closed. Rotation is
   not in-place: the coordinator key is pinned in the manifest, so a new key is a new run; re-keying
   the stored rows would be an explicit migration, which this ADR does not provide. A chain written
   before the key existed (unkeyed ids) is read through the legacy fallback of point 6.
3. **Key space.** Every event written by a server-only path is stored under `"srv:" + id`, where `id`
   is the keyed id of point 1. A client id is a `Hex64` (`^[0-9a-f]{64}$`), so it can never start with
   `srv:`: the two key spaces are disjoint **by construction** (the guarantee), and a third party
   cannot compute a server id from public inputs (the depth D51 asks for). Client events (`lock`,
   `transfer`, `release`) stay exactly as they are: bare `Hex64` key, same `UNIQUE` column. No consumer
   of a client lock id changes: `release`, `slash`, `locked()`, the dispute lookups, `admission.py:1100`
   and the `JOIN` of `verified_inputs_v2` (`store.py:7788`, hashed) keep reading bare ids.
4. **The server API is separate by type (I4).** `lock`, `transfer` and `release` are the client
   operations and can only write the client key space. The server operations are `test_genesis`,
   `production_bootstrap`, `reward_finalize`, `mature`, `slash`, `pay` and a new
   `release_settlement` (the service's release of a lock, called by `DisputesV2.settle_lock`;
   `release` and `release_settlement` share one private implementation that takes the key space as a
   mandatory argument). There is no default and no flag a caller can forget. `Admission.release_bond`
   and `Admission.seize_bond` of ADR 0010 Part 6 must call `release_settlement` and `slash`; a test
   walks the abstract syntax tree of the whole source and fails if any call site of a client
   operation is outside the client route (`ChallengeStore.admission_v2`, `AdmissionStore.reconcile_lock`)
   or if a server operation is reached from a request handler, and a second test runs every public
   mutating method of the ledger and asserts the key space it wrote.
5. `EscrowReceipt.operation_id` and every request body keep the id the caller named; the stored key
   and the event-hash preimage of **new server events** carry the prefixed keyed key. No persisted
   byte is rewritten; no event hash, receipt or snapshot of an existing chain changes; no run id,
   golden or vector changes (no test or document pins an escrow event hash [V grep]).
6. **Legacy rows.** A chain written before this change holds its server events under bare ids.
   `_existing` for a server key falls back to the bare id **with an identical request** (a squatter's
   different request is ignored, a replayed server event returns its stored receipt), so `verify()`
   still opens a ledger that holds a finalized shadow reward and a chain that was **already squatted
   heals**: the server writes under its own key [V on a base-code chain, `f1db` notes]. Origin rows
   already issued keep their ids; only new origins are keyed.
7. **What `verify()` guarantees without the secret.** The read-only authority snapshot
   (`scripts/network_authority_snapshot.py`) has no coordinator secret. Its `verify()` recomputes every
   event hash from the stored key and checks conservation; for a server event it checks the **shape**
   (`srv:` + 64 hex) and that the key space matches the kind, not the binding between the key and the
   receipt's id, which needs the secret. So without the secret `verify()` proves disjointness and the
   chain, not the keyed binding; a ledger with the key proves both. Shadow events are found by their
   request, not by a derived id.
8. Hardening in the same lot, not part of the guarantee: `EscrowV2.transfer` refuses
   `recipient == owner` (`SELF_TRANSFER`, after the replay lookup, so old self-transfers replay); the
   quota of Part 5 meters new client events.
9. **Origin ids are not learnable (I2).** With point 1 an origin id is no longer a function of public
   values, so it can be neither enumerated nor pre-computed from a finalize hash. It is readable by the
   holder (the miner receives its origins from the operator that signs the reward, as today: the CLI takes
   `--origin`) and by the admin; no unauthenticated route returns an origin id [V grep of `challenge/app.py`
   and the store's responses: origin ids appear only in the admin finalize result and in records]. Typed
   references (Part 2) and the disjoint space remove what a learned id could be used for; keying is the
   depth D51 3 requires. `finalize_v2` builds the reward's origin list itself (`store.py:8354`), so this
   edit changes `implementation_hash` (see Consequences).
10. **Residual, stated plainly (ESC-D5).** A client can still occupy another client's id, because the
   client space stays global: the miner CLI derives its admission lock id from public values
   (`miner/coldkey_ops.py:139-149`), a third party that signs any event under it makes the victim's
   identical retry fail `OPERATION_CONFLICT`. It is per victim and recoverable (another unit count or
   origin list changes the id), cannot block a server operation, and costs the squatter a funded key plus
   a quota slot. Closing it needs owner-scoped client keys (four consumers, a hashed function, the outbox
   key); it is **not** done here.

Rejected: a `scope` column with `UNIQUE(scope, operation_id)` (needs a rebuild of two tables that SQLite
cannot `ALTER`, an old image then fails with `OperationalError`, per-owner lock ids re-open settlement
aliasing and change `verified_inputs_v2`); a composite client key (changes the hash and every lock
lookup); ids derived from `(owner, nonce)` (a wire change of `EscrowOperation` and of the deployed CLI);
refusing client ids that equal a derivable server id (the set is unbounded); making server operations
tolerate `OPERATION_CONFLICT` (the id is the idempotency key and the occupant cannot be removed); a rate
limit alone (one unit funds unlimited squats); a `server=False` default keyword on `release` (forgettable;
replaced by two functions).

### Part 2. Typed settlement references (F4)

1. A reference is typed by the call site or by the kind of the lock event, never by guessing from
   the string. `DisputesV2.settlement(dispute_id)` becomes a primary-key lookup; the new
   `DisputesV2.contest_settlement(lock)` finds the dispute through the lock's own signed
   `dispute_id` and requires `turn.lock_id == lock.operation_id`; `settle_lock` asks
   `settlement(turn.dispute_id)`; `ChallengeStore._settlement_v2` matches a dispute by id only
   and a reward origin by `records_v2`.
2. `EscrowV2.release`, `release_settlement` and `slash` ask `_lock_status(lock_id)`, which picks the
   resolver by the kind of the lock event (`LOCK_CONTEST` -> `contest_settlement`, fail closed
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
the name of its own lock, but no other key space ever consults that name, and the ids the service
derives (dispute ids, origin ids, operation ids) are keyed or hash-derived and not client-choosable.
Rejected: bond ids derived by the server (`sha256(["ht-contest-bond/2", run, dispute_id])`): a
public function in the shared client space is squattable like ESC-D5 and changes nine test literals
and every hand-built client.

### Part 3. Reward evidence is judged at its acceptance beacon (M2, D48)

Rule (D48): an envelope that was valid at its acceptance stays valid for the finalisation of its
round; `finalize` never redoes a temporal check on a commitment it already accepted. M2 is fixed in
**this** lot, in the same zone (`ledger/escrow_v2.py`) as F1, so that `v3-audit-integrity` (ADR 0011
owns M1, the lease queue, the DSP-01 exit and the single rule for a round closed without a reward)
does not duplicate it. Merge order: F1 + F4 + M2 (this lot) before `v3-audit-integrity`.

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
   coordinator's challenge. `created` is a **lower bound** of the sealing beacon (it is `d_audit` on the
   base, `now` after ADR 0011's re-arm), never later than it, so an envelope valid at `created` was valid
   when sealed; a test pins that `created <= the beacon at which the challenge was sealed` on the real
   scheduling path, so a future change that seals earlier than it records fails. The helper is a new
   method beside `_settlement_v2`, wired in `_services`; `finalize_v2` is **not** edited for M2.
3. Not done, and why: bounding `exp_drand` at intake (hardening only: there is no single right
   minimum, honest clients seal `d_final`, `d_upload`, `lease_expires` and `deadline_beacon`, and it
   edits `training_v2`/`complete_audit_v2`, both hashed and edited by ADR 0011); sealing the challenge
   beyond `d_final + vesting` (not needed once evidence is judged at acceptance).
4. The operator tool seals the Finalize envelope with `exp = d_final` (`scripts/ops/live_operator.py`),
   so a finalize command after `d_final` is refused by the **Finalize** envelope itself even with the
   ledger fixed. The tool seals it with `max(d_final, now) + 1000` (the margin the trial Finalize
   already uses). The Finalize is a fresh request, not stored evidence, and keeps its check.
5. The mandatory test (D48): a round finalized at `d_final + N` after realistic expiries (commit
   `d_final`, verdict `lease_expires`, challenge `absolute`) settles and a repeat returns the stored
   result; a ledger test with `exp` below the finalize beacon and above the acceptance beacon settles;
   an envelope that expired **before** its acceptance beacon is still refused. "After a contest" is
   exercised through the existing MATCH-verdict contest closed by `referee_v2`; ADR 0011's DSP-01
   closure adds the assertion after both lots merge. What a round closed without a reward does to the
   next finalize is ADR 0011's rule and is not tested here.

### Part 4. Third-party growth of ledger state (F7)

F7a (liveness, conditional blocker):

1. `locked()` is rewritten on one statement over a **partial index** created with `CREATE INDEX IF
   NOT EXISTS escrow_open_admission_locks ON escrow_units(owner, ref) WHERE bucket='admission_locked'
   AND units>0`. Its cost is the number of **open** locks of the owner, not the ledger. It returns
   the same `(units, hash)` bit for bit (checked on real 1,000- and 2,000-lock chains; the committed
   test is a randomised differential against the legacy statement plus a literal golden value on a
   deterministic fixture), because the value feeds `lock_receipt_hash` in the signed tape. The index is
   derived data kept by SQLite in the same transaction, an older image on a database that has it is
   harmless, and `network_authority_snapshot` builds it in memory [V]. No side table or running
   aggregate: a missed writer would make `locked()` silently wrong and it decides `eligible`. A
   general `(owner, bucket, ref)` index must not be added later without re-checking the plan (a test
   pins `EXPLAIN QUERY PLAN`).
2. Caps on open `LOCK_ADMISSION` locks, enforced in `EscrowV2.lock` after the replay lookup (a coldkey
   at a cap still gets its old receipt back; the miner CLI and the protected-opening fixture re-post
   saved locks) and pre-checked in `Admission.lock` so no outbox row is left that `reconcile_lock` can
   never apply. They are sized from the real roster, not from the pending queue (D52 m1, D53 2): the
   roster ceiling is 1,024 miners [V `store.py` `max_roster`], a coldkey that owns a whole roster needs
   one lock and one top-up per miner, so **2,048 per coldkey** (`OPEN_LOCK_CAP`) and **4,096 in the
   ledger** (`LEDGER_LOCK_CAP`, twice the honest worst case; a lever of capital, see below). The per-coldkey cap bounds the cost of
   `locked()` (about 4 µs per open lock, 8 ms at the cap [V]); the ledger cap bounds the state that many
   funded coldkeys can create, because "40,006 unreachable locks" holds per coldkey only. **A minimum
   amount per lock** of `max(1, S_min_units // 10)` (100 units in the example policy, HTTP 409
   `LOCK_BELOW_MINIMUM`) makes the ledger cap costly to exhaust: filling it needs 4,096 locks of at
   least a tenth of the funding floor each, held by funded coldkeys, instead of 4,096 dust locks. The
   floor is a code constant derived from the policy value, not a policy field. The ledger cap remains a
   liveness lever for funded accounts: two coldkeys that hold 409,600 units can fill it with 4,096 locks of
   100 units, and those locks are not releasable until ADR 0010 Part 6 lands. It is a capital cost (the
   attacker's units are locked for as long as the locks stay), not a free denial; if Forge prefers no such
   lever the ledger cap is dropped and only the per-coldkey cap stays (Decisions for Forge 3). Contest locks are not
   capped (33 contests by one owner is a pinned case).
3. `GET /admission/{hotkey}` stays public and gets cheap, not authenticated or cached: four
   in-repo clients (the miner, the coldkey CLI, the operator tool, the proof script) call it unsigned
   and ADR 0010's `admission_owner()` relies on admission ids being public. After the index it costs
   2.4 ms at 100 locks and 0.17 s at 40,006 locks of one coldkey [V], and at most a few ms at the cap. A flood of cheap GETs is the
   generic ingress gap (no limiter exists), not F7a.

F7b (IMPORTANT):

4. A local encoder in `escrow_v2.py` replaces the pure-Python `canonicalize` in `_snapshot`,
   `_event` and `verify()`: `json.dumps(value, separators=(",", ":"), ensure_ascii=False,
   allow_nan=False)` is byte-identical to JCS for lists of strings, integers within ±(2^53−1) and
   `None` (all 5,006 stored event hashes of a real chain recompute identically; 3,000 Hypothesis
   examples agree [V]) and 21× faster for events and `verify()`; a range guard keeps the errors
   `canonicalize` raised. `protocol/jcs.py` is not touched.
5. `transfer` skips a zero credit; `lock`, `release` and `slash` do **not**: a lock that lists two
   origins leaves a zero-unit row for the second, and `settle_lock` rebuilds the release from the
   original origin list, which `_move` requires a row for [V reproduced]. `SELF_TRANSFER` (Part 1.8).
6. **Not done: a digest in the event-hash preimage with periodic full checkpoints** (storage and
   `verify()` become linear, prototype 3,000 locks in 7.6 s against 92 s). It changes the hash format
   of all new events and makes the chain forward-only (an older image refuses to start); the escrow
   chain format is documented nowhere. It needs maintainer approval; see "Decisions for Forge". Row
   deltas are not small and are rejected.

### Part 5. Escrow ingress (ESC-02)

1. New client events (never a replay, never a service-driven call) are metered by the quota that
   `join` already uses: identity `escrow:<owner>:<now // work_screen_epoch_rounds>`, rate 0, burst
   **2,048** per coldkey and window, i.e. twice the roster ceiling (D53 2: burst at least twice the
   expected miners; a coldkey that admits 16 miners with a lock and a top-up each needs 32, and the
   first sizes, 4 then 16, would have throttled an honest operator). It is charged after the signature is verified, before any
   ledger work, in the same transaction (a refusal consumes neither the token nor the replay nonce).
   Refusal `ESCROW_QUOTA` (HTTP 409). The burst is a module constant, not a policy field. The cleanup
   uses a key-range predicate that can use the primary-key index (a default `LIKE` cannot). With the
   ledger-wide cap of Part 4.2 and this per-coldkey bucket, the events a window can add are bounded
   per coldkey and in total.
2. A transfer must go to a recipient the ledger already knows (`UNKNOWN_RECIPIENT`): the quota alone
   is bypassed by hops to fresh keys (79 signers accepted once each, reproduced over HTTP [V]). No
   in-repo flow transfers to an unknown wallet. This is a product rule; see "Decisions for Forge".

### Part 6. Relay grants (TRN-02, D51 5, D53 1)

The rule (D53 1): an unattended run must never block on the relay journal, and nothing in the path
needs an operator. Four mechanisms, each in its own commit, in this order.

1. **The quota counts held uploads and slides (A).** An upload is held while it is a reservation
   (service deadline not passed, no receipt) or custody (pins neither released nor lapsed). Receipt and
   deadline release a reservation; a lapsed horizon stops counting custody. The count cap is optional
   (`RelayConfig.max_uploads`); the default is none, because the physical limit is the journal. `ready()`
   no longer depends on the count: the exit path (release, extension, retrievals, chunk PUTs) goes through
   the same Service, so a quota that turns `/readyz` false removes the means of clearing it.
2. **The journal budget is a guard, an alarm and a size (A).** The journal is one JSON blob of at most
   16 MiB rewritten whole on every mutation. `accept` refuses with `relay journal capacity exhausted`
   before a mutation would take it above a 12 MiB budget (4 MiB headroom for chunk acks, receipts,
   retrievals, extensions and releases). `/readyz` and a log line report the fill ratio and raise an
   **alarm above 80 %** (9.6 MiB); the runbook says what to do (below). **Sizing, measured [V]:** a record
   is about 4.2 KB fixed plus 0.234 KB per roster entry plus 1.1 KB per chunk plus 1.0 KB for the receipt,
   so about 10 KB at 16 miners and 13 KB at 32 miners with one-chunk deltas; a round is 335 beacons, about
   16.75 minutes; custody lives to its horizon, about 10 hours or 36 rounds. **With the rotation of point 4
   (records leave at their horizon) the steady state is 16 miners × 36 rounds × 10 KB = 5.8 MB, 46 % of the
   12 MiB budget; at 32 miners it is 15 MB, 119 %, so a run of 32 miners needs the early release of point 3.** A
   2 GiB delta (512 chunks) is about 580 KB per record.
3. **Release is automatic (B, D).** The relay learns what the master settled from a public, read-only,
   coordinator-signed `RelaySettlementView` (B) and feeds the existing `settlement` callback; no admin
   credential enters the relay. **What the view exposes (D51 3):** the grant hash, the finality hash, the
   finality beacon, the closed dispute root, the closed beacon and the vesting beacon; no hotkey, no
   coldkey, no origin id, no roster. The finality hash was an input of the origin-id derivation; origin
   ids are keyed (Part 1.9), so publishing it no longer helps to compute one. The master then releases
   without an operator (D): a periodic task of the master lists the grants that have a durable receipt and
   no `relay-release` record and calls the existing release path (`relay_control_v2`, which already
   refuses before vesting, dispute closure and the epoch grace) for each that is due. A grant is thus
   released a round or two after its finality instead of at its horizon, and the steady state at 32 miners
   is about three rounds of records (1.2 MB). `POST .../admin/relay/{grant}/release` stays for the operator.
4. **Rotation by age, and pruning (C), never under an open dispute (D54).** `collect()` runs from a
   periodic task in the relay. A pin whose horizon is past by the epoch grace (100 beacons) with no
   extension is **lapsed**: the signed promise has ended (a retrieval is only valid until `min(requested +
   10, horizon)` and an extension only before the horizon), and the object is deleted and the record pruned
   like a released one, with no release. **No custody is destroyed or rotated while a dispute references it:**
   a pin whose last extension names dispute ids (`pin.disputes` not empty) does not lapse; its expiry is
   suspended and resumes when the master releases the grant at the dispute's close (which clears the list).
   The master keeps such custody alive: its lifecycle task (point 3) **extends** a grant whose round has an
   unresolved dispute before the horizon, repeatedly, until the dispute closes by resolution or by the general
   exclusion exit of ADR 0011; this ADR does not define that exit. A round that closes without a reward never
   settles, so its grants are never released; age frees them once no dispute references them. Spent records are
   pruned only when no live record or in-flight publication needs the same object. This changes the earlier
   rule "bytes stay pinned until a release": after the signed horizon plus the grace, and with no dispute
   referencing it, the relay owes nothing. Test: a dispute open at the horizon keeps the custody; the release at
   its close deletes it.
5. **Runbook (docs).** Alarm: `journal_fill > 0.8` on `/readyz` or the log line. First check that the
   master's release task runs (`relay-release` records appear one or two rounds after finality). If it
   does and the fill still rises, the run is larger than the journal: add a relay to the signed registry
   (required before any run of 16 miners or more, D51 5). Do not raise the budget above 12 MiB (the
   16 MiB wall bricks every mutation, including release).
6. Rejected: renewal of the 64 grants (a new wire message that refreshes a number with no physical
   meaning); spreading assignments (production has one relay [A], and `assignment_policy` is a literal
   pinned in nine places); embedding the settlement fields in `CustodyRelease` (smaller, but removes the
   cross-check against the master's current record); keeping lapsed bytes pinned for ever (it is what makes
   an unattended relay fill up). Not done: a closure for a dispute nobody can decide (ADR 0011's exit).

### Part 7. Shadow maturity is failure-isolated (F6, CL-14)

`push_beacon` isolates each origin: the three "waiting" codes stay silent, ledger conservation and
`sqlite3.Error` stay fatal, anything else is journaled as `records_v2 kind='maturity-failed'` (id =
origin, rewritten only on change and without a beacon field, because `_rollback_snapshot_v2` hashes
every record) and the loop goes on. Each `mature` already runs under a savepoint. The failure is
not silent (the record is the alarm) and retries every round. After Part 1, `OPERATION_CONFLICT` on a
server id can no longer come from a client; it is isolated and journaled, not tolerated.

### Part 8. No signature in HTTP headers (D51 2, D52 I1)

The master proxy is the only entry path and forwards only `content-type`, `accept` and
`authorization` [A: `src/cortex/challenges/proxy.py:14`]. **Decided, not asked:** no route authenticates by a
signature carried in an HTTP header. Whatever must be signed is signed in the **body**, as a signed ht/2
envelope with an expiry that the server bounds. Four header signatures exist; all four leave:
the dispute watch (`X-Dispute-Signature`, SIG-01/DSP-09), the trial job (`X-Trial-Signature`), the object
upload (`X-Object-Signature`, `PUT /v2/runs/{run}/objects/{sha}`) and the challenge GET that ADR 0010
decision 3 adds. This lot converts the first three. The fourth is owned by `v3-admission-hardening`
(RTA-04), whose code is not in this base. ADR 0010 decision 3 and its open proxy question (0010 lines
121, 137-145 and 373) are **amended by their own lot, on instruction**: the challenge request is a `POST`
whose body is a signed envelope, not a `GET` with two headers. This ADR keeps only the rule and this
cross-reference. The dispute watch also gets a per-party and a total bound on concurrent long polls, on its own
thread pool. Other IMPORTANT items follow in their own commits and are recorded in the report with their sibling
status: HTP-07 (the observer challenge is handed back until answered; the pin id carries the round), ADM-04
(no store lock on the event loop before the signature check; the anonymous relay-receipt route too),
ADM-06, ESC-05, TRN-03, TRN-05. ADM-03 belongs to `v3-executor-perf` (ADR 0012 Part B); ADM-02 waits for
`v3-audit-integrity` because it edits `lease_v2`.

## Consequences

- `implementation_hash`: it **changes**, once, because of Part 1.9 (the reward's origin list is built in
  `finalize_v2`, one of the 28). Everything else in Parts 1 to 7 stays outside the 28 and the 10 files. A
  stored `service-admission` record then fails `SERVICE_PROFILE_AUTHORITY` until the run is re-bootstrapped,
  the same consequence ADR 0011 states for its own edits of hashed functions. Measured with
  `ChallengeStore._service_implementation_v2`: `02e208a3…89cc` on the base and after the key space and the
  typed settlement; `20e0333c…53ac` after the keyed origin ids; later commits that touch one of the 28 will
  say so.
- Schema: no table, column or key changes. One partial index is created if absent.
- Compatibility: new server events carry a new key and new hashes, new origins new ids; old chains verify
  unchanged and are forward-only for the first new server event (an older image then fails closed with an
  `EscrowError` and cannot write: take a database backup before the digest moves, as `deploy/README.md` and
  `docs/OPERATOR_SECURITY.md` already say). HTTP gains `OPEN_LOCK_CAP`, `LEDGER_LOCK_CAP`, `SELF_TRANSFER`,
  `ESCROW_QUOTA`, `UNKNOWN_RECIPIENT`, `CONTEST_LOCK_ID_COLLISION` (all 409) and 429 on the dispute watch;
  the watch, trial job and object upload change from header-signed requests to body envelopes. The relay
  gains one message type and one public GET, a periodic collect task and a fill report on `/readyz`; the
  master gains a periodic release task; an old relay still fails `release` closed.
- `sources()` of `network_qualification` changes like for any source edit; stored qualification
  configs validate against their own frozen tree.
- A chain that already holds an early release of an open dispute's bond (the F4 evasion happened)
  stays wedged after the fix (`settle_lock` fails `UNFUNDED_OR_UNKNOWN_ORIGIN`) [V]. No such chain is
  known.

## Decisions for Forge

1. **ESC-D5.** Accept the residual of Part 1.10, or approve owner-scoped client keys (a bigger change in
   four consumers, a hashed function and the outbox key) and a CLI lock id derived with a secret.
2. **F7b structural.** Approve (or refuse) the digest-in-preimage scheme with 1,000-event checkpoints.
3. **Caps and quota.** 2,048 open admission locks per coldkey, 4,096 in the ledger, a minimum lock of a
   tenth of the funding floor, escrow burst 2,048 per window; the `UNKNOWN_RECIPIENT` rule.
4. **Early-release chain** (Context): is a repair needed for a chain that already released a bond?

Decided by the Principal SN100 (D52, D53) and applied above, not asked: no header signatures, origin ids not
enumerable, continuity pin and fail-closed, the server key space enforced by type, no unkeyed mode, one rule
for `FINALIZE_ORDER` in ADR 0011, caps with a minimum amount and a burst sized from the roster, an automatic
release with an alarm and a runbook, the journal estimate.

## Not verified

No run on the production master or behind the real Cortex proxy; real SQLite 3.40 (the image's) was
not available, plans were checked on 3.46 and 3.53; the F7 numbers are CPU/SQLite cost on tmpfs
(no `fsync`); production roster, round cadence and relay count; the gateway's behaviour when all relay
pods are unready; a threaded reproduction of DSP-03 (lot `v3-audit-integrity`); shadow reward delay
at store level. The contest-then-finalize chain with the DSP-01 closure needs that lot.
