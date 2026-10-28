"""Mutants of the http gap tests: one small regression each, which a named test must catch."""

from __future__ import annotations

G = "tests/ledger_invariants/test_gaps_http.py::"
APP = "src/hypertrain/challenge/app.py"
RELAY_APP = "src/hypertrain/relay/app.py"
RELAY_CORE = "src/hypertrain/relay/core.py"
STORE = "src/hypertrain/challenge/store.py"
DISPUTES = "src/hypertrain/challenge/disputes_v2.py"
ADMISSION = "src/hypertrain/challenge/admission.py"
RELAY_CLIENT = "src/hypertrain/relay/client.py"
PUBLIC = "src/hypertrain/public_api/__init__.py"

DRAIN = G + "test_the_relay_drain_route_refuses_wrong_credentials_and_drains_on_the_exact_one"
DRAIN_FLOOR = G + "test_a_drain_credential_under_32_characters_never_builds_the_relay_app"
SCRUB_COVERS = G + "test_the_scrub_list_covers_every_name_config_from_env_reads"
SCRUB_NAMES = (
    G + "test_every_name_of_the_scrub_list_is_a_setting_of_config_and_the_scrub_restores_it"
)
HERMETIC = G + "test_a_service_world_takes_nothing_from_a_polluted_ambient_environment"
NETWORK_FROM_MASTER = (
    G + "test_the_master_derives_the_relay_network_manifest_from_the_training_hash"
)
RELAY_PINNED = G + "test_a_relay_pinned_on_another_base_hash_refuses_the_master_grants"
REMAINING = G + "test_an_envelope_made_for_another_run_is_refused_on_the_remaining_routes"
REFUSED_FINALIZE = (
    G + "test_a_refused_finalize_mints_nothing_and_every_epoch_answers_the_defined_burn"
)
PURE_READS = G + "test_every_pure_read_get_writes_nothing_durable_on_its_success_path_too"
ACK = G + "test_an_event_ack_made_for_another_run_is_refused_and_stores_no_ack"
POLL_ORDER = G + "test_a_dispute_poll_never_waits_for_the_other_writers_of_a_dispute"
RECEIPT = G + "test_a_relay_receipt_of_another_run_is_not_applied_and_a_relabeled_one_fails"


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


MUTANTS = [
    mutant(
        "gh-drain-compares-a-prefix-of-the-credential",
        ["R-17", "R-20"],
        RELAY_APP,
        "        if not hmac.compare_digest(token, drain_token):\n",
        "        if not hmac.compare_digest(token[:16], drain_token[:16]):\n",
        [DRAIN],
        "A credential that agrees with the drain secret on its first bytes drains the relay.",
    ),
    mutant(
        "gh-drain-opens-without-a-credential",
        ["R-17", "R-20"],
        RELAY_APP,
        "        if not hmac.compare_digest(token, drain_token):\n",
        "        if not hmac.compare_digest(token, drain_token) and False:\n",
        [DRAIN],
        "Any caller of the internal route drains the relay, whatever it presents.",
    ),
    mutant(
        "gh-drain-answers-without-draining",
        ["R-17", "R-20"],
        RELAY_APP,
        "        relay.draining = True\n        await relay.recover()\n",
        "        await relay.recover()\n",
        [DRAIN],
        "The route answers draining and the relay keeps taking new grants.",
    ),
    mutant(
        "gh-drain-credential-floor-lowered",
        ["R-17", "R-20"],
        RELAY_APP,
        "    if len(drain_token) < 32:\n",
        "    if len(drain_token) < 16:\n",
        [DRAIN_FLOOR],
        "A 16 to 31 character drain credential builds the application.",
    ),
    mutant(
        "gh-config-reads-a-sixteenth-name",
        ["M-17"],
        APP,
        '            netuid=int(env("CHALLENGE_NETUID", "100")),\n',
        '            netuid=int(env("CHALLENGE_NETUID", env("HYPERTRAIN_NETUID", "100"))),\n',
        [SCRUB_COVERS],
        "A new knob reaches the service through the environment and the scrub list does not "
        "name it.",
    ),
    mutant(
        "gh-config-renames-a-setting",
        ["M-17"],
        APP,
        '                epoch_seconds=int(env("HYPERTRAIN_EPOCH_SECONDS", "4320")),\n',
        '                epoch_seconds=int(env("HYPERTRAIN_EPOCH_SECS", "4320")),\n',
        [SCRUB_COVERS, SCRUB_NAMES],
        "The service reads another name than the scrub list holds, so a polluted ambient value "
        "of the old name is still scrubbed and a polluted value of the new one is not.",
    ),
    mutant(
        "gh-create-app-takes-its-state-directory-from-the-environment",
        ["M-17"],
        APP,
        '    server_keys = ServerKeys(config.state_dir / "server_keys.db")\n',
        "    server_keys = ServerKeys(\n"
        '        Path(os.environ.get("CHALLENGE_STATE_DIR", config.state_dir)) / "server_keys.db"\n'
        "    )\n",
        [HERMETIC],
        "The application resolves a state directory from the ambient environment before its own "
        "configuration, so a stray variable moves the server-key registry out of the test's world.",
    ),
    mutant(
        "gh-master-network-manifest-base-hash-is-the-run-id",
        ["M-10", "RTA-08"],
        STORE,
        "                base_manifest_hash=manifest.training.training_hash(),\n",
        "                base_manifest_hash=run_id,\n",
        [NETWORK_FROM_MASTER],
        "The master pins the relay network on the run id and not on the training hash "
        "docs/prod-run-signing.md says it must bind.",
    ),
    mutant(
        "gh-master-assignment-names-another-manifest",
        ["M-10", "RTA-08"],
        STORE,
        "                manifest_hash=network.digest(),\n",
        "                manifest_hash=registry.digest(),\n",
        [NETWORK_FROM_MASTER],
        "The assignment the relay checks names the registry, not the network manifest it is "
        "served with.",
    ),
    mutant(
        "gh-relay-accepts-a-grant-of-another-network-manifest",
        ["M-10", "RTA-08"],
        RELAY_CORE,
        "            or assignment.manifest_hash != self.network_manifest_hash\n",
        "",
        [RELAY_PINNED],
        "A relay pinned on a network manifest that differs from the master's in its base hash "
        "accepts the master's grants.",
    ),
]


def route_reads_the_run_of_its_envelope(
    route: str, path: str, old: str, new: str, tests: list[str], what: str
) -> dict[str, object]:
    """The route builds its intake on the run the envelope names, not on the run of its path."""
    return mutant(
        f"gh-{route}-trusts-the-run-its-envelope-names",
        ["T-SG-01", "SG-01"],
        path,
        old,
        new,
        tests,
        f"{what} accepts an envelope sealed for another run.",
    )


MUTANTS += [
    route_reads_the_run_of_its_envelope(
        "resolution",
        DISPUTES,
        '                self.run_id, {"ResolutionV2": lambda h: h == turn.contest.referee}\n',
        '                env.run_id, {"ResolutionV2": lambda h: h == turn.contest.referee}\n',
        [REMAINING],
        "The referee resolution",
    ),
    route_reads_the_run_of_its_envelope(
        "observer-response",
        STORE,
        '                model = self._authenticated_v2(run_id, raw, "Receipt", observer)\n',
        "                model = self._authenticated_v2(\n"
        '                    envelope_v2.parse_envelope(raw).run_id, raw, "Receipt", observer\n'
        "                )\n",
        [REMAINING],
        "The relay observer response",
    ),
    route_reads_the_run_of_its_envelope(
        "upload-grant",
        STORE,
        '                run_id, {"UploadChunkManifest": env.signer.__eq__}\n',
        '                env.run_id, {"UploadChunkManifest": env.signer.__eq__}\n',
        [REMAINING],
        "The upload grant request",
    ),
    route_reads_the_run_of_its_envelope(
        "trial-finalize",
        ADMISSION,
        "            final = Intake("
        'self.escrow.run_id, {"Finalize": lambda s: s == self.coord.ss58}'
        ")"
        ".accept(\n",
        "            final = Intake(\n"
        '                env.run_id, {"Finalize": lambda s: s == self.coord.ss58}\n'
        "            ).accept(\n",
        [REMAINING],
        "The trial finalize",
    ),
    route_reads_the_run_of_its_envelope(
        "rollback",
        STORE,
        '            model = self._authenticated_v2(run_id, raw, "Rollback", self._coord().ss58)\n',
        "            model = self._authenticated_v2(\n"
        "                envelope_v2.parse_envelope(raw).run_id,\n"
        '                raw,\n                "Rollback",\n                self._coord().ss58,\n'
        "            )\n",
        [REMAINING],
        "The rollback request",
    ),
    route_reads_the_run_of_its_envelope(
        "relay-registry",
        STORE,
        '                run_id, {"RelayRegistryV1": self._coord().ss58.__eq__}\n',
        "                relay_envelope.parse_envelope(raw).run_id,"
        ' {"RelayRegistryV1": self._coord().ss58.__eq__}\n',
        [REMAINING],
        "The relay registry rotation",
    ),
    route_reads_the_run_of_its_envelope(
        "bootstrap",
        STORE,
        "            model = self._authenticated_v2("
        'run_id, raw, "ProductionBootstrap", self._coord().ss58)\n',
        "            model = self._authenticated_v2(\n"
        "                envelope_v2.parse_envelope(raw).run_id,\n"
        '                raw,\n                "ProductionBootstrap",\n'
        "                self._coord().ss58,\n            )\n",
        [REMAINING],
        "The production bootstrap",
    ),
    mutant(
        "gh-relay-failure-ignores-the-run-of-its-evidence",
        ["T-SG-01", "SG-01"],
        STORE,
        "                or env.run_id != run_id\n                or evidence.run_id != run_id\n",
        "",
        [REMAINING],
        "A failure evidence sealed for another run is read as this run's.",
    ),
    mutant(
        "gh-event-ack-ignores-its-run",
        ["T-SG-01", "SG-01"],
        DISPUTES,
        "        if not authentic(ack) or ack.run_id != self.run_id"
        " or ack.received_beacon > now:\n",
        "        if not authentic(ack) or ack.received_beacon > now:\n",
        [ACK],
        "An acknowledgement made for another run is stored for this one.",
    ),
    mutant(
        "gh-relay-receipt-signature-unchecked",
        ["T-SG-01", "SG-01"],
        RELAY_CLIENT,
        '        or raw.type != "RelayReceipt"\n'
        "        or not relay_envelope.verify_envelope(raw.model_dump())\n",
        '        or raw.type != "RelayReceipt"\n',
        [RECEIPT],
        "A receipt relabeled to this run keeps its force without its signature being checked.",
    ),
]


MUTANTS += [
    mutant(
        "gh-auditor-stats-logs-its-reads",
        ["T-GT-07", "GT-06"],
        STORE,
        "            _, manifest = self._run(db, run_id)\n"
        "            honeypots: dict[str, str] = {}\n",
        "            _, manifest = self._run(db, run_id)\n"
        "            db.execute(\n"
        "                \"INSERT OR REPLACE INTO meta VALUES('stats-read', ?)\", (run_id,)\n"
        "            )\n"
        "            honeypots: dict[str, str] = {}\n",
        [PURE_READS],
        "The auditor-stats read of a run that exists leaves a row behind, which a refusal path "
        "(unknown run) never shows.",
    ),
    mutant(
        "gh-public-mount-logs-its-reads",
        ["T-GT-07", "GT-06"],
        PUBLIC,
        '                    db = sqlite3.connect(f"file:{p}?mode=ro", uri=True)\n'
        "                    db.row_factory = sqlite3.Row\n"
        "                    dbs.append(db)\n",
        '                    db = sqlite3.connect(f"file:{p}?mode=rw", uri=True)\n'
        "                    db.row_factory = sqlite3.Row\n"
        "                    dbs.append(db)\n"
        "                    db.execute(\n"
        "                        \"INSERT OR REPLACE INTO meta VALUES('public-read', ?)\","
        " (path,)\n"
        "                    )\n"
        "                    db.commit()\n",
        [PURE_READS],
        "The public mount opens the database writable and logs each read it serves.",
    ),
]


MUTANTS += [
    mutant(
        "gh-a-repair-is-finalized-for-a-reward",
        ["M-02", "FIN-18"],
        STORE,
        '            if applied.get("kind") == "repair":\n'
        "                raise ChallengeError(\n"
        '                    409, "repair evidence is model-only; cannot mint miner rewards"\n'
        "                )\n",
        "",
        [REFUSED_FINALIZE],
        "The Finalize of a round that was only repaired goes on towards the reward credit.",
    ),
    mutant(
        "gh-an-empty-epoch-burns-nothing",
        ["M-02", "WGT-10"],
        "src/hypertrain/ledger/ledger.py",
        '                "units_burned_this_epoch": FULL_SHARE_MASS - paid_now,\n',
        '                "units_burned_this_epoch": 0,\n',
        [REFUSED_FINALIZE],
        "An epoch that paid nobody reports no burned share, so the unclaimed mass is not burned.",
    ),
]


MUTANTS += [
    route_reads_the_run_of_its_envelope(
        "shadow-reservation-finalize",
        STORE,
        "            final = envelope_v2.Intake("
        'run_id, {"Finalize": self._coord().ss58.__eq__}'
        ").accept(\n"
        '                request.finalize.model_dump(mode="json"), now\n',
        "            final = envelope_v2.Intake(\n"
        '                request.finalize.run_id, {"Finalize": self._coord().ss58.__eq__}\n'
        "            ).accept(\n"
        '                request.finalize.model_dump(mode="json"), now\n',
        [REMAINING],
        "The finalize inside a shadow reservation",
    ),
    route_reads_the_run_of_its_envelope(
        "shadow-reservation-commit",
        STORE,
        "            commit = envelope_v2.Intake("
        'run_id, {"CommitV2": row["hotkey"].__eq__}'
        ").accept(\n"
        '                request.commit.model_dump(mode="json"), now\n',
        "            commit = envelope_v2.Intake(\n"
        '                request.commit.run_id, {"CommitV2": row["hotkey"].__eq__}\n'
        "            ).accept(\n"
        '                request.commit.model_dump(mode="json"), now\n',
        [REMAINING],
        "The commit inside a shadow reservation",
    ),
]


EXPIRY = "tests/ledger_invariants/test_expiry.py::"
LOCKS = "tests/ledger_invariants/test_locks.py::"

MUTANTS += [
    mutant(
        "gh-finalize-has-no-early-return-for-a-replay",
        ["M-01", "W-01"],
        STORE,
        "            if previous_final is not None:\n"
        '                if json.loads(previous_final[0])["body"]'
        ' != model.model_dump(mode="json"):\n',
        "            if False:\n"
        '                if json.loads(previous_final[0])["body"]'
        ' != model.model_dump(mode="json"):\n',
        [
            EXPIRY
            + "test_an_identical_finalize_replay_returns_the_stored_result_and_changes_nothing",
            EXPIRY + "test_a_finalize_with_other_content_is_refused_and_the_stored_result_stands",
            LOCKS + "test_a_finalized_round_answers_a_replay_with_its_stored_result"
            "_and_refuses_another_body",
        ],
        "A replay of a finalized round goes on to the ledger at a later beacon, where it carries "
        "another maturity and ends in an operation conflict or an expired envelope instead of the "
        "stored result.",
    ),
]


MUTANTS += [
    mutant(
        "gh-dispute-poll-reads-its-table-inside-the-condition",
        ["DS-04", "DSP-03"],
        DISPUTES,
        "            with self._leaf():\n"
        "                seen = self._beats  # read before the table:"
        " a wake-up in between is not lost\n"
        "            found = available()\n",
        "            with self._leaf():\n"
        "                seen = self._beats  # read before the table:"
        " a wake-up in between is not lost\n"
        "                found = available()\n",
        [POLL_ORDER],
        "The long poll asks for the store lock while it holds the dispute condition, as before "
        "ADR 0011 part 7, so a writer that notifies inside its transaction and the poller wait "
        "for each other.",
    ),
]
