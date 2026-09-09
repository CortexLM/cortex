# Reading the Cortex whitepaper

Source: [*A Proposal for a New Incentive Mechanism on Cortex (Bittensor Subnet
100): Bounty and Proof*](../whitepaper.pdf), version 1.1, September 2026.
This guide covers all six pages. It explains the proposal; it does not change
the protocol, scoring rules, or deployment configuration.

## The central idea

**Pay for reproducible discoveries that others can build on, not only for the
best finished model.**

The paper critiques a particular research competition: miners train separate
checkpoints, recipes remain private, and a static evaluator selects a winner.
The network gets the chosen model, but does not necessarily retain the methods
behind the other experiments. Repeated tuning to a visible evaluator can also
improve the test score without improving performance elsewhere.

Proof proposes submitting a claim together with code, an environment/data
manifest, a compute budget, and an artifact commitment. Operators publish signed
topics. A pinned judge investigates reproduction and cheating, while a separate
harness measures private holdouts against a baseline committed before opening.
The useful object is the reproducible experiment, not weights alone.

## What “improving the network” means

Section 7 proposes keeping proven artifacts so complementary findings can be
reused across the network. One result might improve training speed; another
might improve data quality. A collection preserves both instead of reducing
research to the identity of one winning checkpoint.

The proposed **synthesiser** is a second, digest-pinned agent. It reads those
artifacts and proposes changes to the shared harness, training recipe, or data
policy. It does not merge raw miner weights. Its proposal must itself pass
evaluation against the current stack before adoption.

The paper explicitly calls this the **intended end state**, not a deployed
component (§7–8, pages 4–5). A multi-metric frontier is a record of best findings;
it does not prove a combined implementation can achieve them all. That requires
another experiment.

## Reading map

| Pages / sections | Argument |
|------------------|----------|
| 1, §1–2 | Incentives determine what a subnet produces; visible static tests can reward overfitting and hide recipes |
| 2–3, §3–4, Figures 2–5 | Signed topics, reproducible artifacts, sealed baselines, private measurements, winner/discovery rewards |
| 3–5, §5, Figure 6 | Bounty pays for human-adjudicated production defects with published evidence and severity |
| 4 and 6, §6, Figure 8 | Hashes, signatures, commitments, and later disclosure make parts of centralized evaluation auditable |
| 4–5, §7, Figure 7 | A verified research collection could support shared, repeatedly tested improvements |
| 5, §8 | Operator/judge trust, reproduction scale, and unfinished synthesis remain limitations |
| 5–6 | References |

## Proposal versus current code

The whitepaper is not an exact specification of the current implementation.
Use [Proof](PROOF.md), [Bounty](BOUNTY.md), and the
[bundle specification](BUNDLE_SPEC.md) for today's contracts.

| Area | Paper's proposal | Current repository |
|------|------------------|--------------------|
| Topics and baselines | Signed dynamic topics; measured baseline sealed before opening | Topic signatures, floors, holdout commitments, and baseline verification exist in [`proof-task`](../crates/proof-task/src/topic.rs), [`proof-store`](../crates/proof-store/src/lib.rs), and [`proof-eval`](../crates/proof-eval/src/lib.rs); operators supply the files and configuration |
| Autonomous judge | Plan an investigation, reproduce submitted code, detect cheating | [`judge.py`](../eval/src/proof_eval/judge.py) requests a short acknowledgement and does not parse a scientific verdict. [`agent.py`](../eval/src/proof_eval/agent.py) performs static text checks. This is not the proposed recursive research agent |
| Artifact reproduction | Run the committed recipe under its declared budget | [`cli.py`](../eval/src/proof_eval/cli.py) inspects the claim plus a local baseline script and measures a configured model directory. It does not implement general reproduction of arbitrary committed miner recipes |
| Private measurements | Blind harness, canaries, provenance checks, later replay | Manifest overlap checks and holdout commitments exist. [`harness.py`](../eval/src/proof_eval/harness.py) measures model loss and optional throughput; custom and canary metrics remain unset. This is not evidence that all proposed scientific checks are complete |
| Winner payout | Multi-metric improvement; earliest commitment breaks ties | [`payout.rs`](../crates/proof-score/src/payout.rs) ranks one primary metric and splits exact ties |
| Discovery payout | Ordered multi-metric frontier increments plus method-descriptor novelty and floor controls | The code splits a pass floor and a primary-metric improvement pool, using the sealed baseline/optional champion and exact artifact-digest duplicate checks. It does not implement the paper's full frontier or descriptor-distance rule |
| Topic allocation | Weighted topics and proposed composition equations | Open topics receive equal masses; miner scores sum those masses. The signed configuration allocates Proof 8000 bps and Bounty 2000 bps |
| Proof emission | Evaluated research reaches network rewards | [`ProofEmitter`](../crates/proof-challenge/src/emit.rs) signs exact-`E` leaves every `PROOF_EMIT_POLL_SECS` (default 120) from in-process scores, or covers `E` with `NoScore(ChallengeInternal)` so D24 can seal. Store is in-memory (a restart loses submissions); the scored-epoch watermark is persisted and the gateway refuses a burn from replacing a positive leaf. This is not the paper's automatic research-to-payment path |
| Shared research collection | Open artifacts with durable, replayable reports | Proof exposes submission records through HTTP, but the service uses `MemoryStore`; a restart loses submissions and scores. An artifact URI/digest is not a durable public archive |
| Bounty and validators | Validators consume public adjudications directly | The Bounty service consumes the external feed and emits signed leaves. Validators verify bundles and recompute allocation, rather than independently re-judge research or poll that feed |
| Failure handling | Abstain rather than invent scores | Missing prerequisites refuse scoring. Current bundle rules use explicit `NoScore` leaves and burn allocation; an unsealed fallback is never a valid validator submit path |
| Synthesis and adoption | Combine proven methods, verify the proposal, adopt only without regression | No synthesiser or automatic shared-stack adoption loop is implemented |

These gaps are documented, not silently filled by this cleanup. In particular,
the scoring equations and reward configuration are unchanged.

## What the comparison does not prove

- The checkpoint example assumes private recipes and selection of one result.
  It is not a description of every other subnet or of checkpoint sharing itself.
- The paper's compute and scaling comparison is a model under assumptions, not
  a measured cost saving or demonstrated advantage for a deployed Cortex system.
- Private tests and pinned images can reduce some attack opportunities; they do
  not guarantee zero overfitting, an honest operator, or a correct judge.
- A signature establishes who signed a result. A digest identifies content.
  Neither alone proves that the committed code ran correctly or that a claim
  generalizes. See the [threat model](THREAT_MODEL.md).
- Progress should be measured by reproducible findings, independent reuse,
  verified combined improvements, and evaluation cost, not submission volume
  alone.
