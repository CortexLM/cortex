"""CPU network-v2 multi-round mining through the real CLI, up to a positive get_weights.

Reuses scripts/mine_smoke_v2.py: its chain (real challenge app on 127.0.0.1, TLS relay, fixture
keys, join-v2 -> escrow-lock -> 4 probation trials -> run-v2 --round 0 for miner 0) runs first.
Then, for rounds 0..ROUNDS-1, every roster miner (4 fixture hotkeys) runs
`uv run --frozen hypertrain-miner run-v2 --round w` as its own process; the operator role leases
and completes the 4 audits, aggregates, finalizes and opens the next round. After the last
round, beacon rounds pass the reward maturity and GET /internal/v1/get_weights (internal token
+ slug header, over HTTP) must return a positive weight for miner 0's hotkey.

Two more rounds probe restart: miner 0 reruns run-v2 after UPLOADED (the accepted delta and
relay receipts must not change), then is SIGKILLed once its island job is staged (training)
and rerun (must UPLOAD).

Writes evidence JSON to $MINE_MULTI_ROUND_V2_EVIDENCE (default: a fresh private temp file);
exit 0 = PASS. MINE_SMOKE_V2_BREAK=run-v2 points every run-v2 at a closed port;
MINE_MULTI_ROUND_V2_BREAK=mature asks get_weights before round 0 matures, which freezes that
epoch's answer unpaid (the final check must then FAIL).
"""

from __future__ import annotations

import contextlib
import json
import os
import secrets
import signal
import subprocess
import sys
import tempfile
import time
import tomllib
from pathlib import Path
from types import SimpleNamespace
from typing import Any

import mine_smoke_v2 as smoke

ROUNDS = 2


def capture(held: dict[str, Any]) -> Any:
    """The smoke's service module, with its network fixture handing back the live Network."""
    svc = smoke._load_service()
    fixture = svc.network.__wrapped__

    def network(path: Path, request: Any) -> Any:
        gen = fixture(path, request)
        try:
            held["n"] = n = next(gen)
            yield n
        finally:
            gen.close()

    svc.network = SimpleNamespace(__wrapped__=network)
    return svc


def miner_configs(tmp: Path) -> list[Path]:
    """miner.toml from the chain for HOT[0]; same keys, own fixture hotkey, for HOT[1..3]."""
    base = tomllib.loads((tmp / "miner.toml").read_text())
    out = [tmp / "miner.toml"]
    for i in (1, 2, 3):
        key = smoke._write_key(tmp / f"hot-{i}.key", bytes([80 + i]) * 32)
        conf = tmp / f"miner-{i}.toml"
        conf.write_text(
            "\n".join(f"{k} = {json.dumps(str(v))}" for k, v in {**base, "keyfile": key}.items())
        )
        out.append(conf)
    return out


def run_v2(stages: list[dict[str, Any]], conf: Path, w: int, i: int) -> None:
    out = json.loads(
        smoke.stage(
            stages,
            "run-v2",
            ["hypertrain-miner", "run-v2", "--config", str(conf), "--round", str(w)],
        )
    )
    stages[-1].update(miner=i, w=w, result=out)
    if out != {"status": "UPLOADED"}:
        raise RuntimeError(f"run-v2 miner {i} round {w}: {out}")


def open_round(svc: Any, n: Any, w: int, prev_final: str) -> Any:
    """Operator role: signed RoundOpenV2 for w >= 1 from the applied state and restored anchors."""
    import torch

    from hypertrain.aggregator.core import load_state
    from hypertrain.auditor.replay import AnchorCache, optimizer_hash, pack_state
    from hypertrain.data.store import put_large
    from hypertrain.protocol.hashing import sha256_hex
    from hypertrain.protocol.jcs import canonicalize
    from hypertrain.protocol.messages import f32hex
    from hypertrain.protocol.messages_v2 import PolicyHashes, RoundOpenV2, StartStateV2
    from hypertrain.trainer.compress import state_hash

    rid = n.manifest.run_id()
    applied = n.store._record_v2(rid, "applied", str(w - 1))
    theta = {
        k: torch.from_numpy(v.copy())
        for k, v in load_state(n.store.objects, applied["out_state"]).theta.items()
    }
    cache = AnchorCache()
    starts, roster = [], []
    for i, hot in enumerate(svc.HOT):
        record = n.store._services(rid)[1].status(hot.ss58, now=n.now).record
        anchor = n.store._restore_anchor_v2(rid, hot.ss58, w - 1, cache)
        starts.append(
            StartStateV2(
                run_id=rid,
                w=w,
                hotkey=hot.ss58,
                theta_hash=state_hash(theta),
                state_object_sha256=put_large(pack_state(theta, anchor.state), sha256_hex),
                opt_state_hash=optimizer_hash(anchor.state),
                ef_object_sha256=put_large(pack_state(anchor.ef), sha256_hex),
                ef_hash=state_hash(anchor.ef),
                parent_anchor_hash=anchor.anchor_hash,
                global_step0=anchor.state.step,
                anchor_verdict_hash=anchor.proof_hash,
            )
        )
        roster.append(
            {
                "hotkey": hot.ss58,
                "slot": i,
                "q_i": f32hex(1),
                "admission_id": record.admission_id,
                "coldkey_group": record.coldkey,
                "state": record.state,
                "eligible_weight": 4194304,
            }
        )
    opening = RoundOpenV2(
        w=w,
        prev_final_hash=prev_final,
        theta_hash=state_hash(theta),
        outer_state_hash="0" * 64,
        center_hash="0" * 64,
        roster_hash=sha256_hex(canonicalize(roster)),
        honeypot_commit="0" * 64,
        d_open=n.now + 1,
        d_assign=n.now + 2,
        d_commit=n.now + 30,
        d_audit=n.now + 31,
        d_upload=n.now + 200,
        d_final=n.now + 300,
        contract_version=2,
        policy_hashes=PolicyHashes.model_validate(
            {k: getattr(n.manifest.network, k) for k in PolicyHashes.model_fields}
        ),
        registry_epoch=0,
        start_state_index_hash=sha256_hex(canonicalize([s.body() for s in starts])),
        audit_mode="anchored-full",
        roster=roster,
    )
    ok(
        n.client.post(
            n.url + "/admin/rounds",
            json=n.signed(svc.COORD, "RoundOpenV2", opening),
            headers=svc.admin(),
        ),
        f"round {w} open",
    )
    n.push(opening.d_assign)
    return opening


def ok(response: Any, what: str) -> Any:
    if response.status_code != 200:
        raise RuntimeError(f"{what}: {response.status_code} {response.text[:500]}")
    return response.json()


def settle(svc: Any, n: Any, w: int) -> str:
    """Operator role: 4 anchored audits (MATCH), aggregate, finalize; returns the finalize hash."""
    from hypertrain.protocol.hashing import sha256_hex
    from hypertrain.protocol.jcs import canonicalize
    from hypertrain.protocol.messages import Finalize

    rid = n.manifest.run_id()
    n.push(n.store._record_v2(rid, "round", str(w))["body"]["d_audit"])
    ok(n.client.post(n.url + f"/admin/rounds/{w}/audits", headers=svc.admin()), "audits")
    for index in range(len(svc.HOT)):
        auditor = svc.AUDITORS[index % 2]

        def receipt(nonce: str, auditor: Any = auditor) -> Any:
            body = {"w": w, "commit_hash": nonce, "received_round": n.now}
            return n.signed(auditor, "Receipt", body)

        job = ok(
            n.client.post(n.url + "/worker/lease", json=receipt(f"{w * 4 + index + 1:064x}")),
            "lease",
        )["body"]
        url = n.url + "/worker/jobs/" + job["job_id"]
        verdict = ok(n.client.post(url + "/execute", json=receipt(job["lease_nonce"])), "execute")
        if verdict["verdict"]["result"] != "MATCH":
            raise RuntimeError(f"round {w} audit {index}: {verdict['verdict']['result']}")
        signed = n.signed(auditor, "ReplayVerdict", verdict["verdict"])
        ok(n.client.post(url + "/complete", json=signed), "complete")
    applied = ok(n.client.post(n.url + f"/admin/rounds/{w}/aggregate", headers=svc.admin()), "agg")
    final = Finalize(
        w=w,
        final_theta_hash_w1=applied["theta_hash"],
        included=sorted(k.ss58 for k in svc.HOT),
        entitlements_root="0" * 64,
    )
    settled = ok(
        n.client.post(
            n.url + f"/admin/rounds/{w}/finalize",
            json=n.signed(svc.COORD, "Finalize", final),
            headers=svc.admin(),
        ),
        "finalize",
    )
    if settled.get("status") != "SETTLED":
        raise RuntimeError(f"round {w} finalize: {settled.get('status')}")
    return sha256_hex(canonicalize(n.store._record_v2(rid, "finalize", str(w))["body"]))


def get_weights(svc: Any, n: Any, api: str, tmp: Path) -> dict[str, Any]:
    """GET /internal/v1/get_weights over HTTP, as Cortex asks: internal token + slug header."""
    import httpx

    route = next(
        r for r in n.client.app.routes if getattr(r, "path", "") == "/internal/v1/get_weights"
    )
    code = route.endpoint.__code__
    config = route.endpoint.__closure__[code.co_freevars.index("config")].cell_contents
    token = secrets.token_hex(16)  # throwaway, never written to the evidence
    (tmp / "internal").write_text(token)
    config.internal_token_file = tmp / "internal"
    epoch_at = int(n.store.clock())
    epoch = (epoch_at - n.store.params.genesis_unix) // n.store.params.epoch_seconds
    response = httpx.get(
        f"{api}/internal/v1/get_weights",
        params={"epoch": epoch, "epoch_at": epoch_at},
        headers={"Authorization": f"Bearer {token}", "x-platform-challenge-slug": config.slug},
        timeout=60,
    )
    return {"status_code": response.status_code, "epoch": epoch, "answer": response.json()}


def restart_probes(
    stages: list[dict[str, Any]],
    svc: Any,
    n: Any,
    configs: list[Path],
    prev: str,
    tmp: Path,
    probe: dict[str, Any],
) -> None:
    """Round ROUNDS: miner 0 reruns run-v2 after UPLOADED (a fresh process, as after a kill
    between rounds); the accepted delta and relay receipts must not change. Round ROUNDS+1:
    miner 0's run-v2 is SIGKILLed once its island job is staged (it trains), then rerun; it
    must end UPLOADED (resume)."""
    rid, hot = n.manifest.run_id(), svc.HOT[0].ss58
    env = {**os.environ, "OMP_NUM_THREADS": "1", "MKL_NUM_THREADS": "1"}

    def argv(w: int) -> list[str]:
        conf = ["--config", str(configs[0]), "--round", str(w)]
        return ["uv", "run", "--frozen", "hypertrain-miner", "run-v2", *conf]

    def count(kind: str, identity: str | None = None) -> int:
        sql = "SELECT COUNT(*) FROM records_v2 WHERE run_id=? AND kind=?"
        args = (rid, kind) if identity is None else (rid, kind, identity)
        return n.store._db.execute(sql + (" AND id=?" if identity else ""), args).fetchone()[0]

    w = open_round(svc, n, ROUNDS, prev).w
    for i, conf in enumerate(configs):
        run_v2(stages, conf, w, i)
    delta, receipts = n.store._record_v2(rid, "delta", f"{w}:{hot}"), count("relay-receipt")
    replay = subprocess.run(argv(w), env=env, capture_output=True, text=True)  # noqa: S603
    stages.append({"name": "run-v2-replay", "mode": "subprocess", "probe": True, "w": w})
    stages[-1].update(rc=replay.returncode, stderr_tail=replay.stderr[-1500:])
    probe["after_upload"] = {
        "rc": replay.returncode,
        "stdout": replay.stdout.strip()[-200:],
        "delta_unchanged": n.store._record_v2(rid, "delta", f"{w}:{hot}") == delta,
        "relay_receipts_unchanged": count("relay-receipt") == receipts,
    }
    if not (
        probe["after_upload"]["rc"] == 0
        and probe["after_upload"]["delta_unchanged"]
        and probe["after_upload"]["relay_receipts_unchanged"]
    ):
        raise RuntimeError(f"rerun after UPLOADED not a clean no-op: {probe}")

    w = open_round(svc, n, ROUNDS + 1, settle(svc, n, w)).w
    for i, conf in enumerate(configs[1:], 1):
        run_v2(stages, conf, w, i)
    proc = subprocess.Popen(  # noqa: S603 - fixed argv, no shell
        argv(w),
        env=env,
        start_new_session=True,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    deadline = time.monotonic() + 120
    while (
        not count("island-job", f"{w}:{hot}")
        and proc.poll() is None
        and time.monotonic() < deadline
    ):
        time.sleep(0.1)
    alive = proc.poll() is None
    if alive:
        os.killpg(proc.pid, signal.SIGKILL)
    proc.wait()
    smoke.reap(str(tmp))
    probe["kill_mid_round"] = {
        "killed_after_job_staged": alive and bool(count("island-job", f"{w}:{hot}"))
    }
    if not probe["kill_mid_round"]["killed_after_job_staged"]:
        raise RuntimeError("kill probe did not interrupt a staged job")
    try:
        run_v2(stages, configs[0], w, 0)
        probe["kill_mid_round"]["resume"] = "UPLOADED"
    except RuntimeError as e:
        probe["kill_mid_round"]["resume"] = str(e)[:200]
        raise RuntimeError(f"resume after SIGKILL mid-round: {e}") from None
    finally:
        probe["kill_mid_round"]["delta_recorded"] = bool(count("delta", f"{w}:{hot}"))


def main() -> int:
    ev: dict[str, Any] = {"stages": []}
    env = os.environ.get("MINE_MULTI_ROUND_V2_EVIDENCE")
    fd = os.open(env, os.O_WRONLY | os.O_CREAT | os.O_NOFOLLOW, 0o600) if env else None
    out = Path(env) if env else None
    if fd is None:
        fd, name = tempfile.mkstemp(prefix="mine-multi-round-v2-evidence-", suffix=".json")
        out = Path(name)
    started = time.monotonic()
    signal.signal(signal.SIGTERM, smoke._budget_exceeded)
    held: dict[str, Any] = {}
    try:
        with tempfile.TemporaryDirectory() as d, contextlib.ExitStack() as stack:
            tmp = Path(d)
            svc = capture(held)
            smoke._load_service = lambda: svc
            smoke.chain(tmp, ev, stack)
            n, stages = held["n"], ev["stages"]
            api = tomllib.loads((tmp / "miner.toml").read_text())["api"]
            configs = miner_configs(tmp)
            prev_final, rounds = "", []
            for w in range(ROUNDS):
                if w:
                    open_round(svc, n, w, prev_final)
                for i, conf in enumerate(configs):
                    if (w, i) != (0, 0):  # the chain already ran miner 0 for round 0
                        run_v2(stages, conf, w, i)
                prev_final = settle(svc, n, w)
                if w == 0 and os.environ.get("MINE_MULTI_ROUND_V2_BREAK") == "mature":
                    # An epoch answer is final: asked before round 0 matures, it pays nothing.
                    ev["get_weights_immature"] = get_weights(svc, n, api, tmp)
                rounds.append({"w": w, "uploaded": len(configs), "finalize_hash": prev_final})
            ev["rounds"] = rounds
            rid = n.manifest.run_id()
            mature_at = n.store._db.execute(
                "SELECT MAX(mature_at) FROM escrow_origins "
                "WHERE reward_round IS NOT NULL AND issued>0"
            ).fetchone()[0]
            ev["mature_at"], ev["now_before_maturity"] = mature_at, n.now
            n.push(max(n.now, mature_at) + 1)
            weights = get_weights(svc, n, api, tmp)
            ev["get_weights"] = weights
            hot0 = svc.HOT[0].ss58
            paid = weights["answer"].get("weights", {})
            ev["miner_hotkey"], ev["miner_weight"] = hot0, paid.get(hot0, 0)
            if weights["status_code"] != 200 or not paid.get(hot0, 0) > 0:
                raise RuntimeError(f"get_weights: no positive weight for {hot0}: {weights}")
            if not set(paid) <= {k.ss58 for k in svc.HOT}:
                raise RuntimeError(f"get_weights pays a non-roster hotkey: {sorted(paid)}")
            ev["run_id"] = rid
            ev["restart"] = {}
            restart_probes(stages, svc, n, configs, prev_final, tmp, ev["restart"])
    except Exception as e:  # noqa: BLE001 - the evidence must record every failure
        ev["failed"] = f"{type(e).__name__}: {e}"
    required = [s for s in ev["stages"] if not s.get("probe")]
    failure = ev.get("failed") or smoke.verdict(required)
    ev["elapsed_seconds"] = round(time.monotonic() - started, 2)
    ev["result"] = "PASS" if failure is None else "FAIL"
    os.ftruncate(fd, 0)
    with os.fdopen(fd, "w") as f:
        f.write(json.dumps(ev, indent=1, sort_keys=True, default=str))
    summary = {
        "result": ev["result"],
        "failed": failure,
        "elapsed_seconds": ev["elapsed_seconds"],
        "rounds": ev.get("rounds"),
        "miner_hotkey": ev.get("miner_hotkey"),
        "miner_weight": ev.get("miner_weight"),
        "weights": ev.get("get_weights", {}).get("answer", {}).get("weights"),
        "restart": ev.get("restart"),
        "evidence": str(out),
    }
    print(json.dumps(summary, sort_keys=True))
    return 0 if failure is None else 1


if __name__ == "__main__":
    sys.exit(main())
