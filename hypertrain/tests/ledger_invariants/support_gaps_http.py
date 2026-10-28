"""Helpers private to the http gap tests of the ledger-invariants lot. Not collected.

The environment scan reads, from the source text of `Config.from_env`, every variable name the
service takes from the process environment, so that the scrub list of the gap tests can be
checked against the code and not against a memory of it. The bearer helpers rebuild what
`test_http_auth.py` keeps to itself. `envelope_routes` seals, for a run of the caller's choice, the
request of every envelope-bearing route the first wave's table of `test_expiry_2.py` leaves out.
`dispute_write` and `PollerGate` stage a dispute write against a long-poll.
"""

from __future__ import annotations

import ast
import textwrap
import threading
from collections.abc import Callable, Mapping
from dataclasses import dataclass, field
from typing import Any

import httpx
import pytest

from .support import World
from .support_exit_roster import control
from .support_http_auth import CHALLENGE_ROUTES, INT, OP, WK

BEARER_FILES = {OP: "admin", WK: "worker", INT: "internal"}
UNDECODABLE = b"\xff\xfe\xfd"

# Every name `Config.from_env` reads. test_gaps_http derives the names from the source and
# requires them to be inside this tuple, so a new knob of the service cannot reach the lot's
# tests through an ambient variable without this list (and the scrub built from it) saying so.
SCRUB = (
    "CHALLENGE_SLUG",
    "CHALLENGE_STATE_DIR",
    "CHALLENGE_MASTER_URL",
    "CHALLENGE_NETUID",
    "CHALLENGE_INTERNAL_TOKEN_FILE",
    "CHALLENGE_ADMIN_TOKEN_FILE",
    "CHALLENGE_WORKER_TOKEN_FILE",
    "HYPERTRAIN_COORD_KEY_FILE",
    "HYPERTRAIN_OWNER_HOTKEY",
    "HYPERTRAIN_GENESIS_UNIX",
    "HYPERTRAIN_EPOCH_SECONDS",
    "HYPERTRAIN_EPOCHS_PER_ROUND",
    "HYPERTRAIN_VEST_ROUNDS",
    "HYPERTRAIN_ROLE_DISPATCH",
    "HYPERTRAIN_CUDA_EXECUTOR",
    "HYPERTRAIN_RELAY_LIFECYCLE_INTERVAL",
)

# A value for each name that is valid for its type and differs from its default.
POLLUTION = {
    "CHALLENGE_SLUG": "polluted-slug",
    "CHALLENGE_STATE_DIR": "{polluted}/state",
    "CHALLENGE_MASTER_URL": "https://polluted.test",
    "CHALLENGE_NETUID": "999",
    "CHALLENGE_INTERNAL_TOKEN_FILE": "{polluted}/internal",
    "CHALLENGE_ADMIN_TOKEN_FILE": "{polluted}/admin",
    "CHALLENGE_WORKER_TOKEN_FILE": "{polluted}/worker",
    "HYPERTRAIN_COORD_KEY_FILE": "{polluted}/coord.key",
    "HYPERTRAIN_OWNER_HOTKEY": "polluted-owner",
    "HYPERTRAIN_GENESIS_UNIX": "1234567890",
    "HYPERTRAIN_EPOCH_SECONDS": "1234",
    "HYPERTRAIN_EPOCHS_PER_ROUND": "7",
    "HYPERTRAIN_VEST_ROUNDS": "77",
    "HYPERTRAIN_ROLE_DISPATCH": "{polluted}/dispatch",
    "HYPERTRAIN_CUDA_EXECUTOR": "{polluted}/cuda",
    "HYPERTRAIN_RELAY_LIFECYCLE_INTERVAL": "7",
}


def scrub(patch: pytest.MonkeyPatch) -> None:
    """Remove every name of SCRUB from the environment for as long as `patch` lives."""
    for name in SCRUB:
        patch.delenv(name, raising=False)


class _EnvReads(ast.NodeVisitor):
    """The names a source reads from the environment, whatever the spelling of the read."""

    READERS = frozenset({"env", "path", "getenv"})
    QUALIFIED = frozenset({"os.environ.get", "os.getenv", "environ.get"})

    def __init__(self) -> None:
        self.names: set[str] = set()
        self._inside: list[str] = []

    def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
        self._inside.append(node.name)
        self.generic_visit(node)
        self._inside.pop()

    def visit_Call(self, node: ast.Call) -> None:
        func = node.func
        reads = (isinstance(func, ast.Name) and func.id in self.READERS) or (
            isinstance(func, ast.Attribute) and ast.unparse(func) in self.QUALIFIED
        )
        if reads and node.args:
            first = node.args[0]
            if isinstance(first, ast.Constant) and isinstance(first.value, str):
                self.names.add(first.value)
            elif "path" not in self._inside:
                # A computed name outside the one helper that is only called with constants.
                self.names.add("<computed>")
        self.generic_visit(node)

    def visit_Subscript(self, node: ast.Subscript) -> None:
        if ast.unparse(node.value) == "os.environ":
            key = node.slice
            if isinstance(key, ast.Constant) and isinstance(key.value, str):
                self.names.add(key.value)
            else:
                self.names.add("<computed>")
        self.generic_visit(node)


def env_names_read(source: str) -> set[str]:
    """Every environment variable the code of `source` reads; "<computed>" for a name it builds."""
    reads = _EnvReads()
    reads.visit(ast.parse(textwrap.dedent(source)))
    return reads.names


def bearer_keys(principal: str) -> list[tuple[str, str]]:
    return sorted(key for key, auth in CHALLENGE_ROUTES.items() if auth.principal == principal)


def bearer_headers(world: World, principal: str) -> dict[str, str]:
    return {OP: world.admin, WK: world.worker, INT: world.internal}[principal]


def expect(condition: object, what: str) -> None:
    """A scenario step of a strict xfail test: its failure is a broken scenario, not the hole."""
    control(condition, what)


# --- T-SG-01: the envelope-bearing routes the first wave's table leaves out ---------------------


REFUSALS = {
    "sealed-for-another-run": (409, "envelope belongs to another run"),
    "relabeled-to-this-run": (409, "sr25519 signature does not verify"),
}


@dataclass(frozen=True)
class EnvelopeRoute:
    """One route type that reads a signed envelope: where to post, the payload sealed for a given
    run with a given expiry, and the refusal each way of making the envelope unfit earns."""

    name: str
    method: str
    path: str
    admin: bool
    payload: Callable[[str, int], Any]
    refusals: Mapping[str, tuple[int, str]] = field(default_factory=lambda: dict(REFUSALS))

    def post(self, world: World, payload: Any, client: Any = None) -> httpx.Response:
        headers = world.admin if self.admin else {}
        return (client or world.client).request(
            self.method, f"/v2/runs/{world.run_id}{self.path}", json=payload, headers=headers
        )


def envelope_routes(world: World) -> dict[str, EnvelopeRoute]:
    """The routes, with the stored state each of them needs before it reaches its envelope.

    One open contest of miner 0 (resolution), a relay scenario on the same miner (observer,
    relay failure), an admitted miner 0 (upload grant, trial finalize). Everything is written the
    way the first-wave tests write it; the routes are only ever sent refusals.
    """
    from hypertrain.protocol import envelope_v2, relay_envelope
    from hypertrain.protocol.hashing import sha256_hex
    from hypertrain.protocol.jcs import canonicalize
    from hypertrain.protocol.messages import Finalize, Receipt, Rollback
    from hypertrain.protocol.messages_v2 import CommitV2, OriginAllocation, ProductionBootstrap
    from hypertrain.protocol.relay_messages import (
        AvailabilityContext,
        RelayFailureEvidence,
        UploadChunk,
        UploadChunkManifest,
    )

    from . import support_exit_disputes as X
    from .support_http_auth import seed_relay_scenario

    service = X.service()
    hot, coord, referee, auditor = (
        service.HOT[0],
        service.COORD,
        service.REFEREE,
        service.AUDITORS[0],
    )
    dispute_id = X.open_contest(world, 0)
    resolution = X.register(
        world, X.disputes(world).get(dispute_id), reason="FRAUD", loser=hot.ss58
    )
    scenario = seed_relay_scenario(world)
    admission_id = world.store._db.execute(
        "SELECT admission_id FROM admissions_v2 WHERE hotkey=?", (hot.ss58,)
    ).fetchone()[0]
    observer_path = f"/relay-observers/{scenario.grant_hash}/{auditor.ss58}"
    challenge = world.client.get(f"/v2/runs/{world.run_id}{observer_path}")
    control(challenge.status_code == 200, f"observer challenge: {challenge.text}")
    data = b"abc"
    chunks = UploadChunkManifest(
        size=len(data),
        chunks=[UploadChunk(index=0, off=0, len=len(data), chunk_sha256=sha256_hex(data))],
    )
    rollback = Rollback(
        w=0,
        excluded=[hot.ss58],
        old_theta_hash_w2=sha256_hex(b"old"),
        new_theta_hash_w2=sha256_hex(b"new"),
        new_outer_state_hash=sha256_hex(b"outer"),
        recomputed=["agg_w", "step_w", "agg_w1", "step_w1"],
        cause_hashes=[sha256_hex(b"cause")],
    )
    finalize = Finalize(
        w=0,
        final_theta_hash_w1=sha256_hex(b"theta"),
        included=[hot.ss58],
        entitlements_root="0" * 64,
    )
    registry = world.store._registry_v2(world.network.manifest)
    spec = next(item for item in registry.specs if item.id == scenario.grant.relay_id)
    one = sha256_hex(b"one")

    def bootstrap(run: str) -> ProductionBootstrap:
        return ProductionBootstrap(
            run_id=run,
            operation_id=one,
            allocation_hash=one,
            total_units=1,
            origins=[
                OriginAllocation(origin_id=one, owner=service.COLD[0].ss58, units=1, mature_at=1)
            ],
            authority_sig="0" * 128,
        )

    def failure(run: str) -> RelayFailureEvidence:
        return RelayFailureEvidence(
            run_id=run,
            relay_id=spec.id,
            key_id=spec.pubkeys[0].key_id,
            assignment_hash=one,
            grant_hash=scenario.grant_hash,
            upload_ack_hash=one,
            chunk_manifest_hash=one,
            chunk_ack_hashes=[],
            receipt_hash=None,
            retention_hash=one,
            request=None,
            response=None,
            observer_ids=[auditor.ss58],
            observed_beacons=[1],
            result="UNCONFIRMED_AVAILABILITY",
            availability_context=AvailabilityContext(
                master_healthy=True,
                beacon_healthy=True,
                reference_healthy=True,
                observer_healthy=True,
            ),
            evidence_hash=sha256_hex(canonicalize([])),
        )

    commit = CommitV2(
        w=0,
        hotkey=hot.ss58,
        leaf_scheme="ht-leaf-v1",
        n_leaves=1,
        leaves_root=one,
        metrics_root=one,
        final_theta_hash=one,
        ef_in_hash=one,
        ef_out_hash=one,
        delta_hash=one,
        delta_bytes=1,
        tokens=1,
    )

    def shadow_reservation(unfit: str) -> Callable[[str, int], dict[str, Any]]:
        """The request with the envelope named `unfit` sealed for the run asked for and the
        other one for this run, so that each of the two intakes is told apart."""

        def build(run: str, exp: int) -> dict[str, Any]:
            return {
                "finalize": envelope_v2.seal(
                    coord, "Finalize", run if unfit == "finalize" else world.run_id, finalize, exp
                ),
                "commit": envelope_v2.seal(
                    hot, "CommitV2", run if unfit == "commit" else world.run_id, commit, exp
                ),
                "custody": dict.fromkeys(
                    (
                        "job_hash",
                        "reference_publication_hash",
                        "miner_publication_hash",
                        "reference_result_hash",
                        "miner_result_hash",
                        "reference_operation_hash",
                        "miner_operation_hash",
                    ),
                    one,
                ),
            }

        return build

    def v2(key: Any, kind: str, body: Any) -> Callable[[str, int], Any]:
        return lambda run, exp: envelope_v2.seal(key, kind, run, body, exp)

    def relay(key: Any, kind: str, body: Any) -> Callable[[str, int], Any]:
        return lambda run, exp: relay_envelope.seal(key, kind, run, body, exp)

    def failure_request(run: str, exp: int) -> dict[str, Any]:
        sealed = relay_envelope.seal(coord, "RelayFailureEvidence", run, failure(run), exp)
        return {"evidence": sealed, "artifacts": [], "observations": []}

    domain = (403, "failure authority/domain mismatch")
    routes = [
        EnvelopeRoute(
            "resolution", "POST", "/resolution", False, v2(referee, "ResolutionV2", resolution)
        ),
        EnvelopeRoute(
            "observer-response",
            "POST",
            observer_path,
            False,
            v2(auditor, "Receipt", Receipt.model_validate(challenge.json()["body"])),
        ),
        EnvelopeRoute(
            "upload-grant",
            "POST",
            "/upload-grant",
            False,
            relay(hot, "UploadChunkManifest", chunks),
        ),
        EnvelopeRoute(
            "trial-finalize",
            "POST",
            f"/admin/join/{admission_id}/finalize",
            True,
            v2(coord, "Finalize", finalize),
        ),
        EnvelopeRoute("rollback", "POST", "/admin/rollback", True, v2(coord, "Rollback", rollback)),
        EnvelopeRoute(
            "relay-registry",
            "POST",
            "/admin/relay-registry",
            True,
            relay(coord, "RelayRegistryV1", registry),
        ),
        EnvelopeRoute(
            "shadow-reservation-finalize",
            "POST",
            f"/admin/join/{admission_id}/shadow-reservation",
            True,
            shadow_reservation("finalize"),
        ),
        EnvelopeRoute(
            "shadow-reservation-commit",
            "POST",
            f"/admin/join/{admission_id}/shadow-reservation",
            True,
            shadow_reservation("commit"),
        ),
        EnvelopeRoute(
            "bootstrap",
            "POST",
            "/admin/bootstrap",
            True,
            lambda run, exp: envelope_v2.seal(
                coord, "ProductionBootstrap", run, bootstrap(run), exp
            ),
        ),
        EnvelopeRoute(
            "relay-failure",
            "POST",
            "/admin/relay-failure",
            True,
            failure_request,
            {"sealed-for-another-run": domain, "relabeled-to-this-run": domain},
        ),
    ]
    return {route.name: route for route in routes}


def relabeled(payload: Any, run_id: str) -> Any:
    """`payload` with the run of its envelope changed; a request that wraps envelopes (a relay
    failure, a shadow reservation) has them all relabeled."""
    if isinstance(payload, dict) and "evidence" in payload:
        return {**payload, "evidence": {**payload["evidence"], "run_id": run_id}}
    if isinstance(payload, dict) and "finalize" in payload:
        return {
            **payload,
            "finalize": {**payload["finalize"], "run_id": run_id},
            "commit": {**payload["commit"], "run_id": run_id},
        }
    return {**payload, "run_id": run_id}


# --- T-DS-01: the writers of a dispute against a long-poll --------------------------------------

PATIENCE = 4.0  # seconds a thread may wait for a lock before the test declares it stuck
START = 30.0  # seconds a started thread may take to reach the service on a loaded host


class PollerGate:
    """Test-owned stand-in for `EscrowV2.mutex`: the store's own lock, plus a signal when the
    poller asks for it and a bounded wait for the poller only, so a stuck pair can be released."""

    def __init__(self, inner: Any, attempted: threading.Event) -> None:
        self.inner, self.attempted = inner, attempted

    def __enter__(self) -> PollerGate:
        if threading.current_thread().name != "poller":
            self.inner.acquire()
            return self
        self.attempted.set()
        if not self.inner.acquire(timeout=PATIENCE):
            raise TimeoutError("the poller waited for the store lock while holding the condition")
        return self

    def __exit__(self, *exc: object) -> None:
        self.inner.release()


def in_thread(name: str, work: Callable[[], Any]) -> tuple[threading.Thread, dict[str, Any]]:
    box: dict[str, Any] = {}

    def run() -> None:
        try:
            box["result"] = work()
        except BaseException as error:  # reported to the test, never swallowed
            box["error"] = error

    thread = threading.Thread(target=run, name=name, daemon=True)
    thread.start()
    return thread, box


@dataclass(frozen=True)
class DisputeWrite:
    """A write that changes a dispute: the method of `DisputesV2` it calls inside the store
    transaction, how to send it, and how to see that it took effect."""

    method: str
    act: Callable[[], Any]
    took_effect: Callable[[], bool]


def dispute_write(world: World, kind: str) -> tuple[str, DisputeWrite]:
    """Open miner 0's contest and make the write `kind` ready; returns the dispute id."""
    from hypertrain.protocol.jcs import canonicalize
    from hypertrain.protocol.messages_v2 import BisectV2

    from . import support_exit_disputes as X

    store, net, hot = world.store, world.network, X.service().HOT[0]
    dispute_id = X.open_contest(world, 0)
    d = X.disputes(world)
    turn = d.get(dispute_id)
    if kind == "bisect":
        body = BisectV2(
            dispute_id=dispute_id,
            seq=turn.seq,
            level=turn.level,
            ctx=turn.ctx,
            interval=turn.interval,
            N=2,
            hashes=X.AGREE,
            party=hot.ss58,
            previous_transcript_hash=turn.transcript_hash,
        )
        raw = canonicalize(net.signed(hot, "BisectV2", body))
        return dispute_id, DisputeWrite(
            "bisect",
            lambda: store.admission_v2(world.run_id, "bisect", raw),
            lambda: d.get(dispute_id).seq > turn.seq,
        )
    if kind == "resolution":
        resolution = X.register(world, turn, reason="MATCH", loser=None)
        raw = canonicalize(
            net.signed(X.service().REFEREE, "ResolutionV2", resolution, expiry=10**6)
        )
        return dispute_id, DisputeWrite(
            "resolution",
            lambda: store.admission_v2(world.run_id, "resolution", raw),
            lambda: d.get(dispute_id).resolution is not None,
        )
    if kind == "state-request":
        events = len(d.events(hot.ss58, 0))
        return dispute_id, DisputeWrite(
            "state_request",
            lambda: store.dispute_state_v2(world.run_id, dispute_id, 0),
            lambda: len(d.events(hot.ss58, 0)) > events,
        )
    X.acknowledge_turn(world, dispute_id)
    net.push(turn.turn_deadline + 1)
    request = canonicalize(X.timeout_request(world, dispute_id))
    return dispute_id, DisputeWrite(
        "timeout",
        lambda: store.dispute_timeout_v2(world.run_id, dispute_id, request),
        lambda: d.get(dispute_id).resolution is not None,
    )


# --- T-AU-03: weak and shared secrets of the bearer classes the first wave left out -------------

STRONG = {"admin": "a" * 40, "worker": "b" * 40, "internal": "c" * 40}
PROBES = {
    "admin": ("GET", "/v1/aggregator/runs/{run_id}/rounds/{w}/inputs"),
    "worker": ("GET", "/v1/objects/{sha256}"),
    "internal": ("GET", "/internal/v1/get_weights"),
}
# Whatever way a fix refuses weak secrets at start-up, building or starting the app raises.
REFUSED_START = (ValueError, RuntimeError)


def reaches_the_gate(root: Any, tokens: Mapping[str, str], presented: str, cls: str) -> bool:
    """True when `presented` as the bearer gets past the gate of the probe of class `cls` on an
    application whose three bearer files hold `tokens`. An application that refuses to start with
    these secrets has held the line, so it is False."""
    from fastapi.testclient import TestClient

    from hypertrain.challenge.app import Config, create_app
    from hypertrain.ledger import Params

    from .support import SLUG
    from .support_http_auth import bearer, call, with_slug

    root.mkdir(parents=True, exist_ok=True)
    files = {}
    for name, token in tokens.items():
        files[name] = root / name
        files[name].write_text(token)
    try:
        app = create_app(
            Config(
                SLUG,
                root / "state",
                "https://master.test",
                files["internal"],
                files["admin"],
                files["worker"],
                None,
                None,
                Params(SLUG, 1_900_000_000, 300, 1, 1),
            ),
            transport=httpx.MockTransport(lambda _: httpx.Response(503)),
        )
        with TestClient(app) as client:
            headers = with_slug(bearer(presented))
            return call(client, PROBES[cls], headers).status_code not in (401, 503)
    except REFUSED_START:
        return False
