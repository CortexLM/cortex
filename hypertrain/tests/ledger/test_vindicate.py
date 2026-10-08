import json
from pathlib import Path

import pytest

from hypertrain.ledger import Ledger, LedgerError, Params, RebuildMismatch

EPOCH = 100
RUN = "r" * 64


def params() -> Params:
    return Params("hypertrain", 0, EPOCH, 1, 3)


def round0(led: Ledger, b_verdict: str = "NO_UPLOAD") -> None:
    for hk in ("a", "b"):
        led.commit(0, hk, 10)
    led.verdict(0, "a", "MATCH", 1000, 0, 20)
    led.verdict(0, "b", b_verdict, 3000, 0, 20)
    led.finalize(0, 30)


def weights(led: Ledger, epoch: int) -> dict:
    return json.loads(led.get_weights(epoch, epoch * EPOCH + 99, epoch))["weights"]


def conserved(led: Ledger) -> None:
    s = led.state()
    assert s.minted == s.burned + s.paid + s.pending


def test_vindicated_round_is_paid_after_vesting_from_credit_round(tmp_path: Path) -> None:
    led = Ledger(tmp_path, params())
    round0(led)
    assert weights(led, 0) == {}
    assert led.state().burned == 750_000
    assert led.vindicate(RUN, 0, "b", 150) is True
    conserved(led)
    assert led.state().burned == 0
    paid: dict[str, float] = {}
    for epoch in range(1, 6):
        for hk, units in weights(led, epoch).items():
            paid[hk] = paid.get(hk, 0) + units
        if epoch == 3:
            assert set(paid) == {"a"}
    assert paid == {"a": 250_000.0, "b": 750_000.0}
    conserved(led)
    Ledger(tmp_path, params()).verify()


def test_vindicate_is_idempotent_per_run_round_hotkey(tmp_path: Path) -> None:
    led = Ledger(tmp_path, params())
    round0(led)
    assert led.vindicate(RUN, 0, "b", 150) is True
    assert led.vindicate(RUN, 0, "b", 160) is False
    assert sum(e.amount for e in led.state().entitlements if e.hotkey == "b") == 750_000
    reopened = Ledger(tmp_path, params())
    assert reopened.vindicate(RUN, 0, "b", 170) is False


@pytest.mark.parametrize(
    ("verdict", "w", "hk"),
    [("MATCH", 0, "b"), ("FAULT", 0, "b"), ("NO_UPLOAD", 1, "b"), ("NO_UPLOAD", 0, "zz")],
)
def test_only_finalized_no_upload_can_be_vindicated(
    tmp_path: Path, verdict: str, w: int, hk: str
) -> None:
    led = Ledger(tmp_path, params())
    round0(led, verdict)
    with pytest.raises(LedgerError):
        led.vindicate(RUN, w, hk, 150)


def test_unfinalized_round_cannot_be_vindicated(tmp_path: Path) -> None:
    led = Ledger(tmp_path, params())
    led.commit(0, "b", 10)
    led.verdict(0, "b", "NO_UPLOAD", 10, 0, 20)
    with pytest.raises(LedgerError):
        led.vindicate(RUN, 0, "b", 25)


def test_forged_duplicate_vindicate_record_breaks_rebuild(tmp_path: Path) -> None:
    led = Ledger(tmp_path, params())
    round0(led)
    led.vindicate(RUN, 0, "b", 150)
    rec = {"run_id": RUN, "w": 0, "hotkey": "b", "at": 160, "final_round": 1}
    led.journal.append("vindicate", rec)
    with pytest.raises(RebuildMismatch):
        Ledger(tmp_path, params())
