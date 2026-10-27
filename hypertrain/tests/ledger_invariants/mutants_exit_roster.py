"""Mutants of the exit_roster package: one small regression of src each, killed by named tests."""

T = "tests/ledger_invariants/test_exit_roster.py::"
STORE = "src/hypertrain/challenge/store.py"

MUTANTS = [
    {
        "id": "exit-roster-queue-served-backwards",
        "invariants": ["A2W-18"],
        "path": STORE,
        "old": "\"AND state='queued' ORDER BY created,id\",",
        "new": "\"AND state='queued' ORDER BY created,id DESC\",",
        "tests": [T + "test_the_job_first_in_the_queue_is_the_first_to_be_leased"],
        "why": "The lease goes to the last job of the queue instead of the first.",
    },
    {
        "id": "exit-roster-live-lease-guard-removed",
        "invariants": ["A2W-24", "SG-10"],
        "path": STORE,
        "old": 'raise ChallengeError(409, "one live lease per auditor")',
        "new": "pass",
        "tests": [T + "test_an_ask_repeated_while_its_lease_runs_is_refused_and_grants_nothing"],
        "why": "An auditor that holds a running lease is granted a second one for the same ask.",
    },
    {
        "id": "exit-roster-conflicting-ask-accepted",
        "invariants": ["A2W-24", "SG-10"],
        "path": STORE,
        "old": "if known is not None and known[0] != digest:",
        "new": "if known is not None and known[0] != known[0]:",
        "tests": [T + "test_the_same_ask_nonce_with_another_body_is_refused_and_grants_nothing"],
        "why": "A second body under a known signed nonce is accepted instead of refused.",
    },
    {
        "id": "exit-roster-horizon-one-round-late",
        "invariants": ["A2W-20", "FIN-22"],
        "path": STORE,
        "old": '            elif now >= row["absolute"]:',
        "new": '            elif now > row["absolute"]:',
        "tests": [
            T + "test_the_horizon_exhausts_every_unleased_job_and_excludes_its_member_as_"
            "infrastructure"
        ],
        "why": "A queued job survives the round its absolute deadline names.",
    },
    {
        "id": "exit-roster-horizon-keeps-the-audit-counter",
        "invariants": ["A2W-20", "FIN-22"],
        "path": STORE,
        "old": 'unresolved_audits"])) - 1)\n                self._put_record_v2(',
        "new": 'unresolved_audits"])))\n                self._put_record_v2(',
        "tests": [
            T + "test_the_horizon_exhausts_every_unleased_job_and_excludes_its_member_as_"
            "infrastructure"
        ],
        "why": "An exhausted job leaves the audit counter of its round unchanged.",
    },
    {
        "id": "exit-roster-horizon-blames-the-miner",
        "invariants": ["A2W-20", "FIN-22"],
        "path": STORE,
        "old": '{"reason": "INFRASTRUCTURE", "evidence_hash": sha256_hex(row["id"].encode())},',
        "new": '{"reason": "UNVERIFIED", "evidence_hash": sha256_hex(row["id"].encode())},',
        "tests": [
            T + "test_the_horizon_exhausts_every_unleased_job_and_excludes_its_member_as_"
            "infrastructure"
        ],
        "why": "A job nobody leased excludes its member as a failed audit.",
    },
    {
        "id": "exit-roster-excluded-member-not-skipped",
        "invariants": ["A2W-21", "FIN-19"],
        "path": STORE,
        "old": "            if excluded:\n                continue",
        "new": "            if False:\n                continue",
        "tests": [T + "test_an_excluded_member_is_left_out_of_the_tape_and_the_others_are_applied"],
        "why": "The aggregation reads the records of a member that has an exclusion record.",
    },
]
