"""Adversarial archive inputs must never expose evaluator-owned files to candidates."""

import os

import pytest

from cortex.vm.candidate import _copy_artifact, candidate_command, run_candidate
from cortex.vm.models import VmError


@pytest.mark.parametrize("kind", ["symlink", "directory-link", "fifo"])
def test_candidate_copy_refuses_special_files_without_reading_private_data(tmp_path, kind):
    artifact = tmp_path / "artifact"
    artifact.mkdir()
    private = tmp_path / "private"
    private.mkdir()
    (private / "holdout").write_text("never exposed")
    link = artifact / "escape"
    if kind == "symlink":
        link.symlink_to(private / "holdout")
    elif kind == "directory-link":
        link.symlink_to(private, target_is_directory=True)
    else:
        os.mkfifo(link)

    with pytest.raises(VmError, match="special file"):
        _copy_artifact(artifact, tmp_path / "copy")
    assert not (tmp_path / "copy" / "escape").exists()


def test_copied_candidate_is_regular_readable_data_with_no_owner_write_access(tmp_path):
    artifact = tmp_path / "artifact"
    artifact.mkdir()
    source = artifact / "program.py"
    source.write_text("print(1)")
    source.chmod(0o4755)

    _copy_artifact(artifact, tmp_path / "copy")

    copied = tmp_path / "copy" / "program.py"
    assert copied.read_text() == "print(1)"
    assert copied.stat().st_mode & 0o7777 == 0o644
    assert copied.stat().st_ino != source.stat().st_ino


def test_candidate_command_mounts_only_runtime_artifact_work_and_private_tmp(tmp_path):
    artifact = tmp_path / "artifact"
    command = candidate_command(
        artifact, 9, ["python3", "/artifact/main.py"], wall_seconds=31, memory_mib=256
    )

    assert "--unshare-all" in command and "--disable-userns" in command
    assert "--unshare-user" in command
    assert command[command.index("--uid") + 1] == "65534"
    assert command[command.index("--seccomp") + 1] == "9"
    assert command[command.index("--cap-drop") + 1] == "ALL"
    assert "--as=268435456" in command and "--cpu=31" in command
    assert "--clearenv" in command
    assert not any(
        value in command for value in ["/workspace", "/etc", "/root", "/run", "/opt/cortex"]
    )


def test_candidate_refuses_to_execute_on_an_ordinary_host():
    with pytest.raises(VmError, match="not running"):
        run_candidate(["python3", "/artifact/main.py"], {})


@pytest.mark.parametrize(
    "limits", [{"wall_seconds": 0, "memory_mib": 256}, {"wall_seconds": 30, "memory_mib": 32769}]
)
def test_candidate_limits_cannot_exceed_guest_ceilings(tmp_path, limits):
    with pytest.raises(VmError, match="resource limit"):
        candidate_command(tmp_path, 9, ["python3"], **limits)
