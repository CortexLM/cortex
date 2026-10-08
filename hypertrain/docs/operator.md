# Operator guide

This guide covers running hypertrain yourself: first locally, then on a Cortex master, then for a training run. Every block tagged `doctest` is run by `bash scripts/doctest_docs.sh` from the tree root against the local stack, in order, in one shell. Blocks tagged `doctest-skip:prod` need a real Cortex master. The script lists them but doesn't run them, and they were checked by reading them against the pinned Cortex docs (cortex@d738424: `deploy/README.md`, `docs/how-to/trust-root.md`, `docs/CHALLENGES.md`).

Contents:

1. [Local setup](#1-local-setup)
2. [Image and canary](#2-image-and-canary)
3. [A local challenge server](#3-a-local-challenge-server)
4. [Launching a run](#4-launching-a-run)
5. [Auditors and honeypots](#5-auditors-and-honeypots)
6. [Aggregation and relays per region](#6-aggregation-and-relays-per-region)
7. [Production on a Cortex master](#7-production-on-a-cortex-master)
8. [Budget and teardown](#8-budget-and-teardown)
9. [Known limits](#9-known-limits)

## 1. Local setup

You need Python 3.12 through [uv](https://docs.astral.sh/uv/), `curl`, `jq`, `openssl`, and for the image steps podman (to build) plus docker (to run the canary, see section 2). Always run commands from the tree root with `uv run --frozen`, so the lock file is never rewritten.

```bash doctest
uv sync --frozen --all-extras
```

The training data is one pinned FineWeb-Edu parquet (ODC-By 1.0, revision `87f09149`), packed into byte-level u32 shards with a Merkle manifest under `data/`. The first run of `uv run --frozen python experiments/fetch_data.py` downloads and builds it (about 770 MB). After that, re-hash it offline with:

```bash doctest
uv run --frozen python experiments/fetch_data.py --verify-only
```

It prints `merkle_root=8bd23b4f...`, `n_samples=196608` and `VERIFY OK`. Auditors and miners must hold the same bytes, since replay is bitwise.

The protocol pieces each ship a QA script that runs the real code on small inputs. They are the quickest check that a checkout is sound:

```bash doctest
uv run --frozen python tests/ledger/qa_scenarios.py >/dev/null
uv run --frozen python tests/aggregator/qa_scenarios.py >/dev/null
uv run --frozen --all-extras python tests/auditor/qa_scenarios.py >/dev/null
```

## 2. Image and canary

The server image is built with podman. The canary uses docker on purpose: it starts the container with the Cortex supervisor's exact flags, and podman 5 rejects `--tmpfs /data:uid=65532,gid=65532`. So build with podman and load the image into docker:

```bash doctest
podman build -q -f docker/Dockerfile.server -t hypertrain:local .
podman save hypertrain:local | docker load -q
docker tag localhost/hypertrain:local hypertrain:local
ENGINE=docker scripts/verify_image.sh hypertrain:local
```

`verify_image.sh` mirrors the supervisor's label check: slug `hypertrain`, contract `1`, source `https://github.com/CortexLM/hypertrain`, user `65532:65532`, port 8000.

The canary then checks `/version` without secrets, `/health` 503 without secrets, a read-only root, `/data` writable by uid 65532, and `get_weights` 200 with the internal token and 401 with a wrong one. Pick a free host port:

```bash doctest
HOST_PORT=18011 scripts/canary.sh
```

The GPU miner image is `docker/Dockerfile.gpu` (`podman build -f docker/Dockerfile.gpu -t hypertrain-gpu:local .`). Its entry point is `hypertrain-miner`. CI publishes both images as `ghcr.io/cortexlm/hypertrain` and `ghcr.io/cortexlm/hypertrain-gpu`.

## 3. A local challenge server

The server is a FastAPI app, `hypertrain.challenge.app:app`. It reads its settings from the environment:

| Variable | Default | Meaning |
| --- | --- | --- |
| `CHALLENGE_SLUG` | `hypertrain` | challenge id |
| `CHALLENGE_STATE_DIR` | `/data` | SQLite state and object store |
| `CHALLENGE_MASTER_URL` | `http://cortex-master:8080` | for `/v1/metagraph/latest` |
| `CHALLENGE_INTERNAL_TOKEN_FILE` | unset | master bearer for `get_weights` |
| `CHALLENGE_ADMIN_TOKEN_FILE` | unset | operator bearer for `/v1/admin/*` and `/v1/aggregator/*` |
| `CHALLENGE_WORKER_TOKEN_FILE` | unset | auditor bearer for `/v1/worker/*` |
| `HYPERTRAIN_COORD_KEY_FILE` | unset | K_coord seed (32 raw bytes or 64 hex), never the Cortex leaf seed |
| `HYPERTRAIN_OWNER_HOTKEY` | unset | ss58 of K_owner, the only key that may sign a RunManifest |
| `HYPERTRAIN_EPOCH_SECONDS` | `4320` | Cortex epoch length |
| `HYPERTRAIN_EPOCHS_PER_ROUND` | `1` | must equal the manifest `budget.epochs_per_round` |
| `HYPERTRAIN_VEST_ROUNDS` | `10` (`ceil(1/q)` for q = 0.1) | must equal the manifest `verify.E_vest_rounds` |
| `HYPERTRAIN_GENESIS_UNIX` | drand quicknet genesis | clock origin |

Tokens are read per request, so a missing file just means 401 or 503 on the routes that need it. `/health` is 200 only when the database is writable, the internal token is readable and K_coord loads.

This block starts a server on port 18081 with fresh secrets in a temp dir. The `EXIT` trap stops it and removes the temp dir when the doc-test shell ends. The K_coord seed here is `00..1f`, because the example manifest in section 4 pins that public key; a real run generates its own K_coord and puts the matching ss58 in its manifest.

```bash doctest
export HT=$(mktemp -d)
trap 'kill $(cat "$HT/pid" 2>/dev/null) 2>/dev/null; rm -rf "$HT"' EXIT
mkdir -m 0700 "$HT/secrets" "$HT/data"
(umask 077
 openssl rand -hex 32 >"$HT/secrets/internal_token"
 openssl rand -hex 32 >"$HT/secrets/admin_token"
 openssl rand -hex 32 >"$HT/secrets/worker_token"
 openssl rand -hex 32 >"$HT/secrets/owner.seed"
 uv run --frozen python -c 'print(bytes(range(32)).hex())' >"$HT/secrets/coord.key")
export HYPERTRAIN_OWNER_HOTKEY=$(uv run --frozen python -c '
import sys
from hypertrain.protocol.keys import Keypair
print(Keypair(bytes.fromhex(open(sys.argv[1]).read().strip())).ss58)' "$HT/secrets/owner.seed")
CHALLENGE_STATE_DIR="$HT/data" \
CHALLENGE_INTERNAL_TOKEN_FILE="$HT/secrets/internal_token" \
CHALLENGE_ADMIN_TOKEN_FILE="$HT/secrets/admin_token" \
CHALLENGE_WORKER_TOKEN_FILE="$HT/secrets/worker_token" \
HYPERTRAIN_COORD_KEY_FILE="$HT/secrets/coord.key" \
  uv run --frozen uvicorn hypertrain.challenge.app:app --host 127.0.0.1 --port 18081 \
  >"$HT/server.log" 2>&1 & echo $! >"$HT/pid"
API=http://127.0.0.1:18081
for _ in $(seq 60); do curl -fs "$API/version" >/dev/null && break; sleep 1; done
curl -fsS "$API/version" | jq -e '.slug == "hypertrain" and .contract == 1'
curl -fsS "$API/health" | jq -e '.ok'
```

## 4. Launching a run

A run goes through five admin calls, all with `Authorization: Bearer <admin token>`:

1. push a drand quicknet round (`POST /v1/admin/beacon`), since every deadline is counted in drand rounds;
2. create the run (`POST /v1/admin/runs`) with a RunManifest envelope signed by K_owner;
3. configure it (`PUT .../config`);
4. admit miners (`PUT .../roster/{hotkey}`);
5. unpause it (`PUT .../paused`).

The beacon payload is the relay's JSON as is. The server checks the BLS signature against the quicknet key, so you can't push a made-up round:

```bash doctest
ADMIN="Authorization: Bearer $(cat "$HT/secrets/admin_token")"
curl -fsS https://api.drand.sh/52db9ba70e0cc0f6eaf7803dd07447a1f5477735fd3f661792ba94600c84e971/public/latest \
  | curl -fsS -H "$ADMIN" -H 'Content-Type: application/json' --data-binary @- "$API/v1/admin/beacon" \
  | jq -e '.round > 0'
uv run --frozen python -m hypertrain.beacon check --live
```

`beacon check --live` fetches and verifies the latest round from the default relays; pass `--relay URL` (repeatable) to use your own.

### The manifest

`hypertrain.protocol.example.example_manifest()` builds the example CPU manifest. Its `PROVENANCE` dict says where each hyperparameter comes from in the plan and its advisories:

| Value | Source |
| --- | --- |
| `verify.q_base = 0.1` | ultrabrain section 3a default audit rate |
| `E_vest_rounds = probation_rounds = S_min multiple = ceil(1/q) = 10` | ultrabrain section 4 |
| `forgive_per_epoch = 1` | ultrabrain section 4 (verification synthesis section 6) |
| timeouts 20/40/300/900/600 drand rounds (3 s each) | ultrabrain section 1.2 |
| `inner.H = 30`, `J = 5` (7 leaves per round) | plan todo 16 Phase B round shape, ultrabrain section 1.1 |
| `outer` Nesterov `lr = 0.4`, `momentum = 0.9`, center `prev_outer_update` | parity section 2 starting grid, ultrabrain section 3c |
| honeypot rate 0.03 | ultrabrain section 4 (1 to 5%) |
| vocab 259 | plan todo 4 byte-level tokenizer |
| everything listed as `UNSOURCED (provisional, todo 13)` | not sourced yet; to be tuned by todo 13 |

Inner optimizer state: the todo 7 experiment chose arm A0, carry the inner AdamW state across rounds (`state_policy = "carry"`, no re-warmup). The reset, partial reset and re-warmup arms (A1, A2, A3) all failed the gate in `experiments/results/decision.json`. The example manifest now says `carry` with no re-warmup. The Phase A and Phase B GPU profiles (`experiments/gpu_phase_a/phase_a.json`, `experiments/gpu_phase_b/phase_b.json`) pin `reset` explicitly, because they run a single round from the public start where no previous state exists and the pin keeps their run ids unchanged.

Parity thresholds and the final tuned values: **not available (todo 13 CENSORED)**. The full CPU grid stopped with 0{ev:experiments/results/summary.json#full_scale_status.jobs_done} of 150{ev:experiments/results/summary.json#full_scale_status.jobs_censored} jobs finished, so no tuned inner or outer values and no measured parity gap exist. Don't treat the provisional values above as validated for a paid run. See [parity.md section 5](parity.md#5-results-of-the-cpu-grid).

Print the example with its provenance:

```bash doctest
uv run --frozen python -c '
import json
from hypertrain.protocol.example import example_document
print(json.dumps(example_document(), indent=2))' >"$HT/example.json"
jq -e '.provenance["verify.q_base=0.1"] and .manifest.verify.E_vest_rounds == 10' "$HT/example.json"
```

The server accepts a manifest only if `run_id` is `sha256(JCS(body))`, `coord_pubkey` is this server's K_coord, and `budget.epochs_per_round`, `verify.E_vest_rounds` and `verify.forgive_per_epoch` match its ledger settings. Only one run per state dir. Seal it with K_owner (here the temp owner seed; in production this happens offline on the owner's machine):

```bash doctest
uv run --frozen python -c '
import json, sys
from hypertrain.protocol.envelope import seal
from hypertrain.protocol.example import example_manifest
from hypertrain.protocol.keys import Keypair
owner = Keypair(bytes.fromhex(open(sys.argv[1]).read().strip()))
m = example_manifest()
print(json.dumps(seal(owner, "RunManifest", m.run_id(), m, 2**40)))' "$HT/secrets/owner.seed" >"$HT/manifest.json"
RUN=$(jq -r .run_id "$HT/manifest.json")
curl -fsS -H "$ADMIN" -H 'Content-Type: application/json' --data-binary @"$HT/manifest.json" \
  "$API/v1/admin/runs" | jq -e '.status == "created"'
```

### Configure, admit, unpause, pause

`train_rounds` is the number of drand rounds between `d_assign` and `d_commit` (the training window). `final_after_upload` is how many rounds after the later of `d_audit` and `d_upload` a round may finalize.

```bash doctest
curl -fsS -X PUT -H "$ADMIN" -H 'Content-Type: application/json' \
  -d '{"train_rounds": 10, "final_after_upload": 5}' "$API/v1/admin/runs/$RUN/config" | jq -c .
```

Admission is a roster row. A miner sends you the signed request printed by `hypertrain-miner join` (see the [miner guide](miner.md#3-admission-probation-and-bond)); you check the signature and hardware, then admit. Roster fields:

- `bond` (bool): the miner's escrow has reached S_min. Until then, and always during the first `probation_rounds` after admission, it's audited at q = 1;
- `probation` (bool): force q = 1, verify before apply;
- `cluster`, `region` (strings, up to 64 chars): miners sharing a cluster go to q = 1 together after any member is blacklisted;
- `flags` (up to 8): `outlier` or `copy` force q = 1.

```bash doctest
MINER=$(uv run --frozen python -c 'from hypertrain.protocol.keys import Keypair; print(Keypair(b"\x41"*32).ss58)')
curl -fsS -X PUT -H "$ADMIN" -H 'Content-Type: application/json' \
  -d '{"bond": false, "probation": true, "cluster": "dc-eu-1", "region": "eu", "flags": []}' \
  "$API/v1/admin/runs/$RUN/roster/$MINER" | jq -c .
```

Remove a miner with `DELETE .../roster/{hotkey}`. Unpausing needs a config and a pushed beacon round; it opens round 0. Pausing stops new rounds and keeps all state:

```bash doctest
curl -fsS -X PUT -H "$ADMIN" -H 'Content-Type: application/json' -d '{"paused": false}' \
  "$API/v1/admin/runs/$RUN/paused" | jq -e '.status == "running"'
curl -fsS "$API/v1/runs/$RUN" | jq -e '.status == "running"'
curl -fsS "$API/v1/runs/$RUN/rounds/0" | jq -e '.round_open.body.w == 0'
curl -fsS -X PUT -H "$ADMIN" -H 'Content-Type: application/json' -d '{"paused": true}' \
  "$API/v1/admin/runs/$RUN/paused" | jq -e '.status == "paused"'
```

Public read routes, no auth: `GET /v1/runs`, `/v1/beacon/latest`, `/v1/runs/{id}`, `/v1/runs/{id}/rounds/{w}`, `/run/{id}/auditor-stats`. The full list with bodies and status codes is in [challenge-routes.md](challenge-routes.md).

## 5. Auditors and honeypots

Auditors replay sampled training segments bit for bit and post signed verdicts. Each auditor hotkey must be listed in the manifest `auditors`. The worker leases jobs for one run with the worker token:

```bash doctest
uv run --frozen --all-extras python -m hypertrain.auditor --help | grep -q -- '--token-file'
```

```bash doctest-skip:prod
uv run --frozen --extra trainer python -m hypertrain.auditor \
  --api https://master.example/challenge/hypertrain --run-id "$RUN" \
  --token-file /secrets/worker.token --keyfile /secrets/auditor.key \
  --data-dir data --device cuda --image-digest "sha256:<reference image digest>"
```

Run auditors on the reference hardware and image of `reference_spec` (RTX 5090, 170 SMs). The Phase A test (todo 12) showed 6 honest runs bitwise identical across 3 RTX 5090 hosts with drivers 580.105.08, 580.178.04 and 595.84, and the negative control diverged at leaf 11; it cost USD 0.63. Run at least two auditors so a dispute can go to one other than the first. `--once` settles one job and exits, which is handy for cron or tests. Audit outcomes per epoch, including honeypot catch and false-positive rates, are at `GET /run/{id}/auditor-stats`.

Honeypots are operator-run miners that cheat on purpose (`fabricate`, `last_step` or `noise`) or play honest, to measure whether auditors catch them. You commit to the set before the round with a salted hash and reveal it afterwards, so auditors can't tell which miners are honeypots:

```bash doctest
uv run --frozen python -c '
import json, secrets, sys
from hypertrain.auditor.honeypot import Honeypot, commit
salt = secrets.token_bytes(32)
pots = [Honeypot(sys.argv[1], "fabricate")]
print(json.dumps({"commitment": commit(pots, salt), "salt": salt.hex(),
                  "members": [{"hotkey": p.hotkey, "mode": p.mode} for p in pots]}))' "$MINER" >"$HT/pot.json"
jq -c '{commitment}' "$HT/pot.json" | curl -fsS -X PUT -H "$ADMIN" -H 'Content-Type: application/json' \
  --data-binary @- "$API/v1/admin/runs/$RUN/honeypot" | jq -c .
jq -c . "$HT/pot.json" | curl -fsS -H "$ADMIN" -H 'Content-Type: application/json' \
  --data-binary @- "$API/v1/admin/runs/$RUN/honeypot/reveal" | jq -e '.verified'
```

Keep the salt secret until the reveal. Rates only count revealed commitments.

## 6. Aggregation and relays per region

The aggregator is the operator's outer step: for each round it reads the uploaded deltas (`GET /v1/aggregator/runs/{id}/rounds/{w}/inputs`), applies preclip, CenteredClip and Nesterov in float32 with fixed order (`hypertrain.aggregator.core`), and posts a K_coord-signed RoundOpen for the next round (`POST .../aggregate`). It also publishes the round start state (`PUT .../state`), rolls back a round with an open dispute (`POST .../rollback`) and finalizes rounds in order (`POST .../finalize`). Every merge writes a signed event tape that replays to the same bits. Anyone can check a final checkpoint:

```bash doctest
uv run --frozen hypertrain verify-checkpoint --help | grep -q -- '--signer'
```

```bash doctest-skip:prod
uv run --frozen hypertrain verify-checkpoint /path/to/final-checkpoint --signer "<K_coord ss58>"
```

It exits 0 on `VERIFIED` and 1 on `FAIL`.

Relays per region: what is built is the budget line and the beacon side. The manifest's `operator_budget.relay` records how many relay instances you run and what they cost per round, and every component takes its own drand relay list (`--relay URL` on `beacon check`). A dedicated regional delta relay process (forwarding and compressing deltas between islands in one region and the aggregator) is not part of this tree. Miners upload straight to the challenge server today. Put relays in the regions where your miners are, list them in the manifest, and keep at least two drand relays reachable from each region.

## 7. Production on a Cortex master

These steps follow the pinned Cortex deploy guide. None of them run in the doc-test.

### Secrets

The supervisor runs the container as uid 65532 with `/run/secrets` read-only. Create the secrets dir and the tokens on the master:

```bash doctest-skip:prod
dir="$BASE_CHALLENGE_SECRETS_HOST_DIR/hypertrain"
install -d -m 0700 -o 65532 -g 65532 "$dir"
(umask 077; openssl rand -hex 32 >"$dir/internal.token"; openssl rand -hex 32 >"$dir/admin.token")
(umask 077; openssl rand -hex 32 >"$dir/worker.token"; openssl rand -hex 32 >"$dir/coord.key")
chown 65532:65532 "$dir"/*.token "$dir/coord.key"
```

`internal.token` is the master's bearer for `get_weights`. `admin.token` is yours. `worker.token` goes to auditors. `coord.key` is K_coord, which signs receipts, RoundOpen and Forfeit messages; its ss58 goes in the manifest `coord_pubkey`. It is not the Cortex leaf key.

### Leaf key

The leaf key signs the challenge's weight leaves for the trust root. Generate it offline; `keygen` refuses to overwrite a file:

```bash doctest-skip:prod
cortex keygen \
  --seed-out /private/cortex-ceremony/hypertrain.seed \
  --public-out /private/cortex-ceremony/hypertrain.pubkey
```

Install the seed on the master at `deploy/secrets/master/hypertrain.key` as a private regular file. The master fails closed if it's missing, group-readable, a symlink, or doesn't match the signed public key.

### Registry row and burn-in

Append [deploy/registry-row.toml](../deploy/registry-row.toml) to `deploy/challenges/registry.toml` in the Cortex checkout, with `HYPERTRAIN_OWNER_HOTKEY` set to K_owner's ss58. The supervisor pulls the image, checks labels and build provenance, runs a secretless canary, starts `cortex-challenge-hypertrain`, and rolls back when the new digest doesn't answer `/version`. Start or restart the master role:

```bash doctest-skip:prod
docker compose --project-directory . --env-file deploy/env/master.env \
  -f deploy/compose/role-master.yml --profile master up -d
curl -fsS https://<master>/challenge/hypertrain/version
```

A registered id that the trust root doesn't list runs without emission. That's the burn-in: check `/challenge/hypertrain/version`, `/health` and the logs, launch a small run, and let at least one round finalize before you sign.

### Trust root activation (owner-signed, version 3 or higher)

Add [deploy/trust-root-v3-row.example.toml](../deploy/trust-root-v3-row.example.toml) to the owner's next challenges document (version 3 or higher, algorithm 3), with `public_key` set to the leaf public key and every share rebalanced so the total is exactly 10000 bps. Raise `version`, set `introduced_epoch`, then sign and verify offline:

```bash doctest-skip:prod
cortex trust-sign \
  --kind challenges \
  --input config/challenges.toml \
  --seed-file /private/cortex-ceremony/owner.seed \
  --signature-out config/challenges.toml.sig
cortex trust-verify \
  --challenges config/challenges.toml \
  --measurements config/measurements.toml \
  --owner-public config/owner.pubkey \
  --gateway-public "$(tr -d '\n' </private/cortex-ceremony/gateway.pubkey)" \
  --epoch 0
```

Drain and pause as the trust-root guide says, install the signed documents and version pins on the master and validators, resume, and check a new `sealed: true` latest response with algorithm 3 leaves and an independent validator recomputation. Hypertrain returns `full_share_mass = 10^6` per epoch, so any unverified or unvested mass burns rather than going to other miners.

### Rollback

- Image: the supervisor already rolls back a digest that fails `/version`. To hold a known-good image, set `pin = "sha256:..."` in the registry row.
- Trust root: restoring an older signed file is rejected by version watermarks. Stop submission and sign a higher-version recovery profile through the same ceremony, for example one without the hypertrain row.
- Retire: sign a higher version without the hypertrain row first, then remove the registry row. Deleting the row stops the container and keeps its volume.
- A training round: `POST /v1/aggregator/runs/{id}/rounds/{w}/rollback` before finalize. After finality, fraud is handled with money only (burn and clawback), never by subtracting a delta.

## 8. Budget and teardown

The manifest's `operator_budget` lists relay, auditor and honeypot instances, GPUs per instance and micro-USD per round, so miners can see what audit coverage costs. GPU rentals for tests go through `hypertrain.gpu_ops.launcher`. It refuses provider writes unless the base URL is loopback (the mock) or `--live` is passed, admits a host only when the worst-case cost (price times the hard deadline, plus storage and egress) fits the cap, keeps a resumable journal, copies every output before it deletes a host, and has a separate deadline supervisor:

```bash doctest
uv run --frozen python -m hypertrain.gpu_ops.launcher run --help | grep -q -- '--run-dir'
```

```bash doctest-skip:prod
uv run --frozen python -m hypertrain.gpu_ops.launcher run \
  --config experiments/gpu_phase_a/live-config.json --run-dir runs/phase-a --live
```

Exit codes: 0 ok, 2 admission rejected, 3 failed but cleaned up, 4 a rental may still be live (go check the provider), 5 refused. Teardown of a run: pause it, let the last rounds finalize and vest, then retire as in section 7. The local server needs nothing more than killing the process and deleting its state dir, which the `EXIT` trap above does.

## 9. Known limits

- The miner client trains single-GPU layouts only (`n_gpus == 1`). Multi-GPU islands exist in `hypertrain.trainer.island` but aren't wired into the client.
- There's no public join route in contract v1: `hypertrain-miner join` prints a signed request and you admit with `PUT /roster`.
- Bond is an operator flag. The server doesn't compute S_min from escrow by itself.
- Disputes are bisected at step level only.
- The canary needs docker, since podman rejects `--tmpfs ...:uid=`.
- The journal anchor is optional.
- Parity thresholds and tuned hyperparameters: not available, the CPU grid was CENSORED (0 jobs finished); see [parity.md section 5](parity.md#5-results-of-the-cpu-grid).
