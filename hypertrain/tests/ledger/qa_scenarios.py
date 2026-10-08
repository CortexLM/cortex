"""Task-3 QA: HAPPY (3 honest, E=3, q=0.5, 8 rounds) and FAILURE (fault at round 5, torn tail)."""

import json
import sys
import tempfile
from pathlib import Path

from hypertrain.ledger import ChainBreak, Ledger, Params, vest_rounds_for_q

EPOCH = 360
MINERS = ("hk-a", "hk-b", "hk-c")


def run(directory: Path, fault_round: int | None) -> tuple[Ledger, list[dict]]:
    vest = 3
    assert vest >= vest_rounds_for_q("0.5")
    led = Ledger(directory, Params("hypertrain", 0, EPOCH, 1, vest))
    answers = []
    for w in range(8):
        t = w * EPOCH
        for hk in MINERS:
            led.commit(w, hk, t + 1)
        for hk in MINERS:
            verdict = "FAULT" if (w == fault_round and hk == "hk-c") else "MATCH"
            led.verdict(w, hk, verdict, 4096, 1_000_000, t + 2)
        led.finalize(w, t + 3)
        answers.append(json.loads(led.get_weights(w, t + EPOCH - 1, t + EPOCH)))
    return led, answers


def main() -> int:
    out: dict = {}
    with tempfile.TemporaryDirectory(prefix="ht-ledger-qa-") as tmp:
        root = Path(tmp)
        led, happy = run(root / "happy", None)
        ents = {(e.w, e.hotkey): e.amount for e in led.state().entitlements}
        vest = led.params.vest_rounds
        expect = [
            {hk: float(ents[(e - vest, hk)]) for hk in MINERS} if e >= vest else {}
            for e in range(8)
        ]
        out["happy"] = {
            "E": vest,
            "weights": [a["weights"] for a in happy],
            "burned": [a["metadata"]["units_burned_this_epoch"] for a in happy],
            "pass": [a["weights"] for a in happy] == expect
            and all(a["metadata"]["units_burned_this_epoch"] == 10**6 for a in happy[:vest]),
        }
        fled, faulty = run(root / "fault", 5)
        state = fled.state()
        c_burned = sum(e.burned for e in state.entitlements if e.hotkey == "hk-c")
        others_same = all(
            f["weights"].get(hk) == h["weights"].get(hk)
            for f, h in zip(faulty, happy, strict=True)
            for hk in ("hk-a", "hk-b")
        )
        c_after = [a["weights"].get("hk-c", 0.0) for a in faulty[5:]]
        journal = root / "fault" / "journal.jsonl"
        good = journal.read_bytes()
        torn = b'{"kind":"commit","seq":99,"w":'
        journal.write_bytes(good + torn)
        reopened = Ledger(root / "fault", fled.params)
        sidecars = sorted(p.name for p in (root / "fault").glob("journal.torn-*.bin"))
        quarantined = (
            reopened.journal.records[-1]["kind"] == "journal_repaired"
            and len(sidecars) == 1
            and (root / "fault" / sidecars[0]).read_bytes() == torn
            and reopened.get_weights(7, 0, 0) == fled.get_weights(7, 0, 0)
            and json.loads(reopened.get_weights(7, 0, 0)) == faulty[7]
        )
        flipped = bytearray(journal.read_bytes())
        flipped[len(flipped) // 2] ^= 0x01
        journal.write_bytes(bytes(flipped))
        try:
            Ledger(root / "fault", fled.params)
            flip_detected = False
        except ChainBreak as exc:
            flip_detected = True
            out["flip_error"] = str(exc)
        out["failure"] = {
            "escrow_rounds_burned": sorted(
                e.w for e in state.entitlements if e.hotkey == "hk-c" and e.burned
            ),
            "hk_c_units_burned": c_burned,
            "hk_c_weights_from_epoch_5": c_after,
            "others_unchanged": others_same,
            "blacklisted": "hk-c" in state.blacklist,
            "torn_tail_quarantined": quarantined,
            "sidecars": sidecars,
            "flipped_byte_chainbreak": flip_detected,
            "pass": others_same
            and all(v == 0.0 for v in c_after)
            and c_burned > 0
            and quarantined
            and flip_detected,
        }
        out["ledger_invariant"] = state.minted == state.burned + state.paid + state.pending
    out["pass"] = out["happy"]["pass"] and out["failure"]["pass"] and out["ledger_invariant"]
    print(json.dumps(out, indent=1, sort_keys=True))
    return 0 if out["pass"] else 1


if __name__ == "__main__":
    sys.exit(main())
