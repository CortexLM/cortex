"""Mutants of the disputes package: one realistic regression each of the contest machine.

Run: scripts/check_ledger_invariant_mutants.py --only <id> ... (see its --help). The five
mutants named after the FORGED test run a real round (about 45 s each); every other mutant is a
few seconds.
"""

# ruff: noqa: E501  (snippets and node ids are verbatim; wrapping them would change them)

from __future__ import annotations

BARE = "tests/ledger_invariants/test_disputes.py::"
HTTP = "tests/ledger_invariants/test_disputes_2.py::"

D = "src/hypertrain/challenge/disputes_v2.py"
S = "src/hypertrain/challenge/store.py"
A = "src/hypertrain/challenge/app.py"
E = "src/hypertrain/ledger/escrow_v2.py"

BINDING = BARE + "test_a_contest_lock_is_bound_to_one_dispute_and_the_first_binding_never_moves"
OWN_BOND = BARE + "test_a_contest_is_opened_only_on_the_miners_own_funded_bond_of_that_dispute"
IDENTITY = BARE + "test_only_the_audited_miner_signing_for_itself_can_contest_its_recorded_verdict"
PINNED_PARTY = (
    BARE
    + "test_a_policy_that_pins_a_party_as_referee_cannot_make_it_the_referee_of_its_own_dispute"
)
SIGNER = BARE + "test_only_the_referee_of_the_dispute_can_sign_its_resolution"
REASSIGN = BARE + "test_a_reassigned_referee_stays_independent_and_the_absolute_horizon_never_moves"
ONE_PER_MINER = BARE + "test_a_miner_has_one_unresolved_dispute_and_resolving_it_frees_the_slot"
THIRTY_TWO = BARE + "test_at_most_thirty_two_disputes_are_unresolved_and_a_closed_one_frees_a_slot"
EXACT_TURN = (
    BARE + "test_a_bisect_advances_the_dispute_only_on_the_exact_turn_and_a_replay_never_rewinds"
)
EMPTY = BARE + "test_an_empty_next_interval_is_refused_and_the_answer_leaves_no_entry"
CLOSED = BARE + "test_no_answer_is_taken_from_a_paused_or_resolved_dispute"
BURN = BARE + "test_the_bond_is_burned_only_when_the_contesting_miner_loses_by_fraud"
ONE_DECISION = (
    BARE
    + "test_a_resolved_dispute_takes_no_second_decision_and_an_identical_replay_changes_nothing"
)
CITES = (
    BARE
    + "test_a_resolution_must_cite_evidence_the_service_registered_for_this_dispute_and_transcript"
)
OLDER = BARE + "test_evidence_and_resolutions_of_an_older_transcript_are_refused"
LOSER = BARE + "test_a_resolution_can_name_only_one_of_the_two_parties_as_loser"
SERVE = BARE + "test_a_state_serve_is_stored_only_against_a_live_request_addressed_to_the_signer"
EVENTS = (
    BARE + "test_events_are_durable_cursors_grow_across_a_restart_and_a_lost_wakeup_delays_only"
)
ACK = BARE + "test_an_ack_is_stored_only_from_the_addressed_party_over_the_exact_event"
TIMEOUT = (
    BARE + "test_a_timeout_needs_the_acknowledged_turn_and_two_distinct_healthy_permitted_witnesses"
)
PROPERTY = BARE + "test_any_sequence_of_moves_keeps_a_dispute_monotone_and_its_decision_final"
RACE = BARE + "test_a_timeout_racing_a_resolution_has_one_winner_and_the_loser_is_refused"
ADVERSE = (
    BARE
    + "test_a_miner_with_an_adverse_audit_verdict_is_minted_nothing_even_if_the_finalize_names_it"
)
UNVERIFIED = BARE + "test_a_reward_cannot_be_minted_for_a_hotkey_without_verified_work"
CONVICT = BARE + "test_a_referee_cannot_convict_with_evidence_the_service_never_registered"
UNRESOLVED = BARE + "test_an_unresolved_contest_releases_nothing_whatever_the_clock_says"
STATE_REQUEST = (
    BARE + "test_a_state_request_is_refused_for_a_closed_dispute_a_late_round_or_a_bad_checkpoint"
)
SERVED_EVENT = BARE + "test_a_served_event_is_signed_by_the_coordinator_and_no_field_can_be_altered"
ATOMIC_BARE = (
    BARE + "test_a_failing_settlement_leaves_the_dispute_unresolved_when_both_share_one_transaction"
)
HORIZON = BARE + "test_an_answer_near_the_end_never_extends_its_turn_past_the_absolute_horizon"
XFAIL_TIMEOUT = (
    HTTP + "test_a_timeout_leaves_the_same_durable_resolution_record_as_a_referee_resolution"
)

ROUTE_OPEN = HTTP + "test_only_the_audited_miner_can_open_a_contest_over_the_service_route"
PRINCIPAL = HTTP + "test_each_dispute_route_accepts_only_its_intended_principal"
ACK_ROUTE = HTTP + "test_an_ack_over_the_route_binds_the_addressed_party_and_stores_one_row"
RECOVER = HTTP + "test_a_receipt_signed_for_another_purpose_cannot_recover_a_suspended_miner"
V1 = HTTP + "test_v1_dispute_routes_never_reach_a_v2_run"
ATOMIC = HTTP + "test_the_resolution_route_decides_and_settles_the_bond_in_one_transaction"
REFEREE_GATE = (
    HTTP + "test_the_referee_route_is_closed_to_a_resolved_dispute_and_to_the_absolute_horizon"
)
ROLLBACK_STATE = HTTP + "test_a_rollback_is_refused_for_a_settled_unapplied_or_unaudited_round"
ROLLBACK_DISPUTE = HTTP + "test_a_rollback_is_refused_while_a_dispute_of_the_round_is_unresolved"
ROLLBACK_CAUSE = (
    HTTP
    + "test_a_rollback_exclusion_needs_one_referee_signed_fraud_resolution_with_matching_causes"
)
REVISION = (
    HTTP + "test_any_change_of_a_dispute_changes_the_revision_a_rollback_repair_must_still_match"
)
PUSH_SNAPSHOT = HTTP + "test_a_beacon_push_survives_a_referee_call_registering_a_guard_meanwhile"
FORGED = HTTP + "test_a_forged_commit_is_never_decided_by_the_referee_and_earns_no_reward"
DECISION_TABLE = FORGED

EVIDENCE_TUPLE = (
    "            if (evidence.dispute_id, evidence.transcript_hash, evidence.reason, evidence.loser) != (\n"
    "                turn.dispute_id,\n"
    "                turn.transcript_hash,\n"
    "                model.reason,\n"
    "                model.loser,\n"
    "            ):"
)

MUTANTS = [
    {
        "id": "dsp-lock-of-another-dispute-accepted",
        "invariants": ["DIA-02", "DIB-16"],
        "path": D,
        "old": "            if (lock.owner, lock.dispute_id) != (self.owner_of(contest.miner), dispute_id) or (",
        "new": "            if lock.owner != self.owner_of(contest.miner) or (",
        "tests": [BINDING, OWN_BOND + "[another-dispute]"],
        "why": "The bond check compares the owner only, so a lock bound to another dispute backs this one.",
    },
    {
        "id": "dsp-contest-replay-with-another-bond-accepted",
        "invariants": ["DIA-02"],
        "path": D,
        "old": '                    raise DisputeError("CONTEST_REPLAY_CONFLICT")',
        "new": "                    pass",
        "tests": [BINDING],
        "why": "A second open of the same verdict naming another lock returns the old turn instead of a conflict.",
    },
    {
        "id": "dsp-bond-below-referee-cost-accepted",
        "invariants": ["DIB-16", "DS-01"],
        "path": D,
        "old": "                amount != lock.units or amount < self.policy.max_referee_cost_units",
        "new": "                amount != lock.units",
        "tests": [OWN_BOND + "[below-referee-cost]", ROUTE_OPEN],
        "why": "The minimum bond is no longer enforced, so a contest costs less than the referee.",
    },
    {
        "id": "dsp-admission-stake-accepted-as-bond",
        "invariants": ["DIB-16"],
        "path": D,
        "old": '            if lockrow is None or lockrow[0] != "LOCK_CONTEST":',
        "new": "            if lockrow is None:",
        "tests": [OWN_BOND + "[admission-stake]"],
        "why": "The kind of the lock is not checked, so an admission stake can be named as a contest bond.",
    },
    {
        "id": "dsp-contest-signed-by-another-miner",
        "invariants": ["DIA-03", "DS-01"],
        "path": D,
        "old": "            if contest.miner != env.signer or contest.auditor not in self.escrow.auditors:",
        "new": "            if contest.auditor not in self.escrow.auditors:",
        "tests": [IDENTITY, ROUTE_OPEN],
        "why": "Any registered hotkey can open the contest of a verdict that audited another miner.",
    },
    {
        "id": "dsp-accept-opens-a-contest",
        "invariants": ["DIA-03", "DS-01"],
        "path": D,
        "old": '            if not isinstance(model, DisputeV2) or model.action != "contest":',
        "new": "            if not isinstance(model, DisputeV2):",
        "tests": [IDENTITY],
        "why": "An accept without a bond reaches the funding check instead of being refused as unfunded-by-design.",
    },
    {
        "id": "dsp-contest-window-geometry-unchecked",
        "invariants": ["DIA-03"],
        "path": D,
        "old": '                raise DisputeError("CONTEST_WINDOW_GEOMETRY")',
        "new": "                pass",
        "tests": [IDENTITY],
        "why": "A contest record whose step span is not H // J opens a dispute over the wrong window.",
    },
    {
        "id": "dsp-auditor-allowed-as-referee",
        "invariants": ["DIA-04"],
        "path": D,
        "old": (
            "                contest.miner,\n"
            "                contest.auditor,\n"
            "                self.key.ss58,\n"
            "            ):\n"
            '                raise DisputeError("INDEPENDENT_REFEREE_REQUIRED")'
        ),
        "new": (
            "                contest.miner,\n"
            "                self.key.ss58,\n"
            "            ):\n"
            '                raise DisputeError("INDEPENDENT_REFEREE_REQUIRED")'
        ),
        "tests": [PINNED_PARTY],
        "why": "A pinned referee that is the audited auditor may judge its own audit.",
    },
    {
        "id": "dsp-reassign-to-the-sitting-referee",
        "invariants": ["DIA-04"],
        "path": D,
        "old": (
            "                    turn.contest.auditor,\n"
            "                    turn.contest.referee,\n"
            "                    self.key.ss58,"
        ),
        "new": "                    turn.contest.auditor,\n                    self.key.ss58,",
        "tests": [REASSIGN],
        "why": "The one reassignment may name the referee that is already sitting.",
    },
    {
        "id": "dsp-resolution-signed-by-anyone",
        "invariants": ["DIA-04", "DIB-01", "DIB-25", "DS-05"],
        "path": D,
        "old": '                self.run_id, {"ResolutionV2": lambda h: h == turn.contest.referee}',
        "new": '                self.run_id, {"ResolutionV2": lambda h: True}',
        "tests": [SIGNER, PRINCIPAL],
        "why": "Intake accepts a resolution from any signer, so the referee pin is gone.",
    },
    {
        "id": "dsp-second-dispute-of-one-miner",
        "invariants": ["DIA-05"],
        "path": D,
        "old": "                t.resolution is None and t.contest.miner == contest.miner for t in active",
        "new": "                False for t in active",
        "tests": [ONE_PER_MINER],
        "why": "The per-miner slot is not counted, so one miner holds many open disputes.",
    },
    {
        "id": "dsp-queue-cap-off-by-one",
        "invariants": ["DIA-05"],
        "path": D,
        "old": "            if sum(t.resolution is None for t in active) >= self.policy.max_open or any(",
        "new": "            if sum(t.resolution is None for t in active) > self.policy.max_open or any(",
        "tests": [THIRTY_TWO],
        "why": "The queue admits a 33rd unresolved dispute.",
    },
    {
        "id": "dsp-pause-leaves-the-turn-open",
        "invariants": ["DIA-07"],
        "path": D,
        "old": '            turn = turn.model_copy(update={"paused": True})',
        "new": '            turn = turn.model_copy(update={"paused": False})',
        "tests": [PROPERTY],
        "why": "pause() rewrites the turn but does not pause it.",
    },
    {
        "id": "dsp-reassignment-is-not-counted",
        "invariants": ["DIA-07", "DIA-04"],
        "path": D,
        "old": '            turn = turn.model_copy(update={"contest": contest, "reassignments": 1})',
        "new": '            turn = turn.model_copy(update={"contest": contest, "reassignments": 0})',
        "tests": [PROPERTY, REASSIGN],
        "why": "The reassignment counter stays at zero, so the referee can be replaced again and again.",
    },
    {
        "id": "dsp-bisect-on-a-stale-transcript",
        "invariants": ["DIA-09"],
        "path": D,
        "old": (
            "                model.previous_transcript_hash,\n"
            "            ) != (\n"
            "                turn.seq,\n"
            "                turn.level,\n"
            "                turn.ctx,\n"
            "                turn.interval,\n"
            "                turn.expected_party,\n"
            "                turn.transcript_hash,\n"
            "            ):"
        ),
        "new": (
            "            ) != (\n"
            "                turn.seq,\n"
            "                turn.level,\n"
            "                turn.ctx,\n"
            "                turn.interval,\n"
            "                turn.expected_party,\n"
            "            ):"
        ),
        "tests": [EXACT_TURN],
        "why": "The previous transcript hash is no longer part of the turn binding, so stale answers apply.",
    },
    {
        "id": "dsp-bisect-sequence-conflict-silenced",
        "invariants": ["DIA-09"],
        "path": D,
        "old": '                    raise DisputeError("SEQUENCE_REPLAY_CONFLICT")',
        "new": "                    pass",
        "tests": [EXACT_TURN],
        "why": "A different body for an answered sequence returns the stored receipt instead of a conflict.",
    },
    {
        "id": "dsp-bisect-on-a-closed-turn",
        "invariants": ["DIA-09", "DIA-07"],
        "path": D,
        "old": '                raise DisputeError("CLOSED_PAUSED_OR_EXPIRED")',
        "new": "                pass",
        "tests": [EXACT_TURN, CLOSED],
        "why": "Answers are applied to resolved, paused and expired turns.",
    },
    {
        "id": "dsp-empty-interval-accepted",
        "invariants": ["DIA-09"],
        "path": D,
        "old": "            if next_turn.interval[0] >= next_turn.interval[1]:",
        "new": "            if False:",
        "tests": [EMPTY],
        "why": "An answer that leaves an empty interval advances the turn.",
    },
    {
        "id": "dsp-timeout-burns-the-bond",
        "invariants": ["DIA-12", "DIB-10", "DIB-14"],
        "path": D,
        "old": '            and resolution.reason == "FRAUD"\n            and (resolution.loser == turn.contest.miner)',
        "new": '            and resolution.reason in ("FRAUD", "PARTY_TIMEOUT")\n            and (resolution.loser == turn.contest.miner)',
        "tests": [TIMEOUT + "[miner]"],
        "why": "A silent miner counts as lost by fraud, so its bond is burned.",
    },
    {
        "id": "dsp-auditor-loss-burns-the-bond",
        "invariants": ["DIA-12", "DIB-10"],
        "path": D,
        "old": "            and (resolution.loser == turn.contest.miner)",
        "new": "            and (resolution.loser in (turn.contest.miner, turn.contest.auditor))",
        "tests": [BURN + "[the-auditor-cheated]"],
        "why": "A FRAUD against the auditor is read as a loss of the contesting miner's bond.",
    },
    {
        "id": "dsp-settlement-id-changes-with-the-round",
        "invariants": ["DIB-10"],
        "path": D,
        "old": '                operation_id=sha256_hex(canonicalize([turn.dispute_id, "settle"])),',
        "new": '                operation_id=sha256_hex(canonicalize([turn.dispute_id, "settle", now])),',
        "tests": [BURN + "[audit-confirmed]"],
        "why": "The settle operation id depends on the clock, so a replay can settle the bond twice.",
    },
    {
        "id": "dsp-second-resolution-silently-kept",
        "invariants": ["DIB-13"],
        "path": D,
        "old": '                    raise DisputeError("RESOLUTION_CONFLICT")',
        "new": "                    pass",
        "tests": [ONE_DECISION],
        "why": "A different second resolution is acknowledged as if it were the first.",
    },
    {
        "id": "dsp-timeout-after-a-resolution",
        "invariants": ["DIB-13", "DIB-24"],
        "path": D,
        "old": "            if turn.resolution is not None or now <= min(",
        "new": "            if now <= min(",
        "tests": [ONE_DECISION, RACE],
        "why": "A due timeout is applied to a dispute that was already decided.",
    },
    {
        "id": "dsp-ack-signature-unchecked",
        "invariants": ["DIA-17", "DIA-18", "SG-14"],
        "path": D,
        "old": "        if not authentic(ack) or ack.run_id != self.run_id or ack.received_beacon > now:",
        "new": "        if ack.run_id != self.run_id or ack.received_beacon > now:",
        "tests": [ACK, ACK_ROUTE],
        "why": "An acknowledgement is stored without verifying that the party signed it.",
    },
    {
        "id": "dsp-ack-of-another-event-accepted",
        "invariants": ["DIA-18", "SG-14"],
        "path": D,
        "old": '                raise DisputeError("ACK_EVENT_OR_DEADLINE")',
        "new": "                pass",
        "tests": [ACK],
        "why": "An ack naming the wrong event hash, or sent outside the turn window, is stored.",
    },
    {
        "id": "dsp-ack-for-a-party-without-an-event",
        "invariants": ["DIA-18", "SG-14"],
        "path": D,
        "old": '                raise DisputeError("ACK_UNKNOWN_EVENT")',
        "new": "                pass",
        "tests": [ACK, ACK_ROUTE],
        "why": "A party with no event at that cursor can still have an acknowledgement stored.",
    },
    {
        "id": "dsp-timeout-with-one-witness-twice",
        "invariants": ["DIB-14", "DIA-17"],
        "path": D,
        "old": "                or len(authorities) != 2",
        "new": "                or len(authorities) < 1",
        "tests": [TIMEOUT],
        "why": "One observer signing twice is enough witness for a timeout.",
    },
    {
        "id": "dsp-timeout-with-a-party-as-witness",
        "invariants": ["DIB-14", "DIA-17"],
        "path": D,
        "old": (
            "                or authorities\n"
            "                & {\n"
            "                    turn.contest.miner,\n"
            "                    turn.contest.auditor,\n"
            "                }"
        ),
        "new": "                or authorities & set()",
        "tests": [TIMEOUT],
        "why": "A party to the dispute may be one of the two witnesses of its own opponent's silence.",
    },
    {
        "id": "dsp-timeout-counts-an-old-ack-as-delivery",
        "invariants": ["DIB-14"],
        "path": D,
        "old": (
            "                    and (event.turn.seq, event.turn.transcript_hash)\n"
            "                    == (turn.seq, turn.transcript_hash)"
        ),
        "new": "                    and event.turn.seq >= 0",
        "tests": [TIMEOUT + "[miner]"],
        "why": "An acknowledgement of an earlier turn authenticates delivery of the current one.",
    },
    {
        "id": "dsp-timeout-with-a-stale-head",
        "invariants": ["DIB-14"],
        "path": D,
        "old": "== (self.run_id, dispute_id, turn.seq, turn.transcript_hash, now, True, True, True)",
        "new": "== (\n                    self.run_id, dispute_id, turn.seq, turn.transcript_hash, p.observed_beacon, True, True, True\n                )",
        "tests": [TIMEOUT],
        "why": "Witness observations of an older beacon head are accepted.",
    },
    {
        "id": "dsp-resolution-cites-evidence-of-an-older-transcript",
        "invariants": ["DIA-19", "DIB-01", "DS-05"],
        "path": D,
        "old": EVIDENCE_TUPLE,
        "new": (
            "            if (evidence.dispute_id, evidence.reason, evidence.loser) != (\n"
            "                turn.dispute_id,\n"
            "                model.reason,\n"
            "                model.loser,\n"
            "            ):"
        ),
        "tests": [OLDER],
        "why": "A resolution may cite evidence registered for an earlier transcript of the dispute.",
    },
    {
        "id": "dsp-resolution-names-another-loser-than-the-evidence",
        "invariants": ["DIA-19", "DIB-01", "DS-05"],
        "path": D,
        "old": EVIDENCE_TUPLE,
        "new": (
            "            if (evidence.dispute_id, evidence.transcript_hash, evidence.reason) != (\n"
            "                turn.dispute_id,\n"
            "                turn.transcript_hash,\n"
            "                model.reason,\n"
            "            ):"
        ),
        "tests": [CITES],
        "why": "The referee may sign a loser the registered evidence does not name.",
    },
    {
        "id": "dsp-bystander-convicted",
        "invariants": ["DIA-19", "DIB-01"],
        "path": D,
        "old": "            if model.loser is not None and model.loser not in (",
        "new": "            if False and model.loser not in (",
        "tests": [LOSER],
        "why": "A resolution may name a hotkey that is neither the miner nor the auditor as the loser.",
    },
    {
        "id": "dsp-stale-evidence-registered",
        "invariants": ["DIA-19"],
        "path": D,
        "old": '                raise DisputeError("REFEREE_STALE_EVIDENCE")',
        "new": "                pass",
        "tests": [OLDER],
        "why": "Evidence of an older transcript can be registered for the current one.",
    },
    {
        "id": "dsp-state-served-after-the-horizon",
        "invariants": ["DIA-20"],
        "path": D,
        "old": "                and now <= e.turn.absolute_deadline",
        "new": "                and now >= 0",
        "tests": [SERVE],
        "why": "A state serve is stored after the absolute deadline of its request.",
    },
    {
        "id": "dsp-state-serve-conflict-silenced",
        "invariants": ["DIA-20"],
        "path": D,
        "old": '                raise DisputeError("STATE_SERVE_REPLAY_CONFLICT")',
        "new": "                pass",
        "tests": [SERVE],
        "why": "A different body for a served checkpoint is accepted as a repeat.",
    },
    {
        "id": "dsp-event-cursor-read-twice",
        "invariants": ["DIA-21"],
        "path": D,
        "old": '"SELECT event FROM dispute_events_v2 WHERE party=? AND cursor>? "',
        "new": '"SELECT event FROM dispute_events_v2 WHERE party=? AND cursor>=? "',
        "tests": [EVENTS],
        "why": "A watcher that resumes from its last cursor receives that event again.",
    },
    {
        "id": "dsp-resolution-route-skips-the-bond",
        "invariants": ["DIA-13", "DIB-10"],
        "path": S,
        "old": '                            disputes.settle_lock(result["dispute_id"], now=now)',
        "new": "                            pass",
        "tests": [ATOMIC],
        "why": "The resolution route records the decision but never settles the bond.",
    },
    {
        "id": "dsp-escrow-transactions-unserialized",
        "invariants": ["DIB-24", "DIA-13"],
        "path": E,
        "old": "        with self.mutex:\n            nested = self.db.in_transaction",
        "new": "        if True:\n            nested = self.db.in_transaction",
        "tests": [RACE],
        "why": "Escrow transactions no longer take the shared lock, so a closer does not wait for another.",
    },
    {
        "id": "dsp-reward-for-an-adverse-verdict",
        "invariants": ["DIB-06"],
        "path": E,
        "old": '                    x.result != "MATCH"\n                    or x.first_bad_leaf is not None',
        "new": "                    x.first_bad_leaf is not None",
        "tests": [ADVERSE],
        "why": "A verdict that is not a MATCH no longer withholds the reward unless it names a bad leaf.",
    },
    {
        "id": "dsp-reward-for-an-unverified-hotkey",
        "invariants": ["DIB-06"],
        "path": E,
        "old": "            or set(work) != set(final.included)\n",
        "new": "            or False\n",
        "tests": [UNVERIFIED],
        "why": "The Finalize may list a hotkey that has no verified work and still be minted.",
    },
    {
        "id": "dsp-referee-after-the-horizon-round",
        "invariants": ["DIB-12"],
        "path": S,
        "old": "            if turn.resolution is not None or self._now(self._db) >= turn.absolute_deadline:",
        "new": "            if turn.resolution is not None or self._now(self._db) > turn.absolute_deadline:",
        "tests": [REFEREE_GATE],
        "why": "The referee replays at the horizon round, where only its signature still counts.",
    },
    {
        "id": "dsp-rollback-over-an-open-dispute",
        "invariants": ["DIB-19"],
        "path": S,
        "old": "                if not status.applied or status.unresolved_audits or status.unresolved_disputes:",
        "new": "                if not status.applied or status.unresolved_audits:",
        "tests": [ROLLBACK_DISPUTE],
        "why": "A rollback proceeds although a dispute of the round is unresolved.",
    },
    {
        "id": "dsp-rollback-over-a-settled-round",
        "invariants": ["DIB-19"],
        "path": S,
        "old": '                if status.disposition == "SETTLED":',
        "new": "                if False:",
        "tests": [ROLLBACK_STATE + "[round-w-settled]"],
        "why": "A finalized round can be rolled back after its reward was minted.",
    },
    {
        "id": "dsp-rollback-cause-without-referee-work",
        "invariants": ["DIB-20"],
        "path": S,
        "old": "                if independent is None:",
        "new": "                if False:",
        "tests": [ROLLBACK_CAUSE],
        "why": "A resolution whose evidence row is gone still justifies removing a contribution.",
    },
    {
        "id": "dsp-rollback-cause-against-the-auditor",
        "invariants": ["DIB-20"],
        "path": S,
        "old": '                    or resolution["loser"] != hotkey\n                ):',
        "new": "                ):",
        "tests": [ROLLBACK_CAUSE],
        "why": "A FRAUD resolution that convicted the auditor excludes the miner.",
    },
    {
        "id": "dsp-rollback-cause-not-exact",
        "invariants": ["DIB-20"],
        "path": S,
        "old": "            if len(matched) != 1:",
        "new": "            if len(matched) > 1:",
        "tests": [ROLLBACK_CAUSE],
        "why": "An excluded miner without any matching accepted cause no longer stops the rollback there.",
    },
    {
        "id": "dsp-v1-dispute-routes-reach-v2-runs",
        "invariants": ["DIB-23"],
        "path": S,
        "old": (
            '        if json.loads(row["manifest"]).get("manifest_version") == 2:\n'
            '            raise ChallengeError(409, "route and stored contract version differ")'
        ),
        "new": (
            '        if json.loads(row["manifest"]).get("manifest_version") == 3:\n'
            '            raise ChallengeError(409, "route and stored contract version differ")'
        ),
        "tests": [V1],
        "why": "The v1 run lookup accepts a v2 run, so the v1 dispute handlers can touch it.",
    },
    {
        "id": "dsp-rollback-revision-ignores-disputes",
        "invariants": ["DIB-24", "DIB-19"],
        "path": S,
        "old": "                    for rows in (records, admissions, disputes, intake, origins)",
        "new": "                    for rows in (records, admissions, intake, origins)",
        "tests": [REVISION],
        "why": "A dispute opened during a repair no longer changes the revision the repair must match.",
    },
    {
        "id": "dsp-beacon-push-walks-the-live-guard-registry",
        "invariants": ["DIB-24", "DIB-D12"],
        "path": S,
        "old": "            guards = tuple(self._lease_guards_v2.values())\n",
        "new": "            guards = self._lease_guards_v2.values()\n",
        "tests": [PUSH_SNAPSHOT],
        "why": "The beacon push iterates the live guard registry, so a lease execution that registers its guard meanwhile breaks the push (RuntimeError).",
    },
    {
        "id": "dsp-referee-route-without-the-operator-bearer",
        "invariants": ["DIB-25", "DIB-12"],
        "path": A,
        "old": "        admin(authorization)\n        return await run(store.for_run(run_id).referee_v2, run_id, dispute_id)",
        "new": "        return await run(store.for_run(run_id).referee_v2, run_id, dispute_id)",
        "tests": [PRINCIPAL],
        "why": "Anyone can start the master's referee replay of a dispute.",
    },
    {
        "id": "dsp-timeout-route-without-the-operator-bearer",
        "invariants": ["DIB-25"],
        "path": A,
        "old": (
            "        admin(authorization)\n"
            "        return await run(\n"
            "            store.for_run(run_id).dispute_timeout_v2, run_id, dispute_id, await v2_raw(request)\n"
            "        )"
        ),
        "new": (
            "        return await run(\n"
            "            store.for_run(run_id).dispute_timeout_v2, run_id, dispute_id, await v2_raw(request)\n"
            "        )"
        ),
        "tests": [PRINCIPAL],
        "why": "Anyone can submit witnesses that close a dispute by timeout.",
    },
    {
        "id": "dsp-dispute-routes-open-to-hotkeys-off-the-subnet",
        "invariants": ["DIB-25", "DS-01"],
        "path": A,
        "old": '    party_signed = {"dispute", "bisect", "state-serve"}',
        "new": "    party_signed: set[str] = set()",
        "tests": [ROUTE_OPEN, PRINCIPAL],
        "why": "The dispute, bisect and state-serve routes no longer require a pinned role or a subnet member.",
    },
    {
        "id": "dsp-watch-feed-without-the-party-signature",
        "invariants": ["DIB-25", "DIA-17"],
        "path": A,
        "old": '        if not valid:\n            raise ChallengeError(403, "party event access requires signature")',
        "new": '        if False:\n            raise ChallengeError(403, "party event access requires signature")',
        "tests": [PRINCIPAL],
        "why": "Anyone can read the dispute events addressed to a party.",
    },
    {
        "id": "dsp-recovery-reference-unchecked",
        "invariants": ["SG-12"],
        "path": S,
        "old": "                        if row is None or row[0] != auth.commit_hash:",
        "new": "                        if row is None:",
        "tests": [
            RECOVER,
            "tests/challenge/test_service_network_v2.py::test_recovery_wrong_pending_dispute_cannot_clear_suspension",
        ],
        "why": "A Receipt that cites any reference, signed by the suspended miner, reaches the settlement check.",
    },
    {
        "id": "dsp-recovery-open-to-hotkeys-off-the-subnet",
        "invariants": ["SG-12"],
        "path": A,
        "old": '    miner_signed = {"recover"}',
        "new": "    miner_signed: set[str] = set()",
        "tests": [RECOVER],
        "why": "A Receipt signed by a key with no standing on the subnet is taken for a recovery request.",
    },
    {
        "id": "dsp-finalize-names-an-excluded-miner",
        "invariants": ["DIB-06"],
        "path": S,
        "old": "            if model.w != w or model.included != included:",
        "new": "            if model.w != w:",
        "tests": [FORGED],
        "why": "A Finalize that lists a miner whose audit verdict is adverse passes the included-set check.",
    },
    {
        "id": "dsp-referee-convicts-from-an-adverse-verdict",
        "invariants": ["DIB-02"],
        "path": S,
        "old": '            if accepted["result"] != "MATCH":',
        "new": "            if False:",
        "tests": [DECISION_TABLE],
        "why": "A MISMATCH contest that never reached the final op is adjudicated like a MATCH one.",
    },
    {
        "id": "dsp-referee-blames-without-one-wrong-claimant",
        "invariants": ["DIB-02"],
        "path": S,
        "old": "            if len(wrong) != 1:",
        "new": "            if len(wrong) > 1:",
        "tests": [DECISION_TABLE],
        "why": "When the replay agrees with both claimants the first one is named the loser.",
    },
    {
        "id": "dsp-referee-trusts-the-agreed-input",
        "invariants": ["DIB-02"],
        "path": S,
        "old": "            if any(a.hashes[0] != truth[0] for a in answers):",
        "new": "            if False:",
        "tests": [DECISION_TABLE],
        "why": "A loser is named although the replay does not reproduce the input both claimants agreed on.",
    },
    {
        "id": "dsp-referee-descriptor-rewritten",
        "invariants": ["DIB-12"],
        "path": S,
        "old": (
            '            if old is not None and json.loads(old[0])["descriptor_hash"] != descriptor_hash:\n'
            '                raise ChallengeError(409, "immutable referee descriptor conflicts")'
        ),
        "new": '            if False:\n                raise ChallengeError(409, "immutable referee descriptor conflicts")',
        "tests": [DECISION_TABLE],
        "why": "A second referee call can run against a descriptor that differs from the first.",
    },
    {
        "id": "dsp-turn-deadline-past-the-horizon",
        "invariants": ["DIA-07"],
        "path": D,
        "old": (
            'update["turn_deadline"] = min(\n'
            "                now + self.escrow.manifest.training.verify.T.dispute_per_level,\n"
            "                turn.absolute_deadline,\n"
            "            )"
        ),
        "new": 'update["turn_deadline"] = now + self.escrow.manifest.training.verify.T.dispute_per_level',
        "tests": [HORIZON],
        "why": "An answer near the end extends the turn past the absolute horizon.",
    },
    {
        "id": "dsp-reassign-after-resolution",
        "invariants": ["DIA-07", "DIB-13"],
        "path": D,
        "old": (
            "                not turn.paused\n"
            "                or turn.resolution is not None\n"
            "                or now >= turn.absolute_deadline"
        ),
        "new": (
            "                not turn.paused\n                or now >= turn.absolute_deadline"
        ),
        "tests": [PROPERTY],
        "why": "A decided and paused dispute can still have its referee replaced.",
    },
    {
        "id": "dsp-unresolved-settlement-passes",
        "invariants": ["DIA-12", "DIB-10"],
        "path": D,
        "old": (
            "            if status.unresolved:\n"
            '                raise DisputeError("UNRESOLVED_CONTEST")'
        ),
        "new": ("            if False:\n                raise DisputeError('UNRESOLVED_CONTEST')"),
        "tests": [UNRESOLVED],
        "why": "settle_lock goes on with an open contest.",
    },
    {
        "id": "dsp-timeout-without-any-ack",
        "invariants": ["DIB-14"],
        "path": D,
        "old": (
            "            if not delivered:\n"
            '                raise DisputeError("NO_AUTHENTICATED_TURN_DELIVERY")'
        ),
        "new": (
            "            if False:\n"
            "                raise DisputeError('NO_AUTHENTICATED_TURN_DELIVERY')"
        ),
        "tests": [TIMEOUT + "[miner]"],
        "why": "A turn that no acknowledgement shows delivered can be timed out.",
    },
    {
        "id": "dsp-closed-dispute-blocks-the-miner",
        "invariants": ["DIA-05"],
        "path": D,
        "old": "                t.resolution is None and t.contest.miner == contest.miner for t in active",
        "new": "                t.contest.miner == contest.miner for t in active",
        "tests": [ONE_PER_MINER],
        "why": "A miner whose dispute was closed can never contest again.",
    },
    {
        "id": "dsp-coordinator-may-referee",
        "invariants": ["DIA-04"],
        "path": D,
        "old": (
            "                contest.miner,\n"
            "                contest.auditor,\n"
            "                self.key.ss58,\n"
            "            ):\n"
            '                raise DisputeError("INDEPENDENT_REFEREE_REQUIRED")'
        ),
        "new": (
            "                contest.miner,\n"
            "                contest.auditor,\n"
            "            ):\n"
            '                raise DisputeError("INDEPENDENT_REFEREE_REQUIRED")'
        ),
        "tests": [PINNED_PARTY],
        "why": "The coordinator can be the referee of a dispute it runs.",
    },
    {
        "id": "dsp-miner-may-referee",
        "invariants": ["DIA-04"],
        "path": D,
        "old": (
            "                contest.miner,\n"
            "                contest.auditor,\n"
            "                self.key.ss58,\n"
            "            ):\n"
            '                raise DisputeError("INDEPENDENT_REFEREE_REQUIRED")'
        ),
        "new": (
            "                contest.auditor,\n"
            "                self.key.ss58,\n"
            "            ):\n"
            '                raise DisputeError("INDEPENDENT_REFEREE_REQUIRED")'
        ),
        "tests": [PINNED_PARTY],
        "why": "The contesting miner can judge its own dispute.",
    },
    {
        "id": "dsp-reassign-to-an-unpinned-key",
        "invariants": ["DIA-04"],
        "path": D,
        "old": (
            "                or referee not in self.policy.referees\n"
            "                or referee\n"
            "                in ("
        ),
        "new": ("                or referee\n                in ("),
        "tests": [REASSIGN],
        "why": "The replacement referee need not be one the operator pinned.",
    },
    {
        "id": "dsp-reassign-to-the-coordinator",
        "invariants": ["DIA-04"],
        "path": D,
        "old": (
            "                    turn.contest.referee,\n"
            "                    self.key.ss58,\n"
            "                )\n"
            "            ):\n"
            '                raise DisputeError("REFEREE_REASSIGNMENT_BOUND")'
        ),
        "new": (
            "                    turn.contest.referee,\n"
            "                )\n"
            "            ):\n"
            '                raise DisputeError("REFEREE_REASSIGNMENT_BOUND")'
        ),
        "tests": [REASSIGN],
        "why": "The coordinator can be named the replacement referee.",
    },
    {
        "id": "dsp-state-request-on-a-resolved-dispute",
        "invariants": ["DIA-20"],
        "path": D,
        "old": (
            "                turn.resolution is not None\n"
            "                or now > turn.absolute_deadline\n"
            "                or not (0 <= checkpoint <= turn.contest.step_span)"
        ),
        "new": (
            "                now > turn.absolute_deadline\n"
            "                or not (0 <= checkpoint <= turn.contest.step_span)"
        ),
        "tests": [STATE_REQUEST],
        "why": "A checkpoint is requested of a miner for a dispute already decided.",
    },
    {
        "id": "dsp-ack-stored-twice",
        "invariants": ["DIA-18", "SG-14"],
        "path": D,
        "old": (
            "self.db.execute(\n"
            '                "INSERT OR IGNORE INTO dispute_acks_v2 VALUES(?,?,?)",'
        ),
        "new": ('self.db.execute(\n                "INSERT INTO dispute_acks_v2 VALUES(?,?,?)",'),
        "tests": [ACK],
        "why": "The same acknowledgement sent twice breaks on the primary key instead of being a no-op.",
    },
    {
        "id": "dsp-resolution-commits-early",
        "invariants": ["DIA-13", "DIB-10"],
        "path": D,
        "old": (
            '            turn = turn.model_copy(update={"resolution": model, "resolved_beacon": now})\n'
            "            self._save(turn)\n"
            "        self.notify()"
        ),
        "new": (
            '            turn = turn.model_copy(update={"resolution": model, "resolved_beacon": now})\n'
            "            self._save(turn)\n"
            "            self.db.commit()\n"
            "        self.notify()"
        ),
        "tests": [ATOMIC_BARE],
        "why": "The decision is committed before the caller has settled the bond.",
    },
    {
        "id": "dsp-dispute-state-route-without-the-operator-bearer",
        "invariants": ["DIB-25"],
        "path": A,
        "old": (
            "        admin(authorization)\n"
            "        return await run(store.for_run(run_id).dispute_state_v2, run_id, dispute_id, checkpoint)"
        ),
        "new": "        return await run(store.for_run(run_id).dispute_state_v2, run_id, dispute_id, checkpoint)",
        "tests": [PRINCIPAL],
        "why": "Anyone can ask a miner to serve a checkpoint.",
    },
    {
        "id": "dsp-rollback-preview-route-without-the-operator-bearer",
        "invariants": ["DIB-25"],
        "path": A,
        "old": (
            "        admin(authorization)\n"
            "        try:\n"
            "            return await run(\n"
            "                store.for_run(run_id).rollback_preview_v2, run_id, await v2_raw(request)\n"
            "            )"
        ),
        "new": (
            "        try:\n"
            "            return await run(\n"
            "                store.for_run(run_id).rollback_preview_v2, run_id, await v2_raw(request)\n"
            "            )"
        ),
        "tests": [PRINCIPAL],
        "why": "Anyone can run the master's rollback preview.",
    },
    {
        "id": "dsp-timeout-skips-the-round-recount",
        "invariants": ["DIB-15", "DIB-13"],
        "path": S,
        "old": (
            "                disputes.settle_lock(dispute_id, now=self._now(self._db))\n"
            "                self._recount_disputes_v2(run_id, result.contest.w)\n"
            "        disputes.notify()"
        ),
        "new": (
            "                disputes.settle_lock(dispute_id, now=self._now(self._db))\n"
            "        disputes.notify()"
        ),
        "tests": [XFAIL_TIMEOUT],
        "why": "A timed-out dispute keeps holding its round open.",
    },
    {
        "id": "dsp-state-serve-for-another-challenge",
        "invariants": ["DIA-20"],
        "path": D,
        "old": (
            "                and (\n"
            "                    e.turn.contest.challenge_hash,\n"
            "                    e.checkpoint * stride,\n"
            "                )\n"
            "                == (model.challenge_hash, model.t)"
        ),
        "new": "                and e.checkpoint * stride == model.t",
        "tests": [SERVE],
        "why": "A state serve for another challenge answers the request.",
    },
    {
        "id": "dsp-event-signature-skips-the-turn",
        "invariants": ["DIA-17"],
        "path": D,
        "old": 'model.model_dump(mode="json", exclude={"sig"}),',
        "new": 'model.model_dump(mode="json", exclude={"sig", "turn"}),',
        "tests": [SERVED_EVENT],
        "why": "The coordinator signature no longer covers the turn an event announces.",
    },
    {
        "id": "dsp-rollback-over-an-unresolved-audit",
        "invariants": ["DIB-19"],
        "path": S,
        "old": "                if not status.applied or status.unresolved_audits or status.unresolved_disputes:",
        "new": "                if not status.applied or status.unresolved_disputes:",
        "tests": [ROLLBACK_STATE + "[audit-unresolved]"],
        "why": "A rollback proceeds over a round whose audit is still open.",
    },
    {
        "id": "dsp-rollback-over-an-unapplied-round",
        "invariants": ["DIB-19"],
        "path": S,
        "old": "                if not status.applied or status.unresolved_audits or status.unresolved_disputes:",
        "new": "                if status.unresolved_audits or status.unresolved_disputes:",
        "tests": [ROLLBACK_STATE + "[not-applied]"],
        "why": "A rollback proceeds over a round that was never applied.",
    },
    {
        "id": "dsp-resolution-trusts-the-referee",
        "invariants": ["DIA-19", "DIB-01", "DS-05"],
        "path": D,
        "old": (
            "            row = self.db.execute(\n"
            '                "SELECT body FROM dispute_evidence_v2 WHERE hash=?", (model.evidence_hash,)\n'
            "            ).fetchone()\n"
            "            if row is None:\n"
            '                raise DisputeError("UNVERIFIED_REFEREE_EVIDENCE")\n'
            "            evidence = RefereeEvidence.model_validate_json(row[0])\n"
            "            if (evidence.dispute_id, evidence.transcript_hash, evidence.reason, evidence.loser) != (\n"
            "                turn.dispute_id,\n"
            "                turn.transcript_hash,\n"
            "                model.reason,\n"
            "                model.loser,\n"
            "            ):\n"
            '                raise DisputeError("REFEREE_EVIDENCE_BINDING")\n'
        ),
        "new": "",
        "tests": [CONVICT],
        "why": "The referee's resolution is taken without any evidence the service registered.",
    },
    {
        "id": "dsp-bond-of-another-coldkey",
        "invariants": ["DIB-16"],
        "path": D,
        "old": "            if (lock.owner, lock.dispute_id) != (self.owner_of(contest.miner), dispute_id) or (",
        "new": "            if lock.dispute_id != dispute_id or (",
        "tests": [OWN_BOND + "[another-coldkey]"],
        "why": "A bond funded by another coldkey backs the contest.",
    },
    {
        "id": "dsp-miner-fraud-releases-the-bond",
        "invariants": ["DIA-12", "DIB-10"],
        "path": D,
        "old": (
            '                and turn.resolution.reason == "FRAUD"\n'
            "                and status.loser == turn.contest.miner"
        ),
        "new": (
            '                and turn.resolution.reason == "NEVER"\n'
            "                and status.loser == turn.contest.miner"
        ),
        "tests": [BURN + "[the-miner-cheated]"],
        "why": "A miner convicted of fraud gets its bond back.",
    },
    {
        "id": "dsp-rollback-ignores-the-next-round",
        "invariants": ["DIB-19"],
        "path": S,
        "old": "            for w in (model.w, model.w + 1):",
        "new": "            for w in (model.w,):",
        "tests": [ROLLBACK_STATE + "[round-w1-settled]"],
        "why": "A rollback checks the finality of round w only, so a settled w+1 is rewritten.",
    },
]
