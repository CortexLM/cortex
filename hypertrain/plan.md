# Miner CLI, auto-update, CI, incentive audit - plan

Source directive: ../MINER-CLI-UPDATE.md (2026-10-12). Branch boat/3924dab79a27.

## Trust model (WS-A)
- Coldkey (mnemonic) stays on the operator's machine. It never reaches a Vast host.
- Operator runs `hypertrain-miner key create --coldkey <local> --server <name>` locally. This generates a fresh
  server keypair, and the coldkey signs a `ServerKeyGrant{run_id, coldkey, server_pub, server_name, scope, issued_at, key_id}`.
  Scope is `mine` only: commit/upload/heartbeat. It explicitly excludes escrow withdraw, rotate, and coldkey operations.
- Only the server secret + grant are copied to the host (`install` emits a one-shot bundle, mode 0600).
- Challenge server: `POST /v2/server-keys` (grant), `GET /v2/server-keys?coldkey=` (list),
  `POST /v2/server-keys/revoke` (coldkey-signed revoke). Revoked keys are rejected on every authenticated miner route.
  Lifetime: forever until revoked.
- The local registry `~/.hypertrain/keys.json` mirrors the server list for `key list`.

## Auto-update (WS-B)
- Only signed release manifests count. CI release job signs `UpdateManifest{version, tag, commit, wheel_sha256, announce_at, changeover_at}`
  with the release key (sr25519, pinned pubkey in the package). Unsigned tags, non-tag commits, and downgrades are rejected.
- Timeline: announce (manifest published) -> window >= 600 s -> changeover_at (absolute UTC, beacon-aligned).
  During the window, the miner downloads + verifies + stages the new venv while training continues.
  At changeover, it finishes/journals the current inner step at the next round boundary <= changeover_at,
  checkpoints, swaps the `current` symlink atomically, restarts, and resumes from the checkpoint.
  A miner still on the old version after changeover_at + grace (60 s) refuses new rounds, so no mixed-version rounds occur.
- Bitwise replay MATCH is required before and after the swap (same version = identical roots).

## CI (WS-C)
`.github/workflows/miner-flow.yml`: a fresh ubuntu container runs install.sh. The test covers key create/list/revoke against a
local challenge server, revoked-key rejection, a signed update with changeover 10 s ahead across 2 miners, a synchronized swap,
resume from checkpoint, and replay MATCH.

## Incentive audit (WS-D)
Review emission, escrow/vesting, burns, honest rewards, and penalties in ledger/, challenge/, and auditor/. Fix small gaps
with tests. Document the rest in the result file.

## Vast (WS-E)
Install + update flow on one vastai/pytorch host, <= USD 4, inet_down >= 1000, delete + confirm absence.

## Rules
ruff/mypy/pytest green, one commit per workstream, no secrets printed, English only.
