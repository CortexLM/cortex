"""A seeded audit round on the 0.2 s `network` fixture: no 151 s admission graduation needed."""

from __future__ import annotations

import json

from hypertrain.auditor.replay import AnchorCache, optimizer_hash, pack_state
from hypertrain.protocol.hashing import MerkleTree, sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages import LeafPreimage, Receipt, StageState
from hypertrain.protocol.messages_v2 import (
    AuditPolicyV2,
    CommitV2,
    IslandJobV1,
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


def seed_round(
    net,
    n=4,
    *,
    executable=False,
    gap=200,
    commit_exp=None,
    nan_leaf=None,
    silent=(),
    deltas=True,
    admission_ids=None,
    opened=False,
    keys=None,
):
    """Write round 0 (opening, starts, commits, leaves, deltas, finality) into `records_v2`.

    `executable` also persists the genesis anchors and writes the island inputs, so that
    `execute_audit_v2` reaches the executor boundary. `gap` is `d_final - d_audit`: 100 leaves
    one lease window, 200 two full ones. `commit_exp` (one round for everybody, or {miner index:
    round}; default 10000, a commit that outlives every window) and `nan_leaf` ({miner index:
    value}) set or poison what a miner sealed: an honest miner seals `d_final`. `silent`: indexes
    of roster members that get a start record and nothing else (opened, started, no commit).
    `deltas`: every member that committed also delivered its delta manifest; a round nobody
    delivered to reads, to the exit reaper, as an upload path that failed for everybody.
    `admission_ids`: {miner index: the real admission id} of a member the test activated, so that a
    sanction of that member reaches its bond; the others carry a synthetic id.
    `opened`: round 0 was opened through the route (`live_operator.open_round`), so its opening,
    starts and finality are the store's own and only what the miners deliver is seeded; an honest
    commit seals the d_final of that opening. `keys`: the n keys of the members, where `miners(n)`
    is not wanted (an auditor of the manifest that is also one)."""
    nan_leaf = nan_leaf or {}
    expiry = commit_exp if isinstance(commit_exp, dict) else dict.fromkeys(range(n), commit_exp)
    store, manifest = net.store, net.manifest
    run = manifest.run_id()
    sealed_until = 10000
    if opened:
        sealed_until = RoundOpenV2.model_validate(
            store._record_v2(run, "round", "0")["body"]
        ).d_final
    cfg = TrainConfig.from_manifest_v2(manifest)
    t = manifest.training
    theta = init_params(cfg.model)
    batch, H, J = t.batch_samples(), t.inner.H, t.inner.J
    sample_ids = list(range(batch))
    dataset = store._record_v2(run, "dataset", "inputs")
    width = (cfg.model.seq_len + 1) * (2 if t.dataset.sample_format.startswith("u16") else 4)
    raw_samples = store.objects.get(dataset["samples_hash"])
    proofs = json.loads(store.objects.get(dataset["proofs_hash"]))
    keys = keys or miners(n)
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
                "admission_id": (admission_ids or {}).get(slot)
                or sha256_hex(f"adm-{slot}".encode()),
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
        if not opened:
            store._put_record_v2(
                run, "round", "0", net.signed(fixtures.COORD, "RoundOpenV2", opening)
            )
        for index, (key, anchor, start) in enumerate(per):
            ident = f"0:{key.ss58}"
            if index in silent:
                if not opened:
                    store._put_record_v2(run, "start", ident, start.body())
                continue
            if executable:
                store._persist_anchor_v2(manifest, anchor, cache)
                inputs = {
                    "start_state": pack_state(theta),
                    "ef_in": pack_state(anchor.ef),
                    "v0": pack_state({}),
                    "samples": b"".join(
                        raw_samples[i * width : (i + 1) * width] for i in sample_ids
                    ),
                    "sample_proofs": canonicalize([proofs[i] for i in sample_ids]),
                }
                source = store.state_dir / "jobs-v2/0" / key.ss58
                source.mkdir(parents=True, exist_ok=True)
                hashes = {}
                for name, raw in inputs.items():
                    (source / name).write_bytes(raw)
                    hashes[name] = store.objects.put(raw)
                store._put_record_v2(
                    run,
                    "island-job",
                    ident,
                    IslandJobV1(
                        job_version=1,
                        run_id=run,
                        w=0,
                        manifest=manifest,
                        sample_ids=sample_ids,
                        global_step0=0,
                        start_state_sha256=hashes["start_state"],
                        ef_in_sha256=hashes["ef_in"],
                        v0_sha256=hashes["v0"],
                        object_paths={k: k for k in inputs},
                        deadline=1,
                    ).body(),
                )
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
            if not opened:
                store._put_record_v2(run, "start", ident, start.body())
            store._put_record_v2(
                run,
                "commit",
                ident,
                net.signed(key, "CommitV2", commit, expiry=expiry.get(index) or sealed_until),
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
            if deltas:
                chunk = {"off": 0, "len": commit.delta_bytes, "sha256": commit.delta_hash}
                delta = {
                    "w": 0,
                    "hotkey": key.ss58,
                    "delta_hash": commit.delta_hash,
                    "size": commit.delta_bytes,
                    "uri": "sha256:" + commit.delta_hash,
                    "format": "ht-sparse-v1",
                    "chunks": [chunk],
                    "grant_hash": "00" * 32,
                    "master_acceptance_hash": "00" * 32,
                }
                store._put_record_v2(run, "delta", ident, net.signed(key, "DeltaManifestV2", delta))
        if not opened:
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


def execute_request(net, auditor, job):
    return canonicalize(
        net.signed(
            auditor, "Receipt", Receipt(w=0, commit_hash=job["lease_nonce"], received_round=net.now)
        )
    )
