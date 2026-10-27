"""SSH host-key attestation primitives; real `ssh-keygen` with throw-away keys, no network."""

from __future__ import annotations

import base64
import json
import os
import subprocess
from pathlib import Path
from typing import Any

import pytest
from pydantic import ValidationError

from hypertrain.gpu_ops import host_attest
from hypertrain.gpu_ops.launcher import Reject

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
