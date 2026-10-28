"""Regressions of src/hypertrain that the bootstrap gap tests must catch (D39, group bootstrap).

Each entry is one small realistic change: a comparison flipped, a term dropped, a check deleted.
`old` occurs exactly once in `path`; `tests` are node ids that must fail under the change. The
strict xfails of the gap file have no mutant: they are holes, and their fix proofs are private
copies recorded in the result rows.
"""

from __future__ import annotations

G = "tests/ledger_invariants/test_gaps_bootstrap.py::"
ESCROW = "src/hypertrain/ledger/escrow_v2.py"
STORE = "src/hypertrain/challenge/store.py"
TRUST = "src/hypertrain/challenge/trust_v2.py"
MESSAGES = "src/hypertrain/protocol/messages_v2.py"
ADMISSION = "src/hypertrain/challenge/admission.py"

BOOTSTRAP_ORDER = (
    G
    + "test_a_bootstrap_with_the_pinned_origin_ids_in_the_wrong_order_is_refused_and_issues_nothing"
)
GENESIS_ORDER = G + "test_a_test_genesis_with_the_pinned_origin_ids_in_the_wrong_order_is_refused"
CONSISTENT_BUDGET = (
    G + "test_a_reward_consistent_with_its_own_budget_one_unit_off_the_round_budget_is_refused"
)
FLOAT_WITNESS = G + "test_the_run_bound_is_decided_to_the_unit_where_a_float_evaluation_is_not"
FIELD_FAULT = G + "test_a_policy_field_fault_raises_one_error_located_at_that_field"
CEILING = G + "test_a_policy_amount_may_reach_two_to_the_53_minus_one_and_not_one_unit_more"
CLOSURE = G + "test_a_round_does_not_open_while_the_economic_bound_closure_is_false"
SIX_WORKERS = (
    G + "test_a_round_of_six_audited_workers_is_finalized_with_the_remainder_to_the_first_hotkeys"
)
REPIN = G + "test_the_dataset_linkage_may_be_repinned_before_the_first_round_and_confirmed_after_it"
NO_NONCE = (
    G + "test_a_service_profile_refused_after_a_round_exists_keeps_no_nonce_and_stays_admissible"
)

BOUND_EXPRESSION = """\
    return (
        policy.q_floor
        * policy.s_lower_ppm
        * policy.beta_ppm
        * (policy.R_collectible_units + policy.S_min_units)
        >= policy.G_max_units * 10**18
    )
"""
BOUND_TRUE_DIVISION = """\
    return (
        policy.q_floor
        * policy.s_lower_ppm
        * policy.beta_ppm
        * (policy.R_collectible_units + policy.S_min_units)
        / 10**18
        >= policy.G_max_units
    )
"""
BOUND_BOTH_SIDES_FLOAT = """\
    return float(
        policy.q_floor
        * policy.s_lower_ppm
        * policy.beta_ppm
        * (policy.R_collectible_units + policy.S_min_units)
    ) >= float(policy.G_max_units * 10**18)
"""
ROSTER_GATE = "                if not status.eligible or (\n                    entry.admission_id,"
ROSTER_GATE_GONE = "                if (\n                    entry.admission_id,"
STORE_REMAINDER = "            for hot in included[:remainder]:\n"
STORE_REMAINDER_LAST = "            for hot in included[len(included) - remainder :]:\n"
PROFILE_REFUSAL = """\
                "SELECT 1 FROM records_v2 WHERE run_id=? AND kind='round' LIMIT 1", (run_id,)
            ).fetchone():
                raise ChallengeError(409, "SERVICE_PROFILE_BEFORE_OPEN")
"""
PROFILE_REFUSAL_COMMITTED = """\
                "SELECT 1 FROM records_v2 WHERE run_id=? AND kind='round' LIMIT 1", (run_id,)
            ).fetchone():
                self._db.execute("COMMIT")
                self._db.execute("BEGIN IMMEDIATE")
                raise ChallengeError(409, "SERVICE_PROFILE_BEFORE_OPEN")
"""

DATASET_PUT = """\
            self._put_record_v2(run_id, "dataset", "inputs", data)
        return {"status": "configured"}
"""
DATASET_PUT_ONCE = """\
            if self._db.execute(
                "SELECT 1 FROM records_v2 WHERE run_id=? AND kind='dataset'", (run_id,)
            ).fetchone():
                raise ChallengeError(409, "dataset linkage is fixed")
            self._put_record_v2(run_id, "dataset", "inputs", data)
        return {"status": "configured"}
"""
DATASET_PUT_NO_ROUND = """\
            if self._db.execute(
                "SELECT 1 FROM records_v2 WHERE run_id=? AND kind='round' LIMIT 1", (run_id,)
            ).fetchone():
                raise ChallengeError(409, "dataset linkage is fixed once a round exists")
            self._put_record_v2(run_id, "dataset", "inputs", data)
        return {"status": "configured"}
"""

MUTANTS = [
    {
        "id": "gap-bootstrap-origin-id-any-valid-id",
        "invariants": ["BND-10", "BND-13"],
        "path": ESCROW,
        "old": 'raise EscrowError("BOOTSTRAP_ORIGIN_DOMAIN")',
        "new": (
            "keys = [self._origin_key()] if hasattr(self, '_origin_key') else []\n"
            "                    pin = bootstrap.allocation_hash\n"
            "                    if o.origin_id not in {\n"
            "                        bootstrap_origin_id(self.run_id, pin, j, *keys)\n"
            "                        for j in range(len(bootstrap.origins))\n"
            "                    }:\n"
            "                        raise EscrowError('BOOTSTRAP_ORIGIN_DOMAIN')"
        ),
        "tests": [BOOTSTRAP_ORDER],
        "why": "a bootstrap origin may carry the id of another index of the same pinned list.",
    },
    {
        "id": "gap-genesis-origin-id-any-valid-id",
        "invariants": ["BND-10", "BND-14"],
        "path": ESCROW,
        "old": 'raise EscrowError("GENESIS_ORIGIN_DOMAIN")',
        "new": (
            "keys = [self._origin_key()] if hasattr(self, '_origin_key') else []\n"
            "                    pin = genesis.allocation_hash\n"
            "                    if o.origin_id not in {\n"
            "                        genesis_origin_id(self.run_id, pin, j, *keys)\n"
            "                        for j in range(len(genesis.origins))\n"
            "                    }:\n"
            "                        raise EscrowError('GENESIS_ORIGIN_DOMAIN')"
        ),
        "tests": [GENESIS_ORDER],
        "why": "a genesis origin may carry the id of another index of the same pinned list.",
    },
    {
        "id": "gap-round-budget-deleted",
        "invariants": ["BND-20", "BND-11"],
        "path": ESCROW,
        "old": "if reward.budget_units != self.policy.round_reward_units:",
        "new": "if False:",
        "tests": [CONSISTENT_BUDGET],
        "why": "a reward consistent with its own budget mints units the round does not allow.",
    },
    {
        "id": "gap-round-budget-only-above",
        "invariants": ["BND-20"],
        "path": ESCROW,
        "old": "if reward.budget_units != self.policy.round_reward_units:",
        "new": "if reward.budget_units > self.policy.round_reward_units:",
        "tests": [CONSISTENT_BUDGET + "[-1]"],
        "why": "a round may be paid one unit under its budget when the body is self-consistent.",
    },
    {
        "id": "gap-round-budget-only-below",
        "invariants": ["BND-20", "BND-11"],
        "path": ESCROW,
        "old": "if reward.budget_units != self.policy.round_reward_units:",
        "new": "if reward.budget_units < self.policy.round_reward_units:",
        "tests": [CONSISTENT_BUDGET + "[1]"],
        "why": "a round may be paid one unit over its budget when the body is self-consistent.",
    },
    {
        "id": "gap-bound-true-division",
        "invariants": ["BND-01"],
        "path": TRUST,
        "old": BOUND_EXPRESSION,
        "new": BOUND_TRUE_DIVISION,
        "tests": [FLOAT_WITNESS + "[a deficit of 0.4 units at the top of the exact range]"],
        "why": "the bound compares a float quotient, which rounds a deficit of a fraction away.",
    },
    {
        "id": "gap-bound-float-on-both-sides",
        "invariants": ["BND-01"],
        "path": TRUST,
        "old": BOUND_EXPRESSION,
        "new": BOUND_BOTH_SIDES_FLOAT,
        "tests": [FLOAT_WITNESS + "[a deficit of 0.13 units]"],
        "why": "the bound converts both sides to floats, which cannot tell two goals a unit apart.",
    },
    {
        "id": "gap-policy-positive-ceiling",
        "invariants": ["BND-17"],
        "path": MESSAGES,
        "old": "Positive = Annotated[StrictInt, Field(ge=1, le=2**53 - 1)]",
        "new": "Positive = Annotated[StrictInt, Field(ge=1, le=2**63 - 1)]",
        "tests": [
            FIELD_FAULT + "[beta_ppm at 2^53]",
            CEILING + "[beta_ppm]",
        ],
        "why": "a positive policy field may reach 2^63, past the exact range of the encoding.",
    },
    {
        "id": "gap-policy-positive-lax-integers",
        "invariants": ["BND-17"],
        "path": MESSAGES,
        "old": "Positive = Annotated[StrictInt, Field(ge=1, le=2**53 - 1)]",
        "new": "Positive = Annotated[int, Field(ge=1, le=2**53 - 1)]",
        "tests": [
            FIELD_FAULT + "[a boolean in a positive field]",
            FIELD_FAULT + "[a float in a positive field]",
        ],
        "why": "a positive policy field accepts True or 1000000.0 where an integer is due.",
    },
    {
        "id": "gap-policy-ppm-lax-integers",
        "invariants": ["BND-17"],
        "path": MESSAGES,
        "old": "PPM = Annotated[StrictInt, Field(ge=0, le=1_000_000)]",
        "new": "PPM = Annotated[int, Field(ge=0, le=1_000_000)]",
        "tests": [
            FIELD_FAULT + "[a boolean in a ppm field]",
            FIELD_FAULT + "[a float in a ppm field]",
        ],
        "why": "a ppm policy field accepts True or 1000000.0 where an integer is due.",
    },
    {
        "id": "gap-policy-unsigned-lax-integers",
        "invariants": ["BND-17"],
        "path": MESSAGES,
        "old": "U64 = Annotated[StrictInt, Field(ge=0, le=2**53 - 1)]",
        "new": "U64 = Annotated[int, Field(ge=0, le=2**53 - 1)]",
        "tests": [
            FIELD_FAULT + "[a boolean in an unsigned field]",
            FIELD_FAULT + "[a string in an integer field]",
        ],
        "why": "an unsigned policy field accepts True or a numeric string where an integer is due.",
    },
    {
        "id": "gap-roster-skips-the-eligibility-gate",
        "invariants": ["BND-03", "BND-07"],
        "path": STORE,
        "old": ROSTER_GATE,
        "new": ROSTER_GATE_GONE,
        "tests": [CLOSURE],
        "why": "a round opens for a roster whose members are not eligible, closure or not.",
    },
    {
        "id": "gap-status-ignores-the-economic-bound",
        "invariants": ["BND-03"],
        "path": ADMISSION,
        "old": "            and funding.conservative_bound\n",
        "new": "",
        "tests": [CLOSURE],
        "why": "a funded miner is eligible whether or not the economic bound is recorded.",
    },
    {
        "id": "gap-store-remainder-to-the-last-hotkeys",
        "invariants": ["BND-20"],
        "path": STORE,
        "old": STORE_REMAINDER,
        "new": STORE_REMAINDER_LAST,
        "tests": [SIX_WORKERS],
        "why": "the store gives the remainder to the last hotkeys, the ledger to the first.",
    },
    {
        "id": "gap-profile-refusal-keeps-the-nonce",
        "invariants": ["BND-22"],
        "path": STORE,
        "old": PROFILE_REFUSAL,
        "new": PROFILE_REFUSAL_COMMITTED,
        "tests": [NO_NONCE],
        "why": "a profile refused for an existing round commits the owner receipt's reservation.",
    },
    {
        "id": "gap-dataset-linkage-pinned-once",
        "invariants": ["BND-04"],
        "path": STORE,
        "old": DATASET_PUT,
        "new": DATASET_PUT_ONCE,
        "tests": [REPIN],
        "why": "the operator route refuses to pin a run's dataset linkage a second time.",
    },
    {
        "id": "gap-dataset-linkage-frozen-with-its-own-pair",
        "invariants": ["BND-04"],
        "path": STORE,
        "old": DATASET_PUT,
        "new": DATASET_PUT_NO_ROUND,
        "tests": [REPIN],
        "why": "the operator route refuses even the pair already pinned once a round exists.",
    },
]
