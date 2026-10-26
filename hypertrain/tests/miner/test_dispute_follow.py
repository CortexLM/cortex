"""Follow-mode miner answers a funded STEP->LAYER->OP dispute over the real FastAPI service."""

from __future__ import annotations

import importlib.util
import os
import sys
import threading
from pathlib import Path
from typing import Any

import httpx
import pytest
import torch

from hypertrain.auditor.island_bisect import IslandParty, adjudicate
from hypertrain.challenge.disputes_v2 import WatchEvent
from hypertrain.miner.core import MinerConfig, NetworkMiner
from hypertrain.miner.dispute_follow import (
    PartyFactory,
    committed_execution,
    follow,
    network_watch,
)
from hypertrain.miner.island_launch import launch_island
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages_v2 import (
    BisectV2,
    DisputeV2,
    EscrowLock,
    IslandJobV1,
    ResolutionV2,
)
from hypertrain.trainer.island import TraceContext, emulate

spec = importlib.util.spec_from_file_location(
    "follow_service_fixture", Path(__file__).parents[1] / "challenge/test_service_network_v2.py"
)
assert spec is not None and spec.loader is not None
fixtures = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = fixtures
spec.loader.exec_module(fixtures)
network = fixtures.network
BASE = "https://service.test"
HOT, COLD, AUDITOR, REFEREE = (
    fixtures.HOT[0],
    fixtures.COLD[0],
    fixtures.AUDITORS[0],
    fixtures.REFEREE,
)


def tampered(hotkey: str, job: IslandJobV1, published: Path) -> IslandParty:
    """A dishonest party: one float in the step-1 attention output is perturbed."""
    run = committed_execution(job, published)
    hit = [False]

    def fault(ctx: TraceContext, x: torch.Tensor) -> torch.Tensor:
        if not hit[0] and ctx.step == 1 and ctx.op == "attn" and x.is_floating_point():
            hit[0] = True
            out = x.clone()
            out.reshape(-1)[0] += 0.1
            return out
        return x

    captures = [c for _, c in emulate(run.layout.n_gpus, lambda comm: run.trace(comm, fault=fault))]
    assert hit[0]
    return IslandParty(hotkey, captures, J=run.cfg.inner.J)


def honest(hotkey: str, job: IslandJobV1, published: Path) -> IslandParty:
    run = committed_execution(job, published)
    return IslandParty(hotkey, [c for _, c in emulate(run.layout.n_gpus, run.trace)], J=1)


SERIAL = threading.Lock()
ANSWERED = threading.Event()


def bridge(net: Any) -> httpx.MockTransport:
    def request(req: httpx.Request) -> httpx.Response:
        with SERIAL:
            response = net.client.request(
                req.method,
                str(req.url).replace(BASE, ""),
                content=req.content,
                headers=dict(req.headers),
            )
        if req.url.path.endswith("/bisect") and response.status_code == 200:
            ANSWERED.set()
        return httpx.Response(
            response.status_code, content=response.content, headers=dict(response.headers)
        )

    return httpx.MockTransport(request)


def committed_round(net: Any, tmp_path: Path) -> tuple[NetworkMiner, httpx.Client]:
    """Admitted miner with a real CPU-launched traced publication for round 0."""
    assert net.join(0).status_code == 200
    keyfile = tmp_path / "hot.key"
    keyfile.write_bytes(bytes([80]) * 32)
    os.chmod(keyfile, 0o600)
    cfg = MinerConfig(
        api=BASE,
        keyfile=keyfile,
        workdir=tmp_path / "miner",
        state_source="file://unused",
        image_digest=net.manifest.training.reference_spec.image_digest,
        run_id=net.manifest.run_id(),
        owner_hotkey=fixtures.OWNER.ss58,
    )
    client = httpx.Client(transport=bridge(net))
    miner = NetworkMiner(cfg, client)
    setup = fixtures.fixture.Setup(
        net.manifest, net.setup.policy, net.setup.admission_policy, net.setup.rows, net.setup.tree
    )
    directory = miner.directory / "0"
    job = fixtures.fixture.stage(
        setup, directory, tuple(range(net.manifest.training.batch_samples())), 0
    )
    launch_island(job, directory, backend="cpu", trace=True)
    return miner, client


def open_contest(net: Any, miner: NetworkMiner) -> str:
    """Accepted contest + trace geometry as L0 records them, then a funded signed dispute."""
    run_id = net.manifest.run_id()
    published = miner.directory / "0" / "published"
    job = IslandJobV1.model_validate_json((published / "job.json").read_bytes())
    party = honest(HOT.ss58, job, published)
    verdict_hash = sha256_hex(b"follow-verdict")
    admission_id = net.store._db.execute(
        "SELECT admission_id FROM admissions_v2 WHERE hotkey=?", (HOT.ss58,)
    ).fetchone()[0]
    layers = party.span("layer", (1,))
    contest = dict(
        verdict_hash=verdict_hash,
        challenge_hash=sha256_hex(b"follow-challenge"),
        w=0,
        miner=HOT.ss58,
        auditor=AUDITOR.ss58,
        referee=REFEREE.ss58,
        step_span=party.span("step", ()),
        layer_span=layers,
        op_span=max(party.span("op", (1, i)) for i in range(layers)),
        admission_id=admission_id,
        coldkey=COLD.ss58,
        evidence_hash=sha256_hex(b"follow-evidence"),
        auditor_coldkey=None,
    )
    with net.store._lock:
        net.store._put_record_v2(run_id, "contest", verdict_hash, contest)
        net.store._put_record_v2(
            run_id,
            "trace-geometry",
            verdict_hash,
            {"job": job.model_dump(mode="json"), "directory": str(published)},
        )
        net.store._put_record_v2(run_id, "finality", "0", {"w": 0})
    dispute_id = sha256_hex(canonicalize([run_id, verdict_hash, HOT.ss58]))
    origin = net.store._db.execute(
        "SELECT origin FROM escrow_units WHERE owner=? AND bucket='available' AND units>=100",
        (COLD.ss58,),
    ).fetchone()[0]
    lock = EscrowLock(
        operation_id="ce" * 32,
        owner=COLD.ss58,
        units=100,
        origin_ids=[origin],
        admission_id=None,
        dispute_id=dispute_id,
        kind="LOCK_CONTEST",
    )
    response = net.client.post(net.url + "/escrow/lock", json=net.signed(COLD, "EscrowLock", lock))
    assert response.status_code == 200, response.text
    body = DisputeV2(
        hotkey=HOT.ss58, verdict_hash=verdict_hash, action="contest", bond_lock=lock.operation_id
    )
    response = net.client.post(net.url + "/dispute", json=net.signed(HOT, "DisputeV2", body))
    assert response.status_code == 200, response.text
    return dispute_id


def auditor_turn(net: Any, cursor: int) -> WatchEvent:
    assert ANSWERED.wait(60), "miner follower did not answer within 60s"
    ANSWERED.clear()
    run_id = net.manifest.run_id()
    signature = AUDITOR.sign(f"hypertrain/watch/2|{run_id}|{AUDITOR.ss58}".encode()).hex()
    with SERIAL:
        response = net.client.get(
            net.url + "/disputes",
            params={"party": AUDITOR.ss58, "cursor": cursor, "timeout": 0},
            headers={"X-Dispute-Signature": signature},
        )
    assert response.status_code == 200, response.text
    events = [WatchEvent.model_validate(e) for e in response.json()]
    assert len(events) == 1
    return events[0]


class Follower:
    def __init__(self, miner: NetworkMiner, party: PartyFactory | None) -> None:
        self.watch = network_watch(miner, party=party)
        self.stop = threading.Event()
        self.errors: list[BaseException] = []
        self.thread = threading.Thread(target=self._run)
        self.thread.start()

    def _run(self) -> None:
        try:
            follow(self.watch, self.stop, timeout=1, idle=0.05)
        except BaseException as error:
            self.errors.append(error)

    def close(self) -> int:
        self.stop.set()
        self.thread.join(30)
        assert not self.thread.is_alive() and not self.errors, self.errors
        cursor = self.watch.cursor
        self.watch.close()
        return cursor


@pytest.mark.parametrize("cheater", ["auditor", "miner"])
def test_follow_answers_step_layer_op_and_honest_party_wins(
    network: Any, tmp_path: Path, cheater: str
) -> None:
    # Given: an admitted miner with a real committed round and a funded contest against it.
    miner, client = committed_round(network, tmp_path)
    published = miner.directory / "0" / "published"
    before = available(network)
    dispute_id = open_contest(network, miner)
    disputes = network.store._services(network.manifest.run_id())[2]
    job = IslandJobV1.model_validate_json((published / "job.json").read_bytes())
    miner_party = tampered if cheater == "miner" else None
    auditor = (tampered if cheater == "auditor" else honest)(AUDITOR.ss58, job, published)
    ANSWERED.clear()
    follower = Follower(miner, miner_party)
    restarted_at = None
    cursor, paused = 0, False
    try:
        # When: the auditor answers each turn the autonomous follower hands it.
        while not paused:
            event = auditor_turn(network, cursor)
            cursor, turn = event.cursor, event.turn
            if restarted_at is None:
                restarted_at = follower.close()
                follower = Follower(miner, miner_party)
                assert follower.watch.cursor == restarted_at > 0
            lo, hi = turn.interval
            at = [lo, -(-(lo + hi) // 2), hi]
            reply = BisectV2(
                dispute_id=dispute_id,
                seq=turn.seq,
                level=turn.level,
                ctx=turn.ctx,
                interval=turn.interval,
                N=2,
                hashes=auditor.hashes(turn.level, tuple(turn.ctx), at),
                party=AUDITOR.ss58,
                previous_transcript_hash=turn.transcript_hash,
            )
            with SERIAL:
                response = network.client.post(
                    network.url + "/bisect", json=network.signed(AUDITOR, "BisectV2", reply)
                )
            assert response.status_code == 200, response.text
            paused = response.json()["paused"]
    finally:
        follower.close()
        client.close()
    # Then: the miner answered at every granularity from its own replay, exactly once per seq.
    final = disputes.get(dispute_id)
    rows = network.store._db.execute(
        "SELECT body FROM dispute_entries_v2 WHERE id=? ORDER BY seq", (dispute_id,)
    ).fetchall()
    entries = [BisectV2.model_validate_json(r[0]) for r in rows]
    mine = [e for e in entries if e.party == HOT.ss58]
    assert [e.seq for e in entries] == list(range(len(entries)))
    assert {e.level for e in mine} == {"step", "layer", "op"}
    assert final.level == "op" and final.interval[1] - final.interval[0] == 1
    answering = (tampered if cheater == "miner" else honest)(HOT.ss58, job, published)
    referee = honest(REFEREE.ss58, job, published)
    evidence = adjudicate((dispute_id, final.transcript_hash), (answering, auditor), referee)
    loser = HOT.ss58 if cheater == "miner" else AUDITOR.ss58
    assert evidence.reason == "FRAUD" and evidence.loser == loser
    assert "op=attn" in evidence.op_spec
    assert "op=attn" in referee.op_name(tuple(final.ctx), final.interval[1])
    resolution = ResolutionV2(
        dispute_id=dispute_id,
        transcript_hash=final.transcript_hash,
        reason="FRAUD",
        loser=loser,
        evidence_hash=disputes.register_evidence(evidence),
    )
    response = network.client.post(
        network.url + "/resolution", json=network.signed(REFEREE, "ResolutionV2", resolution)
    )
    assert response.status_code == 200, response.text
    locked = network.store._db.execute(
        "SELECT COALESCE(SUM(units),0) FROM escrow_units WHERE owner=? AND bucket='dispute_locked'",
        (COLD.ss58,),
    ).fetchone()[0]
    assert locked == 0
    assert available(network) == before - (100 if cheater == "miner" else 0)


def available(net: Any) -> int:
    return int(
        net.store._db.execute(
            "SELECT COALESCE(SUM(units),0) FROM escrow_units WHERE owner=? AND bucket='available'",
            (COLD.ss58,),
        ).fetchone()[0]
    )
