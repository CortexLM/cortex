import hashlib
import json
import random
import shutil
import statistics
import time
from pathlib import Path

import pytest

from hypertrain.ledger import (
    FULL_SHARE_MASS,
    MAX_ENTRIES,
    ChainBreak,
    Ledger,
    LedgerError,
    Params,
    RebuildMismatch,
    vest_rounds_for_q,
)
from hypertrain.ledger.journal import GENESIS_HASH, canonical, parse_chain

EPOCH = 100


def params(vest: int = 3, per_round: int = 1) -> Params:
    return Params("hypertrain", 0, EPOCH, per_round, vest)


def run_round(led: Ledger, w: int, outcomes: dict[str, tuple[str, int, int]]) -> None:
    base = w * EPOCH * led.params.epochs_per_round
    for hk in sorted(outcomes):
        led.commit(w, hk, base + 10)
    for hk, (verdict, tokens, score) in sorted(outcomes.items()):
        led.verdict(w, hk, verdict, tokens, score, base + 20)
    led.finalize(w, base + 30)


def answer(led: Ledger, epoch: int) -> dict:
    return json.loads(led.get_weights(epoch, epoch * EPOCH + 99, 1_000_000 + epoch))


HONEST = {h: ("MATCH", 1000, 10**6) for h in ("a", "b", "c")}


def test_happy_three_honest_miners_paid_after_vesting(tmp_path: Path) -> None:
    led = Ledger(tmp_path, params(vest=3))
    per_epoch = []
    for w in range(8):
        run_round(led, w, HONEST)
        per_epoch.append(answer(led, w))
    for e in range(3):
        assert per_epoch[e]["weights"] == {}
        assert per_epoch[e]["full_share_mass"] == FULL_SHARE_MASS
        assert per_epoch[e]["metadata"]["units_burned_this_epoch"] == FULL_SHARE_MASS
    for e in range(3, 8):
        assert per_epoch[e]["weights"] == {"a": 333333.0, "b": 333333.0, "c": 333333.0}
    state = led.state()
    assert state.minted == 8 * 10**6
    assert state.burned == 8
    assert state.paid == 5 * 999999
    assert state.pending == 3 * 999999
    assert not any(r["kind"] == "verdict" and r["verdict"] == "FAULT" for r in led.journal.records)


def test_entitlement_formula_and_soft_score_clamp(tmp_path: Path) -> None:
    led = Ledger(tmp_path, params(vest=1))
    run_round(
        led,
        0,
        {
            "hi": ("MATCH", 1000, 5 * 10**6),
            "lo": ("UNSAMPLED", 1000, -7),
            "bad": ("FAULT", 3000, 10**6),
        },
    )
    ents = {e.hotkey: e.amount for e in led.state().entitlements}
    total = 1000 * 10**6 + 1000 * 800_000 + 3000 * 10**6
    assert ents == {"hi": 10**6 * 1000 * 10**6 // total, "lo": 10**6 * 1000 * 800_000 // total}
    assert led.state().burned == 10**6 - sum(ents.values())


def test_fault_burns_unvested_escrow_others_unchanged(tmp_path: Path) -> None:
    base = Ledger(tmp_path / "base", params(vest=3))
    led = Ledger(tmp_path / "fault", params(vest=3))
    seen_base, seen = [], []
    for w in range(8):
        run_round(base, w, HONEST)
        outcomes = dict(HONEST)
        if w == 5:
            outcomes["c"] = ("FAULT", 1000, 10**6)
            before = led.state()
            unvested = [e for e in before.entitlements if e.hotkey == "c" and e.outstanding]
            assert [e.w for e in unvested] == [2, 3, 4]
            assert [e.w for e in unvested if e.final_round + 3 > 5] == [3, 4]
            owed = sum(e.outstanding for e in unvested)
        run_round(led, w, outcomes)
        if w == 5:
            after = led.state()
            assert after.burned - before.burned == owed + 333333 + 1
            assert "c" in after.blacklist
        seen_base.append(answer(base, w))
        seen.append(answer(led, w))
    for e in range(8):
        assert seen[e]["weights"].get("c", 0.0) == (333333.0 if e in (3, 4) else 0.0)
        for hk in ("a", "b"):
            assert seen[e]["weights"].get(hk) == seen_base[e]["weights"].get(hk)
    state = led.state()
    assert state.minted == state.burned + state.paid + state.pending


def test_transient_forgiven_once_per_epoch_then_fault(tmp_path: Path) -> None:
    led = Ledger(tmp_path, params(vest=1, per_round=1))
    run_round(led, 0, HONEST)
    run_round(led, 1, {**HONEST, "c": ("TRANSIENT", 1000, 10**6)})
    state = led.state()
    assert "c" not in state.blacklist
    assert [e.w for e in state.entitlements if e.hotkey == "c"] == [0]
    led2 = Ledger(tmp_path / "two", Params("hypertrain", 0, EPOCH, 2, 1))
    for hk in "abc":
        led2.commit(0, hk, 10)
    led2.verdict(0, "a", "MATCH", 1, 0, 20)
    led2.verdict(0, "b", "TRANSIENT", 1, 0, 20)
    led2.verdict(0, "c", "MATCH", 1, 0, 20)
    led2.finalize(0, 30)
    led2.commit(1, "b", 40)
    led2.verdict(1, "b", "TRANSIENT", 1, 0, 50)
    assert "b" in led2.state().blacklist


def test_no_upload_forfeits_round_only(tmp_path: Path) -> None:
    led = Ledger(tmp_path, params(vest=1))
    run_round(led, 0, HONEST)
    run_round(led, 1, {**HONEST, "c": ("NO_UPLOAD", 1000, 10**6)})
    state = led.state()
    assert "c" not in state.blacklist
    assert [(e.w, e.outstanding) for e in state.entitlements if e.hotkey == "c"] == [(0, 333333)]


def test_clawback_takes_escrow_then_future_entitlements(tmp_path: Path) -> None:
    led = Ledger(tmp_path, params(vest=1))
    run_round(led, 0, HONEST)
    led.clawback("a", 500_000, 40)
    state = led.state()
    assert [e.outstanding for e in state.entitlements if e.hotkey == "a"] == [0]
    assert state.debt["a"] == 500_000 - 333333
    run_round(led, 1, HONEST)
    state = led.state()
    assert state.debt["a"] == 0
    assert [e.amount for e in state.entitlements if e.hotkey == "a" and e.w == 1] == [
        333333 - (500_000 - 333333)
    ]
    assert state.minted == state.burned + state.paid + state.pending


def test_zero_finalized_audit_answers_empty_not_error(tmp_path: Path) -> None:
    led = Ledger(tmp_path, params())
    led.commit(0, "a", 10)
    body = answer(led, 0)
    assert body["weights"] == {}
    assert body["full_share_mass"] == FULL_SHARE_MASS
    assert body["metadata"]["units_burned_this_epoch"] == FULL_SHARE_MASS


def test_first_answer_final_even_after_new_entries(tmp_path: Path) -> None:
    led = Ledger(tmp_path, params(vest=1))
    run_round(led, 0, HONEST)
    first = led.get_weights(2, 299, 7)
    assert json.loads(first)["weights"] == {"a": 333333.0, "b": 333333.0, "c": 333333.0}
    led.commit(1, "z", 300)
    led.verdict(1, "z", "MATCH", 10**9, 10**6, 300)
    led.finalize(1, 300)
    assert led.get_weights(2, 399, 8) == first
    assert Ledger(tmp_path, params(vest=1)).get_weights(2, 999, 9) == first


def test_epoch_at_round_mapping() -> None:
    p = Params("hypertrain", 1_000, 360, 4, 3)
    cases = {999: (-1, -1), 1_000: (0, 0), 1_359: (0, 0), 1_360: (1, 0), 2_439: (3, 0)}
    cases |= {2_440: (4, 1), 1_000 + 360 * 4 * 7 + 5: (28, 7)}
    for t, (epoch, rnd) in cases.items():
        assert (p.epoch_of(t), p.round_of(t)) == (epoch, rnd)
    assert [vest_rounds_for_q(q) for q in ("0.5", "0.1", "0.05", "0.3", "1")] == [2, 10, 20, 4, 1]


def test_release_waits_for_epoch_at_round(tmp_path: Path) -> None:
    led = Ledger(tmp_path, Params("hypertrain", 0, EPOCH, 2, 1))
    for hk in "ab":
        led.commit(0, hk, 10)
    for hk in "ab":
        led.verdict(0, hk, "MATCH", 1, 10**6, 20)
    led.finalize(0, 30)
    assert json.loads(led.get_weights(0, 199, 0))["weights"] == {}
    paid = json.loads(led.get_weights(1, 200, 0))
    assert paid["metadata"]["round_at"] == 1
    assert paid["weights"] == {"a": float(FULL_SHARE_MASS)}
    rest = json.loads(led.get_weights(2, 500, 0))
    assert rest["weights"] == {"b": float(FULL_SHARE_MASS)}


def test_cap_65536_largest_ties_by_hotkey_rest_carried(tmp_path: Path) -> None:
    n = 70_000
    led = Ledger(tmp_path, params(vest=1))
    hotkeys = [f"hk{i:05d}" for i in range(n)]
    for hk in hotkeys:
        led.commit(0, hk, 10)
    for i, hk in enumerate(hotkeys):
        led.verdict(0, hk, "MATCH", 1 + i % 3, 10**6, 20)
    led.finalize(0, 30)
    amounts = {e.hotkey: e.amount for e in led.state().entitlements}
    assert len(amounts) == n and sum(amounts.values()) <= FULL_SHARE_MASS
    copies = [tmp_path / f"copy{i}" for i in range(3)]
    for d in copies:
        shutil.copytree(tmp_path, d, ignore=shutil.ignore_patterns("copy*"))
    timings, answers = [], set()
    for d in copies:
        other = Ledger(d, params(vest=1))
        assert json.loads(other.get_weights(0, 99, 0))["weights"] == {}
        start = time.process_time()
        answers.add(other.get_weights(2, 299, 0))
        timings.append(time.process_time() - start)
    assert statistics.median(timings) < 2.0, timings
    led.get_weights(0, 99, 0)
    body = json.loads(led.get_weights(2, 299, 0))
    assert answers == {canonical(body)}
    expected = sorted(amounts, key=lambda h: (-amounts[h], h))[:MAX_ENTRIES]
    assert sorted(body["weights"]) == sorted(expected)
    assert all(body["weights"][h] == amounts[h] for h in expected)
    assert body["metadata"]["hotkeys_capped"] == n - MAX_ENTRIES
    later = json.loads(led.get_weights(3, 399, 0))
    assert set(later["weights"]) == set(amounts) - set(expected)


def test_rejects_malformed_and_stale_input(tmp_path: Path) -> None:
    led = Ledger(tmp_path, params())
    led.commit(0, "a", 50)
    bad_calls = [
        lambda: led.commit(0, "a", 60),
        lambda: led.commit(0, "", 60),
        lambda: led.commit(0, "b", 40),
        lambda: led.commit(-1, "b", 60),
        lambda: led.commit(0, "b", 1.5),  # type: ignore[arg-type]
        lambda: led.verdict(0, "zz", "MATCH", 1, 0, 60),
        lambda: led.verdict(0, "a", "MAYBE", 1, 0, 60),
        lambda: led.finalize(0, 60),
        lambda: led.finalize(1, 60),
    ]
    for call in bad_calls:
        with pytest.raises(LedgerError):
            call()
    led.get_weights(0, 99, 0)
    with pytest.raises(LedgerError):
        led.commit(1, "b", 99)
    with pytest.raises(LedgerError):
        Ledger(tmp_path, params(vest=9))


def test_torn_tail_quarantined_and_corruption_detected(tmp_path: Path) -> None:
    led = Ledger(tmp_path, params(vest=1))
    run_round(led, 0, HONEST)
    first = led.get_weights(2, 299, 0)
    journal = tmp_path / "journal.jsonl"
    good = journal.read_bytes()
    journal.write_bytes(good + b'{"kind":"commit","seq":')
    reopened = Ledger(tmp_path, params(vest=1))
    assert reopened.journal.records[-1]["kind"] == "journal_repaired"
    sidecars = list(tmp_path.glob("journal.torn-*.bin"))
    assert [s.read_bytes() for s in sidecars] == [b'{"kind":"commit","seq":']
    assert reopened.get_weights(2, 0, 0) == first
    journal.write_bytes(good[:-1] + b"X")
    with pytest.raises(ChainBreak):
        Ledger(tmp_path, params(vest=1))


def test_one_flipped_byte_raises_chain_break(tmp_path: Path) -> None:
    led = Ledger(tmp_path, params(vest=1))
    run_round(led, 0, HONEST)
    led.get_weights(2, 299, 0)
    good = (tmp_path / "journal.jsonl").read_bytes()
    rng = random.Random(3)
    for pos in rng.sample(range(len(good)), 200) + [0, len(good) - 1]:
        flipped = bytearray(good)
        flipped[pos] ^= 1 << rng.randrange(8)
        with pytest.raises(ChainBreak):
            parse_chain(bytes(flipped))
        (tmp_path / "journal.jsonl").write_bytes(bytes(flipped))
        with pytest.raises(ChainBreak):
            Ledger(tmp_path, params(vest=1))


def test_property_seeded_random_scenarios(tmp_path: Path) -> None:
    rng = random.Random(20261007)
    verdicts = ["MATCH", "MATCH", "MATCH", "UNSAMPLED", "FAULT", "TRANSIENT", "NO_UPLOAD"]
    for scenario in range(1000):
        p = Params("hypertrain", 0, EPOCH, rng.randint(1, 3), rng.randint(1, 3))
        d = tmp_path / f"s{scenario}"
        led = Ledger(d, p)
        t, epoch, answers = 0, 0, {}
        miners = [f"m{i}" for i in range(rng.randint(1, 5))]
        for w in range(rng.randint(1, 6)):
            t = max(t, w * EPOCH * p.epochs_per_round) + 1
            active = [m for m in miners if rng.random() < 0.85]
            for m in active:
                led.commit(w, m, t)
            for m in active:
                led.verdict(
                    w,
                    m,
                    rng.choice(verdicts),
                    rng.randint(0, 5000),
                    rng.randint(-(10**6), 2 * 10**6),
                    t,
                )
            if rng.random() < 0.15:
                led.clawback(rng.choice(miners), rng.randint(1, 10**6), t)
            led.finalize(w, t)
            for _ in range(rng.randint(0, 2 * p.epochs_per_round)):
                t += rng.randint(1, EPOCH)
                answers[epoch] = led.get_weights(epoch, t, scenario)
                body = json.loads(answers[epoch])
                total = sum(body["weights"].values())
                assert total <= FULL_SHARE_MASS
                assert all(v >= 0 and v == int(v) for v in body["weights"].values())
                m = body["metadata"]
                assert (
                    m["ledger_minted"]
                    == m["ledger_burned"] + m["ledger_paid"] + m["ledger_pending"]
                )
                epoch += 1
        state = led.state()
        assert state.minted == state.burned + state.paid + state.pending
        assert all(e.outstanding >= 0 for e in state.entitlements)
        if answers:
            some = rng.choice(sorted(answers))
            led.commit(10**6, "late", t + 1)
            assert led.get_weights(some, t + 2, 0) == answers[some]
        rebuilt = Ledger(d, p)
        for e, raw in answers.items():
            assert rebuilt.get_weights(e, 0, 0) == raw


def test_vesting_anchored_to_finalize_round_not_w(tmp_path: Path) -> None:
    led = Ledger(tmp_path, params(vest=3))
    t = 10 * EPOCH
    for hk in "abc":
        led.commit(0, hk, t + 1)
    for hk in "abc":
        led.verdict(0, hk, "MATCH", 1000, 10**6, t + 2)
    led.finalize(0, t + 3)
    for e in (10, 11, 12):
        body = answer(led, e)
        assert body["metadata"]["round_at"] == e
        assert body["weights"] == {}
        assert body["metadata"]["units_burned_this_epoch"] == FULL_SHARE_MASS
    assert answer(led, 13)["weights"] == {"a": 333333.0, "b": 333333.0, "c": 333333.0}


def test_fifo_carry_over_pays_older_round_first(tmp_path: Path) -> None:
    led = Ledger(tmp_path, Params("hypertrain", 0, EPOCH, 2, 1))
    led.commit(0, "old", 10)
    led.verdict(0, "old", "MATCH", 1, 10**6, 20)
    led.finalize(0, 30)
    led.commit(1, "new", 200)
    led.verdict(1, "new", "MATCH", 1, 10**6, 210)
    led.finalize(1, 220)
    paid = [json.loads(led.get_weights(e, 400 + e, 0))["weights"] for e in range(4)]
    assert paid == [
        {"old": 1e6},
        {"old": 1e6},
        {"new": 1e6},
        {"new": 1e6},
    ]


def build_long_journal(directory: Path, rounds: int, miners: int, per_round: int) -> Ledger:
    led = Ledger(directory, Params("hypertrain", 0, EPOCH, per_round, 3))
    hotkeys = [f"m{i:03d}" for i in range(miners)]
    epoch = 0
    for w in range(rounds):
        base = w * EPOCH * per_round
        for hk in hotkeys:
            led.commit(w, hk, base + 1)
        for i, hk in enumerate(hotkeys):
            verdict = "FAULT" if (w == rounds // 2 and i == 0) else "MATCH"
            led.verdict(w, hk, verdict, 1000 + i, 10**6, base + 2)
        led.finalize(w, base + 3)
        for k in range(per_round):
            led.get_weights(epoch, base + k * EPOCH + 99, epoch)
            epoch += 1
    return led


@pytest.mark.parametrize(("rounds", "miners", "per_round"), [(200, 3, 1), (42, 60, 2)])
def test_open_is_linear_and_fast(tmp_path: Path, rounds: int, miners: int, per_round: int) -> None:
    led = build_long_journal(tmp_path, rounds, miners, per_round)
    n_records = len(led.journal.records)
    if miners == 60:
        assert n_records >= 5000
    answers = {e: r["answer"] for e, r in led._engine.answers.items()}
    start = time.perf_counter()
    reopened = Ledger(tmp_path, led.params)
    elapsed = time.perf_counter() - start
    assert elapsed < 5.0, (n_records, elapsed)
    for e, ans in answers.items():
        assert (
            reopened.get_weights(e, 0, 0)
            == json.dumps(ans, sort_keys=True, separators=(",", ":")).encode()
        )


def test_rechained_tampered_answer_raises_rebuild_mismatch(tmp_path: Path) -> None:
    led = Ledger(tmp_path, params(vest=1))
    run_round(led, 0, HONEST)
    led.get_weights(1, 199, 0)
    records = [dict(r) for r in led.journal.records]
    target = next(r for r in records if r["kind"] == "weights")
    target["answer"] = {**target["answer"], "weights": {"a": 999999.0}}
    prev, lines = GENESIS_HASH, []
    for rec in records:
        rec["prev_hash"] = prev
        rec.pop("hash")
        rec["hash"] = hashlib.sha256(canonical(rec)).hexdigest()
        prev = rec["hash"]
        lines.append(canonical(rec) + b"\n")
    (tmp_path / "journal.jsonl").write_bytes(b"".join(lines))
    with pytest.raises(RebuildMismatch):
        Ledger(tmp_path, params(vest=1))
