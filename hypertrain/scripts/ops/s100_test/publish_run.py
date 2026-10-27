"""Publish a built TEST run on the s100 TEST challenge, from inside its container.

docker exec -i hypertrain-test-challenge python -I - STAGE STEP < publish_run.py
Optional DATA SECRETS API (default /data /run/secrets http://127.0.0.1:8000) for a rehearsal.
STAGE holds the public build outputs only: manifest-envelope.json, policy-*.json,
relay-registry.json, samples.u32, proofs.json, genesis-allocation.json (build_test_manifest.sh).
STEP: objects | run | dataset | genesis | status, in that order, as tests/challenge/
test_service_network_v2.py `network` does. The admin bearer and the coordinator key are read
by path from the container's /run/secrets and never printed. With no qualification record the
test ledger freezes the run to the cpu backend at POST (store._backend_v2).
"""

from __future__ import annotations

import json
import sqlite3
import sys
from pathlib import Path

import httpx

from hypertrain.data.store import LocalFSStore
from hypertrain.ledger.escrow_v2 import authority_message, escrow_id_key, genesis_origin_id
from hypertrain.miner.core import load_keyfile
from hypertrain.protocol import envelope_v2
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages_v2 import TestGenesis

stage, step = Path(sys.argv[1]), sys.argv[2]
# Container paths; a local rehearsal passes its own DATA SECRETS API.
data, secret, api = (
    sys.argv[3:6] + ["/data", "/run/secrets", "http://127.0.0.1:8000"][len(sys.argv[3:6]) :]
)
headers = {"Authorization": "Bearer " + Path(secret, "admin.token").read_text().strip()}
client = httpx.Client(base_url=api, timeout=600)
raw = (stage / "manifest-envelope.json").read_bytes()
run_id = envelope_v2.parse_envelope(raw).run_id
# A run POSTed while the root store already hosts one lives in its own child store.
child = Path(data, "runs", run_id)
store = (
    child if step not in ("objects", "run") and (child / "challenge.db").is_file() else Path(data)
)
objects = LocalFSStore(store / "objects")
out: dict[str, object] = {"run_id": run_id, "step": step}
if step == "objects":
    names = sorted(stage.glob("policy-*.json")) + [stage / "relay-registry.json"]
    out["objects"] = [objects.put(p.read_bytes()) for p in names]
elif step == "run":
    r = client.post("/v2/admin/runs", content=raw, headers=headers)
    out["create"] = [r.status_code, r.json()]
elif step == "dataset":
    hashes = {
        "samples_hash": objects.put((stage / "samples.u32").read_bytes()),
        "proofs_hash": objects.put((stage / "proofs.json").read_bytes()),
    }
    r = client.put(f"/v2/runs/{run_id}/admin/dataset", json=hashes, headers=headers)
    out["dataset"] = [r.status_code, r.text[:300], hashes]
elif step == "genesis":
    coord = load_keyfile(Path(secret, "coord.key"))
    allocation = json.loads((stage / "genesis-allocation.json").read_bytes())
    alloc_hash = json.loads((stage / "policy-economics.json").read_bytes())[
        "genesis_allocation_hash"
    ]
    origins = [
        {
            "origin_id": genesis_origin_id(run_id, alloc_hash, i, escrow_id_key(coord._secret)),
            "owner": o,
            "units": u,
            "mature_at": m,
        }
        for i, (o, u, m) in enumerate(allocation)
    ]
    body = {
        "run_id": run_id,
        "allocation_hash": alloc_hash,
        "total_units": sum(u for _, u, _ in allocation),
        "origins": origins,
    }
    unsigned = TestGenesis.model_construct(**body, authority_sig="0" * 128)
    genesis = TestGenesis.model_validate(
        {**body, "authority_sig": coord.sign(authority_message(unsigned)).hex()}
    )
    now = client.get(f"/v2/runs/{run_id}").json()["now_round"]
    sealed = canonicalize(envelope_v2.seal(coord, "TestGenesis", run_id, genesis, now + 1000))
    r = client.post(f"/v2/runs/{run_id}/admin/genesis", content=sealed, headers=headers)
    out["genesis"] = [r.status_code, r.text[:300]]
    out["origins"] = [o["origin_id"] for o in origins]
elif step == "status":
    db = sqlite3.connect((store / "challenge.db").as_uri() + "?mode=ro", uri=True)
    out["runs"] = db.execute("SELECT run_id, status FROM runs").fetchall()
    out["records"] = db.execute(
        "SELECT kind, id, substr(data, 1, 160) FROM records_v2 WHERE run_id=? AND kind IN "
        "('execution-backend', 'qualification', 'dataset') ORDER BY kind",
        (run_id,),
    ).fetchall()
    out["view"] = client.get(f"/v2/runs/{run_id}").json().get("status")
else:
    raise SystemExit("STEP: objects | run | dataset | genesis | status")
print(json.dumps(out))
