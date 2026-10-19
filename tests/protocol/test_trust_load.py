from hashlib import sha256
from pathlib import Path

import pytest

from cortex.protocol import ProtocolError
from cortex.protocol.trust import load_trust, trust_from_json, trust_to_json

CHALLENGES = """
version = 3
introduced_epoch = {epoch}

[[challenges]]
id = "bounty"
emission_share_bps = 3000
policy = "all_metagraph_hotkeys"

[[challenges]]
id = "proof"
emission_share_bps = 7000
policy = "all_metagraph_hotkeys"
"""


def files(tmp_path: Path, epoch: str) -> dict[str, Path]:
    (tmp_path / "c.toml").write_text(CHALLENGES.format(epoch=epoch))
    (tmp_path / "m.toml").write_text("version = 2\n")
    return {"challenges_path": tmp_path / "c.toml", "measurements_path": tmp_path / "m.toml"}


def test_load_trust_builds_the_profile_from_the_toml_files_and_roundtrips(tmp_path):
    trust = load_trust(**files(tmp_path, "14"))
    assert trust.shares == ((b"bounty", 3000), (b"proof", 7000))
    assert (trust.challenges_version, trust.measurements_version) == (3, 2)
    assert trust.introduced_epoch == 14
    assert trust.measurements_digest == sha256(b"\x00").digest()  # empty allowlist
    assert trust_from_json(trust_to_json(trust)) == trust


@pytest.mark.parametrize("epoch", ['"CHOOSE_ACTIVATION_EPOCH"', "-1", "1.5", "true"])
def test_load_trust_refuses_a_placeholder_or_non_integer_activation_epoch(tmp_path, epoch):
    with pytest.raises(ProtocolError, match="introduced_epoch"):
        load_trust(**files(tmp_path, epoch))


@pytest.mark.parametrize("name", ["challenges-v2.example.toml", "challenges-v3.example.toml"])
def test_example_profiles_fail_with_a_clear_error_until_the_epoch_is_chosen(name):
    root = Path(__file__).parents[2] / "config"
    with pytest.raises(ProtocolError, match="introduced_epoch"):
        load_trust(challenges_path=root / name, measurements_path=root / "measurements.toml")


def test_repository_default_profile_loads():
    root = Path(__file__).parents[2] / "config"
    trust = load_trust(
        challenges_path=root / "challenges.toml", measurements_path=root / "measurements.toml"
    )
    assert sum(value for _, value in trust.shares) == 10000


GOOD = CHALLENGES.format(epoch="0")


def policy(value: str) -> str:
    return GOOD.replace('policy = "all_metagraph_hotkeys"', f"policy = {value}", 1)


MALFORMED = {
    "epoch-placeholder": (CHALLENGES.format(epoch='"CHOOSE_ACTIVATION_EPOCH"'), "version = 2"),
    "version-str": (GOOD.replace("version = 3", 'version = "3"'), "version = 2"),
    "id-int": (GOOD.replace('id = "bounty"', "id = 5"), "version = 2"),
    "bps-str": (GOOD.replace("= 3000", '= "3000"'), "version = 2"),
    "bps-bool": (GOOD.replace("= 3000", "= true"), "version = 2"),
    "bps-range": (GOOD.replace("= 3000", "= 70000"), "version = 2"),
    "id-missing": (GOOD.replace('id = "bounty"\n', ""), "version = 2"),
    "bps-missing": (GOOD.replace("emission_share_bps = 3000\n", ""), "version = 2"),
    "policy-int": (GOOD.replace('policy = "all_metagraph_hotkeys"', "policy = 3"), "version = 2"),
    "challenges-int": (GOOD.replace("[[challenges]]", "challenges = 1\n[[x]]", 1), "version = 2"),
    "measurement-int": (GOOD, "version = 2\nmeasurements = [1]"),
    "measurement-field-int": (GOOD, "version = 2\nmeasurements = [{ mr_td = 1 }]"),
    "toml-syntax": (GOOD, "version = [2"),
    "hotkeys-int": (
        policy('{ type = "explicit_allowlist", hotkeys = 5 }'),
        "version = 2",
    ),
    "hotkey-int": (
        policy('{ type = "explicit_allowlist", hotkeys = [5] }'),
        "version = 2",
    ),
    "min-stake-str": (
        policy('{ type = "stake_at_least", min_stake = "x" }'),
        "version = 2",
    ),
}


@pytest.mark.parametrize(("challenges", "measurements"), MALFORMED.values(), ids=MALFORMED)
def test_master_turns_a_placeholder_profile_into_a_clean_503(tmp_path, challenges, measurements):
    from cortex.config import MasterConfig
    from cortex.errors import ServiceError

    (tmp_path / "c.toml").write_text(challenges)
    (tmp_path / "m.toml").write_text(measurements)
    config = MasterConfig(
        netuid=541,
        state_dir=tmp_path,
        challenges_file=tmp_path / "c.toml",
        measurements_file=tmp_path / "m.toml",
        proof_seed_file=tmp_path / "unused",
        operator_token_file=tmp_path / "unused",
        intake_token_file=tmp_path / "unused",
        challenge_secrets_dir=tmp_path,
    )
    with pytest.raises(ServiceError, match="challenge profile unavailable") as refused:
        config.challenge_profile(0)
    assert refused.value.status == 503
