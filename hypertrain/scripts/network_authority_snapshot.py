"""Admitted OD MLM finalized-admission export; no reconstructed historical intake."""

from __future__ import annotations

import argparse
import json
import os
import shutil
import sqlite3
import stat
import time
from collections.abc import Iterator, Mapping
from contextlib import closing, contextmanager
from pathlib import Path, PurePosixPath
from typing import Never

from pydantic import BaseModel, ConfigDict, Field

from hypertrain.beacon.core import BeaconRound, FixtureBeacon
from hypertrain.challenge.admission import Admission
from hypertrain.challenge.app import Config, create_app
from hypertrain.data.store import LocalFSStore
from hypertrain.data.trial_assignment import trial_assignment_hash, trial_samples
from hypertrain.ledger import Params
from hypertrain.ledger.escrow_v2 import EscrowV2, FinalityEvidence, authority_message
from hypertrain.miner.island_launch import validate_artifacts
from hypertrain.protocol import envelope_v2
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import Keypair, decode_hotkey, verify
from hypertrain.protocol.messages_v2 import (
    AdmissionPolicyV2,
    DisputePolicyV2,
    EconomicsPolicyV2,
    IslandJobV1,
    JoinChallenge,
    RunManifestV2,
    TestGenesis,
    WorkProof,
    WorkScreenV2,
)
from hypertrain.protocol.relay_messages import RelayRegistryV1

RUN = "587fc10ec7aeee92e235d39c9332c763e16cde05e5e13a8562204c700adce325"
MISSING = ("original dual-signed JoinRequest envelopes", "original signed EscrowLock envelopes")
MAX_FILES, MAX_BYTES, MAX_FILE = 8192, 1 << 30, 16 << 20


class FileEntry(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True)
    sha256: str = Field(pattern=r"^[0-9a-f]{64}$")
    size: int = Field(strict=True, ge=0, le=MAX_FILE)


class Bundle(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True)
    version: int = Field(default=1, strict=True, ge=1, le=1)
    run_id: str = Field(default=RUN, pattern="^" + RUN + "$")
    capability: str = Field(
        default="restored-finalized-admission", pattern="^restored-finalized-admission$"
    )
    missing_provenance: tuple[str, ...] = MISSING
    files: dict[str, FileEntry] = Field(min_length=1, max_length=MAX_FILES)
    public_roles: dict[str, str]
    manifest_envelope_sha256: str
    genesis_envelope_sha256: str
    beacon_round: int = Field(strict=True, ge=1)


def _files(root: Path) -> dict[str, FileEntry]:
    """Bound regular files; no symlinks, sockets, path escapes or SQLite sidecars."""
    if root.is_symlink():
        raise ValueError("snapshot root symlink")
    entries: dict[str, FileEntry] = {}
    total = 0
    for path in sorted(root.rglob("*")):
        if path.is_symlink():
            raise ValueError("snapshot symlink")
        if path.is_dir():
            continue
        relative = path.relative_to(root).as_posix()
        if relative == "bundle.json" or relative in ("challenge.db-wal", "challenge.db-shm"):
            continue
        if not stat.S_ISREG(path.stat().st_mode) or path.stat().st_size > MAX_FILE:
            raise ValueError("snapshot file size/type")
        total += path.stat().st_size
        if len(entries) >= MAX_FILES or total > MAX_BYTES:
            raise ValueError("snapshot total bound")
        entries[relative] = FileEntry(
            sha256=sha256_hex(path.read_bytes()), size=path.stat().st_size
        )
    return entries


def _unavailable(*args: str | int | FinalityEvidence) -> Never:
    raise ValueError("snapshot validation cannot authorize live settlement")


class _StatusCoordinator(Keypair):
    """Public-only identity for status; historical validation has no signing capability."""

    def __init__(self, public: str) -> None:
        self.ss58 = public
        self.public = decode_hotkey(public)

    def sign(self, message: bytes) -> Never:
        raise ValueError("status-only coordinator cannot sign")


def _authority(root: Path) -> Bundle:
    """Validate original signatures/assignments and saved rank artifacts, never train."""
    from hypertrain.auditor.replay import AnchorCache, pack_state, unpack_state
    from hypertrain.protocol.hashing import MerkleTree
    from hypertrain.trainer.compress import state_hash
    from hypertrain.trainer.config import TrainConfig
    from hypertrain.trainer.model import init_params

    if not (root / "objects").is_dir() or not (root / "challenge.db").is_file():
        raise ValueError("critical snapshot storage missing")
    objects = LocalFSStore(root / "objects")
    with (
        closing(sqlite3.connect((root / "challenge.db").as_uri() + "?mode=ro", uri=True)) as source,
        closing(sqlite3.connect(":memory:", isolation_level=None)) as db,
    ):
        source.backup(db)
        db.row_factory = sqlite3.Row
        runs = db.execute("SELECT * FROM runs").fetchall()
        if len(runs) != 1:
            raise ValueError("snapshot run count")
        manifest = RunManifestV2.model_validate_json(runs[0]["manifest"])
        signed = envelope_v2.parse_envelope(runs[0]["envelope"])
        if (
            manifest.run_id() != RUN
            or runs[0]["run_id"] != RUN
            or signed.type != "RunManifestV2"
            or signed.run_id != RUN
            or signed.body != manifest.body()
            or not envelope_v2.verify_envelope(runs[0]["envelope"])
        ):
            raise ValueError("foreign original manifest")
        m, layout = manifest.training.model, manifest.training.reference_spec.layout
        if (
            m.arch != "od-encoder"
            or m.od is None
            or m.od.objective != "mlm"
            or m.param_count != 116288
            or layout.n_gpus != 2
            or layout.dp_size != 2
            or layout.ep_size != 1
            or not layout.zero1
        ):
            raise ValueError("OD MLM N2 profile mismatch")
        policy = EconomicsPolicyV2.model_validate_json(
            objects.get(manifest.network.economics_policy_hash)
        )
        admission = AdmissionPolicyV2.model_validate_json(
            objects.get(manifest.network.admission_policy_hash)
        )
        dispute = DisputePolicyV2.model_validate_json(
            objects.get(manifest.network.dispute_policy_hash)
        )
        registry = RelayRegistryV1.model_validate_json(
            objects.get(manifest.network.relay_registry_hash)
        )
        for field in ("aggregation_policy_hash", "audit_policy_hash"):
            objects.get(getattr(manifest.network, field))
        if (
            policy.ledger_mode != "test"
            or db.execute("SELECT COUNT(*) FROM rounds").fetchone()[0]
            or db.execute("SELECT COUNT(*) FROM disputes_v2").fetchone()[0]
            or db.execute("SELECT COUNT(*) FROM audit_leases_v2").fetchone()[0]
        ):
            raise ValueError("snapshot is not admission-only test authority")
        if db.execute(
            "SELECT COUNT(*) FROM records_v2 WHERE kind NOT IN ('dataset','execution-backend')"
        ).fetchone()[0]:
            raise ValueError("snapshot contains production state")
        beacons = db.execute("SELECT * FROM beacon ORDER BY round").fetchall()
        for b in beacons:
            fixture = FixtureBeacon(current=b["round"]).get(b["round"])
            if b["bls"] or (b["signature"], b["randomness"]) != (
                fixture.signature,
                fixture.randomness,
            ):
                raise ValueError("fixture beacon mapping differs")
        for pin in db.execute("SELECT * FROM beacon_pins_v2"):
            b = db.execute("SELECT * FROM beacon WHERE round=?", (pin["pinned_round"],)).fetchone()
            if (
                b is None
                or pin["run_id"] != RUN
                or pin["signature"] != b["signature"]
                or pin["signature_hash"] != sha256_hex(bytes.fromhex(b["signature"]))
                or pin["paused"]
            ):
                raise ValueError("accepted beacon pin differs")
        genesis_rows = db.execute("SELECT envelope FROM accepted_v2").fetchall()
        if len(genesis_rows) != 1:
            raise ValueError("original genesis intake missing")
        genesis_raw = genesis_rows[0][0]
        env = envelope_v2.parse_envelope(genesis_raw)
        genesis = TestGenesis.model_validate(env.body)
        event = db.execute("SELECT request FROM escrow_events WHERE kind='TEST_GENESIS'").fetchone()
        if (
            env.type != "TestGenesis"
            or env.run_id != RUN
            or env.signer != manifest.training.coord_pubkey
            or not envelope_v2.verify_envelope(genesis_raw)
            or not verify(
                decode_hotkey(env.signer),
                authority_message(genesis),
                bytes.fromhex(genesis.authority_sig),
            )
            or event is None
            or genesis.model_dump(mode="json", exclude={"authority_sig"}) != json.loads(event[0])
        ):
            raise ValueError("original signed genesis differs")
        escrow = EscrowV2(
            db,
            manifest,
            policy,
            auditors=frozenset(manifest.training.auditors),
            replay_finality=_unavailable,
            settlement=_unavailable,
            owner_of=_unavailable,
            assignment_of=_unavailable,
        )
        escrow.verify()
        current_round = beacons[-1]["round"]

        def beacon(number: int) -> BeaconRound:
            row = db.execute("SELECT * FROM beacon WHERE round=?", (number,)).fetchone()
            if row is None:
                raise ValueError("current verified beacon missing")
            return BeaconRound(number, row["signature"], row["randomness"], False)

        def no_stage(c: JoinChallenge, samples: tuple[int, ...], epoch: int) -> Never:
            raise ValueError("status validation cannot execute a new reference")

        current_admission = Admission(
            escrow,
            admission,
            _StatusCoordinator(manifest.training.coord_pubkey),
            beacon=beacon,
            stage_reference=no_stage,
            objects=objects,
            ip_secret=b"",
            qualified=lambda screen: False,
            conservative_bound=lambda: policy.ledger_mode == "test",
            backend="cpu",
        )
        rows = db.execute("SELECT * FROM admissions_v2 ORDER BY hotkey").fetchall()
        if len(rows) != 4 or len({r["coldkey"] for r in rows}) != 4:
            raise ValueError("four original identities required")
        roles = {"coord": manifest.training.coord_pubkey, "owner": signed.signer}
        for prefix, keys in (
            ("auditor", manifest.training.auditors),
            ("referee", dispute.referees),
            ("relay", [k.pubkey for spec in registry.specs for k in spec.pubkeys]),
        ):
            roles.update({f"{prefix}-{i}": key for i, key in enumerate(keys)})
        dataset = json.loads(
            db.execute(
                "SELECT data FROM records_v2 WHERE kind='dataset' AND id='inputs'"
            ).fetchone()[0]
        )
        samples_raw = objects.get(dataset["samples_hash"])
        proofs = json.loads(objects.get(dataset["proofs_hash"]))
        width = (m.seq_len + 1) * 2
        if (
            len(samples_raw) != manifest.training.dataset.n_samples * width
            or len(proofs) != manifest.training.dataset.n_samples
        ):
            raise ValueError("signed dataset dimensions differ")
        tree = MerkleTree([samples_raw[i : i + width] for i in range(0, len(samples_raw), width)])
        if tree.root.hex() != manifest.training.dataset.merkle_root:
            raise ValueError("signed dataset root differs")
        theta = init_params(TrainConfig.from_manifest_v2(manifest).model)
        if state_hash(theta) != manifest.training.init_state_hash:
            raise ValueError("signed genesis theta differs")
        if (
            db.execute("SELECT COUNT(*) FROM admission_trials").fetchone()[0] != 48
            or db.execute("SELECT COUNT(*) FROM admission_trial_results").fetchone()[0] != 48
        ):
            raise ValueError("unexpected trial authority rows")
        for i, row in enumerate(rows):
            roles.update({f"hot-{i}": row["hotkey"], f"cold-{i}": row["coldkey"]})
            status = current_admission.status(row["hotkey"], now=current_round)
            history = db.execute(
                "SELECT admission_id,coldkey FROM admission_history WHERE hotkey=?",
                (row["hotkey"],),
            ).fetchone()
            if (
                not status.eligible
                or status.shadow_only
                or history is None
                or tuple(history) != (row["admission_id"], row["coldkey"])
            ):
                raise ValueError("current funded admission is ineligible at restored beacon")
            if (
                row["run_id"] != RUN
                or row["state"] != "ACTIVE"
                or row["clean_count"] != 12
                or row["canary_blocks"] != "1,2,3"
                or row["strikes"]
                or row["pending_dispute"]
                or row["policy_hash"] != admission.digest()
                or escrow.locked(row["admission_id"], row["coldkey"])[0] < 1000
            ):
                raise ValueError("original admission eligibility differs")
            trials = db.execute(
                "SELECT t.*,r.challenge,r.reference,r.proof,r.screen,r.outcome "
                "FROM admission_trials t JOIN admission_trial_results r USING(epoch) "
                "WHERE admission_id=?",
                (row["admission_id"],),
            ).fetchall()
            if len(trials) != 12:
                raise ValueError("twelve original trials required")
            for trial in trials:
                c = JoinChallenge.model_validate_json(trial["challenge"])
                ref = WorkProof.model_validate_json(trial["reference"])
                ch_raw = db.execute(
                    "SELECT receipt FROM admission_reservations WHERE reservation=?",
                    (f"challenge|{row['admission_id']}|{c.nonce}",),
                ).fetchone()[0]
                final_raw = db.execute(
                    "SELECT receipt FROM admission_reservations WHERE reservation=?",
                    (f"trial-final|{row['admission_id']}|{trial['epoch']}",),
                ).fetchone()[0]
                envs = [
                    envelope_v2.parse_envelope(raw)
                    for raw in (ch_raw, trial["proof"], trial["screen"], final_raw)
                ]
                for raw, envelope, expected_type, signer in zip(
                    (ch_raw, trial["proof"], trial["screen"], final_raw),
                    envs,
                    ("JoinChallenge", "WorkProof", "WorkScreenV2", "Finalize"),
                    (roles["coord"], row["hotkey"], row["hotkey"], roles["coord"]),
                    strict=True,
                ):
                    if (
                        envelope.type != expected_type
                        or envelope.run_id != RUN
                        or envelope.signer != signer
                        or not envelope_v2.verify_envelope(raw)
                        or envelope.exp_drand < trial["finalized_beacon"]
                    ):
                        raise ValueError("original trial signature/expiry differs")
                screen = WorkScreenV2.model_validate(envs[2].body)
                b = db.execute("SELECT * FROM beacon WHERE round=?", (c.seed_beacon,)).fetchone()
                ids = trial_samples(
                    manifest,
                    row["admission_id"],
                    c.nonce,
                    BeaconRound(b["round"], b["signature"], b["randomness"], False),
                )
                if (
                    trial["outcome"] != "MATCH"
                    or ref != WorkProof.model_validate(envs[1].body)
                    or c.body() != envs[0].body
                    or c.admission_id != row["admission_id"]
                    or c.nonce != trial["nonce"]
                    or c.manifest_hash != RUN
                    or c.policy_hash != admission.digest()
                    or c.assignment_hash != trial_assignment_hash(manifest, trial["epoch"], ids)
                    or ref.challenge_hash != c.digest()
                    or ref.digest() != trial["evidence_hash"]
                    or screen.challenge_hash != c.digest()
                    or screen.nonce != c.nonce
                    or screen.admission_id != row["admission_id"]
                    or screen.layout != layout
                ):
                    raise ValueError("original reference/challenge binding differs")
                directory = root / "trials-v2" / c.nonce
                inputs = {
                    name: (directory / name).read_bytes()
                    for name in ("start_state", "ef_in", "v0", "samples", "sample_proofs")
                }
                genesis_anchor = AnchorCache().genesis(manifest, row["hotkey"], theta)
                if (
                    inputs["start_state"] != pack_state(genesis_anchor.theta, genesis_anchor.state)
                    or inputs["ef_in"] != pack_state(genesis_anchor.ef)
                    or inputs["v0"] != pack_state({})
                ):
                    raise ValueError("saved trial genesis anchor differs")
                if (
                    not trial["received_beacon"] <= trial["finalized_beacon"] <= c.deadline_beacon
                    or screen.image_digest != manifest.training.reference_spec.image_digest
                    or screen.artifact_hashes != [a.sha256 for a in ref.artifact_refs]
                ):
                    raise ValueError("original trial deadline/screen differs")
                if inputs["samples"] != b"".join(
                    samples_raw[x * width : (x + 1) * width] for x in ids
                ) or json.loads(inputs["sample_proofs"]) != [proofs[x] for x in ids]:
                    raise ValueError("saved trial assignment inputs differ")
                job = IslandJobV1(
                    job_version=1,
                    run_id=RUN,
                    w=trial["epoch"],
                    manifest=manifest,
                    sample_ids=list(ids),
                    global_step0=0,
                    start_state_sha256=sha256_hex(inputs["start_state"]),
                    ef_in_sha256=sha256_hex(inputs["ef_in"]),
                    v0_sha256=sha256_hex(inputs["v0"]),
                    object_paths={k: k for k in inputs},
                    deadline=manifest.training.beacon.genesis_time + (c.deadline_beacon - 1) * 3,
                )
                artifacts = validate_artifacts(job, directory / "published")
                if list(artifacts.ranks) != screen.rank_results:
                    raise ValueError("original screen rank artifacts differ")
                for path, artifact in zip(
                    (artifacts.state, artifacts.ef, artifacts.delta, artifacts.leaves),
                    ref.artifact_refs,
                    strict=True,
                ):
                    if (
                        path.read_bytes() != objects.get(artifact.sha256)
                        or path.stat().st_size != artifact.size
                    ):
                        raise ValueError("original artifact reference differs")
                final = envs[3].body
                if (
                    final["w"] != trial["epoch"]
                    or final["included"] != [row["hotkey"]]
                    or final["entitlements_root"] != ref.digest()
                    or final["final_theta_hash_w1"]
                    != state_hash(unpack_state(artifacts.state.read_bytes())[0])
                ):
                    raise ValueError("original final reference differs")
        manifest_raw = runs[0]["envelope"]
        if isinstance(manifest_raw, str):
            manifest_raw = manifest_raw.encode()
        if isinstance(genesis_raw, str):
            genesis_raw = genesis_raw.encode()
        return Bundle(
            files=_files(root),
            public_roles=roles,
            manifest_envelope_sha256=sha256_hex(manifest_raw),
            genesis_envelope_sha256=sha256_hex(genesis_raw),
            beacon_round=beacons[-1]["round"],
        )


def export(source: Path, destination: Path) -> str:
    """Export exact snapshot, return root-admittable index digest; no secrets included."""
    source = source.resolve(strict=True)
    before = _files(source)
    _authority(source)
    if destination.exists():
        raise ValueError("export destination exists")
    destination.mkdir(mode=0o700, parents=True)
    with (
        closing(sqlite3.connect((source / "challenge.db").as_uri() + "?mode=ro", uri=True)) as src,
        closing(sqlite3.connect(destination / "challenge.db")) as dst,
    ):
        src.backup(dst)
        original_database = sha256_hex(dst.serialize())
    (destination / "challenge.db").chmod(0o600)
    for relative in before:
        if relative != "challenge.db":
            target = destination / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source / relative, target)
    bundle = _authority(destination)
    if _files(source) != before:
        raise ValueError("source changed during export")
    with (
        closing(sqlite3.connect((source / "challenge.db").as_uri() + "?mode=ro", uri=True)) as src,
        closing(sqlite3.connect(":memory:")) as check,
    ):
        src.backup(check)
        if sha256_hex(check.serialize()) != original_database:
            raise ValueError("source database changed during export")
    raw = canonicalize(bundle.model_dump(mode="json"))
    (destination / "bundle.json").write_bytes(raw)
    return sha256_hex(raw)


def validate(bundle_dir: Path, admitted_hash: str) -> Bundle:
    """Fail before restore for missing, foreign or modified admitted package bytes."""
    bundle_dir = bundle_dir.resolve(strict=True)
    if (bundle_dir / "bundle.json").is_symlink():
        raise ValueError("bundle index symlink")
    raw = (bundle_dir / "bundle.json").read_bytes()
    if len(raw) > 2 << 20 or sha256_hex(raw) != admitted_hash:
        raise ValueError("admitted bundle hash differs")
    bundle = Bundle.model_validate_json(raw)
    for relative in bundle.files:
        path = PurePosixPath(relative)
        if path.is_absolute() or ".." in path.parts or "\\" in relative or str(path) != relative:
            raise ValueError("unsafe indexed path")
    if (
        bundle.missing_provenance != MISSING
        or _files(bundle_dir) != bundle.files
        or _authority(bundle_dir) != bundle
    ):
        raise ValueError("admitted authority/index differs")
    return bundle


def verify_secrets(secrets: Path, bundle: Bundle) -> None:
    """Require supplied original role identities; never mint or export replacement keys."""
    if (
        secrets.is_symlink()
        or secrets.stat().st_mode & 0o077
        or secrets.stat().st_uid != os.getuid()
    ):
        raise ValueError("secret directory must be owned private")
    for role, public in bundle.public_roles.items():
        path = secrets / (role + ".seed")
        if (
            path.is_symlink()
            or not stat.S_ISREG(path.stat().st_mode)
            or path.stat().st_mode & 0o077
            or path.stat().st_uid != os.getuid()
            or path.stat().st_size != 32
        ):
            raise ValueError("role secret must be owned regular0600 raw32")
        if Keypair(path.read_bytes()).ss58 != public:
            raise ValueError("role public identity differs")
    token = secrets / "admin.token"
    if (
        token.is_symlink()
        or not stat.S_ISREG(token.stat().st_mode)
        or token.stat().st_mode & 0o077
        or token.stat().st_uid != os.getuid()
        or not 1 <= token.stat().st_size <= 4096
        or not token.read_bytes().strip()
    ):
        raise ValueError("private admin token required")


@contextmanager
def bootstrap(
    bundle_dir: Path, admitted_hash: str, state_dir: Path, secrets: Path, master_url: str
) -> Iterator[Config]:
    """Configure existing actual service with original-role files and bounded fixture clock."""
    bundle = validate(bundle_dir, admitted_hash)
    verify_secrets(secrets, bundle)
    token = secrets / "admin.token"
    if state_dir.exists():
        raise ValueError("bootstrap state already exists")
    state_dir.mkdir(mode=0o700, parents=True)
    for relative in bundle.files:
        target = state_dir / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(bundle_dir / relative, target)
    (state_dir / "challenge.db").chmod(0o600)
    with closing(sqlite3.connect(state_dir / "challenge.db")) as db:
        manifest = RunManifestV2.model_validate_json(
            db.execute("SELECT manifest FROM runs").fetchone()[0]
        )
    ledger = state_dir / "ledger/journal.jsonl"
    params = (
        Params(**json.loads(ledger.read_text().splitlines()[0])["params"])
        if ledger.exists()
        else Params(
            "hypertrain",
            manifest.training.beacon.genesis_time,
            4320,
            manifest.training.budget.epochs_per_round,
            manifest.training.verify.E_vest_rounds,
        )
    )
    historical = manifest.training.beacon.genesis_time + (bundle.beacon_round - 1) * 3
    baseline = time.monotonic()
    old_time = time.time
    offset = old_time() - historical
    clock_dir = state_dir / "fixture-clock"
    clock_dir.mkdir()
    (clock_dir / "sitecustomize.py").write_text(
        "import os,time\n_real=time.time\n"
        "_offset=float(os.environ['HT_FIXTURE_CLOCK_OFFSET'])\n"
        "time.time=lambda:_real()-_offset\n"
    )
    previous = {k: os.environ.get(k) for k in ("PYTHONPATH", "HT_FIXTURE_CLOCK_OFFSET")}
    os.environ["PYTHONPATH"] = (
        str(clock_dir) + os.pathsep + previous["PYTHONPATH"]
        if previous["PYTHONPATH"]
        else str(clock_dir)
    )
    os.environ["HT_FIXTURE_CLOCK_OFFSET"] = str(offset)
    time.time = lambda: historical + time.monotonic() - baseline
    try:
        yield Config(
            params.challenge_slug,
            state_dir,
            master_url,
            None,
            token,
            None,
            secrets / "coord.seed",
            bundle.public_roles["owner"],
            params,
        )
    finally:
        time.time = old_time
        for key, value in previous.items():
            if value is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = value


def fixture_beacon(payload: Mapping[str, int | str]) -> BeaconRound:
    """Explicit deterministic CPU test authority; never advertise BLS verification."""
    number = int(payload["round"])
    expected = FixtureBeacon(current=number).get(number)
    if payload["signature"] != expected.signature or payload["randomness"] != expected.randomness:
        raise ValueError("fixture beacon payload differs")
    return expected


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("operation", choices=("export", "validate", "serve"))
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--destination", type=Path)
    parser.add_argument("--admitted-hash")
    parser.add_argument("--secrets", type=Path)
    parser.add_argument("--master-url", default="https://master.test")
    parser.add_argument("--port", type=int, default=8080)
    parser.add_argument("--host", default="127.0.0.1")
    args = parser.parse_args()
    if args.operation == "export":
        if args.destination is None:
            parser.error("export requires --destination")
        print(export(args.source, args.destination))
    elif args.operation == "validate":
        if args.admitted_hash is None:
            parser.error("validate requires --admitted-hash")
        print(validate(args.source, args.admitted_hash).model_dump_json(exclude={"files"}))
    else:
        if args.admitted_hash is None or args.destination is None or args.secrets is None:
            parser.error("serve requires --admitted-hash --destination --secrets")
        import uvicorn

        with bootstrap(
            args.source, args.admitted_hash, args.destination, args.secrets, args.master_url
        ) as config:
            uvicorn.run(
                create_app(config, verify_beacon=fixture_beacon), host=args.host, port=args.port
            )


if __name__ == "__main__":
    main()
