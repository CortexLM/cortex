"""SSH host-key attestation primitives; real `ssh-keygen` with throw-away keys, no network."""

from __future__ import annotations

import base64
import errno
import getpass
import json
import os
import shlex
import shutil
import socket
import subprocess
import sys
import time
from pathlib import Path
from typing import Any

import pytest
from pydantic import ValidationError

from hypertrain.gpu_ops import host_attest
from hypertrain.gpu_ops import network_qualification as driver
from hypertrain.gpu_ops.journal import fsha
from hypertrain.gpu_ops.launcher import Reject

from .test_network_gpu_qualification import (
    boot_id,
    host_gpus,
    key_file,
    manifest,
    pins,
    results,
    typed_fingerprint,
)

TREE = Path(__file__).parents[2]
GPU_A = "GPU-0a1b2c3d-0000-4000-8000-000000000001"
GPU_B = "GPU-0a1b2c3d-0000-4000-8000-000000000002"
BOOT_ID = "5e2f3b6c-7e5a-4b0f-9c8d-1f2e3d4c5b6a"


def keypair(directory: Path, name: str = "host") -> tuple[Path, str]:
    """A throw-away Ed25519 key: its private file and the `ssh-ed25519 <base64>` form."""
    private = directory / name
    subprocess.run(
        ["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", str(private)],
        check=True,
        capture_output=True,
    )
    return private, " ".join(private.with_suffix(".pub").read_text().split()[:2])


def sign(data: bytes, private: Path, namespace: str = host_attest.NAMESPACE) -> bytes:
    return subprocess.run(
        ["ssh-keygen", "-Y", "sign", "-f", str(private), "-n", namespace],
        input=data,
        check=True,
        capture_output=True,
    ).stdout


def structural_key(seed: int) -> str:
    """A well-formed `ssh-ed25519` key line field; pinned_key checks the form, not the curve."""
    wire = b"\x00\x00\x00\x0bssh-ed25519\x00\x00\x00\x20" + bytes([seed]) * 32
    return "ssh-ed25519 " + base64.b64encode(wire).decode()


KEY, OTHER = structural_key(1), structural_key(2)


def body(**override: Any) -> dict[str, Any]:
    return {
        "version": 1,
        "config_sha256": "11" * 32,
        "result_sha256": "22" * 32,
        "image_digest": "sha256:" + "44" * 32,
        "gpu_uuids": [GPU_A, GPU_B],
        "drivers": ["595.84"],
        "boot_id": BOOT_ID,
    } | override


def statement(**override: Any) -> host_attest.HostStatement:
    return host_attest.HostStatement.model_validate(body(**override))


def test_a_genuine_statement_verifies_under_its_pin(tmp_path: Path) -> None:
    # Given: the control for every refusal below, made with the real ssh-keygen.
    private, key = keypair(tmp_path)
    raw = host_attest.statement_bytes(statement())
    # When / Then
    assert host_attest.verify(raw, sign(raw, private), key) is None


@pytest.mark.parametrize(
    "case",
    ["signed_by_another_key", "wrong_namespace", "flipped_statement_byte", "garbage", "empty"],
)
def test_verify_refuses_what_the_pinned_key_did_not_sign(tmp_path: Path, case: str) -> None:
    # Given
    private, key = keypair(tmp_path)
    attacker, _ = keypair(tmp_path, "attacker")
    raw = host_attest.statement_bytes(statement())
    signature = sign(raw, private)
    if case == "signed_by_another_key":
        signature = sign(raw, attacker)
    elif case == "wrong_namespace":
        signature = sign(raw, private, "file")
    elif case == "flipped_statement_byte":
        raw = bytes([raw[0] ^ 1]) + raw[1:]
    elif case == "garbage":
        signature = b"not an armored signature"
    else:
        signature = b""
    # When / Then
    with pytest.raises(Reject, match="^qualification_attestation_signature$"):
        host_attest.verify(raw, signature, key)


def test_verify_refuses_a_key_that_smuggles_a_second_allowed_signer(tmp_path: Path) -> None:
    # Given: an attacker-signed statement and a "key" that appends the attacker as a second signer.
    private, key = keypair(tmp_path)
    attacker, attacker_key = keypair(tmp_path, "attacker")
    raw = host_attest.statement_bytes(statement())
    smuggled = f'{key}\nhost namespaces="{host_attest.NAMESPACE}" {attacker_key}'
    # When / Then
    with pytest.raises(Reject, match="^qualification_host_key_pin_invalid$"):
        host_attest.verify(raw, sign(raw, attacker), smuggled)


@pytest.mark.parametrize(
    "script,code",
    [
        (None, "qualification_ssh_keygen_unavailable"),
        (
            "echo 'No user exists for uid 4242' >&2; exit 255",
            "qualification_ssh_keygen_unavailable",
        ),
        ("exec /bin/sleep 30", "qualification_ssh_keygen_unavailable"),
        ("echo 'Could not verify signature.' >&2; exit 255", "qualification_attestation_signature"),
    ],
    ids=["no_ssh_keygen", "no_passwd_entry", "hangs", "plain_refusal"],
)
def test_verify_tells_a_broken_tool_from_a_bad_signature(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, script: str | None, code: str
) -> None:
    # Given: a search path holding a stub ssh-keygen, or nothing.
    _, key = keypair(tmp_path)
    stubs = tmp_path / "stubs"
    stubs.mkdir()
    if script is not None:
        stub = stubs / "ssh-keygen"
        stub.write_text(f"#!/bin/sh\n{script}\n")
        stub.chmod(0o755)
    monkeypatch.setattr(host_attest, "SEARCH_PATH", str(stubs))
    monkeypatch.setattr(host_attest, "KEYGEN_TIMEOUT", 0.5)
    # When / Then
    with pytest.raises(Reject, match=f"^{code}$"):
        host_attest.verify(b"statement", b"signature", key)


@pytest.mark.parametrize(
    "text",
    [
        f"host {KEY}\n",
        f"[203.0.113.7]:2222 {KEY}\n",
        f"|1|c2FsdHNhbHRzYWx0c2FsdA==|aGFzaGhhc2hoYXNoaGFzaA== {KEY}\n",
        f"# a comment\n\nhost {KEY} trailing comment\nhost ssh-rsa AAAAB3NzaC1yc2E\n"
        "host ecdsa-sha2-nistp256 AAAAE2VjZHNh\n",
        f"host {KEY}\nalias {KEY}\n",
        f"@revoked host {OTHER}\nhost {KEY}\r\n",
    ],
    ids=[
        "plain",
        "port",
        "hashed",
        "comments_and_other_types",
        "same_key_twice",
        "marker_and_crlf",
    ],
)
def test_pinned_key_accepts_the_one_ed25519_key_in_any_known_hosts_form(text: str) -> None:
    # When / Then
    assert host_attest.pinned_key(text.encode()) == KEY


@pytest.mark.parametrize(
    "text",
    [
        f"host {KEY}\nhost {OTHER}\n",
        "# nothing\nhost ssh-rsa AAAAB3NzaC1yc2E\n",
        "",
        f"@cert-authority * {KEY}\n",
        f"host {KEY[:-1]}\n",
        f"host {KEY}A\n",
        f"host ssh-ed25519 {'A' * 68}\n",
        f"host {KEY}\rhost {OTHER}\n",
    ],
    ids=[
        "two_distinct_keys",
        "no_ed25519_key",
        "empty",
        "only_a_certificate_authority",
        "short_blob",
        "long_blob",
        "blob_without_the_ed25519_header",
        "carriage_return_joins_two_lines",
    ],
)
def test_pinned_key_refuses_anything_but_exactly_one_key(text: str) -> None:
    # When / Then
    with pytest.raises(Reject, match="^qualification_host_key_pin_invalid$"):
        host_attest.pinned_key(text.encode())


def test_fingerprint_equals_ssh_keygen_lf(tmp_path: Path) -> None:
    # Given
    private, key = keypair(tmp_path)
    printed = subprocess.run(
        ["ssh-keygen", "-lf", str(private.with_suffix(".pub"))],
        check=True,
        capture_output=True,
        text=True,
    ).stdout.split()[1]
    # When / Then
    assert host_attest.fingerprint(key) == printed
    assert not printed.endswith("=")


@pytest.mark.parametrize(
    "override",
    [
        {"unknown": 1},
        {"version": 2},
        {"gpu_uuids": []},
        {"gpu_uuids": [GPU_A.upper()]},
        {"gpu_uuids": [GPU_A, GPU_A]},
        {"gpu_uuids": [GPU_B, GPU_A]},
        {"gpu_uuids": [f"GPU-0a1b2c3d-0000-4000-8000-{i:012x}" for i in range(17)]},
        {"gpu_uuids": ["0a1b2c3d-0000-4000-8000-000000000001"]},
        {"drivers": ["595.91", "595.84"]},
        {"drivers": []},
        {"boot_id": BOOT_ID.upper()},
        {"boot_id": BOOT_ID + "\n"},
        {"image_digest": "44" * 32},
    ],
    ids=[
        "unknown_field",
        "version_2",
        "no_gpu",
        "uppercase_uuid",
        "duplicate_uuid",
        "unsorted_uuids",
        "seventeen_gpus",
        "uuid_without_prefix",
        "unsorted_drivers",
        "no_driver",
        "uppercase_boot_id",
        "boot_id_newline",
        "digest_without_scheme",
    ],
)
def test_statement_refuses_what_is_not_the_exact_form(override: dict[str, Any]) -> None:
    # Given a statement body with one field outside its form, When / Then
    with pytest.raises(ValidationError):
        host_attest.HostStatement.model_validate(body(**override))


def write_pair(result: Path, raw: bytes, signature: bytes = b"sig") -> None:
    (result.parent / host_attest.ATTESTATION).write_bytes(raw)
    (result.parent / host_attest.SIGNATURE).write_bytes(signature)


def test_load_returns_the_canonical_statement_and_its_signature(tmp_path: Path) -> None:
    # Given
    result = tmp_path / "result.json"
    raw = host_attest.statement_bytes(statement())
    signature = b"s" * host_attest.MAX_SIGNATURE  # the largest signature load() promises to read
    write_pair(result, raw, signature)
    # When
    loaded = host_attest.load(result)
    # Then
    assert loaded == host_attest.Attestation(statement(), raw, signature)


@pytest.mark.parametrize(
    "fault,code",
    [
        ("no_statement", "missing"),
        ("no_signature", "missing"),
        ("statement_symlink", "missing"),
        ("signature_symlink", "missing"),
        ("statement_directory", "missing"),
        ("statement_fifo", "missing"),
        ("statement_too_big", "missing"),
        ("signature_too_big", "missing"),
        ("spaced_json", "not_canonical"),
        ("unsorted_keys", "not_canonical"),
        ("not_utf8", "not_canonical"),
        ("unknown_field", "not_canonical"),
    ],
)
def test_load_refuses_what_is_not_a_plain_canonical_pair(
    tmp_path: Path, fault: str, code: str
) -> None:
    # Given
    result = tmp_path / "result.json"
    raw = host_attest.statement_bytes(statement())
    write_pair(result, raw)
    statement_file, signature_file = (
        tmp_path / host_attest.ATTESTATION,
        tmp_path / host_attest.SIGNATURE,
    )
    elsewhere = tmp_path / "elsewhere"
    if fault == "no_statement":
        statement_file.unlink()
    elif fault == "no_signature":
        signature_file.unlink()
    elif fault in {"statement_symlink", "signature_symlink"}:
        link = statement_file if fault == "statement_symlink" else signature_file
        elsewhere.write_bytes(link.read_bytes())
        link.unlink()
        link.symlink_to(elsewhere)
    elif fault == "statement_directory":
        statement_file.unlink()
        statement_file.mkdir()
    elif fault == "statement_fifo":
        statement_file.unlink()
        os.mkfifo(statement_file)
    elif fault == "statement_too_big":
        statement_file.write_bytes(b" " * (host_attest.MAX_STATEMENT + 1))
    elif fault == "signature_too_big":
        signature_file.write_bytes(b"s" * (host_attest.MAX_SIGNATURE + 1))
    elif fault == "spaced_json":
        statement_file.write_text(json.dumps(body()))
    elif fault == "unsorted_keys":
        statement_file.write_text(json.dumps(body(), separators=(",", ":")))
    elif fault == "not_utf8":
        statement_file.write_bytes(b"\xff\xfe" + raw)
    else:
        statement_file.write_bytes(host_attest.canonicalize(body(unknown=1)))
    # When / Then
    with pytest.raises(Reject, match=f"^qualification_attestation_{code}$"):
        host_attest.load(result)


def host_that_prints(index: int, key: Path, *, driver="595.84", sudo=True, key_path=None):
    """A run() standing for the host over its SSH channel: it answers the fixed facts command from
    a table and signs with the real ssh-keygen and a throw-away key standing for the host key."""
    facts = "".join(f"{u}, {driver}\n" for u in host_gpus(index)) + f"{boot_id(index)}\n"
    path = key_path or host_attest.HOST_KEY_PATH
    signing = f"ssh-keygen -Y sign -f {shlex.quote(path)} -n {host_attest.NAMESPACE}"

    def run(command: str, stdin: bytes) -> bytes:
        if command == host_attest.FACTS:
            return facts.encode()
        assert command == ("sudo -n " if sudo else "") + signing  # the one fixed signing command
        return sign(stdin, key)

    return run


def unattested(tmp_path: Path) -> list[Path]:
    """Two results with their attestation files removed: what the rescue leaves."""
    paths = results(tmp_path)
    for path in paths:
        (path.parent / host_attest.ATTESTATION).unlink()
        (path.parent / host_attest.SIGNATURE).unlink()
    return paths


def test_attest_output_passes_strict_compare_and_equals_the_independent_statement(
    tmp_path: Path,
) -> None:
    # Given: the statements the test fixture writes on its own, then results without any.
    paths = results(tmp_path)
    independent = [(p.parent / host_attest.ATTESTATION).read_bytes() for p in paths]
    for path in paths:
        (path.parent / host_attest.ATTESTATION).unlink()
        (path.parent / host_attest.SIGNATURE).unlink()
    keys = pins(paths)
    # When: the producer attests each host over its (fake) channel and writes the files.
    for index, path in enumerate(paths):
        host = host_that_prints(index, key_file(path, index))
        raw, signature = host_attest.attest(host, path, pin=keys[index])
        host_attest.write_files(path, raw, signature)
    # Then
    assert [(p.parent / host_attest.ATTESTATION).read_bytes() for p in paths] == independent
    assert driver.compare(paths, manifest(), host_keys=keys) == {
        "two_host_bitwise_match": True,
        "workload_allowed": False,
    }


@pytest.mark.parametrize(
    "sudo,key_path",
    [(True, None), (False, None), (True, "/srv/host keys/ssh_host_ed25519_key")],
    ids=["sudo", "no_sudo", "path_with_a_space"],
)
def test_attest_runs_one_fixed_signing_command(
    tmp_path: Path, sudo: bool, key_path: str | None
) -> None:
    # Given
    [path, _] = unattested(tmp_path)
    host = host_that_prints(0, key_file(path, 0), sudo=sudo, key_path=key_path)
    # When: the fake host asserts the exact command text it is given.
    raw, signature = host_attest.attest(
        host, path, key_path=key_path or host_attest.HOST_KEY_PATH, sudo=sudo, pin=pins([path])[0]
    )
    # Then
    assert host_attest.HostStatement.model_validate_json(raw).result_sha256 == fsha(path)
    assert signature.startswith(b"-----" + b"BEGIN SSH SIGNATURE-----")  # armor, not a key


def test_attest_refuses_a_result_from_other_gpus(tmp_path: Path) -> None:
    # Given: the operator points the producer at host 1 for host 0's result.
    [path, _] = unattested(tmp_path)
    wrong_box = host_that_prints(1, key_file(path, 1))
    # When / Then
    with pytest.raises(Reject, match="^qualification_attestation_wrong_host$"):
        host_attest.attest(wrong_box, path, pin=pins([path])[0])


def test_attest_refuses_a_result_with_another_driver(tmp_path: Path) -> None:
    # Given: the box runs a driver the result never recorded.
    [path, _] = unattested(tmp_path)
    host = host_that_prints(0, key_file(path, 0), driver="595.91")
    # When / Then
    with pytest.raises(Reject, match="^qualification_attestation_wrong_host$"):
        host_attest.attest(host, path, pin=pins([path])[0])


def test_attest_refuses_a_result_without_recorded_gpus(tmp_path: Path) -> None:
    # Given: a result written before device UUIDs were recorded.
    [path, _] = unattested(tmp_path)
    data = json.loads(path.read_bytes())
    del data["environment"]["gpu_uuids"]
    path.write_text(json.dumps(data))
    # When / Then
    with pytest.raises(Reject, match="^qualification_attestation_wrong_host$"):
        host_attest.attest(host_that_prints(0, key_file(path, 0)), path, pin=pins([path])[0])


def test_attest_refuses_output_not_signed_by_the_pin(tmp_path: Path) -> None:
    # Given: the pin the owner has is another host's key.
    [path, other] = unattested(tmp_path)
    host = host_that_prints(0, key_file(path, 0))
    # When / Then: the producer verifies its own output before it hands anything back.
    with pytest.raises(Reject, match="^qualification_attestation_signature$"):
        host_attest.attest(host, path, pin=pins([other])[0])


@pytest.mark.parametrize(
    "plant",
    ["statement", "signature", "statement_symlink", "signature_symlink"],
)
def test_write_files_refuses_to_overwrite_a_planted_file(tmp_path: Path, plant: str) -> None:
    # Given: a container planted one of the two files in its output directory.
    [path, _] = unattested(tmp_path)
    victim = tmp_path / "victim"
    victim.write_bytes(b"not for us")
    name = host_attest.ATTESTATION if plant.startswith("statement") else host_attest.SIGNATURE
    planted = path.parent / name
    if plant.endswith("symlink"):
        planted.symlink_to(victim)
    else:
        planted.write_bytes(b"planted")
    # When / Then
    with pytest.raises(Reject, match="^qualification_attestation_exists$"):
        host_attest.write_files(path, b"statement", b"signature")
    assert victim.read_bytes() == b"not for us"
    assert {p.name for p in path.parent.glob("attestation.*")} == {name}  # no half-written pair
    if plant.endswith("symlink"):
        assert planted.is_symlink()
    else:
        assert planted.read_bytes() == b"planted"


def test_write_files_writes_private_files(tmp_path: Path) -> None:
    # Given
    [path, _] = unattested(tmp_path)
    # When
    host_attest.write_files(path, b"statement", b"signature")
    # Then
    modes = {p.name: p.stat().st_mode & 0o777 for p in path.parent.glob("attestation.*")}
    assert modes == {host_attest.ATTESTATION: 0o600, host_attest.SIGNATURE: 0o600}
    assert (path.parent / host_attest.SIGNATURE).read_bytes() == b"signature"


@pytest.mark.parametrize("failing", ["create", "publish"])
def test_write_files_leaves_nothing_when_the_second_file_fails(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, failing: str
) -> None:
    # Given: a rescue directory, and a disk that refuses the second of the two files.
    [path, _] = unattested(tmp_path)
    before = sorted(p.name for p in path.parent.iterdir())
    real_open, real_link = os.open, os.link

    def full_disk_for_the_signature(file: Any, *args: Any, **kwargs: Any) -> int:
        if failing == "create" and host_attest.SIGNATURE in os.fspath(file):
            raise OSError(errno.ENOSPC, "No space left on device")
        return real_open(file, *args, **kwargs)

    def no_link_for_the_signature(source: Any, destination: Any, **kwargs: Any) -> None:
        if failing == "publish" and os.fspath(destination).endswith(host_attest.SIGNATURE):
            raise OSError(errno.EPERM, "Operation not permitted")
        real_link(source, destination, **kwargs)

    monkeypatch.setattr(os, "open", full_disk_for_the_signature)
    monkeypatch.setattr(os, "link", no_link_for_the_signature)
    # When
    with pytest.raises(OSError) as refused:
        host_attest.write_files(path, b"statement", b"signature")
    # Then: neither file exists and no temporary file is left; the error is the disk's own.
    assert refused.value.errno in (errno.ENOSPC, errno.EPERM)
    assert sorted(p.name for p in path.parent.iterdir()) == before


GOOD_ROWS = f"{GPU_A}, 595.84\n{GPU_B}, 595.84\n"


def test_parse_facts_reads_the_two_commands_output() -> None:
    # Given: nvidia-smi prints upper case hex on some drivers; boot_id is one line.
    text = f"{GPU_A.upper()}, 595.91.07\n{GPU_B}, 595.84\n{BOOT_ID}\n"
    # When
    facts = host_attest.parse_facts(text)
    # Then
    assert facts == host_attest.HostFacts([GPU_A, GPU_B], ["595.91.07", "595.84"], BOOT_ID)


@pytest.mark.parametrize(
    "text",
    [
        "",
        "garbage\n",
        GOOD_ROWS,
        f"{BOOT_ID}\n",
        f"{GPU_A}, 595.84\n{GPU_A}, 595.84\n{BOOT_ID}\n",
        f"{GPU_A}, 595.84\n{BOOT_ID.upper()}\n",
        f"{GPU_A}\n{BOOT_ID}\n",
        f"{GPU_A}, 595.84\n\n{BOOT_ID}\n",
        f"{GPU_A}, 595.84\n{BOOT_ID}\nextra\n",
        "".join(f"GPU-0a1b2c3d-0000-4000-8000-{i:012x}, 595.84\n" for i in range(17)) + BOOT_ID,
    ],
    ids=[
        "empty",
        "garbage",
        "no_boot_id",
        "no_gpu",
        "duplicate_uuid",
        "bad_boot_id",
        "row_without_driver",
        "blank_line",
        "trailing_output",
        "seventeen_gpus",
    ],
)
def test_parse_facts_is_strict(text: str) -> None:
    # When / Then
    with pytest.raises(Reject, match="^qualification_host_facts_malformed$"):
        host_attest.parse_facts(text)


def test_qualification_attest_help_runs() -> None:
    # Given / When: the operator tool is top-level code, so loading it is the smoke test.
    done = subprocess.run(
        [sys.executable, str(TREE / "scripts/ops/qualification_attest.py"), "--help"],
        capture_output=True,
        text=True,
        timeout=120,
        check=False,
    )
    # Then
    assert done.returncode == 0 and "--executor-config" in done.stdout


SSHD = Path("/usr/sbin/sshd")


class Daemon:
    """A loopback sshd of the current user standing for GPU host `index`: its host key is the key
    attested; nvidia-smi, cat and sudo are stubs on the remote PATH, ssh-keygen is the real one."""

    def __init__(self, tmp_path: Path, index: int) -> None:
        self.state = tmp_path / f"sshd-{index}"
        self.state.mkdir(mode=0o700)
        for name in ("host", "client"):
            subprocess.run(
                ["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", str(self.state / name)],
                check=True,
            )
        authorized = self.state / "authorized_keys"
        authorized.write_text((self.state / "client.pub").read_text())
        authorized.chmod(0o600)
        self.host_key = " ".join((self.state / "host.pub").read_text().split()[:2])
        stubs = self.state / "bin"
        stubs.mkdir()
        rows = "".join(f"{u}, 595.84\\n" for u in host_gpus(index))
        for name, body in {
            "nvidia-smi": f"printf '{rows}'",
            "cat": f"echo {boot_id(index)}",  # the only file FACTS reads is the boot id
            "sudo": f'touch {self.state}/sudo-used; shift; exec "$@"',  # drops -n
        }.items():
            (stubs / name).write_text(f"#!/bin/sh\n{body}\n")
            (stubs / name).chmod(0o755)
        with socket.socket() as probe:
            probe.bind(("127.0.0.1", 0))
            self.port = int(probe.getsockname()[1])
        (self.state / "sshd_config").write_text(
            f"ListenAddress 127.0.0.1\nPort {self.port}\nHostKey {self.state}/host\n"
            f"PidFile {self.state}/pid\nAuthorizedKeysFile {authorized}\nStrictModes no\n"
            "PasswordAuthentication no\nKbdInteractiveAuthentication no\nUsePAM no\n"
            f"AllowUsers {getpass.getuser()}\nLogLevel ERROR\n"
            f"SetEnv PATH={stubs}:/usr/bin:/bin\n"
        )
        self.process = subprocess.Popen(
            [str(SSHD), "-D", "-e", "-f", str(self.state / "sshd_config")],
            stdout=(self.state / "sshd.log").open("ab"),
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        for _ in range(400):
            try:
                socket.create_connection(("127.0.0.1", self.port), timeout=0.2).close()
                return
            except OSError:
                time.sleep(0.025)
        self.process.kill()
        raise AssertionError("sshd did not start: " + (self.state / "sshd.log").read_text())

    def executor_config(self, tmp_path: Path, label: str) -> Path:
        known_hosts = self.state / "known_hosts"
        known_hosts.write_text(f"[127.0.0.1]:{self.port} {self.host_key}\n")
        config = tmp_path / f"executor-{label}.json"
        config.write_text(
            json.dumps(
                {
                    "run_id": "ab" * 32,
                    "image_repository": "example/image",
                    "state_dir": str(tmp_path / f"executor-state-{label}"),
                    "hosts": [
                        {
                            "label": label,
                            "address": "127.0.0.1",
                            "port": self.port,
                            "user": getpass.getuser(),
                            "identity_file": str(self.state / "client"),
                            "known_hosts_file": str(known_hosts),
                        }
                    ],
                }
            )
        )
        return config

    def stop(self) -> None:
        self.process.terminate()
        self.process.wait(timeout=10)


@pytest.mark.skipif(
    not SSHD.exists() or shutil.which("ssh") is None, reason="needs ssh and /usr/sbin/sshd"
)
def test_qualification_attest_attests_two_hosts_over_real_ssh(tmp_path: Path) -> None:
    # Given: two loopback daemons, each its own host key and GPUs, and results without attestations.
    paths = unattested(tmp_path)
    daemons: list[Daemon] = []
    independent = [
        host_attest.statement_bytes(
            host_attest.HostStatement(
                version=1,
                config_sha256="11" * 32,
                result_sha256=fsha(path),
                image_digest="sha256:" + "44" * 32,
                gpu_uuids=sorted(host_gpus(index)),
                drivers=["595.84"],
                boot_id=boot_id(index),
            )
        )
        for index, path in enumerate(paths)
    ]
    try:
        for index in range(2):
            daemons.append(Daemon(tmp_path, index))
        # When: the operator tool runs once per host; host 1 lets its SSH user read the host key.
        for index, (path, daemon) in enumerate(zip(paths, daemons, strict=True)):
            config = daemon.executor_config(tmp_path, f"host-{index}")
            done = subprocess.run(
                [sys.executable, str(TREE / "scripts/ops/qualification_attest.py")]
                + ["--executor-config", str(config), "--host", f"host-{index}"]
                + ["--result", str(path), "--host-key-path", str(daemon.state / "host")]
                + (["--no-sudo"] if index == 1 else []),
                capture_output=True,
                text=True,
                timeout=120,
                check=False,
            )
            # Then: it prints the fingerprint the owner compares with the provider console...
            assert done.returncode == 0, done.stderr
            known_hosts = daemon.state / "known_hosts"
            assert f"fingerprint {typed_fingerprint(known_hosts)}\n" in done.stdout
            assert (daemon.state / "sudo-used").exists() == (index == 0)
            # ...and the statement is the independent one for that box.
            assert (path.parent / host_attest.ATTESTATION).read_bytes() == independent[index]
        verdict = driver.compare(paths, manifest(), host_keys=[d.host_key for d in daemons])
    finally:
        for daemon in daemons:
            daemon.stop()
    assert verdict == {"two_host_bitwise_match": True, "workload_allowed": False}
