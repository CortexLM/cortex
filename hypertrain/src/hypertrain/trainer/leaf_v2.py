"""ht-leaf-v2 commitment (plan 1.8): fixed 1 MiB chunk tree hash over full unsharded tensors.

tree_hash(T): names in UTF-8-sorted order; header = b"ht-leaf-v2|" + JCS([[name, shape], ...]);
stream = concatenation of each tensor's little-endian fp32 bytes in that order, cut into fixed
1 MiB chunks (last one shorter). digests = [sha256(header)] + [sha256(chunk) ...];
root = sha256(digests concatenated). SHA-256 only: commitments must stay collision-resistant.

leaf_digest = sha256(b"hypertrain/1|LeafV2|" + JCS({...})) over the theta/m/v tree hashes, the
window's batch-id hash, and the f32 loss/norm. ht-leaf-v1 (loop._make_leaf) is unchanged.
"""

from __future__ import annotations

import hashlib

import torch
from torch import Tensor

from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages import f32hex

Params = dict[str, Tensor]
CHUNK = 1 << 20
HEADER = b"ht-leaf-v2|"
LEAF_DOMAIN = b"hypertrain/1|LeafV2|"


def tree_hash(t: Params) -> str:
    names = sorted(t, key=lambda s: s.encode("utf-8"))
    for n in names:
        if t[n].dtype != torch.float32:
            raise ValueError(f"{n}: ht-leaf-v2 hashes fp32 tensors only")
    header = HEADER + canonicalize([[n, list(t[n].shape)] for n in names])
    # ponytail: materialises the whole stream on host; stream chunks from pinned D2H copies
    # (plan 1.8) once leaf time shows up in step profiles.
    stream = b"".join(
        t[n].detach().cpu().contiguous().numpy().astype("<f4", copy=False).tobytes() for n in names
    )
    digests = [hashlib.sha256(header).digest()]
    digests += [
        hashlib.sha256(stream[i : i + CHUNK]).digest() for i in range(0, len(stream), CHUNK)
    ]
    return hashlib.sha256(b"".join(digests)).hexdigest()


def leaf_digest(
    run_id: str,
    w: int,
    t: int,
    theta: Params,
    m: Params,
    v: Params,
    batch_ids_sha256: str,
    loss: float,
    norm: float,
) -> bytes:
    body = {
        "version": "ht-leaf-v2",
        "run_id": run_id,
        "w": w,
        "t": t,
        "theta": tree_hash(theta),
        "m": tree_hash(m),
        "v": tree_hash(v),
        "batch_ids_sha256": batch_ids_sha256,
        "loss_f32": f32hex(loss),
        "norm_f32": f32hex(norm),
    }
    return hashlib.sha256(LEAF_DOMAIN + canonicalize(body)).digest()
