"""Build the owner-signed genuine CUDA index from real RTX 5090 executions (runs on the GPU host).

Every result file referenced by the index is produced by CUDA execution during the same run:
two-role carry qualification (network_qualification, rescued by the reviewed launcher), then
the accepted network generated here on a CUDA-backed store (graduation references, round-0
island jobs, audits, dispute) through the product HTTP API. The index is signed with the
deterministic test OWNER key of tests/challenge/test_service_network_v2.py.

  python scripts/genuine_cuda_index.py index OUT --qual-dir Q --seed SEED --d2-dir D2
  python scripts/genuine_cuda_index.py generate OUT --prelim OUT/index-d2.json
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import os
import sys
from pathlib import Path

TREE = Path(__file__).resolve().parents[1]
IMAGE = "sha256:c16a28dd29df300eca0cfb5c5aeacb8947681708634e3cbc8639b530321fe2e2"


def load(name: str, rel: str):
    spec = importlib.util.spec_from_file_location(name, TREE / rel)
    assert spec and spec.loader
    mod = importlib.util.module_from_spec(spec)
    sys.modules[name] = mod
    spec.loader.exec_module(mod)
    return mod


def service():
    return sys.modules.get("service_network_fixture") or load(
        "service_network_fixture", "tests/challenge/test_service_network_v2.py"
    )


def fsha(path: Path) -> str:
    from hypertrain.gpu_ops.journal import fsha as f

    return f(path)


def sign_index(out: Path, record: dict, beacon_first: int, exp: int, name: str) -> Path:
    from hypertrain.protocol import envelope_v2
    from hypertrain.protocol.hashing import sha256_hex
    from hypertrain.protocol.jcs import canonicalize
    from hypertrain.protocol.messages import Receipt

    s = service()
    receipt = envelope_v2.seal(
        s.OWNER,
        "Receipt",
        record["run_id"],
        Receipt(w=0, commit_hash=sha256_hex(canonicalize(record)), received_round=beacon_first),
        exp,
    )
    path = out / name
    path.write_bytes(canonicalize({"record": record, "receipt": receipt}))
    s.genuine_index(path)
    return path


def add(files: dict, out: Path, key: str, path: Path) -> str:
    rel = path.resolve().relative_to(out.resolve())
    files[key] = {"path": str(rel), "sha256": fsha(path)}
    return key


def write(out: Path, rel: str, data: bytes) -> Path:
    path = out / rel
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)
    return path


def fixture_beacon_bytes(number: int) -> bytes:
    from hypertrain.protocol.jcs import canonicalize

    return canonicalize(service().fixture_beacon(number))


def bootstrap_files(out: Path, seed: Path, qual: Path, run_id: str, beacon: int) -> dict:
    import numpy as np

    from hypertrain.auditor.replay import AnchorCache  # noqa: F401  (trainer registry)
    from hypertrain.data.store import LocalFSStore
    from hypertrain.protocol import envelope_v2
    from hypertrain.protocol.hashing import MerkleTree, sha256_hex
    from hypertrain.protocol.jcs import canonicalize
    from hypertrain.protocol.messages import Receipt
    from hypertrain.protocol.messages_v2 import RunManifestV2

    s = service()
    files: dict = {}
    manifest = RunManifestV2.model_validate_json((seed / "manifest.json").read_bytes())
    assert manifest.run_id() == run_id
    add(
        files, out, "manifest", write(out, "b/manifest.json", (seed / "manifest.json").read_bytes())
    )
    for key, name in (
        ("manifest_envelope", "manifest-envelope.json"),
        ("genesis_envelope", "genesis-envelope.json"),
    ):
        add(files, out, key, write(out, "b/" + name, (seed / name).read_bytes()))
    add(files, out, "admin_token", write(out, "b/admin-token", b"integration-admin"))
    add(files, out, "coord_key", write(out, "b/coord.seed", bytes(range(32))))
    add(files, out, "bootstrap_beacon", write(out, "b/beacon.json", fixture_beacon_bytes(beacon)))
    rows = np.frombuffer((seed / "samples.u32").read_bytes(), dtype="<u4").reshape(-1, 17)
    tree = MerkleTree([row.tobytes() for row in rows])
    assert tree.root.hex() == manifest.training.dataset.merkle_root
    objects = LocalFSStore(out / "b/objects")
    samples_hash = objects.put(rows.tobytes())
    proofs_hash = objects.put(
        canonicalize([[p.hex() for p in tree.proof(i)] for i in range(len(rows))])
    )
    add(
        files,
        out,
        "dataset_intake",
        write(
            out,
            "b/dataset.json",
            json.dumps({"samples_hash": samples_hash, "proofs_hash": proofs_hash}).encode(),
        ),
    )
    for path in sorted((seed / "objects").rglob("*")):
        if path.is_file():
            objects.put(path.read_bytes())
    for path in sorted((out / "b/objects").rglob("*")):
        if path.is_file():
            add(files, out, "object:" + sha256_hex(path.read_bytes()), path)
    configs, results = [], []
    for role in ("h0", "h1"):
        source = next((qual / role).rglob("result.json")).parent
        for path in sorted(source.rglob("*")):
            if path.is_file():
                rel = path.relative_to(source)
                copy = write(out, f"q/{role}/{rel}", path.read_bytes())
                add(files, out, f"q:{role}:{rel}", copy)
        configs.append(f"q:{role}:config.json")
        results.append(f"q:{role}:result.json")
    ref = manifest.training.reference_spec
    evidence = {
        "run_id": run_id,
        "backend": "cuda",
        "reference_hash": sha256_hex(canonicalize(ref.model_dump(mode="json"))),
        "layout_hash": ref.layout.model_dump_json(),
        "configs": [files[c]["sha256"] for c in configs],
        "results": [files[r]["sha256"] for r in results],
        "summaries": [
            files[f"q:{role}:round-{i}/published/rank-{rank}/summary.json"]["sha256"]
            for role in ("h0", "h1")
            for i in range(2)
            for rank in range(ref.layout.n_gpus)
        ],
    }
    raw = envelope_v2.seal(
        s.OWNER,
        "Receipt",
        run_id,
        Receipt(w=0, commit_hash=sha256_hex(canonicalize(evidence)), received_round=beacon),
        beacon + 100000,
    )
    add(files, out, "backend_receipt", write(out, "b/backend-receipt.json", canonicalize(raw)))
    return {"files": files, "configs": configs, "results": results}


def cmd_index(a) -> None:
    from hypertrain.protocol.messages_v2 import RunManifestV2

    out = a.out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    run_id = RunManifestV2.model_validate_json((a.seed / "manifest.json").read_bytes()).run_id()
    base = bootstrap_files(out, a.seed, a.qual_dir, run_id, a.beacon)
    files = base["files"]
    for key, name in (
        ("lifecycle_config", "lifecycle-config.json"),
        ("runtime_profile", "runtime-profile.json"),
        ("runtime_evidence", "runtime-evidence.json"),
        ("d2_reservation", "d2-reservation.json"),
    ):
        add(files, out, key, write(out, "d2/" + name, (a.d2_dir / name).read_bytes()))
    record = {
        "schema": "ht-genuine-cuda-context/1",
        "run_id": run_id,
        "beacon": a.beacon,
        "files": files,
        "configs": base["configs"],
        "results": base["results"],
        "snapshot": {},
        "cases": {},
        "lifecycle": str(a.lifecycle.resolve()),
        "d2_reservation": "d2_reservation",
    }
    print(sign_index(out, record, a.beacon, a.beacon + 100000, a.name))


def main(argv: list[str] | None = None) -> int:
    p = argparse.ArgumentParser()
    sub = p.add_subparsers(dest="cmd", required=True)
    i = sub.add_parser("index")
    i.add_argument("out", type=Path)
    i.add_argument("--seed", type=Path, required=True)
    i.add_argument("--qual-dir", type=Path, required=True)
    i.add_argument("--d2-dir", type=Path, required=True)
    i.add_argument("--lifecycle", type=Path, required=True)
    i.add_argument("--beacon", type=int, required=True)
    i.add_argument("--name", default="index-d2.json")
    g = sub.add_parser("generate")
    g.add_argument("out", type=Path)
    g.add_argument("--prelim", type=Path, required=True)
    a = p.parse_args(argv)
    os.environ.setdefault("CUBLAS_WORKSPACE_CONFIG", ":4096:8")
    {"index": cmd_index, "generate": cmd_generate}[a.cmd](a)
    return 0


def cmd_generate(a) -> None:
    import genuine_cuda_round0

    genuine_cuda_round0.generate(a.out.resolve(), a.prelim.resolve())


if __name__ == "__main__":
    sys.path.insert(0, str(Path(__file__).parent))
    try:
        code = main()
    except BaseException:
        import traceback

        traceback.print_exc()
        code = 1
    sys.stdout.flush()
    sys.stderr.flush()
    os._exit(code)
