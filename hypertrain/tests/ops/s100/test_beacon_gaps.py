"""s100_test/beacon_gaps.py on a challenge database: which rounds are missing, who waits for one."""

from __future__ import annotations

import json
import sqlite3
from pathlib import Path

import pytest
from scripts.ops.s100_test import beacon_gaps

WALL = beacon_gaps.GENESIS + 3 * 999 + 1  # the drand clock is at round 1000
RUN = "ab" * 32


def database(path: Path, rounds, admissions=()) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    db = sqlite3.connect(path)
    db.execute("CREATE TABLE beacon(round INTEGER PRIMARY KEY, signature TEXT, randomness TEXT)")
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
        "now": 999, "emitted": 1000, "lag": 1, "since": 1800, "first": 900,
        "missing": [], "missing_count": 0, "stranded": [],
    }  # fmt: skip


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
    database(tmp_path / "challenge.db", range(900, 1000))
    database(tmp_path / "runs" / RUN / "challenge.db", [*range(900, 950), *range(960, 1000)])
    assert run(capsys, tmp_path)[0] == 0
    code, out = run(capsys, tmp_path, "1800", RUN)
    assert code == 3 and out["missing_count"] == 10
    assert run(capsys, tmp_path, "1800", "cd" * 32)[0] == 0  # no such child: the root store


def test_an_empty_beacon_table_is_not_a_gap(tmp_path, capsys):
    database(tmp_path / "challenge.db", [])
    code, out = run(capsys, tmp_path)
    assert code == 0 and out["first"] is None and out["now"] == 0 and out["missing_count"] == 0


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
