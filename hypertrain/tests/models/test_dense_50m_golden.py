"""Known answers for the init of the two D94 dense TEST rows (ADR 0016): the 31 M run profile and
the larger 44 M option, at their real size.

Each state hash is the `init_state_hash` the TEST builder (scripts/b300/d20/build_test_manifest.sh,
HT_D20_GEOMETRY=dense-0.05b or dense-0.03b) signs into the manifest, so a change to the profile row
or to the decoder init moves it, and with it every run id built from the row.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

import pytest
import torch

from hypertrain.protocol.env_pins import EnvPin, class_pin
from hypertrain.trainer.compress import state_hash
from hypertrain.trainer.config import TrainConfig
from hypertrain.trainer.model import init_params

SCRIPTS = Path(__file__).resolve().parents[2] / "scripts"
sys.path.insert(0, str(SCRIPTS))
from run_mfu import PROFILES, manifest_body  # noqa: E402  (isort: skip)

# Every manifest pins its environment (DET-01); a CPU class with no GPU, as CI runs these.
CPU_PIN = EnvPin.from_body(class_pin("avx2", "none"))

INIT_STATE_HASHES = {
    "mfu-dense-0.05b": "9cbe3ee93068dbf7cf08791f02bd3bd07e5181fb3bc13efa4396b4ade5eaf897",
    "mfu-dense-0.03b": "e372a68edb6485fce5f3df4efa9d72705fb04bbf7fd130dc3805d84075bf0aa6",
}


@pytest.mark.parametrize("name", INIT_STATE_HASHES)
def test_two_inits_of_the_row_are_bitwise_equal_and_hash_to_the_pinned_state(name: str) -> None:
    row = json.loads(PROFILES.read_text())["profiles"][name]
    cfg = TrainConfig.from_manifest(manifest_body(row, CPU_PIN, torch.device("cpu"))).model

    first, second = init_params(cfg), init_params(cfg)

    assert first.keys() == second.keys()
    assert all(torch.equal(first[name], second[name]) for name in first)
    assert state_hash(first) == INIT_STATE_HASHES[name]
