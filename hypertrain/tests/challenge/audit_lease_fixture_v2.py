"""A seeded audit round on the 0.2 s `network` fixture: no 151 s admission graduation needed."""

from __future__ import annotations

from hypertrain.auditor.replay import AnchorCache, optimizer_hash, pack_state
from hypertrain.protocol.hashing import MerkleTree, sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages import LeafPreimage, Receipt, StageState
from hypertrain.protocol.messages_v2 import (
    AuditPolicyV2,
    CommitV2,
    PolicyHashes,
    RoundOpenV2,
    StartStateV2,
)
from hypertrain.trainer.compress import state_hash
from hypertrain.trainer.config import TrainConfig
from hypertrain.trainer.model import init_params

from . import test_service_network_v2 as fixtures
from .test_service_network_v2 import network  # noqa: F401  (pytest fixture import)

AUDITOR_A, AUDITOR_B = fixtures.AUDITORS
D_AUDIT = 20
EXTRA = [Keypair(bytes([200 + i]) * 32) for i in range(8)]


def miners(n: int) -> list[Keypair]:
    return [*fixtures.HOT, *EXTRA][:n]


def lease_rounds(net) -> int:
    policy = AuditPolicyV2.model_validate_json(
        net.store.objects.get(net.manifest.network.audit_policy_hash)
    )
    return policy.lease_rounds


def seed_round(net, n=4, *, gap=200, commit_exp=None, nan_leaf=None):
    """Write round 0 (opening, starts, commits, leaves, finality) straight into `records_v2`.

    `gap` is `d_final - d_audit`: 100 leaves one lease window, 200 two full ones.
    `commit_exp` / `nan_leaf` ({miner index: value}) poison one miner's sealed inputs."""
    commit_exp, nan_leaf = commit_exp or {}, nan_leaf or {}
    store, manifest = net.store, net.manifest
    run = manifest.run_id()
    cfg = TrainConfig.from_manifest_v2(manifest)
    t = manifest.training
    theta = init_params(cfg.model)
    batch, H, J = t.batch_samples(), t.inner.H, t.inner.J
    sample_ids = list(range(batch))
    keys = miners(n)
    cache = AnchorCache()
    roster, starts, per = [], [], []
    for slot, key in enumerate(keys):
        anchor = cache.genesis(manifest, key.ss58, theta)
        ef_hash = store.objects.put(pack_state(anchor.ef))
        state_blob = store.objects.put(pack_state(theta, anchor.state))
        start = StartStateV2(
            run_id=run,
            w=0,
            hotkey=key.ss58,
            theta_hash=state_hash(theta),
            state_object_sha256=state_blob,
            opt_state_hash=optimizer_hash(anchor.state),
            ef_object_sha256=ef_hash,
            ef_hash=state_hash(anchor.ef),
            parent_anchor_hash=anchor.anchor_hash,
            global_step0=0,
            anchor_verdict_hash=anchor.proof_hash,
        )
        starts.append(start)
        per.append((key, anchor, start))
        roster.append(
            {
                "hotkey": key.ss58,
                "slot": slot,
                "q_i": "0000803f",
                "admission_id": sha256_hex(f"adm-{slot}".encode()),
                "coldkey_group": (fixtures.COLD + EXTRA)[slot].ss58,
                "state": "ACTIVE",
                "eligible_weight": 4194304,
            }
        )
    opening = RoundOpenV2(
        w=0,
        prev_final_hash="0" * 64,
        theta_hash=state_hash(theta),
        outer_state_hash="0" * 64,
        center_hash="0" * 64,
        roster_hash=sha256_hex(canonicalize(roster)),
        honeypot_commit="0" * 64,
        d_open=D_AUDIT - 6,
        d_assign=D_AUDIT - 5,
        d_commit=D_AUDIT - 4,
        d_audit=D_AUDIT,
        d_upload=D_AUDIT + 1,
        d_final=D_AUDIT + gap,
        contract_version=2,
        policy_hashes=PolicyHashes.model_validate(
            {name: getattr(manifest.network, name) for name in PolicyHashes.model_fields}
        ),
        registry_epoch=0,
        start_state_index_hash=sha256_hex(canonicalize([s.body() for s in starts])),
        audit_mode="anchored-full",
        roster=roster,
    )
    with store._tx():
        store._put_record_v2(run, "round", "0", net.signed(fixtures.COORD, "RoundOpenV2", opening))
        for index, (key, anchor, start) in enumerate(per):
            ident = f"0:{key.ss58}"
            leaves = [
                LeafPreimage(
                    run_id=run,
                    w=0,
                    t=i * J,
                    stages=[StageState(theta="77" * 32, m="88" * 32, v="99" * 32)],
                    batch_ids_sha256="aa" * 32,
                    rng_ctr=0,
                    loss_f32="0000c07f" if nan_leaf.get(index) == i else "0000803f",
                    norm_f32="0000803f",
                )
                for i in range(H // J + 1)
            ]
            root = MerkleTree([bytes.fromhex(p.digest()) for p in leaves]).root.hex()
            commit = CommitV2(
                w=0,
                hotkey=key.ss58,
                leaf_scheme="ht-leaf-v1",
                n_leaves=len(leaves),
                leaves_root=root,
                metrics_root="cc" * 32,
                final_theta_hash="dd" * 32,
                ef_in_hash=state_hash(anchor.ef),
                ef_out_hash="ee" * 32,
                delta_hash="ff" * 32,
                delta_bytes=10,
                tokens=batch * t.model.seq_len,
            )
            store._put_record_v2(run, "start", ident, start.body())
            store._put_record_v2(
                run,
                "commit",
                ident,
                net.signed(key, "CommitV2", commit, expiry=commit_exp.get(index, 10000)),
            )
            store._put_record_v2(
                run, "assignment", ident, {"samples": sample_ids, "assignment_hash": "12" * 32}
            )
            store._put_record_v2(
                run,
                "leaves",
                ident,
                {
                    "object": store.objects.put(
                        canonicalize([p.model_dump(mode="json") for p in leaves])
                    )
                },
            )
        store._put_record_v2(
            run,
            "finality",
            "0",
            {
                "run_id": run,
                "w": 0,
                "applied": False,
                "disposition": "PENDING",
                "unresolved_disputes": 0,
                "unresolved_audits": n,
                "rollback_complete": False,
                "state_hash": state_hash(theta),
                "resolution_hash": None,
            },
        )
    return keys


def schedule(net):
    net.push(D_AUDIT)
    r = net.client.post(net.url + "/admin/rounds/0/audits", headers=fixtures.admin())
    assert r.status_code == 200, r.text
    return r.json()["jobs"]


def lease(net, auditor, nonce):
    body = Receipt(w=0, commit_hash=f"{nonce:064x}", received_round=net.now)
    return net.client.post(net.url + "/worker/lease", json=net.signed(auditor, "Receipt", body))
