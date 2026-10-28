"""Mutants of the shadow-reward credit path (package shadow of the ledger-invariants lot).

Each entry is one small regression of src/hypertrain that the named tests must catch. See
scripts/check_ledger_invariant_mutants.py for the schema.
"""

from __future__ import annotations

LEDGER = "src/hypertrain/ledger/escrow_v2.py"
STORE = "src/hypertrain/challenge/store.py"
ADMISSION = "src/hypertrain/challenge/admission.py"
APP = "src/hypertrain/challenge/app.py"


def node(module: str, name: str, parameter: str = "") -> str:
    suffix = f"[{parameter}]" if parameter else ""
    return f"tests/ledger_invariants/{module}.py::{name}{suffix}"


def ledger(name: str, parameter: str = "") -> str:
    return node("test_shadow", name, parameter)


def service(name: str, parameter: str = "") -> str:
    return node("test_shadow_2", name, parameter)


# Ledger half (test_shadow.py).
CREDITED_ONCE = "test_a_shadow_reward_is_credited_once_however_often_it_is_delivered"
REOPENED_RETURNS = "test_a_reopened_ledger_returns_the_stored_receipt_and_does_not_issue_again"
TABLE_REFUSES = (
    "test_the_shadow_table_itself_refuses_a_second_row_for_an_ordinal_a_reservation_or_an_origin"
)
ONE_SLOT = "test_reservations_take_one_pending_slot_and_the_ordinals_zero_to_eleven_in_order"
RANDOM_LAWS = "test_random_shadow_operations_keep_every_ledger_law"
DERIVED = "test_a_signed_shadow_reward_that_differs_from_what_the_ledger_derives_is_refused"
OVER_BUDGET = "test_an_over_budget_reward_is_refused_even_when_its_allocation_agrees_with_it"
RESERVED_OWNER = "test_shadow_work_is_credited_to_the_reserved_owner_and_assignment_only"
OWN_SIGNATURE = "test_a_shadow_reward_needs_its_own_authority_signature"
OTHER_DOMAIN = "test_a_reward_is_refused_with_the_other_domains_evidence_whichever_way_round"
PAYABLE = "test_a_matured_shadow_origin_is_never_payable_while_a_live_origin_of_the_same_hotkey_is"
SETTLEMENT_GATE = "test_shadow_units_stay_pending_unless_the_shadow_settlement_is_clean_and_due"
NO_SETTLEMENT = "test_a_ledger_without_a_shadow_settlement_matures_nothing"
REOPEN_CHECKS = "test_reopening_a_ledger_checks_every_credited_shadow_row_against_its_reservation"
CAP_HOLDS = "test_the_cap_holds_pending_reservations_against_live_issuance_and_new_reservations"

# Service half (test_shadow_2.py).
FIRST_TRIAL = "test_a_new_reservation_needs_a_matching_clean_dispute_free_first_trial"
CUSTODY = "test_a_reservation_needs_two_byte_exact_retained_executions_of_the_service_reference"
EVIDENCE = "test_acceptance_refuses_evidence_that_is_not_bound_to_anchor_auditor_and_receipt"
NOT_CREDITED = "test_a_banned_disputed_or_non_matching_identity_is_not_credited"
NEVER_MATURES = "test_credit_of_a_banned_disputed_or_non_matching_identity_never_matures"
BEARER = "test_the_shadow_routes_need_the_operator_bearer_before_they_read_the_body"
ROUTES = "test_only_the_two_admin_routes_reach_shadow_credit_and_none_reaches_operator_custody"
RETAIN_GUARD = "test_accepted_execution_custody_cannot_be_retained_outside_the_test_cpu_ledger"
OPERATOR = "test_a_production_reservation_is_refused_unless_the_operator_authority_covers_it"
NO_OPERATOR = "test_a_production_reservation_without_any_operator_record_is_refused"
RESTART = (
    "test_a_restart_between_acceptance_and_retry_returns_the_stored_response_and_issues_nothing"
)
STAGED = "test_staged_trial_inputs_are_created_once_and_never_overwritten"
ROUTE_CUSTODY = "test_a_settled_first_trial_is_reserved_with_its_custody_record_and_issues_nothing"
ATOMIC_CUSTODY = "test_a_reservation_whose_custody_record_cannot_be_stored_takes_no_ordinal"
TEST_AUTHORITY = "test_a_test_ledger_reservation_binds_the_policy_digest_not_an_operator_record"
LAPSING = "test_a_commit_envelope_that_lapses_after_the_reservation_cannot_strand_the_credit"
DEFAULT_FLOW = "test_the_reference_the_service_runs_retains_no_accepted_execution_by_default"
LOST_RESPONSE = (
    "tests/challenge/test_service_network_v2.py::test_shadow_acceptance_issues_once_and_recovers_"
    "lost_response"
)
INTERRUPTED = (
    "tests/challenge/test_service_network_v2.py::"
    "test_shadow_acceptance_failure_rolls_back_and_dispute_blocks_maturity"
)

MUTANTS = [
    {
        "id": "shd01-authority-signature-in-the-operation-request",
        "invariants": ["SHD-01"],
        "path": LEDGER,
        "old": 'request = reward.model_dump_json(exclude={"authority_sig"})',
        "new": "request = reward.model_dump_json()",
        "tests": [ledger(CREDITED_ONCE), ledger(REOPENED_RETURNS)],
        "why": "a retry signed again no longer matches the stored request and is refused.",
    },
    {
        "id": "shd01-replay-does-not-return-the-stored-receipt",
        "invariants": ["SHD-01", "SHD-13"],
        "path": LEDGER,
        "old": (
            "            old = self._existing(operation, request, server=True)\n"
            "            if old is not None:\n"
            "                return old\n"
            "            if origins is None:"
        ),
        "new": (
            "            old = self._existing(operation, request, server=True)\n"
            "            if origins is None:"
        ),
        "tests": [ledger(CREDITED_ONCE), ledger(REOPENED_RETURNS)],
        "why": (
            "a delivered reward is ordered and validated again instead of answered "
            "from the journal."
        ),
    },
    {
        "id": "shd01-reservation-hash-not-unique",
        "invariants": ["SHD-01"],
        "path": LEDGER,
        "old": "reservation_hash TEXT UNIQUE, origin TEXT UNIQUE);",
        "new": "reservation_hash TEXT, origin TEXT UNIQUE);",
        "tests": [ledger(TABLE_REFUSES)],
        "why": "the schema accepts a second credit row for one reservation.",
    },
    {
        "id": "shd02-pending-slot-not-enforced",
        "invariants": ["SHD-02", "SHD-15"],
        "path": LEDGER,
        "old": (
            "            if pending is not None:\n"
            '                raise EscrowError("SHADOW_RESERVATION_PENDING")'
        ),
        "new": (
            '            if False:\n                raise EscrowError("SHADOW_RESERVATION_PENDING")'
        ),
        "tests": [ledger(ONE_SLOT), ledger(RANDOM_LAWS)],
        "why": (
            "a second reservation is taken while one is pending, so the cap headroom is held twice."
        ),
    },
    {
        "id": "shd02-thirteenth-ordinal-allowed",
        "invariants": ["SHD-02"],
        "path": LEDGER,
        "old": "if ordinal >= self.policy.shadow_bootstrap_rounds:",
        "new": "if ordinal > self.policy.shadow_bootstrap_rounds:",
        "tests": [ledger(ONE_SLOT)],
        "why": "the bootstrap bound moves from twelve ordinals to thirteen.",
    },
    {
        "id": "shd12-reservation-accepts-an-open-transaction",
        "invariants": ["SHD-12"],
        "path": LEDGER,
        "old": (
            "        if self.db.in_transaction:\n"
            '            raise EscrowError("SHADOW_RESERVATION_REQUIRES_COMMIT")\n'
        ),
        "new": "",
        "tests": [
            "tests/ledger/test_escrow_v2.py::test_shadow_reservation_requires_durable_outer_commit"
        ],
        "why": "an ordinal is handed out from a transaction that can still roll back.",
    },
    {
        "id": "shd12-accepted-marker-never-written",
        "invariants": ["SHD-12", "SHD-02"],
        "path": LEDGER,
        "old": (
            "                        \"AND kind='shadow-reservation' AND id=?\",\n"
            "                        (\n"
            "                            canonicalize("
        ),
        "new": (
            "                        \"AND kind='shadow-reservation-x' AND id=?\",\n"
            "                        (\n"
            "                            canonicalize("
        ),
        "tests": [ledger(ONE_SLOT), ledger(RANDOM_LAWS)],
        "why": (
            "a credited reservation stays pending, so it holds the single slot and "
            "the cap for ever."
        ),
    },
    {
        "id": "shd14-authority-signature-not-verified",
        "invariants": ["SHD-14"],
        "path": LEDGER,
        "old": "if model.run_id != self.run_id or not verify(",
        "new": "if model.run_id != self.run_id or False and not verify(",
        "tests": [ledger(OWN_SIGNATURE, "zeros")],
        "why": (
            "an origin authority message is accepted without a valid reward-authority signature."
        ),
    },
    {
        "id": "shd14-authority-message-without-the-class-name",
        "invariants": ["SHD-14"],
        "path": LEDGER,
        "old": (
            '        b"hypertrain/origin/2|"\n'
            "        + type(model).__name__.encode()\n"
            '        + b"|"\n'
        ),
        "new": '        b"hypertrain/origin/2|"\n        + b"|"\n',
        "tests": [ledger(OWN_SIGNATURE, "live reward class name")],
        "why": "the origin authority message no longer separates the shadow and the live reward.",
    },
    {
        "id": "shd14-round-budget-not-checked",
        "invariants": ["SHD-14"],
        "path": LEDGER,
        "old": (
            "        if reward.budget_units != self.policy.round_reward_units:\n"
            '            raise EscrowError("ROUND_BUDGET")'
        ),
        "new": '        if False:\n            raise EscrowError("ROUND_BUDGET")',
        "tests": [ledger(OVER_BUDGET)],
        "why": "a signed budget above the policy round reward is credited.",
    },
    {
        "id": "shd14-origin-ids-taken-from-the-signed-body",
        "invariants": ["SHD-14"],
        "path": LEDGER,
        "old": (
            "if reward.origin_ids != [o.origin_id for o in origins] or reward.allocation_hash != ("
        ),
        "new": "if reward.allocation_hash != (",
        "tests": [ledger(DERIVED, "origin ids")],
        "why": (
            "the origin ids a signer lists are no longer compared with the ones the ledger derived."
        ),
    },
    {
        "id": "shd14-credit-owner-not-bound-to-the-reservation",
        "invariants": ["SHD-14"],
        "path": LEDGER,
        "old": "digest(c) != reservation.commit_hash or item.owner != reservation.coldkey",
        "new": "digest(c) != reservation.commit_hash",
        "tests": [ledger(RESERVED_OWNER, "owner changed after the reservation")],
        "why": "units go to whoever owns the hotkey now, not to the coldkey the reservation named.",
    },
    {
        "id": "shd08-assignment-not-compared-with-the-reservation",
        "invariants": ["SHD-08"],
        "path": LEDGER,
        "old": (
            "                or item.sample_ids\n"
            "                != (\n"
            "                    self.shadow_assignment_of(reservation)\n"
            "                    if reservation is not None "
            "and self.shadow_assignment_of is not None\n"
            "                    else self.assignment_of(final.w, c.hotkey)\n"
            "                )\n"
        ),
        "new": "",
        "tests": [ledger(RESERVED_OWNER, "another assignment")],
        "why": "work on samples other than the service-derived assignment is credited.",
    },
    {
        "id": "shd16-live-reward-accepts-shadow-evidence",
        "invariants": ["SHD-16"],
        "path": LEDGER,
        "old": (
            "            case RewardFinalize():\n"
            "                if evidence.shadow:\n"
            '                    raise EscrowError("REWARD_DOMAIN")'
        ),
        "new": (
            "            case RewardFinalize():\n"
            '                if False:\n                    raise EscrowError("REWARD_DOMAIN")'
        ),
        "tests": [ledger(OTHER_DOMAIN)],
        "why": "a live round reward is accepted on evidence built for the shadow domain.",
    },
    {
        "id": "shd17-shadow-origin-payable-as-emission",
        "invariants": ["SHD-17"],
        "path": LEDGER,
        "old": '            "AND o.origin NOT IN (SELECT origin FROM escrow_shadow_finalized) "\n',
        "new": "",
        "tests": [ledger(PAYABLE), ledger(RANDOM_LAWS)],
        "why": "a matured shadow origin with a hotkey is paid to that hotkey as live emission.",
    },
    {
        "id": "shd18-maturity-ignores-an-unresolved-dispute",
        "invariants": ["SHD-18"],
        "path": LEDGER,
        "old": (
            "                        status.unresolved\n"
            '                        or status.outcome == "FRAUD"\n'
            "                        or now_beacon < status.release_beacon\n"
            "                        or finality is None"
        ),
        "new": (
            '                        status.outcome == "FRAUD"\n'
            "                        or now_beacon < status.release_beacon\n"
            "                        or finality is None"
        ),
        "tests": [ledger(SETTLEMENT_GATE, "unresolved dispute")],
        "why": "collateral of an identity under an open dispute matures.",
    },
    {
        "id": "shd18-maturity-ignores-a-fraud-outcome",
        "invariants": ["SHD-18"],
        "path": LEDGER,
        "old": (
            "                        status.unresolved\n"
            '                        or status.outcome == "FRAUD"\n'
            "                        or now_beacon < status.release_beacon\n"
            "                        or finality is None"
        ),
        "new": (
            "                        status.unresolved\n"
            "                        or now_beacon < status.release_beacon\n"
            "                        or finality is None"
        ),
        "tests": [ledger(SETTLEMENT_GATE, "fraud outcome")],
        "why": "collateral of an identity whose dispute ended in fraud matures.",
    },
    {
        "id": "shd18-maturity-without-a-shadow-settlement",
        "invariants": ["SHD-18"],
        "path": LEDGER,
        "old": (
            "                        if self.shadow_settlement is None:\n"
            '                            raise EscrowError("SHADOW_AUTHORITY_UNAVAILABLE")\n'
        ),
        "new": (
            "                        if False:\n"
            '                            raise EscrowError("SHADOW_AUTHORITY_UNAVAILABLE")\n'
        ),
        "tests": [ledger(NO_SETTLEMENT)],
        "why": "a ledger built without the shadow settlement fails with a type error, not closed.",
    },
    {
        "id": "shd13-reopen-does-not-compare-the-stored-receipt",
        "invariants": ["SHD-13"],
        "path": LEDGER,
        "old": '            if event is None or json.loads(event["receipt"]) != data["accepted"]:',
        "new": "            if event is None:",
        "tests": [ledger(REOPEN_CHECKS, "stored receipt")],
        "why": "a restart accepts a reservation whose stored acceptance receipt was altered.",
    },
    {
        "id": "shd13-reopen-does-not-compare-the-trial-epoch",
        "invariants": ["SHD-13"],
        "path": LEDGER,
        "old": '                or binding.trial_epoch != row["trial_epoch"]\n',
        "new": "",
        "tests": [ledger(REOPEN_CHECKS, "trial epoch")],
        "why": "a restart accepts a credited row whose trial epoch disagrees with its reservation.",
    },
    {
        "id": "shd15-live-issuance-ignores-pending-reservations",
        "invariants": ["SHD-15"],
        "path": LEDGER,
        "old": "self.balances().issued + pending_units + reward.budget_units",
        "new": "self.balances().issued + reward.budget_units",
        "tests": [ledger(CAP_HOLDS)],
        "why": "a live round takes the headroom a pending shadow reservation holds.",
    },
    {
        "id": "shd03-second-clean-trial-reserved",
        "invariants": ["SHD-03"],
        "path": STORE,
        "old": (
            "                                current.clean_count != 1\n"
            "                                or first != final.w\n"
        ),
        "new": "                                first != final.w\n",
        "tests": [service(FIRST_TRIAL, "a second clean trial")],
        "why": (
            "an admission that already finalized a second trial gets a reservation for its first."
        ),
    },
    {
        "id": "shd03-later-trial-of-the-admission-reserved",
        "invariants": ["SHD-03", "SHD-02"],
        "path": STORE,
        "old": "                                or first != final.w\n",
        "new": "",
        "tests": [service(FIRST_TRIAL, "an earlier finalized trial")],
        "why": "a trial that is not the admission's first finalized one obtains an ordinal.",
    },
    {
        "id": "shd03-pending-dispute-ignored-by-the-reservation",
        "invariants": ["SHD-03"],
        "path": STORE,
        "old": (
            "                                or current.pending_dispute\n"
            "                                or db.execute("
        ),
        "new": "                                or db.execute(",
        "tests": [service(FIRST_TRIAL, "a pending dispute flag")],
        "why": "an admission under a pending dispute is reserved collateral.",
    },
    {
        "id": "shd03-mismatching-original-trial-reserved",
        "invariants": ["SHD-03"],
        "path": STORE,
        "old": '                            if result["outcome"] != "MATCH":',
        "new": "                            if False:",
        "tests": [service(FIRST_TRIAL, "a mismatched original trial")],
        "why": "a trial that did not match the service reference obtains a reservation.",
    },
    {
        "id": "shd05-reference-and-miner-executions-not-ordered",
        "invariants": ["SHD-05"],
        "path": STORE,
        "old": (
            '                    accepted["operation"] != operation\n'
            '                    or accepted["run_id"] != run_id'
        ),
        "new": '                    accepted["run_id"] != run_id',
        "tests": [service(CUSTODY, "reference and miner swapped")],
        "why": "the miner's execution can stand where the reference execution is required.",
    },
    {
        "id": "shd05-publication-manifest-not-compared",
        "invariants": ["SHD-05"],
        "path": STORE,
        "old": "                if actual_files != publication:",
        "new": "                if False:",
        "tests": [service(CUSTODY, "publication bytes changed")],
        "why": "retained publication bytes are no longer compared with the accepted file manifest.",
    },
    {
        "id": "shd05-publication-outside-the-store-accepted",
        "invariants": ["SHD-05"],
        "path": STORE,
        "old": (
            "if directory.is_symlink() or not directory.resolve().is_relative_to(\n"
            "                    self.state_dir.resolve()\n"
            "                ):"
        ),
        "new": "if directory.is_symlink():",
        "tests": [service(CUSTODY, "publication outside the store")],
        "why": "a publication kept outside the store's custody backs a reservation.",
    },
    {
        "id": "shd08-trial-nonce-not-compared-with-the-signed-challenge",
        "invariants": ["SHD-08"],
        "path": STORE,
        "old": '                or challenge.nonce != trial["nonce"]\n',
        "new": "",
        "tests": [service(CUSTODY, "nonce of the trial row")],
        "why": "the assignment nonce may differ from the one the service chose for the trial.",
    },
    {
        "id": "shd07-accepted-proof-not-compared-with-the-reference",
        "invariants": ["SHD-07"],
        "path": STORE,
        "old": "                or proof != reference\n",
        "new": "",
        "tests": [service(CUSTODY, "accepted proof differs from the reference")],
        "why": "a miner proof that differs from the service reference backs a reservation.",
    },
    {
        "id": "shd11-runtime-custody-read-as-accepted-execution",
        "invariants": ["SHD-11", "SHD-06"],
        "path": STORE,
        "old": 'accepted = self._record_v2(run_id, "shadow-execution", operation_hash)',
        "new": (
            "try:\n"
            '                    accepted = self._record_v2(run_id, "shadow-execution", '
            "operation_hash)\n"
            "                except ChallengeError:\n"
            '                    accepted = self._record_v2(run_id, "runtime-custody", '
            "operation_hash)"
        ),
        "tests": [
            service(
                "test_credit_needs_accepted_execution_records_and_private_runtime_custody_is_not_one"
            )
        ],
        "why": "a reader falls back to private runtime custody when no accepted execution exists.",
    },
    {
        "id": "shd09-genesis-anchor-not-checked",
        "invariants": ["SHD-09"],
        "path": STORE,
        "old": (
            "            or audit.anchor_hash\n"
            "            != self._shadow_anchor_v2("
            'job, binding.hotkey, Path(operations[0]["directory"]))\n'
        ),
        "new": "",
        "tests": [service(EVIDENCE, "anchor of another genesis")],
        "why": "an audit anchored on a genesis the service did not derive backs a credit.",
    },
    {
        "id": "shd09-auditor-need-not-be-pinned",
        "invariants": ["SHD-09"],
        "path": STORE,
        "old": "if auditor not in manifest.training.auditors or auditor == binding.hotkey:",
        "new": "if False:",
        "tests": [service(EVIDENCE, "receipt signed by a key that is not a pinned auditor")],
        "why": "a receipt signed by any key, the miner's included, backs a credit.",
    },
    {
        "id": "shd09-receipt-not-compared-with-the-service-records",
        "invariants": ["SHD-09", "SHD-08", "SHD-07"],
        "path": STORE,
        "old": "if receipt.body() != expected or (",
        "new": "if (",
        "tests": [service(EVIDENCE, "assignment of the receipt")],
        "why": "an auditor receipt that disagrees with the service's own records is accepted.",
    },
    {
        "id": "shd22-banned-identity-credited",
        "invariants": ["SHD-22", "SHD-18"],
        "path": STORE,
        "old": 'fraud = admission.state == "BANNED" or trial[0] == "FRAUD"',
        "new": 'fraud = trial[0] == "FRAUD"',
        "tests": [service(NOT_CREDITED, "banned"), service(NEVER_MATURES, "banned")],
        "why": "a banned identity is credited and its collateral matures.",
    },
    {
        "id": "shd22-pending-dispute-ignored-by-the-shadow-settlement",
        "invariants": ["SHD-22", "SHD-18"],
        "path": STORE,
        "old": 'unresolved = admission.pending_dispute or trial[0] != "MATCH"',
        "new": 'unresolved = trial[0] != "MATCH"',
        "tests": [
            service(NOT_CREDITED, "pending dispute flag"),
            service(NEVER_MATURES, "pending dispute flag"),
        ],
        "why": "an identity with a pending dispute is credited and its collateral matures.",
    },
    {
        "id": "shd24-reservation-route-without-the-operator-bearer",
        "invariants": ["SHD-24"],
        "path": APP,
        "old": (
            "        admin(authorization)\n"
            "        return await run(\n"
            "            store.for_run(run_id).shadow_reservation_v2"
        ),
        "new": "        return await run(\n            store.for_run(run_id).shadow_reservation_v2",
        "tests": [service(BEARER, "reservation-no authorization header")],
        "why": "anyone can reserve shadow ordinals.",
    },
    {
        "id": "shd24-reward-route-without-the-operator-bearer",
        "invariants": ["SHD-24"],
        "path": APP,
        "old": (
            "        admin(authorization)\n"
            "        return await run(\n"
            "            store.for_run(run_id).shadow_reward_v2"
        ),
        "new": "        return await run(\n            store.for_run(run_id).shadow_reward_v2",
        "tests": [service(BEARER, "reward-no authorization header")],
        "why": "anyone can issue shadow credit.",
    },
    {
        "id": "shd06-reservation-route-renamed-to-runtime-custody",
        "invariants": ["SHD-06", "SHD-11", "SHD-24"],
        "path": APP,
        "old": '@app.post("/v2/runs/{run_id}/admin/join/{admission_id}/shadow-reservation")',
        "new": '@app.post("/v2/runs/{run_id}/admin/join/{admission_id}/runtime-custody")',
        "tests": [service(ROUTES)],
        "why": "a route named for runtime custody appears next to the shadow routes.",
    },
    {
        "id": "shd06-default-reference-flow-retains-accepted-custody",
        "invariants": ["SHD-06"],
        "path": STORE,
        "old": "self, run_id: str, admission_id: str, *, retain_shadow_custody: bool = False",
        "new": "self, run_id: str, admission_id: str, *, retain_shadow_custody: bool = True",
        "tests": [service(DEFAULT_FLOW)],
        "why": "every route-driven reference run writes accepted shadow-execution records.",
    },
    {
        "id": "shd06-custody-writer-open-to-any-ledger-and-backend",
        "invariants": ["SHD-06"],
        "path": ADMISSION,
        "old": (
            '        if self.backend != "cpu" or self.escrow.policy.ledger_mode != "test":\n'
            '            raise AdmissionError("SHADOW_ACCEPTED_RUNTIME_CUSTODY_UNAVAILABLE")'
        ),
        "new": (
            "        if False:\n"
            '            raise AdmissionError("SHADOW_ACCEPTED_RUNTIME_CUSTODY_UNAVAILABLE")'
        ),
        "tests": [
            service(RETAIN_GUARD, "production"),
            service(RETAIN_GUARD, "cuda"),
        ],
        "why": "the seam that creates shadow-execution records works on production and CUDA runs.",
    },
    {
        "id": "shd10-identity-outside-the-signed-roster-reserved",
        "invariants": ["SHD-10"],
        "path": STORE,
        "old": 'if identity is None or trial["finalized_beacon"] > authority["expires_beacon"]:',
        "new": 'if trial["finalized_beacon"] > authority["expires_beacon"]:',
        "tests": [service(OPERATOR, "the identity is not in the signed roster")],
        "why": (
            "an identity absent from the owner's signed roster obtains a production reservation."
        ),
    },
    {
        "id": "shd10-trial-finalized-after-the-authority-expired-reserved",
        "invariants": ["SHD-10"],
        "path": STORE,
        "old": 'if identity is None or trial["finalized_beacon"] > authority["expires_beacon"]:',
        "new": "if identity is None:",
        "tests": [service(OPERATOR, "the trial finalized after the authority expired")],
        "why": "a trial finalized after the operator authority expired obtains a reservation.",
    },
    {
        "id": "shd10-production-reservation-uses-the-test-authority",
        "invariants": ["SHD-10"],
        "path": STORE,
        "old": (
            '            if escrow.policy.ledger_mode == "test":\n'
            "                economic_hash = escrow.policy.digest()"
        ),
        "new": "            if True:\n                economic_hash = escrow.policy.digest()",
        "tests": [service(NO_OPERATOR)],
        "why": (
            "a production ledger reserves under the synthetic test authority, no operator record."
        ),
    },
    {
        "id": "shd12-custody-record-not-written-with-the-reservation",
        "invariants": ["SHD-12"],
        "path": STORE,
        "old": (
            "                        if old is None:\n"
            "                            store._put_record_v2(\n"
            "                                run_id,\n"
            '                                "shadow-custody",'
        ),
        "new": (
            "                        if False:\n"
            "                            store._put_record_v2(\n"
            "                                run_id,\n"
            '                                "shadow-custody",'
        ),
        "tests": [service(ROUTE_CUSTODY)],
        "why": "an ordinal is returned for a reservation whose original custody was not recorded.",
    },
    {
        "id": "shd12-custody-record-written-after-the-reservation-commits",
        "invariants": ["SHD-12"],
        "path": STORE,
        "old": (
            "                        yield db\n"
            "                        if old is None:\n"
            "                            store._put_record_v2(\n"
            "                                run_id,\n"
            '                                "shadow-custody",\n'
            "                                semantic_key,\n"
            "                                {\n"
            '                                    "request": request.model_dump(mode="json"),\n'
            '                                    "accepted_at": now,\n'
            "                                },\n"
            "                            )\n"
            "\n"
            "            intake = ReservationEscrow(\n"
            "                self._db,\n"
            "                escrow.manifest,\n"
            "                escrow.policy,\n"
            "                auditors=escrow.auditors,\n"
            "                replay_finality=escrow.replay_finality,\n"
            "                settlement=escrow.settlement,\n"
            "                owner_of=escrow.owner_of,\n"
            "                assignment_of=escrow.assignment_of,\n"
            "            )\n"
            "            intake.mutex = self._lock\n"
            "            reserved = intake.reserve_shadow(\n"
            "                binding,\n"
            "                ShadowReservationEvidence(\n"
            "                    finalize=request.finalize,\n"
            "                    commit=request.commit,\n"
            "                    owner=row.coldkey,\n"
            "                    sample_ids=tuple(job.sample_ids),\n"
            "                    custody_hash=request.custody.digest(),\n"
            "                    reference_final_state_hash=final.final_theta_hash_w1,\n"
            "                ),\n"
            "            )\n"
        ),
        "new": (
            "                        yield db\n"
            "                        pass\n"
            "\n"
            "            intake = ReservationEscrow(\n"
            "                self._db,\n"
            "                escrow.manifest,\n"
            "                escrow.policy,\n"
            "                auditors=escrow.auditors,\n"
            "                replay_finality=escrow.replay_finality,\n"
            "                settlement=escrow.settlement,\n"
            "                owner_of=escrow.owner_of,\n"
            "                assignment_of=escrow.assignment_of,\n"
            "            )\n"
            "            intake.mutex = self._lock\n"
            "            reserved = intake.reserve_shadow(\n"
            "                binding,\n"
            "                ShadowReservationEvidence(\n"
            "                    finalize=request.finalize,\n"
            "                    commit=request.commit,\n"
            "                    owner=row.coldkey,\n"
            "                    sample_ids=tuple(job.sample_ids),\n"
            "                    custody_hash=request.custody.digest(),\n"
            "                    reference_final_state_hash=final.final_theta_hash_w1,\n"
            "                ),\n"
            "            )\n"
            "            if (\n"
            "                self._db.execute(\n"
            '                    "SELECT 1 FROM records_v2 WHERE run_id=? "\n'
            "                    \"AND kind='shadow-custody' AND id=?\",\n"
            "                    (run_id, semantic_key),\n"
            "                ).fetchone()\n"
            "                is None\n"
            "            ):\n"
            "                store._put_record_v2(\n"
            "                    run_id,\n"
            '                    "shadow-custody",\n'
            "                    semantic_key,\n"
            "                    {\n"
            '                        "request": request.model_dump(mode="json"),\n'
            '                        "accepted_at": now,\n'
            "                    },\n"
            "                )\n"
        ),
        "tests": [service(ATOMIC_CUSTODY)],
        "why": "a failure between the reservation and its custody record leaves an orphan ordinal.",
    },
    {
        "id": "shd10-test-ledger-authority-is-not-the-economics-policy",
        "invariants": ["SHD-10"],
        "path": STORE,
        "old": "                economic_hash = escrow.policy.digest()",
        "new": "                economic_hash = graph_hash",
        "tests": [service(TEST_AUTHORITY)],
        "why": "the test authority bound into a reservation stops being the policy digest.",
    },
    {
        "id": "shd13-original-custody-validated-at-the-current-beacon",
        "invariants": ["SHD-13", "SHD-18", "SHD-01"],
        "path": STORE,
        "old": '            accepted_at=record["accepted_at"],\n',
        "new": "",
        "tests": [service(LAPSING)],
        "why": "a miner-chosen expiry that lapses after the reservation strands the credit.",
    },
    {
        "id": "shd23-staged-trial-input-overwritten",
        "invariants": ["SHD-23"],
        "path": STORE,
        "old": "            if path.exists():\n                if path.read_bytes() != value:",
        "new": "            if False:\n                if path.read_bytes() != value:",
        "tests": [service(STAGED, "samples")],
        "why": "a staged input that changed on disk is silently replaced by newly derived bytes.",
    },
    {
        "id": "shd12-acceptance-outside-one-transaction",
        "invariants": ["SHD-12"],
        "path": STORE,
        "old": (
            "        with self._tx():\n"
            "            binding = self._shadow_binding_v2(run_id, request.reservation_hash)\n"
        ),
        "new": (
            "        if True:\n"
            "            binding = self._shadow_binding_v2(run_id, request.reservation_hash)\n"
        ),
        "tests": [INTERRUPTED],
        "why": (
            "issuance commits before the evidence and settlement records, so a "
            "failure leaves a half credit."
        ),
    },
    {
        "id": "shd13-accepted-response-not-stored",
        "invariants": ["SHD-13"],
        "path": STORE,
        "old": '{**record, "response": response}',
        "new": "record",
        "tests": [service(RESTART), LOST_RESPONSE],
        "why": "a lost reply cannot be redelivered: the retry finds evidence without a response.",
    },
    {
        "id": "shd01-shadow-origin-not-unique",
        "invariants": ["SHD-01"],
        "path": LEDGER,
        "old": "reservation_hash TEXT UNIQUE, origin TEXT UNIQUE);",
        "new": "reservation_hash TEXT UNIQUE, origin TEXT);",
        "tests": [ledger(TABLE_REFUSES)],
        "why": "the schema accepts two credit rows for one origin.",
    },
    {
        "id": "shd01-shadow-ordinal-not-unique",
        "invariants": ["SHD-01"],
        "path": LEDGER,
        "old": "ordinal INTEGER PRIMARY KEY, trial_epoch INTEGER, finalize_hash TEXT,",
        "new": "ordinal INTEGER, trial_epoch INTEGER, finalize_hash TEXT,",
        "tests": [ledger(TABLE_REFUSES)],
        "why": "the schema accepts two credit rows for one ordinal.",
    },
    {
        "id": "shd02-conflicting-request-returns-the-stored-reservation",
        "invariants": ["SHD-02"],
        "path": LEDGER,
        "old": (
            '                if old["digest"] != semantic:\n'
            '                    raise EscrowError("SHADOW_RESERVATION_CONFLICT")'
        ),
        "new": (
            "                if False:\n"
            '                    raise EscrowError("SHADOW_RESERVATION_CONFLICT")'
        ),
        "tests": [ledger(ONE_SLOT)],
        "why": "another request under a reserved key silently returns the stored reservation.",
    },
    {
        "id": "shd14-verdict-root-not-checked",
        "invariants": ["SHD-14"],
        "path": LEDGER,
        "old": (
            "        if sha256_hex(canonicalize(verdicts)) != reward.verdict_root:\n"
            '            raise EscrowError("VERDICT_ROOT")'
        ),
        "new": '        if False:\n            raise EscrowError("VERDICT_ROOT")',
        "tests": [ledger(DERIVED, "verdict root")],
        "why": "a reward naming other verdicts than the validated ones is credited.",
    },
    {
        "id": "shd15-last-round-under-the-cap-cannot-be-reserved",
        "invariants": ["SHD-15"],
        "path": LEDGER,
        "old": (
            "                self.balances().issued + self.policy.round_reward_units\n"
            "                > self.policy.max_total_issuance"
        ),
        "new": (
            "                self.balances().issued + self.policy.round_reward_units\n"
            "                >= self.policy.max_total_issuance"
        ),
        "tests": [ledger(CAP_HOLDS), ledger(RANDOM_LAWS)],
        "why": "the reservation bound refuses a round that still fits under the cap.",
    },
    {
        "id": "shd18-collateral-of-another-finality-matures",
        "invariants": ["SHD-18"],
        "path": LEDGER,
        "old": '                        or status.finality_hash != finality["finalize_hash"]\n',
        "new": "",
        "tests": [ledger(SETTLEMENT_GATE, "another finality")],
        "why": "a settlement of another finality releases the collateral.",
    },
    {
        "id": "shd18-collateral-matures-before-the-release-round",
        "invariants": ["SHD-18"],
        "path": LEDGER,
        "old": (
            "                        or now_beacon < status.release_beacon\n"
            "                        or finality is None\n"
        ),
        "new": "                        or finality is None\n",
        "tests": [ledger(SETTLEMENT_GATE, "release after now")],
        "why": "collateral matures before the settlement release round.",
    },
    {
        "id": "shd03-unresolved-pending-reference-ignored-by-the-reservation",
        "invariants": ["SHD-03"],
        "path": STORE,
        "old": (
            "                                or db.execute(\n"
            "                                    "
            '"SELECT 1 FROM admission_pending WHERE admission_id=? "\n'
            '                                    "AND resolved=0",\n'
            "                                    (admission_id,),\n"
            "                                ).fetchone()\n"
            "                                is not None\n"
        ),
        "new": "",
        "tests": [service(FIRST_TRIAL, "an unresolved pending reference")],
        "why": "an admission with an unresolved pending reference obtains a reservation.",
    },
    {
        "id": "shd05-symlink-in-a-retained-publication-accepted",
        "invariants": ["SHD-05"],
        "path": STORE,
        "old": (
            "                    if path.is_symlink() or not (path.is_file() or path.is_dir()):\n"
        ),
        "new": "                    if not (path.is_file() or path.is_dir()):\n",
        "tests": [service(CUSTODY, "special file in the publication")],
        "why": "a symlink inside a retained publication backs a reservation.",
    },
    {
        "id": "shd09-receipt-completed-after-the-evidence-time-accepted",
        "invariants": ["SHD-09"],
        "path": STORE,
        "old": "not binding.finalized_beacon <= receipt.completed_beacon <= now",
        "new": "not binding.finalized_beacon <= receipt.completed_beacon",
        "tests": [service(EVIDENCE, "completion after the evidence time")],
        "why": "a receipt completed after the time of the evidence backs a credit.",
    },
    {
        "id": "shd09-replay-image-not-compared-by-the-store",
        "invariants": ["SHD-09"],
        "path": STORE,
        "old": (
            "            or verdict.replay_env.image_digest != "
            "manifest.training.reference_spec.image_digest\n"
        ),
        "new": "",
        "tests": [service(EVIDENCE, "replay image that is not the pinned one")],
        "why": "the store no longer checks the pinned replay image of the verdict.",
    },
    {
        "id": "shd22-infrastructure-outcome-matures",
        "invariants": ["SHD-18", "SHD-22"],
        "path": STORE,
        "old": 'unresolved = admission.pending_dispute or trial[0] != "MATCH"',
        "new": 'unresolved = admission.pending_dispute or trial[0] == "FRAUD"',
        "tests": [service(NEVER_MATURES, "infrastructure outcome")],
        "why": "only a FRAUD trial outcome keeps collateral pending; INFRASTRUCTURE matures.",
    },
    {
        "id": "shd24-reward-route-checks-only-that-a-header-exists",
        "invariants": ["SHD-24"],
        "path": APP,
        "old": (
            "        admin(authorization)\n"
            "        return await run(\n"
            "            store.for_run(run_id).shadow_reward_v2"
        ),
        "new": (
            "        if authorization is None:\n"
            "            admin(authorization)\n"
            "        return await run(\n"
            "            store.for_run(run_id).shadow_reward_v2"
        ),
        "tests": [service(BEARER, "reward-a wrong bearer")],
        "why": "any presented credential opens the acceptance route.",
    },
]
