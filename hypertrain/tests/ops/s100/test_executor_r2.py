"""`executor --r2`, `--dry-run` and `executor-off` of s100_test/challenge_image.sh (docs/adr/0013).

The challenge program that the script runs in the container is canned here (what it answers is the
test's choice) except for the one thing the fake computes itself: whether the recreated container's
variables are the ones asked for. The program really runs in test_r2_programs.py; the test that the
script sends the program that file runs ties the two. Box.run adds a check to every command: no
value of the token file in the output, in what docker was asked or given, or in any file but the
token file.
"""

from __future__ import annotations

import json
import os
import re
import stat
from pathlib import Path

import pytest

from hypertrain.gpu_ops.artifact_transfer import ArtifactSettings

from .sandbox import (
    ARTIFACTS_VAR,
    CHALLENGE,
    COMPOSE,
    EPOCH,
    EXECUTOR_PATH,
    EXECUTOR_VAR,
    MASTER,
    OTHER_ID,
    R2_VALUES,
    SETTINGS_PATH,
    TOKEN_PATH,
    Box,
    digest_of,
    program,
    tree,
)

OFFER = ("executor", EXECUTOR_PATH, "--r2", TOKEN_PATH)
BOTH = [EXECUTOR_VAR, ARTIFACTS_VAR]


def r2(box: Box, backup: Path, *extra: str, **kwargs):
    """`executor BACKUP PATH --r2 TOKEN ...`, with a token file in place (made if there is none)."""
    if not box.executor_dir.exists():
        box.token()
    cmd, path, *flags = OFFER
    return box.run(cmd, backup, path, *flags, *extra, **kwargs)


def uid_of(box: Box) -> int:
    return int(re.search(r"^CUID=(\d+)$", box.script.read_text(), re.M)[1])  # type: ignore[index]


def override_text(*names: str) -> str:
    values = {EXECUTOR_VAR: EXECUTOR_PATH, ARTIFACTS_VAR: SETTINGS_PATH}
    return "services:\n  challenge:\n    environment:\n" + "".join(
        f"      {n}: {values[n]}\n" for n in names
    )


def token_state(box: Box) -> tuple[str, int]:
    (path,) = box.tokens
    return digest_of(path), stat.S_IMODE(path.stat().st_mode)


# --- the switch -----------------------------------------------------------------------------------


def test_executor_r2_has_the_challenge_load_what_it_validated_and_leaves_the_token_alone(
    box, backup
):
    box.token()
    original, token = tree(backup), token_state(box)
    out = r2(box, backup, mutates={CHALLENGE}).out
    env = box.env(CHALLENGE)
    assert (env[EXECUTOR_VAR], env[ARTIFACTS_VAR], env["TZ"]) == (
        EXECUTOR_PATH,
        SETTINGS_PATH,
        "UTC",
    )
    assert ARTIFACTS_VAR not in box.env("hypertrain-test-beacon")
    changed = [f"services.challenge.environment.{n}" for n in BOTH]
    assert f"changed {changed}" in out
    assert box.override.read_text() == override_text(*BOTH)
    assert box.f.read_text() == COMPOSE  # the base file is never edited
    # the settings are what the challenge's own model reads; they name the token and nothing else
    settings = ArtifactSettings.model_validate_json(box.settings.read_bytes())
    assert settings.credentials_file == Path(TOKEN_PATH)
    assert settings.operations == ("audit", "referee")
    assert json.loads(box.settings.read_text()).keys() == {"credentials_file", "operations"}
    for path in (box.settings, box.override):
        assert stat.S_IMODE(path.stat().st_mode) == 0o600
    uid = uid_of(box)
    installed = (box.fake / "install.log").read_text().splitlines()
    assert any(
        re.fullmatch(rf"-m 0600 -o {uid} -g {uid} /dev/stdin \S+/r2-artifacts\.json\.new", x)
        for x in installed
    ), installed
    assert not box.settings.with_name("r2-artifacts.json.new").exists()
    assert not box.override.with_name("compose.executor.yml.new").exists()
    assert token_state(box) == token  # never read, written, moved or chmod-ed
    assert {k: v for k, v in tree(backup).items() if k in original} == original
    assert "preflight ok" in out and "postflight ok" in out
    assert re.search(r"R2_[A-Z_]+", out) is None


def test_the_challenge_is_asked_before_anything_is_written_and_again_once_it_is_recreated(
    box, backup
):
    r2(box, backup, mutates={CHALLENGE})
    pre, post = box.exec_calls()
    assert (pre["mode"], pre["args"]) == ("pre", [EXECUTOR_PATH])
    assert (pre["settings"], pre["override"]) == (None, None)  # nothing exists yet
    assert json.loads(pre["stdin"]) == json.loads(box.settings.read_text())  # the settings to be
    assert (post["mode"], post["args"]) == ("post", [EXECUTOR_PATH, SETTINGS_PATH])
    assert post["settings"] == digest_of(box.settings) and post["override"] == digest_of(
        box.override
    )


def test_the_script_sends_the_program_that_test_r2_programs_runs(box, backup):
    r2(box, backup, mutates={CHALLENGE})
    assert {c["program"] for c in box.exec_calls()} == {program("CHALLENGE_PY")}


def test_operations_are_written_as_asked(box, backup):
    r2(box, backup, "--operations", "reference,audit,referee", mutates={CHALLENGE})
    assert ArtifactSettings.model_validate_json(box.settings.read_bytes()).operations == (
        "reference",
        "audit",
        "referee",
    )


def test_the_same_command_twice_recreates_nothing_and_new_settings_do(box, backup):
    def started() -> str:
        return box.container(CHALLENGE)["State"]["StartedAt"]

    r2(box, backup, mutates={CHALLENGE})
    first, files = started(), (digest_of(box.settings), digest_of(box.override))
    r2(box, backup, mutates={CHALLENGE})  # compose is asked again and has nothing to do
    assert started() == first
    assert (digest_of(box.settings), digest_of(box.override)) == files
    # the override is unchanged but the challenge reads the settings once per process
    r2(box, backup, "--operations", "audit", mutates={CHALLENGE})
    assert started() != first
    assert ArtifactSettings.model_validate_json(box.settings.read_bytes()).operations == ("audit",)
    again = started()
    r2(box, backup, "--operations", "audit", mutates={CHALLENGE})
    assert started() == again


# --- refusals: nothing is written, nothing is recreated -------------------------------------------


def refused(box: Box, backup: Path, rc: int, *extra: str, calls: int = 0, **kwargs) -> str:
    """Run `executor --r2` and require that it changed nothing; `calls` challenge programs ran."""
    if not box.executor_dir.exists():
        box.token()
    before = box.snapshot()
    out = r2(box, backup, *extra, rc=rc, **kwargs).out
    assert box.snapshot() == before
    assert len(box.exec_calls()) == calls
    return out


@pytest.mark.parametrize("mode", [0o644, 0o640, 0o660, 0o666, 0o400, 0o700, 0o777])
def test_a_token_file_that_is_not_exactly_0600_is_refused_before_the_challenge_is_asked(
    box, backup, mode
):
    box.token(mode=mode)
    out = refused(box, backup, 4)  # no program ran: the script's own check refused it
    assert f"mode {mode:o}, want 600" in out


def test_a_token_file_of_mode_0600_is_what_the_refusals_are_measured_against(box, backup):
    box.token()
    r2(box, backup, mutates={CHALLENGE})


def test_a_token_file_of_another_user_is_refused(tmp_path, golden):
    box = Box(tmp_path, challenge_uid=os.getuid() + 1)
    backup = box.seed_backup(golden)
    box.token()
    out = refused(box, backup, 4)
    assert f"owner uid {os.getuid()}, want {os.getuid() + 1}" in out


def make_directory(box: Box) -> None:
    box.token()
    (box.executor_dir / "r2.env").unlink()
    (box.executor_dir / "r2.env").mkdir()


def make_link(box: Box) -> None:
    box.token("real.env")
    (box.executor_dir / "r2.env").symlink_to("real.env")


def make_dangling_link(box: Box) -> None:
    box.token("other.env")
    (box.executor_dir / "r2.env").symlink_to("nowhere.env")


def make_missing(box: Box) -> None:
    box.token("other.env")


@pytest.mark.parametrize(
    "make",
    [make_directory, make_link, make_dangling_link, make_missing],
    ids=["directory", "link-to-a-0600-token", "dangling-link", "missing"],
)
def test_a_token_file_that_is_missing_a_link_or_not_a_plain_file_is_refused(box, backup, make):
    make(box)
    box.tokens.discard(box.executor_dir / "r2.env")
    out = r2(box, backup, rc=4).out
    assert "missing, a link or not a plain file" in out and len(box.exec_calls()) == 0
    assert not box.settings.exists() and not box.override.exists()


def test_without_an_executor_directory_there_is_no_token_to_use(box, backup):
    before = box.snapshot()
    assert "missing" in box.run(*OFFER[:1], backup, *OFFER[1:], rc=4).out
    assert box.snapshot() == before


def test_an_executor_directory_open_to_others_is_refused(box, backup):
    box.token()
    box.executor_dir.chmod(0o755)
    assert "mode 755" in refused(box, backup, 4)


BAD_TOKEN_PATHS = {
    "relative": "r2.env",
    "outside-data": "/etc/r2.env",
    "data-root": "/data/r2.env",
    "dot-dot": "/data/executor/../r2.env",
    "nested": "/data/executor/sub/r2.env",
    "hidden": "/data/executor/.r2.env",
    "space": "/data/executor/r2 env",
    "newline": "/data/executor/r2.env\nprivileged: true",
    "directory": "/data/executor/",
    "the-directory-itself": "/data/executor",
}


@pytest.mark.parametrize("path", BAD_TOKEN_PATHS.values(), ids=BAD_TOKEN_PATHS)
def test_a_token_path_must_be_one_plain_name_in_the_executor_directory(box, backup, path):
    box.token()
    before = box.snapshot()
    out = box.run("executor", backup, EXECUTOR_PATH, "--r2", path, rc=4).out
    assert "must be /data/executor/NAME" in out and box.exec_calls() == []
    assert box.snapshot() == before


USAGE = {
    "no-value": ["--r2"],
    "operations-without-r2": ["--operations", "audit"],
    "unknown-operation": ["--r2", TOKEN_PATH, "--operations", "nothing"],
    "trailing-comma": ["--r2", TOKEN_PATH, "--operations", "audit,"],
    "empty-operations": ["--r2", TOKEN_PATH, "--operations", ""],
    "case": ["--r2", TOKEN_PATH, "--operations", "Audit"],
    "unknown-option": ["--bogus"],
    "stray-word": ["stray"],
    "twice-dry-run": ["--dry-run", "--dry-run"],
    "twice-r2": ["--r2", TOKEN_PATH, "--r2", TOKEN_PATH],
    "twice-operations": ["--r2", TOKEN_PATH, "--operations", "audit", "--operations", "audit"],
}


@pytest.mark.parametrize("args", USAGE.values(), ids=USAGE)
def test_a_usage_error_stops_before_the_gate_and_before_any_call(box, backup, args):
    box.token()
    before = box.snapshot()
    box.run("executor", backup, EXECUTOR_PATH, *args, rc=2)
    assert box.snapshot() == before and box.polls(f"{MASTER}/readyz") == 0
    assert box.docker()["calls"] == []


# The gate is the one the other gated commands use (test_challenge_image.GATE). These cases are the
# ones that fail if --r2 steps around it: a run with a closed window changes nothing.
@pytest.mark.parametrize(
    ("seal", "rc"),
    [({"age": 1800}, 3), ({"sealed": False}, 3), ({"epoch": EPOCH}, 3), ({"ready": 503}, 2)],
    ids=["age-1800", "sealed-false", "epoch-is-now", "readyz-503"],
)
def test_a_closed_window_refuses_executor_r2_before_the_challenge_is_asked(box, backup, seal, rc):
    box.seal(**seal)
    refused(box, backup, rc)


def test_a_compose_file_naming_a_production_container_is_refused_before_the_challenge_is_asked(
    box, backup
):
    box.f.write_text(COMPOSE.replace(f"name: {CHALLENGE}", "name: cortex-challenge-hypertrain"))
    assert "REFUSED" in refused(box, backup, 4)


def test_the_challenge_is_not_asked_unless_it_runs_the_image_the_compose_file_resolves(box, backup):
    box.token()
    state = box.docker()
    state["containers"][CHALLENGE]["Image"] = OTHER_ID
    box._write("docker.json", state)
    assert "not on the image" in refused(box, backup, 4)


def test_a_challenge_that_is_not_running_cannot_vouch_for_anything(box, backup):
    box.token()
    state = box.docker()
    state["containers"][CHALLENGE]["State"]["Status"] = "exited"
    box._write("docker.json", state)
    out = refused(box, backup, 4, calls=1)
    assert "(the challenge program exited 1)" in out and "nothing written" in out


def test_a_refusal_of_the_challenge_stops_the_run_with_its_reason(box, backup):
    reason = "preflight refused: max_artifact_bytes is above the executor's max_result_bytes"
    box.exec_out("pre", 1, reason)
    out = refused(box, backup, 4, calls=1)
    assert reason in out and "nothing written" in out


def test_only_the_sentences_of_the_program_reach_the_operator(box, backup):
    trace = "Traceback (most recent call last):\nValueError: " + R2_VALUES["R2_SECRET_ACCESS_KEY"]
    box.exec_out("pre", 1, trace + "\npreflight refused: ValueError")
    out = refused(box, backup, 4, calls=1)  # Box.run also looks for the value in the output
    assert "Traceback" not in out and "preflight refused: ValueError" in out


def test_a_settings_file_the_script_did_not_write_is_not_overwritten(box, backup):
    box.token()
    foreign = {"credentials_file": TOKEN_PATH, "operations": ["audit"], "workers": 3}
    box.settings.write_text(json.dumps(foreign))
    out = refused(box, backup, 4)
    assert "not a file this script wrote" in out and json.loads(box.settings.read_text()) == foreign


# --- after the files are written ------------------------------------------------------------------


def test_a_container_that_compose_did_not_recreate_is_caught_by_the_second_look(box, backup):
    box.docker(stale_up=True)
    out = r2(box, backup, rc=7, mutates={CHALLENGE}).out
    assert f"{EXECUTOR_VAR} is not set as the script wrote it" in out
    assert f"executor-off {backup}" in out


def test_a_failed_up_says_how_to_go_back_and_leaves_the_files_for_executor_off(box, backup):
    box.docker(fail_up=True)
    out = r2(box, backup, rc=7, mutates={CHALLENGE}).out
    assert f"executor-off {backup}" in out
    assert box.override.exists() and box.settings.exists()
    box.docker(fail_up=False)
    box.run("executor-off", backup, mutates={CHALLENGE})
    assert not box.override.exists() and not box.settings.exists()


def test_settings_the_recreated_challenge_cannot_load_are_reported_after_the_variables_are_found(
    box, backup
):
    box.exec_out("post", 1, "postflight refused: PermissionError")
    out = r2(box, backup, rc=7, mutates={CHALLENGE}).out
    assert "postflight refused: PermissionError" in out and f"executor-off {backup}" in out


# --- dry run --------------------------------------------------------------------------------------


def test_a_dry_run_asks_the_challenge_and_changes_nothing(box, backup):
    box.token()
    before, original = box.snapshot(), tree(backup)
    out = r2(box, backup, "--dry-run").out
    assert box.snapshot() == before
    assert tree(backup) == original  # not even the config files a real run leaves in the backup
    assert [c["mode"] for c in box.exec_calls()] == ["pre"]
    assert f"variables: {EXECUTOR_VAR} {ARTIFACTS_VAR}" in out
    assert "settings keys: credentials_file operations" in out and "preflight ok" in out
    assert "gate: open" in out and "dry-run: nothing written, nothing recreated" in out
    assert re.search(r"R2_[A-Z_]+", out) is None and box.docker()["mutations"] == []


def test_a_dry_run_leaves_no_file_of_its_own_behind(box, backup):
    box.token()
    r2(box, backup, "--dry-run")
    assert sorted(p.name for p in box.t.iterdir()) == ["compose.relay.yml", "compose.test.yml"]


def test_a_dry_run_says_whether_the_challenge_would_be_recreated(box, backup):
    r2(box, backup, mutates={CHALLENGE})
    same = r2(box, backup, "--dry-run").out
    other = r2(box, backup, "--operations", "audit", "--dry-run").out
    assert "recreate: only if the resolved config changes" in same
    assert "recreate: --force-recreate" in other


def test_a_dry_run_with_a_closed_window_reports_the_rest_and_exits_with_the_gate(box, backup):
    box.token()
    box.seal(age=1800)
    out = refused(box, backup, 3, "--dry-run", calls=1)
    assert "gate: closed, a real run would exit 3" in out and "preflight ok" in out


def test_a_dry_run_refuses_what_the_real_run_refuses(box, backup):
    box.token(mode=0o644)
    assert "mode 644" in refused(box, backup, 4, "--dry-run")


def test_a_dry_run_without_r2_names_the_one_variable(box, backup):
    before = box.snapshot()
    out = box.run("executor", backup, EXECUTOR_PATH, "--dry-run").out
    assert f"variables: {EXECUTOR_VAR} \n" in out and "settings keys" not in out
    assert box.snapshot() == before and box.exec_calls() == []


# --- executor-off and a plain executor ------------------------------------------------------------


def test_executor_off_removes_every_variable_and_the_file_it_wrote_and_nothing_else(box, backup):
    box.token()
    r2(box, backup, mutates={CHALLENGE})
    token = token_state(box)
    out = box.run("executor-off", backup, mutates={CHALLENGE}).out
    assert not box.override.exists() and not box.settings.exists()
    env = box.env(CHALLENGE)
    assert EXECUTOR_VAR not in env and ARTIFACTS_VAR not in env and env["TZ"] == "UTC"
    assert box.f.read_text() == COMPOSE and token_state(box) == token  # the operator's token stays
    assert "removed " + SETTINGS_PATH in out
    post = box.exec_calls("post")[-1]
    assert post["args"] == ["-", "-"] and post["settings"] is not None  # the file went last


def test_executor_off_is_the_way_back_even_when_nothing_was_switched(box, backup):
    box.run("executor-off", backup, mutates={CHALLENGE})
    assert not box.settings.exists()


def test_a_failed_recreate_leaves_the_running_challenge_the_settings_it_reads(box, backup):
    r2(box, backup, mutates={CHALLENGE})
    box.docker(fail_up=True)
    box.run("executor-off", backup, rc=1, mutates={CHALLENGE})
    assert box.settings.exists()


def test_executor_off_keeps_a_settings_file_it_did_not_write(box, backup):
    foreign = '{"credentials_file": "/data/executor/r2.env", "operations": ["audit"], "workers": 3}'
    box.executor_dir.mkdir()
    box.settings.write_text(foreign)
    out = box.run("executor-off", backup, mutates={CHALLENGE}).out
    assert box.settings.read_text() == foreign and f"kept {SETTINGS_PATH}" in out


def test_executor_off_fails_when_the_live_compose_file_still_sets_the_variable(box, backup):
    # a hand edit of compose.test.yml: the override is gone and the challenge still has the variable
    pin = f"container_name: {CHALLENGE}\n"
    box.f.write_text(
        COMPOSE.replace(pin, f"{pin}    environment:\n      {ARTIFACTS_VAR}: {SETTINGS_PATH}\n")
    )
    out = box.run("executor-off", backup, rc=7, mutates={CHALLENGE}).out
    assert f"{ARTIFACTS_VAR} is not set as the script wrote it" in out


def test_a_plain_executor_returns_to_the_ssh_path_and_removes_the_settings_it_wrote(box, backup):
    box.token()
    r2(box, backup, mutates={CHALLENGE})
    token = token_state(box)
    out = box.run("executor", backup, EXECUTOR_PATH, mutates={CHALLENGE}).out
    env = box.env(CHALLENGE)
    assert env[EXECUTOR_VAR] == EXECUTOR_PATH and ARTIFACTS_VAR not in env
    assert box.override.read_text() == override_text(EXECUTOR_VAR)
    assert not box.settings.exists() and "removed " + SETTINGS_PATH in out
    assert token_state(box) == token
    assert box.exec_calls("post")[-1]["args"] == [EXECUTOR_PATH, "-"]


def test_a_plain_executor_keeps_a_settings_file_it_did_not_write(box, backup):
    foreign = '{"credentials_file": "/data/executor/r2.env", "operations": ["audit"], "workers": 3}'
    box.executor_dir.mkdir()
    box.settings.write_text(foreign)
    box.run("executor", backup, EXECUTOR_PATH, mutates={CHALLENGE})
    assert box.settings.read_text() == foreign
