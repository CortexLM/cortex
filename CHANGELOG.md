# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Upgrade (BREAKING for validators)

- `deploy/compose/role-validator.yml` is now verify-only: it runs `--verify-only`,
  mounts no wallet and never submits, while its healthcheck stays green. An
  existing validator that set weights with it stops setting weights after the
  upgrade. Deploy `deploy/compose/role-weight-setter.yml` instead, and never run
  both with the same wallet. `role-validator.yml` now ignores `BASE_WALLET_NAME`,
  `BASE_WALLET_HOTKEY`, `BASE_VALIDATOR_WALLETS_DIR` and `consensus.key` in
  `BASE_VALIDATOR_IDENTITY_DIR`; `BASE_GATEWAY_HOTKEY` is removed everywhere.
- The weight-setter uses a new state volume, `base-weight-setter-state`: its
  submission journal and consensus watermarks start empty, so rollback
  protection restarts from the first profile it is served. To keep them, stop the
  old role and copy its volume first; see the
  [validator upgrade note](deploy/README.md#upgrading-from-the-single-validator-role).
- `role-weight-setter.yml` reads its identity directory from the new, required
  `BASE_WEIGHT_SETTER_IDENTITY_DIR` instead of `BASE_VALIDATOR_IDENTITY_DIR`,
  which is now the verify-only role's alone. Set it in
  `deploy/env/python-validator.env` to the directory holding `consensus.key`,
  `tls.crt` and `tls.key`, and point `BASE_VALIDATOR_IDENTITY_DIR` at a
  separate directory without `consensus.key`; `scripts/check_deploy.py` refuses
  one path, or one state volume, for both roles.

### Upgrade (BREAKING for masters)

- `POST /v1/weights/raw` now requires a separate intake bearer
  (`BASE_GATEWAY_INTAKE_TOKEN_FILE`); `POST /v1/admin/seal` keeps the operator
  bearer, and each refuses the other with 401. Both bearers need at least 32
  bytes, and the master refuses to start, and answers 503 on every private
  route, while they share a path or a value. Before upgrading:
  1. Create `intake.token` in the master secrets directory
     (`BASE_MASTER_SECRETS_DIR`, default `deploy/secrets/master/`) with a new
     value of at least 32 bytes, for example `openssl rand -hex 32`, mode 0600
     (or 0400) and readable by container UID 65532.
  2. Check that `operator.token` is also at least 32 bytes; rotate it if not.
  3. Keep the `role-master.yml` secrets bind to `/run/secrets`: it already
     mounts `intake.token` and sets
     `BASE_GATEWAY_INTAKE_TOKEN_FILE=/run/secrets/intake.token`. A custom
     deployment must mount the file and set that variable itself.
  4. Give `intake.token` to the raw-intake client only, then restart the master.

### Added

- `GET /v1/trust`: the gateway serves the challenge profile (ids, shares,
  measurements). Validators build their trust view from it over HTTPS.
- `deploy/compose/role-weight-setter.yml`: the only role with the validator
  hotkey and consensus seed; `role-validator.yml` is verify-only.
- Challenge containers (`docs/CHALLENGES.md`): the master loads any Docker
  challenge from an operator registry, polls its authenticated `get_weights`
  once per epoch, records its leaves and proxies its public routes under
  `/challenge/<id>/`. `cortex challenge-supervisor` pulls `stable`/`edge`/pinned
  GHCR images, checks labels and GitHub build provenance, canaries without
  secrets, updates and rolls back automatically. It alone holds the Docker socket.
- Algorithm 3 (challenge profile version >=3): 1..64 challenges,
  each paying `share * min(sum(leaves), 10^12) / 10^12`, remainder burned.
  Validators need no change beyond the release; they recompute the unsigned leaves.
- `GET /v1/metagraph/latest` serves the sealed hotkey map to challenges.
- `cortex validator-watermark-reset`: operator-only, local repair that lowers a
  wrongly advanced `challenges_version`, `measurements_version` or `epoch`
  watermark (requires `--confirm`, never raises one) and audits it in
  `consensus_watermark_resets`. It checks the file read-only first and never
  writes to a foreign or empty database; `--netuid` refuses another subnet's
  journal.

### Changed

- Wire format v2 (`protocol_version` 2, BREAKING): leaves and bundles carry no
  signatures and no gateway key. `Leaf` ends at its score, `Bundle` is the body,
  `ChallengeEntry` has no public key. The old v1 bundles and a gateway `state_dir`
  created under v1 are refused: start the gateway on a new `state_dir`. Hash
  integrity (Merkle root, metagraph root, block hash, recomputed final vector) is
  unchanged, and so is the validator hotkey signature on the chain submission.
- The validator hotkey (`--wallet-name`/`--wallet-hotkey`) is required only to
  submit weights or run `--peer-consensus`; `--verify-only` runs without it.
  `scripts/run-validator.sh` needs `WALLET_NAME`/`WALLET_HOTKEY` only to submit.
- The challenge profile is the unsigned `config/challenges.toml` and
  `measurements.toml`; changing shares is an edit, a version bump and a master restart.
- `POST /v1/weights/raw` answers 409 `epoch sealing in progress` while the
  master replaces a completed epoch's challenge leaves and seals it.
- A challenge profile whose `measurements_version` exceeds
  `MAX_MEASUREMENTS_VERSION` (1000) is refused by the master and by validators
  before the rollback watermark moves.

### Removed

- Leaf and bundle signatures: `sign_leaf`, `challenge_sig`, `gateway_sig`,
  `gateway_hotkey`, challenge leaf keys (`<id>.key`, `BASE_CHALLENGE_KEYS_DIR`),
  the gateway sealing key (`gateway.key`, `BASE_GATEWAY_SK_FILE`), validator
  `--gateway-public`/`GATEWAY_PUBLIC` and `cortex keygen`. The gateway and TLS are
  the only anchor for what validators and miners read.
- The trust-sign system: `cortex trust-sign`, `trust-verify`, owner keygen and
  ceremony, `.sig` files, `owner.pubkey`, validator `--owner-public`,
  `--challenges`, `--measurements`, `--minimum-*-version` and `BASE_GATEWAY_HOTKEY`,
  `BASE_*_MIN_VERSION`, `BASE_OWNER_PUBKEY_FILE`, `docs/how-to/trust-root.md`.
- The in-process Bounty service (`src/cortex/bounty`, `docs/BOUNTY.md` and the
  `BOUNTY_*` master settings). Bounty now runs as the CortexLM/bounty container;
  its operator guide migrates `bounty.sqlite3` and the session key unchanged.

- Algorithm 2: each valid Bounty report earned one point, all authors shared
  proportionally, and ten valid reports unlocked Bounty's full 30% of emission;
  Proof kept 70% and unused Bounty mass burned to UID0. Algorithm 1 and its
  20/80 profile stayed supported for historical sealed bytes. Activation then
  needed an offline owner signature over the profile; that signature was later
  removed with wire v2, and activation is now a version bump in the unsigned
  `config/challenges.toml`.
- Bounty-only launch mode with a revision-pinned, fully paginated CortexLM
  backend feed, bounded public writes and readiness checks that fail closed.
- Validator production preflight, resilient bounded master/validator Bittensor RPC failover and
  retry-safe official-SDK commit/reveal dispatch with ambiguous broadcasts held
  pending and non-exact preflights returning failure.
- Automatic GHCR runtime builds from `main` with vulnerability scanning, SBOM,
  immutable source tags, provenance attestations and operator-approved update
  evidence.
- Python gateway, Bounty, Proof, validator and miner commands with durable
  submission state and independent verification of sealed rewards.
- Cortex's recursive research engine with shared execution budgets, private
  journals, context compaction archives and owner-approved shared knowledge.
- Operator-driven topic setup, generated evaluator packs and measured baselines
  behind the Firecracker host/guest boundary.
- Signed Proof submission receipts and authenticated result lookup after a
  disconnected request.
- Python runtime and guest image builds, reproducible rootfs conversion and
  offline CI covering protocol, intake, persistence and reward boundaries.
- Local VM-host preflight and an opt-in Firecracker lifecycle smoke with private
  evidence, separate from model inference and challenge reward verification.

### Changed

- Initial production operation keeps the signed 2000/8000 Bounty/Proof split;
  unwired Proof burns its share instead of scaling Bounty to 100%. (Superseded:
  the profile is no longer signed.)
- **BREAKING:** Proof topic and VM JSON documents use schema version 2. Operators
  must generate compatible signed topics and guest images; old JSON signatures
  are not reinterpreted. Frozen bundle and signature domains remain unchanged.
- **BREAKING:** Bounty pairing requires a one-use operator grant binding the
  verified Cortex Chat account to its hotkey, valid for at most five minutes.
- Lium's injected execution boundary validates the complete signed topic, exact
  private setup pack and verified artifact before rent, and binds returned
  evidence to a versioned request commitment. Master wiring and a compatible
  GPU-isolated guest/image remain unavailable; pod deletion alone is not VM proof.
- Master VM resource settings explicitly size both topic and experiment guests;
  deployment examples use a small-host shape while runtime defaults stay unchanged.

### Fixed

- Historical bundle-root lookup retains accepted trust profiles across rotation
  and restart; current weights still require the current profile. The unsigned
  activation template requires an explicit epoch before it can be signed.
  (Superseded: the profile is no longer signed.)
- Bounty reconstructs a backend-capped leaderboard from the complete report
  snapshot, avoiding a permanent burn after the public log exceeds 1,000 hotkeys.
- Bounty adjudication rejects missing or misplaced severity, and external
  snapshots reject unverifiable duplicate references before scoring.
- Operator-authorized Bounty re-pairing may replace a hotkey and always revokes
  the account's previous session.
- The raw single-leaf endpoint cannot replace a positive challenge leaf with a
  nonpositive result; authoritative challenge emitters may atomically replace
  their complete unsealed snapshot. Validators refuse unsealed fallback weights.
- Validator dispatch preserves the exact sealed `u16` vector and prevents
  reconciliation from releasing an epoch while its SDK call is still active.
- Proof jobs preserve their frozen topic revision and already-earned rewards
  when topics are revised or closed.
- Accepted submissions survive client disconnects; pending credentials are
  reconciled after restart and removed only after durable terminal state.
- Bounty pairing nonces remain single-use across restarts and concurrent
  requests; rejected attempts cannot consume an unrelated operator grant.
- Explicit RLM recovery preserves host budgets and measured evidence without
  repeating completed setup or experiment commands while the original topic VM
  remains running, including responses lost after durable execution success.
- The Proof client negotiates one resume of an existing research job only on the
  authenticated host's exact recovery marker; other failures are not retried.
- Research completion closes the guest callback session before accepting a
  verdict, preventing late commands from starting after a successful result.
- A required miner BYOK name no longer needs a redundant allowlist entry at
  the VM host; missing credentials fail before research starts.
- Recursive subtasks can correct an invalid research summary within the original
  shared budgets, without replaying completed VM work or treating memory archives
  as execution evidence.
- Guest startup handles kernels that already mounted devtmpfs, and the lifecycle
  probe uses a scratch file separate from its execution workspace.
- Generated evaluator scripts can import Python's standard modules and their
  local helpers without `inspect.py` shadowing the standard library.
- Source distributions include the documentation and deployment fixtures needed
  to run their repository and deployment contract tests.

### Security

- Public Bounty status/readiness probes share short success/failure caches, and
  report intake permits one feed validation per hotkey, preventing snapshot
  amplification before quota checks.
- Private state paths reject unsafe file aliases and permissions, while BYOK
  remains in private files and is excluded from public receipts and status.
- Bounty operator grant authentication precedes bounded request parsing.
- Configuring the Proof VM host requires an explicit TLS CA file.
- Lium SSH requires operator-provisioned known host keys with strict host
  verification before rent; unknown keys are never accepted automatically.
- Lium errors and report rationales exclude raw guest and provider diagnostics
  that may contain private holdouts or credentials. Requests and private material
  are revalidated after rent and confirmed teardown before results can score.
- VM results require exact execution identity and confirmed teardown; missing
  evidence, image pins or compatible execution infrastructure fails closed.
- Proof refuses incompatible host resource ceilings before submission acceptance
  and refuses to attach a topic VM whose image or resource shape differs.
- Invalid artifact transport URIs fail before signature checks or nonce use.
  Uploaded artifact bytes discard the unused URI before any submission is stored.

[Unreleased]: https://github.com/CortexLM/cortex/tree/main
