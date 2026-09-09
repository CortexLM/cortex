# Cortex in five minutes

**Cortex is an autonomous research network.**

Its purpose is to make AI research accumulate: useful findings from independent
contributors should become reusable methods for the whole network, not disappear
when a competition selects its winning model. Cortex uses Bittensor subnet
**100** for network incentives.

## Why another research network?

The [whitepaper](../whitepaper.pdf) targets a specific pattern: contributors train
separate models, submit finished checkpoints, and tune them to a fixed evaluator.
If the network keeps only the winning checkpoint while recipes remain private,
it selects a result without retaining all the discoveries that produced it.
Repeated feedback from a visible test can also reward memorizing that test.

Proof changes the proposed unit of work from **“here are my model weights”** to
**“here is a finding, the recipe to reproduce it, and evidence that it works.”**

| Checkpoint-only competition, as modeled in the paper | Cortex's proposed research loop |
|----------------------------------------------------|--------------------------------|
| Select one finished model | Retain reproducible findings from multiple contributors |
| Recipe and data choices may stay private | Require code, a data manifest, and a declared compute budget |
| Optimize against a fixed, observable test | Publish research topics with private evaluation data and a sealed reference |
| Start another isolated training round | Reuse proven methods in a shared stack, then verify the combined update |

This is a design argument, not evidence that Cortex already outperforms every
other network. It depends on disclosure, sound evaluation, and actual reuse.
Other networks may also share recipes or use private evaluation.

## How research can accumulate

Imagine one contributor finds a faster training method and another finds a
better data-filtering rule. Keeping only one model can hide both recipes.
Keeping their code and reproducible results makes both methods available to
test in later work.

In the whitepaper's intended loop:

1. Contributors submit experiments against signed research topics.
2. A judge investigates reproduction and cheating; a separate harness measures
   private test data against a previously measured reference.
3. Accepted artifacts form a shared research collection.
4. A **synthesis agent** proposes an update to shared software, training recipes,
   or data policy using those findings.
5. The update must pass the same verification process against the current stack
   before becoming the next version.

**The synthesis agent is not implemented.** Nor does collecting two good results
prove they work together: the combined recipe must be tested. The paper's
“frontier” records the best findings across metrics; it is not automatically a
single model that achieves all of them.

See [§7 and Figure 7, pages 4–5](../whitepaper.pdf#page=4), and the
[paper-to-code comparison](WHITEPAPER.md).

## Two complementary contributions

- **Proof:** research into training, optimizers, data, systems, or agent methods.
  The intended output is a reusable experiment with measured evidence.
- **Bounty:** reproducible defects in Cortex products and services, reviewed by
  an operator. This supports product reliability, not automatic model training.

These are research and maintenance incentives, not a claim that every proposed
topic is currently available.

## How incentives work

The current configuration allocates **80% to Proof** and **20% to Bounty**.
The implemented scoring rules, not the paper's proposed equations, govern the
software:

- Proof divides its allocation equally among open topics. A topic either rewards
  the best qualifying result, with ties sharing, or uses a discovery model that
  splits a pass floor and an improvement pool. The current improvement calculation
  uses a primary metric and duplicate-artifact checks, not the paper's full
  multi-metric novelty model.
- Bounty rewards report accuracy and bug severity. Duplicate or already-fixed
  reports do not earn rewards; malicious reports can reduce a contributor's score.
- An allocation does not guarantee payment. Work must meet the evaluation rules,
  and a challenge without the evidence needed to score cannot pay contributors.

Proof now has a service leaf-emission loop (`PROOF_EMIT_POLL_SECS`, default 120).
Configured shares and a signed leaf set are not evidence of completed payments,
revenue, equity, or a promised yield.

## What “autonomous” means today

The software includes signed topics, submission intake, evaluation orchestration,
payout functions, signed bundles, and validator reward checks. These are building
blocks, not proof that the complete autonomous workflow is running.

**People still make important decisions.** Operators publish topics, configure
the judge, measure reference results, and adjudicate Bounty reports. Evaluation
runs through services on the operator's master host and rented compute.
Validators check signatures and calculations, not the scientific truth of every
claim.

Important gaps remain: the Python judge currently makes an authenticated
acknowledgement request and applies static checks, rather than investigating and
reproducing arbitrary submitted code. Proof submissions live in memory, and its
automatic reward emission is not wired into the service. Durable research
publication and the synthesis/adoption loop are also unfinished.

See the [implementation comparison](WHITEPAPER.md#proposal-versus-current-code)
and [component status](COMPLETENESS.md). A configured endpoint or image digest
does not establish scientific correctness.

## What to assess before investing

This repository demonstrates software and its tests. It does not establish
traction, revenue, research quality, or the economics of running the network.
Those require separate evidence.

Useful diligence questions include:

- How many topics are open, and how many independent contributors complete them?
- What proportion of submissions produce reproducible improvements?
- What does evaluation cost relative to the value of accepted work?
- How many accepted recipes have been reused, combined, and independently checked?
- Do combined updates improve the shared stack without regressions?
- How concentrated are contributions, rewards, and operator control?

Technical limits also matter. Proof needs the missing end-to-end wiring as well
as configured evaluation and open topics. Bounty depends on an external scoring feed.
The system does not guarantee honest operators or judges, and the master gateway
does not have automatic high availability. See the
[implementation status](COMPLETENESS.md) and [threat model](THREAT_MODEL.md).

## A short glossary

| Term | Meaning |
|------|---------|
| Miner | A contributor who submits research or bug reports |
| Topic | A research question with evaluation and reward rules |
| Baseline | A measured reference result used for comparison |
| Artifact | The submitted code or experiment files, identified by a content hash |
| Holdout | Evaluation data kept private to reduce test memorization |
| Shared stack | The software, training methods, and data practices the network aims to improve together |
| Validator | Software that verifies signed results and reward calculations |
| Weights | The relative allocation of network rewards |
| Fail closed | Stop rather than accept or reward work that cannot be verified |

[Participate](external-miner/README.md) · [Explore the architecture](ARCHITECTURE.md) ·
[Browse all documentation](README.md)
