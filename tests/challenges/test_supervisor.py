"""Auto-updater behaviour against a fake Docker Engine API and a fake GitHub."""

import json
from pathlib import Path
from urllib.parse import unquote

import httpx
import pytest

from cortex.challenges.registry import parse_registry
from cortex.challenges.supervisor import Supervisor, SupervisorConfig, SupervisorError

IMAGE = "ghcr.io/cortexlm/bounty"
GOOD, NEXT, BROKEN = ("sha256:" + c * 64 for c in "abc")
LABELS = {
    "io.cortex.challenge.slug": "bounty",
    "io.cortex.challenge.contract": "1",
    "org.opencontainers.image.source": "https://github.com/CortexLM/bounty",
}


class FakeEngine:
    """Enough of Docker Engine v1.44 for the supervisor: images, containers, labels."""

    def __init__(self):
        self.tags = {"stable": GOOD}
        self.labels = {GOOD: LABELS, NEXT: LABELS, BROKEN: LABELS}
        self.containers: dict[str, dict] = {}
        self.created: list[tuple[str, dict]] = []
        self.fail: set[str] = set()  # "create" or "start" of the production container

    def handle(self, request: httpx.Request) -> httpx.Response:
        path = unquote(request.url.path).removeprefix("/v1.44")
        params = request.url.params
        if path == "/images/create":
            return httpx.Response(200, text=json.dumps({"status": "pulled"}) + "\n")
        if path.startswith("/images/"):
            reference = path.removeprefix("/images/").removesuffix("/json")
            digest = (
                reference.split("@", 1)[1]
                if "@" in reference
                else self.tags[reference.rsplit(":", 1)[1]]
            )
            return httpx.Response(
                200,
                json={
                    "RepoDigests": [f"{IMAGE}@{digest}"],
                    "Config": {"Labels": self.labels[digest]},
                },
            )
        if path == "/containers/json":
            return httpx.Response(
                200,
                json=[{"Id": name, "Labels": c["Labels"]} for name, c in self.containers.items()],
            )
        if path == "/containers/create":
            spec = json.loads(request.content)
            if "create" in self.fail and params["name"] == "cortex-challenge-bounty":
                return httpx.Response(500)
            self.containers[params["name"]] = {
                "Labels": spec["Labels"],
                "Running": False,
                "Spec": spec,
            }
            self.created.append((params["name"], spec))
            return httpx.Response(201, json={"Id": params["name"]})
        name = path.split("/")[2]
        if request.method == "DELETE":
            return httpx.Response(204 if self.containers.pop(name, None) else 404)
        if name not in self.containers:
            return httpx.Response(404)
        if path.endswith("/start"):
            fresh = self.containers[name]["Spec"] is self.created[-1][1]
            if "start" in self.fail and name == "cortex-challenge-bounty" and fresh:
                return httpx.Response(500)
            self.containers[name]["Running"] = True
            return httpx.Response(204)
        if path.endswith("/stop"):
            self.containers[name]["Running"] = False
            return httpx.Response(204)
        if path.endswith("/rename"):
            if params["name"] in self.containers:
                return httpx.Response(409)
            self.containers[params["name"]] = self.containers.pop(name)
            return httpx.Response(204)
        container = self.containers[name]
        return httpx.Response(
            200,
            json={
                "Config": {"Labels": container["Labels"]},
                "State": {"Running": container["Running"]},
            },
        )

    def digest(self, name: str) -> str | None:
        container = self.containers.get(name)
        return container and container["Labels"]["io.cortex.challenge.digest"]

    def env(self, name: str) -> list[str]:
        return self.containers[name]["Spec"]["Env"]


def network(engine: FakeEngine, attested: set[str]):
    def handle(request: httpx.Request) -> httpx.Response:
        if request.url.host == "api.github.com":
            digest = request.url.path.rsplit("/", 1)[1]
            return httpx.Response(200, json={"attestations": [{}] if digest in attested else []})
        container = engine.containers.get(request.url.host)
        if container is None or not container["Running"]:
            raise httpx.ConnectError("no route", request=request)
        if container["Labels"]["io.cortex.challenge.digest"] == BROKEN:
            return httpx.Response(500)
        return httpx.Response(200, json={"slug": "bounty", "contract": 1, "version": "1.0.0"})

    return handle


def registry(**overrides):
    row = {
        "id": "bounty",
        "image": IMAGE,
        "source": "https://github.com/CortexLM/bounty",
        "env": {"BOUNTY_BACKEND_PUBLIC_URL": "https://backend.invalid"},
    }
    return parse_registry({"version": 1, "challenge": [{**row, **overrides}]})["bounty"]


@pytest.fixture
def world(tmp_path):
    engine = FakeEngine()
    attested = {GOOD, NEXT, BROKEN}

    async def no_wait(_seconds):
        return None

    docker = httpx.AsyncClient(
        transport=httpx.MockTransport(engine.handle), base_url="http://docker/v1.44"
    )
    http = httpx.AsyncClient(transport=httpx.MockTransport(network(engine, attested)))
    config = SupervisorConfig(
        registry_file=tmp_path / "registry.toml",
        secrets_host_dir=Path("/srv/challenge-secrets"),
        ready_seconds=0.05,
    )
    return engine, attested, Supervisor(config, docker, http, sleep=no_wait)


async def test_rollout_runs_hardened_container_with_secrets_only_after_a_secretless_canary(world):
    engine, _, supervisor = world

    assert await supervisor.reconcile(registry()) == GOOD

    (canary_name, canary), (name, spec) = engine.created
    assert canary_name == "cortex-challenge-bounty-canary" and name == "cortex-challenge-bounty"
    assert canary["HostConfig"]["Mounts"] == [] and "/data" in canary["HostConfig"]["Tmpfs"]
    host = spec["HostConfig"]
    assert spec["Image"] == f"{IMAGE}@{GOOD}" and spec["User"] == "65532:65532"
    assert host["ReadonlyRootfs"] and host["CapDrop"] == ["ALL"]
    assert host["NetworkMode"] == "cortex-challenges" and "PortBindings" not in host
    assert {
        "Type": "bind",
        "Source": "/srv/challenge-secrets/bounty",
        "Target": "/run/secrets",
        "ReadOnly": True,
    } in host["Mounts"]
    assert "CHALLENGE_SLUG=bounty" in spec["Env"]
    assert "BOUNTY_BACKEND_PUBLIC_URL=https://backend.invalid" in spec["Env"]
    assert set(engine.containers) == {"cortex-challenge-bounty"}


async def test_channel_move_updates_and_a_failed_rollout_rolls_back_and_is_not_retried(world):
    engine, _, supervisor = world
    await supervisor.reconcile(registry())
    engine.tags["stable"] = NEXT
    assert await supervisor.reconcile(registry()) == NEXT

    engine.tags["stable"] = BROKEN
    with pytest.raises(SupervisorError, match="canary"):
        await supervisor.reconcile(registry())
    assert engine.digest("cortex-challenge-bounty") == NEXT
    created = len(engine.created)
    with pytest.raises(SupervisorError, match="refused"):
        await supervisor.reconcile(registry())
    assert len(engine.created) == created  # the refused digest is not pulled into a canary again


@pytest.mark.parametrize("fault", ["slug", "source", "contract", "attestation"])
async def test_untrusted_image_is_refused_and_the_running_digest_keeps_serving(world, fault):
    engine, attested, supervisor = world
    await supervisor.reconcile(registry())
    engine.tags["stable"] = NEXT
    if fault == "attestation":
        attested.discard(NEXT)
    else:
        key = {
            "slug": "io.cortex.challenge.slug",
            "source": "org.opencontainers.image.source",
            "contract": "io.cortex.challenge.contract",
        }[fault]
        engine.labels[NEXT] = {
            **LABELS,
            key: "https://github.com/evil/fork" if fault == "source" else "2",
        }

    with pytest.raises(SupervisorError):
        await supervisor.reconcile(registry())
    assert engine.digest("cortex-challenge-bounty") == GOOD
    assert not any(name.endswith("-canary") for name in engine.containers)


async def test_pin_freezes_the_digest_even_when_the_channel_moves(world):
    engine, _, supervisor = world
    engine.tags["stable"] = NEXT
    assert await supervisor.reconcile(registry(pin=GOOD)) == GOOD


async def test_stopped_container_is_restarted_without_a_new_rollout(world):
    engine, _, supervisor = world
    await supervisor.reconcile(registry())
    engine.containers["cortex-challenge-bounty"]["Running"] = False
    created = len(engine.created)
    await supervisor.reconcile(registry())
    assert engine.containers["cortex-challenge-bounty"]["Running"]
    assert len(engine.created) == created


async def test_unregistered_container_is_pruned_on_tick(world):
    engine, _, supervisor = world
    await supervisor.reconcile(registry())
    supervisor.config.registry_file.write_text("version = 1\n")
    await supervisor.tick(0)
    assert engine.containers == {}


@pytest.mark.parametrize("fault", ["create", "start"])
async def test_production_create_or_start_failure_restores_the_serving_container(world, fault):
    engine, _, supervisor = world
    await supervisor.reconcile(registry())
    engine.tags["stable"] = NEXT
    engine.fail.add(fault)

    with pytest.raises(SupervisorError, match="after rollout"):
        await supervisor.reconcile(registry())

    assert set(engine.containers) == {"cortex-challenge-bounty"}
    assert engine.digest("cortex-challenge-bounty") == GOOD
    assert engine.containers["cortex-challenge-bounty"]["Running"]


async def test_registry_change_with_the_same_digest_redeploys_without_a_canary(world):
    engine, _, supervisor = world
    await supervisor.reconcile(registry())
    created = len(engine.created)

    changed = registry(memory_mib=2048, env={"BOUNTY_BACKEND_PUBLIC_URL": "https://new.invalid"})
    assert await supervisor.reconcile(changed) == GOOD

    assert [name for name, _ in engine.created[created:]] == ["cortex-challenge-bounty"]
    host = engine.containers["cortex-challenge-bounty"]["Spec"]["HostConfig"]
    assert host["Memory"] == 2048 * 1024 * 1024
    assert "BOUNTY_BACKEND_PUBLIC_URL=https://new.invalid" in engine.env("cortex-challenge-bounty")
    created = len(engine.created)
    await supervisor.reconcile(changed)
    assert len(engine.created) == created  # converged: no redeploy loop


async def test_restart_mid_rollout_recovers_the_set_aside_container(world):
    engine, _, supervisor = world
    await supervisor.reconcile(registry())
    # The supervisor died after stopping and renaming the serving container.
    engine.containers["cortex-challenge-bounty-previous"] = engine.containers.pop(
        "cortex-challenge-bounty"
    )
    engine.containers["cortex-challenge-bounty-previous"]["Running"] = False
    engine.tags["stable"] = NEXT
    engine.fail.add("start")

    with pytest.raises(SupervisorError, match="after rollout"):
        await supervisor.reconcile(registry())

    assert set(engine.containers) == {"cortex-challenge-bounty"}
    assert engine.digest("cortex-challenge-bounty") == GOOD
    assert engine.containers["cortex-challenge-bounty"]["Running"]


async def test_source_edit_with_the_same_digest_is_verified_before_rollout(world):
    engine, attested, supervisor = world
    await supervisor.reconcile(registry())
    created = len(engine.created)

    moved = registry(source="https://github.com/Other/bounty", memory_mib=2048)
    with pytest.raises(SupervisorError, match="labels"):
        await supervisor.reconcile(moved)

    assert len(engine.created) == created
    assert engine.digest("cortex-challenge-bounty") == GOOD
    assert engine.containers["cortex-challenge-bounty"]["Running"]
