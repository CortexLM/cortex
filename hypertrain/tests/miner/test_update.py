from __future__ import annotations

import hashlib
import json
from pathlib import Path

import pytest
from scripts import sign_release

from hypertrain.miner import update
from hypertrain.miner.update import (
    GRACE_S,
    TEST_PUBKEY_ENV,
    UpdateManifest,
    Updater,
    UpdateRejected,
    sign_manifest,
)
from hypertrain.protocol.keys import Keypair

KEY = Keypair(bytes(range(32)))
OTHER = Keypair(bytes(range(1, 33)))
WHEEL = b"fake wheel bytes"
URL = "https://example.test/hypertrain-0.2.0-py3-none-any.whl"
MANIFEST_URL = "https://example.test/manifest.json"
NOW = 1_800_000_000


@pytest.fixture(autouse=True)
def test_key(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv(TEST_PUBKEY_ENV, KEY.public.hex())


def manifest(**over: object) -> UpdateManifest:
    fields: dict[str, object] = {
        "version": "0.2.0",
        "tag": "v0.2.0",
        "commit": "a" * 40,
        "wheel_url": URL,
        "wheel_sha256": hashlib.sha256(WHEEL).hexdigest(),
        "announce_at": NOW - 598,
        "changeover_at": NOW + 2,
    }
    fields.update(over)
    return UpdateManifest.model_validate(fields)


class Host:
    def __init__(self, signed: bytes, wheel: bytes = WHEEL) -> None:
        self.files = {MANIFEST_URL: signed, URL: wheel}
        self.installs = 0

    def fetch(self, url: str) -> bytes:
        return self.files[url]

    def install(self, wheel: Path, dest: Path) -> None:
        self.installs += 1
        (dest / "venv").mkdir()
        (dest / "venv" / "marker").write_bytes(wheel.read_bytes())


def updater(root: Path, host: Host, version: str = "0.1.0") -> Updater:
    return Updater(root, version, fetch=host.fetch, install=host.install)


def test_two_miners_swap_at_changeover_not_before(tmp_path: Path) -> None:
    host = Host(sign_manifest(manifest(), KEY))
    miners = [updater(tmp_path / f"m{i}", host) for i in range(2)]
    for m in miners:
        m.poll(MANIFEST_URL)
        assert not m.due(NOW)  # announced, not staged
        m.stage()
    for t in (NOW, NOW + 1, NOW + 1.999):
        assert [m.due(t) for m in miners] == [False, False]
        assert not any(m.must_halt(t) for m in miners)
    assert not (tmp_path / "m0" / "current").exists()
    assert [m.due(NOW + 2) for m in miners] == [True, True]
    for i, m in enumerate(miners):
        dest = m.swap()
        current = tmp_path / f"m{i}" / "current"
        assert current.is_symlink() and current.resolve() == dest.resolve()
        assert (current / "venv" / "marker").read_bytes() == WHEEL
    assert host.installs == 2


def test_swap_replaces_existing_current(tmp_path: Path) -> None:
    host = Host(sign_manifest(manifest(), KEY))
    (tmp_path / "versions" / "0.1.0").mkdir(parents=True)
    (tmp_path / "current").symlink_to("versions/0.1.0")
    m = updater(tmp_path, host)
    m.poll(MANIFEST_URL)
    m.stage()
    m.swap()
    assert (tmp_path / "current").resolve() == (tmp_path / "versions" / "0.2.0").resolve()


def raw(doc: dict[str, object]) -> bytes:
    return json.dumps(doc).encode()


@pytest.mark.parametrize(
    "signed",
    [
        pytest.param(raw({"manifest": manifest().model_dump()}), id="unsigned"),
        pytest.param(raw({"manifest": manifest().model_dump(), "signature": ""}), id="empty-sig"),
        pytest.param(b"not json", id="garbage"),
        pytest.param(sign_manifest(manifest(), OTHER), id="wrong-signer"),
        pytest.param(sign_manifest(manifest(tag="0.2.0"), KEY), id="tag-no-v"),
        pytest.param(sign_manifest(manifest(tag="v0.2.0-rc1"), KEY), id="tag-suffix"),
        pytest.param(sign_manifest(manifest(tag="v0.3.0"), KEY), id="tag-version-mismatch"),
        pytest.param(sign_manifest(manifest(commit="main"), KEY), id="non-sha-commit"),
        pytest.param(sign_manifest(manifest(version="0.1.0", tag="v0.1.0"), KEY), id="equal"),
        pytest.param(sign_manifest(manifest(version="0.0.9", tag="v0.0.9"), KEY), id="downgrade"),
        pytest.param(sign_manifest(manifest(changeover_at=NOW + 1), KEY), id="window-599s"),
    ],
)
def test_poll_rejects(tmp_path: Path, signed: bytes) -> None:
    m = updater(tmp_path, Host(signed))
    with pytest.raises(UpdateRejected):
        m.poll(MANIFEST_URL)
    assert m.pending is None and not (tmp_path / "pending.json").exists()


def test_tampered_manifest_rejected(tmp_path: Path) -> None:
    doc = json.loads(sign_manifest(manifest(), KEY))
    doc["manifest"]["changeover_at"] += 1
    with pytest.raises(UpdateRejected, match="signature"):
        updater(tmp_path, Host(raw(doc))).poll(MANIFEST_URL)


def test_pinned_pubkey_without_override(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.delenv(TEST_PUBKEY_ENV)
    with pytest.raises(UpdateRejected, match="signer"):
        updater(tmp_path, Host(sign_manifest(manifest(), KEY))).poll(MANIFEST_URL)


def test_sha_mismatch_does_not_stage(tmp_path: Path) -> None:
    host = Host(sign_manifest(manifest(), KEY), wheel=b"tampered")
    m = updater(tmp_path, host)
    m.poll(MANIFEST_URL)
    with pytest.raises(UpdateRejected, match="sha256"):
        m.stage()
    assert not m.staged() and not m.due(NOW + 10) and host.installs == 0
    with pytest.raises(UpdateRejected):
        m.swap()


def test_staged_state_survives_restart(tmp_path: Path) -> None:
    host = Host(sign_manifest(manifest(), KEY))
    first = updater(tmp_path, host)
    first.poll(MANIFEST_URL)
    first.stage()
    restarted = updater(tmp_path, host)
    assert restarted.pending == first.pending and restarted.staged()
    assert not restarted.due(NOW) and restarted.due(NOW + 2)
    restarted.stage()
    assert host.installs == 1  # no re-download after restart
    restarted.swap()
    after = updater(tmp_path, host, version="0.2.0")  # process restarted on the new version
    assert after.pending is None and not after.must_halt(NOW + 10_000)
    assert not (tmp_path / "pending.json").exists()


def test_tampered_state_file_rejected_on_restart(tmp_path: Path) -> None:
    host = Host(sign_manifest(manifest(), KEY))
    updater(tmp_path, host).poll(MANIFEST_URL)
    host.files[MANIFEST_URL] = sign_manifest(manifest(), OTHER)
    (tmp_path / "pending.json").write_bytes(host.files[MANIFEST_URL])
    with pytest.raises(UpdateRejected):
        updater(tmp_path, host)


def test_must_halt_timing(tmp_path: Path) -> None:
    m = updater(tmp_path, Host(sign_manifest(manifest(), KEY)))
    assert not m.must_halt(NOW + 10_000)  # nothing announced
    m.poll(MANIFEST_URL)
    assert not m.must_halt(NOW + 2)
    assert not m.must_halt(NOW + 2 + GRACE_S - 0.001)
    assert m.must_halt(NOW + 2 + GRACE_S)
    m.stage()
    m.swap()
    assert m.must_halt(NOW + 2 + GRACE_S)  # swapped but this process is still the old version


def test_newer_manifest_supersedes_pending(tmp_path: Path) -> None:
    host = Host(sign_manifest(manifest(), KEY))
    m = updater(tmp_path, host)
    m.poll(MANIFEST_URL)
    assert m.poll(MANIFEST_URL) == m.pending  # idempotent re-poll
    host.files[MANIFEST_URL] = sign_manifest(manifest(changeover_at=NOW + 5), KEY)
    with pytest.raises(UpdateRejected, match="conflicts"):
        m.poll(MANIFEST_URL)
    host.files[MANIFEST_URL] = sign_manifest(manifest(version="0.3.0", tag="v0.3.0"), KEY)
    assert m.poll(MANIFEST_URL).version == "0.3.0"


def test_sign_release_script(tmp_path: Path) -> None:
    key_file = tmp_path / "release.key"
    key_file.write_text(bytes(range(32)).hex())
    wheel = tmp_path / "hypertrain-0.2.0-py3-none-any.whl"
    wheel.write_bytes(WHEEL)
    out = tmp_path / "manifest.json"
    args = ["--key-file", str(key_file), "--tag", "v0.2.0", "--commit", "b" * 40]
    args += ["--wheel", str(wheel), "--wheel-url", URL, "--announce-at", str(NOW)]
    assert sign_release.main([*args, "--out", str(out)]) == 0
    got = update.verify_manifest(out.read_bytes(), KEY.public)
    assert got.wheel_sha256 == hashlib.sha256(WHEEL).hexdigest()
    assert got.changeover_at - got.announce_at >= update.MIN_WINDOW_S
    m = updater(tmp_path / "root", Host(out.read_bytes()))
    assert m.poll(MANIFEST_URL).tag == "v0.2.0"
