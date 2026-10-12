# Miner CLI, auto-update, CI, incentive audit - result

Branch `boat/3924dab79a27` on `echobt/hypertrain`. Directive: `../MINER-CLI-UPDATE.md`. Design: `plan.md`.

## Commits
| Commit | Workstream | Local evidence |
|---|---|---|
| `1e350c2` | Vast miner-flow profile (1x RTX 5090, `vastai/pytorch@sha256:4507...`, inet_down>=1000) | `tests/test_vast_e2e.py` 9 passed; real-API dry run picked offer 55180264 (1114 Mbps), worst case $0.55 |
| `90deb91` | Per-server coldkey-granted revocable keys (create/list/revoke; 403 when revoked) | `tests/miner/test_server_keys.py` 4 passed |
| `0b84b1b` | Signed-release auto-update: announce, >=600 s window, swap at the round boundary, `must_halt` after +60 s | `tests/miner/test_update.py` 21 passed |
| `514f9ef` | CLI v3 (`key`, `install`, `register`, `status`, `update`, `run --auto-update`), `install-miner.sh`, ledger vesting fix, audit doc | `tests/ledger` 113 passed; full `scripts/check.sh` NOT run on this commit (pushed on request, without verification) |
| `1789bed` | CI `miner-flow.yml` (CPU, fresh `ubuntu:24.04` container, no Vast); `gpu-vast.yml` manual only | `scripts/miner_flow_e2e.py` exit 0, 19/19 checks; `tests/e2e/test_miner_flow.py` 1 passed |

ruff + mypy (`--package hypertrain`, 117 files) were clean after the keys/update/cli/ledger lanes.

## Trust model
The coldkey/mnemonic stays on the operator machine. `key create` makes a fresh server hotkey, and the coldkey signs a `mine`-scope grant, valid forever until revoked. Only `{server.key, grant.json}` (0600) is copied to the GPU host. The e2e asserts that no coldkey bytes exist anywhere under the server HOME. `key revoke` is coldkey-signed; afterwards the challenge returns 403 for that hotkey.

## Update semantics
Signed `UpdateManifest` (pinned release pubkey; only vX.Y.Z tags; no downgrades). announce -> download + verify sha256 + stage, while training continues -> at the first round boundary >= `changeover_at`: checkpoint, atomic `current` symlink swap, restart, resume from the checkpoint. A process not on the target version after `changeover_at + 60 s` refuses new rounds. E2E: two miners swap at the same round (2), at or after changeover, and every round before and after the swap replays bitwise **MATCH**.

## Incentive audit
See `docs/incentive-audit.md` (14 mechanisms). Fixed: ledger vesting now uses the f32 value of q, like the protocol (test added). Documented, not fixed (`store.py` is hash-pinned and `app.py` routing is out of scope):
- **Deregistered hotkeys:** units dropped at answer time burn instead of staying owed.
- **Equal emission share per run:** every run gets the same share, and there is no cross-run 65,536-hotkey cap.
- **No production clawback:** `Ledger.clawback` has no production caller.
- **Forgiven TRANSIENT:** it still burns its round slice.
- **Stale ponytail comment:** the comment at `store.py:1756` is stale.

## Honest gaps
- **Full gate not run after the last commits:** `scripts/check.sh` has not run in full since the last commits. The whole suite is slow (>25 min on this shared host).
- **Vast run not executed:** the user said Vast is not needed for CI. The harness and remote script are ready (`--profile miner-flow`, max $3.90). No instance was created; the account showed 0 instances.
- **GitHub Actions run not observed:** `miner-flow.yml` was proven by running its script locally. I have no access to GitHub Actions results (no `gh` CLI).
- **Simulated release in the e2e:** the e2e uses an injected wheel fetch/install. Real `release.yml` signing needs the `HYPERTRAIN_RELEASE_KEY` secret, and the production release pubkey must be pinned before the first real release.
- **Exposed token:** the `origin` remote URL embeds a GitHub token. Rotate it.
