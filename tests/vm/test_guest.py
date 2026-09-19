"""Real guest-adaptor subprocess tests in a temporary guest filesystem."""

import base64
import hashlib
import io
import tarfile
from pathlib import Path

import pytest

from cortex.rlm.models import VmAction, VmContext
from cortex.vm.guest import GuestExecutor, GuestIdentity, extract_verified
from cortex.vm.models import ExecuteRequest, VmError


@pytest.mark.parametrize("phase", ["preflight", "experiment"])
async def test_generated_inspector_does_not_shadow_stdlib_imports(tmp_path, phase):
    executor, _ = guest(tmp_path)
    script = (
        "import inspect,json,os\nfrom helper import value\n"
        "assert callable(inspect.signature)\n"
        "json.dump({'metrics':[{'name':'synthetic','value':value}],'flops_used':0},"
        "open(os.environ['PROOF_OUTPUT_DIR']+'/report.json','w'))\n"
    )
    stream = io.BytesIO()
    with tarfile.open(fileobj=stream, mode="w") as archive:
        for name, source in {
            "inspect.py": script,
            "run.py": script,
            "helper.py": "value=10",
        }.items():
            raw = source.encode()
            member = tarfile.TarInfo(name)
            member.size = len(raw)
            archive.addfile(member, io.BytesIO(raw))
    pack = stream.getvalue()
    request = job()
    request = request.model_copy(
        update={
            "pack_b64": base64.b64encode(pack).decode(),
            "params": {
                "baseline_runner": "generated-python",
                "experiment_pack_digest": hashlib.sha256(pack).hexdigest(),
            },
            "action": request.action.model_copy(update={"phase": phase}),
        }
    )

    result = await executor.execute(request)

    assert result.exit_code == 0
    assert result.measurement.metrics[0].value == 10


def guest(tmp_path):
    runners = tmp_path / "runners"
    runner = runners / "operator-runner"
    runner.mkdir(parents=True)
    executor = GuestExecutor(
        GuestIdentity("vm-one", "topic-a", "ab" * 32, "experiment"),
        workspace=tmp_path / "workspace",
        runners_dir=runners,
    )
    return executor, runner


def job(**changes):
    stream = io.BytesIO()
    with tarfile.open(fileobj=stream, mode="w") as archive:
        member = tarfile.TarInfo("operator-data.txt")
        member.size = 4
        archive.addfile(member, io.BytesIO(b"data"))
    pack = stream.getvalue()
    params = {
        "baseline_runner": "operator-runner",
        **changes.pop("params", {}),
        "experiment_pack_digest": hashlib.sha256(pack).hexdigest(),
    }
    return ExecuteRequest(
        execution_id="execution-a",
        context=VmContext(
            topic_id="topic-a", job_id="job-a", purpose="setup", image_digest="ab" * 32
        ),
        action=VmAction(operation="run", phase="experiment", argv=["ignored"]),
        pack_b64=base64.b64encode(pack).decode(),
        params=params,
        **changes,
    )


async def test_operator_report_supplies_actual_metrics_and_output_is_bounded(tmp_path):
    executor, runner = guest(tmp_path)
    (runner / "run").write_text(
        "#!/usr/bin/python3\nimport json,os\n"
        'print("x" * 100000)\n'
        'json.dump({"metrics":[{"name":"accuracy","value":0.75}],"flops_used":123},'
        'open(os.environ["PROOF_OUTPUT_DIR"]+"/report.json","w"))\n'
    )
    (runner / "run").chmod(0o755)

    result = await executor.execute(job())

    assert result.measurement.metrics[0].value == 0.75
    assert result.measurement.flops_used == 123
    assert len(result.stdout_tail.encode()) <= 16384
    assert result.exit_code == 0


async def test_successful_process_without_report_cannot_invent_a_score(tmp_path):
    executor, runner = guest(tmp_path)
    (runner / "run").write_text("#!/bin/sh\necho 'accuracy: 1.0'\n")
    (runner / "run").chmod(0o755)

    with pytest.raises(VmError, match="report.json required"):
        await executor.execute(job())


async def test_miner_secret_is_private_and_redacted(tmp_path):
    executor, runner = guest(tmp_path)
    (runner / "run").write_text(
        "#!/usr/bin/python3\nimport os,json,stat\n"
        'p=os.environ["PROOF_MINER_ENV_DIR"]+"/MINER_TOKEN"\n'
        "assert stat.S_IMODE(os.stat(p).st_mode)==0o600\n"
        "print(open(p).read())\n"
        'json.dump({"flops_used":0},open(os.environ["PROOF_OUTPUT_DIR"]+"/report.json","w"))\n'
    )
    (runner / "run").chmod(0o755)

    result = await executor.execute(
        job(
            params={"baseline_runner": "operator-runner", "miner_byok": "MINER_TOKEN"},
            env={"MINER_TOKEN": "private-miner-value"},
        )
    )

    assert "private-miner-value" not in result.stdout_tail
    assert "[redacted]" in result.stdout_tail


async def test_secret_crossing_the_rolling_tail_boundary_does_not_leak_suffix(tmp_path):
    executor, runner = guest(tmp_path)
    (runner / "run").write_text(
        "#!/usr/bin/python3\nimport os,json\n"
        "print(os.environ['MINER_TOKEN'] + 'x' * 16375)\n"
        "json.dump({'flops_used':0},open(os.environ['PROOF_OUTPUT_DIR']+'/report.json','w'))\n"
    )
    (runner / "run").chmod(0o755)
    result = await executor.execute(
        job(params={"miner_byok": "MINER_TOKEN"}, env={"MINER_TOKEN": "secret-value-SUFFIX"})
    )
    assert "SUFFIX" not in result.stdout_tail


def test_tar_traversal_never_writes_outside_artifact_directory(tmp_path):
    stream = io.BytesIO()
    with tarfile.open(fileobj=stream, mode="w") as archive:
        member = tarfile.TarInfo("../escape")
        member.size = 4
        archive.addfile(member, io.BytesIO(b"evil"))
    data = stream.getvalue()

    with pytest.raises(VmError, match="unsafe artifact"):
        extract_verified(data, hashlib.sha256(data).hexdigest(), tmp_path / "artifact")

    assert not (tmp_path / "escape").exists()


def test_host_process_has_no_guest_execution_identity():
    if "proof_vm=" in Path("/proc/cmdline").read_text():
        pytest.fail("CI must not run inside a live Proof guest")
    with pytest.raises(VmError, match="not running"):
        GuestIdentity.from_system()
