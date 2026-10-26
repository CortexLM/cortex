"""Contract-2 dataset objects: `samples` (all rows in id order) and `proofs` (one hex path per
sample id), the pair `PUT /v2/runs/{run_id}/admin/dataset` links and `stage_trial_v2` slices.

ponytail: register copies the objects into the service's local per-run store, because the
object route only accepts a subnet-registered, run-admitted hotkey signature; switch to the
signed upload route once the operator holds such a hotkey.
"""

from __future__ import annotations

import os
import re
import sys
from pathlib import Path
from typing import Any

import httpx

from hypertrain.data.store import LocalFSStore, Store
from hypertrain.datasets.shards16 import (
    ShardSet16Manifest,
    U16ShardSamples,
    shard_name16,
    verify_shards16,
)
from hypertrain.protocol.hashing import MerkleTree
from hypertrain.protocol.jcs import canonicalize


class PinError(ValueError):
    pass


def pin_objects(data_dir: Path, store: Store, holdout_dir: Path | None = None) -> dict[str, Any]:
    m = ShardSet16Manifest.from_json((data_dir / "manifest.json").read_text())
    errs = verify_shards16(data_dir, m)
    if errs:
        raise PinError(
            f"{data_dir}: shards differ from manifest merkle_root {m.merkle_root}: {errs}"
        )
    tree = MerkleTree(U16ShardSamples(data_dir, m.n_shards, m.samples_per_shard, m.seq_len))
    if tree.root.hex() != m.merkle_root:
        raise PinError(f"{data_dir}: sample tree root {tree.root.hex()} != merkle_root")
    fragment: dict[str, Any] = {
        "merkle_root": m.merkle_root,
        "depth": m.depth,
        "n_samples": m.n_samples,
        "sample_format": m.sample_format,
        "shard_uri_template": m.shard_uri_template,
        "shard_sha256_root": m.shard_sha256_root,
        "assign_unit": m.unit,
        "unit_sha256_root": m.unit_sha256_root,
    }
    if holdout_dir is not None:
        hold = ShardSet16Manifest.from_json((holdout_dir / "manifest.json").read_text())
        if hold.seq_len != m.seq_len:
            raise PinError(f"holdout seq_len {hold.seq_len} != dataset seq_len {m.seq_len}")
        if errs := verify_shards16(holdout_dir, hold):
            raise PinError(f"{holdout_dir}: holdout shards differ from its manifest: {errs}")
        fragment["holdout_commit"] = hold.merkle_root
    # Shards are row-major u16 LE in id order, so their concatenation is the samples object.
    # ponytail: each object is built whole in memory, one at a time (the service loads them whole
    # too); stream into the store if a full-size set outgrows host RAM.
    samples_hash = store.put(
        b"".join((data_dir / shard_name16(i)).read_bytes() for i in range(m.n_shards))
    )
    proofs_hash = store.put(
        canonicalize([[p.hex() for p in tree.proof(i)] for i in range(m.n_samples)])
    )
    return {
        "samples_hash": samples_hash,
        "proofs_hash": proofs_hash,
        "n_samples": m.n_samples,
        "merkle_root": m.merkle_root,
        "unit_sha256_root": m.unit_sha256_root,
        "shard_sha256_root": m.shard_sha256_root,
        "dataset_fragment": fragment,
    }


def register(
    client: httpx.Client,
    run_id: str,
    token: str,
    objects: Path,
    state_dir: Path,
    samples_hash: str,
    proofs_hash: str,
) -> int:
    """Copy both objects into state_dir/runs/<run_id>/objects, then link them over HTTP."""
    run = state_dir / "runs" / run_id
    if not re.fullmatch(r"[0-9a-f]{64}", run_id) or not (run / "challenge.db").is_file():
        raise PinError(
            f"{run}/challenge.db not found: --state-dir must be the service's state dir and "
            "the run a hosted sub-run (the first run lives in the root store, refused here)"
        )
    # A symlinked run, objects or prefix dir would pass the checks above and write the root store.
    root = state_dir.resolve()
    prefixes = [run / "objects" / h[:2] for h in (samples_hash, proofs_hash)]
    for p in (run.resolve(), *(d.resolve() for d in (run / "objects", *prefixes))):
        if root / "runs" not in p.parents or p == root / "objects":
            raise PinError(f"{run}: resolves to {p}, outside {root}/runs or the root store")
    # New prefix dirs take the caller's uid; a foreign one would block the service's own writes.
    if (run / "objects").stat().st_uid != os.geteuid():
        raise PinError(f"{run}/objects belongs to another uid: run register as the service user")
    if not objects.is_dir():
        raise PinError(f"{objects}: not a directory")
    # httpx echoes an illegal header value in its error, so never let one reach the header.
    if not re.fullmatch(r"[\x21-\x7e]+", token):
        raise PinError("admin token file must hold one line of printable ASCII")
    src, dst = LocalFSStore(objects), LocalFSStore(run / "objects")
    for sha in (samples_hash, proofs_hash):
        dst.put(src.get(sha))  # put skips an object the run already has
    r = client.put(
        f"/v2/runs/{run_id}/admin/dataset",
        json={"samples_hash": samples_hash, "proofs_hash": proofs_hash},
        headers={"Authorization": f"Bearer {token}"},
    )
    if not r.is_success:
        print(f"HTTP {r.status_code}: {r.text}", file=sys.stderr)
        return 1
    print(r.text)
    return 0
