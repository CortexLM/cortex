"""Generic CUDA executor, pure parts (docs/adr/0008): config, host probe, command shapes, host
selection and the whitelist reader of the result tar. No ssh, docker or GPU runs here."""

from __future__ import annotations

import copy
import hashlib
import io
import json
import os
import shlex
import stat
import subprocess
import tarfile
import threading
import time
import uuid as uuidlib
from pathlib import Path
from types import SimpleNamespace
from typing import Any

import pytest
from pydantic import ValidationError

import hypertrain.trainer  # noqa: F401 - pins determinism before any test imports torch
from hypertrain.gpu_ops import cuda_executor as ce
from hypertrain.gpu_ops import cuda_executor_runner as runner
from hypertrain.protocol.env_pins import class_pin
from hypertrain.protocol.example import example_manifest
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages import EnvPins
from hypertrain.protocol.messages_v2 import IslandJobV1, RunManifestV2

from .pin_wire import PIN, honest_report

REPOSITORY = "registry.test/hypertrain-gpu"
DIGEST = "sha256:" + "ab" * 32
IMAGE = f"{REPOSITORY}@{DIGEST}"
DRIVER = "580.95.05"
B300 = (10, 3)
SM_B300 = 148
# The GPU class a manifest of that SM count pins (docs/adr/0007), written down on purpose.
CLASS_OF_SM = {148: "sm_103", 170: "sm_120"}
NAME = "ht-exec-0123456789abcdef"


def uid(index: int) -> str:
    """Distinct nvidia-smi UUIDs that sort in index order."""
    return "GPU-" + str(uuidlib.UUID(int=((index + 1) << 120) | 0xABCDEF0123))


def sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


# -- configuration ----------------------------------------------------------------------------


@pytest.fixture
def pinned(tmp_path: Path) -> tuple[Path, Path]:
    key, known = tmp_path / "id_ed25519", tmp_path / "known_hosts"
    key.write_text("operator-provided private key lives here")
    key.chmod(0o600)
    known.write_text("gpu-1.example.com ssh-ed25519 AAAA\n")
    return key, known


def host_data(pinned: tuple[Path, Path], label: str = "h1", **over: Any) -> dict[str, Any]:
    key, known = pinned
    return {
        "label": label,
        "address": "gpu-1.example.com",
        "user": "ubuntu",
        "identity_file": str(key),
        "known_hosts_file": str(known),
        **over,
    }


def config_data(tmp_path: Path, pinned: tuple[Path, Path], **over: Any) -> dict[str, Any]:
    return {
        "run_id": "a" * 64,
        "image_repository": REPOSITORY,
        "state_dir": str(tmp_path / "state"),
        "hosts": [host_data(pinned)],
        **over,
    }


def load_config(data: dict[str, Any]) -> ce.ExecutorConfig:
    return ce.ExecutorConfig.model_validate_json(json.dumps(data))


def with_value(data: dict[str, Any], path: tuple[str | int, ...], value: Any) -> dict[str, Any]:
    data = copy.deepcopy(data)
    node: Any = data
    for key in path[:-1]:
        node = node[key]
    node[path[-1]] = value
    return data


def test_valid_config_takes_the_documented_defaults(
    tmp_path: Path, pinned: tuple[Path, Path]
) -> None:
    config = load_config(config_data(tmp_path, pinned))
    assert (config.network, config.max_attempts, config.sweeps, config.sweep_pause_seconds) == (
        "default",
        3,
        3,
        15,
    )
    assert (config.idle_gpu_memory_mib, config.max_result_bytes) == (2048, 16 << 30)
    assert (config.hosts[0].port, config.hosts[0].docker) == (22, ("docker",))
    sudo = load_config(
        config_data(tmp_path, pinned, hosts=[host_data(pinned, docker=["sudo", "-n", "docker"])])
    )
    assert sudo.hosts[0].docker == ("sudo", "-n", "docker")


H = ("hosts", 0)
REFUSED_FIELDS = [
    # An address is handed to ssh as an argument: no option injection, whitespace or userinfo.
    pytest.param(H + ("address",), "-oProxyCommand=x", id="address-option-injection"),
    pytest.param(H + ("address",), "gpu 1", id="address-space"),
    pytest.param(H + ("address",), "gpu-1;id", id="address-semicolon"),
    pytest.param(H + ("address",), "gpu-1\n", id="address-trailing-newline"),
    pytest.param(H + ("address",), "$(id)", id="address-substitution"),
    pytest.param(H + ("address",), "root@gpu-1", id="address-userinfo"),
    pytest.param(H + ("address",), "a" * 254, id="address-too-long"),
    pytest.param(H + ("address",), "", id="address-empty"),
    pytest.param(H + ("user",), "-oProxyCommand=x", id="user-option-injection"),
    pytest.param(H + ("user",), "Root", id="user-uppercase"),
    pytest.param(H + ("user",), "ubuntu\n", id="user-trailing-newline"),
    pytest.param(H + ("user",), "a" * 33, id="user-too-long"),
    pytest.param(H + ("user",), "", id="user-empty"),
    pytest.param(H + ("label",), "H1", id="label-uppercase"),
    pytest.param(H + ("label",), "-h1", id="label-leading-dash"),
    pytest.param(H + ("label",), "h_1", id="label-underscore"),
    pytest.param(H + ("label",), "h1\n", id="label-trailing-newline"),
    pytest.param(H + ("label",), "a" * 33, id="label-too-long"),
    pytest.param(H + ("port",), 0, id="port-zero"),
    pytest.param(H + ("port",), 65536, id="port-over"),
    pytest.param(H + ("port",), "22", id="port-string"),
    pytest.param(H + ("port",), True, id="port-bool"),
    pytest.param(H + ("docker",), [], id="docker-empty"),
    pytest.param(H + ("docker",), ["-docker"], id="docker-leading-dash"),
    pytest.param(H + ("docker",), ["doc ker"], id="docker-space"),
    pytest.param(H + ("docker",), ["docker;rm"], id="docker-semicolon"),
    pytest.param(H + ("docker",), ["docker", "$(id)"], id="docker-substitution"),
    pytest.param(H + ("docker",), ["docker", ""], id="docker-empty-word"),
    pytest.param(H + ("docker",), ["sudo", "-n", "docker", "a", "b"], id="docker-five-words"),
    pytest.param(H + ("key",), "-" * 5 + "BEGIN KEY-----", id="host-key-field"),
    pytest.param(H + ("password",), "x", id="host-password-field"),
    pytest.param(("token",), "x", id="config-extra-field"),
    pytest.param(("run_id",), "A" * 64, id="run-id-uppercase"),
    pytest.param(("run_id",), "a" * 63, id="run-id-short"),
    pytest.param(("image_repository",), "repo:latest", id="image-tag"),
    pytest.param(("image_repository",), "repo@sha256:" + "a" * 64, id="image-with-digest"),
    pytest.param(("image_repository",), "Repo", id="image-uppercase"),
    pytest.param(("image_repository",), "-repo", id="image-leading-dash"),
    pytest.param(("network",), "host", id="network-host"),
    pytest.param(("compute_cache",), "yes", id="cache-string"),
    pytest.param(("compute_cache",), 1, id="cache-integer"),
    pytest.param(("compute_cache",), None, id="cache-null"),
    pytest.param(H + ("compute_cache_dir",), "cache", id="cache-dir-relative"),
    pytest.param(H + ("compute_cache_dir",), "", id="cache-dir-empty"),
    pytest.param(H + ("compute_cache_dir",), "/", id="cache-dir-root"),
    pytest.param(H + ("compute_cache_dir",), "/mnt/../etc", id="cache-dir-dotdot"),
    pytest.param(H + ("compute_cache_dir",), "/mnt/a b", id="cache-dir-space"),
    pytest.param(H + ("compute_cache_dir",), "/mnt/a,dst=/etc", id="cache-dir-mount-option"),
    pytest.param(H + ("compute_cache_dir",), "/mnt/a=b", id="cache-dir-equals"),
    pytest.param(H + ("compute_cache_dir",), "/mnt/$HOME", id="cache-dir-variable"),
    pytest.param(H + ("compute_cache_dir",), "~/cache", id="cache-dir-tilde"),
    pytest.param(H + ("compute_cache_dir",), "/mnt/a;rm", id="cache-dir-semicolon"),
    pytest.param(H + ("compute_cache_dir",), "/mnt/cache\n", id="cache-dir-newline"),
    pytest.param(H + ("compute_cache_dir",), "/" + "a" * 200, id="cache-dir-201-characters"),
    pytest.param(H + ("compute_cache_dir",), 7, id="cache-dir-integer"),
    pytest.param(("hosts",), [], id="no-host"),
    pytest.param(("max_attempts",), 0, id="attempts-zero"),
    pytest.param(("max_attempts",), 11, id="attempts-over"),
    pytest.param(("max_attempts",), "3", id="attempts-string"),
    pytest.param(("sweeps",), 0, id="sweeps-zero"),
    pytest.param(("sweeps",), 21, id="sweeps-over"),
    pytest.param(("sweep_pause_seconds",), -1, id="pause-negative"),
    pytest.param(("sweep_pause_seconds",), 601, id="pause-over"),
    pytest.param(("idle_gpu_memory_mib",), -1, id="idle-negative"),
    pytest.param(("max_clock_skew_seconds",), 0, id="skew-zero"),
    pytest.param(("max_clock_skew_seconds",), 61, id="skew-over"),
    pytest.param(("max_result_bytes",), (1 << 20) - 1, id="result-bytes-under"),
    pytest.param(("pull_timeout_seconds",), 29, id="pull-timeout-under"),
    pytest.param(("ssh_connect_timeout_seconds",), 0, id="connect-timeout-zero"),
    pytest.param(("ssh_connect_timeout_seconds",), 121, id="connect-timeout-over"),
]


@pytest.mark.parametrize(("path", "value"), REFUSED_FIELDS)
def test_config_refuses_one_bad_field(
    tmp_path: Path, pinned: tuple[Path, Path], path: tuple[str | int, ...], value: Any
) -> None:
    data = with_value(config_data(tmp_path, pinned), path, value)
    with pytest.raises(ValidationError) as refusal:
        load_config(data)
    assert [error["loc"] for error in refusal.value.errors()] == [path]


def test_config_bounds_are_inclusive(tmp_path: Path, pinned: tuple[Path, Path]) -> None:
    low = {
        "max_attempts": 1,
        "sweeps": 1,
        "sweep_pause_seconds": 0,
        "idle_gpu_memory_mib": 0,
        "max_clock_skew_seconds": 1,
        "max_result_bytes": 1 << 20,
        "pull_timeout_seconds": 30,
        "ssh_connect_timeout_seconds": 1,
    }
    high = {
        "max_attempts": 10,
        "sweeps": 20,
        "sweep_pause_seconds": 600,
        "max_clock_skew_seconds": 60,
        "ssh_connect_timeout_seconds": 120,
        "network": "none",
    }
    first = host_data(pinned, "h0", port=1)
    last = [host_data(pinned, f"h{i}", port=65535 - i) for i in range(8)]
    assert load_config(config_data(tmp_path, pinned, hosts=[first], **low)).max_attempts == 1
    assert len(load_config(config_data(tmp_path, pinned, hosts=last, **high)).hosts) == 8
    with pytest.raises(ValidationError):
        load_config(config_data(tmp_path, pinned, hosts=[*last, host_data(pinned, "h8")]))


def test_config_host_labels_are_unique(tmp_path: Path, pinned: tuple[Path, Path]) -> None:
    twice = [host_data(pinned, "h1"), host_data(pinned, "h1", address="gpu-2.example.com")]
    with pytest.raises(ValidationError, match="host labels must be unique"):
        load_config(config_data(tmp_path, pinned, hosts=twice))


@pytest.mark.parametrize("mode", [0o640, 0o604, 0o660, 0o606, 0o666, 0o777])
def test_identity_file_with_group_or_other_bits_is_refused(
    tmp_path: Path, pinned: tuple[Path, Path], mode: int
) -> None:
    pinned[0].chmod(mode)
    with pytest.raises(ValidationError, match="identity_file must be a regular file"):
        load_config(config_data(tmp_path, pinned))


def test_identity_file_must_be_a_regular_file_of_this_user(
    tmp_path: Path, pinned: tuple[Path, Path], monkeypatch: pytest.MonkeyPatch
) -> None:
    key = pinned[0]
    link, folder, fifo = tmp_path / "link", tmp_path / "folder", tmp_path / "fifo"
    link.symlink_to(key)
    folder.mkdir(mode=0o700)
    os.mkfifo(fifo, 0o600)
    for other in (link, folder, fifo):
        data = config_data(tmp_path, pinned, hosts=[host_data(pinned, identity_file=str(other))])
        with pytest.raises(ValidationError, match="identity_file must be a regular file"):
            load_config(data)
    monkeypatch.setattr(ce.os, "getuid", lambda: os.stat(key).st_uid + 1)
    with pytest.raises(ValidationError, match="identity_file must be a regular file"):
        load_config(config_data(tmp_path, pinned))


def test_known_hosts_must_be_a_non_empty_regular_file(
    tmp_path: Path, pinned: tuple[Path, Path]
) -> None:
    empty, link, folder = tmp_path / "empty", tmp_path / "link", tmp_path / "folder"
    empty.write_text("")
    link.symlink_to(pinned[1])
    folder.mkdir()
    for other in (empty, link, folder):
        data = config_data(tmp_path, pinned, hosts=[host_data(pinned, known_hosts_file=str(other))])
        with pytest.raises(ValidationError, match="known_hosts_file must be a non-empty regular"):
            load_config(data)


@pytest.mark.parametrize("field", ["identity_file", "known_hosts_file"])
def test_a_missing_file_fails_loading_as_an_oserror_or_valueerror(
    tmp_path: Path, pinned: tuple[Path, Path], field: str
) -> None:
    # The challenge maps exactly (OSError, ValueError) to 503 ROLE_DISPATCH_UNAVAILABLE.
    data = config_data(tmp_path, pinned, hosts=[host_data(pinned, **{field: str(tmp_path / "no")})])
    with pytest.raises((OSError, ValueError)):
        load_config(data)


# -- host probe -------------------------------------------------------------------------------


def probe_text(
    gpus: str | None = None,
    apps: str | None = "",
    epoch: str | None = "1800000000",
    image: str | None = f'["{IMAGE}"]',
) -> str:
    """Output of `snapshot_command`; a None section is left out entirely."""
    rows = f"{uid(0)}, NVIDIA B300 SXM6 AC, {DRIVER}, 10.3, 810" if gpus is None else gpus
    parts = [rows]
    for header, body in (("apps", apps), ("epoch", epoch), ("image", image)):
        if body is not None:
            parts += [f"##{header}", body]
    return "\n".join(parts) + "\n"


def test_parse_snapshot_reads_gpus_busy_apps_epoch_and_digests() -> None:
    shouted = "GPU-" + uid(1)[4:].upper()  # nvidia-smi spells UUIDs lower case; be liberal
    gpus = (
        f"{uid(0)}, NVIDIA B300 SXM6 AC, {DRIVER}, 10.3, 810\n"
        f"{shouted}, NVIDIA B300 SXM6 AC, {DRIVER}, 10.3, 183000"
    )
    apps = f"{shouted}, 4242, python\n{uid(1)}, 4243, [Not Found]"
    snapshot = ce.parse_snapshot(probe_text(gpus, apps))
    name = "NVIDIA B300 SXM6 AC"
    assert snapshot == ce.Snapshot(
        (ce.Gpu(uid(0), name, DRIVER, B300, 810), ce.Gpu(uid(1), name, DRIVER, B300, 183000)),
        frozenset({uid(1)}),
        1800000000,
        (IMAGE,),
    )


def test_parse_snapshot_no_compute_app_rows_and_absent_image() -> None:
    snapshot = ce.parse_snapshot(probe_text(apps="", image="ABSENT"))
    assert snapshot.busy == frozenset()
    assert snapshot.repo_digests is None


GOOD_ROW = f"{uid(0)}, NVIDIA B300 SXM6 AC, {DRIVER}, 10.3, 810"
NOT_HEX = "GPU-" + "-".join("z" * n for n in (8, 4, 4, 4, 12))  # 36 characters, 4 dashes, no hex
REFUSED_PROBES = [
    pytest.param(probe_text(gpus=""), id="no-gpu"),
    pytest.param(probe_text(gpus=f"{GOOD_ROW}\n{GOOD_ROW}"), id="duplicate-uuid"),
    pytest.param(probe_text(gpus=f"GPU-1234, n, {DRIVER}, 10.3, 810"), id="malformed-uuid"),
    pytest.param(probe_text(gpus=f"{uid(0)}, n, {DRIVER}, 10, 810"), id="capability-no-minor"),
    pytest.param(probe_text(gpus=f"{uid(0)}, n, {DRIVER}, x.y, 810"), id="capability-letters"),
    pytest.param(probe_text(gpus=f"{uid(0)}, n, {DRIVER}, 10.3.1, 810"), id="capability-three"),
    pytest.param(probe_text(gpus=f"{uid(0)}, n, {DRIVER}, 10.3, N/A"), id="memory-not-int"),
    pytest.param(probe_text(gpus=f"{uid(0)}, n, {DRIVER}, 10.3, 810.5"), id="memory-fraction"),
    pytest.param(probe_text(gpus=f"{uid(0)}, n, {DRIVER}, 10.3"), id="row-too-short"),
    pytest.param(probe_text(gpus="NVIDIA-SMI has failed"), id="driver-error-text"),
    pytest.param(probe_text(apps="GPU-1234, 1, python"), id="busy-malformed-uuid"),
    # gpu_uuid() lets bytes.fromhex raise a bare ValueError for a well-shaped, non-hex UUID.
    pytest.param(probe_text(gpus=f"{NOT_HEX}, n, {DRIVER}, 10.3, 810"), id="uuid-shaped-not-hex"),
    pytest.param(probe_text(apps=f"{NOT_HEX}, 1, python"), id="busy-uuid-shaped-not-hex"),
    # csv.Error for a field over the 131072-character limit is not a ValueError.
    pytest.param(
        probe_text(gpus=f"{uid(0)}, {'n' * 200_000}, {DRIVER}, 10.3, 810"),
        id="gpu-field-over-csv-limit",
    ),
    pytest.param(probe_text(apps=f"{uid(0)}, 1, {'p' * 200_000}"), id="apps-field-over-csv-limit"),
    pytest.param(
        probe_text(image=json.dumps([f"{REPOSITORY}@sha256:{i:064x}" for i in range(33)])),
        id="digests-33-entries",
    ),
    pytest.param(probe_text(image=json.dumps(["r" * 257])), id="digest-257-characters"),
    pytest.param(probe_text(apps=None), id="no-apps-section"),
    pytest.param(probe_text(epoch="soon"), id="epoch-not-int"),
    pytest.param(probe_text(epoch=""), id="epoch-empty"),
    pytest.param(probe_text(epoch=None), id="no-epoch-section"),
    # a time_t holds 63 bits and a float 308 digits: `date +%s` prints nothing beyond the first
    pytest.param(probe_text(epoch=str(1 << 63)), id="epoch-beyond-time_t"),
    pytest.param(probe_text(epoch=str(-(1 << 63) - 1)), id="epoch-below-time_t"),
    pytest.param(probe_text(epoch="9" * 400), id="epoch-400-digits"),
    pytest.param(probe_text(image="not json"), id="digests-not-json"),
    pytest.param(probe_text(image="[" * 100_000 + "]" * 100_000), id="digests-100000-levels"),
    pytest.param(probe_text(image="[" * 900_000), id="digests-900000-opening-brackets"),
    pytest.param(probe_text(image='{"a":' * 100_000), id="digests-100000-levels-of-objects"),
    pytest.param(probe_text(image=json.dumps([IMAGE, "\ud800"])), id="digest-lone-surrogate"),
    pytest.param(
        probe_text(image=json.dumps([IMAGE, "café"], ensure_ascii=False)), id="digest-non-ascii"
    ),
    pytest.param(probe_text(image=json.dumps([IMAGE, 'a"b'])), id="digest-with-an-escaped-quote"),
    pytest.param(probe_text(image='["\\u0072"]'), id="digest-with-an-escape"),
    pytest.param(probe_text(image="[1, 2]"), id="digests-not-strings"),
    pytest.param(probe_text(image="null"), id="digests-null"),
    pytest.param(probe_text(image=None), id="no-image-section"),
]


@pytest.mark.parametrize("text", REFUSED_PROBES)
def test_parse_snapshot_refuses_the_host_on_anything_unexpected(text: str) -> None:
    with pytest.raises(ce.HostRefused):
        ce.parse_snapshot(text)


def test_parse_snapshot_accepts_repo_digests_exactly_at_their_bounds() -> None:
    listed = [f"{'r' * 252}{i:04d}" for i in range(32)]  # 32 entries of 256 characters
    assert ce.parse_snapshot(probe_text(image=json.dumps(listed))).repo_digests == tuple(listed)


def test_parse_snapshot_reads_the_compact_list_the_daemon_prints() -> None:
    # `{{json .RepoDigests}}` is Go's json.Marshal: no space after the comma.
    listed = [IMAGE, f"{REPOSITORY}@sha256:{'cd' * 32}"]
    text = probe_text(image=json.dumps(listed, separators=(",", ":")))
    assert ce.parse_snapshot(text).repo_digests == tuple(listed)


@pytest.mark.parametrize("image", ['"abc"', '{"a": 1}'])
def test_parse_snapshot_repo_digests_must_be_a_json_list(image: str) -> None:
    # tuple("abc") and tuple({"a": 1}) would pass for a list of digests.
    with pytest.raises(ce.HostRefused):
        ce.parse_snapshot(probe_text(image=image))


def find(tokens: list[str], run: list[str]) -> int:
    return next(i for i in range(len(tokens)) if tokens[i : i + len(run)] == run)


def test_snapshot_command_queries_what_parse_snapshot_reads() -> None:
    tokens = shlex.split(ce.snapshot_command(("sudo", "-n", "docker"), IMAGE))
    inspect = ["sudo", "-n", "docker", "image", "inspect", "--format", "{{json .RepoDigests}}"]
    at = find(tokens, [*inspect, IMAGE])
    assert tokens[at + len(inspect) + 1 :][:4] == ["2>/dev/null", "||", "echo", "ABSENT"]
    # Build each nvidia-smi answer in the column order the command asks for, then parse it.
    values = {
        "uuid": uid(2),
        "name": "NVIDIA B300 SXM6 AC",
        "driver_version": DRIVER,
        "compute_cap": "10.3",
        "memory.used": "810",
        "gpu_uuid": uid(2),
        "pid": "7",
        "process_name": "python",
    }
    queries = {t.split("=")[0]: t.split("=")[1].split(",") for t in tokens if "--query-" in t}
    assert set(queries) == {"--query-gpu", "--query-compute-apps"}
    gpus = ", ".join(values[c] for c in queries["--query-gpu"])
    apps = ", ".join(values[c] for c in queries["--query-compute-apps"])
    snapshot = ce.parse_snapshot(probe_text(gpus, apps))
    assert snapshot.gpus == (ce.Gpu(uid(2), "NVIDIA B300 SXM6 AC", DRIVER, B300, 810),)
    assert snapshot.busy == frozenset({uid(2)})


def test_snapshot_command_quotes_hostile_words_as_single_tokens() -> None:
    hostile = "repo@sha256:x; touch /tmp/pwned $(id) `id` 'q'"
    tokens = shlex.split(ce.snapshot_command(("docker",), hostile))
    assert tokens[find(tokens, ["--format", "{{json .RepoDigests}}"]) + 2] == hostile


@pytest.mark.parametrize("network", ["default", "none"])
def test_run_command_shape(network: str) -> None:
    uuids = [uid(0), uid(3)]
    docker = ("sudo", "-n", "docker")
    argv = shlex.split(ce.run_command(docker, IMAGE, NAME, "d" * 64, uuids, network))
    options, program = argv[: argv.index(IMAGE)], argv[argv.index(IMAGE) :]
    assert options[:4] == [*docker, "run"]
    assert {"--rm", "-i", "--init", "--ipc=host"} <= set(options)
    assert options[options.index("--name") + 1] == NAME
    assert options[options.index("--label") + 1] == f"{ce.LABEL}={'d' * 16}"
    # docker parses the device list as CSV: the literal double quotes are part of the argument.
    assert options[options.index("--gpus") + 1] == f'"device={uuids[0]},{uuids[1]}"'
    assert options[options.index("--entrypoint") + 1] == "/opt/hypertrain/venv/bin/python"
    # NCCL needs unlimited locked memory and a 64 MiB stack; the pairs are flag then value.
    assert [options[i + 1] for i, t in enumerate(options) if t == "--ulimit"] == [
        "memlock=-1",
        "stack=67108864",
    ]
    assert ("--network" in options) is (network == "none")
    if network == "none":
        assert options[options.index("--network") + 1] == "none"
    # What starts after the image is the program: -I -c <source>, nothing else.
    assert program == [IMAGE, "-I", "-c", ce.RUNNER_SOURCE]
    # Nothing is mounted, injected, or widened: inputs and settings travel on stdin.
    forbidden = {"-v", "--volume", "--mount", "-e", "--env", "--env-file", "--privileged", "-p"}
    assert not forbidden & set(options)
    assert not any(t.startswith(("--cap-add", "--device", "--pid", "--net=")) for t in options)
    assert [t for t in options if t.startswith("/")] == ["/opt/hypertrain/venv/bin/python"]


# -- machine identity -------------------------------------------------------------------------


@pytest.fixture
def executor(tmp_path: Path, pinned: tuple[Path, Path]) -> ce.Executor:
    hosts = [host_data(pinned, "h1"), host_data(pinned, "h2", address="gpu-2.example.com")]
    return ce.Executor(load_config(config_data(tmp_path, pinned, hosts=hosts)), transports={})


def gpu(index: int, *, cap: tuple[int, int] = B300, driver: str = DRIVER, mem: int = 810) -> ce.Gpu:
    # Names do not sort like UUIDs: only the UUID decides which GPUs are taken.
    return ce.Gpu(uid(index), f"name-{(index * 7) % 10}", driver, cap, mem)


def snap(
    gpus: list[ce.Gpu], busy: tuple[ce.Gpu, ...] = (), digests: tuple[str, ...] | None = (IMAGE,)
) -> ce.Snapshot:
    return ce.Snapshot(tuple(gpus), frozenset(g.uuid for g in busy), 0, digests)


def test_machine_id_follows_the_whole_gpu_set_not_the_order(executor: ce.Executor) -> None:
    four = [gpu(i) for i in range(4)]
    first = executor._admit("h1", snap(four))
    assert 0 < first < 2**52
    assert executor._admit("h1", snap(list(reversed(four)))) == first
    twin = ce.Executor(executor.config, transports={})
    assert twin._admit("h2", snap(four)) == first  # the id is a function of the UUIDs alone
    other = ce.Executor(executor.config, transports={})
    assert other._admit("h1", snap([gpu(i) for i in range(4, 8)])) != first
    one_gpu_swapped = ce.Executor(executor.config, transports={})
    assert one_gpu_swapped._admit("h1", snap([gpu(0), gpu(1), gpu(2), gpu(4)])) != first


def test_one_machine_is_one_label_and_a_label_keeps_its_machine(executor: ce.Executor) -> None:
    box = snap([gpu(0), gpu(1)])
    executor._admit("h1", box)
    with pytest.raises(ce.HostRefused, match="same machine as host h1"):
        executor._admit("h2", snap([gpu(1), gpu(0)]))  # the same box listed under a second label
    assert executor._admit("h2", snap([gpu(2)])) != executor._admit("h1", box)
    with pytest.raises(ce.HostRefused, match="machine behind this host changed"):
        executor._admit("h1", snap([gpu(0), gpu(1), gpu(5)]))
    with pytest.raises(ce.HostRefused, match="machine behind this host changed"):
        executor._admit("h2", snap([gpu(0)]))


# -- selection ---------------------------------------------------------------------------------


def spec(
    *,
    n: int = 1,
    sm: int = SM_B300,
    drivers: tuple[str, ...] = (DRIVER,),
    gpu_class: str | None = None,
) -> Any:
    """The example reference for a CUDA host; `gpu_class` overrides the class its SM count names
    (model_copy does not validate, so a test can pair a class with a count it never admits)."""
    base = example_manifest().reference_spec
    layout = base.layout.model_copy(update={"n_gpus": n, "dp_size": n})
    pinned = EnvPins.model_validate(class_pin("avx2", gpu_class or CLASS_OF_SM[sm]))
    return base.model_copy(
        update={
            "sm_count": sm,
            "driver_allowlist": list(drivers),
            "layout": layout,
            "image_digest": DIGEST,
            "env": base.env.model_copy(update={"pinned": pinned}),
        }
    )


def test_select_takes_the_n_lowest_idle_uuids_in_uuid_order(executor: ce.Executor) -> None:
    gpus = [gpu(i) for i in range(5)]
    scrambled = [gpus[3], gpus[0], gpus[4], gpus[2], gpus[1]]
    chosen = executor._select(snap(scrambled, busy=(gpus[0],)), spec(n=2), IMAGE)
    assert [g.uuid for g in chosen] == [uid(1), uid(2)]


def test_select_idle_memory_limit_is_inclusive(executor: ce.Executor) -> None:
    limit = executor.config.idle_gpu_memory_mib
    assert executor._select(snap([gpu(0, mem=limit)]), spec(), IMAGE)[0].uuid == uid(0)
    with pytest.raises(ce.HostBusy):
        executor._select(snap([gpu(0, mem=limit + 1)]), spec(), IMAGE)


def test_select_a_gpu_with_a_compute_app_is_not_idle(executor: ce.Executor) -> None:
    gpus = [gpu(0), gpu(1)]
    with pytest.raises(ce.HostBusy, match="1 idle GPUs, 2 needed"):
        executor._select(snap(gpus, busy=(gpus[1],)), spec(n=2), IMAGE)


@pytest.mark.parametrize(
    "gpus",
    [
        pytest.param([gpu(0, cap=(9, 0))], id="capability-not-in-table"),
        pytest.param([gpu(0, cap=(12, 0))], id="sm-count-of-another-class"),
        pytest.param([gpu(0, driver="999.99")], id="driver-not-allowlisted"),
        pytest.param([gpu(0), gpu(1, driver="575.64.03")], id="drivers-differ"),
    ],
)
def test_select_refuses_a_host_that_contradicts_the_manifest(
    executor: ce.Executor, gpus: list[ce.Gpu]
) -> None:
    ref = spec(n=len(gpus), drivers=(DRIVER, "575.64.03") if len(gpus) > 1 else (DRIVER,))
    with pytest.raises(ce.HostRefused):
        executor._select(snap(gpus), ref, IMAGE)


@pytest.mark.parametrize("digests", [None, (), (f"{REPOSITORY}@sha256:{'cd' * 32}",)])
def test_select_refuses_an_image_the_host_does_not_report(
    executor: ce.Executor, digests: tuple[str, ...] | None
) -> None:
    with pytest.raises(ce.HostRefused, match="image digest absent"):
        executor._select(snap([gpu(0)], digests=digests), spec(), IMAGE)


def test_select_judges_only_the_gpus_it_would_use(executor: ce.Executor) -> None:
    other = gpu(1, cap=(9, 0), driver="999.99")
    assert [g.uuid for g in executor._select(snap([gpu(0), other]), spec(), IMAGE)] == [uid(0)]
    busy = gpu(0, cap=(9, 0), driver="999.99")
    assert [
        g.uuid for g in executor._select(snap([busy, gpu(1)], busy=(busy,)), spec(), IMAGE)
    ] == [uid(1)]


def test_select_accepts_the_5090_class_with_its_own_sm_count(executor: ce.Executor) -> None:
    chosen = executor._select(snap([gpu(0, cap=(12, 0))]), spec(sm=170), IMAGE)
    assert chosen[0].capability == (12, 0)


# -- environment cross-check -----------------------------------------------------------------


def observed(n: int = 1, **over: Any) -> ce.ObservedEnvironment:
    data: dict[str, Any] = {
        "backend": "cuda",
        "python": ce.IMAGE_PYTHON,
        "torch": "2.14.0+cu130",
        "torch_path": "/opt/hypertrain/venv/lib/python3.12/site-packages/torch/__init__.py",
        "cuda": "13.0",
        "gpu_uuids": [uid(i) for i in range(n)],
        "gpu_names": ["NVIDIA B300 SXM6 AC"] * n,
        "sm_counts": [SM_B300] * n,
        "capabilities": [list(B300)] * n,
        "drivers": [DRIVER] * n,
        "pinned_env": honest_report()["pinned_env"],
        "cpu_capability": PIN.capability,
        **over,
    }
    return ce.ObservedEnvironment.model_validate(data)


@pytest.mark.parametrize("field", ["python", "torch", "torch_path", "cuda"])
def test_observed_strings_are_bounded_to_256_characters(field: str) -> None:
    assert getattr(observed(**{field: "x" * 256}), field) == "x" * 256
    with pytest.raises(ValidationError):
        observed(**{field: "x" * 257})


@pytest.mark.parametrize("field", ["gpu_uuids", "gpu_names", "drivers"])
def test_observed_string_lists_bound_each_element_and_the_count(field: str) -> None:
    assert getattr(observed(**{field: ["x" * 256] * 8}), field) == ["x" * 256] * 8
    with pytest.raises(ValidationError):
        observed(**{field: ["x" * 257]})
    with pytest.raises(ValidationError):
        observed(**{field: ["x"] * 9})


@pytest.mark.parametrize(("field", "item"), [("sm_counts", SM_B300), ("capabilities", [10, 3])])
def test_observed_number_lists_are_bounded_to_8_entries(field: str, item: Any) -> None:
    assert len(getattr(observed(**{field: [item] * 8}), field)) == 8
    with pytest.raises(ValidationError):
        observed(**{field: [item] * 9})


def test_check_environment_returns_what_the_store_reads(executor: ce.Executor) -> None:
    chosen = [gpu(0), gpu(1)]
    record = executor._check_environment(observed(2), spec(n=2), chosen)
    assert record["backend"] == "cuda"
    assert record["image_digest"] == DIGEST
    assert record["drivers"] == [DRIVER, DRIVER]
    assert record["sm_counts"] == [SM_B300, SM_B300]
    assert record["gpu_uuids"] == [uid(0), uid(1)]
    assert record["capabilities"] == [[10, 3], [10, 3]]
    json.dumps(record)


CONTRADICTIONS = [
    pytest.param(dict(gpu_uuids=[uid(1), uid(0)]), id="uuids-reordered"),
    pytest.param(dict(gpu_uuids=[uid(0), uid(7)]), id="uuid-not-selected"),
    pytest.param(dict(gpu_uuids=[uid(0), uid(0)]), id="uuid-twice"),
    pytest.param(dict(gpu_uuids=[uid(0), "GPU-1234"]), id="uuid-malformed"),
    pytest.param(dict(gpu_uuids=[uid(0), NOT_HEX]), id="uuid-shaped-not-hex"),
    pytest.param(dict(gpu_uuids=[uid(0)]), id="uuid-missing"),
    pytest.param(dict(gpu_names=["n"]), id="names-missing"),
    pytest.param(dict(drivers=[DRIVER]), id="drivers-missing"),
    pytest.param(dict(sm_counts=[SM_B300]), id="sm-counts-missing"),
    pytest.param(dict(capabilities=[[10, 3]]), id="capabilities-missing"),
    pytest.param(dict(sm_counts=[SM_B300, 170]), id="sm-count-differs-from-manifest"),
    pytest.param(dict(sm_counts=[170, 170]), id="sm-count-of-another-class"),
    pytest.param(dict(capabilities=[[10, 3], [9, 0]]), id="capability-not-in-table"),
    pytest.param(
        dict(capabilities=[[12, 0], [12, 0]], sm_counts=[170, 170]), id="pair-valid-not-probed"
    ),
    pytest.param(dict(drivers=[DRIVER, "575.64.03"]), id="driver-differs-from-probe"),
    pytest.param(dict(drivers=["575.64.03", "575.64.03"]), id="driver-not-the-probed-one"),
]


@pytest.mark.parametrize("override", CONTRADICTIONS)
def test_check_environment_refuses_a_container_that_contradicts_the_probe(
    executor: ce.Executor, override: dict[str, Any]
) -> None:
    ref = spec(n=2, drivers=(DRIVER, "575.64.03"))
    with pytest.raises(ce.HostRefused):
        executor._check_environment(observed(2, **override), ref, [gpu(0), gpu(1)])


def test_check_environment_refuses_mixed_drivers_even_when_probed(executor: ce.Executor) -> None:
    mixed = [gpu(0), gpu(1, driver="575.64.03")]
    environment = observed(2, drivers=[DRIVER, "575.64.03"])
    with pytest.raises(ce.HostRefused):
        executor._check_environment(environment, spec(n=2, drivers=(DRIVER, "575.64.03")), mixed)


@pytest.mark.parametrize(
    ("probed", "claimed", "claimed_sm", "manifest_sm"),
    [
        pytest.param((9, 0), (9, 0), 148, 148, id="class-not-in-the-table"),
        pytest.param((12, 0), (12, 0), 170, 148, id="class-fits-its-sm-count-not-the-manifests"),
        pytest.param((12, 0), (10, 3), 148, 148, id="claims-a-class-the-probe-did-not-see"),
    ],
)
def test_check_environment_each_pairing_rule_refuses_on_its_own(
    executor: ce.Executor,
    probed: tuple[int, int],
    claimed: tuple[int, int],
    claimed_sm: int,
    manifest_sm: int,
) -> None:
    # Positive control: the same shapes pass when probe, claim, table and manifest agree.
    agreed = observed(capabilities=[[12, 0]], sm_counts=[170])
    executor._check_environment(agreed, spec(sm=170), [gpu(0, cap=(12, 0))])
    environment = observed(capabilities=[list(claimed)], sm_counts=[claimed_sm])
    with pytest.raises(ce.HostRefused):
        executor._check_environment(environment, spec(sm=manifest_sm), [gpu(0, cap=probed)])


def test_check_environment_refuses_a_driver_the_manifest_does_not_list(
    executor: ce.Executor,
) -> None:
    with pytest.raises(ce.HostRefused):
        executor._check_environment(observed(), spec(drivers=("575.64.03",)), [gpu(0)])


HONEST_ENV = honest_report()["pinned_env"]
PIN_CONTRADICTIONS = [
    pytest.param(
        dict(pinned_env={**HONEST_ENV, "HT_ATTN_BACKEND": "sdpa-flash"}),
        "HT_ATTN_BACKEND",
        id="another-attention-backend",
    ),
    pytest.param(dict(pinned_env={**HONEST_ENV, "HT_FP8": "1"}), "HT_FP8", id="fp8-switched-on"),
    pytest.param(
        dict(pinned_env={**HONEST_ENV, "OMP_THREAD_LIMIT": "2"}),
        "OMP_THREAD_LIMIT",
        id="a-removed-name-is-still-set",
    ),
    pytest.param(
        dict(pinned_env={**HONEST_ENV, "MKL_CBWR": None}), "MKL_CBWR", id="a-set-name-is-missing"
    ),
    pytest.param(
        dict(pinned_env={**HONEST_ENV, "OMP_DYNAMIC": "TRUE"}),
        "OMP_DYNAMIC",
        id="a-set-name-has-another-value",
    ),
    pytest.param(
        dict(pinned_env={**HONEST_ENV, "LD_PRELOAD": "/tmp/x"}),
        "LD_PRELOAD",
        id="a-name-outside-the-tables-is-set",
    ),
    pytest.param(
        dict(cpu_capability="DEFAULT"), "cpu_capability", id="the-cpu-cannot-honor-the-isa"
    ),
    pytest.param(
        dict(cpu_capability="AVX512"), "cpu_capability", id="the-cpu-resolves-another-isa"
    ),
]


@pytest.mark.parametrize(("override", "culprit"), PIN_CONTRADICTIONS)
def test_check_environment_refuses_a_container_that_does_not_show_the_manifest_pin(
    executor: ce.Executor, override: dict[str, Any], culprit: str
) -> None:
    chosen = [gpu(0), gpu(1)]
    executor._check_environment(observed(2), spec(n=2), chosen)  # the unedited twin is accepted
    with pytest.raises(ce.HostRefused, match=culprit):
        executor._check_environment(observed(2, **override), spec(n=2), chosen)


def test_check_environment_judges_the_gpu_class_apart_from_the_sm_table(
    executor: ce.Executor,
) -> None:
    # A 170-SM count on a (12, 0) device agrees with the host probe and with the SM table; the
    # manifest pins another class, which only the class comparison can see.
    environment = observed(capabilities=[[12, 0]], sm_counts=[170])
    executor._check_environment(environment, spec(sm=170), [gpu(0, cap=(12, 0))])
    with pytest.raises(ce.HostRefused, match="'gpu'"):
        executor._check_environment(
            environment, spec(sm=170, gpu_class="sm_103"), [gpu(0, cap=(12, 0))]
        )


def test_check_environment_shows_a_name_the_host_chose_through_a_short_repr(
    executor: ce.Executor,
) -> None:
    name = "N" * 200 + "\nforged line"
    environment = observed(pinned_env={**HONEST_ENV, name: "1"})
    with pytest.raises(ce.HostRefused) as refused:
        executor._check_environment(environment, spec(), [gpu(0)])
    assert "forged line" not in str(refused.value) and "\n" not in str(refused.value)
    assert len(str(refused.value)) < 200


@pytest.mark.parametrize(
    "broken",
    [
        pytest.param({"pinned_env": None}, id="pinned-env-missing"),
        pytest.param({"cpu_capability": None}, id="cpu-capability-missing"),
        pytest.param({"pinned_env": {f"HT_NAME_{i}": None for i in range(17)}}, id="17-names"),
        pytest.param({"pinned_env": {**HONEST_ENV, "HT_FP8": 1}}, id="a-value-that-is-a-number"),
        pytest.param({"pinned_env": {**HONEST_ENV, "HT_FP8": "x" * 257}}, id="a-value-too-long"),
        pytest.param({"pinned_env": list(HONEST_ENV)}, id="not-an-object"),
        pytest.param({"cpu_capability": ["AVX2"]}, id="capability-not-a-string"),
    ],
)
def test_a_report_of_the_wrong_shape_is_a_malformed_result_and_not_a_refused_host(
    executor: ce.Executor, tmp_path: Path, broken: dict[str, Any]
) -> None:
    document = {**observed(2).model_dump(), **broken}
    document = {k: v for k, v in document.items() if v is not None}
    entries = result_entries(environment=json.dumps(document).encode())
    with pytest.raises(ce.AttemptFailed, match="result metadata is malformed"):
        read(executor, tmp_path, tar_of(entries))


# -- result tar --------------------------------------------------------------------------------

REQUEST = b'{"version": 1, "request": "as received by the runner"}'


def tar_of(entries: list[tuple[str, bytes, bytes]]) -> bytes:
    """(name, payload, tar type) members; link-like types carry a target in the payload."""
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w", format=tarfile.PAX_FORMAT) as archive:
        for name, payload, kind in entries:
            info = tarfile.TarInfo(name)
            info.type = kind
            if kind in (tarfile.SYMTYPE, tarfile.LNKTYPE):
                info.linkname = payload.decode()
            else:
                info.size = len(payload)
            archive.addfile(info, io.BytesIO(payload) if info.size else None)
    return buffer.getvalue()


def result_entries(
    n: int = 2, request: bytes = REQUEST, **override: Any
) -> list[tuple[str, bytes, bytes]]:
    files = {f"rank-{r}/summary.json": f"summary {r}".encode() for r in range(n)}
    files["rank-0/checkpoints/0.safetensors"] = b"checkpoint"
    environment = override.get("environment", json.dumps(observed(n).model_dump()).encode())
    listing = {**{k: sha(v) for k, v in files.items()}, "environment.json": sha(environment)}
    done = override.get(
        "done",
        json.dumps({"version": 1, "files": listing, "request_sha256": sha(request)}).encode(),
    )
    regular = tarfile.REGTYPE
    return [
        *((name, raw, regular) for name, raw in files.items()),
        ("environment.json", environment, regular),
        ("COMPLETE", done, regular),
    ]


def read(
    executor: ce.Executor, tmp_path: Path, raw: bytes, n: int = 2, request: bytes = REQUEST
) -> tuple[dict[str, str], ce.ObservedEnvironment]:
    tar_path = tmp_path / "sandbox" / "result.tar"
    tar_path.parent.mkdir(parents=True, exist_ok=True)
    tar_path.write_bytes(raw)
    return executor._read_result(tar_path, tmp_path / "sandbox" / "stage", n, request)


def test_read_result_extracts_ranks_and_returns_the_hashes(
    executor: ce.Executor, tmp_path: Path
) -> None:
    written, environment = read(executor, tmp_path, tar_of(result_entries()))
    stage = tmp_path / "sandbox" / "stage"
    assert written == {
        "rank-0/summary.json": sha(b"summary 0"),
        "rank-0/checkpoints/0.safetensors": sha(b"checkpoint"),
        "rank-1/summary.json": sha(b"summary 1"),
    }
    assert (stage / "rank-0/checkpoints/0.safetensors").read_bytes() == b"checkpoint"
    assert not (stage / "environment.json").exists()  # metadata never reaches the publication
    assert not (stage / "COMPLETE").exists()
    assert environment == observed(2)


def test_read_result_accepts_what_the_runner_writes(executor: ce.Executor, tmp_path: Path) -> None:
    attempt = tmp_path / "attempt"
    (attempt / "rank-0" / "checkpoints").mkdir(parents=True)
    (attempt / "rank-1").mkdir()
    (attempt / "rank-0/summary.json").write_bytes(b"s0")
    (attempt / "rank-0/checkpoints/0.safetensors").write_bytes(b"c" * 5000)
    (attempt / "rank-1/summary.json").write_bytes(b"s1")
    buffer = io.BytesIO()
    environment = observed(2).model_dump(mode="json")
    runner.write_outputs(buffer, attempt, environment, REQUEST)
    written, parsed = read(executor, tmp_path, buffer.getvalue())
    assert sorted(written) == [
        "rank-0/checkpoints/0.safetensors",
        "rank-0/summary.json",
        "rank-1/summary.json",
    ]
    assert parsed == observed(2)
    with pytest.raises(ce.AttemptFailed, match="COMPLETE does not match"):
        read(executor, tmp_path / "again", buffer.getvalue(), request=REQUEST + b" ")


def altered(name: str, payload: bytes) -> list[tuple[str, bytes, bytes]]:
    entries = result_entries()
    return [(n, payload, k) if n == name else (n, p, k) for n, p, k in entries]


def complete_with(**changes: Any) -> bytes:
    done = json.loads(result_entries()[-1][1])
    done.update(changes)
    return json.dumps(done).encode()


def with_member(name: str, payload: bytes, kind: bytes = tarfile.REGTYPE, at: int = 0) -> list:
    entries = result_entries()
    entries.insert(at, (name, payload, kind))
    return entries


def drop(name: str) -> list[tuple[str, bytes, bytes]]:
    return [e for e in result_entries() if e[0] != name]


def environment_with(**changes: Any) -> bytes:
    return json.dumps({**observed(2).model_dump(), **changes}).encode()


MEMBERS = [
    pytest.param(with_member("rank-0/link", b"/etc/passwd", tarfile.SYMTYPE), id="symlink"),
    pytest.param(
        with_member("rank-0/hard", b"rank-0/summary.json", tarfile.LNKTYPE), id="hardlink"
    ),
    pytest.param(with_member("rank-0/folder", b"", tarfile.DIRTYPE), id="directory"),
    pytest.param(with_member("rank-0/pipe", b"", tarfile.FIFOTYPE), id="fifo"),
    pytest.param(with_member("rank-0/tty", b"", tarfile.CHRTYPE), id="character-device"),
    # TarInfo.isreg() is true for both of these; only plain regular files are results.
    pytest.param(with_member("rank-0/contiguous", b"x", tarfile.CONTTYPE), id="contiguous-file"),
    pytest.param(with_member("rank-0/sparse", b"x", tarfile.GNUTYPE_SPARSE), id="gnu-sparse-file"),
    pytest.param(with_member("/etc/passwd", b"x"), id="absolute"),
    pytest.param(with_member("../escape", b"x"), id="parent-directory"),
    pytest.param(with_member("rank-0/../../escape", b"x"), id="embedded-parent-directory"),
    pytest.param(with_member("rank-0//x", b"x"), id="empty-component"),
    pytest.param(with_member("rank-0/./x", b"x"), id="dot-component"),
    pytest.param(with_member("rank-0/dir\\file", b"x"), id="backslash"),
    pytest.param(with_member("rank-2/summary.json", b"x"), id="rank-out-of-range"),
    pytest.param(with_member("rank-x/summary.json", b"x"), id="rank-not-a-number"),
    pytest.param(with_member("rank-00/summary.json", b"x"), id="rank-not-canonical"),
    pytest.param(with_member("rank-0", b"x"), id="rank-as-a-file"),
    # a host must not seed the dispute traces that launch_window treats as already computed
    pytest.param(with_member("rank-0/windows/1.json", b"[]"), id="dispute-window-trace"),
    pytest.param(with_member("rank-" + "9" * 5000 + "/x", b"x"), id="rank-with-5000-digits"),
    pytest.param(with_member("other/summary.json", b"x"), id="foreign-directory"),
    pytest.param(with_member("extra.json", b"x"), id="foreign-top-level-file"),
    pytest.param(with_member("rank-0/summary.json", b"again"), id="duplicate-rank-file"),
    pytest.param(with_member("environment.json", b"{}"), id="duplicate-environment"),
    pytest.param(with_member("COMPLETE", b"{}", at=-1), id="duplicate-complete"),
    pytest.param(
        with_member("rank-1/late.json", b"x", at=len(result_entries())), id="after-complete"
    ),
]


@pytest.mark.parametrize("entries", MEMBERS)
def test_read_result_refuses_an_unacceptable_member(
    executor: ce.Executor, tmp_path: Path, entries: list
) -> None:
    with pytest.raises(ce.AttemptFailed, match="unacceptable result member"):
        read(executor, tmp_path, tar_of(entries))
    assert list(tmp_path.rglob("escape")) == []


def test_read_result_refuses_a_name_with_a_nul_that_only_a_pax_path_can_carry(
    executor: ce.Executor, tmp_path: Path
) -> None:
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w", format=tarfile.PAX_FORMAT) as archive:
        info = tarfile.TarInfo("rank-0/placeholder")
        info.size = 1
        info.pax_headers = {"path": "rank-0/na\0me"}
        archive.addfile(info, io.BytesIO(b"x"))
        for name, raw, _ in result_entries():
            info = tarfile.TarInfo(name)
            info.size = len(raw)
            archive.addfile(info, io.BytesIO(raw))
    with pytest.raises(ce.AttemptFailed, match="unacceptable result member 'rank-0/na"):
        read(executor, tmp_path, buffer.getvalue())


def pax_sparse_result(payload: bytes, realsize: int) -> bytes:
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w", format=tarfile.PAX_FORMAT) as archive:
        info = tarfile.TarInfo("rank-0/big.bin")
        info.size = len(payload)
        info.pax_headers = {
            "GNU.sparse.major": "1",
            "GNU.sparse.minor": "0",
            "GNU.sparse.realsize": str(realsize),
            "GNU.sparse.name": "rank-0/big.bin",
        }
        archive.addfile(info, io.BytesIO(payload))
        for name, raw, _ in result_entries():
            info = tarfile.TarInfo(name)
            info.size = len(raw)
            archive.addfile(info, io.BytesIO(raw))
    return buffer.getvalue()


@pytest.mark.parametrize(
    ("payload", "realsize", "message"),
    [
        # a well-formed PAX 1.0 sparse map whose logical size is small: tarfile reports a plain
        # regular file; only its pax header tells that it is sparse.
        (b"1\n0\n4\n".ljust(512, b"\0") + b"abcd", 4, "unacceptable result member"),
        # the same with a terabyte of logical size: the byte cap refuses it as well
        (b"1\n0\n4\n".ljust(512, b"\0") + b"abcd", 1 << 40, "unacceptable result member"),
        # a map tarfile itself cannot read raises ValueError from the iterator
        (b"not a sparse map", 4, "result tar unreadable"),
    ],
    ids=["well-formed", "terabyte", "malformed"],
)
def test_read_result_refuses_a_pax_sparse_member_whatever_tarfile_makes_of_it(
    executor: ce.Executor, tmp_path: Path, payload: bytes, realsize: int, message: str
) -> None:
    with pytest.raises(ce.AttemptFailed, match=message):
        read(executor, tmp_path, pax_sparse_result(payload, realsize))
    assert not (tmp_path / "sandbox" / "stage" / "rank-0" / "big.bin").exists()


@pytest.mark.parametrize("n_gpus", [ce.MAX_GPUS, ce.MAX_GPUS + 1])
def test_launch_refuses_a_layout_wider_than_the_provenance_can_record_before_any_host(
    tmp_path: Path, pinned: tuple[Path, Path], n_gpus: int
) -> None:
    job = island_job(tmp_path / "job")
    reference = job.manifest.training.reference_spec
    layout = reference.layout.model_copy(update={"n_gpus": n_gpus, "dp_size": n_gpus})
    training = job.manifest.training.model_copy(
        update={"reference_spec": reference.model_copy(update={"layout": layout})}
    )
    wide = job.model_copy(
        update={"manifest": job.manifest.model_copy(update={"training": training})}
    )
    order: list[str] = []
    executor = executor_with(tmp_path, pinned, {"h1": Down("h1", order)}, sweeps=1)
    reaches_a_host = n_gpus <= ce.MAX_GPUS  # the dp8 box is the largest layout it carries
    expected = "no host able to run the job" if reaches_a_host else "at most 8 GPUs"
    with pytest.raises(ce.IslandFailure, match=expected) as caught:
        executor.launch("audit", "b" * 64, wide, tmp_path / "job", trace=False, cancel=None)
    assert order == (["h1"] if reaches_a_host else []) and not isinstance(caught.value, ce.Aborted)
    assert executor.journal.path.exists() is reaches_a_host


def test_read_result_still_accepts_old_style_regular_members(
    executor: ce.Executor, tmp_path: Path
) -> None:
    legacy = [(name, raw, tarfile.AREGTYPE) for name, raw, _ in result_entries()]
    written, environment = read(executor, tmp_path, tar_of(legacy))
    assert sorted(written) == [
        "rank-0/checkpoints/0.safetensors",
        "rank-0/summary.json",
        "rank-1/summary.json",
    ]
    assert environment == observed(2)


def entries_for(files: dict[str, bytes], n: int = 2) -> list[tuple[str, bytes, bytes]]:
    """`files` plus a matching environment.json and COMPLETE; the metadata size does not depend on
    the content or size of the files."""
    environment = json.dumps(observed(n).model_dump()).encode()
    listing = {
        **{name: sha(raw) for name, raw in files.items()},
        "environment.json": sha(environment),
    }
    done = json.dumps({"version": 1, "files": listing, "request_sha256": sha(REQUEST)}).encode()
    return [
        *((name, raw, tarfile.REGTYPE) for name, raw in files.items()),
        ("environment.json", environment, tarfile.REGTYPE),
        ("COMPLETE", done, tarfile.REGTYPE),
    ]


def capped(tmp_path: Path, pinned: tuple[Path, Path], cap: int = 1 << 20) -> ce.Executor:
    config = load_config(config_data(tmp_path, pinned, max_result_bytes=cap))
    return ce.Executor(config, transports={})


def test_read_result_bounds_the_sum_of_all_members_not_each_member(
    tmp_path: Path, pinned: tuple[Path, Path]
) -> None:
    cap = 1 << 20  # the smallest the config accepts
    two = {"rank-0/a.bin": b"a" * 600_000, "rank-1/b.bin": b"b" * 600_000}
    assert max(map(len, two.values())) < cap < sum(map(len, two.values()))
    with pytest.raises(ce.AttemptFailed, match="unacceptable result member 'rank-1/b.bin'"):
        read(capped(tmp_path, pinned, cap), tmp_path, tar_of(entries_for(two)))
    stage = tmp_path / "sandbox" / "stage"
    assert (stage / "rank-0/a.bin").is_file()  # what fitted was extracted ...
    assert not (stage / "rank-1/b.bin").exists()  # ... and nothing past the member that did not


@pytest.mark.parametrize("name", ["summary.json", "leaves.json", "trace.json"])
@pytest.mark.parametrize(("extra", "fits"), [(0, True), (1, False)], ids=["at-the-cap", "over"])
def test_read_result_bounds_the_json_members_of_a_rank_before_anything_parses_them(
    executor: ce.Executor,
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    name: str,
    extra: int,
    fits: bool,
) -> None:
    # json.loads grows a 14 MiB file of "[]," items by 365 MiB: the bound is on the bytes parsed
    monkeypatch.setattr(ce, "RANK_JSON_BYTES", 4096)
    member = f"rank-0/{name}"
    files = {member: b"x" * (4096 + extra)}
    if fits:
        written = read(executor, tmp_path, tar_of(entries_for(files)))[0]
        assert written == {member: sha(files[member])}
    else:
        with pytest.raises(ce.AttemptFailed, match=f"unacceptable result member '{member}'"):
            read(executor, tmp_path, tar_of(entries_for(files)))
        assert not (tmp_path / "sandbox" / "stage" / member).exists()


def test_read_result_json_cap_is_for_the_json_members_of_a_rank_only(
    executor: ce.Executor, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(ce, "RANK_JSON_BYTES", 4096)
    files = {"rank-0/state.safetensors": b"x" * 5000, "rank-0/checkpoints/trace.json": b"x" * 5000}
    assert sorted(read(executor, tmp_path, tar_of(entries_for(files)))[0]) == sorted(files)


def test_read_result_cap_is_inclusive_of_the_metadata_members(
    tmp_path: Path, pinned: tuple[Path, Path]
) -> None:
    cap = 1 << 20
    bare = entries_for({"rank-0/a.bin": b""})
    # every member costs its payload plus its 512-byte tar header
    fixed = sum(len(raw) + tarfile.BLOCKSIZE for _, raw, _ in bare)
    fits = {"rank-0/a.bin": b"a" * (cap - fixed)}  # every member together: exactly `cap` bytes
    written, _ = read(capped(tmp_path, pinned, cap), tmp_path / "fits", tar_of(entries_for(fits)))
    assert written == {"rank-0/a.bin": sha(fits["rank-0/a.bin"])}
    over = {"rank-0/a.bin": b"a" * (cap - fixed + 1)}  # one byte more: the last member tips it
    with pytest.raises(ce.AttemptFailed, match="unacceptable result member 'COMPLETE'"):
        read(capped(tmp_path, pinned, cap), tmp_path / "over", tar_of(entries_for(over)))


def test_read_result_bounds_the_number_of_members_not_only_their_bytes(
    tmp_path: Path, executor: ce.Executor
) -> None:
    # Header-only members cost the byte cap nothing; a host must not be able to fill the inodes
    # of the staging filesystem with them (the default cap is 16 GiB).
    flood = [(f"rank-0/empty-{i}", b"", tarfile.REGTYPE) for i in range(ce.MAX_MEMBERS + 1)]
    with pytest.raises(ce.AttemptFailed, match="unacceptable result member"):
        read(executor, tmp_path, tar_of([*flood, *result_entries()]))
    staged = [p for p in (tmp_path / "sandbox" / "stage").rglob("*") if p.is_file()]
    assert len(staged) <= ce.MAX_MEMBERS


def test_read_result_member_cap_is_inclusive(
    executor: ce.Executor, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    members = len(result_entries())
    monkeypatch.setattr(ce, "MAX_MEMBERS", members)  # a result of exactly the cap is fine ...
    written, _ = read(executor, tmp_path / "fits", tar_of(result_entries()))
    assert len(written) == members - 2
    monkeypatch.setattr(ce, "MAX_MEMBERS", members - 1)  # ... one member more is not
    with pytest.raises(ce.AttemptFailed, match="unacceptable result member 'COMPLETE'"):
        read(executor, tmp_path / "over", tar_of(result_entries()))


@pytest.mark.parametrize(("extra", "fits"), [(0, True), (1, False)], ids=["at-the-cap", "over"])
def test_read_result_small_member_cap_is_inclusive(
    executor: ce.Executor, tmp_path: Path, extra: int, fits: bool
) -> None:
    environment = json.dumps(observed(2).model_dump()).encode().ljust(ce.SMALL + extra)
    raw = tar_of(result_entries(environment=environment))
    if fits:
        assert read(executor, tmp_path, raw)[1] == observed(2)
    else:
        with pytest.raises(ce.AttemptFailed, match="result member 'environment.json' too large"):
            read(executor, tmp_path, raw)


@pytest.mark.parametrize(
    "name", ["known hosts", "known%h", "known\ttab", "relative/known_hosts"], ids=str
)
def test_known_hosts_path_that_ssh_would_misread_is_refused(
    tmp_path: Path, pinned: tuple[Path, Path], name: str, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.chdir(tmp_path)  # the relative case must not write into the checkout
    # OpenSSH reads UserKnownHostsFile as a list of files and expands % tokens: a path with a space
    # would silently pin nothing and every host would fail as merely lost.
    path = tmp_path / name if not name.startswith("relative") else Path(name)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("[h]:22 ssh-ed25519 AAAA\n")
    data = with_value(config_data(tmp_path, pinned), ("hosts", 0, "known_hosts_file"), str(path))
    with pytest.raises(ValidationError, match="absolute path without spaces"):
        load_config(data)


COLLISIONS = [
    pytest.param([("rank-0/a", b"1"), ("rank-0/a/b", b"2")], id="directory-over-a-file"),
    pytest.param([("rank-0/a/b", b"2"), ("rank-0/a", b"1")], id="file-over-a-directory"),
    pytest.param([("rank-0/" + "n" * 300, b"1")], id="name-too-long"),
]


@pytest.mark.parametrize("extra", COLLISIONS)
def test_read_result_names_the_filesystem_cannot_hold_fail_the_attempt(
    executor: ce.Executor, tmp_path: Path, extra: list[tuple[str, bytes]]
) -> None:
    # The failure model retries an unsafe result on another host; a raw OSError would abort the
    # whole launch instead.
    members = [(name, raw, tarfile.REGTYPE) for name, raw in extra]
    with pytest.raises(ce.AttemptFailed):
        read(executor, tmp_path, tar_of([*members, *result_entries()[:-1]]))


INCONSISTENT = [
    pytest.param(drop("COMPLETE"), "missing environment.json or the COMPLETE", id="no-complete"),
    pytest.param(drop("environment.json"), "missing environment.json or the COMPLETE", id="no-env"),
    pytest.param(altered("COMPLETE", b"not json"), "metadata is malformed", id="complete-garbage"),
    pytest.param(altered("COMPLETE", b"[]"), "COMPLETE does not match", id="complete-list"),
    pytest.param(
        altered("COMPLETE", complete_with(version=2)), "COMPLETE does not match", id="version"
    ),
    pytest.param(
        altered("COMPLETE", complete_with(request_sha256=sha(b"another request"))),
        "COMPLETE does not match",
        id="request-hash",
    ),
    pytest.param(
        altered(
            "COMPLETE",
            complete_with(
                files={
                    **json.loads(result_entries()[-1][1])["files"],
                    "rank-0/summary.json": "0" * 64,
                }
            ),
        ),
        "COMPLETE does not match",
        id="altered-file-hash",
    ),
    pytest.param(
        altered(
            "COMPLETE",
            complete_with(
                files={**json.loads(result_entries()[-1][1])["files"], "rank-1/x": "1" * 64}
            ),
        ),
        "COMPLETE does not match",
        id="listed-file-not-sent",
    ),
    pytest.param(
        altered(
            "COMPLETE",
            complete_with(
                files={
                    k: v
                    for k, v in json.loads(result_entries()[-1][1])["files"].items()
                    if k != "rank-1/summary.json"
                }
            ),
        ),
        "COMPLETE does not match",
        id="sent-file-not-listed",
    ),
    pytest.param(
        altered("COMPLETE", b"{" + b" " * (1 << 20) + b"}"), "too large", id="oversize-complete"
    ),
    pytest.param(
        altered("environment.json", b"{" + b" " * (1 << 20) + b"}"), "too large", id="oversize-env"
    ),
    pytest.param(
        result_entries(environment=b"not json"), "metadata is malformed", id="environment-garbage"
    ),
    pytest.param(
        result_entries(environment=environment_with(backend="cpu")),
        "metadata is malformed",
        id="environment-cpu",
    ),
    pytest.param(
        result_entries(environment=environment_with(sm_counts=[148.5, 148])),
        "metadata is malformed",
        id="environment-float",
    ),
    pytest.param(
        result_entries(environment=environment_with(surprise=1)),
        "metadata is malformed",
        id="environment-extra-field",
    ),
    pytest.param(
        result_entries(environment=environment_with(torch_path="/x" * 2500)),
        "metadata is malformed",
        id="environment-5000-character-path",
    ),
    # Numbers that only look like the integers the manifest pins are not the container's report.
    pytest.param(
        result_entries(environment=environment_with(sm_counts=[148.0, 148.0])),
        "metadata is malformed",
        id="environment-integral-float",
    ),
    pytest.param(
        result_entries(environment=environment_with(sm_counts=["148", "148"])),
        "metadata is malformed",
        id="environment-numeric-string",
    ),
    pytest.param(
        result_entries(environment=environment_with(capabilities=[[10.0, 3], [10, 3]])),
        "metadata is malformed",
        id="environment-integral-float-capability",
    ),
]


@pytest.mark.parametrize(("entries", "message"), INCONSISTENT)
def test_read_result_refuses_metadata_that_does_not_vouch_for_the_bytes(
    executor: ce.Executor, tmp_path: Path, entries: list, message: str
) -> None:
    with pytest.raises(ce.AttemptFailed, match=message):
        read(executor, tmp_path, tar_of(entries))


def test_read_result_never_accepts_a_cut_or_foreign_stream(
    executor: ce.Executor, tmp_path: Path
) -> None:
    whole = tar_of(result_entries())
    before_complete = tar_of(result_entries()[:-1])
    with tarfile.open(fileobj=io.BytesIO(whole)) as archive:
        last = archive.getmember("COMPLETE")
    cases = {
        "cut-inside-the-last-member": whole[: last.offset_data + last.size // 2],
        "cut-before-complete": before_complete,
        "cut-inside-a-header": before_complete[:700],
        "garbage": b"HTTP/1.1 502 Bad Gateway\r\n" * 100,
        "empty": b"",
    }
    for name, raw in cases.items():
        with pytest.raises(ce.AttemptFailed):
            read(executor, tmp_path / name, raw)


def test_check_rank_refuses_a_trace_over_the_budget_of_the_audit_reader(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from hypertrain.auditor import island_bisect

    monkeypatch.setattr(island_bisect, "TRACE_BYTES", 8)
    rank = tmp_path / "rank-0"
    rank.mkdir()
    (rank / "summary.json").write_bytes(b'{"backend": "cuda"}')
    (rank / "trace.json").write_bytes(b"[" + b" " * 16 + b"]")  # a JSON list, over the budget
    with pytest.raises(ce.IslandFailure, match="requested trace is absent or malformed"):
        ce._check_rank(rank, trace=True)


# -- documents a host wrote --------------------------------------------------------------------


def nested(depth: int) -> bytes:
    return b"[" * depth + b"]" * depth


# The runner writes 2 levels in COMPLETE and 3 in environment.json: the limit is 8.
@pytest.mark.parametrize(("depth", "accepted"), [(8, True), (9, False)])
def test_shallow_nesting_limit_is_inclusive(depth: int, accepted: bool) -> None:
    if accepted:
        assert ce.shallow(nested(depth)) == nested(depth).decode()
    else:
        with pytest.raises(ce.HostRefused, match="nests deeper"):
            ce.shallow(nested(depth))


@pytest.mark.parametrize(
    "raw",
    [b"[" * 100_000, b'{"a":' * 100_000, nested(100_000)],
    ids=["opening-brackets", "opening-objects", "closed-brackets"],
)
def test_shallow_refuses_a_bomb_before_any_parser_sees_it(raw: bytes) -> None:
    with pytest.raises(ce.HostRefused):
        ce.shallow(raw)


def test_shallow_counts_nesting_not_brackets() -> None:
    # closers bring the depth back down, and brackets inside a string (even one that holds an
    # escaped quote) do not count
    again = nested(8) * 3
    inside = json.dumps({"a": "[" * 100, "b": '\\"' + "{" * 100}).encode()
    assert ce.shallow(again) == again.decode() and ce.shallow(inside) == inside.decode()


@pytest.mark.parametrize(
    "entries",
    [
        pytest.param(result_entries(done=nested(100_000)), id="complete-100000-levels"),
        pytest.param(result_entries(done=nested(9)), id="complete-one-level-too-deep"),
        pytest.param(result_entries(environment=nested(100_000)), id="environment-100000-levels"),
    ],
)
def test_read_result_refuses_the_host_whose_metadata_nests_beyond_any_runner(
    executor: ce.Executor, tmp_path: Path, entries: list[tuple[str, bytes, bytes]]
) -> None:
    with pytest.raises(ce.HostRefused, match="nests deeper"):
        read(executor, tmp_path, tar_of(entries))


def test_read_result_takes_the_metadata_as_utf8_and_nothing_else(
    executor: ce.Executor, tmp_path: Path
) -> None:
    # json.loads(bytes) would take UTF-16 for a document, and a parser differential is a way in
    entries = result_entries()
    done = dict((name, raw) for name, raw, _ in entries)["COMPLETE"]
    entries = [
        (n, done.decode().encode("utf-16") if n == "COMPLETE" else p, k) for n, p, k in entries
    ]
    with pytest.raises(ce.AttemptFailed, match="result metadata is malformed"):
        read(executor, tmp_path, tar_of(entries))


def test_the_tar_reader_serves_a_metadata_sized_read_and_nothing_larger(tmp_path: Path) -> None:
    path = tmp_path / "x"
    path.write_bytes(b"a" * 10)
    with ce._CappedReader(io.FileIO(path)) as reader:
        assert reader.read(ce.SMALL) == b"a" * 10  # members are streamed in chunks of this size
        for size in (ce.SMALL + 1, 1 << 62, -1, None):
            with pytest.raises(ce.HostRefused, match="beyond the metadata bound"):
                reader.read(size)


# -- provenance identity -----------------------------------------------------------------------


class FakeHost:
    """The remote commands of an attempt: the host probe, the compute cache directory (answered
    with the path it was asked for, below a home of /home/ubuntu), and the container (a runner
    that answers with the right COMPLETE for the request it received)."""

    def __init__(self, gpus: list[ce.Gpu], busy: tuple[ce.Gpu, ...] = ()) -> None:
        self.gpus, self.busy, self.sent, self.prepared = gpus, busy, [], []

    def run(self, command: str, *, stdin: Path | None, stdout: Path, **kwargs: Any) -> int:
        if command.startswith("D="):
            self.prepared.append(command)
            asked = shlex.split(command.split(";")[0])[0].removeprefix("D=")
            stdout.write_text(asked.replace("$HOME", "/home/ubuntu") + "\n")
            return 0
        if "##apps" in command:
            rows = [
                f"{g.uuid}, {g.name}, {g.driver}, {g.capability[0]}.{g.capability[1]}, 810"
                for g in self.gpus
            ]
            apps = [f"{g.uuid}, 99, python" for g in self.busy]
            text = "\n".join([*rows, "##apps", *apps, "##epoch", str(int(time.time()))])
            stdout.write_text(text + f'\n##image\n["{IMAGE}"]\n')
            return 0
        assert stdin is not None
        with tarfile.open(stdin) as archive:
            request = archive.extractfile("request.json").read()  # type: ignore[union-attr]
        uuids = json.loads(request)["uuids"]
        environment = json.dumps(observed(1, gpu_uuids=uuids).model_dump()).encode()
        self.sent.append(tar_of(result_entries(1, request, environment=environment)))
        stdout.write_bytes(self.sent[-1])
        return 0


def island_job(
    directory: Path, *, cpu: str = "avx2", gpu: str = "sm_103", attention: str = "math"
) -> IslandJobV1:
    from tests.miner.test_island_launch_v2 import tiny_manifest

    body = tiny_manifest(1).body()
    body["training"]["reference_spec"].update(
        sm_count=SM_B300 if gpu in ("sm_103", "none") else 170,
        driver_allowlist=[DRIVER],
        image_digest=DIGEST,
    )
    body["training"]["reference_spec"]["env"]["pinned"] = class_pin(cpu, gpu, attention)
    manifest = RunManifestV2.model_validate(body)
    directory.mkdir(parents=True, exist_ok=True)
    (directory / "start_state").write_bytes(b"state")
    return IslandJobV1(
        job_version=1,
        run_id=manifest.run_id(),
        w=1,
        manifest=manifest,
        sample_ids=list(range(manifest.training.batch_samples())),
        global_step0=0,
        start_state_sha256="11" * 32,
        ef_in_sha256="22" * 32,
        v0_sha256="33" * 32,
        object_paths={"start_state": "start_state"},
        deadline=int(time.time()) + 600,
    )


def test_attempt_record_tells_machines_apart_by_gpus_and_instances_by_address_and_host_keys(
    tmp_path: Path, pinned: tuple[Path, Path], monkeypatch: pytest.MonkeyPatch
) -> None:
    other_keys = tmp_path / "known_hosts_2"
    other_keys.write_text("gpu-1.example.com ssh-ed25519 BBBB\n")
    layout = {
        "h1": ("gpu-1.example.com", 22, pinned[1]),
        "h2": ("gpu-2.example.com", 22, pinned[1]),  # another address
        "h3": ("gpu-1.example.com", 2222, pinned[1]),  # another port
        "h4": ("gpu-1.example.com", 22, other_keys),  # another host key
    }
    hosts = [
        host_data(pinned, label, address=address, port=port, known_hosts_file=str(known))
        for label, (address, port, known) in layout.items()
    ]
    boxes = {
        "h1": FakeHost([gpu(0), gpu(1)]),  # two GPUs, one of them leased per job
        "h2": FakeHost([gpu(2)]),
        "h3": FakeHost([gpu(3)]),
        "h4": FakeHost([gpu(4)]),
    }
    config = load_config(config_data(tmp_path, pinned, hosts=hosts))
    executor = ce.Executor(config, transports=boxes)
    records: list[dict[str, Any]] = []

    def capture(
        self: ce.Executor, job: Any, directory: Path, staging: Path, record: dict, trace: bool
    ) -> str:
        records.append(record)
        return "published"

    monkeypatch.setattr(ce.Executor, "_publish", capture)  # the record is what is under test
    job = island_job(tmp_path / "job")
    for label in layout:
        executor._attempt(label, 1, "audit", "b" * 64, job, tmp_path / "job", False, None)
    boxes["h1"].busy = (boxes["h1"].gpus[0],)  # the other GPU of the same box this time
    executor._attempt("h1", 2, "reference", "c" * 64, job, tmp_path / "job", False, None)
    by_attempt = {(r["host"]["label"], r["operation"]): r for r in records}
    first, again = by_attempt["h1", "audit"], by_attempt["h1", "reference"]

    assert first["environment"]["gpu_uuids"] == [uid(0)]
    assert again["environment"]["gpu_uuids"] == [uid(1)]
    assert first["host"] == {
        **again["host"],
        "label": "h1",
    }  # same box: same machine, same instance
    for record in records:
        assert 0 < record["host"]["machine_id"] < 2**52
        assert 0 < record["host"]["instance_id"] < 2**52
    machines = {r["host"]["machine_id"] for r in records if r["operation"] == "audit"}
    instances = {r["host"]["instance_id"] for r in records if r["operation"] == "audit"}
    assert len(machines) == len(instances) == 4
    assert first["host"]["known_hosts_sha256"] == sha(pinned[1].read_bytes())
    assert by_attempt["h4", "audit"]["host"]["known_hosts_sha256"] == sha(other_keys.read_bytes())
    assert (first["executor"], first["binding"], first["job_sha256"]) == (
        "cuda-executor-v1",
        "b" * 64,
        job.digest(),
    )
    assert first["image"] == {"requested": IMAGE, "repo_digests": [IMAGE]}
    assert first["runner_sha256"] == sha(ce.RUNNER_SOURCE.encode())
    assert first["result_tar_sha256"] == sha(boxes["h1"].sent[0])
    assert first["environment"]["image_digest"] == DIGEST
    canonicalize(first)  # JSON-safe: no float reaches role-dispatch.json


class Swapping(FakeHost):
    """A host whose pinned known_hosts file is rewritten while the attempt is talking to it."""

    def __init__(self, gpus: list[ce.Gpu], known: Path) -> None:
        super().__init__(gpus)
        self.known = known

    def run(self, command: str, **kwargs: Any) -> int:
        self.known.write_text("gpu-1.example.com ssh-ed25519 A-DIFFERENT-KEY-SWAPPED-IN-MID-RUN\n")
        return super().run(command, **kwargs)


def test_the_pinned_host_keys_are_read_once_before_the_first_ssh_call(
    tmp_path: Path, pinned: tuple[Path, Path], monkeypatch: pytest.MonkeyPatch
) -> None:
    records: list[dict[str, Any]] = []

    def capture(
        self: ce.Executor, job: Any, directory: Path, staging: Path, record: dict, trace: bool
    ):
        records.append(record)
        return "published"

    monkeypatch.setattr(ce.Executor, "_publish", capture)
    job = island_job(tmp_path / "job")
    original = pinned[1].read_bytes()
    steady = executor_with(tmp_path / "steady", pinned, {"h1": FakeHost([gpu(0)])})
    steady._attempt("h1", 1, "audit", "b" * 64, job, tmp_path / "job", False, None)
    assert pinned[1].read_bytes() == original

    swapped = executor_with(tmp_path / "swapped", pinned, {"h1": Swapping([gpu(0)], pinned[1])})
    swapped._attempt("h1", 1, "audit", "b" * 64, job, tmp_path / "job", False, None)

    assert pinned[1].read_bytes() != original  # the swap did happen during the attempt
    steady_host, swapped_host = records[0]["host"], records[1]["host"]
    # What ssh verified the host against is what the record names, however the file moved since.
    assert swapped_host["known_hosts_sha256"] == sha(original) == steady_host["known_hosts_sha256"]
    assert swapped_host["instance_id"] == steady_host["instance_id"]


# -- executor construction ---------------------------------------------------------------------


def test_a_missing_ssh_client_fails_loading_as_an_oserror(
    tmp_path: Path, pinned: tuple[Path, Path], monkeypatch: pytest.MonkeyPatch
) -> None:
    # The challenge maps exactly (OSError, ValueError) to 503 ROLE_DISPATCH_UNAVAILABLE.
    path = tmp_path / "executor.json"
    path.write_text(json.dumps(config_data(tmp_path, pinned)))
    asked: list[str] = []

    def which(command: str) -> str | None:
        asked.append(command)
        return None

    monkeypatch.setattr(ce.shutil, "which", which)
    with pytest.raises(FileNotFoundError, match="ssh client not found"):
        ce.load(path)
    assert asked == ["ssh"]
    monkeypatch.setattr(ce.shutil, "which", lambda command: "/usr/bin/ssh")
    assert callable(ce.load(path))


def test_state_and_log_directories_are_private_whatever_the_umask(
    tmp_path: Path, pinned: tuple[Path, Path]
) -> None:
    state = tmp_path / "var" / "executor"
    previous = os.umask(0)
    try:
        ce.Executor(load_config(config_data(tmp_path, pinned, state_dir=str(state))), transports={})
    finally:
        os.umask(previous)
    assert [stat.S_IMODE(p.stat().st_mode) for p in (state, state / "logs")] == [0o700, 0o700]


def test_state_and_log_directories_that_already_exist_are_made_private(
    tmp_path: Path, pinned: tuple[Path, Path]
) -> None:
    state = tmp_path / "var" / "executor"
    (state / "logs").mkdir(parents=True)
    for directory in (state, state / "logs"):
        directory.chmod(0o755)
    ce.Executor(load_config(config_data(tmp_path, pinned, state_dir=str(state))), transports={})
    assert [stat.S_IMODE(p.stat().st_mode) for p in (state, state / "logs")] == [0o700, 0o700]


# -- scripted hosts: probe and pull limits, host order, sweeps and attempts ----------------------


class FixedClock:
    """Stands in for the `time` module of cuda_executor with a `time()` that never moves."""

    def __init__(self, now: float) -> None:
        self.now = now

    def time(self) -> float:
        return self.now

    def __getattr__(self, name: str) -> Any:
        return getattr(time, name)


class Reply:
    """A Transport that answers every command with fixed text and exit code and remembers the
    deadline it was given."""

    def __init__(self, text: str = "", code: int = 0) -> None:
        self.text, self.code = text, code
        self.deadlines: list[float] = []

    def run(self, command: str, *, stdout: Path, deadline: float, **kwargs: Any) -> int:
        self.deadlines.append(deadline)
        stdout.write_text(self.text)
        return self.code


class Down:
    """A host that never answers; `order` collects the labels in the order they are tried."""

    def __init__(self, label: str, order: list[str]) -> None:
        self.label, self.order = label, order

    def run(self, command: str, **kwargs: Any) -> int:
        self.order.append(self.label)
        raise ce.HostLost("ssh could not reach or authenticate the host")


def kind_of(command: str) -> str:
    if command.startswith("nvidia-smi"):
        return "probe"
    return "prepare" if command.startswith("D=") else shlex.split(command)[1]


class ScriptedHost(FakeHost):
    """FakeHost whose n-th command of a kind can time out or fail (`script`), and whose image can
    be absent until pulled. Every command lands in the shared `log` as (label, kind)."""

    def __init__(
        self,
        label: str,
        log: list[tuple[str, str]],
        gpus: list[ce.Gpu],
        *,
        script: dict[str, list[str]] | None = None,
        absent: bool = False,
    ) -> None:
        super().__init__(gpus)
        self.label, self.log, self.absent, self.pulled = label, log, absent, False
        self.script = {kind: list(steps) for kind, steps in (script or {}).items()}

    def run(self, command: str, *, stdin: Path | None, stdout: Path, **kwargs: Any) -> int:
        kind = kind_of(command)
        self.log.append((self.label, kind))
        step = self.script[kind].pop(0) if self.script.get(kind) else "ok"
        if step == "timeout":
            raise ce.RemoteTimeout("remote command timed out")
        if step == "oom":
            kwargs["stderr"].write_text("CUDA out of memory\n")
            return 1
        if kind in ("pull", "rm"):
            self.pulled = self.pulled or kind == "pull"
            return 0
        code = super().run(command, stdin=stdin, stdout=stdout, **kwargs)
        if kind == "probe" and self.absent and not self.pulled:
            stdout.write_text(stdout.read_text().replace(f'["{IMAGE}"]', "ABSENT"))
        return code


def executor_with(
    tmp_path: Path, pinned: tuple[Path, Path], transports: dict[str, Any], **over: Any
) -> ce.Executor:
    """One host per transport, in order; no pause between sweeps unless `over` asks for one."""
    sleep = over.pop("sleep", None)
    hosts = [
        host_data(pinned, label, address=f"gpu-{number + 1}.example.com")
        for number, label in enumerate(transports)
    ]
    settings = {"sweep_pause_seconds": 0, **over}
    config = load_config(config_data(tmp_path, pinned, hosts=hosts, **settings))
    return ce.Executor(config, transports=transports, **({} if sleep is None else {"sleep": sleep}))


def journal_of(executor: ce.Executor, kind: str | None = None) -> list[dict[str, Any]]:
    rows = [json.loads(line) for line in executor.journal.path.read_text().splitlines() if line]
    return rows if kind is None else [row for row in rows if row["kind"] == kind]


def start_at(monkeypatch: pytest.MonkeyPatch, digest_head: str) -> None:
    """Give every job a known digest: the executor spreads jobs over its hosts by it."""
    digest = digest_head + "0" * (64 - len(digest_head))
    monkeypatch.setattr(IslandJobV1, "digest", lambda self: digest)


def stub_publication(monkeypatch: pytest.MonkeyPatch) -> None:
    """The result tar goes through the real reader; only the publication step is stubbed out."""
    monkeypatch.setattr(ce.Executor, "_publish", lambda self, *args: "published")


def probe_for(tmp_path: Path, pinned: tuple[Path, Path], reply: Reply, **over: Any) -> ce.Executor:
    (tmp_path / "work").mkdir()
    return executor_with(tmp_path, pinned, {"h1": reply}, **over)


def test_snapshot_with_a_non_zero_exit_loses_the_host_even_when_its_output_parses(
    tmp_path: Path, pinned: tuple[Path, Path]
) -> None:
    reply = Reply(probe_text(epoch=str(int(time.time()))), code=7)
    executor = probe_for(tmp_path, pinned, reply)
    with pytest.raises(ce.HostLost, match="host probe exited 7"):
        executor._snapshot("h1", IMAGE, time.time() + 600, None, tmp_path / "work")


@pytest.mark.parametrize(
    ("skew", "accepted"),
    [(0, True), (3, True), (-5, True), (5, True), (6, False), (-6, False), (8, False)],
)
def test_snapshot_accepts_a_host_clock_up_to_the_configured_skew_and_no_further(
    tmp_path: Path,
    pinned: tuple[Path, Path],
    monkeypatch: pytest.MonkeyPatch,
    skew: int,
    accepted: bool,
) -> None:
    now = 1_800_000_000.0
    monkeypatch.setattr(ce, "time", FixedClock(now))
    reply = Reply(probe_text(epoch=str(int(now) + skew)))
    executor = probe_for(tmp_path, pinned, reply, max_clock_skew_seconds=5)
    if accepted:
        assert executor._snapshot("h1", IMAGE, now + 600, None, tmp_path / "work").epoch == (
            int(now) + skew
        )
    else:
        with pytest.raises(ce.HostRefused, match="host clock differs from the challenge clock"):
            executor._snapshot("h1", IMAGE, now + 600, None, tmp_path / "work")


@pytest.mark.parametrize(("skew", "accepted"), [(8, True), (10, True), (11, False)])
def test_the_configured_skew_limit_is_the_one_applied(
    tmp_path: Path,
    pinned: tuple[Path, Path],
    monkeypatch: pytest.MonkeyPatch,
    skew: int,
    accepted: bool,
) -> None:
    now = 1_800_000_000.0
    monkeypatch.setattr(ce, "time", FixedClock(now))
    executor = probe_for(
        tmp_path, pinned, Reply(probe_text(epoch=str(int(now) + skew))), max_clock_skew_seconds=10
    )
    if accepted:
        executor._snapshot("h1", IMAGE, now + 600, None, tmp_path / "work")
    else:
        with pytest.raises(ce.HostRefused, match="host clock differs"):
            executor._snapshot("h1", IMAGE, now + 600, None, tmp_path / "work")


class TickingClock(FixedClock):
    """`time()` moves by `step` on every call, like a probe whose round trip takes real time."""

    def __init__(self, now: float, step: float) -> None:
        super().__init__(now)
        self.step = step

    def time(self) -> float:
        value, self.now = self.now, self.now + self.step
        return value


@pytest.mark.parametrize(
    ("offset", "accepted"), [(0, True), (5, True), (-5, True), (6, False), (-6, False)]
)
def test_snapshot_judges_the_host_clock_at_the_middle_of_the_probe_round_trip(
    tmp_path: Path,
    pinned: tuple[Path, Path],
    monkeypatch: pytest.MonkeyPatch,
    offset: int,
    accepted: bool,
) -> None:
    now, step = 1_800_000_000.0, 8.0  # the probe took 16 s: the host read its clock 8 s in
    epoch = int(now + step) + offset
    executor = probe_for(
        tmp_path, pinned, Reply(probe_text(epoch=str(epoch))), max_clock_skew_seconds=5
    )
    monkeypatch.setattr(ce, "time", TickingClock(now, step))
    if accepted:
        assert executor._snapshot("h1", IMAGE, now + 600, None, tmp_path / "work").epoch == epoch
    else:
        with pytest.raises(ce.HostRefused, match="host clock differs from the challenge clock"):
            executor._snapshot("h1", IMAGE, now + 600, None, tmp_path / "work")


@pytest.mark.parametrize("lead", [3600, 10], ids=["job-far", "job-near"])
@pytest.mark.parametrize("kind", ["probe", "pull"])
def test_probe_and_pull_get_their_own_limit_and_never_outlive_the_job(
    tmp_path: Path,
    pinned: tuple[Path, Path],
    monkeypatch: pytest.MonkeyPatch,
    kind: str,
    lead: int,
) -> None:
    now = 1_800_000_000.0
    monkeypatch.setattr(ce, "time", FixedClock(now))
    reply = Reply(probe_text(epoch=str(int(now))))
    executor = probe_for(tmp_path, pinned, reply, pull_timeout_seconds=300)
    if kind == "probe":
        executor._snapshot("h1", IMAGE, now + lead, None, tmp_path / "work")
    else:
        executor._pull("h1", IMAGE, now + lead, None, tmp_path / "work")
    limit = 60 if kind == "probe" else 300
    assert reply.deadlines == [now + min(lead, limit)]


def test_removing_a_container_gets_twenty_seconds_and_no_more(
    tmp_path: Path, pinned: tuple[Path, Path], monkeypatch: pytest.MonkeyPatch
) -> None:
    now = 1_800_000_000.0
    monkeypatch.setattr(ce, "time", FixedClock(now))
    reply = Reply()
    executor = probe_for(tmp_path, pinned, reply)
    executor._remove("h1", NAME, tmp_path / "work")
    assert reply.deadlines == [now + 20]


@pytest.mark.parametrize(("lead", "budget"), [(60, 55), (10, 5), (9, None), (7, None)])
def test_request_budget_is_what_is_left_of_the_job_minus_the_margin(
    tmp_path: Path,
    pinned: tuple[Path, Path],
    monkeypatch: pytest.MonkeyPatch,
    lead: int,
    budget: int | None,
) -> None:
    job = island_job(tmp_path / "job")
    monkeypatch.setattr(ce, "time", FixedClock(job.deadline - float(lead)))
    executor = probe_for(tmp_path, pinned, Reply())
    tar = tmp_path / "input.tar"
    if budget is None:
        with pytest.raises(ce.Aborted, match="job deadline too close to start"):
            executor._request(job, tmp_path / "job", [gpu(0)], False, tar)
        assert not tar.exists()
    else:
        request = executor._request(job, tmp_path / "job", [gpu(0)], False, tar)
        assert json.loads(request)["budget_seconds"] == budget


PINS = [
    ("avx2", "sm_103", "math"),
    ("avx2", "sm_103", "sdpa-flash"),
    ("avx512", "sm_103", "math"),
    ("avx2", "sm_120", "sdpa-flash"),
    ("avx512", "sm_120", "math"),
]


@pytest.mark.parametrize(("cpu", "klass", "attention"), PINS)
def test_request_carries_the_manifest_pin_in_the_form_the_runner_admits(
    tmp_path: Path, pinned: tuple[Path, Path], cpu: str, klass: str, attention: str
) -> None:
    job = island_job(tmp_path / "job", cpu=cpu, gpu=klass, attention=attention)
    executor = probe_for(tmp_path, pinned, Reply())
    tar = tmp_path / "input.tar"
    raw = executor._request(job, tmp_path / "job", [gpu(0)], False, tar)

    body = class_pin(cpu, klass, attention)
    request = json.loads(raw)
    assert {name: request["env"][name] for name in body["present"]} == body["present"]
    assert request["unset"] == body["absent"] and request["gpu"] == klass
    (tmp_path / "admit").mkdir()
    with open(tar, "rb") as stream:  # what the container program reads from stdin
        admitted, received = runner.read_inputs(stream, tmp_path / "admit")
    assert (admitted, received) == (request, raw)


def test_launch_refuses_a_manifest_that_names_no_gpu_class_before_it_touches_a_host(
    tmp_path: Path, pinned: tuple[Path, Path]
) -> None:
    # Every GPU host would show a class and the manifest says none: that is the manifest's fault,
    # and re-judging it per host would quarantine them all.
    job = island_job(tmp_path / "job", gpu="none")
    order: list[str] = []
    executor = executor_with(tmp_path, pinned, {"h1": Down("h1", order)}, sweeps=1)
    with pytest.raises(ce.IslandFailure, match="no GPU class") as refused:
        executor.launch("audit", "b" * 64, job, tmp_path / "job", trace=False, cancel=None)
    assert order == [] and type(refused.value) is ce.IslandFailure
    assert not executor.journal.path.exists()


@pytest.mark.parametrize(("lead", "reaches_a_host"), [(9.0, False), (9.99, False), (10.0, True)])
def test_launch_needs_two_margins_of_time_before_it_touches_a_host(
    tmp_path: Path,
    pinned: tuple[Path, Path],
    monkeypatch: pytest.MonkeyPatch,
    lead: float,
    reaches_a_host: bool,
) -> None:
    job = island_job(tmp_path / "job")
    monkeypatch.setattr(ce, "time", FixedClock(job.deadline - lead))
    order: list[str] = []
    executor = executor_with(tmp_path, pinned, {"h1": Down("h1", order)}, sweeps=1)
    expected = "no host able to run the job" if reaches_a_host else "job deadline too close"
    with pytest.raises(ce.IslandFailure, match=expected) as caught:
        executor.launch("audit", "b" * 64, job, tmp_path / "job", trace=False, cancel=None)

    if reaches_a_host:
        assert order == ["h1"] and not isinstance(caught.value, ce.Aborted)
    else:
        assert order == [] and isinstance(caught.value, ce.Aborted)
        assert not executor.journal.path.exists()  # not even an attempt was journaled


def test_publish_copies_each_object_through_the_confined_path_again(
    executor: ce.Executor, tmp_path: Path
) -> None:
    # `_request` confined the objects minutes ago; a link planted since must not be followed.
    directory = tmp_path / "job"
    job = island_job(directory)
    outside = tmp_path / "outside"
    outside.write_bytes(b"state")
    (directory / "start_state").unlink()
    (directory / "start_state").symlink_to(outside)
    with pytest.raises(ce.IslandFailure, match="object escapes attempt directory") as caught:
        executor._publish(job, directory, tmp_path / "staging", {}, False)
    assert not isinstance(caught.value, ce.AttemptFailed)


@pytest.mark.parametrize(
    ("head", "expected"),
    [
        ("00000000", ["h1", "h2", "h3"]),
        ("00000001", ["h2", "h3", "h1"]),
        ("00000002", ["h3", "h1", "h2"]),
        ("00000003", ["h1", "h2", "h3"]),
        ("0000000a", ["h2", "h3", "h1"]),
        ("0000000100000001", ["h2", "h3", "h1"]),  # only the first 8 hex digits count
    ],
)
def test_hosts_are_tried_in_a_rotation_that_starts_where_the_job_digest_says(
    tmp_path: Path,
    pinned: tuple[Path, Path],
    monkeypatch: pytest.MonkeyPatch,
    head: str,
    expected: list[str],
) -> None:
    start_at(monkeypatch, head)
    order: list[str] = []
    hosts = {label: Down(label, order) for label in ("h1", "h2", "h3")}
    executor = executor_with(tmp_path, pinned, hosts, sweeps=1)
    job = island_job(tmp_path / "job")
    with pytest.raises(ce.IslandFailure, match="no host able to run the job"):
        executor.launch("audit", "b" * 64, job, tmp_path / "job", trace=False, cancel=None)
    assert order == expected


@pytest.mark.parametrize(
    ("lead", "configured", "pause"),
    [(600, 7, 7.0), (12, 20, 12.0)],
    ids=["ample", "job-ends-before-the-pause"],
)
def test_sweeps_are_separated_by_a_pause_that_never_outlasts_the_job_and_none_follows_the_last(
    tmp_path: Path,
    pinned: tuple[Path, Path],
    monkeypatch: pytest.MonkeyPatch,
    lead: int,
    configured: int,
    pause: float,
) -> None:
    job = island_job(tmp_path / "job")
    monkeypatch.setattr(ce, "time", FixedClock(job.deadline - float(lead)))
    order: list[str] = []
    sleeps: list[float] = []
    hosts = {label: Down(label, order) for label in ("h1", "h2")}
    executor = executor_with(
        tmp_path, pinned, hosts, sweeps=3, sweep_pause_seconds=configured, sleep=sleeps.append
    )
    with pytest.raises(ce.IslandFailure, match="no host able to run the job"):
        executor.launch("audit", "b" * 64, job, tmp_path / "job", trace=False, cancel=None)
    assert len(order) == 6  # two hosts, three sweeps
    assert sleeps == [pause, pause]


def test_the_pause_after_a_sweep_that_outlived_the_job_is_zero_not_negative(
    tmp_path: Path, pinned: tuple[Path, Path], monkeypatch: pytest.MonkeyPatch
) -> None:
    job = island_job(tmp_path / "job")
    clock = FixedClock(job.deadline - 600.0)
    monkeypatch.setattr(ce, "time", clock)

    class Slow(Down):
        def run(self, command: str, **kwargs: Any) -> int:
            clock.now = job.deadline + 1000.0  # this call is what takes the job past its deadline
            return super().run(command, **kwargs)

    sleeps: list[float] = []
    executor = executor_with(
        tmp_path,
        pinned,
        {"h1": Slow("h1", [])},
        sweeps=3,
        sweep_pause_seconds=15,
        sleep=sleeps.append,
    )
    with pytest.raises(ce.IslandFailure, match="no host able to run the job"):
        executor.launch("audit", "b" * 64, job, tmp_path / "job", trace=False, cancel=None)
    assert sleeps == [0.0, 0.0]  # time.sleep(-1000) would raise instead


def test_a_sweep_that_leaves_every_host_refused_ends_the_launch_without_a_pause(
    tmp_path: Path, pinned: tuple[Path, Path], monkeypatch: pytest.MonkeyPatch
) -> None:
    start_at(monkeypatch, "00000000")  # h1 first
    log: list[tuple[str, str]] = []
    sleeps: list[float] = []
    boxes = {
        label: ScriptedHost(label, log, [gpu(number, driver="999.99")])
        for number, label in enumerate(("h1", "h2"))
    }
    executor = executor_with(
        tmp_path, pinned, boxes, sweeps=3, sweep_pause_seconds=15, sleep=sleeps.append
    )
    job = island_job(tmp_path / "job")
    with pytest.raises(ce.IslandFailure) as caught:
        executor.launch("audit", "b" * 64, job, tmp_path / "job", trace=False, cancel=None)

    assert sleeps == []  # nothing can change for a quarantined host: no 15 s pause, no 2nd sweep
    assert [kind for _, kind in log] == ["probe", "probe"]
    why = "driver is not the one allowlisted by the manifest"
    assert str(caught.value).endswith(f"; h1 was refused: {why}; h2 was refused: {why}")
    assert [row["kind"] for row in journal_of(executor)] == ["exec_host_refused"] * 2


def test_one_refused_host_does_not_end_the_sweeps_while_another_may_still_answer(
    tmp_path: Path, pinned: tuple[Path, Path], monkeypatch: pytest.MonkeyPatch
) -> None:
    order: list[str] = []
    log: list[tuple[str, str]] = []
    sleeps: list[float] = []
    boxes = {"h1": ScriptedHost("h1", log, [gpu(0, driver="999.99")]), "h2": Down("h2", order)}
    executor = executor_with(
        tmp_path, pinned, boxes, sweeps=3, sweep_pause_seconds=15, sleep=sleeps.append
    )
    job = island_job(tmp_path / "job")
    with pytest.raises(ce.IslandFailure, match="h1 was refused"):
        executor.launch("audit", "b" * 64, job, tmp_path / "job", trace=False, cancel=None)

    assert order == ["h2"] * 3  # the host that is merely down is tried in every sweep
    assert [kind for _, kind in log] == ["probe"]  # the refused one is never contacted again
    assert len(sleeps) == 2 and all(0 < pause <= 15 for pause in sleeps)


def test_a_host_refused_in_the_same_launch_does_not_spend_an_attempt(
    tmp_path: Path, pinned: tuple[Path, Path], monkeypatch: pytest.MonkeyPatch
) -> None:
    start_at(monkeypatch, "00000000")  # h1 first
    stub_publication(monkeypatch)
    log: list[tuple[str, str]] = []
    wrong_driver = ScriptedHost("h1", log, [gpu(0, driver="999.99")])
    flaky = ScriptedHost("h2", log, [gpu(1)], script={"run": ["oom"]})
    executor = executor_with(tmp_path, pinned, {"h1": wrong_driver, "h2": flaky}, max_attempts=2)
    job = island_job(tmp_path / "job")
    published = executor.launch("audit", "b" * 64, job, tmp_path / "job", trace=False, cancel=None)

    assert published == "published"
    assert [label for label, kind in log if kind == "run"] == ["h2", "h2"]  # failed once, then ran
    assert [row["kind"] for row in journal_of(executor)] == [
        "exec_host_refused",
        "exec_attempt",
        "exec_attempt_failed",
        "exec_attempt",
        "exec_published",
    ]


@pytest.mark.parametrize("kind", ["probe", "pull"])
def test_probe_or_pull_that_outlives_its_limit_loses_the_host_not_the_job(
    tmp_path: Path, pinned: tuple[Path, Path], monkeypatch: pytest.MonkeyPatch, kind: str
) -> None:
    start_at(monkeypatch, "00000000")  # h1 first
    stub_publication(monkeypatch)
    log: list[tuple[str, str]] = []
    slow = ScriptedHost("h1", log, [gpu(0)], absent=True, script={kind: ["timeout"]})
    fine = ScriptedHost("h2", log, [gpu(1)])
    executor = executor_with(tmp_path, pinned, {"h1": slow, "h2": fine})
    job = island_job(tmp_path / "job")
    published = executor.launch("audit", "b" * 64, job, tmp_path / "job", trace=False, cancel=None)

    assert published == "published"
    tried = [("h1", "probe")] + ([("h1", "pull")] if kind == "pull" else [])
    assert log == [*tried, ("h2", "probe"), ("h2", "prepare"), ("h2", "run")]
    (lost,) = journal_of(executor, "exec_host_unavailable")
    assert (lost["label"], lost["reason"]) == ("h1", "remote command timed out")
    assert journal_of(executor, "exec_host_refused") == []  # lost, not quarantined


# -- compute cache (docs/adr/0012) --------------------------------------------------------------

LEAF = DIGEST.removeprefix("sha256:")[: ce.CACHE_LEAF]
DEFAULT_DIR = f"/home/ubuntu/.cortex-cuda-executor/compute-cache/{LEAF}"


def test_cache_is_on_by_default_and_a_host_may_name_its_root(
    tmp_path: Path, pinned: tuple[Path, Path]
) -> None:
    config = load_config(config_data(tmp_path, pinned))
    assert config.compute_cache is True and config.hosts[0].compute_cache_dir is None
    named = load_config(
        config_data(
            tmp_path,
            pinned,
            compute_cache=False,
            hosts=[host_data(pinned, compute_cache_dir="/mnt/nvme/ht-cache")],
        )
    )
    assert named.compute_cache is False
    assert named.hosts[0].compute_cache_dir == Path("/mnt/nvme/ht-cache")
    longest = "/" + "a" * 199  # 200 characters is the limit
    load_config(config_data(tmp_path, pinned, hosts=[host_data(pinned, compute_cache_dir=longest)]))


def test_run_command_with_the_cache_differs_by_exactly_one_mount() -> None:
    args = (("sudo", "-n", "docker"), IMAGE, NAME, "d" * 64, [uid(0)], "default")
    plain = shlex.split(ce.run_command(*args))
    cached = shlex.split(ce.run_command(*args, DEFAULT_DIR))
    at = cached.index("--mount")
    # docker parses the spec itself: a plain `-v` would create a missing source as root.
    assert cached[at : at + 2] == ["--mount", f"type=bind,src={DEFAULT_DIR},dst=/root/.nv"]
    assert cached[:at] + cached[at + 2 :] == plain
    assert cached.count("--mount") == 1
    assert not {"-v", "--volume", "-e", "--env", "--env-file", "--privileged", "-p"} & set(cached)
    assert not any(t.startswith(("--cap-add", "--device", "--pid", "--net=")) for t in cached)
    assert [t for t in cached if t.startswith("/")] == ["/opt/hypertrain/venv/bin/python"]


@pytest.mark.parametrize(
    "hostile",
    [
        "",
        "relative/dir",
        "/a,dst=/etc",
        "/a b",
        "/a\n",
        "/a;id",
        "/a$(id)",
        "/a/../b",
        "/a/./b",
        "/" + "a" * ce.CACHE_PATH_MAX,
    ],
)
def test_run_command_refuses_a_cache_directory_that_is_not_plain(hostile: str) -> None:
    with pytest.raises(ValueError, match="plain words"):
        ce.run_command(("docker",), IMAGE, NAME, "d" * 64, [uid(0)], "default", hostile)


def test_cache_location_names_one_directory_per_image_digest() -> None:
    other = "sha256:" + "cd" * 32
    default, pattern = ce.cache_location(None, DIGEST)
    assert default == f'D="$HOME/.cortex-cuda-executor/compute-cache/{LEAF}"'
    assert pattern.fullmatch(DEFAULT_DIR) and pattern.fullmatch(
        DEFAULT_DIR.replace("ubuntu", "u.1")
    )
    assert not pattern.fullmatch(DEFAULT_DIR.replace(LEAF, "cd" * 16))  # another image's directory
    assert not pattern.fullmatch(DEFAULT_DIR + "/x") and not pattern.fullmatch(LEAF)
    assert not pattern.fullmatch("/etc/passwd") and not pattern.fullmatch("")
    assert ce.cache_location(None, other)[0] != default
    named, exact = ce.cache_location(Path("/mnt/nvme/ht-cache"), DIGEST)
    assert named == f"D=/mnt/nvme/ht-cache/{LEAF}"
    assert exact.fullmatch(f"/mnt/nvme/ht-cache/{LEAF}")
    assert not exact.fullmatch(f"/mnt/nvme/ht-cache/{LEAF}/x") and not exact.fullmatch(DEFAULT_DIR)


@pytest.mark.parametrize("home_name", ["home", "ho me"])
def test_prepare_command_in_a_real_shell_creates_the_private_directory_and_names_it(
    tmp_path: Path, home_name: str
) -> None:
    home = tmp_path / home_name
    home.mkdir()
    assignment, pattern = ce.cache_location(None, DIGEST)
    command = ce.prepare_command(assignment)
    env = {"HOME": str(home), "PATH": os.environ["PATH"]}

    def prepare() -> str:
        done = subprocess.run(["sh", "-c", command], env=env, capture_output=True, text=True)
        assert (done.returncode, done.stderr) == (0, "")
        return done.stdout

    answer = prepare()
    directory = home / ".cortex-cuda-executor/compute-cache" / LEAF
    assert answer == f"{directory}\n"
    assert stat.S_IMODE(directory.stat().st_mode) == 0o700 and directory.is_dir()
    assert [p.name for p in home.iterdir()] == [".cortex-cuda-executor"]  # nothing else is written
    directory.chmod(0o755)
    assert prepare() == answer and stat.S_IMODE(directory.stat().st_mode) == 0o700  # idempotent
    # A home with a space is created correctly but its path is not one the executor will mount.
    assert (pattern.fullmatch(answer.strip()) is not None) is (" " not in home_name)


def test_prepare_command_makes_every_directory_it_creates_private_whatever_the_umask(
    tmp_path: Path,
) -> None:
    assignment, _ = ce.cache_location(None, DIGEST)
    done = subprocess.run(
        ["sh", "-c", "umask 000; " + ce.prepare_command(assignment)],
        env={"HOME": str(tmp_path), "PATH": os.environ["PATH"]},
        capture_output=True,
        text=True,
    )
    assert done.returncode == 0, done.stderr
    walked = [tmp_path / ".cortex-cuda-executor", tmp_path / ".cortex-cuda-executor/compute-cache"]
    assert [stat.S_IMODE(p.stat().st_mode) for p in walked] == [0o700, 0o700]


def test_prepare_command_refuses_a_link_and_leaves_its_target_alone(tmp_path: Path) -> None:
    assignment, _ = ce.cache_location(None, DIGEST)
    target = tmp_path / "elsewhere"
    target.mkdir()
    target.chmod(0o755)  # mkdir(mode=) is masked by the umask; the test needs this exact mode
    leaf = tmp_path / ".cortex-cuda-executor/compute-cache" / LEAF
    leaf.parent.mkdir(parents=True)
    leaf.symlink_to(target)
    done = subprocess.run(
        ["sh", "-c", ce.prepare_command(assignment)],
        env={"HOME": str(tmp_path), "PATH": os.environ["PATH"]},
        capture_output=True,
        text=True,
    )
    assert done.returncode != 0 and done.stdout == ""
    assert stat.S_IMODE(target.stat().st_mode) == 0o755


def test_prepare_command_fails_instead_of_using_a_directory_it_cannot_secure(
    tmp_path: Path,
) -> None:
    blocker = tmp_path / ".cortex-cuda-executor"
    blocker.write_text("a file where the cache root should be")
    assignment, _ = ce.cache_location(None, DIGEST)
    done = subprocess.run(
        ["sh", "-c", ce.prepare_command(assignment)],
        env={"HOME": str(tmp_path), "PATH": os.environ["PATH"]},
        capture_output=True,
        text=True,
    )
    assert done.returncode != 0 and done.stdout == ""


class PrepareTransport:
    """Answers the compute cache command with `answer` and records every command it was given."""

    def __init__(self, answer: str = DEFAULT_DIR + "\n", code: int = 0) -> None:
        self.answer, self.code, self.calls, self.deadlines = answer, code, [], []

    def run(self, command: str, *, stdout: Path, stderr: Path, deadline: float, **_: Any) -> int:
        self.calls.append(command)
        self.deadlines.append(deadline)
        stdout.write_text(self.answer)
        stderr.write_text("mkdir: cannot create directory: Permission denied\n")
        return self.code


def cache_executor(
    tmp_path: Path, pinned: tuple[Path, Path], transport: Any, **over: Any
) -> ce.Executor:
    config = load_config(config_data(tmp_path, pinned, **over))
    return ce.Executor(config, transports={"h1": transport})


def ask_cache(executor: ce.Executor, tmp_path: Path, digest: str = DIGEST) -> str | None:
    ref = SimpleNamespace(image_digest=digest)
    work = tmp_path / f"work-{time.monotonic_ns()}"
    work.mkdir()
    return executor._cache("h1", ref, time.time() + 600, None, work)  # type: ignore[arg-type]


def test_cache_directory_is_created_once_per_host_and_image(
    tmp_path: Path, pinned: tuple[Path, Path]
) -> None:
    transport = PrepareTransport()
    executor = cache_executor(tmp_path, pinned, transport)
    assert ask_cache(executor, tmp_path) == DEFAULT_DIR
    assert ask_cache(executor, tmp_path) == DEFAULT_DIR
    assert len(transport.calls) == 1  # remembered: no second ssh call
    assert transport.calls[0] == ce.prepare_command(ce.cache_location(None, DIGEST)[0])
    other = "sha256:" + "cd" * 32
    transport.answer = DEFAULT_DIR.replace(LEAF, "cd" * 16) + "\n"
    assert ask_cache(executor, tmp_path, other) == transport.answer.strip()
    assert len(transport.calls) == 2  # another image: another directory
    # A new executor (a restarted challenge) asks again and finds the same directory.
    transport.answer = DEFAULT_DIR + "\n"
    assert ask_cache(cache_executor(tmp_path, pinned, transport), tmp_path) == DEFAULT_DIR
    assert len(transport.calls) == 3


def test_cache_directory_command_runs_for_the_ssh_user_and_never_through_sudo(
    tmp_path: Path, pinned: tuple[Path, Path]
) -> None:
    transport = PrepareTransport()
    ask_cache(cache_executor(tmp_path, pinned, transport), tmp_path)
    assert not transport.calls[0].startswith(("sudo", "docker"))  # never root, never the daemon


def test_a_named_root_is_used_as_given(tmp_path: Path, pinned: tuple[Path, Path]) -> None:
    where = f"/mnt/nvme/ht-cache/{LEAF}"
    transport = PrepareTransport(where + "\n")
    hosts = [host_data(pinned, compute_cache_dir="/mnt/nvme/ht-cache")]
    executor = cache_executor(tmp_path, pinned, transport, hosts=hosts)
    assert ask_cache(executor, tmp_path) == where
    assert transport.calls[0].startswith(f"D=/mnt/nvme/ht-cache/{LEAF};")
    transport.answer = DEFAULT_DIR + "\n"  # the home directory is not the root that was named
    executor = cache_executor(tmp_path, pinned, transport, hosts=hosts)
    with pytest.raises(ce.AttemptFailed, match="unexpected compute cache directory"):
        ask_cache(executor, tmp_path)


def test_no_cache_means_no_ssh_call(tmp_path: Path, pinned: tuple[Path, Path]) -> None:
    transport = PrepareTransport()
    executor = cache_executor(tmp_path, pinned, transport, compute_cache=False)
    assert ask_cache(executor, tmp_path) is None and transport.calls == []


@pytest.mark.parametrize(
    "answer",
    [
        "",
        "\n",
        DEFAULT_DIR + "\n" + DEFAULT_DIR + "\n",
        DEFAULT_DIR + "\nextra\n",
        "/etc/passwd\n",
        DEFAULT_DIR.replace(LEAF, "cd" * 16) + "\n",  # another image's directory
        DEFAULT_DIR + "/sub\n",
        DEFAULT_DIR.replace("/home/ubuntu", "/home/ubuntu/../root") + "\n",
        DEFAULT_DIR.replace("/home/ubuntu", "//home/ubuntu") + "\n",
        DEFAULT_DIR.replace("ubuntu", "ubu ntu") + "\n",
        DEFAULT_DIR.replace("ubuntu", "ub,ntu") + "\n",
        "relative/" + DEFAULT_DIR.lstrip("/") + "\n",
    ],
    ids=[
        "empty",
        "blank-line",
        "same-path-twice",
        "second-line",
        "other-path",
        "other-image",
        "below-the-directory",
        "dotdot",
        "double-slash",
        "space",
        "comma",
        "relative",
    ],
)
def test_a_cache_directory_report_that_is_not_the_one_asked_for_is_refused(
    tmp_path: Path, pinned: tuple[Path, Path], answer: str
) -> None:
    transport = PrepareTransport(answer)
    executor = cache_executor(tmp_path, pinned, transport)
    with pytest.raises(ce.AttemptFailed, match="unexpected compute cache directory"):
        ask_cache(executor, tmp_path)
    transport.answer = DEFAULT_DIR + "\n"
    assert ask_cache(executor, tmp_path) == DEFAULT_DIR  # the refusal was not remembered
    assert len(transport.calls) == 2


def test_a_failing_cache_command_costs_an_attempt_and_shows_the_host_reason(
    tmp_path: Path, pinned: tuple[Path, Path]
) -> None:
    transport = PrepareTransport(code=1)
    executor = cache_executor(tmp_path, pinned, transport)
    with pytest.raises(ce.AttemptFailed, match="cannot prepare.*Permission denied"):
        ask_cache(executor, tmp_path)
    assert executor._cache_dirs == {}


def test_a_cache_command_that_outlives_its_limit_loses_the_host(
    tmp_path: Path, pinned: tuple[Path, Path]
) -> None:
    class Slow(PrepareTransport):
        def run(self, command: str, **kwargs: Any) -> int:
            raise ce.RemoteTimeout("remote command timed out")

    with pytest.raises(ce.HostLost, match="timed out"):
        ask_cache(cache_executor(tmp_path, pinned, Slow()), tmp_path)


def attempt_twice(
    tmp_path: Path, pinned: tuple[Path, Path], monkeypatch: pytest.MonkeyPatch, script: dict
) -> ScriptedHost:
    stub_publication(monkeypatch)
    host = ScriptedHost("h1", [], [gpu(0)], script=script)
    executor = executor_with(tmp_path, pinned, {"h1": host})
    job = island_job(tmp_path / "job")
    for number in (1, 2):
        try:
            executor._attempt("h1", number, "audit", "b" * 64, job, tmp_path / "job", False, None)
        except ce.IslandFailure:
            pass
    return host


def test_the_directory_is_asked_for_again_only_after_an_attempt_that_failed(
    tmp_path: Path, pinned: tuple[Path, Path], monkeypatch: pytest.MonkeyPatch
) -> None:
    steady = attempt_twice(tmp_path / "ok", pinned, monkeypatch, {})
    assert len(steady.prepared) == 1  # two good attempts, one ssh call for the directory
    failing = attempt_twice(tmp_path / "oom", pinned, monkeypatch, {"run": ["oom"]})
    # The first container died: the directory may be what failed, so the next attempt checks it.
    assert len(failing.prepared) == 2


def test_a_cache_command_that_times_out_inside_an_attempt_loses_the_host_instead_of_falling_back(
    tmp_path: Path, pinned: tuple[Path, Path], monkeypatch: pytest.MonkeyPatch
) -> None:
    stub_publication(monkeypatch)
    log: list[tuple[str, str]] = []
    host = ScriptedHost("h1", log, [gpu(0)], script={"prepare": ["timeout"]})
    executor = executor_with(tmp_path, pinned, {"h1": host})
    job = island_job(tmp_path / "job")
    with pytest.raises(ce.HostLost, match="timed out"):
        executor._attempt("h1", 1, "audit", "b" * 64, job, tmp_path / "job", False, None)
    assert [kind for _, kind in log] == ["probe", "prepare"]  # no container was started
    journal = executor.journal.path
    assert "exec_cache_unavailable" not in (journal.read_text() if journal.exists() else "")


def test_a_cache_directory_the_host_cannot_make_runs_the_attempt_without_the_cache(
    tmp_path: Path, pinned: tuple[Path, Path], monkeypatch: pytest.MonkeyPatch
) -> None:
    records: list[dict[str, Any]] = []

    def capture(
        self: ce.Executor, job: Any, directory: Path, staging: Path, record: dict, trace: bool
    ) -> str:
        records.append(record)
        return "published"

    monkeypatch.setattr(ce.Executor, "_publish", capture)
    commands: list[str] = []
    real = ce.run_command
    monkeypatch.setattr(
        ce, "run_command", lambda *a, **k: commands.append(real(*a, **k)) or commands[-1]
    )

    class Refusing(FakeHost):
        def run(self, command: str, **kwargs: Any) -> int:
            if command.startswith("D="):
                kwargs["stdout"].write_text("")
                return 1
            return super().run(command, **kwargs)

    executor = executor_with(tmp_path, pinned, {"h1": Refusing([gpu(0)])})
    job = island_job(tmp_path / "job")
    executor._attempt("h1", 1, "audit", "b" * 64, job, tmp_path / "job", False, None)
    assert records[0]["compute_cache"] is False and "--mount" not in commands[0]
    (why,) = journal_of(executor, "exec_cache_unavailable")
    assert why["label"] == "h1" and "cannot prepare" in why["reason"]
    (attempt,) = journal_of(executor, "exec_attempt")
    assert attempt["compute_cache"] is False


def test_the_attempt_names_the_directory_in_the_container_the_record_and_the_journal(
    tmp_path: Path, pinned: tuple[Path, Path], monkeypatch: pytest.MonkeyPatch
) -> None:
    records: list[dict[str, Any]] = []

    def capture(
        self: ce.Executor, job: Any, directory: Path, staging: Path, record: dict, trace: bool
    ) -> str:
        records.append(record)
        return "published"

    monkeypatch.setattr(ce.Executor, "_publish", capture)
    commands: list[str] = []
    real = ce.run_command

    def spy(*args: Any, **kwargs: Any) -> str:
        commands.append(real(*args, **kwargs))
        return commands[-1]

    monkeypatch.setattr(ce, "run_command", spy)
    job = island_job(tmp_path / "job")
    for flag in (True, False):
        executor = executor_with(
            tmp_path / str(flag), pinned, {"h1": FakeHost([gpu(0)])}, compute_cache=flag
        )
        executor._attempt("h1", 1, "audit", "b" * 64, job, tmp_path / "job", False, None)
        (attempt,) = journal_of(executor, "exec_attempt")
        assert attempt["compute_cache"] is flag and records[-1]["compute_cache"] is flag
        assert (f"src={DEFAULT_DIR}" in commands[-1]) is flag


# -- compute cache: behaviours the mutation pass found unpinned ---------------------------


def shell_prepare(command: str, home: Path, first_on_path: Path | None = None) -> Any:
    path = os.environ["PATH"]
    if first_on_path is not None:
        path = f"{first_on_path}{os.pathsep}{path}"
    return subprocess.run(
        ["sh", "-c", command], env={"HOME": str(home), "PATH": path}, capture_output=True, text=True
    )


def test_prepare_command_does_not_report_a_cache_path_that_is_a_file(tmp_path: Path) -> None:
    leaf = tmp_path / ".cortex-cuda-executor/compute-cache" / LEAF
    leaf.parent.mkdir(parents=True)
    leaf.write_text("a file where the cache directory should be")
    leaf.chmod(0o644)
    assignment, _ = ce.cache_location(None, DIGEST)
    done = shell_prepare(ce.prepare_command(assignment), tmp_path)
    assert done.returncode != 0 and done.stdout == ""
    assert stat.S_IMODE(leaf.stat().st_mode) == 0o644  # nothing was done to it after mkdir failed


def test_prepare_command_fails_when_the_directory_cannot_be_made_private(tmp_path: Path) -> None:
    shim, home = tmp_path / "shim", tmp_path / "home"
    shim.mkdir()
    home.mkdir()
    (shim / "chmod").write_text("#!/bin/sh\necho 'chmod: Operation not permitted' >&2\nexit 1\n")
    (shim / "chmod").chmod(0o755)
    assignment, _ = ce.cache_location(None, DIGEST)
    done = shell_prepare(ce.prepare_command(assignment), home, shim)
    assert done.returncode != 0 and done.stdout == ""


def two_hosts(
    tmp_path: Path, pinned: tuple[Path, Path], first: Any, second: Any, root: str | None = None
) -> ce.Executor:
    hosts = [
        host_data(pinned, "h1"),
        host_data(pinned, "h2", address="gpu-2.example.com", compute_cache_dir=root),
    ]
    config = load_config(config_data(tmp_path, pinned, hosts=hosts))
    return ce.Executor(config, transports={"h1": first, "h2": second})


def ask_label(executor: ce.Executor, tmp_path: Path, label: str) -> str | None:
    ref = SimpleNamespace(image_digest=DIGEST)
    work = tmp_path / f"work-{label}-{time.monotonic_ns()}"
    work.mkdir()
    return executor._cache(label, ref, time.time() + 600, None, work)  # type: ignore[arg-type]


def test_the_directory_is_created_on_every_host_not_only_the_first(
    tmp_path: Path, pinned: tuple[Path, Path]
) -> None:
    first, second = PrepareTransport(), PrepareTransport()
    executor = two_hosts(tmp_path, pinned, first, second)
    assert ask_label(executor, tmp_path, "h1") == DEFAULT_DIR
    assert ask_label(executor, tmp_path, "h2") == DEFAULT_DIR
    assert (len(first.calls), len(second.calls)) == (1, 1)  # same image, two hosts, two mkdir


def test_each_host_uses_the_cache_root_of_its_own_entry(
    tmp_path: Path, pinned: tuple[Path, Path]
) -> None:
    named = f"/mnt/nvme/ht-cache/{LEAF}"
    first, second = PrepareTransport(), PrepareTransport(named + "\n")
    executor = two_hosts(tmp_path, pinned, first, second, root="/mnt/nvme/ht-cache")
    assert ask_label(executor, tmp_path, "h1") == DEFAULT_DIR
    assert ask_label(executor, tmp_path, "h2") == named
    assert first.calls[0].startswith('D="$HOME/') and second.calls[0].startswith(f"D={named};")


class FailsFirstRun(ScriptedHost):
    """The first `docker run` raises `failure` (an exception) or exits with it (an integer).
    Every other command is answered as by ScriptedHost."""

    def __init__(self, failure: Exception | int, *args: Any, **kwargs: Any) -> None:
        super().__init__(*args, **kwargs)
        self.failure: Exception | int | None = failure

    def run(self, command: str, **kwargs: Any) -> int:
        if kind_of(command) == "run" and self.failure is not None:
            failure, self.failure = self.failure, None
            if isinstance(failure, int):
                kwargs["stderr"].write_text("the container died\n")
                return failure
            raise failure
        return super().run(command, **kwargs)


@pytest.mark.parametrize(
    "failure",
    [
        ce.RemoteTimeout("remote command timed out"),
        ce.Aborted("lease cancelled"),
        ce.AttemptFailed("result stream too large"),
        ce.RUNNER_PROBE_FAILED,
    ],
    ids=["host-lost", "aborted", "attempt-failed", "probe-failed-exit"],
)
def test_the_directory_is_asked_for_again_after_any_run_that_failed(
    tmp_path: Path, pinned: tuple[Path, Path], monkeypatch: pytest.MonkeyPatch, failure: Any
) -> None:
    stub_publication(monkeypatch)
    host = FailsFirstRun(failure, "h1", [], [gpu(0)])
    executor = executor_with(tmp_path, pinned, {"h1": host})
    job = island_job(tmp_path / "job")
    with pytest.raises(ce.IslandFailure):
        executor._attempt("h1", 1, "audit", "b" * 64, job, tmp_path / "job", False, None)
    assert len(host.prepared) == 1
    executor._attempt("h1", 2, "audit", "b" * 64, job, tmp_path / "job", False, None)
    assert len(host.prepared) == 2  # the first container died: the directory is checked again


def test_a_failed_attempt_forgets_only_the_directory_of_its_own_host(
    tmp_path: Path, pinned: tuple[Path, Path], monkeypatch: pytest.MonkeyPatch
) -> None:
    stub_publication(monkeypatch)
    log: list[tuple[str, str]] = []
    first = ScriptedHost("h1", log, [gpu(0)], script={"run": ["ok", "oom"]})
    second = ScriptedHost("h2", log, [gpu(1)])
    executor = executor_with(tmp_path, pinned, {"h1": first, "h2": second})
    job = island_job(tmp_path / "job")

    def attempt(label: str, number: int) -> None:
        executor._attempt(label, number, "audit", "b" * 64, job, tmp_path / "job", False, None)

    attempt("h1", 1)
    attempt("h2", 1)
    with pytest.raises(ce.AttemptFailed, match="out of memory"):
        attempt("h1", 2)
    attempt("h2", 2)
    attempt("h1", 3)
    assert (len(first.prepared), len(second.prepared)) == (2, 1)


def test_the_cache_directory_command_gets_exactly_thirty_seconds(
    tmp_path: Path, pinned: tuple[Path, Path]
) -> None:
    transport = PrepareTransport()
    executor = cache_executor(tmp_path, pinned, transport)
    before = time.time()
    ask_cache(executor, tmp_path)
    after = time.time()
    assert before + 30 <= transport.deadlines[0] <= after + 30


def test_a_cache_directory_of_400_characters_is_mounted_and_one_more_is_refused() -> None:
    args = (("docker",), IMAGE, NAME, "d" * 64, [uid(0)], "default")
    longest = "/" + "a" * 399
    assert f"type=bind,src={longest},dst=/root/.nv" in shlex.split(ce.run_command(*args, longest))
    with pytest.raises(ValueError, match="plain words"):
        ce.run_command(*args, longest + "a")


@pytest.mark.parametrize("word", ["caché", "٣", "日本"], ids=["latin", "arabic-digit", "cjk"])
def test_cache_directory_words_are_ascii(
    tmp_path: Path, pinned: tuple[Path, Path], word: str
) -> None:
    with pytest.raises(ValueError, match="plain words"):
        ce.run_command(("docker",), IMAGE, NAME, "d" * 64, [uid(0)], "default", f"/mnt/{word}")
    with pytest.raises(ValidationError, match="plain words"):
        load_config(
            config_data(
                tmp_path, pinned, hosts=[host_data(pinned, compute_cache_dir=f"/mnt/{word}")]
            )
        )


@pytest.mark.parametrize(
    "answer",
    [
        f"/home/ubuntu/{LEAF}\n",
        f"/home/ubuntu/.cortex-cuda-executor/{LEAF}\n",
        f"/home/ubuntu/compute-cache/{LEAF}\n",
    ],
    ids=["no-root", "no-compute-cache", "no-executor-directory"],
)
def test_a_report_of_the_right_leaf_in_the_wrong_place_is_refused(
    tmp_path: Path, pinned: tuple[Path, Path], answer: str
) -> None:
    executor = cache_executor(tmp_path, pinned, PrepareTransport(answer))
    with pytest.raises(ce.AttemptFailed, match="unexpected compute cache directory"):
        ask_cache(executor, tmp_path)
    assert executor._cache_dirs == {}


def test_a_report_that_is_not_text_is_refused_like_any_other_wrong_report(
    tmp_path: Path, pinned: tuple[Path, Path]
) -> None:
    class Binary(PrepareTransport):
        def run(self, command: str, *, stdout: Path, stderr: Path, **_: Any) -> int:
            stdout.write_bytes(b"\xff\xfe" + DEFAULT_DIR.encode() + b"\n")
            stderr.write_text("")
            return 0

    executor = cache_executor(tmp_path, pinned, Binary())
    with pytest.raises(ce.AttemptFailed, match="unexpected compute cache directory"):
        ask_cache(executor, tmp_path)


def test_a_cancelled_job_does_not_wait_for_the_cache_directory(
    tmp_path: Path, pinned: tuple[Path, Path]
) -> None:
    class Watching(PrepareTransport):
        """Stops like SshTransport does once the job's cancel event is set."""

        def run(
            self,
            command: str,
            *,
            stdout: Path,
            stderr: Path,
            deadline: float,
            cancel: threading.Event | None = None,
            **_: Any,
        ) -> int:
            if cancel is not None and cancel.is_set():
                raise ce.Aborted("lease cancelled")
            return super().run(command, stdout=stdout, stderr=stderr, deadline=deadline)

    cancel = threading.Event()
    cancel.set()
    executor = cache_executor(tmp_path, pinned, Watching())
    work = tmp_path / "work"
    work.mkdir()
    ref = SimpleNamespace(image_digest=DIGEST)
    with pytest.raises(ce.Aborted, match="lease cancelled"):
        executor._cache("h1", ref, time.time() + 600, cancel, work)  # type: ignore[arg-type]
    assert executor._cache_dirs == {}


def test_prepare_command_says_why_it_refuses_a_link(tmp_path: Path) -> None:
    assignment, _ = ce.cache_location(None, DIGEST)
    leaf = tmp_path / ".cortex-cuda-executor/compute-cache" / LEAF
    leaf.parent.mkdir(parents=True)
    leaf.symlink_to(tmp_path)
    done = shell_prepare(ce.prepare_command(assignment), tmp_path)
    assert done.returncode == 1 and done.stdout == ""
    assert "compute cache directory is a link" in done.stderr  # it reaches the journal


@pytest.mark.skipif(os.getuid() == 0, reason="root owns /usr: nothing would be refused")
def test_prepare_command_refuses_a_directory_the_user_does_not_own_before_it_touches_it() -> None:
    # /usr belongs to root; as root, chmod would succeed on a directory someone else can still
    # write to, so the owner is checked first and the mode is never changed
    before = Path("/usr").stat().st_mode
    done = subprocess.run(
        ["sh", "-c", ce.prepare_command("D=/usr")],
        capture_output=True,
        text=True,
        env={"PATH": os.environ["PATH"]},
    )
    assert done.returncode == 1 and done.stdout == ""
    assert "not owned by this user" in done.stderr
    assert Path("/usr").stat().st_mode == before
