<!-- protocol_version: 1 -->

# Validator guide

## Quick start (one script)

From a clone of this repository, after `uv sync --locked --extra chain`:

```bash
scripts/run-validator.sh --verify-only   # no wallet or hotkey needed
# To submit weights, a registered validator wallet is required:
export WALLET_NAME=validator WALLET_HOTKEY=default
scripts/run-validator.sh
```

`scripts/run-validator.sh` defaults to `GATEWAY=https://chain.joinbase.ai`,
`NETUID=100`, `NETWORK=finney` and `STATE_DB=$HOME/.cortex/validator.sqlite3`.
It checks that `GET $GATEWAY/v1/weights/latest` answers with the expected
netuid, reads the challenge profile from the gateway, reads the
subnet `weights_version` from the chain (override with `VERSION_KEY`) and then
runs `cortex validator`. `--dry-run` prints the command without running it;
`--once` runs one tick. The script refuses master-only commands and master
environments. The sections below describe the underlying CLI.

Cortex runs a centralized authoritative gateway. The master gateway is the
authority for weights: it serves `/v1/weights/latest`, `/v1/weights/{epoch}`
and `/v1/bundle/{epoch}`. A Python validator consumes those endpoints,
independently verifies the bundle against the gateway-served challenge profile
and the chain, and submits the verified vector on Bittensor. Cross-checking
authenticated peer roots against other independently-operated validators is an
opt-in hardening (`--peer-consensus`), off by default.

Validators never run challenge containers or Proof evaluation, rent GPUs or
receive miner provider credentials; they need no registry, Docker or challenge
token. Challenge shares come from the challenge profile the gateway serves at
`GET /v1/trust` (the master reads `config/challenges.toml`): legacy
bounty/proof 2000/8000 (algorithm 1), `bounty` 3000 bps and `proof` 7000 bps
(algorithm 2), or any unique challenge set summing to 10000 bps (algorithm 3,
see [Changing emission shares](../../deploy/README.md#changing-emission-shares)).

Sealed `GET /v1/weights/latest` responses expose `algorithm_version` from the
bundle. `emission_shares` contains the configured ceilings; each
`source_challenges[].emission_percent` reflects its allocated share, including
the ten-report Bounty ramp under algorithm 2 and
`min(sum(leaves), 10^12) / 10^12` of the share under algorithm 3. Author allocations to UID0 or
unmapped hotkeys still burn. Validators always recompute from the leaves;
they never trust these display fields as consensus inputs.

## Prepare identity

Install the [Python package with its chain extra](README.md#installation).
Obtain the gateway HTTPS URL. The validator builds its challenge profile
(challenge ids, shares, measurements) from `GET /v1/trust` on that gateway and
refuses a profile whose version goes backwards. TLS to the gateway is the trust anchor;
bundles are unsigned and checked by content hashes and the chain.

Verifying needs no wallet: run with `--verify-only` and no `--wallet-name` or
`--wallet-hotkey`. Submitting weights needs a registered Bittensor hotkey in a
standard wallet (`--wallet-name`, `--wallet-hotkey`, optional `--wallet-path`);
the CLI refuses to submit without them. `--consensus-seed-file`, a private file
with the 32-byte hexadecimal seed for **that same hotkey**, is required only with
`--peer-consensus`; startup refuses a mismatch. The miner CLI's encrypted-wallet
adapter and `--dev-seed-file` option do not apply here. Do not assume that a
derived wallet key has an exportable matching seed.

The Bittensor wallet signs on-chain extrinsics. The consensus seed signs peer
roots and dissent using the frozen Cortex context. No gateway wallet or
gateway administrative token is required on a validator.

## Configure peers (opt-in)

Peer cross-check is **off by default**: with the centralized gateway as the
weight authority, a validator submits on the gateway's authority alone and
`--peers` and `--min-peer-sample` are inert. Pass `--peer-consensus` to require
a peer-root sample; do that only in a deployment with other independently
operated validators whose endpoints you have configured, because the sample
then becomes a precondition for submission. The local equivocation guard
applies in both models: a validator that has already signed a different root
for an epoch refuses and persists a `PeerRootConflict` dissent.

With `--peer-consensus` enabled, the options below apply as written.

`--peers` reads a JSON object mapping independently selected validator hotkeys
(SS58 or hex) to HTTPS origins. Use actual registered peers, for example the
structure below with your own hotkey and origin:

```json
{
  "<peer hotkey>": "https://<peer host>:8091"
}
```

Origins may not include credentials, paths, queries or fragments. The default
`--min-peer-sample 1` requires one other validator with `validator_permit` in
the same metagraph snapshot. If there is no other permitted validator, the
single-validator case is allowed. Setting the sample to zero cannot bypass
peer checking when another permitted validator exists, so a single validator on
a subnet that has other permitted validators must leave `--peer-consensus` off
rather than lower the sample. Peer endpoint discovery is manual in this
implementation.

Each peer serves a signed root for the requested epoch. Wrong identity,
invalid signature, insufficient reachable peers, conflicting roots or
persisted equivocation prevent submission. A matching HTTP response alone is
not authentication; the expected hotkey signature is required.

## Run the validator

Verify-only (no hotkey, never submits):

```bash
uv run cortex validator --verify-only --once \
  --gateway "$GATEWAY" --network "$CHAIN_NETWORK" --netuid "$NETUID" \
  --version-key "$VALIDATOR_VERSION_KEY" --state-db ./state/validator.sqlite3
```

The command below can submit on-chain weights and is the only form that needs a
wallet. Set its network, netuid, peer address and local paths to the intended
deployment before running it:

```bash
uv run cortex validator \
  --gateway "$GATEWAY" \
  --network "$CHAIN_NETWORK" --netuid "$NETUID" \
  --fallback-endpoints "$CHAIN_FALLBACK_ENDPOINTS_JSON" \
  --version-key "$VALIDATOR_VERSION_KEY" \
  --wallet-name validator --wallet-hotkey default \
  --consensus-seed-file /private/consensus.key \
  --peers ./config/peers.json \
  --state-db ./state/validator.sqlite3 \
  --peer-bind "$PEER_BIND" --peer-port 8091 \
  --peer-tls-certificate /private/tls.crt \
  --peer-tls-key /private/tls.key
```

`--wallet-path` defaults to `~/.bittensor/wallets`; polling defaults to
30 seconds. The subnet validator version key is required; obtain it from the live
subnet instead of guessing. `--fallback-endpoints` is a JSON list of at most eight unique `wss://`
RPC origins without credentials, queries or fragments. The Bittensor SDK uses
them when the primary endpoint fails. Fallbacks are origins only: URL paths are
also rejected so provider bearer material cannot appear in the process argv or
container metadata.

The example passes `--peers` without `--peer-consensus`, so the file is loaded
and validated but no peer sample is required; add `--peer-consensus` to enforce
it in a multi-validator deployment.

The peer listener defaults to loopback and requires TLS when bound outside
loopback. It serves this validator's signed roots to peers regardless of
`--peer-consensus`, which governs only what this validator requires of others.
`--once` executes one tick and can submit weights. Add both
`--verify-only --once` for a preflight that recomputes the current seal but
never claims its epoch in the journal and never creates an extrinsic. It exits
successfully only for the exact `verified` outcome; unsealed, changed, degraded
or peer-unavailable states return a nonzero process status.

For Compose use the Python
[validator environment example](../../deploy/env/python-validator.env.example)
with the verify-only [validator role](../../deploy/compose/role-validator.yml) (no
hotkey) or the [weight-setter role](../../deploy/compose/role-weight-setter.yml)
(the only role with the validator hotkey and consensus seed). Keep the SQLite
state persistent and wallet, identity and config directories read-only. The
role points to the master gateway and has no gateway or challenge service.

## Verify before dispatch

The validator reads `GET /v1/weights/latest`, obtains the corresponding binary
bundle, then verifies the gateway-served challenge profile. It checks protocol version, epoch and block hash,
metagraph stakes and UID mapping, emission shares,
participant completeness, measurements and Merkle root. Frozen wire details
are in the [bundle specification](../BUNDLE_SPEC.md).

The profile is refetched each tick, its activation window is checked, and
durable version watermarks reject rollback. The sealed block must not be
in the future or more than `--max-block-lag` blocks old (default `256`). Before
dispatch the validator rechecks both chain snapshot and latest gateway state;
a reorg or a changed/unsealed latest response prevents submission.

| Latest state or verification outcome | Validator action |
|-------------------------------------|------------------|
| `sealed: false`, including no bundle or decode failure | Do not submit; do not reuse a previously verified seal |
| Verified sealed Match with `burn_outcome: true`, `uids: [0]`, `weights: [1.0]` | Submit the sealed burn to UID 0 |
| Verified sealed vector allocating 100% to a nonzero owner or validator-permit UID | Refuse; this is not a burn |
| Valid inputs (and agreeing peers where required), but gateway's final vector differs | Class A: submit independently recomputed weights and persist signed dissent |
| A challenge has a wrong leaf epoch or a leaf hash that breaks the Merkle root or incomplete participants | Quarantine that challenge; submit only if surviving mass is at least 5000 bps |
| Structural failure, unknown challenge, invalid Merkle root, own prior root for the epoch disagrees, or peer disagreement with `--peer-consensus` | Refuse |

Under algorithm 2, quarantining Bounty retains Proof's absolute 7000 bps
and burns Bounty's 3000 bps. Quarantining Proof leaves only 3000 bps and
cannot be submitted. Algorithm 3 applies the same rule to every challenge: the
quarantined share burns and the rest must keep at least 5000 bps. No
quarantined share is reassigned. Legacy algorithm 1
keeps its original 5000-bps gate and survivor renormalization. Valid
`NoScore(ChallengeInternal)` leaves from an unavailable scorer still cover the
expected participants and are not themselves a consensus fault.

The unsealed fallback is deliberately a 100% UID 0 vector with `sealed: false`.
A missing gateway owner wallet is unrelated. Only a verified **sealed** burn
is eligible for submission. Neither a status response nor a last-known-good
bundle overrides that rule.

## Chain submission and evidence

The validator verifies the exact sealed `u16` vector, then uses the official
Bittensor SDK pallet-call builder, drand helper and `Subtensor` signer without
renormalizing those values. This preserves the frozen on-chain payload; the
high-level SDK weight helper is not used because it rescales multi-UID vectors.
Preflight requires live registration, permit, rate-limit, minimum-weight,
maximum-weight and CRv4 metadata compatible with the sealed bundle. Unknown
state fails closed. Inclusion and finalization are awaited, with the Cortex
journal as the only retry authority.

The persistent journal is bound to one netuid and prevents duplicate dispatch
for an epoch. Active calls are recorded as `dispatching`; only an `uncertain`
attempt may be reconciled, so an operator cannot release a claim while the SDK
call is still running. A failure before broadcast or a finalized on-chain
rejection releases the claim and can be retried. A timeout or failure after
submission may have reached the chain; it returns `dispatch_pending`, logs a durable
`attempt_id` plus any SDK extrinsic hash/nonce, and keeps the claim until the
operator reconciles chain evidence. Validator startup changes an interrupted
`dispatching` attempt to `uncertain`; the reconciliation CLI itself never does.
Hash the private evidence file, then resolve
the exact attempt with either `submitted` or `not_broadcast`:

```bash
cortex validator-reconcile \
  --state-db /var/lib/base/validator.sqlite3 \
  --netuid 100 --epoch 25203 \
  --digest <bundle-digest> --attempt-id <attempt-id> \
  --result submitted --evidence-digest <sha256-of-private-evidence>
```

Only `not_broadcast` releases the epoch for a retry. Keep the underlying chain
receipt or nonce query private; the journal stores its digest. Do not delete the
database to force a retry.

### Repairing a consensus watermark

The journal keeps rollback watermarks for `challenges_version`,
`measurements_version` and `epoch`; a lower served value fails with
`consensus rollback refused`. If a watermark was advanced wrongly (for example a
profile version published by mistake and then withdrawn), stop the validator and
lower it locally. The command touches only the state database: no network, no
wallet.

```bash
cortex validator-watermark-reset \
  --state-db /var/lib/base/validator.sqlite3 \
  --name challenges_version --to 2 --confirm
```

It refuses without `--confirm`, refuses a name outside those three, a negative
value, a watermark that was never set and any value above the current one (it
only lowers or repairs). It prints the old and new values and appends an audit
row to `consensus_watermark_resets(name, old_value, new_value, at)` in the same
transaction.

**Warning:** resetting below a version or epoch this validator really accepted
re-opens rollback to that value. Confirm the correct value with the master
operator first.

The peer listener exposes:

| Route | Response |
|-------|----------|
| `GET /livez` | Process liveness for the local container healthcheck |
| `GET /v1/consensus/root/{epoch}` | Signed observed root, or `404` |
| `GET /v1/bundle/root/{root}` | Persisted binary bundle, or `404` |
| `GET /v1/dissent/{epoch}` | Signed SCALE dissent records encoded as hex |
| `POST /v1/attest/nonce` and `/v1/attest/submit` | `503`, `verified: false`; DCAP verification is not implemented |

Consensus uses sr25519 signing context `base-sr25519-v1` and SCALE-encoded
domain/payload vectors. Exact domains are `base-root-v1`, `base-dissent-v1`,
(`base-bundle-v1`, `base-rawweight-v1` and `base-trustroot-v1` are retired). Generic Substrate
`wallet.sign()` signatures cannot replace these signatures.

The Python metagraph projection uses chain-provided integer stakes. Validate
interoperability before mixing it with historical validators that projected
different metagraph data. This implementation does not claim DCAP attestation
or demonstrated production payment merely because peer roots agree.

See [troubleshooting](troubleshoot.md) for refusal outcomes.
