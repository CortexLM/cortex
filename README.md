# Cortex

[![CI](https://github.com/CortexLM/cortex/actions/workflows/ci.yml/badge.svg)](https://github.com/CortexLM/cortex/actions/workflows/ci.yml)
[![License](https://img.shields.io/github/license/CortexLM/cortex)](LICENSE)

**An autonomous research network.**

Cortex is building a shared research process on
[Bittensor](https://bittensor.com/) subnet **100**. Its goal is to turn independent
AI experiments into reusable knowledge that improves the network's shared
software, training methods, and data practices.

[Understand Cortex in five minutes](docs/OVERVIEW.md) ·
[Whitepaper](whitepaper.pdf) ·
[Documentation](docs/README.md) ·
[Start contributing research](docs/external-miner/README.md)

## Why Cortex exists

A competition that only selects a finished model checkpoint can lose the most
useful part of research: how the result was obtained. If recipes stay private,
the next contributor must discover them again. A fixed, visible benchmark can
also reward tuning to the test rather than an improvement that works elsewhere.

Cortex's proposed alternative is **verified research as the unit of work**:
a claim, reproducible code, a data manifest, a compute budget, and measured
evidence. The aim is to retain useful methods from many contributors, not just
the weights of one winner.

The [whitepaper, §7](whitepaper.pdf#page=4) describes the next step: an autonomous
agent combines proven findings into a proposed update to the shared stack.
That update must pass evaluation against the current stack before adoption.
**This synthesis agent is a goal, not a shipped component.** See
[the paper-to-code comparison](docs/WHITEPAPER.md) for what exists today.

## The research loop

1. **Set a research goal.** Operators publish a question, evaluation rules, and a
   measured reference result.
2. **Run experiments.** Contributors, called miners, submit a claim with the code,
   experiment files, and compute budget needed to check it.
3. **Check the result.** The intended evaluation reproduces the experiment and
   measures it on private test data against the reference.
4. **Reward useful work.** Challenge scores feed signed reward calculations that
   validators verify before submitting weights to Bittensor.
5. **Build on what worked.** Preserve the research recipe for reuse. Automated
   synthesis and verified adoption into a shared stack remain future work.

Autonomous describes the research workflow the network is building. It does
**not** mean the current system operates without people: operators choose topics,
configure evaluation, and adjudicate bug reports. Validators verify the signed
results and reward calculation; they do not independently repeat every experiment.

## Two ways to contribute

| Challenge | Useful work | Share of network emissions |
|-----------|-------------|----------------------------|
| **Proof** | Reproducible AI research against published topics | **80%** |
| **Bounty** | Verified bugs in Cortex products and backend services | **20%** |

Proof rewards follow each topic's rules: the best result wins, or qualifying
discoveries share rewards. Bounty rewards depend on report accuracy and severity.
These are configured allocations, **not guaranteed earnings or investment returns**.

## What exists today

This repository includes the submission tools, challenge services, evaluation
components, validator software, and deployment tooling.

Implemented components are not the same as an end-to-end research system:

- **Proof** has signed topics, submission intake, evaluation guards, and
  winner/discovery payout functions. The Python judge is still partial, submission
  state is in memory, and the service does not yet drive automatic reward-leaf
  emission. The complete autonomous research loop is not implemented.
- Proof evaluation needs an open topic, verified private test data, a sealed
  baseline, a pinned evaluation image, and configured Lium and judge access.
- **Bounty** needs a readable public scoring feed from CortexLM/backend. A valid
  report without an assigned severity cannot earn a reward.
- Missing scoring prerequisites cause **503** refusals. Check `ctx status` before
  spending compute, but do not treat readiness alone as proof of reproduction or
  an end-to-end payment path.

See [implementation status and known limits](docs/COMPLETENESS.md). This README
describes the software, not a claim that every research topic is open or every
deployment is ready.

## Get started

The public gateway is [https://network.cortex.foundation](https://network.cortex.foundation).

Install the `ctx` command-line tool using
[`scripts/install-ctx.sh`](scripts/install-ctx.sh):

```bash
curl -fsSL https://raw.githubusercontent.com/CortexLM/cortex/main/scripts/install-ctx.sh | sh
ctx challenges
ctx status
```

Then follow the [Proof guide](docs/external-miner/proof.md),
[Bounty guide](docs/external-miner/bounty.md), or
[validator guide](docs/external-miner/validators.md).
Never put wallet recovery phrases or challenge signing keys in a miner client.

## Explore the project

| I want to… | Start here |
|------------|------------|
| Understand the opportunity and current limits | [Project overview](docs/OVERVIEW.md) |
| Find a guide | [Documentation index](docs/README.md) |
| Understand the software | [Architecture](docs/ARCHITECTURE.md) |
| Run the network | [Deployment](deploy/README.md) |
| Contribute code | [Contributing](CONTRIBUTING.md) |
| Get help or report a vulnerability | [Support](SUPPORT.md) · [Security](SECURITY.md) |

The implementation uses Rust, with Python for research evaluation. These are
implementation choices, not the definition of Cortex. Historical `BASE_*` names
remain where required for compatibility, as explained in [Naming](docs/NAMING.md).

## License

Apache License 2.0. See [LICENSE](LICENSE).
