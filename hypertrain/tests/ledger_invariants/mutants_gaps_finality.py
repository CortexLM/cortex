"""Mutants of the finality gap tests: one small regression each, which a named test must catch."""

from __future__ import annotations

G = "tests/ledger_invariants/test_gaps_finality.py::"
STORE = "src/hypertrain/challenge/store.py"
TAPE = "src/hypertrain/aggregator/tape_v2.py"
MESSAGES = "src/hypertrain/protocol/messages_v2.py"

SIGNER_GATE = (
    G + "test_a_finalize_of_the_wrong_role_is_refused_before_the_applied_round_is_looked_up"
)
ROSTER_FIRST = (
    G + "test_the_roster_guard_answers_a_finalize_for_a_round_that_does_not_exist_before_the_signer"
)
TAPE_FACTS = G + "test_a_tape_is_never_built_over_a_replay_that_is_not_a_full_match_of_its_commit"
ENTITLEMENT = (
    G + "test_a_commit_whose_tokens_are_not_the_servers_entitlement_is_refused_on_both_sides"
)
BINDING = G + "test_a_leaf_publication_that_does_not_bind_its_commit_is_refused_and_keeps_nothing"
OBJECT = G + "test_a_leaf_object_that_contradicts_its_commit_is_refused_and_keeps_nothing"
TOKENS = G + "test_a_commit_that_declares_another_entitlement_than_the_servers_is_refused_at_intake"
WINDOWS = (
    G + "test_the_commit_window_closes_for_a_late_member_and_the_upload_window_for_a_late_delta"
)
PREDECESSOR = G + "test_a_round_does_not_open_before_its_predecessor_was_applied"
NO_TAPE = (
    G + "test_a_finalize_for_a_round_whose_audits_are_not_done_is_refused_for_its_missing_tape"
)
WIDE_ROUND = G + "test_a_round_wider_than_one_lease_is_refused_at_opening_or_stays_auditable"
SURVIVORS = G + "test_the_survivors_of_a_round_with_an_exhausted_member_are_paid_their_whole_round"
# Pinned by the tests of the base (PIN_EXISTING): the lot keeps no copy of them.
BASE_POISON = (
    "tests/challenge/test_audit_liveness_v2.py::"
    "test_a_miner_sealed_input_that_cannot_be_leased_does_not_block_the_queue"
)
BASE_NO_SHOW = (
    "tests/challenge/test_no_show_v2.py::"
    "test_a_member_that_never_commits_no_longer_stalls_the_round"
)
BASE_NO_LEAVES = (
    "tests/challenge/test_no_show_v2.py::"
    "test_a_job_without_leaves_does_not_starve_the_audits_behind_it"
)
BASE_TWO_WINDOWS = (
    "tests/challenge/test_audit_liveness_v2.py::"
    "test_a_job_nobody_audits_in_two_windows_drops_its_miner_and_nothing_else"
)
G2 = "tests/ledger_invariants/test_gaps_finality_2.py::"
STALE = G2 + "test_a_finalize_under_a_stale_beacon_is_paused_with_nothing_written_and_settles_later"
BURN = G2 + "test_an_epoch_after_a_refused_finalize_burns_the_whole_share_and_pays_nobody"
BESIDE = G2 + "test_a_refused_round_adds_no_weight_to_the_epoch_answer_of_a_settled_one"
LEAVES_COMPARE = (
    "            if (model.leaves_root, model.delta_hash) != "
    "(commit.leaves_root, commit.delta_hash):\n"
)
PREVIOUS = '                previous = self._record_v2(run_id, "applied", str(body.w - 1))\n'


def mutant(
    id: str, invariants: list[str], path: str, old: str, new: str, tests: list[str], why: str
) -> dict[str, object]:
    return {
        "id": id,
        "invariants": invariants,
        "path": path,
        "old": old,
        "new": new,
        "tests": tests,
        "why": why,
    }


def replay_fact(name: str, field: str, why: str) -> dict[str, object]:
    """The tape input check compares the replay's `field` with the commit's (the assignment hash
    with the roster's); here it compares the value with itself."""
    own = "work" if field == "assignment_hash" else "c"
    return mutant(
        f"gf-tape-ignores-the-replay-{name}",
        ["A2W-01", "A2W-07"],
        TAPE,
        f"            r.{field},\n",
        f"            {own}.{field},\n",
        [TAPE_FACTS],
        why,
    )


MUTANTS = [
    mutant(
        "gf-finalize-has-no-signer-gate",
        ["FIN-01"],
        STORE,
        '            model = self._authenticated_v2(run_id, raw, "Finalize", '
        "manifest.training.coord_pubkey)\n",
        "            model = Finalize.model_validate(envelope_v2.parse_envelope(raw).body)\n",
        [SIGNER_GATE],
        "A Finalize of a miner or an auditor is read like the coordinator's until a later lookup "
        "fails, and the single-use reservation of its signed operation is never taken.",
    ),
    mutant(
        "gf-finalize-gate-before-the-roster-guard",
        ["FIN-01"],
        STORE,
        "            manifest = self._run_v2(run_id)\n"
        "            if self._service_boundary_v2(run_id, w) is not None:\n"
        "                final_request = envelope_v2.Intake(\n"
        '                    run_id, {"Finalize": manifest.training.coord_pubkey.__eq__}\n',
        "            manifest = self._run_v2(run_id)\n"
        '            self._authenticated_v2(run_id, raw, "Finalize", '
        "manifest.training.coord_pubkey)\n"
        "            if self._service_boundary_v2(run_id, w) is not None:\n"
        "                final_request = envelope_v2.Intake(\n"
        '                    run_id, {"Finalize": manifest.training.coord_pubkey.__eq__}\n',
        [ROSTER_FIRST],
        "The signer gate moves in front of the roster guard (the order the Lens asked for): the "
        "test that documents the order of today fails, which is how a maintainer is told to "
        "update it.",
    ),
    mutant(
        "gf-tape-ignores-the-replay-result",
        ["A2W-01", "A2W-07"],
        TAPE,
        '        r.result != "MATCH"\n',
        "        False\n",
        [TAPE_FACTS],
        "A MISMATCH, WITHHELD, BAD_PROOF or ASSIGNMENT_VIOLATION replay still yields a tape "
        "input, so a model nobody proved is applied.",
    ),
    mutant(
        "gf-tape-ignores-the-audit-mode",
        ["A2W-01", "A2W-07"],
        TAPE,
        '        or r.audit_mode != "anchored-full"\n',
        "",
        [TAPE_FACTS],
        "A replay that did not recompute the whole round is accepted as a full audit.",
    ),
    replay_fact(
        "assignment-hash",
        "assignment_hash",
        "A replay of another assignment than the member's own is accepted for its commit.",
    ),
    replay_fact(
        "leaves-root",
        "leaves_root",
        "A replay of other leaves than the committed ones is accepted.",
    ),
    replay_fact(
        "delta-hash", "delta_hash", "A replay of another delta than the committed one is accepted."
    ),
    replay_fact(
        "final-theta-hash",
        "final_theta_hash",
        "A replay that ends in another model state than the committed one is accepted.",
    ),
    replay_fact(
        "ef-in-hash",
        "ef_in_hash",
        "A replay that started from another error-feedback state than the committed one is "
        "accepted.",
    ),
    replay_fact(
        "ef-out-hash",
        "ef_out_hash",
        "A replay that ended in another error-feedback state than the committed one is accepted.",
    ),
    mutant(
        "gf-entitlement-allows-fewer-tokens",
        ["A2W-01", "FIN-17"],
        MESSAGES,
        "        if self.tokens != sample_count * manifest.training.model.seq_len:\n",
        "        if self.tokens > sample_count * manifest.training.model.seq_len:\n",
        [ENTITLEMENT],
        "A commit may declare fewer tokens than the server derived, so its share is the miner's "
        "to shrink.",
    ),
    mutant(
        "gf-leaves-ignore-the-committed-root",
        ["A2W-07"],
        STORE,
        LEAVES_COMPARE,
        "            if (model.delta_hash,) != (commit.delta_hash,):\n",
        [BINDING],
        "A leaf publication may name another leaves root than the one its commit committed to.",
    ),
    mutant(
        "gf-leaves-ignore-the-committed-delta",
        ["A2W-07"],
        STORE,
        LEAVES_COMPARE,
        "            if (model.leaves_root,) != (commit.leaves_root,):\n",
        [BINDING],
        "A leaf publication may name another delta than the one its commit committed to.",
    ),
    mutant(
        "gf-leaves-bind-no-commit",
        ["A2W-07"],
        STORE,
        "            if len(found) != 1:\n",
        "            if len(found) > 1:\n",
        [BINDING],
        "A leaf publication that matches no accepted commit is read as if it did.",
    ),
    mutant(
        "gf-leaves-accept-any-leaf-count",
        ["A2W-07"],
        STORE,
        "                len(parsed) != commit.n_leaves\n"
        "                or any(p.run_id != run_id or p.w != commit.w for p in parsed)\n",
        "                any(p.run_id != run_id or p.w != commit.w for p in parsed)\n",
        [OBJECT],
        "A leaf object with fewer leaves than the commit declares is published for it.",
    ),
    mutant(
        "gf-leaves-accept-leaves-of-another-round",
        ["A2W-07"],
        STORE,
        "                len(parsed) != commit.n_leaves\n"
        "                or any(p.run_id != run_id or p.w != commit.w for p in parsed)\n",
        "                len(parsed) != commit.n_leaves\n",
        [OBJECT],
        "A leaf object that carries a leaf of another round is published for the commit.",
    ),
    mutant(
        "gf-commit-takes-any-token-count",
        ["A2W-01", "FIN-17"],
        STORE,
        "                    model.validate_assignment(manifest, samples)\n",
        "                    pass\n",
        [TOKENS],
        "A commit is stored with the token count the miner chose instead of the server's.",
    ),
    mutant(
        "gf-commit-window-never-closes",
        ["A2W-07"],
        STORE,
        '            if action in ("accept", "commit") '
        "and not body.d_assign <= now < body.d_commit:\n",
        '            if False and action in ("accept", "commit"):\n',
        [WINDOWS],
        "A member can accept and commit after the commit window of its round has closed.",
    ),
    mutant(
        "gf-upload-window-never-closes",
        ["A2W-07"],
        STORE,
        '            if action == "delta" and now >= body.d_upload:\n',
        "            if False:\n",
        [WINDOWS],
        "A delta manifest is accepted after the upload window of its round has closed.",
    ),
    mutant(
        "gf-opening-needs-no-applied-predecessor",
        ["FIN-12", "FIN-13"],
        STORE,
        "            if body.w:\n" + PREVIOUS,
        "            if False:\n" + PREVIOUS,
        [PREDECESSOR],
        "Round w+1 opens on the genesis state although round w was never applied.",
    ),
    mutant(
        "gf-finalize-needs-no-applied-tape",
        ["A2W-04", "FIN-02"],
        STORE,
        '            applied = self._record_v2(run_id, "applied", str(w))\n'
        '            if applied.get("kind") == "repair":\n',
        '            applied = {"tape_hash": "", "theta_hash": "", "out_state": ""}\n'
        '            if applied.get("kind") == "repair":\n',
        [NO_TAPE],
        "A Finalize for a round that was never aggregated is read as if a tape had been applied.",
    ),
    mutant(
        "gf-horizon-leaves-a-queued-job-open",
        ["A2W-20", "FIN-22"],
        STORE,
        '            elif row["state"] == "queued" and now >= row["absolute"]:\n',
        "            elif False:\n",
        [BASE_TWO_WINDOWS],
        "A job nobody could serve is never exhausted, so its member keeps the round open for ever.",
    ),
    mutant(
        "gf-lease-blocks-on-a-job-without-leaves",
        ["A2W-18", "FIN-22"],
        STORE,
        "                    continue  # leaves have no deadline: leasable once posted, "
        "never blocking\n",
        "                    pass\n",
        [BASE_NO_LEAVES],
        "A queued job whose leaves were never posted heads the queue and every ask answers 409, so "
        "the jobs behind it are never leased.",
    ),
    mutant(
        "gf-lease-dates-the-job-at-its-creation",
        ["A2W-16"],
        STORE,
        "                created_beacon=now,\n",
        '                created_beacon=row["created"],\n',
        [WIDE_ROUND],
        "A lease taken late in a window wider than one lease is dated at the audit round, so the "
        "job outlives the 100 rounds AuditJobV2 allows and cannot be built.",
    ),
    mutant(
        "gf-lease-blocks-on-a-miner-sealed-input",
        ["A2W-17", "CRD-D02"],
        STORE,
        "                refusal = self._refused_miner_input_v2(commit_env, leaves, now)\n",
        "                refusal = None\n",
        [BASE_POISON],
        "A commit that expired or leaves with a non-finite metric still head the queue, so the "
        "audits behind them are never leased.",
    ),
    mutant(
        "gf-schedule-keeps-the-round-for-a-member-that-never-committed",
        ["A2W-20", "FIN-21"],
        STORE,
        "                if identity in excluded:\n"
        "                    continue\n"
        '                commit_env = self._record_v2(run_id, "commit", identity)\n',
        '                commit_env = self._record_v2(run_id, "commit", identity)\n',
        [BASE_NO_SHOW],
        "The scheduling no longer skips a member left out of the round, so the first roster "
        "member without a commit refuses it and the members that committed are never audited.",
    ),
    mutant(
        "gf-no-show-keeps-its-audit-open",
        ["A2W-20", "FIN-21"],
        STORE,
        '                finality["unresolved_audits"] = '
        'max(0, int(finality["unresolved_audits"]) - 1)\n',
        '                finality["unresolved_audits"] = int(finality["unresolved_audits"])\n',
        [BASE_NO_SHOW],
        "A member left out for never committing keeps one audit open, so its round can be neither "
        "aggregated nor settled.",
    ),
    mutant(
        "gf-an-exhausted-job-voids-the-round",
        ["FIN-19", "A2W-22"],
        STORE,
        '        finality = self._record_v2(run_id, "finality", str(row["w"]))\n'
        '        finality["unresolved_audits"] = '
        'max(0, int(str(finality["unresolved_audits"])) - 1)\n',
        '        finality = self._record_v2(run_id, "finality", str(row["w"]))\n'
        '        finality["disposition"] = "EXCLUDED"\n'
        '        finality["resolution_hash"] = sha256_hex(row["id"].encode())\n'
        '        finality["rollback_complete"] = not finality["applied"]\n'
        '        finality["unresolved_audits"] = '
        'max(0, int(str(finality["unresolved_audits"])) - 1)\n',
        [SURVIVORS],
        "One job nobody could audit marks the whole round EXCLUDED, so the members whose audits "
        "matched are never settled and never paid.",
    ),
    mutant(
        "gf-finalize-skips-the-freshness-gate",
        ["FIN-04", "FIN-23"],
        STORE,
        '                now = self._fresh_v2(manifest, f"finalize:{w}:{self._now(self._db)}")\n',
        "                now = self._now(self._db)\n",
        [STALE],
        "A Finalize is settled and minted while the beacon feed is paused.",
    ),
    # Three guards keep the collateral of the genesis out of the epoch answer: the hotkey test and
    # the pending-reward test of EscrowV2.payable, and the skip in _pay_v2 of a payee that
    # _payee_v2 cannot name (a NULL hotkey). The pin sees no single revert of them, so this one
    # answers the epoch from every available unit of the ledger, which bypasses the three.
    mutant(
        "gf-payable-includes-collateral-origins",
        ["WGT-10", "FIN-02"],
        STORE,
        "            for row in escrow.payable(now):\n"
        "                if budget == 0:\n"
        "                    break\n"
        '                payee = self._payee_v2(escrow.run_id, row["hotkey"])\n'
        "                if payee is None:\n",
        "            for row in escrow.db.execute(\n"
        '                "SELECT u.origin,u.owner,COALESCE(o.hotkey,u.owner) AS hotkey,u.units "\n'
        '                "FROM escrow_units u JOIN escrow_origins o ON o.origin=u.origin "\n'
        "                \"WHERE u.bucket='available' AND u.units>0 AND o.mature_at<=? \"\n"
        '                "ORDER BY o.mature_at,u.origin,u.owner",\n'
        "                (now,),\n"
        "            ).fetchall():\n"
        "                if budget == 0:\n"
        "                    break\n"
        '                payee = row["hotkey"]\n'
        "                if False:\n",
        [BESIDE],
        "The epoch answer is filled from every available unit, so the collateral origins of the "
        "genesis are emitted as weight beside the settled round.",
    ),
    mutant(
        "gf-settle-refusal-is-swallowed",
        ["FIN-02", "WGT-10"],
        STORE,
        "            settled = settle(status, model, final_hash)\n",
        "            try:\n"
        "                settled = settle(status, model, final_hash)\n"
        "            except Exception:\n"
        "                settled = status\n",
        [BURN, BESIDE],
        "A round that may not settle is minted anyway and its units reach the epoch answer.",
    ),
]
