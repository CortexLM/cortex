"""QA: uv run --frozen python tests/aggregator/qa_scenarios.py (exit 0 iff both PASS)."""

from __future__ import annotations

import json
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

import hypertrain.trainer  # noqa: E402,F401  (determinism before torch)

# isort: split
from agg_helpers import commit, hotkey, raw_delta, scenario, setup  # noqa: E402

from hypertrain.aggregator.core import HashMismatch  # noqa: E402


def happy(root: Path) -> dict[str, object]:
    sc = scenario(root / "run")
    agg, bad = sc["agg"], sc["bad"]
    rb = agg.rollback(sc["t0"], sc["t1"], [bad], ["ab" * 32], 10**6)
    ref, s0 = setup(root / "ref")
    r0 = ref.apply_round(0, s0, [commit(ref.store, 0, i, raw_delta(i)) for i in range(3)])
    c1 = [commit(ref.store, 1, i, raw_delta(10 + i)) for i in range(3)]
    r1 = ref.apply_round(1, r0["body"]["out_state"], c1)
    got, want = rb.tapes[1]["body"]["theta_hash"], r1["body"]["theta_hash"]
    return {
        "scenario": "HAPPY 3 honest + 1 faulty, fault at d_final -> rollback",
        "faulty": bad,
        "tainted_theta_w2": sc["t1"]["body"]["theta_hash"],
        "rollback_theta_w2": got,
        "reference_theta_w2": want,
        "pass": got == want != sc["t1"]["body"]["theta_hash"],
    }


def failure(root: Path) -> dict[str, object]:
    agg, s0 = setup(root)
    cs = [commit(agg.store, 0, i, raw_delta(i)) for i in range(4)]
    p = root / cs[2].delta_hash[:2] / cs[2].delta_hash
    b = bytearray(p.read_bytes())
    b[len(b) // 2] ^= 0x01
    p.write_bytes(bytes(b))
    err = None
    try:
        agg.apply_round(0, s0, cs)
    except HashMismatch as e:
        err = f"HashMismatch: {e}"
    return {
        "scenario": "FAILURE stored delta object corrupted",
        "corrupted_hotkey": hotkey(2),
        "error": err,
        "round_applied": 0 in agg.applied,
        "pass": err is not None and 0 not in agg.applied,
    }


def main() -> int:
    with tempfile.TemporaryDirectory(prefix="ht-agg-qa-") as d:
        res = [happy(Path(d) / "h"), failure(Path(d) / "f")]
    print(json.dumps(res, indent=1))
    return 0 if all(r["pass"] for r in res) else 1


if __name__ == "__main__":
    raise SystemExit(main())
