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

When a lot lands, the strict xfails it fixes turn red (XPASS). Find them with the tag, delete the
marker and the sentence of the reason, and the test is a pin:

```bash
grep -rn "fix-lot: escrow-relay-fixes" tests/ledger_invariants
```

A test written against behaviour survives the lot; one that fails for another reason is coupled to
an internal and must be fixed before the marker is removed.

## Adding a test

State the invariant (a Lens id or a finding id), assert a value at a public boundary, keep a
control inside an xfail, and add a mutant to `mutants_<area>.py` whose `old` snippet occurs once
in the source. The guards refuse a table that no longer matches the tree.
