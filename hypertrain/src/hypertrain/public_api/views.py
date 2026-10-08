"""Pure read-only views of the challenge DB in the shape the subnet frontend validates (B3)."""

from __future__ import annotations

import json
import sqlite3
from collections.abc import Mapping, Sequence
from datetime import UTC, datetime
from typing import Any

from hypertrain.protocol.messages import f32val

# Mirror of hypertrain.challenge.store.FAULTS (kept local: this package never imports the store).
FAULTS = frozenset({"MISMATCH", "WITHHELD", "BAD_PROOF", "ASSIGNMENT_VIOLATION"})
MAX_PARTICIPANTS = 500
_RANK = {"training": 0, "syncing": 1, "finished": 2, "offline": 3}


def prune_none(x: Any) -> Any:
    """Omit-not-null: drop every key whose value is None, recursively."""
    if isinstance(x, dict):
        return {k: prune_none(v) for k, v in x.items() if v is not None}
    if isinstance(x, list):
        return [prune_none(v) for v in x]
    return x


def od_param_count(d: int, n_layers: int, n_heads: int, head_layers: int, vocab: int) -> int:
    """Exact sum over the pinned OpenDecision state-dict names (A3); no torch."""
    block = 12 * d * d + 4 * (d // n_heads) + 9 * d  # qkv,o,ff weights; qn/kn; n1,n2; ff biases
    head = 16 * d * d + 11 * d  # opt_attn + cross + ff + n1..n3
    extras = (
        3 * d + d + 2 * d + (d * d + d) + (d + 1) + 3
    )  # type_emb, unknown, scorer.0 (norm), .1, .3, log_temp
    return vocab * d + n_layers * block + 2 * d + head_layers * head + extras


def _iso(t: int) -> str:
    return datetime.fromtimestamp(t, UTC).strftime("%Y-%m-%dT%H:%M:%SZ")


class _Run:
    """One parsed runs row plus the helpers every view needs."""

    def __init__(self, row: sqlite3.Row) -> None:
        self.id: str = row["run_id"]
        self.status: str = row["status"]
        self.m: dict[str, Any] = json.loads(row["manifest"])
        self.cfg: dict[str, Any] = json.loads(row["config"])
        b = self.m["beacon"]
        self.genesis: int = b["genesis_time"]
        self.period: int = b["period"]

    def chain(self, rnd: int) -> int:
        return self.genesis + (rnd - 1) * self.period

    @property
    def is_od(self) -> bool:
        return bool(self.m["model"].get("arch") == "od-encoder")


def _runs(db: sqlite3.Connection) -> list[_Run]:
    out = []
    for row in db.execute("SELECT * FROM runs WHERE config IS NOT NULL ORDER BY run_id"):
        run = _Run(row)
        if run.m["outer"]["opt"] != "nesterov":
            continue
        if db.execute("SELECT 1 FROM rounds WHERE run_id=?", (run.id,)).fetchone() is None:
            continue
        out.append(run)
    return out


def _rounds(db: sqlite3.Connection, run: _Run) -> list[dict[str, Any]]:
    out = []
    for r in db.execute(
        "SELECT w, body, final_at FROM rounds WHERE run_id=? ORDER BY w", (run.id,)
    ):
        out.append({"w": r["w"], "body": json.loads(r["body"]), "final_at": r["final_at"]})
    return out


def _miners(db: sqlite3.Connection, run: _Run) -> list[dict[str, Any]]:
    out = []
    for r in db.execute(
        "SELECT w, hotkey, status, accept, commit_env FROM miners WHERE run_id=?", (run.id,)
    ):
        commit = json.loads(r["commit_env"])["body"] if r["commit_env"] else None
        accept = json.loads(r["accept"])["body"] if r["accept"] else None
        out.append(
            {
                "w": r["w"],
                "hotkey": r["hotkey"],
                "status": r["status"],
                "commit": commit,
                "accept": accept,
            }
        )
    return out


def _counted(mn: Mapping[str, Any], finals: Mapping[int, int | None]) -> bool:
    return (
        mn["commit"] is not None and mn["status"] not in FAULTS and finals.get(mn["w"]) is not None
    )


def _architecture(run: _Run) -> dict[str, Any] | None:
    m, mod = run.m, run.m["model"]
    common = {
        "layers": mod["n_layers"],
        "dModel": mod["d_model"],
        "heads": mod["n_heads"],
        "mlpHidden": mod["d_ff"],
        "vocabSize": mod["vocab"],
    }
    if run.is_od:
        od = mod["od"]
        full = od_param_count(
            mod["d_model"], mod["n_layers"], mod["n_heads"], od["head_layers"], mod["vocab"]
        )
        rec = od.get("record")
        ctx = mod["seq_len"] if od["objective"] == "mlm" or not rec else rec["state_len"]
        return {
            "family": "encoder + decision head",
            "parameters": full,
            **common,
            "contextLength": ctx,
            "norm": "LayerNorm",
            "activation": "GELU",
            "positional": "RoPE",
            "pretrainObjective": "mlm",
            "calibration": "temperature",
            "decisionHead": {
                "layers": od["head_layers"],
                "questionTypes": ["choice", "score", "noul"],
                "unknownSlot": True,
                "trainedInThisRun": od["objective"] != "mlm",
            },
        }
    if mod.get("arch") == "decoder" and m["inner"]["opt"] == "adamw":
        return {
            "family": "decoder-only transformer",
            "parameters": mod["param_count"],
            **common,
            "contextLength": mod["seq_len"],
            "innerOptimizer": {"type": "adamw"},
            "norm": "RMSNorm",
            "activation": "SwiGLU",
            "positional": "RoPE",
            "tiedEmbeddings": False,
        }
    return None


def _run_view(
    db: sqlite3.Connection, run: _Run, rounds: list[dict[str, Any]], mn: list[dict[str, Any]]
) -> dict[str, Any]:
    m, mod, inner = run.m, run.m["model"], run.m["inner"]
    finals = {r["w"]: r["final_at"] for r in rounds}
    n_final = sum(1 for r in rounds if r["final_at"] is not None)
    total = run.cfg.get("total_rounds")
    current = max(r["w"] for r in rounds)
    tokens = sum(x["commit"]["tokens"] for x in mn if _counted(x, finals))
    pc = mod["param_count"] / 1e6
    if run.is_od:
        model = f"OpenDecision {mod['od']['preset']} ({pc:.0f}M)"
    else:
        model = f"decoder {pc:.0f}M"
    src = m["dataset"].get("source")
    data = f"{src['mix_id']}" if src else f"data {m['dataset']['merkle_root'][:8]}"
    status = {"created": "scheduled", "paused": "stopped"}.get(run.status, "running")
    if status == "running" and total is not None and n_final >= total:
        status = "completed"
    ended = None
    if status in ("completed", "stopped") and n_final:
        ended = _iso(max(r["final_at"] for r in rounds if r["final_at"] is not None))
    target = tokens
    if total is not None:
        roster = rounds[-1]["body"]["roster"]
        target = (
            total
            * len(roster)
            * inner["H"]
            * inner["micro_batch"]
            * inner["grad_accum"]
            * mod["seq_len"]
        )
    outer = m["outer"]
    return {
        "id": run.id,
        "name": f"{model} / {data}",
        "model": model,
        "startedAt": _iso(run.chain(rounds[0]["body"]["d_open"])),
        "endedAt": ended,
        "status": status,
        "innerSteps": inner["H"],
        "totalRounds": total if total is not None else current + 1,
        "currentRound": current,
        "tokensTrained": tokens,
        "targetTokens": target,
        "outerOptimizer": {
            "type": "nesterov",
            "lr": f32val(outer["lr"]),
            "momentum": f32val(outer["momentum"]),
        },
        "pseudoGradDtype": "int8",
    }


def list_runs(db: sqlite3.Connection) -> list[dict[str, Any]]:
    views = [_run_view(db, r, _rounds(db, r), _miners(db, r)) for r in _runs(db)]
    views.sort(key=lambda v: v["id"])
    views.sort(key=lambda v: v["startedAt"], reverse=True)
    return prune_none(views)


def run_snapshot(
    db: sqlite3.Connection, run_id: str, metrics: Sequence[dict[str, Any]]
) -> dict[str, Any] | None:
    run = next((r for r in _runs(db) if r.id == run_id), None)
    if run is None:
        return None
    rounds, mn = _rounds(db, run), _miners(db, run)
    return prune_none(_snapshot(db, run, rounds, mn, metrics))


def default_snapshot(
    db: sqlite3.Connection, metrics: Sequence[dict[str, Any]]
) -> dict[str, Any] | None:
    runs = list_runs(db)
    if not runs:
        return None
    pick = next((r for r in runs if r["status"] == "running"), runs[0])
    return run_snapshot(db, pick["id"], [x for x in metrics if x.get("run_id") == pick["id"]])


def _snapshot(
    db: sqlite3.Connection,
    run: _Run,
    rounds: list[dict[str, Any]],
    mn: list[dict[str, Any]],
    metrics: Sequence[dict[str, Any]],
) -> dict[str, Any]:
    m, inner = run.m, run.m["inner"]
    H = inner["H"]
    view = _run_view(db, run, rounds, mn)
    finals = {r["w"]: r["final_at"] for r in rounds}
    by_w = {r["w"]: r for r in rounds}
    current = rounds[-1]
    loss = {x["w"]: x["value"] for x in metrics if x.get("run_id", run.id) == run.id}
    ddp_p = m["model"]["param_count"]

    def compute(w: int) -> int:
        b = by_w[w]["body"]
        return int(run.chain(b["d_commit"]) - run.chain(b["d_assign"]))

    out_rounds = []
    for r in rounds:
        b, w = r["body"], r["w"]
        n = len(b["roster"])
        sync = max(r["final_at"] - run.chain(b["d_commit"]), 0) if r["final_at"] is not None else 0
        out_rounds.append(
            {
                "index": w,
                "startedAt": _iso(run.chain(b["d_open"])),
                "syncedAt": _iso(r["final_at"]) if r["final_at"] is not None else None,
                "participants": n,
                "computeSeconds": compute(w),
                "syncSeconds": sync,
                "bytesExchanged": sum(
                    x["commit"]["delta_bytes"] for x in mn if x["w"] == w and x["commit"]
                ),
                "bytesIfDDP": 4 * ddp_p * H * n,
                "globalLoss": loss.get(w),
            }
        )

    sm = m["reference_spec"]["sm_count"]
    gpu_type = "RTX 5090" if sm == 170 else f"sm{sm}"
    roster = [
        dict(r)
        for r in db.execute(
            "SELECT hotkey, cluster, region FROM roster "
            "WHERE run_id=? AND removed=0 ORDER BY hotkey",
            (run.id,),
        )
    ]

    def last_good(hk: str) -> dict[str, Any] | None:
        rows = [x for x in mn if x["hotkey"] == hk and _counted(x, finals)]
        return max(rows, key=lambda x: x["w"]) if rows else None

    def gpus_of(hk: str) -> int:
        rows = [x for x in mn if x["hotkey"] == hk and x["accept"]]
        return int(max(rows, key=lambda x: x["w"])["accept"]["n_gpus"]) if rows else 1

    def rate(hks: list[str]) -> float:
        last = [lg for lg in (last_good(h) for h in hks) if lg]
        if not last:
            return 0.0
        w = max(x["w"] for x in last)
        secs = compute(w)
        tok = sum(x["commit"]["tokens"] for x in last if x["w"] == w)
        return tok / secs if secs > 0 else 0.0

    def tok_of(hk: str) -> int:
        return sum(x["commit"]["tokens"] for x in mn if x["hotkey"] == hk and _counted(x, finals))

    total_tok = sum(x["commit"]["tokens"] for x in mn if _counted(x, finals))
    completed = view["status"] == "completed"
    latest_beacon = db.execute("SELECT MAX(round) AS r FROM beacon").fetchone()["r"] or 0
    cb = current["body"]

    def pstatus(hk: str) -> str:
        if completed:
            return "finished"
        row = next((x for x in mn if x["hotkey"] == hk and x["w"] == current["w"]), None)
        if row is not None and row["commit"] is not None and row["status"] not in FAULTS:
            return "training" if current["final_at"] is not None else "syncing"
        if row is not None and row["status"] not in FAULTS and latest_beacon <= cb["d_commit"]:
            return "training"
        return "offline"

    groups: dict[str, list[str]] = {}
    region: dict[str, str] = {}
    for r in roster:
        cid = r["cluster"] or "unassigned"
        groups.setdefault(cid, []).append(r["hotkey"])
        region.setdefault(cid, r["region"] or "unknown")
    started = view["startedAt"]
    clusters = []
    for cid, hks in sorted(groups.items()):
        tok = sum(tok_of(h) for h in hks)
        last = [lg for lg in (last_good(h) for h in hks) if lg]
        lw = max((x["w"] for x in last), default=0)
        sync_at = _iso(finals[lw]) if last and finals[lw] is not None else started
        done = sum(
            H
            for x in mn
            if x["hotkey"] in hks
            and x["w"] == current["w"]
            and x["commit"]
            and x["status"] not in FAULTS
        )
        clusters.append(
            {
                "id": cid,
                "name": cid,
                "region": region[cid],
                "gpuType": gpu_type,
                "nodes": len(hks),
                "gpus": sum(gpus_of(h) for h in hks),
                "status": min((pstatus(h) for h in hks), key=_RANK.__getitem__),
                "innerStepsDone": done,
                "tokensPerSec": rate(hks),
                "lastSyncRound": lw,
                "lastSyncAt": sync_at,
                "tokensContributed": tok,
                "share": tok / total_tok if total_tok else 0.0,
            }
        )

    cluster_of = {h: cid for cid, hks in groups.items() for h in hks}
    seen = {x["hotkey"] for x in mn}
    parts = []
    for hk in sorted(seen & set(cluster_of)):
        tok = tok_of(hk)
        parts.append(
            {
                "id": hk,
                "hotkey": hk,
                "label": hk[:6] + "..." + hk[-4:],
                "clusterId": cluster_of[hk],
                "gpus": gpus_of(hk),
                "gpuType": gpu_type,
                "status": pstatus(hk),
                "joinedRound": min(x["w"] for x in mn if x["hotkey"] == hk),
                "roundsContributed": sum(
                    1 for x in mn if x["hotkey"] == hk and _counted(x, finals)
                ),
                "tokensContributed": tok,
                "tokensPerSec": rate([hk]),
                "share": tok / total_tok if total_tok else 0.0,
            }
        )
    parts.sort(key=lambda p: (-p["tokensContributed"], p["hotkey"]))

    points = [
        {"t": _iso(finals[w]), "round": w, "step": (w + 1) * H, "value": v}
        for w, v in sorted(loss.items())
        if finals.get(w) is not None
    ]
    return {
        "run": view,
        "architecture": _architecture(run),
        "rounds": out_rounds,
        "clusters": clusters,
        "participants": parts[:MAX_PARTICIPANTS],
        "metrics": {"loss": points},
    }


def model_description(manifest: Mapping[str, Any]) -> dict[str, Any]:
    """Full architecture description from a signed manifest dict (B3 `/model`)."""
    run = _Run.__new__(_Run)
    run.m = dict(manifest)
    mod = run.m["model"]
    if not run.is_od:
        return _architecture(run) or {}
    od, tok, ds, outer, inner = (
        mod["od"],
        run.m["tokenizer"],
        run.m["dataset"],
        run.m["outer"],
        run.m["inner"],
    )
    d, h = mod["d_model"], mod["n_heads"]
    full = od_param_count(d, mod["n_layers"], h, od["head_layers"], mod["vocab"])
    src = ds.get("source")
    rec = od.get("record")
    return prune_none(
        {
            "family": "encoder + decision head",
            "arch": "od-encoder",
            "preset": od["preset"],
            "objective": od["objective"],
            "parameters": mod["param_count"],
            "parametersFull": full,
            "encoder": {
                "layers": mod["n_layers"],
                "dModel": d,
                "heads": h,
                "headDim": d // h,
                "mlpHidden": mod["d_ff"],
                "activation": "GELU",
                "norm": "LayerNorm (pre-LN)",
                "qkNorm": True,
                "positional": "RoPE",
                "ropeTheta": mod["rope_theta"],
                "attention": "bidirectional SDPA",
                "bias": {"qkv": False, "o": False, "mlp": True},
                "contextLength": mod["seq_len"]
                if od["objective"] == "mlm" or not rec
                else rec["state_len"],
            },
            "embedding": {
                "vocabSize": mod["vocab"],
                "tiedMlmHead": True,
                "tokenizer": {
                    "name": tok["name"],
                    "sha256": tok["sha256"],
                    "offset": od["tokenizer_offset"],
                    "reserved": {"pad": 0, "mask": 1, "cls": 2},
                },
            },
            "decisionHead": {
                "layers": od["head_layers"],
                "presentInRun": od["objective"] != "mlm",
                "optionSelfAttention": True,
                "crossAttentionToState": True,
                "unknownSlot": True,
                "questionTypes": ["choice", "score", "noul"],
                "temperaturePerType": True,
            },
            "training": {
                "maskRatio": f32val(od["mask_ratio"]),
                "innerOptimizer": {"type": inner["opt"], "statePolicy": inner["state_policy"]},
                "outerOptimizer": {
                    "type": outer["opt"],
                    "lr": f32val(outer["lr"]),
                    "momentum": f32val(outer["momentum"]),
                },
                "profile": run.m["reference_spec"].get("profile"),
                "computeDtype": mod["compute_dtype"],
                "masterDtype": mod["master_dtype"],
            },
            "dataset": {
                "mixId": src["mix_id"] if src else None,
                "merkleRoot": ds["merkle_root"],
                "sampleFormat": ds["sample_format"],
            },
            "manifest": {"initStateHash": run.m["init_state_hash"]},
        }
    )
