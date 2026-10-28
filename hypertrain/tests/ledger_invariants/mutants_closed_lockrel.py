"""Mutants that re-open what the lock-release lots closed: a request that computes for minutes does
not hold the store lock (executor-perf ADR 0012 part 2, aggregate-lock-release 6d623b0d and
f2fdf603).

Four pins were strict expected failures until those lots landed. `trial_reference_v2` now claims the
reference under the lock, replays it without (`run_reference`) and settles under the lock again, so
POST /join/proof no longer holds it for the whole replay (CL-30 + LV-01); `export_trial_v2` plans
the staging under the lock and stages and puts the inputs after it let go (CL-30 + LV-01);
`finalize_v2` replays the applied tape before its transaction (`_prime_finality_replay_v2`), which
also builds the services of a store that has just restarted outside the transaction that cannot
build them (FIN-04, CRD-05 + CRD-D03). Each mutant reverts one behaviour in a private copy of src,
so that the runner proves a pin fails without it:

- the two CL-30 pins of test_gaps_admission_lock fail when the reference is replayed, or the
  staged inputs of a trial are put into the object store, while the store lock is held; their
  stand-ins stall exactly there, and the assertion is that another thread could take the lock;
- the two restart pins (test_finality_2 and test_credit, the second through the HTTP route) fail
  when the finality replay is not primed and when the priming cannot build the services: the first
  Finalize after a restart then builds them inside its transaction and fails with the
  sqlite3.OperationalError the fix removed, a symptom the pins name in their docstrings.

What no pin or mutant covers here: the release of the lock while an aggregate computes (c8b3bd68),
the staging of the trial inputs under the lock (the pin of the export replaces the staging with a
stand-in and stalls the object puts), and the key and the age of a kept finality verdict (a verdict
of another replay would be trusted): tests/e2e/test_aggregate_lock_v2_e2e.py and
test_finalize_lock_v2_e2e.py hold them.

Run from hypertrain/: .venv/bin/python scripts/check_ledger_invariant_mutants.py --only <ids>
"""

from __future__ import annotations

STORE = "src/hypertrain/challenge/store.py"

LOCK = "tests/ledger_invariants/test_gaps_admission_lock.py::"
REFERENCE_UNLOCKED = (
    LOCK + "test_the_store_lock_is_free_while_the_public_proof_route_computes_the_reference"
)
EXPORT_UNLOCKED = LOCK + "test_the_store_lock_is_free_while_the_staged_inputs_of_a_trial_are_hashed"
RESTART = (
    "tests/ledger_invariants/test_finality_2.py::"
    "test_finalize_settles_as_the_first_request_after_a_restart"
)
RESTART_HTTP = (
    "tests/ledger_invariants/test_credit.py::test_the_first_finalize_after_a_restart_is_accepted"
)

REPLAY = (
    "                result = admission.run_reference(\n"
    "                    claim,\n"
    "                    now=now,\n"
    "                    trusted_launch=trusted,\n"
    "                    cancel=None if guard is None else guard.cancelled,\n"
    "                )\n"
)
PUTS = (
    "        objects = {\n"
    "            name: self.objects.put((directory / relative).read_bytes())\n"
    "            for name, relative in job.object_paths.items()\n"
    "        }\n"
    "        return {\n"
    '            "job": job.body(),\n'
)
POLICY = "        reference_reward_units = self._services(run_id)[0].policy.R_collectible_units\n"


def indented(text: str, by: int = 4) -> str:
    return "".join(" " * by + line + "\n" for line in text.splitlines())


MUTANTS = [
    {
        "id": "closed-lockrel-the-reference-is-replayed-under-the-store-lock",
        "invariants": ["CL-30", "LV-01"],
        "path": STORE,
        "old": REPLAY,
        "new": "                with self._lock:\n" + indented(REPLAY),
        "tests": [REFERENCE_UNLOCKED],
        "why": (
            "The replay of the trusted reference runs inside the store lock again, so a miner's "
            "POST /join/proof keeps every other request waiting for as long as it computes."
        ),
    },
    {
        "id": "closed-lockrel-the-trial-objects-are-put-under-the-store-lock",
        "invariants": ["CL-30", "LV-01"],
        "path": STORE,
        "old": PUTS,
        "new": (
            "        with self._lock:\n"
            + indented(PUTS[: PUTS.index("        return {")])
            + PUTS[PUTS.index("        return {") :]
        ),
        "tests": [EXPORT_UNLOCKED],
        "why": (
            "The staged inputs of a trial are put into the object store while the store lock is "
            "held, so hashing O(model) bytes stops every other request."
        ),
    },
    {
        "id": "closed-lockrel-the-finality-replay-is-not-primed-before-the-transaction",
        "invariants": ["CRD-05", "CRD-D03", "FIN-04"],
        "path": STORE,
        "old": "        self._prime_finality_replay_v2(run_id, w, raw)\n",
        "new": "",
        "tests": [RESTART_HTTP],
        "why": (
            "Finalize no longer replays the tape before its transaction, so the first request "
            "after a restart builds the services of the store inside it and answers a server "
            "error."
        ),
    },
    {
        "id": "closed-lockrel-the-priming-cannot-build-the-services-of-a-restarted-store",
        "invariants": ["CRD-05", "CRD-D03", "FIN-04"],
        "path": STORE,
        "old": POLICY,
        "new": POLICY.replace("self._services(run_id)[0]", "self._services_v2[run_id][0]"),
        "tests": [RESTART],
        "why": (
            "The replay job reads the escrow policy from the cache of built services, which a "
            "restarted store has not filled, so the priming gives up and the services are built "
            "inside the transaction of the Finalize."
        ),
    },
]
