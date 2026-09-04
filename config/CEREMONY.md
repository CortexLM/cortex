# Trust-root signing ceremony (task 18 / D12 / D18 / D21)

Offline, operator-only. Never commit secrets. Prefer `/root/.base-secrets/` (mode `0700`).

## Artifacts in git (public only)

| Path | Contents |
|------|----------|
| `config/owner.pubkey` | 32-byte owner public key (hex). **Throwaway for tests** — not the production `base-owner` coldkey mnemonic. |
| `config/challenges.toml` | Challenge id, public key, emission bps (sum = 10000), participant policy. |
| `config/challenges.toml.sig` | Detached sr25519 signature under `base-trustroot-v1`. |
| `config/measurements.toml` | Measurement allowlist; empty = fail-closed (base-agent CVM path removed). |
| `config/measurements.toml.sig` | Detached owner signature. |

### Live challenges (Bounty + Proof)

Current committed `challenges.toml` has **two** rows: `bounty` @ 2000 and
`proof` @ 8000 bps (sum = 10000). This is a Proof-weighted 20%/80% lock
regardless of eval digest. Proof's `eval_image_digest` is empty (live
submits **503**); do not invent a sha256. Neither share is a leftover Relearn
/ Prism / Design inheritance. Operator may retune shares; the sum must
remain 10000, and no two rows may share a public key.

`relearn`, `relearn-image`, `relearn-agent`, `relearn-mm`, `design`, and
`prism` are **off**: they have no row, so they have no emission and no leaf
signed by their keys can verify. Relearn* code stays in the repo behind the
`relearn` / `mm` compose profiles. Turning one on later is a normal ceremony —
add a row with its own key and move bps out of the live two.

A production owner/key ceremony:

1. Keygen production `bounty_sk` / `proof_sk` (keep off-git; materialize under
   `deploy/secrets/`).
2. Replace the matching `public_key` rows in `config/challenges.toml`.
3. Optionally move bps between challenges (sum must remain 10000).
4. Re-sign with the **production** owner key (`sign --kind challenges`).
5. Verify under `config/owner.pubkey` (or the production owner pubkey after rotation).

### Re-signing without the previous owner secret

`config/owner.pubkey` is a throwaway CI key and its secret is not in git, so
any edit to a signed body needs a **new** throwaway keypair. That is the
supported CI path, not a workaround:

```bash
cargo run -p trustroot-bin -- keygen \
  --out-pub /tmp/ceremony/owner.pubkey --out-secret /tmp/ceremony/owner.secret
cp /tmp/ceremony/owner.pubkey config/owner.pubkey
for f in challenges challenges.staging; do
  cargo run -p trustroot-bin -- sign --key /tmp/ceremony/owner.secret \
    --input "config/$f.toml" --kind challenges
done
cargo run -p trustroot-bin -- sign --key /tmp/ceremony/owner.secret \
  --input config/measurements.toml --kind measurements
```

Every body has to be re-signed together: the three `.sig` files must all
verify under the single committed `owner.pubkey`. The secret stays in `/tmp`
(or `~/.base-secrets/`) and never enters the repo. A production rotation uses
the offline owner key instead and follows
[`../docs/runbooks/trust-root-rotation.md`](../docs/runbooks/trust-root-rotation.md).

## Secret layout (never git)

| Path | Contents |
|------|----------|
| `~/.base-secrets/age-identity.txt` | age X25519 identity (mode 600). |
| `~/.base-secrets/owner-throwaway.age` | age-encrypted owner mini-secret. |
| `~/.base-secrets/challenge-*.age` | age-encrypted challenge mini-secrets. |

## Commands

```bash
# 1. age identity (once)
age-keygen -o ~/.base-secrets/age-identity.txt
RECIPIENT=$(grep 'public key:' ~/.base-secrets/age-identity.txt | awk '{print $4}')

# 2. Owner keypair (throwaway for CI; production uses offline HSM / air-gapped owner key)
cargo run -p trustroot-bin -- keygen \
  --out-pub config/owner.pubkey \
  --out-secret ~/.base-secrets/owner-throwaway.age \
  --age-recipient "$RECIPIENT"

# 3. Challenge keypair (secret stays off-git)
cargo run -p trustroot-bin -- keygen \
  --out-pub ~/.base-secrets/challenge-dummy.pub \
  --out-secret ~/.base-secrets/challenge-dummy.age \
  --age-recipient "$RECIPIENT"
# paste public_key into challenges.toml

# 4. Sign bodies (payload = scale(version, introduced_epoch, scale(body)))
cargo run -p trustroot-bin -- sign \
  --key ~/.base-secrets/owner-throwaway.age \
  --age-identity ~/.base-secrets/age-identity.txt \
  --input config/challenges.toml --kind challenges

cargo run -p trustroot-bin -- sign \
  --key ~/.base-secrets/owner-throwaway.age \
  --age-identity ~/.base-secrets/age-identity.txt \
  --input config/measurements.toml --kind measurements

# 5. Verify
cargo run -p trustroot-bin -- verify \
  --owner-pub config/owner.pubkey \
  --input config/challenges.toml --kind challenges
```

## Signature preimage

Domain tag: `base-trustroot-v1` (via `crypto`).

```text
payload = scale(version: u32, introduced_epoch: u64, body: Vec<u8>)
body    = scale(ChallengesBody | MeasurementsBody)
```

## D21 rotation

Publish `v(n+1)` beside `v(n)` (directory of versioned TOML files). Loaders accept both for `rotation_epochs` (default 3 from `config`) after `introduced_epoch` of the newer file, then drop the old version.

## Fail-closed rules

- Missing TOML or `.sig` → error
- Signature not under `owner.pubkey` → `NonOwner`
- Empty `measurements` → every quote rejected (pre-task-35 bootstrap)
- No HTTP: this crate never fetches trust roots over the network
