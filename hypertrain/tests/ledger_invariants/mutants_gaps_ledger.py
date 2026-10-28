"""Mutants of the ledger gap wave (test_gaps_ledger.py).

Two kinds of entry. A `gapledger-*` mutant is a realistic regression that a pin of the wave must
catch. A `fixproof-*` entry is the opposite: the smallest change of src that closes a hole the
wave records as a strict xfail; the runner calls it KILLED when the xfail then passes
(XPASS(strict) fails the test), which is the machine-checked proof that the hole is what its
reason says and that the asserted defence is reachable. When a maintainer applies the real fix,
its `fixproof-*` entry (whose snippet is then gone) and the xfail marker go together.

Run: scripts/check_ledger_invariant_mutants.py --only <id> ... (see its --help).
"""

from __future__ import annotations

from typing import Any

APP = "src/hypertrain/challenge/app.py"
STORE = "src/hypertrain/challenge/store.py"
REPLAY = "src/hypertrain/auditor/replay.py"

G = "tests/ledger_invariants/test_gaps_ledger.py::"
SCRUB_LIST = (
    G + "test_the_scrub_list_names_every_environment_variable_the_challenge_and_ledger_read"
)
SCRUB_WORKS = G + "test_the_scrub_removes_every_name_the_service_reads_and_leaves_its_defaults"
POLLUTED = G + "test_a_service_the_lot_builds_ignores_a_polluted_environment"
RERUN_KEPT = G + "test_a_rerun_that_is_not_the_auditors_root_keeps_the_forfeit_and_the_blacklist"
RERUN_COPIED = G + "test_a_rerun_claim_copied_from_the_published_verdict_does_not_clear_a_mismatch"
LATER_BEACON = G + (
    "test_an_identical_finalize_at_a_later_beacon_returns_the_stored_answer_and_changes_nothing"
)
EXPIRED_ENVELOPE = G + (
    "test_a_stored_finalize_is_not_answered_to_an_expired_envelope_and_is_to_a_fresh_one"
)
VESTING = (
    G + "test_the_store_vests_each_reward_origin_a_fixed_span_after_the_beacon_it_finalized_at"
)
OWNERSHIP = G + (
    "test_a_rotation_between_the_credit_and_the_pay_keeps_every_unit_with_the_coldkey_that_earned_it"
)
ROTATED = G + (
    "test_the_units_of_a_rotated_miner_are_emitted_to_the_hotkey_its_admission_holds_now"
)

REPEATED_COMPLETION = G + (
    "test_a_repeated_audit_completion_neither_decrements_the_counter_again_nor_changes_anything"
)
ANOTHER_ROUND = G + (
    "test_a_challenge_issued_for_another_round_is_no_audit_of_this_round_and_credits_nothing"
)
ESCROW = "src/hypertrain/ledger/escrow_v2.py"
ADMISSION = "src/hypertrain/challenge/admission.py"
BANS_ONLY_THE_LOSER = G + (
    "test_a_lost_fraud_contest_bans_only_the_loser_for_proven_training_fraud"
)
STAKE_BURNED = G + (
    "test_the_stake_of_a_convicted_admission_is_burned_and_cannot_be_released_by_its_owner"
)
REPAIRED = G + "test_a_repaired_round_cannot_be_finalized_and_the_weights_stay_a_full_burn"
ORDINARY_TRIAL = G + (
    "test_an_ordinary_trial_through_the_routes_creates_no_shadow_record_and_issues_nothing"
)

AUTHENTICATE_FINALIZE = (
    '            model = self._authenticated_v2(run_id, raw, "Finalize", '
    "manifest.training.coord_pubkey)\n"
    "            assert isinstance(model, Finalize)\n"
)

MUTANTS: list[dict[str, Any]] = [
    # --- M-17: the environment the lot runs in ------------------------------------------------
    {
        "id": "gapledger-config-gains-a-knob-the-scrub-does-not-list",
        "invariants": ["M-17", "T-01"],
        "path": APP,
        "old": 'owner_hotkey=env("HYPERTRAIN_OWNER_HOTKEY") or None,',
        "new": 'owner_hotkey=env("HYPERTRAIN_OWNER_HOTKEY") or env("HYPERTRAIN_NEW_KNOB") or None,',
        "tests": [SCRUB_LIST, SCRUB_WORKS],
        "why": "Config.from_env reads one more variable than the lot scrubs, so an ambient value "
        "could change what a service of the lot does.",
    },
    {
        "id": "gapledger-the-internal-bearer-file-comes-from-the-environment",
        "invariants": ["M-17", "T-01"],
        "path": APP,
        "old": "        _require(config.internal_token_file, authorization)\n",
        "new": "        _require(\n"
        "            Config.from_env().internal_token_file or config.internal_token_file, "
        "authorization\n"
        "        )\n",
        "tests": [POLLUTED],
        "why": "A service built with explicit paths still takes its internal bearer file from the "
        "ambient environment, so a polluted shell decides who may ask for weights.",
    },
    # --- GAP-10: the v1 rerun ------------------------------------------------------------------
    {
        "id": "gapledger-a-rerun-that-matches-nothing-is-forgiven",
        "invariants": ["GAP-10"],
        "path": REPLAY,
        "old": '        return "CONTEST"\n    return "FAULT"\n',
        "new": '        return "CONTEST"\n    return "TRANSIENT"\n',
        "tests": [RERUN_KEPT + "[another root-FAULT]"],
        "why": "A rerun root that is neither the auditor's nor the commitment is forgiven as a "
        "transient fault instead of being a FAULT, so any signed junk clears a MISMATCH.",
    },
    {
        "id": "gapledger-a-rerun-that-stands-by-the-commit-is-forgiven",
        "invariants": ["GAP-10"],
        "path": REPLAY,
        "old": '        return "CONTEST"\n',
        "new": '        return "TRANSIENT"\n',
        "tests": [RERUN_KEPT + "[its own commit-CONTEST]"],
        "why": "A miner that stands by its commitment is forgiven as a transient fault instead "
        "of entering the contest, so it keeps the reward its commitment lost.",
    },
    {
        "id": "fixproof-a-rerun-copied-from-the-verdict-is-not-forgiven",
        "invariants": ["GAP-10"],
        "path": REPLAY,
        "old": '        return "TRANSIENT" if transients_this_epoch < forgive_per_epoch '
        'else "FAULT"\n',
        "new": '        return "FAULT"\n',
        "tests": [RERUN_COPIED],
        "why": "FIX PROOF: a claim that is only the published recomputed root no longer clears the "
        "forfeit (the honest path then needs evidence the maintainers have yet to choose).",
    },
    # --- M-01: a finalize is idempotent at a later beacon --------------------------------------
    {
        "id": "gapledger-a-later-finalize-is-evaluated-again-instead-of-answered",
        "invariants": ["M-01", "FIN-05", "CRD-02"],
        "path": STORE,
        "old": """            if previous_final is not None:
                if json.loads(previous_final[0])["body"] != model.model_dump(mode="json"):
                    raise ChallengeError(409, "conflicting finalized origin authority")
                return self._record_v2(run_id, "finalize-result", str(w))
""",
        "new": "",
        "tests": [LATER_BEACON],
        "why": "The stored-finalize early return is gone, so the same Finalize at a later beacon "
        "reaches the ledger with a vesting that moved and is refused as another operation.",
    },
    {
        "id": "gapledger-a-stored-finalize-is-answered-before-the-envelope-is-authenticated",
        "invariants": ["M-01", "FIN-05"],
        "path": STORE,
        "old": AUTHENTICATE_FINALIZE,
        "new": """            stored = self._db.execute(
                "SELECT data FROM records_v2 WHERE run_id=? AND kind='finalize-result' AND id=?",
                (run_id, str(w)),
            ).fetchone()
            if stored is not None:
                return dict(json.loads(stored[0]))
"""
        + AUTHENTICATE_FINALIZE,
        "tests": [EXPIRED_ENVELOPE],
        "why": "The stored result is returned before the Finalize is authenticated, so an "
        "envelope whose expiry has passed is answered like a live one.",
    },
    # --- GAP-03: the vesting span the store hands the ledger -----------------------------------
    {
        "id": "gapledger-the-store-vests-a-reward-at-once",
        "invariants": ["GAP-03", "CRD-20"],
        "path": STORE,
        "old": "            vesting = now + manifest.training.verify.E_vest_rounds\n",
        "new": "            vesting = now\n",
        "tests": [VESTING],
        "why": "The origins of a round mature at the beacon of their own finalize, leaving no "
        "window for a contest to cost them.",
    },
    {
        "id": "gapledger-the-store-vests-a-reward-one-beacon-short",
        "invariants": ["GAP-03", "CRD-20"],
        "path": STORE,
        "old": "            vesting = now + manifest.training.verify.E_vest_rounds\n",
        "new": "            vesting = now + manifest.training.verify.E_vest_rounds - 1\n",
        "tests": [VESTING],
        "why": "The vesting span is one beacon shorter than the manifest says.",
    },
    # --- T-CRD-18: ownership through a rotation ------------------------------------------------
    {
        "id": "gapledger-a-payment-rewrites-the-owner-of-the-units-to-the-hotkey",
        "invariants": ["T-CRD-18", "CRD-12"],
        "path": STORE,
        "old": '                escrow.pay(op, signer=row["owner"])\n',
        "new": '                escrow.pay(op, signer=row["owner"])\n'
        "                escrow.db.execute(\n"
        '                    "UPDATE escrow_units SET owner=? WHERE origin=?",\n'
        '                    (row["hotkey"], row["origin"]),\n'
        "                )\n",
        "tests": [OWNERSHIP],
        "why": "Paying an origin re-keys its units from the coldkey that earned them to the "
        "hotkey the weights name, the shape of a naive fix for the rotated payee.",
    },
    {
        "id": "gapledger-the-units-of-a-rotated-miner-are-paid-to-the-hotkey-that-signed-the-work",
        "invariants": ["CRD-12", "WGT-13", "CL-29"],
        "path": STORE,
        "old": '        return None if row["banned"] else str(row["hotkey"])\n',
        "new": '        return None if row["banned"] else hotkey\n',
        "tests": [ROTATED],
        "why": "The weight of a matured origin goes to the hotkey that signed the work, not to "
        "the one its admission holds now, so a rotated miner's quarter is emitted to a key it no "
        "longer runs.",
    },
    # --- T-CRD-04 and T-CRD-12 -----------------------------------------------------------------
    {
        "id": "gapledger-the-audit-binding-ignores-the-round-of-the-challenge",
        "invariants": ["T-CRD-04", "CRD-07"],
        "path": ESCROW,
        "old": "or (challenge.w, challenge.target) != (c.w, c.hotkey)",
        "new": "or challenge.target != c.hotkey",
        "tests": [ANOTHER_ROUND + "[challenge_of_another_round]"],
        "why": "A challenge the coordinator issued for round 1 is accepted as the audit of the "
        "commit of round 0, so one audit can be spent on any round.",
    },
    {
        "id": "gapledger-a-repeated-completion-decrements-the-open-audit-counter-again",
        "invariants": ["T-CRD-12", "CRD-23"],
        "path": STORE,
        "old": """                    raise ChallengeError(409, "audit replay conflicts")
                return {"result": row["result"]}
""",
        "new": """                    raise ChallengeError(409, "audit replay conflicts")
                again = self._record_v2(run_id, "finality", str(row["w"]))
                again["unresolved_audits"] = max(0, int(str(again["unresolved_audits"])) - 1)
                self._put_record_v2(run_id, "finality", str(row["w"]), again)
                return {"result": row["result"]}
""",
        "tests": [REPEATED_COMPLETION],
        "why": "The completed-lease branch also decrements the counter, so the answer is the "
        "same and the round believes four audits are still open when five are done.",
    },
    # --- T-SHD-03: what an ordinary trial leaves behind -----------------------------------------
    {
        "id": "gapledger-the-route-driven-reference-retains-accepted-custody",
        "invariants": ["T-SHD-03", "SHD-06", "SHD-24"],
        "path": STORE,
        "old": "self, run_id: str, admission_id: str, *, retain_shadow_custody: bool = False",
        "new": "self, run_id: str, admission_id: str, *, retain_shadow_custody: bool = True",
        "tests": [ORDINARY_TRIAL],
        "why": "The reference an application triggers through its route retains accepted "
        "shadow-execution custody, the raw material of a shadow reward, in an ordinary flow.",
    },
    # --- M-02: a repaired round ------------------------------------------------------------------
    {
        "id": "gapledger-a-repaired-round-is-finalized-like-a-round-of-work",
        "invariants": ["M-02", "CRD-04"],
        "path": STORE,
        "old": """            if applied.get("kind") == "repair":
                raise ChallengeError(
                    409, "repair evidence is model-only; cannot mint miner rewards"
                )
""",
        "new": "",
        "tests": [REPAIRED],
        "why": "A round whose tape the rollback repaired is finalized and credited like a round "
        "of audited work, minting rewards for model history only.",
    },
    # --- T-CRD-20: proven fraud costs the admission, and only that admission ------------------
    {
        "id": "gapledger-a-resolution-no-longer-convicts-the-admission",
        "invariants": ["T-CRD-20", "CRD-21"],
        "path": STORE,
        "old": "                            admission.settle_fraud("
        'result["dispute_id"], now=now)\n',
        "new": "",
        "tests": [BANS_ONLY_THE_LOSER, STAKE_BURNED],
        "why": "The accepted referee resolution settles the contest bond and stops there: the "
        "admission is never banned and its stake stays locked.",
    },
    {
        "id": "gapledger-a-conviction-seizes-the-bond-of-every-admission",
        "invariants": ["T-CRD-20", "CRD-21"],
        "path": ADMISSION,
        "old": """            self.seize_bond(
                r.admission_id,
                reason="TRAINING_FRAUD",
                evidence_hash=pending["evidence_hash"],
                now=now,
            )
""",
        "new": """            rows = self.store.db.execute("SELECT admission_id FROM admissions_v2")
            for other in rows:
                self.seize_bond(
                    other["admission_id"],
                    reason="TRAINING_FRAUD",
                    evidence_hash=pending["evidence_hash"],
                    now=now,
                )
""",
        "tests": [BANS_ONLY_THE_LOSER],
        "why": "A training-fraud conviction bans and burns the stake of every admission of the "
        "run, not of the miner the referee ruled against.",
    },
    {
        "id": "gapledger-a-convicted-admission-is-banned-under-another-reason",
        "invariants": ["T-CRD-20", "CRD-21"],
        "path": ADMISSION,
        "old": 'self.store.transition(admission_id, "BANNED", "FINALIZED_TRAINING_FRAUD", now)',
        "new": 'self.store.transition(admission_id, "BANNED", "TRAINING_FRAUD", now)',
        "tests": [BANS_ONLY_THE_LOSER],
        "why": "The ban carries a reason no other part of the service recognises as proven "
        "training fraud.",
    },
    {
        "id": "gapledger-a-conviction-burns-no-stake",
        "invariants": ["T-CRD-20", "CRD-21"],
        "path": ADMISSION,
        "old": """            for lock in self._bond_locks(admission_id):
                origins, units = self._residual(lock)
                if not units:
                    continue
                self.store.db.execute(
                    "INSERT OR IGNORE INTO admission_seizures""",
        "new": """            for lock in ():
                origins, units = self._residual(lock)
                if not units:
                    continue
                self.store.db.execute(
                    "INSERT OR IGNORE INTO admission_seizures""",
        "tests": [STAKE_BURNED],
        "why": "The conviction bans the admission but finds no lock to seize, so the stake stays "
        "locked behind a banned identity.",
    },
    {
        "id": "gapledger-a-fraud-settled-lock-is-releasable",
        "invariants": ["T-CRD-20", "ES-04"],
        "path": ESCROW,
        "old": """                status.unresolved
                or status.outcome == "FRAUD"
                or now_beacon < status.release_beacon""",
        "new": """                status.unresolved
                or now_beacon < status.release_beacon""",
        "tests": [STAKE_BURNED],
        "why": "A release no longer looks at the outcome of the settlement, so the owner of a "
        "lock settled as FRAUD is not refused as disputed.",
    },
]
