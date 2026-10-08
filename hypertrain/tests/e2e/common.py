"""Keys, tokens and the CPU RunManifest shared by the e2e harness and its actor processes."""

from __future__ import annotations

import sys
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "trainer"))

from trainer_fixtures import small_manifest_body  # noqa: E402

import hypertrain.trainer  # noqa: F401,E402  (determinism setup precedes torch)
from hypertrain.beacon import BeaconRound, BeaconVerificationError, FixtureBeacon  # noqa: E402
from hypertrain.protocol.keys import Keypair  # noqa: E402
from hypertrain.protocol.messages import ReplayEnv, RunManifest, f32hex  # noqa: E402
from hypertrain.trainer.compress import state_hash  # noqa: E402
from hypertrain.trainer.config import TrainConfig  # noqa: E402
from hypertrain.trainer.model import init_params  # noqa: E402

INTERNAL, ADMIN, WORKER = "e2e-internal-token", "e2e-admin-token", "e2e-worker-token"
SLUG = "hypertrain"
COORD = Keypair(bytes(range(32)))  # example_manifest coord_pubkey
AUDITOR = Keypair(bytes(31) + b"\x01")
REFEREE = Keypair(b"\x31" * 32)
OWNER = Keypair(b"\x77" * 32)
IMAGE = "sha256:" + "1" * 64
EPOCH_SECONDS = 86_400  # every scenario's finalize events fall in one ledger epoch
DELTA = 1e-3  # size of every injected single-element fault
# Drand-round timeouts (manifest verify.T); audit_after_commit >= 40 is enforced by the protocol.
TIMEOUTS = {
    "assign_after_open": 2,
    "audit_after_commit": 40,
    "upload_after_commit": 45,
    "serve_deadline": 50,
    "dispute_per_level": 10,
}
RUN_CONFIG = {"train_rounds": 2, "final_after_upload": 1}
CPU_ENV = ReplayEnv(
    image_digest=IMAGE,
    driver="cpu",
    gpu_uuid_sha256="3c9909afec25354d551dae21590bb26e38d53f2173b8d3dc3eee4c047e7ab1c1",
    sm_count=1,
)


_ORACLE = FixtureBeacon(current=2**40)


def beacon_payload(rnd: int) -> dict[str, Any]:
    br = _ORACLE.get(rnd)
    return {"round": br.round, "signature": br.signature, "randomness": br.randomness}


def verify_fixture(payload: Any) -> BeaconRound:
    """Fixture stand-in for the drand BLS check: only oracle payloads verify."""
    rnd = payload.get("round")
    if type(rnd) is not int or rnd < 1 or dict(payload) != beacon_payload(rnd):
        raise BeaconVerificationError("signature does not verify")
    return _ORACLE.get(rnd)


def bearer(token: str) -> dict[str, str]:
    return {"authorization": f"Bearer {token}"}


def miner_key(i: int) -> Keypair:
    """Miner/honeypot hotkeys; seeds 0x31 (referee) and 0x77 (owner) are reserved."""
    if i in (0x31, 0x77) or not 1 < i < 256:
        raise ValueError(f"reserved or invalid miner key index {i}")
    return Keypair(bytes([i]) * 32)


def manifest(
    *,
    policy: str = "reset",
    H: int = 6,
    J: int = 2,
    q: float = 0.5,
    E: int = 3,
    k_segments: int = 3,
    q_top: int = 1,
) -> RunManifest:
    body: Any = small_manifest_body()
    body["inner"].update(state_policy=policy, H=H, J=J)
    body["verify"].update(
        q_base=f32hex(q),
        E_vest_rounds=E,
        probation_rounds=E,
        s_min_reward_multiple=E,
        k_segments=k_segments,
        Q_top=q_top,
        T=dict(TIMEOUTS),
    )
    body["auditors"] = [AUDITOR.ss58, REFEREE.ss58]
    body["reference_spec"]["image_digest"] = IMAGE
    body["init_state_hash"] = state_hash(init_params(TrainConfig.from_manifest(body).model))
    return RunManifest.model_validate(body)
