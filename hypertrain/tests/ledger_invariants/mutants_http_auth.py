"""Mutants of package http_auth: small regressions of src/ that the tests of the package must catch.

Run from hypertrain/:
    .venv/bin/python scripts/check_ledger_invariant_mutants.py --only <id> ... --jobs 2
"""

from __future__ import annotations

from typing import Any

A = "tests/ledger_invariants/test_http_auth.py::"
B = "tests/ledger_invariants/test_http_auth_2.py::"
APP = "src/hypertrain/challenge/app.py"
STORE = "src/hypertrain/challenge/store.py"
KEYS = "src/hypertrain/challenge/server_keys.py"
RELAY_CORE = "src/hypertrain/relay/core.py"
RELAY_APP = "src/hypertrain/relay/app.py"

OP_MATRIX = A + "test_a_bearer_route_refuses_every_wrong_credential_and_changes_nothing[OP]"
INT_MATRIX = A + "test_a_bearer_route_refuses_every_wrong_credential_and_changes_nothing[INT]"
WK_MATRIX = A + "test_a_bearer_route_refuses_every_wrong_credential_and_changes_nothing[WK]"
OP_FOREIGN = A + "test_no_bearer_class_opens_a_route_of_another_class[OP]"
WK_FOREIGN = A + "test_no_bearer_class_opens_a_route_of_another_class[WK]"
INT_FOREIGN = A + "test_no_bearer_class_opens_a_route_of_another_class[INT]"
OP_UNCONFIGURED = A + "test_an_unconfigured_bearer_makes_its_routes_503_for_every_caller[OP]"
ROUTE_TABLE = A + "test_every_route_of_the_three_applications_has_a_recorded_principal"
RELAY_TABLE = A + "test_every_relay_route_has_a_recorded_principal"
UNREGISTERED = (
    A + "test_a_signer_off_the_subnet_is_refused_403_before_any_state_change_on_every_gated_route"
)
REVOKED = (
    A + "test_a_revoked_server_key_is_refused_403_on_every_gated_route_though_still_on_the_subnet"
)
FORGED = A + "test_a_forged_signer_is_refused_on_the_signed_routes_that_have_no_membership_gate"
PREFIX = A + "test_no_signing_domain_is_a_byte_prefix_of_another"
PREIMAGE = A + "test_every_signature_preimage_opens_with_exactly_its_own_domain"
# The owner requests of the trial routes (ADR 0010 decision 3) are pinned by the base tests of
# tests/challenge/test_trial_export.py: who may ask, the other route's request, the expiry and its
# bound. This package adds the revoked server key (AU-05) and the route table.
T = "tests/challenge/test_trial_export.py::"
CHALLENGE_OWNER_ONLY = T + "test_challenge_route_refuses_everyone_but_the_owner"
JOB_OWNER_ONLY = T + "test_job_route_refuses_everyone_but_the_owner"
EXPIRY_BOUND = T + "test_an_owner_request_may_expire_only_so_far_ahead"
SITES = A + "test_every_production_intake_names_a_signer_predicate_for_a_real_message_type"
PREDICATE = A + "test_an_intake_refuses_a_valid_signature_from_a_signer_its_predicate_rejects"
KEY_LIFECYCLE = (
    A + "test_a_server_key_grant_needs_the_coldkey_and_the_key_holder_and_a_revocation_is_permanent"
)
KEY_CENSOR = A + "test_nobody_but_the_key_holder_can_censor_a_hotkey_through_a_server_key_grant"
PURE_READS = A + "test_a_pure_read_get_writes_nothing_durable"
IDEMPOTENT_INFRA = A + "test_health_and_admission_status_write_only_idempotent_infrastructure_rows"
LATER_ROUND = A + "test_admission_status_in_a_later_round_writes_at_most_its_pin_row"
ROUND_READS = (
    B + "test_repeated_anonymous_reads_of_a_round_leave_the_stored_state_as_the_first_left_it"
)
CHUNK = B + "test_a_relay_chunk_needs_an_unexpired_master_grant_and_the_exact_length_and_hash"


def mutant(
    id: str, invariants: list[str], path: str, old: str, new: str, tests: list[str], why: str
) -> dict[str, Any]:
    return {
        "id": id,
        "invariants": invariants,
        "path": path,
        "old": old,
        "new": new,
        "tests": tests,
        "why": why,
    }


MUTANTS: list[dict[str, Any]] = [
    mutant(
        "http-auth-economic-bound-without-bearer",
        ["AU-01"],
        APP,
        "async def economic_bound_v2(run_id: str, authorization: Auth = None) -> Any:\n"
        "        admin(authorization)\n",
        "async def economic_bound_v2(run_id: str, authorization: Auth = None) -> Any:\n",
        [OP_MATRIX, ROUTE_TABLE],
        "an operator handler loses its admin() call, so anyone can seal the economic bound.",
    ),
    mutant(
        "http-auth-worker-object-without-bearer",
        ["AU-01"],
        APP,
        "async def get_object(sha256: str, authorization: Auth = None) -> Response:\n"
        "        worker(authorization)\n",
        "async def get_object(sha256: str, authorization: Auth = None) -> Response:\n",
        [WK_MATRIX],
        "the v1 object download drops its worker check and serves any caller.",
    ),
    mutant(
        "http-auth-unconfigured-bearer-is-open",
        ["AU-01"],
        APP,
        "    if expected is None:\n"
        '        raise ChallengeError(503, "this route is not configured")\n',
        "    if expected is None:\n        return\n",
        [OP_UNCONFIGURED],
        "a missing or empty token file lets every caller through instead of answering 503.",
    ),
    mutant(
        "http-auth-bearer-scheme-optional",
        ["AU-01"],
        APP,
        'if not authorization or not authorization.startswith("Bearer "):',
        "if not authorization:",
        [OP_MATRIX],
        "the bare token without the Bearer scheme is accepted as the credential.",
    ),
    mutant(
        "http-auth-bearer-compared-as-prefix",
        ["AU-01"],
        APP,
        "    if not hmac.compare_digest(\n"
        "        hashlib.sha256(presented.encode()).digest(),\n"
        "        hashlib.sha256(expected.encode()).digest(),\n"
        "    ):\n",
        "    if not expected.startswith(presented):\n",
        [OP_MATRIX],
        "a prefix of the token opens the route because the comparison is no longer exact.",
    ),
    mutant(
        "http-auth-admin-gate-reads-worker-token",
        ["AU-03"],
        APP,
        "        _require(config.admin_token_file, authorization)",
        "        _require(config.worker_token_file, authorization)",
        [OP_FOREIGN],
        "the operator routes check the worker token file, so the worker bearer opens them.",
    ),
    mutant(
        "http-auth-worker-gate-reads-internal-token",
        ["AU-03"],
        APP,
        "        _require(config.worker_token_file, authorization)",
        "        _require(config.internal_token_file, authorization)",
        [WK_FOREIGN],
        "the worker routes check the internal token file, so the internal bearer opens them.",
    ),
    mutant(
        "http-auth-get-weights-accepts-admin-token",
        ["AU-03"],
        APP,
        "        _require(config.internal_token_file, authorization)\n"
        "        if x_platform_challenge_slug != config.slug:",
        "        _require(config.admin_token_file, authorization)\n"
        "        if x_platform_challenge_slug != config.slug:",
        [INT_FOREIGN],
        "get_weights checks the operator token, so the operator bearer reads the weights.",
    ),
    mutant(
        "http-auth-revoked-server-key-passes-subnet-gate",
        ["AU-05"],
        APP,
        "        if isinstance(hotkey, str):\n            await not_revoked(hotkey)\n",
        "",
        [REVOKED],
        "subnet_member no longer asks whether the server key was revoked.",
    ),
    mutant(
        "http-auth-v1-registered-skips-revocation",
        ["AU-05"],
        APP,
        "        await not_revoked(signer)\n        if signer not in await metagraph.hotkeys():",
        "        if signer not in await metagraph.hotkeys():",
        [REVOKED],
        "the v1 intake check no longer refuses a revoked server key.",
    ),
    mutant(
        "http-auth-hotkey-need-not-be-on-metagraph",
        ["AU-05"],
        APP,
        "        if not isinstance(hotkey, str) or hotkey not in await metagraph.hotkeys():",
        "        if not isinstance(hotkey, str):",
        [UNREGISTERED],
        "any validly signed hotkey passes the subnet gate, registered or not.",
    ),
    mutant(
        "http-auth-leaves-route-without-subnet-gate",
        ["AU-05"],
        APP,
        "        raw = await v2_raw(request)\n"
        "        await subnet_member(signer(raw))\n"
        "        return await run(store.for_run(run_id).leaves_v2, run_id, raw)",
        "        raw = await v2_raw(request)\n"
        "        return await run(store.for_run(run_id).leaves_v2, run_id, raw)",
        [UNREGISTERED],
        "the leaves publication route forgets to gate its signer on the subnet.",
    ),
    mutant(
        "http-auth-v1-uploads-without-subnet-gate",
        ["AU-05"],
        APP,
        "        item = await read_model(request, UploadRequest)\n"
        '        await registered({"signer": item.hotkey})\n',
        "        item = await read_model(request, UploadRequest)\n",
        [UNREGISTERED],
        "the v1 upload-url route stops checking the hotkey it presigns for.",
    ),
    mutant(
        "http-auth-dispute-party-gate-open",
        ["AU-05"],
        APP,
        "        await subnet_member(party)\n        return False",
        "        return False",
        [UNREGISTERED],
        "a party that is neither a pinned role nor a subnet miner is admitted to dispute routes.",
    ),
    mutant(
        "http-auth-lease-asked-by-any-signer",
        ["AU-05"],
        STORE,
        "            if env.signer not in manifest.training.auditors:\n"
        '                raise ChallengeError(403, "auditor not allowlisted")',
        "            if False:\n"
        '                raise ChallengeError(403, "auditor not allowlisted")',
        [FORGED],
        "the audit lease route answers any signer, not only the pinned auditors.",
    ),
    mutant(
        "http-auth-escrow-owner-not-checked",
        ["AU-05"],
        "src/hypertrain/ledger/escrow_v2.py",
        '        if signer != op.owner:\n            raise EscrowError("OWNER_AUTHORITY")',
        '        if False:\n            raise EscrowError("OWNER_AUTHORITY")',
        [FORGED],
        "a transfer or release signed by someone other than the owner is applied.",
    ),
    mutant(
        "http-auth-relay-receipt-signer-not-checked",
        ["AU-05"],
        "src/hypertrain/relay/client.py",
        "        and k.pubkey == raw.signer\n",
        "",
        [FORGED],
        "a durability receipt signed by any key passes as the relay's own.",
    ),
    mutant(
        "http-auth-observer-need-not-be-pinned",
        ["AU-05"],
        STORE,
        "            if observer not in manifest.training.auditors or observer in {",
        "            if observer in {",
        [FORGED],
        "any signer may answer a relay-observer challenge, pinned auditor or not.",
    ),
    mutant(
        "http-auth-join-domain-equals-envelope-domain",
        ["SG-02"],
        "src/hypertrain/protocol/envelope_v2.py",
        'b"hypertrain/join/2|", "JoinRequest"',
        'b"hypertrain/2|", "JoinRequest"',
        [PREIMAGE, PREFIX],
        "a join signature becomes a valid envelope signature over the same bytes.",
    ),
    mutant(
        "http-auth-relay-domain-equals-envelope-domain",
        ["SG-02"],
        "src/hypertrain/protocol/relay_envelope.py",
        'DOMAIN = b"hypertrain/relay/1|"',
        'DOMAIN = b"hypertrain/2|"',
        [PREIMAGE],
        "relay envelopes and training envelopes sign the same preimage.",
    ),
    mutant(
        "http-auth-server-key-domain-loses-separator",
        ["SG-02"],
        "src/hypertrain/protocol/server_keys.py",
        'DOMAIN = b"hypertrain/server-key/1|"',
        'DOMAIN = b"hypertrain/server-key/1"',
        [PREFIX],
        "the server-key domain no longer ends in its separator, so it can prefix other messages.",
    ),
    mutant(
        "http-auth-finalize-intake-accepts-any-signer",
        ["SG-03"],
        "src/hypertrain/ledger/escrow_v2.py",
        'Intake(self.run_id, {"Finalize": self.policy.reward_authority.__eq__}).accept(',
        "Intake(self.run_id).accept(",
        [SITES],
        "the reward Finalize is read by an Intake with no signer predicate, which is fail-open.",
    ),
    mutant(
        "http-auth-audit-challenge-predicate-misspelled",
        ["SG-03"],
        STORE,
        'envelope_v2.Intake(run_id, {"AuditChallengeV2": self._coord().ss58.__eq__}).accept(',
        'envelope_v2.Intake(run_id, {"AuditChallenge": self._coord().ss58.__eq__}).accept(',
        [SITES],
        "the predicate is keyed by a type that does not exist, so it never fires.",
    ),
    mutant(
        "http-auth-intake-predicate-result-ignored",
        ["SG-03"],
        "src/hypertrain/protocol/envelope_v2.py",
        "        if check is not None and not check(env.signer):",
        "        if check is not None and not check:",
        [PREDICATE],
        "the signer predicate is looked up but its answer is never used.",
    ),
    mutant(
        "http-auth-server-key-holder-not-asked-to-sign",
        ["SG-15"],
        KEYS,
        "        if not check_sig(g.server_pub, message, signed.server_sig):",
        "        if False:",
        [KEY_CENSOR, KEY_LIFECYCLE],
        "a coldkey can register somebody else's hotkey as its server key and later revoke it.",
    ),
    mutant(
        "http-auth-server-key-coldkey-not-asked-to-sign",
        ["SG-15"],
        KEYS,
        "        if not check_sig(g.coldkey, message, signed.cold_sig):",
        "        if False:",
        [KEY_LIFECYCLE],
        "a grant is accepted without the coldkey's signature.",
    ),
    mutant(
        "http-auth-any-coldkey-revokes-a-server-key",
        ["SG-15"],
        KEYS,
        '            if record["coldkey"] != r.coldkey:',
        "            if False:",
        [KEY_LIFECYCLE],
        "a coldkey that did not grant the server key can revoke it.",
    ),
    mutant(
        "http-auth-revocation-restamped-on-repeat",
        ["SG-15"],
        KEYS,
        '            if not record["revoked"]:',
        "            if True:",
        [KEY_LIFECYCLE],
        "a second revocation rewrites the first revocation record.",
    ),
    mutant(
        "http-auth-revoked-server-key-can-be-granted-again",
        ["SG-15"],
        KEYS,
        "            if existing:",
        "            if False:",
        [KEY_LIFECYCLE],
        "a revoked server key is granted again under a new key id.",
    ),
    mutant(
        "http-auth-revocation-not-reported-to-the-gate",
        ["SG-15", "AU-05"],
        KEYS,
        '"SELECT 1 FROM server_keys WHERE server_pub=? AND revoked_at IS NOT NULL",',
        '"SELECT 1 FROM server_keys WHERE server_pub=? AND revoked_at IS NOT NULL AND 0",',
        [REVOKED],
        "is_revoked never answers true, so a revoked key keeps its access.",
    ),
    mutant(
        "http-auth-run-listing-writes-a-marker",
        ["GT-06"],
        STORE,
        '            rows = self._db.execute("SELECT run_id, status FROM runs").fetchall()',
        "            self._db.execute(\"INSERT OR REPLACE INTO meta VALUES('runs-read', '1')\")\n"
        '            rows = self._db.execute("SELECT run_id, status FROM runs").fetchall()',
        [PURE_READS],
        "a pure read of the run list leaves a row behind.",
    ),
    mutant(
        "http-auth-v2-run-status-pins-a-beacon",
        ["GT-06"],
        STORE,
        "        with self._lock:\n"
        "            self._run_v2(run_id)\n"
        '            row = self._db.execute("SELECT * FROM runs '
        'WHERE run_id=?", (run_id,)).fetchone()\n',
        "        with self._lock:\n"
        '            self._fresh_v2(self._run_v2(run_id), "status-read")\n'
        '            row = self._db.execute("SELECT * FROM runs '
        'WHERE run_id=?", (run_id,)).fetchone()\n',
        [PURE_READS],
        "the public run status starts pinning a beacon round on every read.",
    ),
    mutant(
        "http-auth-health-stamps-every-call",
        ["GT-07"],
        STORE,
        "db.execute(\"INSERT OR REPLACE INTO meta VALUES('health', '1')\")",
        "db.execute(\"INSERT OR REPLACE INTO meta VALUES('health', hex(randomblob(8)))\")",
        [IDEMPOTENT_INFRA],
        "the health probe writes a different marker each time, so it is no longer idempotent.",
    ),
    mutant(
        "http-auth-admission-status-opens-a-trial",
        ["GT-07"],
        STORE,
        "                        result = asdict(admission.status(identity, now=now))",
        "                        admission.challenge(identity, now=now)\n"
        "                        result = asdict(admission.status(identity, now=now))",
        [LATER_ROUND, IDEMPOTENT_INFRA],
        "the anonymous admission status read opens a trial for the admission it reports.",
    ),
    mutant(
        "http-auth-relay-assignment-resealed-on-every-read",
        ["GT-05"],
        STORE,
        '                (run_id, f"{w}:{hotkey}"),\n'
        "            ).fetchone()\n"
        "            if prior is not None:",
        '                (run_id, f"{w}:{hotkey}"),\n'
        "            ).fetchone()\n"
        "            if False:",
        [ROUND_READS],
        "each read of a relay assignment signs and stores a new envelope.",
    ),
    mutant(
        "http-auth-chunk-capability-never-expires",
        ["LV-08"],
        RELAY_CORE,
        "            or raw.exp_drand < now\n            or raw.type != cls.__name__",
        "            or raw.type != cls.__name__",
        [CHUNK],
        "an expired master-signed grant keeps authorizing chunk uploads.",
    ),
    mutant(
        "http-auth-chunk-capability-signer-ignored",
        ["LV-08"],
        RELAY_CORE,
        "            raw.run_id != self.run_id\n            or raw.signer != self.master\n",
        "            raw.run_id != self.run_id\n",
        [CHUNK],
        "a grant signed by anyone but the master authorizes chunk uploads.",
    ),
    mutant(
        "http-auth-chunk-capability-not-bound-to-path",
        ["LV-08"],
        RELAY_APP,
        "        if grant.digest() != grant_hash:\n"
        '            raise RelayError("upload capability path mismatch")',
        '        if False:\n            raise RelayError("upload capability path mismatch")',
        [CHUNK],
        "a master grant for one upload authorizes chunks of another.",
    ),
    mutant(
        "http-auth-chunk-accepted-after-the-upload-deadline",
        ["LV-08"],
        RELAY_CORE,
        "            if now > ack.service_deadline or "
        "not 0 <= index < len(record.manifest.chunks):",
        "            if not 0 <= index < len(record.manifest.chunks):",
        [CHUNK],
        "a chunk arriving after the granted upload deadline is stored and acknowledged.",
    ),
    mutant(
        "http-auth-new-route-without-classification",
        ["AU-01"],
        APP,
        '    @app.get("/version")\n    async def version() -> dict[str, Any]:',
        '    @app.get("/v1/debug/state")\n'
        "    async def debug_state() -> dict[str, Any]:\n"
        '        return {"runs": await run(store.runs)}\n\n'
        '    @app.get("/version")\n    async def version() -> dict[str, Any]:',
        [ROUTE_TABLE],
        "a new route is added without anybody deciding which credential it needs.",
    ),
    mutant(
        "http-auth-new-relay-route-without-classification",
        ["AU-01"],
        RELAY_APP,
        '    @app.get("/livez")\n    async def live() -> JSONResponse:',
        '    @app.get("/internal/journal")\n'
        "    async def journal() -> JSONResponse:\n"
        '        return JSONResponse((await relay.state()).model_dump(mode="json"))\n\n'
        '    @app.get("/livez")\n    async def live() -> JSONResponse:',
        [RELAY_TABLE],
        "a new relay route exposes the journal without anybody deciding who may read it.",
    ),
    mutant(
        "http-auth-worker-fail-checks-the-operator-bearer",
        ["AU-03", "AU-01"],
        APP,
        "async def fail(job_id: str, request: Request, authorization: Auth = None) -> Any:\n"
        "        worker(authorization)\n",
        "async def fail(job_id: str, request: Request, authorization: Auth = None) -> Any:\n"
        "        admin(authorization)\n",
        [WK_MATRIX, ROUTE_TABLE],
        "a worker route is gated by the operator bearer, so workers lose it and operators gain it.",
    ),
    mutant(
        "http-auth-unset-token-path-reads-as-configured",
        ["AU-01"],
        APP,
        'no secret files at all."""\n    if path is None:\n        return None\n',
        'no secret files at all."""\n    if path is None:\n        return "unset"\n',
        [A + "test_an_unconfigured_bearer_makes_its_routes_503_for_every_caller[WK]"],
        "a route whose token path is not configured at all accepts the placeholder as its bearer.",
    ),
    mutant(
        "http-auth-trial-challenge-without-owner-gate",
        ["AU-05"],
        APP,
        "        await owner_request(run_id, admission_id, request, ChallengeRequest)\n",
        "",
        [REVOKED, CHALLENGE_OWNER_ONLY],
        "the trial challenge stops asking for the owner's signed request and subnet membership.",
    ),
    mutant(
        "http-auth-trial-job-without-owner-gate",
        ["AU-05"],
        APP,
        "        await owner_request(run_id, admission_id, request, JobRequest)\n",
        "",
        [REVOKED, JOB_OWNER_ONLY],
        "the trial job export stops asking for the owner's signed request and subnet membership.",
    ),
    mutant(
        "http-auth-trial-owner-helper-skips-the-subnet-gate",
        ["AU-05"],
        APP,
        "        hotkey = (await run(admission.store.by_id, admission_id)).hotkey\n"
        "        await subnet_member(hotkey)\n",
        "        hotkey = (await run(admission.store.by_id, admission_id)).hotkey\n",
        [REVOKED],
        "the owner helper of the trial challenge and job routes forgets the subnet and "
        "revocation gate on the owner's hotkey.",
    ),
    mutant(
        "http-auth-trial-owner-request-of-another-message-type-is-accepted",
        ["SG-02", "AU-05"],
        APP,
        "            valid = isinstance(asked, message) and asked.admission_id == admission_id\n",
        "            valid = asked.admission_id == admission_id\n",
        [CHALLENGE_OWNER_ONLY, JOB_OWNER_ONLY],
        "a request of the other owner route passes the check of this one, though the signer "
        "predicate only watches the message type this route names.",
    ),
    mutant(
        "http-auth-trial-owner-request-expiry-is-unbounded",
        ["SG-05"],
        APP,
        "            if envelope_v2.parse_envelope(raw).exp_drand"
        " > now + OWNER_REQUEST_MAX_ROUNDS:\n"
        '                raise ExpiredError("expiry too far ahead")\n',
        "",
        [EXPIRY_BOUND],
        "an owner request may carry any expiry, so a capture of it replays for as long as it says.",
    ),
    mutant(
        "http-auth-rotation-consent-domain-equals-envelope-domain",
        ["SG-02"],
        "src/hypertrain/protocol/envelope_v2.py",
        'b"hypertrain/rotate/2|", "RotateRequest"',
        'b"hypertrain/2|", "RotateRequest"',
        [PREIMAGE, PREFIX],
        "the new hotkey's rotation consent becomes a valid envelope signature over the same bytes.",
    ),
    mutant(
        "http-auth-get-weights-without-bearer",
        ["AU-01"],
        APP,
        "        _require(config.internal_token_file, authorization)\n"
        "        if x_platform_challenge_slug != config.slug:",
        "        if x_platform_challenge_slug != config.slug:",
        [INT_MATRIX],
        "get_weights no longer asks for the internal bearer, so the slug header is all it checks.",
    ),
    mutant(
        "http-auth-empty-token-file-is-a-token",
        ["AU-01"],
        APP,
        "    return token or None",
        "    return token",
        [OP_UNCONFIGURED],
        "an empty or blank token file is an empty secret, so an empty bearer opens the routes.",
    ),
    mutant(
        "http-auth-bearer-compared-by-length",
        ["AU-01"],
        APP,
        "    if not hmac.compare_digest(\n"
        "        hashlib.sha256(presented.encode()).digest(),\n"
        "        hashlib.sha256(expected.encode()).digest(),\n"
        "    ):\n",
        "    if len(presented) != len(expected):\n",
        [OP_MATRIX],
        "only the length of the bearer is compared, so any token of that length opens a route.",
    ),
    mutant(
        "http-auth-v1-registered-metagraph-dropped",
        ["AU-05"],
        APP,
        "        if signer not in await metagraph.hotkeys():\n"
        '            raise ChallengeError(403, "the hotkey is not registered on the subnet")\n'
        '\n    @app.get("/health")',
        '\n    @app.get("/health")',
        [UNREGISTERED],
        "the v1 intake check keeps the revocation test but no longer asks the metagraph.",
    ),
    mutant(
        "http-auth-dispute-watch-without-party-gate",
        ["AU-05"],
        APP,
        "        pinned = await party_member(run_id, poll.party)\n",
        "        pinned = poll.party in await run(protocol_roles, run_id)\n",
        [UNREGISTERED],
        "the dispute watch forgets its membership gate; its signed body is the only check.",
    ),
    mutant(
        "http-auth-admission-lock-accepts-any-signer",
        ["AU-05"],
        "src/hypertrain/challenge/admission.py",
        'Intake(self.escrow.run_id, {"EscrowLock": lambda s: s == r.coldkey}).accept(raw, now)',
        'Intake(self.escrow.run_id, {"EscrowLock": lambda s: True}).accept(raw, now)',
        [FORGED],
        "an admission stake lock is accepted from a signer other than the admission's coldkey.",
    ),
    mutant(
        "http-auth-audit-execution-ignores-lease-owner",
        ["AU-05"],
        STORE,
        '            if row is None or row["state"] != "running" or row["auditor"] != env.signer:',
        '            if row is None or row["state"] != "running":',
        [FORGED],
        "an audit execution of a leased job is served to a signer that does not hold the lease.",
    ),
    mutant(
        "http-auth-server-key-may-equal-the-coldkey",
        ["SG-15"],
        KEYS,
        "        if g.server_pub == g.coldkey:\n"
        '            raise ChallengeError(422, "the server key must differ from the coldkey")\n',
        "",
        [KEY_LIFECYCLE],
        "a coldkey registers itself as its own server key.",
    ),
    mutant(
        "http-auth-relay-acks-without-capability",
        ["AU-01"],
        RELAY_APP,
        "    async def acks(grant_hash: str, request: Request, cursor: int = 0) -> JSONResponse:\n"
        "        await upload_capability(request, grant_hash)\n",
        "    async def acks(grant_hash: str, request: Request, cursor: int = 0) -> JSONResponse:\n",
        [RELAY_TABLE],
        "the relay ack listing no longer asks for the master-signed upload capability.",
    ),
]
