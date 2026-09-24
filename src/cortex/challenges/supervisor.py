"""Pull, verify, canary, run and auto-update challenge containers through the Docker API.

This is the only Cortex process with Docker control. It never reads a secret:
it passes host secret directories to Docker as read-only binds.
"""

from __future__ import annotations

import asyncio
import hashlib
import json
import logging
from collections.abc import Awaitable, Callable
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

import httpx

from .registry import RegistryEntry, container_name, load_registry

CONTRACT = "1"
MANAGED = "io.cortex.managed"
CONFIG = "io.cortex.challenge.config"
LOG = logging.getLogger("cortex.challenges")


class SupervisorError(Exception):
    """A refused image or failed rollout; the running container is left as it was."""


@dataclass(frozen=True)
class SupervisorConfig:
    registry_file: Path
    secrets_host_dir: Path
    network: str = "cortex-challenges"
    master_url: str = "http://cortex-master:8080"
    ready_seconds: float = 60

    def __post_init__(self) -> None:
        if not self.secrets_host_dir.is_absolute():
            raise ValueError("challenge secrets host directory must be absolute")


@dataclass
class Supervisor:
    config: SupervisorConfig
    docker: httpx.AsyncClient
    http: httpx.AsyncClient
    sleep: Callable[[float], Awaitable[None]] = asyncio.sleep
    refused: dict[str, str] = field(default_factory=dict)
    checked: dict[str, float] = field(default_factory=dict)

    async def _docker(self, method: str, path: str, **kwargs) -> httpx.Response:
        response = await self.docker.request(method, path, **kwargs)
        if response.status_code >= 400 and response.status_code != 404:
            raise SupervisorError(f"docker {method} {path.split('?')[0]}: {response.status_code}")
        return response

    async def resolve(self, entry: RegistryEntry) -> tuple[str, dict[str, str]]:
        """Pull the channel or pin and return (repository digest, labels)."""
        tag = entry.pin or entry.channel
        async with self.docker.stream(
            "POST", "/images/create", params={"fromImage": entry.image, "tag": tag}, timeout=900
        ) as response:
            if response.status_code != 200:
                raise SupervisorError(f"{entry.id}: pull failed with {response.status_code}")
            async for line in response.aiter_lines():
                if line.strip() and "error" in json.loads(line):
                    raise SupervisorError(f"{entry.id}: pull reported an error")
        reference = f"{entry.image}@{entry.pin}" if entry.pin else f"{entry.image}:{tag}"
        image = (await self._docker("GET", f"/images/{reference}/json")).json()
        digests = [
            value.split("@", 1)[1]
            for value in image.get("RepoDigests") or []
            if value.split("@", 1)[0] == entry.image
        ]
        if len(set(digests)) != 1:
            raise SupervisorError(f"{entry.id}: image has no single repository digest")
        return digests[0], (image.get("Config") or {}).get("Labels") or {}

    def verify_labels(self, entry: RegistryEntry, labels: dict[str, str]) -> None:
        if (
            labels.get("io.cortex.challenge.slug") != entry.id
            or labels.get("io.cortex.challenge.contract") != CONTRACT
            or labels.get("org.opencontainers.image.source", "").rstrip("/") != entry.source
        ):
            raise SupervisorError(f"{entry.id}: image labels do not match the registry entry")

    async def verify_attestation(self, entry: RegistryEntry, digest: str) -> None:
        # ponytail: checks that GitHub holds build provenance for this digest in the source
        # repository, not the Sigstore bundle signature. Upgrade: verify the returned bundle
        # with sigstore-python (or `gh attestation verify`) before trusting a new digest.
        response = await self.http.get(
            f"https://api.github.com/repos/{entry.owner_repo}/attestations/{digest}",
            headers={"accept": "application/vnd.github+json"},
            timeout=30,
        )
        if response.status_code != 200 or not response.json().get("attestations"):
            raise SupervisorError(f"{entry.id}: no build provenance for {digest}")

    def _spec(self, entry: RegistryEntry, digest: str, *, canary: bool) -> dict:
        env = {
            **entry.env,
            "CHALLENGE_SLUG": entry.id,
            "CHALLENGE_STATE_DIR": "/data",
            "CHALLENGE_INTERNAL_TOKEN_FILE": "/run/secrets/internal.token",
            "CHALLENGE_ADMIN_TOKEN_FILE": "/run/secrets/admin.token",
            "CHALLENGE_MASTER_URL": self.config.master_url,
        }
        mounts = []
        tmpfs = {"/tmp": "rw,noexec,nosuid,size=64m"}
        if canary:
            tmpfs["/data"] = "rw,noexec,nosuid,size=64m,uid=65532,gid=65532,mode=0700"
        else:
            mounts = [
                {"Type": "volume", "Source": f"{container_name(entry.id)}-data", "Target": "/data"},
                {
                    "Type": "bind",
                    "Source": str(self.config.secrets_host_dir / entry.id),
                    "Target": "/run/secrets",
                    "ReadOnly": True,
                },
            ]
        spec: dict[str, Any] = {
            "Image": f"{entry.image}@{digest}",
            "Env": [f"{name}={value}" for name, value in sorted(env.items())],
            "User": "65532:65532",
            "Labels": {
                MANAGED: "true",
                "io.cortex.challenge.id": entry.id,
                "io.cortex.challenge.digest": digest,
            },
            "HostConfig": {
                "ReadonlyRootfs": True,
                "CapDrop": ["ALL"],
                "SecurityOpt": ["no-new-privileges:true"],
                "Init": True,
                "Memory": entry.memory_mib * 1024 * 1024,
                "NanoCpus": int(entry.cpus * 1e9),
                "PidsLimit": entry.pids,
                "Mounts": mounts,
                "Tmpfs": tmpfs,
                "NetworkMode": self.config.network,
                "RestartPolicy": {"Name": "no" if canary else "unless-stopped"},
                "LogConfig": {"Type": "json-file", "Config": {"max-size": "20m", "max-file": "3"}},
            },
        }
        # Any registry, secret-path or network change alters this, so it redeploys even
        # when the image digest stays the same.
        canonical = json.dumps(spec, sort_keys=True, separators=(",", ":")).encode()
        spec["Labels"][CONFIG] = hashlib.sha256(canonical).hexdigest()
        return spec

    async def _remove(self, name: str) -> None:
        await self._docker("DELETE", f"/containers/{name}", params={"force": "true"})

    async def _start(self, name: str, spec: dict) -> None:
        await self._docker("POST", "/containers/create", params={"name": name}, json=spec)
        await self._docker("POST", f"/containers/{name}/start")

    async def _answers_version(self, name: str, entry: RegistryEntry) -> bool:
        loop = asyncio.get_running_loop()
        deadline = loop.time() + self.config.ready_seconds
        while loop.time() < deadline:
            try:
                response = await self.http.get(f"http://{name}:8000/version", timeout=5)
                body = response.json() if response.status_code == 200 else {}
                if body.get("slug") == entry.id and str(body.get("contract")) == CONTRACT:
                    return True
            except (httpx.HTTPError, ValueError):
                pass
            await self.sleep(1)
        return False

    async def _inspect(self, name: str) -> tuple[dict[str, str], bool] | None:
        """(labels, running) of a container, or None when it does not exist."""
        response = await self._docker("GET", f"/containers/{name}/json")
        if response.status_code == 404:
            return None
        state = response.json()
        labels = (state.get("Config") or {}).get("Labels") or {}
        return labels, bool((state.get("State") or {}).get("Running"))

    async def _rollout(self, entry: RegistryEntry, spec: dict, *, replace: bool) -> bool:
        """Swap in `spec`; any failure restores the exact previous container."""
        name = container_name(entry.id)
        backup = f"{name}-previous"
        await self._remove(backup)
        if replace:
            await self._docker("POST", f"/containers/{name}/stop", params={"t": "30"})
            await self._docker("POST", f"/containers/{name}/rename", params={"name": backup})
        try:
            await self._start(name, spec)
            ready = await self._answers_version(name, entry)
        except (SupervisorError, httpx.HTTPError):
            ready = False
        if ready:
            await self._remove(backup)
            return True
        await self._remove(name)
        if replace:
            await self._docker("POST", f"/containers/{backup}/rename", params={"name": name})
            await self._docker("POST", f"/containers/{name}/start")
            if await self._answers_version(name, entry):
                LOG.warning("challenge %s rolled back to its previous container", entry.id)
            else:
                LOG.error("challenge %s previous container did not come back", entry.id)
        return False

    async def reconcile(self, entry: RegistryEntry) -> str:
        """Converge one challenge; returns the digest that is running afterwards."""
        digest, labels = await self.resolve(entry)
        name = container_name(entry.id)
        spec = self._spec(entry, digest, canary=False)
        fingerprint = spec["Labels"][CONFIG]
        deployed = await self._inspect(name)
        if deployed is None and await self._inspect(f"{name}-previous") is not None:
            # A restart mid-rollout left only the set-aside container: it is the last
            # known-good service, so restore it instead of letting _rollout delete it.
            await self._docker("POST", f"/containers/{name}-previous/rename", params={"name": name})
            deployed = await self._inspect(name)
            LOG.warning(
                "challenge %s recovered its container from an interrupted rollout", entry.id
            )
        if deployed is not None and deployed[0].get(CONFIG) == fingerprint:
            if not deployed[1]:
                await self._docker("POST", f"/containers/{name}/start")
            return digest
        if self.refused.get(entry.id) == fingerprint:
            raise SupervisorError(f"{entry.id}: {digest} was refused; waiting for a change")
        try:
            # Every new spec re-checks labels and provenance against the current entry,
            # so a source edit is verified even when the digest stays the same.
            self.verify_labels(entry, labels)
            if entry.attestation:
                await self.verify_attestation(entry, digest)
        except SupervisorError:
            self.refused[entry.id] = fingerprint
            raise
        # A configuration-only change skips the canary: that digest already booted here.
        if deployed is None or deployed[0].get("io.cortex.challenge.digest") != digest:
            try:
                canary = f"{name}-canary"
                await self._remove(canary)
                try:
                    await self._start(canary, self._spec(entry, digest, canary=True))
                    if not await self._answers_version(canary, entry):
                        raise SupervisorError(f"{entry.id}: canary did not answer /version")
                finally:
                    await self._remove(canary)
            except SupervisorError:
                self.refused[entry.id] = fingerprint
                raise
        if not await self._rollout(entry, spec, replace=deployed is not None):
            self.refused[entry.id] = fingerprint
            raise SupervisorError(f"{entry.id}: {digest} failed after rollout")
        self.refused.pop(entry.id, None)
        LOG.info("challenge %s now runs %s", entry.id, digest)
        return digest

    async def prune(self, registry: dict[str, RegistryEntry]) -> None:
        filters = json.dumps({"label": [f"{MANAGED}=true"]})
        listed = await self._docker(
            "GET", "/containers/json", params={"all": "true", "filters": filters}
        )
        for container in listed.json():
            identifier = (container.get("Labels") or {}).get("io.cortex.challenge.id")
            if identifier not in registry:
                # The data volume is kept so a re-registered challenge resumes its state.
                await self._remove(container["Id"])
                LOG.info("removed unregistered challenge container %s", identifier)

    async def tick(self, now: float) -> None:
        registry = load_registry(self.config.registry_file)
        await self.prune(registry)
        for entry in registry.values():
            if now - self.checked.get(entry.id, float("-inf")) < entry.poll_seconds:
                continue
            self.checked[entry.id] = now
            try:
                await self.reconcile(entry)
            except (SupervisorError, httpx.HTTPError) as error:
                LOG.warning("challenge %s not updated: %s", entry.id, error)

    async def run(self) -> None:
        loop = asyncio.get_running_loop()
        while True:
            try:
                await self.tick(loop.time())
            except (ValueError, SupervisorError, httpx.HTTPError) as error:
                LOG.warning("challenge supervisor tick failed: %s", error)
            await self.sleep(15)
