"""A dynamically authored environment must run unchanged in a fresh isolated guest."""

import base64
import hashlib
import json

import pytest

from cortex.rlm.models import VmAction, VmContext
from cortex.vm.guest import GuestExecutor, GuestIdentity
from cortex.vm.models import ExecuteRequest, VmError
from cortex.vm.setup import SetupExport, SetupManifest, export_setup


async def test_guest_setup_exports_real_code_and_private_holdout_then_measures_baseline(tmp_path):
    topic = GuestExecutor(
        GuestIdentity("topic-vm", "topic-a", "ab" * 32, "topic"), workspace=tmp_path / "topic"
    )
    holdout = b"operator-private-data"
    manifest = {
        "content_hashes": [hashlib.sha256(holdout).hexdigest()],
        "dataset_ids": ["operator-dataset"],
        "flops_budget": 1000,
        "wall_budget_s": 30,
    }
    runner = (
        "import json,os\n"
        "from pathlib import Path\n"
        "assert (Path(os.environ['PROOF_PACK_DIR'])/'holdout').read_bytes() == "
        f"{holdout!r}\n"
        "Path(os.environ['PROOF_OUTPUT_DIR']).joinpath('report.json').write_text("
        "json.dumps({'metrics':[{'name':'quality','value':0.75}], 'flops_used':123}))\n"
    )
    command = (
        "import json,os\nfrom pathlib import Path\n"
        "p=Path(os.environ['PROOF_SETUP_DIR'])\n"
        f"(p/'run.py').write_text({runner!r})\n"
        "(p/'inspect.py').write_text('print(1)')\n"
        f"(p/'holdout').write_bytes({holdout!r})\n"
        "Path(os.environ['PROOF_OUTPUT_DIR']).joinpath('setup.json').write_text("
        f"json.dumps({manifest!r}))\n"
    )
    context = VmContext(
        topic_id="topic-a", job_id="setup-a", purpose="setup", image_digest="ab" * 32
    )
    exported = await topic.execute(
        ExecuteRequest(
            execution_id="setup-command",
            context=context,
            action=VmAction(
                operation="run", phase="setup", argv=["/usr/bin/python3", "-c", command]
            ),
        )
    )
    raw, evidence = SetupExport.model_validate(exported.setup_export).verified()
    sister = GuestExecutor(
        GuestIdentity("experiment-vm", "topic-a", "ab" * 32, "experiment"),
        workspace=tmp_path / "sister",
    )
    measured = await sister.execute(
        ExecuteRequest(
            execution_id="baseline-command",
            context=context,
            action=VmAction(operation="run", phase="experiment", argv=["model-command-ignored"]),
            params={
                "baseline_runner": "generated-python",
                "experiment_pack_digest": evidence.environment_digest,
            },
            pack_b64=base64.b64encode(raw).decode(),
        )
    )

    assert evidence.script_sha256 == hashlib.sha256(runner.encode()).hexdigest()
    assert evidence.content_hashes == manifest["content_hashes"]
    assert exported.report_digest == evidence.setup_report_digest
    assert measured.measurement.metrics[0].value == 0.75
    assert measured.measurement.flops_used == 123
    assert "operator-private-data" not in json.dumps(exported.measurement.model_dump())


def test_setup_refuses_unbacked_holdout_hashes_and_symlink_exports(tmp_path):
    pack = tmp_path / "pack"
    pack.mkdir()
    (pack / "run.py").write_text("print(1)")
    (pack / "inspect.py").write_text("print(2)")
    manifest = SetupManifest(content_hashes=["ab" * 32], flops_budget=100, wall_budget_s=30)
    with pytest.raises(VmError, match="must identify files"):
        export_setup(pack, manifest)
    (pack / "secret").symlink_to(tmp_path / "owner-key")
    with pytest.raises(VmError, match="special file"):
        export_setup(pack, manifest)
