<!-- protocol_version: 1 -->

# Validator guide

Python validators independently verify gateway bundles, compare authenticated
peer roots and submit weights on Bittensor. They never run Bounty or Proof
evaluation, rent GPUs or receive miner provider credentials. The only live
challenge shares are `bounty` 2000 bps and `proof` 8000 bps.

## Prepare local trust and identity

Install the [Python package with its chain extra](README.md#installation).
Obtain the gateway URL and independently pinned gateway public key, plus the
owner public key and signed `challenges.toml` and `measurements.toml` trust
files. Detached signatures live beside each TOML as `.toml.sig`. Do not replace
local trust files with unverified content received from the gateway.

Use a registered Bittensor hotkey in a standard wallet. The current validator
CLI also requires `--consensus-seed-file`: a private file containing the
32-byte hexadecimal seed for **that same hotkey**. Startup refuses a mismatch.
This additional seed requirement is a current validator limitation; the miner
CLI's encrypted-wallet adapter and `--dev-seed-file` option do not apply here.
Do not assume that a derived wallet key has an exportable matching seed.

The Bittensor wallet signs on-chain extrinsics. The consensus seed signs peer
roots and dissent using the frozen Cortex context. No gateway wallet or
gateway administrative token is required on a validator.

## Configure peers

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
peer checking when another permitted validator exists. Peer endpoint discovery
is manual in this implementation.

Each peer serves a signed root for the requested epoch. Wrong identity,
invalid signature, insufficient reachable peers, conflicting roots or
persisted equivocation prevent submission. A matching HTTP response alone is
not authentication; the expected hotkey signature is required.

## Run the validator

This command can submit on-chain weights. Set its network, netuid, peer address
and local paths to the intended deployment before running it:

```bash
uv run cortex validator \
  --gateway "$GATEWAY" --gateway-public "$GATEWAY_PUBLIC_KEY" \
  --network "$CHAIN_NETWORK" --netuid "$NETUID" \
  --fallback-endpoints "$CHAIN_FALLBACK_ENDPOINTS_JSON" \
  --owner-public ./trust/owner.pubkey \
  --challenges ./trust/challenges.toml \
  --measurements ./trust/measurements.toml \
  --minimum-challenges-version "$CHALLENGES_MIN_VERSION" \
  --minimum-measurements-version "$MEASUREMENTS_MIN_VERSION" \
  --version-key "$VALIDATOR_VERSION_KEY" \
  --wallet-name validator --wallet-hotkey default \
  --consensus-seed-file /private/consensus.key \
  --peers ./trust/peers.json \
  --state-db ./state/validator.sqlite3 \
  --peer-bind "$PEER_BIND" --peer-port 8091 \
  --peer-tls-certificate /private/tls.crt \
  --peer-tls-key /private/tls.key
```

`--wallet-path` defaults to `~/.bittensor/wallets`; polling defaults to
30 seconds. Trust minimums and the subnet validator version key are required;
obtain them from the signed production ceremony and live subnet instead of
guessing. `--fallback-endpoints` is a JSON list of at most eight unique `wss://`
RPC origins without credentials, queries or fragments. The Bittensor SDK uses
them when the primary endpoint fails. Fallbacks are origins only: URL paths are
also rejected so provider bearer material cannot appear in the process argv or
container metadata.

The peer listener defaults to loopback and requires TLS when bound outside
loopback. `--once` executes one tick and can submit weights. Add both
`--verify-only --once` for a preflight that recomputes the current seal but
never claims its epoch in the journal and never creates an extrinsic. It exits
successfully only for the exact `verified` outcome; unsealed, changed, degraded
or peer-unavailable states return a nonzero process status.

For Compose use the Python
[validator environment example](../../deploy/env/python-validator.env.example)
and [validator role](../../deploy/compose/role-validator.yml). Keep the SQLite
state persistent and wallet, identity and trust directories read-only. The
role points to the master gateway and has no gateway or challenge service.

## Verify before dispatch

The validator reads `GET /v1/weights/latest`, obtains the corresponding binary
bundle, then verifies the independently pinned gateway signature and local
owner-signed trust root. It checks protocol version, epoch and block hash,
metagraph stakes and UID mapping, challenge keys and emission shares,
participant completeness, measurements and Merkle root. Frozen wire details
are in the [bundle specification](../BUNDLE_SPEC.md).

Trust files are reloaded, their activation windows and minimum versions are
checked, and durable watermarks reject rollback. The sealed block must not be
in the future or more than `--max-block-lag` blocks old (default `256`). Before
dispatch the validator rechecks both chain snapshot and latest gateway state;
a reorg or a changed/unsealed latest response prevents submission.

| Latest state or verification outcome | Validator action |
|-------------------------------------|------------------|
| `sealed: false`, including no bundle or decode failure | Do not submit; do not reuse a previously verified seal |
| Verified sealed Match with `burn_outcome: true`, `uids: [0]`, `weights: [1.0]` | Submit the sealed burn to UID 0 |
| Verified sealed vector allocating 100% to a nonzero owner or validator-permit UID | Refuse; this is not a burn |
| Valid inputs and agreeing peers, but gateway's final vector differs | Class A: submit independently recomputed weights and persist signed dissent |
| A challenge has invalid leaf signatures, wrong leaf epoch or incomplete participants | Quarantine that challenge; submit only if surviving signed mass is at least 5000 bps |
| Structural failure, unknown challenge, invalid Merkle root or peer disagreement | Refuse |

With the fixed split, quarantining Bounty can retain Proof's 8000 bps;
quarantining Proof leaves only 2000 bps and cannot be submitted. Valid
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
`base-bundle-v1`, `base-rawweight-v1` and `base-trustroot-v1`. Generic Substrate
`wallet.sign()` signatures cannot replace these signatures.

The Python metagraph projection uses chain-provided integer stakes. Validate
interoperability before mixing it with historical validators that projected
different metagraph data. This implementation does not claim DCAP attestation
or demonstrated production payment merely because peer roots agree.

See [troubleshooting](troubleshoot.md) for refusal outcomes.
