"""TEST-only CUDA backend authority for a TEST run, without the two-host Vast qualification.

A CUDA run needs a `qualification` record in the challenge's root store before its manifest is
POSTed (`store._backend_v2`; a run created without one is frozen to cpu for good). The production
record comes from `_bootstrap_backend_v2`: two Vast lifecycle hosts, owner-reviewed `config.json`
and `result.json` files and a bitwise `compare`. None of that exists for a single ssh-reachable
GPU host, so this tool installs the record `_backend_v2` and `qualify` actually read, signed by the
TEST owner, with stand-in evidence:

    {run_id, backend: "cuda", reference_hash, layout_hash, harness: <marker>}

What is TEST-ONLY, exactly:
- the record carries no engine `Result`, no config, no summaries and no two-host comparison, so it
  does NOT prove that the GPU image reproduces the reference bitwise on any machine;
- `build` refuses a manifest whose economics policy is not `ledger_mode == "test"`, a manifest not
  signed by the owner seed given, and `install` refuses a challenge whose owner hotkey differs;
- `install` refuses once the run exists or its backend is frozen, and never overwrites a record.

    build   STAGE OWNER_SEED     on the machine that built STAGE (owner seed by path, never printed)
            -> STAGE/qualification-test.json (public: evidence, authority hash, signed Receipt)
    install STAGE [DATA]         inside the TEST challenge:
            docker exec -i hypertrain-test-challenge python -I - install /data/<stage> < this file
            (HYPERTRAIN_OWNER_HOTKEY comes from the container environment; DATA defaults to /data)
"""

from __future__ import annotations

import json
import os
import sqlite3
import sys
from pathlib import Path
from typing import Any

from hypertrain.data.store import LocalFSStore
from hypertrain.miner.core import load_keyfile
from hypertrain.protocol import envelope_v2
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages_v2 import EconomicsPolicyV2, RunManifestV2

FILE = "qualification-test.json"
MARKER = (
    "TEST-ONLY stand-in: owner-signed, no engine Result, no two-host comparison "
    "(scripts/ops/s100_test/cuda_qualification.py)"
)


def _manifest(stage: Path) -> tuple[RunManifestV2, envelope_v2.EnvelopeV2, bytes]:
    raw = (stage / "manifest-envelope.json").read_bytes()
    env = envelope_v2.parse_envelope(raw)
    manifest = RunManifestV2.model_validate(env.body)
    manifest.validate_run_id(env.run_id)
    if not envelope_v2.verify_envelope(json.loads(raw)):
        raise SystemExit("the manifest envelope signature does not verify")
    return manifest, env, raw


def evidence_for(manifest: RunManifestV2) -> dict[str, Any]:
    ref = manifest.training.reference_spec
    return {
        "run_id": manifest.run_id(),
        "backend": "cuda",
        "reference_hash": sha256_hex(canonicalize(ref.model_dump(mode="json"))),
        "layout_hash": ref.layout.model_dump_json(),
        "harness": MARKER,
    }


def _refuse_non_test(stage: Path, manifest: RunManifestV2) -> None:
    economics = EconomicsPolicyV2.model_validate_json(
        (stage / "policy-economics.json").read_bytes()
    )
    if economics.digest() != manifest.network.economics_policy_hash:
        raise SystemExit("policy-economics.json is not the manifest's economics policy")
    if economics.ledger_mode != "test":
        raise SystemExit("not a TEST run: ledger_mode is " + economics.ledger_mode)


def build(stage: Path, owner_seed: Path) -> dict[str, Any]:
    manifest, env, _ = _manifest(stage)
    _refuse_non_test(stage, manifest)
    owner = load_keyfile(owner_seed)
    if env.signer != owner.ss58:
        raise SystemExit("the manifest is not signed by this owner seed")
    evidence = evidence_for(manifest)
    authority = sha256_hex(canonicalize(evidence))
    receipt = envelope_v2.seal(
        owner,
        "Receipt",
        manifest.run_id(),
        {"w": 0, "commit_hash": authority, "received_round": 1},
        1 << 40,
    )
    record = {"evidence": evidence, "authority_hash": authority, "receipt": receipt}
    (stage / FILE).write_bytes(canonicalize(record))
    return {"run_id": manifest.run_id(), "authority_hash": authority, "owner": owner.ss58}


def install(stage: Path, data: Path, owner_hotkey: str) -> dict[str, Any]:
    manifest, _, _ = _manifest(stage)
    _refuse_non_test(stage, manifest)
    record = json.loads((stage / FILE).read_bytes())
    evidence = evidence_for(manifest)
    authority = sha256_hex(canonicalize(evidence))
    receipt = envelope_v2.parse_envelope(record["receipt"])
    if (
        record["evidence"] != evidence
        or record["authority_hash"] != authority
        or receipt.type != "Receipt"
        or receipt.run_id != manifest.run_id()
        or receipt.signer != owner_hotkey
        or receipt.body["commit_hash"] != authority
        or not envelope_v2.verify_envelope(record["receipt"])
    ):
        raise SystemExit("the qualification file is not the one this manifest and owner allow")
    run_id, image = manifest.run_id(), manifest.training.reference_spec.image_digest
    row = canonicalize({**evidence, "authority_hash": authority, "receipt": record["receipt"]})
    db = sqlite3.connect(data / "challenge.db", timeout=30, isolation_level=None)
    try:
        db.execute("BEGIN IMMEDIATE")
        if (
            db.execute("SELECT 1 FROM runs WHERE run_id=?", (run_id,)).fetchone()
            or (data / "runs" / run_id).exists()
        ):
            raise SystemExit(
                "the run already exists: its backend is frozen, a new run_id is needed"
            )
        if db.execute(
            "SELECT 1 FROM records_v2 WHERE run_id=? AND kind='execution-backend'", (run_id,)
        ).fetchone():
            raise SystemExit("the run's execution backend is already frozen")
        old = db.execute(
            "SELECT data FROM records_v2 WHERE run_id=? AND kind='qualification' AND id=?",
            (run_id, image),
        ).fetchone()
        if old is not None and old[0].encode() != row:
            raise SystemExit("a different qualification record is already installed")
        if LocalFSStore(data / "objects").put(canonicalize(evidence)) != authority:
            raise SystemExit("the object store hashed the evidence differently")
        if old is None:
            db.execute(
                "INSERT INTO records_v2 VALUES(?,?,?,?)",
                (run_id, "qualification", image, row.decode()),
            )
        db.execute("COMMIT")
    except BaseException:
        if db.in_transaction:
            db.execute("ROLLBACK")
        raise
    finally:
        db.close()
    return {"run_id": run_id, "image_digest": image, "authority_hash": authority, "test_only": True}


def main(argv: list[str]) -> int:
    match argv:
        case ["build", stage, owner_seed]:
            out = build(Path(stage), Path(owner_seed))
        case ["install", stage, *rest] if len(rest) <= 1:
            owner = os.environ.get("HYPERTRAIN_OWNER_HOTKEY")
            if not owner:
                raise SystemExit(
                    "HYPERTRAIN_OWNER_HOTKEY is not set: run this inside the challenge"
                )
            out = install(Path(stage), Path(rest[0]) if rest else Path("/data"), owner)
        case _:
            raise SystemExit("usage: build STAGE OWNER_SEED | install STAGE [DATA]")
    print(json.dumps(out, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
