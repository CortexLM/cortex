"""Mutants of the expiry package: one small regression of src each, killed by named tests."""

from __future__ import annotations

ONE = "tests/ledger_invariants/test_expiry.py::"
TWO = "tests/ledger_invariants/test_expiry_2.py::"
BOUND = TWO + "test_a_signed_envelope_is_bound_to_its_run_its_expiry_and_its_signature"
WRITE = ONE + "test_a_stale_beacon_refuses_a_round_write_with_503_and_leaves_no_trace"
ADMISSION = TWO + "test_a_stale_beacon_refuses_an_admission_mutation_before_its_signature_is_read"
LAG = TWO + "test_the_freshness_gate_tolerates_two_rounds_of_lag_and_refuses_the_third"
LIVE = TWO + "test_an_escrow_envelope_is_live_through_its_expiry_beacon_and_refused_after"
REPLAY = ONE + "test_an_identical_finalize_replay_returns_the_stored_result_and_changes_nothing"
LATE = ONE + "test_a_finalize_replay_past_its_expiry_is_refused_unless_it_is_signed_again"
OTHER = ONE + "test_a_finalize_with_other_content_is_refused_and_the_stored_result_stands"
NARROW = ONE + "test_a_round_whose_window_fits_one_lease_is_auditable_up_to_its_last_beacon"
BOOTSTRAP = (
    ONE + "test_a_production_bootstrap_envelope_of_one_run_is_refused_by_the_store_of_another"
)
WIDE = ONE + "test_a_round_wider_than_one_lease_is_not_left_unauditable"
# Closed on v3 by ADR 0011 part 2 and pinned by the lot that closed it (PIN_EXISTING).
LIVENESS = "tests/challenge/test_audit_liveness_v2.py::"
RE_LEASE = LIVENESS + "test_a_re_lease_after_the_first_window_validates"
POISONED_HEAD = (
    LIVENESS + "test_a_miner_sealed_input_that_cannot_be_leased_does_not_block_the_queue"
)

PAIR = TWO + "test_a_trial_pair_with_one_unfit_envelope_is_refused_whichever_one_it_is"
# Hotfix 2 (S19, D105): a lease and its replay start tolerated through a beacon pause.
PAUSE = "tests/challenge/test_audit_freshness_pause_v2.py::"
PAUSE_TAKEN = PAUSE + "test_production_takes_a_lagging_result_on_its_bls_verified_pinned_round"
PAUSE_REFUSED = PAUSE + "test_production_refuses_a_lagging_result_without_its_conditions"
# hotfix-finalize-freshness (S21, D118): an audited round's aggregate and finalize tolerated
# through a beacon pause.
ROUND_PAUSE = "tests/challenge/test_finalize_freshness_v2.py::"
ROUND_TAKEN = ROUND_PAUSE + "test_an_audited_round_is_aggregated_and_finalized_on_a_lagging_view"
ROUND_REFUSED = (
    ROUND_PAUSE + "test_a_lagging_aggregate_or_finalize_is_refused_without_its_conditions"
)
ROUND_BLS = ROUND_PAUSE + "test_production_refuses_a_lagging_audited_round_without_a_verified_pin"
S21 = "tests/e2e/test_finalize_freshness_v2_e2e.py::"
S21_AGGREGATE = S21 + "test_s21_an_aggregate_retried_on_a_flicker_answers_its_stored_tape"
# Branch hotfix-finalize-entry-gate: the finalize's gate judged once, before its replay.
S21_ENTRY = S21 + "test_a_finalize_refused_at_entry_replays_nothing"
S21_REPLAY = S21 + "test_s21_a_finalize_whose_replay_outlasts_the_cap_settles_at_its_entry_round"

STORE = "src/hypertrain/challenge/store.py"
ADMISSION_PY = "src/hypertrain/challenge/admission.py"
ENVELOPE = "src/hypertrain/protocol/envelope_v2.py"
ESCROW = "src/hypertrain/ledger/escrow_v2.py"

MUTANTS = [
    {
        "id": "exp-intake-skips-run-check",
        "invariants": ["SG-01", "ESC-21", "BND-16"],
        "path": ENVELOPE,
        "old": "if env.run_id != self.run_id:",
        "new": "if False:",
        "tests": [BOUND + "[transfer-sealed-for-another-run]", BOOTSTRAP],
        "why": "an envelope sealed for another run passes the route's run check and executes.",
    },
    {
        "id": "exp-intake-skips-embedded-run-check",
        "invariants": ["BND-16"],
        "path": ENVELOPE,
        "old": "if body_run != env.run_id:",
        "new": "if False:",
        "tests": [
            TWO + "test_a_signed_body_naming_another_run_is_refused_whatever_run_its_envelope_names"
        ],
        "why": "a body that names another run is accepted inside an envelope of this run.",
    },
    {
        "id": "exp-store-trusts-the-envelopes-run",
        "invariants": ["SG-01", "BND-16"],
        "path": STORE,
        "old": "envelope_v2.Intake(run_id, {expected: signer.__eq__}).accept(",
        "new": "envelope_v2.Intake(env.run_id, {expected: signer.__eq__}).accept(",
        "tests": [BOUND + "[commit-sealed-for-another-run]", BOOTSTRAP],
        "why": "the store takes the run to check against from the envelope it is checking.",
    },
    {
        "id": "exp-signature-drops-the-run",
        "invariants": ["SG-01", "ESC-21"],
        "path": ENVELOPE,
        "old": '+ str(exp_drand).encode("ascii")\n        + b"|"\n        + run_id.encode("ascii")',
        "new": '+ str(exp_drand).encode("ascii")',
        "tests": [
            BOUND + "[transfer-relabeled-to-this-run]",
            BOUND + "[open-relabeled-to-this-run]",
        ],
        "why": "the signature no longer covers the run, so a relabeled envelope verifies.",
    },
    {
        "id": "exp-signature-drops-the-expiry",
        "invariants": ["SG-01", "ESC-21"],
        "path": ENVELOPE,
        "old": '        + b"|"\n        + str(exp_drand).encode("ascii")\n',
        "new": "",
        "tests": [BOUND + "[transfer-expiry-extended]", BOUND + "[commit-expiry-extended]"],
        "why": "the signature no longer covers exp_drand, so a captured envelope can be extended.",
    },
    {
        "id": "exp-intake-skips-expiry",
        "invariants": ["SG-01", "ESC-21", "FIN-05"],
        "path": ENVELOPE,
        "old": "if now_round > env.exp_drand:",
        "new": "if False:",
        "tests": [BOUND + "[transfer-expired]", LIVE, LATE],
        "why": "an envelope past its expiry is accepted.",
    },
    {
        "id": "exp-intake-expiry-excludes-the-last-beacon",
        "invariants": ["ESC-21", "CK-01"],
        "path": ENVELOPE,
        "old": "if now_round > env.exp_drand:",
        "new": "if now_round >= env.exp_drand:",
        "tests": [LIVE],
        "why": "an envelope expires one beacon before the round its expiry names.",
    },
    {
        "id": "exp-store-judges-expiry-on-the-wall-clock",
        "invariants": ["CK-01"],
        "path": STORE,
        "old": "raw, self._now(self._db)\n        )\n        key = \x22auth:\x22",
        "new": (
            "raw, (int(self.clock()) - self._run_v2(run_id).training.beacon.genesis_time)"
            " // 3 + 1\n        )\n        key = \x22auth:\x22"
        ),
        "tests": [TWO + "test_expiry_is_judged_on_the_stored_beacon_whatever_the_wall_clock_says"],
        "why": "expiry is read off the wall clock instead of the verified beacon in the database.",
    },
    {
        "id": "exp-trial-screen-never-checked",
        "invariants": ["SG-01"],
        "path": ADMISSION_PY,
        "old": "            intake.accept(raw, at)\n            intake.accept(screen_raw, at)\n",
        "new": "            intake.accept(raw, at)\n",
        "tests": [
            PAIR + "[screen-sealed-for-another-run]",
            PAIR + "[screen-relabeled-to-this-run]",
            PAIR + "[screen-expiry-extended]",
            PAIR + "[screen-expired]",
        ],
        "why": "the work screen of a trial pair is never verified once its proof is.",
    },
    {
        "id": "exp-freshness-lag-of-one",
        "invariants": ["FIN-23", "CK-01"],
        "path": STORE,
        "old": "if now < emitted - MAX_LAG_ROUNDS and not self._result_in_lease_v2(",
        "new": "if now < emitted - 1 and not self._result_in_lease_v2(",
        "tests": [LAG],
        "why": "a beacon two rounds behind the clock is already stale.",
    },
    {
        "id": "exp-freshness-lag-of-three",
        "invariants": ["FIN-23", "CK-01"],
        "path": STORE,
        "old": "if now < emitted - MAX_LAG_ROUNDS and not self._result_in_lease_v2(",
        "new": "if now < emitted - 3 and not self._result_in_lease_v2(",
        "tests": [LAG],
        "why": "a beacon three rounds behind the clock is still current.",
    },
    {
        "id": "exp-admission-skips-freshness",
        "invariants": ["CK-02", "FIN-23"],
        "path": STORE,
        "old": 'now = self._fresh_v2(manifest, f"admission:{action}:{self._now(self._db)}")',
        "new": "now = self._now(self._db)",
        "tests": [
            ADMISSION + "[lock]",
            TWO + "test_a_stale_beacon_refuses_a_resolution_before_the_dispute_it_names_is_read",
        ],
        "why": "admission and escrow mutations no longer pass the freshness gate.",
    },
    {
        "id": "exp-training-skips-freshness",
        "invariants": ["FIN-23"],
        "path": STORE,
        "old": 'now = self._fresh_v2(manifest, f"{action}:{w}:{self._now(self._db)}")',
        "new": "now = self._now(self._db)",
        "tests": [WRITE + "[accept]", WRITE + "[commit]"],
        "why": "accept, commit and delta are taken while the beacon is stale.",
    },
    {
        "id": "exp-open-skips-freshness",
        "invariants": ["FIN-23"],
        "path": STORE,
        "old": 'now = self._fresh_v2(manifest, f"open:{body.w}")',
        "new": "now = self._now(self._db)",
        "tests": [WRITE + "[open]"],
        "why": "a round opens while the beacon is stale.",
    },
    {
        "id": "exp-audit-schedule-skips-freshness",
        "invariants": ["FIN-23"],
        "path": STORE,
        "old": 'now = self._fresh_v2(manifest, f"audit:{w}", body.d_audit)',
        "new": "now = self._now(self._db)",
        "tests": [WRITE + "[audits]"],
        "why": "audit jobs are scheduled while the beacon is stale.",
    },
    {
        "id": "exp-aggregate-skips-freshness",
        "invariants": ["FIN-23"],
        "path": STORE,
        "old": (
            '            now = self._fresh_v2(manifest, f"aggregate:{w}:{self._now(self._db)}")\n'
        ),
        "new": "            now = self._now(self._db)\n",
        "tests": [WRITE + "[aggregate]"],
        "why": "a round is aggregated while the beacon is stale.",
    },
    {
        "id": "exp-lease-skips-freshness",
        "invariants": ["FIN-23"],
        "path": STORE,
        "old": 'now = self._fresh_v2(manifest, f"lease:{self._now(self._db)}")',
        "new": "now = self._now(self._db)",
        "tests": [WRITE + "[lease]"],
        "why": "an audit lease is granted while the beacon is stale.",
    },
    {
        "id": "exp-lagging-view-judged-at-its-stale-round",
        "invariants": ["FIN-23", "CK-01"],
        "path": STORE,
        "old": "return max(now, emitted - MAX_LAG_ROUNDS)",
        "new": "return now",
        "tests": [
            PAUSE_TAKEN + "[lease]",
            PAUSE + "test_a_lease_during_a_pause_is_granted_no_longer_than_a_healthy_view_grants",
        ],
        "why": "a lease granted on a lagging view is dated at its stale round and ends early.",
    },
    {
        "id": "exp-lease-tolerated-on-an-admitted-run",
        "invariants": ["FIN-23"],
        "path": STORE,
        "old": "            ).fetchone()\n            is not None\n        ):\n"
        '            return False\n        if kind == "lease":',
        "new": "            ).fetchone()\n            is False\n        ):\n"
        '            return False\n        if kind == "lease":',
        "tests": [
            PAUSE_REFUSED + "[replay start of an admitted run]",
            ROUND_REFUSED + "[aggregate-an admitted run]",
            ROUND_REFUSED + "[finalize-an admitted run]",
        ],
        "why": "a service-admitted run takes a lease or a replay start on a lagging view.",
    },
    {
        "id": "exp-lease-tolerated-on-a-stopped-run",
        "invariants": ["FIN-23"],
        "path": STORE,
        "old": '.fetchone()[0]\n            != "running"',
        "new": '.fetchone()[0]\n            == "never"',
        "tests": [
            PAUSE_REFUSED + "[lease of a paused run]",
            ROUND_REFUSED + "[aggregate-a paused run]",
            ROUND_REFUSED + "[finalize-a paused run]",
        ],
        "why": "a run that is not running takes a lease on a lagging view.",
    },
    {
        "id": "exp-replay-start-skips-the-lease-deadline",
        "invariants": ["FIN-23"],
        "path": STORE,
        "old": 'if kind == "lease":\n            return True',
        "new": 'if kind in ("lease", "audit-execute"):\n            return True',
        "tests": [PAUSE_REFUSED + "[replay start at the deadline]"],
        "why": "a replay starts on a lagging view at or after its lease's real deadline.",
    },
    {
        "id": "exp-round-pause-uncapped",
        "invariants": ["FIN-23"],
        "path": STORE,
        "old": "            or now < emitted - MAX_LAG_ROUNDS - RESULT_PAUSE_ROUNDS\n",
        "new": "            or False\n",
        "tests": [
            ROUND_REFUSED + "[aggregate-past the cap]",
            ROUND_REFUSED + "[finalize-past the cap]",
        ],
        "why": "an aggregate or a finalize is taken however long the beacon has been paused.",
    },
    {
        "id": "exp-round-pause-without-a-stored-tape",
        "invariants": ["FIN-23"],
        "path": STORE,
        "old": 'return status.get("applied") is True and status.get("unresolved_audits") == 0',
        "new": 'return status.get("unresolved_audits") == 0',
        "tests": [
            ROUND_REFUSED + "[aggregate-not aggregated]",
            ROUND_REFUSED + "[finalize-not aggregated]",
            S21_AGGREGATE,
        ],
        "why": "a first aggregate judges its members at a round not pushed yet.",
    },
    {
        "id": "exp-round-pause-with-an-open-audit",
        "invariants": ["FIN-23"],
        "path": STORE,
        "old": 'return status.get("applied") is True and status.get("unresolved_audits") == 0',
        "new": 'return status.get("applied") is True',
        "tests": [
            ROUND_REFUSED + "[aggregate-an audit unresolved]",
            ROUND_REFUSED + "[finalize-an audit unresolved]",
        ],
        "why": "a round with an audit still queued, running or parked is taken on a lagging view.",
    },
    {
        "id": "exp-round-pause-on-a-stopped-or-admitted-run",
        "invariants": ["FIN-23"],
        "path": STORE,
        "old": '        if kind in ("lease", "audit-execute", "aggregate", "finalize") and (\n',
        "new": '        if kind in ("lease", "audit-execute") and (\n',
        "tests": [
            ROUND_REFUSED + "[aggregate-an admitted run]",
            ROUND_REFUSED + "[finalize-a paused run]",
        ],
        "why": "a paused or service-admitted run is aggregated or finalized on a lagging view.",
    },
    {
        "id": "exp-round-pause-skips-the-bls-check",
        "invariants": ["FIN-23", "CK-01"],
        "path": STORE,
        "old": "lag = emitted - pin  # health round = the pinned round, still BLS-verified\n",
        "new": "return max(now, emitted - MAX_LAG_ROUNDS)\n",
        "tests": [
            ROUND_BLS + "[aggregate-forged signature]",
            ROUND_BLS + "[finalize-fixture signature]",
        ],
        "why": "production takes a lagging aggregate or finalize on a pinned round never verified.",
    },
    {
        "id": "exp-finalize-skips-the-expiry-at-its-gate",
        "invariants": ["FIN-23", "FIN-04"],
        "path": STORE,
        "old": "                if now > envelope_v2.parse_envelope(raw).exp_drand:\n",
        "new": "                if False:\n",
        "tests": [S21_ENTRY],
        "why": "a Finalize expired at the round it settles at is replayed before it is refused.",
    },
    {
        "id": "exp-finalize-judged-again-after-its-replay",
        "invariants": ["FIN-23", "FIN-04"],
        "path": STORE,
        "old": "assert now is not None  # gated above: a finalize record is never removed\n",
        "new": 'now = self._fresh_v2(manifest, f"finalize:{w}:{self._now(self._db)}")\n',
        "tests": [S21_REPLAY],
        "why": "the gate is judged after the replay, on the lag the call's own replay made (S21).",
    },
    {
        "id": "exp-finalize-replay-compares-nothing",
        "invariants": ["FIN-05"],
        "path": STORE,
        "old": 'if json.loads(previous_final[0])["body"] != model.model_dump(mode="json"):',
        "new": 'if json.loads(previous_final[0])["body"] == model.model_dump(mode="json"):',
        "tests": [REPLAY, OTHER + "[False-conflicting finalized origin authority]"],
        "why": "a finalize with other content gets the stored result; an identical one is refused.",
    },
    {
        "id": "exp-finalize-replay-returns-nothing",
        "invariants": ["FIN-05"],
        "path": STORE,
        "old": 'return self._record_v2(run_id, "finalize-result", str(w))',
        "new": "return {}",
        "tests": [REPLAY],
        "why": "an identical finalize replay answers with an empty result.",
    },
    {
        "id": "exp-nonce-conflict-undetected",
        "invariants": ["FIN-05", "ESC-21"],
        "path": STORE,
        "old": "if known is not None and known[0] != digest:",
        "new": "if known is not None and known[0] is None:",
        "tests": [
            OTHER + "[True-conflicting signed operation nonce]",
            TWO + "test_a_replayed_escrow_transfer_moves_units_once_and_a_changed_body_is_refused",
        ],
        "why": "a second body under a reserved operation key is not recognized as a conflict.",
    },
    {
        "id": "exp-lease-drops-the-absolute-cap",
        "invariants": ["A2W-16"],
        "path": STORE,
        "old": 'expires = min(now + lease_rounds, row["absolute"])',
        "new": "expires = now + lease_rounds",
        "tests": [NARROW],
        "why": "a lease granted late in the window runs past the end of the round's audit window.",
    },
    {
        "id": "exp-audit-window-drops-d-final",
        "invariants": ["A2W-16"],
        "path": STORE,
        "old": "absolute = min(body.d_final, body.d_audit + policy.absolute_rounds())",
        "new": "absolute = body.d_audit + policy.absolute_rounds()",
        "tests": [NARROW],
        "why": "the audit window of a round outlasts its d_final.",
    },
    {
        "id": "exp-attempt-dated-by-its-creation-round",
        "invariants": ["A2W-16"],
        "path": STORE,
        "old": "created_beacon=now,",
        "new": 'created_beacon=row["created"],',
        "tests": [WIDE, RE_LEASE],
        "why": "a lease granted after d_audit is dated by its queue row and fails the lease bound.",
    },
    {
        "id": "exp-expired-commit-blocks-the-audit-queue",
        "invariants": ["A2W-17"],
        "path": STORE,
        "old": "if envelope_v2.parse_envelope(commit_env).exp_drand < now:",
        "new": "if False:",
        "tests": [POISONED_HEAD + "[commit_exp]"],
        "why": "the job of a miner whose commit expired is built, fails, and stops every lease.",
    },
    {
        "id": "exp-authority-drops-the-run-check",
        "invariants": ["BND-16"],
        "path": ESCROW,
        "old": "if model.run_id != self.run_id or not verify(",
        "new": "if not verify(",
        "tests": [
            ONE + "test_a_signed_reward_of_one_run_is_refused_by_the_ledger_of_another",
            ONE + "test_a_signed_genesis_of_one_run_is_refused_by_the_ledger_of_another",
            ONE + "test_a_signed_bootstrap_of_one_run_is_refused_by_the_ledger_of_another",
        ],
        "why": "the ledger no longer refuses an origin authority signed for another run.",
    },
]
