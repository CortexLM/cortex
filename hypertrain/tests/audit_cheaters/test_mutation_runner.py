"""The mutation runner of the audit-cheaters suite (scripts/check_audit_mutants.py).

A runner that reports every mutant killed would make the whole table worthless, so its verdicts are
proved on a toy tree first: a mutant its test notices is killed, one it does not is a survivor, a
stale snippet or a missing test is a problem of the table. The real table is checked against the
real tree: every snippet still occurs exactly once and every named test still exists.
"""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location(
    "check_audit_mutants", ROOT / "scripts" / "check_audit_mutants.py"
)
assert spec is not None and spec.loader is not None
runner = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = runner
spec.loader.exec_module(runner)

MODULE = "def limit(x):\n    return x if x < 10 else 10\n"
TESTS = (
    "import toy\n\n\n"
    "def test_values_above_the_limit_are_capped():\n"
    "    assert toy.limit(99) == 10\n\n\n"
    "def test_values_below_the_limit_pass():\n"
    "    assert toy.limit(3) == 3\n\n\n"
    "def test_an_unrelated_check():\n"
    "    assert toy.limit(0) == 0\n"
)


@pytest.fixture
def toy(tmp_path: Path) -> Path:
    (tmp_path / "src").mkdir()
    (tmp_path / "src" / "toy.py").write_text(MODULE)
    (tmp_path / "tests").mkdir()
    (tmp_path / "tests" / "test_toy.py").write_text(TESTS)
    return tmp_path


def mutant(name: str, old: str, new: str, test: str) -> object:
    return runner.Mutant(name, "src/toy.py", old, new, (f"tests/test_toy.py::{test}",))


def test_a_mutant_the_named_test_notices_is_killed(toy: Path, tmp_path_factory) -> None:
    m = mutant("cap removed", "else 10", "else x", "test_values_above_the_limit_are_capped")
    result, _ = runner.check(toy, m, tmp_path_factory.mktemp("scratch"))
    assert result == "killed"
    assert (toy / "src" / "toy.py").read_text() == MODULE


def test_a_mutant_no_named_test_notices_is_a_survivor(toy: Path, tmp_path_factory) -> None:
    m = mutant("cap removed", "else 10", "else x", "test_an_unrelated_check")
    result, _ = runner.check(toy, m, tmp_path_factory.mktemp("scratch"))
    assert result.startswith("SURVIVED")


def test_a_run_in_which_nothing_passes_is_not_a_survivor() -> None:
    assert runner.verdict(0, "3 skipped in 0.1s").startswith("NOT RUN")
    assert runner.verdict(5, "no tests ran").startswith("ERROR")


def test_the_table_problems_name_a_stale_snippet_a_missing_test_and_a_noop(toy: Path) -> None:
    stale = mutant("stale", "else 11", "else x", "test_values_below_the_limit_pass")
    missing = mutant("missing", "else 10", "else x", "test_that_was_renamed")
    noop = mutant("noop", "else 10", "else 10", "test_values_below_the_limit_pass")
    found = runner.problems([stale, missing, noop], toy)
    assert any("snippet found 0 times" in line for line in found)
    assert any("test_that_was_renamed does not exist" in line for line in found)
    assert any("changes nothing" in line for line in found)


def test_the_real_table_matches_the_real_tree() -> None:
    assert runner.problems(runner.mutants(), ROOT) == []
