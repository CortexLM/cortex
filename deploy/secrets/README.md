# Deploy secrets (NEVER commit secret bytes)

Containers run as `base` (uid **65532**). Host secret files MUST be:

```bash
for f in gateway_sk bounty_sk proof_sk; do
  chown 65532:65532 "deploy/secrets/$f"
  chmod 0400 "deploy/secrets/$f"
done
```

Bind-mounts use the file inode; directory mode 0700 is OK.

## Challenge / gateway keys

| Path | Used by | Notes |
|------|---------|-------|
| `gateway_sk` | gateway | Bundle seal mini-secret (`BASE_GATEWAY_SK_FILE`) |
| `gateway_admin_token` | gateway + seal scripts | Bearer for `/v1/admin/*` (`BASE_GATEWAY_ADMIN_TOKEN_FILE`). **Required** when `BASE_GATEWAY_REQUIRE_OWNER=1`. Mode **0400**, uid **65532** |
| `bounty_sk` | bounty-challenge | Bounty leaf mini-secret; pub must match `config/challenges.toml` |
| `proof_sk` | proof-challenge | Proof leaf + topic-document mini-secret; pub must match `config/challenges.toml` |

```bash
# Generate once per environment; never commit the bytes.
openssl rand -hex 32 > deploy/secrets/gateway_admin_token
chown 65532:65532 deploy/secrets/gateway_admin_token
chmod 0400 deploy/secrets/gateway_admin_token
```

## Proof + Bounty operator files

| Path | Used by | Notes |
|------|---------|-------|
| `proof/topics.json` | proof-challenge | Signed topic documents (JSON array). **Never commit secrets**; the documents themselves are operator-published. Mode **0400**, uid **65532** |
| `proof/holdouts.json` | proof-challenge | Per-topic holdout records (array or map keyed by `topic_id`). **Never commit.** Verified at boot against each topic's `holdout_commitment`. Mode **0400**, uid **65532** |
| `proof/baselines.json` | proof-challenge | Sealed baseline measurements keyed by topic id. **Never commit.** Mode **0400**, uid **65532** |
| `proof/admin_tokens` | proof-challenge | One operator bearer per line for `POST /v1/admin/proof/topics` |
| `proof/inference_offer.json` | proof-challenge | Live RLM judge `InferenceOffer` (provider kind, origin, mode, model_ref, token caps, `config_commitment`, status). Consumed by proof-eval; **not** a miner training proxy. **Never commit.** Missing/closed → `can_score=false` / 503. Mode **0400**, uid **65532** |
| `proof/inference_api_key` | proof-challenge | Provider API key for the eval image. **Never commit, never log.** Mode **0400**, uid **65532** |
| `proof/inference_base_url` | proof-challenge | Optional secret-backed origin (`PROOF_INFERENCE_BASE_URL_FILE`) when pin `[inference].base_url` and the topic omit one. **Never commit, never log.** Mode **0400**, uid **65532** |
| `bounty/admin_tokens` | bounty-challenge | Operator bearer for `POST /v1/admin/adjudicate` |
| `bounty/session_secret` | bounty-challenge | Pairing session HMAC secret |

## Other

- `lium/` — Lium API + SSH keys for Proof harvest (BYOK). Never log or commit `LIUM_API_KEY`.
- `wallets/` — btcli wallet trees for gateway owner / validator hotkeys
