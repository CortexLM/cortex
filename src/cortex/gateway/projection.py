"""The established master weights JSON, derived from immutable signed bytes."""

import json
from datetime import UTC, datetime, timedelta
from hashlib import sha256
from uuid import UUID

from cortex.protocol import Bundle, Score, aggregate_leaves
from cortex.protocol.aggregate import challenge_emission_percent
from cortex.protocol.crypto import encode_hotkey


def _identity(digest: str) -> str:
    return str(UUID(bytes=sha256(digest.encode()).digest()[:16], version=5))


def timestamp(value: datetime) -> str:
    return value.astimezone(UTC).isoformat(timespec="microseconds").replace("+00:00", "Z")


def project(bundle: Bundle | None, *, netuid: int, now: datetime, chain_endpoint: str = "") -> dict:
    computed = timestamp(now)
    view: dict = dict(
        protocol_version="1.0",
        vector_id=None,
        vector_digest=None,
        epoch=None,
        revision=0,
        netuid=netuid,
        chain_endpoint=chain_endpoint,
        uids=[0],
        weights=[1.0],
        hotkey_weights={},
        computed_at=computed,
        expires_at=timestamp(now + timedelta(seconds=720)),
        source_challenges=[],
        source_snapshots=[],
        source_outcomes=[],
        emission_policy_version="emission-shares.absolute.v1",
        emission_shares={},
        burn_policy_version="burn-uid0.v1",
        mapping_policy_version="hotkey-to-uid.v1",
        metagraph_identity=dict(hash=None, block=None, uid_count=0, burn_uid=0),
        metagraph_hash=None,
        metagraph_block=None,
        burn_outcome=True,
        metagraph_updated_at=None,
        merkle_root="",
        final_vector=[[0, 65535]],
        sealed=False,
    )
    if bundle is not None:
        body = bundle.body
        floats = aggregate_leaves(
            body.leaves,
            body.emission_shares,
            body.uid_map,
            algorithm_version=body.algorithm_version,
        )
        digest = sha256(body.encode()).hexdigest()
        view.update(
            vector_id=_identity(digest),
            vector_digest=digest,
            epoch=body.epoch,
            revision=1,
            algorithm_version=body.algorithm_version,
            netuid=body.netuid,
            uids=list(floats.uids),
            weights=list(floats.weights),
            hotkey_weights={
                encode_hotkey(bytes.fromhex(key)): value
                for key, value in floats.hotkey_weights.items()
            },
            metagraph_identity=dict(
                hash=body.metagraph_root.hex(),
                block=body.block_b,
                uid_count=len(body.uid_map),
                burn_uid=0,
            ),
            metagraph_hash=body.metagraph_root.hex(),
            metagraph_block=body.block_b,
            burn_outcome=any(uid == 0 for uid, _ in body.final_vector),
            metagraph_updated_at=computed,
            merkle_root=body.merkle_root.hex(),
            final_vector=[list(pair) for pair in body.final_vector],
            sealed=True,
        )
        for challenge, bps in body.emission_shares:
            slug = challenge.decode()
            leaves = [leaf for leaf in body.leaves if leaf.challenge_id == challenge]
            source_digest = (
                sha256(b"".join(leaf.encode() for leaf in leaves)).hexdigest() if leaves else None
            )
            snapshot_id = _identity(source_digest) if source_digest is not None else None
            outcome = "accepted" if leaves else "missing"
            source_weights: dict[str, float] = {}
            for leaf in leaves:
                if isinstance(leaf.score, Score) and leaf.score.value > 0:
                    hotkey = encode_hotkey(leaf.miner_hotkey)
                    source_weights[hotkey] = source_weights.get(hotkey, 0.0) + leaf.score.value
            view["emission_shares"][slug] = bps / 10000
            raw_total = sum(
                leaf.score.value
                for leaf in leaves
                if isinstance(leaf.score, Score) and leaf.score.value > 0
            )
            emission_percent = challenge_emission_percent(
                challenge, bps, raw_total, algorithm_version=body.algorithm_version
            )
            view["source_challenges"].append(
                dict(
                    slug=slug,
                    emission_percent=emission_percent,
                    weights=source_weights,
                    ok=bool(leaves),
                    error=None if leaves else outcome,
                )
            )
            if leaves:
                view["source_snapshots"].append(
                    dict(
                        challenge_slug=slug,
                        snapshot_id=snapshot_id,
                        payload_digest=source_digest,
                        outcome=outcome,
                    )
                )
            view["source_outcomes"].append(
                dict(
                    challenge_slug=slug,
                    outcome=outcome,
                    reason_code=outcome,
                    snapshot_id=snapshot_id,
                    payload_digest=source_digest,
                    revision=None,
                )
            )
    view["chain_domain_bytes"] = json.dumps(
        dict(netuid=view["netuid"], uids=view["uids"], weights=view["weights"]),
        sort_keys=True,
        separators=(",", ":"),
        allow_nan=False,
    )
    return view
