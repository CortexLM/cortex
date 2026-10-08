"""Checkpoints: provisional until every audit in their round range closes, then final.

Layout of a checkpoint directory:
  model.safetensors  theta (float32, sorted names)
  lineage.json       signed global event tapes for rounds [w_start, w_end] (+ regional tapes)
  MANIFEST.json      signed {status, run_id, rounds, files: {name: sha256}, theta_hash,
                     included hotkeys, license, dataset attribution}
"""

from __future__ import annotations

import json
from collections.abc import Iterable, Mapping, Sequence
from pathlib import Path
from typing import Any

import numpy as np
from safetensors.numpy import load as st_load
from safetensors.numpy import save as st_save

from hypertrain.aggregator.core import (
    AggregatorError,
    tape_key,
    th,
    verify_tape,
)
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import KeyError_, Keypair, decode_hotkey, verify

MANIFEST = "MANIFEST.json"
MODEL = "model.safetensors"
LINEAGE = "lineage.json"
CKPT_DOMAIN = b"hypertrain/1|Checkpoint|"


class CheckpointError(AggregatorError):
    pass


def _msg(body: Mapping[str, Any]) -> bytes:
    return CKPT_DOMAIN + sha256_hex(canonicalize(body, allow_float=False)).encode()


def _write_manifest(d: Path, kp: Keypair, body: Mapping[str, Any]) -> None:
    doc = {"body": dict(body), "signer": kp.ss58, "sig": kp.sign(_msg(body)).hex()}
    tmp = d / (MANIFEST + ".tmp")
    tmp.write_bytes(canonicalize(doc, allow_float=False))
    tmp.replace(d / MANIFEST)


def _included(tapes: Sequence[Mapping[str, Any]], regional: Mapping[str, Any]) -> list[str]:
    ids: set[str] = set()
    for t in [*tapes, *regional.values()]:
        ids.update(i["id"] for i in t["body"]["inputs"] if not i["id"].startswith("region:"))
    return sorted(ids, key=lambda s: s.encode("utf-8"))


def write_checkpoint(
    d: Path,
    kp: Keypair,
    theta: Mapping[str, Any],
    tapes: Sequence[Mapping[str, Any]],
    *,
    license: str,
    dataset: Mapping[str, str],
    regional_tapes: Sequence[Mapping[str, Any]] = (),
    journal_anchor: Mapping[str, Any] | None = None,
) -> dict[str, Any]:
    """Write a PROVISIONAL checkpoint for the consecutive global tapes `tapes`.

    `journal_anchor` (Aggregator.journal.anchor) is signed into the manifest; pass it back as
    Aggregator(anchor=...) on restart so a truncated aggregator journal fails closed."""
    if not tapes:
        raise CheckpointError("a checkpoint needs at least one global tape")
    d.mkdir(parents=True, exist_ok=False)
    model = st_save({n: np.ascontiguousarray(theta[n], dtype="<f4") for n in theta})
    reg = {tape_key(t): dict(t) for t in regional_tapes}
    lineage = canonicalize({"tapes": [dict(t) for t in tapes], "regional": reg}, allow_float=False)
    (d / MODEL).write_bytes(model)
    (d / LINEAGE).write_bytes(lineage)
    body = {
        "v": "ht-ckpt-v1",
        "status": "provisional",
        "run_id": tapes[0]["body"]["run_id"],
        "rounds": [tapes[0]["body"]["w"], tapes[-1]["body"]["w"]],
        "theta_hash": th({n: np.asarray(theta[n], dtype=np.float32) for n in theta}),
        "included": _included(tapes, reg),
        "license": license,
        "dataset": dict(dataset),
        "files": {MODEL: sha256_hex(model), LINEAGE: sha256_hex(lineage)},
    }
    if journal_anchor is not None:
        body["journal_anchor"] = dict(journal_anchor)
    _write_manifest(d, kp, body)
    return body


def read_manifest(d: Path) -> dict[str, Any]:
    doc: dict[str, Any] = json.loads((d / MANIFEST).read_bytes())
    return doc


def finalize_checkpoint(
    d: Path,
    kp: Keypair,
    open_audit_rounds: Iterable[int],
    journal_anchor: Mapping[str, Any] | None = None,
) -> dict[str, Any]:
    """Mark final; refuses while any audit for a round in the checkpoint range is still open.
    `journal_anchor` (taken after finalize_round) re-anchors the journal head past the finals."""
    errors = verify_checkpoint(d, require_final=False, signer=kp.ss58)
    if errors:
        raise CheckpointError("refusing to finalize an invalid checkpoint: " + "; ".join(errors))
    body = read_manifest(d)["body"]
    lo, hi = body["rounds"]
    blocking = sorted({w for w in open_audit_rounds if lo <= w <= hi})
    if blocking:
        raise CheckpointError(
            f"audits still open for rounds {blocking}; checkpoint stays provisional"
        )
    body = {**body, "status": "final"}
    if journal_anchor is not None:
        body["journal_anchor"] = dict(journal_anchor)
    _write_manifest(d, kp, body)
    return body


def verify_checkpoint(
    d: Path, *, require_final: bool = True, signer: str | None = None
) -> list[str]:
    """Return a list of problems (empty = valid): sha256 files, signature, lineage, provenance."""
    try:
        doc = read_manifest(d)
        body, who, sig = doc["body"], doc["signer"], bytes.fromhex(doc["sig"])
        if set(doc) != {"body", "signer", "sig"}:
            return ["manifest has unexpected fields"]
        if not verify(decode_hotkey(who), _msg(body), sig):
            return ["manifest signature does not verify"]
    except (OSError, ValueError, KeyError, TypeError, KeyError_) as exc:
        return [f"manifest unreadable: {type(exc).__name__}"]
    errs: list[str] = []
    if signer is not None and who != signer:
        errs.append("manifest signer is not the expected coordinator")
    if require_final and body.get("status") != "final":
        errs.append(f"checkpoint status is {body.get('status')!r}, not final")
    files = body.get("files", {})
    on_disk = sorted(p.name for p in d.iterdir() if p.name != MANIFEST)
    if on_disk != sorted(files):
        errs.append(f"files on disk {on_disk} differ from manifest {sorted(files)}")
    for name, want in files.items():
        p = d / name
        if not p.is_file() or sha256_hex(p.read_bytes()) != want:
            errs.append(f"{name}: sha256 mismatch")
    if errs:
        return errs
    if not str(body.get("license", "")).strip():
        errs.append("license missing")
    ds = body.get("dataset", {})
    for k in ("name", "license", "attribution", "merkle_root"):
        if not str(ds.get(k, "")).strip():
            errs.append(f"dataset {k} missing")
    try:
        lin = json.loads((d / LINEAGE).read_bytes())
        tapes, reg = lin["tapes"], lin["regional"]
        theta = st_load((d / MODEL).read_bytes())
    except (ValueError, KeyError, TypeError) as exc:
        return [*errs, f"lineage/model unreadable: {type(exc).__name__}"]
    errs += _check_lineage(body, who, tapes, reg, theta)
    return errs


def _check_lineage(
    body: Mapping[str, Any],
    who: str,
    tapes: Sequence[Mapping[str, Any]],
    reg: Mapping[str, Any],
    theta: Mapping[str, Any],
) -> list[str]:
    errs: list[str] = []
    if not tapes:
        return ["lineage has no tapes"]
    for h, t in reg.items():
        if not verify_tape(t, who) or tape_key(t) != h or t["body"]["kind"] != "regional":
            errs.append(f"regional tape {h[:12]} invalid")
    for j, t in enumerate(tapes):
        b = t["body"]
        if not verify_tape(t, who) or b["kind"] != "global" or b["run_id"] != body["run_id"]:
            errs.append(f"tape {j} signature/kind/run_id invalid")
            continue
        if j and (
            b["w"] != tapes[j - 1]["body"]["w"] + 1
            or b["prev_state"] != tapes[j - 1]["body"]["out_state"]
        ):
            errs.append(f"tape {j} does not chain from tape {j - 1}")
        for hashes in b.get("regional_tapes", {}).values():
            errs += [f"tape {j}: regional tape {h[:12]} missing" for h in hashes if h not in reg]
    if [tapes[0]["body"]["w"], tapes[-1]["body"]["w"]] != list(body["rounds"]):
        errs.append("manifest rounds differ from lineage")
    if tapes[-1]["body"]["theta_hash"] != body["theta_hash"]:
        errs.append("last tape theta_hash differs from manifest")
    if th(theta) != body["theta_hash"]:
        errs.append("model.safetensors theta hash differs from manifest")
    if _included(tapes, reg) != body["included"]:
        errs.append("included hotkeys differ from lineage inputs")
    return errs
