"""CPU network-v2 mining smoke: join-v2 -> probe-v2 -> proof-v2 -> run-v2 through the real CLI.

Starts the real challenge app (the `network` service fixture: tiny CPU model, test ledger,
throwaway fixture keys, fixture beacon) on an ephemeral 127.0.0.1 port, plus a TLS relay on
localhost with a self-signed certificate trusted via SSL_CERT_FILE. One miner then runs every
miner stage as its own `uv run --frozen hypertrain-miner ...` process:

  join-v2 -> escrow-lock -> 4 x (trial_export, probe-v2, proof-v2) -> run-v2 --round 0

A round only opens on a roster of four funded eligible miners with at most one PROBATION
(4 clean trials) member, so the three other roster identities are graduated in-process by the
operator role (12 real reference trials each, `"mode": "in-process"` in the evidence).

Writes evidence JSON to $MINE_SMOKE_V2_EVIDENCE (default: a fresh private temp file, path
printed); exit 0 = PASS (every required stage ran with rc 0), exit 1 = a stage failed.
MINE_SMOKE_V2_BREAK=<join-v2|probe-v2|proof-v2|run-v2> points that stage at a closed port
(failure-detection check). SIGTERM (the shell's budget timeout) is recorded as a failure.
"""

from __future__ import annotations

import contextlib
import importlib.util
import json
import os
import secrets
import signal
import socket
import subprocess
import sys
import tempfile
import threading
import time
from collections.abc import Iterator
from pathlib import Path
from types import SimpleNamespace
from typing import Any

ROOT = Path(__file__).resolve().parents[1]

import hypertrain.trainer  # noqa: E402,F401  (determinism before torch)

REQUIRED = ("join-v2", "probe-v2", "proof-v2", "run-v2")
# PROBATION is round-eligible from 4 clean trials (challenge/admission.py:status).
TRIALS = 4


def evidence_file() -> tuple[Path, int]:
    """$MINE_SMOKE_V2_EVIDENCE (never via a symlink), else a fresh 0600 file, never a fixed name."""
    env = os.environ.get("MINE_SMOKE_V2_EVIDENCE")
    if env:
        # No O_TRUNC: a failed start must not empty an existing file; main() truncates at write.
        fd = os.open(env, os.O_WRONLY | os.O_CREAT | os.O_NOFOLLOW, 0o600)
        os.fchmod(fd, 0o600)  # O_CREAT keeps the mode of a file that already exists
        return Path(env), fd
    fd, name = tempfile.mkstemp(prefix="mine-smoke-v2-evidence-", suffix=".json")
    return Path(name), fd


def verdict(stages: list[dict[str, Any]]) -> str | None:
    """None = PASS: every stage ran with rc 0 and each required stage ran at least once."""
    for s in stages:
        if s.get("skipped") or s.get("rc") is None:
            return f"stage {s['name']} skipped"
        if s["rc"] != 0:
            return f"stage {s['name']} failed rc={s['rc']}"
    missing = [n for n in REQUIRED if n not in {s["name"] for s in stages}]
    return f"stage(s) not run: {', '.join(missing)}" if missing else None


def stage(stages: list[dict[str, Any]], name: str, args: list[str], **extra: Any) -> str:
    """Run one CLI stage as its own process; record name/rc/ms; raise unless rc == 0."""
    entry: dict[str, Any] = {"name": name, "mode": "subprocess", "rc": None, **extra}
    stages.append(entry)
    env = {**os.environ, "OMP_NUM_THREADS": "1", "MKL_NUM_THREADS": "1"}
    if name in REQUIRED and os.environ.get("MINE_SMOKE_V2_BREAK") == name:
        # Failure-detection check: point this miner stage (it reads miner.toml) at a closed port.
        env["HYPERTRAIN_MINER_API"] = "http://127.0.0.1:1"
        entry["broken_on_purpose"] = True
    started = time.monotonic()
    proc = subprocess.run(  # noqa: S603 - fixed argv, no shell
        ["uv", "run", "--frozen", *args],  # noqa: S607 - uv from PATH, as mine-smoke.sh
        capture_output=True,
        text=True,
        env=env,
    )
    entry["rc"] = proc.returncode
    entry["ms"] = round((time.monotonic() - started) * 1000)
    if proc.returncode != 0:
        entry["stderr_tail"] = proc.stderr[-2000:]
        raise RuntimeError(f"stage {name} failed rc={proc.returncode}")
    return proc.stdout


def reap(marker: str) -> list[int]:
    """SIGKILL every other process whose argv names `marker` (this run's temp dir).

    torchrun islands start in their own session (island_launch.py), so a timed-out stage
    leaves them running; their argv carries the attempt directory under the temp dir.
    """
    killed = []
    for proc in Path("/proc").iterdir():
        if not proc.name.isdigit() or int(proc.name) == os.getpid():
            continue
        try:
            argv = (proc / "cmdline").read_bytes().split(b"\0")
        except OSError:
            continue
        if any(marker.encode() in a for a in argv):
            with contextlib.suppress(ProcessLookupError):
                os.kill(int(proc.name), signal.SIGKILL)
                killed.append(int(proc.name))
    return killed


def _budget_exceeded(signum: int, frame: Any) -> None:
    raise RuntimeError("budget exceeded (SIGTERM)")


def _load_service() -> Any:
    spec = importlib.util.spec_from_file_location(
        "mine_smoke_v2_service", ROOT / "tests/challenge/test_service_network_v2.py"
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


@contextlib.contextmanager
def serve(app: Any, sock: socket.socket, **tls: str) -> Iterator[None]:
    import uvicorn

    class Ready(uvicorn.Server):
        def __init__(self, config: uvicorn.Config) -> None:
            super().__init__(config)
            self.ready = threading.Event()

        async def startup(self, sockets: Any = None) -> None:
            await super().startup(sockets=sockets)
            self.ready.set()

    server = Ready(uvicorn.Config(app, log_level="error", **tls))
    thread = threading.Thread(target=lambda: server.run(sockets=[sock]), daemon=True)
    thread.start()
    if not server.ready.wait(30):
        raise RuntimeError("server did not start")
    try:
        yield
    finally:
        server.should_exit = True
        thread.join(timeout=10)


def _write_key(path: Path, seed: bytes) -> Path:
    path.write_bytes(seed)
    path.chmod(0o600)
    return path


def _graduate(svc: Any, n: Any, i: int) -> None:
    """Operator role: 12 actual reference trials + signed finality, as _graduate_four does."""
    from hypertrain.auditor.replay import unpack_state
    from hypertrain.data.store import get_large
    from hypertrain.data.trial_assignment import trial_samples
    from hypertrain.miner.island_launch import validate_artifacts
    from hypertrain.protocol.messages import Finalize
    from hypertrain.protocol.messages_v2 import EscrowLock, JoinChallenge, WorkProof, WorkScreenV2
    from hypertrain.trainer.compress import state_hash

    def ok(response: Any) -> Any:
        if response.status_code != 200:
            raise RuntimeError(f"peer {i}: {response.status_code} {response.text[:500]}")
        return response.json()

    n.push(n.now + 101)  # join_burst/quota window, as tests/e2e/_graduate_four
    identity = n.join(i).json()["admission_id"]
    lock = EscrowLock(
        operation_id=f"{i + 1:064x}",
        owner=svc.COLD[i].ss58,
        units=1000,
        origin_ids=[n.origin_ids[i]],
        admission_id=identity,
        dispute_id=None,
        kind="LOCK_ADMISSION",
    )
    ok(n.client.post(n.url + "/escrow/lock", json=n.signed(svc.COLD[i], "EscrowLock", lock)))
    record: dict[str, Any] = {}
    for _ in range(12):
        pending = n.store._db.execute(
            "SELECT r.outcome FROM admissions_v2 a JOIN admission_trial_results r "
            "ON r.epoch=a.trial_epoch WHERE a.admission_id=?",
            (identity,),
        ).fetchone()
        if pending is None or pending["outcome"] != "OPEN":
            n.push(n.now + 1)
        body = ok(n.client.get(n.url + "/join/" + identity + "/challenge"))["body"]
        challenge = JoinChallenge.model_validate(body)
        ok(n.client.post(n.url + "/admin/join/" + identity + "/reference", headers=svc.admin()))
        row = n.store._db.execute(
            "SELECT * FROM admissions_v2 WHERE admission_id=?", (identity,)
        ).fetchone()
        proof = WorkProof.model_validate_json(row["reference_json"])
        samples = trial_samples(
            n.manifest, identity, challenge.nonce, n.store._beacon_v2(challenge.seed_beacon)
        )
        job, directory = n.store.stage_trial_v2(
            n.manifest.run_id(), challenge, tuple(samples), row["trial_epoch"]
        )
        artifacts = validate_artifacts(job, directory / "published")
        screen = WorkScreenV2(
            evidence_kind="work-proof-v1",
            admission_id=identity,
            nonce=challenge.nonce,
            policy_hash=challenge.policy_hash,
            image_digest=job.manifest.training.reference_spec.image_digest,
            layout=challenge.layout,
            challenge_hash=challenge.digest(),
            rank_results=list(artifacts.ranks),
            artifact_hashes=[r.sha256 for r in proof.artifact_refs],
        )
        signed = {
            "proof": n.signed(svc.HOT[i], "WorkProof", proof),
            "screen": n.signed(svc.HOT[i], "WorkScreenV2", screen),
        }
        ok(n.client.post(n.url + "/join/proof", json=signed))
        theta, _ = unpack_state(get_large(proof.artifact_refs[0].sha256, n.store.objects.get))
        final = Finalize(
            w=row["trial_epoch"],
            final_theta_hash_w1=state_hash(theta),
            included=[svc.HOT[i].ss58],
            entitlements_root=proof.digest(),
        )
        record = ok(
            n.client.post(
                n.url + "/admin/join/" + identity + "/finalize",
                json=n.signed(svc.COORD, "Finalize", final),
                headers=svc.admin(),
            )
        )["record"]
    if record.get("state") != "ACTIVE":
        raise RuntimeError(f"peer {i} not ACTIVE after 12 trials: {record.get('state')}")


def _open_round(svc: Any, n: Any) -> Any:
    """Operator role: signed RoundOpenV2 over the four eligible identities, as _live_checkpoint."""
    from hypertrain.auditor.replay import AnchorCache, optimizer_hash, pack_state
    from hypertrain.data.store import put_large
    from hypertrain.protocol.hashing import sha256_hex
    from hypertrain.protocol.jcs import canonicalize
    from hypertrain.protocol.messages import f32hex
    from hypertrain.protocol.messages_v2 import PolicyHashes, RoundOpenV2, StartStateV2
    from hypertrain.trainer.compress import state_hash
    from hypertrain.trainer.config import TrainConfig
    from hypertrain.trainer.model import init_params

    manifest = n.manifest
    cache = AnchorCache()
    theta = init_params(TrainConfig.from_manifest_v2(manifest).model)
    starts, roster = [], []
    for i, hot in enumerate(svc.HOT):
        record = n.store._services(manifest.run_id())[1].status(hot.ss58, now=n.now).record
        anchor = cache.genesis(manifest, hot.ss58, theta)
        starts.append(
            StartStateV2(
                run_id=manifest.run_id(),
                w=0,
                hotkey=hot.ss58,
                theta_hash=state_hash(theta),
                state_object_sha256=put_large(pack_state(theta, anchor.state), sha256_hex),
                opt_state_hash=optimizer_hash(anchor.state),
                ef_object_sha256=put_large(pack_state(anchor.ef), sha256_hex),
                ef_hash=state_hash(anchor.ef),
                parent_anchor_hash=anchor.anchor_hash,
                global_step0=0,
                anchor_verdict_hash=anchor.proof_hash,
            )
        )
        roster.append(
            {
                "hotkey": hot.ss58,
                "slot": i,
                "q_i": f32hex(1),
                "admission_id": record.admission_id,
                "coldkey_group": record.coldkey,
                "state": record.state,
                "eligible_weight": 4194304,
            }
        )
    policies = manifest.network
    opening = RoundOpenV2(
        w=0,
        prev_final_hash="0" * 64,
        theta_hash=state_hash(theta),
        outer_state_hash="0" * 64,
        center_hash="0" * 64,
        roster_hash=sha256_hex(canonicalize(roster)),
        honeypot_commit="0" * 64,
        d_open=n.now + 1,
        d_assign=n.now + 2,
        d_commit=n.now + 30,
        d_audit=n.now + 31,
        d_upload=n.now + 200,
        d_final=n.now + 300,
        contract_version=2,
        policy_hashes=PolicyHashes.model_validate(
            {k: getattr(policies, k) for k in PolicyHashes.model_fields}
        ),
        registry_epoch=0,
        start_state_index_hash=sha256_hex(canonicalize([s.body() for s in starts])),
        audit_mode="anchored-full",
        roster=roster,
    )
    response = n.client.post(
        n.url + "/admin/rounds",
        json=n.signed(svc.COORD, "RoundOpenV2", opening),
        headers=svc.admin(),
    )
    if response.status_code != 200:
        raise RuntimeError(f"round open: {response.status_code} {response.text[:500]}")
    n.push(opening.d_assign)
    return roster


def _relay_app(svc: Any, n: Any, stack: contextlib.ExitStack) -> Any:
    import anyio
    import httpx

    from hypertrain.data.store import LocalFSStore
    from hypertrain.data.stream_store import StreamStore
    from hypertrain.protocol.hashing import sha256_hex
    from hypertrain.protocol.jcs import canonicalize
    from hypertrain.protocol.relay_messages import RelayNetworkManifest
    from hypertrain.relay.app import create_app
    from hypertrain.relay.core import Relay

    rid = n.manifest.run_id()
    registry = n.store._registry_v2(n.manifest)
    # The manifest store.relay_assignment_v2 signs; its record only exists after run-v2 accepts.
    network = RelayNetworkManifest(
        run_id=rid,
        base_manifest_hash=n.manifest.training.training_hash(),
        registry_hash=registry.digest(),
        specs_hash=sha256_hex(canonicalize([s.body() for s in registry.specs])),
        assignment_policy="master-observed-median3-v1",
    )
    backing_http = httpx.AsyncClient()
    stack.callback(anyio.run, backing_http.aclose)

    async def now() -> int:
        return int(n.now)

    relay = Relay(
        run_id=rid,
        master=svc.COORD.ss58,
        registry=registry,
        network_manifest_hash=sha256_hex(canonicalize(network.body())),
        observers={a.ss58 for a in svc.AUDITORS},
        relay_id="local",
        region="local",
        keys={"k1": svc.RELAY},
        active_key="k1",
        backing=StreamStore(LocalFSStore(n.store.state_dir / "relay-backing"), backing_http),
        now=now,
    )
    return create_app(relay, secrets.token_hex(16))


def _tls(directory: Path) -> tuple[Path, Path]:
    cert, key = directory / "relay-cert.pem", directory / "relay.key"
    # The same self-signed recipe as tests/e2e/test_service_network_v2_e2e.py.
    subprocess.run(  # noqa: S603 - fixed argv, no shell
        [  # noqa: S607 - openssl from PATH
            "openssl",
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-days",
            "1",
            "-keyout",
            str(key),
            "-out",
            str(cert),
            "-subj",
            "/CN=localhost",
            "-addext",
            "subjectAltName=DNS:localhost",
        ],
        check=True,
        capture_output=True,
    )
    return cert, key


def chain(tmp: Path, ev: dict[str, Any], stack: contextlib.ExitStack) -> None:
    import httpx

    from hypertrain.protocol.hashing import sha256_hex
    from hypertrain.protocol.relay_messages import RelayRegistryV1

    stages: list[dict[str, Any]] = ev["stages"]
    stack.callback(reap, str(tmp))  # last out: after both servers and the fixture stopped
    cert, tls_key = _tls(tmp)
    # The miner processes and the master's relay-receipt call trust only this localhost CA.
    os.environ["SSL_CERT_FILE"] = str(cert)
    relay_sock, master_sock = socket.socket(), socket.socket()
    for sock in (relay_sock, master_sock):
        sock.bind(("127.0.0.1", 0))
        stack.callback(sock.close)
    relay_url = f"https://localhost:{relay_sock.getsockname()[1]}"

    svc = _load_service()

    class LocalRegistry:
        """The fixture's relay registry, pointed at this run's TLS relay before signing."""

        @staticmethod
        def model_validate(body: dict[str, Any]) -> RelayRegistryV1:
            body["specs"][0]["https_url"] = relay_url
            return RelayRegistryV1.model_validate(body)

    svc.RelayRegistryV1 = LocalRegistry
    (tmp / "service").mkdir()
    fixture = svc.network.__wrapped__(tmp / "service", SimpleNamespace(param=None))
    n = next(fixture)
    stack.callback(fixture.close)
    stack.enter_context(serve(n.client.app, master_sock))
    api = f"http://127.0.0.1:{master_sock.getsockname()[1]}"
    rid = n.manifest.run_id()
    ev["run_id"] = rid

    # Throwaway fixture test seeds (the only hotkey/coldkey the fake metagraph and test genesis
    # know), written 0600 inside the temp dir; never in the evidence.
    hot = _write_key(tmp / "hot.key", bytes([80]) * 32)
    cold = _write_key(tmp / "cold.key", bytes([90]) * 32)
    config = tmp / "miner.toml"
    config.write_text(
        "\n".join(
            f"{k} = {json.dumps(str(v))}"
            for k, v in {
                "api": api,
                "keyfile": hot,
                "workdir": tmp / "work",
                "state_source": "cpu",
                "image_digest": n.manifest.training.reference_spec.image_digest,
                "run_id": rid,
                "owner_hotkey": svc.OWNER.ss58,
                "device": "cpu",
            }.items()
        )
    )
    miner = ["hypertrain-miner"]
    conf = ["--config", str(config)]
    joined = json.loads(
        stage(
            stages,
            "join-v2",
            [*miner, "join-v2", *conf, "--coldkey", str(cold)]
            + ["--request-id", sha256_hex(b"mine-smoke-v2"), "--expiry", "10000"],
        )
    )
    admission = joined["admission_id"]
    stage(
        stages,
        "escrow-lock",
        [*miner, "escrow-lock", "--api", api, "--run-id", rid, "--coldkey", str(cold)]
        + ["--hotkey-ss58", svc.HOT[0].ss58, "--units", "1000", "--origin", n.origin_ids[0]],
        role="operator",
    )
    with httpx.Client(base_url=api, timeout=60) as http:
        for trial in range(1, TRIALS + 1):
            n.push(n.now + 1)
            # The challenge GET issues the trial; export reads the stored challenge.
            http.get(f"/v2/runs/{rid}/join/{admission}/challenge").raise_for_status()
            out = tmp / f"trial-{trial}"
            stage(
                stages,
                "trial-export",
                ["python", "-m", "hypertrain.challenge.trial_export", "--api", api]
                + ["--run-id", rid, "--admission-id", admission]
                + ["--admin-token-file", str(tmp / "service/admin"), "--out", str(out)],
                role="operator",
                trial=trial,
            )
            proof = tmp / f"proof-{trial}.json"
            proof.write_text(
                stage(
                    stages,
                    "probe-v2",
                    [*miner, "probe-v2", *conf, "--job", str(out / "job.json")]
                    + ["--challenge", str(out / "challenge.json")],
                    trial=trial,
                )
            )
            record = json.loads(
                stage(stages, "proof-v2", [*miner, "proof-v2", *conf, "--proof", str(proof)])
            )
            stages[-1]["trial"] = trial
            stages[-1]["record"] = {k: record.get(k) for k in ("state", "clean_count")}
            if record.get("clean_count") != trial:
                raise RuntimeError(
                    f"proof-v2 trial {trial}: clean_count {record.get('clean_count')}"
                )

    started = time.monotonic()
    stages.append({"name": "graduate-peers", "mode": "in-process", "role": "operator", "rc": None})
    for i in (1, 2, 3):
        _graduate(svc, n, i)
    ev["roster"] = [{"hotkey": r["hotkey"], "state": r["state"]} for r in _open_round(svc, n)]
    stages[-1].update(rc=0, ms=round((time.monotonic() - started) * 1000))

    stack.enter_context(
        serve(
            _relay_app(svc, n, stack), relay_sock, ssl_certfile=str(cert), ssl_keyfile=str(tls_key)
        )
    )
    result = json.loads(stage(stages, "run-v2", [*miner, "run-v2", *conf, "--round", "0"]))
    delta = n.store._record_v2(rid, "delta", "0:" + svc.HOT[0].ss58)
    ev["round"] = {"result": result, "delta_recorded": delta is not None}
    if result != {"status": "UPLOADED"} or delta is None:
        raise RuntimeError(f"run-v2: {result}, delta recorded: {delta is not None}")


def main() -> int:
    ev: dict[str, Any] = {"stages": []}
    out, fd = evidence_file()
    started = time.monotonic()
    # The shell's `timeout` sends SIGTERM: unwind so cleanup runs and the evidence is written.
    signal.signal(signal.SIGTERM, _budget_exceeded)
    try:
        with tempfile.TemporaryDirectory() as d, contextlib.ExitStack() as stack:
            chain(Path(d), ev, stack)
    except Exception as e:  # noqa: BLE001 - the evidence must record every failure
        ev["failed"] = f"{type(e).__name__}: {e}"
    failure = ev.get("failed") or verdict(ev["stages"])
    ev["elapsed_seconds"] = round(time.monotonic() - started, 2)
    ev["result"] = "PASS" if failure is None else "FAIL"
    os.ftruncate(fd, 0)
    with os.fdopen(fd, "w") as f:
        f.write(json.dumps(ev, indent=1, sort_keys=True, default=str))
    print(
        json.dumps(
            {
                "result": ev["result"],
                "elapsed_seconds": ev["elapsed_seconds"],
                "stages": [(s["name"], s.get("rc"), s.get("ms")) for s in ev["stages"]],
                "evidence": str(out),
                "failed": failure,
            },
            sort_keys=True,
        )
    )
    return 0 if failure is None else 1


if __name__ == "__main__":
    sys.exit(main())
