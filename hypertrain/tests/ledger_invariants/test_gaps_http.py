"""Gaps of the http group: what the first wave left open in T-AU, T-SG, T-GT, T-DS, T-LV (D39).

Each test states a defence the service must hold ("closes the route with 503 and never answers
500", "refuses the credential and drains nothing", "reads only its own configuration"). A hole is
a strict xfail whose body asserts the defence after a control that proves the scenario reaches
the point; its reason starts with the finding id and says [candidate] only when the failure was
not reproduced. A scenario step is checked with `expect`, which raises ScenarioError and not an
AssertionError, so a broken scenario fails loudly instead of hiding behind the expected failure.
"""

from __future__ import annotations

import asyncio
import inspect
import threading
from importlib import import_module
from pathlib import Path
from typing import Any

import httpx
import pytest

from hypertrain.aggregator.core import FinalityError, TapeError
from hypertrain.challenge.app import Config
from hypertrain.challenge.disputes_v2 import EventAck, signed_message
from hypertrain.challenge.store import ChallengeError
from hypertrain.ledger import FULL_SHARE_MASS
from hypertrain.protocol import relay_envelope
from hypertrain.protocol.messages import Finalize
from hypertrain.protocol.relay_messages import RelayAssignment, RelayNetworkManifest, RelayReceipt
from hypertrain.relay.app import create_app as create_relay_app
from hypertrain.relay.core import RelayError

from . import support_exit_disputes as X
from .support import SLUG, Real, load_fixture_module, service_world
from .support_gaps_http import (
    BEARER_FILES,
    PATIENCE,
    POLLUTION,
    REFUSALS,
    SCRUB,
    START,
    STRONG,
    UNDECODABLE,
    PollerGate,
    bearer_headers,
    bearer_keys,
    dispute_write,
    env_names_read,
    envelope_routes,
    expect,
    in_thread,
    reaches_the_gate,
    relabeled,
    scrub,
)
from .support_http_auth import (
    HASH,
    INT,
    OP,
    WK,
    call,
    changed,
    detail,
    durable_state,
    open_round,
    quiet_client,
    seed_relay_scenario,
    with_slug,
)

RELAY_FIXTURE = ("li_relay_fixture", "relay/test_transport_v2.py")
DRAIN_CREDENTIAL = "drain-credential-" + "x" * 16
OTHER = "22" * 32


@pytest.fixture(autouse=True)
def scrubbed_environment(monkeypatch: pytest.MonkeyPatch) -> None:
    """No ambient CHALLENGE_* or HYPERTRAIN_* variable reaches a test of this module."""
    scrub(monkeypatch)


# --- R-03: a token file that is not text closes its routes, it does not crash them ------------


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason=(
        "HTP-R03 (catalogue R-03): _token catches OSError only, so a bearer file whose bytes are "
        "not UTF-8 raises UnicodeDecodeError out of _require and every route of that class "
        "answers 500 where an unreadable or empty file answers 503; the file was damaged by the "
        "test, the service reproduces the 500"
    ),
)
@pytest.mark.parametrize("principal", [OP, WK, INT])
def test_a_bearer_file_that_is_not_text_closes_its_routes_with_503_and_never_500(
    tmp_path: Path, principal: str
) -> None:
    keys = bearer_keys(principal)
    path = tmp_path / BEARER_FILES[principal]
    with service_world(tmp_path) as world:
        client = quiet_client(world.app)
        own = with_slug(bearer_headers(world, principal))
        callers = {"its own bearer": own, "no credential": with_slug({})}
        original = path.read_text()

        def answers() -> dict[str, dict[tuple[str, str], int]]:
            return {
                name: {key: call(client, key, headers).status_code for key in keys}
                for name, headers in callers.items()
            }

        # Control: the intact file opens every route of the class to its bearer, and an empty
        # file closes them all with 503, which is the answer an unusable secret must give.
        opened = answers()["its own bearer"]
        expect(
            {code for code in opened.values() if code in (401, 503)} == set(),
            f"the own bearer does not pass the gate of every route: {opened}",
        )
        path.write_text("")
        closed = answers()
        expect(
            all(code == 503 for seen in closed.values() for code in seen.values()),
            "an empty bearer file does not close the routes with 503",
        )
        before = world.db_digest()
        path.write_bytes(UNDECODABLE)
        damaged = answers()
        after = world.db_digest()
        health = client.get("/health").status_code
        path.write_text(original)
        reopened = answers()["its own bearer"]
        assert {name: set(seen.values()) for name, seen in damaged.items()} == {
            name: {503} for name in callers
        }
        if principal == INT:
            assert health == 503, "the health probe reads the same file and must not crash"
        assert after == before
        assert {code for code in reopened.values() if code in (401, 503)} == set()


# --- R-17 / R-20: the relay drain route -------------------------------------------------------


def relay_wire(app: Any) -> httpx.AsyncClient:
    """A client on the relay application that answers a server error instead of raising it."""
    return httpx.AsyncClient(
        transport=httpx.ASGITransport(app=app, raise_app_exceptions=False),
        base_url="https://relay.test",
    )


def wrong_drain_credentials(credential: str) -> dict[str, dict[str, str]]:
    swapped = credential[:-1] + ("y" if credential[-1] != "y" else "z")
    return {
        "no credential": {},
        "empty bearer": {"Authorization": "Bearer "},
        "same length, last byte changed": {"Authorization": f"Bearer {swapped}"},
        "one byte appended": {"Authorization": f"Bearer {credential}x"},
        "last byte missing": {"Authorization": f"Bearer {credential[:-1]}"},
        "another scheme": {"Authorization": f"Basic {credential}"},
    }


def test_the_relay_drain_route_refuses_wrong_credentials_and_drains_on_the_exact_one(
    tmp_path: Path,
) -> None:
    """R-17 / R-20: a wrong or short credential drains nothing, the exact one drains the relay and
    a drained relay takes no new grant. The route also accepts the credential without its
    'Bearer ' prefix (relay/app.py removeprefix, critic W-04); that is not pinned here."""
    fixture = load_fixture_module(*RELAY_FIXTURE)

    async def scenario() -> None:
        async with httpx.AsyncClient() as backing:
            rig = fixture.Rig(tmp_path, backing)
            relay = rig.relay
            async with relay_wire(create_relay_app(relay, DRAIN_CREDENTIAL)) as wire:
                refused = {}
                for name, headers in wrong_drain_credentials(DRAIN_CREDENTIAL).items():
                    response = await wire.post("/internal/drain", headers=headers)
                    refused[name] = (response.status_code, response.json(), relay.draining)
                assert refused == {
                    name: (403, {"detail": "drain denied"}, False)
                    for name in wrong_drain_credentials(DRAIN_CREDENTIAL)
                }
                # Control: nothing above drained the relay, so a first upload is still accepted.
                _, manifest, raw, assignment, opened = rig.upload(b"abc", nonce="01" * 32, w=1)
                await relay.accept(raw, manifest, assignment, opened)

                drained = await wire.post(
                    "/internal/drain", headers={"Authorization": f"Bearer {DRAIN_CREDENTIAL}"}
                )
                assert (drained.status_code, drained.json(), relay.draining) == (
                    200,
                    {"draining": True},
                    True,
                )
                # A drained relay takes no new grant.
                _, manifest, raw, assignment, opened = rig.upload(b"xyz", nonce="02" * 32, w=2)
                with pytest.raises(RelayError, match="relay draining; new grants disabled"):
                    await relay.accept(raw, manifest, assignment, opened)

    asyncio.run(scenario())


@pytest.mark.parametrize("length", [0, 1, 16, 31])
def test_a_drain_credential_under_32_characters_never_builds_the_relay_app(
    tmp_path: Path, length: int
) -> None:
    fixture = load_fixture_module(*RELAY_FIXTURE)

    async def scenario() -> None:
        async with httpx.AsyncClient() as backing:
            rig = fixture.Rig(tmp_path, backing)
            with pytest.raises(RelayError, match="drain credential must contain >=32 characters"):
                create_relay_app(rig.relay, "x" * length)
            # Control: 32 characters is enough, so the refusal above is about the length.
            assert create_relay_app(rig.relay, "x" * 32) is not None

    asyncio.run(scenario())


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason=(
        "HTP-R17 (catalogue R-17): the drain route compares the Authorization header with "
        "hmac.compare_digest on two str, which raises TypeError for a non-ASCII character, so a "
        "header with such a byte answers 500 where every other wrong credential answers 403; "
        "reproduced with a latin-1 byte"
    ),
)
def test_a_drain_credential_with_a_non_ascii_byte_is_a_403_not_a_crash(
    tmp_path: Path,
) -> None:
    fixture = load_fixture_module(*RELAY_FIXTURE)

    async def scenario() -> None:
        async with httpx.AsyncClient() as backing:
            rig = fixture.Rig(tmp_path, backing)
            async with relay_wire(create_relay_app(rig.relay, DRAIN_CREDENTIAL)) as wire:
                ascii_only = await wire.post(
                    "/internal/drain", headers={"Authorization": "Bearer wrong"}
                )
                expect(ascii_only.status_code == 403, "an ASCII wrong credential is not a 403")
                non_ascii: dict[str, Any] = {"Authorization": b"Bearer caf\xe9"}
                refused = await wire.post("/internal/drain", headers=non_ascii)
                assert (refused.status_code, rig.relay.draining) == (403, False)

    asyncio.run(scenario())


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason=(
        "HTP-R17 (catalogue R-17): upload_capability decodes X-Upload-Grant with "
        "base64.b64decode and maps only binascii.Error, so a header with a non-ASCII character "
        "raises ValueError and the chunk route answers 500 where a malformed ASCII capability "
        "answers 400; reproduced with a latin-1 byte"
    ),
)
def test_an_upload_capability_with_a_non_ascii_byte_is_a_400_not_a_crash(
    tmp_path: Path,
) -> None:
    fixture = load_fixture_module(*RELAY_FIXTURE)

    async def scenario() -> None:
        async with httpx.AsyncClient() as backing:
            rig = fixture.Rig(tmp_path, backing)
            grant, manifest, raw, assignment, opened = rig.upload(b"abc")
            await rig.relay.accept(raw, manifest, assignment, opened)
            url = f"/v1/uploads/{grant.digest()}/chunks/0"
            async with relay_wire(create_relay_app(rig.relay, DRAIN_CREDENTIAL)) as wire:
                ascii_only = await wire.put(url, content=b"abc", headers={"X-Upload-Grant": "!!"})
                expect(ascii_only.status_code == 400, "an ASCII malformed capability is not a 400")
                non_ascii: dict[str, Any] = {"X-Upload-Grant": b"caf\xe9"}
                refused = await wire.put(url, content=b"abc", headers=non_ascii)
                assert refused.status_code == 400

    asyncio.run(scenario())


# --- M-17: the lot runs on a scrubbed environment, and the scrub list is the service's ---------


def test_the_scrub_list_covers_every_name_config_from_env_reads() -> None:
    read = env_names_read(inspect.getsource(Config.from_env))
    expect(len(read) >= 10, f"the scan finds the knobs of Config.from_env: {sorted(read)}")
    assert read - set(SCRUB) == set()


def test_the_environment_scan_finds_every_spelling_of_an_environment_read() -> None:
    source = (
        "def from_env():\n"
        "    def path(name):\n"
        "        return os.environ.get(name)\n"
        "    env = os.environ.get\n"
        "    a = env('ONE', 'x')\n"
        "    b = path('TWO')\n"
        "    c = os.environ.get('THREE')\n"
        "    d = os.getenv('FOUR')\n"
        "    e = os.environ['FIVE']\n"
        "    f = os.environ.get('SIX' + suffix)\n"
        "    g = other('NOT_AN_ENV_READ')\n"
    )
    assert env_names_read(source) == {"ONE", "TWO", "THREE", "FOUR", "FIVE", "<computed>"}


def test_every_name_of_the_scrub_list_is_a_setting_of_config_and_the_scrub_restores_it(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    baseline = Config.from_env()
    expect(baseline.state_dir == Path("/data"), "the scrubbed environment gives the defaults")
    expect(set(POLLUTION) == set(SCRUB), "a polluting value is missing for a name of the list")
    inert = []
    for name in SCRUB:
        with monkeypatch.context() as scoped:
            scoped.setenv(name, POLLUTION[name].format(polluted=tmp_path))
            if Config.from_env() == baseline:
                inert.append(name)
    assert inert == [], "a name of the scrub list that Config.from_env does not read"
    # The scrub itself: with every name set, one call leaves none of them in the environment.
    with monkeypatch.context() as scoped:
        for name in SCRUB:
            scoped.setenv(name, POLLUTION[name].format(polluted=tmp_path))
        expect(Config.from_env() != baseline, "the polluted environment changes the Config")
        scrub(scoped)
        assert Config.from_env() == baseline


def test_a_service_world_takes_nothing_from_a_polluted_ambient_environment(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    polluted = tmp_path / "polluted"
    polluted.mkdir()
    for name in SCRUB:
        monkeypatch.setenv(name, POLLUTION[name].format(polluted=polluted))
    with service_world(tmp_path / "world") as world:
        state = world.store.state_dir
        version = world.client.get("/version").json()
        assert state.is_relative_to(tmp_path / "world")
        assert (state / "server_keys.db").exists()
        assert version["slug"] == SLUG
        assert sorted(polluted.iterdir()) == []


# --- M-10: the base manifest hash of the relay network manifest (RTA-08) ----------------------


def test_the_master_derives_the_relay_network_manifest_from_the_training_hash(
    tmp_path: Path,
) -> None:
    with service_world(tmp_path) as world:
        hotkeys = open_round(world)
        client = quiet_client(world.app)
        # The round view derives the member assignments the relay assignment is read from.
        expect(client.get(f"/v2/runs/{world.run_id}/rounds/0").status_code == 200, "round view")
        response = client.get(f"/v2/runs/{world.run_id}/rounds/0/relay/{hotkeys[0].ss58}")
        expect(response.status_code == 200, f"relay assignment: {response.text}")
        answer = response.json()
        training_hash = world.network.manifest.training.training_hash()
    sealed = relay_envelope.parse_envelope(answer["network_manifest"])
    network = RelayNetworkManifest.model_validate(sealed.body)
    assignment = RelayAssignment.model_validate(answer["assignment"]["body"])
    assert relay_envelope.verify_envelope(answer["network_manifest"])
    assert (network.run_id, network.base_manifest_hash) == (world.run_id, training_hash)
    assert network.base_manifest_hash != world.run_id
    # The assignment the relay checks names exactly this manifest.
    assert assignment.manifest_hash == network.digest()


def test_a_relay_pinned_on_another_base_hash_refuses_the_master_grants(
    tmp_path: Path,
) -> None:
    """RTA-08: the base hash is never read on its own, but it is part of the digest every master
    assignment carries and the relay compares; a relay pinned on a manifest that differs from the
    master's in that one field takes no grant, and the matching manifest takes it."""
    fixture = load_fixture_module(*RELAY_FIXTURE)
    master_view = RelayNetworkManifest(
        run_id=fixture.RUN,
        base_manifest_hash=fixture.H,
        registry_hash=fixture.H,
        specs_hash=fixture.H,
        assignment_policy="master-observed-median3-v1",
    )
    drifted = master_view.model_copy(update={"base_manifest_hash": OTHER})
    expect(drifted.digest() != master_view.digest(), "the base hash is part of the digest")

    async def scenario() -> None:
        async with httpx.AsyncClient() as backing:
            rig = fixture.Rig(tmp_path, backing)
            _, manifest, raw, assignment, opened = rig.upload(b"abc")
            from_master = RelayAssignment.model_validate(
                {
                    **RelayAssignment.model_validate(assignment.body).body(),
                    "manifest_hash": master_view.digest(),
                }
            )
            sealed = rig.seal(from_master)
            rig.relay.network_manifest_hash = drifted.digest()
            with pytest.raises(RelayError, match="grant is not bound to this region"):
                await rig.relay.accept(raw, manifest, sealed, opened)
            assert (await rig.relay.state()).uploads == {}
            rig.relay.network_manifest_hash = master_view.digest()
            await rig.relay.accept(raw, manifest, sealed, opened)
            assert len((await rig.relay.state()).uploads) == 1

    asyncio.run(scenario())


# --- GAP-03: the vesting of a reward against the horizons it is meant to outlast ---------------


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason=(
        "GAP-03: finalize vests a reward at the finalize beacon plus E_vest_rounds, a count of "
        "3-second drand rounds (10 here, 30 seconds), while the shortest contest the dispute "
        "policy allows lasts 2 x dispute_per_level x 3 = 3600 drand rounds; the unit mechanism.md "
        "section 5 sizes the escrow in is the training round, so the stake can be paid out long "
        "before any contest could end; whether the unit is the defect is a Forge decision"
    ),
)
def test_the_vesting_of_a_reward_outlasts_the_shortest_contest_the_run_allows(
    real_reward: Real,
) -> None:
    setup, evidence, reward = real_reward
    verify = setup.manifest.training.verify
    span = reward.mature_at - evidence.finalized_beacon
    # Control: the signed reward vests exactly E_vest_rounds beacons after its finalize beacon.
    expect(span == verify.E_vest_rounds, f"vesting span {span} is not E_vest_rounds")
    shortest_contest = 2 * verify.T.dispute_per_level * 3
    assert span > shortest_contest, f"vesting lasts {span} beacons, a contest {shortest_contest}"


# --- T-SG-01: an envelope of another run is refused on the route types the table left out ------

ROUTE_NAMES = [
    "resolution",
    "observer-response",
    "upload-grant",
    "trial-finalize",
    "shadow-reservation-finalize",
    "shadow-reservation-commit",
    "rollback",
    "relay-registry",
    "bootstrap",
    "relay-failure",
]
VARIANTS = list(REFUSALS)


@pytest.mark.parametrize("variant", VARIANTS)
@pytest.mark.parametrize("route", ROUTE_NAMES)
def test_an_envelope_made_for_another_run_is_refused_on_the_remaining_routes(
    tmp_path: Path, route: str, variant: str
) -> None:
    with service_world(tmp_path) as world:
        target = envelope_routes(world)[route]
        foreign = "cd" * 32
        sent = target.payload(foreign, 10_000)
        if variant == "relabeled-to-this-run":
            sent = relabeled(sent, world.run_id)
        client = quiet_client(world.app)
        before = durable_state(world)
        refused = detail(target.post(world, sent, client))
        after = durable_state(world)
        # Control: the same body sealed for this run is not turned away as another run's. Some
        # routes go on to meet the thin state of this scenario, so a server error is no refusal.
        own = detail(target.post(world, target.payload(world.run_id, 10_000), client))
    assert refused == target.refusals[variant]
    assert changed(before, after) == set()
    expect(
        own != REFUSALS["sealed-for-another-run"], f"{route}: the own envelope is refused: {own}"
    )


def test_an_event_ack_made_for_another_run_is_refused_and_stores_no_ack(tmp_path: Path) -> None:
    with service_world(tmp_path) as world:
        hot = X.service().HOT[0]
        X.open_contest(world, 0)
        event = X.disputes(world).events(hot.ss58, 0)[-1]

        def ack(run_id: str) -> EventAck:
            made = EventAck.model_validate(
                {
                    "run_id": run_id,
                    "cursor": event.cursor,
                    "event_hash": event.digest(),
                    "party": hot.ss58,
                    "received_beacon": world.network.now,
                    "sig": "",
                }
            )
            return made.model_copy(update={"sig": hot.sign(signed_message(made)).hex()})

        url = f"/v2/runs/{world.run_id}/disputes/ack"
        before = durable_state(world)
        refused = detail(world.client.post(url, json=ack("cd" * 32).body()))
        after = durable_state(world)
        assert (refused, world.count("dispute_acks_v2")) == ((409, "ACK_AUTHORITY_OR_TIME"), 0)
        assert changed(before, after) == set()
        # Control: the same acknowledgement made for this run is stored.
        accepted = world.client.post(url, json=ack(world.run_id).body())
        assert (accepted.status_code, world.count("dispute_acks_v2")) == (200, 1)


def test_a_relay_receipt_of_another_run_is_not_applied_and_a_relabeled_one_fails(
    tmp_path: Path,
) -> None:
    with service_world(tmp_path) as world:
        relay_key = X.service().RELAY
        scenario = seed_relay_scenario(world)
        client = quiet_client(world.app)
        body = RelayReceipt(
            w=0,
            hotkey=scenario.miner.ss58,
            delta_hash=HASH,
            size=3,
            grant_hash=scenario.grant_hash,
            chunk_manifest_hash=HASH,
            durable_object_key="x",
            key_id="k1",
            received_drand=5,
            retain_until=500,
        )

        def sealed(run_id: str) -> dict[str, Any]:
            return relay_envelope.seal(relay_key, "RelayReceipt", run_id, body, 1000)

        # Control: the relay's own receipt for this run passes its signature and reaches the
        # assignment lookup, so the two refusals below are about the run and the signature.
        own = detail(client.post("/v2/relay-receipts", json=sealed(world.run_id)))
        foreign, relabeled_one = sealed("cd" * 32), {**sealed("cd" * 32), "run_id": world.run_id}
        expect(relay_envelope.verify_envelope(foreign), "the foreign receipt is correctly signed")
        before = durable_state(world)
        refused = {
            "made for another run": detail(client.post("/v2/relay-receipts", json=foreign)),
            "relabeled to this run": detail(client.post("/v2/relay-receipts", json=relabeled_one)),
        }
        after = durable_state(world)
    assert own[0] == 409 and str(own[1]).startswith("missing accepted relay-assignment")
    assert refused == {
        "made for another run": (404, "unknown run"),
        "relabeled to this run": (409, "forged/mismatched durability receipt"),
    }
    assert changed(before, after) == set()


# --- T-GT-06: the anonymous reads of a round do bounded work -----------------------------------

REPEATS = 19


def anonymous_round_urls(world: Any, hotkey: str) -> dict[str, str]:
    run = world.run_id
    return {
        "round view": f"/v2/runs/{run}/rounds/0",
        "island job": f"/v2/runs/{run}/rounds/0/job/{hotkey}",
        "relay assignment": f"/v2/runs/{run}/rounds/0/relay/{hotkey}",
    }


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason=(
        "GT-06+HTP-02: the round view, the island job and the relay assignment of a round that "
        "was opened are anonymous GETs that read their objects again on every call: 19 repeats "
        "cost 19 times the object reads of the first call, so nothing is derived once; "
        "reproduced by counting the reads of the hosted object store"
    ),
)
@pytest.mark.parametrize("read", ["round view", "island job", "relay assignment"])
def test_a_repeated_anonymous_read_of_a_round_does_not_read_its_objects_again(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, read: str
) -> None:
    with service_world(tmp_path) as world:
        hotkeys = open_round(world)
        hosted = world.store.for_run(world.run_id)
        client = quiet_client(world.app)
        urls = anonymous_round_urls(world, hotkeys[0].ss58)
        url = urls[read]
        # The round view derives the member assignments the other two read.
        expect(client.get(urls["round view"]).status_code == 200, "the round view is not served")
        reads: list[str] = []
        original = hosted.objects.get

        def counting(sha256: str, *args: Any, **kwargs: Any) -> Any:
            reads.append(sha256)
            return original(sha256, *args, **kwargs)

        monkeypatch.setattr(hosted.objects, "get", counting)
        # Control: the first call is served and costs at least one object read.
        first = client.get(url)
        once = len(reads)
        expect(first.status_code == 200 and once >= 1, f"{read}: {first.status_code}, {once} reads")
        for _ in range(REPEATS):
            expect(client.get(url).status_code == 200, f"{read}: a repeat is not served")
        repeated = len(reads) - once
        assert repeated <= once, f"{REPEATS} repeats read {repeated} objects, the first call {once}"


def lock_is_free(hosted: Any) -> bool:
    """Whether another thread can take the store lock right now."""
    outcome: list[bool] = []

    def try_lock() -> None:
        taken = hosted._lock.acquire(blocking=False)
        if taken:
            hosted._lock.release()
        outcome.append(taken)

    other = threading.Thread(target=try_lock)
    other.start()
    other.join(timeout=30)
    return bool(outcome and outcome[0])


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason=(
        "GT-06+HTP-02: the anonymous round view reads the dataset and proof objects of the round "
        "while it holds the store lock and its write transaction, so one slow object read stops "
        "every other request of the run; reproduced by trying the lock from another thread "
        "inside the object read"
    ),
)
@pytest.mark.parametrize("read", ["round view", "island job", "relay assignment"])
def test_an_anonymous_round_read_leaves_the_store_lock_free_while_it_reads(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, read: str
) -> None:
    with service_world(tmp_path) as world:
        hotkeys = open_round(world)
        hosted = world.store.for_run(world.run_id)
        client = quiet_client(world.app)
        urls = anonymous_round_urls(world, hotkeys[0].ss58)
        expect(client.get(urls["round view"]).status_code == 200, "the round view is not served")
        free_during_reads: list[bool] = []
        original = hosted.objects.get
        # Control: with no request in flight another thread takes the lock at once.
        expect(lock_is_free(hosted), "the store lock is not free while the store is idle")

        def probing(sha256: str, *args: Any, **kwargs: Any) -> Any:
            free_during_reads.append(lock_is_free(hosted))
            return original(sha256, *args, **kwargs)

        monkeypatch.setattr(hosted.objects, "get", probing)
        served = client.get(urls[read])
        expect(served.status_code == 200 and free_during_reads, f"{read}: {served.status_code}")
        assert all(free_during_reads), "the store lock is held while the objects are read"


# --- T-GT-07: the pure reads write nothing on their success path either -----------------------


def seed_v1_run(world: Any) -> str:
    """A v1 run in the store's database, written the way tests/public_api seeds its fixtures.

    It makes the five reads the first wave could only see refuse (auditor stats, the public
    snapshot, the public run and its model) answer 200, so a writer hidden in their success path
    has somewhere to run. The store is idle: the seeding connection is the only one writing."""
    seed = import_module("public_api.test_public_api").seed
    with world.store._lock:
        world.store._db.commit()
    run_id: str = seed(world.store.state_dir / "challenge.db")
    return run_id


def test_every_pure_read_get_writes_nothing_durable_on_its_success_path_too(tmp_path: Path) -> None:
    service_keys = X.service()
    with service_world(tmp_path) as world:
        v2_run = world.run_id
        stored = world.store.for_run(v2_run).objects.put(b"a public object")
        expect(world.network.join(0).status_code == 200, "the miner joined")
        v1_run = seed_v1_run(world)
        client = quiet_client(world.app)
        urls = [
            "/version",
            "/v1/runs",
            "/v1/beacon/latest",
            f"/v1/runs/{v1_run}/auditor-stats",
            f"/run/{v1_run}/auditor-stats",
            f"/v2/runs/{v2_run}",
            f"/v2/runs/{v2_run}/objects/{stored}",
            f"/v2/server-keys?coldkey={service_keys.COLD[0].ss58}",
            "/public/v1/runs",
            "/public/v1/snapshot",
            f"/public/v1/runs/{v1_run}",
            f"/public/v1/runs/{v1_run}/model",
        ]
        before = durable_state(world)
        first = {url: client.get(url).status_code for url in urls}
        expect(set(first.values()) == {200}, f"a read is not served: {first}")
        for _ in range(2):
            again = {url: client.get(url).status_code for url in urls}
            assert again == first
        assert changed(before, durable_state(world)) == set()


# --- T-DS-01: the other writers of a dispute never wait for a long-poll ------------------------


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason=(
        "DS-04+DIA-22: events() holds the dispute condition and asks for the store lock while a "
        "writer holds the store lock and notify()s inside its transaction; bisect, resolution, "
        "state request and timeout all call notify() there, as open and the beacon exit do, so "
        "the poller and the writer wait for each other"
    ),
)
@pytest.mark.parametrize("write", ["bisect", "resolution", "state-request", "timeout"])
def test_a_dispute_poll_never_waits_for_the_other_writers_of_a_dispute(
    tmp_path: Path, write: str
) -> None:
    with service_world(tmp_path) as world:
        store, hot = world.store, X.service().HOT[0]
        dispute_id, target = dispute_write(world, write)
        d = X.disputes(world)
        attempted, inside = threading.Event(), threading.Event()
        d.escrow.mutex = PollerGate(store._lock, attempted)  # type: ignore[assignment]
        real = getattr(d, target.method)

        def once_the_poll_is_waiting(*args: Any, **kwargs: Any) -> Any:
            inside.set()  # the store lock is held by the writer from here to its commit
            expect(attempted.wait(START), "the poller never asked for the store lock")
            return real(*args, **kwargs)

        setattr(d, target.method, once_the_poll_is_waiting)
        writer, written = in_thread("writer", target.act)
        expect(inside.wait(START), "the write never reached the dispute engine")
        poller, polled = in_thread("poller", lambda: d.events(hot.ss58, 0, timeout=5))
        writer.join(3 * PATIENCE)
        poller.join(3 * PATIENCE)
        assert not writer.is_alive() and not poller.is_alive()
        assert "error" not in polled, polled.get("error")
        assert "error" not in written, written.get("error")
        assert dispute_id in {event.turn.dispute_id for event in polled["result"]}
        assert target.took_effect()


# --- M-02: a round whose finalize was refused pays nothing and the epoch burns ----------------

REPAIR_REFUSED = (409, "repair evidence is model-only; cannot mint miner rewards")


def chain_time(world: Any) -> int:
    """The wall clock of the service world, as unix seconds."""
    return int(world.network.manifest.training.beacon.genesis_time + (world.network.now - 1) * 3)


def test_a_refused_finalize_mints_nothing_and_every_epoch_answers_the_defined_burn(
    tmp_path: Path,
) -> None:
    """The coordinator's Finalize of a round that was only repaired is refused before any credit
    (a repair is model history). The weights of the epochs that follow pay no unit, burn the
    whole share, and answer the same bytes again."""
    with service_world(tmp_path) as world:
        hot = X.service().HOT[0]
        X.join(world, 0)
        X.seed_open_round(world, 0, [0])
        theta = "ab" * 32
        applied = {"theta_hash": theta, "tape_hash": theta, "prev_state": theta}
        final = Finalize(
            w=0, final_theta_hash_w1=theta, included=[hot.ss58], entitlements_root=theta
        )

        def finalize() -> Any:
            return world.client.post(
                f"/v2/runs/{world.run_id}/admin/rounds/0/finalize",
                json=world.network.signed(X.service().COORD, "Finalize", final),
                headers=world.admin,
            )

        # Control: applied as an ordinary round, the same Finalize goes past the repair check and
        # is turned away for the audit evidence it lacks, so the refusal below is the repair's.
        with world.store._tx():
            world.store._put_record_v2(world.run_id, "applied", "0", applied)
        ordinary = detail(finalize())
        expect(ordinary[0] == 409 and ordinary != REPAIR_REFUSED, f"an ordinary round: {ordinary}")
        with world.store._tx():
            world.store._put_record_v2(world.run_id, "applied", "0", {**applied, "kind": "repair"})
        before = world.db_digest()
        refused = detail(finalize())
        assert (refused, world.db_digest() == before) == (REPAIR_REFUSED, True)

        first = world.get_weights(0, epoch_at=chain_time(world))
        again = world.get_weights(0, epoch_at=chain_time(world) + 9)
        answer = first.json()
        assert first.status_code == 200 and again.content == first.content
        assert answer["weights"] == {} and answer["metadata"]["reason"] == "ok"
        assert answer["metadata"]["units_burned_this_epoch"] == FULL_SHARE_MASS
        assert answer["metadata"]["units_paid"] == 0
        assert world.count("escrow_events", "kind IN ('PAY','REWARD_FINALIZE')") == 0


# --- CL-48: the aggregation route maps the refusals of the tape and finality rules -------------


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason=(
        "CL-48: the aggregate route maps ChallengeError only, while the rollback routes map "
        "TapeError to 409; a TapeError or FinalityError raised by the aggregation (an included "
        "member with an open contest, a round that may not settle) answers an unhandled 500 with "
        "no reason; reproduced by raising the error at the store boundary, because no stored "
        "state that reaches TapeError from this route was found (an open contest suspends its "
        "member and the aggregation then meets a 403 first)"
    ),
)
@pytest.mark.parametrize(
    "error",
    [TapeError("input unsettled/excluded"), FinalityError("round cannot settle")],
    ids=["tape", "finality"],
)
def test_an_aggregation_refused_by_tape_or_finality_rules_is_a_4xx_with_its_reason(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, error: Exception
) -> None:
    with service_world(tmp_path) as world:
        hosted = world.store.for_run(world.run_id)
        client = quiet_client(world.app)
        url = f"/v2/runs/{world.run_id}/admin/rounds/0/aggregate"

        def refusing(raised: Exception) -> None:
            def refuse(*args: Any, **kwargs: Any) -> None:
                raise raised

            monkeypatch.setattr(hosted, "aggregate_v2", refuse)

        # Control: an error of the kind the route maps reaches the client as its own refusal.
        refusing(ChallengeError(409, "a mapped refusal"))
        mapped = detail(client.post(url, headers=world.admin))
        expect(
            mapped == (409, "a mapped refusal"), f"the route does not map ChallengeError: {mapped}"
        )
        refusing(error)
        answer = client.post(url, headers=world.admin)
        assert 400 <= answer.status_code < 500 and str(error) in answer.text


# --- T-AU-03: a weak or shared secret is refused for every bearer class, not only two ----------


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason=(
        "AU-02+HTP-01: the worker and internal bearers accept a one-character secret like the "
        "operator bearer does; the first wave pins the operator class only"
    ),
)
@pytest.mark.parametrize("cls", ["worker", "internal"])
def test_a_secret_shorter_than_32_bytes_is_never_honoured_for_the_worker_and_internal_class(
    tmp_path: Path, cls: str
) -> None:
    # Control: 32+ byte, pairwise different secrets open the probe of the class.
    expect(reaches_the_gate(tmp_path / "strong", STRONG, STRONG[cls], cls), "a strong secret")
    weak = {**STRONG, cls: "x"}
    assert not reaches_the_gate(tmp_path / "weak", weak, "x", cls)


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason=(
        "AU-02+HTP-01: one secret serves the internal class and the operator or worker class; "
        "the first wave pins the operator and worker pair only"
    ),
)
@pytest.mark.parametrize("other", ["admin", "worker"])
def test_one_secret_never_serves_the_internal_class_and_another_class(
    tmp_path: Path, other: str
) -> None:
    expect(reaches_the_gate(tmp_path / "own", STRONG, STRONG["internal"], "internal"), "internal")
    expect(reaches_the_gate(tmp_path / "other", STRONG, STRONG[other], other), other)
    shared = {**STRONG, "internal": STRONG[other]}
    opens = [
        reaches_the_gate(tmp_path / f"shared-{cls}", shared, STRONG[other], cls)
        for cls in ("internal", other)
    ]
    assert opens.count(True) <= 1, f"the same secret opens the internal and the {other} routes"
