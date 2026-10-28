"""Mutants of the closed hole F6: one origin that cannot mature stops the beacon.

The fix (escrow lot, 4a7a1bea, ADR 0014 part 7) lets `ChallengeStore.push_beacon` catch the failure
of one origin, journal it and go on. The two pins below were strict xfails until it landed. The ten
entries are not all reverts of the fix.

Seven undo a behaviour of the fix or the guarantee it adds, in a private copy of src, so that the
runner proves the pins fail without it: the old handler, the handler narrowed to EscrowError or to
an allowlist of classes (the alternative the commit rejected), the custody refusals treated as a
broken ledger, the failure re-raised after its journal, the journal that cannot record a first
failure, and the loop that ends at the first stuck origin.

Three are regressions the pin must survive and are not part of 4a7a1bea: they model a stuck origin
that makes the push skip a later step (the lease expiry, the closing of blocked disputes, the
reaping of trials) while a maturity failure is on record. The pin kills them through the Calls
spies on those steps, which record that each step ran and not what it did, so a mutant that still
calls a step and turns it into a no-op would pass.

Run from hypertrain/: .venv/bin/python scripts/check_ledger_invariant_mutants.py --only <ids>
"""

from __future__ import annotations

STORE = "src/hypertrain/challenge/store.py"

# Lines of the per-origin handler of push_beacon, at their indentation in the source. HANDLER stops
# before the comment that ends its line, so that rewording the comment breaks no snippet; a
# replacement that ends the line must therefore accept a trailing comment.
HANDLER = " " * 20 + "except Exception as error:"
BROKEN_LEDGER = " " * 24 + "if isinstance(error, sqlite3.Error) or code in (\n"
JOURNAL = " " * 24 + "self._journal_maturity_failure_v2(run_id, origin[0], error)\n"
# The three steps of the push that follow the loop over the due origins.
EXPIRE_LEASES = " " * 16 + "self._expire_leases_v2(run_id, br.round)\n"
CLOSE_DISPUTES = " " * 12 + 'self._reap_v2(run_id, "dispute", self._close_blocked_disputes_v2)\n'
REAP_TRIALS = " " * 16 + "admission.reap_expired_trials(now=self._now(self._db))\n"


def guarded(step: str) -> str:
    """A source line behind a check that no maturity failure is on record."""
    indent = " " * (len(step) - len(step.lstrip()))
    return (
        f"{indent}if not self._db.execute(\n"
        f"{indent}    \"SELECT 1 FROM records_v2 WHERE kind='maturity-failed'\"\n"
        f"{indent}).fetchone():\n"
        f"    {step}"
    )


PUSH = "tests/ledger_invariants/test_gaps_admission_lock.py::"
LEDGER = "tests/ledger_invariants/test_shadow_2.py::"

PUSH_POISONED = "test_one_poisoned_shadow_origin_never_stops_the_work_of_a_beacon"
LEDGER_UNVALIDATABLE = (
    "test_an_origin_with_unvalidatable_custody_does_not_stop_the_other_due_origins"
)

# One id per variant of each pin: the first two fail with a ChallengeError, the third with an
# UnauthorizedSigner (the admission's hotkey was rotated after the credit).
PUSH_MISSING = PUSH + PUSH_POISONED + "[the shadow-custody record is missing]"
PUSH_CHANGED = PUSH + PUSH_POISONED + "[a retained publication changed]"
PUSH_ROTATED = PUSH + PUSH_POISONED + "[the admission was rotated to a spare hotkey]"
LEDGER_MISSING = LEDGER + LEDGER_UNVALIDATABLE + "[the shadow-custody record is missing]"
LEDGER_CHANGED = LEDGER + LEDGER_UNVALIDATABLE + "[a retained publication changed]"
LEDGER_ROTATED = LEDGER + LEDGER_UNVALIDATABLE + "[the admission's hotkey was rotated]"

MUTANTS = [
    {
        "id": "closed-f6-old-handler-restored",
        "invariants": ["SHD-19", "SHD-D3", "CL-14"],
        "path": STORE,
        "old": HANDLER,
        "new": (
            " " * 20
            + "except EscrowError as error:\n"
            + " " * 24
            + "if error.code not in (\n"
            + " " * 28
            + '"DISPUTED_OR_UNVESTED",\n'
            + " " * 28
            + '"IMMATURE_OR_UNKNOWN_ORIGIN",\n'
            + " " * 28
            + '"ALREADY_MATURED",\n'
            + " " * 24
            + "):\n"
            + " " * 28
            + "raise"
        ),
        "tests": [
            PUSH_MISSING,
            PUSH_CHANGED,
            PUSH_ROTATED,
            LEDGER_MISSING,
            LEDGER_CHANGED,
            LEDGER_ROTATED,
        ],
        "why": (
            "The per-origin handler is the old one again, which re-raises every failure but the "
            "three waiting codes (the new body stays behind that guard and only meets a waiting "
            "code), so an origin whose custody no longer revalidates rolls back the maturity of "
            "the others and the beacon answers an error."
        ),
    },
    {
        "id": "closed-f6-handler-catches-only-escrow-errors",
        "invariants": ["SHD-19", "SHD-D3"],
        "path": STORE,
        "old": HANDLER,
        "new": HANDLER.replace("Exception", "EscrowError"),
        "tests": [PUSH_MISSING, LEDGER_MISSING],
        "why": (
            "The handler is narrowed to EscrowError while it keeps the journal, so the custody "
            "failures that are not ledger errors still escape the loop, roll back the maturity "
            "of the other origins and fail the push."
        ),
    },
    {
        "id": "closed-f6-handler-allowlist-spares-the-rotation-failure",
        "invariants": ["SHD-19", "SHD-D3", "SHD-21"],
        "path": STORE,
        "old": HANDLER,
        "new": HANDLER.replace("Exception", "(EscrowError, ChallengeError)"),
        "tests": [PUSH_ROTATED, LEDGER_ROTATED],
        "why": (
            "The handler names the classes seen so far, so the signer error that a hotkey "
            "rotation raises while the original custody is revalidated escapes it and stops "
            "the maturity of every other origin."
        ),
    },
    {
        "id": "closed-f6-custody-failure-treated-as-a-broken-ledger",
        "invariants": ["SHD-19", "SHD-D3", "CL-36"],
        "path": STORE,
        "old": BROKEN_LEDGER,
        "new": BROKEN_LEDGER.replace("sqlite3.Error", "(sqlite3.Error, ChallengeError)"),
        "tests": [PUSH_MISSING, PUSH_CHANGED, LEDGER_MISSING, LEDGER_CHANGED],
        "why": (
            "A custody refusal is re-raised with the signs of a broken ledger or store, so a "
            "missing or altered retained record of one origin aborts the push instead of being "
            "journaled and retried."
        ),
    },
    {
        "id": "closed-f6-failure-is-journaled-then-reraised",
        "invariants": ["SHD-19", "SHD-D3", "CL-14"],
        "path": STORE,
        "old": JOURNAL,
        "new": JOURNAL + " " * 24 + "raise\n",
        "tests": [PUSH_CHANGED, LEDGER_CHANGED],
        "why": (
            "The failure is written to the journal and then raised again, so the record is "
            "kept but the maturity transaction of the other due origins is still aborted."
        ),
    },
    {
        "id": "closed-f6-first-failure-has-no-journal-row-to-compare",
        "invariants": ["SHD-19", "SHD-D3"],
        "path": STORE,
        "old": (
            " " * 12
            + "old: dict[str, Any] | None = "
            + 'self._record_v2(run_id, "maturity-failed", origin)\n'
            + " " * 8
            + "except ChallengeError:\n"
        ),
        "new": (
            " " * 12
            + "old: dict[str, Any] | None = "
            + 'self._record_v2(run_id, "maturity-failed", origin)\n'
            + " " * 8
            + "except sqlite3.Error:\n"
        ),
        "tests": [PUSH_MISSING, LEDGER_MISSING],
        "why": (
            "The journal no longer tolerates the absence of a previous row for the origin, so "
            "the very first failure of an origin raises from the journal itself and aborts the "
            "push it was meant to isolate."
        ),
    },
    {
        "id": "closed-f6-a-stuck-origin-ends-the-loop",
        "invariants": ["SHD-19", "SHD-D3"],
        "path": STORE,
        "old": JOURNAL,
        "new": JOURNAL + " " * 24 + "break\n",
        "tests": [PUSH_MISSING, LEDGER_MISSING],
        "why": (
            "The loop over the due origins stops at the first one that cannot mature, so every "
            "origin after it waits for a later round although nothing stops it."
        ),
    },
    {
        "id": "closed-f6-leases-are-not-expired-while-a-maturity-failure-is-on-record",
        "invariants": ["SHD-19", "CL-14"],
        "path": STORE,
        "old": EXPIRE_LEASES,
        "new": guarded(EXPIRE_LEASES),
        "tests": [PUSH_MISSING],
        "why": (
            "The lease expiry of a round is skipped while any origin of the run has a journaled "
            "maturity failure, so one stuck origin keeps expired leases open round after round."
        ),
    },
    {
        "id": "closed-f6-blocked-disputes-are-not-closed-while-a-maturity-failure-is-on-record",
        "invariants": ["SHD-19", "CL-36"],
        "path": STORE,
        "old": CLOSE_DISPUTES,
        "new": guarded(CLOSE_DISPUTES),
        "tests": [PUSH_MISSING],
        "why": (
            "The closing of blocked disputes is skipped while any origin of the run has a "
            "journaled maturity failure, so one stuck origin keeps every blocked dispute and its "
            "bond open."
        ),
    },
    {
        "id": "closed-f6-trials-are-not-reaped-while-a-maturity-failure-is-on-record",
        "invariants": ["SHD-19", "CL-36"],
        "path": STORE,
        "old": REAP_TRIALS,
        "new": guarded(REAP_TRIALS),
        "tests": [PUSH_MISSING],
        "why": (
            "The reaping of expired admission trials is skipped while any origin of the run has a "
            "journaled maturity failure, so one stuck origin keeps trials and their locks alive."
        ),
    },
]
