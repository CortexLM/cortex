# Cortex review rules

Cortex is a Python Bittensor research subnet with exactly two live challenges:
`bounty` at 3,000 basis points and `proof` at 7,000 under algorithm 2. The
owner-signed legacy 2,000/8,000 profile retains algorithm 1. The trust-root sum
is always 10,000; the new profile requires challenge-document version >=2.
Design, Prism and Relearn are historical only.

- Preserve frozen SCALE encodings, Merkle construction, aggregation and every
  `base-*-v1` signature preimage. Cross-language vectors must remain green.
  Algorithm 2 counts each valid Bounty report once, distributes proportionally,
  scales its 30% share by min(total_valid/10, 1), and burns unused/quarantined
  mass. The total includes only the signed expected participant population.
  Never allow an algorithm 1 body under the new profile or activate before
  the owner-signed epoch; historical seals and journals stay immutable.
- Preserve existing `BASE_*` names and deployed compatibility paths. New master
  settings may add the matching `CORTEX_*` alias but conflicting values fail.
- Missing or unknown image digests, offers, baselines, topics, keys, feeds and
  VM evidence fail closed. Never add a guessed digest or a simulation fallback.
- Refused Proof intake does not reserve a nonce, create a row or start paid work
  unless the documented durable acceptance point has been reached.
- Proof environment values are checked before signatures, stored only in the
  private vault, injected only into paid jobs and removed at terminal state.
- A measured result scores only after topic/job/image/artifact binding and
  confirmed dedicated VM teardown. Experiment guests have no network.
- Recursive children share the parent's hard call, token, tool, depth and wall
  budgets. External side effects require durable intent and idempotent recovery.
- Shared observations are untrusted and private until owner-signed approval.
- Bounty scoring reads only the stable CortexLM/backend public feed. An outage
  returns 503 at intake and explicit `ChallengeInternal` leaves at emission.
- Validators independently fetch historical chain state, recompute exact u16
  weights and refuse the unsealed UID0 fallback. A sealed UID0 burn is valid.
- Production images are digest-pinned. Secrets are private files and must never
  enter Git, image layers, request/status output, fixtures or logs.
- CI remains offline: no OpenRouter call, Lium rent, Firecracker boot or chain
  submission. Fake-boundary tests cannot be described as live evidence.
- API changes update the matching `docs/external-miner/` guide.
- Do not edit the three byte-pinned frozen specifications. Do not weaken
  `scripts/check_repo.py` to accept protocol drift.
