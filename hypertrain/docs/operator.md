# Operator guide

This guide covers running hypertrain yourself: first locally, then on a Cortex master, then for a training run. Every block tagged `doctest` is run by `bash scripts/doctest_docs.sh` from the tree root against the local stack, in order, in one shell. Blocks tagged `doctest-skip:prod` need a real Cortex master. The script lists them but doesn't run them, and they were checked by reading them against the pinned Cortex docs (Cortex `deploy/README.md` and `docs/CHALLENGES.md`).

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
10. [Hosting data on R2](#10-hosting-data-on-r2)

## 1. Local setup

You need Python 3.12 through [uv](https://docs.astral.sh/uv/), `curl`, `jq`, `openssl`, and for the image steps podman (to build) plus docker (to run the canary, see section 2). Always run commands from the tree root with `uv run --frozen`, so the lock file is never rewritten.

```bash doctest
uv sync --frozen --all-extras
```

The training data is a registry mix (`src/hypertrain/datasets/registry.json`, for example `od-a-proxy-v1`), tokenized with the pinned tokenizer into u16 shards with a Merkle manifest. A contract-2 run needs three steps. `build` reads the pinned sources you already fetched into `cache/<source id>/` plus `cache/tokenizer.json`; it never downloads. `pin` re-verifies the shards and writes the `samples` and `proofs` objects the service slices for every trial. It also prints the `training.dataset` fragment for the manifest; `--holdout-dir` adds `holdout_commit` from an existing holdout shard set. `register` copies both objects into the run's store under `CHALLENGE_STATE_DIR/runs/<run_id>/objects` and links them with `PUT /v2/runs/{run_id}/admin/dataset`. So it runs on the service host as the service's uid, after the run is created. The first run of a fresh state dir lives in the root store and is refused.

```bash doctest
uv run --frozen python -m hypertrain.datasets pin --help >/dev/null
uv run --frozen python -m hypertrain.datasets register --help >/dev/null
```

```bash doctest-skip:prod
uv run --frozen python -m hypertrain.datasets build --mix od-a-proxy-v1 --out data --cache cache
uv run --frozen python -m hypertrain.datasets pin --data-dir data --out-objects pinned \
  --holdout-dir data/holdout | tee pin.json
uv run --frozen python -m hypertrain.datasets register --api "$API" --run-id "$RUN" \
  --admin-token-file /secrets/admin.token --objects pinned --state-dir "$CHALLENGE_STATE_DIR" \
  --samples-hash "$(jq -r .samples_hash pin.json)" --proofs-hash "$(jq -r .proofs_hash pin.json)"
```

`python -m hypertrain.datasets verify data` re-hashes a shard set offline and prints `OK`. `pin` and `verify` exit 1 on any shard, unit or `merkle_root` mismatch, and `register` exits 1 on any 4xx/5xx and prints the service's answer. `proofs` holds `n_samples` paths of `depth` hashes (about `n_samples * depth * 67` bytes). Auditors and miners must hold the same bytes, since replay is bitwise.

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
| `HYPERTRAIN_COORD_KEY_FILE` | unset | K_coord seed (32 raw bytes or 64 hex) |
| `HYPERTRAIN_OWNER_HOTKEY` | unset | ss58 of K_owner, the only key that may sign a RunManifest |
| `HYPERTRAIN_EPOCH_SECONDS` | `4320` | Cortex epoch length |
| `HYPERTRAIN_EPOCHS_PER_ROUND` | `1` | must equal the manifest `budget.epochs_per_round` |
| `HYPERTRAIN_VEST_ROUNDS` | `10` (`ceil(1/q)` for q = 0.1) | must equal the manifest `verify.E_vest_rounds` |
| `HYPERTRAIN_GENESIS_UNIX` | drand quicknet genesis | clock origin |
| `HYPERTRAIN_RELAY_LIFECYCLE_INTERVAL` | `120` | seconds between the passes that release relay custody and extend it under an open dispute; `0` turns them off |

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

The server refuses v2 operations once it is more than two rounds (6 s) behind, so keep `uv run --frozen python -m hypertrain.beacon push --api "$API" --admin-token-file "$HT/secrets/admin_token"` running next to it. Every `--interval` (3 s) it asks the relays (`--relay URL`, repeatable, at least two distinct ones, all must agree) for the latest round, verifies it, and pushes the rounds the server is missing, at most the last `--catch-up` (20): the latest round first, so the freshness gate reopens as soon as it is in, then one older missing round per tick, the oldest. The older rounds still missing are kept in memory (a pusher restarted before it has refilled them leaves a hole), and a round older than the last `--catch-up` rounds when the gap is first seen is never pushed. It prints one JSON line per tick, `{"pushed": [...], "latest": n, "known": n, "pending": n}`; `--once` runs a single tick, for cron, and pushes every missing round in it (the latest first, then the older ones oldest first), since a new process keeps no list. Relay, network, 408, 429 and 5xx failures are retried; exit codes: 0 after a good `--once` tick or on SIGTERM; 1 on any other service status (a 409 means a conflicting round, never overwritten, or a ledger integrity stop, in which case the round itself is stored: read the server log), a failed `--once` tick or `--max-failures` (10) failures in a row; 2 on a usage error, a 401/403, or a token file that is not one `[A-Za-z0-9._~-]+` token. The same push is the clock that closes disputes nobody can decide (see `docs/network-v2.md`): keep the pusher running, and expect the first push after an outage to close every expired dispute.

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

Parity thresholds and the final tuned values: the full grid finished (150{ev:experiments/results/summary.json#full_scale_status.jobs_done} jobs, 1.1M-parameter proxy), but no cell reached parity with data-parallel training, so the provisional values above are still not validated for a paid run. See [parity.md section 5](parity.md#5-results-of-the-full-grid).

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

### Contract-2 manifest CLI

Create editable CPU test-mode inputs, then build the owner-signed manifest and stage its six dependency objects before starting the service:

```bash
uv run --frozen python -m hypertrain.ops.manifest_v2 example --out /tmp/ex
uv run --frozen python -m hypertrain.ops.manifest_v2 build \
  --training /tmp/ex/training.json --policies /tmp/ex/policies.json \
  --relays /tmp/ex/relays.json --owner-seed /private/owner.seed \
  --exp-round 1000000 --state-dir /tmp/state --out /tmp/out
```

The example's `coord_pubkey`, two auditors, referees, `reward_authority` and relay public key are **placeholder SS58 public identities, not operational keys**. Replace them before a real run; `coord_pubkey` must match the service's K_coord. The example uses a tiny decoder, `ledger_mode="test"`, a pinned genesis allocation hash and one `.test` relay. It creates no dataset, genesis funding or training round. `example --ledger-mode production` instead writes a production economics policy pinning a placeholder bootstrap allocation (`bootstrap_allocation_hash`, no genesis); funding then uses `POST /v2/runs/{run_id}/admin/bootstrap` and `POST /v2/runs/{run_id}/admin/economic-bound` (see [network-v2.md](network-v2.md)). A test-ledger run hosted on the production challenge steers real emission once its first live reward matures; keep test-mode runs off the production challenge unless that is intended. Custom CPU/GPU runs use the same three JSON shapes; provide valid model dimensions and parameter count, an audit budget of `2H`, and the H/step-bound-derived audit lease. Build computes `init_state_hash`; an existing value must match. It also rejects economics whose `reward_authority` differs from `training.coord_pubkey` (`PINNED_AUTHORITIES`) or whose `round_reward_units` differs from `training.budget.epochs_per_round * 1_000_000` (`MANIFEST_REWARD_BUDGET`). The service's configured auditor set must match `training.auditors`; the offline CLI cannot validate that configuration. Building needs the existing `trainer` extra (`uv sync --frozen --extra trainer`); help and example do not.

The owner seed is an existing private, current-user-owned 0600 file containing 32 raw bytes or 64 hex characters; it is read through the miner's checked loader, never printed or copied. The shared loader strips leading/trailing ASCII whitespace bytes: provide hex for any raw seed whose first or last byte is ASCII whitespace. Choose an expiry drand round later than submission. Both commands require a new or empty output directory. Build writes `manifest.json`, `manifest-envelope.json`, five `policy-<name>.json` files, `relay-registry.json` and `index.json`. The index records `run_id`, `owner_ss58`, six canonical object hashes and hashes of the eight other files; it excludes its own hash.

`--state-dir` is optional; when supplied, the six objects are stored under `/tmp/state/objects/<sha[:2]>/<sha>`. Start the service with `CHALLENGE_STATE_DIR=/tmp/state`, the matching K_coord and `HYPERTRAIN_OWNER_HOTKEY` set to `index.json`'s owner. Submit `manifest-envelope.json` to `POST /v2/admin/runs` with the admin bearer after the service has a valid beacon round. The CLI makes no HTTP requests. Success prints JSON; exit codes: 0 success, 2 input/validation error, 1 unexpected error.

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

An auditor whose lease expires without a verdict is refused with `403 auditor cooldown until round N` on `/worker/lease` (and in the relay observer role) for one lease, for two leases from its second strike, and a strike is forgotten after one quiet lease. The last configured auditor that is not in a cooldown is never refused, so a manifest with two auditors always keeps one; a configured auditor that never polls still counts as available. `python -m hypertrain.auditor` exits with code 2 on that 403 and on any HTTP error: restart it with a delay (the cooldown ends at round N). A `503 LEAF_OBJECT_LOST: round W job J` on `/worker/lease` is the master's own fault, not the auditor's and not the miner's: the master lost the leaf object of that job, parks it and retires round W at the next beacon push (once every earlier round is closed), and nobody is put in a cooldown or sanctioned; the auditor stops with code 2 like on any HTTP error, and restarting it is served the next queued job, if there is one. The loss is in the master's own object store: put the object back (the same bytes) before that push and the job is queued again and the round goes on; after it, the round is lost for all its members. The run's `leaf-object-lost` record names the object. An auditor that did ask for the execution (`/execute`, even if the master failed or the lease ran out meanwhile) is not put in a cooldown. One that asked, got a `MISMATCH` stored by the master and never signed it for the third time in a run (and each time after) is refused for two absolute windows (`4 * lease_rounds`) instead: sign the stored verdict and post it within 20 rounds (60 s) of the `/execute` answer, or the lease is taken back at once, without waiting for its expiry, and the job goes to the other auditor, which is never the same one twice, in a re-audit window of its own that starts at that round and lasts `reaudit_rounds` of the audit policy (one lease when it is absent) without outliving the miner's commit; if the second auditor does not decide the miner in it, that member alone is left out of the round as an `INFRASTRUCTURE` exclusion and nobody is sanctioned; the others keep their round (ADR 0011 Part 9).  When `/execute` fails on the master side (HTTP 500) the attempt is retired at once and the job goes to the other auditor; if the same error class already failed another job within one lease (a host-wide outage), the lease is held to its expiry instead, so an outage cannot burn the attempts of every job in seconds. A job that no auditor could finish in its window gets one more window of up to two leases that never outlasts the miner's sealed commit (an honest miner seals it until `d_final`, so a job still unfinished at `d_final` gets none); if there is none or that fails too, its miner is left out of that round (`excluded`, reason `INFRASTRUCTURE`, no sanction) and the round settles without it. Nothing needs an operator, except to restore a leaf object the master lost.

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

These steps follow the Cortex deploy guide. None of them run in the doc-test.

### Secrets

The supervisor runs the container as uid 65532 with `/run/secrets` read-only. Create the secrets dir and the tokens on the master:

```bash doctest-skip:prod
dir="$BASE_CHALLENGE_SECRETS_HOST_DIR/hypertrain"
install -d -m 0700 -o 65532 -g 65532 "$dir"
(umask 077; openssl rand -hex 32 >"$dir/internal.token"; openssl rand -hex 32 >"$dir/admin.token")
(umask 077; openssl rand -hex 32 >"$dir/worker.token"; openssl rand -hex 32 >"$dir/coord.key")
chown 65532:65532 "$dir"/*.token "$dir/coord.key"
```

`internal.token` is the master's bearer for `get_weights`. `admin.token` is yours. `worker.token` goes to auditors. `coord.key` is K_coord, which signs receipts, RoundOpen and Forfeit messages inside hypertrain; its ss58 goes in the manifest `coord_pubkey`. Cortex needs no key for hypertrain: it scores through `get_weights` and the gateway is the single source of truth.

### Registry row and burn-in

Append [deploy/registry-row.toml](../deploy/registry-row.toml) to `deploy/challenges/registry.toml` in the Cortex checkout, with `HYPERTRAIN_OWNER_HOTKEY` set to K_owner's ss58. The supervisor pulls the image, checks labels and build provenance, runs a secretless canary, starts `cortex-challenge-hypertrain`, and rolls back when the new digest doesn't answer `/version`. Start or restart the master role:

```bash doctest-skip:prod
docker compose --project-directory . --env-file deploy/env/master.env \
  -f deploy/compose/role-master.yml --profile master up -d
curl -fsS https://<master>/challenge/hypertrain/version
```

A registered id that the challenge profile doesn't list runs without emission. That's the burn-in: check `/challenge/hypertrain/version`, `/health` and the logs, launch a small run, and let at least one round finalize before you add the profile row.

### Challenge profile activation

Add [deploy/challenge-profile-row.example.toml](../deploy/challenge-profile-row.example.toml) to `config/challenges.toml` in the Cortex checkout, with every share rebalanced so the total is exactly 10000 bps. Bump `version` (a third challenge needs `challenges_version >= 3`) and leave `introduced_epoch` at or below the current epoch: a future value stops the master from sealing older epochs and the gateway from decoding stored seals, which burns the emission. There is no signing step: the gateway is the source of truth, the master reads this file and the gateway serves it at `GET /v1/trust`, which validators fetch over HTTPS. Restart the master, then check the gateway's `/v1/trust` lists hypertrain, a new `sealed: true` latest response carries hypertrain weights, and an independent validator recomputation matches. Hypertrain returns `full_share_mass = 10^6` per epoch, so any unverified or unvested mass burns rather than going to other miners.

### Rollback

- Image: the supervisor already rolls back a digest that fails `/version`. To hold a known-good image, set `pin = "sha256:..."` in the registry row.
- Challenge profile: restoring an older `version` is rejected by the gateway and the validator watermark. Publish a higher-version profile instead, for example one without the hypertrain row.
- Retire: publish a higher profile version without the hypertrain row first, then remove the registry row. Deleting the row stops the container and keeps its volume.
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
- Parity thresholds and tuned hyperparameters: the proxy grid finished without any cell reaching parity, so no values are validated; see [parity.md section 5](parity.md#5-results-of-the-full-grid).

## 10. Hosting data on R2

Shard sets, `manifest.json`, `build_record.json` and teacher-label caches are published to Cloudflare R2 with `python -m hypertrain.datasets publish <dir> --to r2` (and `publish-labels`). Run `python -m hypertrain.storage doctor` first. Setup, token scope, CORS and the full flow: [r2.md](r2.md).
