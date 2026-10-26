"""S5 / D20 local harness: the S3 E2E scripts with a CUDA backend on one B300 GPU.

Runs scripts/mine_smoke_v2.py (`smoke`) or scripts/mine_multi_round_v2.py (`multi`) inside one
`--rm` container of the GPU image, with the checkout mounted read-only at /opt/ht and first on
PYTHONPATH. The challenge app comes from the pytest network fixture: a fake subnet transport and
a fixture beacon whose verifier accepts any signature. The TLS relay, the miner CLI processes,
admission, audits and get_weights are the repository's own code. The harness changes this:

1. The fixture manifest's reference_spec pins the B300 values: HT_IMAGE_DIGEST, driver
   HT_D20_DRIVER and sm_count HT_D20_SM_COUNT. The run_id changes accordingly.
2. Before any run is created, the harness installs a CUDA qualification record signed by the
   fixture owner, for every manifest posted (with no prior signature check). It carries only
   the fields `_backend_v2` checks; the bootstrap's configs, results and summaries are absent.
   This is a STAND-IN. No engine Result backs it, and S3's qualification (889M dense, one GPU
   per host) does not cover this tiny n_gpus=1 fixture model. Because it replaces
   `_bootstrap_backend_v2`, the only in-process user of gpu_ops/network_qualification.py, the
   PASS does not exercise the 8a90d9bc fix. A real qualification needs two results from two
   distinct machines (`compare` refuses equal machine_id, instance_id or role), so one host cannot
   replace this stand-in. The harness does run the repository's own device-class check
   (`network_qualification.unqualified_device`, ADR 0007) on the (capability, SM count) pairs that
   torch reports for the visible GPUs and logs it as a `device_class` event.
3. After each CUDA audit replay, and before `_dispatch_env_v2` reads it, the harness writes
   role-dispatch.json from this challenge process's GPU: nvidia-smi driver and UUID, plus the
   torch SM count. It is not a dispatcher and it is not bound to the publication. Its
   image_digest is HT_IMAGE_DIGEST, so the image check is circular; the original method still
   checks driver, SM count and UUIDs against the manifest.

The miner device comes from HYPERTRAIN_MINER_DEVICE=cuda (MinerConfig env override). This is not
proof of production qualification, of the s100 TEST stack or of any on-chain payment.
"""

from __future__ import annotations

import dataclasses
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path
from typing import Any

SCRIPTS = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(SCRIPTS))

import mine_smoke_v2 as smoke  # noqa: E402

IMAGE = os.environ["HT_IMAGE_DIGEST"]
DRIVER = os.environ.get("HT_D20_DRIVER", "595.91.07")
SM_COUNT = int(os.environ.get("HT_D20_SM_COUNT", "148"))
STANDIN = "s5-d20 harness stand-in: fixture-owner signed, no engine Result"


def _local_gpus(n: int) -> dict[str, Any]:
    import torch

    from hypertrain.miner.island_launch import gpu_uuid

    smi = shutil.which("nvidia-smi")
    if smi is None:
        raise RuntimeError("nvidia-smi unavailable")
    rows = subprocess.run(  # noqa: S603 - PATH-resolved nvidia-smi, fixed query
        [smi, "--query-gpu=uuid,driver_version", "--format=csv,noheader"],
        check=True,
        capture_output=True,
        text=True,
        timeout=10,
    ).stdout.splitlines()
    pairs = [tuple(p.strip() for p in row.split(",")) for row in rows if row.strip()]
    if len(pairs) != n or torch.cuda.device_count() != n:
        raise RuntimeError(f"need exactly {n} visible GPU(s), nvidia-smi sees {len(pairs)}")
    return {
        "backend": "cuda",
        "image_digest": IMAGE,
        "drivers": [p[1] for p in pairs],
        "sm_counts": [torch.cuda.get_device_properties(i).multi_processor_count for i in range(n)],
        "gpu_uuids": [gpu_uuid(p[0]) for p in pairs],
    }


def _event(**fields: Any) -> None:
    """One JSON line per harness observation in $HT_D20_EVENTS (no secrets are logged)."""
    path = os.environ.get("HT_D20_EVENTS")
    if path:
        with open(path, "a") as out:
            out.write(json.dumps(fields, sort_keys=True, default=str) + "\n")


def _device_class() -> None:
    """Log the repository's device-class verdict for the GPUs this process sees (real values)."""
    import torch

    from hypertrain.gpu_ops.network_qualification import unqualified_device

    props = [torch.cuda.get_device_properties(i) for i in range(torch.cuda.device_count())]
    capabilities = [(p.major, p.minor) for p in props]
    sm_counts = [p.multi_processor_count for p in props]
    _event(
        event="device_class",
        capabilities=capabilities,
        sm_counts=sm_counts,
        unqualified=unqualified_device(capabilities, sm_counts),
    )


def _patch_store(owner: Any) -> None:
    from hypertrain.challenge.store import ChallengeStore
    from hypertrain.protocol import envelope_v2
    from hypertrain.protocol.envelope_v2 import seal
    from hypertrain.protocol.hashing import sha256_hex
    from hypertrain.protocol.jcs import canonicalize
    from hypertrain.protocol.messages_v2 import RunManifestV2

    if getattr(ChallengeStore, "_d20_patched", False):
        return
    create, dispatch = ChallengeStore._create_run_v2, ChallengeStore._dispatch_env_v2
    backend_of, audit = ChallengeStore._backend_v2, ChallengeStore.execute_audit_v2
    seen: set[tuple[str, str]] = set()

    def _create_run_v2(self: Any, raw: bytes) -> Any:
        env = envelope_v2.parse_envelope(raw)
        manifest = RunManifestV2.model_validate(env.body)
        ref = manifest.training.reference_spec
        exists = self._db.execute(
            "SELECT 1 FROM records_v2 WHERE run_id=? AND kind='qualification' AND id=?",
            (manifest.run_id(), ref.image_digest),
        ).fetchone()
        if exists is None:
            evidence = {
                "run_id": manifest.run_id(),
                "backend": "cuda",
                "reference_hash": sha256_hex(canonicalize(ref.model_dump(mode="json"))),
                "layout_hash": ref.layout.model_dump_json(),
                "harness": STANDIN,
            }
            authority = self.objects.put(canonicalize(evidence))
            receipt = seal(
                owner,
                "Receipt",
                manifest.run_id(),
                {"w": 0, "commit_hash": authority, "received_round": max(1, self._now(self._db))},
                10_000_000,
            )
            record = {**evidence, "authority_hash": authority, "receipt": receipt}
            self._put_record_v2(manifest.run_id(), "qualification", ref.image_digest, record)
        return create(self, raw)

    def _dispatch_env_v2(self: Any, manifest: Any, directory: Path) -> Any:
        target = directory / "role-dispatch.json"
        if not target.exists():
            n = manifest.training.reference_spec.layout.n_gpus
            body = {"environment": _local_gpus(n), "source": "s5-d20 measured in-container"}
            target.write_bytes(canonicalize(body))
        replay, raw = dispatch(self, manifest, directory)
        _event(event="dispatch_env", replay_env=replay.model_dump(mode="json"))
        return replay, raw

    def _backend_v2(self: Any, manifest: Any) -> Any:
        backend = backend_of(self, manifest)
        if (manifest.run_id(), backend) not in seen:
            seen.add((manifest.run_id(), backend))
            _event(event="backend", run_id=manifest.run_id(), backend=backend)
        return backend

    def execute_audit_v2(self: Any, run_id: str, job_id: str, raw: bytes) -> Any:
        out = audit(self, run_id, job_id, raw)
        verdict = out["verdict"]
        _event(
            event="audit",
            job_id=job_id,
            identity=out["identity"],
            result=verdict["result"],
            replay_env=verdict["replay_env"],
        )
        return out

    ChallengeStore._create_run_v2 = _create_run_v2  # type: ignore[method-assign]
    ChallengeStore._dispatch_env_v2 = _dispatch_env_v2  # type: ignore[method-assign]
    ChallengeStore._backend_v2 = _backend_v2  # type: ignore[method-assign]
    ChallengeStore.execute_audit_v2 = execute_audit_v2  # type: ignore[method-assign]
    ChallengeStore._d20_patched = True  # type: ignore[attr-defined]


_load = smoke._load_service


def _load_service() -> Any:
    from hypertrain.protocol.messages_v2 import RunManifestV2

    svc = _load()
    fixture = svc.fixture
    setup = fixture.setup

    def b300_setup(mode: str = "test") -> Any:
        s = setup(mode)
        body = s.manifest.body()
        body["training"]["reference_spec"].update(
            image_digest=IMAGE, driver_allowlist=[DRIVER], sm_count=SM_COUNT
        )
        return dataclasses.replace(s, manifest=RunManifestV2.model_validate(body))

    fixture.setup = b300_setup
    _patch_store(svc.OWNER)
    _device_class()
    return svc


smoke._load_service = _load_service


def main() -> int:
    if os.environ.get("HYPERTRAIN_MINER_DEVICE") != "cuda":
        raise SystemExit("set HYPERTRAIN_MINER_DEVICE=cuda")
    print(json.dumps({"image": IMAGE, "driver": DRIVER, "sm_count": SM_COUNT}), flush=True)
    match sys.argv[1:]:
        case ["smoke"]:
            return smoke.main()
        case ["multi"]:
            import mine_multi_round_v2 as multi

            return multi.main()
    raise SystemExit("usage: gpu_run_v2.py smoke|multi")


if __name__ == "__main__":
    sys.exit(main())
