"""Generic CUDA executor against a fake SSH + docker + nvidia-smi host (no network, no GPU).

The real `cuda_executor.load`, `SshTransport` (a subprocess `ssh`), runner source and artifact
checks run; only the host side is fake (tests/gpu_ops/fake_cuda_host.py) and the island training is
replaced by a canned, metadata-valid publication labelled CUDA. This is not GPU evidence.
"""

from __future__ import annotations

import builtins
import contextlib
import hashlib
import importlib.util
import json
import logging
import os
import re
import shlex
import shutil
import signal
import stat
import subprocess
import sys
import threading
import time
from copy import deepcopy
from pathlib import Path

import pytest

from hypertrain.gpu_ops import cuda_executor, cuda_executor_runner
from hypertrain.miner.island_launch import IslandFailure, validate_artifacts
from hypertrain.protocol.envelope import body_digest
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import CanonicalizationError, canonicalize

from . import fake_cuda_host as fake

spec = importlib.util.spec_from_file_location(
    "cuda_dispatch_backend_fixture",
    Path(__file__).parents[1] / "challenge/test_backend_authority_v2.py",
)
assert spec is not None and spec.loader is not None
backend = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = backend
spec.loader.exec_module(backend)
network = backend.network  # pytest fixture: the real challenge service and manifest

HANG_SECONDS = 120

# The contexts the store hands the dispatcher (challenge/store.py: reference ~3000, audit ~7243,
# referee ~8832): nested dicts and lists of strings and integers, receipts included.
DESCRIPTOR = {
    "round": 17,
    "referee": "5FakeRefereeForDispatch",
    "claims": [{"rank": 0, "root": "ef" * 32, "steps": [1, 2, 3]}, {"rank": 1, "root": "01" * 32}],
}
RECEIPT = {
    "v": "ht/2",
    "type": "AcceptV2",
    "body": {"epoch": 2, "refs": [], "x": None},
    "sig": "0a" * 64,
}
CONTEXTS = {
    "reference": {
        "run_id": "ab" * 32,
        "admission_id": "adm-7",
        "epoch": 12,
        "challenge": {"nonce": "cd" * 16, "deadline_beacon": 4100, "tags": ["a", "b"]},
        "challenge_hash": "ef" * 32,
        "challenge_envelope": RECEIPT,
        "hotkey": "5FakeHotkeyForDispatch",
    },
    "audit": {
        "run_id": "ab" * 32,
        "job_id": 41,
        "audit_job": {
            "w": 3,
            "sample_ids": [4, 5, 6],
            "manifest": {"training": {"k": [1, {"z": 2}]}},
        },
        "descriptor_hash": "cd" * 32,
        "descriptor": DESCRIPTOR,
        "descriptor_receipt": RECEIPT,
        "hotkey": "5FakeHotkeyForDispatch",
        "auditor": "5FakeAuditorForDispatch",
    },
    "referee": {
        "run_id": "ab" * 32,
        "dispute_id": 9,
        "turn": {"index": 2, "contest": {"referee": "5FakeRefereeForDispatch", "bonds": [10, 20]}},
        "descriptor_hash": "cd" * 32,
        "descriptor": DESCRIPTOR,
        "descriptor_receipt": RECEIPT,
        "historical_geometry": {"ranks": [0, 1], "steps": [{"t": 0}, {"t": 8}]},
    },
}
CONTEXT = CONTEXTS["audit"]


@contextlib.contextmanager
def hang_guard(seconds):
    """Fail the running test, from the main thread, once it has run for `seconds`."""
    if threading.current_thread() is not threading.main_thread():
        yield
        return

    def expired(signum, frame):
        pytest.fail(
            f"no progress after {seconds} s: a lock is held or a remote call is stuck",
            pytrace=False,
        )

    previous = signal.signal(signal.SIGALRM, expired)
    signal.setitimer(signal.ITIMER_REAL, seconds)
    try:
        yield
    finally:
        signal.setitimer(signal.ITIMER_REAL, 0)
        signal.signal(signal.SIGALRM, signal.SIG_DFL if previous is None else previous)


@pytest.fixture(autouse=True)
def _hang_guard():
    """A lock-release regression fails this test instead of wedging the whole CI shard."""
    with hang_guard(HANG_SECONDS):
        yield


def test_hang_guard_fails_a_stuck_test_and_leaves_no_alarm_behind():
    handler = signal.getsignal(signal.SIGALRM)
    with pytest.raises(pytest.fail.Exception, match="no progress after 0.3 s"):
        with hang_guard(0.3):
            time.sleep(5)  # cut short by the alarm
    assert signal.getsignal(signal.SIGALRM) is handler
    assert signal.getitimer(signal.ITIMER_REAL)[0] == 0
    with hang_guard(30):  # a test that finishes in time is left alone
        pass
    assert signal.getitimer(signal.ITIMER_REAL)[0] == 0


@pytest.fixture(scope="module")
def canned(tmp_path_factory):
    """One metadata-valid CUDA-labelled publication, shared read-only by every test."""
    manifest = fake.dispatch_manifest(
        backend.service.fixture.setup().manifest,
        n_gpus=2,
        sm_count=148,
        drivers=[fake.DRIVER, fake.OTHER_DRIVER],
    )
    return fake.canned_publication(
        manifest,
        backend.service.HOT[0].ss58,
        backend.saved_geometry,
        tmp_path_factory.mktemp("canned"),
        deadline=int(time.time()) + 3600,
    )


@pytest.fixture
def make_pool(tmp_path, monkeypatch, canned):
    def build(**kwargs):
        return fake.make_pool(tmp_path / "pool", monkeypatch, canned, **kwargs)

    return build


def dispatch(pool, home, operation="audit", *, factory=None, job=None, cancel=None):
    """The seam the challenge uses: factory(operation, job, home, context) -> launch(...)."""
    job = job or pool.canned.job
    factory = factory or cuda_executor.load(pool.config_path)
    launch = factory(operation, job, home, CONTEXTS[operation])
    return launch(job, home, backend="cuda", cancel=cancel, trace=operation != "reference")


def sha(raw):
    return hashlib.sha256(raw).hexdigest()


RUNNER_SHA = sha(Path(cuda_executor_runner.__file__).read_bytes())


def publication_digest(published):
    files = {
        p.relative_to(published).as_posix(): sha(p.read_bytes())
        for p in sorted(published.rglob("*"))
        if p.is_file()
    }
    return sha(canonicalize(files))


def uuids(host, *indices):
    return [fake.gpu_uuid(host, i) for i in indices]


def record_of(home):
    return json.loads((home / "role-dispatch.json").read_bytes())


def journal_kinds(pool):
    return [r["kind"] for r in fake.journal_records(pool.state_dir)]


def journal_of(pool, kind):
    return [r for r in fake.journal_records(pool.state_dir) if r["kind"] == kind]


def assert_nothing_published(home):
    assert not (home / "published").exists()
    assert not (home / "role-dispatch.json").exists()
    assert not (home / "role-dispatch.json").is_symlink()
    assert list(home.glob("attempt-*")) == []


def pid_alive(pid):
    try:
        state = Path(f"/proc/{pid}/stat").read_text().rpartition(")")[2].split()[0]
    except FileNotFoundError:
        return False
    return state != "Z"


def first_host(job, labels):
    """The host the executor tries first for this job: jobs are spread over the hosts by digest."""
    return labels[int(job.digest()[:8], 16) % len(labels)]


@pytest.mark.parametrize("operation", ["reference", "audit", "referee"])
def test_dispatch_publishes_validated_artifacts_with_accepted_provenance(
    network, make_pool, canned, tmp_path, operation
):
    pool = make_pool(labels=("a",))
    home = canned.stage(tmp_path / "home")
    artifacts = dispatch(pool, home, operation)

    published = home / "published"
    assert artifacts.directory == published.resolve() and len(artifacts.ranks) == 2
    validate_artifacts(canned.job, published)
    assert [(published / f"rank-{r}/trace.json").is_file() for r in range(2)] == [
        operation != "reference"
    ] * 2
    for relative, raw in canned.inputs.items():
        assert (published / relative).read_bytes() == raw
    assert (published / "job.json").read_bytes() == canonicalize(canned.job.model_dump(mode="json"))
    assert list(home.glob("attempt-*")) == []

    raw = (home / "role-dispatch.json").read_bytes()
    record = json.loads(raw)
    assert raw == canonicalize(record)
    (run,) = pool.toolbox.runs()
    ref = canned.manifest.training.reference_spec
    selected = uuids(0, 0, 1)
    host = record.pop("host")
    pinned = sha(pool.known_hosts["a"].read_bytes())
    assert host["label"] == "a" and host["known_hosts_sha256"] == pinned
    assert all(0 < host[k] < 2**52 for k in ("machine_id", "instance_id"))
    assert record.pop("environment") == {
        "backend": "cuda",
        "image_digest": ref.image_digest,
        "python": "/opt/hypertrain/venv/bin/python",
        "torch": "2.14.0+cu130",
        "torch_path": "/opt/hypertrain/venv/lib/python3.12/site-packages/torch/__init__.py",
        "cuda": "13.0",
        "drivers": [fake.DRIVER] * 2,
        "sm_counts": [148] * 2,
        "capabilities": [[10, 3]] * 2,
        "gpu_uuids": selected,
        "gpu_names": ["NVIDIA B300 SXM6 AC"] * 2,
    }
    assert record == {
        "executor": "cuda-executor-v1",
        "operation": operation,
        "binding": body_digest(CONTEXTS[operation]),
        "run_id": canned.job.run_id,
        "job_sha256": canned.job.digest(),
        "image": {"requested": pool.image, "repo_digests": [pool.image]},
        "runner_sha256": RUNNER_SHA,
        "result_tar_sha256": run["stdout_sha256"],
        "compute_cache": True,
        "publication_sha256": publication_digest(published),
    }

    replay, _ = network.store._dispatch_env_v2(canned.manifest, home)
    assert (replay.driver, replay.sm_count) == (fake.DRIVER, 148)
    assert replay.image_digest == ref.image_digest
    assert replay.gpu_uuid_sha256 == sha256_hex(",".join(selected).encode())

    # The journal names the container docker knows, so an uncertain run can be reconciled by name.
    ranks = sum(p.is_file() for r in range(2) for p in (published / f"rank-{r}").rglob("*"))
    (attempt,) = journal_of(pool, "exec_attempt")
    (done,) = journal_of(pool, "exec_published")
    assert {k: attempt[k] for k in ("label", "name", "operation", "job")} == {
        "label": "a",
        "name": run["name"],
        "operation": operation,
        "job": canned.job.digest(),
    }
    assert {k: done[k] for k in ("label", "name", "files", "job")} == {
        "label": "a",
        "name": run["name"],
        "files": ranks,
        "job": canned.job.digest(),
    }


@pytest.mark.parametrize("operation", ["reference", "audit"])
def test_container_receives_the_job_inputs_and_the_pinned_environment(
    make_pool, canned, tmp_path, operation
):
    pool = make_pool(labels=("a",))
    home = canned.stage(tmp_path / "home")
    dispatch(pool, home, operation)

    (launched,) = pool.toolbox.events("launch")
    expected = {r: sha(raw) for r, raw in canned.inputs.items()}
    expected["job.json"] = sha(canonicalize(canned.job.model_dump(mode="json")))
    assert launched["inputs"] == expected
    ref = canned.manifest.training.reference_spec
    devices = ",".join(uuids(0, 0, 1))
    assert launched["env"] == {
        "CUBLAS_WORKSPACE_CONFIG": ref.env.CUBLAS_WORKSPACE_CONFIG,
        "CUDA_DISABLE_PTX_JIT": ref.env.CUDA_DISABLE_PTX_JIT,
        "CUDA_VISIBLE_DEVICES": devices,
        "HT_GPU_UUIDS": devices,
        "HT_IMAGE_DIGEST": ref.image_digest,
        "HT_ISLAND_TRACE": "1" if operation == "audit" else "0",
        "MKL_NUM_THREADS": "1",
        "NVIDIA_TF32_OVERRIDE": "0",
        "OMP_NUM_THREADS": "1",
        "PYTHONHASHSEED": "0",
    }
    assert 0 < launched["budget"] <= canned.job.deadline - time.time()


@pytest.mark.parametrize("variant", ["plain", "sudo", "network-none", "no-cache"])
def test_container_is_started_by_digest_with_at_most_the_compute_cache_mounted(
    make_pool, canned, tmp_path, variant
):
    kwargs = {
        "plain": {},
        "sudo": {"docker": ("sudo", "-n", "docker")},
        "network-none": {"config": {"network": "none"}},
        "no-cache": {"config": {"compute_cache": False}},
    }[variant]
    pool = make_pool(labels=("a",), **kwargs)
    home = canned.stage(tmp_path / "home")
    dispatch(pool, home)

    # The flag-by-flag shape of the command is pinned by test_run_command_shape (units); this is
    # what only an attempt knows: the image, the devices, the name and the job digest it passes.
    (run,) = pool.toolbox.runs()
    options = {option[0]: option[1] for option in run["options"] if len(option) == 2}
    assert re.fullmatch(r"[a-z0-9./_-]+@sha256:[0-9a-f]{64}", run["image"])
    assert run["image"] == pool.image
    selected = uuids(0, 0, 1)
    assert run["devices"] == selected
    assert run["gpus_raw"] == '"device=' + ",".join(selected) + '"'
    assert re.fullmatch(r"ht-exec-[0-9a-f]{16}", options["--name"])
    assert options["--label"] == f"io.cortex.cuda-executor={canned.job.digest()[:16]}"
    assert run["source_sha256"] == RUNNER_SHA
    assert options.get("--network") == ("none" if variant == "network-none" else None)
    # The only mount there can be is the compute cache, in the ssh user's home, one directory per
    # image digest. No other host path, no volume flag, whatever the variant.
    leaf = canned.manifest.training.reference_spec.image_digest.removeprefix("sha256:")[:32]
    cache = pool.home("a") / ".cortex-cuda-executor/compute-cache" / leaf
    expected = (
        [] if variant == "no-cache" else [{"type": "bind", "src": str(cache), "dst": "/root/.nv"}]
    )
    assert run["mounts"] == expected
    assert [o[0] for o in run["options"] if o[0] in ("-v", "--volume", "--mount")] == (
        [] if variant == "no-cache" else ["--mount"]
    )
    prefix = "sudo -n docker" if variant == "sudo" else "docker"
    assert all(e["via_sudo"] == (variant == "sudo") for e in pool.toolbox.docker_log())
    docker_calls = [c for c in pool.toolbox.ssh_calls() if c["kind"] not in ("probe", "prepare")]
    assert docker_calls and all(c["command_head"].startswith(prefix + " ") for c in docker_calls)
    # The directory is made by the ssh user (never through sudo), and only when the cache is on.
    prepared = pool.toolbox.ssh_calls("prepare")
    assert len(prepared) == (0 if variant == "no-cache" else 1)
    assert all(not c["command_head"].startswith("sudo") for c in prepared)
    assert pool.toolbox.events("image_inspect")


@pytest.mark.parametrize(
    ("gpus", "expected"),
    [
        pytest.param(
            [fake.gpu(0, i) for i in (3, 2, 1, 0)], (0, 1), id="two-lowest-uuids-not-listing-order"
        ),
        pytest.param(
            [fake.gpu(0, 0, mem=2049), fake.gpu(0, 1), fake.gpu(0, 2, mem=2048), fake.gpu(0, 3)],
            (1, 2),
            id="memory-threshold-is-inclusive",
        ),
    ],
)
def test_the_lowest_idle_gpus_are_selected(make_pool, canned, tmp_path, gpus, expected):
    pool = make_pool(labels=("a",), hosts={"a": {"gpus": gpus, "busy": uuids(0, 3)}})
    home = canned.stage(tmp_path / "home")
    dispatch(pool, home)

    (run,) = pool.toolbox.runs()
    chosen = uuids(0, *expected)
    assert run["devices"] == chosen
    assert record_of(home)["environment"]["gpu_uuids"] == chosen


def test_missing_image_is_pulled_by_digest_before_the_run(make_pool, canned, tmp_path):
    pool = make_pool(labels=("a",), docker=("sudo", "-n", "docker"), hosts={"a": {"images": []}})
    home = canned.stage(tmp_path / "home")
    dispatch(pool, home)

    sequence = [e["event"] for e in pool.toolbox.docker_log()]
    assert sequence == ["image_inspect", "pull", "image_inspect", "run_start", "launch", "run_end"]
    (pulled,) = pool.toolbox.events("pull")
    assert pulled["ref"] == pool.image and "--quiet" in pulled["args"] and pulled["via_sudo"]
    assert record_of(home)["image"] == {"requested": pool.image, "repo_digests": [pool.image]}


def test_denied_pull_makes_the_host_unavailable_and_runs_nothing(make_pool, canned, tmp_path):
    pool = make_pool(labels=("a",), hosts={"a": {"images": [], "pull": {"fails": True}}})
    home = canned.stage(tmp_path / "home")
    with pytest.raises(IslandFailure, match="image pull exited 1: .*pull access denied"):
        dispatch(pool, home)

    assert pool.toolbox.runs() == [] and journal_kinds(pool) == ["exec_host_unavailable"] * 3
    assert_nothing_published(home)


def test_ssh_is_hardened_and_never_inherits_the_challenge_environment(
    make_pool, canned, tmp_path, monkeypatch
):
    names = (
        "HYPERTRAIN_CANARY_SECRET",
        "HYPERTRAIN_BEARER_TOKEN",
        "OPENROUTER_API_KEY",
        "AWS_SECRET_ACCESS_KEY",
        "SSH_AUTH_SOCK",
        "SSH_ASKPASS",
    )
    planted = {name: "/nonexistent/" + name.lower() for name in names}
    for name, value in planted.items():
        monkeypatch.setenv(name, value)
    pool = make_pool(labels=("a",))
    dispatch(pool, canned.stage(tmp_path / "home"))

    calls = pool.toolbox.ssh_log()
    assert {c["kind"] for c in calls} == {"probe", "prepare", "run"}
    for call in calls:
        argv = call["argv"]
        assert call["env_names"] == ["HOME", "LC_ALL", "PATH"]
        assert argv[argv.index("-F") + 1] == "/dev/null"
        assert argv[argv.index("-i") + 1] == str(pool.identity)
        assert argv[argv.index("-p") + 1] == "22" and "-T" in argv
        assert {argv[i + 1] for i, a in enumerate(argv) if a == "-o"} >= {
            "StrictHostKeyChecking=yes",
            "BatchMode=yes",
            "IdentitiesOnly=yes",
            f"UserKnownHostsFile={pool.known_hosts['a']}",
            "GlobalKnownHostsFile=/dev/null",
            "IdentityAgent=none",
            "ForwardAgent=no",
            "ClearAllForwardings=yes",
            "PasswordAuthentication=no",
            "KbdInteractiveAuthentication=no",
            "ControlMaster=no",
            "ControlPath=none",
            "LogLevel=ERROR",
            "ServerAliveInterval=15",
            "ServerAliveCountMax=3",
            "ConnectTimeout=15",
        }
        separator = argv.index("--")
        assert argv[separator + 1] == "ubuntu@192.0.2.10" and len(argv) == separator + 3


def test_each_host_is_reached_with_its_own_user_port_key_and_pinned_host_key(
    make_pool, canned, tmp_path
):
    labels = ("a", "b")
    tried_first = first_host(canned.job, labels)
    other = labels[1 - labels.index(tried_first)]
    # The host tried first has no idle GPU, so both hosts get probed and one of them runs the job.
    pool = make_pool(
        labels=labels,
        config={"ssh_connect_timeout_seconds": 30},
        hosts={tried_first: {"busy": uuids(labels.index(tried_first), 0, 1)}},
    )
    home = canned.stage(tmp_path / "home")
    dispatch(pool, home)

    specs, owner = pool.specs, {address: label for label, address in pool.addresses.items()}
    for key in ("user", "port", "identity_file", "known_hosts_file"):
        assert len({spec[key] for spec in specs.values()}) == 2  # nothing is shared by accident
    assert specs["b"]["port"] == 2222
    calls = pool.toolbox.ssh_log()
    assert {call["host"] for call in calls} == set(pool.addresses.values())
    for call in calls:
        spec, argv = specs[owner[call["host"]]], call["argv"]
        options = {argv[i + 1] for i, arg in enumerate(argv) if arg == "-o"}
        assert call["destination"] == f"{spec['user']}@{spec['address']}"
        assert argv[argv.index("-p") + 1] == str(spec["port"])
        assert argv[argv.index("-i") + 1] == spec["identity_file"]
        assert f"UserKnownHostsFile={spec['known_hosts_file']}" in options
        assert "ConnectTimeout=30" in options
    assert record_of(home)["host"]["label"] == other


def test_unpinned_host_key_loses_the_host_and_nothing_is_run(make_pool, canned, tmp_path):
    pool = make_pool(labels=("a",), hosts={"a": {"hostkey_ok": False}})
    home = canned.stage(tmp_path / "home")
    with pytest.raises(IslandFailure, match="Host key verification failed"):
        dispatch(pool, home)

    assert pool.toolbox.docker_log() == []
    assert {c["exit"] for c in pool.toolbox.ssh_log()} == {255}
    assert {r["kind"] for r in fake.journal_records(pool.state_dir)} == {"exec_host_unavailable"}
    assert_nothing_published(home)


@pytest.mark.parametrize(("query", "exit_code"), [("query-gpu", 41), ("query-compute-apps", 42)])
def test_host_whose_nvidia_smi_query_fails_is_unavailable_and_never_taken_for_idle(
    make_pool, canned, tmp_path, query, exit_code
):
    labels = ("a", "b")
    lost = first_host(canned.job, labels)
    other = labels[1 - labels.index(lost)]
    # Every GPU of the failing host is busy: reading its empty answer as "idle" would run on it.
    busy = uuids(labels.index(lost), 0, 1)
    pool = make_pool(labels=labels, hosts={lost: {"smi_fail": query, "busy": busy}})
    home = canned.stage(tmp_path / "home")
    dispatch(pool, home)

    assert [run["host"] for run in pool.toolbox.runs()] == [pool.address(other)]
    assert pool.toolbox.ssh_calls("run", pool.address(lost)) == []
    (unavailable,) = journal_of(pool, "exec_host_unavailable")
    assert unavailable["label"] == lost
    assert re.search(
        rf"host probe exited {exit_code}: .*Unable to determine the device handle",
        unavailable["reason"],
    )
    assert journal_of(pool, "exec_host_refused") == []  # lost, not quarantined
    assert record_of(home)["host"]["label"] == other


REFUSALS = {
    "pull-yields-another-digest": (
        {"images": [], "pull": {"digest": "sha256:" + "cd" * 32}},
        "image digest absent from the host's RepoDigests",
        0,
    ),
    "daemon-reports-another-digest": (
        {"inspect_reports": [f"{fake.REPOSITORY}@sha256:" + "cd" * 32]},
        "image digest absent from the host's RepoDigests",
        0,
    ),
    "driver-outside-the-allowlist": (
        {"gpus": [fake.gpu(0, i, driver="580.00.00") for i in range(2)]},
        "driver is not the one allowlisted by the manifest",
        0,
    ),
    "mixed-drivers": (
        {"gpus": [fake.gpu(0, 0), fake.gpu(0, 1, driver=fake.OTHER_DRIVER)]},
        "driver is not the one allowlisted by the manifest",
        0,
    ),
    "capability-sm-pair-not-in-the-table": (
        {"gpus": [fake.gpu(0, i, cc="12.0", sm=148) for i in range(2)]},
        r"device 0: capability 12\.0 qualifies \[170\] SMs, not 148",
        0,
    ),
    "probe-field-over-the-csv-limit": (
        {"gpus": [{**fake.gpu(0, i), "name": "n" * 200_000} for i in range(2)]},
        "unparseable host probe: Error",
        0,
    ),
    "clock-ahead": ({"clock_skew": 30}, "host clock differs from the challenge clock", 0),
    "clock-behind": ({"clock_skew": -30}, "host clock differs from the challenge clock", 0),
    "container-reports-another-sm-count": (
        {"runs": [{"mode": "env", "env": {"sm_counts": [170, 170]}}]},
        r"device 0: capability 10\.3 qualifies \[148\] SMs, not 170",
        1,
    ),
    "container-reports-another-device-class": (
        {
            "runs": [
                {"mode": "env", "env": {"capabilities": [[12, 0]] * 2, "sm_counts": [170, 170]}}
            ]
        },
        "container environment contradicts the host probe",
        1,
    ),
    "container-driver-differs-from-the-host-probe": (
        {"runs": [{"mode": "env", "env": {"drivers": [fake.OTHER_DRIVER] * 2}}]},
        "container environment contradicts the host probe",
        1,
    ),
    "container-sees-other-uuids": (
        {"runs": [{"mode": "env", "env": {"gpu_uuids": uuids(0, 2, 3)}}]},
        "container refused the devices: .*visible GPUs differ from the requested devices",
        1,
    ),
    "container-reports-a-uuid-that-is-not-hex": (
        {
            "runs": [
                {
                    "mode": "mutate",
                    "name": "environment",
                    "set": {
                        "gpu_uuids": ["GPU-zzzzzzzz-zzzz-zzzz-zzzz-zzzzzzzzzzzz", uuids(0, 0, 1)[1]]
                    },
                }
            ]
        },
        "container reported a malformed GPU UUID",
        1,
    ),
    "environment-json-lists-the-uuids-reordered": (
        {"runs": [{"mode": "mutate", "name": "environment", "set": {"gpu_uuids": uuids(0, 1, 0)}}]},
        "container sees other devices than the ones selected",
        1,
    ),
}


@pytest.mark.parametrize("case", REFUSALS)
def test_host_contradicting_the_manifest_is_refused_and_nothing_is_published(
    make_pool, canned, tmp_path, case
):
    overrides, cause, container_runs = REFUSALS[case]
    pool = make_pool(labels=("a",), hosts={"a": overrides})
    home = canned.stage(tmp_path / "home")
    with pytest.raises(IslandFailure, match=cause) as caught:
        dispatch(pool, home)

    assert not isinstance(caught.value, cuda_executor.Aborted)
    assert "a refused" in str(caught.value)
    assert_nothing_published(home)
    (refusal,) = journal_of(pool, "exec_host_refused")
    assert refusal["label"] == "a" and re.search(cause, refusal["reason"])
    assert "exec_published" not in journal_kinds(pool)
    assert len(pool.toolbox.runs()) == container_runs  # a host seen to contradict runs nothing


@pytest.mark.parametrize(
    ("limit", "taken"), [(None, False), (4096, True)], ids=["default-2048", "configured-4096"]
)
def test_the_idle_gpu_memory_limit_comes_from_the_config(make_pool, canned, tmp_path, limit, taken):
    gpus = [fake.gpu(0, i, mem=3000) for i in range(2)]  # 3000 MiB resident on each GPU
    config = {"sweeps": 1, **({} if limit is None else {"idle_gpu_memory_mib": limit})}
    pool = make_pool(labels=("a",), config=config, hosts={"a": {"gpus": gpus}})
    home = canned.stage(tmp_path / "home")
    if taken:
        dispatch(pool, home)
        assert record_of(home)["environment"]["gpu_uuids"] == uuids(0, 0, 1)
    else:
        with pytest.raises(IslandFailure, match="0 idle GPUs, 2 needed"):
            dispatch(pool, home)
        assert pool.toolbox.runs() == []


def test_a_host_on_the_second_allowlisted_driver_is_accepted_end_to_end(
    network, make_pool, canned, tmp_path
):
    gpus = [fake.gpu(0, i, driver=fake.OTHER_DRIVER) for i in range(2)]
    pool = make_pool(labels=("a",), hosts={"a": {"gpus": gpus}})
    home = canned.stage(tmp_path / "home")
    dispatch(pool, home)

    assert record_of(home)["environment"]["drivers"] == [fake.OTHER_DRIVER] * 2
    replay, _ = network.store._dispatch_env_v2(canned.manifest, home)
    assert replay.driver == fake.OTHER_DRIVER


def test_repo_digests_naming_other_images_too_are_kept_in_the_reported_order(
    make_pool, canned, tmp_path
):
    other = f"{fake.REPOSITORY}@sha256:" + "ff" * 32
    assert canned.image < other  # listed first, it makes the pair unsorted, with ours second
    pool = make_pool(labels=("a",), hosts={"a": {"inspect_reports": [other, canned.image]}})
    home = canned.stage(tmp_path / "home")
    dispatch(pool, home)

    assert record_of(home)["image"] == {
        "requested": canned.image,
        "repo_digests": [other, canned.image],
    }


def test_a_refused_host_stays_quarantined_until_the_challenge_restarts(make_pool, canned, tmp_path):
    pool = make_pool(
        labels=("a",), hosts={"a": {"gpus": [fake.gpu(0, i, driver="580.00.00") for i in range(2)]}}
    )
    factory = cuda_executor.load(pool.config_path)
    with pytest.raises(IslandFailure, match="allowlisted"):
        dispatch(pool, canned.stage(tmp_path / "first"), factory=factory)
    calls = len(pool.toolbox.ssh_log())

    pool.toolbox.scenario.update(
        pool.address("a"), gpus=[fake.gpu(0, i, driver=fake.DRIVER) for i in range(2)]
    )
    with pytest.raises(
        IslandFailure,
        match="no host able to run the job: no host answered; "
        "a was refused: driver is not the one allowlisted by the manifest",
    ):
        dispatch(pool, canned.stage(tmp_path / "second"), factory=factory)
    assert len(pool.toolbox.ssh_log()) == calls

    restarted = cuda_executor.load(pool.config_path)
    dispatch(pool, canned.stage(tmp_path / "third"), factory=restarted)
    assert (tmp_path / "third/published").is_dir()


def test_the_same_machine_behind_a_second_label_is_never_used(make_pool, canned, tmp_path):
    pool = make_pool(labels=("a", "b"), same_machine=True)
    factory = cuda_executor.load(pool.config_path)
    first = canned.stage(tmp_path / "first")
    dispatch(pool, first, factory=factory)
    used = record_of(first)["host"]["label"]
    twin = "b" if used == "a" else "a"

    pool.toolbox.scenario.update(pool.address(used), down=True)
    second = canned.stage(tmp_path / "second")
    with pytest.raises(IslandFailure, match="no host able to run the job"):
        dispatch(pool, second, factory=factory)

    assert_nothing_published(second)
    assert {r["label"] for r in journal_of(pool, "exec_host_unavailable")} == {used}
    (refusal,) = journal_of(pool, "exec_host_refused")
    assert (refusal["label"], refusal["reason"]) == (twin, f"same machine as host {used}")
    assert len(pool.toolbox.runs()) == 1  # the duplicate never got a container


def test_a_machine_swapped_behind_the_same_label_is_refused(make_pool, canned, tmp_path):
    pool = make_pool(labels=("a",))
    factory = cuda_executor.load(pool.config_path)
    dispatch(pool, canned.stage(tmp_path / "first"), factory=factory)

    pool.toolbox.scenario.update(pool.address("a"), gpus=[fake.gpu(7, i) for i in range(2)])
    second = canned.stage(tmp_path / "second")
    with pytest.raises(IslandFailure, match="a refused: the machine behind this host changed"):
        dispatch(pool, second, factory=factory)

    assert_nothing_published(second)
    assert len(pool.toolbox.runs()) == 1


@pytest.mark.parametrize("host_down", [False, True], ids=["host-stays-up", "host-stays-down"])
def test_job_lost_with_its_host_is_retried_on_another_host(make_pool, canned, tmp_path, host_down):
    pool = make_pool(labels=("a", "b"), docker=("sudo", "-n", "docker"))
    pool.toolbox.scenario.global_runs({"mode": "vanish", "host_down": host_down}, {"mode": "ok"})
    home = canned.stage(tmp_path / "home")
    dispatch(pool, home)

    lost, served = pool.toolbox.runs()
    label_of = {address: label for label, address in pool.addresses.items()}
    assert lost["host"] != served["host"] and lost["code"] == 199
    assert [c["exit"] for c in pool.toolbox.ssh_calls("run", lost["host"])] == [255]
    (unavailable,) = journal_of(pool, "exec_host_unavailable")
    assert unavailable["label"] == label_of[lost["host"]]
    assert "ssh could not reach or authenticate the host" in unavailable["reason"]

    (cleanup,) = pool.toolbox.ssh_calls("rm", lost["host"])
    assert cleanup["command_head"] == f"sudo -n docker rm -f {lost['name']}"
    if host_down:
        assert cleanup["exit"] == 255 and pool.toolbox.events("rm") == []
        assert journal_of(pool, "exec_cleanup_failed")[0]["name"] == lost["name"]
    else:
        assert [(e["name"], e["via_sudo"]) for e in pool.toolbox.events("rm")] == [
            (lost["name"], True)
        ]

    record = record_of(home)
    assert record["host"]["label"] == label_of[served["host"]]
    assert record["result_tar_sha256"] == served["stdout_sha256"] != lost["stdout_sha256"]
    for source in sorted(canned.tree.rglob("*")):
        if source.is_file():
            published = home / "published" / source.relative_to(canned.tree)
            assert published.read_bytes() == source.read_bytes()
    assert list(home.glob("attempt-*")) == []


def _link_leaving_the_directory(canned, home, tmp_path):
    relative = sorted(canned.inputs)[0]
    outside = tmp_path / "outside.bin"
    outside.write_bytes(canned.inputs[relative])  # same bytes: only the link is wrong
    (home / relative).unlink()
    (home / relative).symlink_to(outside)
    return canned.job


def _object_named_like(transport_file):
    def plant(canned, home, tmp_path):
        (home / transport_file).write_bytes(b"{}")
        paths = {**canned.job.object_paths, "shadow": transport_file}
        return canned.job.model_copy(update={"object_paths": paths})

    return plant


@pytest.mark.parametrize(
    ("plant", "cause"),
    [
        pytest.param(
            _link_leaving_the_directory, "object escapes attempt directory", id="link-out"
        ),
        pytest.param(
            _object_named_like("request.json"), "collides with the transport", id="request"
        ),
        pytest.param(_object_named_like("job.json"), "collides with the transport", id="job"),
    ],
)
def test_object_that_leaves_the_directory_or_shadows_the_transport_runs_no_container(
    make_pool, canned, tmp_path, plant, cause
):
    pool = make_pool(labels=("a",))
    home = canned.stage(tmp_path / "home")
    job = plant(canned, home, tmp_path)
    with pytest.raises(IslandFailure, match=cause) as caught:
        dispatch(pool, home, job=job)

    assert type(caught.value) is IslandFailure  # not a host verdict: no quarantine, no retry
    assert pool.toolbox.ssh_log() == [] and pool.toolbox.docker_log() == []  # not even a probe
    assert {"exec_host_refused", "exec_attempt_failed", "exec_published"}.isdisjoint(
        journal_kinds(pool)
    )
    assert_nothing_published(home)


# Host-controlled bytes that reach a parser of ours: whatever it raises must be a failed attempt.
UNPARSEABLE_RESULTS = {
    "corrupt-state-safetensors": (
        "result failed validation",
        {"mode": "mutate", "name": "corrupt-state"},
    ),
    "trace-valid-json-not-a-list": (
        "result failed validation: requested trace is absent or malformed",
        {"mode": "mutate", "name": "trace-not-a-list"},
    ),
    "trace-not-json": ("result failed validation", {"mode": "mutate", "name": "trace-garbage"}),
    "environment-5000-character-path": (
        "result metadata is malformed",
        {"mode": "env", "env": {"torch_path": "/x" * 2500}},
    ),
}
MALFORMED_RESULTS = {
    **UNPARSEABLE_RESULTS,
    "truncated": ("COMPLETE member", {"mode": "truncate"}),
    "symlink-member": ("unacceptable result member", {"mode": "mutate", "name": "symlink-member"}),
    "traversal-name": ("unacceptable result member", {"mode": "mutate", "name": "traversal-name"}),
    "foreign-name": ("unacceptable result member", {"mode": "mutate", "name": "foreign-name"}),
    "duplicate-member": (
        "unacceptable result member",
        {"mode": "mutate", "name": "duplicate-member"},
    ),
    "member-after-complete": (
        "unacceptable result member",
        {"mode": "mutate", "name": "member-after-complete"},
    ),
    "extra-rank-file": (
        "COMPLETE does not match the received bytes",
        {"mode": "mutate", "name": "extra-rank-file"},
    ),
    "flipped-byte": (
        "COMPLETE does not match the received bytes",
        {"mode": "mutate", "name": "flipped-byte"},
    ),
    "wrong-request-hash": (
        "COMPLETE does not match the received bytes",
        {"mode": "mutate", "name": "wrong-request-hash"},
    ),
    "cpu-backend": (
        "rank summary is not a CUDA publication",
        {"mode": "mutate", "name": "cpu-backend"},
    ),
    "no-trace": (
        "requested trace is absent or malformed",
        {"mode": "mutate", "name": "no-trace"},
    ),
    "corrupt-leaves": ("result failed validation", {"mode": "mutate", "name": "corrupt-leaves"}),
    "wrong-job-hash": (
        "result failed validation: rank job binding mismatch",
        {"mode": "mutate", "name": "wrong-job-hash"},
    ),
}


@pytest.mark.parametrize("case", MALFORMED_RESULTS)
def test_tampered_or_incomplete_result_is_refused_and_nothing_is_published(
    make_pool, canned, tmp_path, case
):
    cause, run = MALFORMED_RESULTS[case]
    pool = make_pool(labels=("a",), config={"max_attempts": 1}, hosts={"a": {"runs": [run]}})
    home = canned.stage(tmp_path / "home")
    with pytest.raises(IslandFailure, match=f"cuda executor gave up: a: .*{cause}"):
        dispatch(pool, home)

    assert_nothing_published(home)
    assert journal_kinds(pool).count("exec_attempt_failed") == 1
    assert "exec_published" not in journal_kinds(pool) and len(pool.toolbox.runs()) == 1


def test_refused_result_is_followed_by_a_fresh_attempt_and_only_its_bytes_are_published(
    make_pool, canned, tmp_path
):
    flipped = {"mode": "mutate", "name": "flipped-byte"}
    pool = make_pool(labels=("a",), hosts={"a": {"runs": [flipped, {"mode": "ok"}]}})
    home = canned.stage(tmp_path / "home")
    dispatch(pool, home)

    refused, accepted = pool.toolbox.runs()
    assert refused["name"] != accepted["name"]
    assert refused["stdout_sha256"] != accepted["stdout_sha256"]
    assert record_of(home)["result_tar_sha256"] == accepted["stdout_sha256"]
    assert (home / "published/rank-0/delta.bin").read_bytes() == (
        canned.tree / "rank-0/delta.bin"
    ).read_bytes()
    assert journal_kinds(pool) == [
        "exec_attempt",
        "exec_attempt_failed",
        "exec_attempt",
        "exec_published",
    ]


@pytest.mark.parametrize("case", UNPARSEABLE_RESULTS)
def test_result_that_breaks_one_of_our_parsers_fails_over_to_the_next_host(
    make_pool, canned, tmp_path, case
):
    cause, run = UNPARSEABLE_RESULTS[case]
    pool = make_pool(labels=("a", "b"))
    pool.toolbox.scenario.global_runs(run, {"mode": "ok"})
    home = canned.stage(tmp_path / "home")
    dispatch(pool, home)

    refused, served = pool.toolbox.runs()
    label_of = {address: label for label, address in pool.addresses.items()}
    assert refused["host"] != served["host"]
    (failed,) = journal_of(pool, "exec_attempt_failed")
    assert failed["label"] == label_of[refused["host"]] and re.search(cause, failed["reason"])
    assert journal_of(pool, "exec_host_refused") == []  # a bad result is not a bad host
    record = record_of(home)
    assert record["host"]["label"] == label_of[served["host"]]
    assert record["result_tar_sha256"] == served["stdout_sha256"] != refused["stdout_sha256"]
    for source in sorted(canned.tree.rglob("*")):
        if source.is_file():
            published = home / "published" / source.relative_to(canned.tree)
            assert published.read_bytes() == source.read_bytes()
    assert list(home.glob("attempt-*")) == []


OOM = {"mode": "exit", "code": 1, "stderr": "CUDA out of memory"}


def test_container_failure_is_retried_on_the_same_single_host(make_pool, canned, tmp_path):
    pool = make_pool(labels=("a",), hosts={"a": {"runs": [OOM, {"mode": "ok"}]}})
    home = canned.stage(tmp_path / "home")
    dispatch(pool, home)

    first, second = pool.toolbox.runs()
    assert (first["code"], second["code"]) == (1, 0) and first["name"] != second["name"]
    (failed,) = journal_of(pool, "exec_attempt_failed")
    assert "container exited 1" in failed["reason"] and "CUDA out of memory" in failed["reason"]
    assert (home / "published").is_dir()


@pytest.mark.parametrize("code", [0, 1, 137], ids=["control-exit-0", "exit-1", "exit-137-oom-kill"])
def test_container_failing_after_a_complete_valid_stream_publishes_nothing(
    make_pool, canned, tmp_path, code
):
    # The first stream is complete and valid (its environment names a different torch build, so its
    # bytes differ from the retry's); only the exit status can tell it from a good result.
    first_torch = "2.14.0+cu130-first-attempt"
    streamed = {"mode": "exit-after", "code": code, "env": {"torch": first_torch}}
    pool = make_pool(labels=("a",), hosts={"a": {"runs": [streamed, {"mode": "ok"}]}})
    home = canned.stage(tmp_path / "home")
    dispatch(pool, home)

    if code == 0:  # control: accepted as it is, so the failures below are down to the exit status
        (only,) = pool.toolbox.runs()
        assert record_of(home)["environment"]["torch"] == first_torch
        assert record_of(home)["result_tar_sha256"] == only["stdout_sha256"]
        return
    first, second = pool.toolbox.runs()
    assert (first["code"], second["code"]) == (code, 0)
    (failed,) = journal_of(pool, "exec_attempt_failed")
    assert f"container exited {code}" in failed["reason"]
    assert journal_kinds(pool) == [
        "exec_attempt",
        "exec_attempt_failed",
        "exec_attempt",
        "exec_published",
    ]
    record = record_of(home)
    assert record["result_tar_sha256"] == second["stdout_sha256"] != first["stdout_sha256"]
    assert record["environment"]["torch"] == "2.14.0+cu130"


def test_container_whose_device_probe_cannot_run_costs_an_attempt_and_quarantines_nothing(
    make_pool, canned, tmp_path
):
    # Runner exit 9. Were it a free retry, the third (default) run would succeed and publish; were
    # it a refusal of the host, the failure would read "no host able to run the job".
    broken = {"mode": "probe-fail"}
    pool = make_pool(labels=("a",), config={"max_attempts": 2}, hosts={"a": {"runs": [broken] * 2}})
    home = canned.stage(tmp_path / "home")
    cause = "gave up: a: the device probe in the container could not run: .*NVML mismatch"
    with pytest.raises(IslandFailure, match=cause):
        dispatch(pool, home)

    assert [run["code"] for run in pool.toolbox.runs()] == [9, 9]
    assert journal_of(pool, "exec_host_refused") == []
    assert journal_kinds(pool).count("exec_attempt_failed") == 2
    assert_nothing_published(home)


def test_remote_stderr_is_shown_as_a_bounded_tail_without_control_characters(
    make_pool, canned, tmp_path
):
    noise = "HEAD" + "." * 3000 + "\x1b[31mTAIL-red\x1b[0m\r\nlast\tline"
    crash = {"mode": "exit", "code": 1, "stderr": noise}
    pool = make_pool(labels=("a",), config={"max_attempts": 1}, hosts={"a": {"runs": [crash]}})
    with pytest.raises(IslandFailure) as caught:
        dispatch(pool, canned.stage(tmp_path / "home"))

    text = str(caught.value)
    assert "container exited 1" in text and "TAIL-red" in text and "last line" in text
    assert "HEAD" not in text  # only the tail of the remote stderr is kept
    assert text.isprintable()  # no escape sequence, carriage return, newline or tab reaches a log
    (failed,) = journal_of(pool, "exec_attempt_failed")
    assert failed["reason"].isprintable() and "TAIL-red" in failed["reason"]


def test_attempts_are_bounded_by_max_attempts(make_pool, canned, tmp_path):
    pool = make_pool(labels=("a",), config={"max_attempts": 2}, hosts={"a": {"runs": [OOM] * 5}})
    home = canned.stage(tmp_path / "home")
    with pytest.raises(IslandFailure, match="gave up: a: container exited 1: .*CUDA out of memory"):
        dispatch(pool, home)

    assert len(pool.toolbox.ssh_calls("run")) == 2 and len(pool.toolbox.runs()) == 2
    assert_nothing_published(home)


def test_busy_host_is_waited_out_without_spending_an_attempt(make_pool, canned, tmp_path):
    host = {"busy_sequence": [uuids(0, 0)], "runs": [OOM, {"mode": "ok"}]}
    pool = make_pool(labels=("a",), config={"max_attempts": 2}, hosts={"a": host})
    home = canned.stage(tmp_path / "home")
    dispatch(pool, home)

    assert journal_kinds(pool)[:2] == ["exec_host_unavailable", "exec_attempt"]
    assert "1 idle GPUs, 2 needed" in journal_of(pool, "exec_host_unavailable")[0]["reason"]
    assert [r["code"] for r in pool.toolbox.runs()] == [1, 0]
    assert (home / "published").is_dir()


IDLE_SHORTAGES = {
    "foreign-compute-app": lambda machine: {"busy": uuids(machine, 0)},
    "memory-in-use": lambda machine: {
        "gpus": [fake.gpu(machine, 0, mem=2049), fake.gpu(machine, 1)]
    },
    "fewer-gpus-than-the-layout": lambda machine: {"gpus": [fake.gpu(machine, 0)]},
}


@pytest.mark.parametrize("shortage", IDLE_SHORTAGES)
def test_hosts_without_enough_idle_gpus_fail_after_every_sweep(
    make_pool, canned, tmp_path, shortage
):
    hosts = {"a": IDLE_SHORTAGES[shortage](0), "b": IDLE_SHORTAGES[shortage](1)}
    pool = make_pool(labels=("a", "b"), config={"sweeps": 2}, hosts=hosts)
    home = canned.stage(tmp_path / "home")
    with pytest.raises(IslandFailure, match="no host able to run the job: .*1 idle GPUs, 2 needed"):
        dispatch(pool, home)

    assert len(pool.toolbox.ssh_calls("probe")) == 4  # two hosts, two sweeps: never given up on
    assert pool.toolbox.runs() == [] and journal_kinds(pool) == ["exec_host_unavailable"] * 4
    assert_nothing_published(home)


def test_a_result_beyond_the_smallest_cap_is_accepted_under_the_configured_one(
    make_pool, canned, tmp_path
):
    large = {"mode": "oversize", "bytes": 3 << 20, "linger": 0}  # the stream outgrows 1 MiB
    pool = make_pool(labels=("a",), config={"max_attempts": 1}, hosts={"a": {"runs": [large]}})
    home = canned.stage(tmp_path / "home")
    dispatch(pool, home)  # a second attempt would be a plain, small result: there is only one

    assert (home / "role-dispatch.json").is_file() and (home / "published").is_dir()


@pytest.mark.parametrize("linger", [20, 0], ids=["stream-still-open", "stream-already-closed"])
def test_result_beyond_max_result_bytes_is_refused(make_pool, canned, tmp_path, linger):
    oversize = {"mode": "oversize", "bytes": 3 << 20, "linger": linger}
    pool = make_pool(
        labels=("a",),
        config={"max_attempts": 1, "max_result_bytes": 1 << 20},
        hosts={"a": {"runs": [oversize]}},
    )
    home = canned.stage(tmp_path / "home")
    with pytest.raises(IslandFailure, match="gave up: a: result exceeds max_result_bytes"):
        dispatch(pool, home)

    assert_nothing_published(home)
    if linger:  # the container was still running: it is removed by name
        assert [e["name"] for e in pool.toolbox.events("rm")] == [pool.toolbox.runs()[0]["name"]]


@pytest.mark.parametrize("linger", [10, 0], ids=["container-still-running", "container-gone"])
def test_remote_stderr_beyond_its_bound_fails_the_attempt(make_pool, canned, tmp_path, linger):
    flood = {"mode": "flood", "bytes": 17 << 20, "linger": linger}
    pool = make_pool(labels=("a",), config={"max_attempts": 1}, hosts={"a": {"runs": [flood]}})
    home = canned.stage(tmp_path / "home")
    with pytest.raises(IslandFailure, match="gave up: a: remote stderr exceeds its bound"):
        dispatch(pool, home)

    assert_nothing_published(home)
    if linger:  # the container was still running: it is removed by name
        assert [e["name"] for e in pool.toolbox.events("rm")] == [pool.toolbox.runs()[0]["name"]]


@pytest.mark.parametrize("trigger", ["deadline", "cancel"])
def test_abort_during_a_run_kills_ssh_removes_the_container_and_does_not_retry(
    make_pool, canned, tmp_path, monkeypatch, trigger
):
    clock = fake.SteppedClock()
    monkeypatch.setattr(cuda_executor, "time", clock)
    pool = make_pool(labels=("a",), hosts={"a": {"runs": [{"mode": "hang"}]}})
    home = canned.stage(tmp_path / "home")
    cancel = threading.Event()

    def fire():
        fake.wait_for(lambda: pool.toolbox.events("launch"))
        if trigger == "deadline":
            clock.offset += 7200
        else:
            cancel.set()

    watcher = threading.Thread(target=fire)
    watcher.start()
    cause = "job deadline expired" if trigger == "deadline" else "lease cancelled"
    try:
        with pytest.raises(cuda_executor.Aborted, match=cause):
            dispatch(pool, home, cancel=cancel)
    finally:
        watcher.join()

    (run,) = pool.toolbox.runs()
    assert [c["exit"] for c in pool.toolbox.ssh_calls("run")] == [None]  # killed, never exited
    fake.wait_for(lambda: not pid_alive(run["pid"]), timeout=5)
    assert [e["name"] for e in pool.toolbox.events("rm")] == [run["name"]]
    assert "exec_attempt_failed" not in journal_kinds(pool)
    assert_nothing_published(home)


def test_aborted_run_leaves_the_container_alive_until_docker_rm_removes_it_by_name(
    make_pool, canned, tmp_path
):
    # The container is a process of its own session, as on a real host: killing the local ssh (and
    # with it the docker client) does not stop it. Only `docker rm -f <its name>` does.
    pool = make_pool(labels=("a",), hosts={"a": {"runs": [{"mode": "hang", "detached": True}]}})
    pool.toolbox.scenario.set_global(rm_hold=True)  # hold the cleanup call to look in between
    home = canned.stage(tmp_path / "home")
    cancel, outcome = threading.Event(), []

    def work():
        try:
            dispatch(pool, home, cancel=cancel)
        except BaseException as error:  # noqa: BLE001 - inspected below
            outcome.append(error)

    worker = threading.Thread(target=work)
    worker.start()
    try:
        (container,) = fake.wait_for(lambda: pool.toolbox.events("container_start"))
        fake.wait_for(lambda: pool.toolbox.events("launch"))
        cancel.set()
        (cleanup,) = fake.wait_for(lambda: pool.toolbox.events("rm_start"))

        (run,) = pool.toolbox.runs()
        (ssh,) = pool.toolbox.ssh_calls("run")
        assert cleanup["name"] == run["name"]
        for gone in (ssh["pid"], run["pid"]):  # the local ssh and the docker client died with it
            fake.wait_for(lambda gone=gone: not pid_alive(gone), timeout=5)
        assert pool.toolbox.events("rm") == []  # nothing has removed the container yet ...
        assert pid_alive(container["pid"])  # ... and it still runs, in its own session
    finally:
        pool.toolbox.scenario.set_global(rm_hold=False)
        worker.join(timeout=30)

    fake.wait_for(lambda: not pid_alive(container["pid"]), timeout=10)
    assert [e["name"] for e in pool.toolbox.events("rm")] == [run["name"]]
    assert [type(error) for error in outcome] == [cuda_executor.Aborted]
    assert_nothing_published(home)


def test_cancel_while_waiting_for_a_busy_host_aborts_promptly(make_pool, canned, tmp_path):
    pool = make_pool(
        labels=("a",), config={"sweep_pause_seconds": 600}, hosts={"a": {"busy": uuids(0, 0)}}
    )
    home = canned.stage(tmp_path / "home")
    cancel = threading.Event()

    def revoke():
        fake.wait_for(lambda: pool.toolbox.ssh_calls("probe"))
        cancel.set()

    watcher = threading.Thread(target=revoke)
    watcher.start()
    started = time.monotonic()
    try:
        with pytest.raises(cuda_executor.Aborted, match="lease cancelled"):
            dispatch(pool, home, cancel=cancel)
    finally:
        watcher.join()

    assert time.monotonic() - started < 30  # not the 600 s sweep pause
    assert pool.toolbox.runs() == []
    assert_nothing_published(home)


def test_cancel_while_queued_behind_another_job_on_the_only_host_aborts(
    make_pool, canned, tmp_path
):
    pool = make_pool(labels=("a",), hosts={"a": {"runs": [{"mode": "hang"}]}})
    factory = cuda_executor.load(pool.config_path)
    cancels = [threading.Event(), threading.Event()]
    homes = [canned.stage(tmp_path / f"home-{i}") for i in range(2)]
    outcomes = {}

    def work(index):
        try:
            dispatch(pool, homes[index], factory=factory, cancel=cancels[index])
        except BaseException as error:  # noqa: BLE001 - inspected below
            outcomes[index] = error

    first = threading.Thread(target=work, args=(0,))
    first.start()
    fake.wait_for(lambda: pool.toolbox.events("launch"))
    second = threading.Thread(target=work, args=(1,))
    second.start()
    time.sleep(1)  # the queued job polls for the host every quarter second
    cancels[1].set()
    second.join(timeout=30)
    assert not second.is_alive() and isinstance(outcomes[1], cuda_executor.Aborted)
    assert len(pool.toolbox.runs()) == 1 and 0 not in outcomes  # the first job still holds the host
    cancels[0].set()
    first.join(timeout=30)

    assert isinstance(outcomes[0], cuda_executor.Aborted)
    assert [list(h.glob("published")) for h in homes] == [[], []]


def test_a_launch_queued_behind_another_job_aborts_when_its_own_deadline_passes(
    make_pool, canned, tmp_path, monkeypatch
):
    clock = fake.SteppedClock()
    monkeypatch.setattr(cuda_executor, "time", clock)
    pool = make_pool(labels=("a",), hosts={"a": {"runs": [{"mode": "hang"}]}})
    queued_cancel, holder_cancel, polls = threading.Event(), threading.Event(), []

    def sleep(seconds):
        # Only a launch waiting for the busy host sleeps. Its first poll moves the clock past its
        # deadline (not past the holder's); a second poll means that did not end the wait.
        polls.append(seconds)
        if len(polls) == 1:
            clock.offset += 2400
        else:
            queued_cancel.set()

    config = cuda_executor.ExecutorConfig.model_validate_json(pool.config_path.read_bytes())
    executor = cuda_executor.Executor(config, sleep=sleep)
    holder_errors = []

    def hold():
        try:
            home = canned.stage(tmp_path / "holder")
            executor.launch("audit", "bb" * 32, canned.job, home, trace=True, cancel=holder_cancel)
        except BaseException as error:  # noqa: BLE001 - inspected below
            holder_errors.append(error)

    holder = threading.Thread(target=hold)
    holder.start()
    queued = canned.stage(tmp_path / "queued")
    try:
        fake.wait_for(lambda: pool.toolbox.events("launch"))
        sooner = canned.job.model_copy(update={"deadline": canned.job.deadline - 1800})
        with pytest.raises(cuda_executor.Aborted, match="job deadline expired"):
            executor.launch("audit", "bb" * 32, sooner, queued, trace=True, cancel=queued_cancel)
    finally:
        holder_cancel.set()
        holder.join(timeout=30)

    assert len(polls) == 1 and len(pool.toolbox.runs()) == 1  # the queued job never got a host
    assert [type(error) for error in holder_errors] == [cuda_executor.Aborted]
    assert_nothing_published(queued)


def test_job_that_expires_between_the_run_and_the_publication_publishes_nothing(
    make_pool, canned, tmp_path, monkeypatch
):
    clock = fake.SteppedClock()
    monkeypatch.setattr(cuda_executor, "time", clock)
    validate = cuda_executor.validate_artifacts

    def validate_then_expire(job, directory):
        artifacts = validate(job, directory)
        clock.offset += 7200  # the staged result is valid; the lease is not
        return artifacts

    monkeypatch.setattr(cuda_executor, "validate_artifacts", validate_then_expire)
    pool = make_pool(labels=("a",))
    home = canned.stage(tmp_path / "home")
    with pytest.raises(cuda_executor.Aborted, match="expired before atomic publication"):
        dispatch(pool, home)

    assert len(pool.toolbox.runs()) == 1  # the result existed; it was not accepted
    assert "exec_published" not in journal_kinds(pool)
    assert_nothing_published(home)


class PublishSpy:
    """Stands in for the `os` of cuda_executor, and wraps its `durable_write`: records in call
    order what is fsynced, written and renamed, and what the staged tree held at the rename."""

    def __init__(self):
        self.events = []  # ("fsync", path) | ("write", file name) | ("rename", source)
        self.staged = []  # (path, is_dir) of the staging root and everything below it
        self.provenance_matches = []

    def fsync(self, fd):
        self.events.append(("fsync", os.readlink(f"/proc/self/fd/{fd}")))
        return os.fsync(fd)

    def rename(self, source, target):
        provenance = Path(target).parent / "role-dispatch.json"
        written = json.loads(provenance.read_bytes()) if provenance.is_file() else None
        self.provenance_matches.append(
            written is not None
            and written["publication_sha256"] == publication_digest(Path(source))
        )
        self.events.append(("rename", str(source)))
        tree = Path(source)
        self.staged = [(str(p), p.is_dir()) for p in [*sorted(tree.rglob("*")), tree]]
        return os.rename(source, target)

    def writing(self, real):
        def write(path, data, *args):
            self.events.append(("write", Path(path).name))
            return real(path, data, *args)

        return write

    def __getattr__(self, name):
        return getattr(os, name)


def test_staged_tree_is_durable_before_the_provenance_is_written_and_before_the_rename(
    make_pool, canned, tmp_path, monkeypatch
):
    spy = PublishSpy()
    monkeypatch.setattr(cuda_executor, "os", spy)
    monkeypatch.setattr(cuda_executor, "durable_write", spy.writing(cuda_executor.durable_write))
    pool = make_pool(labels=("a",))
    home = canned.stage(tmp_path / "home")
    dispatch(pool, home)

    # One rename, with the provenance on disk and vouching for exactly the staged bytes: a crash
    # after it still leaves a verifiable publication.
    assert spy.provenance_matches == [True]
    assert (home / "published").is_dir()
    kinds = [event[0] if event[0] != "write" else event[1] for event in spy.events]
    provenance_at, rename_at = kinds.index("role-dispatch.json"), kinds.index("rename")
    assert provenance_at < rename_at
    # Every file and every directory (the root included) is fsynced before the provenance exists.
    synced = {Path(path).resolve() for kind, path in spy.events[:provenance_at] if kind == "fsync"}
    staged = {Path(path).resolve() for path, _ in spy.staged}
    assert any(is_dir for _, is_dir in spy.staged) and len(staged) > 8
    assert staged <= synced
    # The directory that now holds both is fsynced after the rename, so the entry itself is durable.
    after = {Path(path).resolve() for kind, path in spy.events[rename_at + 1 :] if kind == "fsync"}
    assert home.resolve() in after


@pytest.mark.parametrize(
    ("entry", "cause"),
    [
        ("deadline-passed", "job deadline/cancellation"),
        ("already-cancelled", "job deadline/cancellation"),
        ("nine-seconds-left", "job deadline too close to start"),
    ],
)
def test_job_that_cannot_run_aborts_before_any_ssh_call(
    make_pool, canned, tmp_path, monkeypatch, entry, cause
):
    pool = make_pool(labels=("a",))
    home = canned.stage(tmp_path / "home")
    cancel = threading.Event()
    job = canned.job
    if entry == "deadline-passed":
        job = job.model_copy(update={"deadline": 1})
    elif entry == "already-cancelled":
        cancel.set()
    else:  # the executor's clock reads 9 s before the deadline: under the 10 s it needs to start
        clock = fake.SteppedClock()
        clock.offset = job.deadline - 9 - time.time()
        monkeypatch.setattr(cuda_executor, "time", clock)
    with pytest.raises(cuda_executor.Aborted, match=cause):
        dispatch(pool, home, job=job, cancel=cancel)

    assert pool.toolbox.ssh_log() == [] and journal_kinds(pool) == []
    assert_nothing_published(home)


def test_second_launch_returns_the_publication_without_touching_a_host(make_pool, canned, tmp_path):
    pool = make_pool(labels=("a",))
    home = canned.stage(tmp_path / "home")
    factory = cuda_executor.load(pool.config_path)
    first = dispatch(pool, home, factory=factory)
    record, calls = (home / "role-dispatch.json").read_bytes(), len(pool.toolbox.ssh_log())

    assert dispatch(pool, home, factory=factory) == first
    assert len(pool.toolbox.ssh_log()) == calls
    assert (home / "role-dispatch.json").read_bytes() == record


def _remove_provenance(home):
    (home / "role-dispatch.json").unlink()


def _symlink_provenance(home):
    record = home / "role-dispatch.json"
    copy = home / "copy.json"
    copy.write_bytes(record.read_bytes())
    record.unlink()
    record.symlink_to(copy)


def _edit_publication(home):
    with open(home / "published/rank-0/delta.bin", "ab") as delta:
        delta.write(b"x")


def _edit_provenance(field, value):
    def damage(home):
        record = record_of(home)
        record[field] = value
        (home / "role-dispatch.json").write_bytes(canonicalize(record))

    return damage


def _overwrite_provenance(raw):
    def damage(home):
        (home / "role-dispatch.json").write_bytes(raw)

    return damage


def _link_published_file(home):
    # The bytes are identical: only the link makes the publication unacceptable.
    published = home / "published/rank-0/delta.bin"
    copy = home / "delta.copy"
    copy.write_bytes(published.read_bytes())
    published.unlink()
    published.symlink_to(copy)


def _special_published_file(home):
    os.mkfifo(home / "published/rank-0/pipe")


@pytest.mark.parametrize(
    ("damage", "cause"),
    [
        pytest.param(_remove_provenance, "without executor provenance", id="missing"),
        pytest.param(_symlink_provenance, "without executor provenance", id="symlinked"),
        pytest.param(_edit_publication, "does not match the publication", id="edited"),
        pytest.param(
            _edit_provenance("job_sha256", "00" * 32),
            "does not match the publication",
            id="provenance-of-another-job",
        ),
        pytest.param(
            _edit_provenance("executor", "vast-lifecycle-v1"),
            "does not match the publication",
            id="provenance-of-another-executor",
        ),
        pytest.param(
            _overwrite_provenance(b"not json"),
            "executor provenance is unreadable",
            id="provenance-not-json",
        ),
        pytest.param(
            _overwrite_provenance(b"{}"),
            "executor provenance is unreadable",
            id="provenance-without-its-fields",
        ),
        pytest.param(
            _overwrite_provenance(b"[]"),
            "executor provenance is unreadable",
            id="provenance-not-an-object",
        ),
        pytest.param(_link_published_file, "link or a special file", id="published-file-linked"),
        pytest.param(_special_published_file, "link or a special file", id="published-fifo"),
    ],
)
def test_publication_without_matching_provenance_is_refused(
    make_pool, canned, tmp_path, damage, cause
):
    pool = make_pool(labels=("a",))
    home = canned.stage(tmp_path / "home")
    factory = cuda_executor.load(pool.config_path)
    dispatch(pool, home, factory=factory)
    calls = len(pool.toolbox.ssh_log())

    damage(home)
    with pytest.raises(IslandFailure, match=cause):
        dispatch(pool, home, factory=factory)
    assert len(pool.toolbox.ssh_log()) == calls


def test_provenance_that_cannot_be_read_fails_the_resume_as_an_island_failure(
    make_pool, canned, tmp_path
):
    pool = make_pool(labels=("a",))
    home = canned.stage(tmp_path / "home")
    factory = cuda_executor.load(pool.config_path)
    dispatch(pool, home, factory=factory)
    provenance = home / "role-dispatch.json"
    provenance.chmod(0)
    try:
        if os.access(provenance, os.R_OK):
            pytest.skip("this user reads files whatever their mode")
        with pytest.raises(IslandFailure, match="executor provenance is unreadable"):
            dispatch(pool, home, factory=factory)
    finally:
        provenance.chmod(0o600)


def reseal(home):
    """Make the provenance vouch for whatever `published/` holds now, to reach the later checks."""
    record = record_of(home)
    record["publication_sha256"] = publication_digest(home / "published")
    (home / "role-dispatch.json").write_bytes(canonicalize(record))


def test_dispute_window_traces_added_to_a_publication_do_not_void_it(make_pool, canned, tmp_path):
    pool = make_pool(labels=("a",))
    home = canned.stage(tmp_path / "home")
    factory = cuda_executor.load(pool.config_path)
    first = dispatch(pool, home, factory=factory)
    record, calls = (home / "role-dispatch.json").read_bytes(), len(pool.toolbox.ssh_log())

    for rank, window in ((0, "1"), (1, "2")):  # what launch_window writes after the fact
        (home / f"published/rank-{rank}/windows").mkdir()
        (home / f"published/rank-{rank}/windows/{window}.json").write_bytes(b"[]")
    assert dispatch(pool, home, factory=factory) == first
    (home / "published/rank-0/windows/1.json").write_bytes(b'["rewritten"]')
    assert dispatch(pool, home, factory=factory) == first
    assert len(pool.toolbox.ssh_log()) == calls
    assert (home / "role-dispatch.json").read_bytes() == record


@pytest.mark.parametrize(
    "relative",
    [
        pytest.param("rank-0/notes.json", id="beside-the-rank-files"),
        pytest.param("rank-0/extra/1.json", id="in-a-directory-that-is-not-windows"),
        pytest.param("rank-0/checkpoints/windows/1.json", id="windows-below-checkpoints"),
        pytest.param("windows/1.json", id="windows-at-the-top"),
        pytest.param("rank-0/windows", id="a-file-named-windows"),
    ],
)
def test_any_other_file_added_to_a_publication_voids_it(make_pool, canned, tmp_path, relative):
    pool = make_pool(labels=("a",))
    home = canned.stage(tmp_path / "home")
    factory = cuda_executor.load(pool.config_path)
    dispatch(pool, home, factory=factory)
    calls = len(pool.toolbox.ssh_log())

    (home / "published" / relative).parent.mkdir(parents=True, exist_ok=True)
    (home / "published" / relative).write_bytes(b"[]")
    with pytest.raises(IslandFailure, match="does not match the publication"):
        dispatch(pool, home, factory=factory)
    assert len(pool.toolbox.ssh_log()) == calls


def test_resume_refuses_a_publication_whose_ranks_say_cpu(make_pool, canned, tmp_path):
    pool = make_pool(labels=("a",))
    home = canned.stage(tmp_path / "home")
    factory = cuda_executor.load(pool.config_path)
    dispatch(pool, home, factory=factory)
    calls = len(pool.toolbox.ssh_log())

    for rank in range(2):  # both ranks agree, so only the executor's own CUDA check can refuse
        summary = home / f"published/rank-{rank}/summary.json"
        summary.write_bytes(canonicalize({**json.loads(summary.read_bytes()), "backend": "cpu"}))
    reseal(home)
    validate_artifacts(canned.job, home / "published")  # a valid CPU publication, not a CUDA one
    with pytest.raises(IslandFailure, match="not a usable CUDA one: rank summary is not a CUDA"):
        dispatch(pool, home, factory=factory)
    assert len(pool.toolbox.ssh_log()) == calls


def test_resume_asks_for_a_trace_only_when_the_launch_wants_one(make_pool, canned, tmp_path):
    pool = make_pool(labels=("a",))
    home = canned.stage(tmp_path / "home")
    factory = cuda_executor.load(pool.config_path)
    first = dispatch(pool, home, "reference", factory=factory)  # published without trace.json
    calls = len(pool.toolbox.ssh_log())

    assert not (home / "published/rank-0/trace.json").exists()
    assert dispatch(pool, home, "reference", factory=factory) == first
    with pytest.raises(IslandFailure, match="not a usable CUDA one: requested trace is absent"):
        dispatch(pool, home, "audit", factory=factory)
    assert len(pool.toolbox.ssh_log()) == calls


@pytest.mark.parametrize(
    "damage",
    [
        pytest.param(b'{"events": []}', id="json-but-not-a-list"),
        pytest.param(b"not json", id="not-json"),
        pytest.param(None, id="missing"),
    ],
)
def test_resume_refuses_a_missing_or_malformed_trace_when_one_is_requested(
    make_pool, canned, tmp_path, damage
):
    pool = make_pool(labels=("a",))
    home = canned.stage(tmp_path / "home")
    factory = cuda_executor.load(pool.config_path)
    first = dispatch(pool, home, "audit", factory=factory)
    trace = home / "published/rank-1/trace.json"
    assert trace.is_file()
    if damage is None:
        trace.unlink()
    else:
        trace.write_bytes(damage)
    reseal(home)
    calls = len(pool.toolbox.ssh_log())

    with pytest.raises(IslandFailure, match="not a usable CUDA one"):
        dispatch(pool, home, "audit", factory=factory)
    assert dispatch(pool, home, "reference", factory=factory) == first  # no trace asked for
    assert len(pool.toolbox.ssh_log()) == calls


@pytest.mark.parametrize("stale", ["file", "symlink", "dangling-symlink"])
def test_stale_provenance_without_a_publication_is_replaced(
    network, make_pool, canned, tmp_path, stale
):
    pool = make_pool(labels=("a",))
    home = canned.stage(tmp_path / "home")
    decoy = tmp_path / "decoy.json"
    if stale != "dangling-symlink":  # a link to nothing: only is_symlink() sees it
        decoy.write_bytes(b'{"decoy": true}')
    if stale == "file":
        (home / "role-dispatch.json").write_bytes(b'{"stale": true}')
    else:
        (home / "role-dispatch.json").symlink_to(decoy)
    dispatch(pool, home)

    provenance = home / "role-dispatch.json"
    assert provenance.is_file() and not provenance.is_symlink()
    assert record_of(home)["job_sha256"] == canned.job.digest()
    assert (decoy.read_bytes() if decoy.exists() else None) == (
        None if stale == "dangling-symlink" else b'{"decoy": true}'
    )
    network.store._dispatch_env_v2(canned.manifest, home)


def test_cuda_job_of_another_run_is_refused_before_any_ssh_call(make_pool, canned, tmp_path):
    pool = make_pool(labels=("a",))
    foreign = canned.job.model_copy(update={"run_id": "ef" * 32})
    with pytest.raises(RuntimeError, match="not the authorized run") as caught:
        dispatch(pool, canned.stage(tmp_path / "home"), job=foreign)

    assert not isinstance(caught.value, IslandFailure) and pool.toolbox.ssh_log() == []


@pytest.mark.parametrize("run_id", ["served", "foreign"])
def test_cpu_launch_runs_in_process_and_never_touches_a_host(
    make_pool, canned, tmp_path, monkeypatch, run_id
):
    pool = make_pool(labels=("a",))
    home = canned.stage(tmp_path / "home")
    job = canned.job if run_id == "served" else canned.job.model_copy(update={"run_id": "ef" * 32})
    calls, sentinel, cancel = [], object(), threading.Event()

    def launch_island(*args, **kwargs):
        calls.append((args, kwargs))
        return sentinel

    monkeypatch.setattr(cuda_executor, "launch_island", launch_island)
    launch = cuda_executor.load(pool.config_path)("reference", job, home, CONTEXTS["reference"])

    assert launch(job, home, backend="cpu", cancel=cancel, trace=True) is sentinel
    assert calls == [((job, home), {"backend": "cpu", "cancel": cancel, "trace": True})]
    assert pool.toolbox.ssh_log() == []


@pytest.mark.parametrize("operation", ["probe", "live", "window"])
def test_operation_the_executor_does_not_serve_is_refused_at_factory_time(
    make_pool, canned, tmp_path, operation
):
    pool = make_pool(labels=("a",))
    factory = cuda_executor.load(pool.config_path)
    with pytest.raises(RuntimeError, match="does not serve"):
        factory(operation, canned.job, canned.stage(tmp_path / "home"), CONTEXT)
    assert pool.toolbox.ssh_log() == []


@pytest.mark.parametrize("backend_name", ["cuda", "cpu"])
@pytest.mark.parametrize("differs", ["job", "directory"])
def test_launch_that_differs_from_the_accepted_descriptor_is_refused(
    make_pool, canned, tmp_path, differs, backend_name
):
    pool = make_pool(labels=("a",))
    home = canned.stage(tmp_path / "home")
    launch = cuda_executor.load(pool.config_path)("audit", canned.job, home, CONTEXT)
    job, directory = canned.job, home
    if differs == "job":
        job = job.model_copy(update={"w": job.w + 1})
    else:
        directory = canned.stage(tmp_path / "elsewhere")
    with pytest.raises(RuntimeError, match="differs from the accepted descriptor"):
        launch(job, directory, backend=backend_name, cancel=None, trace=True)
    assert pool.toolbox.ssh_log() == []


def run_together(pool, homes):
    """One executor, one thread per home, all released together."""
    factory = cuda_executor.load(pool.config_path)
    gate, errors = threading.Barrier(len(homes)), []

    def work(home):
        gate.wait()
        try:
            dispatch(pool, home, factory=factory)
        except BaseException as error:  # noqa: BLE001 - re-raised below in the test thread
            errors.append(error)

    threads = [threading.Thread(target=work, args=(h,)) for h in homes]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join()
    assert errors == []
    return sorted(pool.toolbox.runs(), key=lambda run: run["start"])


def test_concurrent_launches_for_one_directory_run_the_container_once(make_pool, canned, tmp_path):
    pool = make_pool(labels=("a",), hosts={"a": {"runs": [{"mode": "ok", "delay": 0.5}]}})
    home = canned.stage(tmp_path / "home")
    (run,) = run_together(pool, [home, home])

    assert record_of(home)["result_tar_sha256"] == run["stdout_sha256"]
    assert journal_kinds(pool) == ["exec_attempt", "exec_published"]


def test_one_host_runs_one_job_at_a_time(make_pool, canned, tmp_path):
    slow = {"mode": "ok", "delay": 0.8}
    pool = make_pool(labels=("a",), hosts={"a": {"runs": [slow, slow]}})
    homes = [canned.stage(tmp_path / f"home-{i}") for i in range(2)]
    first, second = run_together(pool, homes)

    assert first["end"] - first["start"] >= 0.8
    assert first["end"] <= second["start"]
    assert all((home / "published").is_dir() for home in homes)


def test_two_hosts_run_two_jobs_in_parallel(make_pool, canned, tmp_path):
    both_running = {"mode": "ok", "rendezvous": 2}  # neither finishes until both have started
    pool = make_pool(
        labels=("a", "b"), hosts={"a": {"runs": [both_running]}, "b": {"runs": [both_running]}}
    )
    homes = [canned.stage(tmp_path / f"home-{i}") for i in range(2)]
    first, second = run_together(pool, homes)

    assert first["host"] != second["host"]
    assert second["start"] < first["end"]
    hosts = [record_of(h)["host"] for h in homes]
    assert {host["label"] for host in hosts} == {"a", "b"}
    assert (
        len({host["machine_id"] for host in hosts})
        == len({host["instance_id"] for host in hosts})
        == 2
    )


@pytest.mark.parametrize("outcome", ["success", "failure"])
def test_no_secret_reaches_state_logs_publication_or_exception_text(
    make_pool, canned, tmp_path, monkeypatch, caplog, capsys, outcome
):
    env_secret = "canary-env-9d1f7c33"
    monkeypatch.setenv("HYPERTRAIN_CANARY_SECRET", env_secret)
    monkeypatch.setenv("HYPERTRAIN_BEARER_TOKEN", env_secret)
    hosts = {"a": {"runs": [OOM]}} if outcome == "failure" else {}
    pool = make_pool(labels=("a",), config={"max_attempts": 1}, hosts=hosts)
    home = canned.stage(tmp_path / "home")
    caplog.set_level(logging.DEBUG)
    texts = []
    try:
        dispatch(pool, home)
    except IslandFailure as error:
        texts += [str(error), repr(error)]
    assert (outcome == "failure") == bool(texts)

    needles = [fake.CANARY, env_secret, pool.identity.read_text()]
    files = [
        path
        for root in (pool.state_dir, home, pool.toolbox.root)
        for path in sorted(root.rglob("*"))
        if path.is_file() and not path.is_symlink()
    ]
    expected = {"journal.jsonl", "ssh.log", "docker.log", "scenario.json"}
    expected |= {"role-dispatch.json"} if outcome == "success" else {"ht-exec-"}
    assert all(any(e in str(path) for path in files) for e in expected)
    leaks = [
        str(path)
        for path in files
        if any(n.encode() in path.read_bytes() or n in path.name for n in needles)
    ]
    out, err = capsys.readouterr()
    texts += [caplog.text, out, err]
    assert leaks == [] and not [t for t in texts if any(n in t for n in needles)]


@contextlib.contextmanager
def umask(mask):
    previous = os.umask(mask)
    try:
        yield
    finally:
        os.umask(previous)


def test_files_the_ssh_transport_creates_are_private_whatever_the_umask(
    make_pool, canned, tmp_path
):
    pool = make_pool(labels=("a",))
    config = cuda_executor.ExecutorConfig.model_validate_json(pool.config_path.read_bytes())
    transport = cuda_executor.SshTransport(config.hosts[0], config, shutil.which("ssh"))
    out, err = tmp_path / "result.tar", tmp_path / "remote.stderr"
    with umask(0o022):
        code = transport.run(
            "date +%s",
            stdin=None,
            stdout=out,
            stderr=err,
            deadline=time.time() + 60,
            cancel=None,
            max_stdout_bytes=1 << 20,
        )

    assert code == 0 and out.read_text().strip().isdigit()  # it really ran, through the fake ssh
    assert [stat.S_IMODE(path.stat().st_mode) for path in (out, err)] == [0o600, 0o600]


def ssh_transport(pool):
    config = cuda_executor.ExecutorConfig.model_validate_json(pool.config_path.read_bytes())
    return cuda_executor.SshTransport(config.hosts[0], config, shutil.which("ssh"))


def run_remote(transport, command, stdout, stderr, cap=1 << 20):
    return transport.run(
        command,
        stdin=None,
        stdout=stdout,
        stderr=stderr,
        deadline=time.time() + 60,
        cancel=None,
        max_stdout_bytes=cap,
    )


def test_the_ssh_transport_never_reuses_a_result_file_and_appends_to_the_remote_log(
    make_pool, canned, tmp_path
):
    transport = ssh_transport(make_pool(labels=("a",)))
    out, err = tmp_path / "result.tar", tmp_path / "remote.stderr"
    out.write_bytes(b"bytes of an earlier attempt")
    err.write_text("the first command said: no such device\n")

    with pytest.raises(FileExistsError):
        run_remote(transport, "date +%s", out, err)
    assert out.read_bytes() == b"bytes of an earlier attempt"
    assert run_remote(transport, "echo and then this >&2", tmp_path / "next.out", err) == 0
    assert err.read_text() == "the first command said: no such device\nand then this\n"


@pytest.mark.parametrize(
    ("size", "fits"), [(4096, True), (4097, False)], ids=["at-the-cap", "over"]
)
def test_the_ssh_transport_cap_on_the_result_is_inclusive(make_pool, canned, tmp_path, size, fits):
    transport = ssh_transport(make_pool(labels=("a",)))
    out, err = tmp_path / "result.tar", tmp_path / "remote.stderr"
    command = f"head -c {size} /dev/zero"
    if fits:
        assert run_remote(transport, command, out, err, cap=4096) == 0
        assert out.stat().st_size == 4096
    else:
        with pytest.raises(IslandFailure, match="result exceeds max_result_bytes"):
            run_remote(transport, command, out, err, cap=4096)


def test_remote_stderr_logs_are_private_files_in_private_directories(make_pool, canned, tmp_path):
    pool = make_pool(labels=("a",), config={"max_attempts": 1}, hosts={"a": {"runs": [OOM]}})
    with umask(0o022), pytest.raises(IslandFailure, match="CUDA out of memory"):
        dispatch(pool, canned.stage(tmp_path / "home"))

    (log,) = (pool.state_dir / "logs").iterdir()
    assert log.name.startswith("ht-exec-") and "CUDA out of memory" in log.read_text()
    assert stat.S_IMODE(log.stat().st_mode) == 0o600
    assert [stat.S_IMODE(d.stat().st_mode) for d in (pool.state_dir, log.parent)] == [0o700] * 2


def test_gpus_held_by_others_do_not_change_the_identity_of_the_machine(make_pool, canned, tmp_path):
    pool = make_pool(labels=("a",), gpus=4)
    factory = cuda_executor.load(pool.config_path)
    first = canned.stage(tmp_path / "first")
    dispatch(pool, first, factory=factory)
    pool.toolbox.scenario.update(pool.address("a"), busy=uuids(0, 0, 1))
    second = canned.stage(tmp_path / "second")
    dispatch(pool, second, factory=factory)

    assert record_of(first)["environment"]["gpu_uuids"] == uuids(0, 0, 1)
    assert record_of(second)["environment"]["gpu_uuids"] == uuids(0, 2, 3)
    assert record_of(first)["host"] == record_of(second)["host"]


class BoundedRead:
    """A binary file whose `read()` must say how much it wants, and not too much."""

    def __init__(self, stream):
        self._stream = stream

    def read(self, size=-1):
        if size is None or size < 0 or size > 1 << 24:
            raise AssertionError("a whole file was read into memory")
        return self._stream.read(size)

    def __enter__(self):
        self._stream.__enter__()
        return self

    def __exit__(self, *exc):
        return self._stream.__exit__(*exc)

    def __getattr__(self, name):
        return getattr(self._stream, name)


def test_result_tar_and_published_files_are_hashed_in_chunks(
    make_pool, canned, tmp_path, monkeypatch
):
    reads, real = [], Path.read_bytes

    def read_bytes(self):
        if sys._getframe(1).f_globals["__name__"] == cuda_executor.__name__:
            reads.append(self)
        return real(self)

    def open_binary(path, mode="r", *args, **kwargs):
        stream = builtins.open(path, mode, *args, **kwargs)
        return BoundedRead(stream) if "r" in mode and "b" in mode else stream

    monkeypatch.setattr(Path, "read_bytes", read_bytes)
    monkeypatch.setattr(cuda_executor, "open", open_binary, raising=False)
    pool = make_pool(labels=("a",))
    home = canned.stage(tmp_path / "home")
    factory = cuda_executor.load(pool.config_path)
    dispatch(pool, home, factory=factory)  # receives, hashes and publishes
    dispatch(pool, home, factory=factory)  # resumes: hashes the publication again

    names = {(path.parent.name, path.name) for path in reads}
    assert ("rank-0", "summary.json") in names  # the spy does see this module's whole-file reads
    whole = [
        path
        for path in reads
        if path.name == "result.tar"
        or path.parent.name.startswith("rank-")
        and path.name not in ("summary.json", "trace.json")
    ]
    assert whole == []  # only small JSON, never the tar or a state, delta or checkpoint file


def test_a_record_the_store_could_not_read_back_is_never_published(
    make_pool, canned, tmp_path, monkeypatch
):
    real = cuda_executor.canonicalize

    def canonicalize(value, **kwargs):
        # The store reads the record with envelope_v2.load_json, which refuses a raw float. The
        # real record cannot hold one: every host-reported value is bounded before this point.
        # So the refusal is simulated here, to prove the executor asks before it publishes.
        if isinstance(value, dict) and value.get("executor") == cuda_executor.EXECUTOR:
            return b'{"executor": "cuda-executor-v1", "load": 0.5}'
        return real(value, **kwargs)

    monkeypatch.setattr(cuda_executor, "canonicalize", canonicalize)
    pool = make_pool(labels=("a",), config={"max_attempts": 1})
    home = canned.stage(tmp_path / "home")
    with pytest.raises(IslandFailure, match="gave up: a: result failed validation"):
        dispatch(pool, home)

    assert_nothing_published(home)
    assert "exec_published" not in journal_kinds(pool)


@pytest.mark.parametrize(
    ("value", "cause"), [(0.25, "float"), (2**53, "integer outside I-JSON safe range")]
)
def test_a_float_or_unsafe_integer_in_the_context_fails_when_the_launch_is_built(
    make_pool, canned, tmp_path, value, cause
):
    # The binding is body_digest(context), which refuses what a protocol preimage may not hold. The
    # store's contexts are JSON read back from signed envelopes and cannot contain a float.
    pool = make_pool(labels=("a",))
    factory = cuda_executor.load(pool.config_path)
    context = deepcopy(CONTEXTS["audit"])
    context["descriptor"]["claims"][0]["steps"][2] = value
    with pytest.raises(CanonicalizationError, match=cause):
        factory("audit", canned.job, canned.stage(tmp_path / "home"), context)

    assert pool.toolbox.ssh_log() == []
    context["descriptor"]["claims"][0]["steps"][2] = 3  # the same context without it is accepted
    assert callable(factory("audit", canned.job, canned.stage(tmp_path / "again"), context))


# -- compute cache (docs/adr/0012) ------------------------------------------------------------
# The fake docker checks a bind mount like the real daemon does (the source must exist) and models
# a persistent CUDA compute cache: a run that mounts /root/.nv is `cold` until some run has left a
# marker there, then `warm`. That models what S13 measured on a B300 host; it is not GPU evidence.

CACHE_ROOT = ".cortex-cuda-executor/compute-cache"


def cache_leaf(canned) -> str:
    return canned.manifest.training.reference_spec.image_digest.removeprefix("sha256:")[:32]


def run_starts(pool):
    return pool.toolbox.events("run_start")


def test_the_first_container_warms_a_cache_that_every_later_one_finds(make_pool, canned, tmp_path):
    pool = make_pool(labels=("a",))
    factory = cuda_executor.load(pool.config_path)
    for number in (1, 2, 3):
        dispatch(pool, canned.stage(tmp_path / f"home{number}"), factory=factory)

    runs = run_starts(pool)
    assert [r["cache"] for r in runs] == ["cold", "warm", "warm"]
    directory = pool.home("a") / CACHE_ROOT / cache_leaf(canned)
    assert {r["mounts"][0]["src"] for r in runs} == {str(directory)}  # one directory, three runs
    assert len(pool.toolbox.ssh_calls("prepare")) == 1  # made once, then remembered

    # A restarted challenge builds a new executor: it asks again and finds the same warm cache.
    dispatch(pool, canned.stage(tmp_path / "home4"))
    assert run_starts(pool)[-1]["cache"] == "warm"
    assert len(pool.toolbox.ssh_calls("prepare")) == 2


def test_without_the_cache_every_container_starts_cold_and_the_host_is_untouched(
    make_pool, canned, tmp_path
):
    pool = make_pool(labels=("a",), config={"compute_cache": False})
    factory = cuda_executor.load(pool.config_path)
    for number in (1, 2):
        dispatch(pool, canned.stage(tmp_path / f"home{number}"), factory=factory)

    assert [r["cache"] for r in run_starts(pool)] == ["none", "none"]
    assert all(r["mounts"] == [] for r in run_starts(pool))
    assert pool.toolbox.ssh_calls("prepare") == []
    assert list(pool.home("a").iterdir()) == []


def test_the_executor_writes_one_private_directory_on_the_host_and_nothing_else(
    make_pool, canned, tmp_path
):
    pool = make_pool(labels=("a",))
    dispatch(pool, canned.stage(tmp_path / "home"))

    home = pool.home("a")
    top = ".cortex-cuda-executor"
    base = f"{top}/compute-cache/{cache_leaf(canned)}"
    # The last two entries are what the (fake) driver left inside the mount.
    assert sorted(p.relative_to(home).as_posix() for p in home.rglob("*")) == [
        top,
        f"{top}/compute-cache",
        base,
        f"{base}/ComputeCache",
        f"{base}/ComputeCache/index",
    ]
    assert stat.S_IMODE((home / base).stat().st_mode) == 0o700
    # Not the workspace, not the state directory: the job's own files never reach the host.
    assert not list(pool.toolbox.root.joinpath("host-tmp").iterdir())


def test_a_named_root_replaces_the_home_for_that_host(make_pool, canned, tmp_path):
    pool = make_pool(labels=("a",))
    root = tmp_path / "nvme" / "ht-cache"
    data = json.loads(pool.config_path.read_text())
    data["hosts"][0]["compute_cache_dir"] = str(root)
    pool.config_path.write_text(json.dumps(data))
    dispatch(pool, canned.stage(tmp_path / "home"))

    (run,) = run_starts(pool)
    assert run["mounts"] == [
        {"type": "bind", "src": str(root / cache_leaf(canned)), "dst": "/root/.nv"}
    ]
    assert stat.S_IMODE((root / cache_leaf(canned)).stat().st_mode) == 0o700
    assert list(pool.home("a").iterdir()) == []


@pytest.mark.parametrize("enabled", [True, False])
def test_the_provenance_and_the_journal_say_whether_a_cache_served_the_result(
    make_pool, canned, tmp_path, enabled
):
    pool = make_pool(labels=("a",), config={"compute_cache": enabled})
    home = canned.stage(tmp_path / "home")
    dispatch(pool, home)

    assert record_of(home)["compute_cache"] is enabled
    (attempt,) = journal_of(pool, "exec_attempt")
    assert attempt["compute_cache"] is enabled


def test_a_cache_directory_removed_by_hand_costs_one_attempt_not_the_job(
    make_pool, canned, tmp_path
):
    pool = make_pool(labels=("a",))
    factory = cuda_executor.load(pool.config_path)
    dispatch(pool, canned.stage(tmp_path / "home1"), factory=factory)
    shutil.rmtree(pool.home("a") / CACHE_ROOT)

    home = canned.stage(tmp_path / "home2")
    dispatch(pool, home, factory=factory)  # remembered path is gone: docker refuses, we recreate

    (failed,) = journal_of(pool, "exec_attempt_failed")
    assert "bind source path does not exist" in failed["reason"]
    assert journal_kinds(pool).count("exec_published") == 2
    assert len(pool.toolbox.ssh_calls("prepare")) == 2
    assert [r["cache"] for r in run_starts(pool)] == ["cold", "cold"]  # a new, empty directory
    assert (home / "published").is_dir()


def test_a_host_that_cannot_make_the_directory_still_runs_the_job_without_the_cache(
    make_pool, canned, tmp_path
):
    pool = make_pool(labels=("a",))
    shutil.rmtree(pool.home("a"))
    pool.home("a").write_text("a file where the home directory should be")
    home = canned.stage(tmp_path / "home")

    dispatch(pool, home)  # a speed-up must not fail a job the host can run

    (run,) = run_starts(pool)
    assert run["mounts"] == [] and run["cache"] == "none"
    assert record_of(home)["compute_cache"] is False
    (attempt,) = journal_of(pool, "exec_attempt")
    assert attempt["compute_cache"] is False
    (why,) = journal_of(pool, "exec_cache_unavailable")
    assert why["label"] == "a" and "cannot prepare the compute cache directory" in why["reason"]
    assert journal_kinds(pool).count("exec_attempt_failed") == 0
    assert (home / "published").is_dir()


def test_a_link_in_place_of_the_cache_directory_is_refused_and_its_target_left_alone(
    make_pool, canned, tmp_path
):
    pool = make_pool(labels=("a",))
    target = tmp_path / "somewhere-else"
    target.mkdir(mode=0o755)
    link = pool.home("a") / CACHE_ROOT / cache_leaf(canned)
    link.parent.mkdir(parents=True)
    link.symlink_to(target)
    home = canned.stage(tmp_path / "home")

    dispatch(pool, home)

    assert run_starts(pool)[0]["mounts"] == []  # not mounted: the daemon would follow the link
    assert stat.S_IMODE(target.stat().st_mode) == 0o755 and list(target.iterdir()) == []
    assert "exec_cache_unavailable" in journal_kinds(pool)


def test_each_host_keeps_its_cache_in_its_own_home(make_pool, canned, tmp_path):
    labels = ("a", "b")
    pool = make_pool(labels=labels)
    first = first_host(canned.job, labels)
    other = labels[1 - labels.index(first)]
    dispatch(pool, canned.stage(tmp_path / "home1"))
    pool.toolbox.scenario.update(pool.address(first), down=True)
    dispatch(pool, canned.stage(tmp_path / "home2"))

    assert [r["mounts"][0]["src"] for r in run_starts(pool)] == [
        str(pool.home(first) / CACHE_ROOT / cache_leaf(canned)),
        str(pool.home(other) / CACHE_ROOT / cache_leaf(canned)),
    ]
    assert [r["cache"] for r in run_starts(pool)] == ["cold", "cold"]  # not the first host's cache


# -- compute cache: what the fake host promises --------------------------------------------


@pytest.mark.parametrize(
    ("delay", "slow_run"), [("cold_start", 0), ("warm_start", 1)], ids=["cold", "warm"]
)
def test_the_fake_host_delays_the_run_whose_cache_has_the_named_temperature(
    make_pool, canned, tmp_path, delay, slow_run
):
    pool = make_pool(labels=("a",), hosts={"a": {delay: 1.5}})
    factory = cuda_executor.load(pool.config_path)
    for number in (1, 2):
        dispatch(pool, canned.stage(tmp_path / f"home{number}"), factory=factory)

    runs = pool.toolbox.runs()
    assert [r["cache"] for r in runs] == ["cold", "warm"]
    # `start` is the container's own run_start; the delay is slept after it and before `end`.
    assert runs[slow_run]["end"] - runs[slow_run]["start"] >= 1.5


def test_the_fake_docker_refuses_the_cache_mounts_the_real_daemon_refuses(
    make_pool, canned, tmp_path
):
    pool = make_pool(labels=("a",))
    cache = tmp_path / "cache"
    cache.mkdir()
    name = "ht-exec-0123456789abcdef"
    command = cuda_executor.run_command(
        ("docker",), pool.image, name, "d" * 64, uuids(0, 0), "default", str(cache)
    )
    argv = shlex.split(command)
    argv[0] = str(pool.toolbox.host_bin / "docker")  # what the remote shell would find on its PATH
    at = argv.index("--mount")
    env = {**os.environ, "FAKE_HOST_ADDRESS": pool.address("a")}
    refused = [
        (("-v", f"{cache}:/root/.nv"), "supports --mount only"),
        (("--mount", f"type=volume,src={cache},dst=/root/.nv"), "invalid mount config"),
        (("--mount", "type=bind,dst=/root/.nv"), "invalid mount config"),
    ]
    for mount, message in refused:
        done = subprocess.run(
            [*argv[:at], *mount, *argv[at + 2 :]], input=b"", env=env, capture_output=True
        )
        assert done.returncode == 125 and message in done.stderr.decode(), mount
