from __future__ import annotations

import json
import logging
from pathlib import Path

import numpy as np
import pytest

from hypertrain.datasets.fetch import FetchError
from hypertrain.datasets.shards16 import ShardSet16Manifest, shard_name16, write_shards16
from hypertrain.storage.doctor import doctor
from hypertrain.storage.publish import publish_dir, publish_shardset, verify_public
from hypertrain.storage.r2 import ConfigError, ObjectConflict, R2Config, R2Error, load_config

from .fakes import CFG, SECRET, make


@pytest.fixture
def shard_dir(tmp_path: Path) -> Path:
    rng = np.random.default_rng(0)
    write_shards16(
        (rng.integers(0, 60000, 4) for _ in range(16)), tmp_path, 3, unit=2, samples_per_shard=4
    )
    (tmp_path / "build_record.json").write_text(json.dumps({"fragment": {}}))
    return tmp_path


def test_order_manifest_last(shard_dir: Path) -> None:
    fake, c, _ = make()
    publish_shardset(c, shard_dir)
    puts = [k for m, k in fake.log if m == "PUT"]
    assert len(puts) == 6 and puts[-2:] == ["build_record.json", "manifest.json"]


def test_idempotent_rerun(shard_dir: Path) -> None:
    fake, c, _ = make()
    publish_shardset(c, shard_dir)
    n = fake.puts
    rep = publish_shardset(c, shard_dir)
    assert fake.puts == n and not rep.uploaded and len(rep.skipped) == 6


def test_resume_after_failure(shard_dir: Path) -> None:
    fake, c, _ = make()
    fake.fail_put_after = 2
    with pytest.raises(R2Error):
        publish_shardset(c, shard_dir)
    assert "manifest.json" not in fake.objs and len(fake.objs) == 2
    fake.fail_put_after = None
    rep = publish_shardset(c, shard_dir)
    assert len(rep.skipped) == 2 and "manifest.json" in fake.objs and rep.verified_units == 8


def test_if_none_match_conflict(shard_dir: Path) -> None:
    fake, c, _ = make()
    m = ShardSet16Manifest.from_json((shard_dir / "manifest.json").read_text())
    key = m.shard_sha256s[0]
    fake.objs[key] = (b"other", {"sha256": "x"})
    # HEAD says different content, publisher tries a plain PUT only when absent -> overwrite is
    # refused by the raw immutable API:
    with pytest.raises(ObjectConflict):
        c.put_object(key, (shard_dir / shard_name16(0)).read_bytes(), immutable=True)
    # same content under If-None-Match is a benign False
    data = b"same"
    assert c.put_object("k", data, immutable=True) is True
    assert c.put_object("k", data, immutable=True) is False


def test_verify_detects_corruption(shard_dir: Path) -> None:
    fake, c, http = make()
    publish_shardset(c, shard_dir, verify_units=0)
    m = ShardSet16Manifest.from_json((shard_dir / "manifest.json").read_text())
    k = m.shard_sha256s[1]
    d, meta = fake.objs[k]
    fake.objs[k] = (bytes([d[0] ^ 1]) + d[1:], meta)
    with pytest.raises(FetchError):
        verify_public(m, "https://pub.test", 16, client=http)


def test_publish_dir_index_last(tmp_path: Path) -> None:
    (tmp_path / "a").mkdir()
    (tmp_path / "a" / "x.bin").write_bytes(b"1")
    (tmp_path / "y.bin").write_bytes(b"22")
    fake, c, _ = make()
    publish_dir(c, tmp_path, prefix="labels/v1")
    puts = [k for m, k in fake.log if m == "PUT"]
    assert puts[-1] == "labels/v1/labels-index.json" and "labels/v1/a/x.bin" in puts
    assert c.list_prefix("labels/v1/")[0][0].startswith("labels/v1/")


def test_doctor_pass_and_fail() -> None:
    fake, c, http = make()
    res = doctor(CFG, client=http, cors_origin="")
    assert all(ok for _, ok, _ in res) and [n for n, *_ in res][-1] == "delete" and not fake.objs
    fake.public_range = False
    res = doctor(CFG, client=http)
    assert any(n == "public-range" and not ok for n, ok, _ in res)


def test_config_missing_and_secrets(tmp_path: Path, caplog: pytest.LogCaptureFixture) -> None:
    with pytest.raises(ConfigError, match="R2_ACCOUNT_ID.*R2_BUCKET"):
        load_config({"R2_ACCESS_KEY_ID": "a", "R2_SECRET_ACCESS_KEY": "b"})
    f = tmp_path / ".env"
    f.write_text(
        f"R2_ACCOUNT_ID=a\nR2_ACCESS_KEY_ID=k\nR2_SECRET_ACCESS_KEY={SECRET}\nR2_BUCKET=bkt\n"
    )
    cfg = load_config({}, f)
    assert cfg.secret_access_key == SECRET and cfg.url == "https://a.r2.cloudflarestorage.com"
    assert SECRET not in repr(cfg) and SECRET not in str(cfg) and SECRET not in repr(make()[1])
    caplog.set_level(logging.DEBUG)
    fake, c, _ = make()
    fake.fail_put_after = 0
    with pytest.raises(R2Error) as ei:
        c.put_object("k", b"x")
    assert SECRET not in str(ei.value) and SECRET not in caplog.text
    assert isinstance(cfg, R2Config)
