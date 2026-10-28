"""The program challenge_image.sh runs inside the challenge container, run for real.

test_executor_r2.py canned its answers; here `python -I -c CHALLENGE_PY` reads real files with the
real loader (hypertrain.gpu_ops.artifact_transfer), as the user of the test. What it prints is an
operator's screen: no value of the token file, no byte of it, no path of a URL.
"""

from __future__ import annotations

import http.server
import json
import os
import re
import shlex
import shutil
import socket
import ssl
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

import httpx
import pytest
from tests.gpu_ops.artifact_fakes import BUCKET, TimedFakeR2, credentials_file, serve_http
from tests.storage.fakes import CFG, SECRET

from hypertrain.gpu_ops import artifact_transfer as at
from hypertrain.gpu_ops import cuda_executor as ce
from hypertrain.storage.r2 import R2Client

from .sandbox import ARTIFACTS_VAR, EXECUTOR_VAR, program

ENDPOINT = "https://sentinel-endpoint.example.test"
VALUES = [SECRET, CFG.access_key_id, BUCKET, "sentinel-endpoint"]


class World:
    """A directory like /data/executor: a token, the executor config and the settings."""

    def __init__(self, root: Path) -> None:
        self.root = root
        root.mkdir()
        root.chmod(0o700)
        self.bin = root / "bin"
        self.bin.mkdir()
        (self.bin / "ssh").write_text("#!/bin/sh\nexit 0\n")
        (self.bin / "ssh").chmod(0o755)
        self.token = credentials_file(root, ENDPOINT)
        (root / "id").write_text("k")
        (root / "id").chmod(0o600)
        (root / "hosts").write_text("host key")
        self.config_path = root / "cuda-executor.json"
        self.write_config()
        self.settings = {"credentials_file": str(self.token), "operations": ["audit", "referee"]}

    def write_config(self, **over: object) -> None:
        config = {
            "run_id": "0" * 64,
            "image_repository": "ghcr.io/cortexlm/cortex/hypertrain-gpu",
            "state_dir": str(self.root / "state"),
            "hosts": [
                {
                    "label": "x",
                    "address": "203.0.113.5",
                    "user": "ubuntu",
                    "identity_file": str(self.root / "id"),
                    "known_hosts_file": str(self.root / "hosts"),
                }
            ],
        }
        self.config_path.write_text(json.dumps(config | over))

    @property
    def raw(self) -> bytes:
        return json.dumps(self.settings).encode()

    def run(
        self, *args: str, stdin: bytes | None = None, env: dict[str, str] | None = None
    ) -> subprocess.CompletedProcess[str]:
        run_env = {"PATH": str(self.bin), **(env or {})}
        done = subprocess.run(
            [sys.executable, "-I", "-c", program("CHALLENGE_PY"), *args],
            input=(stdin or b"").decode("latin-1"),
            capture_output=True,
            text=True,
            env=run_env,
            check=False,
            timeout=120,
        )
        for text in (done.stdout, done.stderr):
            leaked = [v for v in VALUES if v in text]
            assert not leaked, f"{leaked} reached the operator: {text!r}"
        return done

    def pre(self, **kwargs: object) -> subprocess.CompletedProcess[str]:
        return self.run("pre", str(self.config_path), stdin=self.raw, **kwargs)  # type: ignore[arg-type]

    def post(self, config: str, settings: str, **env: str) -> subprocess.CompletedProcess[str]:
        return self.run("post", config, settings, env=env)


@pytest.fixture
def world(tmp_path: Path) -> World:
    return World(tmp_path / "executor")


def refusal(done: subprocess.CompletedProcess[str]) -> str:
    assert done.returncode == 1 and done.stderr == "", done
    (line,) = done.stdout.splitlines()
    assert line.startswith(("preflight refused: ", "postflight refused: ", "r2-check refused: "))
    return line.split("refused: ", 1)[1]


# --- pre ------------------------------------------------------------------------------------------


def test_pre_accepts_what_the_challenge_would_load(world, monkeypatch):
    done = world.pre()
    assert (done.returncode, done.stdout, done.stderr) == (0, "preflight ok\n", "")
    assert constructor_accepts(world, monkeypatch)  # the real chain takes the same files


def test_pre_builds_nothing_it_could_harm_the_live_challenge_with(world):
    world.pre()
    assert not (world.root / "state").exists()  # an Executor would have made it, and a Journal


def corrupt_mode(w: World) -> None:
    w.token.chmod(0o644)


def corrupt_link(w: World) -> None:
    real = w.token.with_name("real.env")
    w.token.rename(real)
    w.token.symlink_to(real)


def corrupt_extra_name(w: World) -> None:
    w.token.write_text(w.token.read_text() + "CLOUDFLARE_API_TOKEN=sentinel-account-token\n")


def corrupt_missing_name(w: World) -> None:
    kept = [x for x in w.token.read_text().splitlines() if not x.startswith("R2_ACCOUNT_ID")]
    w.token.write_text("\n".join(kept) + "\n")


def corrupt_prefix(w: World) -> None:
    w.token.write_text(w.token.read_text().replace(BUCKET, "production-bucket"))


def corrupt_bucket_name(w: World) -> None:
    w.token.write_text(w.token.read_text().replace(BUCKET, "cortex-v3-Bad_Name"))


def corrupt_plain_http(w: World) -> None:
    w.token.write_text(
        w.token.read_text().replace(ENDPOINT, "http://sentinel-endpoint.example.test")
    )


def corrupt_bytes(w: World) -> None:
    w.token.write_bytes(b"\xff\xfeR2_SECRET_ACCESS_KEY=" + SECRET.encode())


def corrupt_gone(w: World) -> None:
    w.token.unlink()


def corrupt_settings_key(w: World) -> None:
    w.settings["workers"] = "sentinel-not-a-number"
    w.settings["sentinel-extra-key"] = 1


def corrupt_network(w: World) -> None:
    w.write_config(network="none")


def corrupt_big_artifact(w: World) -> None:
    w.settings["max_artifact_bytes"] = (16 << 30) + 1


def corrupt_small_result(w: World) -> None:
    w.write_config(max_result_bytes=1 << 30)


def corrupt_operations(w: World) -> None:
    w.settings["operations"] = ["audit", "audit"]


def corrupt_config_gone(w: World) -> None:
    w.config_path.unlink()


LOADER = "credentials_file must be a regular file of this user with mode 0600"
REFUSED = {
    "mode": (corrupt_mode, LOADER),
    "link": (corrupt_link, LOADER),
    "extra-name": (corrupt_extra_name, "credentials_file holds names other than the R2 settings"),
    "missing-name": (corrupt_missing_name, "missing R2 settings: R2_ACCOUNT_ID"),
    "bucket-prefix": (corrupt_prefix, "the artifact bucket must be a cortex-v3-* bucket"),
    "bucket-name": (corrupt_bucket_name, "invalid bucket name"),
    "plain-http": (corrupt_plain_http, "the R2 endpoint must be https"),
    "not-utf8": (corrupt_bytes, "UnicodeDecodeError"),
    "token-gone": (corrupt_gone, "FileNotFoundError: "),
    "settings-key": (corrupt_settings_key, "workers: Input should be a valid integer"),
    "network-none": (corrupt_network, "artifacts need network 'default'"),
    "big-artifact": (corrupt_big_artifact, "max_artifact_bytes is above the executor's"),
    "small-result": (corrupt_small_result, "max_artifact_bytes is above the executor's"),
    "repeated-operations": (corrupt_operations, "operations must be a non-empty list"),
    "config-gone": (corrupt_config_gone, "FileNotFoundError: "),
}


@pytest.mark.parametrize(("corrupt", "said"), REFUSED.values(), ids=REFUSED)
def test_pre_refuses_what_the_loader_refuses_and_says_so_without_quoting_the_file(
    world, monkeypatch, corrupt, said
):
    corrupt(world)
    reason = refusal(world.pre())
    assert said in reason
    assert "sentinel" not in reason and "\\xff" not in reason and "0xff" not in reason
    assert not constructor_accepts(world, monkeypatch)  # and so does the chain the challenge runs


def test_pre_does_not_echo_a_settings_document_it_cannot_parse(world):
    done = world.run("pre", str(world.config_path), stdin=b'{"credentials_file": "sentinel-path')
    assert "sentinel" not in refusal(done)


def test_pre_does_not_listen_to_the_proxy_variables_the_transfer_ignores(world):
    # the transfer's client has trust_env=False; one that read a proxy variable would refuse this
    done = world.pre(env={"HTTPS_PROXY": "ftp://proxy", "ALL_PROXY": "ftp://proxy"})
    assert (done.returncode, done.stdout) == (0, "preflight ok\n")


def test_pre_does_not_print_the_name_of_a_key_of_the_settings_document(world):
    world.settings["sentinel-extra-key"] = 1
    reason = refusal(world.pre())
    assert reason == "?: Extra inputs are not permitted"


def test_pre_prints_the_path_of_a_file_it_cannot_open_on_one_line_and_in_ascii(world):
    world.settings["credentials_file"] = str(world.root / "no\nsuch\u00e9.env")
    reason = refusal(world.pre())  # refusal() wants exactly one line
    assert reason == f"FileNotFoundError: {f'{world.root}/no?such?.env'[:120]}"


def test_pre_names_the_file_a_config_points_at_that_is_not_there(world):
    (world.root / "id").unlink()
    assert refusal(world.pre()) == f"FileNotFoundError: {f'{world.root}/id'[:120]}"


def test_pre_refuses_a_challenge_without_an_ssh_client(world):
    done = world.run("pre", str(world.config_path), stdin=world.raw, env={"PATH": "/nonexistent"})
    assert refusal(done) == "FileNotFoundError"


# --- the same verdict as the constructor ----------------------------------------------------------


def constructor_accepts(world: World, monkeypatch, *, ssh: bool = True) -> bool:
    """Whether the loader chain the challenge runs (`artifact_transfer.build`) takes these files."""
    monkeypatch.setattr(shutil, "which", lambda name: "/bin/ssh" if ssh else None)
    try:
        config = ce.ExecutorConfig.model_validate_json(world.config_path.read_bytes())
        settings = at.ArtifactSettings.model_validate_json(world.raw)
        at.ArtifactExecutor(config, settings, at.load_credentials(settings.credentials_file))
    except Exception:  # noqa: BLE001 - every kind of refusal is a refusal
        return False
    return True


def test_pre_and_the_real_constructor_agree_when_there_is_no_ssh_client(world, monkeypatch):
    refused = world.run(
        "pre", str(world.config_path), stdin=world.raw, env={"PATH": "/nonexistent"}
    )
    assert (refused.returncode == 0) == constructor_accepts(world, monkeypatch, ssh=False)
    assert refused.returncode == 1


# --- post -----------------------------------------------------------------------------------------


def test_post_accepts_the_variables_and_the_files_the_script_wrote(world):
    settings = world.root / "r2-artifacts.json"
    settings.write_bytes(world.raw)
    done = world.post(
        str(world.config_path),
        str(settings),
        **{EXECUTOR_VAR: str(world.config_path), ARTIFACTS_VAR: str(settings)},
    )
    assert (done.returncode, done.stdout) == (0, "postflight ok\n")


def test_post_accepts_a_challenge_without_either_variable_when_asked_for_none(world):
    done = world.post("-", "-")
    assert (done.returncode, done.stdout) == (0, "postflight ok\n")


@pytest.mark.parametrize(
    ("env", "named"),
    [
        ({}, EXECUTOR_VAR),
        ({EXECUTOR_VAR: "sentinel-other-path"}, EXECUTOR_VAR),
        ({EXECUTOR_VAR: "CONFIG"}, ARTIFACTS_VAR),
        ({EXECUTOR_VAR: "CONFIG", ARTIFACTS_VAR: "sentinel-other-path"}, ARTIFACTS_VAR),
        ({EXECUTOR_VAR: "CONFIG", ARTIFACTS_VAR: ""}, ARTIFACTS_VAR),
    ],
    ids=["nothing-set", "other-config", "no-settings", "other-settings", "empty-settings"],
)
def test_post_names_the_variable_that_is_not_as_asked_and_never_its_value(world, env, named):
    env = {k: (str(world.config_path) if v == "CONFIG" else v) for k, v in env.items()}
    reason = refusal(world.post(str(world.config_path), "/data/executor/r2-artifacts.json", **env))
    assert reason == f"{named} is not set as the script wrote it"


@pytest.mark.parametrize("var", [EXECUTOR_VAR, ARTIFACTS_VAR])
def test_post_for_no_variables_refuses_one_that_is_set(world, var):
    reason = refusal(world.post("-", "-", **{var: "sentinel-path"}))
    assert reason == f"{var} is not set as the script wrote it"


def test_post_reads_the_settings_file_as_the_challenge_would(world):
    settings = world.root / "r2-artifacts.json"  # not written: the variable points at nothing
    done = world.post(
        str(world.config_path),
        str(settings),
        **{EXECUTOR_VAR: str(world.config_path), ARTIFACTS_VAR: str(settings)},
    )
    assert refusal(done) == f"FileNotFoundError: {str(settings)[:120]}"  # the path, cut at 120
    settings.write_bytes(world.raw)
    world.token.chmod(0o644)
    done = world.post(
        str(world.config_path),
        str(settings),
        **{EXECUTOR_VAR: str(world.config_path), ARTIFACTS_VAR: str(settings)},
    )
    assert refusal(done) == LOADER


def test_an_unknown_mode_is_a_refusal_not_a_traceback(world):
    done = world.run("bogus")
    assert done.returncode == 1 and done.stderr == ""
    assert done.stdout == "program refused: KeyError\n"


# --- roundtrip ------------------------------------------------------------------------------------

LINE = (
    r"r2-check challenge put=200 get=200 delete=204 absent=404 bytes=1048576"
    r" put_s=\d+\.\d{3} get_s=\d+\.\d{3} delete_s=\d+\.\d{3}"
)
PROXIES = {name: "http://127.0.0.1:9" for name in ("HTTP_PROXY", "http_proxy", "ALL_PROXY")}


class Flipping(TimedFakeR2):
    """A store that answers a read of the check object with other bytes."""

    def __call__(self, req: httpx.Request) -> httpx.Response:
        response = super().__call__(req)
        if req.method == "GET" and "_r2-check" in req.url.path and response.status_code == 200:
            return httpx.Response(200, content=b"\0" * len(response.content))
        return response


class Recording(TimedFakeR2):
    """A store that keeps what it was asked to write, though the check deletes it."""

    def __init__(self) -> None:
        super().__init__()
        self.written: list[tuple[str, bytes]] = []

    def __call__(self, req: httpx.Request) -> httpx.Response:
        if req.method == "PUT":
            self.written.append((req.url.path, bytes(req.content)))
        return super().__call__(req)


@pytest.fixture
def store(world):
    fake = TimedFakeR2()
    front = serve_http(fake)
    world.token = credentials_file(world.root, front.base)
    world.settings["credentials_file"] = str(world.token)
    (world.root / "r2-artifacts.json").write_bytes(world.raw)
    yield fake, front, world
    front.close()


def configured(world: World) -> dict[str, str]:
    return {ARTIFACTS_VAR: str(world.root / "r2-artifacts.json")}


def test_a_round_trip_puts_reads_deletes_and_asks_for_the_absent_key(store):
    fake, front, world = store
    done = world.run("roundtrip", str(world.token))
    assert done.returncode == 0 and done.stderr == ""
    assert re.fullmatch(LINE + "\n", done.stdout), done.stdout
    ops = [(row[0], row[2]) for row in fake.ledger]
    # the last request is what Broker.begin asks first, of a key that is not there: 404 is "absent"
    assert ops == [
        ("PutObject", 200),
        ("GetObject", 200),
        ("DeleteObject", 204),
        ("HeadObject", 404),
    ]
    keys = {row[1] for row in fake.ledger}
    assert len(keys) == 1 and next(iter(keys)).startswith("exec/_r2-check/")
    assert (fake.ledger[0][3], fake.ledger[1][4]) == (1 << 20, 1 << 20)
    assert fake.objects == {}  # nothing is left in the bucket
    for text in (next(iter(keys)), "exec/", "127.0.0.1", front.base):
        assert text not in done.stdout


def test_two_round_trips_write_other_bytes_to_other_keys(tmp_path):
    fake = Recording()
    front = serve_http(fake)
    try:
        world = World(tmp_path / "executor")
        world.token = credentials_file(world.root, front.base)
        for _ in range(2):
            assert world.run("roundtrip", str(world.token)).returncode == 0
    finally:
        front.close()
    (key1, data1), (key2, data2) = fake.written
    # one mebibyte of the same bytes could be deduplicated or compressed on its way
    assert key1 != key2 and data1 != data2 and len(data1) == len(data2) == 1 << 20
    assert len(set(data1)) > 200


def test_a_round_trip_without_a_file_uses_the_token_the_settings_of_the_process_name(store):
    fake, _, world = store
    done = world.run("roundtrip", "-", env=configured(world))
    assert done.returncode == 0 and "put=200 get=200 delete=204 absent=404" in done.stdout
    assert len(fake.ledger) == 4


def test_a_round_trip_with_nothing_configured_says_what_is_missing(store):
    _, _, world = store
    assert refusal(world.run("roundtrip", "-")) == f"{ARTIFACTS_VAR} is not set"


def test_a_round_trip_does_not_listen_to_the_proxy_variables_the_transfer_ignores(store):
    _, _, world = store
    done = world.run("roundtrip", str(world.token), env=PROXIES)
    assert done.returncode == 0 and "put=200 get=200 delete=204" in done.stdout


def test_a_store_that_is_down_is_a_class_name_and_a_skipped_read(store):
    _, _, world = store
    with socket.socket() as dead:  # bound and never listening: refused by construction
        dead.bind(("127.0.0.1", 0))
        world.token = credentials_file(world.root, f"http://127.0.0.1:{dead.getsockname()[1]}")
        done = world.run("roundtrip", str(world.token))
    assert done.returncode == 1 and done.stderr == ""
    assert re.fullmatch(
        r"r2-check challenge put=ConnectError get=skipped delete=ConnectError absent=ConnectError"
        r" bytes=1048576 put_s=\d+\.\d{3} delete_s=\d+\.\d{3}\n",
        done.stdout,
    ), done.stdout


@pytest.mark.parametrize("status", [403, 500])
def test_a_refused_put_is_its_http_status_and_the_key_stays_out_of_the_line(store, status):
    fake, _, world = store
    fake.fault_key("PutObject", "", status)
    done = world.run("roundtrip", str(world.token))
    assert done.returncode == 1
    assert f"put={status} get=skipped delete=204 absent=404" in done.stdout
    assert "exec/" not in done.stdout


def test_a_read_that_returns_other_bytes_is_a_mismatch_and_the_object_is_still_deleted(tmp_path):
    fake = Flipping()
    front = serve_http(fake)
    try:
        world = World(tmp_path / "executor")
        world.token = credentials_file(world.root, front.base)
        done = world.run("roundtrip", str(world.token))
    finally:
        front.close()
    assert done.returncode == 1 and "put=200 get=mismatch delete=204 absent=404" in done.stdout
    assert fake.objects == {}


def test_a_delete_that_fails_is_reported_and_the_object_is_seen_to_be_still_there(store):
    fake, _, world = store
    fake.fault_key("DeleteObject", "", 403)
    done = world.run("roundtrip", str(world.token))
    assert done.returncode == 1 and "put=200 get=200 delete=403 absent=present" in done.stdout


def test_a_store_that_does_not_say_404_for_an_absent_key_is_the_silent_fallback_to_ssh(store):
    # a token that cannot list gets 403 for a key that is not there; Broker.begin takes that for
    # "the store is unavailable" and, with fallback classic, runs the attempt over ssh unannounced
    fake, _, world = store
    fake.fault_key("HeadObject", "", 403)
    done = world.run("roundtrip", str(world.token))
    assert done.returncode == 1 and "put=200 get=200 delete=204 absent=403" in done.stdout


def test_a_round_trip_refuses_the_token_the_loader_refuses(store):
    _, _, world = store
    world.token.chmod(0o644)
    assert refusal(world.run("roundtrip", str(world.token))) == LOADER


@pytest.mark.parametrize("env", [{}, {EXECUTOR_VAR: "UNSET"}], ids=["no-config", "no-such-host"])
def test_a_host_that_is_not_there_is_refused_before_the_bucket_is_touched(store, env):
    fake, _, world = store
    env = {k: str(world.config_path) for k, v in env.items()}
    done = world.run("roundtrip", str(world.token), "sentinel-label", DIGEST, env=env)
    reason = refusal(done)
    assert reason in {f"{EXECUTOR_VAR} is not set", NO_HOST} and "sentinel-label" not in reason
    assert fake.ledger == []


# --- the host's side ------------------------------------------------------------------------------

DIGEST = "sha256:" + "ab" * 32
NO_HOST = "no executor host has this label, or the image digest is not sha256:<64 hex>"
KEY = "exec/_r2-check/0123456789abcdef0123456789abcdef"
SIGNATURE = re.compile(r"X-Amz-Signature=[0-9a-f]{64}")


def functions() -> dict:
    """The program's functions, without running it (its last lines are the entry point)."""
    source = program("CHALLENGE_PY").split("\nmode = sys.argv[1]")[0]
    names: dict = {"__name__": "challenge_program"}
    exec(compile(source, "CHALLENGE_PY", "exec"), names)  # noqa: S102 - the program under test
    return names


class Host:
    """Stands in for SshTransport: records what the program asks of the host, answers as told."""

    def __init__(self) -> None:
        self.runs: list[dict] = []
        self.out, self.err, self.code, self.raises = "", "", 0, None

    def __call__(self, host, config, ssh):
        self.ssh = ssh
        return self

    def run(self, command, *, stdin, stdout, stderr, deadline, cancel, max_stdout_bytes):
        assert os.readlink(stdin).startswith("/memfd:"), "the URLs are in a file"
        self.runs.append(
            {"command": command, "stdin": Path(stdin).read_text(), "deadline": deadline,
             "cancel": cancel, "max_stdout_bytes": max_stdout_bytes}
        )  # fmt: skip
        Path(stdout).write_text(self.out)
        Path(stderr).write_text(self.err)
        if self.raises is not None:
            raise self.raises
        return self.code


@pytest.fixture
def host(world, monkeypatch, tmp_path):
    fake = Host()
    monkeypatch.setattr(ce, "SshTransport", fake)
    monkeypatch.setenv(EXECUTOR_VAR, str(world.config_path))
    scratch = tmp_path / "tmp"
    scratch.mkdir()
    monkeypatch.setattr(tempfile, "tempdir", str(scratch))
    return fake


def good_reply() -> dict:
    return {
        "python": "3.12.4", "openssl": "OpenSSL 3.0.13 30 Jan 2024", "cafile": True,
        "openssl_cafile": False, "capath_entries": 146, "ssl_env": False,
        "head": {"status": 200, "bytes": 1 << 20, "elapsed_s": 0.4},
        "get": {"status": 206, "bytes": 1, "elapsed_s": 0.3},
        "absent": {"status": 404, "bytes": 0, "elapsed_s": 0.3},
    }  # fmt: skip


def probe(world: World, label: str = "x", digest: str = DIGEST):
    client = R2Client(at.load_credentials(world.token), client=httpx.Client(trust_env=False))
    program_ = functions()
    return program_["probe"](client, KEY, program_["resolve"](label, digest))


def test_the_host_gets_the_urls_on_its_stdin_and_a_command_that_has_none(world, host):
    host.out = json.dumps(good_reply())
    line, ok = probe(world)
    (run,) = host.runs
    argv = shlex.split(run["command"])
    image = f"ghcr.io/cortexlm/cortex/hypertrain-gpu@{DIGEST}"
    assert argv[:2] == ["docker", "run"] and argv.index(image) == len(argv) - 4
    assert argv[-3:-1] == ["-I", "-c"] and argv[-1] == functions()["NODE_PY"]
    options = argv[2 : argv.index(image)]  # everything after the image is the program's
    assert options[:4] == ["--rm", "-i", "--pull", "never"]
    assert options[options.index("--entrypoint") :] == ["--entrypoint", ce.IMAGE_PYTHON]
    # it holds presigned URLs on a rented machine: these options and no other (a denylist would let
    # an attached value through: --volume=/:/host, --privileged=true, --env=X=1)
    assert options == [
        "--rm", "-i", "--pull", "never",
        "--cap-drop=ALL", "--read-only", "--pids-limit=64", "--memory=256m",
        "--security-opt=no-new-privileges",
        "--entrypoint", ce.IMAGE_PYTHON,
    ]  # fmt: skip
    for secret in (KEY, "X-Amz", "sentinel-endpoint", SECRET, CFG.access_key_id):
        assert secret not in run["command"]
    head, get, absent = run["stdin"].splitlines()
    assert len(run["stdin"].splitlines()) == 3 and len({head, get, absent}) == 3
    for url in (head, get, absent):
        assert url.startswith(f"{ENDPOINT}/{BUCKET}/exec/_r2-check/") and "X-Amz-Expires=120" in url
        assert SIGNATURE.search(url) and "X-Amz-SignedHeaders=host" in url
    assert head.startswith(f"{ENDPOINT}/{BUCKET}/{KEY}?") and get.startswith(head.split("?")[0])
    assert absent.startswith(f"{ENDPOINT}/{BUCKET}/{KEY}-absent?")  # a key nobody wrote
    assert len({u.split("X-Amz-Signature=")[1] for u in (head, get, absent)}) == 3
    assert (run["cancel"], run["max_stdout_bytes"]) == (None, 4096)
    assert 0 < run["deadline"] - time.time() <= 90
    assert ok and line.startswith("x python=3.12.4 openssl=OpenSSL_3.0.13_30_Jan_2024 cafile=1")
    assert "head=200 head_bytes=1048576" in line and "get=206 get_bytes=1" in line
    assert "absent=404" in line
    for secret in (KEY, "X-Amz", SECRET):
        assert secret not in line


def test_the_urls_are_in_no_file_and_no_descriptor_is_left_open(world, host):
    descriptors = len(os.listdir("/proc/self/fd"))
    host.out = json.dumps(good_reply())
    probe(world)
    assert sorted(Path(tempfile.gettempdir()).iterdir()) == []
    assert len(os.listdir("/proc/self/fd")) == descriptors


HOSTILE_URL = "https://sentinel-endpoint.example.test/b/k?X-Amz-Signature=" + "0" * 64


def test_what_a_host_prints_is_rebuilt_in_a_closed_shape_before_it_reaches_the_operator(
    world, host
):
    reply = good_reply() | {
        "python": HOSTILE_URL, "openssl": "\x1b[31m" + HOSTILE_URL, "extra": HOSTILE_URL,
        "capath_entries": True, "cafile": "yes", "error": HOSTILE_URL,
    }  # fmt: skip
    reply["head"] |= {"error": HOSTILE_URL, "verify_code": HOSTILE_URL, "elapsed_s": "soon"}
    reply["get"] |= {"bytes": "lots"}
    host.out = json.dumps(reply)
    host.err = HOSTILE_URL + "\n\x1b[2J"
    line, ok = probe(world)
    assert ok
    for text in (
        HOSTILE_URL,
        "\x1b",
        "X-Amz",
        "sentinel",
        "yes",
        "extra",
        "soon",
        "capath_entries",
    ):
        assert text not in line
    assert "head=200" in line and "get=206" in line and "absent=404" in line


def test_a_word_in_place_of_a_status_is_a_word_and_nothing_else(world):
    reply = {
        "head": {"error": HOSTILE_URL, "verify_code": 20},
        "get": {"error": "SSLCertVerificationError", "verify_code": HOSTILE_URL},
        "absent": {"error": "SSLCertVerificationError", "verify_code": 77},
    }
    line = functions()["shown"](reply)
    for text in ("X-Amz", "sentinel", "https", "InvalidURL"):
        assert text not in line
    assert "head=" not in line and "get=SSLCertVerificationError" in line
    assert "get_verify" not in line and "absent=SSLCertVerificationError absent_verify=77" in line


@pytest.mark.parametrize(
    ("field", "value"),
    [
        ("python", "\u0661.\u0662.\u0663"),  # Arabic-Indic digits, which \d would take
        ("python", "1.2.3" + "0" * 5000),
        ("python", "1.2.3\n"),
        ("openssl", "SENTKEYIDzzqq92" + "0" * 25),
        ("openssl", "OpenSSL 3.0.13 \u0663 Jan 2024"),
        ("openssl", "OpenSSL 3.0.13 30 Jan 2024 " + HOSTILE_URL),
    ],
    ids=["unicode-digits", "long-python", "newline", "alphanumeric-echo", "unicode-date", "url"],
)
def test_the_version_fields_take_only_what_a_version_looks_like(field, value):
    line = functions()["shown"](good_reply() | {field: value})
    assert f"{field}=" not in line and re.fullmatch(r"[ -~]{0,300}", line)


@pytest.mark.parametrize(
    ("field", "value"),
    [("openssl", "OpenSSL 3.0.13 30 Jan 2024"), ("openssl", "OpenSSL 1.1.1w  11 Sep 2023"),
     ("openssl", "LibreSSL 3.8.2"), ("python", "3.12.4")],
)  # fmt: skip
def test_the_version_fields_a_real_interpreter_prints_are_kept(field, value):
    line = functions()["shown"](good_reply() | {field: value})
    assert f"{field}={value.replace(' ', '_')}" in line


def test_the_longest_line_a_host_can_make_fits_what_the_script_lets_through():
    worst = good_reply() | {"openssl": "OpenSSL 99.99.99z 99 Aaa 9999", "python": "9999.9999.9999"}
    for name in ("head", "get", "absent"):
        worst[name] = {"error": "A" * 40, "verify_code": 99, "bytes": 1 << 40, "elapsed_s": 999.999}
    line = functions()["shown"](worst)
    assert 0 < len(line) <= 600 and re.fullmatch(r"[ -~]*", line)


@pytest.mark.parametrize(
    ("reply", "ok"),
    [
        (good_reply() | {"head": {"status": 403, "bytes": 0, "elapsed_s": 0.1}}, False),
        (good_reply() | {"head": {"status": 403, "bytes": 1 << 20, "elapsed_s": 0.1}}, False),
        (good_reply() | {"head": {"status": 200, "bytes": 5, "elapsed_s": 0.1}}, False),
        (good_reply() | {"get": {"status": 403, "bytes": 0, "elapsed_s": 0.1}}, False),
        (good_reply() | {"get": {"status": 200, "bytes": 1 << 20, "elapsed_s": 0.1}}, True),
        (good_reply() | {"absent": {"status": 403, "bytes": 0, "elapsed_s": 0.1}}, False),
        (good_reply() | {"absent": {"status": 200, "bytes": 0, "elapsed_s": 0.1}}, False),
        (good_reply() | {"absent": {"error": "ConnectError", "elapsed_s": 0.1}}, False),
        ({k: v for k, v in good_reply().items() if k != "absent"}, False),
        ({"error": "OSError"}, False),
    ],
    ids=["head-403", "head-403-right-size", "head-wrong-size", "get-403", "get-ignores-range",
         "absent-403", "absent-exists", "absent-error", "absent-not-asked", "no-answers"],
)  # fmt: skip
def test_the_host_works_only_if_it_sees_the_object_and_is_told_the_absent_key_is_absent(
    world, host, reply, ok
):
    host.out = json.dumps(reply)
    assert probe(world)[1] is ok


@pytest.mark.parametrize(
    ("code", "word"),
    [(18, "self_signed"), (20, "unknown_issuer"), (62, "hostname_mismatch"), (64, "ip_mismatch"),
     (10, "10"), (99, "99")],
)  # fmt: skip
def test_a_certificate_error_is_a_fixed_word_and_never_the_message_with_the_name_in_it(code, word):
    answer = {"error": "SSLCertVerificationError", "verify_code": code, "elapsed_s": 0.1}
    line = functions()["shown"](good_reply() | {"head": answer})
    assert f"head=SSLCertVerificationError head_verify={word}" in line


@pytest.mark.parametrize(
    ("code", "said"), [(125, "x exit_125"), (255, "x exit_255"), (1, "x exit_1")], ids=str
)
def test_a_host_that_exits_with_a_status_is_that_status(world, host, code, said):
    host.code, host.err = code, "docker: Unable to find image 'ghcr.io/x@sha256:...' locally"
    assert probe(world) == (said, False)  # not a word of its stderr


@pytest.mark.parametrize("out", ["", "not json", "[1, 2]", "null"], ids=repr)
def test_a_host_without_a_json_object_gave_no_reply(world, host, out):
    host.out = out
    assert probe(world) == ("x no_reply", False)


def test_a_transport_that_raises_is_its_class_name_only(world, host):
    host.raises = ce.HostLost("ssh: connect to host 203.0.113.5 port 22: Connection refused")
    assert probe(world) == ("x HostLost", False)


def test_a_label_the_config_does_not_have_is_refused_without_echoing_it(world, host):
    with pytest.raises(Exception, match="no executor host has this label") as raised:
        probe(world, label="sentinel-label")
    assert "sentinel-label" not in str(raised.value) and host.runs == []


def test_an_image_digest_that_is_not_a_digest_never_reaches_the_command_line(world, host):
    with pytest.raises(Exception, match="no executor host has this label"):
        probe(world, digest="sha256:$(touch pwned)")
    assert host.runs == []


def test_without_an_executor_config_in_the_environment_there_is_no_host_to_ask(
    world, host, monkeypatch
):
    monkeypatch.delenv(EXECUTOR_VAR)
    with pytest.raises(Exception, match=f"{EXECUTOR_VAR} is not set"):
        probe(world)


def test_a_round_trip_with_a_host_runs_the_host_between_the_read_and_the_delete(store):
    fake, _, world = store
    log = world.root / "ssh.log"
    # the host: ssh runs the remote command's program on this machine, stdin through
    (world.bin / "ssh").write_text(
        f"#!{sys.executable}\n"
        "import shlex, subprocess, sys\n"
        f"open({str(log)!r}, 'a').write(' '.join(sys.argv[1:]) + '\\n')\n"
        "remote = shlex.split(sys.argv[-1])\n"
        "sys.exit(subprocess.run([sys.executable, *remote[remote.index('-I'):]]).returncode)\n"
    )
    entered, release = fake.gate("DeleteObject")  # holds the delete until the test lets it go
    env = {EXECUTOR_VAR: str(world.config_path)}
    done: list = []
    runner = threading.Thread(
        target=lambda: done.append(world.run("roundtrip", str(world.token), "x", DIGEST, env=env))
    )
    runner.start()
    try:
        assert entered.wait(60), "the program never got to the delete"
        # the host has been asked, and the object is still in the bucket for it to find
        assert log.exists() and len(fake.objects) == 1
    finally:
        release.set()
        runner.join(60)
    (finished,) = done
    # the store is plain http, which the host refuses: all three URLs reached its stdin
    assert finished.returncode == 1 and finished.stderr == ""
    first, second = finished.stdout.splitlines()
    assert re.fullmatch(LINE, first), first
    assert second.startswith("r2-check node x ")
    assert "head=ValueError" in second and "get=ValueError" in second
    assert "absent=ValueError" in second
    asked = log.read_text()
    assert "ubuntu@203.0.113.5" in asked and "X-Amz" not in asked and "_r2-check" not in asked
    ops = [row[0] for row in fake.ledger]
    assert ops == ["PutObject", "GetObject", "DeleteObject", "HeadObject"]
    assert fake.objects == {}
    assert "X-Amz" not in finished.stdout and "_r2-check" not in finished.stdout


# --- the program on the host, against TLS servers -------------------------------------------------


def openssl(*args: str) -> None:
    subprocess.run(["openssl", *args], check=True, capture_output=True)


def certificates(directory: Path) -> None:
    """self.pem and other.pem, self-signed for localhost and other.test; leaf.pem for localhost,
    signed by ca.pem. Each next to its .key."""
    d = str(directory)
    key = ["-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:P-256", "-nodes", "-days", "1"]
    for name, san in (("self", "localhost"), ("other", "other.test")):
        subject = ["-subj", f"/CN={san}", "-addext", f"subjectAltName=DNS:{san}"]
        openssl(
            "req", "-x509", *key, *subject, "-keyout", f"{d}/{name}.key", "-out", f"{d}/{name}.pem"
        )
    ca = ["-subj", "/CN=ca", "-addext", "basicConstraints=critical,CA:TRUE"]
    openssl("req", "-x509", *key, *ca, "-keyout", f"{d}/ca.key", "-out", f"{d}/ca.pem")
    openssl(
        "req",
        "-new",
        *key,
        "-subj",
        "/CN=localhost",
        "-keyout",
        f"{d}/leaf.key",
        "-out",
        f"{d}/leaf.csr",
    )
    (directory / "leaf.ext").write_text("subjectAltName=DNS:localhost\n")
    sign = ["-CA", f"{d}/ca.pem", "-CAkey", f"{d}/ca.key", "-CAcreateserial", "-days", "1"]
    openssl(
        "x509",
        "-req",
        "-in",
        f"{d}/leaf.csr",
        *sign,
        "-extfile",
        f"{d}/leaf.ext",
        "-out",
        f"{d}/leaf.pem",
    )


class Servers:
    """TLS servers on loopback, one for each certificate of `certificates`."""

    def __init__(self, directory: Path) -> None:
        self.directory, self.running = directory, []
        self.asked: list[tuple[str, str]] = []  # (method, path) of every request, in order
        certificates(directory)
        self.ports = {name: self.serve(name) for name in ("self", "other", "leaf")}

    def serve(self, name: str) -> int:
        asked = self.asked

        class Handler(http.server.BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def do_HEAD(self):
                asked.append((self.command, self.path))
                self.send_response(200)
                self.send_header("content-length", str(1 << 20))
                self.end_headers()

            def do_GET(self):
                asked.append((self.command, self.path))
                ranged = self.headers.get("range") == "bytes=0-0"  # honours the range
                status = 404 if "-absent" in self.path else 206 if ranged else 200
                body = b"" if status == 404 else b"x" if ranged else b"x" * ((1 << 20) + 7)
                self.send_response(status)
                self.send_header("content-length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

            def log_message(self, *args):
                return

        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.load_cert_chain(self.directory / f"{name}.pem", self.directory / f"{name}.key")
        server.socket = context.wrap_socket(server.socket, server_side=True)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        self.running.append((server, thread))
        return server.server_address[1]

    def urls(self, name: str, host: str = "localhost") -> str:
        base = f"https://{host}:{self.ports[name]}/bucket/exec/_r2-check/k"
        sign = "?X-Amz-Signature=" + "0" * 64
        return f"{base}{sign}\n{base}{sign}\n{base}-absent{sign}\n"

    def close(self) -> None:
        for server, thread in self.running:
            server.shutdown()
            server.server_close()
            thread.join(10)


@pytest.fixture(scope="module")
def tls(tmp_path_factory):
    servers = Servers(tmp_path_factory.mktemp("tls"))
    yield servers
    servers.close()


def on_the_host(urls: str, env: dict[str, str] | None = None) -> tuple[dict, str]:
    done = subprocess.run(
        [sys.executable, "-I", "-c", functions()["NODE_PY"]],
        input=urls,
        capture_output=True,
        text=True,
        env={"PATH": "/usr/bin:/bin", **(env or {})},
        check=False,
        timeout=60,
    )
    assert done.returncode == 0 and done.stderr == "", done
    (line,) = done.stdout.splitlines()
    return json.loads(line), line


def test_a_host_without_the_ca_cannot_verify_a_self_signed_endpoint_and_says_which_error(tls):
    reply, line = on_the_host(tls.urls("self"))
    for name in ("head", "get", "absent"):
        assert (
            reply[name]["error"] == "SSLCertVerificationError" and reply[name]["verify_code"] == 18
        )
    assert functions()["shown"](reply).count("head_verify=self_signed") == 1
    assert reply["cafile"] in (True, False) and reply["capath_entries"] >= 0
    assert reply["python"] == ".".join(map(str, sys.version_info[:3]))
    assert "X-Amz" not in line and "localhost" not in line


def test_a_host_without_the_ca_of_the_issuer_says_unknown_issuer(tls):
    # the case ADR 0013 leaves open: the image's Python has no CA bundle that vouches for R2
    reply, _ = on_the_host(tls.urls("leaf"))
    assert reply["head"]["verify_code"] == 20
    assert "head_verify=unknown_issuer" in functions()["shown"](reply)


def test_a_host_with_the_ca_gets_the_head_the_ranged_get_and_a_404_for_the_absent_key(tls):
    ca = {"SSL_CERT_FILE": str(tls.directory / "ca.pem")}
    reply, _ = on_the_host(tls.urls("leaf"), ca)
    # the server answers an unranged read with 7 bytes more than the object, and a ranged one with
    # one: a HEAD made of a plain GET, or a GET without its range, would show
    assert (reply["head"]["status"], reply["head"]["bytes"]) == (200, 1 << 20)
    assert (reply["get"]["status"], reply["get"]["bytes"]) == (206, 1)
    assert reply["absent"]["status"] == 404 and reply["ssl_env"] is True
    assert functions()["shown"](reply).count("error") == 0


def test_the_host_reports_whether_python_can_see_a_ca_file_and_how_many_entries_a_directory_has(
    tls, tmp_path
):
    ca = tmp_path / "ca-dir"
    ca.mkdir()
    for name in ("one.pem", "two.pem"):
        (ca / name).write_text("x")
    both = {"SSL_CERT_FILE": str(tls.directory / "ca.pem"), "SSL_CERT_DIR": str(ca)}
    reply, _ = on_the_host("", both)
    assert (reply["cafile"], reply["capath_entries"], reply["ssl_env"]) == (True, 2, True)
    missing = {"SSL_CERT_FILE": str(tmp_path / "none.pem"), "SSL_CERT_DIR": str(tmp_path / "none")}
    reply, _ = on_the_host("", missing)  # the environment decides, not the build's defaults
    assert (reply["cafile"], reply["capath_entries"]) == (False, 0)
    exists = os.path.exists(ssl.get_default_verify_paths().openssl_cafile or "")
    assert reply["openssl_cafile"] is exists  # a property of this interpreter's OpenSSL build


def test_the_first_request_is_a_head_and_the_second_a_ranged_get_and_the_third_a_get(tls):
    tls.asked.clear()
    ca = {"SSL_CERT_FILE": str(tls.directory / "ca.pem")}
    on_the_host(tls.urls("leaf"), ca)
    methods = [m for m, _ in tls.asked]
    assert methods == ["HEAD", "GET", "GET"] and tls.asked[2][1].split("?")[0].endswith("-absent")


def test_a_certificate_for_another_name_is_a_hostname_error_and_the_name_stays_out(tls):
    own = {"SSL_CERT_FILE": str(tls.directory / "self.pem")}
    reply, line = on_the_host(tls.urls("self", "127.0.0.1"), own)  # trusted, but for `localhost`
    assert reply["head"]["verify_code"] == 64 and "127.0.0.1" not in line
    assert "head_verify=ip_mismatch" in functions()["shown"](reply)
    other = {"SSL_CERT_FILE": str(tls.directory / "other.pem")}
    reply, line = on_the_host(tls.urls("other"), other)  # trusted, but for `other.test`
    assert reply["head"]["verify_code"] == 62
    assert "other.test" not in line and "localhost" not in line
    assert "head_verify=hostname_mismatch" in functions()["shown"](reply)
    reply, _ = on_the_host(tls.urls("self"), other)  # a CA file that does not vouch for it
    assert reply["head"]["verify_code"] == 18


@pytest.mark.parametrize(
    "url",
    [
        "https://localhost:1/k?X-Amz-Signature=SECRETSIGNATURE\x01",
        "http://localhost:1/k?a=SECRETSIGNATURE",
    ],
    ids=["control-character", "not-https"],
)
def test_a_url_it_cannot_use_is_a_class_name_and_not_a_traceback(url):
    reply, line = on_the_host(f"{url}\n{url}\n{url}\n")
    assert reply["head"]["error"] in {"InvalidURL", "ValueError"}
    assert "SECRETSIGNATURE" not in line


@pytest.mark.skipif(os.geteuid() == 0, reason="root reads any directory")
def test_a_ca_directory_it_cannot_read_is_an_error_word_and_not_a_traceback(tmp_path):
    closed = tmp_path / "closed"
    closed.mkdir(mode=0o000)
    try:
        reply, _ = on_the_host("", {"SSL_CERT_DIR": str(closed)})
    finally:
        closed.chmod(0o700)
    assert reply == {"error": "PermissionError"}
    assert functions()["shown"](reply) == "error=PermissionError"
