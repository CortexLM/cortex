"""In-container half of the generic CUDA executor (docs/adr/0008): stdin tar in, stdout tar out.

`serve` runs in this process with injected probe and launch; the island launcher, the device probe
and the shipped `python -I -c` entry run for real with a fake torch, a fake nvidia-smi and a fake
`Popen`. No GPU, ssh or docker."""

from __future__ import annotations

import ast
import hashlib
import io
import json
import os
import signal
import subprocess
import sys
import tarfile
import tempfile
import time
import uuid as uuidlib
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path
from types import SimpleNamespace
from typing import Any

import pytest

import hypertrain.trainer  # noqa: F401 - pins determinism before any test imports torch
from hypertrain.gpu_ops import cuda_executor as ce
from hypertrain.gpu_ops import cuda_executor_runner as runner
from hypertrain.gpu_ops import network_qualification as qualification
from hypertrain.miner.island_launch import launch_argv
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages_v2 import IslandJobV1

REG = tarfile.REGTYPE
TREE = Path(__file__).parents[2]
MISSING = object()
DIGEST = "sha256:" + "ab" * 32
BUDGET = 600


def gpu_uuid(index: int) -> str:
    return "GPU-" + str(uuidlib.UUID(int=((index + 1) << 120) | 0xABCDEF0123))


U0, U1 = gpu_uuid(0), gpu_uuid(1)


def sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


@pytest.fixture(autouse=True)
def process_state(tmp_path: Path, monkeypatch: pytest.MonkeyPatch):
    """`serve` takes SIGTERM, mutates os.environ and creates its work directory in tempdir."""
    scratch = tmp_path / "tmp"
    scratch.mkdir()
    monkeypatch.setattr(tempfile, "tempdir", str(scratch))
    term, environment = signal.getsignal(signal.SIGTERM), dict(os.environ)

    def not_installed(signum: int, frame: Any) -> None:
        raise AssertionError("serve did not install its own SIGTERM handler")

    signal.signal(signal.SIGTERM, not_installed)
    yield
    signal.signal(signal.SIGTERM, term if term is not None else signal.SIG_DFL)
    os.environ.clear()
    os.environ.update(environment)


def tar_of(entries: list[tuple[str, bytes, bytes]]) -> bytes:
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


def request_env() -> dict[str, str]:
    return {
        "CUBLAS_WORKSPACE_CONFIG": ":4096:8",
        "CUDA_DISABLE_PTX_JIT": "1",
        "OMP_NUM_THREADS": "1",
        "MKL_NUM_THREADS": "1",
        "PYTHONHASHSEED": "0",
        "NVIDIA_TF32_OVERRIDE": "0",
        "HT_IMAGE_DIGEST": DIGEST,
        "HT_ISLAND_TRACE": "0",
        "HT_GPU_UUIDS": U0,
        "CUDA_VISIBLE_DEVICES": U0,
    }


def request_bytes(files: dict[str, bytes], **over: Any) -> bytes:
    """request.json as the challenge describes `files`; a value of MISSING drops the key. The
    bytes are indented on purpose: COMPLETE must bind them as received, not re-serialized."""
    body: dict[str, Any] = {
        "version": 1,
        "n_gpus": 1,
        "uuids": [U0],
        "job_sha256": sha(files.get("job.json", b"")),
        "budget_seconds": BUDGET,
        "inputs": {name: sha(raw) for name, raw in files.items()},
        "env": request_env(),
    }
    for key, value in over.items():
        if value is MISSING:
            del body[key]
        else:
            body[key] = value
    return json.dumps(body, indent=1).encode()


def input_files() -> dict[str, bytes]:
    return {"job.json": b'{"job": 1}', "start_state": b"s" * 3000, "samples/part-0": b"nested"}


def package(files: dict[str, bytes], raw: bytes | None = None, extra: tuple = ()) -> bytes:
    request = request_bytes(files) if raw is None else raw
    members = [(name, payload, REG) for name, payload in files.items()]
    return tar_of([("request.json", request, REG), *members, *extra])


@dataclass
class Outcome:
    code: int
    stdout: bytes
    stderr: str


def run(stdin: bytes, **kwargs: Any) -> Outcome:
    out, err = io.BytesIO(), io.StringIO()
    code = runner.serve(io.BytesIO(stdin), out, err, **kwargs)
    leftovers = list(Path(tempfile.gettempdir()).iterdir())
    assert leftovers == [], "the work directory must not outlive the run"
    return Outcome(code, out.getvalue(), err.getvalue())


def probe_ok(request: dict[str, Any]) -> dict[str, Any]:
    return {"backend": "cuda", "gpu_uuids": list(request["uuids"]), "sm_counts": [148]}


def never(what: str) -> Callable[..., Any]:
    def fail(*args: Any, **kwargs: Any) -> Any:
        pytest.fail(f"{what} must not run")

    return fail


def write_ranks(request: dict[str, Any], attempt: Path, budget: float, log: Path) -> int:
    for rank in range(request["n_gpus"]):
        (attempt / f"rank-{rank}/checkpoints").mkdir(parents=True)
        (attempt / f"rank-{rank}/summary.json").write_bytes(f"summary {rank}".encode())
        (attempt / f"rank-{rank}/checkpoints/0.safetensors").write_bytes(b"c" * 4096)
    return 0


def read_tar(raw: bytes) -> list[tuple[str, bytes]]:
    with tarfile.open(fileobj=io.BytesIO(raw), mode="r:") as archive:
        members = list(archive)
        assert all(m.isreg() for m in members)
        return [(m.name, archive.extractfile(m).read()) for m in members]  # type: ignore[union-attr]


# -- serve: the contract with the challenge --------------------------------------------------


def test_serve_ships_the_rank_files_environment_and_a_closing_complete() -> None:
    files = input_files()
    raw = request_bytes(files, n_gpus=2, uuids=[U0, U1])
    seen: dict[str, Any] = {}

    def launch(request: dict[str, Any], attempt: Path, budget: float, log: Path) -> int:
        seen["inputs"] = {
            p.relative_to(attempt).as_posix(): p.read_bytes()
            for p in attempt.rglob("*")
            if p.is_file()
        }
        seen["budget"] = budget
        (attempt / "notes.tmp").write_bytes(b"not a rank file")
        return write_ranks(request, attempt, budget, log)

    def probe(request: dict[str, Any]) -> dict[str, Any]:
        return {"backend": "cuda", "gpu_uuids": [U0, U1], "sm_counts": [148, 148]}

    out = run(package(files, raw), probe=probe, launch=launch)
    assert (out.code, out.stderr) == (0, "")
    assert seen["inputs"] == files  # extracted where the island worker reads them, bytes intact
    assert BUDGET - 30 < seen["budget"] <= BUDGET
    members = read_tar(out.stdout)
    names = [name for name, _ in members]
    assert sorted(names[:-2]) == [
        "rank-0/checkpoints/0.safetensors",
        "rank-0/summary.json",
        "rank-1/checkpoints/0.safetensors",
        "rank-1/summary.json",
    ]
    assert names[-2:] == ["environment.json", "COMPLETE"]  # COMPLETE closes the stream
    body = dict(members)
    assert json.loads(body["environment.json"]) == probe({})
    assert json.loads(body["COMPLETE"]) == {
        "version": 1,
        "files": {name: sha(body[name]) for name in names[:-1]},
        "request_sha256": sha(raw),
    }


def entries_of(files: dict[str, bytes]) -> list[tuple[str, bytes, bytes]]:
    return [(name, payload, REG) for name, payload in files.items()]


def refusals() -> dict[str, bytes]:
    files = input_files()
    good_request = request_bytes(files)
    cases: dict[str, bytes] = {}

    def add(name: str, request: bytes | None, members: list | None = None) -> None:
        body = entries_of(files) if members is None else members
        head = [] if request is None else [("request.json", request, REG)]
        cases[name] = tar_of([*head, *body])

    for name, bad in {
        "traversal": "../../evil",
        "absolute": "/abs/evil",
        "backslash": "dir\\evil",
        "empty-component": "dir//evil",
        "dot-component": "./evil",
        "embedded-parent": "dir/../evil",
    }.items():
        add(
            f"name-{name}",
            request_bytes({**files, bad: b"x"}),
            [*entries_of(files), (bad, b"x", REG)],
        )
    for name, (kind, payload) in {
        "symlink": (tarfile.SYMTYPE, b"/etc/passwd"),
        "hardlink": (tarfile.LNKTYPE, b"job.json"),
        "directory": (tarfile.DIRTYPE, b""),
        "fifo": (tarfile.FIFOTYPE, b""),
        "device": (tarfile.CHRTYPE, b""),
    }.items():
        add(f"member-{name}", good_request, [*entries_of(files), ("extra", payload, kind)])
    add("member-duplicate", good_request, [*entries_of(files), ("job.json", b'{"job": 1}', REG)])
    add("no-request", None)
    add("request-not-json", b"{not json")
    add("request-null", b"null")
    add("request-list", b"[]")
    add("request-empty-object", b"{}")
    add("request-too-large", good_request + b" " * (1 << 20))
    add("inputs-declares-a-missing-file", request_bytes({**files, "absent": b"x"}))
    add(
        "inputs-omits-a-sent-file",
        request_bytes({k: v for k, v in files.items() if k != "start_state"}),
    )
    add("inputs-byte-altered", good_request, entries_of({**files, "start_state": b"t" * 3000}))
    add("version-2", request_bytes(files, version=2))
    add("version-string", request_bytes(files, version="1"))
    add("no-version", request_bytes(files, version=MISSING))
    for name in ("LD_PRELOAD", "PYTHONPATH", "PATH"):
        add(f"env-{name}", request_bytes(files, env={**request_env(), name: "/tmp/x"}))
    add("env-integer", request_bytes(files, env={**request_env(), "OMP_NUM_THREADS": 1}))
    add("env-null", request_bytes(files, env={**request_env(), "OMP_NUM_THREADS": None}))
    add("env-list", request_bytes(files, env=["HT_IMAGE_DIGEST"]))
    add("no-env", request_bytes(files, env=MISSING))
    for key in ("budget_seconds", "n_gpus"):
        for label, value in {"string": "1", "float": 1.5, "null": None, "missing": MISSING}.items():
            add(f"{key}-{label}", request_bytes(files, **{key: value}))
    whole = package(files)
    with tarfile.open(fileobj=io.BytesIO(whole)) as archive:
        inside = archive.getmember("start_state")
    cases["stdin-empty"] = b""
    cases["stdin-garbage"] = b"HTTP/1.1 400 Bad Request\r\n" * 100
    cases["stdin-cut-in-a-header"] = whole[: inside.offset - 100]
    cases["stdin-cut-in-a-member"] = whole[: inside.offset_data + 1000]
    return cases


REFUSALS = refusals()


@pytest.mark.parametrize("stdin", REFUSALS.values(), ids=list(REFUSALS))
def test_serve_refuses_any_input_that_the_request_does_not_describe_exactly(
    stdin: bytes, tmp_path: Path
) -> None:
    out = run(stdin, probe=never("the probe"), launch=never("the island"))
    assert out.code == 2
    assert out.stdout == b""
    assert out.stderr.startswith("cuda-executor-runner: ")
    assert list(tmp_path.rglob("evil")) == []  # nothing was written outside the work directory


@pytest.mark.parametrize(("extra", "code"), [(0, 0), (1, 2)], ids=["at-the-limit", "over"])
def test_serve_bounds_request_json_inclusively(extra: int, code: int) -> None:
    files = input_files()
    request = request_bytes(files).ljust(runner.REQUEST_LIMIT + extra)  # trailing blanks are JSON
    out = run(package(files, request), probe=probe_ok, launch=write_ranks)
    assert out.code == code


@pytest.mark.parametrize(
    ("uuids", "n_gpus", "seen"),
    [
        pytest.param([U0], 1, [U1], id="other-device"),
        pytest.param([U0, U1], 2, [U0], id="fewer-devices"),
        pytest.param([U0, U1], 2, [U1, U0], id="other-order"),
        pytest.param([U0], 1, [U0, U1], id="more-devices"),
        pytest.param([U0], 1, [], id="no-device"),
        pytest.param([U0, U1], 1, [U0, U1], id="request-count-differs-from-n-gpus"),
        pytest.param(MISSING, 1, [U0], id="request-without-uuids"),
        pytest.param(MISSING, 1, None, id="request-without-uuids-and-a-probe-without-any"),
        pytest.param(U0, 1, [U0], id="request-uuids-not-a-list"),
    ],
)
def test_serve_exits_4_when_the_container_sees_other_devices_and_never_launches(
    uuids: Any, n_gpus: int, seen: list[str] | None
) -> None:
    files = input_files()
    stdin = package(files, request_bytes(files, uuids=uuids, n_gpus=n_gpus))
    out = run(stdin, probe=lambda request: {"gpu_uuids": seen}, launch=never("the island"))
    assert out.code == 4
    assert out.stdout == b""
    assert "differ from the requested devices" in out.stderr


@pytest.mark.parametrize(
    "plant",
    [
        pytest.param(lambda rank: (rank / "link").symlink_to("/etc/passwd"), id="symlink"),
        pytest.param(lambda rank: os.mkfifo(rank / "pipe"), id="fifo"),
        pytest.param(
            lambda rank: (rank / "inner").mkdir() or (rank / "inner/l").symlink_to("/"),
            id="nested-symlink",
        ),
    ],
)
def test_serve_exits_5_and_writes_no_byte_when_the_island_leaves_a_link_or_special_file(
    plant: Callable[[Path], object],
) -> None:
    def launch(request: dict[str, Any], attempt: Path, budget: float, log: Path) -> int:
        write_ranks(request, attempt, budget, log)
        plant(attempt / "rank-0")
        return 0

    files = input_files()
    out = run(package(files), probe=probe_ok, launch=launch)
    assert (out.code, out.stdout) == (5, b"")  # a partial tar would look like a cut stream
    assert "link or a special file" in out.stderr


@pytest.mark.parametrize(
    "layout",
    [
        pytest.param(lambda attempt: None, id="nothing"),
        pytest.param(lambda attempt: (attempt / "rank-0").mkdir(), id="empty-rank"),
        pytest.param(lambda attempt: (attempt / "stray.bin").write_bytes(b"x"), id="no-rank-dir"),
        pytest.param(
            lambda attempt: (
                (attempt / "elsewhere").mkdir(),
                (attempt / "elsewhere/f").write_bytes(b"x"),
                (attempt / "rank-0").symlink_to(attempt / "elsewhere"),
            ),
            id="rank-dir-is-a-symlink",
        ),
    ],
)
def test_serve_exits_5_when_the_island_produced_no_rank_files(
    layout: Callable[[Path], object],
) -> None:
    def launch(request: dict[str, Any], attempt: Path, budget: float, log: Path) -> int:
        layout(attempt)
        return 0

    out = run(package(input_files()), probe=probe_ok, launch=launch)
    assert (out.code, out.stdout) == (5, b"")
    assert "no rank files" in out.stderr


class Gone(io.BytesIO):
    """A stdout whose reader went away (ssh dropped), noticed on write or on the final flush."""

    def __init__(self, when: str) -> None:
        super().__init__()
        self.when = when

    def write(self, data: Any) -> int:
        if self.when == "write":
            raise BrokenPipeError(32, "Broken pipe")
        return super().write(data)

    def flush(self) -> None:
        if self.when == "flush":
            raise BrokenPipeError(32, "Broken pipe")


@pytest.mark.parametrize("when", ["write", "flush"])
def test_serve_exits_8_when_the_reader_is_gone(when: str) -> None:
    stdin = io.BytesIO(package(input_files()))
    err = io.StringIO()
    assert runner.serve(stdin, Gone(when), err, probe=probe_ok, launch=write_ranks) == 8


def test_serve_gives_the_island_the_budget_minus_the_time_already_spent(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    ticks = iter([1000.0 + 100.0 * i for i in range(50)])
    monkeypatch.setattr(runner.time, "monotonic", lambda: next(ticks))
    seen: list[float] = []

    def launch(request: dict[str, Any], attempt: Path, budget: float, log: Path) -> int:
        seen.append(budget)
        return write_ranks(request, attempt, budget, log)

    assert run(package(input_files()), probe=probe_ok, launch=launch).code == 0
    assert BUDGET - 300 < seen[0] <= BUDGET - 100  # probe and input transfer eat the wall budget


# -- torchrun: the island process -------------------------------------------------------------


def island_job(n: int) -> IslandJobV1:
    from tests.miner.test_island_launch_v2 import tiny_manifest

    manifest = tiny_manifest(n)
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
        deadline=2**31,
    )


def fake_island(
    monkeypatch: pytest.MonkeyPatch, on_wait: Callable[[Any, float], int], gone: bool = False
) -> SimpleNamespace:
    """Replace Popen and killpg: nothing real is started or signalled. `gone`: the island group
    has already exited, so killpg answers ESRCH."""
    seen = SimpleNamespace(procs=[], kills=[])

    def killpg(pid: int, sig: int) -> None:
        seen.kills.append((pid, sig))
        if gone:
            raise ProcessLookupError(3, "No such process")

    class Proc:
        pid = 4242

        def __init__(self, argv: list[str], **kwargs: Any) -> None:
            self.argv, self.kwargs, self.waits = argv, kwargs, []
            seen.procs.append(self)

        def wait(self, timeout: float | None = None) -> int:
            self.waits.append(timeout)
            return -9 if timeout is None else on_wait(self, timeout)

    monkeypatch.setattr(runner.subprocess, "Popen", Proc)
    monkeypatch.setattr(runner.os, "killpg", killpg)
    return seen


def island_stdin(n: int) -> tuple[bytes, list[str]]:
    files = input_files()
    uuids = [gpu_uuid(i) for i in range(n)]
    return package(files, request_bytes(files, n_gpus=n, uuids=uuids)), uuids


@pytest.mark.parametrize("gone", [False, True], ids=["group-alive", "group-already-gone"])
def test_torchrun_launches_the_challenge_argv_with_the_request_env_in_its_own_session(
    monkeypatch: pytest.MonkeyPatch, gone: bool
) -> None:
    def finish(proc: Any, timeout: float) -> int:
        attempt = Path(proc.argv[-2])
        assert (attempt / "job.json").read_bytes() == input_files()["job.json"]
        write_ranks({"n_gpus": 2}, attempt, timeout, attempt / "log")
        return 0

    seen = fake_island(monkeypatch, finish, gone)
    stdin, uuids = island_stdin(2)
    out = run(stdin, probe=lambda request: {"gpu_uuids": uuids})
    assert (out.code, out.stderr) == (0, "")
    (proc,) = seen.procs
    # The runner cannot import the challenge's hypertrain; this pins its copy of the argv to it.
    assert proc.argv == launch_argv(island_job(2), Path(proc.argv[-2]), "cuda")
    assert proc.kwargs["start_new_session"] is True
    assert {k: proc.kwargs["env"][k] for k in request_env()} == request_env()
    assert BUDGET - 30 < proc.waits[0] <= BUDGET
    assert seen.kills == [(4242, signal.SIGKILL)]  # stragglers of the island group die as well
    assert proc.waits[-1] is None  # and the group leader is reaped


@pytest.mark.parametrize(
    ("budget", "waited"),
    [(0.2, 1.0), (-30.0, 1.0), (42.5, 42.5)],
    ids=["under-1s", "spent", "ample"],
)
def test_torchrun_gives_the_island_at_least_a_second_whatever_budget_is_left(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path, budget: float, waited: float
) -> None:
    seen = fake_island(monkeypatch, lambda proc, timeout: 0)
    assert runner.torchrun({"n_gpus": 1}, tmp_path, budget, tmp_path / "island.log") == 0
    assert seen.procs[0].waits[0] == waited


def test_serve_exits_3_with_the_tail_of_the_island_log(monkeypatch: pytest.MonkeyPatch) -> None:
    def crash(proc: Any, timeout: float) -> int:
        proc.kwargs["stdout"].write(b"x" * 9000 + b"\nRuntimeError: driver refused by the worker\n")
        return 1

    seen = fake_island(monkeypatch, crash)
    stdin, uuids = island_stdin(1)
    out = run(stdin, probe=lambda request: {"gpu_uuids": uuids})
    assert (out.code, out.stdout) == (3, b"")
    assert "island exited 1" in out.stderr
    assert "RuntimeError: driver refused by the worker" in out.stderr
    assert 3000 < out.stderr.count("x") <= 4000  # the tail, bounded, not the whole log
    assert seen.kills == [(4242, signal.SIGKILL)]


def sigterm(proc: Any, timeout: float) -> int:
    os.kill(os.getpid(), signal.SIGTERM)
    time.sleep(5)  # the handler raises here; a lost signal ends in the wrong exit code instead
    return 0


def too_slow(proc: Any, timeout: float) -> int:
    raise subprocess.TimeoutExpired(proc.argv, timeout)


@pytest.mark.parametrize(
    ("on_wait", "code", "message"),
    [
        pytest.param(too_slow, 6, "wall budget", id="budget-exhausted"),
        pytest.param(sigterm, 7, "terminated", id="sigterm"),
    ],
)
def test_serve_kills_the_whole_island_group_when_the_budget_ends_or_docker_stops_it(
    monkeypatch: pytest.MonkeyPatch, on_wait: Callable[[Any, float], int], code: int, message: str
) -> None:
    seen = fake_island(monkeypatch, on_wait)
    stdin, uuids = island_stdin(1)
    out = run(stdin, probe=lambda request: {"gpu_uuids": uuids})
    assert (out.code, out.stdout) == (code, b"")
    assert message in out.stderr
    assert seen.kills == [(4242, signal.SIGKILL)]
    assert seen.procs[0].waits[-1] is None


# -- the device probe -------------------------------------------------------------------------

FAKE_TORCH = """
import types

__version__ = "2.14.0+cu130"
version = types.SimpleNamespace(cuda="13.0")


class _Uuid:
    def __init__(self, text):
        self.text = text

    def __str__(self):
        return self.text


def _device(uuid, name):
    return types.SimpleNamespace(
        uuid=_Uuid(uuid), name=name, multi_processor_count=148, major=10, minor=3
    )


_DEVICES = [
    _device("%(u0)s", "NVIDIA B300 SXM6 AC"),
    _device("GPU-%(u1_upper)s", "NVIDIA B300 SXM6 AC 2"),
]
cuda = types.SimpleNamespace(
    device_count=lambda: len(_DEVICES), get_device_properties=lambda i: _DEVICES[i]
)
"""

FAKE_SMI = """#!/bin/sh
[ "$1" = "-i" ] && [ "$3" = "--query-gpu=driver_version" ] || exit 64
[ "$4" = "--format=csv,noheader" ] || exit 64
case "$2" in
  %(u0)s) echo 580.95.05 ;;
  %(u1)s) echo 575.64.03 ;;
  *) echo "no such device $2" >&2; exit 65 ;;
esac
"""


def install_fake_devices(tmp_path: Path, monkeypatch: pytest.MonkeyPatch, smi: str) -> Path:
    site, bin_dir = tmp_path / "site", tmp_path / "bin"
    (site / "torch").mkdir(parents=True)
    bin_dir.mkdir()
    names = {"u0": U0[4:], "u1_upper": U1[4:].upper()}
    (site / "torch/__init__.py").write_text(FAKE_TORCH % names)
    smi_path = bin_dir / "nvidia-smi"
    smi_path.write_text(smi)
    smi_path.chmod(0o755)
    linked = tmp_path / "site-link"
    linked.symlink_to(site)
    monkeypatch.setenv("PYTHONPATH", str(linked))
    monkeypatch.setenv("PATH", f"{bin_dir}{os.pathsep}{os.environ['PATH']}")
    return site / "torch/__init__.py"


def test_probe_reports_normalized_uuids_and_each_devices_own_driver(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    torch_file = install_fake_devices(tmp_path, monkeypatch, FAKE_SMI % {"u0": U0, "u1": U1})
    observed = runner.probe_environment({})
    assert observed == {
        "backend": "cuda",
        "python": sys.executable,
        "torch": "2.14.0+cu130",
        "torch_path": str(torch_file.resolve()),
        "cuda": "13.0",
        # torch prints a bare lower-case UUID for one device, an upper-case GPU- one for the other
        "gpu_uuids": [U0, U1],
        "gpu_names": ["NVIDIA B300 SXM6 AC", "NVIDIA B300 SXM6 AC 2"],
        "sm_counts": [148, 148],
        "capabilities": [[10, 3], [10, 3]],
        "drivers": ["580.95.05", "575.64.03"],
    }


def test_probe_refuses_with_exit_9_when_nvidia_smi_fails(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # 9 is "the probe could not run"; 4 is "it ran and saw other devices" and quarantines the host.
    install_fake_devices(tmp_path, monkeypatch, "#!/bin/sh\necho 'NVML mismatch' >&2\nexit 9\n")
    with pytest.raises(runner.Refused) as refusal:
        runner.probe_environment({})
    assert refusal.value.code == 9
    assert "environment probe failed" in str(refusal.value)
    assert "CalledProcessError" in str(refusal.value)

    out = run(package(input_files()), launch=never("the island"))  # serve with the real probe
    assert (out.code, out.stdout) == (9, b"")
    assert "environment probe failed" in out.stderr


@pytest.mark.parametrize(
    ("outcome", "label"),
    [
        (subprocess.TimeoutExpired(["probe"], 300), "TimeoutExpired"),
        (SimpleNamespace(returncode=0, stdout="", stderr=""), "IndexError"),
        (SimpleNamespace(returncode=0, stdout="not json\n", stderr=""), "JSONDecodeError"),
    ],
    ids=["timeout", "no-output", "not-json"],
)
def test_probe_that_gives_no_answer_is_exit_9_not_a_traceback(
    monkeypatch: pytest.MonkeyPatch, outcome: Any, label: str
) -> None:
    def fake_run(*args: Any, **kwargs: Any) -> Any:
        if isinstance(outcome, Exception):
            raise outcome
        return outcome

    monkeypatch.setattr(runner.subprocess, "run", fake_run)
    with pytest.raises(runner.Refused) as refusal:
        runner.probe_environment({})
    assert refusal.value.code == 9
    assert f"environment probe gave no answer: {label}" in str(refusal.value)


@pytest.mark.parametrize("stdout", ["[1, 2]\n", "null\n", '"cuda"\n'], ids=["list", "null", "text"])
def test_probe_that_answers_with_something_other_than_an_object_is_exit_9(
    monkeypatch: pytest.MonkeyPatch, stdout: str
) -> None:
    monkeypatch.setattr(
        runner.subprocess,
        "run",
        lambda *args, **kwargs: SimpleNamespace(returncode=0, stdout=stdout, stderr=""),
    )
    with pytest.raises(runner.Refused) as refusal:
        runner.probe_environment({})
    assert refusal.value.code == 9
    assert "environment probe answered a non-object" in str(refusal.value)


def test_probe_takes_its_answer_from_the_last_line_whatever_torch_printed_before(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    printed = "UserWarning: a warning torch wrote to stdout on import\n" + '{"backend": "cuda"}\n'
    monkeypatch.setattr(
        runner.subprocess,
        "run",
        lambda *args, **kwargs: SimpleNamespace(returncode=0, stdout=printed, stderr=""),
    )
    assert runner.probe_environment({}) == {"backend": "cuda"}


# -- the sources of the installed image -----------------------------------------------------------


def package_files_of_the_tree() -> dict[str, str]:
    """The `src/hypertrain/**/*.py` entries of the qualification's own map of a tree."""
    return {
        name: digest
        for name, digest in qualification.sources(TREE).items()
        if name.startswith("src/hypertrain/") and name.endswith(".py")
    }


def test_image_sources_digest_is_the_jcs_digest_of_the_package_files_of_sources() -> None:
    # Given: the installed package (an editable install of this tree) and the tree's own map.
    expected = package_files_of_the_tree()
    # When
    measured = runner.measure_image_sources()
    # Then: one map, and one digest that the challenge side can compute from `sources()` alone.
    assert measured == expected
    assert runner.sources_digest(measured) == sha(canonicalize(expected))


def test_measure_ignores_pythonpath_and_the_working_directory(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # Given: a package named hypertrain in the working directory and on PYTHONPATH.
    decoy = tmp_path / "decoy"
    (decoy / "hypertrain").mkdir(parents=True)
    (decoy / "hypertrain/__init__.py").write_text("DECOY = True\n")
    (decoy / "hypertrain/stolen.py").write_text("x = 1\n")
    monkeypatch.setenv("PYTHONPATH", str(decoy))
    monkeypatch.chdir(decoy)
    # When
    measured = runner.measure_image_sources()
    # Then: the installed package was measured, not the one a caller put in front of it.
    assert measured == package_files_of_the_tree()
    assert "src/hypertrain/stolen.py" not in measured


def test_image_sources_program_imports_only_stdlib() -> None:
    # The program runs in the image's own interpreter before anything of hypertrain is trusted.
    nodes = list(ast.walk(ast.parse(runner.IMAGE_SOURCES)))
    imported = {a.name.split(".")[0] for n in nodes if isinstance(n, ast.Import) for a in n.names}
    imported |= {
        n.module.split(".")[0] for n in nodes if isinstance(n, ast.ImportFrom) and n.module
    }
    assert imported and imported <= sys.stdlib_module_names


def measure_fake_package(directory: Path, name: str, content: bytes = b"x = 1\n") -> Any:
    """Run the measuring program against a hypertrain package built in `directory`."""
    (directory / "hypertrain").mkdir(exist_ok=True)
    (directory / "hypertrain/__init__.py").write_bytes(b"")
    target = directory / "hypertrain" / name
    if name == "link.py":
        target.symlink_to(directory / "hypertrain/__init__.py")
    else:
        target.write_bytes(content)
    code = f"import sys; sys.path.insert(0, {str(directory)!r})\n{runner.IMAGE_SOURCES}"
    return subprocess.run(
        [sys.executable, "-I", "-c", code], capture_output=True, text=True, timeout=60, check=False
    )


@pytest.mark.parametrize("name", ["link.py", "caf\u00e9.py"], ids=["symlink", "non-ascii"])
def test_image_sources_program_refuses_a_link_and_a_non_ascii_name(
    tmp_path: Path, name: str
) -> None:
    done = measure_fake_package(tmp_path, name)
    assert done.returncode != 0 and done.stdout == ""
    assert "refused" in done.stderr


def test_image_sources_program_lists_a_plain_package_in_sorted_compact_json(
    tmp_path: Path,
) -> None:
    done = measure_fake_package(tmp_path, "b.py", b"y = 2\n")
    assert done.returncode == 0, done.stderr
    assert done.stdout == (
        "{"
        f'"src/hypertrain/__init__.py":"{sha(b"")}",'
        f'"src/hypertrain/b.py":"{sha(b"y = 2" + chr(10).encode())}"'
        "}\n"
    )


GOOD_KEY = b'"src/hypertrain/x.py"'


@pytest.mark.parametrize(
    "outcome,why",
    [
        (SimpleNamespace(returncode=1, stdout=b"", stderr=b"boom"), "could not be measured"),
        (SimpleNamespace(returncode=0, stdout=b"not json", stderr=b""), "are not a package map"),
        (SimpleNamespace(returncode=0, stdout=b"[]", stderr=b""), "are not a package map"),
        (SimpleNamespace(returncode=0, stdout=b"{}", stderr=b""), "are not a package map"),
        (
            SimpleNamespace(
                returncode=0, stdout=b'{"other/x.py":"' + b"a" * 64 + b'"}', stderr=b""
            ),
            "are not a package map",
        ),
        (
            SimpleNamespace(returncode=0, stdout=b"{" + GOOD_KEY + b':"abc"}', stderr=b""),
            "are not a package map",
        ),
        (SimpleNamespace(returncode=0, stdout=b" " * ((1 << 20) + 1), stderr=b""), "are too large"),
        (subprocess.TimeoutExpired(["measure"], 60), "could not be measured"),
    ],
    ids=[
        "failed",
        "not-json",
        "list",
        "empty",
        "foreign-key",
        "short-digest",
        "oversized",
        "timeout",
    ],
)
def test_measure_refuses_an_answer_that_is_not_a_map_of_package_files_with_exit_9(
    monkeypatch: pytest.MonkeyPatch, outcome: Any, why: str
) -> None:
    def fake_run(*args: Any, **kwargs: Any) -> Any:
        if isinstance(outcome, Exception):
            raise outcome
        return outcome

    monkeypatch.setattr(runner.subprocess, "run", fake_run)
    with pytest.raises(runner.Refused) as refusal:
        runner.measure_image_sources()
    assert refusal.value.code == 9
    assert f"image sources {why}" in str(refusal.value)


def test_shipped_source_runs_under_isolated_python_and_refuses_a_host_without_cuda(
    tmp_path: Path,
) -> None:
    # What `docker run ... python -I -c <RUNNER_SOURCE>` executes; torch here has no CUDA device.
    done = subprocess.run(
        [sys.executable, "-I", "-c", ce.RUNNER_SOURCE],
        input=package(input_files()),
        capture_output=True,
        timeout=180,
        cwd=tmp_path,
        env={**os.environ, "TMPDIR": str(tmp_path)},
        check=False,
    )
    assert done.returncode == 4
    assert done.stdout == b""
    assert b"cuda-executor-runner: visible GPUs differ from the requested devices" in done.stderr
