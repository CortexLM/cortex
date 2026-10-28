# Ledger, escrow, finality and route-authentication invariants

Acceptance tests of the defences the challenge service must hold: a credit is issued once, an
epoch answer pays only audited and finalized rounds, a lock moves only under its owner's
signature, every state-changing route needs its holder, and a service step cannot be blocked by a
stranger. They run in process with test keys and a fake metagraph. None of them is an attack tool.

## Layout

| File | Role |
| --- | --- |
| `conftest.py`, `support.py` | the `li` hypothesis profile, the shared real reward, ledger and service builders |
| `test_<area>.py`, `test_<area>_2.py` | the tests of one area (credit, finality, audit, escrow_laws, weights, disputes, locks, admission_http, bounds, shadow, http_auth, opids, expiry, exit_disputes, exit_roster) |
| `test_gaps_<group>.py` | tests added for the consolidated Lens list of one group |
| `support_<area>.py` | helpers private to one area |
| `mutants_<area>.py` | the mutants that prove the tests of the area fail when a defence is removed |
| `mutants_closed_<finding>.py` | the same for a hole a landed fix closed (`f1`, `p2`, `m2`, `f7a`, `f6`, `rd`, `rcb` for the escrow lot, roster-contract-b and ledger-refusals; `d47`, `d59`, `d63`, `d66`, `d70`, `dsp04`, `lockrel`, `p68` for the decisions and lots that landed later): each mutant reverts one behaviour of the fix (or, where the docstring says so, models a regression the pin must survive), and every flipped pin is named by at least one of them, or the docstring says why not |
| `test_infra_guards.py` | hygiene of the lot itself: strict xfail only, fix-lot tags, no wall-clock thresholds, mutant tables that match the tree |

## Run

```bash
cd hypertrain
uv run --frozen pytest -q -m "not od" tests/ledger_invariants            # slow in one process: shard it
uv run --frozen pytest -q tests/ledger_invariants/test_infra_guards.py  # the hygiene guards
uv run --frozen python scripts/check_ledger_invariant_mutants.py --only <mutant id> --jobs 2
```

CI shards the files like the rest of the suite; the shared real reward costs about 8 s once per
shard process.

## Holes and fix lots

A defence that does not hold yet is a `pytest.mark.xfail(strict=True, raises=..., reason=...)`
that asserts the desired outcome after a control that proves the scenario was reached. The reason
starts with the invariant id and ends with `[fix-lot: <lot> ...]`, where the lot is
`escrow-relay-fixes`, `audit-integrity-2`, `admission-hardening`, `executor-perf`, `none (no design
yet)` or `decision (product choice pending)`.

When a lot lands, the strict xfails it fixes turn red (XPASS). Find them with the tag and flip them:

```bash
grep -rn "fix-lot: escrow-relay-fixes" tests/ledger_invariants
```

A test written against behaviour survives the lot; one that fails for another reason is coupled to
an internal and must be fixed before the marker is removed.

An xfail that waits for a lot not merged yet ends its reason with `[fix-lot: <lot> <ADR part or
decision>]`; `test_infra_guards.py` refuses any other lot name. A tag names a lot only while that
lot is yet to close the hole: when it lands and the test still XFAILs, retag it
(`decision (product choice pending)` or `none (no design yet)`) and say in the reason why the lot
leaves it open.

### Flipping a hole

1. Merge the lot, run `tests/ledger_invariants` (sharded) and read each `[XPASS(strict)]` failure.
2. Delete the marker and the sentence of the reason. The invariant ids of the reason must already
   be in the docstring of the test (write them there when you write the xfail, so that the flip is
   a pure deletion and the ids survive it): an id that lives only in a reason is lost with it.
3. Write or extend `mutants_closed_<finding>.py` with one mutant per behaviour of the fix (`old`
   occurs once in the merged source) and run the runner on it: the pin is then proven to fail
   without the fix, not only to pass with it. The runner stops at the first failing test of a
   mutant (`-x`), so a mutant proves only that test: give a pin whose failure matters one of its
   own. A former mutant of the pin is removed or turned into a revert, never duplicated. A pin held
   by two independent guards is killed by no single mutant: revert both, or say so in the
   docstring. A table may name a test of the base suite when the lot holds no pin of that behaviour.

### Two trees

A test is written to hold on the branch and on the future base, a scratch worktree of the branch
with the lot merged in (`git worktree add --detach <dir> HEAD`, then `git merge <lot>` there; the
edited files of the lot are copied into it). Run it with the branch's interpreter and
`PYTHONPATH=<dir>/hypertrain/src`: the venv is an editable install of the branch, whose src is
appended AFTER PYTHONPATH, so without the variable the scratch tree silently runs the branch's
code. The strict xfails the lot closes XFAIL on the branch and XPASS(strict) on the future base,
every other test passes on both, and the merge is one run whose only failures are the XPASS of
step 2. When the lot
changes a behaviour on purpose, restate the old assertion by driving the scenario through the step
the new contract requires (deliver the delta) so that the same invariant holds on both trees; never
widen a status or drop a control.

## Adding a test

State the invariant (a Lens id or a finding id), assert a value at a public boundary, keep a
control inside an xfail, and add a mutant to `mutants_<area>.py` whose `old` snippet occurs once
in the source. The guards refuse a table that no longer matches the tree.
