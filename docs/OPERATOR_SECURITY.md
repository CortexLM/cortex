# Operator security checklist

Use this before every promote and after every incident. Architecture: [`ARCHITECTURE.md`](./ARCHITECTURE.md). Threat bounds: [`THREAT_MODEL.md`](./THREAT_MODEL.md).

---

## 1. Secrets and keys

- [ ] No secrets in git (`git log -p` / `git grep` clean of `dop_v1_`, `secretPhrase`, PEM private keys).
- [ ] Coldkeys and age identities are mode `600`; secret dirs mode `700`.
- [ ] Compose env files under `deploy/env/*.env` are mode `0600` after `materialize-env.sh`.
- [ ] Age identity delivered **out of band** to `/etc/base/age-identity.txt` (or `AGE_IDENTITY`). Never in terraform state or cloud-init user-data.
- [ ] Challenge signing secrets are **files** mounted into the challenge service, not env values (D11).
- [ ] Owner and challenge mini-secrets never committed; only `*.pubkey` / TOML bodies + detached `.sig` in git.
- [ ] Cloudflare / DO / Phala tokens live only in operator secret stores, not in docs or CI logs.
- [ ] Proof miner BYOK (`LIUM_API_KEY` / `X-Lium-Api-Key`) is never written to git, compose env committed files, or logs. Control-plane Lium mounts under `deploy/secrets/lium` are files, mode **0400**, uid **65532**.
- [ ] The eval-image InferenceOffer / `proxy_model` is the **RLM judge agent**, not a miner training proxy. Never commit judge credentials.
- [ ] Experimental Proof v2 uses a separate restricted `base_app` URL file, never an owner connection. Apply canonical migrations 0020–0028 with a separate owner; runtime events are append-only and update grants are column-scoped. Its public API accepts no Lium keys. Do not connect the legacy operator-key client as a v2 fallback; follow the [v2 integration boundary](runbooks/proof-autonomy-local.md).
- [ ] Private experiment/Atlas IPC sockets remain outside every agent workspace, with a canonical `0700` parent and `0600` socket. Never mount the private router on public HTTP.
- [ ] Atlas config/DB URL/pin/signing-key files are canonical owner-private regular files (`0600`, immediate parents `0700`, no links or unsafe ancestors). Raw 32-byte or hex signer bytes are read from the checked open file without reopening its path; the derived public key must match the pin's `topic_pubkey`.
- [ ] Headless model configuration and separate `apiKeyFile` stay host-only/private, outside runtime state/workspace/sandbox. Use HTTPS; permit literal `127.0.0.1` / `[::1]` HTTP only with deliberate `allowLoopbackHttp: true`. No Factory/Prime, environment-key or alternate-model fallback.
- [ ] Preserve original run UUID, absolute deadline, resume state and budget journal across recovery; never clear them to reset spending. Same-fence invocation is refused and the generic controller enforces the original DB deadline even if an agent ignores stop. Shutdown aborts lease acquisition and drops suspended operation/heartbeat futures before DB bookkeeping. Keep independent cleanup capacity and reconcile uncertain rents before declaring termination.
- [ ] Set `headless.pid_namespace` to the absolute `unshare` path on hosts that allow unprivileged user namespaces; without it, process-group supervision cannot stop a `setsid` escape (17 tests, including one documenting that limit). The namespace is PID containment only, not a mount/network/user sandbox.
- [ ] Do not activate v2 spending/publication from local test results. Real CPU execution and strict gateway transport exist, but independent metric/FLOPs observation and verified live Lium/credential/quote adapters do not. Lium request-id/expiry/cost/billing guarantees are unestablished, not disproved; `collect` remains `UnobservedMeasurements`, not successful science.
- [ ] Do not initialize stock W&B v0.28.0: automatic runtime/environment telemetry exceeds the seven-field public allowlist. No safe adapter is wired.
- [ ] Use the separate [Atlas operator contract](runbooks/proof-autonomy-local.md#separate-atlas-operator-contract), not `proof-challenge`, to opt into scheduling/publication. `--check` checks Rust-side files/signer/policy/restricted DB, not TS provider/model initialization, image presence, chain/gateway compatibility or deployment. Normal mode may spend inference and publish; it never rents, seals or submits chain weights.
- [ ] Keep the authorized live headless regression ignored in routine tests; it additionally requires explicit `CORTEX_TEST_HEADLESS_MODEL_CONFIG`. Its synthetic pass proves neither reproduction nor cost. Verify configured prices separately: the test's temporary 100 USD/million-token rates were accounting assumptions, not a tariff or measured charge. The passing [local validation checkpoint](runbooks/proof-autonomy-local.md#local-verification) is not deployment approval.
- [ ] Distinguish scoped shutdown coverage from deployment readiness: three Atlas startup/shutdown regressions passed, including SIGTERM during finality wait, alongside canonical DB migration/private-file tests. A separate cancellation regression prevents a late blocking RPC result from freezing a round after shutdown.

---

## 2. Images and compose

- [ ] Every image reference is digest-pinned (`repo@sha256:<64 hex>`). No `:latest`.
- [ ] Exactly one mount of `/var/run/docker.sock`: on `socket-proxy` (read-only).
- [ ] socket-proxy allowlist matches updater needs (`CONTAINERS`, `IMAGES`, `POST` as configured).
- [ ] Staging/prod never set `BASE_ALLOW_HOST_SIM` / `PROOF_FORCE_SIM=true` as a live scoring path (asserted by `assert-compose-matrix.sh` for droplet overlays).
- [ ] Proof live rent **and** live scoring require `config/proof-pin.toml` `eval_image_digest` starting with `sha256:`. No floating eval tags. Do not invent a digest. Empty digest stays fail-closed (`503`). Confirm `GET /challenge/proof/v1/status` reports `can_score` only after harvest is wired, a baseline is sealed, and ≥1 topic is open.
- [ ] Proof live harvest: miner BYOK `LIUM_API_KEY` present and never committed; `/v1/status` reports `live_harvest_wired` when harvest can rent. A contaminated or empty-evidence `manifest` must be rejected without renting. Details: [`docs/PROOF.md`](./PROOF.md).
- [ ] Gateway service uses compose profile **`master`** only on the owner host.
- [ ] Profile `evil-gateway` is **absent** from prod hosts. Spot-check:

```bash
./deploy/scripts/assert-evil-gateway-not-default.sh
```

---

## 3. Trust roots

- [ ] Validators load `config/challenges.toml` and `config/measurements.toml` from **local disk** only.
- [ ] Detached signatures verify under `config/owner.pubkey`:

```bash
cargo run -q -p trustroot-bin -- verify \
  --owner-pub config/owner.pubkey \
  --input config/challenges.toml --kind challenges

cargo run -q -p trustroot-bin -- verify \
  --owner-pub config/owner.pubkey \
  --input config/measurements.toml --kind measurements
```

- [ ] Rotation follows dual-accept (D21); never hot-push a new root without a signed release. See [`runbooks/trust-root-rotation.md`](./runbooks/trust-root-rotation.md).
- [ ] You understand R12: owner signs roots **and** runs the gateway.

---

## 4. Gateway and TLS

- [ ] Gateway hotkey equals on-chain `SubnetOwnerHotkey` (else process exits 2).
- [ ] `BASE_GATEWAY_ADMIN_TOKEN_FILE` (or `BASE_GATEWAY_ADMIN_TOKEN`) is set whenever `BASE_GATEWAY_REQUIRE_OWNER=1` — `/v1/admin/*` must not be open on a public listener.
- [ ] Spot-check from the public Internet: `GET /v1/admin/backends` → **401/403** (not 200). Localhost/VPC seal scripts still work with the bearer.
- [ ] TLS terminates **only** in the gateway process (D20). No second reverse proxy claiming TLS.
- [ ] `BASE_DOMAIN` is a real delegated zone when ACME is enabled (D25).
- [ ] Manual failover procedure is known: [`runbooks/gateway-failover.md`](./runbooks/gateway-failover.md). HA is **not** claimed (R9).
- [ ] Enable the strict Proof round receiver only by separate operator decision: `BASE_GATEWAY_PROOF_V2=1` plus `BASE_GATEWAY_PROOF_ANCHOR_BLOCK`. Activation is durable/sticky, blocks legacy Proof ingress and stale seals, and is not reversed by unsetting the flag. No deployment/live publication has been verified by the local tests.
- [ ] For v2, confirm the immutable signed full batch through `POST /v2/weights/proof/rounds` and `GET /v2/weights/proof/rounds/{round}`: exact receipt **and** identical read-back bytes are required. HTTP **409 is not success**. Publication does not seal; use only the existing explicit seal path, then verify `sealed: true` and validator recomputation.
- [ ] The ignored headless-delivery regression passed scheduler/Postgres → real runtime/Docker Python → private decision → strict HTTP gateway/readback → production `seal_epoch` → served router/independent Python vector `[0.4, 0.4, 0.2]`. Lost-ack recovery replays identical bytes without model rerun. Chain/science/inference are synthetic; this directly calls the seal helper and does **not** validate the public operator admin seal HTTP route.

---

## 5. Promote / backup

- [ ] `pg_dump` taken **before** every production promote.
- [ ] Rollback path tested or at least dry-run documented: [`runbooks/promote-rollback-restore.md`](./runbooks/promote-rollback-restore.md).
- [ ] Updater desired image is digest-pinned; health gate must pass or auto-rollback.
- [ ] Self-update of the updater is **operator one-shot**, never automatic in prod (D14).

---

## 6. Claims discipline (read aloud)

- [ ] Pitch / status updates use D19 wording; no "master cannot lie about scores" or "publicly auditable merkle on-chain".
- [ ] Merkle root is **not** in `WeightsTlockPayload`.
- [ ] Attestation does **not** prove env values (D11).
- [ ] Zero emission can still be a **pass** for extrinsic/reveal criteria (R4).

---

## 7. Quick hygiene commands

```bash
# Workspace gates (from repo root)
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo run -q -p xtask -- loc-cap
cargo run -q -p xtask -- consensus-lint
cargo run -q -p xtask -- spec-check
cargo run -q -p xtask -- design-check
cargo run -q -p xtask -- external-docs-check

# No accidental secret patterns in tracked docs (expect empty)
git grep -nEi 'dop_v1_|secretPhrase|BEGIN [A-Z ]*PRIVATE KEY' -- '*.md' || true
```
