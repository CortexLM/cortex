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
   hosts. `compare` refuses a pair whose machine_id, instance_id or role are equal, but those are
   labels the caller writes into each result: a relabelled copy of one host's result passes, so
   `compare` does not stop one host from supplying both. The harness does run the repository's own
   device-class check (`network_qualification.unqualified_device`, ADR 0007) on the (capability,
   SM count) pairs that torch reports for the visible GPUs and logs it as a `device_class` event.
   That check reads neither the model name nor a GPU UUID: an RTX 5090 (12.0, 170 SMs) is in the
   table and would pass here with HT_D20_SM_COUNT=170, and a GB300 that reports the same pair as
   a B300 cannot be told from one. "B300" in an evidence folder is the operator's word plus that
   class check; authenticating the machine is future work (D26, branch v3-cuda-executor, not in
   this tree).
3. After each CUDA audit replay, and before `_dispatch_env_v2` reads it, the harness writes
   role-dispatch.json from this challenge process's GPU: nvidia-smi driver and UUID, plus the
   torch SM count. It is not a dispatcher and it is not bound to the publication. Its
   image_digest is HT_IMAGE_DIGEST, so the image check is circular; the original method still
   checks driver, SM count and UUIDs against the manifest.

The miner device comes from HYPERTRAIN_MINER_DEVICE=cuda (MinerConfig env override). This is not
proof of production qualification, of the s100 TEST stack or of any on-chain payment.

Modes: `smoke`, `multi` (dense fixture model), and for kimi-k3-pro-tiny (ADR 0006, D18)
`smoke-kimi`, `multi-kimi` and `job-kimi`. The kimi modes swap the fixture model for the MICRO
geometry of tests/miner/test_client_network_v2.py (the fixture admission policy caps one object at
1,000,000 bytes, so the 77M-parameter PROFILE cannot ride this challenge). `job-kimi` is that
test on CUDA: join-v2 then `run-v2 --job`, and the miner's torchrun worker must equal an
in-process CUDA `train_round` bit for bit.
"""

from __future__ import annotations

import dataclasses
import json
import os
import shutil
import subprocess
import sys
import time
from collections.abc import Iterable
from pathlib import Path
from typing import Any

SCRIPTS = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(SCRIPTS))

import mine_smoke_v2 as smoke  # noqa: E402

IMAGE = os.environ["HT_IMAGE_DIGEST"]
DRIVER = os.environ.get("HT_D20_DRIVER", "595.91.07")
SM_COUNT = int(os.environ.get("HT_D20_SM_COUNT", "148"))
STANDIN = "s5-d20 harness stand-in: fixture-owner signed, no engine Result"
MODEL = "dense"  # "kimi" in the *-kimi modes
KIMI_DTYPE = os.environ.get("HT_D20_KIMI_DTYPE", "bf16")  # the dtype S3 qualified for kimi


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


DEVICE_CLASS: dict[str, Any] = {}


def _device_class() -> None:
    """Record the repository's device-class verdict for the GPUs this process sees (real values) and
    stop when there is no GPU or the verdict refuses the pair: a run there would prove nothing."""
    import torch

    from hypertrain.gpu_ops.network_qualification import unqualified_device

    props = [torch.cuda.get_device_properties(i) for i in range(torch.cuda.device_count())]
    capabilities = [(p.major, p.minor) for p in props]
    sm_counts = [p.multi_processor_count for p in props]
    reason = unqualified_device(capabilities, sm_counts)
    DEVICE_CLASS.update(capabilities=capabilities, sm_counts=sm_counts, unqualified=reason)
    _event(event="device_class", **DEVICE_CLASS)
    if not props:
        raise SystemExit("no CUDA device is visible to the harness")
    if reason is not None:
        raise SystemExit(f"the repository refuses this device class: {reason}")


def _kimi_setup(fixture: Any, s: Any) -> Any:
    """The fixture Setup with the kimi-k3-pro-tiny MICRO model, as in
    tests/miner/test_client_network_v2.py::test_kimi_manifest_joins_and_runs_v2_through_the_cli."""
    import math

    import numpy as np

    import hypertrain.models.kimi_tiny as kt
    from hypertrain.protocol.hashing import MerkleTree
    from hypertrain.protocol.messages import f32hex
    from hypertrain.protocol.messages_v2 import RunManifestV2
    from hypertrain.trainer.compress import state_hash
    from hypertrain.trainer.config import TrainConfig
    from hypertrain.trainer.model import init_params

    geom = kt.MICRO
    body = s.manifest.body()
    training = body["training"]
    training["model"].update(
        arch=kt.ARCH,
        n_layers=geom.n_layers,
        d_model=geom.d_model,
        n_heads=geom.n_heads,
        n_kv_heads=geom.n_kv_heads,
        d_ff=geom.d_ff,
        vocab=geom.vocab,
        seq_len=geom.seq_len,
        n_experts=geom.n_experts,
        top_k_experts=geom.top_k_experts,
        rope_theta=50000,
        init_seed=7,
        capacity_factor=f32hex(1.0),
        aux_loss_coef=f32hex(1e-4),
        compute_dtype=KIMI_DTYPE,
        param_count=sum(math.prod(x) for x in kt.param_shapes(geom).values()),
    )
    training["inner"].update(H=2, J=1, micro_batch=2)
    rows = (
        (np.arange(16 * (geom.seq_len + 1)) * 7919 + 3).reshape(16, geom.seq_len + 1) % geom.vocab
    ).astype("<u4")
    tree = MerkleTree([row.tobytes() for row in rows])
    training["dataset"].update(merkle_root=tree.root.hex(), n_samples=16, depth=4)
    cfg = TrainConfig.from_manifest_v2(RunManifestV2.model_validate(body))
    training["init_state_hash"] = state_hash(init_params(cfg.model))
    return fixture.Setup(
        RunManifestV2.model_validate(body), s.policy, s.admission_policy, rows, tree
    )


def _job_kimi() -> int:
    """join-v2 then `run-v2 --job` for kimi-k3-pro-tiny on this GPU (the D18 CLI test, on CUDA).

    The miner's torchrun worker (device cuda, a fresh interpreter) must equal an in-process CUDA
    `train_round` on the same samples: same leaves_root, final_theta_hash, delta_hash and delta
    bytes, and the router bias must move. The staged job's deadline is the fixture's 120 s, which
    covers join-v2 and the run (65.8 s on the B300). Writes $HT_D20_EVIDENCE and prints the same
    JSON; a failing join or run is written as FAIL with the exit codes, not as a crash.
    """
    import tempfile

    import httpx
    import numpy as np
    import torch

    import hypertrain.models.kimi_tiny as kt
    from hypertrain.auditor.replay import unpack_state
    from hypertrain.miner import cli
    from hypertrain.protocol.envelope_v2 import seal
    from hypertrain.trainer.config import TrainConfig
    from hypertrain.trainer.loop import Assignment, train_round
    from hypertrain.trainer.model import init_params, param_shapes

    fixture = smoke._load_service().fixture
    s = fixture.setup()
    manifest = s.manifest
    run = manifest.run_id()
    cfg = TrainConfig.from_manifest_v2(manifest)
    with tempfile.TemporaryDirectory(prefix="d20-kimi-job-") as work:
        tmp = Path(work)
        job = fixture.stage(s, tmp / "attempt", (0, 1, 2, 3), 0)
        (tmp / "attempt/job.json").write_text(job.model_dump_json())
        for name, seed in (("hot.key", b"b"), ("cold.key", b"c")):
            (tmp / name).write_bytes(seed * 32)
            (tmp / name).chmod(0o600)
        config = tmp / "miner.toml"
        config.write_text(
            f'api = "https://service.test"\nkeyfile = "{tmp / "hot.key"}"\n'
            f'workdir = "{tmp / "work"}"\nstate_source = "unused"\n'
            f'image_digest = "{manifest.training.reference_spec.image_digest}"\n'
            f'run_id = "{run}"\nowner_hotkey = "{fixture.COORD.ss58}"\ndevice = "cuda"\n'
        )
        envelope = seal(fixture.COORD, "RunManifestV2", run, manifest, 1000)
        joins: list[dict[str, Any]] = []

        def master(request: httpx.Request) -> httpx.Response:
            if (request.method, request.url.path) == ("GET", f"/v2/runs/{run}"):
                return httpx.Response(200, json={"manifest_envelope": envelope})
            if (request.method, request.url.path) == ("POST", f"/v2/runs/{run}/join"):
                joins.append(json.loads(request.content))
                return httpx.Response(200, json={"state": "APPLIED"})
            return httpx.Response(404, json={"detail": "unexpected route"})

        client_type = httpx.Client
        cli.httpx.Client = lambda **kwargs: client_type(
            transport=httpx.MockTransport(master), **kwargs
        )  # type: ignore[assignment,misc]
        try:
            args = ["--config", str(config)]
            started = time.monotonic()
            joined = cli.main(
                [
                    "join-v2",
                    *args,
                    "--coldkey",
                    str(tmp / "cold.key"),
                    "--request-id",
                    "ab" * 32,
                    "--expiry",
                    "100",
                ]
            )
            ran = cli.main(["run-v2", *args, "--job", str(tmp / "attempt/job.json")])
            elapsed = time.monotonic() - started
        finally:
            cli.httpx.Client = client_type  # type: ignore[misc]
        published = tmp / "attempt/published/rank-0"
        base: dict[str, Any] = {
            "mode": "job-kimi",
            "run_id": run,
            "dtype": KIMI_DTYPE,
            "gpu": torch.cuda.get_device_name(0),
            "device_class": DEVICE_CLASS,
            "join_v2_plus_run_v2_job_elapsed_seconds": round(elapsed, 1),
        }
        if joined != 0 or ran != 0 or not (published / "summary.json").exists():
            return _finish_job(
                {
                    **base,
                    "result": "FAIL",
                    "checks": {
                        "join_rc0": joined == 0,
                        "run_rc0": ran == 0,
                        "summary_written": (published / "summary.json").exists(),
                    },
                }
            )
        commitments = json.loads((published / "summary.json").read_bytes())["commitments"]
        backend = json.loads((published / "summary.json").read_bytes()).get("backend")
        theta0 = init_params(cfg.model)
        ref = train_round(
            cfg,
            {k: v.to("cuda") for k, v in theta0.items()},
            Assignment(run, 0, (0, 1, 2, 3), 0),
            lambda i: s.rows[i].astype(np.uint32),
        )
        theta, _ = unpack_state((published / "state.safetensors").read_bytes())
        bias = "layers.001.router_bias"
        reference = {
            "leaves_root": ref.leaves_root,
            "final_theta_hash": ref.final_theta_hash,
            "delta_hash": ref.delta_hash,
        }
        verdict = _job_verdict(
            run=run,
            policy_hash=manifest.network.admission_policy_hash,
            joins=joins,
            backend=backend,
            commitments=commitments,
            reference=reference,
            delta=(published / "delta.bin").read_bytes(),
            reference_delta=ref.delta_payload,
            parameters=theta,
            engine_parameters=param_shapes(cfg.model),
            model_parameters=kt.param_shapes(cfg.model),
            bias_before=theta0[bias].numpy(),
            bias_after=theta[bias].numpy(),
        )
        return _finish_job(
            {
                **base,
                **verdict,
                "commitments": {
                    k: commitments[k] for k in ("leaves_root", "final_theta_hash", "delta_hash")
                },
                "reference": reference,
            }
        )


def _job_verdict(
    *,
    run: str,
    policy_hash: str,
    joins: list[dict[str, Any]],
    backend: object,
    commitments: dict[str, str],
    reference: dict[str, str],
    delta: bytes,
    reference_delta: bytes,
    parameters: Iterable[str],
    engine_parameters: Iterable[str],
    model_parameters: Iterable[str],
    bias_before: Any,
    bias_after: Any,
) -> dict[str, Any]:
    """The job-kimi checks and PASS/FAIL from plain values (no torch), so they run without CUDA.

    `commitments` and `backend` come from the miner's summary.json, `reference` and
    `reference_delta` from the in-process CUDA `train_round`, `joins` are the bodies POSTed. A
    failing join or run returned before this point, hence the two True entries.
    """
    import numpy as np

    keys = ("leaves_root", "final_theta_hash", "delta_hash")
    checks = {
        "join_rc0": True,
        "run_rc0": True,
        "join_binds_run_and_policy": [(j["run_id"], j["policy_hash"]) for j in joins]
        == [(run, policy_hash)],
        "summary_backend_cuda": backend == "cuda",
        "commitments_equal_cuda_reference": tuple(commitments[k] for k in keys)
        == tuple(reference[k] for k in keys),
        "delta_bytes_equal": delta == reference_delta,
        "parameter_set": sorted(parameters)
        == sorted(engine_parameters)
        == sorted(model_parameters),
        "router_bias_moved": not np.array_equal(bias_after, bias_before),
    }
    return {"result": "PASS" if all(checks.values()) else "FAIL", "checks": checks}


def _finish_job(result: dict[str, Any]) -> int:
    """Write the job-kimi evidence ($HT_D20_EVIDENCE), print it, and return the exit code."""
    text = json.dumps(result, indent=1, sort_keys=True, default=str)
    if os.environ.get("HT_D20_EVIDENCE"):
        Path(os.environ["HT_D20_EVIDENCE"]).write_text(text)
    print(text)
    return 0 if result["result"] == "PASS" else 1


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
        if MODEL == "kimi":
            s = _kimi_setup(fixture, s)
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
    global MODEL
    mode = sys.argv[1:]
    print(json.dumps({"image": IMAGE, "driver": DRIVER, "sm_count": SM_COUNT}), flush=True)
    if mode and mode[0].endswith("-kimi"):
        MODEL = "kimi"
        _event(event="model", arch="kimi-k3-pro-tiny", dtype=KIMI_DTYPE, geometry="MICRO")
    match mode:
        case ["smoke"] | ["smoke-kimi"]:
            return smoke.main()
        case ["multi"] | ["multi-kimi"]:
            import mine_multi_round_v2 as multi

            return multi.main()
        case ["job-kimi"]:
            return _job_kimi()
    raise SystemExit("usage: gpu_run_v2.py smoke|multi|smoke-kimi|multi-kimi|job-kimi")


if __name__ == "__main__":
    sys.exit(main())
