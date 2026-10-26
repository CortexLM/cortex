# Production run: what the owner and the coordinator sign

> **DEFERRED.** Contains known-wrong steps, listed in
> [review-prod-run-signing.md](review-prod-run-signing.md). Do not sign anything from this
> page.

Checklist for runbook steps 1-7 of a `ledger_mode=production` hypertrain run on the Cortex
master. It names every signature, the key, the tool, the inputs and their digests, the files the
signature touches and the check to run afterwards. `<…>` is a value to fill in. No key material
belongs here or in any command output.

Labels: **[V]** read in this tree (`path:line`), **[A]** reported in operator notes and not
re-checked, **[?]** unknown.

## Keys

| Key | Role in a production run | Where it must be | Pinned as |
|---|---|---|---|
| `K_owner` (owner seed) | Signs the `RunManifestV2` envelope, the qualification record receipts, the backend bootstrap receipt and, through `dispatch.json`, one receipt per dispatched operation. | Offline for S1. **Online on s100** for S3 and S7, see [Unknowns](#unknowns). [V] `scripts/ops/backend_bootstrap.py:66-75`, `gpu_ops/role_dispatch.py:111-120` | `HYPERTRAIN_OWNER_HOTKEY` of the challenge; signer of the manifest envelope. [V] `challenge/store.py:2256-2259` |
| `K_coord` (coordinator) | Signs `ProductionBootstrap` (twice), `relay.json` (registry + network manifest), round opens, finalizations and receipts; the server signs the `economic-bound` record with it. | Online: `HYPERTRAIN_COORD_KEY_FILE` in the challenge container. [V] `docs/operator.md:314` | `training.coord_pubkey` and `economics.reward_authority` (must be equal). [V] `ops/manifest_v2.py:95-96` |
| Auditor seeds (2) | Sign audit verdicts. Not founder material. | Auditor containers. [V] `scripts/ops/auditors.sh:17` | `training.auditors` (2 distinct). |
| Referee seed (≥ 1) | Signs dispute rulings. | Referee host. | `dispute.referees`. |
| Relay key `k1` (per relay) | Signs relay receipts. | Relay host `RELAY_SECRETS`. [V] `relay/app.py:232-237` | `relay_registry.specs[].pubkeys`. |

The miner `sign_release.py` key (auto-update manifests) plays no part in a run. [V]
`scripts/sign_release.py:1-41`

## Order

The runbook order is 1 → 2 → 3 → 4 → 5 → 6 → 7. The code forces one change. The CUDA qualification
record (step 5) must be installed **before** the run is POSTed: `backend_bootstrap.py` runs
"before run POST" and the store refuses it once the run exists (`backend qualification must
precede run services`). A production run with no qualification record is refused with 409
`production CUDA qualification required`. [V] `scripts/ops/backend_bootstrap.py:2`,
`challenge/store.py:2462-2472`, `:2517-2522`. So "the run_id is listed in
`/challenge/hypertrain/public/v1/runs`" is evidence for the end of step 5, not step 2.

| # | Runbook step | Signer | Online? |
|---|---|---|---|
| S0 | 1 Inputs (admission policy, economics, training, relays) | none | no |
| S1 | 2 Sign the manifest | K_owner | offline |
| S2 | 3 Relay registry entry for `relay.joinbase.ai` (in S0's `relays.json`) | none (digest pinned by S1) | no |
| S3 | 5 Qualification records + backend bootstrap, then POST the run | K_owner | online, at the store's current round |
| S4 | after the POST: production bootstrap | K_coord (×2) | online |
| S5 | after S4: economic bound | K_coord (server) | online |
| S6 | 3 `relay.json` for each production relay | K_coord | online or offline |
| S7 | 6 `dispatch.json` | none (names `owner_key_file`) | K_owner online |

Step 4 (image pin switch) has no run signature.

## S0 Inputs (no signature, but they fix the run_id)

`run_id = sha256(JCS(RunManifestV2 body))`: every input below changes it. The owner seed and the
expiry do not. [V] `protocol/messages_v2.py:243-249`

1. Start from `uv run --frozen python -m hypertrain.ops.manifest_v2 example --ledger-mode production --out <inputs>`.
   The example has placeholder public keys and a tiny model: replace them all. [V]
   `docs/operator.md:204`
2. `policies.json` `admission`: keep `work_screen_epoch_rounds` 6000 (see
   [network-v2.md](network-v2.md), admission). Every other admission field is a fixed literal.
3. `policies.json` `economics`: `ledger_mode` `production`, `genesis_allocation_hash` `null`,
   `reward_authority` = `<K_coord ss58>`, `round_reward_units` = `training.budget.epochs_per_round × 1_000_000`,
   and `bootstrap_allocation_hash` over the miners' **coldkeys**, which must exist first:

   ```bash
   # allocation.json: [["<coldkey ss58>", <units>, <mature_at round>], ...]
   uv run --no-sync python -I -c 'import json, sys
   from hypertrain.ledger.escrow_v2 import genesis_allocation_hash
   print(genesis_allocation_hash([tuple(a) for a in json.load(open(sys.argv[1]))]))' allocation.json
   ```

   [V] `ledger/escrow_v2.py:74-76`, `protocol/messages_v2.py:571-594`. The other economics
   values must pass the bound `q_floor·s_lower·beta·(R_collectible+S_min) ≥ G_max·10^18`, or S5
   fails. [V] `challenge/trust_v2.py:148-156`
4. `training.json`: `coord_pubkey` `<K_coord ss58>`, two `auditors`, the production dataset
   fragment, and `reference_spec` = GPU image `<sha256:…>`, `driver_allowlist`, `sm_count`, `layout`.
   The qualification hosts must match all four.
5. `policies.json` `dispute.referees`: `<referee ss58>`.
6. `relays.json` (`RelayRegistryV1`, unsigned, epoch 0): see [Relay registry](#relay-registry-relayjoinbaseai).

## S1 K_owner signs the manifest (runbook step 2)

```bash
uv run --frozen --extra trainer python -m hypertrain.ops.manifest_v2 build \
  --training <inputs>/training.json --policies <inputs>/policies.json \
  --relays <inputs>/relays.json --owner-seed <path to owner seed, 0600> \
  --exp-round <drand round after the POST> --state-dir <staging state dir> --out <out>
```

- Signature: one ht/2 envelope over `RunManifestV2` (type, run_id, body, `exp_drand`). [V]
  `ops/manifest_v2.py:126-131`, `protocol/envelope_v2.py:193-215`
- Files written in `<out>`: `manifest.json`, `manifest-envelope.json` (the only signed file),
  `policy-{admission,economics,aggregation,audit,dispute}.json`, `relay-registry.json`,
  `index.json` (`run_id`, `owner_ss58`, six object hashes, sha256 of the eight other files). With
  `--state-dir`, the six objects go under `<state>/objects/`. [V] `ops/manifest_v2.py:132-153`
- Refusals: `reward_authority` ≠ `coord_pubkey` (`PINNED_AUTHORITIES`), wrong `round_reward_units`
  (`MANIFEST_REWARD_BUDGET`), fewer than 2 auditors. [V] `ops/manifest_v2.py:89-98`

Verify after signing, from `hypertrain/`:

```bash
uv run --no-sync python -I - <out> <owner ss58> <<'EOF'
import hashlib
import json
import sys
from pathlib import Path

from hypertrain.protocol import envelope_v2
from hypertrain.protocol.messages_v2 import RunManifestV2

out, owner = Path(sys.argv[1]), sys.argv[2]
index = json.loads((out / "index.json").read_bytes())
for name, sha in index["files"].items():
    if hashlib.sha256((out / name).read_bytes()).hexdigest() != sha:
        sys.exit(f"{name}: sha256 differs from index.json")
raw = (out / "manifest-envelope.json").read_bytes()
env = envelope_v2.parse_envelope(raw)
manifest = RunManifestV2.model_validate(env.body)
if not envelope_v2.verify_envelope(raw) or env.type != "RunManifestV2":
    sys.exit("manifest envelope signature does not verify")
if env.signer != owner or index["owner_ss58"] != owner:
    sys.exit("manifest is not signed by the expected owner")
if not manifest.run_id() == env.run_id == index["run_id"]:
    sys.exit("run_id differs between manifest, envelope and index")
if manifest.body() != json.loads((out / "manifest.json").read_bytes()):
    sys.exit("manifest.json differs from the signed body")
print(index["run_id"], env.signer, env.exp_drand)
EOF
```

Record the printed `run_id`: S3-S7 all bind it.

## S3 Qualification and backend bootstrap (runbook step 5), then POST

1. Two-host CUDA qualification of **this** run_id on the pinned image. The owner seals each
   reviewed launch record: `scripts/prepare_record.py --action qualification --owner-seed <owner seed>
   --run-id <run_id> --beacon <round> --base <review.json> --file <name=/abs/path> --parent-pid <pid>
   --cutoff-unix <t> --output <record>`. It reads the seed as **32 raw bytes** (not hex). [V]
   `scripts/prepare_record.py:190`, `:227-243`
2. Inside the challenge container, before the POST:
   `python scripts/ops/backend_bootstrap.py --manifest-envelope <out>/manifest-envelope.json
   --owner-seed <owner seed> --config <h0 config.json> <h1 config.json> --result <h0 result.json> <h1 result.json>`.
   The owner seed must be `HYPERTRAIN_OWNER_HOTKEY`. It seals an owner `Receipt` with
   `received_round` = the store's **current** round, so it cannot be signed in advance. It writes
   the `qualification` record keyed by the image digest. [V] `scripts/ops/backend_bootstrap.py:34-82`,
   `challenge/store.py:2406-2415`, `:2474-2485`
3. Stage `<out>/policy-*.json` **and** `<out>/relay-registry.json` in the challenge object store,
   then POST `<out>/manifest-envelope.json` to `/v2/admin/runs` with the operator bearer.
   `launch_cuda_run.sh` copies only `policy-*.json` and would miss the registry. [V]
   `challenge/store.py:2276-2289`, `scripts/launch_cuda_run.sh:219-223`

Check: `GET /challenge/hypertrain/v2/runs/<run_id>` returns the envelope; the run's
`execution-backend` record says `cuda`; `hypertrain-miner status-v2` with `run_id` and
`owner_hotkey` pinned accepts the manifest signature. [V] `miner/core.py:771-784`

## S4 K_coord signs the production bootstrap (after the POST)

`ProductionBootstrap {run_id, operation_id, allocation_hash, total_units, origins[], authority_sig}`
with `allocation_hash` = S0's `bootstrap_allocation_hash` and origins that conserve
`total_units`. Two signatures by K_coord:

- `authority_sig` = K_coord signature over
  `b"hypertrain/origin/2|ProductionBootstrap|" + run_id + b"|" + body_digest(body without authority_sig)`;
- an ht/2 envelope whose signer is the challenge coordinator.

POST to `/v2/runs/<run_id>/admin/bootstrap` (operator bearer). Accepted once per run and
idempotent per `operation_id`. It needs the CUDA qualification and a BLS-verified beacon round.
[V] `protocol/messages_v2.py:737-753`, `ledger/escrow_v2.py:60-71`, `:364-385`,
`challenge/store.py:4148-4157`. **No committed CLI builds this envelope**: only
`tests/ledger/test_production_bootstrap_v2.py` and `tests/challenge/test_production_economics_v2.py`
construct one. A reviewed tool must exist before step 2 is usable.

## S5 Economic bound (no hand signature)

`POST /v2/runs/<run_id>/admin/economic-bound` (operator bearer, no body). The server checks the
bound and seals the immutable record with K_coord. Without it no production miner is ever
eligible. [V] `challenge/store.py:4159-4194`, `:2618-2627`

## S6 `relay.json` per production relay (runbook step 3)

`relay.json` is a `RelayConfig`: `run_id`, `master_public` (K_coord ss58), `master_url`, and two
K_coord envelopes for that run: the `RelayRegistryV1` (S1's `relay-registry.json`) and a
`RelayNetworkManifest {run_id, base_manifest_hash, registry_hash, specs_hash,
assignment_policy "master-observed-median3-v1"}`. Plus `relay_id`, `region`, `active_key`,
`signing_files`, `observer_public_keys` and the backing store. [V] `relay/app.py:49-65`,
`protocol/relay_messages.py:88-93`

`base_manifest_hash` must be `training_hash()` of the manifest, **not** the run_id: the master
builds its network manifest that way and the relay rejects any other. [V]
`challenge/store.py:8409-8416`, `relay/core.py:240`. `scripts/network_gpu_prepare.py:554` uses
the run_id and is not usable for production. The tools used for the TEST relays
(`seal_relay.py`, `build_relay_configs.py`) are not in this repository [A].

Check: the relay starts only if both envelopes are signed by `master_public` for `run_id`, the
registry digest and specs hash match, and neither envelope has expired. Then
`curl -fsS https://<relay host>/readyz` returns 200. [V] `relay/app.py:201-231`

## S7 `dispatch.json` (runbook step 6)

Not signed. It names the `owner_key_file` that the challenge uses online for each dispatched
operation, and authorizes exactly one run_id. Rewriting the production file to the new run_id
stops the role replays of every other run on that challenge, `7753ff75…` included. For od-base it
is also blocked by issue #25. See [role-dispatch.md](role-dispatch.md).

Round operation (runbook step 7) signs `RoundOpenV2` and `Finalize` with K_coord through
`scripts/ops/live_operator.py --coord-key`. [V] `scripts/ops/live_operator.py:223`, `:255`, `:264`

## Relay registry (`relay.joinbase.ai`)

The registry enters the run only by digest: `network.relay_registry_hash` = sha256(JCS(registry)),
part of the run_id. The registry itself is unsigned. [V] `protocol/messages_v2.py:227-228`,
`ops/manifest_v2.py:81`, `:102`

**Edit, in S0, before S1 signs.** Add this spec to `<inputs>/relays.json`, keep `epoch` 0 and
`previous_registry_hash` `null`:

```json
{"id": "relay-joinbase", "region": "<region>", "https_url": "https://relay.joinbase.ai",
 "pubkeys": [{"key_id": "k1", "pubkey": "<production relay k1 ss58>",
              "valid_from_round": 0, "valid_until_round": 1099511627776}],
 "max_object_bytes": 2147483648, "max_inflight_bytes": 8589934592,
 "codecs": ["ht-sparse-v1", "ht-dense-int8-v1"], "mode": "transport"}
```

The limits and codecs repeat the TEST registry `471aeedb…` that carried four uploads in joinbase-6
run 2 [A]. Consequences: `relay-registry.json`, `index.json`, `manifest.json`,
`network.relay_registry_hash`, the run_id and `manifest-envelope.json` all change. The
qualification (S3), `relay.json` (S6) and `dispatch.json` (S7) then bind that run_id.
`training_hash()` does not change. [V] `relay/core.py:162-164` (relay key must match the pinned pubkey)

Before choosing values:

- Miners do not choose a relay. The master assigns every miner the first non-draining relay with a
  valid key, sorted by `(region, id)`, and the miner does not fall back by itself. With `relay-de`
  (region `de`) in the same registry, `relay.joinbase.ai` in region `eu` is never the primary
  unless `relay-de` is drained (`POST /v2/runs/<run_id>/admin/relay-drain/relay-de`). [V]
  `challenge/store.py:8427-8462`, `miner/core.py:1299-1321`
- `https://relay.joinbase.ai` serves the TEST relay today (TEST coordinator, `master_url`
  `…/test-master`) [A]. A production relay there needs its own process, a new production `k1` key
  (its public ss58 goes in the spec above) and an S6 `relay.json` for the production run_id.
- `scripts/ops/manifest/od_large_manifest.py` accepts only `relay-de`, `relay-sg` and `relay-us`,
  and hard-codes `ledger_mode` test: build the production manifest with `manifest_v2 build`.
  [V] `scripts/ops/manifest/od_large_manifest.py:149-179`, `:355`
- After the run exists, the coordinator can still add the relay without a new run_id: a
  K_coord-signed successor registry (epoch 1, `previous_registry_hash` = current digest) posted to
  `/v2/runs/<run_id>/admin/relay-registry`, between rounds, with every relay's `relay.json`
  re-sealed. [V] `challenge/store.py:5237-5289`, `relay/core.py:231-244`

## Unknowns

- [?] Whether K_owner may live on s100. S3's backend receipt must be sealed at the store's current
  round, and S7 signs online. As written, the tools need the owner seed on the host.
- [?] The production coldkeys, units and maturity rounds for `bootstrap_allocation_hash`. The cohort
  is fixed when S1 signs.
- [?] Production values of `G_max`, `R_collectible`, `S_min`, `beta`, `s_lower`, `max_total_issuance`,
  `round_reward_units`. No od-base production template exists.
- [?] A reviewed tool for S4 (`ProductionBootstrap`) and for S6 (production `relay.json`).
- [?] The production GPU image digest, driver allowlist and `sm_count` the qualification hosts
  will match.
- [?] Which owner ss58 the production challenge pins (`HYPERTRAIN_OWNER_HOTKEY`) and whether its
  `coord.key` is the production coordinator.
- [?] The production relay `k1` key, region and host for `relay.joinbase.ai`.
- [?] Issue #25 for od-base dispatch (S7).
