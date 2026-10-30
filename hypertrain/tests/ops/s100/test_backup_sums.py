"""challenge_image.sh only trusts a backup whose SHA256SUMS is complete, not just consistent.

`sha256sum -c` alone verifies the files a truncated SHA256SUMS still lists and ignores the rest:
a `backup` that died while writing the sums, plus a corrupt challenge.db, used to pass.
"""

from __future__ import annotations

import shutil
import subprocess

import pytest

from .sandbox import BEACON, CHALLENGE, COMPOSE, NEW_REF, call, digest_of

CMDS = ("swap", "beacon", "paper", "rollback", "restore-db")


def lines(backup):
    return (backup / "SHA256SUMS").read_text().splitlines(keepends=True)


def without(sums, name):
    return "".join(s for s in sums if not s.endswith(f"  {name}\n"))


TRUNCATED = {  # id: (new SHA256SUMS from the old lines, file corrupted at the same time, message)
    "two-lines": (lambda sums: "".join(sums[:2]), "challenge.db", "challenge.db is not listed"),
    "cut-in-the-middle-of-a-line": (
        lambda sums: "".join(sums[:2]) + sums[2][:30],
        "challenge.db",
        "fails SHA256SUMS",
    ),
    "everything-but-challenge-db": (
        lambda sums: without(sums, "challenge.db"),
        "challenge.db",
        "challenge.db is not listed",
    ),
    "everything-but-server-keys-db": (
        lambda sums: without(sums, "server_keys.db"),
        "server_keys.db",
        "server_keys.db is not listed",
    ),
    "complete-then-a-malformed-line": (
        lambda sums: "".join(sums) + "3f9a\n",
        None,
        "fails SHA256SUMS",
    ),
}


@pytest.mark.parametrize("cmd", CMDS)
@pytest.mark.parametrize(("cut", "corrupt", "message"), TRUNCATED.values(), ids=TRUNCATED)
def test_a_truncated_or_malformed_sums_file_is_refused(box, backup, cmd, cut, corrupt, message):
    box.pull(NEW_REF)
    (backup / "SHA256SUMS").write_text(cut(lines(backup)))
    if corrupt:
        (backup / corrupt).write_bytes(b"not a database")
    before = box.snapshot()
    # nothing was touched: the live DBs are intact and no container received a mutating call
    assert message in box.run(*call(cmd, backup), rc=4).out
    assert box.snapshot() == before


@pytest.mark.parametrize(
    "unlisted", ["compose.relay.yml", "relay.json", "metagraph.json", "inspect-before.json"]
)
@pytest.mark.parametrize("content", ["forged\n", ""], ids=["forged", "zero-byte"])
def test_rollback_refuses_a_file_it_would_use_that_the_sums_do_not_list(
    box, backup, unlisted, content
):
    # a zero-byte file is what a cut-short copy leaves: it is still a file the script would use
    (backup / unlisted).write_text(content)
    (backup / "SHA256SUMS").write_text(without(lines(backup), unlisted))
    before = box.snapshot()
    assert f"{unlisted} is not listed" in box.run("rollback", backup, rc=4).out
    assert box.snapshot() == before


@pytest.mark.parametrize("other", ["compose.test.yml.before-challenge", "compose-test-yml"])
def test_a_listed_file_whose_name_only_resembles_a_required_name_does_not_count(box, backup, other):
    shutil.copy(backup / "compose.test.yml", backup / other)
    sums = without(lines(backup), "compose.test.yml")
    (backup / "SHA256SUMS").write_text(f"{sums}{digest_of(backup / other)}  {other}\n")
    (backup / "compose.test.yml").write_text(COMPOSE + "# forged\n")
    before = box.snapshot()
    assert "compose.test.yml is not listed" in box.run("rollback", backup, rc=4).out
    assert box.snapshot() == before


def test_a_legacy_backup_without_relay_and_stub_files_is_accepted(box, backup):
    # the first versions of `backup` listed these five files and took no relay.json/metagraph.json
    legacy = "compose.test.yml compose.relay.yml challenge.db server_keys.db inspect-before.json"
    (backup / "relay.json").unlink()
    (backup / "metagraph.json").unlink()
    (backup / "SHA256SUMS").write_text(
        "".join(f"{digest_of(backup / n)}  {n}\n" for n in legacy.split())
    )
    box.pull(NEW_REF)
    box.run(*call("swap", backup), mutates={CHALLENGE, BEACON})
    box.run("rollback", backup, mutates={CHALLENGE, BEACON})


def test_sums_written_in_binary_mode_are_accepted(box, backup):
    names = [s.split("  ", 1)[1].strip() for s in lines(backup)]
    binary = subprocess.run(
        ["sha256sum", "-b", *names], cwd=backup, capture_output=True, text=True, check=True
    )
    assert f" *{names[0]}" in binary.stdout
    (backup / "SHA256SUMS").write_text(binary.stdout)
    box.run("rollback", backup, mutates={CHALLENGE, BEACON})


def test_files_added_by_earlier_commands_are_not_part_of_the_check(box, backup):
    box.pull(NEW_REF)
    box.run(*call("swap", backup), mutates={CHALLENGE, BEACON})
    box.run("restore-db", backup, mutates={CHALLENGE})
    names = {p.name for p in backup.iterdir()}
    assert "config-before-challenge.json" in names and "compose.test.yml.before-challenge" in names
    assert any(n.startswith("restore-db-aside-") for n in names)
    box.run("rollback", backup, mutates={CHALLENGE, BEACON})
