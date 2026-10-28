"""The program challenge_image.sh runs inside the challenge container, run for real.

test_executor_r2.py canned its answers; here `python -I -c CHALLENGE_PY` reads real files with the
real loader (hypertrain.gpu_ops.artifact_transfer), as the user of the test. What it prints is an
operator's screen: no value of the token file, no byte of it, no path of a URL.
"""

from __future__ import annotations

import json
import re
import shutil
import subprocess
import sys
from pathlib import Path

import httpx
import pytest
from tests.gpu_ops.artifact_fakes import BUCKET, TimedFakeR2, credentials_file, serve_http
from tests.storage.fakes import CFG, SECRET

from hypertrain.gpu_ops import artifact_transfer as at
from hypertrain.gpu_ops import cuda_executor as ce

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


def test_pre_accepts_what_the_challenge_would_load(world):
    done = world.pre()
    assert (done.returncode, done.stdout, done.stderr) == (0, "preflight ok\n", "")


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
    w.settings["unknown-key"] = 1


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
    "bucket-name": (corrupt_bucket_name, "ConfigError"),
    "plain-http": (corrupt_plain_http, "the R2 endpoint must be https"),
    "not-utf8": (corrupt_bytes, "UnicodeDecodeError"),
    "token-gone": (corrupt_gone, "FileNotFoundError"),
    "settings-key": (corrupt_settings_key, "workers: Input should be a valid integer"),
    "network-none": (corrupt_network, "artifacts need network 'default'"),
    "big-artifact": (corrupt_big_artifact, "max_artifact_bytes is above the executor's"),
    "small-result": (corrupt_small_result, "max_artifact_bytes is above the executor's"),
    "repeated-operations": (corrupt_operations, "operations must be a non-empty list"),
    "config-gone": (corrupt_config_gone, "FileNotFoundError"),
}


@pytest.mark.parametrize(("corrupt", "said"), REFUSED.values(), ids=REFUSED)
def test_pre_refuses_what_the_loader_refuses_and_says_so_without_quoting_the_file(
    world, corrupt, said
):
    corrupt(world)
    reason = refusal(world.pre())
    assert said in reason
    assert "sentinel" not in reason and "\\xff" not in reason and "0xff" not in reason


def test_pre_does_not_echo_a_settings_document_it_cannot_parse(world):
    done = world.run("pre", str(world.config_path), stdin=b'{"credentials_file": "sentinel-path')
    assert "sentinel" not in refusal(done)


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


@pytest.mark.parametrize(
    "corrupt",
    [
        None,
        corrupt_mode,
        corrupt_extra_name,
        corrupt_prefix,
        corrupt_bucket_name,
        corrupt_plain_http,
        corrupt_network,
        corrupt_big_artifact,
        corrupt_small_result,
        corrupt_operations,
    ],
    ids=lambda c: "valid" if c is None else c.__name__.removeprefix("corrupt_"),
)
def test_pre_and_the_real_constructor_take_and_refuse_the_same_files(world, monkeypatch, corrupt):
    if corrupt is not None:
        corrupt(world)
    verdict = world.pre().returncode == 0
    assert verdict == constructor_accepts(world, monkeypatch)


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
    assert refusal(done) == "FileNotFoundError"
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


# --- roundtrip ----------------------------------------------------------------------------------

LINE = (
    r"r2-check challenge put=200 get=200 delete=204 bytes=1048576"
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


def test_a_round_trip_puts_reads_and_deletes_one_mebibyte_and_prints_only_statuses(store):
    fake, front, world = store
    done = world.run("roundtrip", str(world.token))
    assert done.returncode == 0 and done.stderr == ""
    assert re.fullmatch(LINE + "\n", done.stdout), done.stdout
    assert [row[0] for row in fake.ledger] == ["PutObject", "GetObject", "DeleteObject"]
    keys = {row[1] for row in fake.ledger}
    assert len(keys) == 1 and next(iter(keys)).startswith("exec/_r2-check/")
    assert (fake.ledger[0][3], fake.ledger[1][4]) == (1 << 20, 1 << 20)
    assert fake.objects == {}  # nothing is left in the bucket
    for text in (next(iter(keys)), "exec/", "127.0.0.1", front.base):
        assert text not in done.stdout


def test_a_round_trip_without_a_file_uses_the_token_the_settings_of_the_process_name(store):
    fake, _, world = store
    done = world.run("roundtrip", "-", env=configured(world))
    assert done.returncode == 0 and "put=200 get=200 delete=204" in done.stdout
    assert len(fake.ledger) == 3


def test_a_round_trip_with_nothing_configured_says_what_is_missing(store):
    _, _, world = store
    assert refusal(world.run("roundtrip", "-")) == f"{ARTIFACTS_VAR} is not set"


def test_a_round_trip_does_not_listen_to_the_proxy_variables_the_transfer_ignores(store):
    _, _, world = store
    done = world.run("roundtrip", str(world.token), env=PROXIES)
    assert done.returncode == 0 and "put=200 get=200 delete=204" in done.stdout


def test_a_store_that_is_down_is_a_class_name_and_a_skipped_read(store):
    _, front, world = store
    front.close()
    done = world.run("roundtrip", str(world.token))
    assert done.returncode == 1 and done.stderr == ""
    assert re.fullmatch(
        r"r2-check challenge put=ConnectError get=skipped delete=ConnectError bytes=1048576"
        r" put_s=\d+\.\d{3} delete_s=\d+\.\d{3}\n",
        done.stdout,
    ), done.stdout


@pytest.mark.parametrize("status", [403, 500])
def test_a_refused_put_is_its_http_status_and_the_key_stays_out_of_the_line(store, status):
    fake, _, world = store
    fake.fault_key("PutObject", "", status)
    done = world.run("roundtrip", str(world.token))
    assert done.returncode == 1
    assert f"put={status} get=skipped delete=204" in done.stdout and "exec/" not in done.stdout


def test_a_read_that_returns_other_bytes_is_a_mismatch_and_the_object_is_still_deleted(tmp_path):
    fake = Flipping()
    front = serve_http(fake)
    try:
        world = World(tmp_path / "executor")
        world.token = credentials_file(world.root, front.base)
        done = world.run("roundtrip", str(world.token))
    finally:
        front.close()
    assert done.returncode == 1 and "put=200 get=mismatch delete=204" in done.stdout
    assert fake.objects == {}


def test_a_delete_that_fails_is_reported_and_fails_the_check(store):
    fake, _, world = store
    fake.fault_key("DeleteObject", "", 403)
    done = world.run("roundtrip", str(world.token))
    assert done.returncode == 1 and "put=200 get=200 delete=403" in done.stdout


def test_a_round_trip_refuses_the_token_the_loader_refuses(store):
    _, _, world = store
    world.token.chmod(0o644)
    assert refusal(world.run("roundtrip", str(world.token))) == LOADER
