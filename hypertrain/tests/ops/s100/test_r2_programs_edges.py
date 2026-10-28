"""Edges of the challenge program and of `r2-check` that a mutation pass found unpinned.

The exact line an operator reads, which host is asked and how, what the host program measures and
reports, the ranges that keep a hostile number off the screen, what `status` and `why` let out.
Helpers and fixtures are those of test_r2_programs.py and test_r2_check.py.
"""

# ruff: noqa: F811  (the fixtures of test_r2_programs.py are imported by name, as pytest wants)
from __future__ import annotations

import http.server
import io
import json
import os
import re
import ssl
import subprocess
import sys
import threading
import time
from pathlib import Path

import httpx
import pytest
from tests.gpu_ops.artifact_fakes import TimedFakeR2, credentials_file, serve_http

from hypertrain.gpu_ops import artifact_transfer as at
from hypertrain.gpu_ops import cuda_executor as ce
from hypertrain.storage.r2 import NoSuchKey, R2Client, R2Error

from .test_r2_check import DIGEST as CHECK_DIGEST
from .test_r2_check import check
from .test_r2_programs import (  # noqa: F401 - fixtures
    DIGEST,
    EXECUTOR_VAR,
    KEY,
    World,
    certificates,
    functions,
    good_reply,
    host,
    probe,
    refusal,
    store,
    world,
)

# --- what an operator reads ----------------------------------------------------------------------


def test_the_whole_line_a_healthy_host_gives_is_what_the_operator_reads(world, host):
    host.out = json.dumps(good_reply())
    line, ok = probe(world)
    assert ok and line == (
        "x python=3.12.4 openssl=OpenSSL_3.0.13_30_Jan_2024 cafile=1 openssl_cafile=0 ssl_env=0"
        " capath_entries=146 head=200 head_bytes=1048576 head_s=0.400"
        " get=206 get_bytes=1 get_s=0.300 absent=404 absent_bytes=0 absent_s=0.300"
    )


def test_zero_is_shown_and_a_number_outside_its_range_is_not():
    shown = functions()["shown"]
    zero = {"capath_entries": 0, "head": {"status": 200, "bytes": 0, "elapsed_s": 0}}
    assert shown(zero) == "capath_entries=0 head=200 head_bytes=0 head_s=0.000"
    top = {"capath_entries": 10**6, "head": {"status": 599, "bytes": 1 << 40, "elapsed_s": 1000}}
    assert shown(top) == "capath_entries=1000000 head=599 head_bytes=1099511627776 head_s=1000.000"
    assert shown({"head": {"status": 100}}) == "head=100"
    hostile = {
        "capath_entries": 10**6 + 1,
        "head": {"status": 99, "bytes": float("inf"), "elapsed_s": float("nan")},
        "get": {"status": 600, "bytes": (1 << 40) + 1, "elapsed_s": 1000.5},
    }
    assert shown(hostile) == ""


@pytest.mark.parametrize("word", ["x;rm", "a b", "a.b", "\x1b[2J", "http://h", "A" * 41])
def test_a_short_hostile_word_is_dropped_where_a_class_name_is_expected(word):
    shown = functions()["shown"]
    assert shown({"error": word, "head": {"error": word}, "get": {"error": word}}) == ""


@pytest.mark.parametrize("version", ["x;rm", "\x1b[2J", "http://h", "a/b", "A" * 41])
def test_a_short_hostile_version_is_dropped(version):
    assert functions()["shown"]({"openssl": version}) == ""


def test_a_status_wins_over_an_error_in_the_same_answer():
    assert functions()["shown"]({"head": {"status": 200, "error": "OSError"}}) == "head=200"


def test_a_host_that_could_not_run_the_program_at_all_says_which_error():
    shown = functions()["shown"]
    assert shown({"error": "OSError"}) == "error=OSError"
    assert shown({"cafile": 5, "ssl_env": 12345678901234567890, "openssl_cafile": "1"}) == ""
    verdict = {"head": {"error": "SSLCertVerificationError", "verify_code": 101}}
    assert shown(verdict) == "head=SSLCertVerificationError"
    assert shown({"head": {"status": 200, "bytes": 5.0, "elapsed_s": 0.4}}) == (
        "head=200 head_bytes=5 head_s=0.400"
    )


def test_status_is_an_http_status_or_a_class_name_and_never_a_message():
    status = functions()["status"]
    assert status(R2Error("PUT exec/k: HTTP 403")) == "403"
    assert status(R2Error("PUT exec/k: ConnectError")) == "ConnectError"
    assert status(NoSuchKey("GET exec/k: HTTP 404")) == "NoSuchKey"
    assert status(R2Error("GET exec/k: HTTP 206, 0 bytes")) == "R2Error"
    assert status(R2Error("PUT exec/k: HTTP 12345")) == "R2Error"
    assert status(ValueError("sentinel")) == "ValueError"


def test_pre_names_every_missing_setting_and_joins_every_validation_error(world):
    world.settings["workers"] = "x"
    world.settings["unknown-key"] = 1
    assert len(refusal(world.pre()).split("; ")) == 2
    world.settings.pop("workers"), world.settings.pop("unknown-key")
    kept = [
        x
        for x in world.token.read_text().splitlines()
        if not x.startswith(("R2_ACCOUNT_ID", "R2_BUCKET"))
    ]
    world.token.write_text("\n".join(kept) + "\n")
    assert refusal(world.pre()) == "missing R2 settings: R2_ACCOUNT_ID, R2_BUCKET"


def test_a_validation_error_of_the_whole_model_has_no_place_to_name(world):
    world.settings["max_artifact_bytes"] = 10_000 * (16 << 20) + 1  # more parts than a store takes
    reason = refusal(world.pre())
    assert reason.startswith("Value error, max_artifact_bytes needs more than 10,000 parts")


def test_a_validation_error_in_a_list_names_the_item(world):
    world.settings["operations"] = ["bogus"]
    assert refusal(world.pre()).startswith("operations.0: ")


# --- the round trip ------------------------------------------------------------------------------


class Slow(TimedFakeR2):
    def __call__(self, req):
        time.sleep(0.3)
        return super().__call__(req)


def test_the_seconds_printed_are_the_seconds_it_took(tmp_path):
    fake = Slow()
    front = serve_http(fake)
    try:
        world = World(tmp_path / "executor")
        world.token = credentials_file(world.root, front.base)
        done = world.run("roundtrip", str(world.token))
    finally:
        front.close()
    seconds = [float(x) for x in re.findall(r"_s=(\d+\.\d{3})", done.stdout)]
    assert len(seconds) == 3 and all(0.3 <= s < 20 for s in seconds), done.stdout


class Lossy(TimedFakeR2):
    """A store that deletes, and then answers that the delete failed."""

    def __call__(self, req):
        response = super().__call__(req)
        return httpx.Response(500) if req.method == "DELETE" else response


def test_a_delete_that_is_reported_failed_fails_the_check_though_the_object_is_gone(tmp_path):
    fake = Lossy()
    front = serve_http(fake)
    try:
        world = World(tmp_path / "executor")
        world.token = credentials_file(world.root, front.base)
        done = world.run("roundtrip", str(world.token))
    finally:
        front.close()
    assert done.returncode == 1 and "delete=500 absent=404" in done.stdout
    assert fake.objects == {}


def test_the_check_object_has_a_name_that_cannot_collide(store):
    fake, _, world = store
    world.run("roundtrip", str(world.token))
    (key,) = {row[1] for row in fake.ledger if row[0] != "HeadObject"}
    assert re.fullmatch(r"exec/_r2-check/[0-9a-f]{32}", key)


def test_a_read_that_failed_is_not_followed_by_a_look_from_the_host(store):
    fake, _, world = store
    fake.fault_key("GetObject", "", 403)
    log = world.root / "ssh.log"
    (world.bin / "ssh").write_text(f"#!/bin/sh\necho called >> {log}\nexit 0\n")
    env = {EXECUTOR_VAR: str(world.config_path)}
    done = world.run("roundtrip", str(world.token), "x", DIGEST, env=env)
    assert done.returncode == 1 and "get=403" in done.stdout
    assert "r2-check node" not in done.stdout and not log.exists()


def test_the_urls_the_host_gets_are_each_good_for_their_own_request_only(store, host):
    # the store checks the signature of a presigned URL against the method it is used with
    fake, _, world = store
    client = R2Client(at.load_credentials(world.token), client=httpx.Client(trust_env=False))
    client.put_object(KEY, b"x" * (1 << 20))
    host.out = json.dumps(good_reply())
    program = functions()
    program["probe"](client, KEY, program["resolve"]("x", DIGEST))
    head, get, absent = host.runs[0]["stdin"].split()
    assert httpx.head(head).status_code == 200
    assert httpx.get(get, headers={"Range": "bytes=0-0"}).status_code in (200, 206)
    assert httpx.get(absent).status_code == 404  # signed, and for a key nobody wrote
    assert httpx.get(head).status_code == 403 and httpx.head(get).status_code == 403
    assert httpx.get(absent.replace("-absent", "")).status_code == 403  # not good for the object


# --- which host is asked, and how ----------------------------------------------------------------


class Choosing:
    """SshTransport double that remembers which host it was built for."""

    def __call__(self, host, config, ssh):
        self.chosen = host
        return self

    def run(self, command, *, stdin, stdout, stderr, deadline, cancel, max_stdout_bytes):
        Path(stdout).write_text(json.dumps(good_reply()))
        Path(stderr).write_text("")
        self.command, self.deadline = command, deadline
        return 0


def two_hosts(world, monkeypatch):
    fake = Choosing()
    monkeypatch.setattr(ce, "SshTransport", fake)
    monkeypatch.setenv(EXECUTOR_VAR, str(world.config_path))
    first = json.loads(world.config_path.read_text())["hosts"][0]
    second = first | {"label": "gpu-ab", "address": "203.0.113.9", "docker": ["sudo", "docker"]}
    world.write_config(hosts=[first | {"label": "gpu-a"}, second])
    return fake


def test_the_host_asked_is_the_one_with_that_label_and_its_own_docker(world, monkeypatch):
    fake = two_hosts(world, monkeypatch)
    before = time.time()
    probe(world, "gpu-ab")
    assert (fake.chosen.label, fake.chosen.address) == ("gpu-ab", "203.0.113.9")
    assert fake.command.startswith("sudo docker run ")
    assert before + 60 < fake.deadline < time.time() + 120
    probe(world, "gpu-a")
    assert fake.chosen.address == "203.0.113.5" and fake.command.startswith("docker run ")


@pytest.mark.parametrize("label", ["gpu", "gpu-abc", "g", "gpu-a "])
def test_a_label_that_is_part_of_a_host_label_or_holds_one_is_not_that_host(
    world, monkeypatch, label
):
    two_hosts(world, monkeypatch)
    with pytest.raises(Exception, match="no executor host has this label"):
        probe(world, label)


@pytest.mark.parametrize(
    "reply",
    [
        good_reply() | {"head": {"status": 200, "bytes": (1 << 20) + 1, "elapsed_s": 0.1}},
        {k: v for k, v in good_reply().items() if k != "get"},
        good_reply() | {"get": None},
    ],
    ids=["head-bigger", "get-missing", "get-null"],
)
def test_the_host_does_not_work_when_the_object_or_an_answer_is_not_as_written(world, host, reply):
    host.out = json.dumps(reply)
    assert probe(world)[1] is False


def test_a_reply_that_is_not_text_is_no_reply(world, host):
    host.out = b"\xff\xfe{"  # bytes that are no UTF-8
    assert probe(world) == ("x no_reply", False)


# --- the program on the host ---------------------------------------------------------------------


def on_host(urls: str, env: dict[str, str]) -> dict:
    done = subprocess.run(
        [sys.executable, "-I", "-c", functions()["NODE_PY"]],
        input=urls, capture_output=True, text=True,
        env={"PATH": "/usr/bin:/bin", **env}, check=False, timeout=60,
    )  # fmt: skip
    assert done.returncode == 0 and done.stderr == "", done
    return json.loads(done.stdout)


def test_the_host_reports_the_ca_paths_and_variables_it_was_started_with(tmp_path):
    cafile = tmp_path / "ca.pem"
    cafile.write_text("x")
    capath = tmp_path / "ca"
    capath.mkdir()
    for n in "abc":
        (capath / n).write_text("x")
    both = on_host("", {"SSL_CERT_FILE": str(cafile), "SSL_CERT_DIR": str(capath)})
    assert (both["cafile"], both["capath_entries"], both["ssl_env"]) == (True, 3, True)
    only_dir = on_host("", {"SSL_CERT_DIR": str(capath)})
    assert (only_dir["capath_entries"], only_dir["ssl_env"]) == (3, True)
    assert on_host("", {"SSL_CERT_FILE": str(cafile)})["ssl_env"] is True
    none = on_host("", {})
    assert none["ssl_env"] is False
    paths = ssl.get_default_verify_paths()
    assert none["openssl"] == ssl.OPENSSL_VERSION
    assert none["openssl_cafile"] is os.path.exists(paths.openssl_cafile or "")
    assert none["cafile"] is bool(paths.cafile)
    gone = on_host("", {"SSL_CERT_FILE": str(tmp_path / "missing")})
    assert gone["cafile"] is False


def test_the_host_reports_whether_the_default_ca_file_of_its_openssl_exists(
    tmp_path, monkeypatch, capsys
):
    cert = tmp_path / "cert.pem"
    cert.write_text("x")
    for exists in (True, False):
        paths = ssl.DefaultVerifyPaths(
            cafile=None, capath=None, openssl_cafile_env="SSL_CERT_FILE",
            openssl_cafile=str(cert if exists else tmp_path / "missing.pem"),
            openssl_capath_env="SSL_CERT_DIR", openssl_capath="",
        )  # fmt: skip
        monkeypatch.setattr(ssl, "get_default_verify_paths", lambda paths=paths: paths)
        monkeypatch.setattr(sys, "stdin", io.StringIO(""))
        exec(compile(functions()["NODE_PY"], "NODE_PY", "exec"), {})  # noqa: S102
        out = json.loads(capsys.readouterr().out)
        assert out["openssl_cafile"] is exists and out["cafile"] is False
        assert out["capath_entries"] == 0


def test_the_elapsed_time_of_a_request_is_measured_and_a_header_less_answer_has_zero_bytes(
    tmp_path,
):
    certificates(tmp_path)
    seen: list[str] = []

    class Handler(http.server.BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def do_HEAD(self):
            seen.append(self.path)
            time.sleep(0.3)
            self.send_response(200)
            self.send_header("content-length", "5")
            self.end_headers()

        def do_GET(self):
            seen.append(self.path)
            self.send_response(204)  # no content-length at all
            self.send_header("connection", "close")
            self.end_headers()

        def log_message(self, *args):
            return

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.load_cert_chain(tmp_path / "self.pem", tmp_path / "self.key")
    server.socket = context.wrap_socket(server.socket, server_side=True)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        port = server.server_address[1]
        query = "?X-Amz-Algorithm=AWS4&X-Amz-Signature=" + "0" * 8
        url = f"https://localhost:{port}/b/k{query}"
        reply = on_host(f"{url}\n{url}\n", {"SSL_CERT_FILE": str(tmp_path / "self.pem")})
    finally:
        server.shutdown()
        server.server_close()
        thread.join(10)
    assert 0.3 <= reply["head"]["elapsed_s"] < 20 and reply["get"]["elapsed_s"] < 20
    assert reply["get"] == {"status": 204, "bytes": 0, "elapsed_s": reply["get"]["elapsed_s"]}
    assert seen == [f"/b/k{query}"] * 2  # the whole query went out, signature included


# --- the script around the program ---------------------------------------------------------------


def test_the_digest_must_start_with_sha256_and_a_32_character_label_is_a_label(box):
    box.token()
    out = check(box, "--node", "gpu-a", "x" + CHECK_DIGEST, rc=2).out
    assert out.startswith("usage: ")
    label = "a" * 32
    check(box, "--node", label, CHECK_DIGEST)
    (call,) = box.exec_calls()
    assert call["args"] == ["-", label, CHECK_DIGEST]


@pytest.mark.parametrize(
    "line",
    [
        "\x1b[31mr2-check challenge MARKER",
        "x r2-check challenge MARKER",
        "r2-check challenge MARKER \x1b[2J",
        "r2-check MARKER " + "a" * 600,
    ],
    ids=["escape-before", "text-before", "escape-inside", "601-characters"],
)
def test_a_line_that_is_not_a_plain_sentence_of_the_program_is_not_printed(box, line):
    box.exec_out("roundtrip", 1, line)
    assert "MARKER" not in check(box, rc=8).out


def test_a_sentence_of_exactly_600_characters_is_printed(box):
    line = "r2-check " + "a" * 600
    box.exec_out("roundtrip", 1, line)
    assert line in check(box, rc=8).out


def test_an_empty_file_value_is_a_usage_error_and_not_the_configured_token(box):
    check(box, "--file", "", rc=2)
    assert box.exec_calls() == []
