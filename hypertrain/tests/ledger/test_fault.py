import json
from pathlib import Path

import pytest

from hypertrain.ledger import Ledger, LedgerError, Params, RebuildMismatch

EPOCH = 100
RUN = "r" * 64


def params() -> Params:
    return Params("hypertrain", 0, EPOCH, 1, 3)


def two_rounds(led: Ledger) -> None:
    for hk in ("a", "b"):
        led.commit(0, hk, 10)
    led.verdict(0, "a", "MATCH", 1000, 0, 20)
    led.verdict(0, "b", "MATCH", 3000, 0, 20)
    led.finalize(0, 30)
    for hk in ("a", "b"):
        led.commit(1, hk, 40)
    led.verdict(1, "a", "MATCH", 1000, 0, 50)
    led.verdict(1, "b", "NO_UPLOAD", 3000, 0, 50)
    led.finalize(1, 60)


def weights(led: Ledger, epoch: int, epoch_at: int | None = None) -> dict:
    at = epoch * EPOCH + 99 if epoch_at is None else epoch_at
    return json.loads(led.get_weights(epoch, at, epoch))["weights"]


def conserved(led: Ledger) -> None:
    s = led.state()
    assert s.minted == s.burned + s.paid + s.pending


def test_dispute_lost_after_finality_burns_escrow_and_blacklists(tmp_path: Path) -> None:
    led = Ledger(tmp_path, params())
    two_rounds(led)
    before = led.state()
    assert sum(e.outstanding for e in before.entitlements if e.hotkey == "b") == 750_000
    assert led.fault(RUN, 1, "b", 70) is True
    s = led.state()
    assert "b" in s.blacklist
    assert sum(e.outstanding for e in s.entitlements if e.hotkey == "b") == 0
    assert s.burned == before.burned + 750_000
    assert led.burned_for_fault(1, "b") == (750_000, 750_000)
    conserved(led)
    paid: dict[str, float] = {}
    for epoch in range(0, 8):
        for hk, units in weights(led, epoch).items():
            paid[hk] = paid.get(hk, 0) + units
    assert "b" not in paid and paid == {"a": 500_000.0}
    conserved(led)
    Ledger(tmp_path, params()).verify()


def test_vested_escrow_is_frozen_once_the_fault_is_journaled(tmp_path: Path) -> None:
    led = Ledger(tmp_path, params())
    two_rounds(led)
    led.fault(RUN, 1, "b", 500)
    assert "b" not in weights(led, 4, 450)
    assert led.burned_for_fault(1, "b")[1] == 750_000
    conserved(led)


def test_fault_is_idempotent_and_blocks_vindication(tmp_path: Path) -> None:
    led = Ledger(tmp_path, params())
    two_rounds(led)
    assert led.fault(RUN, 1, "b", 70) is True
    burned = led.state().burned
    assert led.fault(RUN, 1, "b", 80) is False
    assert Ledger(tmp_path, params()).fault(RUN, 1, "b", 90) is False
    assert led.state().burned == burned
    with pytest.raises(LedgerError):
        led.vindicate(RUN, 1, "b", 95)


def test_fault_after_vindication_rejected(tmp_path: Path) -> None:
    led = Ledger(tmp_path, params())
    two_rounds(led)
    assert led.vindicate(RUN, 1, "b", 70) is True
    with pytest.raises(LedgerError):
        led.fault(RUN, 1, "b", 80)


@pytest.mark.parametrize(("w", "hk"), [(0, "b"), (1, "a"), (2, "b")])
def test_only_finalized_no_upload_can_fault(tmp_path: Path, w: int, hk: str) -> None:
    led = Ledger(tmp_path, params())
    two_rounds(led)
    with pytest.raises(LedgerError):
        led.fault(RUN, w, hk, 70)


def test_blacklisted_hotkey_gets_no_future_entitlement(tmp_path: Path) -> None:
    led = Ledger(tmp_path, params())
    two_rounds(led)
    led.fault(RUN, 1, "b", 70)
    for hk in ("a", "b"):
        led.commit(2, hk, 80)
    led.verdict(2, "a", "MATCH", 1000, 0, 85)
    led.verdict(2, "b", "MATCH", 3000, 0, 85)
    led.finalize(2, 90)
    assert [e for e in led.state().entitlements if e.hotkey == "b" and e.w == 2] == []
    conserved(led)


def test_forged_duplicate_fault_record_breaks_rebuild(tmp_path: Path) -> None:
    led = Ledger(tmp_path, params())
    two_rounds(led)
    led.fault(RUN, 1, "b", 70)
    led.journal.append("fault", {"run_id": RUN, "w": 1, "hotkey": "b", "at": 80})
    with pytest.raises(RebuildMismatch):
        Ledger(tmp_path, params())


def test_in_round_fault_records_the_burned_slice(tmp_path: Path) -> None:
    led = Ledger(tmp_path, params())
    for hk in ("a", "b"):
        led.commit(0, hk, 10)
    led.verdict(0, "a", "MATCH", 1000, 0, 20)
    led.verdict(0, "b", "FAULT", 3000, 0, 20)
    led.finalize(0, 30)
    assert led.state().burned == 750_000
    assert led.burned_for_fault(0, "b") == (750_000, 0)
    conserved(led)
