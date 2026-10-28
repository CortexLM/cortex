"""Known answer for the init of the D68 intermediate TEST model (ADR 0015), at its real size.

The state hash is the `init_state_hash` the TEST builder (scripts/b300/d20/build_test_manifest.sh,
HT_D20_GEOMETRY=dense-0.1b) signs into the manifest, so a change to the profile row or to the
decoder init moves it, and with it every run id built from the row.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

import torch

from hypertrain.trainer.compress import state_hash
from hypertrain.trainer.config import TrainConfig
from hypertrain.trainer.model import init_params

SCRIPTS = Path(__file__).resolve().parents[2] / "scripts"
sys.path.insert(0, str(SCRIPTS))
from run_mfu import PROFILES, manifest_body  # noqa: E402  (isort: skip)

INIT_STATE_HASH = "be9b7bd63f79e4d7a3e640f51e9cfcc03e445a9de9ab249bcf77d938b8ee28f8"


def test_two_inits_of_the_row_are_bitwise_equal_and_hash_to_the_pinned_state() -> None:
    row = json.loads(PROFILES.read_text())["profiles"]["mfu-dense-0.1b"]
    cfg = TrainConfig.from_manifest(manifest_body(row)).model

    first, second = init_params(cfg), init_params(cfg)

    assert first.keys() == second.keys()
    assert all(torch.equal(first[name], second[name]) for name in first)
    assert state_hash(first) == INIT_STATE_HASH
