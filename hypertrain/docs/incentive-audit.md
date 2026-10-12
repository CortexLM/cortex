# Incentive audit (WS-D)

Scope: emission, escrow and vesting, burns, honest rewards, and penalties, checked against [mechanism.md](mechanism.md). Line numbers refer to the tree on 2026-10-12. Only `ledger/` was changed. `challenge/store.py` and `challenge/app.py` are frozen, so gaps in those files are documented here and not fixed.

| Mechanism | Implemented? | Evidence | Gap | Severity | Status |
|---|---|---|---|---|---|
| Reward formula `contrib = tokens*(800000 + clamp(score)/5)`, floor share of `B_w` | Yes | `ledger/ledger.py` `_finalize` (~L178-200) | None. Integer math; FAULT, NO_UPLOAD, and blacklisted slices burn and are not redistributed. | - | OK |
| Honest rewards: MATCH and UNSAMPLED granted, debt repaid first | Yes | `ledger/ledger.py` `_grant` (~L202) | None. | - | OK |
| Vesting `E = ceil(1/q)` on the f32 value of q | Partial | `protocol/messages.py:200` uses the f32 value. Before this fix, `ledger/ledger.py` `vest_rounds_for_q` used the decimal text. | The two disagreed whenever f32(q) < q, for example q=0.04 (25 vs 26) and q=0.01 (100 vs 101). The ledger's default E could therefore be below the floor the manifest validator enforces, which breaks the deterrence inequality `q(S+R) >= G`. | Medium | **Fixed**: the ledger now rounds q to f32 first. Covered by a test in `tests/ledger/test_ledger.py`. |
| Release: oldest first, cap of `full_share_mass` per epoch, excess carried over | Yes | `ledger/ledger.py` `_Engine.compute` | None. | - | OK |
| Burn accounting via `full_share_mass - W` (UID0) | Yes | `ledger/ledger.py` `FULL_SHARE_MASS`, metadata `units_burned_this_epoch`; invariant `minted = burned + paid + pending` | None at the ledger level. | - | OK |
| Deregistered hotkeys at answer time | Partial | `challenge/app.py:851` ponytail | Dropped units are booked as paid and are effectively burned to UID0. They are not kept as owed. A miner that briefly deregisters loses those vested units. | Low | Documented (app.py is out of scope) |
| Multi-run emission share | Partial | `challenge/store.py:313` ponytail | Each run gets an equal share regardless of size, and there is no cross-run cap of 65,536 hotkeys. | Low | Documented (store.py is frozen) |
| Unresolved audit at `d_final` | Yes | `challenge/store.py:1756` maps it to NO_UPLOAD; ledger `vindicate`/`fault` (`store.py:1306,1324`) | The ponytail comment at store.py:1756 is stale: the vindication credit event now exists (`Ledger.vindicate`). | Info | Documented |
| FAULT: burn escrow and round slice, then blacklist | Yes | `ledger/ledger.py` `_apply` verdict branch, `_fault`; `auditor/replay.py:872` `forfeit_for` | `forfeit_for` is called with 0/0 units (`store.py:1601`) and is used only for the `cause` label. The units actually burned come from `Ledger.burned_for_fault`. The Forfeit message units are informational only. | Info | Documented |
| TRANSIENT: forgiven `forgive_per_epoch` times, then FAULT | Yes | `ledger/ledger.py` `_apply` (`transients`) | The forgiveness only stops the escalation. A forgiven TRANSIENT still burns its round slice, because `_finalize` grants only MATCH and UNSAMPLED. This matches mechanism.md section 5 ("burns only that round's reward"). | - | OK |
| Clawback: escrow first, then debt repaid from future rewards | Yes | `ledger/ledger.py` `clawback` branch, `_grant` | No production caller of `Ledger.clawback` was found in `src/`. The mechanism exists but is not wired to any route. | Low | Documented |
| Post-finality fault freezes payouts | Yes | `ledger/ledger.py` `_Engine.frozen` | None. | - | OK |
| Dispute slashing (v2 escrow) | Yes | `challenge/disputes_v2.py:863` `settle_lock`; `ledger/escrow_v2.py:882` `slash` | FRAUD loser: the lock is burned, after the referee cost is paid, which is capped by `max_referee_cost`. An admission-lock slash also burns `reward_pending`. Otherwise the lock is released. Only the internal authority can slash. | - | OK |
| v2 vesting gate (disputed origins stay pending) | Yes | `ledger/escrow_v2.py:794` `mature` | None. | - | OK |

## Fix applied

`vest_rounds_for_q` (`src/hypertrain/ledger/ledger.py`) now computes `E` on `f32(q)`, as mechanism.md section 5 and `protocol/messages.vesting_rounds` specify. The values for q = 0.5, 0.1, 0.05, 0.3, and 1 are unchanged. The default `HYPERTRAIN_VEST_ROUNDS` (q=0.1) is still 10.

## Follow-ups (outside the write scope)

- `app.py`: keep units of deregistered hotkeys owed instead of burning them (ledger-level freeze).
- `store.py`: give each run an emission weight and add a merged hotkey cap. Also remove the stale ponytail comment at L1756.
- Wire `Ledger.clawback` to an operator route, or remove it if it is not needed.
