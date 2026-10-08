"""One e2e actor OS process: a miner, the auditor or the aggregator/coordinator.

usage: python actor.py ROLE CONFIG_JSON
Prints `READY` once built, then reads one JSON command per stdin line ({"cmd", "args"}) and
answers each with one `RESULT <json>` stdout line; logs go to stderr. Every actor talks to the
challenge app over HTTP only; fault injection is selected by config/command flags.
"""

from __future__ import annotations

import functools
import json
import logging
import os
import sys
import traceback
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent))

import common  # noqa: E402,I001  (determinism before torch)
import httpx  # noqa: E402
import numpy as np  # noqa: E402
import torch  # noqa: E402
from trainer_fixtures import sample  # noqa: E402

import hypertrain.miner.core as core  # noqa: E402
from hypertrain.data.store import LocalFSStore  # noqa: E402
from hypertrain.protocol.envelope import seal  # noqa: E402
from hypertrain.protocol.hashing import MerkleTree  # noqa: E402
from hypertrain.protocol.messages import Commit, RunManifest  # noqa: E402
from hypertrain.trainer.compress import state_hash  # noqa: E402
from hypertrain.trainer.config import TrainConfig  # noqa: E402
from hypertrain.trainer.loop import Assignment, train_round  # noqa: E402
from hypertrain.trainer.model import init_params  # noqa: E402

log = logging.getLogger("e2e-actor")


class Relay:
    """Beacon relay: drand payloads are public, any actor may push the next ones."""

    def __init__(self, c: httpx.Client) -> None:
        self.c = c

    def latest(self) -> int:
        return int(self.c.get("/v1/beacon/latest").json()["round"])

    def push(self, rnd: int) -> None:
        for r in range(self.latest() + 1, rnd + 1):
            resp = self.c.post(
                "/v1/admin/beacon",
                json=common.beacon_payload(r),
                headers=common.bearer(common.ADMIN),
            )
            resp.raise_for_status()

    def tick(self, target: int) -> None:
        d = self.latest()
        self.push(max(d + 1, min(target, d + 60)))


def bump_at(step: int) -> Any:
    def hook(t: int, theta: Any) -> None:
        if t == step:
            with torch.no_grad():
                theta[sorted(theta)[0]].view(-1)[0] += common.DELTA

    return hook


def fabricated(cfg: Any, a: Any, theta0: Any, st0: Any, delta: Any) -> Any:
    """Leaves of a trajectory that never trained: theta_i = theta0 - (i/u) * delta with the
    start optimizer state, correct batch ids, and the given delta committed."""
    from hypertrain.trainer.compress import compress, payload_hash
    from hypertrain.trainer.loop import RoundResult, _make_leaf

    u, j = cfg.inner.n_leaves - 1, cfg.inner.J
    states = [
        ({n: theta0[n] - delta[n] * (i / u) for n in theta0}, st0.clone()) for i in range(u + 1)
    ]
    leaves = [
        _make_leaf(cfg, a, i * j, th, st, 2.5 if i else 0.0, 0.1 if i else 0.0)
        for i, (th, st) in enumerate(states)
    ]
    ef = {n: torch.zeros_like(x) for n, x in theta0.items()}
    payload, ef_out = compress(cfg.compress, delta, ef)
    res = RoundResult(
        leaves=leaves,
        leaves_root=MerkleTree([x.digest for x in leaves]).root.hex(),
        final_theta=states[-1][0],
        final_state=states[-1][1],
        final_theta_hash=state_hash(states[-1][0]),
        delta_payload=payload,
        delta_hash=payload_hash(payload),
        ef_in_hash=state_hash(ef),
        ef_out=ef_out,
        ef_out_hash=state_hash(ef_out),
    )
    return res, states


def _norm(d: dict[str, Any]) -> float:
    return float(torch.sqrt(sum((d[n].double() ** 2).sum() for n in d)))


class E2EMiner(core.Miner):
    """The real miner client; ``mode`` swaps only what it trains/serves/uploads.

    honest: Miner._train unchanged. averager: mean of the peer deltas (recomputed from their
    public assignments) rescaled to the median peer norm, fabricated leaves (V4 recipe).
    shifted: trains on sample ids + 1 (outside its assignment). laststep: perturbs the last
    inner step. fabricate: commits its true delta with fabricated leaves. noupload: never
    uploads. late: relays beacons to d_audit before committing.
    """

    mode = "honest"
    relay: Relay

    def forge(self, w: int, theta: Any, a: Any, carry: Any) -> tuple[Any, Any]:
        cfg = self.train_cfg
        if self.mode == "shifted":
            a = Assignment(a.run_id, a.w, tuple(i + 1 for i in a.sample_ids), a.global_step0)
        if self.mode == "laststep":
            hook = bump_at(cfg.inner.H)
            return train_round(cfg, theta, a, self.get_sample, carry=carry, after_step=hook), None
        if self.mode == "fabricate":
            res = train_round(cfg, theta, a, self.get_sample, carry=carry)
            delta = {n: theta[n] - res.final_theta[n] for n in theta}
            return fabricated(cfg, a, theta, carry, delta)
        if self.mode == "averager":
            deltas = []
            for row in self.view(w)["assignment"]:
                if row["hotkey"] == self.kp.ss58:
                    continue
                pa = Assignment(self.run_id, w, tuple(row["samples"]), w * cfg.inner.H)
                res = train_round(cfg, theta, pa, self.get_sample, carry=carry)
                deltas.append({n: theta[n] - res.final_theta[n] for n in theta})
            norms = sorted(_norm(d) for d in deltas)
            mean = {n: sum(d[n] for d in deltas) / len(deltas) for n in theta}
            med = (norms[1] + norms[2]) / 2
            scaled = {n: (mean[n] * (med / _norm(mean))).float() for n in mean}
            return fabricated(cfg, a, theta, carry, scaled)
        return train_round(cfg, theta, a, self.get_sample, carry=carry), None

    def _train(self, w: int, theta: Any, samples: tuple[int, ...], art: Path) -> None:
        if self.mode in ("honest", "noupload", "late"):
            return super()._train(w, theta, samples, art)
        from hypertrain.auditor.replay import pack_state

        cfg = self.train_cfg
        a = Assignment(self.run_id, w, samples, w * cfg.inner.H)
        carry = self._carry(w, theta)
        res, states = self.forge(w, theta, a, carry)
        if states is not None:
            self.__dict__.setdefault("forged_states", {})[w] = states
        metrics = [bytes.fromhex(x.preimage.loss_f32 + x.preimage.norm_f32) for x in res.leaves]
        commit = {
            "w": w,
            "hotkey": self.kp.ss58,
            "leaf_scheme": "ht-leaf-v1",
            "n_leaves": len(res.leaves),
            "leaves_root": res.leaves_root,
            "metrics_root": MerkleTree(metrics).root.hex(),
            "final_theta_hash": res.final_theta_hash,
            "ef_in_hash": res.ef_in_hash,
            "ef_out_hash": res.ef_out_hash,
            "delta_hash": res.delta_hash,
            "delta_bytes": len(res.delta_payload),
            "tokens": len(samples) * cfg.model.seq_len,
        }
        (art / "delta.bin").write_bytes(res.delta_payload)
        (art / "preimages.json").write_text(
            json.dumps([x.preimage.model_dump(mode="json") for x in res.leaves])
        )
        (art / "start_state.bin").write_bytes(pack_state(theta, carry))
        (art / "final_state.bin").write_bytes(pack_state(res.final_theta, res.final_state))
        (art / "samples.json").write_text(json.dumps(list(samples)))
        (art / "commit.json").write_text(json.dumps(commit, sort_keys=True))
        self.journal.append("trained", w=w, leaves_root=res.leaves_root)

    def _states(self, w: int) -> list[Any]:
        got = self.__dict__.get("forged_states", {}).get(w)
        return got if got is not None else super()._states(w)

    def _commit(self, w: int, commit: Any) -> bool:
        if self.mode == "late":
            self.relay.push(self.round_open(self.view(w)).d_audit)
        return super()._commit(w, commit)

    def _upload(self, w: int, commit: Any, payload: bytes) -> None:
        if self.mode == "noupload":
            self.journal.append("uploaded", w=w, status="NO_UPLOAD")
        else:
            super()._upload(w, commit, payload)


class MinerActor:
    def __init__(self, cfg: dict[str, Any]) -> None:
        m = RunManifest.model_validate(json.loads(Path(cfg["manifest"]).read_text()))
        tcfg = TrainConfig.from_manifest(m.body())
        get = sample(tcfg)
        if cfg.get("poison"):
            vocab = tcfg.model.vocab
            get = functools.partial(lambda g, i: ((g(i) + 1) % vocab).astype(np.uint32), get)
        if cfg.get("bump_step"):
            step = int(cfg["bump_step"])
            from hypertrain.auditor.bisect import Executor, Fault

            core.train_round = functools.partial(core.train_round, after_step=bump_at(step))
            fault = Fault(step, tcfg.model.n_layers, "update", common.DELTA)
            core.Executor = functools.partial(Executor, fault=fault)  # type: ignore[misc]
        self.c = httpx.Client(base_url=cfg["api"], timeout=120)
        self.relay = Relay(self.c)
        objects = LocalFSStore(Path(cfg["objects"]))
        mcfg = core.MinerConfig(
            api=cfg["api"],
            keyfile=Path(cfg["keyfile"]),
            workdir=Path(cfg["workdir"]),
            state_source=cfg["states"],
            image_digest=m.reference_spec.image_digest,
            run_id=m.run_id(),
            allow_file_upload=True,
        )
        self.m = E2EMiner(
            mcfg, httpx.Client(timeout=120), get, wait=self.relay.tick, blob_sink=objects.put
        )
        self.m.relay = self.relay
        self.m.mode = cfg.get("mode", "honest")
        self.manifest = m

    def run_round(self, w: int, mode: str | None = None) -> str:
        if mode is not None:
            self.m.mode = mode
        return self.m.run_round(w)

    def serve(self, w: int) -> int:
        return self.m.serve_challenges(w)

    def dispute(self, w: int) -> str | None:
        return self.m.dispute(w)

    def bisect(self, w: int, dispute_id: str, interval: list[int], n: int) -> dict[str, Any]:
        return self.m.bisect(w, dispute_id, (interval[0], interval[1]), n)

    def last(self, kind: str, w: int) -> Any:
        return self.m.journal.last(kind, w=w)

    def signed_probes(self, w: int, other_keyfile: str, stranger_keyfile: str) -> dict[str, int]:
        """Hostile client: well-formed and malformed signed Accepts; returns HTTP codes."""
        from hypertrain.challenge.store import assignment_hash
        from hypertrain.miner.core import load_keyfile

        kp, run_id = self.m.kp, self.m.run_id
        other, stranger = load_keyfile(Path(other_keyfile)), load_keyfile(Path(stranger_keyfile))
        d = self.relay.latest()
        row = next(a for a in self.m.view(w)["assignment"] if a["hotkey"] == kp.ss58)
        body = {
            "w": w,
            "hotkey": kp.ss58,
            "image_digest": self.manifest.reference_spec.image_digest,
            "assignment_hash": assignment_hash(run_id, w, row["slot"], tuple(row["samples"])),
            "driver_version": "cpu",
            "n_gpus": 1,
        }

        def post(path: str, env: Any) -> int:
            return self.c.post(f"/v1/runs/{run_id}/{path}", json=env).status_code

        good = seal(kp, "Accept", run_id, body, 2**40)
        return {
            "expired": post("accept", seal(kp, "Accept", run_id, body, d - 1)),
            "other_run": post("accept", seal(kp, "Accept", "ab" * 32, body, 2**40)),
            "type_swap": post("commit", good),
            "type_relabel": post("commit", {**good, "type": "Commit"}),
            "bad_sig": post("accept", {**good, "sig": "00" * 64}),
            "accepted": post("accept", good),
            "replay": post("accept", good),
            "other_w": post("accept", seal(kp, "Accept", run_id, {**body, "w": w + 1}, 2**40)),
            "unregistered": post(
                "accept",
                seal(stranger, "Accept", run_id, {**body, "hotkey": stranger.ss58}, 2**40),
            ),
            "signer_mismatch": post("accept", seal(other, "Accept", run_id, body, 2**40)),
        }


def install_no_replay() -> None:
    """HT_E2E_NO_REPLAY=1: keep every proof, assignment and serve-deadline check, but never
    recompute training (committed leaves are trusted)."""
    import hypertrain.auditor.replay as replay_mod
    import hypertrain.auditor.worker as worker_mod
    from hypertrain.auditor.replay import NOT_RECOMPUTED, Outcome, _precheck

    real_segments = replay_mod.audit_segments

    class Trust(Exception):
        pass

    def no_windows(*_: Any, **__: Any) -> Any:
        raise Trust

    def segments(*a: Any, **k: Any) -> Outcome:
        try:
            return real_segments(*a, **k)
        except Trust:
            return Outcome("MATCH", None, NOT_RECOMPUTED)

    def full(x: Any, *_: Any, **__: Any) -> Outcome:
        return _precheck(x) or Outcome("MATCH", None, NOT_RECOMPUTED)

    replay_mod.replay_windows = no_windows
    worker_mod.audit_full = full  # type: ignore[assignment]
    worker_mod.audit_segments = segments  # type: ignore[assignment]


class AuditorActor:
    def __init__(self, cfg: dict[str, Any]) -> None:
        from hypertrain.auditor.worker import Auditor, HttpApi

        if os.environ.get("HT_E2E_NO_REPLAY") == "1":
            install_no_replay()
            log.warning("HT_E2E_NO_REPLAY=1: training replay disabled")
        self.m = RunManifest.model_validate(json.loads(Path(cfg["manifest"]).read_text()))
        self.cfg = TrainConfig.from_manifest(self.m.body())
        self.get = sample(self.cfg)
        self.c = httpx.Client(base_url=cfg["api"], timeout=120)
        self.auditor = Auditor(
            HttpApi(self.c, common.WORKER), common.AUDITOR, common.CPU_ENV, self.get
        )

    def audit(self) -> list[str]:
        out = []
        while (r := self.auditor.run_once()) is not None:
            if r == "FAILED":
                raise RuntimeError(f"audit job failed after {out}")
            out.append(r)
        return out

    def _round0(self, hotkey: str) -> tuple[Any, Any, Any]:
        from dataclasses import replace

        from hypertrain.trainer.optim import init_state

        view = self.c.get(f"/v1/runs/{self.m.run_id()}/rounds/0").json()
        row = next(a for a in view["assignment"] if a["hotkey"] == hotkey)
        theta = init_params(self.cfg.model)
        st = init_state(replace(self.cfg.inner, state_policy="reset"), theta)
        return theta, st, Assignment(self.m.run_id(), 0, tuple(row["samples"]), 0)

    def bisect(self, dispute_id: str, target: str, interval: list[int], n: int) -> Any:
        from hypertrain.auditor.bisect import Executor, points

        theta, st, a = self._round0(target)
        ex = Executor(common.AUDITOR.ss58, self.cfg, theta, a, self.get, carry=st)
        body = {
            "dispute_id": dispute_id,
            "level": "step",
            "interval": interval,
            "N": n,
            "hashes": ex.hashes("step", (), points(interval[0], interval[1], n)),
            "party": common.AUDITOR.ss58,
        }
        env = seal(common.AUDITOR, "Bisect", self.m.run_id(), body, 2**40)
        r = self.c.post(f"/v1/runs/{self.m.run_id()}/bisect", json=env)
        r.raise_for_status()
        return r.json()

    def referee(
        self, dispute_id: str, target: str, n: int, fault: list[Any] | None, post: bool
    ) -> dict[str, Any]:
        """Referee run of the N-ary dispute between the auditor trajectory and the target's
        claimed one (``fault`` = (step, layer, op) the target's claims carry, None = honest)."""
        from hypertrain.auditor.bisect import Executor, Fault, run_dispute

        theta, st, a = self._round0(target)
        f = Fault(fault[0], fault[1], fault[2], common.DELTA) if fault else None
        good = Executor(common.AUDITOR.ss58, self.cfg, theta, a, self.get, carry=st)
        bad = Executor(target, self.cfg, theta, a, self.get, fault=f, carry=st)
        ref = Executor(common.REFEREE.ss58, self.cfg, theta, a, self.get, carry=st)
        res = run_dispute(dispute_id, self.cfg, good, bad, ref, n, (0, self.cfg.inner.H))
        out: dict[str, Any] = {
            "step": res.step,
            "layer": res.layer,
            "op": res.op,
            "rounds": res.rounds,
            "resolution": res.resolution.model_dump(mode="json"),
        }
        if post:
            env = seal(common.REFEREE, "Resolution", self.m.run_id(), res.resolution, 2**40)
            r = self.c.post(
                f"/v1/runs/{self.m.run_id()}/resolution",
                json=env,
                headers=common.bearer(common.WORKER),
            )
            r.raise_for_status()
            out["posted"] = r.json()
        return out


class AggregatorActor:
    """Coordinator: real Aggregator + K_coord; signs RoundOpen/Finalize, publishes states."""

    def __init__(self, cfg: dict[str, Any]) -> None:
        from hypertrain.aggregator.core import Aggregator, OuterParams

        self.m = RunManifest.model_validate(json.loads(Path(cfg["manifest"]).read_text()))
        self.run_id = self.m.run_id()
        self.objects = LocalFSStore(Path(cfg["objects"]))
        self.states = Path(cfg["states"])
        self.c = httpx.Client(base_url=cfg["api"], timeout=120)
        self.h = common.bearer(common.ADMIN)
        self.agg = Aggregator(
            self.objects,
            self.run_id,
            OuterParams.from_manifest(self.m),
            common.COORD,
            Path(cfg["state_dir"]),
            anchor=cfg.get("anchor"),
        )
        self.state_key: dict[int, str] = {}
        self.tapes: dict[int, dict[str, Any]] = {}
        self._template: dict[str, Any] = {}

    def _ok(self, r: httpx.Response) -> Any:
        if r.status_code not in (200, 201):
            raise RuntimeError(f"{r.request.url}: {r.status_code} {r.text[:500]}")
        return r.json()

    def genesis(self) -> str:
        from hypertrain.aggregator.core import OuterState

        theta = init_params(TrainConfig.from_manifest(self.m.body()).model)
        self.state_key[0] = self.agg.put_state(
            OuterState.init({n: x.numpy() for n, x in theta.items()})
        )
        return self.state_key[0]

    def publish(self, w: int) -> str:
        from hypertrain.aggregator.core import load_state
        from hypertrain.auditor.replay import pack_state

        p = load_state(self.objects, self.state_key[w]).theta
        theta = {n: torch.from_numpy(np.array(a, dtype=np.float32)) for n, a in p.items()}
        blob = pack_state(theta)
        (self.states / state_hash(theta)).write_bytes(blob)
        sha = self.objects.put(blob)
        self._ok(
            self.c.put(
                f"/v1/aggregator/runs/{self.run_id}/rounds/{w}/state",
                json={"theta_start_sha256": sha},
                headers=self.h,
            )
        )
        return sha

    def commits(self, w: int, exclude: list[str] = ()) -> list[Commit]:  # type: ignore[assignment]
        inputs = self._ok(
            self.c.get(f"/v1/aggregator/runs/{self.run_id}/rounds/{w}/inputs", headers=self.h)
        )
        self._template = inputs.get("next_round_template")
        return [
            Commit.model_validate(m["commit"]["body"])
            for m in inputs["miners"]
            if m["delta_manifest"] is not None and m["hotkey"] not in exclude
        ]

    def commit_list(self, w: int) -> list[dict[str, Any]]:
        return [c.model_dump(mode="json") for c in self.commits(w)]

    def _open_next(self, w: int, tape: dict[str, Any]) -> dict[str, Any]:
        tb = tape["body"]
        self.tapes[w], self.state_key[w + 1] = tape, tb["out_state"]
        ro = {
            **self._template,
            "prev_final_hash": tb["theta_hash"],
            "theta_hash": tb["theta_hash"],
            "outer_state_hash": tb["outer_state_hash"],
            "center_hash": tb["center_hash"],
        }
        env = seal(common.COORD, "RoundOpen", self.run_id, ro, 2**40)
        self._ok(
            self.c.post(
                f"/v1/aggregator/runs/{self.run_id}/rounds/{w}/aggregate", json=env, headers=self.h
            )
        )
        self.publish(w + 1)
        return tape

    def aggregate(self, w: int, exclude: list[str] = ()) -> dict[str, Any]:  # type: ignore[assignment]
        commits = self.commits(w, exclude)
        tape = self.agg.apply_round(w, self.state_key[w], commits, exclude)
        return self._open_next(w, tape)

    def regional(self, w: int, regions: dict[str, list[str]]) -> dict[str, Any]:
        commits = {c.hotkey: c for c in self.commits(w)}
        start = self.agg.regional_start(self.state_key[w])
        chains = {
            r: [self.agg.regional_merge(w, r, 1, start, [commits[h] for h in hks])]
            for r, hks in regions.items()
        }
        tape = self.agg.global_from_regions(w, self.state_key[w], chains, 1)
        self._open_next(w, tape)
        return {"tape": tape, "chains": chains, "contributors": sorted(self.agg.contributors(tape))}

    def rollback(self, w: int, bad: list[str], post: bool = True) -> dict[str, Any]:
        rb = self.agg.rollback(self.tapes[w], self.tapes[w + 1], bad, ["ab" * 32], 2**40)
        if post:
            self._ok(
                self.c.post(
                    f"/v1/aggregator/runs/{self.run_id}/rounds/{w}/rollback",
                    json=rb.envelope,
                    headers=self.h,
                )
            )
        self.tapes[w], self.tapes[w + 1] = rb.tapes
        return {"envelope": rb.envelope, "state_key": rb.state_key, "tapes": list(rb.tapes)}

    def finalize(self, w: int) -> dict[str, Any]:
        view = self._ok(self.c.get(f"/v1/runs/{self.run_id}/rounds/{w}"))
        excl = (view["rollback"] or {}).get("body", {}).get("excluded", [])
        included = sorted(
            m["hotkey"]
            for m in view["miners"]
            if m["delta_manifest"] is not None
            and m["hotkey"] not in excl
            and (m["status"] == "MATCH" or (m["status"] == "UPLOADED" and not m["selected"]))
        )
        body = {
            "w": w,
            "final_theta_hash_w1": "11" * 32,
            "included": included,
            "entitlements_root": "22" * 32,
        }
        env = seal(common.COORD, "Finalize", self.run_id, body, 2**40)
        r = self.c.post(
            f"/v1/aggregator/runs/{self.run_id}/rounds/{w}/finalize", json=env, headers=self.h
        )
        return {"status": r.status_code, "json": r.json()}

    def finalize_round(self, w: int) -> None:
        self.agg.finalize_round(w)

    def replay_rounds(
        self, start_key: str, commits: list[dict[str, Any]], exclude: list[str], rollback: list[str]
    ) -> dict[str, Any]:
        """Independent recomputation in this process's own journal: apply w=0 (commits minus
        ``exclude``) and an empty w=1, then optionally roll w=0 back without ``rollback``."""
        cs = [Commit.model_validate(c) for c in commits if c["hotkey"] not in exclude]
        t0 = self.agg.apply_round(0, start_key, cs)
        t1 = self.agg.apply_round(1, t0["body"]["out_state"], [])
        out: dict[str, Any] = {"t0": t0, "t1": t1}
        if rollback:
            rb = self.agg.rollback(t0, t1, rollback, ["ab" * 32], 2**40)
            out["rollback_state_key"] = rb.state_key
            out["rollback_tapes"] = list(rb.tapes)
        return out

    def journal(self) -> dict[str, Any]:
        return {"anchor": self.agg.journal.anchor, "path": str(self.agg.journal.path)}

    def keys(self) -> dict[str, Any]:
        return {"state_key": self.state_key, "tapes": self.tapes}


ROLES = {"miner": MinerActor, "auditor": AuditorActor, "aggregator": AggregatorActor}


def main() -> int:
    logging.basicConfig(
        stream=sys.stderr, level=logging.INFO, format="%(asctime)s %(name)s %(message)s"
    )
    role, cfg = sys.argv[1], json.loads(Path(sys.argv[2]).read_text())
    log.info("starting %s pid=%d", role, os.getpid())
    actor = ROLES[role](cfg)
    print("READY", flush=True)
    for line in sys.stdin:
        cmd = json.loads(line)
        if cmd["cmd"] == "exit":
            break
        log.info("cmd %s %s", cmd["cmd"], json.dumps(cmd.get("args", {}))[:300])
        try:
            out: dict[str, Any] = {"ok": getattr(actor, cmd["cmd"])(**cmd.get("args", {}))}
        except Exception as error:  # noqa: BLE001 - reported to the driver, which asserts on it
            traceback.print_exc()
            out = {"error": type(error).__name__, "msg": str(error)[:2000]}
        print("RESULT " + json.dumps(out, default=str), flush=True)
    log.info("exit %s", role)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
