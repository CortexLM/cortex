"""Helpers private to the http_auth package of the ledger-invariants lot. Not collected.

The route tables below pin which principal every route of the three HTTP applications
(challenge service, relay, public mount) answers to. They are derived from the handlers in
`challenge/app.py` and `relay/app.py`, and `test_http_auth.py` cross-checks them against the
source of each handler, so a new route cannot appear without a decision recorded here.
"""

from __future__ import annotations

import hashlib
import inspect
import re
import sqlite3
import sys
import threading
from collections.abc import Callable, Mapping
from importlib import import_module
from pathlib import Path
from types import ModuleType
from typing import Any, NamedTuple

import httpx
from fastapi.testclient import TestClient

from env_pin_fixtures import honest_env
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages_v2 import REQUEST_HORIZON
from hypertrain.protocol.server_keys import (
    ServerKeyGrant,
    ServerKeyRevoke,
    SignedGrant,
    SignedRevoke,
    signing_message,
)

from .support import SLUG, TESTS, World, load_fixture_module, table_names

HASH = "ab" * 32
UNKNOWN_RUN = "cd" * 32
GATE_TEXT = "the hotkey is not registered on the subnet"
REVOKED_TEXT = "the server key grant is revoked"


class Auth(NamedTuple):
    """What a route demands of its caller: `principal` is the credential, `gate` the extra
    membership check the route itself performs on a hotkey ("" when it performs none)."""

    principal: str
    gate: str = ""


OP, WK, INT, NONE = "OP", "WK", "INT", "NONE"
ENVELOPE, HEADER, BODY_KEY, BODY_SIG, DUAL, KEY_GRANT = (
    "ENVELOPE",
    "HEADER",
    "BODY_KEY",
    "BODY_SIG",
    "DUAL",
    "KEY_GRANT",
)
SUBNET, PARTY, OWNER = "subnet", "party", "owner-subnet"

Key = tuple[str, str]
V2 = "/v2/runs/{run_id}"
ADMIN_V2 = V2 + "/admin"
# No signature rides in an HTTP header (the master proxy forwards none): a party polls its dispute
# events with a signed `DisputeWatchRequest` in a POST body, and the old header-signed `/disputes`
# answers 405 "retired" to every method (ADR 0014 part 8).
WATCH_KEY: Key = ("POST", V2 + "/disputes/watch")
RETIRED_WATCH = V2 + "/disputes"
RETIRED_METHODS = ("GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS")

CHALLENGE_ROUTES: dict[Key, Auth] = {
    # Operator bearer: every v1 admin and aggregator route, every v2 /admin route.
    ("POST", "/v1/admin/beacon"): Auth(OP),
    ("POST", "/v1/admin/runs"): Auth(OP),
    ("PUT", "/v1/admin/runs/{run_id}/config"): Auth(OP),
    ("PUT", "/v1/admin/runs/{run_id}/paused"): Auth(OP),
    ("PUT", "/v1/admin/runs/{run_id}/roster/{hotkey}"): Auth(OP),
    ("DELETE", "/v1/admin/runs/{run_id}/roster/{hotkey}"): Auth(OP),
    ("PUT", "/v1/admin/runs/{run_id}/honeypot"): Auth(OP),
    ("POST", "/v1/admin/runs/{run_id}/honeypot/reveal"): Auth(OP),
    ("GET", "/v1/aggregator/runs/{run_id}/rounds/{w}/inputs"): Auth(OP),
    ("POST", "/v1/aggregator/runs/{run_id}/rounds/{w}/aggregate"): Auth(OP),
    ("POST", "/v1/aggregator/runs/{run_id}/rounds/{w}/finalize"): Auth(OP),
    ("POST", "/v1/aggregator/runs/{run_id}/rounds/{w}/rollback"): Auth(OP),
    ("PUT", "/v1/aggregator/runs/{run_id}/rounds/{w}/state"): Auth(OP),
    ("POST", "/v2/admin/runs"): Auth(OP),
    ("PUT", ADMIN_V2 + "/dataset"): Auth(OP),
    ("POST", ADMIN_V2 + "/genesis"): Auth(OP),
    ("POST", ADMIN_V2 + "/bootstrap"): Auth(OP),
    ("POST", ADMIN_V2 + "/economic-bound"): Auth(OP),
    ("POST", ADMIN_V2 + "/rounds"): Auth(OP),
    ("POST", ADMIN_V2 + "/rounds/{w}/audits"): Auth(OP),
    ("POST", ADMIN_V2 + "/rounds/{w}/aggregate"): Auth(OP),
    ("POST", ADMIN_V2 + "/rounds/{w}/finalize"): Auth(OP),
    ("POST", ADMIN_V2 + "/join/{admission_id}/reference"): Auth(OP),
    ("POST", ADMIN_V2 + "/join/{admission_id}/reseed"): Auth(OP),
    ("POST", ADMIN_V2 + "/join/{admission_id}/finalize"): Auth(OP),
    ("POST", ADMIN_V2 + "/join/{admission_id}/shadow-reservation"): Auth(OP),
    ("POST", ADMIN_V2 + "/join/{admission_id}/shadow-reward"): Auth(OP),
    ("POST", ADMIN_V2 + "/trial/{admission_id}/export"): Auth(OP),
    ("POST", ADMIN_V2 + "/disputes/{dispute_id}/referee"): Auth(OP),
    ("POST", ADMIN_V2 + "/disputes/{dispute_id}/state/{checkpoint}"): Auth(OP),
    ("POST", ADMIN_V2 + "/disputes/{dispute_id}/timeout"): Auth(OP),
    ("POST", ADMIN_V2 + "/rollback"): Auth(OP),
    ("POST", ADMIN_V2 + "/rollback-preview"): Auth(OP),
    ("POST", ADMIN_V2 + "/relay-registry"): Auth(OP),
    ("POST", ADMIN_V2 + "/relay-failure"): Auth(OP),
    ("POST", ADMIN_V2 + "/relay-fallback/{failure_hash}"): Auth(OP),
    ("POST", ADMIN_V2 + "/relay-drain/{relay_id}"): Auth(OP),
    ("POST", ADMIN_V2 + "/relay/{grant_hash}/{action}"): Auth(OP),
    ("GET", ADMIN_V2 + "/relay-settlement/{grant_hash}"): Auth(OP),
    # Worker bearer: the v1 auditor queue and the v1 referee resolution.
    ("POST", "/v1/worker/lease"): Auth(WK),
    ("GET", "/v1/worker/jobs/{job_id}/serves"): Auth(WK),
    ("POST", "/v1/worker/jobs/{job_id}/heartbeat"): Auth(WK),
    ("POST", "/v1/worker/jobs/{job_id}/complete"): Auth(WK),
    ("POST", "/v1/worker/jobs/{job_id}/fail"): Auth(WK),
    ("GET", "/v1/objects/{sha256}"): Auth(WK),
    ("POST", "/v1/runs/{run_id}/resolution"): Auth(WK),
    # Internal bearer plus the platform slug header.
    ("GET", "/internal/v1/get_weights"): Auth(INT),
    # No credential at all: probes, public reads and the content-addressed objects.
    ("GET", "/health"): Auth(NONE),
    ("GET", "/version"): Auth(NONE),
    ("GET", "/v1/runs"): Auth(NONE),
    ("GET", "/v1/beacon/latest"): Auth(NONE),
    ("GET", "/v1/runs/{run_id}"): Auth(NONE),
    ("GET", "/v1/runs/{run_id}/rounds/{w}"): Auth(NONE),
    ("GET", "/v1/runs/{run_id}/auditor-stats"): Auth(NONE),
    ("GET", "/run/{run_id}/auditor-stats"): Auth(NONE),
    ("GET", "/v2/server-keys"): Auth(NONE),
    ("GET", V2): Auth(NONE),
    ("GET", V2 + "/admission/{hotkey}"): Auth(NONE),
    ("GET", V2 + "/objects/{sha256}"): Auth(NONE),
    ("GET", V2 + "/rounds/{w}"): Auth(NONE),
    ("GET", V2 + "/rounds/{w}/job/{hotkey}"): Auth(NONE),
    ("GET", V2 + "/rounds/{w}/relay/{hotkey}"): Auth(NONE),
    ("GET", V2 + "/relay-observers/{grant_hash}/{observer}"): Auth(NONE),
    # The master's signed view of a settlement: a relay checks a release against it, so it is
    # public and the coordinator's seal is what authenticates it.
    ("GET", V2 + "/relay-settlements/{grant_hash}"): Auth(NONE),
    ("GET", "/public/v1/runs"): Auth(NONE),
    ("GET", "/public/v1/snapshot"): Auth(NONE),
    ("GET", "/public/v1/runs/{run_id}"): Auth(NONE),
    ("GET", "/public/v1/runs/{run_id}/model"): Auth(NONE),
    # The retired header-signed watch: it reads no credential and does no work, whatever the method.
    **{(method, RETIRED_WATCH): Auth(NONE) for method in RETIRED_METHODS},
    # A request signed in the body: the party's poll, and the frame of an object upload (the length
    # of the signed ObjectUploadRequest, that envelope, then the object).
    WATCH_KEY: Auth(ENVELOPE, PARTY),
    ("POST", V2 + "/objects/{sha256}"): Auth(ENVELOPE, SUBNET),
    # A signed v2 envelope whose signer the route checks against the subnet.
    ("POST", V2 + "/join"): Auth(DUAL, SUBNET),
    # The owner's own request (ADR 0010 decision 3): signed by the admission's hotkey, in the body.
    ("POST", V2 + "/join/{admission_id}/challenge"): Auth(ENVELOPE, OWNER),
    ("POST", V2 + "/join/{admission_id}/job"): Auth(ENVELOPE, OWNER),
    ("POST", V2 + "/join/proof"): Auth(ENVELOPE, SUBNET),
    ("POST", V2 + "/admission/{hotkey}/rotate"): Auth(ENVELOPE, SUBNET),
    ("POST", V2 + "/admission/recover"): Auth(ENVELOPE, SUBNET),
    ("POST", V2 + "/accept"): Auth(ENVELOPE, SUBNET),
    ("POST", V2 + "/commit"): Auth(ENVELOPE, SUBNET),
    ("POST", V2 + "/delta"): Auth(ENVELOPE, SUBNET),
    ("POST", V2 + "/leaves"): Auth(ENVELOPE, SUBNET),
    ("POST", V2 + "/upload-grant"): Auth(ENVELOPE, SUBNET),
    ("POST", V2 + "/dispute"): Auth(ENVELOPE, PARTY),
    ("POST", V2 + "/bisect"): Auth(ENVELOPE, PARTY),
    ("POST", V2 + "/state-serve"): Auth(ENVELOPE, PARTY),
    ("POST", V2 + "/disputes/ack"): Auth(ENVELOPE, PARTY),
    # A signed v2 envelope whose signer only the store checks (coldkey, pinned auditor,
    # referee, observer, relay): the route itself has no membership gate.
    ("POST", V2 + "/escrow/lock"): Auth(ENVELOPE),
    ("POST", V2 + "/escrow/transfer"): Auth(ENVELOPE),
    ("POST", V2 + "/escrow/release"): Auth(ENVELOPE),
    ("POST", V2 + "/resolution"): Auth(ENVELOPE),
    ("POST", V2 + "/worker/lease"): Auth(ENVELOPE),
    ("POST", V2 + "/worker/jobs/{job_id}/execute"): Auth(ENVELOPE),
    ("POST", V2 + "/worker/jobs/{job_id}/complete"): Auth(ENVELOPE),
    ("POST", V2 + "/relay-observers/{grant_hash}/{observer}"): Auth(ENVELOPE),
    ("POST", "/v2/relay-receipts"): Auth(ENVELOPE),
    ("POST", "/v2/server-keys/revoke"): Auth(KEY_GRANT),
    ("POST", "/v2/server-keys"): Auth(KEY_GRANT),
    # The v1 intake: a signed v1 envelope (or a hotkey named in the body) checked on the subnet.
    ("POST", "/v1/runs/{run_id}/accept"): Auth(ENVELOPE, SUBNET),
    ("POST", "/v1/runs/{run_id}/commit"): Auth(ENVELOPE, SUBNET),
    ("POST", "/v1/runs/{run_id}/delta"): Auth(ENVELOPE, SUBNET),
    ("POST", "/v1/runs/{run_id}/state"): Auth(ENVELOPE, SUBNET),
    ("POST", "/v1/runs/{run_id}/dispute"): Auth(ENVELOPE, SUBNET),
    ("POST", "/v1/runs/{run_id}/bisect"): Auth(ENVELOPE),
    ("POST", "/v1/runs/{run_id}/leaves"): Auth(BODY_KEY, SUBNET),
    ("POST", "/v1/runs/{run_id}/uploads"): Auth(BODY_KEY, SUBNET),
    ("POST", "/v1/runs/{run_id}/rerun"): Auth(BODY_SIG, SUBNET),
}
MOUNTS = {("MOUNT", "/public")}

RELAY_ROUTES: dict[Key, Auth] = {
    ("GET", "/livez"): Auth(NONE),
    ("GET", "/readyz"): Auth(NONE),
    ("GET", "/probe"): Auth(NONE),
    ("POST", "/v1/uploads/{grant_hash}/accept"): Auth(ENVELOPE),
    ("PUT", "/v1/uploads/{grant_hash}/chunks/{index}"): Auth(HEADER),
    ("POST", "/v1/uploads/{grant_hash}/complete"): Auth(HEADER),
    ("GET", "/v1/uploads/{grant_hash}/acks"): Auth(HEADER),
    ("POST", "/v1/uploads/{grant_hash}/retention"): Auth(ENVELOPE),
    ("POST", "/v1/uploads/{grant_hash}/release"): Auth(ENVELOPE),
    ("POST", "/v1/retrievals"): Auth(ENVELOPE),
    ("GET", "/v1/objects/{sha}"): Auth(HEADER),
    ("POST", "/internal/drain"): Auth("DRAIN"),
}

GATED_V2 = {
    k: a
    for k, a in CHALLENGE_ROUTES.items()
    if k[1].startswith("/v2/") and a.gate in (SUBNET, PARTY)
}
GATED_V1 = {
    k: a
    for k, a in CHALLENGE_ROUTES.items()
    if k[1].startswith("/v1/") and a.gate in (SUBNET, PARTY)
}

# Template values for the path parameters of a probe: ids that exist nowhere, so a request that
# does get past its gate meets "unknown" and not live state.
PARAMS = {
    "run_id": UNKNOWN_RUN,
    "w": "0",
    "checkpoint": "0",
    "index": "0",
    "action": "release",
    "hotkey": Keypair(b"\x31" * 32).ss58,
}
QUERY = {
    "/v1/worker/jobs/{job_id}/serves": {"lease": "x"},
    "/internal/v1/get_weights": {"epoch": "0"},
}


def concrete(template: str) -> str:
    return re.sub(r"\{(\w+)\}", lambda m: PARAMS.get(m.group(1), HASH), template)


def quiet_client(app: Any) -> TestClient:
    """A client that turns an unhandled server error into a 500 answer instead of raising."""
    return TestClient(app, raise_server_exceptions=False)


def call(client: TestClient, key: Key, headers: Mapping[str, str]) -> httpx.Response:
    method, template = key
    return client.request(
        method,
        concrete(template),
        headers={"content-type": "application/json", **headers},
        params=QUERY.get(template, {}),
        content=b"{}" if method in ("POST", "PUT") else None,
    )


def with_slug(headers: Mapping[str, str]) -> dict[str, str]:
    return {**headers, "x-platform-challenge-slug": SLUG}


def bearer(token: str) -> dict[str, str]:
    return {"Authorization": f"Bearer {token}"}


def sibling_gate() -> ModuleType:
    """tests/challenge/test_subnet_gate_v2.py: its `_routes` is the call table of the 15
    miner-signed v2 routes. `challenge` is the package pytest roots under tests/."""
    return import_module("challenge.test_subnet_gate_v2")


def sibling_service() -> ModuleType:
    return sibling_gate().service  # type: ignore[no-any-return]


def sibling_runtime() -> ModuleType:
    """tests/relay/test_beacon_cache.py: its `serving` runs the real `runtime()` lifespan of the
    relay on a local backing. That module imports the relay fixture by its bare name, so the
    directory is on the import path only while it loads."""
    directory = str(TESTS / "relay")
    sys.path.insert(0, directory)
    try:
        return load_fixture_module("li_relay_runtime", "relay/test_beacon_cache.py")
    finally:
        sys.path.remove(directory)


def endpoints(app: Any) -> dict[Key, Any]:
    """(method, path) -> handler function, mounts excluded."""
    found: dict[Key, Any] = {}
    for route in app.routes:
        endpoint = getattr(route, "endpoint", None)
        if endpoint is None:
            continue
        for method in route.methods or ():
            found[(method, route.path)] = endpoint
    return found


def source_of(handler: Any) -> str:
    return inspect.getsource(handler)


def file_rows(path: Path) -> str:
    """Digest of every row of every table of one sqlite file (a store the World does not hold)."""
    db = sqlite3.connect(f"file:{path}?mode=ro", uri=True)
    try:
        hasher = hashlib.sha256()
        for name in table_names(db):
            rows = sorted(repr(tuple(row)) for row in db.execute(f'SELECT * FROM "{name}"'))
            hasher.update(repr((name, rows)).encode())
        return hasher.hexdigest()
    finally:
        db.close()


def durable_state(world: World) -> dict[str, str]:
    """Per-table digests of the store and its run sub-stores, the server-key registry and the
    names of the files below the state directory: what a request can leave behind."""
    state: dict[str, str] = {}
    for position, store in enumerate(world.store.stores()):
        with store._lock:
            for name in table_names(store._db):
                rows = sorted(repr(tuple(r)) for r in store._db.execute(f'SELECT * FROM "{name}"'))
                state[f"{position}:{name}"] = hashlib.sha256(repr(rows).encode()).hexdigest()
    state["server_keys"] = file_rows(world.store.state_dir / "server_keys.db")
    names = sorted(
        str(p.relative_to(world.store.state_dir))
        for p in world.store.state_dir.rglob("*")
        if p.is_file() and not p.name.endswith(("-wal", "-shm", "-journal"))
    )
    state["files"] = hashlib.sha256(repr(names).encode()).hexdigest()
    return state


def log_record_writes(db: sqlite3.Connection) -> Callable[[], list[tuple[str, str]]]:
    """From now on, log the (kind, id) of every row written to `records_v2` (insert, replace or
    update, whatever the statement) in a TEMP table of this connection, which never reaches the
    database file. Returns the reader of the log."""
    db.execute("CREATE TEMP TABLE record_writes(kind TEXT, id TEXT)")
    for event in ("INSERT", "UPDATE"):
        db.execute(
            f"CREATE TEMP TRIGGER record_{event.lower()} AFTER {event} ON main.records_v2 "
            "BEGIN INSERT INTO record_writes VALUES (new.kind, new.id); END"
        )
    return lambda: [(row[0], row[1]) for row in db.execute("SELECT kind, id FROM record_writes")]


def changed(before: Mapping[str, str], after: Mapping[str, str]) -> set[str]:
    """Names (without the store position) of what differs between two durable states."""
    keys = set(before) | set(after)
    return {k.split(":", 1)[-1] for k in keys if before.get(k) != after.get(k)}


def server_key_grant(
    server: Keypair, cold: Keypair, *, key_id: str = "ab" * 16, name: str = "vast-1"
) -> SignedGrant:
    grant = ServerKeyGrant(
        run_id="*",
        coldkey=cold.ss58,
        server_pub=server.ss58,
        server_name=name,
        scope="mine",
        key_id=key_id,
        issued_at=1,
    )
    message = signing_message("grant", grant)
    return SignedGrant(
        grant=grant, cold_sig=cold.sign(message).hex(), server_sig=server.sign(message).hex()
    )


def server_key_revoke(
    cold: Keypair, key_id: str = "ab" * 16, *, revoked_at: int = 2
) -> SignedRevoke:
    revoke = ServerKeyRevoke(key_id=key_id, coldkey=cold.ss58, revoked_at=revoked_at)
    return SignedRevoke(revoke=revoke, cold_sig=cold.sign(signing_message("revoke", revoke)).hex())


class RelayScenario(NamedTuple):
    """Stored state a relay-observer or relay-receipt request needs, written test-side: an
    accepted upload grant of `miner`, an open round, and an accepted reference with its bytes."""

    run_id: str
    grant: Any
    grant_hash: str
    observer: Keypair
    miner: Keypair


def seed_relay_scenario(world: World) -> RelayScenario:
    from hypertrain.protocol.messages_v2 import ArtifactRef, WorkProof
    from hypertrain.protocol.relay_messages import UploadGrant

    service = sibling_service()
    miner, observer = service.HOT[0], service.AUDITORS[0]
    run_id = world.run_id
    store = world.store.for_run(run_id)
    grant = UploadGrant(
        w=0,
        hotkey=miner.ss58,
        relay_id="local",
        assignment_epoch=0,
        delta_hash=HASH,
        size=3,
        chunk_manifest_hash=HASH,
        retain_until=500,
        nonce=HASH,
        exp_drand=400,
    )
    artifact = store.objects.put(b"accepted reference bytes")
    proof = WorkProof(
        admission_id=HASH,
        challenge_hash=HASH,
        leaves_root=HASH,
        delta_hash=HASH,
        artifact_refs=[ArtifactRef(sha256=artifact, size=len(b"accepted reference bytes"))],
    )
    with store._tx():
        store._put_record_v2(run_id, "grant", grant.digest(), {"body": grant.body()})
        store._put_record_v2(run_id, "round", "0", {"body": {"d_upload": 450}})
        store._db.execute(
            "INSERT INTO admission_trial_results(epoch, challenge, reference, outcome) "
            "VALUES(1, '{}', ?, 'MATCH')",
            (proof.model_dump_json(),),
        )
    return RelayScenario(run_id, grant, grant.digest(), observer, miner)


LEASE_NONCE = "cd" * 32


def seed_audit_lease(world: World, auditor: Keypair, job_id: str = HASH) -> None:
    """A running audit lease of `auditor` on `job_id`, written test-side: what a lease request
    leaves behind. Its nonce is `LEASE_NONCE`, which the asks of the tests do not carry."""
    store = world.store.for_run(world.run_id)
    with store._tx():
        store._db.execute(
            "INSERT INTO audit_leases_v2(id, run_id, w, hotkey, state, auditor, nonce) "
            "VALUES(?, ?, 0, ?, 'running', ?, ?)",
            (job_id, world.run_id, sibling_service().HOT[0].ss58, auditor.ss58, LEASE_NONCE),
        )


# The names of `challenge.test_subnet_gate_v2._routes`, one callable per miner-signed v2 route.
GATE_NAMES: dict[str, Key] = {
    "join": ("POST", V2 + "/join"),
    "join/proof": ("POST", V2 + "/join/proof"),
    "rotate": ("POST", V2 + "/admission/{hotkey}/rotate"),
    "recover": ("POST", V2 + "/admission/recover"),
    "objects": ("POST", V2 + "/objects/{sha256}"),
    "accept": ("POST", V2 + "/accept"),
    "commit": ("POST", V2 + "/commit"),
    "delta": ("POST", V2 + "/delta"),
    "leaves": ("POST", V2 + "/leaves"),
    "upload-grant": ("POST", V2 + "/upload-grant"),
    "dispute": ("POST", V2 + "/dispute"),
    "bisect": ("POST", V2 + "/bisect"),
    "state-serve": ("POST", V2 + "/state-serve"),
    "disputes/watch": WATCH_KEY,
    "disputes/ack": ("POST", V2 + "/disputes/ack"),
}


def gated_v2_calls(world: World, hot: Keypair, cold: Keypair) -> dict[Key, Any]:
    """One callable per v2 route that gates on a hotkey, each carrying a body signed by `hot`."""
    named = sibling_gate()._routes(world.network, hot, cold)
    return {GATE_NAMES[name]: fn for name, fn in named.items()}


def gated_v1_calls(client: TestClient, hot: Keypair) -> dict[Key, Any]:
    """One callable per v1 route that checks the hotkey it is given on the subnet."""
    run = "/v1/runs/" + UNKNOWN_RUN
    envelope = {"signer": hot.ss58}
    bodies: dict[str, dict[str, Any]] = {
        "accept": envelope,
        "commit": envelope,
        "delta": envelope,
        "state": envelope,
        "dispute": envelope,
        "leaves": {"w": 0, "hotkey": hot.ss58, "preimages": []},
        "uploads": {"w": 0, "hotkey": hot.ss58, "sha256": "0" * 64},
        "rerun": {"w": 0, "hotkey": hot.ss58, "leaves_root": "0" * 64, "sig": "0" * 128},
    }
    return {
        ("POST", "/v1/runs/{run_id}/" + name): (
            lambda name=name, body=body: client.post(f"{run}/{name}", json=body)
        )
        for name, body in bodies.items()
    }


def detail(response: httpx.Response) -> tuple[int, Any]:
    if "json" not in response.headers.get("content-type", ""):
        return response.status_code, None
    body = response.json()
    return response.status_code, body.get("detail") if isinstance(body, dict) else body


OWNER_MESSAGES = {"challenge": "ChallengeRequest", "job": "JobRequest"}


def owner_post(
    network: Any,
    route: str,
    identity: str,
    key: Keypair | None = None,
    *,
    expires: int | None = None,
) -> httpx.Response:
    """The owner-only trial request `route` ("challenge" or "job") of the admission `identity`:
    a POST whose body is the signed request of `key`, by default the admission's own hotkey
    (ADR 0010 decision 3). The one place that knows the wire shape of these two routes."""
    request = network.owner_request(OWNER_MESSAGES[route], identity, key, expires=expires)
    response: httpx.Response = network.client.post(
        network.url + f"/join/{identity}/{route}", json=request
    )
    return response


def put_object(network: Any, miner: Keypair, data: bytes) -> httpx.Response:
    """The upload of `data` by `miner`, in the shape the route has: a POST whose body is the frame
    of the miner's signed ObjectUploadRequest and the object (ADR 0014 part 8). The one place that
    knows the wire shape of the upload."""
    response: httpx.Response = network.put_object(miner, data)
    return response


def watch_capture(network: Any, party: Keypair) -> Callable[[], httpx.Response]:
    """The poll of `party`'s dispute events, signed once and now: each call of the result replays
    that capture, byte for byte. The request is a signed DisputeWatchRequest in a POST body."""
    poll = {"party": party.ss58, "cursor": 0, "wait": 0}
    request = network.signed(party, "DisputeWatchRequest", poll, network.now + REQUEST_HORIZON)
    url = network.url + "/disputes/watch"
    return lambda: network.client.post(url, json=request)


def relay_receipt(scenario: RelayScenario, signer: Keypair, *, key_id: str = "k1") -> Any:
    """A sealed durability receipt for the scenario's grant, signed by `signer`."""
    from hypertrain.protocol import relay_envelope
    from hypertrain.protocol.relay_messages import RelayReceipt

    body = RelayReceipt(
        w=0,
        hotkey=scenario.miner.ss58,
        delta_hash=HASH,
        size=3,
        grant_hash=scenario.grant_hash,
        chunk_manifest_hash=HASH,
        durable_object_key="x",
        key_id=key_id,
        received_drand=5,
        retain_until=500,
    )
    return relay_envelope.seal(signer, "RelayReceipt", scenario.run_id, body, 1000)


ROLLBACK_FIXTURE = ("li_rollback_fixture", "e2e/test_rollback_service_v2.py")


def open_round(world: World, count: int = 4) -> list[Keypair]:
    """Open round 0 for `count` identities that are ACTIVE by SQL (the 12-trial ladder has its
    own tests), and move the beacon to the round's assignment deadline.

    Four is the smallest roster the weight allocation accepts. Returns the roster's hotkeys.
    """
    from hypertrain.protocol.hashing import sha256_hex
    from hypertrain.protocol.messages_v2 import EscrowLock, RankWorkResult, WorkScreenV2

    service = sibling_service()
    network = world.network
    spec = network.manifest.training.reference_spec
    for i in range(count):
        if i:
            network.push(network.now + 101)  # the join quota refills with the beacon
        identity = network.join(i).json()["admission_id"]
        lock = EscrowLock(
            operation_id=f"{i + 1:064x}",
            owner=service.COLD[i].ss58,
            units=1000,
            origin_ids=[network.origin_ids[i]],
            admission_id=identity,
            dispute_id=None,
            kind="LOCK_ADMISSION",
        )
        locked = network.client.post(
            network.url + "/escrow/lock", json=network.signed(service.COLD[i], "EscrowLock", lock)
        )
        assert locked.status_code == 200, locked.text
        screen = WorkScreenV2(
            env=honest_env(),
            evidence_kind="work-proof-v1",
            admission_id=identity,
            nonce=sha256_hex(b"nonce"),
            policy_hash=network.manifest.network.admission_policy_hash,
            image_digest=spec.image_digest,
            layout=spec.layout,
            challenge_hash=sha256_hex(b"challenge"),
            rank_results=[
                RankWorkResult(
                    rank=rank,
                    fingerprint=sha256_hex(b"rank"),
                    elapsed_ns=1,
                    allocated_bytes=1,
                    reserved_bytes=1,
                    total_bytes=1 << 30,
                )
                for rank in range(spec.layout.n_gpus)
            ],
            artifact_hashes=[sha256_hex(b"artifact")],
        )
        with network.store._tx():
            network.store._db.execute(
                "UPDATE admissions_v2 SET state='ACTIVE',clean_count=12,screen_json=?,"
                "screen_until=1000000000 WHERE admission_id=?",
                (screen.model_dump_json(), identity),
            )
    hotkeys = list(service.HOT[:count])
    rollback = load_fixture_module(*ROLLBACK_FIXTURE)
    opened = rollback.execute_round(network, 0, open_only=True, hotkeys=hotkeys)
    assert opened.status_code == 200, opened.text
    network.push(network.now + 2)
    return hotkeys


class LockSpy:
    """Delegates to a real lock and records the thread of every successful acquisition."""

    def __init__(self, inner: Any) -> None:
        self.inner = inner
        self.holders: list[int] = []

    def acquire(self, *args: Any, **kwargs: Any) -> bool:
        taken: bool = self.inner.acquire(*args, **kwargs)
        if taken:
            self.holders.append(threading.get_ident())
        return taken

    def release(self) -> None:
        self.inner.release()

    def __enter__(self) -> LockSpy:
        self.acquire()
        return self

    def __exit__(self, *exc: object) -> None:
        self.release()
