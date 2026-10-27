# ADR 0010: admission hardening (Lens red team D36)

- Status: draft for branch `v3-admission-hardening`, 2026-10-27 (Principal SN100 D36 and D36 bis).
  Forge decides promotion to `v3`. Nothing here changes a production run, master, profile,
  emission, pin or digest. Part 6 (the admission bond) waits for the answers in "Decisions
  for Forge" before its code is written.
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

`implementation_hash` is the hash of the AST of 28 named functions of `challenge/store.py` and of
ten files (`challenge/store.py:4279-4322`). `admission.py`, `admission_store.py`, `app.py`,
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
   `CHALLENGE_EXPIRED` and changes nothing.

Why not suspend. The suspension has no route back (`resume` is reachable only from
`resolve_pending`) [V], so a reaper that suspends turns the first catch-up beacon push after a
pause into a mass suspension of honest miners: the S13 loss (`docs/s100-test-stack.md:466-470`)
in a batch, and the D40 case that ADR 0011 owns. ADR 0002 already says that suspending on a lapse
is rejected and that the lazy branch must close a trial past its deadline without suspending
(`docs/adr/0002-eviction-tolerant-core.md` P1 bullet and "Rejected for P1"). This ADR applies
that to every run, not only P1 runs. A squatter gets one trial per `seed_beacon`, because
`challenge()` refuses after `seed + challenge_rounds` and only `join`, `finalize_trial` and
`resume` write the seed [V], so a closed trial cannot be taken again and a campaign is bounded
by the pending places and, with decision 4, by their expiry.

Consequences. The documented v2 rule "a challenge left unproven past its deadline suspends the
miner" (`hypertrain/docs/network-v2.md`) changes for every run: the miner keeps its state, its
`clean_count` and its screen, and is stranded the way an idle miner already is until a re-seed
exists (ADR 0002 P1, ADR 0011 D40). The dead end predates this change; only its penalty goes.
Residual: two fresh identities timed to a victim's join can still hold both places for one
window, because `max_trial_replays` is `Literal[2]`; a funded-only place would need a new
policy field and is left to Forge.

### 3. The challenge GET is signed by the owner (RTA-04)

1. `GET /v2/runs/{run_id}/join/{admission_id}/challenge` requires the headers `X-Hotkey` and
   `X-Trial-Signature`: the admission's current hotkey signs
   `hypertrain/trial-challenge/2|{run_id}|{admission_id}` (the pattern of the `/job` route, with
   its own domain so that a challenge signature cannot open `/job`). One helper serves both
   routes. Order of refusals, as `/job`: unknown run 404, unknown admission 409, deregistered or
   revoked owner 403, any other hotkey or a missing or bad signature 403 `hotkey is not current
   accepted identity`. The admin bearer is not a credential for this route.
2. The check lives in the route, not in `Admission.challenge`, which stays the trusted
   in-process API ("not routes"). The regression is therefore an HTTP test, which also closes
   the Lens caveat that the HTTP path was not run.
3. The admin export (`export_trial_v2`) returns the stored coordinator-signed envelope as
   `challenge_envelope`, and `challenge/trial_export.py` stops calling the route. The miner
   (`NetworkMiner._fetch_trial`) signs the GET. Scripts and tests that fetched the challenge
   unsigned sign it.

Consequences. The signature is static, like the `/job`, `PUT /objects` and `GET /disputes`
signatures: a header pair captured on the wire (TLS is the protection) replays only what the
owner may ask. With decision 2 a replay no longer suspends anyone. Rejected: a beacon-round
freshness field (a client round source, a new server constant, pause handling in five call
paths); a single-use nonce (a lost response locks the miner out); enforcing inside
`Admission.challenge` (22 direct callers, a conflict with decision 2, and a caller identity the
service cannot verify); accepting the admin bearer (contradicts `network-v2.md`). **Risk for Forge:**
the Cortex public proxy forwards only `content-type`, `accept` and `authorization`
(`src/cortex/challenges/proxy.py:14`, `docs/CHALLENGES.md`). Behind it, every signed-header
route loses its headers: this one, `/job`, `PUT /objects` and `GET /disputes` [V by reading, the
production topology is not verified]. The s100 TEST miners call the container directly. This
ADR keeps the `/job` style; moving the four routes to an `authorization` scheme is a separate,
coordinated change.

### 4. Dead unfunded joins release their places; an optional per-prefix cap (RTA-02)

1. An `APPLIED` row is dead once `now > seed_beacon + challenge_rounds`: `challenge()` then
   refuses for ever, and `seed_beacon` is only written by `join` (`now + 1`), `finalize_trial`
   and `resume` [V]. The time to live is therefore the challenge window, derived from a policy
   value that exists. No constant is added.
2. At the start of every `join` (inside its transaction, after the idempotent replay, before the
   history, coldkey, capacity and quota checks), each dead `APPLIED` row with **no locked unit**,
   no pending lock in the outbox and no `OPEN` trial becomes `SUSPENDED` with reason
   `APPLIED_EXPIRED`, as ADR 0002 names it. The row, its transition log and its spent hotkey
   stay. A funded row is never touched: its release belongs to Part 6. A row with an `OPEN`
   trial is left to decision 2.3. The reaper cannot live in `challenge()` or `status()`: a
   raising operation rolls its transaction back, and a GET must not write.
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

Why the cap is opt-in. If the challenge container sees only the address of the master proxy,
every client shares one prefix and a derived cap becomes a global cap that four sybil joins fill
[A: `src/cortex/challenges/proxy.py:51` sets `x-forwarded-for` but nothing makes `uvicorn` trust
it; the image runs plain `uvicorn`]. Enabling it needs real client addresses first (RTA-10).
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
   database-row log of ADR 0003 too, and `tokens.py` and its 35 anchored mutants do not change.
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

### 6. The admission bond (RTA-06) - proposal, final text after the answers of Forge

Nothing in this part changes a manifest, a policy digest, a wire message, an economic constant or
`implementation_hash`: it gives the two ledger operations that exist (`EscrowV2.release`,
`EscrowV2.slash`) the settlement they never had.

Context [V]. The bond is an `EscrowLock(kind=LOCK_ADMISSION)` signed by the coldkey; eligibility
needs locked units >= max(`S_min_units`, `verify.s_min_units(R_collectible_units)`, a
`G_max / (s_lower * beta)` bound) (`admission.py:913-948`). `release` and `slash` read
`settlement(lock_id)`, and `_settlement_v2` answers only for dispute and contest-lock ids and for
the reward-origin rows `finalize_v2` writes; an admission lock gets `UNACCEPTED_SETTLEMENT`.
`Admission.settle_fraud` has no caller and could not succeed (it asks `settlement` about an
admission lock). A referee `FRAUD` against the miner slashes the contest lock only
(`disputes_v2.py:863-901`) and leaves the miner `SUSPENDED` with `pending_dispute` for ever. An
uncontested `MISMATCH` writes `records_v2 excluded UNVERIFIED` and nothing else
(`store.py:7600-7606`). The production gate counts `s_lower * beta * (R + S)` as collectible on
detection (`challenge/trust_v2.py:101-155`): with the bond unslashable that premise is false.
Two further defects of the same mechanism, reproduced: the settlement callback is one namespace
for lock, dispute and reward-origin ids, so a lock whose `operation_id` equals a dispute id is
released as soon as that dispute closes without fraud; and the operation id of a settlement is
predictable (`sha256(JCS([dispute_id, "settle"]))`), so the owner can register it first with a
self-transfer and turn the slash into `OPERATION_CONFLICT` (this also freezes
`DisputesV2.settle_lock`, and with it the referee's resolution transaction: a finding for the
audit-integrity lot).

Proposal.

1. **Typed resolver.** `EscrowV2` takes an optional `admission_settlement` callback. `release`
   and `slash` pick the resolver from the kind of the lock event (`LOCK_ADMISSION` uses it, every
   other kind and `mature` keep `settlement`). `Admission` installs its own derivation. The lock
   kind closes both aliasing directions and `_settlement_v2` is untouched.
2. **Derived, never stored.** The settlement of an admission lock is computed on every call from
   persisted rows. `FRAUD` mirrors the convicting dispute settlement exactly; otherwise it is
   unresolved until the release gate of 4 holds, then `MATCH`.
3. **Slash and ban.** Only a referee `ResolutionV2` with `reason=FRAUD` and `loser ==
   contest.miner` convicts. In the same transaction as `DisputesV2.settle_lock`, the
   `resolution` branch of `admission_v2` calls `Admission.settle_fraud(dispute_id)`: it resolves
   the admission by `Contest.admission_id` (never by hotkey, which a rotation changes), checks
   the `admission_pending` linkage, burns the residual units of every `LOCK_ADMISSION` of that
   admission (`EscrowV2.slash`, which also burns the owner's `reward_pending`), marks the pending
   row resolved and sets `BANNED`. The operation id is an HMAC of the lock id under the
   coordinator-private `ip_secret`: deterministic for retries, unguessable for squatters.
   `Admission.lock` refuses a `BANNED` admission.
4. **Release.** The owner signs the existing `EscrowRelease`; the route is unchanged and
   `GET .../admission/{hotkey}` gains a response-only `bond` key with the exact `finality_hash`
   and `closed_dispute_root` to sign. The derived settlement is `MATCH` only when the admission
   is not `BANNED`, has no pending dispute and no outbox entry, is not live (`ACTIVE` or
   `PROBATION` with four clean trials and `now <= screen_until`: a live miner keeps its bond),
   every round whose roster lists it is closed, none of its disputes is unresolved, none of its
   hotkeys has `reward_pending` units, and `now >=` the largest of the `mature_at` of its reward
   origins, `finalized_beacon + verify.E_vest_rounds` of each rostered round and the resolution
   beacon of each dispute. No constant is added: every term is a policy or ledger value.
5. **Not sanctioned.** An uncontested `MISMATCH`, an `INFRASTRUCTURE` close (ADR 0011),
   `PARTY_TIMEOUT`, `WITHHELD`, `AUDITOR_FAULT` and `FRAUD` against the auditor leave the bond
   intact. ADR 0011 refuses a conviction on the raw comparator while DET-01 and DET-01b can
   `MISMATCH` an honest miner, and no manifest value gives a contest window.

Rejected: a persisted `settlement` row per lock (stale, allows release while rostered, needs a
writer in `finalize_v2` or `complete_audit_v2`, which are in `implementation_hash`); a service
sweep that releases for the owner (frees honest miners that are merely suspended, adds a money
step to the beacon route); slashing an uncontested `MISMATCH` after a window (no window exists,
single-operator replay, DET-01b); a slash fraction or a referee share (a new economic constant;
the whole lock is the only share valid for every `s_lower <= 1`); banning every admission of the
coldkey; hooks in `complete_audit_v2`, `finalize_v2` or `referee_v2` (they change
`implementation_hash`).

## Hash consequences

Baseline measured on `33b50656` with `ChallengeStore._service_implementation_v2`,
`network_qualification.sources()` and `bundle()`, and `manifest_v2 example`. The "after" column
is filled by the last commit of the lot.

| Value | Before | After |
| --- | --- | --- |
| `implementation_hash` | `02e208a3…0189cc` | to measure (expected unchanged) |
| `challenge/store.py` sha256 | `b5bc9904…aaec88` | to measure (changes: `push_beacon`, `admission_v2`, `export_trial_v2`) |
| `AdmissionPolicyV2` digest of the `manifest_v2 example` (test and production) | `c3fd6d4d…b3e3e3` | to measure (expected unchanged) |
| pure-JSON wrapper run id (`tests/protocol/test_network_v2.py`) | `4efbc85d…8cc43f97` | to measure (expected unchanged) |
| `sources()` map (151 files), canonical sha256 | `e0df98e6…507e8b` | to measure |
| `bundle` (2,396,160 bytes) sha256 | `e78f33d1…70ba` | to measure |

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
- Wire: `RotateRequest` gains `new_hot_sig`; `LeaveNotice` gains `epoch`; the export payload gains
  `challenge_envelope`; `GET .../challenge` needs two headers; `GET .../admission/{hotkey}`
  gains `bond` in Part 6. None is a manifest or SCALE wire message; `docs/BUNDLE_SPEC.md` and the
  frozen vectors are untouched.

## Decisions for Forge

1. **Bond burn (Part 6).** Option A (recommended): a referee `FRAUD` against the miner burns the
   admission bond and the coldkey's `reward_pending` and bans the admission. Option D: ban and
   freeze the bond without burning, if the founder withholds the burn (ADR 0002 "Penalties":
   any slash for a failed audit is founder-only).
2. **Uncontested MISMATCH.** The brief says "a miner convicted by a MISMATCH audit is slashed".
   Option A convicts only through a contest and a referee verdict. Convicting an uncontested
   `MISMATCH` needs a window W that no manifest field provides (`E_vest_rounds` is 10 beacon
   rounds in the example, `dispute_per_level` is a per-turn timeout), and should wait for the
   DET-01b fix. Give W, or confirm that it waits.
3. **Release.** A live miner cannot release; the rest follows the gate of Part 6.4. Confirm, or
   ask for an exit that retires the admission (`SUSPENDED / BOND_RELEASED`).
4. **Proxy headers.** Is the public miner path through the Cortex proxy (Part 3)?
5. **Residuals left open:** targeted trial squatting needs a funded-only trial place (policy
   field); the route back for a stranded miner is ADR 0002 P1 / ADR 0011 D40; an admission-trial
   `MISMATCH` binds a pending row that nothing settles (the bond and the coldkey stay locked);
   `PROBATION` rows are not reaped.

## Not verified

No CUDA run, no production manifest, no run behind the real Cortex proxy, no run of the slow e2e
files that write tens of GB (they are listed in the report). The reproducers are in-process or
through the test HTTP client, not against a deployed master.
