"""Mutants of the four IMPORTANT of Lens' verdict on the merged tree 389c45b4 and of IMP-8.

Snippets are the exact source text, indentation included, so they sit at column 0 below.
Run from hypertrain/: .venv/bin/python scripts/check_ledger_invariant_mutants.py --only <ids>
"""

from __future__ import annotations

APP = "src/hypertrain/challenge/app.py"
STORE = "src/hypertrain/challenge/store.py"
ADMISSION = "src/hypertrain/challenge/admission.py"
EXIT = "src/hypertrain/challenge/exit_v2.py"
COLDKEY_OPS = "src/hypertrain/miner/coldkey_ops.py"

SWITCH = "tests/challenge/test_audit_mismatch_switch_v2.py::"
REAPER = "tests/challenge/test_exit_reaper_v2.py::"
BOND = "tests/challenge/test_bond_terms_v2.py::"
IDS = "tests/challenge/test_escrow_server_ids_v2.py::"
READ = "tests/challenge/test_object_read_v2.py::"
OPS = "tests/miner/test_coldkey_ops.py::"
CONTEST = "tests/challenge/test_exit_contest_v2.py::"
PLAN = "tests/challenge/test_exit_plan_v2.py::"

STORE_DEFAULT = SWITCH + "test_a_store_built_without_the_keyword_enforces_a_confirmed_mismatch"
EVIDENCE_BINDING = REAPER + "test_a_stored_mismatch_verdict_backs_only_the_exclusion_that_names_it"
NO_IDENTIFIER = IDS + "test_no_anonymous_route_returns_an_escrow_identifier"
PUBLIC_ROUTE = IDS + "test_no_public_route_returns_an_origin_id"
PUBLIC_STATUS = (
    BOND + "test_the_public_status_of_a_bonded_admission_keeps_the_amounts_and_names_no_identifier"
)
FORGERIES = BOND + "test_the_bond_route_refuses_everyone_but_the_coldkey_of_the_admission"
OPERATOR_BEARER = BOND + "test_the_operator_route_wants_the_operator_bearer"
DEREGISTERED = BOND + "test_a_hotkey_that_left_the_subnet_does_not_strand_its_bond"
CLI_BOND = OPS + "test_escrow_bond_signs_a_bond_request_with_the_coldkey_and_prints_the_terms"
CLI_OWNER = OPS + "test_escrow_bond_refuses_another_admission_owner_without_posting"
STREAMED = READ + "test_a_large_object_is_streamed_in_bounded_blocks_with_bounded_memory"
BAD_KEY = READ + "test_a_key_that_is_not_64_lowercase_hex_is_a_400"
ABSENT = READ + "test_an_object_nobody_stored_is_a_404"
CAP = READ + "test_an_object_above_the_read_cap_is_refused_before_a_byte_is_read"
HELD = CONTEST + "test_a_silent_member_with_a_contest_open_is_left_out_but_not_sanctioned_yet"
LOST_AFTER = (
    CONTEST
    + "test_a_contest_lost_after_the_exclusion_covers_the_absence_and_nothing_more_is_burned"
)
LOST_BEFORE = CONTEST + "test_a_contest_lost_before_the_bound_covers_the_absence_from_the_start"
NO_LOSER = CONTEST + "test_a_contest_a_closure_ended_without_a_loser_leaves_the_no_show_to_apply"
OLD_LOSS = CONTEST + "test_a_contest_lost_before_the_round_opened_covers_no_absence_of_this_round"
BOUND = CONTEST + "test_a_hold_is_dropped_at_the_bound_of_the_wait_and_sanctions_nobody"
ACTIONS = (
    PLAN + "test_a_no_show_with_a_contest_of_its_own_waits_for_it_is_covered_by_a_loss_or_applies"
)

MUTANTS = [
    {
        "id": "imp1-store-default-not-enforcing",
        "invariants": ["IMP-1", "D81"],
        "path": STORE,
        "old": """        max_roster: int = 32,
        enforce_audit_mismatch: bool = True,
""",
        "new": """        max_roster: int = 32,
        enforce_audit_mismatch: bool = False,
""",
        "tests": [STORE_DEFAULT],
        "why": "a store built without the keyword (the bootstrap scripts) records a confirmed "
        "mismatch and never sanctions it.",
    },
    {
        "id": "imp2-verdict-evidence-binding-removed",
        "invariants": ["IMP-2", "D81"],
        "path": STORE,
        "old": """        if body_digest(signed.body) != evidence_hash:
            return None
        verdict = ReplayVerdict.model_validate(signed.body)
""",
        "new": """        verdict = ReplayVerdict.model_validate(signed.body)
""",
        "tests": [EVIDENCE_BINDING],
        "why": "an exclusion that names another verdict is graded by the stored one and burns "
        "the bond for a verdict it never named.",
    },
    {
        "id": "imp3-status-lists-the-full-bond",
        "invariants": ["IMP-3", "D51", "D52-I2"],
        "path": STORE,
        "old": """admission.public_bond(""",
        "new": """admission.bond(""",
        "tests": [PUBLIC_STATUS, NO_IDENTIFIER, PUBLIC_ROUTE],
        "why": "the anonymous status names the lock, its origin ids and the release hashes.",
    },
    {
        "id": "imp3-public-fields-gain-the-finality-hash",
        "invariants": ["IMP-3", "D51"],
        "path": ADMISSION,
        "old": """PUBLIC_BOND_FIELDS = ("units", "outcome", "releasable", "release_beacon")""",
        "new": (
            'PUBLIC_BOND_FIELDS = ("units", "outcome", "releasable", "release_beacon", '
            '"finality_hash")'
        ),
        "tests": [PUBLIC_STATUS, NO_IDENTIFIER],
        "why": "the anonymous status keeps a hash that a release is bound to.",
    },
    {
        "id": "imp3-the-hotkey-may-ask-for-the-bond-terms",
        "invariants": ["IMP-3", "D51"],
        "path": APP,
        "old": """        holder = (await run(admissions.store.by_id, admission_id)).coldkey
""",
        "new": """        holder = (await run(admissions.store.by_id, admission_id)).hotkey
""",
        "tests": [FORGERIES, DEREGISTERED],
        "why": "the hotkey, which a miner process holds, reads the terms the coldkey signs for.",
    },
    {
        "id": "imp3-bond-request-not-bound-to-the-admission",
        "invariants": ["IMP-3", "D51"],
        "path": APP,
        "old": """                and admission_id == asked.admission_id
""",
        "new": "",
        "tests": [FORGERIES],
        "why": "the coldkey's request for one admission opens the terms of another of its own.",
    },
    {
        "id": "imp3-bond-request-expiry-unbounded",
        "invariants": ["IMP-3", "D51"],
        "path": APP,
        "old": """                and envelope.exp_drand <= now + REQUEST_HORIZON
            )
        except (EnvelopeError, ValidationError):
            valid = False
        if not valid:
            raise ChallengeError(403, "the bond terms""",
        "new": """            )
        except (EnvelopeError, ValidationError):
            valid = False
        if not valid:
            raise ChallengeError(403, "the bond terms""",
        "tests": [FORGERIES],
        "why": "a captured request, signed far ahead, opens the terms for as long as it lives.",
    },
    {
        "id": "imp3-bond-request-message-type-unchecked",
        "invariants": ["IMP-3", "D51"],
        "path": APP,
        "old": """                isinstance(asked, BondRequest)
                and admission_id == asked.admission_id""",
        "new": """                admission_id == asked.admission_id""",
        "tests": [FORGERIES],
        "why": "another request of the admission (a challenge or a job request) opens the terms.",
    },
    {
        "id": "imp3-operator-bond-route-without-the-bearer",
        "invariants": ["IMP-3", "D51"],
        "path": APP,
        "old": """        admin(authorization)
        return await run(store.for_run(run_id).admission_v2, run_id, "bond", b"", admission_id)""",
        "new": (
            "        return await run("
            'store.for_run(run_id).admission_v2, run_id, "bond", b"", admission_id)'
        ),
        "tests": [OPERATOR_BEARER],
        "why": "the operator route answers anyone.",
    },
    {
        "id": "imp3-bond-route-gated-by-the-hotkey-registration",
        "invariants": ["IMP-3", "D51"],
        "path": APP,
        "old": """        holder = (await run(admissions.store.by_id, admission_id)).coldkey
""",
        "new": """        record = await run(admissions.store.by_id, admission_id)
        await subnet_member(record.hotkey)
        holder = record.coldkey
""",
        "tests": [DEREGISTERED],
        "why": "a hotkey that left the subnet strands the bond terms its coldkey needs to release.",
    },
    {
        "id": "imp3-cli-bond-request-signed-too-far-ahead",
        "invariants": ["IMP-3", "D51"],
        "path": COLDKEY_OPS,
        "old": """        now + REQUEST_HORIZON,
""",
        "new": """        now + REQUEST_HORIZON + 100,
""",
        "tests": [CLI_BOND],
        "why": "the operator tool signs a request the master refuses as too distant.",
    },
    {
        "id": "imp3-cli-does-not-check-the-owner",
        "invariants": ["IMP-3", "D51"],
        "path": COLDKEY_OPS,
        "old": """    if record["coldkey"] != coldkey.ss58:
        raise MinerError("admission coldkey differs from keyfile owner")
    admission_id = TypeAdapter(Hex64).validate_python(record["admission_id"])
    now""",
        "new": """    admission_id = TypeAdapter(Hex64).validate_python(record["admission_id"])
    now""",
        "tests": [CLI_OWNER],
        "why": "the tool signs and posts a request for an admission the keyfile does not own.",
    },
    {
        "id": "imp8-object-read-whole-in-one-block",
        "invariants": ["IMP-8"],
        "path": APP,
        "old": """await run(file.read, OBJECT_READ_BLOCK)""",
        "new": """await run(file.read, size)""",
        "tests": [STREAMED],
        "why": "the route holds the whole object in memory again.",
    },
    {
        "id": "imp8-object-read-cap-never-refuses",
        "invariants": ["IMP-8"],
        "path": APP,
        "old": """        if size > OBJECT_READ_MAX:
""",
        "new": """        if False:
""",
        "tests": [CAP],
        "why": "an object above the ceiling is served.",
    },
    {
        "id": "imp8-malformed-key-is-a-500",
        "invariants": ["IMP-8"],
        "path": APP,
        "old": """            except ValueError:
                raise ChallengeError(400, "object key must be 64 lowercase hex chars") from None""",
        "new": """            except KeyError:
                raise ChallengeError(400, "object key must be 64 lowercase hex chars") from None""",
        "tests": [BAD_KEY],
        "why": "a key that is not 64 hex raises out of the handler.",
    },
    {
        "id": "imp8-absent-object-is-a-500",
        "invariants": ["IMP-8"],
        "path": APP,
        "old": """            except ObjectNotFound:
                raise ChallengeError(404, "unknown object") from None""",
        "new": """            except KeyError:
                raise ChallengeError(404, "unknown object") from None""",
        "tests": [ABSENT],
        "why": "an object nobody stored raises out of the handler.",
    },
    {
        "id": "imp8-object-read-without-content-length",
        "invariants": ["IMP-8"],
        "path": APP,
        "old": """            headers={"Content-Length": str(size)},
""",
        "new": """            headers={},
""",
        "tests": [STREAMED],
        "why": "a client cannot tell a streamed object from a cut one.",
    },
    {
        "id": "imp4-no-show-never-waits-for-the-contest",
        "invariants": ["IMP-4", "D86"],
        "path": STORE,
        "old": """                    elif record["reason"] == "NO_SHOW":
""",
        "new": """                    elif record["reason"] == "NEVER":
""",
        "tests": [HELD, LOST_AFTER, LOST_BEFORE, BOUND],
        "why": "a member kept out by its own contest is sanctioned for the contest and again for "
        "the absence it caused.",
    },
    {
        "id": "imp4-every-contest-holds-the-no-show",
        "invariants": ["IMP-4", "D86"],
        "path": STORE,
        "old": """            contest_open=any(turn.resolution is None for turn in turns),
""",
        "new": """            contest_open=True,
""",
        "tests": [NO_LOSER],
        "why": "a contest it won, or a closure without a loser, spares the member its NO_SHOW.",
    },
    {
        "id": "imp4-every-member-waits-for-any-contest",
        "invariants": ["IMP-4", "D86"],
        "path": STORE,
        "old": """                "OR json_extract(turn,'$.contest.admission_id')=?",
""",
        "new": """                "OR 1 OR json_extract(turn,'$.contest.admission_id')=?",
""",
        "tests": [HELD],
        "why": "a plain silent member is never punished while anyone else has a contest.",
    },
    {
        "id": "imp4-the-hold-is-never-released",
        "invariants": ["IMP-4", "D86"],
        "path": STORE,
        "old": """            bound_reached=status.closed or now >= BOUNDS[Wait.DISPUTE](schedule),
""",
        "new": """            bound_reached=False,
""",
        "tests": [BOUND],
        "why": "a contest nobody resolves holds the sanction for ever.",
    },
    {
        "id": "imp4-a-closed-round-keeps-the-hold",
        "invariants": ["IMP-4", "D86"],
        "path": STORE,
        "old": """            bound_reached=status.closed or now >= BOUNDS[Wait.DISPUTE](schedule),
""",
        "new": """            bound_reached=now >= BOUNDS[Wait.DISPUTE](schedule),
""",
        "tests": [BOUND],
        "why": "the sanction of a round that is closed still waits for a contest.",
    },
    {
        "id": "imp4-covering-erases-the-contest-sanction",
        "invariants": ["IMP-4", "D86"],
        "path": STORE,
        "old": """                        if action != "APPLY":
                            if action != "WAIT":
""",
        "new": """                        if action != "APPLY":
                            self._db.execute(
                                "DELETE FROM admission_sanctions WHERE admission_id=?",
                                (record["admission_id"],),
                            )
                            if action != "WAIT":
""",
        "tests": [LOST_AFTER, LOST_BEFORE],
        "why": "waiving the absence takes the sanction of the contest with it.",
    },
    {
        "id": "imp4-an-old-loss-covers-this-round",
        "invariants": ["IMP-4", "D86"],
        "path": STORE,
        "old": """            and (turn.resolved_beacon or 0) >= body.d_open
""",
        "new": """            and (turn.resolved_beacon or 0) >= 0
""",
        "tests": [OLD_LOSS],
        "why": "a contest lost before the round opened excuses an absence it did not cause.",
    },
    {
        "id": "imp4-a-win-over-the-auditor-covers-the-absence",
        "invariants": ["IMP-4", "D86"],
        "path": STORE,
        "old": """            and turn.resolution.loser == turn.contest.miner
""",
        "new": """            and turn.resolution.loser == turn.contest.auditor
""",
        "tests": [LOST_AFTER],
        "why": "the member's loss is read as the auditor's: the absence is sanctioned twice.",
    },
    {
        "id": "imp4-a-dropped-hold-stays-the-miners-blame",
        "invariants": ["IMP-4", "D86"],
        "path": STORE,
        "old": (
            "                                    "
            '| ({"blame": "INFRASTRUCTURE"} if action == "DROP" else {}),\n'
        ),
        "new": "                                    | {},\n",
        "tests": [BOUND],
        "why": "an exclusion the sanction gave up on is still booked as the miner's fault.",
    },
    {
        "id": "imp4-a-lost-contest-does-not-cover",
        "invariants": ["IMP-4", "D86"],
        "path": EXIT,
        "old": """    return "COVERED" if contest_lost else "APPLY"
""",
        "new": """    return "APPLY"
""",
        "tests": [ACTIONS, LOST_AFTER],
        "why": "the absence of a member that lost its contest is sanctioned a second time.",
    },
    {
        "id": "imp4-an-open-contest-holds-for-ever",
        "invariants": ["IMP-4", "D86"],
        "path": EXIT,
        "old": """        return "DROP" if bound_reached else "WAIT"
""",
        "new": """        return "WAIT"
""",
        "tests": [ACTIONS, BOUND],
        "why": "the wait for a contest has no bound.",
    },
]
