"""Regressions of src/hypertrain that the bounds tests must catch (package bounds, D39).

Each entry is one small realistic change: a comparison flipped, a term dropped, a check deleted.
`old` occurs exactly once in `path`; `tests` are node ids that must fail under the change.
"""

from __future__ import annotations

T = "tests/ledger_invariants/test_bounds.py::"
T2 = "tests/ledger_invariants/test_bounds_2.py::"
TRUST = "src/hypertrain/challenge/trust_v2.py"
ESCROW = "src/hypertrain/ledger/escrow_v2.py"
MESSAGES = "src/hypertrain/protocol/messages_v2.py"

BOUND_EXACT = T + "test_the_run_bound_holds_up_to_its_capacity_and_not_one_unit_beyond"
BOUND_EDGES = T + "test_the_run_bound_edges_zero_detection_zero_goal_and_every_field_at_its_maximum"
FLOOR_PROPERTY = (
    T + "test_the_stake_floor_is_rounded_up_and_the_same_in_status_gate_and_operator_tool"
)
FLOOR_ROUNDS_UP = T + "test_the_stake_floor_rounds_up_where_the_exact_quotient_is_fractional"
BOOTSTRAP_ONCE = (
    "tests/ledger/test_production_bootstrap_v2.py::"
    "test_bootstrap_idempotent_per_operation_and_once_per_run"
)
BOOTSTRAP_REFUSED = (
    T + "test_a_bootstrap_other_than_the_owner_pinned_allocation_is_refused_and_issues_nothing"
)
ONE_POLICY_FOR_LIFE = T + "test_a_ledger_database_belongs_to_one_policy_and_one_run_for_life"

MUTANTS = [
    {
        "id": "bnd-bound-strict-inequality",
        "invariants": ["BND-01"],
        "path": TRUST,
        "old": ">= policy.G_max_units * 10**18",
        "new": "> policy.G_max_units * 10**18",
        "tests": [BOUND_EXACT, BOUND_EDGES],
        "why": "equality no longer passes: a run whose stake exactly covers its goal is refused.",
    },
    {
        "id": "bnd-bound-drops-collectible",
        "invariants": ["BND-01"],
        "path": TRUST,
        "old": "* (policy.R_collectible_units + policy.S_min_units)",
        "new": "* (policy.S_min_units)",
        "tests": [BOUND_EXACT],
        "why": "collectible reward no longer counts toward the exposure the bound credits.",
    },
    {
        "id": "bnd-gate-floor-division",
        "invariants": ["BND-06"],
        "path": TRUST,
        "old": "(policy.G_max_units * 10**18 + penalty_den - 1) // penalty_den",
        "new": "(policy.G_max_units * 10**18) // penalty_den",
        "tests": [FLOOR_PROPERTY, FLOOR_ROUNDS_UP],
        "why": "the tape gate truncates the required stake instead of rounding it up.",
    },
    {
        "id": "bnd-status-floor-division",
        "invariants": ["BND-06"],
        "path": "src/hypertrain/challenge/admission.py",
        "old": "(p.G_max_units * 10**18 + denominator - 1) // denominator",
        "new": "(p.G_max_units * 10**18) // denominator",
        "tests": [FLOOR_PROPERTY, FLOOR_ROUNDS_UP],
        "why": "the admission status truncates the required stake, so a short lock is eligible.",
    },
    {
        "id": "bnd-cli-floor-division",
        "invariants": ["BND-06"],
        "path": "src/hypertrain/miner/coldkey_ops.py",
        "old": "(policy.G_max_units * 10**18 + denominator - 1) // denominator",
        "new": "(policy.G_max_units * 10**18) // denominator",
        "tests": [FLOOR_PROPERTY, FLOOR_ROUNDS_UP],
        "why": "the operator tool tells a coldkey to lock one unit too few.",
    },
    {
        "id": "bnd-floor-ignores-q-floor",
        "invariants": ["BND-07"],
        "path": TRUST,
        "old": "anomaly_ppm, policy.q_floor, economic)",
        "new": "anomaly_ppm, economic)",
        "tests": [T + "test_the_audit_floor_is_certainty_for_every_state_of_a_roster_entry"],
        "why": "the pinned audit floor leaves the max, so an ACTIVE miner is audited at 25%.",
    },
    {
        "id": "bnd-draw-inclusive-threshold",
        "invariants": ["BND-08"],
        "path": TRUST,
        "old": "selected = x * 1_000_000 < q_ppm * 2**256",
        "new": "selected = x * 1_000_000 <= q_ppm * 2**256",
        "tests": [T + "test_the_audit_draw_selects_exactly_the_digests_below_ceil_q_of_the_space"],
        "why": "the draw selects at the threshold too, so q = 0 audits the digest zero.",
    },
    {
        "id": "bnd-draw-accepts-bool",
        "invariants": ["BND-08"],
        "path": TRUST,
        "old": "type(q_ppm) is not int",
        "new": "not isinstance(q_ppm, int)",
        "tests": [T + "test_the_audit_draw_refuses_out_of_range_or_non_integer_bounds"],
        "why": "True is accepted as the probability one unit in ppm.",
    },
    {
        "id": "bnd-draw-ignores-hotkey",
        "invariants": ["BND-08"],
        "path": TRUST,
        "old": '|{signature_hash}|{hotkey}".encode()',
        "new": '|{signature_hash}".encode()',
        "tests": [
            T + "test_the_audit_draw_is_a_function_of_the_run_the_round_the_beacon_and_the_hotkey"
        ],
        "why": "every hotkey of a round shares one draw.",
    },
    {
        "id": "bnd-policy-q-floor-below-certainty",
        "invariants": ["BND-07", "BND-17"],
        "path": MESSAGES,
        "old": "q_floor: Literal[1_000_000]",
        "new": "q_floor: PPM",
        "tests": [
            T + "test_a_policy_cannot_pin_an_audit_floor_below_certainty",
            T + "test_a_policy_with_an_unknown_or_out_of_range_field_is_refused",
        ],
        "why": "a policy may pin an audit floor below 100 percent.",
    },
    {
        "id": "bnd-policy-q-floor-default",
        "invariants": ["BND-17"],
        "path": MESSAGES,
        "old": "q_floor: Literal[1_000_000]",
        "new": "q_floor: Literal[1_000_000] = 1_000_000",
        "tests": [T + "test_a_policy_without_one_of_its_fields_is_refused_none_has_a_default"],
        "why": "a policy may omit its safety field q_floor and silently take the default.",
    },
    {
        "id": "bnd-policy-integer-cap",
        "invariants": ["BND-17"],
        "path": MESSAGES,
        "old": "U64 = Annotated[StrictInt, Field(ge=0, le=2**53 - 1)]",
        "new": "U64 = Annotated[StrictInt, Field(ge=0, le=2**63 - 1)]",
        "tests": [T + "test_a_policy_with_an_unknown_or_out_of_range_field_is_refused"],
        "why": "policy integers may reach 2^63, past the exact range of the canonical encoding.",
    },
    {
        "id": "bnd-policy-allows-bootstrap-pin-in-test",
        "invariants": ["BND-14", "BND-17"],
        "path": MESSAGES,
        "old": "if self.bootstrap_allocation_hash is not None and",
        "new": "if False and",
        "tests": [
            T + "test_a_policy_with_an_unknown_or_out_of_range_field_is_refused",
            T + "test_units_never_cross_ledger_modes_a_run_has_one_mode_for_life",
        ],
        "why": "a test-mode policy may pin a production bootstrap allocation.",
    },
    {
        "id": "bnd-bootstrap-second-issue-allowed",
        "invariants": ["BND-09"],
        "path": ESCROW,
        "old": "\"SELECT 1 FROM escrow_events WHERE kind='PRODUCTION_BOOTSTRAP'\"",
        "new": "\"SELECT 1 FROM escrow_events WHERE kind='NO_SUCH_KIND'\"",
        "tests": [BOOTSTRAP_ONCE],
        "why": "the one-bootstrap-per-run guard no longer sees the earlier bootstrap event.",
    },
    {
        "id": "bnd-operation-conflict-ignored",
        "invariants": ["BND-09"],
        "path": ESCROW,
        "old": 'if row["request"] != request:',
        "new": "if False:",
        "tests": [
            BOOTSTRAP_ONCE,
            T
            + "test_the_test_genesis_has_one_operation_a_replay_returns_it_other_content_conflicts",
        ],
        "why": "an operation id reused with other content returns the stored receipt silently.",
    },
    {
        "id": "bnd-bootstrap-pin-not-compared",
        "invariants": ["BND-10"],
        "path": ESCROW,
        "old": "bootstrap.allocation_hash != self.policy.bootstrap_allocation_hash",
        "new": "False",
        "tests": [BOOTSTRAP_REFUSED],
        "why": "a self-consistent allocation other than the owner-pinned one is accepted.",
    },
    {
        "id": "bnd-bootstrap-origin-domain-unchecked",
        "invariants": ["BND-10"],
        "path": ESCROW,
        "old": 'raise EscrowError("BOOTSTRAP_ORIGIN_DOMAIN")',
        "new": "pass",
        "tests": [BOOTSTRAP_REFUSED],
        "why": "bootstrap origin ids need not carry the bootstrap domain, run and index.",
    },
    {
        "id": "bnd-authority-ignores-run",
        "invariants": ["BND-10"],
        "path": ESCROW,
        "old": "if model.run_id != self.run_id or not verify(",
        "new": "if not verify(",
        "tests": [BOOTSTRAP_REFUSED],
        "why": "an origin authority signature for another run is accepted by this ledger.",
    },
    {
        "id": "bnd-genesis-after-first-event",
        "invariants": ["BND-13"],
        "path": ESCROW,
        "old": 'or self.db.execute("SELECT 1 FROM escrow_events LIMIT 1").fetchone()',
        "new": "or False",
        "tests": [T + "test_a_test_genesis_is_refused_once_the_ledger_has_any_event"],
        "why": "a test genesis is accepted after a reward event, adding units to an earning run.",
    },
    {
        "id": "bnd-genesis-in-production",
        "invariants": ["BND-14"],
        "path": ESCROW,
        "old": 'self.policy.ledger_mode != "test"',
        "new": "False",
        "tests": [T + "test_units_never_cross_ledger_modes_a_run_has_one_mode_for_life"],
        "why": "a production run accepts a test genesis.",
    },
    {
        "id": "bnd-ledger-policy-edit-honoured",
        "invariants": ["BND-04", "BND-14"],
        "path": ESCROW,
        "old": '\n                or rows[0]["policy"] != policy.model_dump_json()',
        "new": "",
        "tests": [ONE_POLICY_FOR_LIFE],
        "why": "an out-of-band edit of the pinned policy text is no longer detected at start.",
    },
    {
        "id": "bnd-ledger-run-pin-ignored",
        "invariants": ["BND-04", "BND-14"],
        "path": ESCROW,
        "old": '\n                or rows[0]["run_id"] != self.run_id',
        "new": "",
        "tests": [ONE_POLICY_FOR_LIFE],
        "why": "a ledger database may be reopened under another run with the same policy.",
    },
    {
        "id": "bnd-receipt-mode-unchecked",
        "invariants": ["BND-14"],
        "path": ESCROW,
        "old": "\n                or receipt.ledger_mode != self.policy.ledger_mode",
        "new": "",
        "tests": [
            T + "test_a_receipt_of_another_ledger_mode_fails_the_event_chain_at_the_next_start"
        ],
        "why": "restart verification accepts a receipt written under the other ledger mode.",
    },
    {
        "id": "bnd-economics-hash-unchecked",
        "invariants": ["BND-04"],
        "path": ESCROW,
        "old": "if digest(policy) != manifest.network.economics_policy_hash:",
        "new": "if False:",
        "tests": [T + "test_the_ledger_refuses_a_policy_other_than_the_one_the_manifest_pins"],
        "why": "the ledger runs a policy the owner-signed manifest does not pin.",
    },
    {
        "id": "bnd-round-budget-relation-unchecked",
        "invariants": ["BND-04", "BND-20"],
        "path": ESCROW,
        "old": "if policy.round_reward_units !=",
        "new": "if False and policy.round_reward_units !=",
        "tests": [
            T
            + "test_the_ledger_refuses_a_round_reward_that_is_not_epochs_per_round_times_a_million"
        ],
        "why": "a policy may pay a round reward that its manifest's epochs do not justify.",
    },
]

STORE = "src/hypertrain/challenge/store.py"
APP = "src/hypertrain/challenge/app.py"
CAPACITY = "tests/challenge/test_service_capacity_v2.py::"
ECONOMICS = "tests/challenge/test_production_economics_v2.py::"
BOOTSTRAP = "tests/ledger/test_production_bootstrap_v2.py::"

CLI_ZERO = T + "test_no_stake_satisfies_the_floor_when_detection_is_zero_in_any_implementation"
CAP_PROPERTY = (
    T + "test_bootstrap_and_live_rewards_never_issue_more_than_the_cap_and_an_exact_fit_passes"
)
SPLIT_PROPERTY = (
    T + "test_a_round_issues_exactly_its_budget_floor_split_with_the_remainder_to_the_first_hotkeys"
)
OTHER_SPLIT = T + "test_a_reward_body_that_states_another_split_than_the_documented_one_is_refused"
ROUND_BUDGET = T + "test_a_reward_whose_budget_is_not_the_round_reward_is_refused_with_round_budget"
ELIGIBLE_AFTER_BOUND = (
    T2 + "test_a_production_miner_is_not_eligible_until_the_coordinator_wrote_the_bound"
)
FAILING_BOUND = T2 + "test_a_bound_that_fails_writes_no_record_and_nobody_becomes_eligible"
ADMIN_ROUTES = (
    T2 + "test_issuance_bound_dataset_and_run_creation_routes_refuse_every_bearer_but_the_operators"
)
BEARER_REREAD = (
    T2 + "test_the_operator_bearer_is_read_on_every_request_and_no_usable_file_means_no_access"
)
COORDINATOR_ONLY = (
    T2 + "test_a_genesis_envelope_not_signed_by_the_coordinator_is_refused_with_the_operator_bearer"
)
INNER_SIGNATURE = (
    T2
    + "test_a_production_bootstrap_whose_authority_signature_is_not_the_coordinators_issues_nothing"
)
STORE_REPLAY_KEY = (
    T2 + "test_a_second_signed_bootstrap_is_refused_by_the_store_and_the_first_replays_unchanged"
)
OBJECT_EDITED = (
    T2 + "test_a_policy_object_edited_in_the_object_store_cannot_become_the_policy_of_the_run"
)
ROSTER_GATES = T2 + "test_a_round_opening_with_a_roster_entry_below_certainty_or_ceiling_is_refused"
LEASES = T2 + "test_every_roster_entry_of_a_round_gets_one_full_replay_audit_lease"
BOOTSTRAP_WEIGHTS = (
    T2 + "test_bootstrap_units_never_reach_the_epoch_weights_only_the_matured_live_reward_does"
)
GENESIS_COLLATERAL = T2 + "test_test_genesis_units_are_collateral_and_never_payable"
QUALIFICATION = T2 + "test_production_services_exist_only_behind_an_owner_signed_cuda_qualification"
FROZEN_BACKEND = T2 + "test_the_execution_backend_of_a_run_is_frozen_once_chosen"
CHARGE_BOUNDS = T2 + "test_a_service_charge_outside_its_bounds_is_refused_and_records_nothing"
CHARGE_BUDGET = (
    T2 + "test_the_service_budget_is_run_wide_per_kind_a_repeat_is_free_and_equality_is_allowed"
)
CHARGE_DEADLINE = (
    T2 + "test_a_service_charge_after_the_reservation_deadline_is_refused_and_at_it_is_allowed"
)
PROFILE_ONCE = T2 + "test_a_service_profile_is_admitted_before_the_first_round_and_at_most_once"
PROFILE_CODE = (
    T2 + "test_a_changed_capacity_implementation_voids_the_admitted_profile_and_cannot_replace_it"
)
PROFILE_BEFORE_OPEN = T2 + "test_a_service_profile_is_refused_once_a_round_exists"
MISSING_POLICY = T2 + "test_a_manifest_whose_economics_policy_object_is_missing_creates_no_run"
TEST_MODE_RECORD = T2 + "test_a_test_mode_run_has_no_economic_bound_record_to_write"

MUTANTS += [
    {
        "id": "bnd-cli-zero-detection-reachable",
        "invariants": ["BND-06"],
        "path": "src/hypertrain/miner/coldkey_ops.py",
        "old": "else policy.G_max_units + locked + 1",
        "new": "else 0",
        "tests": [CLI_ZERO],
        "why": "with no detection the operator tool asks for a stake the coldkey can satisfy.",
    },
    {
        "id": "bnd-closure-always-bound",
        "invariants": ["BND-03"],
        "path": STORE,
        "old": 'econ.ledger_mode == "test"',
        "new": "True",
        "tests": [ELIGIBLE_AFTER_BOUND, FAILING_BOUND],
        "why": "production eligibility no longer waits for the coordinator-signed economic bound.",
    },
    {
        "id": "bnd-bound-record-without-check",
        "invariants": ["BND-03"],
        "path": STORE,
        "old": "if not economic_bound(policy):",
        "new": "if False:",
        "tests": [FAILING_BOUND],
        "why": "the coordinator signs a bound record for a policy whose inequality fails.",
    },
    {
        "id": "bnd-bound-record-in-test-mode",
        "invariants": ["BND-03"],
        "path": STORE,
        "old": 'if policy.ledger_mode != "production":',
        "new": "if False:",
        "tests": [TEST_MODE_RECORD],
        "why": "a test-mode run gets a production economic-bound record.",
    },
    {
        "id": "bnd-admin-route-open-bootstrap",
        "invariants": ["BND-05"],
        "path": APP,
        "old": "async def bootstrap_v2(run_id: str, request: Request, authorization: Auth = None) "
        "-> Any:\n        admin(authorization)\n",
        "new": "async def bootstrap_v2(run_id: str, request: Request, authorization: Auth = None) "
        "-> Any:\n",
        "tests": [ADMIN_ROUTES],
        "why": "the production bootstrap route stops checking the operator bearer.",
    },
    {
        "id": "bnd-empty-operator-token-accepted",
        "invariants": ["BND-05"],
        "path": APP,
        "old": "return token or None",
        "new": "return token",
        "tests": [BEARER_REREAD],
        "why": "an empty token file configures an operator bearer that anyone can present.",
    },
    {
        "id": "bnd-genesis-signed-by-owner",
        "invariants": ["BND-05"],
        "path": STORE,
        "old": 'self._authenticated_v2(run_id, raw, "TestGenesis", self._coord().ss58)',
        "new": 'self._authenticated_v2(run_id, raw, "TestGenesis", self.owner_hotkey or "")',
        "tests": [COORDINATOR_ONLY],
        "why": "the genesis envelope may be signed by the owner instead of the coordinator.",
    },
    {
        "id": "bnd-authority-signature-unchecked",
        "invariants": ["BND-05", "BND-10"],
        "path": ESCROW,
        "old": "or not verify(\n            decode_hotkey(self.policy.reward_authority),",
        "new": "or False and verify(\n            decode_hotkey(self.policy.reward_authority),",
        "tests": [INNER_SIGNATURE, BOOTSTRAP_REFUSED],
        "why": "the inner origin-authority signature is never verified, only the run id.",
    },
    {
        "id": "bnd-signed-replay-key-ignored",
        "invariants": ["BND-09"],
        "path": STORE,
        "old": "if known is not None and known[0] != digest:",
        "new": "if False:",
        "tests": [STORE_REPLAY_KEY],
        "why": "a second coordinator-signed body of the same kind passes the store's nonce check.",
    },
    {
        "id": "bnd-bootstrap-total-not-conserved",
        "invariants": ["BND-10"],
        "path": MESSAGES,
        "old": (
            "if sum(o.units for o in self.origins) != self.total_units:\n"
            '            raise ValueError("bootstrap'
        ),
        "new": 'if False:\n            raise ValueError("bootstrap',
        "tests": [T + "test_a_bootstrap_conserves_its_total_and_its_origins_are_distinct"],
        "why": "a bootstrap may state a total that differs from the sum of its origins.",
    },
    {
        "id": "bnd-policy-object-unverified",
        "invariants": ["BND-04"],
        "path": "src/hypertrain/data/store.py",
        "old": "if got != sha:",
        "new": "if False:",
        "tests": [OBJECT_EDITED],
        "why": "the object store returns bytes that no longer hash to the key the manifest pins.",
    },
    {
        "id": "bnd-economics-object-not-validated-at-creation",
        "invariants": ["BND-17"],
        "path": STORE,
        "old": '("economics_policy_hash", EconomicsPolicyV2),\n',
        "new": "",
        "tests": [MISSING_POLICY],
        "why": "run creation no longer loads the economics policy it pins before the insert.",
    },
    {
        "id": "bnd-roster-weight-unchecked",
        "invariants": ["BND-07"],
        "path": STORE,
        "old": "if entry.eligible_weight != 4194304:",
        "new": "if False:",
        "tests": [ROSTER_GATES],
        "why": "a round opens with a roster entry whose weight ceiling is not Q/4.",
    },
    {
        "id": "bnd-roster-audit-rate-unchecked",
        "invariants": ["BND-07"],
        "path": STORE,
        "old": (
            "entry.q_i,\n                ) != (r.admission_id, r.coldkey, r.state, f32hex(1.0)):"
        ),
        "new": ") != (r.admission_id, r.coldkey, r.state):",
        "tests": [ROSTER_GATES],
        "why": "a round opens with a roster entry whose audit probability is below one.",
    },
    {
        "id": "bnd-audit-skips-last-roster-entry",
        "invariants": ["BND-07"],
        "path": STORE,
        "old": "for entry, identity, job_id, _commit in missing:",
        "new": "for entry, identity, job_id, _commit in missing[:-1]:",
        "tests": [LEASES],
        "why": "the last roster entry of a round gets no audit lease and is never audited.",
    },
    {
        "id": "bnd-payable-includes-collateral",
        "invariants": ["BND-12"],
        "path": ESCROW,
        "old": "AND o.hotkey IS NOT NULL ",
        "new": "",
        "tests": [BOOTSTRAP_WEIGHTS, GENESIS_COLLATERAL],
        "why": "units issued without an earning hotkey become payable emission.",
    },
    {
        "id": "bnd-bootstrap-origin-earns",
        "invariants": ["BND-12"],
        "path": ESCROW,
        "old": "self._issue(o, None, None)  # hotkey NULL",
        "new": "self._issue(o, None, o.owner)  # hotkey NULL",
        "tests": [BOOTSTRAP_WEIGHTS],
        "why": "bootstrap origins carry an earning hotkey, so the issuer's collateral pays out.",
    },
    {
        "id": "bnd-bootstrap-cap-inclusive",
        "invariants": ["BND-11"],
        "path": ESCROW,
        "old": "+ bootstrap.total_units\n                > self.policy.max_total_issuance",
        "new": "+ bootstrap.total_units\n                >= self.policy.max_total_issuance",
        "tests": [CAP_PROPERTY, BOOTSTRAP + "test_bootstrap_exactly_at_total_issuance_is_accepted"],
        "why": "a bootstrap that exactly fills the cap is refused.",
    },
    {
        "id": "bnd-reward-cap-inclusive",
        "invariants": ["BND-11"],
        "path": ESCROW,
        "old": "+ reward.budget_units\n                > self.policy.max_total_issuance",
        "new": "+ reward.budget_units\n                >= self.policy.max_total_issuance",
        "tests": [CAP_PROPERTY],
        "why": "a live reward that exactly fills the cap is refused.",
    },
    {
        "id": "bnd-bootstrap-ignores-pending-reservation",
        "invariants": ["BND-11"],
        "path": ESCROW,
        "old": "\n                + pending * self.policy.round_reward_units",
        "new": "",
        "tests": [BOOTSTRAP + "test_bootstrap_counts_pending_shadow_reservation_against_cap"],
        "why": "a bootstrap ignores the shadow reservation that already holds a round of the cap.",
    },
    {
        "id": "bnd-live-reward-ignores-pending-reservation",
        "invariants": ["BND-11"],
        "path": ESCROW,
        "old": "self.balances().issued + pending_units + reward.budget_units",
        "new": "self.balances().issued + reward.budget_units",
        "tests": [
            "tests/ledger/test_escrow_v2.py::test_live_issuance_preserves_pending_shadow_headroom"
        ],
        "why": "a live reward ignores a shadow reservation that already holds a round of the cap.",
    },
    {
        "id": "bnd-shadow-reservation-ignores-issued",
        "invariants": ["BND-11"],
        "path": ESCROW,
        "old": "self.balances().issued + self.policy.round_reward_units\n",
        "new": "self.policy.round_reward_units\n",
        "tests": ["tests/ledger/test_escrow_v2.py::test_shadow_and_live_share_pinned_issuance_cap"],
        "why": "a shadow reservation ignores what the ledger already issued against the cap.",
    },
    {
        "id": "bnd-split-remainder-to-everyone",
        "invariants": ["BND-20"],
        "path": ESCROW,
        "old": "for h in ordered[:remainder]:",
        "new": "for h in ordered:",
        "tests": [SPLIT_PROPERTY],
        "why": "the rounding remainder goes to every worker: a round issues more than its budget.",
    },
    {
        "id": "bnd-split-rounded-not-floored",
        "invariants": ["BND-20"],
        "path": ESCROW,
        "old": "units = {h: reward.budget_units * work[h][1] // total for h in ordered}",
        "new": "units = {h: round(reward.budget_units * work[h][1] / total) for h in ordered}",
        "tests": [SPLIT_PROPERTY],
        "why": "shares are rounded to nearest instead of floored, so the remainder is mis-sized.",
    },
    {
        "id": "bnd-allocation-hash-unchecked",
        "invariants": ["BND-20"],
        "path": ESCROW,
        "old": "or reward.allocation_hash != (",
        "new": "or False and reward.allocation_hash != (",
        "tests": [OTHER_SPLIT],
        "why": "the ledger accepts a reward whose stated split differs from the one it recomputed.",
    },
    {
        "id": "bnd-round-budget-unchecked",
        "invariants": ["BND-20"],
        "path": ESCROW,
        "old": "if reward.budget_units != self.policy.round_reward_units:",
        "new": "if False:",
        "tests": [ROUND_BUDGET],
        "why": "a reward may state a budget other than the round reward of the policy.",
    },
    {
        "id": "bnd-bls-beacon-optional",
        "invariants": ["BND-13"],
        "path": "src/hypertrain/challenge/admission.py",
        "old": 'self.escrow.policy.ledger_mode == "production" and not b.bls_verified',
        "new": 'False and self.escrow.policy.ledger_mode == "production" and not b.bls_verified',
        "tests": [ECONOMICS + "test_bootstrap_route_requires_bls_beacon"],
        "why": "a production bootstrap is accepted against a beacon round no one verified.",
    },
    {
        "id": "bnd-qualification-signer-unchecked",
        "invariants": ["BND-18"],
        "path": STORE,
        "old": "or env.signer != self.owner_hotkey\n",
        "new": "or False\n",
        "tests": [QUALIFICATION],
        "why": "a CUDA qualification signed by any key, not the owner, builds production services.",
    },
    {
        "id": "bnd-production-without-qualification",
        "invariants": ["BND-13", "BND-18"],
        "path": STORE,
        "old": (
            'if econ.ledger_mode != "test":\n'
            '                    raise ChallengeError(409, "production'
        ),
        "new": ('if False:\n                    raise ChallengeError(409, "production'),
        "tests": [QUALIFICATION, ECONOMICS + "test_bootstrap_route_refuses_cpu_backend"],
        "why": "a production run without a qualification record falls back to the CPU backend.",
    },
    {
        "id": "bnd-backend-unfrozen",
        "invariants": ["BND-18"],
        "path": STORE,
        "old": "if frozen is not None and json.loads(frozen[0]) != selection:",
        "new": "if False:",
        "tests": [FROZEN_BACKEND],
        "why": "the execution backend of a run may change after services were built.",
    },
    {
        "id": "bnd-qualification-after-services",
        "invariants": ["BND-18"],
        "path": STORE,
        "old": "if (\n                manifest.run_id() in self._services_v2",
        "new": "if False and (\n                manifest.run_id() in self._services_v2",
        "tests": [
            "tests/challenge/test_backend_authority_v2.py::"
            "test_existing_cpu_contract_cannot_promote_after_services"
        ],
        "why": "a CUDA qualification may be installed after the run's services already exist.",
    },
    {
        "id": "bnd-cpu-summary-qualifies",
        "invariants": ["BND-18"],
        "path": STORE,
        # The summary of a CPU run is refused twice, by its backend and, since DET-01, by the
        # absence of the environment it declares: this reverts the first and lets a missing
        # declaration pass.
        "old": (
            '                    if summary.get("backend") != "cuda":\n'
            "                        raise ChallengeError("
            '403, "CPU artifacts cannot qualify CUDA execution")\n'
            '                    declared = summary.get("env")\n'
            "                    if not isinstance(declared, dict) or "
            "observation_divergence(env_pin, declared):\n"
        ),
        "new": (
            '                    declared = summary.get("env")\n'
            "                    if isinstance(declared, dict) and "
            "observation_divergence(env_pin, declared):\n"
        ),
        "tests": [
            "tests/challenge/test_backend_authority_v2.py::"
            "test_invalid_reviewed_authority_rejects_without_install[cpu_summary]"
        ],
        "why": "CPU artifacts are accepted as the evidence that qualifies CUDA execution.",
    },
    {
        "id": "bnd-charge-accepts-zero-units",
        "invariants": ["BND-21"],
        "path": STORE,
        "old": "or units < 1",
        "new": "or units < 0",
        "tests": [CHARGE_BOUNDS],
        "why": "a zero-unit charge is recorded, consuming an attempt for free.",
    },
    {
        "id": "bnd-charge-budget-per-round",
        "invariants": ["BND-21"],
        "path": STORE,
        "old": 'if prior["kind"] == kind:',
        "new": 'if prior["kind"] == kind and prior["w"] == w:',
        "tests": [CHARGE_BUDGET],
        "why": "the work budget is summed per round instead of over the whole run.",
    },
    {
        "id": "bnd-charge-deadline-inclusive",
        "invariants": ["BND-21"],
        "path": STORE,
        "old": 'if self._now(self._db) > reservation["deadline"]:',
        "new": 'if self._now(self._db) >= reservation["deadline"]:',
        "tests": [CHARGE_DEADLINE],
        "why": "a charge at the reservation deadline is refused one round early.",
    },
    {
        "id": "bnd-profile-admitted-after-open",
        "invariants": ["BND-22"],
        "path": STORE,
        "old": "AND kind='round' LIMIT 1\", (run_id,)",
        "new": "AND kind='no-such-kind' LIMIT 1\", (run_id,)",
        "tests": [PROFILE_BEFORE_OPEN],
        "why": "the capacity profile may be admitted after a round already opened.",
    },
    {
        "id": "bnd-profile-receipt-round-unbound",
        "invariants": ["BND-22"],
        "path": STORE,
        "old": "(0, self._now(self._db), digest)",
        "new": "(model.w, self._now(self._db), digest)",
        "tests": [PROFILE_ONCE],
        "why": "an owner receipt for a round other than 0 admits the capacity profile.",
    },
    {
        "id": "bnd-profile-replaceable",
        "invariants": ["BND-22"],
        "path": STORE,
        "old": 'if stored["hash"] != digest or stored["receipt"] != env.model_dump(mode="json"):',
        "new": 'if stored["hash"] != digest:',
        "tests": [PROFILE_ONCE],
        "why": "a second owner receipt for the same profile is accepted instead of refused.",
    },
    {
        "id": "bnd-profile-code-unbound",
        "invariants": ["BND-22"],
        "path": STORE,
        "old": (
            "profile != self._service_profile_v2(run_id)\n"
            "            or canonicalize(profile.model_dump"
        ),
        "new": "canonicalize(profile.model_dump",
        "tests": [PROFILE_CODE],
        "why": "a stored profile keeps governing after the code it was bound to changed.",
    },
    {
        "id": "bnd-heavy-gate-open",
        "invariants": ["BND-23"],
        "path": STORE,
        "old": "if heavy and estimate is not None:",
        "new": "if False:",
        "tests": [
            CAPACITY + "test_heavy32_execution_remains_blocked_when_only_accounting_admitted"
        ],
        "why": "an admitted capacity profile lets a 32-worker round run its heavy path.",
    },
    {
        "id": "bnd-byte-limit-off-by-one",
        "invariants": ["BND-23"],
        "path": STORE,
        "old": "if len(raw) > limit:",
        "new": "if len(raw) >= limit:",
        "tests": [CAPACITY + "test_actual_byte_limits_reject_when_one_byte_over_profile"],
        "why": "an object of exactly the profile's byte limit is refused (the bound moves by one).",
    },
    {
        "id": "bnd-roster-above-16-without-profile",
        "invariants": ["BND-23"],
        "path": STORE,
        "old": "if n <= 16 and not admitted:",
        "new": "if not admitted:",
        "tests": [
            CAPACITY + "test_bad_roster_rejects_before_stateallocation_"
            "when_admission_missing_or_count_excess"
        ],
        "why": "a roster above 16 is accepted without any admitted capacity profile.",
    },
]

MUTANTS += [
    {
        "id": "bnd-bootstrap-duplicate-origin-allowed",
        "invariants": ["BND-10"],
        "path": MESSAGES,
        "old": (
            "        if len({o.origin_id for o in self.origins}) != len(self.origins):\n"
            '            raise ValueError("duplicate bootstrap origin")'
        ),
        "new": '        if False:\n            raise ValueError("duplicate bootstrap origin")',
        "tests": [T + "test_a_bootstrap_conserves_its_total_and_its_origins_are_distinct"],
        "why": "a bootstrap may repeat an origin id whose units still add up to its total.",
    },
    {
        "id": "bnd-round-reward-relation-one-sided",
        "invariants": ["BND-04", "BND-20"],
        "path": ESCROW,
        "old": "if policy.round_reward_units != manifest.training.budget.epochs_per_round",
        "new": "if policy.round_reward_units > manifest.training.budget.epochs_per_round",
        "tests": [
            T
            + "test_the_ledger_refuses_a_round_reward_that_is_not_epochs_per_round_times_a_million"
        ],
        "why": "a policy that pays less per round than its manifest's epochs justify is accepted.",
    },
    {
        "id": "bnd-charge-operation-length-in-chars",
        "invariants": ["BND-21"],
        "path": STORE,
        "old": "or len(operation.encode()) > 256",
        "new": "or len(operation) > 256",
        "tests": [CHARGE_BOUNDS],
        "why": "the 256-byte bound on a charge's operation name counts characters, not bytes.",
    },
]
