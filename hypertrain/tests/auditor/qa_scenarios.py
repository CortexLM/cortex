"""QA: HAPPY (lease -> MATCH -> job closed) and FAILURE (StateServe withheld past
serve_deadline -> WITHHELD verdict + Forfeit) through the auditor worker over HTTP.
Run: uv run --frozen --all-extras python tests/auditor/qa_scenarios.py"""

from __future__ import annotations

import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import conftest  # noqa: E402,F401
from aud_fixtures import (  # noqa: E402
    AUDITOR,
    ENV,
    MINER,
    SERVE_DEADLINE,
    TOKEN,
    FakeChallenge,
    carry_round,
    get_sample,
    job_json,
    make_challenge,
    world,
)
from fastapi.testclient import TestClient  # noqa: E402

from hypertrain.auditor.replay import committed_norms, select_segments  # noqa: E402
from hypertrain.auditor.worker import Auditor, HttpApi  # noqa: E402
from hypertrain.protocol.envelope import verify_envelope  # noqa: E402
from hypertrain.trainer.loop import train_round  # noqa: E402


def happy() -> dict[str, object]:
    w = world("reset")
    res = train_round(w.cfg, w.theta, w.a, get_sample(w))
    fc = FakeChallenge(w.m)
    fc.add_job("happy", job_json(w, fc, res, make_challenge()))
    aud = Auditor(HttpApi(TestClient(fc.app()), TOKEN), AUDITOR, ENV, get_sample(w))
    result = aud.run_once()
    env = fc.verdicts["happy"]
    ok = (
        result == "MATCH"
        and fc.state["happy"] == "closed"
        and verify_envelope(env)
        and env["body"]["recomputed_leaves_root"] == res.leaves_root
        and not fc.forfeits
    )
    return {"pass": ok, "result": result, "job_state": fc.state["happy"], "verdict": env["body"]}


def failure() -> dict[str, object]:
    w = world("carry", H=30, J=1)
    cr = carry_round(w)
    norms = committed_norms([x.preimage for x in cr.result.leaves])
    ch0 = make_challenge()
    segs = select_segments(
        w.run_id,
        3,
        ch0.beacon_sig_sha256,
        MINER.ss58,
        norms,
        w.m.verify.k_segments,
        w.m.verify.Q_top,
    )
    fc = FakeChallenge(w.m, now_round=SERVE_DEADLINE + 1)
    fc.add_job("withheld", job_json(w, fc, cr.result, make_challenge("segments", segs)))
    aud = Auditor(HttpApi(TestClient(fc.app()), TOKEN), AUDITOR, ENV, get_sample(w))
    result = aud.run_once()
    ok = (
        result == "WITHHELD"
        and fc.state["withheld"] == "closed"
        and len(fc.forfeits) == 1
        and fc.forfeits[0]["cause"] == "WITHHELD"
        and fc.forfeits[0]["escrow_burned_units"] > 0
    )
    return {
        "pass": ok,
        "result": result,
        "segments": segs,
        "job_state": fc.state["withheld"],
        "forfeit": fc.forfeits,
    }


if __name__ == "__main__":
    out = {"HAPPY": happy(), "FAILURE": failure()}
    print(json.dumps(out, indent=1))
    sys.exit(0 if all(v["pass"] for v in out.values()) else 1)
