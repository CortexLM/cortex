"""Separate trial domain; no production exposure cursor is read or advanced."""

from __future__ import annotations

import struct

from hypertrain.beacon.core import BeaconRound
from hypertrain.data.assignment import assign_round
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.messages_v2 import RunManifestV2


def trial_samples(
    manifest: RunManifestV2, admission_id: str, nonce: str, beacon: BeaconRound
) -> tuple[int, ...]:
    domain = sha256_hex(
        f"ht-trial-v2|{manifest.run_id()}|{admission_id}|{nonce}|{beacon.round}".encode()
    )
    ds = manifest.training.dataset
    return assign_round(
        bytes.fromhex(domain),
        beacon.round,
        bytes.fromhex(beacon.signature),
        n_samples=ds.n_samples,
        n_slots=1,
        batch=manifest.training.batch_samples(),
        base_w=0,
        unit=ds.assign_unit or 1,
    ).slices[0]


def trial_assignment_hash(manifest: RunManifestV2, epoch: int, samples: tuple[int, ...]) -> str:
    return sha256_hex(
        b"ht-assignment-v1"
        + bytes.fromhex(manifest.run_id())
        + struct.pack(">QQ", epoch, 0)
        + b"".join(struct.pack("<Q", i) for i in samples)
    )
