# Threat model

Cortex protects the integrity and reproducibility of its reward protocol across
an owner-controlled master, independent validators and a dedicated KVM host. It
does not prove that the owner is honest or that generated research is
scientifically valuable.

## Security claim

An honest validator can detect a bundle that differs from the owner-signed trust
root, challenge leaves, historical metagraph or deterministic aggregation. Peer
root consensus can expose equivocation among participating validators, and
signed dissent can be retained as local evidence.

The Merkle root is not part of the Bittensor weight extrinsic. Peer consensus is
therefore not a public chain transparency log. A colluding validator set can
agree on one bad root, and an operator can delete local evidence. The owner signs
the roots and runs the master, so a malicious owner can authorize dishonest
challenge keys or generated topic rules that validators will faithfully accept.

## Trust boundaries

| Boundary | Trusted property | Residual risk |
| --- | --- | --- |
| owner trust root | challenge keys, 20/80 shares, measurement digest | owner can sign a malicious replacement |
| master gateway | durable intake and immutable seals | availability and censorship remain operator risks |
| validator | independent chain snapshot, recomputation and dispatch journal | chain RPC eclipse or colluding validators |
| CortexLM/backend feed | Bounty scoring publication | backend controls the underlying adjudication truth |
| topic RLM guest | topic-scoped setup state | model output is untrusted until checked and signed |
| experiment guest | measured run with no network and confirmed teardown | a compromised KVM host can forge its own evidence |
| model/GPU provider | inference or rented execution result | outage, dishonest output, billing and metadata leakage |
| miner artifact | no trust | parser exploits, resource exhaustion and adversarial content |

## Proof controls

Topics are dynamic owner-signed documents. Git contains no benchmark, holdout or
topic catalog. Setup runs in a topic-bound guest and can publish only after the
control plane verifies baseline measurements, exact artifact and environment
digests, resource budgets, checklist shape and teardown evidence.

Each paid evaluation runs in a fresh experiment guest with no network. The host
binds topic, job, image and artifact digests to the report and must confirm VM
destruction before a result scores. A failed run is retained for operator
diagnosis and produces no score. Artifact parsers reject symlinks, traversal,
non-tar data, empty content and digest mismatches before execution.

Miner BYOK values are validated before signature and nonce reservation, stored
in private regular files, injected only into the paid job and deleted at a
terminal outcome. They are excluded from signed submissions, public status,
receipts and logs. They remain exposed to the dedicated KVM host and the process
that contacts the provider; Cortex does not claim hardware-enforced secrecy from
those operators.

The RLM treats tool and model responses as untrusted data. Recursive children
share hard call, token, tool, depth and wall budgets. External side effects are
journaled before execution, and uncertain VM operations are reconciled instead
of replayed. Compaction stores removed exchanges by digest but does not turn a
model assertion into evidence. Shared knowledge is private and untrusted until
an owner signature approves exact content and visibility.

## Reward controls

Challenge leaves are signed under keys in the owner root. The expected set comes
from the historical metagraph; a challenge cannot shrink it by omission. A
missing score becomes an explicit `NoScore`, and an unavailable challenge burns
its share rather than blocking every other challenge.

The gateway seals only complete exact-epoch data. Individual raw-leaf intake
cannot downgrade a positive score, while the master emitter atomically replaces
the complete participant set for one challenge and epoch. A feed outage therefore
replaces every Bounty participant with `NoScore(ChallengeInternal)` and burns the
full Bounty share without retaining stale positives. `GET /v1/weights/latest`
returns an unsealed UID0 fallback when no valid seal exists; validators refuse
that fallback. A sealed UID0 burn is valid and must still be submitted after
independent verification.

Validator dispatch is journaled. An ambiguous chain response stays pending and
is not automatically retried, because a blind retry can submit twice. No test or
health endpoint proves an on-chain payment; that requires observing the actual
extrinsic and resulting chain state.

## Availability and deployment

The master and SQLite databases are availability dependencies. Durable volumes,
backups and restore drills are operator responsibilities. The Proof VM host is a
separate KVM-capable machine; the control plane has no host-process fallback.
Missing tokens, pins, offers, topics, baselines or host attestations return 503.

Production images use immutable registry digests. Kernel, rootfs, experiment
pack and evaluator digests are computed from installed artifacts. An empty pin
stays closed; operators must never invent a digest to satisfy readiness.

## Out of scope

- owner honesty and decentralized topic governance;
- public chain anchoring of bundle roots;
- scientific truth beyond the published evaluator and evidence checks;
- confidentiality from a compromised master or KVM host;
- availability of Bittensor RPC, OpenRouter, Lium or CortexLM/backend;
- proof of live KVM or on-chain behavior from fake-boundary CI tests.
