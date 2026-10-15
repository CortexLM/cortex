"""Build and preflight the signed task-8 reviewed record (adopt | qualification).

Never launches, rents, or calls a provider. The executor captures fresh read-only provider files
(financial, offers, observations) moments before, passes them with --file, and this script binds
every file by sha256, computes the plan with the controller's own admission_plan, seals the record
with the owner key and preflights it with the same reviewed_launch the lifecycle CLI runs.
"""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import os
import sys
import time
from pathlib import Path
from typing import Any

# Run-time files the executor may supply; everything else comes from the prepared base record.
FRESH = {"financial", "offers", "observations", "authorization", "config", "profile", "evidence"}
# One real uid247 identity qualifies the same job on both hosts: one bound join/seed context each.
ROLE_FILES = {kind + "_" + role for kind in ("join", "seed_context") for role in ("h0", "h1")}
ADOPT = {"ownership", "provider_journal"}
FRESH_GET = {"financial", "offers", "observations", "authorization"}
EXPIRY_ROUNDS = 10  # receipt is valid for a bounded beacon window only


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def parse_files(items: list[str], action: str) -> dict[str, Path]:
    files: dict[str, Path] = {}
    for item in items:
        name, sep, raw = item.partition("=")
        path = Path(raw)
        if (
            not sep
            or name in files
            or name not in FRESH | ROLE_FILES | (ADOPT if action == "adopt" else set())
            or not path.is_absolute()
            or path.is_symlink()
            or not path.is_file()
        ):
            raise SystemExit("invalid --file " + item)
        files[name] = path
    if action == "adopt" and not ADOPT <= set(files):
        raise SystemExit("adopt requires --file ownership=... and --file provider_journal=...")
    if ROLE_FILES & set(files) and not ROLE_FILES <= set(files):
        raise SystemExit("join_h0/h1 and seed_context_h0/h1 are required together")
    return files


def check_parent(pid: int) -> None:
    if pid <= 1 or pid == os.getpid():
        raise SystemExit("--parent-pid must be the long-lived executor shell")
    try:
        os.close(os.pidfd_open(pid))
    except ProcessLookupError:
        raise SystemExit("--parent-pid is not a live process") from None


def load_controller(tree: Path) -> Any:
    path = (tree / "experiments/gpu_network_v2/orchestrate.py").resolve()
    loaded = sys.modules.get("task8_controller")
    if loaded is not None:
        if Path(loaded.__file__ or "").resolve() != path:
            raise SystemExit("loaded controller is not the prepared tree's controller")
        return loaded
    sys.path[:0] = [str(tree / "src"), str(tree / "scripts")]
    spec = importlib.util.spec_from_file_location("task8_controller", path)
    if spec is None or spec.loader is None:
        raise SystemExit("controller missing in prepared tree")
    controller = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = controller
    spec.loader.exec_module(controller)
    return controller


def load_runtime() -> Any:
    spec = importlib.util.spec_from_file_location(
        "task8_prepare_runtime", Path(__file__).with_name("prepare_runtime.py")
    )
    if spec is None or spec.loader is None:
        raise SystemExit("prepare_runtime.py missing beside the builder")
    runtime = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(runtime)
    return runtime


def warm_seed_genesis(controller: Any, base: dict[str, Any], files: dict[str, Path]) -> None:
    """Expensive immutable seed proof before the fresh GET; seed_genesis rehashes every byte."""
    entry, inputs = base["files"], base["role_inputs"]
    hotkeys = {json.loads(files["join_" + role].read_bytes())["hotkey"] for role in ("h0", "h1")}
    for hotkey in sorted(hotkeys):
        controller.seed_genesis(
            Path(entry[inputs["job"]]["path"]),
            entry[inputs["job"]]["sha256"],
            Path(entry["profile"]["path"]),
            entry["profile"]["sha256"],
            hotkey,
            {k: Path(entry[n]["path"]) for k, n in inputs["objects"].items()},
            {k: entry[n]["sha256"] for k, n in inputs["objects"].items()},
        )


def build(
    base: dict[str, Any],
    files: dict[str, Path],
    action: str,
    parent_pid: int,
    cutoff_unix: int,
    admitted_unix: int | None,
    owner_seed: Path,
    run_id: str,
    beacon: int,
    output: Path,
    provider_key: Path | None = None,
) -> dict[str, Any]:
    controller = load_controller(Path(base["tree"]))
    from hypertrain.protocol import envelope_v2
    from hypertrain.protocol.hashing import sha256_hex
    from hypertrain.protocol.jcs import canonicalize
    from hypertrain.protocol.keys import Keypair

    paths = {name: Path(item["path"]) for name, item in base["files"].items()}
    paths.update(files)

    def data(name: str) -> Any:
        return json.loads(paths[name].read_bytes())

    profile, cfg, snapshot = data("profile"), data("config"), data("financial")
    if "key_file" not in cfg:
        # Qualification base: the complete static config plus the two run-time fields.
        if provider_key is None or not provider_key.is_absolute() or not provider_key.is_file():
            raise SystemExit("--provider-key-file is required for a qualification base config")
        cfg = {**cfg, "key_file": str(provider_key), "cleanup_deadline_unix": cutoff_unix}
        config_path = output.with_name(output.stem + "-config.json")
        with config_path.open("x") as text:
            json.dump(cfg, text, sort_keys=True)
        paths["config"] = config_path
    horizon = controller.lifecycle_timing(profile)["horizon"]
    now = time.time()
    if not now < cutoff_unix <= now + horizon:
        raise SystemExit("--cutoff-unix must be in the future and inside the reviewed horizon")
    if admitted_unix is None and action in ("adopt", "admit"):
        raise SystemExit("--admitted-unix is required for adopt and fresh admit")
    if admitted_unix is not None and action not in ("adopt", "admit", "qualification"):
        raise SystemExit("--admitted-unix is only for adopt and adopted qualification")
    # Same subset and planner input as reviewed_launch: foreign rows stay in the signed file.
    if len(controller.owned_instances(snapshot, cfg)) != (0 if action == "admit" else 2):
        raise SystemExit(
            "fresh financial snapshot must hold exactly the two owned instances (none for admit)"
        )
    plan = controller.admission_plan(
        profile,
        data("evidence"),
        data("authorization"),
        dict(snapshot, instances=[]),
        data("offers"),
        qualification=True,
        priced_seconds=None if admitted_unix is None else cutoff_unix - admitted_unix,
    )
    plan_path = output.with_name(output.stem + "-plan.json")
    with plan_path.open("x") as text:
        json.dump(plan, text)
    paths["plan"] = plan_path
    record = {
        k: v
        for k, v in base.items()
        if k not in ("preparation_only", "remaining", "admitted_unix", "role_inputs")
    }
    if ROLE_FILES <= set(files):
        inputs = base["role_inputs"]
        record["roles"] = {
            role: {
                "name": role,
                "job": inputs["job"],
                "objects": inputs["objects"],
                "hotkey": json.loads(files["join_" + role].read_bytes())["hotkey"],
                "join": "join_" + role,
                "seed_context": "seed_context_" + role,
            }
            for role in ("h0", "h1")
        }
    elif "roles" not in record:
        raise SystemExit("qualification base needs --file join_h0/h1 and seed_context_h0/h1")
    owner = Keypair(owner_seed.read_bytes())
    record.update(
        action=action,
        owner=owner.ss58,
        parent_pid=parent_pid,
        cutoff_unix=cutoff_unix,
        quota=controller.execution_quota(profile),
        files={name: {"path": str(p), "sha256": digest(p)} for name, p in sorted(paths.items())},
    )
    if admitted_unix is not None:
        record["admitted_unix"] = admitted_unix
    receipt = envelope_v2.seal(
        owner,
        "Receipt",
        run_id,
        {"w": 0, "commit_hash": sha256_hex(canonicalize(record)), "received_round": beacon},
        beacon + EXPIRY_ROUNDS,
    )
    with output.open("xb") as binary:
        os.fchmod(binary.fileno(), 0o600)
        binary.write(canonicalize({"record": record, "receipt": receipt}))
    checked = controller.reviewed_launch(output, owner.ss58, beacon, action)
    return {
        "review": str(output),
        "review_sha256": digest(output),
        "plan_sha256": digest(plan_path),
        "action": action,
        "owner": owner.ss58,
        "beacon": beacon,
        "parent_pid": parent_pid,
        "cutoff_unix": cutoff_unix,
        "admitted_unix": admitted_unix,
        "adoption": checked["adoption"],
        "_checked": checked,
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base", type=Path, required=True, help="prepared private/review.json")
    parser.add_argument("--file", action="append", default=[], help="name=/abs/path")
    parser.add_argument("--action", required=True, choices=("admit", "adopt", "qualification"))
    parser.add_argument("--parent-pid", type=int, required=True)
    parser.add_argument("--cutoff-unix", type=int, required=True)
    parser.add_argument("--admitted-unix", type=int)
    parser.add_argument("--owner-seed", type=Path, required=True)
    parser.add_argument("--run-id", required=True)
    parser.add_argument("--beacon", type=int, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--provider-key-file", type=Path)
    parser.add_argument("--run-dir", type=Path, help="warm, fresh GET, seal and launch at once")
    parser.add_argument("--source-message", help="authorization source for the fresh GET")
    parser.add_argument("--live", action="store_true")
    args = parser.parse_args()
    os.umask(0o077)
    if args.beacon < 1:
        raise SystemExit("--beacon must be a positive drand round")
    check_parent(args.parent_pid)
    files = parse_files(args.file, args.action)
    base = json.loads(args.base.read_bytes())["record"]

    def seal() -> Any:
        result = build(
            base,
            files,
            args.action,
            args.parent_pid,
            args.cutoff_unix,
            args.admitted_unix,
            args.owner_seed,
            args.run_id,
            args.beacon,
            args.output.resolve(),
            args.provider_key_file,
        )
        checked = result.pop("_checked")
        print(json.dumps(result, sort_keys=True), flush=True)
        return checked

    if args.run_dir is None:
        seal()
        return
    if not args.source_message or args.provider_key_file is None:
        raise SystemExit("--run-dir needs --source-message and --provider-key-file")
    if FRESH_GET & set(files):
        raise SystemExit("--run-dir takes its financial files from its own fresh GET")
    controller = load_controller(Path(base["tree"]))
    # One invocation: immutable seed proof first, then fresh GET, seal + preflight, launch.
    with controller.seed_genesis_scope():
        warm_seed_genesis(controller, base, files)
        fresh = load_runtime().financial(
            Path(base["tree"]),
            Path(base["files"]["config"]["path"]),
            args.provider_key_file,
            args.source_message,
            args.output.resolve().with_name(args.output.stem + "-financial"),
        )
        files.update({n: Path(p) for n, p in (item.split("=", 1) for item in fresh["files"])})
        launched = controller.launch_action(args.action, seal(), args.run_dir, live=args.live)
    print(json.dumps(launched, sort_keys=True, default=str), flush=True)


if __name__ == "__main__":
    main()
