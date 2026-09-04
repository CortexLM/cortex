# Deploy secrets (NEVER commit secret bytes)

Containers run as `base` (uid **65532**). Host secret files MUST be:

```bash
for f in gateway_sk relearn_sk relearn_t2i_sk relearn_agent_sk bounty_sk proof_sk; do
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
| `relearn_sk` | relearn-challenge | Relearn LLM leaf mini-secret; pub must match `config/challenges.toml` |
| `relearn_t2i_sk` | relearn-t2i-challenge | Relearn Image (`relearn-image`) leaf mini-secret; pub must match `config/challenges.toml` |
| `relearn_agent_sk` | relearn-agent-challenge | Relearn Agent leaf mini-secret; pub must match `config/challenges.toml` |
| `bounty_sk` | bounty-challenge | Bounty leaf mini-secret; pub must match `config/challenges.toml` |
| `proof_sk` | proof-challenge | Proof leaf + topic-document mini-secret; pub must match `config/challenges.toml` |
| `prism_sk` / `design_sk` | retired products | Do not mount on the live compose path |

```bash
# Generate once per environment; never commit the bytes.
openssl rand -hex 32 > deploy/secrets/gateway_admin_token
chown 65532:65532 deploy/secrets/gateway_admin_token
chmod 0400 deploy/secrets/gateway_admin_token
```

Local dummy for development: decrypt with age:
`age -d -i ~/.base-secrets/age-identity.txt -o deploy/secrets/design_sk ~/.base-secrets/design-dummy.age`

## Relearn T2I / Relearn Multimodal

| Path | Used by | Notes |
|------|---------|-------|
| `relearn/holdout.json` | relearn-challenge | Frozen holdout items. **Never commit.** Verified at boot against `holdout_commitment` in `config/relearn-pin.toml`; a mismatch means submissions answer **503**. Mode **0400**, uid **65532** |
| `relearn-t2i/holdout.json` | relearn-t2i-challenge | Frozen holdout prompt records. **Never commit.** Verified at boot against `holdout_commitment` in `config/relearn-t2i-pin.toml`; a mismatch means submissions answer **503** rather than falling back to the public split. Mode **0400**, uid **65532** |
| `relearn-t2i/admin_tokens` | relearn-t2i-challenge | One operator bearer per line for `POST /v1/admin/promote` |
| `relearn-t2i/base_champion.json` | relearn-t2i-challenge | Base checkpoint measured by the pinned eval image. **Never commit.** A live host answers **503** on every submission until it exists, because every gate compares against it. Mode **0400**, uid **65532** |
| `relearn-agent/episodes.json` | relearn-agent-challenge | Frozen holdout **recorded traces** (goal, tool schemas, steps with arguments/observations, final answer). **Never commit.** Verified at boot against `holdout_commitment` in `config/relearn-agent-pin.toml`. Mode **0400**, uid **65532** |
| `relearn-agent/base_champion.json` | relearn-agent-challenge | Base checkpoint measured by the pinned agent eval image, including the trace-replay and ablation arms. **Never commit.** Mode **0400**, uid **65532** |
| `relearn-agent/admin_tokens` | relearn-agent-challenge | One operator bearer per line for `POST /v1/admin/promote` |
| `proof/topics.json` | proof-challenge | Signed topic documents (JSON array). **Never commit secrets**; the documents themselves are operator-published. Mode **0400**, uid **65532** |
| `proof/holdouts.json` | proof-challenge | Per-topic holdout records (array or map keyed by `topic_id`). **Never commit.** Verified at boot against each topic's `holdout_commitment`. Mode **0400**, uid **65532** |
| `proof/baselines.json` | proof-challenge | Sealed baseline measurements keyed by topic id. **Never commit.** Mode **0400**, uid **65532** |
| `proof/admin_tokens` | proof-challenge | One operator bearer per line for `POST /v1/admin/proof/topics` |

Regenerate the Relearn holdout with a **private** salt (never the T2I/dev salt
and never a salt that is committed to git):

```bash
mkdir -p deploy/secrets/relearn
mapfile -t EXCLUDE < <(python3 -c '
import tomllib
from pathlib import Path
doc = tomllib.loads(Path("config/relearn-pin.toml").read_text())
for i in doc.get("public_ids", []):
    print(f"--exclude={i}")
')
cargo run -p xtask -- relearn-holdout \
  --catalog ~/.base-secrets/relearn-catalog.json \
  --salt "$RELEARN_HOLDOUT_SALT" \
  --size 120 "${EXCLUDE[@]}" \
  --out deploy/secrets/relearn/holdout.json
# Paste the printed holdout_commitment into config/relearn-pin.toml and
# re-sign the trust root (config/CEREMONY.md).
```

Regenerate the T2I holdout with the private salt (keep the salt off git — it is
what makes the holdout unguessable):

```bash
mkdir -p deploy/secrets/relearn-t2i deploy/secrets/relearn-agent
# Public split ids come from the pin; every one of them must be excluded.
mapfile -t EXCLUDE < <(python3 -c '
import tomllib
from pathlib import Path
doc = tomllib.loads(Path("config/relearn-t2i-pin.toml").read_text())
for i in doc["prompts"]["public_ids"]:
    print(f"--exclude={i}")
')
cargo run -p xtask -- relearn-t2i-holdout \
  --bench ~/.base-secrets/qwen_image_bench_hf_v0518.jsonl \
  --salt "$RELEARN_T2I_HOLDOUT_SALT" \
  --size 40 "${EXCLUDE[@]}" \
  --out deploy/secrets/relearn-t2i/holdout.json
# Paste the printed holdout_commitment into config/relearn-t2i-pin.toml and
# re-sign the trust root (config/CEREMONY.md).
touch deploy/secrets/relearn-t2i/admin_tokens deploy/secrets/relearn-agent/admin_tokens
chown -R 65532:65532 deploy/secrets/relearn-t2i deploy/secrets/relearn-agent
chmod 0400 deploy/secrets/relearn-t2i/* deploy/secrets/relearn-agent/*
```

The Relearn Multimodal service also needs `RELEARN_MM_CHAMPION_LM_HASH` (the
SHA-256 of the champion Relearn LLM weights). It is not a secret — it is the
reference an encoder-only submission must hash-match — but without it those
submissions are rejected.

## Design challenge

| Path | Used by | Notes |
|------|---------|-------|
| `design/annotator_tokens` | design-challenge | One bearer token per line; hashed (SHA-256) at boot. Mode **0400**, uid **65532** |
| `openrouter/api_key` | design-egress-proxy (**proxy only**); also prism reviewer | Never mount OpenRouter key into design-challenge or sandbox containers |

```bash
mkdir -p deploy/secrets/design deploy/secrets/openrouter
touch deploy/secrets/design/annotator_tokens deploy/secrets/openrouter/api_key
chown -R 65532:65532 deploy/secrets/design deploy/secrets/openrouter
chmod 0400 deploy/secrets/design/annotator_tokens deploy/secrets/openrouter/api_key
```

## Other

- `lium/` — prism Lium API + SSH keys (see prism runbook)
- `wallets/` — btcli wallet trees for gateway owner / validator hotkeys
- `github/token` — prism-challenge top-model publisher: fine-grained GitHub
  token with **contents:write** (and **contents:write** on Releases for large
  checkpoints) on `BaseIntelligence/prism` only. Read via
  `PRISM_TOPMODEL_GITHUB_TOKEN_FILE` (`/run/base/github/token`); missing or
  empty file = top-model publish silently disabled. Mode **0400**, uid
  **65532** — never commit it.
- `huggingface/token` — prism-challenge HuggingFace top-model publisher: Hub
  **write** token for `BaseIntelligence/top-prism-architecture` (override
  `PRISM_TOPMODEL_HF_REPO`). Read via `PRISM_TOPMODEL_HF_TOKEN_FILE`
  (`/run/base/huggingface/token`); missing or empty = HF publish no-ops.
  Mode **0400**, uid **65532** — never commit it.

```bash
mkdir -p deploy/secrets/huggingface
touch deploy/secrets/huggingface/token
chown 65532:65532 deploy/secrets/huggingface/token
chmod 0400 deploy/secrets/huggingface/token
```

- `prism/admin_tokens` — one operator bearer per line for Prism
  `/v1/submissions/{id}/retry`, `POST /v1/admin/playground/complete`,
  `POST /v1/admin/gating/{hotkey}/reset`, and
  `POST|GET /v1/admin/artifacts/...`. Read via `PRISM_ADMIN_TOKENS_FILE`
  (`/run/base/prism/admin_tokens`). Empty/missing → those routes answer
  **503 `auth_unconfigured`** (fail-closed). Mode **0400**, uid **65532**.
- `prism/payer_vault_key` — 32-byte (or 64-hex) key for TTL-bounded encrypted
  miner BYOK seals (`PRISM_PAYER_VAULT_KEY_FILE`; default TTL ≥36h). Host dir
  `/var/lib/prism/payer-vault` is mounted RW for `*.seal` files. **Never
  commit the key.** Generate once:

```bash
mkdir -p deploy/secrets/prism /var/lib/prism/payer-vault
openssl rand -hex 32 > deploy/secrets/prism/payer_vault_key
chown 65532:65532 deploy/secrets/prism/payer_vault_key /var/lib/prism/payer-vault
chmod 0400 deploy/secrets/prism/payer_vault_key
chmod 0700 /var/lib/prism/payer-vault
```
