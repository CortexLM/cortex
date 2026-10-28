"""Doubles and helpers of the sanctions-and-wedges scenarios (test_missing_sanctions_and_wedges.py).

The doubles reuse the public seams of ac_cheats.py (an import error is immediate if one is
renamed). Their purpose is not a cheat the
audit catches at intake but a message the master accepts and cannot digest later (a wedge), or a
delta the master accepts under an acceptance that was minted for another upload.
"""

from __future__ import annotations

import json
import math
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import pytest
from ac_cheats import (
    ArtifactCheat,
    Cheat,
    CheatContext,
    CopiedPeer,
    ReplayedRound,
    WireApi,
    WireRule,
    Withdraw,
    leaves_of,
    stage_of,
    write_leaves,
)
from ac_precondition import precondition
from ac_world import Observation, World

from env_pin_fixtures import honest_env
from hypertrain.auditor.island_bisect import IslandParty
from hypertrain.miner.admission import sign_join
from hypertrain.protocol import envelope_v2
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages import f32hex
from hypertrain.protocol.messages_v2 import (
    BisectV2,
    DisputeV2,
    EscrowLock,
    HardwareHint,
    IslandJobV1,
    ResolutionV2,
)

FRESH_HOT = [Keypair(bytes([130 + i]) * 32) for i in range(2)]

WEDGE = 9  # not one of the eight cheats of the brief: a liveness hole, never audited


@dataclass(frozen=True, kw_only=True)
class NonFiniteLeaf(ArtifactCheat):
    """The last leaf carries a NaN norm; the leaves root, the commit and the delta stay
    self-consistent, so /leaves has nothing to compare against but the Merkle root."""

    def tamper(self, directory: Path, published: Path, job: IslandJobV1, ctx: CheatContext) -> None:
        leaves = leaves_of(published)
        leaves[-1]["norm_f32"] = f32hex(math.nan)
        write_leaves(published, leaves)


class _ShortLivedCommit(WireRule):
    """Re-signs the CommitV2 with an expiry that ends before the audit window opens."""

    def __init__(self, miner: Any) -> None:
        self.miner = miner
        self.d_commit: int | None = None

    def after(self, method: str, path: str, result: Any) -> None:
        if method == "GET" and path.rsplit("/", 2)[-2] == "rounds":
            self.d_commit = int(result["round_open"]["body"]["d_commit"])

    def before(self, method: str, path: str, kw: dict[str, Any]) -> tuple[str, dict[str, Any]]:
        if method == "POST" and stage_of(path) == "commit":
            if self.d_commit is None:
                raise RuntimeError("short-lived-commit saw no RoundOpen before the commit")
            env = kw["json"]
            sealed = envelope_v2.seal(
                self.miner.kp, "CommitV2", self.miner.run_id, env["body"], self.d_commit
            )
            return path, {**kw, "json": sealed}
        return path, kw


@dataclass(frozen=True, kw_only=True)
class ShortLivedCommit(Cheat):
    """The commit body is honest; only its envelope expires at d_commit, before d_audit."""

    def wire(self, miner: Any, ctx: CheatContext) -> None:
        miner.api = WireApi(miner.api, _ShortLivedCommit(miner))


@dataclass(frozen=True, kw_only=True)
class CopiedPeerWithoutRelayUpload(CopiedPeer):
    """A peer's published tree is submitted as the miner's own, and the miner never performs the
    relay upload: it stops before the upload grant."""

    def wire(self, miner: Any, ctx: CheatContext) -> None:
        miner.api = WireApi(miner.api, Withdraw("upload-grant"))


@dataclass(frozen=True, kw_only=True)
class ReplayedRoundWithoutRelayUpload(ReplayedRound):
    """The miner's own relabelled tree of round w-1 is committed for round w, and the miner
    never performs the relay upload: it stops before the upload grant."""

    def wire(self, miner: Any, ctx: CheatContext) -> None:
        miner.api = WireApi(miner.api, Withdraw("upload-grant"))


NON_FINITE_LEAF = NonFiniteLeaf(
    cheat=WEDGE,
    name="non-finite-leaf-metric",
    summary="a leaf published under a correct Merkle root carries a NaN norm",
    seam="artifacts",
    expected="stall",
)
SHORT_LIVED_COMMIT = ShortLivedCommit(
    cheat=WEDGE,
    name="short-lived-commit",
    summary="the commit envelope expires at d_commit, before the audit window",
    seam="wire",
    expected="stall",
)
COPIED_PEER_WITHOUT_UPLOAD = CopiedPeerWithoutRelayUpload(
    cheat=4,
    name="copied-peer-without-relay-upload",
    summary="a peer's tree is committed and its delta manifest is posted without an own upload",
    seam="artifacts",
    expected="audit",
)
REPLAYED_ROUND_WITHOUT_UPLOAD = ReplayedRoundWithoutRelayUpload(
    cheat=2,
    name="replayed-round-without-relay-upload",
    summary="the own relabelled tree of round w-1 is committed and its delta manifest is posted "
    "without an own upload",
    seam="artifacts",
    expected="audit",
    needs_previous_round=True,
    relabel=True,
)


def audit_queue_order(world: World, w: int, roster: list[int]) -> list[int]:
    """The roster indices in the order the master leases their audit rows.

    lease_v2 takes queued rows ordered by (created, id) and every row of a round is created at
    d_audit, so the order is the order of the job ids ("ht-audit-job/2|run|w|hotkey" hashed, as
    schedule_audits_v2 derives them). The formula is a copy: a scenario that depends on the
    position of a row checks it against the lease table with queue_position_holds().
    """

    def job_id(index: int) -> str:
        return sha256_hex(f"ht-audit-job/2|{world.run_id}|{w}|{world.hotkey(index)}".encode())

    return sorted(roster, key=job_id)


def queue_position_holds(seen: Observation, hotkey: str, position: int) -> bool:
    """Whether the master's own lease table puts `hotkey` at `position` of the audit queue."""
    queue = [row["hotkey"] for row in sorted(seen.leases, key=lambda row: row["id"])]
    return queue.index(hotkey) == position if hotkey in queue else False


def recorded_envelope(world: World, kind: str, w: int, hotkey: str) -> dict[str, Any]:
    """The signed envelope ('commit' or 'delta') the master stored for `hotkey` in round `w`."""
    row = world.network.store._db.execute(
        "SELECT data FROM records_v2 WHERE run_id=? AND kind=? AND id=?",
        (world.run_id, kind, f"{w}:{hotkey}"),
    ).fetchone()
    precondition(row is not None, f"no accepted {kind} of {w}:{hotkey}")
    return json.loads(row["data"])


def post_delta_manifest(world: World, index: int, manifest: dict[str, Any], expiry: int) -> Any:
    """POST /delta signed by miner `index` for `manifest` (a DeltaManifestV2 body)."""
    n = world.network
    signed = envelope_v2.seal(
        world.svc.HOT[index], "DeltaManifestV2", world.run_id, manifest, expiry
    )
    return n.client.post(n.url + "/delta", json=signed)


def register_on_metagraph(
    world: World, monkeypatch: pytest.MonkeyPatch, keys: list[Keypair]
) -> None:
    """Add `keys` to the fake master metagraph the app consults before it admits a hotkey."""
    metagraph = world.network.client.app.state.metagraph
    known = metagraph.hotkeys

    async def hotkeys() -> dict[str, Any]:
        return {**await known(), **{key.ss58: 1000 + i for i, key in enumerate(keys)}}

    monkeypatch.setattr(metagraph, "hotkeys", hotkeys)


def join_fresh_hotkey(world: World, cold: Keypair, hot: Keypair, label: str) -> Any:
    """POST /join for `hot`, owned by `cold`, as the admission HTTP route receives it."""
    n = world.network
    request = sign_join(
        hot,
        cold,
        run_id=world.run_id,
        request_id=sha256_hex(label.encode()),
        expires_beacon=10000,
        policy_hash=n.manifest.network.admission_policy_hash,
        hardware_hint=HardwareHint(device_name="cpu", device_count=1, driver="cpu"),
        env=honest_env(),
    )
    return n.client.post(n.url + "/join", content=canonicalize(request.body()))


def lose_a_funded_contest(world: World, index: int, w: int = 0) -> dict[str, Any]:
    """Miner `index` contests its own round-`w` verdict with a funded bond, answers the bisection
    with a hash chain that diverges from the independent replay, and the referee rules FRAUD.

    The flow is the one of tests/e2e/test_rollback_service_v2.py::exclude_miner, on the verdict
    the audit of this world produced. Returns the contest id and the resolution digest.
    """
    n, svc = world.network, world.svc
    hot, cold = svc.HOT[index], svc.COLD[index]
    run = world.run_id
    contest = json.loads(
        n.store._db.execute(
            "SELECT data FROM records_v2 WHERE kind='contest' AND json_extract(data,'$.miner')=? "
            "AND json_extract(data,'$.w')=?",
            (hot.ss58, w),
        ).fetchone()[0]
    )
    dispute_id = sha256_hex(canonicalize([run, contest["verdict_hash"], hot.ss58]))
    origin = n.store._db.execute(
        "SELECT origin FROM escrow_units WHERE owner=? AND bucket='available' AND units>=100",
        (cold.ss58,),
    ).fetchone()[0]
    lock = EscrowLock(
        operation_id=sha256_hex(b"audit-cheaters-contest-bond"),
        owner=cold.ss58,
        units=100,
        origin_ids=[origin],
        admission_id=None,
        dispute_id=dispute_id,
        kind="LOCK_CONTEST",
    )
    for path, signer, body in (
        ("/escrow/lock", cold, ("EscrowLock", lock)),
        (
            "/dispute",
            hot,
            (
                "DisputeV2",
                DisputeV2(
                    hotkey=hot.ss58,
                    verdict_hash=contest["verdict_hash"],
                    action="contest",
                    bond_lock=lock.operation_id,
                ),
            ),
        ),
    ):
        response = n.client.post(n.url + path, json=n.signed(signer, *body))
        precondition(response.status_code == 200, f"{path}: {response.text[:160]}")
    disputes = n.store._services(run)[2]
    geometry = n.store._record_v2(run, "trace-geometry", contest["verdict_hash"])
    party = IslandParty.published(
        hot.ss58, IslandJobV1.model_validate(geometry["job"]), Path(geometry["directory"])
    )
    for _ in range(64):
        turn = disputes.get(dispute_id)
        if turn.paused:
            break
        key = (
            hot
            if turn.expected_party == hot.ss58
            else next(k for k in svc.AUDITORS if k.ss58 == turn.expected_party)
        )
        at = [turn.interval[0], -(-sum(turn.interval) // 2), turn.interval[1]]
        hashes = party.hashes(turn.level, tuple(turn.ctx), at)
        if key == hot:
            hashes = [
                digest if i == 0 else sha256_hex(bytes.fromhex(digest))
                for i, digest in enumerate(hashes)
            ]
        answer = BisectV2(
            dispute_id=dispute_id,
            seq=turn.seq,
            level=turn.level,
            ctx=turn.ctx,
            interval=turn.interval,
            N=2,
            hashes=hashes,
            party=key.ss58,
            previous_transcript_hash=turn.transcript_hash,
        )
        response = n.client.post(n.url + "/bisect", json=n.signed(key, "BisectV2", answer))
        precondition(response.status_code == 200, f"/bisect: {response.text[:160]}")
    turn = disputes.get(dispute_id)
    precondition(turn.paused, "the bisection did not reach the referee")
    response = n.client.post(n.url + f"/admin/disputes/{dispute_id}/referee", headers=svc.admin())
    precondition(response.status_code == 200, f"/referee: {response.text[:160]}")
    precondition(
        (response.json()["evidence"]["reason"], response.json()["evidence"]["loser"])
        == ("FRAUD", hot.ss58),
        f"the referee did not rule FRAUD against the miner: {response.json()['evidence']}",
    )
    resolution = ResolutionV2(
        dispute_id=dispute_id,
        transcript_hash=turn.transcript_hash,
        reason="FRAUD",
        loser=hot.ss58,
        evidence_hash=response.json()["evidence_hash"],
    )
    response = n.client.post(
        n.url + "/resolution", json=n.signed(svc.REFEREE, "ResolutionV2", resolution)
    )
    precondition(response.status_code == 200, f"/resolution: {response.text[:160]}")
    return {"dispute_id": dispute_id, "resolution": resolution.digest()}
