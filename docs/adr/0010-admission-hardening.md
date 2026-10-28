# ADR 0010: admission hardening (Lens red team D36)

- Status: draft for branch `v3-admission-hardening`, 2026-10-27 (Principal SN100 D36 and D36 bis).
  Forge decides promotion to `v3`. Nothing here changes a production run, master, profile,
  emission, pin or digest. Part 6 (the admission bond) follows the recommended option; the
  questions that remain are in "Decisions for Forge". Amended 2026-10-28 (branch
  `v3-miner-driver-resilience`, RELAY-03): decision 2 gains point 5, the operator's reseed.
- Order: brief `v3-admission-hardening.md`. Findings come from the Lens red team A
  (`livrables/lens/redteam-live/rtA.md`, RTA-01 to RTA-06) and B (`rtB.md`, EB-01), tree
  `65be1962`. This ADR is written before any code, as the brief requires.
- Labels: **[V]** verified in code, by a test or by a run on the base tree `33b50656`
  (2026-10-27); **[A]** reported by a cited document and not re-read; **[H]** hypothesis;
  **[?]** unknown. Paths are relative to `hypertrain/src/hypertrain/` unless prefixed.
- Rules of the lot: no protocol constant changes. A manifest-covered change is an optional
  policy field that keeps today's behaviour when absent (the pattern of
  `EconomicsPolicyV2.bootstrap_allocation_hash`), or it is challenge-side behaviour outside
  `implementation_hash`. One fix, one commit; each Lens reproducer becomes a regression test
  that is red before and green after, with a mutant it kills.

## Context

All six admission findings reproduce on the base tree [V, Lens `rt_admission_a.py` and
`rt_admission_b.py`: 10 passed in 14.7 s, every `exploit_*` succeeds; `rtB_elastic_tokens_leave.py`:
13 passed]. The RTA-01 and RTA-04 exploits were also run through the HTTP routes, which the Lens
had not done [V]: `POST .../admission/{hot}/rotate` answered 200 and bound the attacker's row to
a registered hotkey that never joined; an unsigned `GET .../join/{id}/challenge` answered 200
with the victim's signed challenge, and a second one after the deadline suspended the victim.

| Id | Defect | Where |
| --- | --- | --- |
| RTA-01 | `RotateRequest` is signed by the coldkey alone: one coldkey burns any registered hotkey that never joined (`HOTKEY_HISTORY_EXISTS` for ever) and chains rotations without limit | `protocol/messages_v2.py:584`, `challenge/admission.py:964-1000` |
| RTA-02 | `join` takes no stake and an `APPLIED` row never expires: 32 unfunded joins hold all `max_pending` places and their coldkeys | `admission.py:158-171`, `:236-240` |
| RTA-03 | A trial that is never proved stays `OPEN` until someone requests that admission's challenge again; two of them block every trial of the run (`TRIAL_CAPACITY`) | `admission.py:216-246` |
| RTA-04 | `GET .../challenge` checks only that the target hotkey is registered; any caller opens a rival's trial and, after the deadline, suspends the rival | `challenge/app.py:472-477` |
| RTA-05 | The same lazy branch suspends a miner whose proof was accepted but whose finalization has not run | `admission.py:222-228` (ignores `proof_digest`) |
| RTA-06 | The admission bond can neither be released nor slashed | `challenge/store.py:2318-2333`, `admission.py:1093` |
| EB-01 | An elastic token log restored from a copy, or copied or hard-linked, re-issues a token, so an old signed leave releases a new lease | `elastic/tokens.py`, `elastic/coordinator.py:285-304` |

`implementation_hash` is the hash of the AST of 35 named definitions of `challenge/store.py` (34
functions and the class `_ServiceCapacityV1`) and of ten files (`challenge/store.py:4345-4394`). `admission.py`, `admission_store.py`, `app.py`,
`elastic/*`, `protocol/messages_v2.py`, `protocol/envelope_v2.py`, `miner/*` and the dispatcher
`admission_v2`, `_settlement_v2`, `_services`, `push_beacon` are not in it [V]. Every decision
below stays outside it.

## Decisions

### 1. Rotation needs the new hotkey's consent (RTA-01)

1. `RotateRequest` gains a required field `new_hot_sig` (128 hex). The new hotkey signs
   `hypertrain/rotate/2|RotateRequest|<sha256 of JCS{coldkey,hotkey,new_hotkey,operation_id}>|<exp_drand>|<run_id>`
   (`envelope_v2._signing_message` with its own domain, as `join` does with `hypertrain/join/2|`).
   `Admission.rotate` verifies it after the coldkey's envelope and refuses with
   `ROTATION_CONSENT`. The signed data covers the run and the expiry (no cross-run or late
   replay), the operation id, both hotkeys and the coldkey (the hotkey agrees to be bound under
   that coldkey, as `hot_sig` of `join` does). The domain is distinct from every other.
2. The durable replay digest of `rotate|<operation_id>` excludes `new_hot_sig`, because sr25519
   signatures are randomized [V]: a retry with a fresh signature stays idempotent. It equals the
   digest the same rotation had before this change.
3. A `BANNED` row cannot rotate (`TERMINAL_BANNED`, as `suspend` already answers). Rotation of a
   `SUSPENDED` row with a pending dispute stays legal: the carry-over tests require it.
4. Quota. A new rotation, never a replay, is charged to the row `rotate:<coldkey>:<now //
   work_screen_epoch_rounds>` of `admission_quota` with rate 0 and burst `join_burst`, and the
   refusal is `ROTATION_QUOTA`. Both numbers come from the policy that exists (4 per 6000
   rounds in production, per 1000 in the test fixture). A window aligned on the epoch allows up to
   twice the burst across a boundary, as the IP bucket does. No new constant.

Consequences. A wire change to a message that is not in any manifest: an out-of-tree client that
signs the three-field body gets 409 `invalid RotateRequest body`; none exists in this repository
[V grep]. No schema file or golden vector pins `RotateRequest` [V]. The squatting of a
registered hotkey is closed because `admission_history` gets rows from `join` (both keys) and
`rotate` (now both keys) only [V].

Rejected: an optional signature (an attacker omits it); a separate `RotateAccept` envelope (a
second message, a route that reads two bodies, no extra guarantee); verifying inside the generic
`Intake` (couples it to an admission rule); an opt-in policy field for the quota (a new constant
and existing runs stay exposed); a token bucket with refill (`join_per_beacon` = 2 per round would
refill two rotations every three seconds).

### 2. Trials expire on the clock and never suspend (RTA-05, RTA-03)

1. A trial is **proven** when `admissions_v2.proof_digest` is set for its epoch. The lazy branch
   of `Admission.challenge` and the reaper never expire a proven trial. Finalization stays
   possible after the deadline: `trial_finality` and `finalize_trial` check neither the deadline
   nor the outcome, and the coordinator's `Finalize` expires at `max(now, deadline) + 1000` [V].
2. An exact replay of the already accepted proof and screen is authenticated at their own
   expiry round and answered with the stored record, after the deadline too. Without it a
   proven trial whose finalization failed (for example `TRIAL_FINALIZATION_CAPACITY`, the proof
   route commits the proof and the finalization separately) could never be finalized by the
   miner, and the change of 1 would turn a suspension into a trial that holds a place for ever.
   `reference_due` accepts the same replay. A fresh proof after the deadline is still refused.
3. `Admission.reap_expired_trials(now)` sets `outcome='EXPIRED'` on every `OPEN` trial that has
   no proof and whose `deadline_beacon < now`. It suspends nobody, adds no strike and does not
   move `seed_beacon`. It runs on every beacon push (`ChallengeStore.push_beacon`, in its own
   transaction after maturation and lease expiry) and at the start of `Admission.challenge`, in
   the transaction that counts `TRIAL_CAPACITY`. It ignores the admission's state, so it also
   frees the places held by suspended, banned and deregistered squatters that no request can
   reach. The strict `<` keeps the deadline round open, because `proof` accepts `now == deadline`.
4. The lazy suspension `CHALLENGE_EXPIRED` is removed. A request after the closure answers 409
   `CHALLENGE_EXPIRED` and changes nothing, until the operator reseeds the admission (point 5).
5. The operator's reseed (2026-10-28, RELAY-03; decided by Forge as the smallest change, option
   ADMIN). The s100 TEST replay stranded a PROBATION miner this way: its driver died on a read
   miss, came back 11 minutes (about 220 rounds) after its trial's deadline, and every request
   answered 409 `CHALLENGE_EXPIRED` while the identity kept its state, with no route to a new seed
   [A, brief `v3-miner-driver-resilience.md`, replay of 2026-10-27, run `fea7e44f`; the refusal is
   reproduced on the real `Admission` by the tests of this point]. `Admission.reseed(admission_id, now)`, behind
   `POST /v2/runs/{run_id}/admin/join/{admission_id}/reseed` (admin bearer only, also
   `live_operator.py reseed --admission-id`), writes `seed_beacon = now + 1` and nothing else, so
   the expiry rule is unchanged for every miner and the honest-versus-late distinction is the
   operator's, not the code's. It is refused with `RESEED_STATE` unless the state is `APPLIED`,
   `PROBATION` or `ACTIVE` with no pending dispute, with `RESEED_TRIAL_OPEN` while the admission's
   trial is `OPEN` after the reaper ran (live, or proven and waiting for its finalization), and
   with `RESEED_NOT_NEEDED` unless `now > seed_beacon + challenge_rounds`: one lapse buys one
   reseed, and a healthy identity cannot re-draw its trial. State, `clean_count`, strikes, screen
   and the closed trial are untouched, so there is no second identity and no strike, and each use
   leaves a reservation row (`reseed|<admission>|<previous seed>`) with the receipt. The miner
   still asks with its own signed `ChallengeRequest`; the next round answers `SEED_NOT_READY`.
   No constant, policy field, message type or run id changes, and `implementation_hash` does not
   (`challenge/admission.py` is outside its paths). It is not the signed re-seed of ADR 0002 P1,
   which removes the operator and needs a budget: that stays a proposal for founder decisions 5
   and 6 of that ADR, and this route is the TEST-stack and operations stopgap until then.

Why not suspend. The suspension has no route back (`resume` is reachable only from
`resolve_pending`) [V], so a reaper that suspends turns the first catch-up beacon push after a
pause into a mass suspension of honest miners: the S13 loss (`docs/s100-test-stack.md:536-544`)
in a batch, and the D40 case, a trial across a pause, which ADR 0011 leaves outside its part 5.
ADR 0002 already says that suspending on a lapse
is rejected and that the lazy branch must close a trial past its deadline without suspending
(`docs/adr/0002-eviction-tolerant-core.md` P1 bullet and "Rejected for P1"). This ADR applies
that to every run, not only P1 runs. A squatter gets one trial per `seed_beacon`, because
`challenge()` refuses after `seed + challenge_rounds` and only `join`, `finalize_trial` and
`resume` write the seed [V], so a closed trial cannot be taken again and a campaign is bounded
by the pending places and, with decision 4, by their expiry.

Consequences. The documented v2 rule "a challenge left unproven past its deadline suspends the
miner" (`hypertrain/docs/network-v2.md`) changes for every run: the miner keeps its state, its
`clean_count` and its screen, and is stranded the way an idle miner already is until the operator
reseeds it (point 5) or the signed re-seed of ADR 0002 P1 exists. The dead end predates this
change; only its penalty goes.
Residual: two fresh identities timed to a victim's join can still hold both places for one
window, because `max_trial_replays` is `Literal[2]`; a funded-only place would need a new
policy field and is left to Forge.

### 3. The owner asks for its trial and reads its job in a signed body (RTA-04)

Decided (Principal SN100 D51 to D54): the master proxy is the only entry path of a miner. It
forwards `content-type`, `accept` and `authorization` and nothing else
(`src/cortex/challenges/proxy.py:14`, `docs/CHALLENGES.md`), so **no signature travels in an HTTP
header**. A miner request whose authority is a hotkey's signature carries that signature as a
signed `ht/2` envelope in the body of the request, checked by `envelope_v2.Intake` (run id, signer,
signature, expiry). This part applies the rule to the two routes of the trial that only its owner
may call: the challenge request and the job read. One helper serves both.

1. `POST /v2/runs/{run_id}/join/{admission_id}/challenge` replaces the unsigned `GET` (now 405).
   The body is the envelope of the new message `ChallengeRequest{admission_id}`, signed by the
   admission's current hotkey. Order of refusals: unknown run 404, unknown admission 409, a
   deregistered or revoked owner 403, then any body that is not an unexpired `ChallengeRequest`
   of this run, for this admission, signed by that hotkey (empty, not JSON, another message, run,
   admission or signer, a bad signature, an expired envelope, an expiry beyond the bound of point
   3) 403 `hotkey is not current accepted identity`. The admin bearer is not a credential.
2. `POST /v2/runs/{run_id}/join/{admission_id}/job` replaces `GET` with the headers `X-Hotkey` and
   `X-Trial-Signature` (now 405). The body is the envelope of the new message
   `JobRequest{admission_id}`, signed by the admission's current hotkey, with the same order of
   refusals. A distinct message type, so that a captured challenge request does not read a job
   and a captured job request does not open a trial.
3. The expiry is the signer's choice, so the service bounds it: an envelope that expires more
   than `OWNER_REQUEST_MAX_ROUNDS` = 100 rounds (five minutes at 3 s per round) after the
   service's current round is refused like any other forgery. The constant is a route constant of
   `challenge/app.py`, outside `implementation_hash`; no manifest field carries a request window
   and a new one would change the run id. The miner signs 20 rounds ahead
   (`OWNER_REQUEST_ROUNDS`, a client-side choice, a minute), from the round the service reports.
4. The check lives in the routes, not in `Admission.challenge`, which stays the trusted
   in-process API ("not routes"). The regression is therefore an HTTP test, which also closes
   the Lens caveat that the HTTP path was not run.
5. The admin export (`export_trial_v2`) returns the stored coordinator-signed envelope as
   `challenge_envelope`, and `challenge/trial_export.py` stops calling the route. The miner
   (`NetworkMiner._fetch_trial`) signs both requests. Scripts and tests sign through
   `Network.challenge` and `Network.owner_request`.

Consequences. The envelope's expiry, bounded by the service, limits a replay of a capture; there
is no nonce store. Both requests are idempotent (asking again returns the stored challenge or
job), and with decision 2 an asking never suspends anyone. A third party who captures the owner's
request on the wire (TLS is the protection) can ask for the trial or read the job only until the
envelope expires, at most five minutes, and learns nothing new. Rejected: a signature in a header,
which the proxy drops; a single-use nonce (a lost response locks the miner out, and it adds a
write per request); enforcing inside `Admission.challenge` (22 direct callers, a conflict with
decision 2, and a caller identity the service cannot verify); accepting the admin bearer
(contradicts `network-v2.md`).

The same rule covers the two other routes that sign in a header and that are not part of
admission. `PUT /v2/runs/{run_id}/objects/{sha256}` (`X-Hotkey` and `X-Object-Signature`,
`miner/core.py:1042`) was assigned to the lot `escrow-relay-fixes` and is not converted in this
tree: it does not work through the proxy until that lot lands. `GET /v2/runs/{run_id}/disputes`
(`x-dispute-signature`) was assigned to the lot `audit-integrity-2` and is converted there: it
is now `POST /v2/runs/{run_id}/disputes` with a `WatchRequest` signed in the body (the `GET` answers
405), the same pattern as the challenge and job requests of this series.

### 4. Dead unfunded joins release their places; an optional per-prefix cap (RTA-02)

1. An `APPLIED` row is dead once `now > seed_beacon + challenge_rounds`: `challenge()` then
   refuses for ever, and `seed_beacon` is only written by `join` (`now + 1`), `finalize_trial`
   and `resume` [V]. The time to live is therefore the challenge window, derived from a policy
   value that exists. No constant is added.
2. At the start of every `join` (inside its transaction, after the idempotent replay, before the
   history, coldkey, capacity and quota checks), the expired trials are closed (decision 2.3),
   then each dead `APPLIED` row with **no locked unit** and no pending lock in the outbox
   becomes `SUSPENDED` with reason `APPLIED_EXPIRED`, as ADR 0002 names it. The row, its
   transition log and its spent hotkey stay. A funded row is never touched: its release belongs
   to Part 6. The reaper cannot live in `challenge()` or `status()`: a raising operation rolls
   its transaction back, and a GET must not write.
3. `join` ignores `SUSPENDED / APPLIED_EXPIRED` rows when it applies the one-non-`ACTIVE`-row-
   per-coldkey rule, so an honest miner whose application lapsed can apply again with a new
   hotkey. Every other non-`ACTIVE` row still blocks.
4. Optional policy field `max_unfunded_per_prefix` (integer 1 to 32), absent by default and
   omitted from the digest when absent. When set, `join` refuses a new row with
   `PREFIX_PENDING_CAPACITY` if the /24 (IPv4) or /64 (IPv6) already holds that many unfunded
   `APPLIED` rows. The prefix is stored as an HMAC tag (key `ip_secret`, not bound to the epoch,
   so it survives the 6000-round quota epoch) in a new table `admission_origin`, created with
   `CREATE TABLE IF NOT EXISTS` (no `ALTER`, no new column in `admissions_v2`, whose `SELECT *`
   feeds the rollback revision). Only unfunded `APPLIED` rows count, so an honest operator who
   funds right after joining is not counted and a prefix cannot be pinned for good. Recommended
   first value when used: `join_burst` = 4.

Why the cap is opt-in. The master proxy is the only entry path, so the challenge container sees
the address of the proxy unless it trusts the forwarded header: every client then shares one
prefix and a derived cap becomes a global cap that four sybil joins fill [A:
`src/cortex/challenges/proxy.py:51` sets `x-forwarded-for` but nothing makes `uvicorn` trust it;
the image runs plain `uvicorn`]. Enabling it needs real client addresses first (RTA-10).
Hash consequence of setting it: a new policy digest, hence a new `admission_policy_hash` and a
new run id, by design. An image without the field refuses a policy that carries it
(`extra='forbid'`), so the challenge image ships first.

Rejected: `dormant_rounds = work_screen_epoch_rounds` (ADR 0002 P1, a new constant, 5 hours
instead of 5 minutes of squatting, meant for a re-seed that does not exist in v2); deleting the
row or a new `AdmissionState` (ADR 0002 rejects both); a column on `admissions_v2` (no migration
precedent in hypertrain); counting `PROBATION` rows (they cost a real reference replay and are
not reaped).

### 5. The coordinator epoch is bound into the signed leave (EB-01)

1. `Membership` draws a fresh 32-byte `epoch` (64 hex) at construction, once per coordinator
   start, and never reads it from or writes it to the token log. Every `Lease` carries it.
   `LeaveNotice` gains the signed field `epoch: Hex64`, and `Coordinator.leave` refuses any
   other epoch with `LeaseError('STALE_EPOCH')` before the signature, the retry answers or any
   lease is looked at.
2. This deviates from the brief's wording ("stored with the log") on purpose. A restore brings
   back whatever is stored: a counter stored with the log collides after a second restore
   (backup at 1, start 2 signs under 2, restore, start 3 reuses 2), and a stored random value is
   redundant once a fresh one is drawn at every start. A mark-derived epoch is killed by a
   mutant (`f"{mark:064x}"`). The epoch is also independent of the log format, so it protects the
   database-row log of ADR 0003 too, and `tokens.py` (a docstring only) and the 29 mutants anchored
   on it do not change.
3. A copy or hard link of the log still gets its own lock, so two coordinators can run and issue
   the same tokens. Each has its own epoch, so a leave signed under one releases no lease of the
   other. Not covered: a restore of the whole process (a VM snapshot brings back the epoch with
   the mark) [H], the fencing of heartbeats and commits (the wiring authenticates them), and the
   single-writer guarantee, which is the lock's and, in production, the database row's.
   A future restore call must draw a new epoch and never persist one.

Consequences. `LeaveNotice` gains a required field (version and domain unchanged: no leave has
been persisted or exchanged outside tests and the simulator [V grep]). Nothing outside `elastic/`
imports the package, `implementation_hash` and every manifest hash are unchanged, the import
surface pinned by `tests/elastic/test_imports.py` is unchanged. The name is `epoch`, not
`incarnation`, because the simulator already uses `incarnation` for a host-slot restart.
`scripts/check_elastic_mutants.py` goes from 165 to 170 mutants.

### 6. The admission bond (RTA-06)

Nothing in this part changes a manifest, a policy digest, a wire message, an economic constant or
`implementation_hash`: it gives the two ledger operations that exist (`EscrowV2.release`,
`EscrowV2.slash`) the settlement they never had. The order for it is Principal SN100 D36 bis
("wire the fraud settlement and the bond release"); the seizure of a no-show's bond is Forge's
request of 2026-10-27 for the audit-integrity lot (AUD-06b), which that lot did not take: see the
note after point 6.

Context [V]. The bond is an `EscrowLock(kind=LOCK_ADMISSION)` signed by the coldkey; eligibility
needs locked units >= max(`S_min_units`, `verify.s_min_units(R_collectible_units)`, a
`G_max / (s_lower * beta)` bound) (`admission.py` `status`). `release` and `slash` read
`settlement(lock_id)`, and `_settlement_v2` answers only for dispute and contest-lock ids and for
the reward-origin rows `finalize_v2` writes; an admission lock got `UNACCEPTED_SETTLEMENT`.
`Admission.settle_fraud` had no caller and could not succeed (it asked `settlement` about an
admission lock). A referee `FRAUD` against the miner slashed the contest lock only
(`disputes_v2.py` `settle_lock`) and left the miner `SUSPENDED` with `pending_dispute` for ever.
An uncontested `MISMATCH` writes `records_v2 excluded UNVERIFIED` and nothing else. The production
gate counts `s_lower * beta * (R + S)` as collectible on detection (`challenge/trust_v2.py`): with
the bond unslashable that premise was false. Two more defects of the same mechanism, reproduced:
the settlement callback is one namespace for lock, dispute and reward-origin ids, so a lock
whose `operation_id` equals a dispute id was released as soon as that dispute closed without
fraud; and the operation id of a settlement is predictable
(`sha256(JCS([dispute_id, "settle"]))`), so the owner can register it first with a self-transfer
and turn the slash into `OPERATION_CONFLICT`. The second also freezes
`DisputesV2.settle_lock` for a contest, and with it the referee's resolution transaction: a
finding for the audit-integrity lot, which this part does not fix there.

Decision.

1. **Typed resolver.** `EscrowV2.admission_settlement` (a callback, `None` keeps the single
   one). `release` and `slash` choose the resolver by the kind of the lock event: a
   `LOCK_ADMISSION` uses it, every other kind and `mature` keep `settlement`. `Admission`
   installs `lock_settlement` when it is built. The lock kind closes both aliasing directions;
   `_settlement_v2` is untouched.
2. **Derived, never stored.** `Admission.lock_settlement(lock_id)` is computed from persisted
   rows on every call. `FRAUD` once the lock has a row in `admission_seizures` (decision 3).
   Otherwise `MATCH`, unresolved until: the admission is not `BANNED`, has no `pending_dispute`,
   no unresolved `admission_pending` row and no lock in the outbox; every round whose roster
   lists it is closed (`RoundStatus.closed`); none of its hotkeys has `reward_pending` units;
   and `release_beacon` is reached, the last `mature_at` of its reward origins and, while it is
   live (`ACTIVE`, or `PROBATION` with four clean trials), the end of its current screen plus
   one: a live miner keeps its bond, which is what makes it eligible. `finality_hash` and
   `closed_dispute_root` are hashes of what was read (rounds and origins, dispute ids), so the
   owner signs the state it saw. No new constant is added: the four clean trials of the live phase
   are the literal that `status()` already uses.
3. **Seizure.** `Admission.seize_bond(admission_id, *, reason, evidence_hash, now)` is an
   internal, authenticated service hook (the coordinator is the ledger's reward authority); no
   route calls it. It records each residual lock in `admission_seizures`, which makes its
   derived settlement `FRAUD` for good (never releasable), and burns it through `EscrowV2.slash`
   under an operation id that is an HMAC of the lock id with the coordinator-private
   `ip_secret`: deterministic for retries, unguessable for a squatter. A second call finds
   nothing left. The admission is resolved by id, never by hotkey, which a rotation changes.
   Reasons: `TRAINING_FRAUD` also burns the owner's unvested rewards (the ledger's rule for a
   slashed admission lock, so only while the admission still holds a residual lock: after an
   earlier bond-only seizure a conviction bans and burns no reward, see Decisions for Forge 4)
   and bans the admission (`BANNED`, terminal; `Admission.lock` then answers `LOCK_BANNED`); `AUDIT_MISMATCH`, a round-stage audit mismatch that the caller has
   confirmed as the miner's (D47), suspends the admission (reason `AUDIT_MISMATCH`), adds one
   strike and burns the bond alone (`EscrowV2.slash(burn_pending=False)`), and the coldkey stays
   bound by the one-non-`ACTIVE`-row rule as in probation; `NO_SHOW` burns the bond and adds a
   strike too, but the first one puts the admission back on a fresh probation and only a second
   one inside `NO_SHOW_STRIKE_WINDOW_SCREENS` work screens suspends it, as ADR 0011 Part 8
   decision 12 (D63) amends it (it first burned the bond and left the admission as it was,
   ineligible until funded again). A (reason, evidence)
   pair is applied once, recorded in `admission_sanctions`: a retry adds no strike. The ledger
   keeps its existing meaning of a slash; the new keyword defaults to it, so no earlier event
   changes. The caller decides whose fault it is: a verdict against the auditor reaches
   `settle_fraud`, which returns `False` for it, and the closures of ADR 0011
   (`_close_blocked_disputes_v2`) call `settle_lock` and `resolve_pending`, never `settle_fraud`
   or `seize_bond`.

   **Strike window of a no-show (D63, D70).** The window is `no_show_strike_window(epoch) =
   NO_SHOW_STRIKE_WINDOW_SCREENS (4) * work_screen_epoch_rounds`, measured in beacon rounds from the
   beacon of the earlier sanction. It has to outlast two full PROBATION -> ACTIVE ladders, or a
   miner that comes back as fast as it can would always meet a closed window and the suspension
   tier would be dead. Inputs: a beacon round is 3 s (20 per minute); the ladder is
   `clean_finalizations` = 12 clean trials (the policy field is a literal); the TEST replay ran
   twelve clean trials in about 29 minutes [A: D70]; a production training round takes about
   10 minutes [A: D70]; `work_screen_epoch_rounds` is 6000 in the TEST and production builders
   (`build_test_manifest.sh`, `ops/manifest_v2.py`, `od_large_manifest.py`, `network_gpu_seed.py`).
   Formula: `ladder = trials * seconds per trial / 3 s`, `window = 4 * 6000`, and the rule is
   `window >= 2 * ladder`.

   | Policy | Ladder | Two ladders | Window | Margin |
   | --- | --- | --- | --- | --- |
   | TEST | 29 min = 1740 s = 580 rounds | 1160 rounds = 58 min | 24000 rounds = 20 h | 20.7x |
   | production | 12 * 10 min = 120 min = 2400 rounds | 4800 rounds = 4 h | 24000 rounds = 20 h | 5x |

   Four screens is therefore not the minimum (one screen, 6000 rounds, would pass too); it keeps a
   miner that rests between trials inside the window. `tests/challenge/test_strike_window_v2.py`
   asserts `window >= 2 * ladder` for both policies from the builders' own literals, and pins the
   numbers above. A policy with a shorter screen must restate the calculation.
4. **Conviction.** `Admission.settle_fraud(dispute_id, *, now)` replaces the old function (no
   caller, no test). The `resolution` branch of `admission_v2` calls it in the transaction of
   `DisputesV2.settle_lock`. Only an accepted, resolved dispute settlement with outcome `FRAUD`
   (reason `FRAUD`, loser == the miner of the contest) convicts, and only if the pending
   reference bound when the dispute opened names the same admission, coldkey and evidence and the
   verdict's release beacon is reached. It returns `False` and changes nothing when there is no
   pending reference, the dispute is unresolved, or the outcome is not `FRAUD` against the miner
   (a verdict against the auditor). A `FRAUD` verdict that does not match the pending reference,
   or whose release beacon is not reached, raises `UNPROVEN_TRAINING_FRAUD`.
   Conviction calls `seize_bond(reason="TRAINING_FRAUD")`. Because it shares the transaction, a
   failure of the hook rolls back the resolution (fail closed).
5. **Release.** The owner signs the existing `EscrowRelease`; the route is unchanged.
   `GET .../admission/{hotkey}` gains a response-only `bond` key listing the residual locks with
   `releasable`, `release_beacon` and the `finality_hash` and `closed_dispute_root` to sign.
   `Admission.release_bond(admission_id, *, now)` is the internal equivalent for an authenticated
   request of the owner: all residual locks or none (`BOND_NOT_RELEASABLE`).
6. **Not sanctioned.** An uncontested `MISMATCH`, an `INFRASTRUCTURE` close (ADR 0011),
   `PARTY_TIMEOUT`, `WITHHELD`, `AUDITOR_FAULT` and `FRAUD` against the auditor leave the bond
   intact. ADR 0011 refuses a conviction on the raw comparator while DET-01 and DET-01b can
   `MISMATCH` an honest miner, and no manifest value gives a contest window.

For other lots (audit-integrity, AUD-06b and D47). At the merge with `origin/v3` only
`TRAINING_FRAUD` has a caller: ADR 0011 part 5 gives a roster member that never commits the
sanction S0 ("no admission, strike, bond or ledger effect"), and no round-stage mismatch is
confirmed as the miner's. If Forge still wants a no-show's bond burned, the call belongs in
`_put_no_show_v2`, in the transaction that excludes the member. The API:
`Admission.seize_bond(admission_id, *,
reason, evidence_hash, now)` with `reason="NO_SHOW"` for a roster member that never committed and
`reason="AUDIT_MISMATCH"` for a confirmed round-stage mismatch; `evidence_hash` is 64 hex naming
the round and what confirmed it; call it from a step that retries, not inside the transaction
that excludes the member (ADR 0011 Part 8 decision 4: a sanction that raises would keep the
member in the round). To give a bond back on the owner's authenticated request,
`Admission.release_bond(admission_id, now=...)`. Both resolve everything by admission id, nest in
`escrow.tx()`, are idempotent and write nothing when there is no residual bond.

Consequences. An honest miner leaves with its bond after its screen lapsed, its last round
closed and its rewards vested; the wait is `E_vest_rounds` beacon rounds after the last
finalization unless a dispute is open. The bound `q * s_lower * beta * (R + S) >= G` becomes true
for contested detections (a no-show keeps its bond, see the note after point 6); a cheater who
never contests is excluded and keeps
its bond until the Forge question on a contest window is answered. Contesting now risks the
whole admission bond (and, while a residual lock exists, the coldkey's unvested rewards) and a
ban instead of the contest bond:
with DET-01b open an honest, falsely `MISMATCH`ed miner should not contest (the cure is DET-01b,
not this part). A round that never closes pins the bonds of its roster. ADR 0011 closes the
cases it names (an undecidable dispute, a silent auditor, a member that never commits) and
leaves open the survivors that cannot carry the round and a round whose audit window was
missed. Other `ACTIVE` admissions of a convicted coldkey keep their state but lose their
unvested rewards to the ledger's burn. An admission-trial `MISMATCH` binds a pending reference
that nothing settles, so that bond stays held (left open below). A round that lists the
admission but has no finality record (which `open_round_v2` always writes with it) is treated as
open: the bond is held, fail closed.

Rejected: a persisted `settlement` row per lock (stale, allows release while rostered, needs a
writer in `finalize_v2` or `complete_audit_v2`, which are in `implementation_hash`); a service
sweep that releases for the owner (frees honest miners that are merely suspended, adds a money
step to the beacon route); slashing an uncontested `MISMATCH` after a window (no window exists,
single-operator replay, DET-01b); a slash fraction or a referee share (a new economic constant;
the whole lock is the only share valid for every `s_lower <= 1`); banning every admission of the
coldkey; a retirement state on release (a live miner cannot release at all, so the grief of
withdrawing the bond just before a round opens does not arise); hooks in `complete_audit_v2`,
`finalize_v2` or `referee_v2` (they change `implementation_hash`).

## Hash consequences

Measured with `ChallengeStore._service_implementation_v2`, `network_qualification.sources()` and
`bundle()`, and `manifest_v2 example` followed by `manifest_v2 build`, on the merge commit
`7a4c0a87`, where the lot meets v3. The `33b50656` column (the base of the lot) is the first
measure of this ADR; the same code reproduces it for `sources()`, `bundle` and `challenge/store.py`. The
`e0969448` column (`origin/v3` when the lot was merged into it) isolates what the other lots of
v3 changed from what this one changes: its `sources()` map, bundle and store hash are rebuilt
from the git blobs of that revision with the same file list and tar header, after that code had
reproduced the `33b50656` values and the real values of the merged tree. A row marked
"unchanged" for `e0969448` was not measured on that tree: the files that produce it are the
same as in the merged tree (`git diff e0969448 7a4c0a87` over `ops/` is empty and over
`protocol/messages_v2.py` only adds the two request classes).

| Value | Before (`33b50656`) | v3 (`e0969448`) | Merged (`7a4c0a87`) |
| --- | --- | --- | --- |
| `implementation_hash` | `02e208a3…0189cc` | `941247f8…a688d1` | unchanged from v3 |
| `challenge/store.py` sha256 | `b5bc9904…aaec88` | `6eadf5bb…1e3972` | unchanged from v3 |
| `AdmissionPolicyV2` digest of the `manifest_v2 example` (test and production) | `c3fd6d4d…b3e3e3` | unchanged | unchanged |
| `manifest_v2 example` + `build` run id, test / production | `87b08906…952485` / `54ee4c87…44f263` | unchanged | unchanged |
| pure-JSON wrapper run id (`tests/protocol/test_network_v2.py`) | `4efbc85d…8cc43f97` | unchanged | unchanged (pinned by a test) |
| `sources()` map, canonical sha256 | 151 files, `e0df98e6…507e8b` | 153 files, `2932236d…85bb79` | 153 files, `0d989cd1…987bf9` |
| `bundle` sha256 | 2,396,160 bytes, `e78f33d1…70ba` | 2,508,800 bytes, `11da44ef…3c9902` | 2,519,040 bytes, `0fbf1c09…280721` |

What the lot alone changes on top of v3: the `sources()` map and the bundle (`admission.py`,
`app.py`, `miner/core.py`, `elastic/*`, `protocol/{messages,envelope}_v2.py` and the scripts that
sign the owner requests; +10,240 bytes). It changes no other row: the eleven inputs of
`implementation_hash` are the same files as on v3 (the store edits of the lot, `settle_fraud` in
the resolution branch and `bond` in the status, are in v3 already, as the older copies merged by
the audit-integrity lot). The difference of `implementation_hash` between `33b50656` and
`e0969448` comes from that lot, which added five definitions to the hashed set
(`_exclude_missing_deltas_v2`, `_exclude_no_shows_v2`, `_put_no_show_v2`, `_require_survivors_v2`,
`_round_ids_v2`). The first version of this table gave `54ee4c87…e263` for the production run id;
the digest ends in `44f263` and the value did not change. The figure `0fba21a9…cb7863` for
`challenge/store.py` in that version described the lot's tree before the merge and no longer
exists.

- No run id, `admission_policy_hash`, golden root or `reference_hash` changes for a policy that
  does not set `max_unfunded_per_prefix`: the field is omitted from the canonical body when
  absent, and tests pin the digest of the example policy and a pure-JSON run id, with a mutant
  that drops `exclude_if`. A run that sets it has a new digest and a new run id, by design.
- `sources()` hashes every `src/hypertrain/**/*.py`, so this change, like any source change,
  invalidates older qualification configs against a tree at this commit
  (`qualification_source_or_profile_changed`), the dispatch source pins
  (`gpu_ops/role_dispatch.py:112`) and reviewed bundles, exactly as ADR 0007 states. A config
  still validates against its own frozen tree; installed qualification records stay valid.
- The server image gets a new digest at rebuild. A manifest that enables the optional cap pins
  nothing else.
- Wire: `RotateRequest` gains `new_hot_sig`; `LeaveNotice` gains `epoch`; two new message types
  `ChallengeRequest` and `JobRequest`; the export payload gains `challenge_envelope`; the unsigned
  `GET .../challenge` and the header-signed `GET .../job` become `POST` with a signed body;
  `GET .../admission/{hotkey}` gains `bond` in Part 6. None is a manifest or SCALE wire message;
  `docs/BUNDLE_SPEC.md` and the frozen vectors are untouched.

## Decisions for Forge

1. **Bond burn (Part 6).** Implemented as recommended: a referee `FRAUD` against the miner burns
   the admission bond and, while a residual lock exists, the coldkey's unvested rewards, and bans
   the admission; a no-show seizure burns the bond and adds a strike, and puts the admission on a
   fresh probation the first time and suspends it the second time inside four work screens (ADR 0011
   Part 8 decision 12, D63; the alternative was bond-only). ADR 0002 "Penalties" makes any slash for a failed audit
   founder-only: the order of D36 bis and Forge's request of 2026-10-27 are taken as that
   decision for these two cases. To withhold the burn, `seize_bond` keeps recording the seizure
   and the ban and only the `slash` call goes.
2. **Uncontested MISMATCH.** The brief says "a miner convicted by a MISMATCH audit is slashed".
   Part 6 convicts only through a contest and a referee verdict. Convicting an uncontested
   `MISMATCH` needs a window W that no manifest field provides (`E_vest_rounds` is 10 beacon
   rounds in the example, `dispute_per_level` is a per-turn timeout), and should wait for the
   DET-01b fix. Give W, or confirm that it waits.
3. **Residuals left open:** targeted trial squatting needs a funded-only trial place (policy
   field); the route back for a stranded miner is ADR 0002 P1 / ADR 0011 D40; an admission-trial
   `MISMATCH` binds a pending reference that nothing settles (the bond and the coldkey stay
   locked); `PROBATION` rows are not reaped; `DisputesV2.settle_lock` derives its operation id
   from public data and can be squatted (audit-integrity lot), as can any server-derived
   operation id that is not an HMAC, since the escrow operation ids share one namespace that any
   holder of one unit can write (it predates this series; ADR 0014 Parts 1 and 2 close it: the
   service's ids live in their own `srv:` key space). Found by the independent review and
   not changed here:
   - a lock of one unit counts as funding, so a dead `APPLIED` row that holds it is neither
     reaped nor counted by the optional cap; counting only locks of at least `S_min_units`
     (the manifest's own value, 1000 in the example) would close it, and changes what "a funded
     row is never touched" means;
   - a proven trial whose holder is later suspended or banned is not reaped (`proof_digest` is
     set) and keeps one of the two places until `resume`;
   - the `rotate:` quota rows are never purged, one per coldkey and window in which it rotated;
   - an IPv4-mapped IPv6 address falls into one `/64` prefix for the join quota and the cap
     (`join_v2` does not unmap it; it predates this series);
   - the status route scans the run's round records once per call for an admission that holds a
     bond, inside the store's write transaction (about 30 ms at 1000 rounds of 32, 150 ms at 4000
     in the review's measure); an admission-to-round table or a rate limit at the proxy removes
     it.
4. **Unvested rewards after an earlier seizure.** The burn of the unvested rewards is part of
   slashing an admission lock, so a conviction that arrives after a `NO_SHOW` or `AUDIT_MISMATCH`
   seizure has emptied the lock bans the admission and burns no reward. That makes the deterrent
   of contesting weaker than the text of Part 6 says, in exactly the D47 flow (mismatch, contest,
   referee verdict). Closing it needs a ledger operation that burns a coldkey's `reward_pending`
   without a lock (a new event kind under an HMAC operation id, with its conservation test),
   which this series does not add. Decide whether to add it, or to accept that the reward
   survives.

## Not verified

No CUDA run, no production manifest, no run behind the real Cortex proxy, no run of the slow e2e
files that write tens of GB (they are listed in the report). The reproducers are in-process or
through the test HTTP client, not against a deployed master.
