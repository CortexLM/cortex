# Operator security checklist

Use this checklist before a deployment, trust-root rotation or incident
recovery. It complements the [threat model](THREAT_MODEL.md).

## Credentials

- [ ] Every seed, bearer, wallet password, provider key and CA private key is an
  untracked regular file with mode 0400 or 0600 inside a 0700 directory.
- [ ] No credential is present in an environment example, image layer, compose
  build argument, Terraform state, cloud-init payload, log or shell history.
- [ ] Master, Bounty, Proof and VM-host tokens are distinct and rotated
  independently.
- [ ] Miner BYOK vault storage is durable only as long as queued work requires,
  and terminal jobs have no remaining secret files.
- [ ] The OpenRouter owner key stays on the VM host and is not exposed to miner
  jobs; miner BYOK never falls back to it.

## Trust and pins

- [ ] `cortex trust-verify` accepts the installed challenge and measurement
  documents at the deployment epoch.
- [ ] Bounty and Proof public keys match their mounted signing seeds, and the
  gateway public key is different from both.
- [ ] The trust root contains only `bounty = 2000` and `proof = 8000`.
- [ ] Runtime, kernel, rootfs, evaluator and experiment-pack references use
  verified SHA-256 digests. No production image uses a floating tag.
- [ ] Empty or unknown pins remain fail-closed; no digest was copied from an
  unrelated artifact or invented to satisfy a check.
- [ ] Inference and executor offers are signed, open, within platform ceilings
  and bound to the exact installed configuration.

## Network and isolation

- [ ] The gateway and both challenge APIs run only on the master role.
- [ ] The validator role exposes no challenge execution route and reaches the
  master only through the configured VPC/TLS endpoint.
- [ ] The VM orchestrator runs on a dedicated KVM-capable host with mutual
  network restrictions, a SAN-correct TLS certificate and a bearer file that is
  re-read per request.
- [ ] Topic guests have only the explicit setup egress allowlist. Experiment
  guests have no network interface.
- [ ] Firecracker and jailer binaries, kernel and rootfs pass the host startup
  validation; production never enables the fake hypervisor or host simulation.
- [ ] Resource ceilings do not exceed 16 vCPU, 32 GiB RAM and the configured VM
  concurrency. Oversized topic requests fail instead of being clamped.

## Service readiness

- [ ] Bounty `/v1/status` reports `can_score: true` after a real stable feed
  probe, and a report outage test returns 503 without a row.
- [ ] Proof `/v1/status` reports a valid topic, sealed baseline, registered
  runner, pinned image, open inference offer and compatible executor offer.
- [ ] A Proof failure matrix confirms missing token, host outage, bad artifact,
  closed offer and teardown failure all return 503 without a scored row.
- [ ] Operator routes reject missing and wrong bearer values from a non-loopback
  client.
- [ ] Gateway latest is sealed before validators are enabled. An unsealed UID0
  fallback is observed and rejected during the drill.

## Persistence and recovery

- [ ] Master SQLite files and WAL state reside on a durable private volume and
  are backed up with the service quiesced or through SQLite's backup API.
- [ ] Restore testing covers gateway seals, epoch journal, Proof jobs, setup
  jobs, topic evidence, Bounty sessions and validator dispatch state.
- [ ] Pending external jobs are reconciled by stable job ID after restart; an
  uncertain paid operation is not blindly repeated.
- [ ] Failed experiment VMs and bounded console tails are retained in the
  designated private RCA directory, reviewed, then deleted deliberately.

## Release gate

```bash
uv run --no-sync ruff format --check src tests scripts
uv run --no-sync ruff check src tests scripts
uv run --no-sync mypy
uv run --no-sync pytest -m 'not live'
uv run --no-sync python scripts/check_repo.py --final
uv run --no-sync python scripts/check_deploy.py --check-examples
uv build --no-build-isolation
```

- [ ] The wheel installs and its `cortex` entry point runs outside the source
  tree.
- [ ] Compose renders for master and validator roles without an unpinned image,
  mounted Docker socket or challenge service on the validator.
- [ ] CI made no live model call, GPU rent, KVM boot or chain submission.
- [ ] The separate live smoke used the intended model and a fresh private state
  directory, with its API key absent from output and repository history.
- [ ] Offline success is described accurately: it does not claim live KVM,
  scientific reproduction, provider billing or confirmed on-chain payment.
