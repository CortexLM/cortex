# Sign and verify the trust root

The owner signs two immutable TOML documents: the two-challenge allocation and
the measurement allowlist. Cortex verifies both before serving or submitting a
bundle. The committed keys are development fixtures; production ceremonies run
offline with private files that never enter Git.

## Prepare an offline directory

```bash
install -d -m 0700 /private/cortex-ceremony
cortex keygen \
  --seed-out /private/cortex-ceremony/owner.seed \
  --public-out /private/cortex-ceremony/owner.pubkey
cortex keygen \
  --seed-out /private/cortex-ceremony/gateway.seed \
  --public-out /private/cortex-ceremony/gateway.pubkey
cortex keygen \
  --seed-out /private/cortex-ceremony/bounty.seed \
  --public-out /private/cortex-ceremony/bounty.pubkey
cortex keygen \
  --seed-out /private/cortex-ceremony/proof.seed \
  --public-out /private/cortex-ceremony/proof.pubkey
```

`keygen` creates files exclusively and refuses to overwrite an existing path.
Seeds are raw 32-byte sr25519 seeds with mode 0600. Copy only the public owner
key into `config/owner.pubkey`. Put the Bounty and Proof public keys in the
matching rows of the selected challenges document. The preserved development
`config/challenges.toml` is the signed legacy 2000/8000 profile (algorithm 1).
The unsigned `config/challenges-v2.example.toml` is the 3000/7000 activation
template (algorithm 2, challenge-document version >=2). The gateway public key
is supplied to verification and is never a challenge row.

## Sign both documents

Increment `version` before replacing an accepted document and set
`introduced_epoch` deliberately. Then write detached signatures beside the
TOML files:

```bash
cortex trust-sign \
  --kind challenges \
  --input config/challenges.toml \
  --seed-file /private/cortex-ceremony/owner.seed \
  --signature-out config/challenges.toml.sig

cortex trust-sign \
  --kind measurements \
  --input config/measurements.toml \
  --seed-file /private/cortex-ceremony/owner.seed \
  --signature-out config/measurements.toml.sig
```

The command refuses to overwrite signatures. Move the previous signatures to
operator-controlled backup storage before a planned rotation. Do not work
around that guard with symlinks.

## Verify the complete set

```bash
cortex trust-verify \
  --challenges config/challenges.toml \
  --measurements config/measurements.toml \
  --owner-public config/owner.pubkey \
  --gateway-public "$(tr -d '\n' </private/cortex-ceremony/gateway.pubkey)" \
  --epoch 0
```

Verification checks detached sr25519 signatures under `base-trustroot-v1`,
document versions and activation epochs, challenge uniqueness, gateway key
separation, and an exact 10,000 basis-point total. Run the repository contract
after replacing the checked-in development fixtures:

```bash
uv run python scripts/check_repo.py --final
```

Install seeds and bearer tokens as private regular files. The master re-reads
signing material from its configured paths and fails closed if a file is
missing, group-readable, symlinked, or does not match the signed public key.

## Activate proportional Bounty

Code installation does not activate the reward change. The committed
`config/challenges.toml` and its detached signature remain the legacy development
fixture. `config/challenges-v2.example.toml` is unsigned and cannot authorize
production rewards. The new profile fixes Bounty/Proof at 3000/7000 bps and
requires bundle algorithm 2; changing only raw scores cannot implement it.

1. Upgrade the gateway/master and every submitting validator to a release that
   supports both algorithms. Keep the old signed profile while validating
   historical chain snapshots and recomputation. Existing algorithm 1 signed
   bytes and frozen vectors remain unchanged; never relax metagraph-root checks
   to admit an incompatible historical implementation.
2. Choose the activation epoch and drain every pending emission epoch older than
   it using the old profile. Pause the master scheduler for the coordinated
   rotation. A new profile cannot sign or verify an older pending epoch; leaving
   one behind blocks recovery. Back up the database, journals, old documents and
   detached signatures privately. Never clear a journal or rewrite a seal.
3. Prepare the 3000/7000 template privately, preserving the actual production
   challenge keys and participant policies. Set a monotonic challenge-document
   `version` of at least 2 and the agreed `introduced_epoch`. Sign offline using
   the existing owner. Retain the independently signed measurement document.
4. At activation, install the exact signed documents and corresponding minimum
   version pins on the gateway and validators. Use the existing verification
   command with the activation epoch. Resume the master scheduler. Missing
   owner signatures, keys or compatible chain snapshots are explicit blockers.
5. Until a new completed epoch seals under algorithm 2, latest is unsealed;
   validators must wait and must not reuse a legacy seal. Raw historical bundle
   bytes remain retrievable by epoch and must be verified using their original
   owner profile. Check `scoring_version: 2`, algorithm 2 signed leaves, a new
   `sealed: true` latest response and independent validator recomputation before
   claiming activation. A health response or local fake-provider test is not
   proof of live chain submission.

If Proof is unavailable, its 70% burns. Bounty pays at most 30%, with unused mass
burned according to [the report-count formula](../BOUNTY.md#score). Rollback must
respect persisted version watermarks: restoring an older trust file is rejected.
Stop submission and prepare an owner-authorized higher-version recovery profile
through the same ceremony rather than deleting watermarks or replaying epochs.
