"""The sandbox's own checks, shown on scripts that break them.

Box.run and Box.snapshot are what every other test leans on: a check that stops checking turns each
test that relies on it into a pass, and nothing says so. Every case here hands the box a copy of the
script with one bug and requires the harness itself to refuse it."""

from __future__ import annotations

import stat

import pytest

from .sandbox import (
    BEACON,
    CHALLENGE,
    EXECUTOR_PATH,
    R2_VALUES,
    Box,
    call,
)
from .test_executor_r2 import refused, token_state

# in `executor-off`, where $XD (the executor directory), $B (the backup) and $HOME are set
ANCHOR = (
    '  date -u +%FT%TZ\n  rm -f "$T/compose.executor.yml";'
    ' compose "$F" up -d --no-deps challenge 2>&1'
)
UP = 'rm -f "$T/compose.executor.yml"; compose "$F" up -d --no-deps challenge 2>&1 | tail -3\n'


def patched(box: Box, old: str, new: str) -> None:
    text = box.script.read_text()
    assert text.count(old) == 1, old
    box.script.write_text(text.replace(old, new))


def with_line(box: Box, line: str) -> None:
    patched(box, ANCHOR, ANCHOR.replace("  rm -f", f"  {line}\n  rm -f", 1))


def off(box: Box, backup, **kwargs):
    return box.run("executor-off", backup, **{"mutates": {CHALLENGE}, **kwargs})


REFUSED = {  # id: (a line the bug adds to executor-off, the message of the refusal)
    "ssh": ("ssh -G localhost >/dev/null 2>&1 || true", "tripwire.log is not empty"),
    "scp": ("scp -h >/dev/null 2>&1 || true", "tripwire.log is not empty"),
    "a-url-nobody-served": (
        'curl -s -m 5 "$API/internal/x" >/dev/null || true',
        "unknown-url.log is not empty",
    ),
    "a-docker-command-the-script-does-not-use": (
        'docker rm -f "$C-x" >/dev/null 2>&1 || true',
        "tripwire.log is not empty",
    ),
    "a-docker-exec-of-another-shape-swallowed": (
        'docker exec "$C" true >/dev/null 2>&1 || true',
        "tripwire.log is not empty",
    ),
    "a-test-container-too-many": (
        "docker restart hypertrain-test-relay >/dev/null",
        "mutated",
    ),
    "a-production-container": (
        "docker restart hypertrain-auditor-0 >/dev/null",
        "(outside the TEST stack|mutated)",
    ),
    "a-read-of-a-production-container": (
        "docker inspect cortex-challenge-hypertrain --format '{{.Image}}' >/dev/null 2>&1 || true",
        "tripwire.log is not empty",
    ),
}


@pytest.mark.parametrize(("line", "message"), REFUSED.values(), ids=REFUSED)
def test_box_run_refuses_a_script_that_reaches_out_or_touches_more(box, backup, line, message):
    with_line(box, line)
    with pytest.raises(AssertionError, match=message):
        off(box, backup)


def test_box_run_refuses_a_production_container_even_when_the_test_lists_it(box, backup):
    with_line(box, "docker restart hypertrain-auditor-0 >/dev/null")
    with pytest.raises(AssertionError, match="outside the TEST stack"):
        off(box, backup, mutates={CHALLENGE, "hypertrain-auditor-0"})


def test_box_run_refuses_a_script_that_recreates_the_beacon_with_the_challenge(box, backup):
    patched(box, UP, UP.replace("challenge 2>&1", "challenge beacon 2>&1"))
    with pytest.raises(AssertionError, match="mutated"):
        off(box, backup)


def test_box_run_refuses_up_without_no_deps(box, backup):
    patched(box, UP, UP.replace("--no-deps ", ""))
    with pytest.raises(AssertionError):
        off(box, backup)


def test_box_run_wants_the_exact_exit_code(box, backup):
    patched(
        box,
        'older than ${max}s, or behind"; exit 3; }',
        'older than ${max}s, or behind"; exit 5; }',
    )
    box.seal(age=4500)
    with pytest.raises(AssertionError, match="exit 5, wanted 3"):
        box.run(*call("executor", backup), rc=3)


def test_box_run_wants_every_container_the_test_says_was_mutated_and_no_other(box, backup):
    with pytest.raises(AssertionError, match="mutated"):
        box.run("paper", backup, mutates={CHALLENGE})  # paper touches nothing
    with pytest.raises(AssertionError, match="mutated"):
        off(box, backup, mutates=set())  # executor-off recreates the challenge
    with pytest.raises(AssertionError, match="mutated"):
        off(box, backup, mutates={CHALLENGE, BEACON})


LEAKS = {  # id: the line a bug adds to executor-off, KEY standing for the name of the value
    "to-the-output": 'sed -n "s/^KEY=//p" "$XD/r2.env"',
    "to-a-file-of-the-working-directory": 'sed -n "s/^KEY=//p" "$XD/r2.env" > "$HOME/leak"',
    "beside-the-token": 'cp "$XD/r2.env" "$XD/r2.env.bak"',
    "into-the-backup-under-the-same-name": 'cp -p "$XD/r2.env" "$B/"',
}


@pytest.mark.parametrize("key", R2_VALUES)
@pytest.mark.parametrize("leak", LEAKS.values(), ids=LEAKS)
def test_box_run_refuses_any_value_of_the_token_file_outside_the_token_file(box, backup, leak, key):
    box.token()
    with_line(box, leak.replace("KEY", key))
    with pytest.raises(AssertionError, match="a value of the token file"):
        off(box, backup)


def test_the_script_is_fed_on_stdin_as_over_ssh(box, backup):
    # a command before the `case` that reads stdin takes the rest of the script, here all of it
    patched(box, "set -euo pipefail\n", 'set -euo pipefail\ncat > "$HOME/stdin-rest"\n')
    box.run("paper", backup)
    assert "r2-check)" in (box.root / "stdin-rest").read_text()


def test_the_working_directory_of_the_script_is_the_root_of_the_box(box, backup):
    with_line(box, 'pwd > "$HOME/cwd"')
    off(box, backup)
    assert (box.root / "cwd").read_text().strip() == str(box.root)


def test_the_fake_docker_counts_what_the_script_asked(box, backup):
    box.run("paper", backup)
    assert box.docker()["calls"] != []
    box.run("executor", backup, EXECUTOR_PATH, "--bogus", rc=2)
    assert box.docker()["calls"] == []


# --- the fake docker refuses the shapes the script does not use -------------------------------

EXEC = 'docker exec ${i[@]+"${i[@]}"} "$C" python -I -c "$CHALLENGE_PY" "$@"'
EXECS = {
    "into-the-production-challenge": EXEC.replace('"$C"', "cortex-challenge-hypertrain"),
    "without-the-isolated-mode": EXEC.replace(" -I", ""),
    "with-stdin-for-every-mode": EXEC.replace('${i[@]+"${i[@]}"}', "-i"),
}


@pytest.mark.parametrize("new", EXECS.values(), ids=EXECS)
def test_the_fake_docker_refuses_an_exec_of_another_shape(box, backup, new):
    patched(box, EXEC, new)
    box.token()
    with pytest.raises(AssertionError, match="(tripwire.log is not empty|exit)"):
        box.run(
            "executor", backup, EXECUTOR_PATH, "--r2", "/data/executor/r2.env", mutates={CHALLENGE}
        )


# --- the snapshot ---------------------------------------------------------------------------------


def test_the_snapshot_notices_a_file_the_script_should_not_have_written_anywhere(box, backup):
    box.token()
    for where in (
        box.state_dir,
        box.t,
        box.executor_dir,
        box.relay_json.parent,
        box.stub_json.parent,
        box.root,
    ):
        before = box.snapshot()
        (where / "stray").write_text("x")
        assert box.snapshot() != before, where
        (where / "stray").unlink()
        assert box.snapshot() == before


def test_the_snapshot_notices_the_mode_of_the_executor_directory_and_of_what_is_in_it(box, backup):
    box.token()
    box.settings.write_text("{}")
    for path in (box.executor_dir, box.settings, box.executor_dir / "r2.env"):
        before, mode = box.snapshot(), stat.S_IMODE(path.stat().st_mode)
        path.chmod(mode ^ 0o044)
        assert box.snapshot() != before, path.name
        path.chmod(mode)
        assert box.snapshot() == before


def test_the_snapshot_notices_the_content_of_a_file_of_the_executor_directory(box, backup):
    box.token()
    box.settings.write_text("{}")
    for path in (box.settings, box.executor_dir / "r2.env"):
        before, content = box.snapshot(), path.read_bytes()
        path.write_bytes(content + b"x")
        assert box.snapshot() != before, path.name
        path.write_bytes(content)
        assert box.snapshot() == before


def test_the_snapshot_notices_what_a_dry_run_leaves_behind(box, backup):
    before = box.snapshot()
    (box.t / "dry.abc123").mkdir()
    assert box.snapshot() != before
    (box.t / "dry.abc123").rmdir()
    (box.t / "compose.executor.yml.dry").write_text("x")
    assert box.snapshot() != before


def test_the_snapshot_notices_a_change_of_a_database_that_is_not_the_first(box, backup):
    before = box.snapshot()
    with (box.state_dir / "server_keys.db").open("ab") as f:
        f.write(b"x")
    assert box.snapshot() != before


# --- the helpers of test_executor_r2.py -------------------------------------------------------

MODE_REFUSAL = '  [ "${m% *}" = 600 ] || { echo "REFUSED $1: mode ${m% *}, want 600"; exit 4; }\n'


def test_refused_sees_a_refusal_that_writes(box, backup):
    box.token(mode=0o644)
    patched(box, MODE_REFUSAL, MODE_REFUSAL.replace("{ echo", '{ : > "$XD/stray"; echo'))
    with pytest.raises(AssertionError):
        refused(box, backup, 4)


def test_refused_sees_a_refusal_that_asked_the_challenge_first(box, backup):
    box.token()
    box.seal(age=4500)
    gate = "  if [ $dry = 1 ]; then (gate) || gate_rc=$?; else gate; fi # a dry run reports"
    patched(
        box,
        gate,
        '  if [ $dry = 1 ]; then (gate) || gate_rc=$?; elif [ -z "$r2" ]; then gate; fi'
        " # a dry run reports",
    )
    ask = (
        '    run_py -i pre "$P" <<< "$json" \\\n'
        '      || { echo "REFUSED: the challenge did not accept these settings,'
        ' or could not be asked; nothing written"; exit 4; }\n'
    )
    patched(box, ask, ask + "    [ $dry = 1 ] || gate\n")
    with pytest.raises(AssertionError):
        refused(box, backup, 3)


def test_token_state_sees_a_change_of_content_and_of_mode(box):
    box.token()
    (path,) = box.tokens
    state = token_state(box)
    path.chmod(0o640)
    assert token_state(box) != state
    path.chmod(0o600)
    assert token_state(box) == state
    path.write_text(path.read_text() + "R2_X=1\n")
    assert token_state(box) != state


def test_box_run_refuses_a_curl_without_a_timeout(box, backup):
    # a hung master would hang the operator's ssh: the fake accepts `-m N` calls only
    patched(box, 'ready=$(curl -sf -m 10 "$MASTER/readyz")', 'ready=$(curl -sf "$MASTER/readyz")')
    with pytest.raises(AssertionError):
        box.run(*call("executor", backup), mutates={CHALLENGE})


GUARDED = {  # id: a line a bug adds to executor-off; none changes the bytes of the token
    "touch": 'touch "$XD/r2.env"',
    "chmod-to-the-same-mode": 'chmod 0600 "$XD/r2.env"',
    "rewrite-of-the-same-bytes": 'sed -i "s/^NOPE=//" "$XD/r2.env"',
    "append": 'echo X=1 >> "$XD/r2.env"',
    "a-looser-mode": 'chmod 0640 "$XD/r2.env"',
    "the-executor-directory-opened": 'chmod 755 "$XD"',
}


@pytest.mark.parametrize("line", GUARDED.values(), ids=GUARDED)
def test_box_run_refuses_a_command_that_changes_a_token_file_or_the_executor_directory(
    box, backup, line
):
    box.token()
    with_line(box, line)
    with pytest.raises(AssertionError, match="token file or the executor directory changed"):
        off(box, backup)


def test_the_snapshot_notices_a_change_of_content_of_a_file_it_does_not_name(box, backup):
    before = box.snapshot()
    other = box.t / "compose.relay.yml"
    other.write_text(other.read_text() + "x")
    assert box.snapshot() != before


def test_the_fake_docker_logs_an_inspect_of_a_production_container_among_several(box):
    patched(
        box,
        'docker inspect "$C" hypertrain-test-beacon hypertrain-test-master-stub'
        " hypertrain-test-relay",
        'docker inspect "$C" hypertrain-test-beacon hypertrain-test-master-stub'
        " cortex-challenge-hypertrain",
    )
    with pytest.raises(AssertionError, match="tripwire.log is not empty"):
        box.run("backup", "T9")
