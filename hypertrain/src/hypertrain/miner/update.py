"""Signed-release synchronized auto-update.

A release is an sr25519-signed UpdateManifest published by CI. Timeline per manifest:
announced (poll, then stage: download, verify sha256, install into <root>/versions/<version>
while training continues) -> at the first round boundary with now >= changeover_at the caller
checkpoints and calls swap(), which atomically repoints <root>/current, then restarts.
After changeover_at + GRACE_S a miner not running the target version must refuse new rounds.

The accepted manifest is persisted in <root>/pending.json, so a staged-but-unswapped state
survives a restart.
"""

from __future__ import annotations

import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
from collections.abc import Callable
from pathlib import Path
from urllib.parse import urlparse

import httpx
from pydantic import BaseModel, ConfigDict, ValidationError

from hypertrain import __version__
from hypertrain.protocol.keys import Keypair, verify

# Release key pubkey (sr25519). The secret seed lives only in the CI secret HYPERTRAIN_RELEASE_KEY.
RELEASE_PUBKEY = bytes.fromhex("844df0e7e836c4e9040229046f6576f17fd8a48ca38d1ab5c4ccff3e8d5e0f5f")
# Tests only: substitutes the pinned pubkey. Never set it on a real miner.
TEST_PUBKEY_ENV = "HYPERTRAIN_TEST_RELEASE_PUBKEY"
DOMAIN = b"hypertrain/release-manifest/v1\n"
MIN_WINDOW_S = 600
GRACE_S = 60

_TAG = re.compile(r"v(\d+)\.(\d+)\.(\d+)")
_HEX40 = re.compile(r"[0-9a-f]{40}")
_HEX64 = re.compile(r"[0-9a-f]{64}")
_WHEEL = re.compile(r"[A-Za-z0-9_.+-]+\.whl")

Fetch = Callable[[str], bytes]
Install = Callable[[Path, Path], None]


class UpdateRejected(ValueError):
    pass


class UpdateManifest(BaseModel):
    model_config = ConfigDict(frozen=True, extra="forbid", strict=True)

    version: str
    tag: str
    commit: str
    wheel_url: str
    wheel_sha256: str
    announce_at: int
    changeover_at: int

    def canonical(self) -> bytes:
        return (
            DOMAIN + json.dumps(self.model_dump(), sort_keys=True, separators=(",", ":")).encode()
        )


def parse_version(version: str) -> tuple[int, int, int]:
    m = _TAG.fullmatch("v" + version)
    if m is None:
        raise UpdateRejected(f"bad version {version!r}")
    return int(m[1]), int(m[2]), int(m[3])


def sign_manifest(manifest: UpdateManifest, key: Keypair) -> bytes:
    """The signed document: what CI publishes and miners poll."""
    doc = {"manifest": manifest.model_dump(), "signature": key.sign(manifest.canonical()).hex()}
    return json.dumps(doc, sort_keys=True, indent=2).encode() + b"\n"


def release_pubkey() -> bytes:
    override = os.environ.get(TEST_PUBKEY_ENV)
    return bytes.fromhex(override) if override else RELEASE_PUBKEY


def verify_manifest(raw: bytes, pubkey: bytes) -> UpdateManifest:
    """Signature and shape checks only; version ordering is the Updater's job."""
    try:
        doc = json.loads(raw)
        manifest = UpdateManifest.model_validate(doc["manifest"])
        signature = bytes.fromhex(doc["signature"])
    except (ValueError, KeyError, TypeError, ValidationError) as e:
        raise UpdateRejected(f"malformed or unsigned manifest: {e}") from None
    if not verify(pubkey, manifest.canonical(), signature):
        raise UpdateRejected("bad signature or wrong signer")
    if _TAG.fullmatch(manifest.tag) is None:
        raise UpdateRejected(f"tag {manifest.tag!r} is not vX.Y.Z")
    if manifest.tag != "v" + manifest.version:
        raise UpdateRejected(f"tag {manifest.tag} != version {manifest.version}")
    if _HEX40.fullmatch(manifest.commit) is None:
        raise UpdateRejected("commit must be a 40-hex sha")
    if _HEX64.fullmatch(manifest.wheel_sha256) is None:
        raise UpdateRejected("wheel_sha256 must be 64 hex")
    if _WHEEL.fullmatch(wheel_name(manifest)) is None:
        raise UpdateRejected("wheel_url must end in a .whl file name")
    if manifest.changeover_at - manifest.announce_at < MIN_WINDOW_S:
        raise UpdateRejected(f"changeover window < {MIN_WINDOW_S} s")
    return manifest


def wheel_name(manifest: UpdateManifest) -> str:
    return urlparse(manifest.wheel_url).path.rsplit("/", 1)[-1]


def http_fetch(url: str) -> bytes:
    r = httpx.get(url, follow_redirects=True, timeout=60.0)
    r.raise_for_status()
    return r.content


def venv_install(wheel: Path, dest: Path) -> None:
    subprocess.run([sys.executable, "-m", "venv", str(dest / "venv")], check=True)  # noqa: S603
    pip = dest / "venv" / "bin" / "pip"
    subprocess.run([str(pip), "install", "--no-input", str(wheel)], check=True)  # noqa: S603


class Updater:
    def __init__(
        self,
        root: Path,
        version: str = __version__,
        *,
        fetch: Fetch = http_fetch,
        install: Install = venv_install,
    ) -> None:
        self.root = root
        self.version = version
        self._fetch = fetch
        self._install = install
        self._pubkey = release_pubkey()
        self._state = root / "pending.json"
        root.mkdir(parents=True, exist_ok=True)
        self.pending: UpdateManifest | None = None
        if self._state.exists():
            pending = verify_manifest(self._state.read_bytes(), self._pubkey)
            if parse_version(pending.version) > parse_version(version):
                self.pending = pending
            else:  # restarted on (or past) the target: the update is done
                self._state.unlink()

    def poll(self, manifest_url: str) -> UpdateManifest:
        raw = self._fetch(manifest_url)
        manifest = verify_manifest(raw, self._pubkey)
        if parse_version(manifest.version) <= parse_version(self.version):
            raise UpdateRejected(f"{manifest.version} is not newer than {self.version}")
        if self.pending is not None:
            if manifest == self.pending:
                return manifest
            if parse_version(manifest.version) <= parse_version(self.pending.version):
                raise UpdateRejected(f"{manifest.version} conflicts with {self.pending.version}")
        tmp = self._state.with_suffix(".tmp")
        tmp.write_bytes(raw)
        os.replace(tmp, self._state)
        self.pending = manifest
        return manifest

    def target_dir(self) -> Path:
        assert self.pending is not None
        return self.root / "versions" / self.pending.version

    def staged(self) -> bool:
        if self.pending is None:
            return False
        marker = self.target_dir() / "STAGED"
        return marker.exists() and marker.read_text() == self.pending.wheel_sha256

    def stage(self) -> Path:
        if self.pending is None:
            raise UpdateRejected("no pending update")
        dest = self.target_dir()
        if self.staged():
            return dest
        data = self._fetch(self.pending.wheel_url)
        if hashlib.sha256(data).hexdigest() != self.pending.wheel_sha256:
            raise UpdateRejected("wheel sha256 mismatch")
        tmp = dest.with_name(f".{dest.name}.tmp")
        shutil.rmtree(tmp, ignore_errors=True)
        tmp.mkdir(parents=True)
        wheel = tmp / wheel_name(self.pending)
        wheel.write_bytes(data)
        self._install(wheel, tmp)
        (tmp / "STAGED").write_text(self.pending.wheel_sha256)
        shutil.rmtree(dest, ignore_errors=True)
        os.replace(tmp, dest)
        return dest

    def due(self, now: float) -> bool:
        """Call at round boundaries: True means checkpoint, swap(), restart."""
        return self.staged() and self.pending is not None and now >= self.pending.changeover_at

    def swap(self) -> Path:
        if not self.staged():
            raise UpdateRejected("nothing staged")
        dest = self.target_dir()
        link = self.root / "current"
        tmp = self.root / ".current.tmp"
        tmp.unlink(missing_ok=True)
        tmp.symlink_to(dest.relative_to(self.root))
        os.replace(tmp, link)
        # pending.json stays until the restarted process runs the target version.
        return dest

    def must_halt(self, now: float) -> bool:
        """True: this process is not on the announced version past changeover + grace."""
        if self.pending is None:
            return False
        return now >= self.pending.changeover_at + GRACE_S
