# /// script
# requires-python = "==3.12.*"
# dependencies = []
# ///
# Run inside the locked repository environment:
# env OMP_NUM_THREADS=1 nice -n 19 taskset -c 0-3 flock /tmp/hypertrain-network-cpu.lock \
#   uv run --frozen python scripts/relay_kind_smoke.py --create \
#   --cluster hypertrain-v2-proof --cpus 0-3 --cpu-quota 4 --cleanup
"""Pinned, policy-enforcing single-node proof. Never executed by lane implementation.

Fixture roles implement actual bounded storage/TLS/master/miner requests, not a
success simulator. Integrated image must contain relay code; historical image
alone fails preflight. Full cross-lane integration proof needs L0 separately.
"""

from __future__ import annotations

import argparse
import base64
import ctypes
import fcntl
import hashlib
import hmac
import importlib
import ipaddress
import json
import math
import os
import re
import secrets
import select
import shutil
import signal
import socket
import sqlite3
import stat
import subprocess
import sys
import tarfile
import tempfile
import time
from collections.abc import AsyncIterator, Callable
from contextlib import asynccontextmanager, closing, contextmanager
from pathlib import Path
from typing import TYPE_CHECKING, Literal

import anyio
import httpx
from anyio.abc import ByteReceiveStream
from fastapi import FastAPI, HTTPException, Request
from fastapi.responses import JSONResponse, StreamingResponse
from pydantic import BaseModel, ConfigDict
from starlette.responses import Response

from hypertrain.data.store import LocalFSStore, S3Credentials, S3Store, sigv4_headers
from hypertrain.data.stream_store import BLOCK, StreamStore, blocks, spool
from hypertrain.protocol import envelope_v2, relay_envelope
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages_v2 import (
    RoundOpenV2,
    RunManifestV2,
)
from hypertrain.protocol.relay_envelope import RelayEnvelope
from hypertrain.protocol.relay_messages import (
    CustodyRelease,
    MasterAcceptanceV2,
    RelayAssignment,
    RelayKey,
    RelayReceipt,
    RelayRegistryV1,
    RelaySpec,
    RetentionExtension,
    RetrievalRequest,
    RetrievalResponse,
    UploadGrant,
)
from hypertrain.relay.app import create_app
from hypertrain.relay.client import (
    RelayClient,
    chunk_manifest,
    forward_receipt,
    receipt_verified,
)
from hypertrain.relay.core import Relay, Settlement

if TYPE_CHECKING:
    from network_authority_snapshot import Bundle

ROOT = Path(__file__).resolve().parents[1]
FIXTURE = Path("/run/proof")
OBJECT_ROOT = Path("/objects")
MASTER_ROOT = Path("/master")
RUN = hashlib.sha256(b"hypertrain-kind-transport-proof-v2").hexdigest()
GATEWAY = "https://gateway.hypertrain-proof-gateway.svc"


def inspect_integration_snapshot(source: Path, admitted_hash: str) -> Bundle:
    """Use the L0-owned original-byte validator, not a second authority gate."""
    import network_authority_snapshot as authority

    return authority.validate(source, admitted_hash)


class Versions(BaseModel):
    model_config = ConfigDict(extra="forbid")
    server_image: str
    server_source_sha: str
    server_build_job: str
    kind_version: str
    kubernetes_version: str
    node_image: str
    calico_version: str
    calico_manifest_url: str
    calico_manifest_sha256: str
    calico_images: dict[str, str]
    harness: str
    max_node_cpus: int
    max_node_memory_bytes: int


class IntegrationServiceConfig(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True)
    snapshot: Path
    admitted_hash: str
    state: Path
    secrets: Path
    master_url: str
    host: str
    port: int = 8080


def integration_role(role: str, config_path: Path) -> None:
    """Use actual owner bootstrap/driver; never construct fixture funding."""
    import network_authority_snapshot as authority

    if role == "service":
        import uvicorn

        from hypertrain.challenge.app import create_app as challenge_app

        config = IntegrationServiceConfig.model_validate_json(config_path.read_bytes())
        with authority.bootstrap(
            config.snapshot,
            config.admitted_hash,
            config.state,
            config.secrets,
            config.master_url,
        ) as actual:
            from hypertrain.beacon.core import FixtureBeacon

            bundle = authority.validate(config.snapshot, config.admitted_hash)
            beacon = FixtureBeacon(current=bundle.beacon_round).get(bundle.beacon_round)
            ready = config.state / "runtime.json"
            ready.write_text(
                json.dumps(
                    {
                        "fixture_clock_offset": float(os.environ["HT_FIXTURE_CLOCK_OFFSET"]),
                        "beacon": {
                            "round": beacon.round,
                            "signature": beacon.signature,
                            "randomness": beacon.randomness,
                        },
                    }
                )
            )
            uvicorn.run(
                challenge_app(actual, verify_beacon=authority.fixture_beacon),
                host=config.host,
                port=config.port,
            )
    elif role == "driver":
        driver = importlib.import_module("network_service_proof")
        settings = json.loads(config_path.read_bytes())
        runtime = json.loads((Path(settings["service_state"]) / "runtime.json").read_bytes())
        settings["fixture_clock_offset"] = runtime["fixture_clock_offset"]
        beacon_path = Path(settings["evidence"]).parent / "verified-beacon.json"
        beacon_path.write_text(json.dumps(runtime["beacon"]))
        settings["verified_beacon"] = str(beacon_path)
        prepared = Path(settings["evidence"]).parent / "driver.json"
        prepared.write_text(json.dumps(settings))
        driver.main(["--config", str(prepared)])
    elif role == "relay":
        import uvicorn

        relay_config = IntegrationRelayConfig.model_validate_json(config_path.read_bytes())
        uvicorn.run(
            integration_relay(relay_config),
            host=relay_config.host,
            port=relay_config.port,
        )
    elif role == "gateway":
        import uvicorn

        uvicorn.run(
            integration_gateway(),
            host=os.environ["D3_BIND_HOST"],
            port=8443,
            ssl_certfile="/proof/tls/server.crt",
            ssl_keyfile="/proof/tls/server.key",
        )
    elif role == "wrong-ca":
        with httpx.Client(timeout=10, trust_env=False) as client:
            try:
                client.get("https://relay.test/health")
            except httpx.ConnectError as error:
                if "CERTIFICATE_VERIFY_FAILED" not in str(error):
                    raise
                print("TLS_WRONG_CA_REJECTED")
            else:
                raise ValueError("wrong CA unexpectedly accepted")
    else:
        raise ValueError("unknown integration role")


class IntegrationRelayConfig(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True)
    state: Path
    backing: Path
    ca: Path
    master_url: str
    relay_seed: Path
    admin_token: Path
    drain_token: Path
    host: str
    port: int = 8000


def integration_relay(config: IntegrationRelayConfig) -> FastAPI:
    """Actual original-registry relay; service supplies clock and accepted settlement."""
    from dataclasses import fields

    from hypertrain.miner.core import load_keyfile
    from hypertrain.protocol.jcs import canonicalize
    from hypertrain.protocol.relay_messages import RelayNetworkManifest

    def authority_state() -> tuple[RunManifestV2, RelayRegistryV1, int]:
        with closing(
            sqlite3.connect((config.state / "challenge.db").as_uri() + "?mode=ro", uri=True)
        ) as db:
            run_id, raw = db.execute("SELECT run_id,manifest FROM runs").fetchone()
            manifest = RunManifestV2.model_validate_json(raw)
            if run_id != manifest.run_id():
                raise ValueError("relay actual-service run mismatch")
            current = db.execute(
                "SELECT data FROM records_v2 WHERE run_id=? AND kind='registry' AND id='current'",
                (run_id,),
            ).fetchone()
            pinned = RelayRegistryV1.model_validate_json(
                json.dumps(json.loads(current[0])["body"])
                if current
                else LocalFSStore(config.state / "objects").get(
                    manifest.network.relay_registry_hash
                )
            )
            beacon = db.execute("SELECT MAX(round) FROM beacon").fetchone()[0]
        return manifest, pinned, beacon

    @asynccontextmanager
    async def lifespan(app: FastAPI) -> AsyncIterator[None]:
        import ssl

        manifest, pinned, _ = authority_state()
        original = pinned.specs[0]
        if (
            len(pinned.specs) != 1
            or original.id != "local"
            or original.https_url != "https://relay.test"
        ):
            raise ValueError("integration relay requires original local endpoint registry")
        network = RelayNetworkManifest(
            run_id=manifest.run_id(),
            base_manifest_hash=manifest.training.training_hash(),
            registry_hash=pinned.digest(),
            specs_hash=hashlib.sha256(canonicalize([s.body() for s in pinned.specs])).hexdigest(),
            assignment_policy="master-observed-median3-v1",
        )
        async with httpx.AsyncClient(
            verify=ssl.create_default_context(cafile=str(config.ca)),
            timeout=60,
            follow_redirects=False,
        ) as client:

            async def now() -> int:
                return (await anyio.to_thread.run_sync(authority_state))[2]

            async def forward(raw: RelayEnvelope) -> RelayEnvelope:
                return await forward_receipt(client, config.master_url, raw)

            async def settlement(grant: str) -> Settlement:
                response = await client.get(
                    config.master_url
                    + f"/v2/runs/{manifest.run_id()}/admin/relay-settlement/{grant}",
                    headers={"Authorization": "Bearer " + config.admin_token.read_text().strip()},
                )
                response.raise_for_status()
                data = response.json()
                if set(data) != {f.name for f in fields(Settlement)}:
                    raise ValueError("actual service settlement schema mismatch")
                return Settlement(**data)

            relay = Relay(
                run_id=manifest.run_id(),
                master=manifest.training.coord_pubkey,
                registry=pinned,
                network_manifest_hash=network.digest(),
                observers=set(manifest.training.auditors),
                relay_id=original.id,
                region=original.region,
                keys={original.pubkeys[0].key_id: load_keyfile(config.relay_seed)},
                active_key=original.pubkeys[0].key_id,
                backing=StreamStore(LocalFSStore(config.backing), client),
                now=now,
                forward=forward,
                settlement=settlement,
            )
            app.mount("/", create_app(relay, config.drain_token.read_text().strip()))
            await relay.recover()
            yield

    return FastAPI(lifespan=lifespan)


def integration_gateway() -> FastAPI:
    """TLS host routing retains original master.test/relay.test identities."""
    app = FastAPI()

    @app.api_route("/{path:path}", methods=["GET", "POST", "PUT", "DELETE"])
    async def proxy(path: str, request: Request) -> Response:
        hostname = request.url.hostname
        if hostname not in {"master.test", "relay.test"}:
            raise HTTPException(421, "unrecognized original authority hostname")
        target = "http://service:8080" if hostname == "master.test" else "http://relay:8000"
        body = bytearray()
        async for part in request.stream():
            if len(body) + len(part) > 1 << 20:
                raise HTTPException(413, "original relay proof request exceeds 1MiB")
            body.extend(part)
        async with httpx.AsyncClient(timeout=900, follow_redirects=False) as client:
            response = await client.request(
                request.method,
                target + "/" + path,
                params=request.query_params,
                headers={
                    k: v
                    for k, v in request.headers.items()
                    if k.lower()
                    not in {"host", "content-length", "transfer-encoding", "connection"}
                },
                content=bytes(body),
            )
        return Response(
            response.content,
            response.status_code,
            headers={
                k: v
                for k, v in response.headers.items()
                if k.lower()
                not in {
                    "content-length",
                    "transfer-encoding",
                    "connection",
                    "content-encoding",
                }
            },
        )

    return app


def integration_manifests(image: str, *, local_loaded: bool = False) -> dict:
    """Render real role jobs against original DNS/registry; no kubectl/image/build calls.

    /proof is the root-admitted isolated kind mount, not ambient production state.
    Config files/authority/TLS/secrets must exist there before root launch.
    """
    if re.fullmatch(r"[^\s]+@sha256:[0-9a-f]{64}", image) is None and not (
        local_loaded and re.fullmatch(r"hypertrain-relay-proof:[0-9a-f]{64}", image)
    ):
        raise ValueError("integration render requires exact image digest")
    namespace = "hypertrain-d3"
    items = [{"apiVersion": "v1", "kind": "Namespace", "metadata": {"name": namespace}}]
    for role, cpu, memory in (
        ("service", "1000m", "1Gi"),
        ("relay", "250m", "256Mi"),
        ("driver", "2000m", "2Gi"),
    ):
        mounts = [
            {"name": "bundle", "mountPath": "/proof/bundle", "readOnly": True},
            {"name": "configs", "mountPath": "/proof/configs", "readOnly": True},
            {"name": "tls", "mountPath": "/proof/tls", "readOnly": True},
            {"name": "secrets", "mountPath": "/proof/secrets", "readOnly": True},
            {
                "name": "state",
                "mountPath": "/proof/state",
                "readOnly": role != "service",
            },
            {"name": "work", "mountPath": "/proof/work"},
            {"name": "scripts", "mountPath": "/scripts", "readOnly": True},
        ]
        volumes = [
            {"name": name, "hostPath": {"path": "/proof/" + path, "type": "Directory"}}
            for name, path in (
                ("bundle", "bundle"),
                ("configs", "configs"),
                ("tls", "tls"),
                ("secrets", "secrets/" + role),
                ("state", "state"),
                ("work", "work/" + role),
            )
        ]
        volumes.append({"name": "scripts", "configMap": {"name": "actual-scripts"}})
        container = {
            "name": role,
            "image": image,
            "imagePullPolicy": "Never" if local_loaded else "IfNotPresent",
            "command": [
                "/usr/bin/nice",
                "-n",
                "19",
                "/opt/venv/bin/python",
                "/scripts/relay_kind_smoke.py",
                "--mode",
                "integration",
                "--integration-role",
                role,
                "--integration-config",
                "/proof/configs/" + role + ".json",
            ],
            "env": [{"name": name, "value": "1"} for name in ("OMP_NUM_THREADS", "MKL_NUM_THREADS")]
            + [{"name": "SSL_CERT_FILE", "value": "/proof/tls/ca.crt"}],
            "resources": {
                "requests": {"cpu": cpu, "memory": memory},
                "limits": {"cpu": cpu, "memory": memory},
            },
            "volumeMounts": mounts,
            "securityContext": {
                "allowPrivilegeEscalation": False,
                "capabilities": {"drop": ["ALL"]},
            },
        }
        spec = {
            "automountServiceAccountToken": False,
            "securityContext": {
                "runAsNonRoot": True,
                "runAsUser": 65532,
                "seccompProfile": {"type": "RuntimeDefault"},
            },
            "hostAliases": [{"ip": "10.96.0.30", "hostnames": ["master.test", "relay.test"]}],
            "containers": [container],
            "volumes": volumes,
            "restartPolicy": "Never",
        }
        if role != "driver":
            container["readinessProbe"] = {
                "httpGet": {
                    "path": "/health",
                    "port": 8080 if role == "service" else 8000,
                },
                "periodSeconds": 1,
                "timeoutSeconds": 1,
                "failureThreshold": 900,
            }
        if role == "driver":
            items.append(
                {
                    "apiVersion": "batch/v1",
                    "kind": "Job",
                    "metadata": {"name": role, "namespace": namespace},
                    "spec": {
                        "backoffLimit": 0,
                        "activeDeadlineSeconds": 9600,
                        "template": {
                            "metadata": {"labels": {"role": role}},
                            "spec": spec,
                        },
                    },
                }
            )
        else:
            spec["activeDeadlineSeconds"] = 10800
            items.append(
                {
                    "apiVersion": "v1",
                    "kind": "Pod",
                    "metadata": {
                        "name": role,
                        "namespace": namespace,
                        "labels": {"role": role},
                    },
                    "spec": spec,
                }
            )
            items.append(
                {
                    "apiVersion": "v1",
                    "kind": "Service",
                    "metadata": {"name": role, "namespace": namespace},
                    "spec": {
                        "selector": {"role": role},
                        "ports": [{"port": 8080 if role == "service" else 8000}],
                    },
                }
            )
    items.append(
        {
            "apiVersion": "v1",
            "kind": "Service",
            "metadata": {"name": "gateway", "namespace": namespace},
            "spec": {
                "clusterIP": "10.96.0.30",
                "selector": {"role": "gateway"},
                "ports": [{"port": 443, "targetPort": 8443}],
            },
        }
    )
    items.append(
        {
            "apiVersion": "v1",
            "kind": "Pod",
            "metadata": {
                "name": "gateway",
                "namespace": namespace,
                "labels": {"role": "gateway"},
            },
            "spec": {
                "automountServiceAccountToken": False,
                "activeDeadlineSeconds": 10800,
                "containers": [
                    {
                        "name": "gateway",
                        "image": image,
                        "imagePullPolicy": "Never" if local_loaded else "IfNotPresent",
                        "command": [
                            "/usr/bin/nice",
                            "-n",
                            "19",
                            "/opt/venv/bin/python",
                            "/scripts/relay_kind_smoke.py",
                            "--mode",
                            "integration",
                            "--integration-role",
                            "gateway",
                            "--integration-config",
                            "/proof/tls/server.crt",
                        ],
                        "ports": [{"containerPort": 8443}],
                        "readinessProbe": {
                            "tcpSocket": {"port": 8443},
                            "periodSeconds": 1,
                        },
                        "env": [
                            {
                                "name": "D3_BIND_HOST",
                                "value": str(ipaddress.IPv4Address(0)),
                            }
                        ],
                        "resources": {"limits": {"cpu": "100m", "memory": "128Mi"}},
                        "volumeMounts": [
                            {
                                "name": "tls",
                                "mountPath": "/proof/tls",
                                "readOnly": True,
                            },
                            {
                                "name": "scripts",
                                "mountPath": "/scripts",
                                "readOnly": True,
                            },
                        ],
                    }
                ],
                "volumes": [
                    {
                        "name": "tls",
                        "hostPath": {"path": "/proof/tls", "type": "Directory"},
                    },
                    {"name": "scripts", "configMap": {"name": "actual-scripts"}},
                ],
            },
        }
    )
    return {"apiVersion": "v1", "kind": "List", "items": items}


def command(
    *args: str,
    input_text: str | None = None,
    timeout: float = 180,
    environment: dict[str, str] | None = None,
) -> str:
    executable = shutil.which(args[0])
    if executable is None or Path(executable).name not in {
        "docker",
        "podman",
        "kubectl",
        "kind",
        "openssl",
        "systemd-run",
        "systemctl",
    }:
        raise ValueError("unapproved/missing proof executable")
    if Path(executable).name == "kind":
        if (
            os.environ.get("KIND_EXPERIMENTAL_PROVIDER", "docker") != "docker"
            or (environment or {}).get("KIND_EXPERIMENTAL_PROVIDER", "docker") != "docker"
        ):
            raise ValueError("kind provider must be explicitly Docker; fallback forbidden")
        environment = {**os.environ, **(environment or {}), "KIND_EXPERIMENTAL_PROVIDER": "docker"}

    async def execute() -> str:
        with anyio.fail_after(timeout):
            async with await anyio.open_process(
                [executable, *args[1:]],
                stdin=subprocess.PIPE if input_text is not None else subprocess.DEVNULL,
                env=environment,
            ) as process:
                captured = [bytearray(), bytearray()]

                async def capture(stream: ByteReceiveStream | None, target: bytearray) -> None:
                    if stream is None:
                        return
                    async for part in stream:
                        if len(target) + len(part) > 4 << 20:
                            process.kill()
                            raise ValueError("proof diagnostic output exceeds 4MiB")
                        target.extend(part)

                async def send() -> None:
                    if process.stdin is not None:
                        await process.stdin.send((input_text or "").encode())
                        await process.stdin.aclose()

                async with anyio.create_task_group() as tasks:
                    tasks.start_soon(capture, process.stdout, captured[0])
                    tasks.start_soon(capture, process.stderr, captured[1])
                    tasks.start_soon(send)
                    exit_code = await process.wait()
                if exit_code:
                    raise RuntimeError(
                        f"{args[0]} exit{exit_code}: " + captured[1].decode(errors="replace")
                    )
                return captured[0].decode()

    return anyio.run(execute)


def key(name: str) -> Keypair:
    return Keypair(bytes.fromhex((FIXTURE / f"{name}.seed").read_text().strip()))


def signed(model: BaseModel, signer: Keypair | None = None) -> RelayEnvelope:
    return relay_envelope.parse_envelope(
        relay_envelope.seal(signer or key("master"), type(model).__name__, RUN, model, 10000)
    )


def registry() -> RelayRegistryV1:
    return RelayRegistryV1.model_validate_json((FIXTURE / "registry.json").read_bytes())


def credentials() -> S3Credentials:
    # Kubernetes projected Secrets are root-owned; copy to an owned restrictive file.
    with tempfile.NamedTemporaryFile() as file:
        file.write((FIXTURE / "object.json").read_bytes())
        file.flush()
        os.fchmod(file.fileno(), 0o600)
        return S3Credentials.from_secret_file(Path(file.name))


def backing(client: httpx.AsyncClient, region: str) -> StreamStore:
    return StreamStore(
        S3Store(
            "http://objects.hypertrain-proof-store.svc:9000",
            "proof",
            credentials(),
            region="local",
            prefix=f"relay/{region}/",
        ),
        client,
    )


def fixture_store() -> FastAPI:
    """Tiny durable S3 subset, validates real SigV4 and conditional writes."""
    app = FastAPI()
    directory = OBJECT_ROOT
    directory.mkdir(exist_ok=True)
    lock = anyio.Lock()

    @app.get("/livez")
    async def live() -> dict[str, bool]:
        return {"live": True}

    @app.api_route("/proof/relay/{region}/{name}", methods=["PUT", "GET", "DELETE"])
    async def object_request(region: Literal["eu", "us"], name: str, request: Request) -> Response:
        if not re.fullmatch(r"[0-9a-f]{64}(\.journal)?", name):
            raise HTTPException(400, "bad key")
        sha = request.headers.get("x-amz-content-sha256", "")
        stamp = request.headers.get("x-amz-date", "")
        extra = {
            k: request.headers[k] for k in ("if-match", "if-none-match") if k in request.headers
        }
        expected = sigv4_headers(
            request.method,
            str(request.url),
            credentials(),
            "local",
            stamp,
            sha,
            extra=extra,
        )
        if not hmac.compare_digest(
            expected["Authorization"], request.headers.get("authorization", "")
        ):
            raise HTTPException(403, "SigV4 denied")
        regional = directory / region
        regional.mkdir(exist_ok=True)
        path = regional / name

        def sync_directory() -> None:
            fd = os.open(regional, os.O_DIRECTORY)
            try:
                os.fsync(fd)
            finally:
                os.close(fd)

        if request.method == "DELETE":
            async with lock:
                path.unlink(missing_ok=True)
                sync_directory()
            return JSONResponse({})
        if request.method == "GET":
            # Bind ETag/body to the same immutable inode through atomic journal replacement.
            async with lock:
                if not path.exists():
                    raise HTTPException(404, "missing")
                file = path.open("rb")
                size = os.fstat(file.fileno()).st_size
                etag = hashlib.file_digest(file, "sha256").hexdigest()
                file.seek(0)

            async def content() -> AsyncIterator[bytes]:
                try:
                    async for part in blocks(file):
                        yield part
                finally:
                    file.close()

            return StreamingResponse(
                content(),
                headers={"ETag": '"' + etag + '"', "Content-Length": str(size)},
            )
        with spool() as upload_file:
            digest, count = hashlib.sha256(), 0
            async for part in request.stream():
                count += len(part)
                if count > 16 << 20:
                    raise HTTPException(413, "fixture object quota")
                upload_file.write(part)
                digest.update(part)
            if digest.hexdigest() != sha:
                raise HTTPException(400, "payload hash mismatch")
            async with lock:
                current = None
                if path.exists():
                    with path.open("rb") as current_file:
                        current = (
                            '"' + hashlib.file_digest(current_file, "sha256").hexdigest() + '"'
                        )
                if (extra.get("if-none-match") == "*" and current is not None) or (
                    "if-match" in extra and extra["if-match"] != current
                ):
                    raise HTTPException(412, "conditional mismatch")
                upload_file.seek(0)
                tmp = directory / (name + ".tmp")
                with tmp.open("wb") as output:
                    while part := upload_file.read(BLOCK):
                        output.write(part)
                    output.flush()
                    os.fsync(output.fileno())
                tmp.replace(path)
                sync_directory()
                return JSONResponse({}, headers={"ETag": '"' + sha + '"'})

    return app


def fixture_master() -> FastAPI:
    """Explicit transport test master; independent reads before durable acceptance."""
    app = FastAPI()
    MASTER_ROOT.mkdir(exist_ok=True)
    database = MASTER_ROOT / "transport.sqlite"
    with sqlite3.connect(database) as db:
        db.execute(
            "CREATE TABLE IF NOT EXISTS grants (hash TEXT PRIMARY KEY, envelope TEXT NOT NULL)"
        )
        db.execute(
            "CREATE TABLE IF NOT EXISTS receipts (grant_hash TEXT PRIMARY KEY, "
            "receipt TEXT NOT NULL, acceptance TEXT NOT NULL)"
        )

    @app.get("/livez")
    async def live() -> dict[str, bool]:
        return {"live": True}

    @app.post("/v2/relay-receipts")
    async def accept(request: Request) -> JSONResponse:
        raw = relay_envelope.parse_envelope(await request.body())
        receipt = RelayReceipt.model_validate(raw.body)
        with sqlite3.connect(database) as db:
            row = db.execute(
                "SELECT envelope FROM grants WHERE hash=?", (receipt.grant_hash,)
            ).fetchone()
        if row is None:
            raise HTTPException(400, "unregistered grant")
        grant = UploadGrant.model_validate(relay_envelope.parse_envelope(row[0]).body)
        receipt_verified(
            raw,
            registry=registry(),
            relay_id=grant.relay_id,
            run_id=RUN,
            grant=grant,
            received_before=50,
        )
        async with httpx.AsyncClient(timeout=30, follow_redirects=False) as client:
            with spool() as file:
                await backing(client, grant.relay_id.split("-", 1)[0]).get(
                    grant.delta_hash, grant.size, file
                )
        accepted = signed(
            MasterAcceptanceV2(
                w=grant.w,
                hotkey=grant.hotkey,
                delta_hash=grant.delta_hash,
                receipt_hash=receipt.digest(),
                received_drand=20,
            )
        )
        with sqlite3.connect(database) as db:
            db.execute("BEGIN IMMEDIATE")
            prior = db.execute(
                "SELECT receipt, acceptance FROM receipts WHERE grant_hash=?",
                (receipt.grant_hash,),
            ).fetchone()
            if prior is not None:
                previous = RelayReceipt.model_validate(relay_envelope.parse_envelope(prior[0]).body)
                if previous != receipt:
                    raise HTTPException(409, "conflicting receipt")
                accepted = relay_envelope.parse_envelope(prior[1])
            else:
                db.execute(
                    "INSERT INTO receipts VALUES (?,?,?)",
                    (
                        receipt.grant_hash,
                        raw.model_dump_json(),
                        accepted.model_dump_json(),
                    ),
                )
        return JSONResponse(accepted.model_dump(mode="json"))

    @app.post("/register")
    async def register(request: Request) -> JSONResponse:
        raw = relay_envelope.parse_envelope(await request.body())
        if (
            raw.signer != key("master").ss58
            or raw.type != "UploadGrant"
            or raw.run_id != RUN
            or raw.exp_drand < 20
            or not relay_envelope.verify_envelope(raw.model_dump())
        ):
            raise HTTPException(403, "grant denied")
        grant = UploadGrant.model_validate(raw.body)
        if grant.exp_drand < 20:
            raise HTTPException(400, "expired grant")
        with sqlite3.connect(database) as db:
            db.execute(
                "INSERT OR IGNORE INTO grants VALUES (?,?)",
                (grant.digest(), raw.model_dump_json()),
            )
        return JSONResponse({"registered": grant.digest()})

    return app


def fixture_gateway() -> FastAPI:
    @asynccontextmanager
    async def lifespan(app: FastAPI) -> AsyncIterator[None]:
        async with httpx.AsyncClient(
            timeout=60, follow_redirects=False, limits=httpx.Limits(max_connections=8)
        ) as client:
            app.state.client = client
            yield

    app = FastAPI(lifespan=lifespan)

    @app.api_route("/{region}/{path:path}", methods=["GET", "POST", "PUT"])
    async def proxy(region: Literal["eu", "us", "master"], path: str, request: Request) -> Response:
        target = (
            "http://master.hypertrain-proof.svc:8000"
            if region == "master"
            else f"http://hypertrain-relay.hypertrain-relay-{region}.svc:8000"
        )
        client: httpx.AsyncClient = app.state.client
        headers = {
            k: v
            for k, v in request.headers.items()
            if k.lower() not in {"host", "connection", "transfer-encoding"}
        }
        upstream = client.build_request(
            request.method,
            target + "/" + path,
            params=request.query_params,
            headers=headers,
            content=request.stream(),
        )
        response = await client.send(upstream, stream=True)

        async def content() -> AsyncIterator[bytes]:
            try:
                async for part in response.aiter_raw(BLOCK):
                    yield part
            finally:
                await response.aclose()

        return StreamingResponse(
            content(),
            status_code=response.status_code,
            headers={
                k: v
                for k, v in response.headers.items()
                if k.lower() not in {"connection", "transfer-encoding"}
            },
        )

    return app


def fixture_relay() -> FastAPI:
    @asynccontextmanager
    async def lifespan(app: FastAPI) -> AsyncIterator[None]:
        region = os.environ["REGION"]
        async with httpx.AsyncClient(
            verify=str(FIXTURE / "ca.crt"),
            timeout=30,
            follow_redirects=False,
            limits=httpx.Limits(max_connections=8),
        ) as client:

            async def now() -> int:
                return 20  # deterministic proof beacon, no production time claim

            async def forward(raw: RelayEnvelope) -> RelayEnvelope:
                return await forward_receipt(client, GATEWAY + "/master", raw)

            relay = Relay(
                run_id=RUN,
                master=(FIXTURE / "master.public").read_text().strip(),
                registry=registry(),
                network_manifest_hash=registry().digest(),
                observers={(FIXTURE / "master.public").read_text().strip()},
                relay_id=f"{region}-1",
                region=region,
                keys={"k1": key(region)},
                active_key="k1",
                backing=backing(client, region),
                now=now,
                forward=forward,
            )
            app.mount("/", create_app(relay, (FIXTURE / "drain.token").read_text().strip()))
            await relay.recover()
            yield

    return FastAPI(lifespan=lifespan)


async def fixture_miner() -> None:
    payload = bytes(range(256)) * (10 << 12)  # exactly 10MiB
    digest = hashlib.sha256(payload).hexdigest()
    path = Path(tempfile.mkdtemp(prefix="miner-")) / "delta"
    await anyio.to_thread.run_sync(path.write_bytes, payload)
    manifest = chunk_manifest(path)
    reg = registry()
    async with httpx.AsyncClient(
        verify=str(FIXTURE / "ca.crt"), timeout=90, follow_redirects=False
    ) as client:
        relay_client = RelayClient(reg, client)
        originals: list[str] = []
        for identity in range(4):
            region = "eu" if identity < 2 else "us"
            fallback = os.environ.get("PROOF_STAGE") == "fallback"
            if fallback and identity != 0:
                continue
            if fallback:
                region = "us"
            round_index = 4 if fallback else identity
            miner = Keypair(hashlib.sha256(f"proof-miner-{identity}".encode()).digest())
            grant = UploadGrant(
                w=round_index,
                hotkey=miner.ss58,
                relay_id=f"{region}-1",
                assignment_epoch=0,
                delta_hash=digest,
                size=len(payload),
                chunk_manifest_hash=manifest.chunk_manifest_hash(),
                retain_until=500,
                nonce=hashlib.sha256(f"proof-grant-{round_index}".encode()).hexdigest(),
                exp_drand=50,
            )
            raw = signed(grant)
            registered = await client.post(
                GATEWAY + "/master/register", content=raw.model_dump_json()
            )
            registered.raise_for_status()
            assignment = RelayAssignment(
                w=round_index,
                hotkey=miner.ss58,
                primary_id="eu-1",
                fallback_ids=["us-1"],
                assignment_epoch=0,
                manifest_hash=reg.digest(),
                exp_drand=50,
            )
            h = "12" * 32
            opened = RoundOpenV2.model_validate(
                {
                    "w": round_index,
                    **{
                        name: h
                        for name in (
                            "prev_final_hash",
                            "theta_hash",
                            "outer_state_hash",
                            "center_hash",
                            "roster_hash",
                            "honeypot_commit",
                            "start_state_index_hash",
                        )
                    },
                    "d_open": 1,
                    "d_assign": 2,
                    "d_commit": 10,
                    "d_audit": 15,
                    "d_upload": 50,
                    "d_final": 60,
                    "contract_version": 2,
                    "policy_hashes": {
                        name: h
                        for name in (
                            "admission_policy_hash",
                            "economics_policy_hash",
                            "aggregation_policy_hash",
                            "dispute_policy_hash",
                            "audit_policy_hash",
                        )
                    },
                    "registry_epoch": 0,
                    "audit_mode": "anchored-full",
                    "roster": [],
                }
            )
            round_raw = envelope_v2.seal(key("master"), "RoundOpenV2", RUN, opened, 100)
            if os.environ.get("PROOF_STAGE") == "prefix":
                url = reg.specs[0].https_url
                accepted = await client.post(
                    url + f"/v1/uploads/{grant.digest()}/accept",
                    json={
                        "grant": raw.model_dump(mode="json"),
                        "manifest": manifest.body(),
                        "assignment": signed(assignment).model_dump(mode="json"),
                        "round_open": round_raw,
                    },
                )
                accepted.raise_for_status()
                capability = base64.b64encode(raw.model_dump_json().encode()).decode()
                uploaded = await client.put(
                    url + f"/v1/uploads/{grant.digest()}/chunks/0",
                    content=payload[: 4 << 20],
                    headers={"X-Upload-Grant": capability},
                )
                uploaded.raise_for_status()
                print(
                    json.dumps({"event": "PREFIX_DURABLE", "grant": grant.digest()}),
                    flush=True,
                )
                return
            receipt_raw = await relay_client.upload(
                path, grant_raw=raw, assignment=signed(assignment), round_open=round_raw
            )
            receipt = RelayReceipt.model_validate(receipt_raw.body)
            request = RetrievalRequest(
                request_id=hashlib.sha256(f"proof-request-{round_index}".encode()).hexdigest(),
                nonce=hashlib.sha256(f"proof-nonce-{round_index}".encode()).hexdigest(),
                relay_id=f"{region}-1",
                key_id="k1",
                assignment_hash=assignment.digest(),
                grant_hash=grant.digest(),
                custody_ack_hashes=[],
                receipt_hash=receipt.digest(),
                retention_hash=receipt.digest(),
                object_or_chunk_hash=digest,
                size=len(payload),
                requested_beacon=20,
                deadline_beacon=30,
            )
            with spool() as file:
                response = await relay_client.retrieval(
                    signed(request), file, relay_public=receipt_raw.signer
                )
                assert RetrievalResponse.model_validate(response.body).status == "SERVED"
                assert hashlib.sha256(file.read()).hexdigest() == digest
            originals.append(receipt.digest())
        print(
            json.dumps(
                {
                    "event": "FOUR_ORIGINAL_OBJECTS_VERIFIED",
                    "sha256": digest,
                    "receipts": originals,
                    "bytes_per_object": len(payload),
                }
            ),
            flush=True,
        )


async def fixture_gc() -> None:
    """Transport-only signed retention controls; never evidence of L0 finality."""
    clock = 490

    async def now() -> int:
        return clock

    async def settlement(grant_hash: str) -> Settlement:
        # Explicit fixture settlement, used only by internal transport GC test.
        return Settlement("12" * 32, 100, "34" * 32, 120, 110)

    async with httpx.AsyncClient(timeout=30) as client:
        eu = Relay(
            run_id=RUN,
            master=key("master").ss58,
            registry=registry(),
            network_manifest_hash=registry().digest(),
            observers={key("master").ss58},
            relay_id="eu-1",
            region="eu",
            keys={"k1": key("eu")},
            active_key="k1",
            backing=backing(client, "eu"),
            now=now,
            settlement=settlement,
        )
        # Separate privileged transport verifier; extensions still go to the actual pod.
        state = await eu.state()
        if not state.uploads or any(u.receipt is None for u in state.uploads.values()):
            raise ValueError("GC proof requires actual complete relay custody")
        extensions: dict[str, list[str]] = {}
        async with httpx.AsyncClient(verify=str(FIXTURE / "ca.crt"), timeout=30) as tls:
            for grant_hash, record in state.uploads.items():
                extensions[grant_hash] = []
                for digest in record.pins:
                    extension = RetentionExtension(
                        grant_hash=grant_hash,
                        custody_hashes=[digest],
                        seq=1,
                        previous_retention_hash=digest,
                        dispute_ids=["56" * 32],
                        issued_beacon=20,
                        retain_until=600,
                    )
                    response = await tls.post(
                        GATEWAY + f"/eu/v1/uploads/{grant_hash}/retention",
                        content=signed(extension).model_dump_json(),
                    )
                    response.raise_for_status()
                    extensions[grant_hash].append(extension.digest())
        clock = 550
        assert not await eu.collect(), "unreleased extended custody was deleted"
        digest = next(iter(state.uploads.values())).manifest.chunks[0].chunk_sha256
        with spool() as file:
            await backing(client, "us").get(digest, 4 << 20, file)
        clock = 1001
        for grant_hash, record in (await eu.state()).uploads.items():
            for custody_hash, pin in record.pins.items():
                await eu.release(
                    signed(
                        CustodyRelease(
                            grant_hash=grant_hash,
                            custody_hashes=[custody_hash],
                            retention_hash=pin.retention_hash,
                            finality_hash="12" * 32,
                            vesting_beacon=110,
                            closed_dispute_root="34" * 32,
                            release_beacon=1000,
                        )
                    )
                )
        deleted = await eu.collect()
        assert deleted, "released expired EU custody did not collect"
        with spool() as file:
            await backing(client, "us").get(digest, 4 << 20, file)
        print(
            json.dumps(
                {
                    "event": "TRANSPORT_RETENTION_GC_PREFIX_ISOLATION",
                    "deleted": deleted,
                    "L0_finality": "NOT_TESTED",
                }
            ),
            flush=True,
        )


def fixture(role: str) -> None:
    import uvicorn

    host = socket.gethostbyname(socket.gethostname())
    match role:
        case "store":
            uvicorn.run(fixture_store(), host=host, port=9000)
        case "master":
            uvicorn.run(fixture_master(), host=host, port=8000)
        case "gateway":
            uvicorn.run(
                fixture_gateway(),
                host=host,
                port=8443,
                ssl_keyfile=str(FIXTURE / "tls.key"),
                ssl_certfile=str(FIXTURE / "tls.crt"),
            )
        case "relay":
            uvicorn.run(fixture_relay(), host=host, port=8000)
        case "miner":
            anyio.run(fixture_miner)
        case "gc":
            anyio.run(fixture_gc)
        case _:
            raise ValueError("unknown fixture role")


def apply(kubectl: list[str], resource: object) -> None:
    command(*kubectl, "apply", "-f", "-", input_text=json.dumps(resource))


class CleanupJournal:
    """Atomic, fsynced evidence for an external lifecycle supervisor."""

    def __init__(self, path: Path) -> None:
        self.path = path

    def write_text(self, data: str) -> None:
        incoming = json.loads(data)
        if self.path.exists():
            previous = json.loads(self.path.read_bytes())
            for key in (
                "cluster",
                "experiment",
                "proof_dir",
                "kubeconfig",
                "deadline_monotonic",
                "cleanup_cutoff_monotonic",
                "cleanup_budget_cutoff_monotonic",
                "boot_id",
                "containers",
                "volumes",
                "networks",
                "provider_baselines",
            ):
                if key in previous and key in incoming and previous[key] != incoming[key]:
                    raise ValueError("immutable journal field changed: " + key)
            incoming = {**previous, **incoming}
        data = json.dumps(incoming)
        with tempfile.NamedTemporaryFile(dir=self.path.parent, delete=False) as file:
            file.write(data.encode())
            file.flush()
            os.fsync(file.fileno())
            temporary = file.name
        os.replace(temporary, self.path)
        fd = os.open(self.path.parent, os.O_DIRECTORY)
        try:
            os.fsync(fd)
        finally:
            os.close(fd)


@contextmanager
def transport_archive(args):
    """Admitted native archive, pinned by no-follow descriptor and Linux read lease."""
    from hypertrain.aggregator.capacity_worker import _directory_fd, _read_at

    if args.mode != "transport" or args.local_image_id or args.image:
        raise ValueError("archive requires transport-only exclusive image route")
    if not all(
        (
            args.archive_sha256,
            args.archive_image_id,
            args.archive_reference,
            args.source_manifest,
            args.source_manifest_sha256,
        )
    ):
        raise ValueError("archive requires admitted hash/image/reference/source")
    if (
        not re.fullmatch(r"[0-9a-f]{64}", args.archive_sha256)
        or not re.fullmatch(r"sha256:[0-9a-f]{64}", args.archive_image_id)
        or not re.fullmatch(r"[a-z0-9][a-z0-9._/-]*:[a-z0-9][a-z0-9._-]*", args.archive_reference)
    ):
        raise ValueError("archive image/hash/reference invalid")
    source = Path(args.source_manifest)
    with _directory_fd(source.parent) as parent:
        source_data = _read_at(parent, Path(source.name), 1 << 20)
    if hashlib.sha256(source_data).hexdigest() != args.source_manifest_sha256:
        raise ValueError("frozen source manifest digest mismatch")
    source_map = json.loads(source_data)
    if set(source_map) != {"files"} or not isinstance(source_map["files"], dict):
        raise ValueError("source manifest requires exact files map")
    files = source_map["files"]
    if len(files) != 109 or any(
        not key.startswith("src/hypertrain/")
        or ".." in Path(key).parts
        or not re.fullmatch(r"[0-9a-f]{64}", value)
        for key, value in files.items()
    ):
        raise ValueError("archive requires frozen source109")
    archive = Path(args.image_archive)
    with _directory_fd(archive.parent) as parent:
        fd = os.open(archive.name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=parent)
        previous = signal.getsignal(signal.SIGIO)
        leased = False
        try:
            identity = os.fstat(fd)
            if (
                not stat.S_ISREG(identity.st_mode)
                or identity.st_uid != os.geteuid()
                or identity.st_mode & 0o222
                or identity.st_nlink != 1
                or not 0 < identity.st_size <= 2 << 30
            ):
                raise ValueError("archive must be owned readonly single-link bounded file")

            def reject_write(*_):
                raise ValueError("archive write/lease break refused")

            signal.signal(signal.SIGIO, reject_write)
            fcntl.fcntl(fd, fcntl.F_SETLEASE, fcntl.F_RDLCK)
            leased = True
            with os.fdopen(os.dup(fd), "rb") as stream:
                if hashlib.file_digest(stream, "sha256").hexdigest() != args.archive_sha256:
                    raise ValueError("archive SHA256 differs from admission")
                stream.seek(0)
                with tarfile.open(fileobj=stream, mode="r:") as tar:

                    def document(name):
                        member = tar.getmember(name)
                        if not member.isfile() or not 0 < member.size <= 1 << 20:
                            raise ValueError("archive metadata must be bounded regular file")
                        content = tar.extractfile(member)
                        assert content is not None
                        return content.read()

                    names = tar.getnames()
                    if len(names) != len(set(names)):
                        raise ValueError("duplicate archive members")
                    manifest = json.loads(document("manifest.json"))
                    config_name = "blobs/sha256/" + args.archive_image_id.split(":", 1)[1]
                    if (
                        len(manifest) != 1
                        or manifest[0]["Config"] != config_name
                        or manifest[0]["RepoTags"] != [args.archive_reference]
                    ):
                        raise ValueError("archive stored reference/config mismatch")
                    config_data = document(config_name)
                    if "sha256:" + hashlib.sha256(config_data).hexdigest() != args.archive_image_id:
                        raise ValueError("archive config digest mismatch")
                    config = json.loads(config_data)
                    if (
                        config.get("os"),
                        config.get("architecture"),
                        config.get("config", {}).get("User"),
                    ) != ("linux", "amd64", "65532:65532"):
                        raise ValueError("archive requires linux/amd64 nonroot65532")
                    index = json.loads(document("index.json"))
                    descriptors = index["manifests"]
                    if len(descriptors) != 1 or descriptors[0]["annotations"].get(
                        "io.containerd.image.name"
                    ) != ("docker.io/library/" + args.archive_reference):
                        raise ValueError("archive OCI reference mismatch")
                    descriptor = descriptors[0]
                    if not re.fullmatch(r"sha256:[0-9a-f]{64}", descriptor["digest"]):
                        raise ValueError("archive manifest descriptor invalid")
                    manifest_data = document(
                        "blobs/sha256/" + descriptor["digest"].split(":", 1)[1]
                    )
                    if (
                        len(manifest_data) != descriptor["size"]
                        or "sha256:" + hashlib.sha256(manifest_data).hexdigest()
                        != descriptor["digest"]
                    ):
                        raise ValueError("archive OCI manifest mismatch")
                    oci = json.loads(manifest_data)
                    if oci["config"]["digest"] != args.archive_image_id or oci["config"][
                        "size"
                    ] != len(config_data):
                        raise ValueError("archive OCI config mismatch")
                    if not oci["layers"] or any(
                        not re.fullmatch(r"sha256:[0-9a-f]{64}", d["digest"]) for d in oci["layers"]
                    ):
                        raise ValueError("archive layer descriptors invalid")
                    layers = ["blobs/sha256/" + d["digest"].split(":", 1)[1] for d in oci["layers"]]
                    if layers != manifest[0]["Layers"] or any(
                        not tar.getmember(name).isfile() or tar.getmember(name).size != d["size"]
                        for name, d in zip(layers, oci["layers"], strict=True)
                    ):
                        raise ValueError("archive layer closure mismatch")
            # A path rename cannot substitute the descriptor passed to kind.
            yield f"/proc/{os.getpid()}/fd/{fd}", args.archive_reference, files
        finally:
            if leased:
                fcntl.fcntl(fd, fcntl.F_SETLEASE, fcntl.F_UNLCK)
            os.close(fd)
            signal.signal(signal.SIGIO, previous)


def load_transport_image(cluster, image, archive=None):
    command(
        "kind",
        "load",
        "image-archive" if archive else "docker-image",
        archive[0] if archive else image,
        "--name",
        cluster,
        timeout=240,
    )


def verify_archive_node(node, reference, image_id, files, kubectl):
    info = json.loads(
        command("docker", "exec", node, "crictl", "inspecti", "-o", "json", reference)
    )
    if (
        info["status"]["id"] != image_id
        or "docker.io/library/" + reference not in info["status"]["repoTags"]
    ):
        raise ValueError("owned node imported image identity differs")
    check = """import hashlib,json,pathlib,os,hypertrain
assert os.getuid()==65532
expected=json.loads(os.environ['SOURCE109'])
root=pathlib.Path(hypertrain.__file__).parent
actual={}
for p in root.rglob('*'):
 if p.is_file() and '__pycache__' not in p.parts and p.suffix!='.pyc':
  key='src/hypertrain/'+p.relative_to(root).as_posix()
  actual[key]=hashlib.sha256(p.read_bytes()).hexdigest()
assert actual==expected
import hypertrain.relay.app
print('ARCHIVE_NODE_SOURCE109_RELAY_IMPORT_VERIFIED',flush=True)
"""
    apply(
        kubectl,
        {
            "apiVersion": "batch/v1",
            "kind": "Job",
            "metadata": {"name": "archive-source-check"},
            "spec": {
                "backoffLimit": 0,
                "activeDeadlineSeconds": 60,
                "template": {
                    "spec": {
                        "restartPolicy": "Never",
                        "nodeName": node,
                        "securityContext": {
                            "runAsUser": 65532,
                            "runAsGroup": 65532,
                            "runAsNonRoot": True,
                            "fsGroup": 65532,
                        },
                        "volumes": [
                            {"name": "data", "emptyDir": {"medium": "Memory", "sizeLimit": "16Mi"}}
                        ],
                        "containers": [
                            {
                                "name": "check",
                                "image": reference,
                                "imagePullPolicy": "Never",
                                "command": ["/opt/venv/bin/python", "-c", check],
                                "volumeMounts": [{"name": "data", "mountPath": "/data"}],
                                "env": [
                                    {"name": "SOURCE109", "value": json.dumps(files)},
                                    {"name": "OMP_NUM_THREADS", "value": "1"},
                                ],
                                "resources": {
                                    "requests": {"cpu": "50m", "memory": "64Mi"},
                                    "limits": {"cpu": "500m", "memory": "256Mi"},
                                },
                            }
                        ],
                    }
                },
            },
        },
    )
    command(
        *kubectl, "wait", "--for=condition=complete", "job/archive-source-check", "--timeout=90s"
    )
    log = command(*kubectl, "logs", "job/archive-source-check")
    if log.strip() != "ARCHIVE_NODE_SOURCE109_RELAY_IMPORT_VERIFIED":
        raise ValueError("owned node source109/import verification failed")
    return log


def verify_local_image(image_id: str, manifest: Path, digest: str) -> str:
    """Local byte evidence, not an attestation or repository manifest digest."""
    if not re.fullmatch(r"sha256:[0-9a-f]{64}", image_id):
        raise ValueError("local image requires actual Docker image ID")
    data = manifest.read_bytes()
    if hashlib.sha256(data).hexdigest() != digest:
        raise ValueError("frozen source manifest digest mismatch")
    parsed = json.loads(data)
    if set(parsed) != {"files"} or not isinstance(parsed["files"], dict):
        raise ValueError("manifest requires exact files hash map")
    files = parsed["files"]
    if len(data) > 1 << 20 or len(files) > 1024:
        raise ValueError("source manifest exceeds bounded scope")
    required = {
        "src/hypertrain/relay/core.py",
        "src/hypertrain/relay/app.py",
        "src/hypertrain/data/stream_store.py",
    }
    if not required <= files.keys() or any(
        not isinstance(name, str)
        or not isinstance(value, str)
        or not name.startswith("src/hypertrain/")
        or ".." in Path(name).parts
        or not re.fullmatch(r"[0-9a-f]{64}", value)
        for name, value in files.items()
    ):
        raise ValueError("invalid/missing frozen source hashes")
    inspected = json.loads(command("docker", "image", "inspect", image_id))
    if len(inspected) != 1 or inspected[0]["Id"] != image_id:
        raise ValueError("Docker image inspection differs from admitted image ID")
    check = (
        "import hashlib,json,pathlib,sys,hypertrain; f=json.loads(sys.stdin.read()); "
        "root=pathlib.Path(hypertrain.__file__).parent; "
        "actual={'src/hypertrain/'+p.relative_to(root).as_posix() for p in root.rglob('*') "
        "if p.is_file() and '__pycache__' not in p.parts and p.suffix!='.pyc'}; "
        "assert actual==set(f),(actual-set(f),set(f)-actual); "
        "bad=[n for n,h in f.items() if hashlib.sha256("
        "(root/n.removeprefix('src/hypertrain/')).read_bytes()).hexdigest()!=h]; "
        "assert not bad,bad; print('LOCAL_SOURCE_BYTES_VERIFIED')"
    )
    result = command(
        "docker",
        "run",
        "--rm",
        "-i",
        "--tmpfs",
        "/data:rw,size=16m,uid=65532,gid=65532",
        "--cpuset-cpus=0-3",
        "--cpus=0.5",
        "--memory=256m",
        "--memory-swap=256m",
        "--entrypoint",
        "/opt/venv/bin/python",
        image_id,
        "-c",
        check,
        input_text=json.dumps(files),
    )
    if result.strip() != "LOCAL_SOURCE_BYTES_VERIFIED":
        raise ValueError("image source verification did not finish")
    tag = "hypertrain-relay-proof:" + image_id.split(":", 1)[1]
    command("docker", "tag", image_id, tag)
    return tag


def validate_node_limits(limits: dict) -> None:
    quota = limits.get("NanoCpus", 0) / 1e9
    if not quota and limits.get("CpuPeriod", 0) > 0:
        quota = limits.get("CpuQuota", 0) / limits["CpuPeriod"]
    if (
        limits.get("CpusetCpus") != "0-3"
        or not 0 < quota <= 3.5
        or not 0 < limits.get("Memory", 0) <= 4 << 30
        or limits.get("MemorySwap") != limits["Memory"]
    ):
        raise ValueError("node lacks admitted positive CPU/memory/swap constraints")


def runner_admission(*, archive: bool = False) -> None:
    """Archive reserves caller0.05/128MiB within original aggregate4CPU/4.5GiB."""
    group = next(
        line.split(":", 2)[2]
        for line in Path("/proc/self/cgroup").read_text().splitlines()
        if line.startswith("0::")
    )
    path = Path("/sys/fs/cgroup") / group.lstrip("/")
    quota, period = (path / "cpu.max").read_text().split()
    memory = (path / "memory.max").read_text().strip()
    if (
        quota == "max"
        or not 0 < int(quota) / int(period) <= (0.40 if archive else 0.45)
        or memory == "max"
        or not 0 < int(memory) <= (256 if archive else 384) << 20
        or (path / "memory.swap.max").read_text().strip() != "0"
        or set(os.sched_getaffinity(0)) - {0, 1, 2, 3}
    ):
        raise ValueError("runner exceeds admitted CPU/memory/swap/cpuset budget")


def integration_workload(
    args: argparse.Namespace, image: str, kubectl: list[str], node: str
) -> dict:
    """Root-only live execution, real authority/driver; no fixture credit or manifest mutation."""
    prepared = Path(args.integration_root).resolve(strict=True)
    if (prepared / "rescue").exists() or (prepared / "state/service").exists():
        raise ValueError("fresh integration state/rescue required")
    config = IntegrationServiceConfig.model_validate_json(
        (prepared / "configs/service.json").read_bytes()
    )
    inspect_integration_snapshot(prepared / "bundle", config.admitted_hash)
    for name in (
        "relay_kind_smoke.py",
        "network_authority_snapshot.py",
        "network_service_proof.py",
    ):
        digest = hashlib.sha256((ROOT / "scripts" / name).read_bytes()).hexdigest()
        if digest != json.loads(Path(args.integration_scripts).read_bytes())[name]:
            raise ValueError("integration script differs from root-admitted source freeze")
    command("docker", "exec", node, "mkdir", "-p", "/proof")
    command("docker", "cp", str(prepared) + "/.", node + ":/proof")
    command("docker", "exec", node, "chown", "-R", "65532:65532", "/proof")
    apply(
        kubectl,
        {
            "apiVersion": "v1",
            "kind": "Namespace",
            "metadata": {"name": "hypertrain-d3"},
        },
    )
    apply(
        kubectl,
        {
            "apiVersion": "v1",
            "kind": "ConfigMap",
            "metadata": {"name": "actual-scripts", "namespace": "hypertrain-d3"},
            "data": {
                name: (ROOT / "scripts" / name).read_text()
                for name in (
                    "relay_kind_smoke.py",
                    "network_authority_snapshot.py",
                    "network_service_proof.py",
                )
            },
        },
    )
    rendered = integration_manifests(image, local_loaded=bool(args.local_image_id))
    job = next(i for i in rendered["items"] if i["kind"] == "Job")
    for item in rendered["items"]:
        if item["kind"] not in {"Job", "Namespace"} and not (
            item["kind"] == "Pod" and item["metadata"]["name"] != "service"
        ):
            apply(kubectl, item)
    for role in ("service", "relay", "gateway"):
        if role != "service":
            apply(
                kubectl,
                next(
                    i
                    for i in rendered["items"]
                    if i["kind"] == "Pod" and i["metadata"]["name"] == role
                ),
            )
        # Runtime readiness is an exact kube event; gateway TLS is tested by driver.
        command(
            *kubectl,
            "wait",
            "--for=condition=Ready",
            "pod/" + role,
            "-n",
            "hypertrain-d3",
            "--timeout=900s",
            timeout=920,
        )
    relay_pod = next(
        i for i in rendered["items"] if i["kind"] == "Pod" and i["metadata"]["name"] == "relay"
    )
    before = json.loads(command(*kubectl, "get", "pod/relay", "-n", "hypertrain-d3", "-o", "json"))[
        "metadata"
    ]["uid"]
    command(
        *kubectl,
        "delete",
        "pod/relay",
        "-n",
        "hypertrain-d3",
        "--wait=true",
        "--timeout=60s",
    )
    apply(kubectl, relay_pod)
    command(
        *kubectl,
        "wait",
        "--for=condition=Ready",
        "pod/relay",
        "-n",
        "hypertrain-d3",
        "--timeout=60s",
    )
    after = json.loads(command(*kubectl, "get", "pod/relay", "-n", "hypertrain-d3", "-o", "json"))[
        "metadata"
    ]["uid"]
    if before == after:
        raise ValueError("pod fault did not replace real relay")
    negative = json.loads(json.dumps(job))
    negative["metadata"]["name"] = "wrong-ca"
    negative["spec"]["activeDeadlineSeconds"] = 60
    neg_container = negative["spec"]["template"]["spec"]["containers"][0]
    neg_container["command"][neg_container["command"].index("driver")] = "wrong-ca"
    neg_container["resources"] = {
        "requests": {"cpu": "100m", "memory": "128Mi"},
        "limits": {"cpu": "100m", "memory": "128Mi"},
    }
    neg_container["env"] = [
        {"name": name, "value": "1"} for name in ("OMP_NUM_THREADS", "MKL_NUM_THREADS")
    ]
    apply(kubectl, negative)
    command(
        *kubectl,
        "wait",
        "--for=condition=complete",
        "job/wrong-ca",
        "-n",
        "hypertrain-d3",
        "--timeout=60s",
    )
    if "TLS_WRONG_CA_REJECTED" not in command(
        *kubectl, "logs", "job/wrong-ca", "-n", "hypertrain-d3"
    ):
        raise ValueError("real wrong-CA negative lacks TLS refusal evidence")
    apply(kubectl, job)
    try:
        command(
            *kubectl,
            "wait",
            "--for=condition=complete",
            "job/driver",
            "-n",
            "hypertrain-d3",
            "--timeout=9600s",
            timeout=9620,
        )
    finally:
        command("docker", "cp", node + ":/proof/.", str(prepared / "rescue"), timeout=300)
    evidence = prepared / "rescue/work/driver/evidence"
    result = json.loads((evidence / "result.json").read_bytes())
    if result["status"] != "PASS":
        raise ValueError("actual driver failed; rescued evidence preserved")
    return {
        "full_D3_integration": "ACTUAL_DRIVER_PASS",
        "driver": result,
        "relay_replaced_uids": [before, after],
        "wrong_ca": "TLS_WRONG_CA_REJECTED",
        "registry": "original-local",
        "rescue": str(evidence),
    }


def prepare_integration_root(destination: Path, bundle_dir: Path, digest: str) -> None:
    """Write exact public role/config topology; original private secrets/TLS supplied by root."""
    import network_authority_snapshot as authority

    authority.validate(bundle_dir, digest)
    destination.mkdir(mode=0o700, parents=True, exist_ok=False)
    shutil.copytree(bundle_dir, destination / "bundle")
    for name in (
        "configs",
        "state",
        "tls",
        "secrets/service",
        "secrets/relay",
        "secrets/driver",
        "work/service",
        "work/relay",
        "work/driver",
    ):
        (destination / name).mkdir(mode=0o700, parents=True, exist_ok=True)
    configs = {
        "service": {
            "snapshot": "/proof/bundle",
            "admitted_hash": digest,
            "state": "/proof/state/service",
            "secrets": "/proof/secrets",
            "master_url": "https://master.test",
            "host": str(ipaddress.IPv4Address(0)),
            "port": 8080,
        },
        "relay": {
            "state": "/proof/state/service",
            "backing": "/proof/work/relay",
            "ca": "/proof/tls/ca.crt",
            "master_url": "https://master.test",
            "relay_seed": "/proof/secrets/relay-0.seed",
            "admin_token": "/proof/secrets/admin.token",
            "drain_token": "/proof/secrets/drain.token",
            "host": str(ipaddress.IPv4Address(0)),
            "port": 8000,
        },
        "driver": {
            "master_https": "https://master.test",
            "relay_https": "https://relay.test",
            "ca": "/proof/tls/ca.crt",
            "snapshot": "/proof/bundle",
            "snapshot_digest": digest,
            "secrets": "/proof/secrets",
            "service_state": "/proof/state/service",
            "evidence": "/proof/work/evidence",
            "verified_beacon": "/proof/work/verified-beacon.json",
            "fixture_clock_directory": "/proof/state/service/fixture-clock",
            "fixture_clock_offset": 0.0,
            "rounds": 2,
            "child_timeout": 900,
        },
    }
    for role, data in configs.items():
        (destination / "configs" / (role + ".json")).write_text(json.dumps(data, indent=2))
    sources = {
        name: hashlib.sha256((ROOT / "scripts" / name).read_bytes()).hexdigest()
        for name in (
            "relay_kind_smoke.py",
            "network_authority_snapshot.py",
            "network_service_proof.py",
        )
    }
    (destination / "scripts.json").write_text(json.dumps(sources, sort_keys=True))


def provision_original_fixture_secrets(root: Path, bundle_dir: Path, digest: str) -> None:
    """Materialize original test identities separately; never sign historical operations."""
    import network_authority_snapshot as authority

    bundle = authority.validate(bundle_dir, digest)
    original = {
        "coord": bytes(range(32)),
        "owner": bytes([113]) * 32,
        "auditor-0": bytes(31) + b"\x01",
        "auditor-1": bytes([114]) * 32,
        "referee-0": bytes([115]) * 32,
        "relay-0": bytes([116]) * 32,
        **{f"hot-{i}": bytes([80 + i]) * 32 for i in range(4)},
        **{f"cold-{i}": bytes([90 + i]) * 32 for i in range(4)},
    }
    by_public = {Keypair(raw).ss58: raw for raw in original.values()}
    if set(by_public) != set(bundle.public_roles.values()):
        raise ValueError("known original fixture identities differ from admitted bundle")
    original = {role: by_public[public] for role, public in bundle.public_roles.items()}
    for role, names in {
        "service": set(original),
        "driver": {"coord", "auditor-0", "auditor-1", *(f"hot-{i}" for i in range(4))},
        "relay": {"relay-0"},
    }.items():
        directory = root / "secrets" / role
        for name in names:
            path = directory / (name + ".seed")
            with path.open("xb") as stream:
                stream.write(original[name])
            path.chmod(0o600)
        with (directory / "admin.token").open("x") as stream:
            stream.write("admin-test-token")
        (directory / "admin.token").chmod(0o600)
        if role == "relay":
            with (directory / "drain.token").open("x") as stream:
                stream.write(secrets.token_hex(32))
            (directory / "drain.token").chmod(0o600)


def provider_inventory(
    provider: str, execute: Callable[..., str] = command
) -> dict[str, list[str]]:
    """Successful parsed snapshots only; malformed output is never absence."""
    result = {}
    for resource, argv in (
        ("containers", ["ps", "-aq", "--no-trunc"]),
        ("volumes", ["volume", "ls", "-q"]),
        ("networks", ["network", "ls", "-q", "--no-trunc"]),
    ):
        if provider == "podman" and resource == "networks":
            try:
                rows = json.loads(execute(provider, "network", "ls", "--format", "json"))
            except json.JSONDecodeError as error:
                raise ValueError("malformed podman networks inventory") from error
            if not isinstance(rows, list) or any(
                not isinstance(row, dict)
                or not isinstance(row.get("id"), str)
                or not isinstance(row.get("name"), str)
                or not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.-]*", row["name"])
                for row in rows
            ):
                raise ValueError("malformed podman networks inventory")
            values = [row["id"] for row in rows]
            names = [row["name"] for row in rows]
            if len(names) != len(set(names)):
                raise ValueError("duplicate podman network names")
            result["network_names"] = sorted(names)
        else:
            values = execute(provider, *argv).splitlines()
        pattern = r"[0-9a-f]{64}" if resource != "volumes" else r"[A-Za-z0-9][A-Za-z0-9_.-]*"
        if len(values) != len(set(values)) or any(not re.fullmatch(pattern, v) for v in values):
            raise ValueError("malformed " + provider + " " + resource + " inventory")
        result[resource] = sorted(values)
    return result


def admitted_host_config_matches(actual: dict, expected: dict) -> bool:
    """Keep admitted resources/security immutable; Docker default false/null is equivalent."""
    fields = (
        "Memory",
        "MemorySwap",
        "NanoCpus",
        "CpuQuota",
        "CpuPeriod",
        "CpusetCpus",
        "Privileged",
        "SecurityOpt",
        "CgroupnsMode",
        "Init",
        "Binds",
        "Tmpfs",
    )
    for name in fields:
        if (
            (name in actual) != (name in expected)
            or type(actual.get(name)) is not type(expected.get(name))
            or actual.get(name) != expected.get(name)
        ):
            return False
    before, after = expected.get("OomKillDisable"), actual.get("OomKillDisable")
    if "OomKillDisable" in expected and "OomKillDisable" not in actual:
        return False
    if before is None or before is False:
        return after is None or after is False
    return before is True and after is True


def require_capped_node(journal_path: Path, node: str) -> dict:
    journal = json.loads(journal_path.read_bytes())
    info = json.loads(command("docker", "inspect", node))[0]
    if (
        not journal.get("node_capped_before_start")
        or info["Id"] not in journal.get("owned_containers", [])
        or not admitted_host_config_matches(info["HostConfig"], journal["create_host_config"])
    ):
        raise ValueError("kind returned without journal-owned pre-start caps")
    validate_node_limits(info["HostConfig"])
    if info["Config"]["Labels"].get("io.hypertrain.proof") != journal["experiment"]:
        raise ValueError("created node proof label mismatch")
    return info


def cleanup_owned(journal_path: Path) -> None:
    initial = json.loads(journal_path.read_bytes())
    cutoff = initial.get(
        "cleanup_budget_cutoff_monotonic",
        min(
            initial["cleanup_cutoff_monotonic"],
            time.monotonic() + 1200,
        ),
    )

    def bounded(*args: str, timeout: float = 180) -> str:
        remaining = cutoff - time.monotonic()
        if remaining <= 0:
            state = json.loads(journal_path.read_bytes())
            CleanupJournal(journal_path).write_text(
                json.dumps(
                    {
                        **state,
                        "state": "CLEANUP_CUTOFF",
                        "proof_status": "CENSORED",
                        "absence_verified": False,
                    }
                )
            )
            raise TimeoutError("aggregate cleanup cutoff exhausted")
        try:
            return command(*args, timeout=min(timeout, remaining))
        except Exception:
            state = json.loads(journal_path.read_bytes())
            CleanupJournal(journal_path).write_text(
                json.dumps(
                    {
                        **state,
                        "proof_status": "CENSORED",
                        "absence_verified": False,
                    }
                )
            )
            raise

    """Serialize rescue/cleanup; deletion never touches a baseline resource."""
    with journal_path.with_suffix(".lock").open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        journal = json.loads(journal_path.read_bytes())
        if journal.get("absence_verified"):
            journal.setdefault("absence_history", []).append(
                {
                    "state": journal["state"],
                    "provider_after": journal.get("provider_after"),
                    "absence_verified": True,
                }
            )
        journal["absence_verified"] = False
        journal["provider_after"] = None
        journal["cleanup_budget_cutoff_monotonic"] = cutoff
        CleanupJournal(journal_path).write_text(json.dumps(journal))
        try:
            cluster = journal["cluster"]
            if not re.fullmatch(r"hypertrain-v2-[a-z0-9-]{1,40}", cluster):
                raise ValueError("deadline journal cluster not owned")
            if journal["proof_dir"] != str(journal_path.parent.resolve()):
                raise ValueError("journal proof directory mismatch")
            labelled = bounded(
                "docker",
                "ps",
                "-aq",
                "--no-trunc",
                "--filter",
                "label=io.x-k8s.kind.cluster=" + cluster,
            ).splitlines()
            errors = []
            for provider, baseline_provider in journal["provider_baselines"].items():
                if provider == "docker":
                    continue
                current = provider_inventory(provider, bounded)
                for identity in set(current["containers"]) - set(baseline_provider["containers"]):
                    info = json.loads(bounded(provider, "inspect", identity))[0]
                    if info["Config"]["Labels"].get("io.x-k8s.kind.cluster") != cluster:
                        continue
                    if (
                        info["Id"] != identity
                        or info["Config"]["Labels"].get("io.hypertrain.proof")
                        != journal["experiment"]
                    ):
                        raise ValueError("unexpected provider node lacks verified proof ownership")
                    rescue_other = journal_path.parent / "rescue" / provider
                    rescue_other.mkdir(parents=True, exist_ok=True)
                    (rescue_other / (identity + "-inspect.json")).write_text(json.dumps(info))
                    if info["State"]["Running"]:
                        bounded(provider, "stop", "--time", "3", identity, timeout=15)
                    try:
                        (rescue_other / (identity + "-logs.txt")).write_text(
                            bounded(provider, "logs", "--tail", "100", identity, timeout=30)
                        )
                    except Exception as error:
                        errors.append(provider + ": " + str(error))
                    bounded(provider, "rm", identity)
                    for mount in info["Mounts"]:
                        if (
                            mount["Type"] != "volume"
                            or mount["Name"] in baseline_provider["volumes"]
                        ):
                            continue
                        volume = json.loads(bounded(provider, "volume", "inspect", mount["Name"]))[
                            0
                        ]
                        if volume.get("Labels", {}).get(cluster + "-control-plane") == "true":
                            refs = bounded(
                                provider, "ps", "-aq", "--filter", "volume=" + mount["Name"]
                            )
                            if not refs.strip():
                                bounded(provider, "volume", "rm", mount["Name"])
            for container in labelled:
                info = json.loads(bounded("docker", "inspect", container))[0]
                if (
                    info["Id"] in journal["containers"]
                    or info["Config"]["Labels"].get("io.hypertrain.proof") != journal["experiment"]
                ):
                    raise ValueError("cluster ownership conflicts with journal")
                journal.setdefault("owned_containers", []).append(info["Id"])
                for mount in info["Mounts"]:
                    if mount["Type"] == "volume" and mount["Name"] not in journal["volumes"]:
                        journal.setdefault("owned_volumes", []).append(mount["Name"])
                for network in info["NetworkSettings"]["Networks"].values():
                    if network["NetworkID"] not in journal["networks"]:
                        journal.setdefault("owned_networks", []).append(network["NetworkID"])
            rescue = journal_path.parent / "rescue"
            rescue.mkdir(exist_ok=True)
            journal["state"] = "RESCUE_INTENT"
            CleanupJournal(journal_path).write_text(json.dumps(journal))
            if labelled:
                kube = [
                    "kubectl",
                    "--kubeconfig",
                    journal["kubeconfig"],
                    "--context",
                    "kind-" + cluster,
                ]
                captures = [
                    (
                        ["kind", "export", "logs", str(rescue / "logs"), "--name", cluster],
                        "kind-logs.txt",
                    ),
                    (
                        kube
                        + [
                            "get",
                            "pods,jobs,services,configmaps,events",
                            "-A",
                            "-o",
                            "json",
                        ],
                        "objects.json",
                    ),
                ]
                for argv, name in captures:
                    try:
                        (rescue / name).write_text(bounded(*argv, timeout=60))
                    except Exception as error:
                        errors.append(name + ": " + str(error))
                paths = [
                    "/var/local/hypertrain-proof-store",
                    "/var/local/hypertrain-proof-master",
                ]
                if journal.get("integration_root"):
                    paths = ["/proof/work", "/proof/state"]
                for path in paths:
                    try:
                        bounded(
                            "docker",
                            "cp",
                            cluster + "-control-plane:" + path,
                            str(rescue / Path(path).name),
                            timeout=120,
                        )
                    except Exception as error:
                        errors.append(path + ": " + str(error))
            journal["rescue_errors"] = errors
            journal["proof_status"] = (
                "CENSORED" if errors else journal.get("proof_status", "INTERRUPTED")
            )
            journal["state"] = "DELETE_INTENT"
            CleanupJournal(journal_path).write_text(json.dumps(journal))
            try:
                if labelled:
                    bounded("kind", "delete", "cluster", "--name", cluster, timeout=120)
            except Exception as error:
                journal["delete_error"] = str(error)
                journal["proof_status"] = "CENSORED"
            finally:
                remaining = set(bounded("docker", "ps", "-aq", "--no-trunc").splitlines())
                for identity in set(journal.get("owned_containers", [])) & remaining:
                    bounded("docker", "rm", "-f", identity)
                for resource, field in (
                    ("volume", "owned_volumes"),
                    ("network", "owned_networks"),
                ):
                    argv = ["docker", resource, "ls", "-q"]
                    if resource == "network":
                        argv.append("--no-trunc")
                    remaining = set(bounded(*argv).splitlines())
                    for identity in set(journal.get(field, [])) & remaining:
                        bounded("docker", resource, "rm", identity)
            after = {
                provider: provider_inventory(provider, bounded)
                for provider in journal["provider_baselines"]
            }
            if any(after[p] != journal["provider_baselines"][p] for p in after):
                journal["state"] = "CLEANUP_FAILED"
                journal["proof_status"] = "CENSORED"
                CleanupJournal(journal_path).write_text(json.dumps(journal))
                raise ValueError("cleanup inventory differs from admitted baseline")
            journal["state"] = "CLEANUP_VERIFIED"
            journal["absence_verified"] = True
            journal["provider_after"] = after
            CleanupJournal(journal_path).write_text(json.dumps(journal))

        except Exception:
            current = json.loads(journal_path.read_bytes())
            current["absence_verified"] = False
            current["provider_after"] = None
            current["proof_status"] = "CENSORED"
            if current["state"] != "CLEANUP_CUTOFF":
                current["state"] = "CLEANUP_FAILED"
            CleanupJournal(journal_path).write_text(json.dumps(current))
            raise


def deadline_supervisor(
    journal_path: Path, parent_pid: int, ready_socket: Path | None = None
) -> None:
    """READY before CREATE; pidfd and immutable absolute monotonic deadline."""
    journal = json.loads(journal_path.read_bytes())
    if journal["boot_id"] != Path("/proc/sys/kernel/random/boot_id").read_text().strip():
        raise ValueError("supervisor clock boot mismatch")
    try:
        fd = os.pidfd_open(parent_pid)
    except ProcessLookupError:
        fd = None
    if ready_socket is not None:
        group = Path("/proc/self/cgroup").read_text().strip()
        cgroup = Path("/sys/fs/cgroup") / group.split("::")[1].lstrip("/")
        controls = {
            name: (cgroup / name).read_text().strip()
            for name in ("cpu.max", "memory.max", "memory.swap.max", "cpuset.cpus.effective")
        }
        quota, period = controls["cpu.max"].split()
        if (
            quota == "max"
            or int(quota) / int(period) > 0.05
            or int(controls["memory.max"]) > 128 << 20
            or controls["memory.swap.max"] != "0"
            or controls["cpuset.cpus.effective"] != "0-3"
        ):
            raise ValueError("supervisor unit exceeds admitted reserve")
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as connection:
            connection.settimeout(30)
            connection.connect(str(ready_socket))
            connection.sendall(
                json.dumps(
                    {
                        "state": "READY",
                        "unit": journal["cluster"] + "-supervisor",
                        "cgroup": group,
                        "controls": controls,
                    }
                ).encode()
            )
    if fd is not None:
        try:
            poll = select.poll()
            poll.register(fd, select.POLLIN)
            remaining = max(0, journal["deadline_monotonic"] - time.monotonic())
            if not poll.poll(math.ceil(remaining * 1000)):
                signal.pidfd_send_signal(fd, signal.SIGKILL)
                poll.poll(30000)
        finally:
            os.close(fd)
    cleanup_owned(journal_path)
    print("SUPERVISOR_CLEANUP_DONE", flush=True)


def start_supervisor(journal_path: Path) -> None:
    journal = json.loads(journal_path.read_bytes())
    unit = journal["cluster"] + "-supervisor"
    if (
        command("systemctl", "show", unit + ".service", "--property=LoadState", "--value").strip()
        != "not-found"
    ):
        raise ValueError("pre-existing supervisor unit ownership conflict")
    remaining = journal["cleanup_cutoff_monotonic"] - time.monotonic()
    if remaining <= 0:
        raise ValueError("supervisor cutoff already expired")
    listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    ready_directory = tempfile.TemporaryDirectory(prefix="ht-ready-")
    ready_path = Path(ready_directory.name) / "ready.sock"
    listener.bind(str(ready_path))
    listener.listen(1)
    listener.settimeout(30)
    created = False
    try:
        command(
            "systemd-run",
            "--unit=" + unit,
            "--property=CPUQuota=5%",
            "--property=MemoryMax=128M",
            "--property=MemorySwapMax=0",
            "--property=AllowedCPUs=0-3",
            "--property=Nice=19",
            "--property=RuntimeMaxSec=" + str(remaining),
            "--property=KillMode=control-group",
            "--setenv=OMP_NUM_THREADS=1",
            "--setenv=MKL_NUM_THREADS=1",
            "--setenv=PYTHONDONTWRITEBYTECODE=1",
            "--setenv=PATH=" + os.environ["PATH"],
            "--property=StandardOutput=append:" + str(journal_path.parent / "supervisor.log"),
            "--property=StandardError=append:" + str(journal_path.parent / "supervisor.log"),
            sys.executable,
            str(Path(__file__).resolve()),
            "--supervise",
            str(journal_path),
            "--parent-pid",
            str(os.getpid()),
            "--ready-socket",
            str(ready_path),
        )
        created = True
        with listener.accept()[0] as connection:
            connection.settimeout(30)
            ready = json.loads(connection.recv(2048))
        own_group = Path("/proc/self/cgroup").read_text().strip()
        if ready["state"] != "READY" or ready["unit"] != unit or ready["cgroup"] == own_group:
            raise ValueError("supervisor unit independence not confirmed")
        CleanupJournal(journal_path).write_text(json.dumps({"supervisor_ready": ready}))
    except Exception:
        if created:
            command("systemctl", "stop", unit)
        raise
    finally:
        listener.close()
        ready_path.unlink(missing_ok=True)
        ready_directory.cleanup()


def docker_create_hook(journal_path: Path, docker: str, args: list[str]) -> None:
    """Kind CLI local provider: create immutable limits, inspect before ANY start."""

    def exit_status(status: int) -> None:
        if status < 0:
            if -status not in {signal.SIGKILL, signal.SIGSTOP}:
                signal.signal(-status, signal.SIG_DFL)
            os.kill(os.getpid(), -status)
        sys.exit(status)

    def forward(argv: list[str]) -> None:
        pid = os.posix_spawn(docker, [docker, *argv], os.environ)
        exit_status(os.waitstatus_to_exitcode(os.waitpid(pid, 0)[1]))

    def budget(cleanup: bool = False) -> float:
        current = json.loads(journal_path.read_bytes())
        cutoff = min(
            current["cleanup_cutoff_monotonic"],
            current.get("cleanup_budget_cutoff_monotonic", current["cleanup_cutoff_monotonic"]),
        )
        if not cleanup:
            cutoff = min(cutoff, current["deadline_monotonic"])
        remaining = cutoff - time.monotonic()
        if remaining <= 0:
            raise TimeoutError("node gate absolute budget exhausted")
        return remaining

    def read_control(path: Path) -> str:
        budget()
        value = path.read_text().strip()
        budget()
        return value

    def save_receipt(receipt: dict, *, cleanup: bool = False) -> None:
        budget(cleanup)
        CleanupJournal(journal_path).write_text(json.dumps({"node_kernel_receipt": receipt}))
        budget(cleanup)

    def start_owned(identity: str) -> None:
        receipt: dict = {
            "identity": identity,
            "verified": False,
            "controls": {},
            "cgroup_read_reached": False,
        }
        try:
            remaining = budget()

            async def start() -> int:
                with anyio.fail_after(min(60, remaining)):
                    async with await anyio.open_process(
                        [docker, "start", identity], stdin=None, stdout=None, stderr=None
                    ) as process:
                        return await process.wait()

            status = anyio.run(start)
            if status:
                exit_status(status)
            info = json.loads(command(docker, "inspect", identity, timeout=min(10, budget())))[0]
            pid = info["State"]["Pid"]
            receipt["pid"] = pid
            resource_fields = (
                "NanoCpus",
                "CpuQuota",
                "CpuPeriod",
                "Memory",
                "MemorySwap",
                "CpusetCpus",
                "Privileged",
                "SecurityOpt",
                "CgroupnsMode",
                "Init",
                "Binds",
                "Tmpfs",
                "OomKillDisable",
            )
            receipt["observed"] = {
                "container_id": str(info["Id"])[:256],
                "pid": pid,
                "running": info["State"]["Running"],
                "proof_label": str(info["Config"]["Labels"].get("io.hypertrain.proof"))[:256],
                "cluster_label": str(info["Config"]["Labels"].get("io.x-k8s.kind.cluster"))[:256],
                "resources": {name: info["HostConfig"].get(name) for name in resource_fields},
            }
            receipt["expected"] = {
                "container_id": identity,
                "pid": "positive strict integer",
                "running": True,
                "proof_label": journal["cluster"],
                "cluster_label": journal["cluster"],
                "resources": {
                    name: journal["create_host_config"].get(name) for name in resource_fields
                },
            }
            receipt["host_config_changed_fields"] = sorted(
                name
                for name in info["HostConfig"].keys() | journal["create_host_config"].keys()
                if info["HostConfig"].get(name) != journal["create_host_config"].get(name)
                or (name in info["HostConfig"]) != (name in journal["create_host_config"])
            )[:128]
            receipt["rejection_reasons"] = [
                reason
                for reason, failed in (
                    ("container_id", info["Id"] != identity),
                    ("running", not info["State"]["Running"]),
                    ("pid", type(pid) is not int or pid <= 0),
                    (
                        "host_config",
                        not admitted_host_config_matches(
                            info["HostConfig"], journal["create_host_config"]
                        ),
                    ),
                    (
                        "proof_label",
                        info["Config"]["Labels"].get("io.hypertrain.proof") != journal["cluster"],
                    ),
                )
                if failed
            ]
            if receipt["rejection_reasons"]:
                raise ValueError("started node identity/PID mismatch")
            if not re.fullmatch(r"[0-9a-f]{64}", identity):
                raise ValueError("node requires full owned container ID")
            receipt["cgroup_read_reached"] = True
            group = read_control(Path(f"/proc/{pid}/cgroup"))
            receipt["cgroup"] = group
            if not group.startswith("0::/") or "\n" in group:
                raise ValueError("node requires unified cgroup")
            relative = group.removeprefix("0::/")
            if not relative or ".." in Path(relative).parts:
                raise ValueError("invalid node cgroup path")
            parts = Path(relative).parts
            authority = next(
                (
                    i
                    for i, part in enumerate(parts)
                    if (
                        i == 1 and parts[0] == "system.slice" and part == f"docker-{identity}.scope"
                    )
                    or (i == 1 and parts[0] == "docker" and part == identity)
                ),
                None,
            )
            if authority is None:
                raise ValueError("node cgroup does not match exact owned container ID")
            parent = Path("/sys/fs/cgroup").joinpath(*parts[: authority + 1])
            receipt["authority_cgroup"] = str(parent)
            for name in ("memory.max", "cpu.max", "memory.swap.max", "cpuset.cpus.effective"):
                receipt["controls"][name] = read_control(parent / name)
            quota, period = map(int, receipt["controls"]["cpu.max"].split())
            if (
                quota <= 0
                or period <= 0
                or quota * 2 != period * 7
                or receipt["controls"]["memory.max"] != str(4 << 30)
                or receipt["controls"]["memory.swap.max"] != "0"
                or receipt["controls"]["cpuset.cpus.effective"] != "0-3"
            ):
                raise ValueError("node kernel limits mismatch")
            receipt["descendants"] = {}
            for depth in range(authority + 2, len(parts) + 1):
                descendant = Path("/sys/fs/cgroup").joinpath(*parts[:depth])
                controls: dict[str, str] = {}
                receipt["descendants"][str(descendant)] = controls
                for name in ("memory.max", "cpu.max", "memory.swap.max", "cpuset.cpus.effective"):
                    controls[name] = read_control(descendant / name)
                # Ancestor limits apply to every descendant, including leaf "max".
                for name in ("memory.max", "memory.swap.max"):
                    if controls[name] != "max" and int(controls[name]) < 0:
                        raise ValueError("invalid descendant kernel limit")
                leaf_quota, leaf_period = controls["cpu.max"].split()
                if int(leaf_period) <= 0 or (leaf_quota != "max" and int(leaf_quota) <= 0):
                    raise ValueError("invalid descendant CPU limit")
                if controls["cpuset.cpus.effective"] != "0-3":
                    raise ValueError("descendant effective CPU set mismatch")
            receipt["effective_bound"] = {"cpu": 3.5, "memory": 4 << 30, "swap": 0}
            current = json.loads(command(docker, "inspect", identity, timeout=min(10, budget())))[0]
            if current["Id"] != identity or current["State"] != info["State"]:
                raise ValueError("node identity/PID changed during kernel inspection")
            if read_control(Path(f"/proc/{pid}/cgroup")) != group:
                raise ValueError("node cgroup changed during kernel inspection")
            receipt["verified"] = True
            save_receipt(receipt)
        except Exception as error:
            receipt["verified"] = False
            receipt["error"] = str(error)
            try:
                save_receipt(receipt, cleanup=True)
            except Exception as diagnostic_error:
                receipt["diagnostic_error"] = str(diagnostic_error)
            try:
                command(docker, "stop", "--time=0", identity, timeout=min(30, budget(True)))
                receipt["stopped"] = True
            except Exception as stop_error:
                receipt["stop_error"] = str(stop_error)
            finally:
                try:
                    save_receipt(receipt, cleanup=True)
                except Exception as diagnostic_error:
                    receipt["diagnostic_error"] = str(diagnostic_error)
            raise

    journal = json.loads(journal_path.read_bytes())
    if not args:
        raise ValueError("empty Docker operation")
    operation = args[0]
    if args in (["-v"], ["--version"]):
        forward(args)
    if operation == "pull":
        versions = Versions.model_validate_json((ROOT / "deploy/k8s/versions.json").read_bytes())
        if args != ["pull", versions.node_image]:
            raise ValueError("pull requires exact trusted pinned kind node image")
        forward(args)
    if operation in {"create", "run"}:
        cluster = journal["cluster"]
        if any(
            a.startswith(
                ("--cpus", "--cpuset", "--cpu-", "--memory", "--label=io.hypertrain.proof")
            )
            for a in args[1:]
        ):
            raise ValueError("kind cannot override admitted node resource policy")
        identity = command(
            docker,
            "create",
            "--cpuset-cpus=0-3",
            "--cpus=3.5",
            "--memory=4g",
            "--memory-swap=4g",
            "--label=io.hypertrain.proof=" + cluster,
            *[a for a in args[1:] if a not in {"-d", "--detach"} and not a.startswith("--detach=")],
            timeout=min(180, budget()),
        ).strip()
        info = json.loads(command(docker, "inspect", identity, timeout=min(10, budget())))[0]
        validate_node_limits(info["HostConfig"])
        if info["State"]["Running"] or info["Id"] in journal["containers"]:
            raise ValueError("node create must be stopped and new")
        labels = info["Config"]["Labels"]
        if (
            labels.get("io.hypertrain.proof") != cluster
            or labels.get("io.x-k8s.kind.cluster") != cluster
        ):
            raise ValueError("node create ownership label mismatch")
        budget()
        CleanupJournal(journal_path).write_text(
            json.dumps(
                {
                    "owned_containers": [info["Id"]],
                    "create_host_config": info["HostConfig"],
                    "node_capped_before_start": True,
                }
            )
        )
        budget()
        if operation == "run":
            journal = json.loads(journal_path.read_bytes())
            start_owned(info["Id"])
        else:
            print(identity)
    elif operation == "start":
        if len(args) != 2:
            raise ValueError("start requires one exact owned node")
        for identity in args[1:]:
            if identity.startswith("-"):
                raise ValueError("unexpected kind start option")
            info = json.loads(command(docker, "inspect", identity, timeout=min(10, budget())))[0]
            validate_node_limits(info["HostConfig"])
            if info["Id"] not in journal.get(
                "owned_containers", []
            ) or not admitted_host_config_matches(
                info["HostConfig"], journal["create_host_config"]
            ):
                raise ValueError("start requires journal-owned capped node")
        start_owned(info["Id"])
    elif operation in {"update", "container"}:
        raise ValueError("Docker operation bypasses immutable create policy")
    elif operation in {
        "inspect",
        "info",
        "version",
        "ps",
        "images",
        "image",
        "load",
        "save",
        "exec",
        "cp",
        "network",
        "rm",
        "logs",
        "wait",
        "stop",
        "kill",
    }:
        forward(args)
    else:
        raise ValueError("unsupported local kind Docker operation: " + operation)


def smoke(args: argparse.Namespace) -> None:
    if args.image_archive:
        archive_cutoffs(args)
        with transport_archive(args) as archive:
            _smoke(args, archive)
    else:
        if any((args.archive_sha256, args.archive_image_id, args.archive_reference)):
            raise ValueError("archive admission fields require image-archive")
        _smoke(args)


class CleanupEvents:
    """File/process subscriptions armed before dispatch; no timed polling."""

    def __init__(self, proof: Path) -> None:
        self.proof = proof
        self.libc = ctypes.CDLL(None, use_errno=True)
        self.fd = self.libc.inotify_init1(os.O_NONBLOCK | os.O_CLOEXEC)
        if self.fd < 0:
            raise OSError(ctypes.get_errno(), "inotify_init1")
        self.processes: dict[int, int] = {}
        self.watched = False
        if self.libc.inotify_add_watch(self.fd, os.fsencode(proof.parent), 0x100 | 0x80) < 0:
            os.close(self.fd)
            raise OSError(ctypes.get_errno(), "inotify parent watch")

    def wait(self, cutoff: float, supervisor_pid: int) -> None:
        if not self.watched and self.proof.exists():
            if self.libc.inotify_add_watch(self.fd, os.fsencode(self.proof), 0x8 | 0x80) < 0:
                raise OSError(ctypes.get_errno(), "inotify proof watch")
            self.watched = True
            return  # Re-read after arming: closes the receipt-before-watch race.
        if supervisor_pid > 0 and supervisor_pid not in self.processes:
            try:
                self.processes[supervisor_pid] = os.pidfd_open(supervisor_pid)
            except ProcessLookupError:
                return  # Terminal state can now be read without a polling delay.
        remaining = cutoff - time.monotonic()
        if remaining <= 0:
            raise TimeoutError("caller original cleanup cutoff exhausted")
        ready, _, _ = select.select([self.fd, *self.processes.values()], [], [], remaining)
        if not ready:
            raise TimeoutError("caller original cleanup cutoff exhausted")
        if self.fd in ready:
            os.read(self.fd, 65536)
        for pid, fd in list(self.processes.items()):
            if fd in ready:
                os.close(fd)
                del self.processes[pid]

    def close(self) -> None:
        for fd in self.processes.values():
            os.close(fd)
        os.close(self.fd)


def caller_resources() -> float:
    """Require the finite 5%/128MiB caller service within the inclusive resource budget."""
    group = Path("/proc/self/cgroup").read_text().strip().removeprefix("0::")
    root = Path("/sys/fs/cgroup") / group.lstrip("/")
    quota, period = (root / "cpu.max").read_text().split()
    if (
        quota == "max"
        or not 0 < int(quota) / int(period) <= 0.05
        or not 0 < int((root / "memory.max").read_text()) <= 128 << 20
        or (root / "memory.swap.max").read_text().strip() != "0"
        or (root / "cpuset.cpus.effective").read_text().strip() != "0-3"
    ):
        raise ValueError("caller requires 5% CPU/128MiB/swap0/cores0-3")
    unit = Path(group).name
    properties = dict(
        line.split("=", 1)
        for line in command(
            "systemctl",
            "show",
            unit,
            "--property=ActiveEnterTimestampMonotonic",
            "--property=RuntimeMaxUSec",
        ).splitlines()
    )
    if properties["RuntimeMaxUSec"] != "3h 22min":
        raise ValueError("caller requires finite 12120s outer cap")
    return int(properties["ActiveEnterTimestampMonotonic"]) / 1e6 + 12120


def caller_launch(args: argparse.Namespace, runner_argv: list[str]) -> None:
    """Keep flock through supervisor terminal/verified absence, otherwise persist INCOMPLETE."""
    if not re.fullmatch(r"[a-z0-9][a-z0-9-]{1,80}", args.caller_unit or ""):
        raise ValueError("caller requires unique outer unit")
    if args.mode != "transport" or not args.image_archive or args.deadline_seconds != 10800:
        raise ValueError("caller requires admitted archive transport 10800s work")
    proof = Path(args.proof_dir).resolve()
    if not proof.parent.is_dir() or proof.exists():
        raise ValueError("caller requires existing owned parent and unique proof path")
    caller_cap = caller_resources()
    lock_path = Path(args.caller_lock)
    barrier = CleanupJournal(ROOT / ".transport-admission.json")
    with os.fdopen(os.open(lock_path, os.O_RDWR | os.O_NOFOLLOW), "a") as lock:
        info = os.fstat(lock.fileno())
        if not stat.S_ISREG(info.st_mode) or info.st_uid != os.geteuid() or info.st_mode & 0o022:
            raise ValueError("caller lock must be existing owned regular no-follow file")
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        if barrier.path.exists() and json.loads(barrier.path.read_bytes())["state"] != "VERIFIED":
            raise ValueError("prior caller admission INCOMPLETE; no dispatch allowed")
        started = time.monotonic()
        work_end = started + 10800
        cutoff = work_end + 1200
        if cutoff >= caller_cap:
            raise ValueError("admitted cleanup endpoint exceeds finite caller cap")
        events = CleanupEvents(proof)
        cancelled = False

        def cancel(*_):
            nonlocal cancelled
            cancelled = True  # Ownership survives cancellation; supervisor owns cleanup.

        previous = {sig: signal.signal(sig, cancel) for sig in (signal.SIGINT, signal.SIGTERM)}
        journal_path = proof / "cleanup.json"
        unit = args.caller_unit + ".service"
        supervisor = args.cluster + "-supervisor.service"
        boot_id = Path("/proc/sys/kernel/random/boot_id").read_text().strip()
        expected_baseline = {
            p: provider_inventory(p, lambda *argv: command(*argv, timeout=10))
            for p in ("docker", "podman")
        }
        primary_error = None
        try:
            # A killed caller leaves ACTIVE, which also blocks subsequent admission.
            barrier.write_text(
                json.dumps(
                    {
                        "state": "ACTIVE",
                        "caller_proof": str(proof),
                        "caller_work_end": work_end,
                        "caller_cleanup_end": cutoff,
                        "caller_boot_id": boot_id,
                    }
                )
            )
            try:
                command(
                    "systemd-run",
                    "--unit=" + args.caller_unit,
                    "--property=CPUQuota=40%",
                    "--property=MemoryMax=256M",
                    "--property=MemorySwapMax=0",
                    "--property=AllowedCPUs=0-3",
                    "--property=Nice=19",
                    "--property=RuntimeMaxSec=12000",
                    "--property=KillMode=control-group",
                    "--setenv=OMP_NUM_THREADS=1",
                    "--setenv=MKL_NUM_THREADS=1",
                    "--setenv=PYTHONDONTWRITEBYTECODE=1",
                    "--setenv=KIND_EXPERIMENTAL_PROVIDER=docker",
                    "--setenv=PATH=" + os.environ["PATH"],
                    "--setenv=TMPDIR=" + os.environ["TMPDIR"],
                    "--property=WorkingDirectory=" + str(ROOT),
                    sys.executable,
                    str(Path(__file__).resolve()),
                    *runner_argv,
                    "--launch-unit",
                    unit,
                    "--work-end",
                    repr(work_end),
                    "--cleanup-end",
                    repr(cutoff),
                    "--clock-boot-id",
                    boot_id,
                    "--caller-cap",
                    repr(caller_cap),
                    timeout=30,
                )
            except (OSError, RuntimeError, TimeoutError, ValueError) as error:
                primary_error = error  # Dispatch may have succeeded; custody cannot unwind.

            def bounded(*argv, timeout=10):
                remaining = cutoff - time.monotonic()
                if remaining <= 0:
                    raise TimeoutError("caller original cleanup cutoff exhausted")
                return command(*argv, timeout=min(timeout, remaining))

            while True:  # Iterations resume only on native file/process notifications.
                pid = 0
                try:
                    state = json.loads(journal_path.read_bytes()) if journal_path.exists() else None
                    if state and state.get("supervisor_ready"):
                        ready = state["supervisor_ready"]
                        if (
                            state["cluster"] != args.cluster
                            or state["experiment"] != args.cluster
                            or state["proof_dir"] != str(proof)
                            or state["boot_id"] != boot_id
                            or ready["state"] != "READY"
                            or ready["unit"] != supervisor.removesuffix(".service")
                            or ready["cgroup"].strip() != "0::/system.slice/" + supervisor
                            or state["provider_baselines"] != expected_baseline
                            or len(state["owned_containers"]) != 1
                            or any(
                                not re.fullmatch(r"[0-9a-f]{64}", identity)
                                or identity in expected_baseline["docker"]["containers"]
                                for identity in state["owned_containers"]
                            )
                        ):
                            raise ValueError(
                                "caller cleanup receipt subject differs from admitted ownership"
                            )
                        if (
                            state["cleanup_cutoff_monotonic"] != cutoff
                            or state["deadline_monotonic"] != work_end
                        ):
                            raise ValueError("caller and journal cutoff differ")
                        status = dict(
                            line.split("=", 1)
                            for line in bounded(
                                "systemctl",
                                "show",
                                supervisor,
                                "--property=MainPID",
                                "--property=ActiveState",
                            ).splitlines()
                        )
                        pid = int(status["MainPID"])
                        log = proof / "supervisor.log"
                        done = (
                            log.exists()
                            and "SUPERVISOR_CLEANUP_DONE" in log.read_text().splitlines()
                        )
                        if done and pid == 0 and status["ActiveState"] in {"inactive", "failed"}:
                            if (
                                state["state"] != "CLEANUP_VERIFIED"
                                or not state.get("absence_verified")
                                or set(state["provider_baselines"]) != {"docker", "podman"}
                            ):
                                raise ValueError(
                                    "supervisor terminal without exact cleanup receipt"
                                )
                            fresh = {
                                p: provider_inventory(p, bounded) for p in ("docker", "podman")
                            }
                            if (
                                fresh != state["provider_baselines"]
                                or fresh != state["provider_after"]
                            ):
                                raise ValueError(
                                    "fresh caller inventories differ from verified cleanup"
                                )
                            barrier.write_text(json.dumps({"state": "VERIFIED"}))
                            break
                except (OSError, RuntimeError, TimeoutError, ValueError, KeyError) as error:
                    if primary_error is None:
                        primary_error = error
                end = cutoff
                try:
                    events.wait(end, pid)
                except TimeoutError as error:
                    if primary_error is not None:
                        raise primary_error from error
                    raise
                except OSError as error:
                    if primary_error is None:
                        primary_error = error
                    # Broken subscription cannot justify early custody release.
                    select.select([], [], [], max(0, end - time.monotonic()))
                    raise primary_error from error
        finally:
            if json.loads(barrier.path.read_bytes()).get("state") != "VERIFIED":
                barrier.write_text(json.dumps({"state": "INCOMPLETE"}))
                if journal_path.exists():
                    try:
                        CleanupJournal(journal_path).write_text(
                            json.dumps({"caller_status": "INCOMPLETE"})
                        )
                    except OSError:
                        if primary_error is None:
                            raise
            events.close()
            for sig, handler in previous.items():
                signal.signal(sig, handler)
        if primary_error is not None:
            raise primary_error
        if cancelled:
            raise KeyboardInterrupt("caller cancelled; verified cleanup completed before release")


def archive_cutoffs(args: argparse.Namespace) -> None:
    """Exact caller-admitted monotonic endpoints, validated before archive admission."""
    if not args.launch_unit or not re.fullmatch(
        r"[a-z0-9][a-z0-9-]{1,80}\.service", args.launch_unit
    ):
        raise ValueError("archive launch requires exact outer service unit")
    if Path("/proc/self/cgroup").read_text().strip() != "0::/system.slice/" + args.launch_unit:
        raise ValueError("archive runner cgroup differs from admitted launch unit")
    now = time.monotonic()
    if (
        args.clock_boot_id != Path("/proc/sys/kernel/random/boot_id").read_text().strip()
        or args.deadline_seconds != 10800
        or not all(
            value is not None and math.isfinite(value)
            for value in (args.work_end, args.cleanup_end, args.caller_cap)
        )
        or not now < args.work_end <= now + 10800
        or args.cleanup_end - args.work_end != 1200
        or not args.cleanup_end < args.caller_cap <= now + 12120
    ):
        raise ValueError("archive boot/deadline endpoints invalid or beyond caller cap")


def archive_workload(text: str, image: str) -> str:
    """Known fixture/rendered YAML image lines: no remote pull fallback."""
    return re.sub(
        r"(?m)^([ \t]*)image: " + re.escape(image) + r"\n(?:[ \t]*imagePullPolicy: [^\n]+\n)?",
        lambda match: f"{match[1]}image: {image}\n{match[1]}imagePullPolicy: Never\n",
        text,
    )


def _smoke(args: argparse.Namespace, archive=None) -> None:
    """Only --create enters cluster lifecycle; --render performs no cluster calls."""
    if archive and time.monotonic() >= args.work_end:
        raise TimeoutError("archive admission exhausted original work deadline")
    barrier = ROOT / ".transport-admission.json"
    if args.create and barrier.exists():
        caller_state = json.loads(barrier.read_bytes())
        if caller_state["state"] == "INCOMPLETE" or (
            caller_state["state"] == "ACTIVE"
            and caller_state["caller_proof"] != str(Path(args.proof_dir).resolve())
        ):
            raise ValueError("prior caller admission INCOMPLETE; CREATE refused")
    versions = Versions.model_validate_json((ROOT / "deploy/k8s/versions.json").read_bytes())
    if args.render:
        for region in ("eu", "us", "apac", "local"):
            rendered = command("kubectl", "kustomize", str(ROOT / "deploy/k8s/overlays" / region))
            assert "@sha256:" in rendered and "automountServiceAccountToken: false" in rendered
            print("RENDERED", region)
        return
    if (
        not args.create
        or not re.fullmatch(r"hypertrain-v2-[a-z0-9-]{1,40}", args.cluster)
        or not 1 <= args.deadline_seconds <= 10800
        or args.cpus != "0-3"
        or args.cpu_quota != 4
    ):
        raise ValueError(
            "proof requires experiment-owned --cluster hypertrain-v2-<id> --cpus 0-3 --cpu-quota 4"
        )
    if not args.cleanup:
        raise ValueError("isolated proof requires --cleanup")
    if not args.proof_dir or (args.image and args.local_image_id):
        raise ValueError("unique --proof-dir and exactly one image route required")
    runner_admission(archive=bool(archive))
    if os.environ.get("KIND_EXPERIMENTAL_PROVIDER", "docker") != "docker":
        raise ValueError("preflight refuses provider fallback environment")
    if args.mode == "integration" and (
        not args.integration_root
        or not args.source_manifest
        or not args.integration_scripts
        or not args.integration_scripts_sha256
    ):
        raise ValueError(
            "integration requires root-admitted private integration root and source freeze"
        )
    if (
        args.mode == "integration"
        and hashlib.sha256(Path(args.integration_scripts).read_bytes()).hexdigest()
        != args.integration_scripts_sha256
    ):
        raise ValueError("integration scripts digest mismatch")
    if args.mode == "integration" and not args.local_image_id:
        raise ValueError(
            "integration requires verified locally loaded package image; "
            "remote image route not frozen"
        )
    if versions.kind_version not in command("kind", "version"):
        raise ValueError("kind executable differs from pinned version")
    baseline = set(command("kind", "get", "clusters").splitlines())
    if args.cluster in baseline:
        raise ValueError("refusing to touch pre-existing proof cluster")
    if command("docker", "ps", "-aq", "--filter", "label=io.x-k8s.kind.cluster=" + args.cluster):
        raise ValueError("pre-existing cluster-labelled container conflicts with ownership")
    containers = set(command("docker", "ps", "-aq", "--no-trunc").splitlines())
    volumes = set(command("docker", "volume", "ls", "-q").splitlines())
    networks = set(command("docker", "network", "ls", "-q", "--no-trunc").splitlines())
    provider_baselines = {
        provider: provider_inventory(provider) for provider in ("docker", "podman")
    }
    for provider in provider_baselines:
        if command(
            provider, "ps", "-aq", "--filter", "label=io.x-k8s.kind.cluster=" + args.cluster
        ):
            raise ValueError("pre-existing experiment label in " + provider)
    # Existing published digest is not claimed to contain unmerged relay changes.
    if archive:
        _, image, _ = archive
    elif args.local_image_id:
        if not args.source_manifest or not args.source_manifest_sha256:
            raise ValueError("local image requires frozen source manifest and admitted digest")
        image = verify_local_image(
            args.local_image_id, Path(args.source_manifest), args.source_manifest_sha256
        )
    else:
        image = args.image or versions.server_image
        if not re.fullmatch(r"[^\s]+@sha256:[0-9a-f]{64}", image):
            raise ValueError("remote integration image must be digest-pinned")
    if not archive:
        command(
            "docker",
            "run",
            "--rm",
            "--cpuset-cpus=0-3",
            "--cpus=0.5",
            "--memory=256m",
            "--memory-swap=256m",
            "--tmpfs",
            "/data:rw,size=16m,uid=65532,gid=65532",
            "--entrypoint",
            "/opt/venv/bin/python",
            image,
            "-c",
            "import hypertrain.relay.app",
        )
    events: list[str] = []
    proof_dir = Path(args.proof_dir).resolve()
    proof_dir.mkdir(parents=True, exist_ok=False)
    journal = CleanupJournal(proof_dir / "cleanup.json")
    journal.write_text(
        json.dumps(
            {
                "cluster": args.cluster,
                "containers": sorted(containers),
                "volumes": sorted(volumes),
                "networks": sorted(networks),
                "experiment": args.cluster,
                "provider_baselines": provider_baselines,
                "proof_dir": str(proof_dir),
                "kubeconfig": str(proof_dir / "kubeconfig"),
                "deadline_monotonic": args.work_end
                if archive
                else time.monotonic() + args.deadline_seconds,
                "cleanup_cutoff_monotonic": args.cleanup_end
                if archive
                else time.monotonic() + args.deadline_seconds + 1200,
                "boot_id": args.clock_boot_id
                if archive
                else Path("/proc/sys/kernel/random/boot_id").read_text().strip(),
                "integration_root": (
                    str(args.integration_root) if args.mode == "integration" else None
                ),
                "state": "ADMITTED_NOT_CREATED",
            }
        )
    )
    result: dict = {}
    with tempfile.TemporaryDirectory(prefix="hypertrain-kind-proof-") as temporary:
        tmp = Path(temporary)
        kubeconfig = proof_dir / "kubeconfig"
        kubectl = [
            "kubectl",
            "--kubeconfig",
            str(kubeconfig),
            "--context",
            "kind-" + args.cluster,
        ]
        try:
            # CPU/memory constrain Docker node during creation, not merely after it.
            # kind invokes `docker run`; wrapper inserts daemon-side constraints.
            real_docker = shutil.which("docker")
            if real_docker is None:
                raise ValueError("Docker missing")
            wrapper = tmp / "docker"
            import shlex

            wrapper.write_text(
                "#!/bin/sh\nexec "
                + shlex.join(
                    [
                        sys.executable,
                        str(Path(__file__).resolve()),
                        "--docker-hook",
                        str(journal.path),
                        "--real-docker",
                        real_docker,
                        "--",
                    ]
                )
                + ' "$@"\n'
            )
            wrapper.chmod(0o700)
            environment = {**os.environ, "PATH": str(tmp) + ":" + os.environ["PATH"]}
            journal.write_text(
                json.dumps(
                    {
                        "cluster": args.cluster,
                        "state": "CREATE_INTENT",
                        "integration_root": str(args.integration_root)
                        if args.mode == "integration"
                        else None,
                        "containers": sorted(containers),
                        "volumes": sorted(volumes),
                        "networks": sorted(networks),
                        "kubeconfig": str(kubeconfig),
                    }
                )
            )
            start_supervisor(journal.path)
            if archive and time.monotonic() >= args.work_end:
                raise TimeoutError("archive preflight exhausted original work deadline")
            command(
                "kind",
                "create",
                "cluster",
                "--name",
                args.cluster,
                "--config",
                str(ROOT / "deploy/k8s/local/kind.yaml"),
                "--kubeconfig",
                str(kubeconfig),
                "--wait",
                "0",
                environment=environment,
                timeout=240,
            )
            journal.write_text(
                json.dumps(
                    {
                        "cluster": args.cluster,
                        "state": "CREATED",
                        "integration_root": str(args.integration_root)
                        if args.mode == "integration"
                        else None,
                        "kubeconfig": str(kubeconfig),
                        "containers": sorted(containers),
                        "volumes": sorted(volumes),
                        "networks": sorted(networks),
                    }
                )
            )
            node = args.cluster + "-control-plane"
            require_capped_node(journal.path, node)
            command(
                "docker",
                "exec",
                node,
                "mkdir",
                "-p",
                "/var/local/hypertrain-proof-store",
            )
            command(
                "docker",
                "exec",
                node,
                "mkdir",
                "-p",
                "/var/local/hypertrain-proof-master",
            )
            command(
                "docker",
                "exec",
                node,
                "chown",
                "65532:65532",
                "/var/local/hypertrain-proof-store",
            )
            command(
                "docker",
                "exec",
                node,
                "chown",
                "65532:65532",
                "/var/local/hypertrain-proof-master",
            )
            node_info = json.loads(command("docker", "inspect", node))[0]
            inspect = node_info["HostConfig"]
            owned_volumes = [
                mount["Name"]
                for mount in node_info["Mounts"]
                if mount["Type"] == "volume" and mount["Name"] not in volumes
            ]
            owned_networks = [
                info["NetworkID"]
                for info in node_info["NetworkSettings"]["Networks"].values()
                if info["NetworkID"] not in networks
            ]
            journal.write_text(
                json.dumps(
                    {
                        "cluster": args.cluster,
                        "state": "OWNERS_RECORDED",
                        "integration_root": str(args.integration_root)
                        if args.mode == "integration"
                        else None,
                        "containers": sorted(containers),
                        "volumes": sorted(volumes),
                        "networks": sorted(networks),
                        "owned_volumes": owned_volumes,
                        "owned_networks": owned_networks,
                    }
                )
            )
            validate_node_limits(inspect)
            added = set(command("docker", "ps", "-aq", "--no-trunc").splitlines()) - containers
            if added != {node_info["Id"]}:
                raise ValueError("unadmitted additional daemon containers during kind creation")
            if archive:
                load_transport_image(args.cluster, image, archive)
            elif args.local_image_id:
                load_transport_image(args.cluster, image)
            with httpx.Client(timeout=30, follow_redirects=False) as client:
                cni = client.get(versions.calico_manifest_url)
                cni.raise_for_status()
                assert hashlib.sha256(cni.content).hexdigest() == versions.calico_manifest_sha256
                manifest = cni.text
            for tag, pin in versions.calico_images.items():
                manifest = manifest.replace(tag.replace("docker.io/", "quay.io/"), pin)
            assert not re.search(r"image:.*calico/.*:v", manifest)
            command(*kubectl, "apply", "-f", "-", input_text=manifest)
            command(
                *kubectl,
                "rollout",
                "status",
                "deployment/calico-kube-controllers",
                "-n",
                "kube-system",
                "--timeout=180s",
            )
            command(
                *kubectl,
                "rollout",
                "status",
                "daemonset/calico-node",
                "-n",
                "kube-system",
                "--timeout=180s",
            )
            command(
                *kubectl,
                "wait",
                "--for=condition=Ready",
                "node/" + node,
                "--timeout=180s",
            )
            events.append("POLICY_CNI_READY")
            if archive:
                log = verify_archive_node(node, image, args.archive_image_id, archive[2], kubectl)
                result["archive_node_verification"] = log.strip()
                result["transport_cluster_smoke"] = "PENDING"
                events.append("ARCHIVE_NODE_SOURCE109_RELAY_IMPORT_VERIFIED")
            if args.mode == "integration":
                supervisor_state = json.loads(journal.path.read_bytes())
                supervisor_state["integration_root"] = str(args.integration_root)
                journal.write_text(json.dumps(supervisor_state))
                result = integration_workload(args, image, kubectl, node)
            else:
                # Generate ephemeral runtime secrets/test CA; never committed.
                for name in ("master", "eu", "us"):
                    (tmp / f"{name}.seed").write_text(secrets.token_hex(32))
                local_registry = RelayRegistryV1(
                    registry_version=1,
                    epoch=0,
                    previous_registry_hash=None,
                    specs=[
                        RelaySpec(
                            id=f"{region}-1",
                            region=region,
                            https_url=f"{GATEWAY}/{region}",
                            pubkeys=[
                                RelayKey(
                                    key_id="k1",
                                    pubkey=Keypair(
                                        bytes.fromhex((tmp / f"{region}.seed").read_text())
                                    ).ss58,
                                    valid_from_round=1,
                                    valid_until_round=10000,
                                )
                            ],
                            max_object_bytes=16 << 20,
                            max_inflight_bytes=64 << 20,
                            codecs=["ht-sparse-v1"],
                            mode="transport",
                        )
                        for region in ("eu", "us")
                    ],
                )
                (tmp / "registry.json").write_text(local_registry.model_dump_json())
                (tmp / "master.public").write_text(
                    Keypair(bytes.fromhex((tmp / "master.seed").read_text())).ss58
                )
                (tmp / "drain.token").write_text(secrets.token_hex(32))
                (tmp / "object.json").write_text(
                    json.dumps(
                        {
                            "access_key_id": "proof-local",
                            "secret_access_key": secrets.token_hex(32),
                        }
                    )
                )
                command(
                    "openssl",
                    "req",
                    "-x509",
                    "-newkey",
                    "rsa:2048",
                    "-nodes",
                    "-days",
                    "1",
                    "-subj",
                    "/CN=hypertrain-proof",
                    "-addext",
                    "subjectAltName=DNS:gateway.hypertrain-proof-gateway.svc",
                    "-keyout",
                    str(tmp / "tls.key"),
                    "-out",
                    str(tmp / "tls.crt"),
                )
                (tmp / "ca.crt").write_bytes((tmp / "tls.crt").read_bytes())
                # Namespaces first; install secrets before relay workloads.
                fixture_text = (
                    (ROOT / "deploy/k8s/local/fixtures.yaml")
                    .read_text()
                    .replace(versions.server_image, image)
                )
                if archive:
                    fixture_text = archive_workload(fixture_text, image)
                command(*kubectl, "apply", "-f", "-", input_text=fixture_text)
                for namespace in (
                    "hypertrain-proof",
                    "hypertrain-proof-store",
                    "hypertrain-proof-gateway",
                    "hypertrain-relay-eu",
                    "hypertrain-relay-us",
                ):
                    if namespace.startswith("hypertrain-relay-"):
                        apply(
                            kubectl,
                            {
                                "apiVersion": "v1",
                                "kind": "Namespace",
                                "metadata": {
                                    "name": namespace,
                                    "labels": {"hypertrain.network/role": "relay"},
                                },
                            },
                        )
                    # Relays/gateway/store never receive master signing seed.
                    names = ["registry.json", "ca.crt", "master.public"]
                    if namespace == "hypertrain-proof":
                        names += ["master.seed", "object.json"]
                    elif namespace == "hypertrain-proof-store":
                        names += ["object.json"]
                    elif namespace == "hypertrain-proof-gateway":
                        names += ["tls.key", "tls.crt"]
                    else:
                        names += [
                            namespace.rsplit("-", 1)[1] + ".seed",
                            "object.json",
                            "drain.token",
                        ]
                    data = {
                        name: base64.b64encode((tmp / name).read_bytes()).decode() for name in names
                    }
                    apply(
                        kubectl,
                        {
                            "apiVersion": "v1",
                            "kind": "Secret",
                            "metadata": {
                                "name": "proof-secrets",
                                "namespace": namespace,
                            },
                            "type": "Opaque",
                            "data": data,
                        },
                    )
                    apply(
                        kubectl,
                        {
                            "apiVersion": "v1",
                            "kind": "ConfigMap",
                            "metadata": {
                                "name": "proof-script",
                                "namespace": namespace,
                            },
                            "data": {"relay_kind_smoke.py": Path(__file__).read_text()},
                        },
                    )
                for region in ("eu", "us"):
                    rendered = command(
                        "kubectl", "kustomize", str(ROOT / "deploy/k8s/overlays/local")
                    )
                    rendered = rendered.replace(
                        "hypertrain-relay-local", "hypertrain-relay-" + region
                    )
                    rendered = rendered.replace("REGION: local", "REGION: " + region)
                    rendered = rendered.replace(versions.server_image, image)
                    if archive:
                        rendered = archive_workload(rendered, image)
                    # Fixture relay reads proof registry, never production authority keys.
                    rendered = rendered.replace(
                        "secretName: relay-runtime-v1", "secretName: proof-secrets"
                    )
                    apply(
                        kubectl,
                        {
                            "apiVersion": "v1",
                            "kind": "ConfigMap",
                            "metadata": {
                                "name": "relay-contract",
                                "namespace": "hypertrain-relay-" + region,
                            },
                            "data": {"relay.json": local_registry.model_dump_json()},
                        },
                    )
                    command(*kubectl, "apply", "-f", "-", input_text=rendered)
                    command(
                        *kubectl,
                        "rollout",
                        "status",
                        "deployment/hypertrain-relay",
                        "-n",
                        "hypertrain-relay-" + region,
                        "--timeout=180s",
                    )
                for namespace, deployment in (
                    ("hypertrain-proof-store", "objects"),
                    ("hypertrain-proof", "master"),
                    ("hypertrain-proof-gateway", "gateway"),
                ):
                    command(
                        *kubectl,
                        "rollout",
                        "status",
                        "deployment/" + deployment,
                        "-n",
                        namespace,
                        "--timeout=180s",
                    )
                prefix_job = json.loads(
                    command(
                        *kubectl,
                        "get",
                        "job/mock-miner",
                        "-n",
                        "hypertrain-proof",
                        "-o",
                        "json",
                    )
                )["spec"]
                prefix_job.pop("selector", None)
                prefix_job["template"]["metadata"] = {
                    "labels": {"hypertrain.network/role": "master"}
                }
                prefix_job["suspend"] = False
                prefix_job["template"]["spec"]["containers"][0]["env"] = [
                    {"name": "PROOF_STAGE", "value": "prefix"}
                ]
                apply(
                    kubectl,
                    {
                        "apiVersion": "batch/v1",
                        "kind": "Job",
                        "metadata": {
                            "name": "prefix-upload",
                            "namespace": "hypertrain-proof",
                        },
                        "spec": prefix_job,
                    },
                )
                command(
                    *kubectl,
                    "wait",
                    "--for=condition=complete",
                    "job/prefix-upload",
                    "-n",
                    "hypertrain-proof",
                    "--timeout=180s",
                )
                prefix_log = command(
                    *kubectl, "logs", "job/prefix-upload", "-n", "hypertrain-proof"
                )
                assert '"event": "PREFIX_DURABLE"' in prefix_log
                command(
                    *kubectl,
                    "delete",
                    "pods",
                    "-n",
                    "hypertrain-relay-eu",
                    "-l",
                    "app=hypertrain-relay",
                    "--grace-period=0",
                    "--force",
                    "--wait=true",
                    "--timeout=30s",
                )
                command(
                    *kubectl,
                    "rollout",
                    "status",
                    "deployment/hypertrain-relay",
                    "-n",
                    "hypertrain-relay-eu",
                    "--timeout=180s",
                )
                events.append("DURABLE_PREFIX_RESTARTED_BEFORE_FULL_RESUME")
                command(
                    *kubectl,
                    "patch",
                    "job/mock-miner",
                    "-n",
                    "hypertrain-proof",
                    "--type=merge",
                    "-p",
                    '{"spec":{"suspend":false}}',
                )
                command(
                    *kubectl,
                    "wait",
                    "--for=condition=complete",
                    "job/mock-miner",
                    "-n",
                    "hypertrain-proof",
                    "--timeout=180s",
                )
                log = command(*kubectl, "logs", "job/mock-miner", "-n", "hypertrain-proof")
                assert '"event": "FOUR_ORIGINAL_OBJECTS_VERIFIED"' in log
                events.append(log.strip())
                # Event-backed actual pod kill and replay through the surviving shared-CAS replica.
                pods = json.loads(
                    command(
                        *kubectl,
                        "get",
                        "pods",
                        "-n",
                        "hypertrain-relay-eu",
                        "-l",
                        "app=hypertrain-relay",
                        "-o",
                        "json",
                    )
                )["items"]
                if len(pods) < 2:
                    raise ValueError("replica-switch proof requires two ready relay pods")
                killed = pods[0]["metadata"]["name"]
                command(
                    *kubectl,
                    "delete",
                    "pod",
                    killed,
                    "-n",
                    "hypertrain-relay-eu",
                    "--grace-period=0",
                    "--force",
                    "--wait=true",
                    "--timeout=30s",
                )
                command(
                    *kubectl,
                    "rollout",
                    "status",
                    "deployment/hypertrain-relay",
                    "-n",
                    "hypertrain-relay-eu",
                    "--timeout=180s",
                )
                events.append("RELAY_POD_KILLED_REPLACEMENT_READY")
                job = json.loads(
                    command(
                        *kubectl,
                        "get",
                        "job/mock-miner",
                        "-n",
                        "hypertrain-proof",
                        "-o",
                        "json",
                    )
                )
                job_spec = job["spec"]
                job_spec.pop("selector", None)
                job_spec["template"]["metadata"] = {"labels": {"hypertrain.network/role": "master"}}
                job_spec["suspend"] = False
                job_spec["template"]["spec"]["containers"][0]["env"] = [
                    {"name": "PROOF_STAGE", "value": "resume"}
                ]
                apply(
                    kubectl,
                    {
                        "apiVersion": "batch/v1",
                        "kind": "Job",
                        "metadata": {
                            "name": "replica-resume",
                            "namespace": "hypertrain-proof",
                        },
                        "spec": job_spec,
                    },
                )
                command(
                    *kubectl,
                    "wait",
                    "--for=condition=complete",
                    "job/replica-resume",
                    "-n",
                    "hypertrain-proof",
                    "--timeout=180s",
                )
                resumed = command(*kubectl, "logs", "job/replica-resume", "-n", "hypertrain-proof")
                first_receipts = json.loads(log.strip().splitlines()[-1])["receipts"]
                assert json.loads(resumed.strip().splitlines()[-1])["receipts"] == first_receipts
                events.append("REPLICA_RESUME_ORIGINAL_RECEIPTS_VERIFIED")
                command(
                    *kubectl,
                    "scale",
                    "deployment/hypertrain-relay",
                    "-n",
                    "hypertrain-relay-eu",
                    "--replicas=0",
                )
                command(
                    *kubectl,
                    "wait",
                    "--for=delete",
                    "pods",
                    "-n",
                    "hypertrain-relay-eu",
                    "-l",
                    "app=hypertrain-relay",
                    "--timeout=60s",
                )
                job_spec["template"]["spec"]["containers"][0]["env"] = [
                    {"name": "PROOF_STAGE", "value": "fallback"}
                ]
                apply(
                    kubectl,
                    {
                        "apiVersion": "batch/v1",
                        "kind": "Job",
                        "metadata": {
                            "name": "regional-fallback",
                            "namespace": "hypertrain-proof",
                        },
                        "spec": job_spec,
                    },
                )
                command(
                    *kubectl,
                    "wait",
                    "--for=condition=complete",
                    "job/regional-fallback",
                    "-n",
                    "hypertrain-proof",
                    "--timeout=180s",
                )
                fallback_log = command(
                    *kubectl, "logs", "job/regional-fallback", "-n", "hypertrain-proof"
                )
                assert len(json.loads(fallback_log.strip().splitlines()[-1])["receipts"]) == 1
                events.append("PRIMARY_REGION_DOWN_AUTHORIZED_FALLBACK_BYTES_VERIFIED")
                command(
                    *kubectl,
                    "scale",
                    "deployment/hypertrain-relay",
                    "-n",
                    "hypertrain-relay-eu",
                    "--replicas=2",
                )
                command(
                    *kubectl,
                    "rollout",
                    "status",
                    "deployment/hypertrain-relay",
                    "-n",
                    "hypertrain-relay-eu",
                    "--timeout=180s",
                )
                gc_data = {
                    name: base64.b64encode((tmp / name).read_bytes()).decode()
                    for name in (
                        "registry.json",
                        "ca.crt",
                        "master.public",
                        "master.seed",
                        "eu.seed",
                        "object.json",
                    )
                }
                apply(
                    kubectl,
                    {
                        "apiVersion": "v1",
                        "kind": "Secret",
                        "metadata": {
                            "name": "proof-gc-only",
                            "namespace": "hypertrain-proof",
                        },
                        "data": gc_data,
                    },
                )
                job_spec["template"]["spec"]["containers"][0]["command"] = [
                    "/opt/venv/bin/python",
                    "/fixture/relay_kind_smoke.py",
                    "--fixture",
                    "gc",
                ]
                for volume in job_spec["template"]["spec"]["volumes"]:
                    if volume["name"] == "secrets":
                        volume["secret"]["secretName"] = "proof-gc-only"
                apply(
                    kubectl,
                    {
                        "apiVersion": "batch/v1",
                        "kind": "Job",
                        "metadata": {
                            "name": "transport-gc",
                            "namespace": "hypertrain-proof",
                        },
                        "spec": job_spec,
                    },
                )
                command(
                    *kubectl,
                    "wait",
                    "--for=condition=complete",
                    "job/transport-gc",
                    "-n",
                    "hypertrain-proof",
                    "--timeout=180s",
                )
                gc_log = command(*kubectl, "logs", "job/transport-gc", "-n", "hypertrain-proof")
                assert '"event": "TRANSPORT_RETENTION_GC_PREFIX_ISOLATION"' in gc_log
                events.append(gc_log.strip())
                # Actual deny control, not presence of YAML.
                code = (
                    "import socket; s=socket.socket(); s.settimeout(3); "
                    "s.connect(('hypertrain-relay.hypertrain-relay-eu.svc',8000))"
                )
                denied = command(
                    *kubectl,
                    "run",
                    "unauthorized",
                    "-n",
                    "hypertrain-proof-denied",
                    "--image=" + image,
                    "--restart=Never",
                    "--overrides="
                    + json.dumps(
                        {
                            "spec": {
                                "automountServiceAccountToken": False,
                                "securityContext": {
                                    "runAsNonRoot": True,
                                    "runAsUser": 65532,
                                    "seccompProfile": {"type": "RuntimeDefault"},
                                },
                                "containers": [
                                    {
                                        "name": "unauthorized",
                                        "image": image,
                                        **({"imagePullPolicy": "Never"} if archive else {}),
                                        "command": ["/opt/venv/bin/python", "-c", code],
                                        "securityContext": {
                                            "allowPrivilegeEscalation": False,
                                            "capabilities": {"drop": ["ALL"]},
                                        },
                                        "resources": {"limits": {"cpu": "100m", "memory": "64Mi"}},
                                    }
                                ],
                            }
                        }
                    ),
                    "-o",
                    "json",
                )
                pod = json.loads(denied)["metadata"]["name"]
                command(
                    *kubectl,
                    "wait",
                    "--for=jsonpath={.status.phase}=Failed",
                    "pod/" + pod,
                    "-n",
                    "hypertrain-proof-denied",
                    "--timeout=30s",
                )
                denied_logs = command(*kubectl, "logs", pod, "-n", "hypertrain-proof-denied")
                assert "TimeoutError" in denied_logs
                events.append("UNAUTHORIZED_NAMESPACE_DENIED")
                result = {
                    **result,
                    "events": events,
                    "cpu_quota": 3.5,
                    "memory_bytes": inspect["Memory"],
                    "transport_cluster_smoke": "PASS",
                    "full_D3_integration": "NOT_RUN",
                }
        finally:
            completed = result.get("transport_cluster_smoke") == "PASS"
            if result:
                if archive:
                    result["transport_cluster_smoke"] = "PENDING"
                (proof_dir / "proof-result.json").write_text(json.dumps(result))
                state = json.loads(journal.path.read_bytes())
                journal.write_text(
                    json.dumps({**state, "proof_status": "INTERRUPTED" if archive else "PASS"})
                )
            cleanup_owned(journal.path)
            cleanup_state = json.loads(journal.path.read_bytes())
            if cleanup_state["proof_status"] == "CENSORED":
                raise ValueError("rescue incomplete; proof CENSORED, cleanup verified")
            if archive and completed:
                result["transport_cluster_smoke"] = "PASS"
                (proof_dir / "proof-result.json").write_text(json.dumps(result))
                journal.write_text(json.dumps({**cleanup_state, "proof_status": "PASS"}))
    print(json.dumps(result))  # PASS only after exact cleanup verification.


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--fixture", choices=("store", "master", "gateway", "relay", "miner", "gc"))
    parser.add_argument("--render", action="store_true")
    parser.add_argument("--create", action="store_true")
    parser.add_argument("--cluster", default="hypertrain-v2-" + secrets.token_hex(8))
    parser.add_argument("--deadline-seconds", type=int, default=10800)
    parser.add_argument(
        "--launch-unit", help="Exact outer archive service identity; secondary runtime ceiling"
    )
    parser.add_argument("--work-end", type=float, help="Admitted absolute monotonic work endpoint")
    parser.add_argument(
        "--cleanup-end", type=float, help="Admitted absolute monotonic cleanup endpoint"
    )
    parser.add_argument("--clock-boot-id", help="Boot identity of admitted monotonic endpoints")
    parser.add_argument("--caller-cap", type=float, help="Finite absolute caller resource cutoff")
    parser.add_argument(
        "--caller-unit", help="Own flock through independent cleanup before release"
    )
    parser.add_argument("--caller-lock", type=Path, help="Existing owned shared flock file")
    parser.add_argument("--cpus", default="0-3")
    parser.add_argument("--cpu-quota", type=int, default=4)
    parser.add_argument("--cleanup", action="store_true")
    parser.add_argument("--image", help="Integrated GHCR server digest containing relay code")
    parser.add_argument("--local-image-id", help="Actual locally built Docker sha256 image ID")
    parser.add_argument(
        "--image-archive", type=Path, help="Owned immutable native archive, transport only"
    )
    parser.add_argument("--archive-sha256", help="Root-admitted complete archive SHA256")
    parser.add_argument("--archive-image-id", help="Root-admitted image config sha256 ID")
    parser.add_argument("--archive-reference", help="Exact stored archive tag; Never pull")
    parser.add_argument("--source-manifest")
    parser.add_argument("--source-manifest-sha256")
    parser.add_argument("--proof-dir", required=False)
    parser.add_argument("--mode", choices=("transport", "integration"), default="transport")
    parser.add_argument("--prepare-integration", action="store_true")
    parser.add_argument("--snapshot", type=Path)
    parser.add_argument("--package-sha256")
    parser.add_argument(
        "--integration-role",
        choices=("service", "relay", "driver", "gateway", "wrong-ca"),
    )
    parser.add_argument("--integration-config", type=Path)
    parser.add_argument("--render-integration", action="store_true")
    parser.add_argument("--integration-root", type=Path)
    parser.add_argument("--integration-scripts", type=Path)
    parser.add_argument("--integration-scripts-sha256")
    parser.add_argument("--supervise", type=Path)
    parser.add_argument("--parent-pid", type=int)
    parser.add_argument("--ready-socket", type=Path)
    parser.add_argument("--docker-hook", type=Path)
    parser.add_argument("--real-docker")
    parser.add_argument("docker_args", nargs=argparse.REMAINDER)
    parser.add_argument("--prepare-root", type=Path)
    parser.add_argument("--original-fixture-secrets", action="store_true")
    args = parser.parse_args()
    if args.caller_unit:
        if args.caller_lock is None:
            parser.error("caller requires existing shared --caller-lock")
        index = sys.argv.index("--caller-unit")
        runner_argv = sys.argv[1:index] + sys.argv[index + 2 :]
        caller_launch(args, runner_argv)
    elif args.docker_hook:
        if not args.real_docker:
            parser.error("Docker hook requires absolute real Docker executable")
        docker_create_hook(
            args.docker_hook,
            args.real_docker,
            args.docker_args[1:] if args.docker_args[:1] == ["--"] else args.docker_args,
        )
    elif args.supervise:
        if args.parent_pid is None:
            parser.error("supervisor requires parent pid")
        deadline_supervisor(args.supervise, args.parent_pid, args.ready_socket)
    elif args.prepare_root:
        if args.snapshot is None or args.package_sha256 is None or args.create:
            parser.error("prepare root requires admitted adapter bundle/hash; no create")
        prepare_integration_root(args.prepare_root, args.snapshot, args.package_sha256)
        if args.original_fixture_secrets:
            provision_original_fixture_secrets(
                args.prepare_root, args.snapshot, args.package_sha256
            )
    elif args.original_fixture_secrets:
        if args.integration_root is None or args.snapshot is None or args.package_sha256 is None:
            parser.error("original private roles require prepared root and admitted bundle/hash")
        provision_original_fixture_secrets(
            args.integration_root, args.snapshot, args.package_sha256
        )
    elif args.render_integration:
        if args.mode != "integration" or args.create or not args.image:
            parser.error("integration render requires integration mode and exact image; no create")
        print(json.dumps(integration_manifests(args.image)))
    elif args.integration_role:
        if (
            args.mode != "integration"
            or args.prepare_integration
            or args.create
            or args.render
            or args.fixture
            or args.integration_config is None
        ):
            parser.error("integration role requires integration mode and config; no fixture/launch")
        integration_role(args.integration_role, args.integration_config)
    elif args.prepare_integration:
        if (
            args.mode != "integration"
            or args.create
            or args.render
            or args.fixture
            or not args.snapshot
            or not args.package_sha256
        ):
            parser.error(
                "preparation requires integration mode, snapshot, "
                "root-admitted package hash; no launch"
            )
        prepared = inspect_integration_snapshot(args.snapshot, args.package_sha256)
        print(prepared.model_dump_json(exclude={"files"}))
    elif args.fixture:
        fixture(args.fixture)
    else:
        smoke(args)


if __name__ == "__main__":
    main()
