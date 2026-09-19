# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

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

- Positive challenge leaves cannot be replaced by nonpositive results before
  sealing, and validators refuse unsealed fallback weights.
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
