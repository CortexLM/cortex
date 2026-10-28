"""Edges of `executor`, `executor-off` and their helpers that a mutation pass found unpinned.

Each test names a behaviour that a change of the script could drop without any other test noticing:
the directory of the token, files and links at the settings path, what a dry run shares with the
real run, what reaches the operator from the challenge program, the compose diff. Helpers and
fixtures are those of test_executor_r2.py.
"""

from __future__ import annotations

import re

import pytest

from .sandbox import (
    ARTIFACTS_VAR,
    CHALLENGE,
    COMPOSE,
    EXECUTOR_PATH,
    EXECUTOR_VAR,
    OTHER_ID,
    R2_VALUES,
    SETTINGS_PATH,
    TOKEN_PATH,
)
from .test_executor_r2 import r2, refused, uid_of

SECRET = R2_VALUES["R2_SECRET_ACCESS_KEY"]


# --- the directory of the token: what stops a swap of the token ----------------------------------


@pytest.mark.parametrize("mode", [0o750, 0o770, 0o710, 0o707, 0o701, 0o755])
def test_an_executor_directory_open_to_a_group_or_to_others_is_refused(box, backup, mode):
    box.token()
    box.executor_dir.chmod(mode)
    assert f"mode {mode:o} uid" in refused(box, backup, 4)


def test_an_executor_directory_that_is_a_link_is_refused(box, backup):
    box.token()
    real = box.state_dir / "elsewhere"
    box.executor_dir.rename(real)
    box.executor_dir.symlink_to(real)
    box.tokens.add(real / "r2.env")
    assert "mode 777" in refused(box, backup, 4)


def test_an_executor_directory_of_another_owner_is_refused(box, backup):
    # the sandbox cannot chown: its stat shim reports FAKE_DIR_UID as the owner of the directory
    box.token()
    before = box.snapshot()
    env = {"FAKE_DIR_UID": "0"}
    out = box.run("executor", backup, EXECUTOR_PATH, "--r2", TOKEN_PATH, rc=4, env=env).out
    assert "want 700 uid" in out and "uid 0" in out
    assert box.snapshot() == before and box.exec_calls() == []


def test_a_token_name_may_start_with_an_underscore(box, backup):
    name = box.token("_r2.env")
    box.run("executor", backup, EXECUTOR_PATH, "--r2", str(name), mutates={CHALLENGE})


def test_a_token_name_may_not_start_with_a_dash(box, backup):
    box.token()
    out = box.run("executor", backup, EXECUTOR_PATH, "--r2", "/data/executor/-r2.env", rc=4).out
    assert "REFUSED --r2 FILE must be /data/executor/NAME" in out and box.exec_calls() == []


# --- the settings path: links, files the script did not write, leftovers -------------------------


def test_a_dangling_link_at_the_settings_path_is_refused_and_left(box, backup):
    box.token()
    box.settings.symlink_to("nowhere.json")
    assert "not a file this script wrote" in refused(box, backup, 4) and box.settings.is_symlink()


def test_executor_off_keeps_a_dangling_link_at_the_settings_path_and_says_so(box, backup):
    box.executor_dir.mkdir()
    box.settings.symlink_to("nowhere.json")
    out = box.run("executor-off", backup, mutates={CHALLENGE}).out
    assert f"kept {SETTINGS_PATH}" in out and box.settings.is_symlink()


def test_a_link_to_a_file_with_the_right_keys_is_not_a_file_the_script_wrote(box, backup):
    r2(box, backup, mutates={CHALLENGE})
    box.settings.rename(box.executor_dir / "mine.json")
    box.settings.symlink_to("mine.json")
    out = box.run("executor-off", backup, mutates={CHALLENGE}).out
    assert box.settings.is_symlink() and f"kept {SETTINGS_PATH}" in out


FOREIGN = {
    "one-key": '{"credentials_file": "/data/executor/r2.env"}',
    "empty-object": "{}",
    "list-of-the-two-names": '["credentials_file", "operations"]',
    "not-json": "this is not json",
    "binary": "\x00\xff\x00",
}


@pytest.mark.parametrize("text", FOREIGN.values(), ids=FOREIGN)
def test_executor_r2_leaves_a_settings_file_that_is_not_exactly_ours_and_prints_no_traceback(
    box, backup, text
):
    box.token()
    box.settings.write_text(text)
    out = refused(box, backup, 4)
    assert "not a file this script wrote" in out and "Traceback" not in out
    assert box.settings.read_text() == text


@pytest.mark.parametrize("text", FOREIGN.values(), ids=FOREIGN)
def test_executor_off_keeps_a_settings_file_that_is_not_exactly_ours(box, backup, text):
    box.executor_dir.mkdir()
    box.settings.write_text(text)
    out = box.run("executor-off", backup, mutates={CHALLENGE}).out
    assert f"kept {SETTINGS_PATH}" in out and "Traceback" not in out
    assert box.settings.read_text() == text


def test_executor_off_removes_a_half_written_settings_file_with_ours(box, backup):
    r2(box, backup, mutates={CHALLENGE})
    stale = box.settings.with_name("r2-artifacts.json.new")
    stale.write_text("half")
    box.run("executor-off", backup, mutates={CHALLENGE})
    assert not stale.exists() and not box.settings.exists()


def test_a_plain_executor_removes_a_half_written_settings_file_with_ours(box, backup):
    r2(box, backup, mutates={CHALLENGE})
    stale = box.settings.with_name("r2-artifacts.json.new")
    stale.write_text("half")
    box.run("executor", backup, EXECUTOR_PATH, mutates={CHALLENGE})
    assert not stale.exists() and not box.settings.exists()


def test_a_plain_executor_asks_the_challenge_before_it_removes_the_settings(box, backup):
    r2(box, backup, mutates={CHALLENGE})
    box.run("executor", backup, EXECUTOR_PATH, mutates={CHALLENGE})
    assert box.exec_calls("post")[-1]["settings"] is not None  # the file went after the look


def test_a_plain_executor_whose_up_fails_leaves_the_running_challenge_its_settings(box, backup):
    r2(box, backup, mutates={CHALLENGE})
    box.docker(fail_up=True)
    box.run("executor", backup, EXECUTOR_PATH, rc=7, mutates={CHALLENGE})
    assert box.settings.exists()


def test_a_plain_executor_whose_container_was_not_recreated_keeps_the_settings_it_still_reads(
    box, backup
):
    r2(box, backup, mutates={CHALLENGE})
    box.docker(
        stale_up=True
    )  # compose recreated nothing: the container still carries both variables
    out = box.run("executor", backup, EXECUTOR_PATH, rc=7, mutates={CHALLENGE}).out
    assert f"{ARTIFACTS_VAR} is not set as the script wrote it" in out
    assert box.settings.exists()


# --- executor-off is the way back ----------------------------------------------------------------


def test_executor_off_is_not_gated(box, backup):
    box.seal(age=7200, ready=503)
    box.run("executor-off", backup, mutates={CHALLENGE})


def test_executor_off_with_nothing_to_undo_recreates_nothing(box, backup):
    box.run("executor-off", backup, mutates={CHALLENGE})  # the fixture container has no spec yet
    started = box.container(CHALLENGE)["State"]["StartedAt"]
    box.run("executor-off", backup, mutates={CHALLENGE})
    assert box.container(CHALLENGE)["State"]["StartedAt"] == started


def test_executor_and_executor_off_end_on_what_an_operator_reads(box, backup):
    on = r2(box, backup, mutates={CHALLENGE}).out
    off = box.run("executor-off", backup, mutates={CHALLENGE}).out
    assert "public runs sha" in on and "public runs sha" in off


# --- arguments -----------------------------------------------------------------------------------


def test_operations_with_junk_before_a_valid_name_is_a_usage_error(box, backup):
    box.token()
    box.run("executor", backup, EXECUTOR_PATH, "--r2", TOKEN_PATH, "--operations", "x,audit", rc=2)


@pytest.mark.parametrize("args", [["--r2"], ["--bogus"], ["--dry-run", "--dry-run"]])
def test_a_usage_error_of_executor_says_usage(box, backup, args):
    assert box.run("executor", backup, EXECUTOR_PATH, *args, rc=2).out.startswith("usage: ")


def test_an_empty_r2_value_is_a_usage_error_and_not_a_plain_executor(box, backup):
    # "$UNSET" as the file would otherwise switch R2 off and remove the settings file
    r2(box, backup, mutates={CHALLENGE})
    before = box.snapshot()
    box.run("executor", backup, EXECUTOR_PATH, "--r2", "", rc=2)
    assert box.snapshot() == before


def test_an_unknown_command_is_exit_2(box):
    assert "unknown bogus" in box.run("bogus", rc=2).out


# --- the compose diff ----------------------------------------------------------------------------


def test_executor_refuses_a_live_file_that_already_sets_the_variable_to_that_value(box, backup):
    pin = f"container_name: {CHALLENGE}\n"
    box.f.write_text(
        COMPOSE.replace(pin, f"{pin}    environment:\n      {EXECUTOR_VAR}: {EXECUTOR_PATH}\n")
    )
    before = box.snapshot()
    out = box.run("executor", backup, EXECUTOR_PATH, rc=5).out
    assert "changed []" in out and box.snapshot() == before


def test_executor_r2_refuses_a_live_file_that_already_sets_one_of_the_two(box, backup):
    box.token()
    pin = f"container_name: {CHALLENGE}\n"
    box.f.write_text(
        COMPOSE.replace(pin, f"{pin}    environment:\n      {ARTIFACTS_VAR}: {SETTINGS_PATH}\n")
    )
    before = box.snapshot()
    out = box.run("executor", backup, EXECUTOR_PATH, "--r2", TOKEN_PATH, rc=5).out
    assert f"changed ['services.challenge.environment.{EXECUTOR_VAR}']" in out
    assert box.snapshot() == before


# --- the dry run ---------------------------------------------------------------------------------

GATES = {
    "readyz-503": ({"ready": 503}, 2, "GATE readyz not 200"),
    "age-4500": ({"age": 4500}, 3, "GATE seal not sealed, older than 4500s, or behind"),
}


@pytest.mark.parametrize(("seal", "rc", "why"), GATES.values(), ids=GATES)
def test_a_dry_run_with_a_closed_window_says_why_and_which_code(box, backup, seal, rc, why):
    box.token()
    box.seal(**seal)
    out = refused(box, backup, rc, "--dry-run", calls=1)
    assert why in out and f"gate: closed, a real run would exit {rc}" in out


def test_a_dry_run_states_the_token_it_wants_in_full(box, backup):
    box.token()
    out = r2(box, backup, "--dry-run").out
    want = f"settings keys: credentials_file operations; token file: mode 600, uid {uid_of(box)}"
    assert want in out.splitlines()


def test_a_dry_run_refuses_a_settings_file_that_is_not_ours(box, backup):
    box.token()
    box.settings.write_text('{"workers": 3}')
    assert "not a file this script wrote" in refused(box, backup, 4, "--dry-run")


def test_a_dry_run_refuses_a_challenge_that_is_not_on_the_resolved_image(box, backup):
    box.token()
    state = box.docker()
    state["containers"][CHALLENGE]["Image"] = OTHER_ID
    box._write("docker.json", state)
    assert "not on the image" in refused(box, backup, 4, "--dry-run")


def test_a_dry_run_keeps_its_scratch_directory_under_the_backups_directory(box, backup):
    box.token()
    r2(box, backup, "--dry-run")
    logged = (box.fake / "python3.log").read_text()
    paths = re.findall(r"'([^']*config-before-executor\.json)'", logged)
    assert paths and all(p.startswith(f"{box.t}/dry.") for p in paths)


# --- what run_py lets through --------------------------------------------------------------------


def test_a_traceback_on_stderr_never_reaches_the_operator(box, backup):
    trace = "Traceback (most recent call last):\nValueError: " + SECRET
    box.exec_out("pre", 1, trace + "\npreflight refused: ValueError", stderr=True)
    out = refused(box, backup, 4, calls=1)
    assert "Traceback" not in out and "preflight refused: ValueError" in out


LINES = {
    "sentence-word-inside-a-line": "ValueError: preflight ok " + SECRET,
    "sentence-then-a-tab": "preflight refused: x\t" + SECRET,
    "a-line-longer-than-a-sentence": "preflight " + "x" * 601,
}


@pytest.mark.parametrize("line", LINES.values(), ids=LINES)
def test_a_line_that_is_not_a_sentence_of_the_program_is_dropped(box, backup, line):
    box.exec_out("pre", 1, line)
    out = refused(box, backup, 4, calls=1)
    assert "(no answer from the challenge: docker exec exited 1" in out


def test_the_exit_status_of_the_program_is_the_one_printed(box, backup):
    box.exec_out("pre", 137, "")
    out = refused(box, backup, 4, calls=1)
    assert "(no answer from the challenge: docker exec exited 137" in out
