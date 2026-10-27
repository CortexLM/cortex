"""In-process world for the audit-cheaters suite: full v2 rounds with honest and cheating miners.

One `World` is the real challenge app (FastAPI + TestClient, signed manifest, test ledger, fixture
beacon, in-process relay) plus N graduated identities. It drives the real protocol path join-v2 ->
probation -> run-v2 -> UPLOADED -> audit -> aggregate -> finalize -> get_weights on CPU with the
tiny fixture model, and it records what every step answered instead of raising on the refusals
and stalls that cheating miners provoke.

Graduating an identity costs about 35 s (12 real reference trials), so a graduated network is
snapshotted once (SQLite backup + object/trial directories) and restored into a function-scoped
clone in about 0.1 s. Everything here runs inside pytest; nothing talks to a live host.
"""

from __future__ import annotations

import contextlib
import json
import os
import resource
import shutil
import sqlite3
import sys
import time
from collections.abc import Callable, Iterator, Mapping, Sequence
from dataclasses import asdict, dataclass
from importlib import util as importlib_util
from pathlib import Path
from types import SimpleNamespace
from typing import Any, Literal, Protocol

import httpx
import pytest

from hypertrain.miner.core import ChallengeHTTPError, MinerConfig, NetworkMiner
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages import Finalize
from hypertrain.protocol.messages_v2 import RoundOpenV2

TESTS = Path(__file__).resolve().parents[1]
ROOT = TESTS.parent
if str(ROOT / "scripts") not in sys.path:
    sys.path.append(str(ROOT / "scripts"))

import mine_multi_round_v2 as multi_round  # noqa: E402
import mine_smoke_v2 as smoke  # noqa: E402

MAX_IDENTITIES = 12
SNAPSHOT_FORMAT = 1
# Directories of the master state dir that the snapshot carries besides challenge.db.
STATE_DIRS = ("objects", "trials-v2", "ledger")
EVIDENCE_KINDS = ("excluded", "contest", "verdict", "replay", "finality")
GPU_SKIP = "the GPU path needs the qualified CUDA TEST stack; not run here"

_REAL_TIME = time.time
_SERVICES: dict[int, Any] = {}


def require_cpu(device: str) -> str:
    """The suite runs on CPU; any other device skips with a clear message."""
    if device != "cpu":
        pytest.skip(GPU_SKIP)
    return device


def record(result: dict[str, Any]) -> None:
    """Append one JSON line to $AUDIT_CHEATERS_EVIDENCE; a no-op when the variable is unset."""
    target = os.environ.get("AUDIT_CHEATERS_EVIDENCE")
    if not target:
        return
    with open(target, "a", encoding="utf-8") as out:
        out.write(json.dumps(result, sort_keys=True, default=str) + "\n")


def peak_rss_mb() -> float:
    return resource.getrusage(resource.RUSAGE_SELF).ru_maxrss / 1024


@dataclass(frozen=True)
class Shape:
    """`identities` exist (hot/cold keys, genesis, fake metagraph); the first `graduated` are
    ACTIVE after 12 reference trials; the others stay unregistered for trial-stage scenarios."""

    identities: int = 5
    graduated: int = 5

    def __post_init__(self) -> None:
        if not 1 <= self.graduated <= self.identities <= MAX_IDENTITIES:
            raise ValueError(f"need 1 <= graduated <= identities <= {MAX_IDENTITIES}")

    @property
    def key(self) -> str:
        return f"i{self.identities}g{self.graduated}"


def load_service(identities: int) -> Any:
    """tests/challenge/test_service_network_v2.py under its own module name, with `identities`
    hot/cold keypairs and a genesis whose total_units is the sum of its origins (precedent:
    tests/e2e/test_rollback_service_v2.py)."""
    if identities in _SERVICES:
        return _SERVICES[identities]
    name = f"audit_cheaters_service_{identities}"
    spec = importlib_util.spec_from_file_location(
        name, TESTS / "challenge/test_service_network_v2.py"
    )
    assert spec is not None and spec.loader is not None
    module = importlib_util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    module.HOT = [module.Keypair(bytes([80 + i]) * 32) for i in range(identities)]
    module.COLD = [module.Keypair(bytes([90 + i]) * 32) for i in range(identities)]
    original = module.Genesis

    def full_genesis(**fields: Any) -> Any:
        fields["total_units"] = sum(origin.units for origin in fields["origins"])
        return original(**fields)

    module.Genesis = full_genesis
    _SERVICES[identities] = module
    return module


def _build_network(svc: Any, directory: Path) -> tuple[Any, Any]:
    directory.mkdir(parents=True, exist_ok=True)
    generator = svc.network.__wrapped__(directory, SimpleNamespace(param=None))
    return generator, next(generator)


@contextlib.contextmanager
def _frozen_time(at: float) -> Iterator[None]:
    """The network fixture stamps genesis_time = int(time.time()) - 1: freeze it so a clone gets
    exactly the snapshot's genesis (hence the same run_id) however slow the host is."""
    patch = pytest.MonkeyPatch()
    patch.setattr(time, "time", lambda: at)
    try:
        yield
    finally:
        patch.undo()


@dataclass(frozen=True)
class MinerOutcome:
    index: int
    hotkey: str
    cheat: str
    status: Literal["UPLOADED", "refused", "withdrawn", "error"]
    http_status: int | None = None
    detail: str | None = None
    endpoint: str | None = None
    stage: str | None = None
    error: str | None = None
    seconds: float = 0.0

    @property
    def uploaded(self) -> bool:
        return self.status == "UPLOADED"


@dataclass(frozen=True)
class Step:
    """One master call of settle(): its HTTP status and detail, or the exception it raised."""

    name: str
    status: int
    detail: str | None = None
    body: Any = None
    subject: str | None = None
    exception: str | None = None

    @property
    def ok(self) -> bool:
        return self.status == 200


@dataclass(frozen=True)
class AuditVerdict:
    hotkey: str
    job_id: str
    result: str
    first_bad_leaf: int | None
    recomputed_leaves_root: str
    completed: bool
    complete_detail: str | None = None


@dataclass(frozen=True)
class Settlement:
    w: int
    steps: tuple[Step, ...]
    verdicts: tuple[AuditVerdict, ...]
    included: tuple[str, ...]
    finalize_hash: str | None
    seconds: float

    def step(self, name: str) -> Step | None:
        return next((s for s in self.steps if s.name == name), None)

    @property
    def stalled_at(self) -> str | None:
        """Name of the first step that did not succeed (an empty lease queue is not a stall)."""
        return next((s.name for s in self.steps if not s.ok and s.status != 204), None)

    @property
    def results(self) -> dict[str, str]:
        return {v.hotkey: v.result for v in self.verdicts}

    @property
    def audits_done(self) -> bool:
        """The audits were scheduled and every lease, execute and complete call went through."""
        audits = self.step("audits")
        return audits is not None and audits.ok and all(s.ok or s.status == 204 for s in self.steps)

    @property
    def settled(self) -> bool:
        finalize = self.step("finalize")
        return finalize is not None and finalize.ok and finalize.body.get("status") == "SETTLED"


@dataclass(frozen=True)
class Weights:
    status_code: int
    epoch: int
    weights: dict[str, float]
    metadata: dict[str, Any]


@dataclass(frozen=True)
class Observation:
    """Plain snapshot of everything a scenario asserts on."""

    admissions: dict[str, dict[str, Any]]
    records: dict[str, dict[str, Any]]
    leases: list[dict[str, Any]]
    escrow: dict[str, Any]
    escrow_by_owner: dict[str, dict[str, Any]]
    weights: Weights | None

    @property
    def excluded(self) -> dict[str, Any]:
        return self.records.get("excluded", {})

    @property
    def finality(self) -> dict[str, Any]:
        return self.records.get("finality", {})


class CheatLike(Protocol):
    """The part of ac_cheats.Cheat that the world applies to one miner."""

    cheat: int
    name: str

    def launch(self, real: Callable[..., Any], ctx: Any) -> Callable[..., Any]: ...

    def wire(self, miner: Any, ctx: Any) -> None: ...


class _Context:
    """ac_cheats.CheatContext for one miner running one round."""

    def __init__(self, world: World, w: int, hotkey: str) -> None:
        self._world, self.w, self.hotkey = world, w, hotkey
        self.rows = world.network.setup.rows
        self.tree = world.network.setup.tree

    def peer_artifacts(self) -> Any:
        for (w, hotkey), artifacts in self._world.published.items():
            if w == self.w and hotkey != self.hotkey:
                return artifacts
        raise LookupError("no honest peer of this round has UPLOADED yet; run one first")

    def own_previous_artifacts(self) -> Any | None:
        return self._world.captured.get((self.w - 1, self.hotkey))

    def advance_beacon(self, number: int) -> None:
        self._world.advance_beacon(number)


def _is_withdrawn(error: BaseException) -> bool:
    cheats = sys.modules.get("ac_cheats")
    return cheats is not None and isinstance(error, cheats.Withdrawn)


class World:
    """A network fixture with graduated identities, an in-process relay and a CPU miner driver.

    `with world:` installs the process-wide pieces a round needs (a clock that starts at the
    manifest genesis, the relay as the global httpx.AsyncClient transport); leave it to restore
    them. Two worlds must not be installed at once. `close()` stops the app's threads.
    """

    def __init__(
        self,
        svc: Any,
        network: Any,
        generator: Any,
        scratch: Path,
        shape: Shape,
        device: str = "cpu",
    ) -> None:
        self.svc, self.network, self.shape, self.device = svc, network, shape, device
        self.scratch = scratch
        self.scratch.mkdir(parents=True, exist_ok=True)
        self.restore_seconds: float | None = None
        self.captured: dict[tuple[int, str], Any] = {}
        self.published: dict[tuple[int, str], Any] = {}
        self._generator = generator
        self._stack = contextlib.ExitStack()
        self._patch: pytest.MonkeyPatch | None = None
        self._leases = 0
        self._weights: Weights | None = None

    @property
    def run_id(self) -> str:
        return str(self.network.manifest.run_id())

    def hotkey(self, index: int) -> str:
        return str(self.svc.HOT[index].ss58)

    def advance_beacon(self, number: int) -> None:
        if number <= self.network.now:
            raise ValueError(f"beacons must increase: {number} <= {self.network.now}")
        self.network.push(number)

    def __enter__(self) -> World:
        self.install()
        return self

    def __exit__(self, *exc: object) -> None:
        self.uninstall()

    def install(self) -> None:
        if self._patch is not None:
            return
        patch = pytest.MonkeyPatch()
        try:
            genesis = self.network.manifest.training.beacon.genesis_time
            offset = _REAL_TIME() - (genesis + 1)
            patch.setattr(time, "time", lambda: _REAL_TIME() - offset)
            # The torchrun worker checks job.deadline against its own wall clock.
            clock = self.scratch / "clock"
            clock.mkdir(exist_ok=True)
            (clock / "sitecustomize.py").write_text(
                "import os,time\n_real=time.time\n"
                "_offset=float(os.environ['HT_FIXTURE_CLOCK_OFFSET'])\n"
                "time.time=lambda:_real()-_offset\n"
            )
            patch.setenv("HT_FIXTURE_CLOCK_OFFSET", str(offset))
            # No empty entry: an empty PYTHONPATH item puts the child's cwd on its sys.path.
            inherited = os.environ.get("PYTHONPATH")
            patch.setenv("PYTHONPATH", os.pathsep.join(p for p in (str(clock), inherited) if p))
            # The relay app must exist before httpx.AsyncClient is replaced: its own backing
            # client is a real one.
            app = smoke._relay_app(self.svc, self.network, self._stack)
            real = httpx.AsyncClient
            patch.setattr(
                httpx,
                "AsyncClient",
                lambda *a, **kw: real(*a, **{**kw, "transport": httpx.ASGITransport(app=app)}),
            )
        except BaseException:
            patch.undo()
            raise
        self._patch = patch

    def uninstall(self) -> None:
        if self._patch is not None:
            self._patch.undo()
            self._patch = None

    def close(self) -> None:
        self.uninstall()
        try:
            self._stack.close()
        finally:
            self._generator.close()

    # -- rounds -----------------------------------------------------------------------------

    def open_round(
        self, w: int = 0, roster: Sequence[int] | None = None, prev_final: str | None = None
    ) -> RoundOpenV2:
        """Sign and open round `w` for the identities in `roster` (default: all graduated).

        Round 0 starts from genesis anchors; w >= 1 needs round w-1 aggregated (and finalized when
        prev_final is not given), and every rostered miner must hold its w-1 anchor, which a
        miner excluded in w-1 does not. A refusal raises RuntimeError with the master's answer.
        """
        indices = list(range(self.shape.graduated) if roster is None else roster)
        view = SimpleNamespace(
            HOT=[self.svc.HOT[i] for i in indices], COORD=self.svc.COORD, admin=self.svc.admin
        )
        if w == 0:
            smoke._open_round(view, self.network)
        else:
            previous = prev_final or self.finalize_hash(w - 1)
            multi_round.open_round(view, self.network, w, previous)
        return self.opening(w)

    def opening(self, w: int) -> RoundOpenV2:
        body = self.network.store._record_v2(self.run_id, "round", str(w))["body"]
        return RoundOpenV2.model_validate(body)

    def finalize_hash(self, w: int) -> str:
        body = self.network.store._record_v2(self.run_id, "finalize", str(w))["body"]
        return sha256_hex(canonicalize(body))

    def miner(self, index: int) -> NetworkMiner:
        """A configured NetworkMiner for identity `index`, talking to this world's master."""
        keys = self.scratch / "keys"
        keys.mkdir(exist_ok=True)
        keyfile = keys / f"miner-{index}.key"
        keyfile.write_bytes(bytes([80 + index]) * 32)
        keyfile.chmod(0o600)
        n = self.network
        config = MinerConfig(
            api="http://testserver",
            keyfile=keyfile,
            workdir=self.scratch / "work",
            state_source="cpu",
            image_digest=n.manifest.training.reference_spec.image_digest,
            run_id=self.run_id,
            owner_hotkey=self.svc.OWNER.ss58,
            device=self.device,
        )
        return NetworkMiner(config, n.client)

    _miner = miner  # the name other files of the suite still use

    def run_miner(self, index: int, cheat: CheatLike | None = None, w: int = 0) -> MinerOutcome:
        """Run miner `index` through round `w`; never raises on a refusal or a withdrawal."""
        from hypertrain.miner import island_launch

        hotkey = self.hotkey(index)
        label = "honest" if cheat is None else cheat.name
        ctx = _Context(self, w, hotkey)
        real = island_launch.launch_island

        def recording(*args: Any, **kwargs: Any) -> Any:
            artifacts = real(*args, **kwargs)
            self.captured[(w, hotkey)] = artifacts
            return artifacts

        seen: list[tuple[str, str]] = []
        started = time.monotonic()
        patch = pytest.MonkeyPatch()
        launch = recording if cheat is None else cheat.launch(recording, ctx)
        patch.setattr(island_launch, "launch_island", launch)
        try:
            miner = self.miner(index)
            if cheat is not None:
                cheat.wire(miner, ctx)
            api_call = miner.api.call

            def tracked(method: str, path: str, **kw: Any) -> Any:
                seen.append((method, path.split("?")[0].removeprefix(miner.url) or "/"))
                return api_call(method, path, **kw)

            miner.api.call = tracked  # type: ignore[method-assign]
            miner.run_round(w)
            fields: dict[str, Any] = {"status": "UPLOADED"}
        except ChallengeHTTPError as error:
            method, path = seen[-1] if seen else ("", "")
            fields = {
                "status": "refused",
                "http_status": error.status,
                "detail": error.detail,
                "endpoint": f"{method} {path}",
            }
        except Exception as error:  # noqa: BLE001 - a cheat may fail anywhere; record, never hide
            if _is_withdrawn(error):
                fields = {"status": "withdrawn", "stage": str(getattr(error, "stage", ""))}
            else:
                fields = {"status": "error", "error": f"{type(error).__name__}: {error}"}
        finally:
            patch.undo()
        if fields["status"] == "UPLOADED" and (cheat is None or cheat.cheat == 0):
            self.published[(w, hotkey)] = self.captured[(w, hotkey)]
        return MinerOutcome(index, hotkey, label, seconds=time.monotonic() - started, **fields)

    def run_roster(self, cheats: Mapping[int, CheatLike | None], w: int = 0) -> list[MinerOutcome]:
        """Run the listed miners one after the other, in mapping order (peers before copiers)."""
        return [self.run_miner(i, cheat, w) for i, cheat in cheats.items()]

    # -- settlement -------------------------------------------------------------------------

    def _step(self, name: str, call: Callable[[], Any], subject: str | None = None) -> Step:
        try:
            response = call()
        except Exception as error:  # noqa: BLE001 - an uncaught server error is data here
            return Step(name, 500, str(error), None, subject, type(error).__name__)
        try:
            body = response.json()
        except ValueError:
            body = None
        detail = None
        if response.status_code >= 400:
            detail = str(body.get("detail", body)) if isinstance(body, dict) else response.text
        return Step(name, response.status_code, detail, body, subject)

    def settle(self, w: int = 0) -> Settlement:
        """Push d_audit, schedule and execute every audit, aggregate and finalize round `w`.

        Every step's answer is recorded; the first failing gate (audits, aggregate) ends the run.
        A scenario that must act between the audits and the aggregate calls `audit_round` and
        then `finish_round` itself.
        """
        audited = self.audit_round(w)
        if audited.audits_done:
            return self.finish_round(audited)
        return audited

    def audit_round(self, w: int = 0) -> Settlement:
        """First half of settle(): push d_audit, schedule and execute every audit of round `w`.

        The Settlement it returns has no aggregate or finalize step yet; `audits_done` tells
        whether the audits all went through.
        """
        started = time.monotonic()
        n, svc = self.network, self.svc
        opening = self.opening(w)
        if opening.d_audit > n.now:
            n.push(opening.d_audit)
        steps: list[Step] = []
        verdicts: list[AuditVerdict] = []

        def post(path: str, body: Any = None, admin: bool = False) -> Callable[[], Any]:
            return lambda: n.client.post(
                n.url + path, json=body, headers=svc.admin() if admin else None
            )

        audits = self._step("audits", post(f"/admin/rounds/{w}/audits", admin=True))
        steps.append(audits)
        if audits.ok:
            for k in range(len(audits.body["jobs"])):
                auditor = svc.AUDITORS[k % 2]
                self._leases += 1

                def receipt(nonce: str, auditor: Any = auditor) -> Any:
                    body = {"w": w, "commit_hash": nonce, "received_round": n.now}
                    return n.signed(auditor, "Receipt", body)

                nonce = sha256_hex(f"audit-cheaters-lease|{self._leases}".encode())
                lease = self._step("lease", post("/worker/lease", receipt(nonce)))
                steps.append(lease)
                if lease.status != 200:
                    break
                job = lease.body["body"]
                path = "/worker/jobs/" + job["job_id"]
                execute = self._step(
                    "execute", post(path + "/execute", receipt(job["lease_nonce"])), job["job_id"]
                )
                steps.append(execute)
                if not execute.ok:
                    break
                verdict = execute.body["verdict"]
                hotkey = execute.body["identity"].split(":", 1)[1]
                complete = self._step(
                    "complete",
                    post(path + "/complete", n.signed(auditor, "ReplayVerdict", verdict)),
                    hotkey,
                )
                steps.append(complete)
                verdicts.append(
                    AuditVerdict(
                        hotkey,
                        job["job_id"],
                        verdict["result"],
                        verdict["first_bad_leaf"],
                        verdict["recomputed_leaves_root"],
                        complete.ok,
                        complete.detail,
                    )
                )
                if not complete.ok:
                    break
        return Settlement(
            w,
            tuple(steps),
            tuple(verdicts),
            self._included(opening),
            None,
            time.monotonic() - started,
        )

    def finish_round(self, audited: Settlement) -> Settlement:
        """Second half of settle(): aggregate and finalize the round of `audited` through the
        admin routes, without scheduling audits again.

        It aggregates whatever the audit steps answered, so a scenario can finish a round whose
        audits the master refused to schedule (an absence that only becomes certain later).
        """
        started = time.monotonic()
        n, svc, w = self.network, self.svc, audited.w
        steps = list(audited.steps)

        def post(path: str, body: Any = None) -> Step:
            return self._step(
                path.rsplit("/", 1)[1],
                lambda: n.client.post(n.url + path, json=body, headers=svc.admin()),
            )

        aggregate = post(f"/admin/rounds/{w}/aggregate")
        steps.append(aggregate)
        # Read the exclusions after the aggregate, which is what finalize must agree with.
        included = self._included(self.opening(w))
        finalize_hash = None
        if aggregate.ok:
            final = Finalize(
                w=w,
                final_theta_hash_w1=aggregate.body["theta_hash"],
                included=list(included),
                entitlements_root="0" * 64,
            )
            finalize = post(f"/admin/rounds/{w}/finalize", n.signed(svc.COORD, "Finalize", final))
            steps.append(finalize)
            if finalize.ok and finalize.body.get("status") == "SETTLED":
                finalize_hash = self.finalize_hash(w)
        return Settlement(
            w,
            tuple(steps),
            audited.verdicts,
            included,
            finalize_hash,
            audited.seconds + time.monotonic() - started,
        )

    def _included(self, opening: RoundOpenV2) -> tuple[str, ...]:
        """Roster members of `opening` without an `excluded` record, sorted."""
        excluded = {
            row[0]
            for row in self.network.store._db.execute(
                "SELECT id FROM records_v2 WHERE run_id=? AND kind='excluded'", (self.run_id,)
            )
        }
        return tuple(
            sorted(e.hotkey for e in opening.roster if f"{opening.w}:{e.hotkey}" not in excluded)
        )

    def ask_weights(self, epoch: int, epoch_at: int) -> httpx.Response:
        """GET /internal/v1/get_weights for `epoch` as of unix time `epoch_at`; the raw answer."""
        n = self.network
        route = next(
            r for r in n.client.app.routes if getattr(r, "path", "") == "/internal/v1/get_weights"
        )
        code = route.endpoint.__code__
        config = route.endpoint.__closure__[code.co_freevars.index("config")].cell_contents
        # No production hook sets the internal token file in-process: point the closure at a
        # throwaway random token, as scripts/mine_multi_round_v2.py does.
        token = os.urandom(16).hex()
        token_file = self.scratch / "internal"
        token_file.write_text(token)
        token_file.chmod(0o600)
        config.internal_token_file = token_file
        return n.client.get(
            "/internal/v1/get_weights",
            params={"epoch": epoch, "epoch_at": epoch_at},
            headers={"Authorization": f"Bearer {token}", "x-platform-challenge-slug": config.slug},
        )

    def weights(self, mature: bool = True) -> Weights:
        """GET /internal/v1/get_weights through the app, after the rewards of finished rounds
        matured. An epoch's answer is final, so ask once per scenario."""
        n = self.network
        if mature:
            latest = n.store._db.execute(
                "SELECT MAX(mature_at) FROM escrow_origins "
                "WHERE reward_round IS NOT NULL AND issued>0"
            ).fetchone()[0]
            if latest is not None:
                n.push(max(n.now, latest) + 1)
        epoch_at = int(n.store.clock())
        epoch = (epoch_at - n.store.params.genesis_unix) // n.store.params.epoch_seconds
        response = self.ask_weights(epoch, epoch_at)
        answer = response.json()
        self._weights = Weights(
            response.status_code,
            epoch,
            dict(answer.get("weights", {})) if isinstance(answer, dict) else {},
            dict(answer.get("metadata", {})) if isinstance(answer, dict) else {},
        )
        return self._weights

    def observe(self) -> Observation:
        n, rid = self.network, self.run_id
        db = n.store._db
        admissions = {
            r["hotkey"]: {
                k: r[k] for k in ("state", "strikes", "pending_dispute", "reason", "clean_count")
            }
            for r in db.execute(
                "SELECT hotkey,state,strikes,pending_dispute,reason,clean_count "
                "FROM admissions_v2 WHERE run_id=?",
                (rid,),
            )
        }
        records: dict[str, dict[str, Any]] = {kind: {} for kind in EVIDENCE_KINDS}
        for r in db.execute(
            f"SELECT kind,id,data FROM records_v2 WHERE run_id=? AND kind IN "
            f"({','.join('?' * len(EVIDENCE_KINDS))}) ORDER BY id",
            (rid, *EVIDENCE_KINDS),
        ):
            records[r["kind"]][r["id"]] = json.loads(r["data"])
        leases = [
            dict(r)
            for r in db.execute(
                "SELECT id,hotkey,w,state,result,attempts FROM audit_leases_v2 "
                "WHERE run_id=? ORDER BY w,hotkey",
                (rid,),
            )
        ]
        escrow = n.store._services(rid)[0]
        total = escrow.balances()
        by_owner = {cold.ss58: {**asdict(escrow.balances(cold.ss58))} for cold in self.svc.COLD}
        return Observation(
            admissions,
            records,
            leases,
            {**asdict(total), "conserved": total.conserved()},
            by_owner,
            self._weights,
        )


@dataclass
class GraduatedSnapshot:
    """An on-disk graduated network. `origin` is the live World that graduated it (None when the
    snapshot was found on disk): the 'fresh network' to compare a restored clone against."""

    root: Path
    shape: Shape
    meta: dict[str, Any]
    origin: World | None = None
    device: str = "cpu"

    def clone(self, scratch: Path) -> World:
        """A new World restored from this snapshot, with the snapshot's run_id and clock origin."""
        started = time.monotonic()
        require_cpu(self.device)
        svc = load_service(self.shape.identities)
        with _frozen_time(self.meta["genesis_time"] + 1.5):
            generator, n = _build_network(svc, scratch / "net")
        world = World(svc, n, generator, scratch / "run", self.shape, self.device)
        try:
            if n.manifest.run_id() != self.meta["run_id"]:
                raise RuntimeError(
                    f"snapshot {self.root} was built for another manifest: delete it"
                )
            n.store._services_v2.clear()
            with sqlite3.connect(self.root / "challenge.db") as source:
                source.backup(n.store._db)
            for name in STATE_DIRS:
                if (self.root / name).is_dir():
                    shutil.copytree(self.root / name, n.store.state_dir / name, dirs_exist_ok=True)
            n.now = n.store._now(n.store._db)
            n.push(n.now)
        except BaseException:
            world.close()
            raise
        world.restore_seconds = time.monotonic() - started
        return world

    def close(self) -> None:
        if self.origin is not None:
            self.origin.close()
            self.origin = None


def ensure_snapshot(
    root: Path, shape: Shape, scratch: Path, device: str = "cpu"
) -> GraduatedSnapshot:
    """Load `root/<shape>` if complete, else graduate the identities in-process and write it."""
    require_cpu(device)
    target = root / shape.key
    meta_file = target / "meta.json"
    if meta_file.is_file():
        meta = json.loads(meta_file.read_text())
        if meta["format"] != SNAPSHOT_FORMAT:
            raise RuntimeError(f"{target}: snapshot format {meta['format']}, delete it")
        return GraduatedSnapshot(target, shape, meta, None, device)
    svc = load_service(shape.identities)
    generator, n = _build_network(svc, scratch / "origin-net")
    origin = World(svc, n, generator, scratch / "origin-run", shape, device)
    partial = root / f".{shape.key}.partial-{os.getpid()}"
    try:
        graduate_seconds = []
        for i in range(shape.graduated):
            started = time.monotonic()
            smoke._graduate(svc, n, i)
            graduate_seconds.append(time.monotonic() - started)
        started = time.monotonic()
        root.mkdir(parents=True, exist_ok=True)
        shutil.rmtree(partial, ignore_errors=True)
        partial.mkdir()
        with sqlite3.connect(partial / "challenge.db") as copy:
            n.store._db.backup(copy)
        for name in STATE_DIRS:
            if (n.store.state_dir / name).is_dir():
                shutil.copytree(n.store.state_dir / name, partial / name)
        meta = {
            "format": SNAPSHOT_FORMAT,
            "shape": asdict(shape),
            "run_id": n.manifest.run_id(),
            "genesis_time": n.manifest.training.beacon.genesis_time,
            "now": n.now,
            "graduate_seconds": graduate_seconds,
            "snapshot_seconds": time.monotonic() - started,
        }
        (partial / "meta.json").write_text(json.dumps(meta, sort_keys=True))
        try:
            os.rename(partial, target)
        except OSError:
            if not meta_file.is_file():
                raise
            # A parallel process published this shape first. Each fixture network stamps its own
            # genesis, hence its own run_id, so the winner's snapshot is not ours: use its meta
            # and drop our live network, which no longer matches what is on disk.
            origin.close()
            return GraduatedSnapshot(target, shape, json.loads(meta_file.read_text()), None, device)
    except BaseException:
        origin.close()
        raise
    finally:
        shutil.rmtree(partial, ignore_errors=True)
    return GraduatedSnapshot(target, shape, meta, origin, device)
