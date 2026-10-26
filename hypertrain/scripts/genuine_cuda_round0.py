"""Accepted genuine network on the CUDA-backed store: graduation, round 0, leases, open dispute."""

from __future__ import annotations

import json
import shutil
import types
from pathlib import Path

import genuine_cuda_index as gi

SKIP_SNAPSHOT = {"challenge.db-wal", "challenge.db-shm"}
SKIP_DIRS = {"trials-v2"}


class Stop(Exception):
    pass


class StopAfter:
    def __init__(self, client, suffix: str, before: bool = False) -> None:
        self._client, self._suffix, self._before = client, suffix, before

    def __getattr__(self, name):
        return getattr(self._client, name)

    def post(self, url, *args, **kwargs):
        if url.endswith(self._suffix) and self._before:
            raise Stop(url)
        response = self._client.post(url, *args, **kwargs)
        if url.endswith(self._suffix):
            assert response.status_code == 200, response.text
            raise Stop(url)
        return response


def force_cuda():
    from hypertrain.miner import island_launch

    original = island_launch.launch_island

    def launch(job, directory, *, backend, **kwargs):
        return original(job, directory, backend="cuda", **kwargs)

    island_launch.launch_island = launch
    return original


def host_driver() -> str:
    import subprocess

    out = subprocess.run(
        ["nvidia-smi", "--query-gpu=driver_version", "--format=csv,noheader"],  # noqa: S607
        capture_output=True,
        text=True,
        check=True,
    ).stdout.split()
    assert len(set(out)) == 1, out
    return out[0]


def run_until(network, fn, suffix: str, before: bool = False) -> None:
    client = network.client
    network.client = StopAfter(client, suffix, before)
    try:
        fn(network)
        raise AssertionError("flow did not reach " + suffix)
    except Stop:
        pass
    finally:
        network.client = client


def leases(network, s) -> dict:
    run_id = network.manifest.run_id()
    held, done, counter = None, 0, 100
    while True:
        rows = network.store._db.execute(
            "SELECT state FROM audit_leases_v2 WHERE run_id=?", (run_id,)
        ).fetchall()
        if all(r["state"] == "completed" for r in rows if r["state"] != "running") and (
            held is not None and done == len(rows) - 1
        ):
            return held
        auditor = next(a for a in s.AUDITORS if held is None or a.ss58 != held["auditor"])
        counter += 1
        response = network.client.post(
            network.url + "/worker/lease",
            json=network.signed(
                auditor,
                "Receipt",
                {"w": 0, "commit_hash": f"{counter:064x}", "received_round": network.now},
            ),
        )
        assert response.status_code == 200, response.text
        job = response.json()["body"]
        row = network.store._db.execute(
            "SELECT * FROM audit_leases_v2 WHERE id=?", (job["job_id"],)
        ).fetchone()
        if row["hotkey"] == s.HOT[0].ss58:
            held = {
                "id": row["id"],
                "expires": row["expires"],
                "auditor": auditor.ss58,
                "nonce": job["lease_nonce"],
            }
            continue
        execute = network.signed(
            auditor,
            "Receipt",
            {"w": 0, "commit_hash": job["lease_nonce"], "received_round": network.now},
        )
        response = network.client.post(
            network.url + "/worker/jobs/" + job["job_id"] + "/execute", json=execute
        )
        assert response.status_code == 200, response.text
        response = network.client.post(
            network.url + "/worker/jobs/" + job["job_id"] + "/complete",
            json=network.signed(auditor, "ReplayVerdict", response.json()["verdict"]),
        )
        assert response.status_code == 200, response.text
        done += 1


def generate(out: Path, prelim: Path) -> Path:
    from hypertrain.ledger.escrow_v2 import genesis_allocation_hash, genesis_origin_id
    from hypertrain.miner.admission import sign_join
    from hypertrain.protocol.envelope_v2 import seal
    from hypertrain.protocol.hashing import sha256_hex
    from hypertrain.protocol.jcs import canonicalize
    from hypertrain.protocol.messages_v2 import EscrowLock, HardwareHint

    s = gi.service()
    e2e = gi.load("genuine_e2e_flows", "tests/e2e/test_service_network_v2_e2e.py")
    original_launch = force_cuda()
    work = out / "generator"
    context = s.genuine_network(prelim, work, accepted=False)
    network, *_ = next(context)
    run_id = network.manifest.run_id()
    alloc = [(k.ss58, 10000, 1) for k in s.COLD]
    network.origin_ids = [
        genesis_origin_id(run_id, genesis_allocation_hash(alloc), i) for i in range(4)
    ]
    e2e._graduate_four(network)
    signed = network.signed
    layout = network.manifest.training.reference_spec.layout
    driver = host_driver()

    def accept_signed(key, kind, body, *args, **kwargs):
        if kind == "AcceptV2":
            body = {**body, "n_gpus": layout.n_gpus, "driver_version": driver}
        return signed(key, kind, body, *args, **kwargs)

    network.signed = accept_signed
    run_until(network, e2e._live_checkpoint, "/admin/rounds/0/audits")
    network.signed = signed
    held = leases(network, s)
    rotated = types.SimpleNamespace(**{k: getattr(s, k) for k in dir(s) if not k.startswith("__")})
    rotated.HOT, rotated.COLD = s.HOT[1:] + s.HOT[:1], s.COLD[1:] + s.COLD[:1]
    e2e.fixtures = rotated
    try:
        run_until(network, e2e._funded_dispute_watch, "/referee", before=True)
    finally:
        e2e.fixtures = s
    (dispute_id,) = network.store._db.execute("SELECT id FROM disputes_v2").fetchall()[0]
    beacon = network.now
    origin = network.store._db.execute(
        "SELECT origin FROM escrow_units WHERE owner=? AND bucket='available' AND units>=1000",
        (s.COLD[3].ss58,),
    ).fetchone()[0]
    join = sign_join(
        s.REFEREE,
        s.COLD[3],
        run_id=run_id,
        request_id="ce" * 32,
        expires_beacon=beacon + 5000,
        policy_hash=network.manifest.network.admission_policy_hash,
        hardware_hint=HardwareHint(device_name="cuda", device_count=2, driver="advisory"),
    )
    join_raw = canonicalize(join.body())
    admission_id = sha256_hex(f"join|{run_id}|{s.COLD[3].ss58}|{join.request_id}".encode())
    lock = EscrowLock(
        operation_id="cf" * 32,
        owner=s.COLD[3].ss58,
        units=1000,
        origin_ids=[origin],
        admission_id=admission_id,
        dispute_id=None,
        kind="LOCK_ADMISSION",
    )
    lock_raw = canonicalize(seal(s.COLD[3], "EscrowLock", run_id, lock, beacon + 5000))
    admission = network.store._services(run_id)[1]
    admission.join(join_raw, now=beacon, ip_prefix="local")
    admission.lock(lock_raw, now=beacon)
    # The join seeds at beacon+1; challenge() later needs that round verified.
    network.store.push_beacon(json.loads(gi.fixture_beacon_bytes(beacon + 1)))
    store = network.store
    store.close_v2_notifications()
    store._db.execute("PRAGMA wal_checkpoint(TRUNCATE)")
    state = store.state_dir
    accepted_round = canonicalize(store._record_v2(run_id, "round", "0"))
    geometry = store._record_v2(run_id, "island-job", "0:" + s.HOT[0].ss58)
    context.close()
    payload = json.loads(prelim.read_bytes())
    record = payload["record"]
    files = {k: v for k, v in record["files"].items()}
    for item in files.values():
        src = prelim.parent / item["path"]
        if prelim.parent.resolve() != out.resolve():
            gi.write(out, item["path"], src.read_bytes())
    snapshot = {}
    for path in sorted(state.rglob("*")):
        if (
            path.is_file()
            and path.name not in SKIP_SNAPSHOT
            and path.relative_to(state).parts[0] not in SKIP_DIRS
        ):
            rel = str(path.relative_to(state))
            snapshot[rel] = gi.add(
                files, out, "snap:" + rel, gi.write(out, "snap/" + rel, path.read_bytes())
            )
    gi.add(files, out, "accepted_round", gi.write(out, "c/accepted-round.json", accepted_round))
    gi.add(files, out, "c:join", gi.write(out, "c/join.json", join_raw))
    gi.add(files, out, "c:lock", gi.write(out, "c/lock.json", lock_raw))
    gi.add(
        files, out, "c:geometry", gi.write(out, "c/geometry.json", json.dumps(geometry).encode())
    )
    auditor = next(a for a in s.AUDITORS if a.ss58 == held["auditor"])
    receipt = seal(
        auditor,
        "Receipt",
        run_id,
        {"w": 0, "commit_hash": held["nonce"], "received_round": beacon},
        beacon + 5000,
    )
    gi.add(files, out, "c:receipt", gi.write(out, "c/audit-receipt.json", canonicalize(receipt)))
    gi.add(
        files,
        out,
        "c:lease",
        gi.write(
            out, "c/lease.json", json.dumps({"id": held["id"], "expires": held["expires"]}).encode()
        ),
    )
    gi.add(
        files,
        out,
        "c:expired",
        gi.write(out, "c/expired-beacon.json", gi.fixture_beacon_bytes(held["expires"])),
    )
    gi.add(
        files,
        out,
        "c:turn",
        gi.write(out, "c/turn.json", json.dumps({"dispute_id": dispute_id}).encode()),
    )
    base = {
        "join": "c:join",
        "admission_lock": "c:lock",
        "geometry_job": "c:geometry",
        "audit_receipt": "c:receipt",
        "lease": "c:lease",
    }
    record = {
        **record,
        "beacon": beacon,
        "files": files,
        "snapshot": snapshot,
        "cases": {
            "audit:live": base,
            "audit:cancelled": base,
            "audit:expired": {**base, "beacon": "c:expired"},
            "referee": {"turn": "c:turn"},
        },
    }
    first = gi.sign_index(out, record, beacon, beacon + 100000, "index-v1.json")
    publication = capture_publication(first, out, held, original_launch)
    members = {}
    for path in sorted(publication["directory"].rglob("*")):
        if path.is_file():
            rel = str(path.relative_to(publication["directory"]))
            members[rel] = gi.add(
                files, out, "pub:" + rel, gi.write(out, "pub/" + rel, path.read_bytes())
            )
    gi.add(
        files,
        out,
        "c:publication",
        gi.write(
            out,
            "c/publication.json",
            json.dumps({"job_digest": publication["digest"], "members": members}).encode(),
        ),
    )
    record["cases"]["audit_publication"] = {"publication": "c:publication"}
    record["files"] = files
    return gi.sign_index(out, record, beacon, beacon + 100000, "index.json")


def capture_publication(index: Path, out: Path, held: dict, launch) -> dict:
    from hypertrain.auditor import worker

    s = gi.service()
    scratch = out / "publication-scratch"
    shutil.rmtree(scratch, ignore_errors=True)
    context = s.genuine_network(index, scratch)
    network, *_ = next(context)
    store, manifest = network.store, network.manifest
    captured = {}

    def audit(*args, **kwargs):
        captured["job"] = kwargs["publication_job"]
        source = Path(kwargs["publication_directory"])
        target = out / "publication-run"
        shutil.rmtree(target, ignore_errors=True)
        for relative in kwargs["publication_job"].object_paths.values():
            (target / relative).parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source / relative, target / relative)
        raise Stop("captured")

    original = worker.execute_island_audit
    worker.execute_island_audit = audit
    try:
        receipt = (out / "c/audit-receipt.json").read_bytes()
        try:
            store.execute_audit_v2(manifest.run_id(), held["id"], receipt)
        except Exception:
            if "job" not in captured:
                raise
    finally:
        worker.execute_island_audit = original
        context.close()
    job = captured["job"]
    artifacts = launch(job, out / "publication-run", backend="cuda", trace=True)
    return {"directory": artifacts.directory, "digest": job.digest()}
