"""Admission carry starts from authoritative full genesis, not theta-only state."""

from __future__ import annotations

import importlib.util
import json
import sys
from pathlib import Path

import pytest
import torch

from hypertrain.auditor.replay import AnchorCache, pack_state, unpack_state
from hypertrain.data.trial_assignment import trial_samples
from hypertrain.protocol.hashing import MerkleTree, sha256_hex
from hypertrain.protocol.messages_v2 import EscrowLock, JoinChallenge
from hypertrain.trainer.compress import state_hash
from hypertrain.trainer.config import TrainConfig
from hypertrain.trainer.model import init_params

spec = importlib.util.spec_from_file_location(
    "carry_genesis_fixture", Path(__file__).parents[1] / "e2e/test_service_network_v2_e2e.py"
)
assert spec is not None and spec.loader is not None
fixture = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = fixture
spec.loader.exec_module(fixture)
od_carry_network = fixture.od_carry_network


@pytest.mark.parametrize("od_carry_network", ["mlm", "decision", "distill"], indirect=True)
def test_trial_stages_pinned_complete_carry_genesis(od_carry_network):
    network = od_carry_network
    identity = network.join().json()["admission_id"]
    cold = fixture.fixtures.COLD[0]
    lock = EscrowLock(
        operation_id="ab" * 32,
        owner=cold.ss58,
        units=1000,
        origin_ids=[network.origin_ids[0]],
        admission_id=identity,
        dispute_id=None,
        kind="LOCK_ADMISSION",
    )
    response = network.client.post(
        network.url + "/escrow/lock", json=network.signed(cold, "EscrowLock", lock)
    )
    assert response.status_code == 200, response.text
    network.push(2)
    response = network.client.get(network.url + "/join/" + identity + "/challenge")
    assert response.status_code == 200, response.text
    challenge = JoinChallenge.model_validate(response.json()["body"])
    samples = tuple(
        trial_samples(
            network.manifest,
            identity,
            challenge.nonce,
            network.store._beacon_v2(challenge.seed_beacon),
        )
    )
    epoch = network.store._db.execute(
        "SELECT trial_epoch FROM admissions_v2 WHERE admission_id=?", (identity,)
    ).fetchone()[0]
    job, directory = network.store.stage_trial_v2(
        network.manifest.run_id(), challenge, samples, epoch
    )
    raw = (directory / "start_state").read_bytes()
    theta, state = unpack_state(raw)
    assert state is not None and state.step == job.global_step0 == 0
    assert set(state.m) == set(state.v) == set(theta)
    assert all(
        torch.count_nonzero(tensor) == 0 for tensor in (*state.m.values(), *state.v.values())
    )
    genesis = AnchorCache().genesis(
        network.manifest,
        fixture.fixtures.HOT[0].ss58,
        init_params(TrainConfig.from_manifest_v2(network.manifest).model),
    )
    assert raw == pack_state(genesis.theta, genesis.state)
    assert sha256_hex(raw) == job.start_state_sha256
    assert state_hash(theta) == challenge.theta_hash
    assert (directory / "ef_in").read_bytes() == pack_state(genesis.ef)
    assert sha256_hex((directory / "ef_in").read_bytes()) == job.ef_in_sha256
    rows = (directory / "samples").read_bytes()
    proofs = json.loads((directory / "sample_proofs").read_bytes())
    width = (network.manifest.training.model.seq_len + 1) * 2
    dataset = network.manifest.training.dataset
    for index, sample in enumerate(samples):
        assert MerkleTree.verify(
            rows[index * width : (index + 1) * width],
            sample,
            [bytes.fromhex(p) for p in proofs[index]],
            bytes.fromhex(dataset.merkle_root),
            dataset.n_samples,
        )
    repeated, _ = network.store.stage_trial_v2(network.manifest.run_id(), challenge, samples, epoch)
    assert repeated == job
