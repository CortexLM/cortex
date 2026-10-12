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

## Vast run (manual, outside CI)
`scripts/vast_e2e.py --profile miner-flow --max-usd 3.5`: offer 52078469 (Denmark, 1x RTX 5090, driver 590.48.01, inet_down 1263.5 Mbps, worst case $0.43), image `vastai/pytorch@sha256:4507...`. `uv sync`, `hypertrain-miner --help`, `scripts/miner_flow_e2e.py` on the host: **PASS, 19/19 checks, replay MATCH**. Harness result: `remote_rc=0, deleted=true, absent=true`. Credit afterwards $8.58 (from $8.78); instances on the account after the run: 1.

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
- **Local gate (`check.sh` with the ci.yml ignore/deselect args):** ruff and mypy are clean. pytest shows failures that are environmental and pre-existing; none come from this work:
  - **Missing SSH server on this host:** 16 failures and 18 errors in `tests/gpu_ops` (`test_gpu_ops`, `test_network_role_dispatch`, `test_network_lifecycle`, `test_phase_b`, `test_network_admit_cli`) raise `FileNotFoundError: /usr/sbin/sshd`. Unmodified CortexLM/main fails the same way here (10 failed + 18 errors on the same files). GitHub `ubuntu-24.04` runners ship sshd.
  - **Capacity tests time out here:** the 3 capacity tests time out as a normal user on a shared host. CI already deselects them from `check.sh` and runs them in its isolated root systemd job.
  - **Everything else passes:** e2e/datasets 40 passed and challenge 171 passed when run in isolation.
- **GitHub Actions run not observed:** `miner-flow.yml` was proven by running its script locally. I have no access to GitHub Actions results (no `gh` CLI).
- **Simulated release in the e2e:** the e2e uses an injected wheel fetch/install. Real `release.yml` signing needs the `HYPERTRAIN_RELEASE_KEY` secret, and the production release pubkey must be pinned before the first real release.
- **Exposed token:** the `origin` remote URL embeds a GitHub token. Rotate it.
