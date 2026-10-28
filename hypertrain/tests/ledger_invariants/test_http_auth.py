"""Authentication of the HTTP routes: bearers, signed envelopes, signing domains, server keys.

Package http_auth of the ledger-invariants lot, part 1. Every test states a defence the service
must hold: a route refuses a caller that lacks the credential it names, and a refusal changes no
stored state. The route tables of `support_http_auth` are the contract; a route nobody classified
fails `test_every_route_of_the_three_applications_has_a_recorded_principal`.
"""

from __future__ import annotations

import ast
import asyncio
import re
from contextlib import ExitStack
from pathlib import Path
from typing import Any

import httpx
import pytest
from fastapi.testclient import TestClient

import hypertrain
from env_pin_fixtures import honest_env
from hypertrain.challenge.app import Config, create_app
from hypertrain.challenge.store import ChallengeError
from hypertrain.ledger import Params
from hypertrain.protocol import envelope_v2, relay_envelope
from hypertrain.protocol.envelope import SignatureError, UnauthorizedSigner
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages import MESSAGE_TYPES, Receipt
from hypertrain.protocol.messages_v2 import MESSAGE_TYPES_V2
from hypertrain.protocol.relay_messages import RELAY_MESSAGE_TYPES
from hypertrain.protocol.server_keys import SignedGrant
from hypertrain.protocol.server_keys import signing_message as server_key_message
from hypertrain.relay.app import create_app as create_relay_app

from .support import SLUG, load_fixture_module, routes, service_world
from .support_gaps_http import expect
from .support_http_auth import (
    CHALLENGE_ROUTES,
    GATE_TEXT,
    GATED_V1,
    GATED_V2,
    HASH,
    INT,
    MOUNTS,
    NONE,
    OP,
    RELAY_ROUTES,
    REVOKED_TEXT,
    WATCH_KEY,
    WK,
    bearer,
    call,
    changed,
    detail,
    durable_state,
    endpoints,
    gated_v1_calls,
    gated_v2_calls,
    owner_post,
    quiet_client,
    relay_receipt,
    seed_audit_lease,
    seed_relay_scenario,
    server_key_grant,
    server_key_revoke,
    sibling_service,
    source_of,
    with_slug,
)

RELAY_FIXTURE = ("li_relay_fixture", "relay/test_transport_v2.py")
BEARER_FILES = {OP: "admin", WK: "worker", INT: "internal"}
BEARER_TOKENS = {OP: "integration-admin", WK: "integration-worker", INT: "integration-internal"}
SOURCE = Path(hypertrain.__file__).resolve().parent


def headers_of(world: Any, principal: str) -> dict[str, str]:
    return {OP: world.admin, WK: world.worker, INT: world.internal}[principal]


def keys_of(principal: str) -> list[tuple[str, str]]:
    return sorted(k for k, a in CHALLENGE_ROUTES.items() if a.principal == principal)


def statuses(client: TestClient, keys: list[tuple[str, str]], headers: dict[str, str]) -> dict:
    return {key: call(client, key, headers).status_code for key in keys}


def derived_principal(handler: Any) -> str:
    """The credential a challenge handler asks for, read from its own source text."""
    text = source_of(handler)
    found = {
        name
        for name, marker in (
            (OP, "admin(authorization)"),
            (WK, "worker(authorization)"),
            (INT, "_require(config.internal_token_file"),
        )
        if marker in text
    }
    assert len(found) <= 1, f"{handler.__name__} checks two bearer classes: {sorted(found)}"
    return found.pop() if found else NONE


def derived_relay_principal(handler: Any) -> str:
    text = source_of(handler)
    for name, marker in (
        ("DRAIN", "drain_token"),
        ("HEADER", "upload_capability("),
        ("HEADER", "X-Retrieval-Hash"),
        ("ENVELOPE", "envelope_v2.load_json"),
        ("ENVELOPE", "relay_envelope.parse_envelope"),
    ):
        if marker in text:
            return name
    return NONE


# T-03 -------------------------------------------------------------------------------------


def test_every_route_of_the_three_applications_has_a_recorded_principal(tmp_path: Path) -> None:
    # Given: the real challenge application (v1, v2 and the public mount) and the relay factory.
    with service_world(tmp_path) as world:
        found = routes(world.app)
        challenge = endpoints(world.app)
    served = {(r.method, r.path) for r in found if r.method != "MOUNT"}
    # When/Then: a route nobody classified, or a classified route that is gone, fails here.
    assert {(r.method, r.path) for r in found if r.method == "MOUNT"} == MOUNTS
    assert (served - set(CHALLENGE_ROUTES), set(CHALLENGE_ROUTES) - served) == (set(), set())
    # And: the recorded bearer class is the one the handler really checks.
    declared = {
        key: auth.principal
        for key, auth in CHALLENGE_ROUTES.items()
        if auth.principal in (OP, WK, INT)
    }
    undeclared = {key for key in challenge if key not in declared}
    assert {key: derived_principal(challenge[key]) for key in declared} == declared
    assert {key for key in undeclared if derived_principal(challenge[key]) != NONE} == set()


def test_every_relay_route_has_a_recorded_principal(tmp_path: Path) -> None:
    fixture = load_fixture_module(*RELAY_FIXTURE)

    async def scenario() -> dict[tuple[str, str], Any]:
        async with httpx.AsyncClient() as wire:
            rig = fixture.Rig(tmp_path, wire)
            return endpoints(create_relay_app(rig.relay, "d" * 32))

    served = asyncio.run(scenario())
    assert (set(served) - set(RELAY_ROUTES), set(RELAY_ROUTES) - set(served)) == (set(), set())
    assert {key: derived_relay_principal(fn) for key, fn in served.items()} == {
        key: auth.principal for key, auth in RELAY_ROUTES.items()
    }


# AU-01 / AU-03 ----------------------------------------------------------------------------


def wrong_bearers(principal: str) -> dict[str, dict[str, str]]:
    token = BEARER_TOKENS[principal]
    return {
        "no credential": {},
        "token without scheme": {"Authorization": token},
        "empty bearer": {"Authorization": "Bearer "},
        "token with a byte appended": bearer(token + "x"),
        "token missing its last byte": bearer(token[:-1]),
        "token of the same length with its last byte changed": bearer(
            token[:-1] + ("0" if token[-1] != "0" else "1")
        ),
    }


@pytest.mark.parametrize("principal", [OP, WK, INT])
def test_a_bearer_route_refuses_every_wrong_credential_and_changes_nothing(
    tmp_path: Path, principal: str
) -> None:
    """AU-01. Without its exact bearer a route answers 401 and no store method runs."""
    keys = keys_of(principal)
    with service_world(tmp_path) as world:
        client = quiet_client(world.app)
        before = world.db_digest()
        refused = {
            name: statuses(client, keys, with_slug(headers))
            for name, headers in wrong_bearers(principal).items()
        }
        assert world.db_digest() == before
        # Control: the exact bearer passes the gate of every route (the answer is then about the
        # unknown ids of the probe, never 401 or 503), so the refusals above are the gate's.
        passed = statuses(client, keys, with_slug(headers_of(world, principal)))
    assert refused == {name: dict.fromkeys(keys, 401) for name in wrong_bearers(principal)}
    assert {key: code for key, code in passed.items() if code in (401, 503)} == {}


@pytest.mark.parametrize("principal", [OP, WK, INT])
def test_no_bearer_class_opens_a_route_of_another_class(tmp_path: Path, principal: str) -> None:
    """AU-03. The worker, internal and operator bearers are three separate secrets."""
    keys = keys_of(principal)
    foreign = [p for p in (OP, WK, INT) if p != principal]
    with service_world(tmp_path) as world:
        client = quiet_client(world.app)
        before = world.db_digest()
        refused = {
            other: statuses(client, keys, with_slug(headers_of(world, other))) for other in foreign
        }
        assert world.db_digest() == before
    assert refused == {other: dict.fromkeys(keys, 401) for other in foreign}


@pytest.mark.parametrize("principal", [OP, WK, INT])
def test_an_unconfigured_bearer_makes_its_routes_503_for_every_caller(
    tmp_path: Path, principal: str
) -> None:
    """AU-01 (T-AU-02). A missing, empty or blank token file never opens a route."""
    keys = keys_of(principal)
    path = tmp_path / BEARER_FILES[principal]
    with service_world(tmp_path) as world:
        original = path.read_text()
        client = quiet_client(world.app)
        callers = {
            "its own bearer": headers_of(world, principal),
            "no credential": {},
            "an empty bearer": {"Authorization": "Bearer "},
        }
        before = world.db_digest()
        seen: dict[str, dict] = {}
        for damage in ("missing", "empty", "blank"):
            if damage == "missing":
                path.unlink()
            else:
                path.write_text("" if damage == "empty" else " \n")
            seen[damage] = {
                name: statuses(client, keys, with_slug(headers))
                for name, headers in callers.items()
            }
        path.write_text(original)
        assert world.db_digest() == before
        restored = statuses(client, keys, with_slug(headers_of(world, principal)))
        # The fixture's own application is configured with the operator token only: for the
        # other two classes the token path is unset, which is the same refusal.
        unset = (
            {}
            if principal == OP
            else statuses(world.network.client, keys, with_slug(headers_of(world, principal)))
        )
    assert seen == {
        damage: {name: dict.fromkeys(keys, 503) for name in callers}
        for damage in ("missing", "empty", "blank")
    }
    assert unset == ({} if principal == OP else dict.fromkeys(keys, 503))
    assert {key: code for key, code in restored.items() if code in (401, 503)} == {}


# AU-02 / HTP-01 ---------------------------------------------------------------------------


def bare_app(root: Path, tokens: dict[str, str]) -> Any:
    """A challenge application whose three bearer files hold `tokens` (admin, worker, internal)."""
    root.mkdir(parents=True, exist_ok=True)
    files = {}
    for name, token in tokens.items():
        files[name] = root / name
        files[name].write_text(token)
    return create_app(
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


ADMIN_PROBE = ("GET", "/v1/aggregator/runs/{run_id}/rounds/{w}/inputs")
WORKER_PROBE = ("GET", "/v1/objects/{sha256}")
# Whatever way a fix refuses weak secrets at start-up, building or starting the app raises.
REFUSED_START = (ValueError, RuntimeError, ChallengeError)


def reaches_the_gate(
    root: Path, tokens: dict[str, str], presented: str, probe: tuple[str, str] = ADMIN_PROBE
) -> bool:
    """True when `presented` as the bearer gets past the gate of the route `probe`.

    An application that refuses to start with these secrets has held the line, so it is False.
    """
    with ExitStack() as stack:
        try:
            client = stack.enter_context(TestClient(bare_app(root, tokens)))
        except REFUSED_START:
            return False
        return call(client, probe, bearer(presented)).status_code not in (401, 503)


STRONG = {"admin": "a" * 40, "worker": "b" * 40, "internal": "c" * 40}


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason=(
        "AU-02+HTP-01: an operator bearer shorter than 32 bytes is never accepted "
        "[fix-lot: none (no design yet)]"
    ),
)
def test_a_bearer_shorter_than_32_bytes_never_opens_a_route(tmp_path: Path) -> None:
    # Control: 32+ byte, pairwise different secrets open the operator route.
    expect(reaches_the_gate(tmp_path / "strong", STRONG, STRONG["admin"]), "a strong secret")
    # The weak operator secret must be refused (no start, or no access), not honoured.
    weak = {**STRONG, "admin": "a"}
    assert not reaches_the_gate(tmp_path / "weak", weak, "a")


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason=(
        "AU-02+HTP-01: one secret never serves two bearer classes [fix-lot: none (no design yet)]"
    ),
)
def test_one_secret_never_serves_two_bearer_classes(tmp_path: Path) -> None:
    # Control: pairwise different secrets, each opens the routes of its own class.
    expect(reaches_the_gate(tmp_path / "distinct", STRONG, STRONG["admin"]), "the operator")
    expect(reaches_the_gate(tmp_path / "own", STRONG, STRONG["worker"], WORKER_PROBE), "the worker")
    shared = {**STRONG, "worker": STRONG["admin"]}
    opens = [
        reaches_the_gate(tmp_path / "shared", shared, STRONG["admin"], probe)
        for probe in (ADMIN_PROBE, WORKER_PROBE)
    ]
    assert opens.count(True) <= 1, "the same secret opens the operator and the worker routes"


# AU-05 ------------------------------------------------------------------------------------


def test_a_signer_off_the_subnet_is_refused_403_before_any_state_change_on_every_gated_route(
    tmp_path: Path,
) -> None:
    """AU-05 (T-AU-06). A valid signature by a hotkey that is not on the metagraph buys nothing."""
    with service_world(tmp_path) as world:
        assert world.network.join(0).status_code == 200  # a live admission exists to be touched
        outsider, outsider_cold = Keypair(b"\x55" * 32), Keypair(b"\x56" * 32)
        calls = {
            **gated_v2_calls(world, outsider, outsider_cold),
            **gated_v1_calls(world.client, outsider),
        }
        before = durable_state(world)
        refused = {key: detail(fn()) for key, fn in calls.items()}
        after = durable_state(world)
    # Every route that gates on a hotkey is exercised, and none that does not is.
    assert set(calls) == set(GATED_V2) | set(GATED_V1)
    assert refused == {key: (403, GATE_TEXT) for key in calls}
    assert changed(before, after) == set()


def test_a_revoked_server_key_is_refused_403_on_every_gated_route_though_still_on_the_subnet(
    tmp_path: Path,
) -> None:
    """AU-05. The coldkey that granted a server key can take it off every miner route."""
    service = sibling_service()
    server, cold = service.HOT[1], service.COLD[1]
    with service_world(tmp_path) as world:
        n, client = world.network, world.client
        identity = n.join(1).json()["admission_id"]
        n.push(2)
        grant = server_key_grant(server, cold)
        assert client.post("/v2/server-keys", json=grant.model_dump(mode="json")).status_code == 201
        calls = {**gated_v2_calls(world, server, cold), **gated_v1_calls(client, server)}
        # The two owner requests that gate on the hotkey of the admission they name: the server
        # key is that hotkey, and each call is a valid request of that owner (the challenge
        # first, which issues the trial the job exports).
        owner_calls = {
            "trial challenge": lambda: owner_post(n, "challenge", identity),
            "trial job": lambda: owner_post(n, "job", identity),
        }
        # Control: granted and not revoked, the server key still passes every gate.
        assert detail(calls[WATCH_KEY]()) == (200, [])
        assert {name: fn().status_code for name, fn in owner_calls.items()} == {
            "trial challenge": 200,
            "trial job": 200,
        }
        revoke = server_key_revoke(cold)
        assert client.post("/v2/server-keys/revoke", json=revoke.model_dump(mode="json")).is_success
        before = durable_state(world)
        refused = {key: detail(fn()) for key, fn in calls.items()}
        refused_owner = {name: detail(fn()) for name, fn in owner_calls.items()}
        after = durable_state(world)
    assert refused == {key: (403, REVOKED_TEXT) for key in calls}
    assert refused_owner == {name: (403, REVOKED_TEXT) for name in owner_calls}
    assert changed(before, after) == set()


def test_a_forged_signer_is_refused_on_the_signed_routes_that_have_no_membership_gate(
    tmp_path: Path,
) -> None:
    """AU-05 (T-AU-06). Escrow, auditor, observer and relay routes pin the signer in the store."""
    service = sibling_service()
    from hypertrain.protocol.messages_v2 import EscrowLock, EscrowRelease, EscrowTransfer

    with service_world(tmp_path) as world:
        n = world.network
        admission = n.join(0).json()["admission_id"]
        scenario = seed_relay_scenario(world)
        seed_audit_lease(world, service.AUDITORS[0])
        owner, stranger = service.COLD[0].ss58, service.COLD[1]
        shared = {"owner": owner, "units": 1000, "origin_ids": [n.origin_ids[0]]}
        lock = EscrowLock(
            operation_id="01" * 32,
            admission_id=admission,
            dispute_id=None,
            kind="LOCK_ADMISSION",
            **shared,
        )
        transfer = EscrowTransfer(
            operation_id="02" * 32,
            admission_id=None,
            dispute_id=None,
            recipient=stranger.ss58,
            **shared,
        )
        release = EscrowRelease(
            operation_id="03" * 32,
            admission_id=None,
            dispute_id=None,
            lock_id=HASH,
            finality_hash=HASH,
            closed_dispute_root=HASH,
            **shared,
        )
        ask = Receipt(w=0, commit_hash=HASH, received_round=1)
        observer_url = n.url + f"/relay-observers/{scenario.grant_hash}/{service.HOT[1].ss58}"
        receipt = relay_receipt(scenario, service.HOT[1])
        execute = n.url + f"/worker/jobs/{HASH}/execute"
        # Control: the lease is live and the pinned auditor owns it, so its own ask gets past the
        # lease check and stops at the lease nonce; the miner below must stop at the lease itself.
        lessee = n.client.post(execute, json=n.signed(service.AUDITORS[0], "Receipt", ask))
        assert detail(lessee) == (403, "audit lease nonce differs")
        attempts = {
            "lock signed for another owner": lambda: n.client.post(
                n.url + "/escrow/lock", json=n.signed(stranger, "EscrowLock", lock)
            ),
            "transfer signed for another owner": lambda: n.client.post(
                n.url + "/escrow/transfer", json=n.signed(stranger, "EscrowTransfer", transfer)
            ),
            "release signed for another owner": lambda: n.client.post(
                n.url + "/escrow/release", json=n.signed(stranger, "EscrowRelease", release)
            ),
            "lease asked by a miner": lambda: n.client.post(
                n.url + "/worker/lease", json=n.signed(service.HOT[0], "Receipt", ask)
            ),
            "execution of a leased job asked by a miner": lambda: n.client.post(
                execute, json=n.signed(service.HOT[0], "Receipt", ask)
            ),
            "observer response by a miner": lambda: n.client.post(
                observer_url, json=n.signed(service.HOT[1], "Receipt", ask)
            ),
            "relay receipt by a miner": lambda: n.client.post("/v2/relay-receipts", json=receipt),
        }
        before = durable_state(world)
        refused = {name: detail(fn()) for name, fn in attempts.items()}
        after = durable_state(world)
    assert refused == {
        "lock signed for another owner": (409, "signer is not authorized for message role"),
        "transfer signed for another owner": (409, "OWNER_AUTHORITY"),
        "release signed for another owner": (409, "OWNER_AUTHORITY"),
        "lease asked by a miner": (403, "auditor not allowlisted"),
        "execution of a leased job asked by a miner": (
            403,
            "audit request lacks owned live lease",
        ),
        "observer response by a miner": (403, "independent pinned observer required"),
        "relay receipt by a miner": (409, "forged/mismatched durability receipt"),
    }
    assert changed(before, after) == set()


# SG-02 ------------------------------------------------------------------------------------

DOMAIN_LITERAL = re.compile(r"^hypertrain/(?:[a-z-]+/)*(?:[0-9]+|v[0-9]+)[|\n]")
SIGNING_DOMAINS = frozenset(
    {
        "hypertrain/1|",
        "hypertrain/2|",
        "hypertrain/checkpoint/2|",
        "hypertrain/dispute/2|",
        "hypertrain/elastic-leave/1|",
        "hypertrain/join/2|",
        "hypertrain/origin/2|",
        "hypertrain/relay/1|",
        "hypertrain/release-manifest/v1\n",
        "hypertrain/rollback/anchor/1|",
        "hypertrain/rollback/context/1|",
        "hypertrain/rollback/replay/1|",
        "hypertrain/server-key/1|",
        "hypertrain/rotate/2|",
        "hypertrain/tape/2|",
    }
)


def signing_domains() -> dict[str, set[str]]:
    """Every string or bytes literal (or f-string head) below src/ that opens a signing domain."""
    found: dict[str, set[str]] = {}
    for path in sorted(SOURCE.rglob("*.py")):
        for node in ast.walk(ast.parse(path.read_text())):
            text = None
            if isinstance(node, ast.Constant) and isinstance(node.value, str | bytes):
                text = node.value if isinstance(node.value, str) else node.value.decode("latin-1")
            elif isinstance(node, ast.JoinedStr) and node.values:
                head = node.values[0]
                text = head.value if isinstance(head, ast.Constant) else None
            if isinstance(text, str) and (match := DOMAIN_LITERAL.match(text)):
                found.setdefault(match.group(0), set()).add(path.name)
    return found


def test_no_signing_domain_is_a_byte_prefix_of_another() -> None:
    """SG-02. Each domain ends in its separator, so no domain can open another one's messages."""
    domains = sorted(signing_domains())
    assert set(domains) == SIGNING_DOMAINS
    assert [(a, b) for a in domains for b in domains if a != b and b.startswith(a)] == []
    # The v1 family `hypertrain/1|<Type>|` stays apart from the named v2 domains only while no
    # v1 message type is called like the first component of one of them.
    assert {"Join", "Rerun", "Checkpoint", "EventTape"} & set(MESSAGE_TYPES) == set()


def test_every_signature_preimage_opens_with_exactly_its_own_domain() -> None:
    """SG-02. A signature made for one purpose covers bytes no other purpose accepts."""
    from hypertrain.ledger.escrow_v2 import authority_message
    from hypertrain.miner.admission import sign_join
    from hypertrain.protocol.messages_v2 import HardwareHint, OriginAllocation, RotateRequest
    from hypertrain.protocol.messages_v2 import TestGenesis as Genesis

    run = "ab" * 32
    body = {"w": 0, "commit_hash": HASH, "received_round": 1}
    hot, cold = Keypair(b"\x41" * 32), Keypair(b"\x42" * 32)
    request = sign_join(
        hot,
        cold,
        run_id=run,
        request_id=HASH,
        expires_beacon=10,
        policy_hash=HASH,
        hardware_hint=HardwareHint(device_name="cpu", device_count=1, driver="advisory"),
        env=honest_env(),
    )
    genesis = Genesis(
        run_id=run,
        allocation_hash=HASH,
        total_units=1,
        origins=[OriginAllocation(origin_id=HASH, owner=cold.ss58, units=1, mature_at=1)],
        authority_sig="0" * 128,
    )
    grant = server_key_grant(hot, cold).grant
    rotation = RotateRequest(
        hotkey=hot.ss58, new_hotkey=cold.ss58, operation_id=HASH, new_hot_sig="0" * 128
    )
    preimages = {
        "hypertrain/2|": envelope_v2.signing_message("Receipt", body, 9, run),
        "hypertrain/relay/1|": relay_envelope.signing_message("Receipt", body, 9, run),
        "hypertrain/join/2|": envelope_v2.join_signing_message(request),
        "hypertrain/tape/2|": envelope_v2.tape_signing_message(body, run),
        "hypertrain/server-key/1|": server_key_message("grant", grant),
        "hypertrain/origin/2|": authority_message(genesis),
        "hypertrain/rotate/2|": envelope_v2.rotate_signing_message(rotation, cold.ss58, run, 9),
    }
    opened_by = {
        own: sorted(d.encode() for d in SIGNING_DOMAINS if message.startswith(d.encode()))
        for own, message in preimages.items()
    }
    assert opened_by == {own: [own.encode()] for own in preimages}


# SG-03 ------------------------------------------------------------------------------------


INTAKE_MODULES = {"envelope": "v1", "envelope_v2": "v2", "relay_envelope": "relay"}


def intake_kind(tree: ast.AST, site: ast.Call) -> str:
    """Which `Intake` a call builds: v1, v2 or relay, from how its module reaches the name."""
    func = site.func
    if isinstance(func, ast.Attribute) and isinstance(func.value, ast.Name):
        return INTAKE_MODULES.get(func.value.id, "unknown")
    for node in ast.walk(tree):
        if isinstance(node, ast.ImportFrom) and any(
            alias.name == "Intake" and alias.asname is None for alias in node.names
        ):
            return INTAKE_MODULES.get((node.module or "").rsplit(".", 1)[-1], "unknown")
    return "unknown"


def intake_sites() -> list[tuple[str, int, str, ast.Call]]:
    """Every call that builds an `Intake` below src/: file, line, which Intake, the call."""
    sites = []
    for path in sorted(SOURCE.rglob("*.py")):
        tree = ast.parse(path.read_text())
        for node in ast.walk(tree):
            if not isinstance(node, ast.Call):
                continue
            func = node.func
            name = func.attr if isinstance(func, ast.Attribute) else getattr(func, "id", "")
            if name == "Intake":
                sites.append(
                    (str(path.relative_to(SOURCE)), node.lineno, intake_kind(tree, node), node)
                )
    return sites


def unpinned_sites(
    sites: list[tuple[str, int, str, ast.Call]], known: dict[str, set[str]]
) -> list[str]:
    """Call sites that accept any signer, or name a message type their Intake cannot read."""
    problems = []
    for file, line, kind, site in sites:
        signers = site.args[1] if len(site.args) > 1 else None
        for keyword in site.keywords:
            if keyword.arg == "allowed_signers":
                signers = keyword.value
        where = f"{file}:{line}"
        if kind not in known:
            problems.append(f"{where}: cannot tell which Intake this is")
        elif signers is None or (isinstance(signers, ast.Dict) and not signers.keys):
            problems.append(f"{where}: no signer predicate")
        elif isinstance(signers, ast.Dict):
            for key in signers.keys:
                # A spread or a variable key names its type at run time: only literals can be read.
                if isinstance(key, ast.Constant) and key.value not in known[kind]:
                    problems.append(f"{where}: predicate for a message type this Intake lacks")
    return problems


def test_every_production_intake_names_a_signer_predicate_for_a_real_message_type() -> None:
    """SG-03 (condition satisfied). The default of an `Intake` is to accept any valid signer, so
    each call site must pass a predicate, and under a type its own message table has, or the
    predicate never fires."""
    known = {
        "v1": set(MESSAGE_TYPES),
        "v2": set(MESSAGE_TYPES_V2),
        "relay": set(RELAY_MESSAGE_TYPES),
    }
    sites = intake_sites()
    assert len(sites) >= 50  # the scan finds the sites: 66 on this tree
    assert [p for p in unpinned_sites(sites, known) if not deliberate(p)] == []


# An Intake without a predicate on purpose: the object upload (put_object_v2) names no role. The
# signer is the only identity in the request, and the admission lookup that runs before the Intake
# (`_owner_v2`) binds it to a miner of the run; the Intake carries the signature, the run and the
# expiry (miner-driver-resilience fad3023c, which moved the upload into a signed body, D52 I1).
DELIBERATE = {("challenge/app.py", "put_object_v2")}


def deliberate(problem: str) -> bool:
    """Whether a flagged site is inside one of the functions that DELIBERATE names."""
    file, line, _ = problem.split(":", 2)
    for node in ast.walk(ast.parse((SOURCE / file).read_text())):
        if (
            isinstance(node, ast.AsyncFunctionDef | ast.FunctionDef)
            and (file, node.name) in DELIBERATE
        ):
            if node.lineno <= int(line) <= (node.end_lineno or node.lineno):
                return True
    return False


def test_the_site_scan_flags_an_intake_that_would_accept_any_signer() -> None:
    """SG-03 (condition broken in a stand-in source): the guard above refuses it."""
    source = (
        "from hypertrain.protocol.envelope_v2 import Intake\n"
        "Intake(run)\n"
        "Intake(run, {})\n"
        "Intake(run, {'Finalise': check})\n"
        "Intake(run, {'Finalize': check})\n"
        "Intake(run, {'AuditChallenge': check})\n"
        "Intake(run, allowed_signers={'Receipt': check})\n"
        "Other(run)\n"
    )
    tree = ast.parse(source)
    sites = [
        ("stand_in.py", node.lineno, intake_kind(tree, node), node)
        for node in ast.walk(tree)
        if isinstance(node, ast.Call) and getattr(node.func, "id", "") == "Intake"
    ]
    known = {"v2": {"Finalize", "Receipt"}, "v1": {"AuditChallenge"}}
    assert sorted(unpinned_sites(sites, known)) == [
        "stand_in.py:2: no signer predicate",
        "stand_in.py:3: no signer predicate",
        "stand_in.py:4: predicate for a message type this Intake lacks",
        "stand_in.py:6: predicate for a message type this Intake lacks",
    ]


def test_an_intake_refuses_a_valid_signature_from_a_signer_its_predicate_rejects() -> None:
    """SG-03. The predicate runs after the signature: a refusal reserves nothing."""
    run, now = "ab" * 32, 5
    owner, other = Keypair(b"\x43" * 32), Keypair(b"\x44" * 32)
    ask = Receipt(w=0, commit_hash=HASH, received_round=1)
    envelope = envelope_v2.seal(other, "Receipt", run, ask, 9)
    intake = envelope_v2.Intake(run, {"Receipt": owner.ss58.__eq__})
    with pytest.raises(UnauthorizedSigner):
        intake.accept(envelope, now)
    assert intake.reservations == {}
    accepted = envelope_v2.Intake(run, {"Receipt": other.ss58.__eq__}).accept(envelope, now)
    assert accepted == ask
    forged = {**envelope, "sig": ("0" if envelope["sig"][0] != "0" else "1") + envelope["sig"][1:]}
    with pytest.raises(SignatureError):
        envelope_v2.Intake(run, {"Receipt": other.ss58.__eq__}).accept(forged, now)


# SG-15 ------------------------------------------------------------------------------------


def test_a_server_key_grant_needs_the_coldkey_and_the_key_holder_and_a_revocation_is_permanent(
    tmp_path: Path,
) -> None:
    """SG-15 (condition satisfied: the first claim of a key is the only one)."""
    service = sibling_service()
    server, cold, stranger = service.HOT[1], service.COLD[1], service.COLD[2]
    with service_world(tmp_path) as world:
        client = world.client

        def post(path: str, model: Any) -> httpx.Response:
            return client.post(path, json=model.model_dump(mode="json"))

        def record(response: httpx.Response) -> tuple[int, Any]:
            return response.status_code, response.json()

        good = server_key_grant(server, cold)
        message = server_key_message("grant", good.grant)
        before = durable_state(world)
        refused = {
            "coldkey signature is a stranger's": detail(
                post(
                    "/v2/server-keys",
                    good.model_copy(update={"cold_sig": stranger.sign(message).hex()}),
                )
            ),
            "key holder signature is a stranger's": detail(
                post(
                    "/v2/server-keys",
                    good.model_copy(update={"server_sig": stranger.sign(message).hex()}),
                )
            ),
            "server key equals the coldkey": detail(
                post("/v2/server-keys", server_key_grant(cold, cold))
            ),
        }
        assert refused == {
            "coldkey signature is a stranger's": (403, "coldkey grant signature mismatch"),
            "key holder signature is a stranger's": (
                403,
                "server key possession signature mismatch",
            ),
            "server key equals the coldkey": (422, "the server key must differ from the coldkey"),
        }
        assert changed(before, durable_state(world)) == set()

        status, granted = record(post("/v2/server-keys", good))
        assert (status, granted["server_pub"], granted["coldkey"], granted["revoked"]) == (
            201,
            server.ss58,
            cold.ss58,
            False,
        )
        # The first claim wins: the same grant again is the same record, any other claim is 409.
        assert record(post("/v2/server-keys", good)) == (201, granted)
        rival = server_key_grant(server, stranger, key_id="cd" * 16)
        other_name = server_key_grant(server, cold, name="vast-2")
        assert post("/v2/server-keys", rival).status_code == 409
        assert post("/v2/server-keys", other_name).status_code == 409

        # Only the granting coldkey revokes, with its own signature.
        forged = server_key_revoke(cold).model_copy(
            update={"cold_sig": stranger.sign(b"anything").hex()}
        )
        assert detail(post("/v2/server-keys/revoke", server_key_revoke(stranger))) == (
            403,
            "only the granting coldkey may revoke",
        )
        assert detail(post("/v2/server-keys/revoke", forged)) == (
            403,
            "coldkey revoke signature mismatch",
        )
        listing = client.get("/v2/server-keys", params={"coldkey": cold.ss58}).json()
        assert [k["revoked"] for k in listing] == [False]

        # Revocation is permanent: later revokes and grants leave the first revocation as it is.
        status, revoked = record(post("/v2/server-keys/revoke", server_key_revoke(cold)))
        assert (status, revoked["revoked"], revoked["revoked_at"]) == (200, True, 2)
        later = server_key_revoke(cold, revoked_at=99)
        assert record(post("/v2/server-keys/revoke", later)) == (200, revoked)
        assert record(post("/v2/server-keys", good)) == (201, revoked)
        assert (
            post("/v2/server-keys", server_key_grant(server, cold, key_id="ef" * 16)).status_code
            == 409
        )
        listing = client.get("/v2/server-keys", params={"coldkey": cold.ss58}).json()
        assert [(k["key_id"], k["revoked"]) for k in listing] == [("ab" * 16, True)]


def test_nobody_but_the_key_holder_can_censor_a_hotkey_through_a_server_key_grant(
    tmp_path: Path,
) -> None:
    """SG-15 (condition broken: a stranger who does not hold the key tries to claim it)."""
    service = sibling_service()
    victim, stranger = service.HOT[2], service.COLD[2]
    with service_world(tmp_path) as world:
        client = world.client
        watch = gated_v2_calls(world, victim, service.COLD[3])[WATCH_KEY]
        assert detail(watch()) == (200, [])  # control: registered, no grant, no refusal
        # The stranger signs the grant of the victim's hotkey with its own keys only.
        grant = server_key_grant(stranger, stranger).grant.model_copy(
            update={"server_pub": victim.ss58}
        )
        message = server_key_message("grant", grant)
        claim = SignedGrant(
            grant=grant,
            cold_sig=stranger.sign(message).hex(),
            server_sig=stranger.sign(message).hex(),
        )
        before = durable_state(world)
        taken = client.post("/v2/server-keys", json=claim.model_dump(mode="json"))
        revoke = client.post(
            "/v2/server-keys/revoke",
            json=server_key_revoke(stranger, grant.key_id).model_dump(mode="json"),
        )
        assert detail(taken) == (403, "server key possession signature mismatch")
        assert detail(revoke) == (404, "unknown server key")
        assert changed(before, durable_state(world)) == set()
        assert detail(watch()) == (200, [])


# GT-06 / GT-07 ----------------------------------------------------------------------------


def test_a_pure_read_get_writes_nothing_durable(tmp_path: Path) -> None:
    """GT-06 (T-GT-07). The listed reads leave no row, no file and no server key behind.

    Seven of them are served here. The other five answer 404 or 409 in this fixture (it holds
    no v1 run and its public snapshot is empty), so only their refusal is covered; `/public` is
    read-only by construction (`mode=ro`)."""
    service = sibling_service()
    with service_world(tmp_path) as world:
        run = world.run_id
        stored = world.store.for_run(run).objects.put(b"a public object")
        assert world.network.join(0).status_code == 200
        client = quiet_client(world.app)
        served = [
            "/version",
            "/v1/runs",
            "/v1/beacon/latest",
            f"/v2/runs/{run}",
            f"/v2/runs/{run}/objects/{stored}",
            f"/v2/server-keys?coldkey={service.COLD[0].ss58}",
            "/public/v1/runs",
        ]
        unserved = [
            f"/v1/runs/{run}/auditor-stats",
            f"/run/{run}/auditor-stats",
            "/public/v1/snapshot",
            f"/public/v1/runs/{run}",
            f"/public/v1/runs/{run}/model",
        ]
        before = durable_state(world)
        first = {url: client.get(url).status_code for url in (*served, *unserved)}
        again = {url: client.get(url).status_code for url in (*served, *unserved)}
        after = durable_state(world)
    assert {url: first[url] for url in served} == dict.fromkeys(served, 200)
    assert max(first.values()) < 500 and again == first
    assert changed(before, after) == set()


def test_health_and_admission_status_write_only_idempotent_infrastructure_rows(
    tmp_path: Path,
) -> None:
    """GT-07 (T-GT-07, condition satisfied). Repeating either read in a round changes nothing,
    and a read touches at most the health marker or the beacon pin table, no other table. A fix
    that makes either read write less still holds the invariant, so the sets are upper bounds."""
    service = sibling_service()
    with service_world(tmp_path) as world:
        run, n = world.run_id, world.network
        assert n.join(0).status_code == 200
        client = quiet_client(world.app)
        status = f"/v2/runs/{run}/admission/{service.HOT[0].ss58}"

        def read(url: str, allowed: set[str]) -> int:
            before = durable_state(world)
            code = client.get(url).status_code
            written = changed(before, durable_state(world))
            assert written <= allowed, f"{url} wrote {sorted(written - allowed)}"
            return code

        assert read("/health", {"meta"}) == 200
        assert read("/health", set()) == 200
        assert read(status, {"beacon_pins_v2"}) == 200
        assert read(status, set()) == 200
        # An admission nobody holds is refused and leaves nothing, not even the pin.
        assert read(f"/v2/runs/{run}/admission/{service.HOT[3].ss58}", set()) == 409


def test_admission_status_in_a_later_round_writes_at_most_its_pin_row(tmp_path: Path) -> None:
    """GT-07 (condition: one pin row per action per round, never admission state)."""
    service = sibling_service()
    with service_world(tmp_path) as world:
        run, n = world.run_id, world.network
        assert n.join(0).status_code == 200
        client = quiet_client(world.app)
        status = f"/v2/runs/{run}/admission/{service.HOT[0].ss58}"
        record = client.get(status).json()
        n.push(2)
        before = durable_state(world)
        later = client.get(status)
        touched = changed(before, durable_state(world))
    assert later.status_code == 200
    assert touched <= {"beacon_pins_v2"}, sorted(touched - {"beacon_pins_v2"})
    assert later.json()["record"] == record["record"]
