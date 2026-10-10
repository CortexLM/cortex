"""Prepare local D2 artifacts from verified frozen bytes; never admit or launch GPUs."""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import io
import json
import os
import secrets
import sys
import tarfile
from pathlib import Path


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def openssl(arguments: list[str]) -> None:
    pid = os.posix_spawn(
        "/usr/bin/openssl",
        ["/usr/bin/openssl", *arguments],
        os.environ,
        file_actions=[(os.POSIX_SPAWN_OPEN, 2, os.devnull, os.O_WRONLY, 0o600)],
    )
    _, status = os.waitpid(pid, 0)
    if os.waitstatus_to_exitcode(status) != 0:
        raise ValueError("experiment-local certificate generation failed")


def save(path: Path, value, private: bool = False) -> None:
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    data = value if isinstance(value, bytes) else json.dumps(value, sort_keys=True).encode()
    with path.open("xb") as stream:
        os.fchmod(stream.fileno(), 0o600 if private else 0o644)
        stream.write(data)
        stream.flush()
        os.fsync(stream.fileno())


def load_frozen(freeze_path: Path):
    """Verify complete staging, qualification and operation maps before importing."""
    freeze = json.loads(freeze_path.read_bytes())
    tree = Path(freeze["snapshot"]) / "tree"
    for item in (
        *freeze["maps"].values(),
        freeze["archive"],
        freeze["qualification_archive"],
    ):
        path = Path(item["path"])
        if (
            path.is_symlink()
            or path.stat().st_size != item["bytes"]
            or digest(path) != item["sha256"]
        ):
            raise ValueError("frozen archive/map changed")
    maps = {
        name: json.loads(Path(item["path"]).read_bytes()) for name, item in freeze["maps"].items()
    }
    for relative, expected in maps["sources.json"].items():
        path = tree / relative
        if (
            Path(relative).is_absolute()
            or ".." in Path(relative).parts
            or path.is_symlink()
            or digest(path) != expected
        ):
            raise ValueError("frozen source changed: " + relative)
    if any(digest(tree / name) != expected for name, expected in freeze["approved_pins"].items()):
        raise ValueError("approved frozen pin changed")
    if any(name == "hypertrain" or name.startswith("hypertrain.") for name in sys.modules):
        raise ValueError("preparation requires clean process without live package imports")
    sys.path[:0] = [str(tree / "src"), str(tree / "scripts")]

    def module(name: str, relative: str):
        spec = importlib.util.spec_from_file_location(name, tree / relative)
        if spec is None or spec.loader is None:
            raise ValueError("frozen callable absent: " + relative)
        loaded = importlib.util.module_from_spec(spec)
        sys.modules[name] = loaded
        spec.loader.exec_module(loaded)
        return loaded

    seed = module("prepared_seed", "scripts/network_gpu_seed.py")
    driver = module("prepared_driver", "scripts/network_service_proof.py")
    controller = module("prepared_controller", "experiments/gpu_network_v2/orchestrate.py")
    from hypertrain.gpu_ops.network_qualification import sources

    if sources(tree) != maps["qualification-sources.json"]:
        raise ValueError("canonical qualification map differs")
    operation = dict(sources(tree))
    operation["scripts/network_gpu_operation.py"] = digest(
        tree / "scripts/network_gpu_operation.py"
    )
    if operation != maps["operation-sources.json"]:
        raise ValueError("operation map differs")
    return freeze, tree, maps, seed, driver, controller


def service_factory(driver, runtime, owner, run_id, sources, roles, current_beacon):
    """Derive receipts only for actual accepted contexts, through frozen factory."""
    from network_service_proof import service_factory as accepted_factory

    return accepted_factory(driver, runtime, owner, run_id, sources, roles, current_beacon)


def validate_launch(controller, profile: dict, remaining: dict) -> None:
    """Exercise original admission refusal; never infer observations from candidates."""
    from hypertrain.gpu_ops.launcher import Reject

    try:
        controller.runtime_admission(profile, {})
    except Reject as error:
        raise ValueError(
            "launch refused: " + str(error) + "; missing=" + ",".join(remaining)
        ) from error
    raise ValueError("launch refused: required observed admission inputs absent")


def candidate_freeze(original: Path, destination: Path, candidates: list[str]) -> Path:
    """Copy verified original bytes; change only the expressly approved profile input."""
    if candidates != ["580.95.05", "580.159.03"]:
        raise ValueError("candidate drivers require exact approved ordered pair")
    freeze = json.loads(original.read_bytes())
    old_tree = Path(freeze["snapshot"]) / "tree"
    for item in (
        *freeze["maps"].values(),
        freeze["archive"],
        freeze["qualification_archive"],
    ):
        path = Path(item["path"])
        if (
            path.is_symlink()
            or path.stat().st_size != item["bytes"]
            or digest(path) != item["sha256"]
        ):
            raise ValueError("original frozen archive/map changed")
    maps = {
        name: json.loads(Path(item["path"]).read_bytes()) for name, item in freeze["maps"].items()
    }
    for relative, expected in maps["sources.json"].items():
        path = old_tree / relative
        if (
            Path(relative).is_absolute()
            or ".." in Path(relative).parts
            or path.is_symlink()
            or digest(path) != expected
        ):
            raise ValueError("original source changed: " + relative)
    key = "experiments/gpu_network_v2/profile.json"
    profile = json.loads((old_tree / key).read_bytes())
    profile["runtime"]["driver_allowlist"] = candidates
    tree = destination / "tree"
    for relative in maps["sources.json"]:
        save(
            tree / relative,
            profile if relative == key else (old_tree / relative).read_bytes(),
        )
    changed = digest(tree / key)
    for name, values in maps.items():
        if name == "files.json":
            for relative, entry in values.items():
                entry.update(
                    origin=str(tree / relative),
                    size=(tree / relative).stat().st_size,
                    sha256=digest(tree / relative),
                )
        else:
            values[key] = changed
        save(destination / name, values)
        freeze["maps"][name].update(
            path=str(destination / name),
            bytes=(destination / name).stat().st_size,
            sha256=digest(destination / name),
        )
    for field, map_name, filename in (
        ("archive", "sources.json", "effective-source.tar"),
        (
            "qualification_archive",
            "qualification-sources.json",
            "qualification-source.tar",
        ),
    ):
        buffer = io.BytesIO()
        with tarfile.open(fileobj=buffer, mode="w") as archive:
            for relative in sorted(maps[map_name]):
                data = (tree / relative).read_bytes()
                info = tarfile.TarInfo(relative)
                info.size, info.mode, info.mtime = len(data), 0o600, 0
                archive.addfile(info, io.BytesIO(data))
        save(destination / filename, buffer.getvalue())
        freeze[field].update(
            path=str(destination / filename),
            bytes=(destination / filename).stat().st_size,
            sha256=digest(destination / filename),
            files=len(maps[map_name]),
        )
    freeze.update(
        snapshot=str(destination),
        effective_profile_sha256=changed,
        parent_freeze_sha256=digest(original),
        candidate_input_scope={
            "drivers": candidates,
            "qualification_executions": 4,
            "observed_driver": "NOT_PRESENT",
            "CUDA_qualified": False,
            "spend_authorized": False,
        },
    )
    for relative, entry in freeze["service_source_observed"].items():
        entry["origin"] = str(tree / relative)
    freeze["gate"] = {"scope": "CANDIDATE_INPUT_REGENERATION_NOT_RUNTIME_QUALIFICATION"}
    freeze["qualification_parser"]["package_origin"] = str(
        tree / "src/hypertrain/gpu_ops/network_qualification.py"
    )
    freeze["qualification_parser"]["wrapper_origin"] = str(
        tree / "scripts/network_gpu_qualification.py"
    )
    freeze["public_inputs"] = {
        "claim": "SYNTHETIC_FUNCTIONAL_SEED_NOT_CUDA_QUALIFIED",
        "historical_only": True,
        "fresh_seed_generated": False,
    }
    save(destination / "freeze.json", freeze)
    return destination / "freeze.json"


def prepare(
    freeze_path: Path, output: Path, genesis: int, candidates: list[str] | None = None
) -> dict:
    if output.exists() or genesis < 1:
        raise ValueError("new output and positive deterministic CPU genesis required")
    if candidates is not None and candidates != ["580.95.05", "580.159.03"]:
        raise ValueError("candidate drivers require exact approved ordered pair")
    if candidates is not None:
        freeze_path = candidate_freeze(freeze_path, output / "source-freeze", candidates)
    freeze, tree, maps, seed, driver, controller = load_frozen(freeze_path)
    output.mkdir(parents=True, exist_ok=candidates is not None, mode=0o700)
    private, public = output / "private", output / "public"
    profile = json.loads((tree / "experiments/gpu_network_v2/profile.json").read_bytes())
    seed_hash = seed.generate(
        public / "seed",
        private / "roles",
        profile["runtime"]["image_digest"],
        profile["runtime"]["driver_allowlist"],
        genesis,
        genesis + 3600,
    )
    job = seed.validate(public / "seed", seed_hash)
    from hypertrain.beacon.core import FixtureBeacon
    from hypertrain.miner.core import MinerConfig
    from hypertrain.protocol.keys import Keypair

    owner = Keypair((private / "roles/owner.seed").read_bytes())
    for role in seed.ROLES:
        save(
            private / (role + ".hex"),
            (private / ("roles/" + role + ".seed")).read_bytes().hex().encode(),
            True,
        )
    for name in ("admin", "worker", "internal", "drain"):
        save(private / (name + ".token"), secrets.token_hex(32).encode(), True)
    # Experiment-local trust only; never publish the CA private key.
    openssl(
        [
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-days",
            "2",
            "-subj",
            "/CN=Hypertrain local experiment CA",
            "-addext",
            "basicConstraints=critical,CA:TRUE",
            "-keyout",
            str(private / "ca.key"),
            "-out",
            str(public / "ca.pem"),
        ],
    )
    (private / "ca.key").chmod(0o600)
    for host in ("master.test", "relay.test"):
        openssl(
            [
                "req",
                "-newkey",
                "rsa:2048",
                "-nodes",
                "-subj",
                "/CN=" + host,
                "-keyout",
                str(private / (host + ".key")),
                "-out",
                str(private / (host + ".csr")),
            ],
        )
        save(
            private / (host + ".ext"),
            ("subjectAltName=DNS:" + host + "\nextendedKeyUsage=serverAuth\n").encode(),
            True,
        )
        openssl(
            [
                "x509",
                "-req",
                "-in",
                str(private / (host + ".csr")),
                "-CA",
                str(public / "ca.pem"),
                "-CAkey",
                str(private / "ca.key"),
                "-set_serial",
                str(1 if host == "master.test" else 2),
                "-days",
                "2",
                "-extfile",
                str(private / (host + ".ext")),
                "-out",
                str(public / (host + ".crt")),
            ],
        )
        (private / (host + ".key")).chmod(0o600)
    beacon = FixtureBeacon(current=1).get(1)
    save(
        public / "cpu-beacon.json",
        {
            "round": 1,
            "signature": beacon.signature,
            "randomness": beacon.randomness,
            "bls_verified": False,
            "purpose": "DETERMINISTIC_CPU_ONLY",
            "genesis_unix": genesis,
        },
    )
    joins = []
    for i in range(4):
        hot, cold = (
            Keypair((private / ("roles/" + kind + f"-{i}.seed")).read_bytes())
            for kind in ("hot", "cold")
        )
        join = driver.sign_join(
            hot,
            cold,
            run_id=job.run_id,
            request_id=f"{i + 1:064x}",
            expires_beacon=10000,
            policy_hash=job.manifest.network.admission_policy_hash,
            hardware_hint=driver.HardwareHint(
                device_name="advisory", device_count=2, driver="not-qualification"
            ),
        )
        save(public / f"join-{i}.json", driver.canonicalize(join.body()))
        joins.append(
            {
                "hotkey": hot.ss58,
                "coldkey": cold.ss58,
                "role": "h0" if i < 2 else "h1",
                "join": f"join-{i}.json",
            }
        )
        config = {
            "api": "https://master.test",
            "keyfile": str(private / f"hot-{i}.hex"),
            "workdir": str(output / f"operation-hot-{i}/miner"),
            "state_source": str(public / "seed/start_state"),
            "image_digest": profile["runtime"]["image_digest"],
            "run_id": job.run_id,
            "owner_hotkey": owner.ss58,
            "device": "cpu",
        }
        toml = "\n".join(key + " = " + json.dumps(value) for key, value in config.items()) + "\n"
        path = private / f"miner-{i}.toml"
        save(path, toml.encode(), True)
        MinerConfig.load(path, env={})
    save(public / "membership.json", joins)
    from hypertrain.data.store import LocalFSStore
    from hypertrain.protocol.relay_messages import RelayNetworkManifest, RelayRegistryV1
    from hypertrain.relay.app import RelayConfig

    registry = RelayRegistryV1.model_validate_json(
        LocalFSStore(public / "seed/objects").get(job.manifest.network.relay_registry_hash)
    )
    coord = Keypair((private / "roles/coord.seed").read_bytes())
    network = RelayNetworkManifest(
        run_id=job.run_id,
        base_manifest_hash=job.manifest.digest(),
        registry_hash=registry.digest(),
        specs_hash=driver.sha256_hex(driver.canonicalize([s.body() for s in registry.specs])),
        assignment_policy="master-observed-median3-v1",
    )
    relay_cfg = RelayConfig(
        run_id=job.run_id,
        master_public=coord.ss58,
        master_url="https://master.test",
        registry=driver.relay_envelope.seal(coord, "RelayRegistryV1", job.run_id, registry, 10000),
        network_manifest=driver.relay_envelope.seal(
            coord, "RelayNetworkManifest", job.run_id, network, 10000
        ),
        relay_id="local",
        region="local",
        active_key="k1",
        signing_files={"k1": "relay.hex"},
        observer_public_keys=job.manifest.training.auditors,
        local_backing=str(output / "relay-objects"),
    )
    save(private / "relay.json", relay_cfg.model_dump(mode="json"), True)
    save(
        private / "tls.json",
        {
            "master": {
                "cert": str(public / "master.test.crt"),
                "key": str(private / "master.test.key"),
            },
            "relay": {
                "cert": str(public / "relay.test.crt"),
                "key": str(private / "relay.test.key"),
            },
            "ca": str(public / "ca.pem"),
        },
        True,
    )
    save(
        private / "service.json",
        {
            "state_dir": str(output / "cpu-service"),
            "production_state_dir": str(output / "production-service"),
            "master_url": "https://master.test",
            "relay_url": "https://relay.test",
            "ca": str(public / "ca.pem"),
            "coord_key_file": str(private / "coord.hex"),
            "owner_hotkey": owner.ss58,
            "admin_token_file": str(private / "admin.token"),
            "genesis_unix": genesis,
            "backend": "cpu",
            "production_launchable": False,
        },
        True,
    )
    (output / "production-service").mkdir(mode=0o700)
    # Genuine signed bootstrap only; qualification must precede creation in the OTHER state.
    from hypertrain.challenge.store import ChallengeStore
    from hypertrain.data.store import LocalFSStore
    from hypertrain.ledger import Params

    store = ChallengeStore(
        output / "cpu-service",
        Params("hypertrain", genesis, 4320, 1, job.manifest.training.verify.E_vest_rounds),
        Keypair((private / "roles/coord.seed").read_bytes()),
        owner.ss58,
        driver.authority.fixture_beacon,
        LocalFSStore(public / "seed/objects"),
    )
    store.clock = lambda: float(genesis)
    try:
        store.push_beacon(
            {"round": 1, "signature": beacon.signature, "randomness": beacon.randomness}
        )
        store.create_run_v2((public / "seed/manifest-envelope.json").read_bytes())
        store.genesis_v2(job.run_id, (public / "seed/genesis-envelope.json").read_bytes())
        escrow, _, _ = store._services(job.run_id)
        escrow.verify()
        if store._db.execute("SELECT COUNT(*) FROM admission_trials").fetchone()[0]:
            raise ValueError("unexpected trial during preparation")
    finally:
        store.close_v2_notifications()
        store._db.close()
    count = driver.decoder_execution_count(profile)
    save(public / "artifact-bounds.json", controller.artifact_budget(profile))
    remaining = {
        name: "NOT_PRESENT"
        for name in (
            "owned_instances_and_offers",
            "CUDA_qualification",
            "observed_driver_and_imports",
            "fresh_financial_observations_and_spend_GO",
            "verified_production_beacon",
            "measured_staging_rescue_rates",
            "current_trial_commit_lease_dispute_anchor_contexts",
            "supervisor_and_custody_receipts",
        )
    }
    refusal = ""
    try:
        validate_launch(controller, profile, remaining)
    except ValueError as error:
        refusal = str(error)
    if not refusal:
        raise ValueError("preparation unexpectedly accepted launch")
    for name in (
        "qualification-sources.json",
        "operation-sources.json",
        "sources.json",
    ):
        save(public / name, maps[name])
    save(
        private / "lifecycle-config.json",
        {
            "hosts": [],
            "cleanup_contract": "network-v2",
            "tree": str(tree),
            "network_profile_file": str(tree / "experiments/gpu_network_v2/profile.json"),
            "image": freeze["image_candidate"]["ref"],
            "remote_python": profile["runtime"]["remote_python"],
            "disk_gb": 80,
            "hard_deadline_seconds": 3600,
            "hard_grace_seconds": 0,
        },
        True,
    )
    save(
        public / "lifecycle-hashes.json",
        {
            name: maps["qualification-sources.json"][name]
            for name in (
                "src/hypertrain/gpu_ops/launcher.py",
                "src/hypertrain/gpu_ops/supervisor.py",
            )
        },
    )
    review_files = {
        name: {"path": str(path), "sha256": digest(path)}
        for name, path in {
            "config": private / "lifecycle-config.json",
            "profile": tree / "experiments/gpu_network_v2/profile.json",
            "sources": Path(freeze["maps"]["qualification-sources.json"]["path"]),
            "bundle": Path(freeze["qualification_archive"]["path"]),
            "manifest": public / "seed/manifest-envelope.json",
            "genesis": public / "seed/genesis-envelope.json",
            "lifecycle_hashes": public / "lifecycle-hashes.json",
        }.items()
    }
    record = {
        "action": "admit",
        "owner": owner.ss58,
        "tree": str(tree),
        "parent_pid": os.getppid(),
        "files": review_files,
        "roles": {
            "h0": [x["hotkey"] for x in joins[:2]],
            "h1": [x["hotkey"] for x in joins[2:]],
        },
        "quota": {"total": 126, "per_role": 63, "qualification": 4, "remaining": 122},
        "preparation_only": True,
        "remaining": remaining,
    }
    signed_record = driver.envelope_v2.seal(
        owner,
        "Receipt",
        job.run_id,
        {
            "w": 0,
            "commit_hash": driver.sha256_hex(driver.canonicalize(record)),
            "received_round": 1,
        },
        10000,
    )
    save(private / "review.json", {"record": record, "receipt": signed_record}, True)
    from hypertrain.gpu_ops.launcher import Reject

    try:
        controller.reviewed_launch(private / "review.json", owner.ss58, 1, "admit")
    except Reject as error:
        controller_refusal = str(error)
    else:
        raise ValueError("incomplete preparation unexpectedly admitted by controller")
    inventory = {
        str(path.relative_to(public)): digest(path)
        for path in sorted(public.rglob("*"))
        if path.is_file()
    }
    save(public / "index.json", inventory)
    receipts = {
        "source": maps["operation-sources.json"],
        "index": inventory,
        "freeze": freeze,
    }
    for name, subject in receipts.items():
        save(
            public / (name + "-receipt.json"),
            driver.canonicalize(
                driver.envelope_v2.seal(
                    owner,
                    "Receipt",
                    job.run_id,
                    {
                        "w": 0,
                        "commit_hash": driver.sha256_hex(driver.canonicalize(subject)),
                        "received_round": 1,
                    },
                    10000,
                )
            ),
        )
    result = {
        "status": "LOCAL_PREPARED_NOT_LAUNCHABLE",
        "run_id": job.run_id,
        "seed_index_sha256": seed_hash,
        "freeze_sha256": digest(freeze_path),
        "tree": str(tree),
        "public_index_sha256": digest(public / "index.json"),
        "count": count,
        "remaining": remaining,
        "launch_refusal": refusal,
        "controller_refusal": controller_refusal,
        "operation_receipts": "CREATED_ONLY_AT_RUNTIME_FROM_ACCEPTED_CONTEXTS_BY_service_factory",
        "keys": "ORIGINAL_DETERMINISTIC_SYNTHETIC_SEED_ROLES_NOT_PRODUCTION_CREDENTIALS",
        "kernels_executed": 0,
        "provider_calls": 0,
        "candidate_input_scope": freeze.get(
            "candidate_input_scope", {"drivers": profile["runtime"]["driver_allowlist"]}
        ),
    }
    result["controller_review"] = str(private / "review.json")
    result["production_state"] = "EMPTY_UNTIL_SAME_INSTANCE_CUDA_QUALIFICATION_BOOTSTRAP"
    save(private / "controller.json", result, True)
    save(output / "prepared.json", result)
    return result


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--freeze", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--cpu-genesis-unix", type=int, required=True)
    parser.add_argument("--candidate-driver", action="append")
    args = parser.parse_args()
    os.umask(0o077)
    print(
        json.dumps(
            prepare(
                args.freeze.resolve(),
                args.output.resolve(),
                args.cpu_genesis_unix,
                args.candidate_driver,
            ),
            sort_keys=True,
        )
    )


if __name__ == "__main__":
    main()
