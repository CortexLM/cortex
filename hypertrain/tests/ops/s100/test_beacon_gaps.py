"""s100_test/beacon_gaps.py on a challenge database: which rounds are missing, who waits for one."""

from __future__ import annotations

import json
import sqlite3
import subprocess
import sys
from pathlib import Path

import pytest
from scripts.ops.s100_test import beacon_gaps

WALL = beacon_gaps.GENESIS + 3 * 999 + 1  # the drand clock is at round 1000
RUN = "ab" * 32


def database(path: Path, rounds, admissions=(), runs=()) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    db = sqlite3.connect(path)
    db.execute("CREATE TABLE beacon(round INTEGER PRIMARY KEY, signature TEXT, randomness TEXT)")
    db.execute("CREATE TABLE runs(run_id TEXT PRIMARY KEY)")
    db.executemany("INSERT INTO runs VALUES(?)", [(r,) for r in runs])
    db.executemany("INSERT INTO beacon VALUES(?, 'aa', 'bb')", [(r,) for r in rounds])
    if admissions:
        db.execute(
            "CREATE TABLE admissions_v2(admission_id TEXT, state TEXT, seed_beacon INTEGER, "
            "hotkey TEXT)"
        )
        db.executemany("INSERT INTO admissions_v2 VALUES(?, ?, ?, 'secret-hotkey')", admissions)
    db.commit()
    db.close()
    return path


def run(capsys, data: Path, *args: str) -> tuple[int, dict | str]:
    code = beacon_gaps.main(list(args), data, WALL)
    out, err = capsys.readouterr()
    return code, json.loads(out) if out else err


def test_a_complete_window_is_reported_without_gaps(tmp_path, capsys):
    database(tmp_path / "challenge.db", range(900, 1000))
    code, out = run(capsys, tmp_path)
    assert code == 0
    assert out == {
        "now": 999,
        "emitted": 1000,
        "lag": 1,
        "gate_open": True,
        "since": 1800,
        "window_start": 400,
        "first": 900,
        "missing": [],
        "missing_count": 0,
        "stale": False,
        "stranded": [],
    }


def test_rounds_the_pusher_skipped_are_listed_and_the_identity_waiting_for_one_is_named(
    tmp_path, capsys
):
    held = [*range(900, 950), *range(956, 1000)]  # 950..955 were never pushed
    admissions = [
        ("a" * 40, "PROBATION", 953),  # waits for a round that will never come
        ("b" * 40, "ACTIVE", 960),  # its seed was pushed
        ("c" * 40, "SUSPENDED", 951),  # asks for no challenge
        ("d" * 40, "APPLIED", 955),
    ]
    database(tmp_path / "challenge.db", held, admissions)
    code, out = run(capsys, tmp_path)
    assert code == 3
    assert out["missing"] == [950, 951, 952, 953, 954, 955] and out["missing_count"] == 6
    assert out["stranded"] == [
        {"admission": "a" * 12, "state": "PROBATION", "seed_beacon": 953},
        {"admission": "d" * 12, "state": "APPLIED", "seed_beacon": 955},
    ]
    assert "secret-hotkey" not in json.dumps(out)


def test_only_the_window_asked_for_counts(tmp_path, capsys):
    held = [*range(500, 520), *range(900, 1000)]  # a hole far in the past
    database(tmp_path / "challenge.db", held)
    assert run(capsys, tmp_path, "300")[0] == 0  # 300 s = 100 rounds: from round 900
    code, out = run(capsys, tmp_path, "3600")
    assert code == 3 and out["missing_count"] == 380 and len(out["missing"]) == beacon_gaps.LISTED


def test_a_child_store_is_read_when_the_run_lives_in_one(tmp_path, capsys):
    database(tmp_path / "challenge.db", range(900, 1000), runs=[RUN])
    database(
        tmp_path / "runs" / RUN / "challenge.db", [*range(900, 950), *range(960, 1000)], runs=[RUN]
    )
    assert run(capsys, tmp_path)[0] == 0
    code, out = run(capsys, tmp_path, "1800", RUN)
    assert code == 3 and out["missing_count"] == 10


def test_a_child_directory_that_never_got_its_run_is_not_read(tmp_path, capsys):
    # store.py drops a runs/<id> whose `runs` table is empty: an aborted creation, a stale copy
    database(tmp_path / "challenge.db", range(900, 1000), runs=[RUN])
    database(tmp_path / "runs" / RUN / "challenge.db", range(10, 50))
    code, err = run(capsys, tmp_path, "1800", RUN)
    assert code == 4 and "is not hosted by" in err


def test_a_run_the_root_hosts_is_read_from_the_root_and_an_unknown_one_is_refused(tmp_path, capsys):
    database(tmp_path / "challenge.db", [*range(900, 950), *range(960, 1000)], runs=[RUN])
    code, out = run(capsys, tmp_path, "1800", RUN)  # no child store: the root hosts it
    assert code == 3 and out["missing_count"] == 10
    code, err = run(capsys, tmp_path, "1800", "cd" * 32)  # a mistyped id is not read from the root
    assert code == 4 and "is not hosted by" in err


def test_a_hole_at_the_start_of_the_window_counts_when_older_rounds_exist(tmp_path, capsys):
    # the window is rounds 900..999 (300 s); the table holds 850..899 too, then nothing until 910
    database(tmp_path / "challenge.db", [*range(850, 900), *range(910, 1000)])
    code, out = run(capsys, tmp_path, "300")
    assert code == 3 and out["window_start"] == 900
    assert out["missing"] == list(range(900, 910)) and out["first"] == 910
    # on a database that never held older rounds the same start is just where it began
    database(tmp_path / "fresh" / "challenge.db", range(910, 1000))
    assert run(capsys, tmp_path / "fresh", "300")[0] == 0


@pytest.mark.parametrize("seconds", ["1", "3", "9"])
def test_a_healthy_pusher_at_its_normal_lag_is_never_stale_whatever_the_window(
    tmp_path, capsys, seconds
):
    database(tmp_path / "challenge.db", range(900, 999))  # lag 2: the normal state
    code, out = run(capsys, tmp_path, seconds)
    assert code == 0 and out["stale"] is False and out["gate_open"] is True


def test_a_dead_pusher_is_stale_and_a_late_one_has_the_gate_closed(tmp_path, capsys):
    database(tmp_path / "challenge.db", range(100, 200))  # nothing for the last 800 rounds
    code, out = run(capsys, tmp_path)
    assert code == 3 and out["stale"] is True and out["gate_open"] is False and out["lag"] == 801
    database(tmp_path / "late" / "challenge.db", range(900, 996))  # complete, 5 rounds behind
    code, out = run(capsys, tmp_path / "late")
    assert code == 0 and out["gate_open"] is False and out["lag"] == 5 and out["stale"] is False
    database(tmp_path / "three" / "challenge.db", range(900, 998))  # lag 3: past the bound of 2
    assert run(capsys, tmp_path / "three")[1]["gate_open"] is False
    database(tmp_path / "edge" / "challenge.db", range(900, 999))  # lag 2: still open
    assert run(capsys, tmp_path / "edge")[1]["gate_open"] is True


def test_an_empty_beacon_table_is_not_a_gap(tmp_path, capsys):
    database(tmp_path / "challenge.db", [])
    code, out = run(capsys, tmp_path)
    assert code == 0 and out["first"] is None and out["now"] == 0 and out["missing_count"] == 0
    assert out["stale"] is False


def test_the_database_is_opened_read_only(tmp_path, capsys):
    path = database(tmp_path / "challenge.db", range(900, 1000))
    before = path.read_bytes()
    run(capsys, tmp_path)
    assert path.read_bytes() == before
    assert not list(tmp_path.glob("challenge.db-*"))  # no journal or wal left behind


@pytest.mark.parametrize(
    "args",
    [
        ("0",),
        ("86401",),
        ("abc",),
        ("-5",),
        ("1800", "not-hex"),
        ("1800", RUN, "extra"),
        ("1800", RUN.upper()),
    ],
    ids=[
        "zero",
        "over-a-day",
        "word",
        "negative",
        "run-not-hex",
        "extra-argument",
        "run-uppercase",
    ],
)
def test_bad_arguments_exit_4_and_read_nothing(tmp_path, capsys, args):
    database(tmp_path / "challenge.db", range(900, 1000))
    code, err = run(capsys, tmp_path, *args)
    assert code == 4 and "usage" in err


def test_a_missing_or_broken_database_exits_4(tmp_path, capsys):
    assert run(capsys, tmp_path)[0] == 4
    (tmp_path / "challenge.db").write_text("not a database")
    code, err = run(capsys, tmp_path)
    assert code == 4 and "cannot read the beacon table" in err


def test_the_drand_clock_is_quicknet_genesis_plus_three_seconds_per_round(tmp_path, capsys):
    database(tmp_path / "challenge.db", [7])
    genesis = 1692803367  # drand quicknet: round 1 opens at genesis, then one round every 3 s
    for wall, emitted in ((genesis, 1), (genesis + 2996, 999), (genesis + 2997, 1000)):
        beacon_gaps.main([], tmp_path, wall)  # stale or not, the clock is what is under test
        assert json.loads(capsys.readouterr().out)["emitted"] == emitted


@pytest.mark.parametrize("seconds", ["1", "86400"])
def test_the_smallest_and_the_largest_window_are_accepted_and_echoed(tmp_path, capsys, seconds):
    database(tmp_path / "challenge.db", range(900, 1001))  # round 1000 is the whole 1 s window
    code, out = run(capsys, tmp_path, seconds)
    assert code == 0 and out["since"] == int(seconds)


def test_the_first_forty_missing_rounds_are_listed_and_all_are_counted(tmp_path, capsys):
    database(tmp_path / "challenge.db", [900, *range(960, 1000)])  # 901..959 never pushed
    code, out = run(capsys, tmp_path)
    assert code == 3
    assert out["missing"] == list(range(901, 941)) and out["missing_count"] == 59


def test_an_identity_waiting_for_a_round_beyond_the_listed_ones_is_still_stranded(tmp_path, capsys):
    database(tmp_path / "challenge.db", [900, *range(960, 1000)], [("e" * 40, "PROBATION", 955)])
    code, out = run(capsys, tmp_path)
    assert 955 not in out["missing"] and out["missing_count"] == 59
    assert out["stranded"] == [{"admission": "e" * 12, "state": "PROBATION", "seed_beacon": 955}]


def test_active_waits_for_a_challenge_and_banned_does_not(tmp_path, capsys):
    admissions = [("a" * 40, "ACTIVE", 953), ("b" * 40, "BANNED", 954)]
    database(tmp_path / "challenge.db", [*range(900, 950), *range(956, 1000)], admissions)
    code, out = run(capsys, tmp_path)
    assert code == 3
    assert out["stranded"] == [{"admission": "a" * 12, "state": "ACTIVE", "seed_beacon": 953}]


@pytest.mark.parametrize(
    "run_id",
    [RUN[:-1], RUN + "0", RUN + "/../..", RUN + "\n"],
    ids=["63-hex", "65-hex", "trailing-path", "trailing-newline"],
)
def test_a_run_id_is_exactly_64_lowercase_hex_characters(tmp_path, capsys, run_id):
    database(tmp_path / "challenge.db", range(900, 1000))
    code, err = run(capsys, tmp_path, "1800", run_id)
    assert code == 4 and "usage" in err


def test_a_wal_store_is_read_without_folding_its_log_into_the_file(tmp_path, capsys):
    """challenge.db is in WAL mode: a read-write open would checkpoint the log and delete it."""
    live, data = tmp_path / "live", tmp_path / "data"
    live.mkdir()
    data.mkdir()
    writer = sqlite3.connect(live / "challenge.db")
    writer.execute("PRAGMA journal_mode=WAL")
    writer.execute("PRAGMA wal_autocheckpoint=0")
    writer.execute(
        "CREATE TABLE beacon(round INTEGER PRIMARY KEY, signature TEXT, randomness TEXT)"
    )
    writer.executemany("INSERT INTO beacon VALUES(?, 'aa', 'bb')", [(r,) for r in range(900, 1000)])
    writer.commit()
    for name in ("challenge.db", "challenge.db-wal"):  # the files as a crash would leave them
        (data / name).write_bytes((live / name).read_bytes())
    writer.close()
    before = (data / "challenge.db").read_bytes()
    code, out = run(capsys, data)
    assert code == 0 and out["now"] == 999  # the rows exist only in the log
    assert (data / "challenge.db").read_bytes() == before
    assert (data / "challenge.db-wal").is_file()


def test_the_report_is_one_json_line_with_sorted_keys(tmp_path, capsys):
    database(tmp_path / "challenge.db", range(900, 1000))
    beacon_gaps.main([], tmp_path, WALL)
    out = capsys.readouterr().out
    assert out.count("\n") == 1
    assert out == json.dumps(json.loads(out), sort_keys=True) + "\n"


@pytest.mark.parametrize(
    ("held", "first", "missing"),
    [
        ([900, *range(902, 1000)], 900, [901]),  # the round `since` ago is inside the window
        ([898, *range(900, 1000)], 900, []),  # the round before it is not
        ([899, *range(901, 1000)], 901, [900]),  # older rounds exist: the window's first is a hole
    ],
    ids=["first-round-inside", "round-before-outside", "first-round-missing"],
)
def test_the_window_opens_at_the_round_asked_for_seconds_ago(
    tmp_path, capsys, held, first, missing
):
    database(tmp_path / "challenge.db", held)
    code, out = run(capsys, tmp_path, "300")  # 300 s = 100 rounds before round 1000
    assert (out["first"], out["missing"]) == (first, missing)
    assert code == (3 if missing else 0)


@pytest.mark.skipif(
    Path("/data/challenge.db").exists(), reason="the script would read a real store"
)
def test_the_script_run_as_documented_ends_with_the_status_main_returns():
    script = Path(beacon_gaps.__file__)
    # docker exec -i hypertrain-test-challenge python -I - [SINCE_SECONDS] < beacon_gaps.py
    with script.open("rb") as source:
        done = subprocess.run(
            [sys.executable, "-I", "-", "1800"],
            stdin=source,
            capture_output=True,
            text=True,
            timeout=60,
            check=False,
        )
    assert done.returncode == 4
    assert "no challenge.db at /data/challenge.db" in done.stderr


def test_a_window_that_reaches_back_past_the_first_round_starts_at_round_one(tmp_path, capsys):
    database(tmp_path / "challenge.db", range(900, 1000))
    code, out = run(capsys, tmp_path, "86400")  # 28800 rounds before round 1000: there is no such
    assert code == 0 and out["window_start"] == 1  # round, the drand chain begins at 1
