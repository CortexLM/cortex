from __future__ import annotations

import hashlib
import json
import sys
from dataclasses import dataclass, field, replace
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "trainer"))

import torch  # noqa: E402
from fastapi import FastAPI, Header, HTTPException, Request, Response  # noqa: E402
from trainer_fixtures import assignment, sample, small_manifest_body  # noqa: E402

from hypertrain.auditor.replay import (  # noqa: E402
    challenge_hash,
    forfeit_for,
    pack_state,
    replay_windows,
    tensor_root,
)
from hypertrain.protocol.envelope import Intake  # noqa: E402
from hypertrain.protocol.hashing import MerkleTree, sha256_hex  # noqa: E402
from hypertrain.protocol.keys import Keypair  # noqa: E402
from hypertrain.protocol.messages import (  # noqa: E402
    AuditChallenge,
    Commit,
    LeafPreimage,
    ReplayEnv,
    RunManifest,
    StateServe,
)
from hypertrain.trainer.compress import compress, payload_hash, state_hash  # noqa: E402
from hypertrain.trainer.config import TrainConfig  # noqa: E402
from hypertrain.trainer.loop import Assignment, Params, RoundResult, StepHook  # noqa: E402
from hypertrain.trainer.model import init_params  # noqa: E402
from hypertrain.trainer.optim import OptState, init_state  # noqa: E402

AUDITOR = Keypair(b"\x07" * 32)
MINER = Keypair(b"\x08" * 32)
TOKEN = "auditor-token-for-tests"
W = 3
SERVE_DEADLINE = 500
ENV = ReplayEnv(
    image_digest="sha256:" + "1" * 64,
    driver="cpu",
    gpu_uuid_sha256=sha256_hex(b"cpu"),
    sm_count=1,
)


def manifest(policy: str = "reset", H: int = 6, J: int = 2) -> RunManifest:
    body: Any = small_manifest_body()
    body["inner"].update(state_policy=policy, H=H, J=J)
    body["auditors"] = [AUDITOR.ss58]
    return RunManifest.model_validate(body)


@dataclass
class World:
    m: RunManifest
    cfg: TrainConfig
    theta: Params
    a: Assignment

    @property
    def run_id(self) -> str:
        return self.m.run_id()


def world(policy: str = "reset", H: int = 6, J: int = 2) -> World:
    m = manifest(policy, H, J)
    cfg = TrainConfig.from_manifest(m.body())
    ids = assignment(cfg).sample_ids
    return World(m, cfg, init_params(cfg.model), Assignment(m.run_id(), W, ids))


def get_sample(w: World) -> Any:
    return sample(w.cfg)


def make_commit(w: World, res: RoundResult, hotkey: str = MINER.ss58) -> Commit:
    metrics = [bytes.fromhex(x.preimage.loss_f32 + x.preimage.norm_f32) for x in res.leaves]
    return Commit(
        w=W,
        hotkey=hotkey,
        leaf_scheme="ht-leaf-v1",
        n_leaves=len(res.leaves),
        leaves_root=res.leaves_root,
        metrics_root=MerkleTree(metrics).root.hex(),
        final_theta_hash=res.final_theta_hash,
        ef_in_hash=res.ef_in_hash,
        ef_out_hash=res.ef_out_hash,
        delta_hash=res.delta_hash,
        delta_bytes=len(res.delta_payload),
        tokens=len(w.a.sample_ids) * w.cfg.model.seq_len,
    )


def make_challenge(
    mode: str = "full",
    segments: list[tuple[int, int]] | None = None,
    beacon: str = "beacon-0",
    target: str = MINER.ss58,
) -> AuditChallenge:
    return AuditChallenge(
        w=W,
        target=target,
        beacon_round=400,
        beacon_sig_sha256=sha256_hex(beacon.encode()),
        mode=mode,  # type: ignore[arg-type]
        segments=segments or [],
        reasons=["random"],
        serve_deadline=SERVE_DEADLINE,
    )


@dataclass
class CarryRound:
    """A carry-policy miner run kept window by window so it can serve StateServe blobs."""

    result: RoundResult
    states: list[tuple[Params, OptState]]


def carry_round(w: World, after_last_step: StepHook | None = None) -> CarryRound:
    cfg, g = w.cfg, get_sample(w)
    theta = {n: x.clone() for n, x in w.theta.items()}
    st = init_state(replace(cfg.inner, state_policy="reset"), theta)
    states = [(theta, st)]
    pres: list[LeafPreimage] = []
    from hypertrain.trainer.loop import _make_leaf

    pres.append(_make_leaf(cfg, w.a, 0, theta, st, 0.0, 0.0).preimage)
    u = cfg.inner.n_leaves - 1
    for i in range(u):
        hook = after_last_step if i == u - 1 else None
        p, theta, st = replay_windows(cfg, theta, st, w.a, i, i + 1, g, hook)
        pres.extend(p)
        states.append((theta, st))
    from hypertrain.trainer.loop import LeafRecord

    leaves = [LeafRecord(p, bytes.fromhex(p.digest())) for p in pres]
    ef = {n: torch.zeros_like(x) for n, x in w.theta.items()}
    payload, ef_out = compress(cfg.compress, {n: w.theta[n] - theta[n] for n in theta}, ef)
    res = RoundResult(
        leaves=leaves,
        leaves_root=MerkleTree([x.digest for x in leaves]).root.hex(),
        final_theta=theta,
        final_state=st,
        final_theta_hash=state_hash(theta),
        delta_payload=payload,
        delta_hash=payload_hash(payload),
        ef_in_hash=state_hash(ef),
        ef_out=ef_out,
        ef_out_hash=state_hash(ef_out),
    )
    return CarryRound(res, states)


def serve_for(
    w: World, cr: CarryRound, ch: AuditChallenge, leaf: int, hotkey: str = MINER.ss58
) -> tuple[StateServe, bytes]:
    theta, st = cr.states[leaf]
    tree = MerkleTree(cr.result.leaf_digests)
    blob = pack_state(theta, st)
    return (
        StateServe(
            hotkey=hotkey,
            challenge_hash=challenge_hash(ch),
            t=leaf * w.cfg.inner.J,
            uri=f"objects/{hashlib.sha256(blob).hexdigest()}",
            tensor_root=tensor_root(theta, st),
            merkle_proof_leaf_in_leaves_root=[p.hex() for p in tree.proof(leaf)],
        ),
        blob,
    )


@dataclass
class FakeChallenge:
    """In-process stand-in for the todo-8 auditor routes (see hypertrain.auditor.worker)."""

    m: RunManifest
    now_round: int = 450
    objects: dict[str, bytes] = field(default_factory=dict)
    jobs: dict[str, dict[str, Any]] = field(default_factory=dict)
    queue: list[str] = field(default_factory=list)
    serves: dict[str, list[dict[str, Any]]] = field(default_factory=dict)
    verdicts: dict[str, dict[str, Any]] = field(default_factory=dict)
    failures: list[dict[str, Any]] = field(default_factory=list)
    forfeits: list[dict[str, Any]] = field(default_factory=list)
    state: dict[str, str] = field(default_factory=dict)

    def put(self, data: bytes) -> str:
        sha = hashlib.sha256(data).hexdigest()
        self.objects[sha] = data
        return sha

    def add_job(self, job_id: str, job: dict[str, Any]) -> None:
        self.jobs[job_id] = {"id": job_id, "lease": f"lease-{job_id}", **job}
        self.queue.append(job_id)
        self.state[job_id] = "queued"

    def app(self) -> FastAPI:
        app = FastAPI()
        intake = Intake(
            self.m.run_id(), allowed_signers={"ReplayVerdict": lambda s: s in self.m.auditors}
        )

        def auth(authorization: str | None) -> None:
            if authorization != f"Bearer {TOKEN}":
                raise HTTPException(401, "bad token")

        def leased(job: str, lease: str) -> dict[str, Any]:
            j = self.jobs.get(job)
            if j is None or j["lease"] != lease or self.state[job] != "leased":
                raise HTTPException(409, "no such lease")
            return j

        @app.post("/v1/worker/lease")
        def lease(authorization: str | None = Header(None)) -> Any:
            auth(authorization)
            if not self.queue:
                return Response(status_code=204)
            job = self.queue.pop(0)
            self.state[job] = "leased"
            return self.jobs[job]

        @app.post("/v1/worker/jobs/{job}/heartbeat")
        async def heartbeat(job: str, request: Request, authorization: str | None = Header(None)):
            auth(authorization)
            leased(job, (await request.json())["lease"])
            return {"ok": True}

        @app.get("/v1/worker/jobs/{job}/serves")
        def serves(job: str, lease: str, authorization: str | None = Header(None)) -> Any:
            auth(authorization)
            leased(job, lease)
            return {"now_round": self.now_round, "serves": self.serves.get(job, [])}

        @app.get("/v1/objects/{sha}")
        def obj(sha: str) -> Response:
            if sha not in self.objects:
                raise HTTPException(404, "no object")
            return Response(self.objects[sha], media_type="application/octet-stream")

        @app.post("/v1/worker/jobs/{job}/complete")
        async def verdict(job: str, request: Request, authorization: str | None = Header(None)):
            auth(authorization)
            body = await request.json()
            j = leased(job, body["lease"])
            try:
                v = intake.accept(body["verdict"], self.now_round)
            except ValueError as error:
                raise HTTPException(422, f"rejected: {error}") from None
            if v.model_dump()["challenge_hash"] != challenge_hash(
                AuditChallenge.model_validate(j["challenge"])
            ):
                raise HTTPException(422, "verdict for another challenge")
            self.verdicts[job] = body["verdict"]
            self.state[job] = "closed"
            result = v.model_dump()["result"]
            if result != "MATCH":
                f = forfeit_for(
                    W, j["commit"]["hotkey"], result, [v.model_dump()["challenge_hash"]], 1000, 5000
                )
                self.forfeits.append(f.model_dump(mode="json"))
            return {"id": job, "state": "done", "result": result}

        @app.post("/v1/worker/jobs/{job}/fail")
        async def fail(job: str, request: Request, authorization: str | None = Header(None)):
            auth(authorization)
            body = await request.json()
            leased(job, body["lease"])
            self.failures.append({"job": job, **body})
            if body["retry"]:
                self.state[job] = "queued"
                self.queue.append(job)
            else:
                self.state[job] = "failed"
            return {"ok": True}

        return app


def job_json(
    w: World,
    fc: FakeChallenge,
    res: RoundResult,
    ch: AuditChallenge,
    commit: Commit | None = None,
) -> dict[str, Any]:
    c = commit or make_commit(w, res)
    return {
        "manifest": w.m.body(),
        "challenge": ch.model_dump(mode="json"),
        "commit": c.model_dump(mode="json"),
        "leaves": [x.digest.hex() for x in res.leaves],
        "preimages": [x.preimage.model_dump(mode="json") for x in res.leaves],
        "assignment": {"sample_ids": list(w.a.sample_ids), "global_step0": w.a.global_step0},
        "theta_start_sha256": fc.put(pack_state(w.theta)),
        "ef_in_sha256": None,
        "v0_sha256": None,
    }


def dumps(x: Any) -> str:
    return json.dumps(x, sort_keys=True)
