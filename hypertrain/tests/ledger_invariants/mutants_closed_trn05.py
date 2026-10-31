"""Mutants that re-open the hole TRN-05 closed (one hotkey uploads at most what one proof can
reference, objects and bytes, per `challenge_rounds` window, ADR 0014 part 5.3, lot
v3-ledger-small-fixes, D137): each one brings back, in one place, an upload the budget does not
count or a budget that does not hold.

The pin `test_one_hotkey_cannot_store_more_objects_than_a_proof_can_reference` was a strict
expected failure that asked for 512 objects per hotkey; 512 counts references, so it now asks for
the objects of `max_refs` references of `max_parts` parts and an index each, on a policy that sets
both to 1. The byte and window pins were written with the guard, and the golden admission policy
of the s100 runs holds the defaults and the digest the optional bounds must not move. The check of
the budget before the object is read only spares the read of a refused body, which an in-process
client cannot observe: no mutant removes it.

Run from hypertrain/: .venv/bin/python scripts/check_ledger_invariant_mutants.py --only <ids>
"""

from __future__ import annotations

APP = "src/hypertrain/challenge/app.py"
STORE = "src/hypertrain/challenge/admission_store.py"
ADMISSION = "src/hypertrain/challenge/admission.py"
MESSAGES = "src/hypertrain/protocol/messages_v2.py"

HTTP = "tests/ledger_invariants/test_http_auth_2.py::"
OBJECTS = HTTP + "test_one_hotkey_cannot_store_more_objects_than_a_proof_can_reference"
BYTES = HTTP + "test_one_hotkey_cannot_store_more_bytes_than_a_proof_can_reference"
WINDOW = HTTP + "test_the_next_challenge_window_gives_a_hotkey_a_new_upload_budget"
GOLDEN = (
    "tests/ops/s100/test_run_0ef600b4_golden.py::"
    "test_the_s20_admission_policy_keeps_its_hash_under_the_upload_bounds"
)

CLEANUP = (
    '            self.db.execute("DELETE FROM object_uploads WHERE beacon_window < ?", (window,))\n'
    '            self.db.execute("DELETE FROM object_budget WHERE beacon_window < ?", (window,))\n'
)
REFUSED = (
    "tests/challenge/test_object_upload_envelope_v2.py::"
    "test_a_refused_upload_stores_nothing[fewer bytes than signed]"
)
DUPLICATE = (
    "        if self.db.execute(\n"
    '            "SELECT 1 FROM object_uploads WHERE beacon_window=? AND hotkey=? AND sha256=?",\n'
    "            (window, hotkey, sha256),\n"
    "        ).fetchone():\n"
    "            return\n"
)

MUTANTS = [
    {
        "id": "closed-trn05-no-guard",
        "invariants": ["TRN-05", "LV-07"],
        "path": APP,
        "old": "        await budget(True)\n",
        "new": "",
        "tests": [OBJECTS],
        "why": (
            "Without the charge the check before the read never sees a spent budget: one admitted "
            "hotkey stores objects without any bound."
        ),
    },
    {
        "id": "closed-trn05-counted-before-the-bytes-are-checked",
        "invariants": ["TRN-05"],
        "path": APP,
        "old": "        await budget(False)\n",
        "new": "        await budget(True)\n",
        "tests": [REFUSED],
        "why": (
            "Counted before the bytes are checked, an upload refused for its length or hash "
            "changes the store, which a refused upload must not do."
        ),
    },
    {
        "id": "closed-trn05-the-refusal-is-a-conflict",
        "invariants": ["TRN-05"],
        "path": APP,
        "old": "                raise ChallengeError(413, error.code) from None\n",
        "new": "                raise ChallengeError(409, error.code) from None\n",
        "tests": [OBJECTS],
        "why": "D137 makes the refusal 413, the status of the other object size refusals.",
    },
    {
        "id": "closed-trn05-a-duplicate-is-counted",
        "invariants": ["TRN-05"],
        "path": STORE,
        "old": DUPLICATE,
        "new": "",
        "tests": [OBJECTS],
        "why": (
            "Counting the retry of an object the hotkey already uploaded in the window spends the "
            "budget of a miner whose upload only lost its answer."
        ),
    },
    {
        "id": "closed-trn05-bytes-uncounted",
        "invariants": ["TRN-05"],
        "path": STORE,
        "old": "        if used + 1 > objects or stored + size > total:\n",
        "new": "        if used + 1 > objects:\n",
        "tests": [BYTES],
        "why": (
            "A count without its bytes lets each counted object be as large as the object limit, "
            "so the storage a window takes is no longer bounded by what a proof can reference."
        ),
    },
    {
        "id": "closed-trn05-one-window-for-ever",
        "invariants": ["TRN-05"],
        "path": ADMISSION,
        "old": "            window=now // self.policy.challenge_rounds,\n",
        "new": "            window=0,\n",
        "tests": [WINDOW],
        "why": "A budget no window renews refuses a hotkey for the rest of the run once spent.",
    },
    {
        "id": "closed-trn05-the-spent-windows-stay",
        "invariants": ["TRN-05"],
        "path": STORE,
        "old": CLEANUP,
        "new": "            pass\n",
        "tests": [WINDOW],
        "why": (
            "Without the cleanup every hotkey leaves its upload rows of every window, a table that "
            "grows with the run."
        ),
    },
    {
        "id": "closed-trn05-references-without-their-parts",
        "invariants": ["TRN-05"],
        "path": MESSAGES,
        "old": "        objects = refs * (parts + 1)\n",
        "new": "        objects = refs\n",
        "tests": [OBJECTS],
        "why": (
            "Counting references instead of the objects they name refuses an honest trial: 512 "
            "references is not 512 objects (one 31 M trial stores 517)."
        ),
    },
    {
        "id": "closed-trn05-default-parts-below-the-protocol",
        "invariants": ["TRN-05"],
        "path": MESSAGES,
        "old": "        parts = MAX_PARTS if self.max_parts is None else self.max_parts\n",
        "new": "        parts = MAX_PARTS - 1 if self.max_parts is None else self.max_parts\n",
        "tests": [GOLDEN],
        "why": (
            "Absent bounds must be the protocol's (512 references of 8,192 parts): a smaller "
            "default refuses a reference of the largest object an index can name."
        ),
    },
    {
        "id": "closed-trn05-an-absent-bound-is-serialised",
        "invariants": ["TRN-05"],
        "path": MESSAGES,
        "old": (
            "    max_refs: Annotated[StrictInt, Field(ge=1, le=MAX_ARTIFACT_REFS)] | None = "
            "Field(\n"
            "        default=None, exclude_if=lambda v: v is None\n"
            "    )\n"
        ),
        "new": (
            "    max_refs: Annotated[StrictInt, Field(ge=1, le=MAX_ARTIFACT_REFS)] | None = None\n"
        ),
        "tests": [GOLDEN],
        "why": (
            "A bound serialised as null when absent changes the bytes and the digest of every "
            "admission policy, so the manifest and the run id of every existing run."
        ),
    },
]
