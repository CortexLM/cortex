"""Production `_role_launch`: master replay dispatched to an owned, qualified remote CUDA island.

Reuses scripts/network_service_proof.py `service_factory` (owner Receipt per operation) and the
experiments/gpu_network_v2 NetworkRuntime (debit, strict SSH, rescue custody, canonical
publication). Nothing runs locally; the config is parsed at startup, and the lifecycle journal
is read per call so a run qualified after startup needs no restart.
"""

from __future__ import annotations

import importlib.util
import json
import sys
import threading
import time
from collections.abc import Callable, Mapping
from pathlib import Path
from types import ModuleType
from typing import TYPE_CHECKING, Any, Literal

from pydantic import BaseModel, ConfigDict

from hypertrain.gpu_ops.journal import Journal, durable_write, fsha
from hypertrain.protocol.jcs import canonicalize

if TYPE_CHECKING:
    from hypertrain.gpu_ops.work_screen import IslandLaunch
    from hypertrain.miner.island_launch import IslandArtifacts
    from hypertrain.protocol.messages_v2 import IslandJobV1

RoleLaunch = Callable[[str, "IslandJobV1", Path, Mapping[str, Any]], "IslandLaunch"]


class DispatchConfig(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True)
    tree: Path  # reviewed source tree holding scripts/ and experiments/gpu_network_v2/
    run_dir: Path  # existing admitted lifecycle journal + config.json (never created here)
    owner_key_file: Path
    roles: dict[str, str]  # admitted hotkey -> owned host role
    run_id: str  # the one run this owner authorized; any other job is refused before any I/O


def _module(name: str, path: Path) -> ModuleType:
    if name in sys.modules:
        return sys.modules[name]
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"role dispatch source missing: {path}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


def provenance(journal: Journal, directory: Path) -> dict[str, Any]:
    """Bind the rescued remote environment of THIS canonical publication into role-dispatch.json."""
    canonical = journal.last("network_canonical_done", directory=str(directory / "published"))
    if canonical is None:
        raise RuntimeError("role dispatch canonical publication absent")
    done = journal.last("network_operation_done", role=canonical["role"], name=canonical["name"])
    result_path = directory / canonical["name"] / "operation-result.json"
    if done is None or result_path.is_symlink() or fsha(result_path) != done["result_sha256"]:
        raise RuntimeError("role dispatch operation result custody changed")
    result = json.loads(result_path.read_bytes())
    value = {
        "role": canonical["role"],
        "name": canonical["name"],
        "operation_result_sha256": done["result_sha256"],
        "tar_sha256": done["tar_sha256"],
        "environment": result["environment"],
    }
    path, data = directory / "role-dispatch.json", canonicalize(value)
    if path.exists() or path.is_symlink():
        # Resume after a crash between this write and lease settlement: identical bytes only.
        if path.is_symlink() or path.read_bytes() != data:
            raise RuntimeError("role dispatch provenance already differs")
        return value
    durable_write(path, data)
    return value


def load(path: Path) -> RoleLaunch:
    config = DispatchConfig.model_validate_json(path.read_bytes())
    scripts = str(config.tree / "scripts")
    if scripts not in sys.path:
        sys.path.insert(0, scripts)

    def factory(
        operation: str, job: IslandJobV1, directory: Path, context: Mapping[str, Any]
    ) -> IslandLaunch:
        from hypertrain.gpu_ops.launcher import Orchestrator
        from hypertrain.gpu_ops.network_qualification import sources
        from hypertrain.gpu_ops.provider import Provider, read_api_key
        from hypertrain.miner.core import load_keyfile

        if job.run_id != config.run_id:
            raise RuntimeError("role dispatch job is not the authorized run")
        proof = _module("network_service_proof", config.tree / "scripts/network_service_proof.py")
        orchestrate = _module(
            "network_role_runtime", config.tree / "experiments/gpu_network_v2/orchestrate.py"
        )
        cfg = json.loads((config.run_dir / "config.json").read_bytes())
        profile = json.loads(Path(cfg["network_profile_file"]).read_bytes())
        journal = Journal(config.run_dir)
        # live=False: dispatch never creates/deletes rentals; only owned-host SSH operations.
        provider = Provider(
            cfg["base_url"], read_api_key(cfg["key_file"]), config.run_dir, journal, False
        )
        runtime = orchestrate.NetworkRuntime(
            Orchestrator(cfg, journal, provider, config.run_dir), profile
        )
        pinned = sources(config.tree)
        pinned["scripts/network_gpu_operation.py"] = fsha(
            config.tree / "scripts/network_gpu_operation.py"
        )
        beacon = job.manifest.training.beacon
        launch = proof.service_factory(
            proof,
            runtime,
            load_keyfile(config.owner_key_file),
            config.run_id,
            pinned,
            {hotkey: {"role": role} for hotkey, role in config.roles.items()},
            lambda: int(time.time() - beacon.genesis_time) // beacon.period + 1,
        )(operation, job, directory, context)

        def dispatched(
            job: IslandJobV1,
            directory: Path,
            *,
            backend: Literal["cpu", "cuda"],
            cancel: threading.Event | None = None,
            trace: bool = False,
        ) -> IslandArtifacts:
            artifacts = launch(job, directory, backend=backend, cancel=cancel, trace=trace)
            provenance(journal, Path(directory))
            return artifacts

        return dispatched

    return factory
