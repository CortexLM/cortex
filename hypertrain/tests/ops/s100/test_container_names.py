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


NEAR_MISSES = ["hypertrain-test-auditor-2", "hypertrain-test-challenge-1", "Hypertrain-Test-Beacon"]


@pytest.mark.parametrize("service", ["beacon", "master-stub"])
@pytest.mark.parametrize("name", NEAR_MISSES)
def test_a_name_one_token_from_a_test_name_is_refused(box, backup, service, name):
    # master-stub is checked against the six names only; beacon must also be exactly its own
    box.f.write_text(named(service, name))
    before = box.snapshot()
    out = box.run("beacon", backup, rc=4).out
    assert f"service {service} names container {name}" in out
    assert box.snapshot() == before


@pytest.mark.parametrize("cmd", ["rollback", "paper"])
def test_a_backup_compose_that_does_not_parse_is_refused_with_4(box, backup, cmd):
    (backup / "compose.test.yml").write_text("services:\n  challenge: [unclosed\n")
    box.reseal(backup)  # the checksums agree: only the compose check can refuse it
    before = box.snapshot()
    box.run(cmd, backup, rc=4)
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


def test_a_beacon_behind_a_profile_is_refused_because_up_would_still_start_it(box, backup):
    # compose config leaves a service whose profile is off out of its output, but
    # `up -d --no-deps beacon` enables that profile and recreates the container it names.
    gated = named("beacon", "hypertrain-beacon").replace(
        "    container_name: hypertrain-beacon\n",
        "    container_name: hypertrain-beacon\n    profiles: [debug]\n",
    )
    assert gated != COMPOSE
    box.f.write_text(gated)
    before = box.snapshot()
    out = box.run("beacon", backup, rc=4).out
    assert "service beacon must be hypertrain-test-beacon" in out
    assert box.snapshot() == before


@pytest.mark.parametrize("cmd", ["beacon", "rollback"])
def test_a_test_name_on_the_wrong_service_is_refused(box, backup, cmd):
    # hypertrain-test-relay is a TEST name, but the service `up` recreates is called beacon.
    wrong = named("beacon", "hypertrain-test-relay")
    if cmd == "beacon":
        box.f.write_text(wrong)
    else:
        (backup / "compose.test.yml").write_text(wrong)
        box.reseal(backup)
    before = box.snapshot()
    out = box.run(*call(cmd, backup), rc=4).out
    assert "service beacon must be hypertrain-test-beacon" in out
    assert box.snapshot() == before


def test_rollback_accepts_a_relative_backup_path(box, backup):
    # compose runs in the backups directory, so the name check must not resolve the path there.
    relative = backup.relative_to(box.root)
    box.run("rollback", relative, mutates={CHALLENGE, BEACON})
    assert box.container(CHALLENGE)["Image"] == OLD_ID
