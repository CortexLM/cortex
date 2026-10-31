"""Mutants that re-open the hole RL-01 closed (the escrow routes are metered per coldkey and window,
`ESCROW_QUOTA`, ADR 0014 part 5.1, lot v3-ledger-small-fixes, D85): each one brings back, in one
place, an escrow event that costs nothing or a budget that does not hold.

The pin `test_rl01_the_escrow_transfers_of_one_key_are_bounded_in_one_beacon` was a strict expected
failure with a bound of 64 until the quota landed; its bound is now the 2,048 of the ADR and it
holds the quota with the three RL-01 pins written with it. The window in the identity and the
cleanup of earlier windows guard the same thing (a new window starts a new budget): removing one
alone is caught by the other, so `closed-rl01-one-budget-for-ever` reverts both.

Run from hypertrain/: .venv/bin/python scripts/check_ledger_invariant_mutants.py --only <ids>
"""

from __future__ import annotations

ADMISSION = "src/hypertrain/challenge/admission.py"
STORE = "src/hypertrain/challenge/store.py"

HTTP = "tests/ledger_invariants/test_admission_http.py::"
BOUNDED = HTTP + "test_rl01_the_escrow_transfers_of_one_key_are_bounded_in_one_beacon"
FREE = HTTP + "test_rl01_a_retry_or_a_refused_escrow_event_costs_no_quota"
ROUTES = HTTP + "test_rl01_every_escrow_route_draws_on_the_budget_of_its_coldkey"
WINDOW = HTTP + "test_rl01_the_next_window_gives_a_new_budget_and_drops_the_spent_one"

CHARGE = (
    "        self.store.quota(\n"
    '            f"escrow:{op.owner}:{now // window}", now, 0, ESCROW_BURST, code="ESCROW_QUOTA"\n'
    "        )\n"
)
CLEANUP = (
    "        self.store.db.execute(\n"
    "            \"DELETE FROM admission_quota WHERE identity >= 'escrow:' "
    "AND identity < 'escrow;' \"\n"
    '            "AND beacon < ?",\n'
    "            (now // window * window,),\n"
    "        )\n"
)

MUTANTS = [
    {
        "id": "closed-rl01-no-quota",
        "invariants": ["RL-01", "ESC-02"],
        "path": ADMISSION,
        "old": CHARGE,
        "new": "",
        "tests": [BOUNDED],
        "why": "Without the charge one funded key adds escrow events without any bound.",
    },
    {
        "id": "closed-rl01-burst-one-above-the-adr",
        "invariants": ["RL-01"],
        "path": ADMISSION,
        "old": "ESCROW_BURST = 2048\n",
        "new": "ESCROW_BURST = 2049\n",
        "tests": [BOUNDED],
        "why": "A burst above the 2,048 of ADR 0014 5.1 serves more events than the design bounds.",
    },
    {
        "id": "closed-rl01-burst-one-below-the-adr",
        "invariants": ["RL-01"],
        "path": ADMISSION,
        "old": "ESCROW_BURST = 2048\n",
        "new": "ESCROW_BURST = 2047\n",
        "tests": [BOUNDED],
        "why": (
            "A burst below 2,048 refuses an honest coldkey that locks and tops up a whole roster "
            "in one window, the sizing D53 2 rejected for the first, smaller bursts."
        ),
    },
    {
        "id": "closed-rl01-a-retry-is-charged",
        "invariants": ["RL-01"],
        "path": ADMISSION,
        "old": "        if self.escrow.recorded(op):\n            return\n",
        "new": "",
        "tests": [FREE],
        "why": (
            "Charging the retry of an event the ledger holds spends the budget of a client that "
            "only lost an answer, and can refuse it its own stored receipt."
        ),
    },
    {
        "id": "closed-rl01-one-budget-for-every-coldkey",
        "invariants": ["RL-01"],
        "path": ADMISSION,
        "old": 'f"escrow:{op.owner}:{now // window}"',
        "new": 'f"escrow::{now // window}"',
        "tests": [ROUTES],
        "why": (
            "One shared budget lets the first funded key that spends it refuse every other "
            "coldkey's locks, transfers and releases until the window ends."
        ),
    },
    {
        "id": "closed-rl01-one-budget-for-ever",
        "invariants": ["RL-01"],
        "path": ADMISSION,
        "old": CLEANUP + CHARGE,
        "new": CHARGE.replace(":{now // window}", ""),
        "tests": [WINDOW],
        "why": (
            "A budget that no window renews and no cleanup drops refuses an owner for the rest of "
            "the run once it spent it."
        ),
    },
    {
        "id": "closed-rl01-the-spent-windows-stay",
        "invariants": ["RL-01"],
        "path": ADMISSION,
        "old": CLEANUP,
        "new": "",
        "tests": [WINDOW],
        "why": (
            "Without the cleanup every coldkey leaves one quota row per window it was active in, "
            "a table that grows with the run."
        ),
    },
    {
        "id": "closed-rl01-the-cleanup-drops-the-live-window",
        "invariants": ["RL-01", "ESC-02"],
        "path": ADMISSION,
        "old": '"AND beacon < ?",\n            (now // window * window,),\n',
        "new": '"AND beacon < ?",\n            (now + 1,),\n',
        "tests": [BOUNDED],
        "why": (
            "A cleanup that also drops the rows of the current window gives back the whole budget "
            "on every charge, so nothing is bounded."
        ),
    },
    {
        "id": "closed-rl01-the-admission-lock-is-free",
        "invariants": ["RL-01", "ESC-02"],
        "path": ADMISSION,
        "old": "            self.meter_escrow(op, now=now)\n",
        "new": "",
        "tests": [ROUTES],
        "why": "An admission lock that is not charged adds ledger events past the budget.",
    },
    {
        "id": "closed-rl01-the-contest-lock-is-free",
        "invariants": ["RL-01", "ESC-02"],
        "path": STORE,
        "old": "                            admission.meter_escrow(op, now=now)\n",
        "new": "",
        "tests": [ROUTES],
        "why": "A contest lock that is not charged adds ledger events past the budget.",
    },
    {
        "id": "closed-rl01-transfers-and-releases-are-free",
        "invariants": ["RL-01", "ESC-02"],
        "path": STORE,
        "old": "                        admission.meter_escrow(model, now=now)\n",
        "new": "",
        "tests": [BOUNDED],
        "why": "Transfers and releases that are not charged add ledger events without any bound.",
    },
]
