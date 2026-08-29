# 20 — Everything works locally before prod

Nothing is "ready", "mergeable", or "shippable" until this page is green **on
your machine**. CI is a second opinion, not your first test run.

## 1. Workspace gates (must all pass)

These are exactly the commands `.github/workflows/ci.yml` runs, in the same
order. If you add a gate to CI, add it here in the same PR — `rules-check`
fails when CI runs a `cargo` or `bash deploy/` command this file does not list.

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check
cargo run -p xtask -- loc-cap
cargo run -p xtask -- consensus-lint
cargo run -p xtask -- spec-check
cargo run -p xtask -- design-check
cargo run -p xtask -- external-docs-check
cargo run -p xtask -- rules-check
cargo run -p xtask -- version check
cargo clippy -p validator-bin --features dcap --all-targets -- -D warnings
bash deploy/scripts/assert-compose-matrix.sh
```

Rust is pinned to **1.96.0** by `rust-toolchain.toml`; use it, do not float.

Prism's Python harness has its own CI job. Run it when you touch
`crates/prism-recipe/harness/`:

```bash
python -m compileall crates/prism-recipe/harness
python crates/prism-recipe/harness/tests/smoke_local.py
python crates/prism-recipe/harness/tests/smoke_battery.py
```

Optional local hooks that mirror the cheap half of CI:

```bash
./scripts/install-githooks.sh   # commit-msg (conventional), pre-commit, pre-push
```

## 2. Version and PR gates (must pass before "ready")

```bash
cargo run -p xtask -- version                              # current version
cargo run -p xtask -- version verify-bump --base origin/main
cargo run -p xtask -- pr-check --body-file /tmp/pr-body.md  # paste your PR body first
```

## 3. Local subnet before staging, staging before prod

Challenges evaluate on **master only**. A validator never executes a
challenge; it fetches sealed weights and submits on-chain.

```bash
./deploy/scripts/materialize-env.sh                 # deploy/env/*.env, mode 0600
./deploy/scripts/local-e2e.sh --dry-run             # plan + compose render, no containers
./deploy/scripts/local-e2e.sh --smoke               # stack + healthz + weights seal smoke
./deploy/scripts/local-e2e.sh --live                # owner wallet + REQUIRE_OWNER=1
./deploy/scripts/local-e2e.sh --down                # teardown
./deploy/scripts/local-e2e.sh --help                # authoritative flag list
```

Host probes default to loopback `2808x` (gateway on `:8080`):

| Service | Probe |
|---------|-------|
| gateway | `http://127.0.0.1:8080/healthz` |
| sealed weights | `http://127.0.0.1:8080/v1/weights/latest` |
| validator | `http://127.0.0.1:28080/healthz` |
| bounty | `http://127.0.0.1:28096/health` |
| proof | `http://127.0.0.1:28100/health` |

Topology, prereqs, secrets layout, compose matrix, and the seal-smoke
semantics live in [`../deploy/AGENTS.md`](../deploy/AGENTS.md) §
*Local testnet E2E* and [`../deploy/README.md`](../deploy/README.md).

## 4. Challenge verification is a submission, not a healthz

`/healthz` proves nothing about a challenge. For any PR touching a challenge:

1. **Happy path** — POST a real intake through the challenge service on master.
2. **Bounty — pair + report** — `ctx bounty pair --hotkey <ss58>
   --account-id <id> --accept-terms`, then `POST /v1/pair` and
   `POST /v1/reports`. Operator bearer `POST /v1/admin/adjudicate`. Scoring
   **reads** CortexLM/backend public JSON (`BOUNTY_BACKEND_PUBLIC_URL`); do
   not serve `/v1/public/*` from this repo. Unset feed → **503** and nobody
   is paid. See [`contracts/BOUNTY.md`](contracts/BOUNTY.md).
3. **Proof — submit** — `POST /v1/submissions` with a `topic_id`. Missing /
   unknown / not-open → **400**. Empty `eval_image_digest`, zero open topics,
   or an unsealed baseline → **503**. See [`contracts/PROOF.md`](contracts/PROOF.md).
4. **Edges** — bad harness, sanitize reject, quota exhaustion, wrong route,
   wrong auth.
5. **Seal** — leaf emission → `POST /v1/weights/raw` → seal →
   `GET /v1/weights/latest` with **`sealed: true`**. The unsealed burn vector
   (uid 0 = 100%, `sealed: false`) is the fail-closed default, never proof of a
   seal.

**Never host Sim in staging or prod.** `PROOF_FORCE_SIM=1` is CI and local
opt-in only (`assert-compose-matrix.sh` fails if a droplet overlay sets one).
Live Proof rent requires a digest pin in `config/proof-pin.toml` plus miner
BYOK. Do not invent `eval_image_digest`.

## 5. Pre-prod spot checks (before promote or tag)

- Every image reference is digest-pinned (`repo@sha256:<64 hex>`); no
  `:latest` anywhere in a measured compose path.
- Gateway runs **only** on the owner host, under
  `docker compose --profile master`.
- `evil-gateway` is absent from staging and prod:
  `./deploy/scripts/assert-evil-gateway-not-default.sh`.
- `/v1/admin/*` answers **401/403** from the public Internet, and
  `BASE_GATEWAY_ADMIN_TOKEN_FILE` is set wherever
  `BASE_GATEWAY_REQUIRE_OWNER=1`.
- Trust roots verify under the owner key:

```bash
cargo run -q -p trustroot-bin -- verify \
  --owner-pub config/owner.pubkey --input config/challenges.toml --kind challenges
cargo run -q -p trustroot-bin -- verify \
  --owner-pub config/owner.pubkey --input config/measurements.toml --kind measurements
```

- `deploy/env/*.env` are mode `0600`; wallets and key files are `0400`, owner
  uid `65532`; the age identity is delivered out of band and never lives in
  Terraform state or cloud-init.
- No secrets in the diff. Expect empty:

```bash
git grep -nEi 'dop_v1_|secretPhrase|BEGIN [A-Z ]*PRIVATE KEY' -- '*.md' '*.toml' '*.yml'
```

- `pg_dump` taken before a production promote; rollback path known
  (`deploy/README.md` § Promotion pipeline).
