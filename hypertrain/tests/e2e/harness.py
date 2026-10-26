from __future__ import annotations

import json
import os
import selectors
import subprocess
import sys
import time
from pathlib import Path
from typing import Any

import httpx
from common import (
    ADMIN,
    HTTP_LIMITS,
    INTERNAL,
    OWNER,
    RUN_CONFIG,
    SLUG,
    WORKER,
    beacon_payload,
    bearer,
)
from trainer_fixtures import sample

from hypertrain.protocol.envelope import seal
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages import RunManifest
from hypertrain.trainer.config import TrainConfig

READY_SECONDS = 60.0
# User CPU budget (2026-10-08): every spawned process runs single-threaded.
ONE_THREAD = {**os.environ, "OMP_NUM_THREADS": "1", "MKL_NUM_THREADS": "1"}
NO_REPLAY = os.environ.get("HT_E2E_NO_REPLAY") == "1"


class Server:
    def __init__(self, tmp: Path, vest: int) -> None:
        self.state = tmp / "state"
        secrets = tmp / "secrets"
        secrets.mkdir(parents=True)
        for name, value in (("internal", INTERNAL), ("admin", ADMIN), ("worker", WORKER)):
            (secrets / f"{name}.token").write_text(value)
        (secrets / "coord.key").write_text(bytes(range(32)).hex())
        self.registry = tmp / "registry.json"
        self.registry.write_text("[]")
        script = Path(__file__).with_name("server.py")
        args = [sys.executable, str(script), str(self.state), str(secrets), str(self.registry)]
        self.log_path = tmp / "challenge-app.log"
        self.log = self.log_path.open("w")
        self.proc = subprocess.Popen(
            [*args, str(vest)], stdout=subprocess.PIPE, stderr=self.log, text=True, env=ONE_THREAD
        )
        self.base = f"http://127.0.0.1:{self._ready_port()}"
        self.c = httpx.Client(base_url=self.base, timeout=120, limits=HTTP_LIMITS)
        r = self.c.get("/health", timeout=READY_SECONDS)
        assert r.status_code == 200, f"challenge /health: {r.status_code} {r.text}"

    def _ready_port(self) -> int:
        assert self.proc.stdout is not None
        sel = selectors.DefaultSelector()
        sel.register(self.proc.stdout, selectors.EVENT_READ)
        deadline = time.monotonic() + READY_SECONDS
        while (left := deadline - time.monotonic()) > 0 and sel.select(left):
            line = self.proc.stdout.readline()
            if not line:
                break
            if line.startswith("READY "):
                return int(line.split()[1])
        self.proc.kill()
        self.log.flush()
        raise AssertionError(f"server not ready: {self.log_path.read_text()[-2000:]}")

    def log_text(self) -> str:
        self.log.flush()
        return self.log_path.read_text()

    def register(self, hotkeys: list[str]) -> None:
        self.registry.write_text(json.dumps(hotkeys))

    def stop(self) -> None:
        self.c.close()
        self.proc.terminate()
        self.proc.wait(timeout=30)
        self.log.close()


ACTOR_READY_SECONDS = 180.0
ACTOR_CALL_SECONDS = 900.0


class Proc:
    """One actor OS process (tests/e2e/actor.py ROLE): READY gate, one RESULT line per command,
    stderr log kept in the scenario dir, exit code recorded on close."""

    def __init__(
        self, name: str, role: str, cfg: dict[str, Any], logdir: Path, expect_fail: bool = False
    ) -> None:
        self.name, self.role = name, role
        logdir.mkdir(parents=True, exist_ok=True)
        cfg_path = logdir / f"{name}.config.json"
        cfg_path.write_text(json.dumps(cfg, indent=1, default=str))
        self.log_path = logdir / f"{name}.log"
        self.log = self.log_path.open("w")
        script = Path(__file__).with_name("actor.py")
        self.proc = subprocess.Popen(
            [sys.executable, str(script), role, str(cfg_path)],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=self.log,
            text=True,
            env=ONE_THREAD,
        )
        self.sel = selectors.DefaultSelector()
        assert self.proc.stdout is not None
        self.sel.register(self.proc.stdout, selectors.EVENT_READ)
        self.exit_code: int | None = None
        self.calls: list[dict[str, Any]] = []
        self.ready = False
        try:
            self.ready = self._line("READY", ACTOR_READY_SECONDS) == "READY"
        except AssertionError:
            if not expect_fail:
                raise
            self.proc.wait(timeout=60)
            self.exit_code = self.proc.returncode
            self.log.close()
            return
        assert self.ready and not expect_fail, f"{name}: READY={self.ready} expect_fail"

    def tail(self) -> str:
        if not self.log.closed:
            self.log.flush()
        return self.log_path.read_text()[-3000:]

    def _line(self, prefix: str, timeout: float) -> str:
        assert self.proc.stdout is not None
        deadline = time.monotonic() + timeout
        while (left := deadline - time.monotonic()) > 0 and self.sel.select(left):
            line = self.proc.stdout.readline()
            if not line:
                break
            if line.startswith(prefix):
                return line.rstrip("\n")
        raise AssertionError(
            f"{self.name}: no {prefix} line (exit={self.proc.poll()}); log {self.tail()}"
        )

    def send(self, cmd: str, **args: Any) -> None:
        assert self.proc.stdin is not None
        self.proc.stdin.write(json.dumps({"cmd": cmd, "args": args}, default=str) + "\n")
        self.proc.stdin.flush()
        self.calls.append({"cmd": cmd, "args": args})

    def recv(self) -> Any:
        out = json.loads(self._line("RESULT ", ACTOR_CALL_SECONDS)[len("RESULT ") :])
        self.calls[-1].update(out)
        if "error" in out:
            raise ActorError(
                f"{self.name}.{self.calls[-1]['cmd']}: {out['error']}: {out['msg']}\n"
                f"actor exit={self.proc.poll()}; stderr:\n{self.tail()}"
            )
        return out["ok"]

    def call(self, cmd: str, **args: Any) -> Any:
        self.send(cmd, **args)
        return self.recv()

    def close(self) -> int:
        if self.exit_code is None:
            if self.proc.poll() is None:
                assert self.proc.stdin is not None
                try:
                    self.proc.stdin.write('{"cmd": "exit"}\n')
                    self.proc.stdin.close()
                except BrokenPipeError:
                    pass
                try:
                    self.proc.wait(timeout=60)
                except subprocess.TimeoutExpired:
                    self.proc.kill()
                    self.proc.wait(timeout=30)
            self.exit_code = self.proc.returncode
            self.log.close()
        return self.exit_code

    def kill(self) -> int:
        self.proc.kill()
        self.proc.wait(timeout=30)
        self.exit_code = self.proc.returncode
        self.log.close()
        return self.exit_code


class ActorError(RuntimeError):
    pass


class World:
    """One run: challenge app (uvicorn process), aggregator process, auditor process and one
    process per miner. The test process is only the owner/admin and the drand relay."""

    def __init__(
        self,
        tmp: Path,
        m: RunManifest,
        roster: dict[Keypair, dict[str, Any]],
        honeypot_commit: str | None = None,
        logdir: Path | None = None,
    ) -> None:
        self.tmp, self.m, self.run_id = tmp, m, m.run_id()
        self.logdir = logdir or tmp / "logs"
        self.srv = Server(tmp, m.verify.E_vest_rounds)
        self.c = self.srv.c
        self.keys = list(roster)
        self.srv.register([k.ss58 for k in self.keys])
        self.cfg = TrainConfig.from_manifest(m.body())
        self.get = sample(self.cfg)
        self.states = tmp / "states"
        self.states.mkdir()
        self.manifest_path = tmp / "manifest.json"
        self.manifest_path.write_text(m.model_dump_json())
        self.procs: dict[str, Proc] = {}
        self.d = 0
        self.push(1000)
        self.agg = self.spawn_aggregator("aggregator", tmp / "agg")
        self.state_key = {0: self.agg.call("genesis")}
        env = seal(OWNER, "RunManifest", self.run_id, m, 2**40)
        self.ok(self.c.post("/v1/admin/runs", json=env, headers=bearer(ADMIN)))
        if honeypot_commit:
            self.admin("/honeypot", {"commitment": honeypot_commit})
        self.admin("/config", RUN_CONFIG)
        for k, entry in roster.items():
            self.admin(f"/roster/{k.ss58}", entry)
        self.admin("/paused", {"paused": False})
        self.agg.call("publish", w=0)
        self.auditor = self.spawn("auditor", "auditor", self._base_cfg())

    def _base_cfg(self) -> dict[str, Any]:
        return {
            "api": self.srv.base,
            "manifest": str(self.manifest_path),
            "objects": str(self.srv.state / "objects"),
            "states": str(self.states),
        }

    def spawn(self, name: str, role: str, cfg: dict[str, Any], expect_fail: bool = False) -> Proc:
        p = Proc(name, role, cfg, self.logdir, expect_fail)
        self.procs[name] = p
        return p

    def spawn_aggregator(
        self, name: str, state_dir: Path, anchor: Any = None, expect_fail: bool = False
    ) -> Proc:
        cfg = {**self._base_cfg(), "state_dir": str(state_dir), "anchor": anchor}
        return self.spawn(name, "aggregator", cfg, expect_fail)

    def miner(self, k: Keypair, mode: str = "honest", **flags: Any) -> Proc:
        keyfile = self.tmp / f"{k.ss58}.key"
        keyfile.write_text(World.seeds[k.ss58].hex())
        keyfile.chmod(0o600)
        cfg = {
            **self._base_cfg(),
            "keyfile": str(keyfile),
            "workdir": str(self.tmp / "miners" / k.ss58),
            "mode": mode,
            **flags,
        }
        return self.spawn(f"miner-{mode}-{k.ss58[:8]}", "miner", cfg)

    def keyfile(self, k: Keypair) -> str:
        path = self.tmp / f"{k.ss58}.key"
        path.write_text(World.seeds[k.ss58].hex())
        path.chmod(0o600)
        return str(path)

    @staticmethod
    def ok(r: httpx.Response) -> Any:
        assert r.status_code in (200, 201), f"{r.request.url}: {r.status_code} {r.text}"
        return r.json()

    def admin(self, path: str, body: Any) -> Any:
        return self.ok(
            self.c.put(f"/v1/admin/runs/{self.run_id}{path}", json=body, headers=bearer(ADMIN))
        )

    def latest(self) -> int:
        return int(self.ok(self.c.get("/v1/beacon/latest"))["round"])

    def run_all(self, miners: list[Proc], w: int, **args: Any) -> list[Any]:
        """Send run_round to every miner process at once, then collect (they run concurrently)."""
        for mn in miners:
            mn.send("run_round", w=w, **args)
        try:
            return [mn.recv() for mn in miners]
        except ActorError as error:
            raise ActorError(
                f"{error}\nchallenge-app exit={self.srv.proc.poll()}; "
                f"stderr:\n{self.srv.log_text()[-5000:]}"
            ) from error

    def push(self, rnd: int) -> None:
        self.d = max(self.d, self.latest())
        while self.d < rnd:
            self.d += 1
            self.ok(
                self.c.post("/v1/admin/beacon", json=beacon_payload(self.d), headers=bearer(ADMIN))
            )

    def jump(self, rnd: int) -> None:
        """Push one later drand round directly (the store's clock is the max known round)."""
        self.ok(self.c.post("/v1/admin/beacon", json=beacon_payload(rnd), headers=bearer(ADMIN)))
        self.d = max(self.d, rnd)

    def view(self, w: int) -> dict[str, Any]:
        out: dict[str, Any] = self.ok(self.c.get(f"/v1/runs/{self.run_id}/rounds/{w}"))
        return out

    def body(self, w: int) -> dict[str, Any]:
        out: dict[str, Any] = self.view(w)["round_open"]["body"]
        return out

    def status(self, w: int) -> dict[str, str]:
        return {m["hotkey"]: m["status"] for m in self.view(w)["miners"]}

    def audit(self, w: int, serving: Any = (), past_serve_deadline: bool = False) -> list[str]:
        self.push(self.body(w)["d_audit"])
        for mn in serving:
            mn.call("serve", w=w)
        jobs = self.view(w)["jobs"]
        if jobs and past_serve_deadline:
            self.push(max(j["challenge"]["body"]["serve_deadline"] for j in jobs) + 1)
        out: list[str] = self.auditor.call("audit")
        return out

    def aggregate(self, w: int, exclude: Any = ()) -> dict[str, Any]:
        self.push(self.body(w)["d_upload"])
        tape: dict[str, Any] = self.agg.call("aggregate", w=w, exclude=list(exclude))
        self.state_key[w + 1] = tape["body"]["out_state"]
        return tape

    def finalize(self, w: int) -> dict[str, Any]:
        self.push(self.body(w)["d_final"])
        out: dict[str, Any] = self.agg.call("finalize", w=w)
        return out

    def weights(self, epoch: int, epoch_at: int) -> dict[str, Any]:
        out: dict[str, Any] = self.ok(
            self.c.get(
                f"/internal/v1/get_weights?epoch={epoch}&epoch_at={epoch_at}",
                headers={**bearer(INTERNAL), "x-platform-challenge-slug": SLUG},
            )
        )
        return out

    def weights_raw(self, epoch: int, epoch_at: int) -> bytes:
        r = self.c.get(
            f"/internal/v1/get_weights?epoch={epoch}&epoch_at={epoch_at}",
            headers={**bearer(INTERNAL), "x-platform-challenge-slug": SLUG},
        )
        assert r.status_code == 200, r.text
        return r.content

    def ledger(self) -> Any:
        import shutil

        from common import EPOCH_SECONDS

        from hypertrain.ledger import Ledger, Params
        from hypertrain.protocol.messages import QUICKNET_GENESIS

        dst = self.tmp / f"ledger-copy-{self.d}"
        shutil.rmtree(dst, ignore_errors=True)
        shutil.copytree(self.srv.state / "ledger", dst)
        p = Params(SLUG, QUICKNET_GENESIS, EPOCH_SECONDS, 1, self.m.verify.E_vest_rounds)
        return Ledger(dst, p)

    def stop(self) -> dict[str, Any]:
        codes = {}
        for name, p in self.procs.items():
            codes[name] = p.close()
        self.srv.stop()
        codes["challenge-app"] = self.srv.proc.returncode
        (self.logdir / "challenge-app.log").write_text(self.srv.log_path.read_text())
        return codes

    seeds: dict[str, bytes] = {}


def key(i: int) -> Keypair:
    seed = bytes([i]) * 32
    kp = Keypair(seed)
    World.seeds[kp.ss58] = seed
    return kp
