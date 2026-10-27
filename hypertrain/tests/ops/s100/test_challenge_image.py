"""s100_test/challenge_image.sh through fake docker, curl and python3 (see sandbox.py).

Every run asserts the exit code, the exact set of containers that received a mutating docker call
(always a subset of the TEST stack) and that no ssh, sudo or unknown URL was touched.
"""

from __future__ import annotations

import json
import sqlite3
import stat

import pytest

from .sandbox import (
    API,
    BEACON,
    BEACON_ID,
    CHALLENGE,
    COMPOSE,
    EPOCH,
    EXECUTOR_PATH,
    EXECUTOR_VAR,
    MASTER,
    NEW_ID,
    NEW_REF,
    OLD_ID,
    OLD_REF,
    OTHER_ID,
    RELAY,
    REPO,
    SCRIPT_TEXT,
    SECRET,
    STUB,
    Box,
    call,
    digest_of,
    rows,
    sandboxed,
    tree,
)

CMDS = ("swap", "beacon", "paper", "rollback", "restore-db", "executor", "executor-off")
DB_FILES = ("challenge.db", "server_keys.db")


@pytest.mark.parametrize(
    "edit",
    [
        lambda t: t + "\ncp x /data/other/y\n",
        lambda t: t.replace("/data/relay", "/relay"),
        lambda t: t.replace(API, "http://127.0.0.1:8000"),  # the production challenge port
        lambda t: t.replace(MASTER, "http://127.0.0.1:8080"),
    ],
    ids=["a new host path", "a path the rewrite expects is gone", "API moved", "MASTER moved"],
)
def test_the_sandbox_refuses_a_script_whose_paths_or_addresses_it_does_not_pin(tmp_path, edit):
    with pytest.raises(AssertionError):
        sandboxed(edit(SCRIPT_TEXT), tmp_path)


def test_the_snapshot_notices_a_change_to_each_file_a_refusal_must_leave_alone(box, backup):
    # every refusal test ends on snapshot == before: a field it ignores hides a script that writes
    box.run(*call("executor", backup), mutates={CHALLENGE})  # an override is in place
    pending = box.override.with_name(box.override.name + ".new")
    tracked = [box.f, box.new, box.override, pending, box.relay_json, box.stub_json]
    for path in [*tracked, *sorted(box.state_dir.glob("*.db"))]:
        before = box.snapshot()
        original = path.read_bytes() if path.exists() else None
        path.write_bytes(b"changed" if original is None else original + b"x")
        assert box.snapshot() != before, path.name
        if original is None:
            path.unlink()
        else:
            path.write_bytes(original)
        assert box.snapshot() == before, path.name


# --- seal window: swap, beacon and executor go through the gate -----------------------------------

GATE = {  # id: (seal arguments, exit code)
    "age-1800": ({"age": 1800}, 3),
    "age-1801": ({"age": 1801}, 3),
    "age-negative": ({"age": -1}, 3),
    "age-half-a-second-in-the-future": ({"age": -0.5}, 3),
    "age-just-over-1800": ({"age": 1800.001}, 3),
    "age-7200": ({"age": 7200}, 3),
    "sealed-false": ({"sealed": False}, 3),
    "sealed-one": ({"sealed": 1}, 3),
    "sealed-string": ({"sealed": "true"}, 3),
    "sealed-null": ({"sealed": None}, 3),
    "epoch-is-now": ({"epoch": EPOCH}, 3),
    "epoch-two-behind": ({"epoch": EPOCH - 2}, 3),
    "epoch-ahead": ({"epoch": EPOCH + 1}, 3),
    "weights-garbage": ({"weights": "not json"}, 3),
    "weights-unreachable": ({"weights": False}, 3),
    "readyz-503": ({"ready": 503}, 2),
}


@pytest.mark.parametrize("cmd", ["swap", "beacon", "executor"])
@pytest.mark.parametrize(("seal", "rc"), GATE.values(), ids=GATE)
def test_a_closed_gate_refuses_before_any_change(box, backup, cmd, seal, rc):
    box.pull(NEW_REF)
    box.seal(**seal)
    before = box.snapshot()
    box.run(*call(cmd, backup), rc=rc)
    assert box.snapshot() == before


@pytest.mark.parametrize("age", [0, 1799, 1799.5])
def test_a_seal_younger_than_30_minutes_for_the_previous_epoch_opens_the_gate(box, backup, age):
    box.pull(NEW_REF)
    box.seal(age=age)
    box.run(*call("swap", backup), mutates={CHALLENGE})


def test_a_second_swap_replaces_the_image_line_of_the_first_instead_of_adding_one(box, backup):
    other = f"{REPO}@{OTHER_ID}"
    box.pull(NEW_REF)
    box.pull(other)
    box.seal(age=60)
    box.run(*call("swap", backup), mutates={CHALLENGE})
    box.run("swap", other, str(backup), mutates={CHALLENGE})
    lines = [x for x in box.f.read_text().splitlines() if x.startswith("    image: ")]
    assert lines == [f"    image: {other}"]  # a key defined twice is a compose parse error
    assert box.container(CHALLENGE)["Config"]["Image"] == other


# HT_GATE_MAX_AGE_S replaces the 30-min limit and nothing else of the gate.
LIMIT = {"HT_GATE_MAX_AGE_S": "4000"}


@pytest.mark.parametrize("cmd", ["swap", "beacon"])
@pytest.mark.parametrize("age", [1800, 3000, 3999.5])
def test_the_age_override_opens_a_gate_that_only_the_age_closed(box, backup, cmd, age):
    box.pull(NEW_REF)
    box.seal(age=age)
    before = box.snapshot()
    box.run(*call(cmd, backup), rc=3)  # without the setting the gate is closed
    assert box.snapshot() == before
    out = box.run(*call(cmd, backup), mutates={CHALLENGE if cmd == "swap" else BEACON}, env=LIMIT)
    assert "age limit overridden: HT_GATE_MAX_AGE_S=4000 (default 1800)" in out.out


OVERRIDDEN = {  # what the override must still refuse
    "age-at-the-new-limit": ({"age": 4000}, 3),
    "age-over-the-new-limit": ({"age": 4000.5}, 3),
    "age-negative": ({"age": -1}, 3),
    "sealed-false": ({"sealed": False}, 3),
    "sealed-string": ({"sealed": "true"}, 3),
    "epoch-is-now": ({"epoch": EPOCH}, 3),
    "epoch-two-behind": ({"epoch": EPOCH - 2}, 3),
    "weights-garbage": ({"weights": "not json"}, 3),
    "weights-unreachable": ({"weights": False}, 3),
    "readyz-503": ({"ready": 503}, 2),
}


@pytest.mark.parametrize("cmd", ["swap", "beacon"])
@pytest.mark.parametrize(("seal", "rc"), OVERRIDDEN.values(), ids=OVERRIDDEN)
def test_the_age_override_keeps_readyz_sealed_and_previous_epoch(box, backup, cmd, seal, rc):
    box.pull(NEW_REF)
    box.seal(**{"age": 3000, **seal})
    before = box.snapshot()
    box.run(*call(cmd, backup), rc=rc, env=LIMIT)
    assert box.snapshot() == before


@pytest.mark.parametrize("value", ["abc", "0", "-5", "4321", "99999", "1e3", "1800 ", "07", "1.5"])
def test_a_malformed_age_override_is_refused_before_any_call(box, backup, value):
    box.pull(NEW_REF)
    box.seal(age=60)
    before = box.snapshot()
    out = box.run(*call("swap", backup), rc=4, env={"HT_GATE_MAX_AGE_S": value}).out
    assert "HT_GATE_MAX_AGE_S must be 1..4320 seconds" in out
    assert box.snapshot() == before
    assert box.polls(f"{MASTER}/readyz") == 0


def test_the_default_gate_does_not_mention_an_override(box, backup):
    box.pull(NEW_REF)
    box.seal(age=60)
    assert "overridden" not in box.run(*call("swap", backup), mutates={CHALLENGE}).out


# --- backup validity ------------------------------------------------------------------------


@pytest.mark.parametrize("cmd", CMDS)
@pytest.mark.parametrize("missing", ["compose.test.yml", *DB_FILES, "SHA256SUMS"])
def test_a_backup_missing_a_required_file_is_refused(box, backup, cmd, missing):
    box.pull(NEW_REF)
    (backup / missing).unlink()
    before = box.snapshot()
    out = box.run(*call(cmd, backup), rc=4).out
    assert f"not a backup dir: {backup} (no {missing})" in out
    assert box.snapshot() == before


@pytest.mark.parametrize("cmd", CMDS)
@pytest.mark.parametrize("altered", ["compose.test.yml", "challenge.db"])
def test_a_backup_that_fails_its_checksums_is_refused(box, backup, cmd, altered):
    box.pull(NEW_REF)
    with (backup / altered).open("ab") as f:
        f.write(b"\n# altered\n")
    before = box.snapshot()
    assert "fails SHA256SUMS" in box.run(*call(cmd, backup), rc=4).out
    assert box.snapshot() == before


# --- backup ---------------------------------------------------------------------------------


def test_backup_writes_a_verified_snapshot_without_env_values(box: Box) -> None:
    out = box.run("backup", "T1").out
    b = box.b()
    listed = dict(line.split("  ", 1)[::-1] for line in (b / "SHA256SUMS").read_text().splitlines())
    assert sorted(listed) == [
        "challenge.db",
        "compose.relay.yml",
        "compose.test.yml",
        "inspect-before.json",
        "metagraph.json",
        "relay.json",
        "server_keys.db",
    ]
    assert all(digest_of(b / name) == digest for name, digest in listed.items())
    assert stat.S_IMODE(b.stat().st_mode) == 0o700
    for name in DB_FILES:
        assert stat.S_IMODE((b / name).stat().st_mode) == 0o600
        db = sqlite3.connect(b / name)
        assert db.execute("pragma integrity_check").fetchone() == ("ok",)
        db.close()
    assert rows(b / "challenge.db") == ["original"] and rows(b / "server_keys.db") == ["key-1"]
    assert (b / "relay.json").read_bytes() == box.relay_json.read_bytes()
    assert (b / "metagraph.json").read_bytes() == box.stub_json.read_bytes()
    inspected = json.loads((b / "inspect-before.json").read_text())
    assert [c["Name"] for c in inspected] == [f"/{n}" for n in (CHALLENGE, BEACON, STUB, RELAY)]
    assert all(c["Config"]["Env"] == ["HYPERTRAIN_ADMIN_TOKEN", "TZ"] for c in inspected)
    assert SECRET not in out
    assert not any(SECRET in p.read_text(errors="ignore") for p in b.iterdir())


def test_backup_never_overwrites_an_existing_backup(box: Box) -> None:
    box.run("backup", "T1")
    before = tree(box.b())
    assert "exists:" in box.run("backup", "T1", rc=4).out
    assert tree(box.b()) == before


def test_backup_fails_when_a_database_copy_fails_its_integrity_check(box: Box) -> None:
    db = sqlite3.connect(box.state_dir / "challenge.db")
    db.executemany("insert into runs(v) values (?)", [(f"r{i}",) for i in range(20)])
    db.execute("create index runs_v on runs(v)")
    db.commit()
    db.execute("pragma writable_schema=on")  # orphan the index pages: readable, but not "ok"
    db.execute("delete from sqlite_master where name='runs_v'")
    db.commit()
    db.close()
    out = box.run("backup", "T1", rc=6).out
    assert "DB backup not ok" in out and "never used" in out


# --- swap -----------------------------------------------------------------------------------


def test_swap_pins_the_new_image_and_recreates_only_the_challenge(box, backup):
    box.pull(NEW_REF)
    original = tree(backup)
    out = box.run(*call("swap", backup), mutates={CHALLENGE}).out
    pin = f"container_name: {CHALLENGE}\n"
    assert box.f.read_text() == COMPOSE.replace(pin, f"{pin}    image: {NEW_REF}\n")
    assert box.container(CHALLENGE)["Image"] == NEW_ID
    assert box.container(CHALLENGE)["Config"]["Image"] == NEW_REF
    assert box.container(BEACON)["Image"] == BEACON_ID
    assert f"challenge {NEW_REF}" in out and "public runs sha" in out
    assert stat.S_IMODE(box.f.stat().st_mode) == 0o600
    assert not box.new.exists()
    assert (backup / "compose.test.yml.before-challenge").read_text() == COMPOSE
    assert {k: v for k, v in tree(backup).items() if k in original} == original


BAD_REFS = {
    "tag": f"{REPO}:latest",
    "other-repository": f"ghcr.io/evil/hypertrain@{NEW_ID}",
    "other-registry": f"registry.evil/{NEW_REF}",
    "newline-then-privileged": f"{NEW_REF}\n    privileged: true",
    "command-substitution": f"{NEW_REF}$(touch pwned)",
    "dot-of-the-registry-is-literal": f"ghcrXio/cortexlm/cortex/hypertrain@{NEW_ID}",
}


@pytest.mark.parametrize("ref", BAD_REFS.values(), ids=BAD_REFS)
def test_swap_refuses_a_ref_that_is_not_a_digest_of_our_repository(box, backup, ref):
    box.pull(ref)  # present locally, so only the ref check can refuse it
    before = box.snapshot()
    assert "bad ref" in box.run("swap", ref, backup, rc=4).out
    assert box.snapshot() == before


def test_swap_refuses_an_image_that_was_not_pulled(box, backup):
    before = box.snapshot()
    assert "image not pulled" in box.run(*call("swap", backup), rc=4).out
    assert box.snapshot() == before


@pytest.mark.parametrize(
    "name",
    ["cortex-challenge-hypertrain", "hypertrain-test-auditor-0"],
    ids=["production", "another-test-name"],
)
def test_swap_refuses_a_challenge_that_is_not_the_test_container(box, backup, name):
    # the second name passes the six-name check, so only the anchor (exit 4, not 5) refuses it
    box.pull(NEW_REF)
    box.f.write_text(COMPOSE.replace(f"name: {CHALLENGE}", f"name: {name}"))
    before = box.snapshot()
    assert "anchor" in box.run(*call("swap", backup), rc=4).out
    assert box.snapshot() == before


def test_a_failed_compose_up_exits_nonzero_and_rollback_recovers(box, backup):
    box.pull(NEW_REF)
    original = tree(backup)
    box.docker(fail_up=True)
    box.run(*call("swap", backup), rc=1, mutates={CHALLENGE})
    assert {k: v for k, v in tree(backup).items() if k in original} == original
    box.docker(fail_up=False)
    box.run("rollback", backup, mutates={CHALLENGE, BEACON})
    assert box.container(CHALLENGE)["Image"] == OLD_ID
    assert box.f.read_bytes() == (backup / "compose.test.yml").read_bytes()


@pytest.mark.parametrize(
    ("fault", "message"), [("version-503", "/version not 2xx"), ("not-recreated", "not on")]
)
def test_swap_fails_unless_the_challenge_answers_and_runs_the_new_image(
    box, backup, fault, message
):
    box.pull(NEW_REF)
    if fault == "version-503":
        box.url(f"{API}/version", 503)
    else:
        box.docker(stale_up=True)
    assert message in box.run(*call("swap", backup), rc=7, mutates={CHALLENGE}).out


def test_swap_gives_the_challenge_the_whole_30_attempt_loop_to_answer(box, backup):
    box.pull(NEW_REF)
    box.url(f"{API}/version", body="{}", down_first=30)
    box.run(*call("swap", backup), mutates={CHALLENGE})
    assert box.polls(f"{API}/version") > 30


# --- beacon ---------------------------------------------------------------------------------


def test_beacon_retunes_exactly_the_pusher_and_a_second_run_is_refused(box, backup):
    out = box.run("beacon", backup, mutates={BEACON}).out
    assert "beacon: pushed round 1" in out and "lag" in out
    retuned = COMPOSE.replace('--interval, "3"', '--interval, "0.2"').replace(
        '--catch-up, "20"', '--catch-up, "3"'
    )
    assert box.f.read_text() == retuned
    entrypoint = box.live_yaml()["services"]["beacon"]["entrypoint"]
    assert (entrypoint[9], entrypoint[11]) == ("0.2", "3")
    assert stat.S_IMODE(box.f.stat().st_mode) == 0o600
    out = box.run("beacon", backup, rc=5).out
    assert "unexpected compose change" in out
    assert box.f.read_text() == retuned and not box.new.exists()


def test_beacon_refuses_a_compose_where_the_retune_would_change_more_than_the_pusher(box, backup):
    # the master-stub carries the same two lines the sed rewrites: the diff then has 4 fields, not 2
    stub = (
        "    entrypoint:\n"
        '      [python, -m, stub, --interval, "3",\n'
        '       --catch-up, "20", --max-failures, "5"]\n'
    )
    pin = f"container_name: {STUB}\n"
    live = COMPOSE.replace(pin, f"{pin}{stub}")
    assert live != COMPOSE
    box.f.write_text(live)
    before = box.snapshot()
    out = box.run("beacon", backup, rc=5).out
    assert "unexpected compose change" in out and "services.master-stub.entrypoint" in out
    assert box.f.read_text() == live
    assert box.snapshot() == before and not box.new.exists()


# --- paper ----------------------------------------------------------------------------------


def test_paper_prints_the_backed_up_image_and_changes_nothing(box, backup):
    box.pull(NEW_REF)
    box.run(*call("swap", backup), mutates={CHALLENGE})
    before = box.snapshot()
    out = box.run("paper", backup).out
    assert f"challenge {OLD_REF}" in out and f"live: {NEW_REF}" in out
    assert f"challenge {NEW_REF}" not in out
    assert box.snapshot() == before


# --- rollback -------------------------------------------------------------------------------


def test_rollback_restores_the_compose_and_both_containers_with_the_gate_closed(box, backup):
    box.pull(NEW_REF)
    box.run(*call("swap", backup), mutates={CHALLENGE})
    box.run("beacon", backup, mutates={BEACON})
    box.seal(age=7200, ready=503)  # rollback and restore-db are the way back: never gated
    out = box.run("rollback", backup, mutates={CHALLENGE, BEACON}).out
    assert box.f.read_bytes() == (backup / "compose.test.yml").read_bytes()
    assert box.container(CHALLENGE)["Image"] == OLD_ID and "public runs sha" in out


def test_rollback_puts_the_challenge_back_on_the_image_its_own_entry_recorded(box, backup):
    box.pull(NEW_REF)
    box.run(*call("swap", backup), mutates={CHALLENGE})
    recorded = {
        c["Name"]: c["Image"] for c in json.loads((backup / "inspect-before.json").read_text())
    }
    # the two entries differ, so reading the wrong one cannot land on the right id by luck
    assert (recorded[f"/{CHALLENGE}"], recorded[f"/{BEACON}"]) == (OLD_ID, BEACON_ID)
    box.run("rollback", backup, mutates={CHALLENGE, BEACON})
    assert box.container(CHALLENGE)["Image"] == recorded[f"/{CHALLENGE}"]


def test_rollback_works_from_a_backup_directory_whose_path_has_a_space(box, golden):
    spaced = box.seed_backup(golden, "T 1")
    assert " " in str(spaced)
    box.run("rollback", spaced, mutates={CHALLENGE, BEACON})
    assert box.f.read_bytes() == (spaced / "compose.test.yml").read_bytes()


@pytest.mark.parametrize(
    ("relay", "stub"),
    [(True, False), (False, True), (True, True)],
    ids=["relay-only", "stub-only", "both"],
)
def test_rollback_restores_a_changed_relay_or_stub_file_and_restarts_only_that_container(
    box, backup, relay, stub
):
    changed = {}
    if relay:
        box.relay_json.write_text("changed\n")
        changed[RELAY] = (box.relay_json, backup / "relay.json")
    if stub:
        box.stub_json.write_text("changed\n")
        changed[STUB] = (box.stub_json, backup / "metagraph.json")
    box.run("rollback", backup, mutates={CHALLENGE, BEACON, *changed})
    for live, saved in changed.values():
        assert live.read_bytes() == saved.read_bytes()


@pytest.mark.parametrize(
    ("fault", "rc", "mutates", "message"),
    [
        ("fail_up", 1, {CHALLENGE}, "cannot start"),
        ("stale_up", 7, {CHALLENGE, BEACON}, f"challenge not on {OLD_ID}"),
    ],
)
def test_rollback_fails_unless_the_challenge_is_back_on_the_old_image(
    box, backup, fault, rc, mutates, message
):
    box.pull(NEW_REF)
    box.run(*call("swap", backup), mutates={CHALLENGE})
    box.docker(**{fault: True})
    assert message in box.run("rollback", backup, rc=rc, mutates=mutates).out


# --- executor, executor-off -----------------------------------------------------------------


def test_executor_adds_the_variable_to_the_challenge_alone_and_executor_off_drops_it(box, backup):
    original = tree(backup)
    out = box.run(*call("executor", backup), mutates={CHALLENGE}).out
    assert box.env(CHALLENGE)[EXECUTOR_VAR] == EXECUTOR_PATH
    assert box.env(CHALLENGE)["TZ"] == "UTC"  # the rest of its environment is kept
    assert EXECUTOR_VAR not in box.env(BEACON)
    assert f"changed ['services.challenge.environment.{EXECUTOR_VAR}']" in out
    assert box.override.read_text() == (
        f"services:\n  challenge:\n    environment:\n      {EXECUTOR_VAR}: {EXECUTOR_PATH}\n"
    )
    assert stat.S_IMODE(box.override.stat().st_mode) == 0o600
    assert box.f.read_text() == COMPOSE  # the base file is never edited
    assert not box.override.with_name("compose.executor.yml.new").exists()
    assert {k: v for k, v in tree(backup).items() if k in original} == original

    box.run("executor-off", backup, mutates={CHALLENGE})
    assert not box.override.exists()
    assert EXECUTOR_VAR not in box.env(CHALLENGE) and box.env(CHALLENGE)["TZ"] == "UTC"
    assert box.f.read_text() == COMPOSE


def test_executor_and_executor_off_bring_up_the_live_compose_not_the_backed_up_one(box, backup):
    box.pull(NEW_REF)
    box.run(*call("swap", backup), mutates={CHALLENGE})  # the live file now pins the new image
    box.run(*call("executor", backup), mutates={CHALLENGE})
    assert box.container(CHALLENGE)["Image"] == NEW_ID and EXECUTOR_VAR in box.env(CHALLENGE)
    box.run("executor-off", backup, mutates={CHALLENGE})
    assert box.container(CHALLENGE)["Image"] == NEW_ID and EXECUTOR_VAR not in box.env(CHALLENGE)


SET_BY_FILE = ("TZ=Europe/Paris", "KEEP=keepme", f"{EXECUTOR_VAR}=/data/old.json")
ALREADY_SET = {  # id: the environment of the live challenge service, which sets the variable
    "mapping": "".join("      " + e.replace("=", ": ") + "\n" for e in SET_BY_FILE),
    "list": "".join(f"      - {e}\n" for e in SET_BY_FILE),
}


@pytest.mark.parametrize("environment", ALREADY_SET.values(), ids=ALREADY_SET)
def test_the_override_wins_over_a_value_the_live_file_already_sets(box, backup, environment):
    pin = f"container_name: {CHALLENGE}\n"
    box.f.write_text(COMPOSE.replace(pin, f"{pin}    environment:\n{environment}"))
    box.run(*call("executor", backup), mutates={CHALLENGE})
    env = box.env(CHALLENGE)
    # the later file wins the variable, compose beats the image environment, the rest is kept
    assert (env[EXECUTOR_VAR], env["TZ"], env["KEEP"]) == (EXECUTOR_PATH, "Europe/Paris", "keepme")


def test_executor_refuses_a_merge_that_changes_more_than_the_variable(box, backup):
    # the names pass, so only the check that the merge differs in the variable alone can refuse
    box.docker(merge_patch={"challenge": {"privileged": True}})
    before = box.snapshot()
    out = box.run(*call("executor", backup), rc=5).out
    assert "unexpected compose change" in out and "services.challenge.privileged" in out
    assert box.snapshot() == before


BAD_PATHS = {
    "relative": "data/x.json",
    "outside-data": "/etc/x.json",
    "under-another-root": "/etc/data/x.json",
    "not-json": "/data/x.txt",
    "json-in-the-middle": "/data/x.json.txt",
    "the-dot-is-literal": "/data/x_json",
    "space": "/data/x y.json",
    "quote": '/data/x".json',
    "yaml-injection": "/data/x.json\n    privileged: true\n#.json",
}


@pytest.mark.parametrize("path", BAD_PATHS.values(), ids=BAD_PATHS)
def test_executor_refuses_a_path_that_is_not_a_json_file_under_data(box, backup, path):
    before = box.snapshot()
    out = box.run("executor", backup, path, rc=4).out
    assert "PATH must be /data/....json" in out
    assert box.snapshot() == before


# --- restore-db -----------------------------------------------------------------------------


def test_restore_db_keeps_the_live_dbs_aside_and_installs_the_backed_up_ones(box, backup):
    live = box.state_dir
    db = sqlite3.connect(live / "challenge.db")
    db.execute("insert into runs(v) values ('late')")
    db.commit()
    db.close()
    for n in DB_FILES:
        (live / f"{n}-wal").write_bytes(b"stale wal")
        (live / f"{n}-shm").write_bytes(b"stale shm")
    box.seal(age=7200, ready=503)
    changed = digest_of(live / "challenge.db")
    run = box.run("restore-db", backup, mutates={CHALLENGE})
    assert "public runs sha" in run.out
    (aside,) = backup.glob("restore-db-aside-*")
    assert stat.S_IMODE(aside.stat().st_mode) == 0o700
    assert rows(aside / "challenge.db") == ["original", "late"]
    assert (aside / "challenge.db-wal").read_bytes() == b"stale wal"
    assert rows(live / "challenge.db") == ["original"]
    assert rows(live / "server_keys.db") == ["key-1"]
    assert sorted(p.name for p in live.iterdir()) == list(DB_FILES)
    for n in DB_FILES:  # not pinned to a mode: only that group and others cannot write the DBs
        assert stat.S_IMODE((live / n).stat().st_mode) & 0o022 == 0
    (stop, _, stopped), (start, _, started) = run.mutations
    assert (stop, start) == ("stop", "start")
    assert stopped["db"] == changed and "challenge.db-wal" in stopped["files"]
    assert started["db"] == digest_of(backup / "challenge.db")
    assert started["files"] == list(DB_FILES)
    assert box.container(CHALLENGE)["State"]["Status"] == "running"
