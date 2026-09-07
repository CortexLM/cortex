# Cortex threat model

This document states the **honest** security claim and the properties we deliberately do **not** claim. Overclaiming is treated as a project failure mode.

Frozen contracts: [`BUNDLE_SPEC.md`](./BUNDLE_SPEC.md), [`DESIGN_CHALLENGE.md`](./DESIGN_CHALLENGE.md).  
Architecture map: [`ARCHITECTURE.md`](./ARCHITECTURE.md). Prism: [`PRISM.md`](./PRISM.md).

CI enforces that §1 matches plan decision **D19** word for word (`cargo run -p xtask -- external-docs-check`).

---

## 1. D19 — honest security claim (verbatim)

The following paragraph is copied from plan decision D19 and MUST remain byte-identical to that decision (modulo a single trailing newline). Do not paraphrase in this section.

base guarantees *no equivocation between validators* and *no undetected deviation by the gateway from the owner-signed challenge and measurement artifacts*. It does **not** guarantee (i) that a challenge's scores are honest, (ii) that the owner is honest — the owner signs the trust roots and runs the gateway, so a malicious owner can authorize a dishonest challenge or a backdoored measurement, (iii) completeness beyond what D24 provides, nor (iv) **chain-anchored, third-party-auditable non-equivocation** — per D5 the property is peer-consensus plus local evidence, verifiable by the participating validators and not by an outside observer after the fact.

---

## 2. What attestation does **not** prove (D11)

Env integrity is scoped honestly:

- We prove the `allowed_envs` **name list** via compose-hash.
- We use a `LAUNCH_TOKEN` whose **hash** is in the measured compose.
- We do **not** claim env **values** are verified.
- Secrets are **mounted files**, never measured env values.

dstack does not measure env values. Any doc or pitch that says "TEE proves all secrets" is false for this system.

Related attestation bounds (see also AGENT_CHALLENGE and attest policy crates):

| Outcome | Meaning |
|---------|---------|
| Cryptographic failure | **Reject** |
| Verifier / collateral outage | **Park** (no attestation credit this epoch; never carries prior `Verified` forward) |
| `report_data` binding | Epoch, netuid, miner key, nonce, validator hotkey (D10) — not "the agent is smart" |

Attestation proves **which measured code** answered a **fresh, bound** challenge for this epoch. It does not prove score honesty (D19(i)).

---

## 3. What non-equivocation does **not** rest on (D5)

**Non-equivocation does NOT use the on-chain weight payload.**

`WeightsTlockPayload` is frozen to `{hotkey, uids, values, version_key}`:

- There is **no field** for a merkle root.
- `version_key` is 64 bits, far too small for a 256-bit root.

Non-equivocation therefore rests on:

1. **(a)** In-epoch signed peer root exchange over hotkey-authenticated HTTPS.
2. **(b)** Every validator durably persisting the signed bundle plus all peer root statements as **local evidence**.
3. **(c)** An on-chain announcement via the commitments pallet is **optional and conditional** — only if metadata snapshot confirms the pallet on the target network. The design does not depend on it.

**State this weakening loudly:** non-equivocation here is *peer-consensus plus local evidence*, **not** chain-anchored auditability.

Consequences:

- There is no public `(epoch → bundle_root)` anchor.
- A third party cannot verify after the fact from chain alone.
- Local evidence can be deleted by whoever holds it.
- A fully colluding set of reachable validators could agree on one root with no public counter-evidence.

**The merkle root is NOT committed in the on-chain weight payload.** Do not re-add it. SCALE would reject extra bytes; the field does not exist.

---

## 4. Owner-concentration caveat (R12)

**The owner is the trust root AND the gateway operator.**

This is not solvable inside the current design. It is bounded by:

- D19 (owner honesty out of scope).
- Trust-root rotation as a **signed, reviewable release**, never a hot push (D21).
- Dual-accept window so validators can adopt `v(n+1)` beside `v(n)` for `rotation_epochs` (default 3).

Future work (not claimed here): multi-sig owner keys, transparency log for trust-root releases.

A malicious owner can authorize a dishonest challenge or a backdoored measurement. Validators will still agree with each other and with the owner-signed artifacts. That is **not** a bug relative to D19; it is the stated trust boundary.

---

## 5. Assets and adversaries (summary)

| Asset | Primary protection |
|-------|--------------------|
| Weight vector integrity among honest validators | Bundle verify + recompute + peer roots (D4/D6/D18/D24) |
| Challenge key provenance | Local owner-signed `challenges.toml` only (D18) |
| Emission shares | Same trust root; gateway copy must match (D23) |
| Participant completeness | Validator-derived expected set (D24) |
| Miner code identity / liveness | TDX quote + D10 `report_data` + measurements allowlist |
| Operator secrets | age-encrypted files, mode 0600, never cloud-init / TF state (R11) |
| Host docker.sock | Only on `socket-proxy` with method allowlist |
| Site origin (joinbase.ai cookies/session/DOM) vs miner HTML | Layered viewer sandbox (R13) |

| Adversary | Expected residual risk |
|-----------|------------------------|
| Deviant gateway | Detected by validators that verify local roots + recompute (task 48 class A/B) |
| Forged challenge key in gateway DB | Rejected: key absent from local trust root (D18) |
| Censorship / set shrinking | Rejected: incompleteness / proper-subset (D24) |
| Eclipsed validator | `Degraded`, no submit below `min_peer_sample` (D26) |
| Compromised challenge sk | Verifiable garbage; quarantine + rotation (R10, D6, D21); honesty not restored |
| Malicious owner | In scope of D19(ii) / R12 — not eliminated |
| Colluding validator set deleting evidence | D19(iv) / D5 — no public anchor |
| Malicious miner HTML/JS (stored XSS on the site origin via `/v1/view`) | Blocked by R13 layering; any single layer suffices |

---

## 6. Operational risks called out in the plan

| ID | Risk | Mitigation (claimed) |
|----|------|----------------------|
| R9 | Gateway death takes down registry, proxy, bundle serving | `restart: unless-stopped` + healthcheck; manual failover runbook. **HA not claimed.** |
| R12 | Owner = trust root + gateway operator | D19 + signed rotation releases |
| R4 | Zero emission possible | Extrinsic success + revealed weights match recompute is pass; emission is not |
| R13 | Miner-generated design pages XSS-ing the joinbase.ai origin (cookie/session theft, phishing) when viewed | Four independent layers, each sufficient alone: (1) ammonia sanitize strips `<script>`/handlers before storage; (2) response CSP `sandbox` with **no** `allow-scripts`/`allow-same-origin` → opaque origin, scripts disabled, no cookie/storage access, `frame-ancestors` allowlist, never `Set-Cookie`; (3) gateway proxy re-applies the header floor and strips `Set-Cookie` on `/challenge/*/v1/view/*` (survives stale upstreams); (4) frontend embeds with `<iframe sandbox="">`. Browser-tested: injected `<script>` stays inert under each layer independently. Produced HTML is never served (screenshots-only). |
| R14 | *(retired)* Screenshot Chromium inside design-challenge SSRF | Design product removed; `design-egress-proxy` is gone. Gateway `/challenge/*/v1/view/*` lockdown (R13 layers 3–4) remains. |

---

## 7. Doc hygiene

- Never claim merkle is on-chain in the weight payload.
- Never claim owner honesty or public third-party auditability of non-equivocation.
- Never put secrets, tokens, mnemonics, or private keys in this tree.
- Miner docs must carry the same `protocol_version` badge as `bundle::PROTOCOL_VERSION` (CI-gated).

---

## 8. Experimental Proof v2 orchestration

Signed consent binds one exact quote, including account, recipe, machine,
image and cost. Postgres serializes consent/nonces and controller fences.
Late provider responses are retained for cleanup, not treated as authority
to revive a cancelled experiment. Resource names never grant access.
Canonical migrations 0020–0028 and `db::test_pool` retain append-only runtime
events and restricted column grants. Worker quote refresh, strict adoption and
same-fence invocation refusal do not grant a model provisioning authority.
An independent cleanup lane remains available when experiment execution is busy.
The generic controller enforces the original DB runtime deadline even if an
agent ignores stop; shutdown aborts pending lease acquisition and drops suspended
operation/heartbeat futures before DB bookkeeping. Atlas cancellation prevents a
blocking RPC completion after shutdown from freezing a new round.

Database fencing cannot cancel an already-issued provider request. The strict
broker requires provider-enforced spending/expiry bounds and authoritative
request-id reconciliation; no live Lium adapter currently proves those
guarantees. Atomic request-id, expiry, cost and stopped-billing guarantees have
not been established for the integration; this is not evidence that Lium lacks
them. DELETE acknowledgement alone cannot certify resource absence or stopped
billing. Local fake-provider tests establish controller behavior, not
provider-enforced isolation. `proof-challenge` mounts only opt-in v2 routes and
does not start experiments; live credential and quote adapters remain unwired.

Agents and miner kernels must not receive provider credentials, signing keys,
or write access to authoritative evidence. Miner administrator control remains
a measurement-integrity risk; a recipe/image commitment does not prove an
honest execution. The local pinned-Docker executor runs real CPU scripts and
retains output, exit status, wall time and failures, but has no independent
metric/FLOPs observer. `collect` fails closed with `UnobservedMeasurements`.
Synthetic paired-observation admission tests and a real authorized model/kernel
IPC round trip create no successful science; independent measurement and
chain-derived collection provenance remain missing.

Shared `HeadlessProcess` / `CortexRuntime` recovery preserves the original run
UUID, absolute deadline and budget journal, with attempt-scoped private sockets.
Missing recovery state must not reset spend. Model config and `apiKeyFile` stay
host-only and private; HTTPS is the default, with `allowLoopbackHttp: true` only
for literal `127.0.0.1` / `[::1]` HTTP. No Factory/environment credential fallback
is allowed. Atlas private files have private parents and the signer is checked
from the opened bytes, without reopening its path; its public key must match
the pin. These controls are not an attestation of installed launcher code.
Private Atlas IPC reads only its frozen corpus, and controller code
validates/signs decisions without exposing keys.

Seventeen process-supervision tests cover inherited pipes, leader exit, a
five-second TERM→KILL grace period, future-drop/reaper behavior and a `setsid`
escape. Process-group signalling alone does not stop that escape; the optional
`headless.pid_namespace` (`unshare --pid --kill-child`) does, because the
launcher is PID 1 of a private namespace. That is PID containment only: no
mount, network or user isolation for the launcher itself. Leader exit alone
does not establish descendant termination without the namespace.

W&B SDK v0.28.0 automatic runtime/environment telemetry violates the seven-field
public allowlist. No safe publication adapter exists; ordinary SDK startup must
not be treated as allowlisted publication. Headless accounting uses configured
prices, not provider billing evidence; the synthetic test's temporary rates do
not establish a tariff or actual cost.

Signed-byte replay and Postgres outbox fences alone cannot prevent delayed remote
overwrites. The concrete strict `proof-publication` / `gateway-proof` receiver
adds immutable full-batch signed publication and round ordering; reconciliation
requires the exact receipt **and** read-back bytes, never HTTP 409 as success.
Sticky gateway v2 activation blocks legacy Proof ingress and stale seals.
It does not automatically seal or submit chain weights.

The separate opt-in Atlas scheduler/publisher does not rent or seal.
Its `--check` validates Rust-side files/signer/policy and restricted DB, not
TypeScript provider/model initialization, installed image presence, live
chain/gateway compatibility or deployment. No deployed receiver/live publication
has been verified. The ignored headless-delivery regression passed real
scheduler/Postgres → unmodified runtime/Docker Python → private decision →
strict HTTP gateway/exact readback → production seal helper → served router and
independent Python vector `[0.4, 0.4, 0.2]`; lost-ack recovery reused identical
signed bytes without model rerun. Chain, science and inference remain synthetic,
and the direct-helper test does not cover the operator admin seal HTTP route.
Three binary startup/shutdown regressions passed, including SIGTERM while
waiting for finality; canonical DB migration and private-file checks passed.
See the [implementation boundary](runbooks/proof-autonomy-local.md).
