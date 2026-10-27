# Role dispatch authorization (`dispatch.json`)

`dispatch.json` lets a challenge run the trusted references, audits and referee replays of
**one** run on an admitted GPU lifecycle host instead of in-process. It is the authorization of
runbook step 6. Nothing signs or hashes it: its integrity rests on file ownership and mode.

## How the challenge reads it

| Item | Behaviour | Source |
|---|---|---|
| Location | `HYPERTRAIN_ROLE_DISPATCH` (path). Unset: no dispatcher, every replay runs in-process. | `challenge/app.py:168`, `:329-333` |
| Schema | `DispatchConfig`, `extra="forbid"`: `tree`, `run_dir`, `owner_key_file`, `roles` (hotkey → owned host role), `run_id`. Nothing else, no `host` field. | `gpu_ops/role_dispatch.py:34-40` |
| Load | On first use. A missing or invalid file answers 503 `ROLE_DISPATCH_UNAVAILABLE` and is retried on the next call. A valid file stays cached: changing it needs a restart. | `challenge/app.py:282-295` |
| One run | A job of any other `run_id` raises `RuntimeError("role dispatch job is not the authorized run")` before any I/O. | `role_dispatch.py:96-97` |
| All hosted runs | The same dispatcher serves every run on that challenge, CPU runs included: only the listed run is served, every other run's replays fail. | `challenge/store.py:540-551` |
| Owner key | `owner_key_file` is used **online**: the challenge seals an owner `Receipt` for every dispatched operation. 0600, owned by the process uid (65532 in the image). | `miner/core.py:156-167`, `scripts/network_service_proof.py:1118-1162` |
| Workload gate | Every dispatched operation needs a `network_workload_promoted` journal record in `run_dir`. Only `NetworkRuntime.promote` writes it. Its callers are the signed decoder `continue` action and the CI harness, both on the decoder profile. So od-base / od-encoder runs are refused (`workload_not_promoted`) until the fix for CortexLM/hypertrain#25 lands in CortexLM/cortex (the hypertrain repository is frozen). | `experiments/gpu_network_v2/orchestrate.py:859-863`, `:872-904`, `:3667`; `scripts/genuine_cuda_ci.py:705` |
| Host files | On every dispatched call the challenge reads `tree/scripts/network_service_proof.py`, `tree/experiments/gpu_network_v2/orchestrate.py`, `run_dir/config.json`, the `network_profile_file` and the `key_file` (Vast API key) named in that config, and the owner seed. All of them must be readable as uid 65532 inside the challenge container. | `role_dispatch.py:98-120` |

`role-dispatch.json` is a different file: the per-publication provenance written next to each
dispatched result (`role_dispatch.py:55-79`) and checked by CUDA audits (`store.py:7337-7380`).

## When a run needs it

- **CPU-backend TEST run** (`ledger_mode=test`, no qualification record): never. Leave
  `HYPERTRAIN_ROLE_DISPATCH` unset; with it set, the run's admission references go through the
  dispatcher and fail.
- **CUDA run** (qualification record present): its audits need `role-dispatch.json`, which only
  the dispatcher writes in repo code. The joinbase-6 CUDA TEST run used the out-of-repo
  same-host harness (`serve_challenge.py`, dispatch unset) instead.
- **Any od-base run** (`training.model.arch` `od-encoder`), TEST or production: needs the file
  **and** the issue #25 fix (workload promotion for od-encoder, a host field). The file alone
  is not enough. `roles` already maps several hotkeys; only `launch_cuda_run.sh:245` rewrites it
  to one.

## The TEST run file

[`examples/dispatch.test.json`](examples/dispatch.test.json) authorizes TEST run
`16bc75dd475eaf7a6bcd85d8dbe89b9f57b31c8d080f252c6478ac52fcac3602` (joinbase-6 `manifest-run2`:
`ledger_mode=test`, admission policy `d0a5538d…` with `work_screen_epoch_rounds` 6000, relay
registry `471aeedb…` with `relay.joinbase.ai`). It was built and sealed and never created on a
challenge. Its `roles` keys are the six TEST miner hotkeys of that build (`miner0-hot` …
`miner5-hot` in `manifest-run2/public-keys.json`, the six `metagraph.json` hotkeys);
`manifest.json` itself names no miner.

It is an **od-base** run. Even with every placeholder resolved, each dispatched reference, audit
or referee reserves a workload execution and is refused `workload_not_promoted` until issue #25
is fixed (`orchestrate.py:1068`, `:859-863`). It also needs its own 2-host CUDA qualification
record on the test challenge. Without one its backend is `cpu`, and `launch_adapter` refuses
it (`orchestrate.py:1724-1729`).

The three paths and the role names are placeholders, and the file is **not installable** until
they name:

- `tree`: the reviewed frozen source tree inside the test challenge container
  (`scripts/ops/prepare_runtime.py freeze`);
- `run_dir`: an admitted lifecycle run dir (`config.json`, journal, SSH keys), created by an
  `admit` against the Vast account. Its `config.json` names the Vast API key file and the
  network profile, which the challenge reads on every dispatched call;
- `owner_key_file`: the TEST owner seed of that stack (`HYPERTRAIN_OWNER_HOTKEY` of
  `hypertrain-test-challenge`), never a production key;
- each role: the lifecycle host role that hotkey owns (`h0`, `h1`, …).

If the TEST manifest is rebuilt (another GPU image digest, other coldkeys, another policy), the
`run_id` changes. Recompute it from the new `manifest.json` and edit `run_id` and `roles`.

**Not for B300 hosts.** `16bc75dd…` pins the RTX 5090 reference:

- `reference_spec.image_digest` `sha256:f4f1663b…`, layout dp2 on 2 GPUs, `sm_count` 170, and a
  driver allowlist without 595.91.07.
- A trial screen qualifies only with the manifest's layout and image digest
  (`challenge/store.py:2586-2602`).
- A CUDA qualification needs every host driver in the allowlist and `sm_count` equal
  (`store.py:2439-2440`).
- The dispatcher drives only Vast lifecycle hosts: `run_dir/config.json` names a Vast `base_url`
  and key file (`role_dispatch.py:102-107`, `gpu_ops/provider.py:1`).

A test on other hardware therefore needs a new TEST manifest for that hardware and its own
qualification. With `HYPERTRAIN_ROLE_DISPATCH` unset, the challenge computes each trusted
reference in-process with the run's backend (`challenge/store.py:2976-2978`), so the challenge
host needs that hardware. Rented or listed machines are never servers (founder rule,
2026-10-26), so the challenge stays on the operator host. GPU references on rented hardware then
need a dispatch target that is not a Vast lifecycle host: the generic CUDA executor
(`HYPERTRAIN_CUDA_EXECUTOR`, [cuda-executor.md](cuda-executor.md)) is that target, over SSH and a
digest-pinned container, and is mutually exclusive with this file.

## Validate

Run the repo validator (`DispatchConfig`) and bind the file to the run's own build output
(`manifest_v2 build --out` directory). It refuses a non-test ledger and a `run_id` of another run.
From `hypertrain/`:

```bash
OUT=<manifest_v2 build --out directory of the TEST run>
uv run --no-sync python -I - "$OUT" docs/examples/dispatch.test.json <<'EOF'
import sys
from pathlib import Path

from hypertrain.gpu_ops.role_dispatch import DispatchConfig
from hypertrain.protocol.messages_v2 import EconomicsPolicyV2, RunManifestV2

out, dispatch = Path(sys.argv[1]), Path(sys.argv[2])
manifest = RunManifestV2.model_validate_json((out / "manifest.json").read_bytes())
economics = EconomicsPolicyV2.model_validate_json((out / "policy-economics.json").read_bytes())
config = DispatchConfig.model_validate_json(dispatch.read_bytes())
if economics.digest() != manifest.network.economics_policy_hash:
    sys.exit("policy-economics.json is not the manifest's economics policy")
if economics.ledger_mode != "test":
    sys.exit("not a TEST run: ledger_mode is " + economics.ledger_mode)
if config.run_id != manifest.run_id():
    sys.exit("dispatch run_id differs from the manifest run_id")
print(config.run_id, economics.ledger_mode, len(config.roles), "roles")
EOF
```

`tests/challenge/test_role_dispatch_lazy.py::test_test_run_dispatch_file_authorizes_only_its_test_run`
loads the file the way the challenge does: it refuses the production-challenge run `7753ff75…`
and the stuck TEST run `b14a975e…`, and stops the TEST run at the unresolved paths.

## Install on the `hypertrain-test` stack only (not run)

Preconditions: issue #25 is fixed (run `16bc75dd…` is od-base), the run has its CUDA
qualification record on the test challenge, the placeholders are resolved, the validation above
passes, the test challenge hosts no CPU-backend run, and the host change is approved. Never write
`/data/role-dispatch/dispatch.json` (the production file, bound to `7753ff75…`) and never restart
`cortex-challenge-hypertrain`. `scripts/launch_cuda_run.sh dispatch` writes that production file
and restarts that container: do not use it for a TEST run.

```sh
# s100, root. Container path of the test state volume (stop if /data/role-dispatch is mounted):
docker inspect hypertrain-test-challenge \
  --format '{{range .Mounts}}{{.Source}} -> {{.Destination}}{{"\n"}}{{end}}'
sha256sum /data/role-dispatch/dispatch.json      # production file, before
# dispatch.test.json: the resolved, validated copy, copied to s100 first
#   (scp <file> root@chain.joinbase.ai:/root/backups/dispatch.test.json)
install -d -o 65532 -g 65532 -m 0700 /data/hypertrain-test/state/role-dispatch
install -o 65532 -g 65532 -m 0600 /root/backups/dispatch.test.json \
  /data/hypertrain-test/state/role-dispatch/dispatch.json
cp -p /root/backups/hypertrain-test/compose.test.yml \
  /root/backups/hypertrain-test/compose.test.yml.<UTC timestamp>
# compose.test.yml, service `challenge`:
#   environment: HYPERTRAIN_ROLE_DISPATCH: <container path of /data/hypertrain-test/state>/role-dispatch/dispatch.json
#   volumes, unless already under the test state volume (uid 65532 readable):
#     <frozen tree>:<tree>:ro, <lifecycle run dir>:<run_dir>, <TEST owner seed>:<owner_key_file>:ro,
#     and the key_file and network_profile_file paths named in <run_dir>/config.json (path only)
docker compose -p hypertrain-test -f /root/backups/hypertrain-test/compose.test.yml up -d challenge
curl -fsS 127.0.0.1:18996/version
# the file loads lazily: check every path it names from inside the container now
docker exec hypertrain-test-challenge python -I -c 'import json, os, sys
from pathlib import Path
from hypertrain.gpu_ops.role_dispatch import DispatchConfig
c = DispatchConfig.model_validate_json(Path(os.environ["HYPERTRAIN_ROLE_DISPATCH"]).read_bytes())
config = c.run_dir / "config.json"
paths = [c.tree / "scripts/network_service_proof.py", c.owner_key_file, config,
         c.tree / "experiments/gpu_network_v2/orchestrate.py"]
if config.is_file():
    cfg = json.loads(config.read_bytes())
    paths += [Path(cfg["key_file"]), Path(cfg["network_profile_file"])]
missing = [str(p) for p in paths if not p.is_file()]
if missing:
    sys.exit("missing: " + " ".join(missing))
print(c.run_id)'
sha256sum /data/role-dispatch/dispatch.json      # production file, after: unchanged
```

Rollback: restore the compose backup, run the same `up -d challenge`, then remove
`/data/hypertrain-test/state/role-dispatch/dispatch.json`.

The compose path, project name, port and volume layout come from the joinbase-3/6 operator
reports. They were not read from the host for this page.
