"""`hypertrain-miner <command> --config miner.toml` (config file + HYPERTRAIN_MINER_* env only).

Operator machine: `key create|list|revoke` (the coldkey never leaves it).
GPU host: `install`, `register`, `run`, `status`, `update` (seedless: server key + grant only).
"""

from __future__ import annotations

import argparse
import json
import logging
import os
import re
import sys
import time
from dataclasses import asdict
from pathlib import Path
from typing import Any

import httpx
import numpy as np
import numpy.typing as npt

from hypertrain.data.shards import ShardSamples, ShardSetManifest, verify_shards
from hypertrain.datasets.fetch import prefetch
from hypertrain.datasets.shards16 import (
    U16,
    ShardSet16Manifest,
    U16ShardSamples,
    shard_name16,
    verify_shards16,
)
from hypertrain.miner.coldkey_ops import bond_terms, cosign_join, escrow_lock, prepare_join
from hypertrain.miner.core import (
    ChallengeExpired,
    Miner,
    MinerConfig,
    MinerError,
    NetworkMiner,
    canonical,
)
from hypertrain.miner.dispute_watch import add_watch_args, run_follow
from hypertrain.miner.keys import create_key, list_keys, revoke_key
from hypertrain.miner.update import Updater, UpdateRejected
from hypertrain.protocol.hashing import MerkleTree
from hypertrain.protocol.messages import RunManifest
from hypertrain.trainer.loop import SampleFn

DATA_BASES_ENV = "HYPERTRAIN_DATA_BASES"
DEFAULT_DATA_BASE = "http://185.8.107.43:18980/"
MAX_DATASET_MANIFEST_BYTES = 64 << 20


def _data_bases() -> list[str]:
    raw = os.environ.get(DATA_BASES_ENV) or DEFAULT_DATA_BASE
    bases = [b.strip() for b in raw.split(",") if b.strip()]
    if not bases or any(not b.startswith(("http://", "https://")) for b in bases):
        raise MinerError(f"{DATA_BASES_ENV} must be comma-separated http(s) base URLs")
    return bases


def _dataset_manifest(data_dir: Path, bases: list[str]) -> ShardSet16Manifest:
    """Local manifest.json, else fetched (bounded) from the first base that serves it."""
    path = data_dir / "manifest.json"
    if not path.is_file():
        errors = []
        for base in bases:
            try:
                with httpx.stream("GET", base.rstrip("/") + "/manifest.json", timeout=60) as r:
                    r.raise_for_status()
                    body = bytearray()
                    for chunk in r.iter_bytes():
                        body += chunk
                        if len(body) > MAX_DATASET_MANIFEST_BYTES:
                            raise MinerError("dataset manifest exceeds 64 MiB")
            except (httpx.HTTPError, MinerError) as e:
                errors.append(f"{base}: {e}")
                continue
            data_dir.mkdir(parents=True, exist_ok=True)
            tmp = path.with_suffix(".tmp")
            tmp.write_bytes(body)
            tmp.replace(path)
            break
        else:
            raise MinerError(f"dataset manifest unavailable: {errors[-3:]}")
    return ShardSet16Manifest.from_json(path.read_text())


class _RemoteUnits:
    """Samples from sha256-verified units fetched by HTTP Range on first use, cached under
    data_dir/units/<unit_sha256>. ponytail: on-demand per unit (8 units per H=512 slot); add a
    whole-assignment prefetch once the miner core passes its sample ids ahead of training."""

    def __init__(self, data_dir: Path, m: ShardSet16Manifest, bases: list[str]) -> None:
        self.m, self.bases, self.cache = m, bases, data_dir / "units"
        self.maps: dict[int, npt.NDArray[np.uint16]] = {}

    def row(self, i: int) -> npt.NDArray[np.uint32]:
        if not 0 <= i < self.m.n_samples:
            raise IndexError(i)
        u, k = divmod(i, self.m.unit)
        if u not in self.maps:
            for _ in prefetch(self.m, [u], self.cache, self.bases, streams=1, depth=1):
                pass
            path = self.cache / self.m.unit_sha256s[u]
            self.maps[u] = np.memmap(path, dtype=U16, mode="r").reshape(-1, self.m.seq_len + 1)
        return self.maps[u][k].astype(np.uint32)


def _u16_sampler(data_dir: Path, manifest: RunManifest) -> SampleFn:
    local = (data_dir / shard_name16(0)).is_file()
    bases = [] if local else _data_bases()
    m = _dataset_manifest(data_dir, bases)
    ds = manifest.dataset
    if (m.merkle_root, m.shard_sha256_root, m.n_samples, m.unit_sha256_root) != (
        ds.merkle_root,
        ds.shard_sha256_root,
        ds.n_samples,
        ds.unit_sha256_root,
    ):
        raise MinerError(
            "local shards are not the run's dataset (merkle/shard root/n_samples/unit root)"
        )
    if not local:
        # Untrusted manifest bytes: the unit list must hash to the signed root, and the units
        # must tile n_samples exactly; each fetched unit is then checked against its listed sha.
        if (
            len(m.unit_sha256s) * m.unit != m.n_samples
            or m.samples_per_shard % m.unit
            or MerkleTree([bytes.fromhex(h) for h in m.unit_sha256s]).root.hex()
            != ds.unit_sha256_root
            or MerkleTree([bytes.fromhex(h) for h in m.shard_sha256s]).root.hex()
            != ds.shard_sha256_root
        ):
            raise MinerError("remote dataset manifest unit/shard lists differ from signed roots")
        return _RemoteUnits(data_dir, m, bases).row
    errors = verify_shards16(data_dir, m)
    if errors:
        raise MinerError(f"shard verification failed: {errors[:3]}")
    samples = U16ShardSamples(data_dir, m.n_shards, m.samples_per_shard, m.seq_len)
    return samples.row


def shard_sampler(data_dir: Path, manifest: RunManifest) -> SampleFn:
    if manifest.dataset.sample_format.startswith("u16"):
        return _u16_sampler(data_dir, manifest)
    m = ShardSetManifest.from_json((data_dir / "manifest.json").read_text())
    ds = manifest.dataset
    if (m.merkle_root, m.shard_sha256_root, m.n_samples) != (
        ds.merkle_root,
        ds.shard_sha256_root,
        ds.n_samples,
    ):
        raise MinerError("local shards are not the run's dataset (merkle/shard root/n_samples)")
    errors = verify_shards(data_dir, m)
    if errors:
        raise MinerError(f"shard verification failed: {errors[:3]}")
    samples = ShardSamples(data_dir, m.n_shards, m.samples_per_shard, m.seq_len)

    def get(i: int) -> npt.NDArray[np.uint32]:
        return samples.row(i)

    return get


def _miner(cfg: MinerConfig) -> Miner:
    if cfg.data_dir is None:
        raise MinerError("data_dir is required")
    client = httpx.Client()
    status = client.get(f"{cfg.api.rstrip('/')}/v1/runs/{cfg.run_id}") if cfg.run_id else None
    if status is not None and status.status_code == 200:
        manifest = RunManifest.model_validate(status.json()["manifest_envelope"]["body"])
        return Miner(cfg, client, shard_sampler(cfg.data_dir, manifest))
    lazy: dict[str, SampleFn] = {}

    def get(i: int) -> npt.NDArray[np.uint32]:
        return lazy["get"](i)

    miner = Miner(cfg, client, get)
    assert cfg.data_dir is not None
    lazy["get"] = shard_sampler(cfg.data_dir, miner.manifest)
    return miner


UPDATE_ROOT_ENV = "HYPERTRAIN_UPDATE_ROOT"
MANIFEST_URL_ENV = "HYPERTRAIN_UPDATE_MANIFEST_URL"
_MNEMONIC = re.compile(r"[a-z]+(\s+[a-z]+){11,23}")


def _update_root() -> Path:
    return Path(os.environ.get(UPDATE_ROOT_ENV) or Path.home() / ".hypertrain" / "miner")


def _manifest_url(arg: str | None) -> str:
    url = arg or os.environ.get(MANIFEST_URL_ENV)
    if not url:
        raise MinerError(f"no update manifest URL (--manifest-url or {MANIFEST_URL_ENV})")
    return url


def _refuse_coldkey(path: Path) -> None:
    """Refuse anything that looks like a coldkey or mnemonic; never echo its content."""
    if "cold" in path.name.lower() or "mnemonic" in path.name.lower():
        raise MinerError(f"{path.name}: refusing a coldkey/mnemonic file on a miner host")
    text = path.read_bytes()[:4096].decode(errors="replace").strip().lower()
    if _MNEMONIC.fullmatch(text) or "mnemonic" in text or "secretphrase" in text:
        raise MinerError(f"{path.name}: looks like a coldkey/mnemonic; refusing it")


def _install(bundle: Path, out: Path, api: str | None, workdir: Path | None) -> dict[str, Any]:
    for f in bundle.iterdir():
        if f.is_file():
            _refuse_coldkey(f)
    key, grant = bundle / "server.key", bundle / "grant.json"
    if not key.is_file() or not grant.is_file():
        raise MinerError(f"{bundle}: bundle needs server.key and grant.json")
    key.chmod(0o600)
    grant_doc = json.loads(grant.read_text())["grant"]
    if out.exists():
        raise MinerError(f"{out}: already exists; remove it first")
    lines = {
        "api": api or "http://127.0.0.1:8000",
        "keyfile": str(key.resolve()),
        "workdir": str((workdir or out.parent / "work").resolve()),
        "state_source": "state",
        "image_digest": "sha256:" + "0" * 64,
    }
    if grant_doc.get("run_id", "*") != "*":
        lines["run_id"] = grant_doc["run_id"]
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text("".join(f"{k} = {json.dumps(v)}\n" for k, v in lines.items()))
    out.chmod(0o600)
    return {
        "config": str(out),
        "server_pub": grant_doc["server_pub"],
        "key_id": grant_doc["key_id"],
    }


def _register(cfg: MinerConfig, args: argparse.Namespace) -> dict[str, Any]:
    if args.coldkey is not None:
        if args.request_id is None or args.expiry is None:
            raise MinerError("register --coldkey requires --request-id and --expiry")
        with httpx.Client(timeout=120, follow_redirects=False) as client:
            return dict(NetworkMiner(cfg, client).join(args.coldkey, args.request_id, args.expiry))
    grant = cfg.keyfile.parent / "grant.json"
    if not grant.is_file():
        raise MinerError("no grant.json next to the keyfile; pass --coldkey (operator machine)")
    r = httpx.post(
        cfg.api.rstrip("/") + "/v2/server-keys", content=grant.read_bytes(), timeout=30
    )  # idempotent: re-posting an identical grant returns the stored record
    if r.status_code >= 400:
        raise MinerError(f"register: {r.status_code} {r.text}")
    return dict(r.json())


def _status(cfg: MinerConfig) -> dict[str, Any]:
    from hypertrain import __version__

    updater = Updater(_update_root())
    pending = updater.pending
    return {
        "version": __version__,
        "api": cfg.api,
        "run_id": cfg.run_id,
        "keyfile": str(cfg.keyfile),
        "pending_update": pending.version if pending else None,
        "changeover_at": pending.changeover_at if pending else None,
        "staged": updater.staged(),
    }


def _update(args: argparse.Namespace) -> dict[str, Any]:
    updater = Updater(_update_root())
    try:
        manifest = updater.poll(_manifest_url(args.manifest_url))
    except UpdateRejected as error:
        if updater.pending is None:
            return {"update": None, "reason": str(error)}
        manifest = updater.pending
    out: dict[str, Any] = {"update": manifest.version, "changeover_at": manifest.changeover_at}
    if args.apply:
        updater.stage()
        out["staged"] = True
        if updater.due(time.time()):
            out["current"] = str(updater.swap())
    return out


def _reexec(target: Path, argv: list[str]) -> None:
    exe = target / "venv" / "bin" / "hypertrain-miner"
    logging.info("update swapped; re-exec %s", exe)
    os.execv(exe, [str(exe), *argv])  # noqa: S606


def _run_updating(
    miner: Miner, rounds: int, start: int | None, manifest_url: str | None, argv: list[str]
) -> list[tuple[int, str]]:
    """Round-at-a-time: each completed round is journaled by Miner (= the checkpoint)."""
    updater = Updater(_update_root())
    out: list[tuple[int, str]] = []
    w = start
    for _ in range(rounds):
        try:
            updater.poll(manifest_url) if manifest_url else None
            if updater.pending is not None:
                updater.stage()
        except (UpdateRejected, httpx.HTTPError) as error:
            logging.warning("update check: %s", error)
        now = time.time()
        if updater.due(now):
            target = updater.swap()
            rest = ["--rounds", str(rounds - len(out))]
            if w is not None:
                rest += ["--start", str(w)]
            _reexec(target, [*argv, *rest])
        if updater.must_halt(now):
            raise MinerError("past changeover + grace on an old version; refusing new rounds")
        done = miner.run(1, w)
        out += done
        w = done[-1][0] + 1
    return out


def main(argv: list[str] | None = None) -> int:
    p = argparse.ArgumentParser(prog="hypertrain-miner")
    sub = p.add_subparsers(dest="command", required=True)
    for name in ("join", "run"):
        s = sub.add_parser(name)
        s.add_argument("--config", type=Path)
        if name == "run":
            s.add_argument("--rounds", type=int, default=1)
            s.add_argument("--start", type=int)
            s.add_argument("--auto-update", action="store_true")
            s.add_argument("--manifest-url")
    key = sub.add_parser("key", help="operator machine: server keys").add_subparsers(
        dest="key_command", required=True
    )
    s = key.add_parser("create")
    s.add_argument("--coldkey", type=Path, required=True)
    s.add_argument("--server", required=True)
    s.add_argument("--out", type=Path)
    s.add_argument("--api", required=True)
    s.add_argument("--run-id", default="*")
    s = key.add_parser("list")
    s.add_argument("--coldkey-ss58", required=True)
    s.add_argument("--api")
    s = key.add_parser("revoke")
    s.add_argument("key_id")
    s.add_argument("--coldkey", type=Path, required=True)
    s.add_argument("--api", required=True)
    s = sub.add_parser("install", help="GPU host: write miner.toml from a key bundle")
    s.add_argument("--bundle", type=Path, required=True)
    s.add_argument("--config", type=Path, required=True)
    s.add_argument("--api")
    s.add_argument("--workdir", type=Path)
    s = sub.add_parser("register", help="join-v2; coldkey-free when a grant exists")
    s.add_argument("--config", type=Path)
    s.add_argument("--coldkey", type=Path)
    s.add_argument("--request-id")
    s.add_argument("--expiry", type=int)
    s = sub.add_parser("status")
    s.add_argument("--config", type=Path)
    s = sub.add_parser("update")
    g = s.add_mutually_exclusive_group(required=True)
    g.add_argument("--check", action="store_true")
    g.add_argument("--apply", action="store_true")
    s.add_argument("--manifest-url")
    s = sub.add_parser(
        "join-v2-cosign", help="operator machine: co-sign and submit a prepared join"
    )
    s.add_argument("--request", type=Path, required=True)
    s.add_argument("--coldkey", type=Path, required=True)
    s.add_argument("--api", required=True)
    s = sub.add_parser("escrow-lock", help="operator machine: lock admission collateral")
    s.add_argument("--api", required=True)
    s.add_argument("--run-id", required=True)
    s.add_argument("--coldkey", type=Path, required=True)
    s.add_argument("--hotkey-ss58", required=True)
    s.add_argument("--units", type=int, required=True)
    s.add_argument("--origin", action="append", required=True)
    s.add_argument("--expiry", type=int)
    s.add_argument("--dry-run", action="store_true")
    s = sub.add_parser("escrow-bond", help="operator machine: read the terms of the admission bond")
    s.add_argument("--api", required=True)
    s.add_argument("--run-id", required=True)
    s.add_argument("--coldkey", type=Path, required=True)
    s.add_argument("--hotkey-ss58", required=True)
    for name in ("join-v2", "status-v2", "probe-v2", "proof-v2", "run-v2", "watch"):
        s = sub.add_parser(name)
        s.add_argument("--config", type=Path)
        if name == "join-v2":
            g = s.add_mutually_exclusive_group(required=True)
            g.add_argument("--coldkey", type=Path)
            g.add_argument("--prepare", type=Path)
            s.add_argument("--coldkey-ss58")
            s.add_argument("--request-id", required=True)
            s.add_argument("--expiry", type=int, required=True)
        if name in ("probe-v2", "run-v2", "watch"):
            s.add_argument("--job", type=Path)
        if name == "run-v2":
            s.add_argument("--round", type=int)
        if name == "probe-v2":
            s.add_argument("--challenge", type=Path)
        if name == "proof-v2":
            s.add_argument("--proof", type=Path, required=True)
        if name == "watch":
            s.add_argument("--timeout", type=float, default=30)
        if name in ("watch", "run-v2"):
            add_watch_args(s)
    args = p.parse_args(argv)
    logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(message)s")
    try:
        if args.command in ("join-v2-cosign", "escrow-lock", "escrow-bond") or (
            args.command == "join-v2" and args.prepare is not None
        ):
            try:
                with httpx.Client(timeout=120, follow_redirects=False) as client:
                    if args.command == "join-v2":
                        if args.coldkey_ss58 is None:
                            raise MinerError("join-v2 --prepare requires --coldkey-ss58")
                        result = prepare_join(
                            client,
                            MinerConfig.load(args.config),
                            args.coldkey_ss58,
                            args.request_id,
                            args.expiry,
                            args.prepare,
                        )
                    elif args.command == "join-v2-cosign":
                        result = cosign_join(client, args.api, args.request, args.coldkey)
                    elif args.command == "escrow-bond":
                        result = bond_terms(
                            client, args.api, args.run_id, args.coldkey, args.hotkey_ss58
                        )
                    else:
                        result = escrow_lock(
                            client,
                            args.api,
                            args.run_id,
                            args.coldkey,
                            args.hotkey_ss58,
                            args.units,
                            args.origin,
                            args.expiry,
                            args.dry_run,
                        )
            except httpx.HTTPStatusError as error:
                print(
                    json.dumps(
                        {"status": error.response.status_code, "body": error.response.text},
                        sort_keys=True,
                    )
                )
                return 1
            except (OSError, ValueError, KeyError, TypeError, httpx.InvalidURL) as error:
                raise MinerError(
                    f"{args.command}: invalid input or response, or file unavailable"
                ) from error
            except httpx.RequestError as error:
                raise MinerError(f"{args.command}: HTTP request failed") from error
            print(json.dumps(result, sort_keys=True))
            return 0
        local = _local_command(args)
        if local is not None:
            print(json.dumps(local, sort_keys=True))
            return 0
        cfg = MinerConfig.load(args.config)
        if args.command in ("register", "status"):
            result = _register(cfg, args) if args.command == "register" else _status(cfg)
            print(json.dumps(result, sort_keys=True))
            return 0
        if getattr(args, "follow", False) or getattr(args, "watch", False):
            return run_follow(cfg, args)
        if args.command not in ("join", "run"):
            with httpx.Client(timeout=120, follow_redirects=False) as client:
                network = NetworkMiner(cfg, client)
                match args.command:
                    case "join-v2":
                        result = network.join(args.coldkey, args.request_id, args.expiry)
                    case "status-v2":
                        result = network.status()
                    case "probe-v2":
                        if (args.job is None) != (args.challenge is None):
                            raise MinerError("probe-v2 takes --job and --challenge, or neither")
                        if args.job is None:
                            result = network.probe_trial()
                        else:
                            result = network.probe(args.job, args.challenge)
                    case "proof-v2":
                        result = network.proof(args.proof)
                    case "run-v2":
                        if args.job is not None:
                            result = network.launch(args.job)
                        elif args.round is not None:
                            result = {"status": network.run_round(args.round)}
                        else:
                            raise MinerError("run-v2 requires --round or --job")
                    case "watch":
                        if args.job is None:
                            raise MinerError("watch requires --job unless --follow")
                        result = {"processed": network.watch(args.job, args.timeout)}
                    case _:
                        raise MinerError("unknown v2 command")
                print(json.dumps(result, sort_keys=True))
                return 0
        miner = _miner(cfg)
        if args.command == "join":
            # ponytail: admission is the operator's PUT /roster (no public join route in
            # contract v1); we emit a hotkey-signed request carrying the hardware self-check.
            req = {"run_id": miner.run_id, "hotkey": miner.kp.ss58, "hardware": asdict(miner.hw)}
            sig = miner.kp.sign(b"hypertrain/1|Join|" + canonical(req)).hex()
            print(json.dumps({**req, "sig": sig}, sort_keys=True))
            return 0
        if args.auto_update or args.manifest_url:
            base = ["run", "--auto-update"]
            base += ["--config", str(args.config)] if args.config else []
            base += ["--manifest-url", args.manifest_url] if args.manifest_url else []
            url = args.manifest_url or os.environ.get(MANIFEST_URL_ENV)
            results = _run_updating(miner, args.rounds, args.start, url, base)
        else:
            results = miner.run(args.rounds, args.start)
    except ChallengeExpired as error:
        # Stranded until the operator reseeds the admission: a status of its own, not a failure
        # to retry.
        logging.error("%s: %s", type(error).__name__, error)
        return 3
    except (MinerError, UpdateRejected) as error:
        logging.error("%s: %s", type(error).__name__, error)
        return 2
    print(json.dumps({"rounds": [{"w": w, "status": s} for w, s in results]}))
    return 0 if all(s == "UPLOADED" for _, s in results) else 1


def _local_command(args: argparse.Namespace) -> Any:
    """Commands that need no miner.toml; None means 'not one of them'."""
    match args.command:
        case "key":
            match args.key_command:
                case "create":
                    bundle = create_key(
                        args.coldkey, args.server, args.api, out_dir=args.out, run_id=args.run_id
                    )
                    return {"bundle": str(bundle)}
                case "list":
                    return list_keys(args.coldkey_ss58, args.api)
                case _:
                    return revoke_key(args.coldkey, args.key_id, args.api)
        case "install":
            return _install(args.bundle, args.config, args.api, args.workdir)
        case "update":
            return _update(args)
    return None


if __name__ == "__main__":
    sys.exit(main())
