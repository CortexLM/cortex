"""The mutants of scripts/check_audit_mutants.py: (name, path, old, new, tests) rows.

`path` is relative to hypertrain/, `old` must occur exactly once in it, and `tests` are pytest node
ids relative to hypertrain/ that must fail once `old` is replaced by `new`. This first table covers
the comparison of the production audit (auditor/worker.py:196-210), the one check that catches
every cheat accepted at intake; the intake, sanction and ledger defences were mutation-proved by
hand in private copies (see ADR 0009) and are the next rows to add.
"""

from __future__ import annotations

WORKER = "src/hypertrain/auditor/worker.py"
CHEATS = "tests/audit_cheaters/test_cheats_audit.py::test_cheat_is_mismatch_at_the_expected_leaf"

TABLE: tuple[tuple[str, str, str, str, tuple[str, ...]], ...] = (
    (
        "audit does not compare the leaf preimages",
        WORKER,
        "if a != b), None\n",
        "if False), None\n",
        (f"{CHEATS}[altered-weights-one-ulp-first-step]",),
    ),
    (
        "audit does not compare the leaves root",
        WORKER,
        '        and summary["leaves_root"] == commit.leaves_root\n',
        "",
        (f"{CHEATS}[commit-lie-leaves-root]",),
    ),
    (
        "audit does not compare the delta hash",
        WORKER,
        '        and summary["delta_hash"] == commit.delta_hash\n',
        "",
        (f"{CHEATS}[forged-delta-valid-hash]", f"{CHEATS}[commit-lie-delta-hash]"),
    ),
    (
        "audit does not compare the final theta hash",
        WORKER,
        '        and summary["final_theta_hash"] == commit.final_theta_hash\n',
        "",
        (f"{CHEATS}[commit-lie-final-theta-hash]",),
    ),
    (
        "audit does not compare the EF out hash",
        WORKER,
        '        and summary["ef_out_hash"] == commit.ef_out_hash\n',
        "",
        (f"{CHEATS}[commit-lie-ef-out-hash]",),
    ),
)
