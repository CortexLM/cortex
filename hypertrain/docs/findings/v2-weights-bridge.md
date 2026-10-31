# Finding: network-v2 rewards never reach `get_weights`

Status: **bridged** (fix option 2 below). Before the fix (base `1818f9e`) a matured
contract-2 reward was never paid as weight. End-to-end proof:
`tests/challenge/test_v2_weights_bridge.py::test_v2_matured_reward_reaches_get_weights`
(slow, no longer xfail). Unit proofs: `tests/challenge/test_v2_weights_pay.py`.

## Bridge
`ChallengeStore.weights` computes the v1 answer, then `_pay_v2` fills the mass it left as
`units_burned_this_epoch` from `EscrowV2.payable(round)`. `round` is the drand round of
`epoch_at` under the run's beacon (`(epoch_at - genesis_time) // period + 1`), so the answer
never reads the wall clock. Each row is moved `available -> paid` with `EscrowV2.pay` inside
the transaction that writes `weights_runs`; the weight goes to the origin's hotkey.
- Paid: free (`available`) units of live reward origins (`escrow_finalized`) whose
  `mature_at <= round`, oldest maturity first, still held by the owner the origin was issued to.
- Never paid: `reward_pending`, `admission_locked`, `dispute_locked`, `burned`; shadow
  (bootstrap) origins and test genesis, which stay collateral (founder decision, 2026-10-19);
  units transferred to another owner or credited to a referee.
- Mass: `full_share_mass` stays 10^6 per store; weights + burned == 10^6. Units over the
  budget stay `available` and pay in later epochs. v1 entitlements are paid first.
- Idempotent: a stored epoch answer is returned before any v2 work. A failed persist rolls
  the pay back; the retry recomputes and pays once. The `weights_runs_from` meta key tells
  such a retry (the v1 journal already holds the epoch) from a genuine pre-multi-run answer.
- Deregistered hotkey: `registered_weights` (`app.py`) still drops it after the store
  answered. The escrow units are already `paid`, so they burn to UID0 at the master and are
  not owed again (the existing `ponytail`). No freeze was added.
- Hosted runs (`runs/<id>/`) are separate SQLite files. `weights` enters the answer transaction
  of the root and of every hosted store on one `ExitStack` (`_epoch_answer`): the hosted stores
  commit first, the root last, and any exception rolls all of them back. A failed call therefore
  leaves no `available -> paid` move and no stored answer in any store (Cortex does not ask that
  epoch again after a failure). Two things are not covered. The v1 journal files are appended
  before the transaction ends and a rollback does not unwrite them; the retry returns those bytes.
  A COMMIT that fails after another store committed leaves those stores holding the epoch and
  their paid units, the call fails and the root holds nothing: a repeat of the same epoch is not
  legacy (`weights_runs_from`), reads their stored answers and merges them again, so their units
  are emitted once, but only if somebody asks that epoch again. The store whose COMMIT failed
  rolls back, even when SQLite kept the transaction open (`_tx`), so it can answer that repeat.
- A hosted run whose stored manifest the model now refuses (`RunRetired`, 409
  `RUN_MANIFEST_RETIRED`, e.g. a manifest without the env pin) is left out of the escrows its store
  pays: its units stay `available`. Such a run is not expected to be served again (ADR 0011: a run
  stored before the pin was required is recreated under a new run id), so they may never pay.
  Its store still answers with its v1 ledger (nothing paid, its `full_share_mass` burned), so the
  share of the other runs does not grow, and `records_v2` kind `weights-skipped` (id = the epoch)
  holds the epoch and the reason. The other runs are answered. Any other failure to build a
  run's services still refuses the whole call, as before.

## Code path
- `GET /internal/v1/get_weights` (`src/hypertrain/challenge/app.py:864`) calls `store.weights`
  (`app.py:874`), then the metagraph filter `registered_weights` (`app.py:884`).
- `ChallengeStore.weights` (`src/hypertrain/challenge/store.py:1992`) returns a stored
  `weights_runs` answer first. Otherwise it computes the v1 answer with
  `self.ledger.get_weights` (`store.py:2029`, `src/hypertrain/ledger/ledger.py:490`) and,
  when a live v2 reward was finalized, fills it with `_pay_v2` (call `store.py:2033`,
  definition `store.py:2043`). Child runs merge through `_merge_answers` (`store.py:322`).
- `_pay_v2` reads `EscrowV2.payable` (`src/hypertrain/ledger/escrow_v2.py:973`) and moves each
  row with `EscrowV2.pay` (`escrow_v2.py:964`, called at `store.py:2077`), skipping a new
  hotkey once the answer holds `MAX_ENTRIES` (`store.py:2064`).
- `finalize_v2` (`store.py:8132`) issues live origins with `escrow.reward_finalize`
  (`store.py:8267`); `push_beacon` (`store.py:663`) matures them to `available`.
- `weights_runs_from` is set on the first v2-era call to one past the newest epoch the v1
  journal answered, so no epoch the journal served before multi-run hosting is re-paid.

Line numbers match the tree of this note's commit; re-grep `def weights`, `_pay_v2`, `def payable` after edits.

## Not addressed (relay review, one line each)
- B4: grants are idempotent per live assignment; nothing revokes one when the original arrives.
- B5: `relay-disabled` is keyed by registry epoch, so a rotation re-enables a disabled key.
- B6: the relay accepts at `now == d_upload` (`now > deadline`) while the master refuses
  `>= d_upload`; no honest relay is disabled by it.

## Test
Reuses `_graduate_four` + `_live_checkpoint` (`tests/e2e/test_service_network_v2_e2e.py`).
They finalize live round 0 via `finalize_v2` (status `SETTLED`, four origins of 250000).
The test pushes the beacon past `mature_at`, reads every `available` round-0 unit, then calls
the route and asserts the exact weight vector (four hotkeys at 250000), `units_paid` and the
burned remainder. The fixture has no internal token, so the test points the app `Config` at a
tmp token file. Run time ~150 s (measured 149 s on the host), so it is marked `slow`.

Before the bridge (base `1818f9e`, earlier `> 0` assertion, no xfail marker):
```
>       assert response.json()["weights"].get(hotkey, 0) > 0, response.text
E       AssertionError: {"challenge_slug":"hypertrain",...,"epoch":0,"full_share_mass":1000000,
"metadata":{...,"ledger_minted":0,"ledger_paid":0,"ledger_pending":0,"reason":"ok",
"round_at":0,"units_burned_this_epoch":1000000,"units_paid":0},"weights":{}}
1 failed, 3 warnings in 139.48s
```
With `xfail(strict=True)`: `1 xfailed, 3 warnings in 142.30s`.

## Fix options (option 2 implemented)
1. Mirror v2 into the v1 ledger. `finalize_v2` adds one ledger entitlement per origin
   (new `Ledger` event kind). Vesting stays in the ledger. Touches `challenge/store.py`
   (`finalize_v2`), `ledger/ledger.py`. Risk: the same vesting is kept in two places, and an
   EscrowV2 dispute or burn must also claw back the ledger.
2. Answer weights from EscrowV2. `ChallengeStore.weights` adds, for v2 runs, the units that
   matured up to `epoch_at` per origin hotkey. It moves them `available` -> `paid` with
   `EscrowV2.pay` in the same transaction, then merges via `_merge_answers`. Touches
   `challenge/store.py` (`weights`, `_merge_answers`), `ledger/escrow_v2.py` (a query by
   maturity round). Escrow stays the only source of truth.
3. Product decision: v2 units are not emissions. Document that `get_weights` pays only v1,
   and stop promising v2 rewards. Touches `docs/` only.
