from __future__ import annotations

import os
from pathlib import Path
from typing import Any

import pytest
from miner_harness import World, write_keyfile

from challenge.conftest import AUDITOR_SEED, WORKER
from hypertrain.auditor.worker import Auditor, HttpApi
from hypertrain.miner.core import KeyfileError, Miner, MinerConfig, StartStateMismatch, load_keyfile
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages import ReplayEnv

ENV = ReplayEnv(
    image_digest="sha256:" + "1" * 64, driver="cpu", gpu_uuid_sha256=sha256_hex(b"cpu"), sm_count=1
)


def _audit_until_done(w: World, rnd: int) -> list[str | None]:
    view = w.op.round(rnd)
    assert view is not None
    w.op.push(view["round_open"]["body"]["d_audit"])
    auditor = Auditor(HttpApi(w.c, WORKER), Keypair(AUDITOR_SEED), ENV, w.get)
    out = []
    while (r := auditor.run_once()) is not None:
        out.append(r)
    return out


def test_keyfile_mode_0644_rejected(tmp_path: Path) -> None:
    key = write_keyfile(tmp_path / "k", b"\x41" * 32, 0o644)
    with pytest.raises(KeyfileError, match="0600"):
        load_keyfile(key)
    key.chmod(0o600)
    assert load_keyfile(key).ss58


def test_honest_and_cheating_round_settle(world_factory: Any) -> None:
    w: World = world_factory(n=2)
    honest = w.miner(0)

    def poisoned(i: int) -> Any:
        x = w.get(i).copy()
        x[0] = (x[0] + 1) % 64
        return x

    cheat = w.miner(1, get=poisoned)
    assert honest.run_round(0) == "UPLOADED"
    assert cheat.run_round(0) == "UPLOADED"
    assert w.status(0) == {k.ss58: "UPLOADED" for k in w.keys}
    assert sorted(_audit_until_done(w, 0)) == ["MATCH", "MISMATCH"]
    assert w.status(0) == {w.keys[0].ss58: "MATCH", w.keys[1].ss58: "MISMATCH"}
    view = w.op.round(0)
    assert view is not None
    assert [f["body"]["hotkey"] for f in view["forfeits"]] == [w.keys[1].ss58]
    assert cheat.dispute(0) == "CONTEST"
    assert honest.dispute(0) is None


def test_two_rounds_follow_published_state(world_factory: Any) -> None:
    w: World = world_factory(n=1)
    m = w.miner(0)
    assert m.run(2) == [(0, "UPLOADED"), (1, "UPLOADED")]
    assert w.status(0) == w.status(1) == {w.keys[0].ss58: "UPLOADED"}
    assert len(m.journal.all("committed")) == 2


def test_crash_after_commit_resumes_without_recommit(
    world_factory: Any, monkeypatch: pytest.MonkeyPatch
) -> None:
    w: World = world_factory(n=1)
    m = w.miner(0)

    def crash(*_: Any) -> None:
        raise SystemExit("killed after commit")

    monkeypatch.setattr(Miner, "_upload", crash)
    with pytest.raises(SystemExit):
        m.run_round(0)
    assert w.status(0) == {w.keys[0].ss58: "COMMITTED"}
    monkeypatch.undo()
    posts: list[str] = []
    again = w.miner(0)
    real = again._post

    def spy(path: str, body: Any) -> Any:
        posts.append(path)
        return real(path, body)

    again._post = spy  # type: ignore[method-assign]
    assert again.run_round(0) == "UPLOADED"
    assert "commit" not in posts and posts.count("uploads") == 1
    assert len(again.journal.all("committed", w=0)) == 1
    assert w.status(0) == {w.keys[0].ss58: "UPLOADED"}


@pytest.mark.parametrize("server_state", ["COMMITTED", "UPLOADED"])
def test_duplicate_commit_409_continues_without_second_upload(
    world_factory: Any, monkeypatch: pytest.MonkeyPatch, server_state: str
) -> None:
    """Crash between the server accepting a step and our journal fsync: the resend gets 409."""
    w: World = world_factory(n=1)
    m = w.miner(0)
    lost = "committed" if server_state == "COMMITTED" else "uploaded"
    append = m.journal.append

    def lose(kind: str, **kw: Any) -> Any:
        if kind == lost:
            raise SystemExit("killed before journal fsync")
        return append(kind, **kw)

    monkeypatch.setattr(m.journal, "append", lose)
    with pytest.raises(SystemExit):
        m.run_round(0)
    for path in m.journal.path.parent.glob("journal.jsonl"):
        lines = [x for x in path.read_text().splitlines() if f'"kind": "{lost}"' not in x]
        if server_state == "UPLOADED":
            lines = [x for x in lines if '"kind": "committed"' not in x]
        path.write_text("".join(x + "\n" for x in lines))
    assert w.status(0) == {w.keys[0].ss58: server_state}
    again = w.miner(0)
    puts: list[str] = []
    real_put = again._put
    again._put = lambda url, data, sha: (puts.append(sha), real_put(url, data, sha))[1]  # type: ignore[method-assign]
    assert again.run_round(0) == "UPLOADED"
    assert len(puts) == (1 if server_state == "COMMITTED" else 0)
    assert w.status(0) == {w.keys[0].ss58: "UPLOADED"}


def test_wrong_start_theta_hash_aborts_with_typed_error(world_factory: Any) -> None:
    w: World = world_factory(n=1, bad_init=True)
    m = w.miner(0)
    with pytest.raises(StartStateMismatch):
        m.run_round(0)
    assert w.status(0) == {w.keys[0].ss58: "ASSIGNED"}
    assert m.run_round(0) == "START_STATE_MISMATCH"


def test_segments_challenge_answered_with_state_serve(world_factory: Any) -> None:
    w: World = world_factory(policy="carry", n=1)
    m = w.miner(0, sink=True)
    assert m.run_round(0) == "UPLOADED"
    view = w.op.round(0)
    assert view is not None
    w.op.push(view["round_open"]["body"]["d_audit"])
    segs = next(j["challenge"]["body"]["segments"] for j in w.op.round(0)["jobs"])  # type: ignore[index]
    assert m.serve_challenges(0) == len(segs) >= 1
    assert m.serve_challenges(0) == 0
    job = w.c.post("/v1/worker/lease", headers={"authorization": f"Bearer {WORKER}"}).json()
    served = w.c.get(
        f"/v1/worker/jobs/{job['id']}/serves",
        params={"lease": job["lease"]},
        headers={"authorization": f"Bearer {WORKER}"},
    ).json()["serves"]
    assert sorted(s["serve"]["t"] for s in served) == sorted(a * w.m.inner.J for a, _ in segs)


def test_config_file_and_env(tmp_path: Path) -> None:
    toml = tmp_path / "m.toml"
    toml.write_text(
        'api = "http://127.0.0.1:1"\nkeyfile = "k"\nworkdir = "work"\n'
        'state_source = "states"\nimage_digest = "sha256:00"\n'
    )
    cfg = MinerConfig.load(toml, {"HYPERTRAIN_MINER_POLL_SECONDS": "0.5"})
    assert cfg.keyfile == (tmp_path / "k").resolve() and cfg.poll_seconds == 0.5
    toml.write_text(toml.read_text() + 'exec_hook = "rm -rf /"\n')
    with pytest.raises(Exception, match="unknown config keys"):
        MinerConfig.load(toml, {})
    assert os.environ.get("HYPERTRAIN_MINER_KEYFILE") is None
