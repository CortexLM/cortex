"""`r2-check` of s100_test/challenge_image.sh: the script's own decisions.

What the challenge program answers is canned (test_r2_programs.py runs it against a store); here
are the arguments it is given, the checks that come before it, the exit codes and what reaches the
terminal. r2-check is not gated and changes nothing in the stack.
"""

from __future__ import annotations

import pytest

from .sandbox import CHALLENGE, MASTER, R2_VALUES, ROUNDTRIP_LINE, TOKEN_PATH, Box, program


def check(box: Box, *args: str, rc: int = 0):
    return box.run("r2-check", *args, rc=rc)


def test_r2_check_uses_the_token_the_challenge_is_configured_with_and_changes_nothing(box):
    box.token()
    before = box.snapshot()
    out = check(box).out
    (call,) = box.exec_calls()
    assert (call["mode"], call["args"]) == ("roundtrip", ["-"])  # "-": the settings of its own env
    assert call["program"] == program("CHALLENGE_PY")
    assert ROUNDTRIP_LINE in out
    assert box.snapshot() == before and box.docker()["mutations"] == []


def test_r2_check_is_not_gated_and_needs_no_backup(box):
    box.seal(age=7200, ready=503)  # a closed window refuses the changes, not a look
    check(box)
    assert box.polls(f"{MASTER}/readyz") == 0


def test_r2_check_can_try_a_token_before_the_challenge_is_switched(box):
    box.token()
    check(box, "--file", TOKEN_PATH)
    (call,) = box.exec_calls()
    assert call["args"] == [TOKEN_PATH]


@pytest.mark.parametrize("mode", [0o644, 0o640, 0o400, 0o777])
def test_a_token_that_is_not_exactly_0600_is_not_tried(box, mode):
    box.token(mode=mode)
    out = check(box, "--file", TOKEN_PATH, rc=4).out
    assert f"mode {mode:o}, want 600" in out and box.exec_calls() == []


def test_a_missing_token_is_not_tried(box):
    box.token("other.env")
    assert "missing" in check(box, "--file", TOKEN_PATH, rc=4).out
    assert box.exec_calls() == []


USAGE = {
    "no-value": ["--file"],
    "twice": ["--file", TOKEN_PATH, "--file", TOKEN_PATH],
    "unknown-option": ["--bogus"],
    "stray-word": ["stray"],
    "backup-dir-of-the-other-commands": ["/root/backups/x"],
}


@pytest.mark.parametrize("args", USAGE.values(), ids=USAGE)
def test_a_usage_error_is_exit_2_before_any_call(box, args):
    box.token()
    check(box, *args, rc=2)
    assert box.docker()["calls"] == [] and box.exec_calls() == []


def test_a_round_trip_that_did_not_complete_is_exit_8_with_what_the_program_said(box):
    line = "r2-check challenge put=403 get=skipped delete=403 bytes=1048576 put_s=0.1 delete_s=0.1"
    box.exec_out("roundtrip", 1, line)
    out = check(box, rc=8).out
    assert line in out and "r2-check did not complete" in out


def test_only_the_sentences_of_the_program_reach_the_terminal(box):
    leak = "Traceback (most recent call last):\nValueError: " + R2_VALUES["R2_SECRET_ACCESS_KEY"]
    box.exec_out("roundtrip", 1, leak)
    out = check(box, rc=8).out  # Box.run also looks for every value of the token in the output
    assert "Traceback" not in out and "(the challenge program exited 1)" in out


def test_a_challenge_that_is_not_running_is_exit_8(box):
    state = box.docker()
    state["containers"][CHALLENGE]["State"]["Status"] = "exited"
    box._write("docker.json", state)
    assert "(the challenge program exited 1)" in check(box, rc=8).out


DIGEST = "sha256:" + "0f" * 32


def test_r2_check_asks_one_executor_host_with_the_image_digest_it_was_given(box):
    box.token()
    check(box, "--node", "gpu-a", DIGEST)
    (call,) = box.exec_calls()
    assert call["args"] == ["-", "gpu-a", DIGEST]  # the config is the one in the challenge's env


def test_a_host_can_be_asked_with_a_token_that_is_not_yet_the_configured_one(box):
    box.token()
    check(box, "--node", "gpu-a", DIGEST, "--file", TOKEN_PATH)
    (call,) = box.exec_calls()
    assert call["args"] == [TOKEN_PATH, "gpu-a", DIGEST]


def test_what_the_host_made_of_the_check_is_what_the_program_printed(box):
    both = (
        ROUNDTRIP_LINE + "\nr2-check node gpu-a python=3.12.4 head=200 head_bytes=1048576 get=206"
    )
    box.exec_out("roundtrip", 0, both)
    out = check(box, "--node", "gpu-a", DIGEST).out
    assert both in out


def test_a_host_that_failed_the_check_is_exit_8(box):
    both = (
        ROUNDTRIP_LINE
        + "\nr2-check node gpu-a head=SSLCertVerificationError head_verify=unknown_issuer"
    )
    box.exec_out("roundtrip", 1, both)
    out = check(box, "--node", "gpu-a", DIGEST, rc=8).out
    assert both in out and "r2-check did not complete" in out


BAD_NODE = {
    "label-only": ["--node", "gpu-a"],
    "no-value": ["--node"],
    "twice": ["--node", "gpu-a", DIGEST, "--node", "gpu-b", DIGEST],
    "upper-case-label": ["--node", "GPU-A", DIGEST],
    "label-with-a-space": ["--node", "gpu a", DIGEST],
    "label-starting-with-a-dash": ["--node", "-gpu", DIGEST],
    "label-too-long": ["--node", "a" * 33, DIGEST],
    "label-with-a-command": ["--node", "gpu;touch pwned", DIGEST],
    "tag-instead-of-a-digest": ["--node", "gpu-a", "latest"],
    "short-digest": ["--node", "gpu-a", "sha256:" + "0f" * 31],
    "upper-case-digest": ["--node", "gpu-a", "sha256:" + "0F" * 32],
    "digest-with-a-command": ["--node", "gpu-a", "sha256:$(touch pwned)" + "0f" * 32],
    "digest-with-a-newline": ["--node", "gpu-a", DIGEST + "\nprivileged"],
}


@pytest.mark.parametrize("args", BAD_NODE.values(), ids=BAD_NODE)
def test_a_label_or_digest_that_is_not_plain_never_reaches_the_challenge(box, args):
    box.token()
    check(box, *args, rc=2)
    assert box.docker()["calls"] == [] and box.exec_calls() == []
