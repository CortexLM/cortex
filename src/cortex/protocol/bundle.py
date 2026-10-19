"""Build and verify complete, independently recomputed epoch bundles (hash integrity only)."""

from collections.abc import Iterable
from hashlib import sha256

from .aggregate import aggregate_leaves
from .merkle import canonical_rows, merkle_root, metagraph_root
from .models import (
    PROPORTIONAL_SHARES,
    Bundle,
    BundleBody,
    Leaf,
    MetagraphRow,
    TrustRoot,
)
from .scale import ProtocolError, byte_vec

BUNDLE_PROTOCOL_VERSION = 2  # v1 carried leaf and gateway signatures; never confused with v2


def _verify_leaves(
    body: BundleBody,
    rows: tuple[MetagraphRow, ...],
    trust: TrustRoot,
    *,
    allow_quarantine: bool = False,
) -> set[bytes]:
    keys = [leaf.sort_key for leaf in body.leaves]
    if keys != sorted(set(keys)):
        raise ProtocolError("leaves must be sorted and unique")
    challenges = {entry.id: entry for entry in trust.challenges}
    bad: set[bytes] = set()
    for leaf in body.leaves:
        challenge = challenges.get(leaf.challenge_id)
        if challenge is None:
            raise ProtocolError("unknown challenge")
        if leaf.epoch != body.epoch:
            bad.add(challenge.id)
    for challenge in trust.challenges:
        present = {leaf.miner_hotkey for leaf in body.leaves if leaf.challenge_id == challenge.id}
        if present != challenge.policy.expected(rows):
            if not allow_quarantine:
                raise ProtocolError("incomplete participant set")
            bad.add(challenge.id)
    if merkle_root(leaf.encode() for leaf in body.leaves) != body.merkle_root:
        raise ProtocolError("Merkle root mismatch")
    if bad and not allow_quarantine:
        raise ProtocolError("leaf epoch mismatch")
    return bad


def build_bundle(
    *,
    epoch: int,
    netuid: int,
    block_b: int,
    block_hash: bytes,
    rows: Iterable[MetagraphRow],
    leaves: Iterable[Leaf],
    trust: TrustRoot,
) -> Bundle:
    trust.validate()
    if epoch < trust.introduced_epoch:
        raise ProtocolError("challenge profile is not active for bundle epoch")
    ordered_rows = canonical_rows(rows)
    ordered_leaves = tuple(sorted(leaves, key=lambda leaf: leaf.sort_key))
    uid_map = tuple((row.hotkey, row.uid) for row in ordered_rows)
    final = aggregate_leaves(
        ordered_leaves, trust.shares, uid_map, algorithm_version=trust.algorithm_version
    )
    body = BundleBody(
        BUNDLE_PROTOCOL_VERSION,
        epoch,
        netuid,
        block_b,
        block_hash,
        metagraph_root(ordered_rows),
        trust.algorithm_version,
        trust.shares,
        trust.measurements_digest,
        uid_map,
        ordered_leaves,
        merkle_root(leaf.encode() for leaf in ordered_leaves),
        final.final_vector,
    )
    _verify_leaves(body, ordered_rows, trust)
    return Bundle(body)


def verify_bundle(
    bundle: Bundle,
    *,
    rows: Iterable[MetagraphRow],
    block_hash: bytes,
    trust: TrustRoot,
    netuid: int,
    epoch: int | None = None,
) -> BundleBody:
    body, _ = verify_inputs(
        bundle, rows=rows, block_hash=block_hash, trust=trust, netuid=netuid, epoch=epoch
    )
    computed = aggregate_leaves(
        body.leaves, body.emission_shares, body.uid_map, algorithm_version=body.algorithm_version
    ).final_vector
    if body.final_vector != computed:
        raise ProtocolError("final vector mismatch")
    return body


def verify_inputs(
    bundle: Bundle,
    *,
    rows: Iterable[MetagraphRow],
    block_hash: bytes,
    trust: TrustRoot,
    netuid: int,
    epoch: int | None = None,
    allow_quarantine: bool = False,
) -> tuple[BundleBody, set[bytes]]:
    """Rows/hash must come from the chain at body.block_b, never from the gateway."""
    trust.validate()
    body = bundle.body
    if (
        body.protocol_version != BUNDLE_PROTOCOL_VERSION
        or body.algorithm_version != trust.algorithm_version
    ):
        raise ProtocolError("unsupported bundle version")
    if body.epoch < trust.introduced_epoch:
        raise ProtocolError("challenge profile is not active for bundle epoch")
    if body.netuid != netuid or (epoch is not None and body.epoch != epoch):
        raise ProtocolError("bundle subnet/epoch mismatch")
    if body.block_hash != block_hash:
        raise ProtocolError("chain block hash mismatch")
    ordered_rows = canonical_rows(rows)
    if body.metagraph_root != metagraph_root(ordered_rows):
        raise ProtocolError("metagraph root mismatch")
    if body.uid_map != tuple((row.hotkey, row.uid) for row in ordered_rows):
        raise ProtocolError("uid map mismatch")
    if body.emission_shares != trust.shares:
        raise ProtocolError("emission shares mismatch")
    if body.measurements_digest != trust.measurements_digest:
        raise ProtocolError("measurements digest mismatch")
    bad = _verify_leaves(body, ordered_rows, trust, allow_quarantine=allow_quarantine)
    return body, bad


def recompute(body: BundleBody, quarantined: set[bytes], minimum_share_mass: int = 5000):
    if body.emission_shares == PROPORTIONAL_SHARES and body.algorithm_version == 1:
        raise ProtocolError("proportional shares require algorithm version 2")
    shares = tuple(pair for pair in body.emission_shares if pair[0] not in quarantined)
    mass = sum(value for _, value in shares)
    if mass < minimum_share_mass or not mass:
        raise ProtocolError("surviving share mass below threshold")
    if body.algorithm_version >= 2:
        # Quarantined mass burns; the surviving challenges cannot inherit its share.
        shares = body.emission_shares
    elif quarantined:
        apportioned = {name: value * 10000 // mass for name, value in shares}
        remaining = 10000 - sum(apportioned.values())
        order = sorted(shares, key=lambda pair: (-(pair[1] * 10000 % mass), byte_vec(pair[0])))
        for name, _ in order[:remaining]:
            apportioned[name] += 1
        shares = tuple((name, apportioned[name]) for name, _ in shares)
    return aggregate_leaves(
        tuple(leaf for leaf in body.leaves if leaf.challenge_id not in quarantined),
        shares,
        body.uid_map,
        algorithm_version=body.algorithm_version,
    )


def bundle_digest(bundle: Bundle) -> str:
    return sha256(bundle.encode()).hexdigest()
