# AGENTS.md - Cortex research network

This file is the working contract for code and operator changes. Link to the
canonical documentation instead of duplicating runbooks.

## Product

Cortex is an autonomous research subnet on Bittensor. Proof runs inside the
master. Every other challenge is a Docker container that implements
[the challenge contract](docs/CHALLENGES.md); the master loads it from an
operator registry and signs its leaves.

| Challenge | Source | Purpose |
| --- | --- | --- |
| `proof` | this repository | operator-created research topics evaluated by Cortex's recursive language-model engine |
| `bounty` | [CortexLM/bounty](https://github.com/CortexLM/bounty) container | useful vulnerability reports, scored from the CortexLM/backend public feed |
| `opentype` | [OpentypeAI/challenge](https://github.com/OpentypeAI/challenge) container | exact-gold typed-decision duels on DiffusionGemma weights |

Shares always sum to 10,000 basis points. Algorithm 1 is the legacy signed
bounty/proof 2,000/8,000 profile; algorithm 2 is bounty/proof 3,000/7,000
(document version >=2); algorithm 3 (document version >=3) admits any 1..64
unique ids and pays each challenge `share * min(sum(leaves), 10^12) / 10^12`.
Activation is an offline owner ceremony. Design, Prism and Relearn are retired
products. Their frozen specifications and historical miner pointers remain for
compatibility; no active code, service, trust-root row or leaf may register
them.

Start with [the architecture](docs/ARCHITECTURE.md),
[Proof](docs/PROOF.md), [challenge containers](docs/CHALLENGES.md) and
[the threat model](docs/THREAT_MODEL.md). Do not describe a fake-boundary test,
one model call or a pinned image as proof of live KVM execution, scientific
reproduction or on-chain payment.

## Source map

| Path | Responsibility |
| --- | --- |
| `src/cortex/protocol/` | frozen SCALE, signatures, Merkle and aggregation |
| `src/cortex/gateway/` | durable leaves, immutable seals and burn fallback |
| `src/cortex/validator/` | independent recomputation, root consensus and chain dispatch |
| `src/cortex/challenges/` | container registry, weights client, public proxy and auto-updating supervisor |
| `src/cortex/proof/` | topics, submissions, setup, executor offers and reward allocation |
| `src/cortex/rlm/` | recursive agent, budgets, compaction, journals and shared knowledge |
| `src/cortex/vm/` | Firecracker host, guest protocol and measured experiment lifecycle |
| `deploy/` | Python images, master/validator roles and VM-host service |
| `tests/` | protocol, service, HTTP, persistence and lifecycle regressions |

The working branch is `main`. Production releases are annotated `v*.*.*` tags.

## Consensus and compatibility

- Preserve `BASE_*` names, deployed compatibility paths and every `base-*-v1`
  signature domain. `CORTEX_*` is an accepted master alias only.
- Do not edit `docs/BUNDLE_SPEC.md`, `docs/DESIGN_CHALLENGE.md` or
  `docs/PRISM.md`. Their exact SHA-256 values are checked by
  `scripts/check_repo.py`.
- Production image references are digest-only. Never invent a digest; an empty
  or unknown pin remains fail-closed.
- Secrets are private files. Do not put seeds, mnemonics, bearer values, BYOK,
  certificates or provider keys in Git, images, command output or cloud-init.
- The gateway and challenge execution run on the master role only. Validators
  fetch sealed bundles and independently submit weights.

`GET /v1/weights/latest` returns an unsealed UID0 burn vector when no valid seal
exists. Validators must refuse that fallback and any persisted last-known seal
while latest is unsealed. A verified **sealed** UID0 burn is a valid Match and
must be submitted. A sealed allocation entirely to an ineligible nonzero owner
or permit UID is not a submit path.

## Key roles

| Key or token | Role |
| --- | --- |
| owner seed | signs challenge and measurement trust documents offline |
| gateway seed | signs immutable epoch bundles |
| challenge seed | `<id>.key`; signs that container challenge's leaves and must match the trust root |
| Proof seed | signs Proof topics and leaves and must match the trust root |
| operator bearer | protects master administrative routes |
| VM orchestrator bearer | authenticates master to the dedicated KVM host; not a wallet |
| validator hotkey | signs root/dissent evidence and Bittensor submissions |
| challenge internal token | master bearer for a container's `get_weights`; never a signing key |
| miner hotkey | signs Bounty pairing or Proof submission payloads |

Do not conflate gateway sealing, master ownership and validator chain signing.
Follow [the trust-root ceremony](docs/how-to/trust-root.md).

## Challenge container contracts

- The owner-signed trust root decides emission; the unsigned registry
  (`deploy/challenges/registry.toml`) only decides what runs. A container never
  receives a leaf-signing seed.
- `challenge-supervisor` is the only process with the Docker socket. It verifies
  image labels and GitHub build provenance, canaries without secrets, rolls back
  on failure and never reads a secret.
- A missing, failing or invalid `get_weights` answer covers every expected
  participant with `NoScore(ChallengeInternal)`: that share burns to UID0 and
  never moves to another challenge.
- The public proxy `/challenge/<id>/` never forwards `internal/` paths or
  credentials other than `authorization`.
- A contract change must update `docs/CHALLENGES.md`, both challenge
  repositories and the E2E tests in the same change. Bounty API changes belong
  to CortexLM/bounty; keep `docs/external-miner/bounty.md` pointing there.

## Proof contracts

Topics are operator-created signed data. No benchmark, dataset, task list,
metric, model, rule catalog, holdout or live topic belongs in Git.

- Setup runs the RLM in one persistent Firecracker VM bound to `topic_id`.
  Generated endpoints, documentation, rules and baselines are published only
  after exact evidence checks and owner signing.
- Every paid evaluation runs in a fresh networkless experiment VM. A result can
  score only after measured evidence is bound to topic, job, image and artifact
  digests and VM destruction is confirmed.
- Production uses a dedicated KVM host over HTTPS plus a bearer file. Missing
  URL, token, CA, image pin, offer, registered runner or teardown attestation is
  503. There is no control-plane host fallback.
- Firecracker resource requests may tighten host ceilings. They never exceed
  16 vCPU or 32 GiB RAM and are rejected rather than clamped.
- Executor shape is exactly `1x`. CI injects a fake adapter and never rents
  Lium. A live adapter must resolve a digest-bound template, rent idempotently
  by job ID and confirm deletion.

Submission intake verifies the topic is open and compatible before spending.
The miner signature domain remains `base-proof-submit-v1`. The signed payload
binds hotkey, topic, exact artifact digest, declared FLOPs, claim, canonical
manifest and a single-use 64-hex nonce. Environment values and URI transport are
not signed; validate them before signature and nonce reservation. A malformed or
undeclared variable is 400 and must not burn the nonce.

Uploaded artifact bytes win over a URI. The served file is the identity:
uncompressed tar, nonempty content, no traversal/symlink/special entry and exact
SHA-256. A fetch or verification failure produces no score. Accepted evaluation
continues after client disconnect and is recoverable using the original signed
envelope; lookup never consumes a nonce or exposes another miner's row.

Miner BYOK is stored in a private file vault, re-read for deferred jobs, injected
only into the paid guest and removed at a terminal result. Never put its value in
the database, status, receipt or drain report. Owner inference credentials never
travel through the miner environment path.

Anti-cheat checks run before paid inference. One failed signed rule persists a
reject without spend. WTA assigns topic mass to its winner; discovery divides
pass-floor and novelty mass. Global Proof score is the sum of per-topic mass.
Zero open topics, an unsealed baseline or an incompatible backend is 503.

Any request, response or scoring change must update
`docs/external-miner/proof.md` in the same change.

## RLM contracts

Cortex uses its own recursive engine; do not add Prime Agent or another agent
harness. Parent and children share hard call, token, tool, recursion and wall
budgets. Compaction preserves immutable policy, completed phases and measured
evidence; exact removed messages remain addressable by digest. Journal an
external side effect before starting it and reconcile an uncertain operation
instead of replaying it.

Shared observations are untrusted and topic-private by default. Cross-topic
visibility requires an owner signature over exact content, visibility and
verification evidence. Shared memory cannot silently revise a signed topic.

The configured OpenRouter model is `deepseek/deepseek-v4.1-flash`. Live model
tests are opt-in, use a private key file and private state directory, and stay
outside CI.

## Required verification

Use meaningful tests at public boundaries. Unit tests cover deterministic
protocol logic; integration tests cover SQLite, HTTP and component boundaries;
E2E tests cover the reward path with fake external providers. Do not add tests
that merely mirror implementation or assert mocks called themselves.

```bash
uv run --no-sync ruff format --check src tests scripts
uv run --no-sync ruff check src tests scripts
uv run --no-sync mypy
uv run --no-sync pytest -m 'not live'
uv run --no-sync python scripts/check_repo.py --final
uv run --no-sync python scripts/check_deploy.py --check-examples
uv build --no-build-isolation
```

Challenge verification must exercise a real intake request, failure probes,
signed leaves, gateway seal, a `sealed: true` fetch and validator recomputation.
Health endpoints alone do not verify a challenge. CI must not contact OpenRouter,
rent provider compute, boot Firecracker or submit an on-chain transaction.

Pull requests require a Greptile review. If silent, comment `@greptileai review`.
Commit subjects use `type(scope): lowercase summary`, at most 72 characters.

## Never commit

- `.env` or materialized `deploy/env/*.env`;
- anything under `deploy/secrets/` except its README;
- wallets, seeds, mnemonics, API keys, bearer values, `*.pem`, `*.key`, `*.age`;
- SQLite state, provider receipts, retained VM consoles or experiment output;
- Terraform state/variables, caches, virtual environments, wheels or coverage;
- generated topics, private holdouts, model transcripts or live offer documents.
