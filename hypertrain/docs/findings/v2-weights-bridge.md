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
- Child runs (`runs/<id>/`) pay and commit in their own database before the root commits.
  If the root then fails, its retry is not legacy (`weights_runs_from`), so it reads the
  child's stored answer and merges it again: the child's units are emitted once.

## Code path
- `GET /internal/v1/get_weights` (`src/hypertrain/challenge/app.py:859`) calls `store.weights`
  (`app.py:869`), then the metagraph filter `registered_weights` (`app.py:876`).
- `ChallengeStore.weights` (`src/hypertrain/challenge/store.py:1990`) answers from
  `self.ledger.get_weights` (`store.py:2000`), the v1 `Ledger` (`store.py:390`), merged with
  child runs that are also v1 ledgers (`store.py:2005-2007`, `_merge_answers` `store.py:319`).
- `Ledger.compute` pays only `state.entitlements` (`src/hypertrain/ledger/ledger.py:316-323`),
  which are added only by v1 `verdict`/`finalize` events (`ledger.py:203`, `ledger.py:445-450`).
  Every caller is v1: `store.py:1158`, `:1316`, `:1334`, `:1819-1820`.
- `finalize_v2` (`store.py:7964`) issues origins with `escrow.reward_finalize`
  (`store.py:8099`, `src/hypertrain/ledger/escrow_v2.py:606`), `mature_at = now + E_vest_rounds`
  (`store.py:8053`). It never touches `self.ledger`.
- `push_beacon` matures them to `available` (`store.py:681-700`, `escrow_v2.py:794`).
  `EscrowV2.pay` (`escrow_v2.py:964-970`) is a terminal sink with no caller in `src/`.

Missing link: nothing reads EscrowV2 `available`/`paid` units into a `get_weights` answer.

Search: `rg -n "get_weights|reward_finalize|\.pay\(|available" src/`, plus `escrow_v2|EscrowV2`.
Hits: `app.py:856,859`; `store.py:2000` (only weights source); `reward_finalize` at
`store.py:3851,3919` (shadow) and `:8099` (live); `ledger.py:490`. EscrowV2 users
(`admission.py`, `admission_store.py`, `disputes_v2.py`) do no weight computation.
`aggregator/`: only an unrelated "unavailable" string (`rollback_v2.py:193`).

## Test
Reuses `_graduate_four` + `_live_checkpoint` (`tests/e2e/test_service_network_v2_e2e.py`).
They finalize live round 0 via `finalize_v2` (status `SETTLED`, four origins of 250000).
The test pushes the beacon past `mature_at`, checks the origin is `available`, then calls the
route. The fixture has no internal token (`tests/challenge/test_service_network_v2.py:661`).
The test points the app `Config` at a tmp token file. Run time ~140 s, so it is marked `slow`.

Without the xfail marker:
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
