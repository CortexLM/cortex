import json
from pathlib import Path

import pytest

from hypertrain.datasets.__main__ import main
from hypertrain.storage.__main__ import main as storage_main


def test_publish_requires_target(tmp_path: Path) -> None:
    with pytest.raises(SystemExit):
        main(["publish", str(tmp_path)])


def test_publish_to_r2_without_credentials(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    for k in ("R2_ACCOUNT_ID", "R2_BUCKET"):
        monkeypatch.delenv(k, raising=False)
    (tmp_path / "manifest.json").write_text(json.dumps({}))
    with pytest.raises(Exception, match="R2_ACCOUNT_ID"):
        main(["publish", str(tmp_path), "--to", "r2"])


def test_doctor_without_credentials(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    for k in ("R2_ACCOUNT_ID", "R2_ACCESS_KEY_ID", "R2_SECRET_ACCESS_KEY", "R2_BUCKET"):
        monkeypatch.delenv(k, raising=False)
    assert storage_main(["doctor"]) == 2
    assert (
        "R2_ACCOUNT_ID, R2_ACCESS_KEY_ID, R2_SECRET_ACCESS_KEY, R2_BUCKET"
        in capsys.readouterr().err
    )
