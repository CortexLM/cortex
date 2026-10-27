"""challenge_image.sh refuses a compose file that names a container outside the TEST stack.

beacon and rollback run `docker compose up -d --no-deps` on the container_name values of the live
compose file or of the backup's: one token away from the TEST names is a production container.
"""

from __future__ import annotations

import pytest

from .sandbox import BEACON, CHALLENGE, COMPOSE, NEW_REF, OLD_ID, STUB, call

NAME = {"challenge": CHALLENGE, "beacon": BEACON, "master-stub": STUB}


def named(service: str, name: str | None) -> str:
    """COMPOSE with one service's container_name replaced (None: removed)."""
    line = f"    container_name: {NAME[service]}\n"
    return COMPOSE.replace(line, "" if name is None else f"    container_name: {name}\n")


BAD = {  # id: (command, which compose is bad, service, its container_name)
    "beacon-live-beacon-is-the-production-twin": ("beacon", "live", "beacon", "hypertrain-beacon"),
    "beacon-live-challenge-is-production": (
        "beacon",
        "live",
        "challenge",
        "cortex-challenge-hypertrain",
    ),
    "swap-live-beacon-is-the-production-twin": ("swap", "live", "beacon", "hypertrain-beacon"),
    "swap-live-stub-has-no-name": ("swap", "live", "master-stub", None),
    "swap-live-stub-looks-like-a-test-name": ("swap", "live", "master-stub", "hypertrain-test-x"),
    "rollback-backup-challenge-is-production": (
        "rollback",
        "backup",
        "challenge",
        "cortex-challenge-hypertrain",
    ),
    "rollback-backup-beacon-is-the-production-twin": (
        "rollback",
        "backup",
        "beacon",
        "hypertrain-beacon",
    ),
    "paper-backup-beacon-is-the-production-twin": (
        "paper",
        "backup",
        "beacon",
        "hypertrain-beacon",
    ),
}


@pytest.mark.parametrize(("cmd", "which", "service", "name"), BAD.values(), ids=BAD)
def test_a_compose_naming_a_container_outside_the_test_stack_is_refused(
    box, backup, cmd, which, service, name
):
    box.pull(NEW_REF)
    if which == "live":
        box.f.write_text(named(service, name))
    else:  # a backup whose checksums are consistent with the forged compose
        (backup / "compose.test.yml").write_text(named(service, name))
        box.reseal(backup)
    before = box.snapshot()
    out = box.run(*call(cmd, backup), rc=4).out
    assert f"service {service}" in out and (name or "no container_name") in out
    assert box.snapshot() == before


def test_all_six_test_names_are_accepted(box, backup):
    extra = "".join(
        f"  {n}:\n    <<: *common\n    container_name: hypertrain-test-{n}\n"
        for n in ("relay", "auditor-0", "auditor-1")
    )
    box.f.write_text(COMPOSE + extra)
    box.run("beacon", backup, mutates={BEACON})


def test_restore_db_runs_no_compose_so_it_ignores_container_names(box, backup):
    (backup / "compose.test.yml").write_text(named("beacon", "hypertrain-beacon"))
    box.reseal(backup)
    box.run("restore-db", backup, mutates={CHALLENGE})
    assert box.container(CHALLENGE)["Image"] == OLD_ID
