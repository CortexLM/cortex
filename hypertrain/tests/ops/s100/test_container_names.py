"""challenge_image.sh refuses a compose file that names a container outside the TEST stack.

beacon, rollback, executor and executor-off run `docker compose up -d --no-deps` on the
container_name values of the live compose file or of the backup's: one token away from the TEST
names is a production container.
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


# --- executor and executor-off: the live file, and for executor the merged config -------------


def extra(name: str | None) -> str:
    """COMPOSE plus a fourth service, which `up -d --no-deps challenge` would leave alone."""
    line = "" if name is None else f"    container_name: {name}\n"
    return f"{COMPOSE}  extra:\n    <<: *common\n{line}"


def behind_a_profile(service: str) -> str:
    """COMPOSE with the service under its own right name but in a profile that is off."""
    line = f"    container_name: {NAME[service]}\n"
    return COMPOSE.replace(line, f"{line}    profiles: [debug]\n")


def names(service: str, name: str) -> str:
    return f"service {service} names container {name}"


def unnamed(service: str) -> str:
    return f"service {service} declares no container_name"


def must_be(service: str) -> str:
    return f"service {service} must be {NAME[service]}"


LIVE = {  # id: (live compose, the line the refusal prints)
    "challenge-is-production": (
        named("challenge", "cortex-challenge-hypertrain"),
        names("challenge", "cortex-challenge-hypertrain"),
    ),
    "challenge-has-no-name": (named("challenge", None), unnamed("challenge")),
    # a TEST name, but not the container this `up` recreates
    "challenge-wears-the-relay-name": (
        named("challenge", "hypertrain-test-relay"),
        must_be("challenge"),
    ),
    # compose config leaves it out, up -d --no-deps challenge would still start it
    "challenge-is-behind-a-profile": (behind_a_profile("challenge"), must_be("challenge")),
    "beacon-is-the-production-twin": (
        named("beacon", "hypertrain-beacon"),
        names("beacon", "hypertrain-beacon"),
    ),
    "stub-has-no-name": (named("master-stub", None), unnamed("master-stub")),
    "extra-service-is-a-production-container": (
        extra("hypertrain-auditor-0"),
        names("extra", "hypertrain-auditor-0"),
    ),
    "extra-service-has-no-name": (extra(None), unnamed("extra")),
}


@pytest.mark.parametrize("cmd", ["executor", "executor-off"])
@pytest.mark.parametrize(("text", "refusal"), LIVE.values(), ids=LIVE)
def test_the_executor_commands_refuse_a_live_compose_naming_a_container_outside_the_test_stack(
    box, backup, cmd, text, refusal
):
    box.run(*call("executor", backup), mutates={CHALLENGE})  # leaves an override in place
    box.f.write_text(text)
    before = box.snapshot()
    assert before["override"] is not None
    out = box.run(*call(cmd, backup), rc=4).out
    assert f"REFUSED {box.f}: {refusal}" in out
    assert box.snapshot() == before  # no file written or removed, the override still in place


def test_executor_off_refuses_a_live_compose_that_does_not_parse(box, backup):
    box.run(*call("executor", backup), mutates={CHALLENGE})
    box.f.write_text("services:\n  challenge: [unclosed\n")
    before = box.snapshot()
    box.run("executor-off", backup, rc=4)
    assert box.snapshot() == before


def test_executor_refuses_when_only_the_merged_config_names_another_container(box, backup):
    # the live file is fine; the fake compose resolves it together with the override differently
    box.docker(merge_patch={"challenge": {"container_name": "cortex-challenge-hypertrain"}})
    before = box.snapshot()
    out = box.run(*call("executor", backup), rc=4).out
    refused = names("challenge", "cortex-challenge-hypertrain")
    assert f"REFUSED {box.f} -f {box.override}.new: {refused}" in out
    assert box.snapshot() == before


@pytest.mark.parametrize("cmd", ["executor", "executor-off"])
def test_the_live_file_is_checked_on_its_own_even_when_a_merge_would_hide_it(box, backup, cmd):
    # swap, rollback and executor-off bring the file up alone, whatever the pair resolves to
    box.run(*call("executor", backup), mutates={CHALLENGE})
    box.f.write_text(named("beacon", "hypertrain-beacon"))
    box.docker(merge_patch={"beacon": {"container_name": BEACON}})
    before = box.snapshot()
    out = box.run(*call(cmd, backup), rc=4).out
    assert f"REFUSED {box.f}: {names('beacon', 'hypertrain-beacon')}" in out
    assert box.snapshot() == before
