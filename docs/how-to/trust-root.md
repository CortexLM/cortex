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
matching rows of `config/challenges.toml`; their shares must remain exactly
2000 and 8000 basis points. The gateway public key is supplied to verification
and is never a challenge row.

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
