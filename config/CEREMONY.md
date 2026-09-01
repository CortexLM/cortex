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

### Live challenges (Relearn + Relearn Image + Relearn Agent + Bounty)

Current committed `challenges.toml` has **four** rows: `relearn` @ 4000,
`relearn-image` @ 1500, `relearn-agent` @ 1500, and `bounty` @ 3000 bps
(sum = 10000). Operator may retune shares; the sum must remain 10000, and no
two rows may share a public key.

`relearn-mm` is **off**: it has no row, so it has no emission and no leaf
signed by its key can verify. Turning it on later is a normal ceremony — add a
row with its own key and move bps out of the other four.

**Key reuse from the pre-launch layout.** `relearn-image` carries the public
key that the pre-launch `relearn-t2i` row used, and `relearn-agent` carries the
one `relearn-mm` used. Both are throwaway CI keys, and only one id is live per
key, so nothing can cross-verify today. Production **must** still keygen a
fresh secret per live challenge — a key shared between two ids would let one
challenge's leaves verify as the other's the moment both are live.

A production owner/key ceremony:

1. Keygen production `relearn_sk` / `relearn_image_sk` / `relearn_agent_sk` /
   `bounty_sk` (keep off-git; materialize under `deploy/secrets/`).
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
