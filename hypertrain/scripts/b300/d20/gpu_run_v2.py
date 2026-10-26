"""S5 / D20 local harness: the S3 E2E scripts with a CUDA backend on one B300 GPU.

Runs scripts/mine_smoke_v2.py (`smoke`) or scripts/mine_multi_round_v2.py (`multi`) inside one
`--rm` container of the GPU image, with the checkout (it contains 8a90d9bc) mounted read-only
at /opt/ht and first on PYTHONPATH. The challenge app, the TLS relay, the miner CLI processes,
admission, audits and get_weights are the repository's own code. Only three things change:

1. The fixture manifest's reference_spec pins the B300 values: HT_IMAGE_DIGEST, driver
   HT_D20_DRIVER and sm_count HT_D20_SM_COUNT. The run_id changes accordingly.
2. Before the run is created, the harness installs a CUDA qualification record signed by the
   fixture owner, with the same shape `_bootstrap_backend_v2` writes. This is a STAND-IN: no
   engine Result backs it. The hardware evidence is S3's 2-host dense qualification.
3. Before each CUDA audit, the role-dispatch provenance the original `_dispatch_env_v2` reads is
   written from this container's GPU (driver, SM count and UUID measured here). It is not a
   dispatcher; the original method still validates it against the manifest.

The miner device comes from HYPERTRAIN_MINER_DEVICE=cuda (MinerConfig env override). Not proof
of production qualification, of the s100 TEST stack or of any on-chain payment.
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
        return dispatch(self, manifest, directory)

    ChallengeStore._create_run_v2 = _create_run_v2  # type: ignore[method-assign]
    ChallengeStore._dispatch_env_v2 = _dispatch_env_v2  # type: ignore[method-assign]
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
