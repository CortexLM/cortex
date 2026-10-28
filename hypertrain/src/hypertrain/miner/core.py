"""Miner client core: keyfile, config, hardware check, journal-resumable round loop.

Round w (docs/challenge-routes.md): wait d_assign -> Accept -> train (trainer.loop.train_round
from the public start state, verified against RoundOpen.theta_hash) -> Commit -> leaves ->
presigned delta upload + DeltaManifest. Every step is journaled (fsynced JSONL) so a restarted
miner never re-commits; artifacts stay on disk for the vesting window.
Nothing fetched from the network is executed: state blobs are safetensors, verified by TH.
"""

from __future__ import annotations

import hashlib
import json
import logging
import os
import random
import shutil
import stat
import tempfile
import threading
import time
import tomllib
from collections.abc import Callable, Mapping
from dataclasses import dataclass, fields, replace
from pathlib import Path
from typing import TYPE_CHECKING, Any, Literal
from urllib.parse import urlsplit

import httpx

if TYPE_CHECKING:
    from hypertrain.gpu_ops.work_screen import IslandLaunch
    from hypertrain.miner.island_launch import IslandArtifacts
    from hypertrain.protocol.messages_v2 import ArtifactRef, IslandJobV1

import hypertrain.trainer  # noqa: F401  (determinism setup before torch)
from hypertrain.auditor.bisect import Executor
from hypertrain.auditor.replay import challenge_hash, pack_state, tensor_root, unpack_state
from hypertrain.challenge.store import assignment_hash, rerun_message
from hypertrain.gpu_ops.journal import Journal
from hypertrain.protocol.envelope import body_digest, seal, verify_envelope
from hypertrain.protocol.hashing import MerkleTree, sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages import AuditChallenge, LeafPreimage, RoundOpen, RunManifest
from hypertrain.trainer.compress import state_hash
from hypertrain.trainer.config import TrainConfig
from hypertrain.trainer.loop import Assignment, Params, SampleFn, train_round
from hypertrain.trainer.model import init_params
from hypertrain.trainer.optim import OptState, init_state

log = logging.getLogger("hypertrain.miner")
EXP_ROUNDS = 2**40  # envelope expiry far beyond any round; replay protection is per (w, type)
# Backoff between attempts of an idempotent request (object PUT/GET, API GET) that hit a
# transport error or a 5xx; the last failure propagates. ~31 s nominal per request, each wait
# jittered by +-50% (up to ~46 s).
RETRY_DELAYS: tuple[float, ...] = (1, 2, 4, 8, 16)
# An object GET also retries a 404: the digest was just listed by the server, so a miss is more
# likely a lost read than an answer (s100 TEST replay, 2026-10-27, RELAY-02; cause not established).
OBJECT_MISS = frozenset({404})
OWNER_REQUEST_ROUNDS = 20  # a signed owner request lives about a minute at 3 s rounds
FETCH_WORKERS = 16  # concurrent 1 MiB part GETs per job input (bounded window, ordered sink)
UPLOAD_WORKERS = 16  # concurrent 1 MiB part PUTs per artifact (bounded window, index last)


def _retrying[T](
    what: str,
    fn: Callable[[], T],
    *,
    also: frozenset[int] = frozenset(),
    noted: Callable[[str, int | None, int, float], None] | None = None,
) -> T:
    """Run an idempotent request, retrying transport errors, 5xx responses and the `also` statuses.

    `noted(what, status, attempt, delay)` sees each retry before its wait; status is None for a
    transport error. It may fail without effect on the request."""
    for attempt, delay in enumerate((*RETRY_DELAYS, None), 1):
        try:
            return fn()
        except (httpx.TransportError, httpx.HTTPStatusError, ChallengeHTTPError) as error:
            status = getattr(getattr(error, "response", None), "status_code", None)
            status = getattr(error, "status", status)
            if delay is None or (status is not None and status < 500 and status not in also):
                raise
            delay *= random.uniform(0.5, 1.5)
            # Type + status only: request URLs can carry the proxy token.
            log.warning(
                "%s failed (%s %s); retry in %.1fs", what, type(error).__name__, status, delay
            )
            if noted is not None:
                # A diagnostic line must not decide the request: if it cannot be written, say so
                # and let the request go on as the HTTP error alone dictates.
                try:
                    noted(what, status, attempt, delay)
                except Exception as failure:
                    log.warning(
                        "%s: retry not journaled (%s: %s)", what, type(failure).__name__, failure
                    )
            time.sleep(delay)
    raise AssertionError("unreachable")


def _upload_ref(path: Path, put: Callable[[bytes], str]) -> ArtifactRef:
    """artifact_ref(path, put) with UPLOAD_WORKERS concurrent part PUTs (index still last)."""
    from hypertrain.data.store import put_file
    from hypertrain.protocol.messages_v2 import ArtifactRef

    last = b""

    def record(data: bytes) -> str:
        nonlocal last
        last = data  # the index (or plain object) is put after every part has completed
        return put(data)

    sha = put_file(path, record, UPLOAD_WORKERS)
    return ArtifactRef(sha256=sha, size=len(last))


class MinerError(RuntimeError):
    pass


def _st_header(f: Any) -> tuple[int, dict[str, Any]]:
    """safetensors header: (data start offset, {name: {dtype, shape, data_offsets}})."""
    n = int.from_bytes(f.read(8), "little")
    if not 0 < n <= 100 << 20:
        raise ValueError("bad safetensors header length")
    header = json.loads(f.read(n))
    header.pop("__metadata__", None)
    return 8 + n, header


def _overwrite_theta(state: Path, theta: Path, chunk: int = 16 << 20) -> bool:
    """Overwrite state's theta/* tensors in place with theta's, chunk by chunk.

    False when the theta file's tensors (names/dtypes/shapes) differ from state's theta/*."""
    with state.open("r+b") as dst, theta.open("rb") as src:
        dst_base, dst_header = _st_header(dst)
        src_base, src_header = _st_header(src)
        if set(src_header) != {k for k in dst_header if k.startswith("theta/")}:
            return False
        for key, entry in src_header.items():
            target = dst_header[key]
            if (entry["dtype"], entry["shape"]) != (target["dtype"], target["shape"]):
                return False
            (a, b), (c, d) = entry["data_offsets"], target["data_offsets"]
            if b - a != d - c:
                return False
            src.seek(src_base + a)
            dst.seek(dst_base + c)
            left = b - a
            while left:
                data = src.read(min(chunk, left))
                if not data:
                    raise ValueError("theta file truncated")
                dst.write(data)
                left -= len(data)
        dst.flush()
        os.fsync(dst.fileno())
    return True


class KeyfileError(MinerError):
    pass


class StartStateMismatch(MinerError):
    """The public start state does not hash to RoundOpen.theta_hash: the round is aborted."""


class HardwareMismatch(MinerError):
    pass


class ChallengeHTTPError(MinerError):
    def __init__(self, status: int, detail: str) -> None:
        super().__init__(f"{status}: {detail}")
        self.status, self.detail = status, detail


def load_keyfile(path: Path) -> Keypair:
    """sr25519 seed file (32 raw bytes or 64 hex); must be a regular 0600 file owned by us."""
    st = os.stat(path, follow_symlinks=False)
    if not stat.S_ISREG(st.st_mode):
        raise KeyfileError(f"{path}: keyfile must be a regular file")
    if st.st_mode & 0o077 or st.st_uid != os.getuid():
        raise KeyfileError(f"{path}: keyfile must be mode 0600 and owned by uid {os.getuid()}")
    raw = path.read_bytes().strip()
    try:
        return Keypair(bytes.fromhex(raw.decode()) if len(raw) == 64 else raw)
    except ValueError:
        raise KeyfileError(f"{path}: expected a 32-byte seed (raw or 64 hex)") from None


@dataclass(frozen=True)
class MinerConfig:
    api: str
    keyfile: Path
    workdir: Path
    state_source: str
    image_digest: str
    data_dir: Path | None = None
    run_id: str | None = None
    owner_hotkey: str | None = None
    device: str = "cpu"
    poll_seconds: float = 2.0
    chunk_bytes: int = 64 * 1024 * 1024
    allow_file_upload: bool = False

    @classmethod
    def load(cls, path: Path | None, env: Mapping[str, str] = os.environ) -> MinerConfig:
        """TOML file, then HYPERTRAIN_MINER_<FIELD> env overrides. No other sources."""
        raw: dict[str, Any] = tomllib.loads(path.read_text()) if path else {}
        names = {f.name: f for f in fields(cls)}
        unknown = set(raw) - set(names)
        if unknown:
            raise MinerError(f"unknown config keys {sorted(unknown)}")
        for name in names:
            value = env.get(f"HYPERTRAIN_MINER_{name.upper()}")
            if value is not None:
                raw[name] = value
        base = path.parent if path else Path.cwd()
        out: dict[str, Any] = {}
        for name, value in raw.items():
            kind = str(names[name].type)
            if "Path" in kind:
                out[name] = (base / str(value)).resolve()
            elif kind.startswith("float"):
                out[name] = float(value)
            elif kind.startswith("int"):
                out[name] = int(value)
            elif kind.startswith("bool"):
                out[name] = value if isinstance(value, bool) else str(value).lower() == "true"
            else:
                out[name] = str(value)
        if "state_source" in out and "://" not in out["state_source"]:
            out["state_source"] = str((base / out["state_source"]).resolve())
        try:
            return cls(**out)
        except TypeError as error:
            raise MinerError(f"incomplete miner config: {error}") from None


@dataclass(frozen=True)
class Gpu:
    index: int
    name: str
    sm_count: int
    uuid: str


@dataclass(frozen=True)
class Hardware:
    device: str
    name: str
    sm_count: int
    driver: str
    n_gpus: int
    gpus: tuple[Gpu, ...] = ()


def detect_gpus() -> tuple[Gpu, ...]:
    """Every GPU visible to this process (honours CUDA_VISIBLE_DEVICES), in CUDA order."""
    import torch

    from hypertrain.miner.island_launch import gpu_uuid

    if not torch.cuda.is_available():
        return ()
    out = []
    for i in range(torch.cuda.device_count()):
        p = torch.cuda.get_device_properties(i)
        out.append(Gpu(i, p.name, p.multi_processor_count, gpu_uuid(p.uuid)))
    return tuple(out)


def select_gpus(gpus: tuple[Gpu, ...], n_gpus: int, sm_count: int) -> tuple[str, ...]:
    """UUIDs of the first n_gpus devices; refuse short, mixed-SM or duplicate-identity hosts."""
    if len(gpus) < n_gpus:
        raise HardwareMismatch(f"{len(gpus)} visible GPU(s) < manifest layout.n_gpus {n_gpus}")
    sms = sorted({g.sm_count for g in gpus})
    if len(sms) != 1:
        raise HardwareMismatch(f"heterogeneous SM counts {sms} across visible GPUs")
    if sms[0] != sm_count:
        raise HardwareMismatch(f"SM count {sms[0]} != reference {sm_count}")
    if len({g.uuid for g in gpus}) != len(gpus):
        raise HardwareMismatch("duplicate GPU UUIDs reported")
    return tuple(g.uuid for g in gpus[:n_gpus])


def _driver() -> str:
    version = Path("/proc/driver/nvidia/version")
    if not version.is_file():
        return "unknown"
    parts = version.read_text().split()
    return next((p for p in parts if p[:1].isdigit() and "." in p), "unknown")


def self_check(device: str, manifest: RunManifest) -> Hardware:
    """Device name, SM count and driver vs reference_spec (sm_count, driver_allowlist)."""
    ref = manifest.reference_spec
    if ref.layout.n_gpus != 1:
        # v1 Accept/Commit/audit replay one rank (loop.train_round, auditor Executor) over a
        # single-rank assignment; a multi-rank island commitment is not replayable there.
        raise HardwareMismatch(
            f"v1 `hypertrain-miner run` trains single-rank layouts only (manifest "
            f"layout.n_gpus={ref.layout.n_gpus}); use `hypertrain-miner run-v2` for multi-GPU"
        )
    if device == "cpu":
        hw = Hardware("cpu", "cpu", 0, "cpu", 1)
    else:
        gpus = detect_gpus()
        if not gpus:
            raise HardwareMismatch("device cuda requested but CUDA is unavailable")
        select_gpus(gpus, 1, ref.sm_count)
        hw = Hardware("cuda", gpus[0].name, gpus[0].sm_count, _driver(), 1, gpus)
    if ref.driver_allowlist and hw.driver not in ref.driver_allowlist:
        raise HardwareMismatch(f"driver {hw.driver} not in the manifest allowlist")
    return hw


class Api:
    def __init__(self, client: httpx.Client, base: str) -> None:
        self.c, self.base = client, base.rstrip("/")

    def call(self, method: str, path: str, **kw: Any) -> Any:
        if method == "GET":  # reads are idempotent; signed POSTs are never replayed here
            return _retrying("GET " + path, lambda: self._call(method, path, **kw))
        return self._call(method, path, **kw)

    def _call(self, method: str, path: str, **kw: Any) -> Any:
        r = self.c.request(method, self.base + path, timeout=120, **kw)
        if r.status_code >= 400:
            try:
                detail = str(r.json().get("detail", r.text))
            except ValueError:
                detail = r.text[:300]
            raise ChallengeHTTPError(r.status_code, detail)
        return r.json()


WaitFn = Callable[[int], None]
BlobSink = Callable[[bytes], str]


class Miner:
    def __init__(
        self,
        cfg: MinerConfig,
        client: httpx.Client,
        get_sample: SampleFn,
        wait: WaitFn | None = None,
        blob_sink: BlobSink | None = None,
        upload_client: httpx.Client | None = None,
    ) -> None:
        self.cfg = cfg
        self.kp = load_keyfile(cfg.keyfile)
        self.api = Api(client, cfg.api)
        self.get_sample = get_sample
        self.wait: WaitFn = wait or (lambda _target: time.sleep(cfg.poll_seconds))
        self.blob_sink = blob_sink
        self.http = upload_client or httpx.Client(timeout=600)
        status = self._run_status()
        self.run_id: str = status["run_id"]
        self.manifest = self._verified_manifest(status["manifest_envelope"])
        self.train_cfg = TrainConfig.from_manifest(self.manifest.body())
        self.journal = Journal(self._dir())
        self.hw = self_check(cfg.device, self.manifest)
        log.info("miner %s run %s hardware %s", self.kp.ss58, self.run_id, self.hw)

    def _run_status(self) -> dict[str, Any]:
        run_id = self.cfg.run_id
        if run_id is None:
            runs = self.api.call("GET", "/v1/runs")["runs"]
            active = [r["run_id"] for r in runs if r["status"] != "finished"]
            if len(active) != 1:
                raise MinerError(f"set run_id: {len(active)} candidate runs")
            run_id = active[0]
        out: dict[str, Any] = self.api.call("GET", f"/v1/runs/{run_id}")
        return out

    def _verified_manifest(self, env: Mapping[str, Any]) -> RunManifest:
        m = RunManifest.model_validate(env["body"])
        if not verify_envelope(env) or m.run_id() != self.run_id or env["run_id"] != self.run_id:
            raise MinerError("run manifest envelope does not verify for this run_id")
        if self.cfg.owner_hotkey and env["signer"] != self.cfg.owner_hotkey:
            raise MinerError("run manifest is not signed by the pinned owner hotkey")
        return m

    def _dir(self, *parts: str) -> Path:
        d = self.cfg.workdir / self.run_id[:16] / self.kp.ss58 / Path(*parts)
        d.mkdir(parents=True, exist_ok=True, mode=0o700)
        return d

    def view(self, w: int) -> dict[str, Any]:
        out: dict[str, Any] = self.api.call("GET", f"/v1/runs/{self.run_id}/rounds/{w}")
        return out

    def round_open(self, view: Mapping[str, Any]) -> RoundOpen:
        env = view["round_open"]
        ok = verify_envelope(env) and env["signer"] == self.manifest.coord_pubkey
        if not ok or env["run_id"] != self.run_id or env["type"] != "RoundOpen":
            raise MinerError("RoundOpen is not signed by the manifest coordinator")
        return RoundOpen.model_validate(env["body"])

    def wait_round(self, w: int, field: str) -> dict[str, Any]:
        while True:
            v = self.view(w)
            target = getattr(self.round_open(v), field)
            if v["now_round"] >= target:
                return v
            self.wait(target)

    def wait_for_round(self, w: int) -> None:
        while True:
            try:
                self.view(w)
                return
            except ChallengeHTTPError as e:
                if e.status != 404:
                    raise
            self.wait(self._run_status()["now_round"] + 1)

    def _signed(self, msg_type: str, body: Mapping[str, Any]) -> dict[str, Any]:
        return seal(self.kp, msg_type, self.run_id, dict(body), EXP_ROUNDS)

    def _post(self, path: str, body: Any) -> Any:
        return self.api.call("POST", f"/v1/runs/{self.run_id}/{path}", json=body)

    def _my(self, view: Mapping[str, Any]) -> dict[str, Any] | None:
        return next((m for m in view["miners"] if m["hotkey"] == self.kp.ss58), None)

    def start_state(self, w: int, ro: RoundOpen) -> Params:
        od = self.train_cfg.model.od
        if w == 0 and not (od and od.warm_start):
            theta = init_params(self.train_cfg.model)
        else:
            theta, _ = unpack_state(self._fetch_state(ro.theta_hash))
        if state_hash(theta) != ro.theta_hash:
            raise StartStateMismatch(f"round {w}: start state TH != RoundOpen.theta_hash")
        return theta

    def _fetch_state(self, theta_hash: str) -> bytes:
        src = self.cfg.state_source
        if src.startswith(("http://", "https://")):
            r = self.http.get(f"{src.rstrip('/')}/{theta_hash}")
            r.raise_for_status()
            return r.content
        path = Path(src) / theta_hash
        if not path.is_file():
            raise StartStateMismatch(f"no state blob {theta_hash} under {src}")
        return path.read_bytes()

    def _done(self, w: int, step: str) -> dict[str, Any] | None:
        return self.journal.last(step, w=w)

    def run_round(self, w: int) -> str:
        """Drive round w to UPLOADED (or a terminal miss); idempotent across restarts."""
        for terminal in ("uploaded", "aborted", "skipped"):
            if rec := self._done(w, terminal):
                return str(rec.get("status", terminal.upper()))
        view = self.wait_round(w, "d_assign")
        ro = self.round_open(view)
        if self.kp.ss58 not in {e.hotkey for e in ro.roster}:
            self.journal.append("skipped", w=w, status="NOT_ROSTERED")
            return "NOT_ROSTERED"
        mine = next(a for a in view["assignment"] if a["hotkey"] == self.kp.ss58)
        samples = tuple(int(i) for i in mine["samples"])
        if assignment_hash(self.run_id, w, mine["slot"], samples) != mine["assignment_hash"]:
            raise MinerError("assignment_hash does not match the served samples")
        art = self._dir("rounds", str(w))
        if not self._done(w, "trained"):
            try:
                theta = self.start_state(w, ro)
            except StartStateMismatch as error:
                self.journal.append("aborted", w=w, status="START_STATE_MISMATCH", why=str(error))
                raise
            self._accept(w, view, mine["assignment_hash"])
            self._train(w, theta, samples, art)
        commit = json.loads((art / "commit.json").read_text())
        if not self._done(w, "committed"):
            if not self._commit(w, commit):
                return "COMMIT_REJECTED"
        if not self._done(w, "leaves"):
            pres = json.loads((art / "preimages.json").read_text())
            self._post("leaves", {"w": w, "hotkey": self.kp.ss58, "preimages": pres})
            self.journal.append("leaves", w=w)
        if not self._done(w, "uploaded"):
            self._upload(w, commit, (art / "delta.bin").read_bytes())
        self._prune(w)
        return "UPLOADED"

    def _accept(self, w: int, view: Mapping[str, Any], ahash: str) -> None:
        if self._done(w, "accepted"):
            return
        body = {
            "w": w,
            "hotkey": self.kp.ss58,
            "assignment_hash": ahash,
            "image_digest": self.cfg.image_digest,
            "driver_version": self.hw.driver,
            "n_gpus": self.hw.n_gpus,
        }
        try:
            self._post("accept", self._signed("Accept", body))
        except ChallengeHTTPError as e:
            mine = self._my(self.view(w))
            if e.status != 409 or mine is None or mine["status"] == "ASSIGNED":
                raise
            log.warning("accept 409 (%s); server already has us as %s", e.detail, mine["status"])
        self.journal.append("accepted", w=w)

    def _carry(self, w: int, theta: Params) -> OptState | None:
        inner = self.train_cfg.inner
        if inner.state_policy != "carry":
            return None
        prev = self.cfg.workdir / self.run_id[:16] / self.kp.ss58 / "rounds" / str(w - 1)
        if w > 0 and (prev / "final_state.bin").is_file():
            _, st = unpack_state((prev / "final_state.bin").read_bytes())
            if st is not None:
                return st
        return init_state(replace(inner, state_policy="reset"), theta)

    def _train(self, w: int, theta: Params, samples: tuple[int, ...], art: Path) -> None:
        cfg = self.train_cfg
        a = Assignment(self.run_id, w, samples, w * cfg.inner.H)
        carry = self._carry(w, theta)
        res = train_round(cfg, theta, a, self.get_sample, carry=carry)
        metrics = [bytes.fromhex(x.preimage.loss_f32 + x.preimage.norm_f32) for x in res.leaves]
        commit = {
            "w": w,
            "hotkey": self.kp.ss58,
            "leaf_scheme": "ht-leaf-v1",
            "n_leaves": len(res.leaves),
            "leaves_root": res.leaves_root,
            "metrics_root": MerkleTree(metrics).root.hex(),
            "final_theta_hash": res.final_theta_hash,
            "ef_in_hash": res.ef_in_hash,
            "ef_out_hash": res.ef_out_hash,
            "delta_hash": res.delta_hash,
            "delta_bytes": len(res.delta_payload),
            "tokens": len(samples) * cfg.model.seq_len,
        }
        pres = [x.preimage.model_dump(mode="json") for x in res.leaves]
        (art / "delta.bin").write_bytes(res.delta_payload)
        (art / "preimages.json").write_text(json.dumps(pres))
        (art / "start_state.bin").write_bytes(pack_state(theta, carry))
        (art / "final_state.bin").write_bytes(pack_state(res.final_theta, res.final_state))
        (art / "samples.json").write_text(json.dumps(list(samples)))
        (art / "commit.json").write_text(json.dumps(commit, sort_keys=True))
        self.journal.append("trained", w=w, leaves_root=res.leaves_root)

    def _commit(self, w: int, commit: dict[str, Any]) -> bool:
        try:
            out = self._post("commit", self._signed("Commit", commit))
        except ChallengeHTTPError as e:
            if e.status != 409:
                raise
            mine = self._my(self.view(w)) or {"status": None, "commit": None, "receipt": None}
            status = mine["status"]
            same = mine["commit"] is not None
            same = same and body_digest(mine["commit"]["body"]) == body_digest(commit)
            log.warning("commit 409 for round %d (%s); server status %s", w, e.detail, status)
            if same and status == "UPLOADED":
                self.journal.append("committed", w=w, receipt=mine["receipt"], via="409")
                self.journal.append("leaves", w=w)
                self.journal.append("uploaded", w=w, status="UPLOADED", via="409")
                return True
            if same and status == "COMMITTED":
                self.journal.append("committed", w=w, receipt=mine["receipt"], via="409")
                return True
            self.journal.append("aborted", w=w, status="COMMIT_REJECTED", why=e.detail)
            return False
        receipt = out["receipt"]
        ok = verify_envelope(receipt) and receipt["signer"] == self.manifest.coord_pubkey
        if not ok or receipt["body"]["commit_hash"] != body_digest(commit):
            raise MinerError("commit receipt is not a coordinator signature over our Commit")
        if out["status"] != "COMMITTED":
            self.journal.append("aborted", w=w, status=out["status"], receipt=receipt)
            return False
        self.journal.append("committed", w=w, receipt=receipt)
        return True

    def _put(self, url: str, data: bytes, sha: str) -> None:
        parts = urlsplit(url)
        if parts.scheme == "file":
            if not self.cfg.allow_file_upload:
                raise MinerError("file:// upload URLs are refused unless allow_file_upload")
            path = Path(parts.path)
            if path.name != sha:
                raise MinerError("presigned file URL is not keyed by the delta sha256")
            path.parent.mkdir(parents=True, exist_ok=True)
            tmp = path.with_suffix(f".tmp{os.getpid()}")
            tmp.write_bytes(data)
            tmp.replace(path)
            return
        if parts.scheme != "https" and parts.hostname not in ("127.0.0.1", "localhost"):
            raise MinerError("upload URLs must be https (or loopback http)")
        self.http.put(url, content=data).raise_for_status()

    def _upload(self, w: int, commit: Mapping[str, Any], payload: bytes) -> None:
        sha = hashlib.sha256(payload).hexdigest()
        if sha != commit["delta_hash"] or len(payload) != commit["delta_bytes"]:
            raise MinerError("stored delta does not match the committed delta_hash")
        try:
            signed_url = self._post("uploads", {"w": w, "hotkey": self.kp.ss58, "sha256": sha})
        except ChallengeHTTPError as e:
            mine = self._my(self.view(w))
            dm = mine["delta_manifest"] if mine else None
            if e.status != 409 or dm is None or dm["body"]["delta_hash"] != sha:
                raise
            log.warning("upload 409 for round %d: server already has our delta", w)
            self.journal.append("uploaded", w=w, status="UPLOADED", via="409")
            return
        self._put(signed_url["url"], payload, sha)
        step = self.cfg.chunk_bytes
        chunks = [
            {
                "off": o,
                "len": len(payload[o : o + step]),
                "sha256": sha256_hex(payload[o : o + step]),
            }
            for o in range(0, len(payload), step)
        ]
        fmt = (
            "ht-sparse-v1" if self.train_cfg.compress.codec == "sparseloco" else "ht-dense-int8-v1"
        )
        body = {
            "w": w,
            "hotkey": self.kp.ss58,
            "delta_hash": sha,
            "uri": f"objects/{sha}",
            "size": len(payload),
            "format": fmt,
            "chunks": chunks,
        }
        out = self._post("delta", self._signed("DeltaManifest", body))
        self.journal.append("uploaded", w=w, status=out["status"], chunks=len(chunks))

    def _prune(self, w: int) -> None:
        keep = self.manifest.verify.E_vest_rounds + 2
        root = self.cfg.workdir / self.run_id[:16] / self.kp.ss58 / "rounds"
        for d in root.iterdir():
            if d.name.isdigit() and int(d.name) < w - keep:
                shutil.rmtree(d)

    def _states(self, w: int) -> list[tuple[Params, OptState]]:
        """States at every leaf boundary, recomputed from the stored round inputs."""
        art = self._dir("rounds", str(w))
        theta, st = unpack_state((art / "start_state.bin").read_bytes())
        samples = tuple(json.loads((art / "samples.json").read_text()))
        cfg = self.train_cfg
        ex = Executor(
            self.kp.ss58,
            cfg,
            theta,
            Assignment(self.run_id, w, samples, w * cfg.inner.H),
            self.get_sample,
            carry=st,
        )
        return [ex.state(i * cfg.inner.J) for i in range(cfg.inner.n_leaves)]

    def serve_challenges(self, w: int) -> int:
        """Answer segments-mode AuditChallenges on us with StateServe; returns serves posted."""
        if self.blob_sink is None:
            return 0
        view, posted = self.view(w), 0
        pres = [
            LeafPreimage.model_validate(p)
            for p in json.loads((self._dir("rounds", str(w)) / "preimages.json").read_text())
        ]
        tree = MerkleTree([bytes.fromhex(p.digest()) for p in pres])
        for job in view["jobs"]:
            env = job["challenge"]
            if job["target"] != self.kp.ss58 or env["signer"] != self.manifest.coord_pubkey:
                continue
            if not verify_envelope(env):
                continue
            ch = AuditChallenge.model_validate(env["body"])
            if ch.mode != "segments" or self._done(w, f"served:{challenge_hash(ch)}"):
                continue
            states = self._states(w)
            j = self.train_cfg.inner.J
            for a, _ in ch.segments:
                if not 0 <= a < len(states):
                    raise MinerError(f"challenge segment start leaf {a} is out of range")
                theta, st = self.served_state(states, a)
                t = a * j
                blob = pack_state(theta, st)
                sha = self.blob_sink(blob)
                body = {
                    "hotkey": self.kp.ss58,
                    "challenge_hash": challenge_hash(ch),
                    "t": t,
                    "uri": f"objects/{sha}",
                    "tensor_root": tensor_root(theta, st),
                    "merkle_proof_leaf_in_leaves_root": [p.hex() for p in tree.proof(a)],
                }
                self._post("state", self._signed("StateServe", body))
                posted += 1
            self.journal.append(f"served:{challenge_hash(ch)}", w=w)
        return posted

    def served_state(
        self, states: list[tuple[Params, OptState]], leaf: int
    ) -> tuple[Params, OptState]:
        return states[leaf]

    def dispute(self, w: int) -> str | None:
        """After a MISMATCH: rerun our own round, post the rerun, then accept or contest."""
        if rec := self._done(w, "disputed"):
            return str(rec["classification"])
        mine = self._my(self.view(w))
        if mine is None or mine["status"] != "MISMATCH" or mine["verdict"] is None:
            return None
        art = self._dir("rounds", str(w))
        theta, st = unpack_state((art / "start_state.bin").read_bytes())
        samples = tuple(json.loads((art / "samples.json").read_text()))
        a = Assignment(self.run_id, w, samples, w * self.train_cfg.inner.H)
        root = train_round(self.train_cfg, theta, a, self.get_sample, carry=st).leaves_root
        sig = self.kp.sign(rerun_message(self.run_id, w, self.kp.ss58, root)).hex()
        out = self._post("rerun", {"w": w, "hotkey": self.kp.ss58, "leaves_root": root, "sig": sig})
        cls = str(out["classification"])
        action = "contest" if cls == "CONTEST" else "accept"
        if cls != "TRANSIENT":
            body = {
                "hotkey": self.kp.ss58,
                "verdict_hash": body_digest(mine["verdict"]["body"]),
                "action": action,
            }
            disp = self._post("dispute", self._signed("Dispute", body))
            self.journal.append("dispute_id", w=w, dispute_id=disp["dispute_id"])
        self.journal.append("disputed", w=w, classification=cls, action=action)
        return cls

    def bisect(self, w: int, dispute_id: str, interval: tuple[int, int], n: int) -> dict[str, Any]:
        """Publish our step-level N+1 hashes for one bisection round of an open dispute."""
        art = self._dir("rounds", str(w))
        theta, st = unpack_state((art / "start_state.bin").read_bytes())
        samples = tuple(json.loads((art / "samples.json").read_text()))
        cfg = self.train_cfg
        ex = Executor(
            self.kp.ss58,
            cfg,
            theta,
            Assignment(self.run_id, w, samples, w * cfg.inner.H),
            self.get_sample,
            carry=st,
        )
        from hypertrain.auditor.bisect import points

        at = points(interval[0], interval[1], n)
        body = {
            "dispute_id": dispute_id,
            "level": "step",
            "interval": list(interval),
            "N": len(at) - 1,
            "hashes": ex.hashes("step", (), at),
            "party": self.kp.ss58,
        }
        out: dict[str, Any] = self._post("bisect", self._signed("Bisect", body))
        return out

    def run(self, rounds: int, start: int | None = None) -> list[tuple[int, str]]:
        status = self._run_status()
        w = start if start is not None else max((r["w"] for r in status["rounds"]), default=0)
        out: list[tuple[int, str]] = []
        for _ in range(rounds):
            self.wait_for_round(w)
            try:
                result = self.run_round(w)
            except StartStateMismatch as error:
                log.error("round %d aborted: %s", w, error)
                result = "START_STATE_MISMATCH"
            log.info("round %d: %s", w, result)
            out.append((w, result))
            for back in range(max(0, w - 2), w):
                if self._done(back, "uploaded"):
                    self.serve_challenges(back)
                    self.dispute(back)
            w += 1
        return out


def canonical(obj: Any) -> bytes:
    return canonicalize(obj, allow_float=False)


class NetworkMiner:
    """Explicit ht/2 client; shared API, keyfile, launcher, admission and watch primitives."""

    def __init__(self, cfg: MinerConfig, client: httpx.Client) -> None:
        from hypertrain.protocol import envelope_v2
        from hypertrain.protocol.messages_v2 import RunManifestV2

        if cfg.run_id is None or cfg.owner_hotkey is None:
            raise MinerError("v2 requires pinned run_id and owner_hotkey")
        self.cfg, self.api, self.kp = cfg, Api(client, cfg.api), load_keyfile(cfg.keyfile)
        self.run_id, self.url = cfg.run_id, f"/v2/runs/{cfg.run_id}"
        status = self.api.call("GET", self.url)
        env = envelope_v2.parse_envelope(status["manifest_envelope"])
        if (
            env.signer != cfg.owner_hotkey
            or not envelope_v2.verify_envelope(env.model_dump())
            or env.type != "RunManifestV2"
        ):
            raise MinerError("v2 manifest authority/signature mismatch")
        self.manifest = RunManifestV2.model_validate(env.body)
        self.manifest.validate_run_id(self.run_id)
        self.directory = cfg.workdir / self.run_id / self.kp.ss58
        self.directory.mkdir(parents=True, exist_ok=True)

    def status(self) -> dict[str, Any]:
        return self.api.call("GET", self.url + "/admission/" + self.kp.ss58)

    def _note_retry(self, route: str, status: int | None, attempt: int, delay: float) -> None:
        """One journal line per object GET retry, so a replay can count them."""
        Journal(self.directory).append(
            "retry",
            utc=time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
            route=route,
            status=status,
            attempt=attempt,
            delay=delay,
        )

    def _retry[T](self, what: str, fn: Callable[[], T], *, also: frozenset[int] = frozenset()) -> T:
        return _retrying(what, fn, also=also, noted=self._note_retry)

    def join(self, coldfile: Path, request_id: str, expiry: int) -> dict[str, Any]:
        from hypertrain.miner.admission import sign_join
        from hypertrain.protocol.messages_v2 import HardwareHint

        request = sign_join(
            self.kp,
            load_keyfile(coldfile),
            run_id=self.run_id,
            request_id=request_id,
            expires_beacon=expiry,
            policy_hash=self.manifest.network.admission_policy_hash,
            hardware_hint=HardwareHint(
                device_name=self.cfg.device,
                device_count=self.manifest.training.reference_spec.layout.n_gpus,
                driver="advisory",
            ),
        )
        return self.api.call("POST", self.url + "/join", content=canonical(request.body()))

    def probe(
        self,
        job_path: Path,
        challenge_path: Path,
        *,
        launch: IslandLaunch | None = None,
        cancel: threading.Event | None = None,
        trace: bool = False,
    ) -> dict[str, Any]:
        from hypertrain.gpu_ops.work_screen import screen_work
        from hypertrain.protocol import envelope_v2
        from hypertrain.protocol.messages_v2 import IslandJobV1, JoinChallenge

        job = IslandJobV1.model_validate_json(job_path.read_bytes())
        if time.time() >= job.deadline or cancel is not None and cancel.is_set():
            raise MinerError("probe deadline/cancellation")
        env = envelope_v2.parse_envelope(challenge_path.read_bytes())
        if (
            job.run_id != self.run_id
            or env.signer != self.manifest.training.coord_pubkey
            or (env.run_id != self.run_id or not envelope_v2.verify_envelope(env.model_dump()))
        ):
            raise MinerError("job/challenge authority mismatch")
        now = self.api.call("GET", self.url)["now_round"]
        challenge = JoinChallenge.model_validate(env.body)
        requested_launch = launch
        if trace:
            from hypertrain.miner.island_launch import confined, launch_island

            def traced_launch(
                job: IslandJobV1,
                directory: Path,
                *,
                backend: Literal["cpu", "cuda"],
                cancel: threading.Event | None = None,
                trace: bool = False,
            ) -> IslandArtifacts:
                artifacts = (launch or launch_island)(
                    job, directory, backend=backend, cancel=cancel, trace=True
                )
                for rank in range(job.manifest.training.reference_spec.layout.n_gpus):
                    relative = f"rank-{rank}/trace.json"
                    if not (artifacts.directory / relative).is_file():
                        raise MinerError("probe required trace absent: " + relative)
                    confined(artifacts.directory, relative)
                return artifacts

            requested_launch = traced_launch
        screen, proof = screen_work(
            job,
            job_path.parent,
            challenge,
            now_beacon=now,
            backend="cpu" if self.cfg.device == "cpu" else "cuda",
            launch=requested_launch,
            cancel=cancel,
        )
        if time.time() >= job.deadline or cancel is not None and cancel.is_set():
            raise MinerError("probe deadline/cancellation")
        result = {
            "proof": envelope_v2.seal(
                self.kp, "WorkProof", self.run_id, proof, challenge.deadline_beacon
            ),
            "screen": envelope_v2.seal(
                self.kp, "WorkScreenV2", self.run_id, screen, challenge.deadline_beacon
            ),
        }
        put = self._bounded_put(job.deadline, cancel)
        # Eager: admission.proof verifies every proof ref against stored bytes (one-time join).
        for ref, filename in zip(
            proof.artifact_refs,
            ("state.safetensors", "ef.safetensors", "delta.bin", "leaves.json"),
            strict=True,
        ):
            if time.time() >= job.deadline or cancel is not None and cancel.is_set():
                raise MinerError("probe deadline/cancellation")
            path = job_path.parent / "published" / "rank-0" / filename
            if _upload_ref(path, put) != ref:
                raise MinerError("published artifact differs from screened ref")
        if time.time() >= job.deadline or cancel is not None and cancel.is_set():
            raise MinerError("probe deadline/cancellation")
        return result

    def probe_trial(self) -> dict[str, Any]:
        """probe-v2 without --job: this hotkey's open trial, fetched into a private temp folder."""
        with tempfile.TemporaryDirectory(prefix="trial-", dir=self.directory) as folder:
            job, challenge = self._fetch_trial(Path(folder).resolve())
            return self.probe(job, challenge)

    def _fetch_trial(self, root: Path) -> tuple[Path, Path]:
        """The folder trial_export writes, from the miner's own signed job read.

        The challenge request, a body signed by this hotkey, issues the trial when none is open
        (else returns the stored one); the job request, signed the same way, reads its job. The
        job, its inputs and the signed challenge are checked as trial_export checks them."""
        from pydantic import TypeAdapter

        from hypertrain.protocol import envelope_v2
        from hypertrain.protocol.messages_v2 import Hex64, IslandJobV1, JoinChallenge

        admission_id = self.status()["record"]["admission_id"]
        url = self.url + f"/join/{admission_id}"
        now = self.api.call("GET", self.url)["now_round"]

        def asked(message: str) -> dict[str, Any]:
            return envelope_v2.seal(
                self.kp,
                message,
                self.run_id,
                {"admission_id": admission_id},
                now + OWNER_REQUEST_ROUNDS,
            )

        envelope = self.api.call("POST", url + "/challenge", json=asked("ChallengeRequest"))
        data = self.api.call("POST", url + "/job", json=asked("JobRequest"))
        try:
            job = IslandJobV1.model_validate(data["job"])
            challenge = JoinChallenge.model_validate(data["challenge"])
            objects = TypeAdapter(dict[str, Hex64]).validate_python(data["objects"])
            nonce = data["nonce"]
            signed = envelope_v2.parse_envelope(envelope)
        except (ValueError, KeyError, TypeError) as error:
            raise MinerError("malformed trial job response") from error
        if (
            job.run_id != self.run_id
            or challenge.manifest_hash != self.run_id
            or challenge.admission_id != admission_id
            or challenge.nonce != nonce
            or set(objects) != set(job.object_paths)
        ):
            raise MinerError("trial job/challenge/object binding differs")
        for name in ("start_state", "ef_in", "v0"):
            if objects.get(name) != getattr(job, name + "_sha256"):
                raise MinerError(f"trial {name} sha256 differs from job")
        if (
            signed.type != "JoinChallenge"
            or signed.run_id != self.run_id
            or signed.signer != self.manifest.training.coord_pubkey
            or signed.body != challenge.body()
            or not envelope_v2.verify_envelope(envelope)
        ):
            raise MinerError("signed challenge differs from the trial job")
        destinations = {root / "job.json", root / "challenge.json"}
        paths: dict[str, Path] = {}
        for name, relative in job.object_paths.items():
            target = (root / relative).resolve()
            if (
                target == root
                or not target.is_relative_to(root)
                or any(
                    target == p or target in p.parents or p in target.parents for p in destinations
                )
            ):
                raise MinerError(f"unsafe trial object path: {relative}")
            destinations.add(target)
            paths[name] = target
        for name, digest in objects.items():
            paths[name].parent.mkdir(parents=True, exist_ok=True)
            self._get_whole(digest, paths[name])
        (root / "job.json").write_bytes(canonicalize(job.body()))
        (root / "challenge.json").write_bytes(canonicalize(envelope))
        return root / "job.json", root / "challenge.json"

    def _get_whole(self, digest: str, target: Path) -> None:
        """Stream one whole plain object to `target` and verify it: the trial export stores each
        input as a single object, so the 1 MiB part bound of _get_object does not apply; the
        bound is the largest object a part index can describe."""
        from hypertrain.protocol.messages_v2 import MAX_PARTS, PART_BYTES

        def get() -> None:
            hasher, size = hashlib.sha256(), 0
            with self.api.c.stream(
                "GET",
                self.api.base + self.url + "/objects/" + digest,
                headers={"Accept-Encoding": "identity"},
            ) as r:
                if not r.is_success:
                    raise ChallengeHTTPError(r.status_code, "trial input object unavailable")
                # As _get_object: httpx would decode gzip/br before the bound check.
                if r.headers.get("Content-Encoding", "identity").lower() != "identity":
                    raise MinerError("trial input object has a non-identity content encoding")
                with target.open("wb") as stream:
                    for chunk in r.iter_bytes(chunk_size=64 << 10):
                        size += len(chunk)
                        if size > MAX_PARTS * PART_BYTES:
                            raise MinerError("trial input object exceeds the object size bound")
                        hasher.update(chunk)
                        stream.write(chunk)
            if hasher.hexdigest() != digest:
                raise MinerError(f"trial input {target.name} sha256 mismatch")

        self._retry("GET object " + digest, get, also=OBJECT_MISS)

    def _bounded_put(
        self, deadline: float, cancel: threading.Event | None
    ) -> Callable[[bytes], str]:
        """_put_object that refuses every further part/index PUT once deadline or cancel hits."""

        def put(data: bytes) -> str:
            if time.time() >= deadline or cancel is not None and cancel.is_set():
                raise MinerError("publication deadline/cancellation")
            return self._put_object(data)

        return put

    def _put_object(self, data: bytes) -> str:
        """One signed v2 object PUT (plain object, part or part index; each <= 1 MiB).

        Bytes body, so httpx sends Content-Length (proxies reject chunked uploads); the PUT is
        content-addressed, so transport errors and 5xx are retried."""
        digest = sha256_hex(data)
        headers = {
            "X-Hotkey": self.kp.ss58,
            "X-Object-Signature": self.kp.sign(
                f"hypertrain/object/2|{self.run_id}|{digest}".encode()
            ).hex(),
        }

        def put() -> str:
            response = self.api.c.put(
                self.api.base + self.url + "/objects/" + digest, content=data, headers=headers
            )
            response.raise_for_status()
            return digest

        return _retrying("PUT object " + digest, put)

    def _fetch_inputs(
        self,
        job: IslandJobV1,
        objects: Mapping[str, str],
        directory: Path,
        theta: str | None = None,
        previous: Path | None = None,
    ) -> None:
        """objects: name -> stored ref (plain or ht-parts-v1 index); the job pins whole SHAs.

        Each file is verified and atomically replaced; a bad ref never leaves a file behind.
        previous: this miner's own prior publication (rank-0); ef_in, and start_state rebuilt
        from the public `theta` ref + its own m/v/step, are reused when they hash to the pins."""
        from hypertrain.data.store import StoreError, fetch_to

        whole = {
            "start_state": job.start_state_sha256,
            "ef_in": job.ef_in_sha256,
            "v0": job.v0_sha256,
        }
        # Untrusted response: names must be exactly the job's validated (confined) object paths.
        targets = {(directory / p).resolve() for p in job.object_paths.values()}
        if set(objects) != set(job.object_paths) or len(targets) != len(job.object_paths):
            raise MinerError("job object names differ from the job's object paths")
        for name, digest in objects.items():
            dest = directory / job.object_paths[name]
            if name in whole and self._local_input(name, whole[name], dest, theta, previous):
                continue
            try:
                fetch_to(digest, self._get_object, dest, workers=FETCH_WORKERS)
            except StoreError as e:
                raise MinerError(f"job input {name} corrupt: {e}") from None
            if name in whole:
                with dest.open("rb") as f:
                    if hashlib.file_digest(f, "sha256").hexdigest() != whole[name]:
                        dest.unlink()
                        raise MinerError("job input hash mismatch")

    def _local_input(
        self, name: str, whole: str, dest: Path, theta: str | None, previous: Path | None
    ) -> bool:
        """True iff dest now holds bytes rebuilt locally that hash to `whole` (else untouched).

        Streamed on disk (bounded RAM): start_state = the previous state file with its theta/*
        byte ranges overwritten from the fetched theta-only file (same names/shapes/dtypes, so
        the safetensors header and offsets are identical). Any mismatch, or a corrupt or
        unfetchable theta ref, returns False so the caller downloads the full pinned object."""
        from hypertrain.data.store import StoreError, fetch_to

        if previous is None:
            return False
        source = previous / ("ef.safetensors" if name == "ef_in" else "state.safetensors")
        if name not in ("ef_in", "start_state") or not source.is_file():
            return False
        if name == "start_state" and theta is None:
            return False
        fd, tmp = tempfile.mkstemp(dir=dest.parent, prefix=f".{dest.name}.", suffix=".part")
        os.close(fd)
        out, theta_tmp = Path(tmp), dest.parent / f".{dest.name}.theta.part"
        try:
            shutil.copyfile(source, out)
            if name == "start_state" and theta is not None:
                fetch_to(theta, self._get_object, theta_tmp, workers=FETCH_WORKERS)
                if not _overwrite_theta(out, theta_tmp):
                    return False
            with out.open("rb") as f:
                if hashlib.file_digest(f, "sha256").hexdigest() != whole:
                    return False
            os.replace(out, dest)
            return True
        except (StoreError, MinerError, ValueError, httpx.HTTPError) as e:
            log.warning("local %s rebuild failed (%s); downloading the pinned object", name, e)
            return False
        finally:
            out.unlink(missing_ok=True)
            theta_tmp.unlink(missing_ok=True)

    def _get_object(self, digest: str) -> bytes:
        """Bounded streamed GET: ht-parts-v1 parts and indexes are <= 1 MiB each, so a larger
        body is refused before it is buffered (legacy >1 MiB plain objects are not fetchable)."""
        from hypertrain.protocol.messages_v2 import MAX_MANIFEST_BYTES, PART_BYTES

        limit = max(PART_BYTES, MAX_MANIFEST_BYTES)

        def get() -> bytes:
            with self.api.c.stream(
                "GET",
                self.api.base + self.url + "/objects/" + digest,
                headers={"Accept-Encoding": "identity"},
            ) as response:
                response.raise_for_status()
                # httpx would decode gzip/br before the bound check; a hostile server ignores
                # headers.
                if response.headers.get("Content-Encoding", "identity").lower() != "identity":
                    raise MinerError("job input object has a non-identity content encoding")
                body = bytearray()
                for chunk in response.iter_bytes(chunk_size=64 << 10):
                    if len(body) + len(chunk) > limit:
                        raise MinerError("job input object exceeds the 1 MiB part bound")
                    body.extend(chunk)
                return bytes(body)

        return self._retry("GET object " + digest, get, also=OBJECT_MISS)

    def proof(self, document: Path) -> dict[str, Any]:
        from hypertrain.protocol.envelope_v2 import load_json

        return self.api.call(
            "POST", self.url + "/join/proof", json=load_json(document.read_bytes())
        )

    def launch(self, job_path: Path, trace: bool = True) -> dict[str, str]:
        from hypertrain.miner.island_launch import launch_island
        from hypertrain.protocol.messages_v2 import IslandJobV1

        job = IslandJobV1.model_validate_json(job_path.read_bytes())
        if job.run_id != self.run_id:
            raise MinerError("cross-run island job")
        if job.manifest != self.manifest:
            raise MinerError("island job manifest differs from the signed run manifest")
        artifacts = launch_island(
            job,
            job_path.parent,
            backend="cpu" if self.cfg.device == "cpu" else "cuda",
            trace=trace,
            devices=self._devices(),
        )
        return {
            "state": str(artifacts.state),
            "delta": str(artifacts.delta),
            "leaves": str(artifacts.leaves),
        }

    def _devices(self) -> tuple[str, ...] | None:
        """CPU: None (gloo ranks). CUDA: the layout.n_gpus GPU UUIDs pinned for torchrun."""
        if self.cfg.device == "cpu":
            return None
        ref = self.manifest.training.reference_spec
        return select_gpus(detect_gpus(), ref.layout.n_gpus, ref.sm_count)

    def run_round(self, w: int) -> str:
        import anyio

        from hypertrain.miner.island_launch import launch_island
        from hypertrain.protocol import envelope_v2, relay_envelope
        from hypertrain.protocol.messages_v2 import IslandJobV1, RoundOpenV2, WorkProof
        from hypertrain.protocol.relay_messages import RelayRegistryV1
        from hypertrain.relay.client import RelayClient, chunk_manifest

        view = self.api.call("GET", self.url + f"/rounds/{w}")
        opening = envelope_v2.parse_envelope(view["round_open"])
        if opening.signer != self.manifest.training.coord_pubkey or not envelope_v2.verify_envelope(
            opening.model_dump()
        ):
            raise MinerError("round authority mismatch")
        schedule = RoundOpenV2.model_validate(opening.body)
        assignment = next((a for a in view["assignment"] if a["hotkey"] == self.kp.ss58), None)
        if assignment is None:
            raise MinerError("authenticated assignment not ready")
        status = self.status()
        screen_hash = status["work_screen_hash"]
        driver = "cpu"
        if self.cfg.device != "cpu":
            # Parent launch validates exact logical count/SM/runtime; this is observed driver only.
            version = Path("/proc/driver/nvidia/version")
            parts = version.read_text().split()
            driver = next((p for p in parts if p[:1].isdigit() and "." in p), "unknown")
        if (
            self.cfg.device != "cpu"
            and driver not in self.manifest.training.reference_spec.driver_allowlist
        ):
            raise HardwareMismatch("observed v2 driver not in manifest allowlist")
        if self.cfg.device == "cpu":
            from hypertrain.protocol.messages_v2 import EconomicsPolicyV2

            policy_response = self.api.c.get(
                self.api.base + self.url + "/objects/" + self.manifest.network.economics_policy_hash
            )
            policy_response.raise_for_status()
            if sha256_hex(
                policy_response.content
            ) != self.manifest.network.economics_policy_hash or (
                EconomicsPolicyV2.model_validate_json(policy_response.content).ledger_mode != "test"
            ):
                raise HardwareMismatch(
                    "CPU client cannot assert production CUDA reference identity"
                )
            driver = self.manifest.training.reference_spec.driver_allowlist[0]
        accept = {
            "w": w,
            "hotkey": self.kp.ss58,
            "assignment_hash": assignment["assignment_hash"],
            "work_screen_hash": screen_hash,
            "image_digest": self.manifest.training.reference_spec.image_digest,
            "driver_version": driver,
            "n_gpus": self.manifest.training.reference_spec.layout.n_gpus,
        }
        self.api.call(
            "POST",
            self.url + "/accept",
            json=envelope_v2.seal(self.kp, "AcceptV2", self.run_id, accept, schedule.d_final),
        )
        staged = self.api.call("GET", self.url + f"/rounds/{w}/job/{self.kp.ss58}")
        job = IslandJobV1.model_validate(staged["job"])
        directory = self.directory / str(w)
        directory.mkdir(parents=True, exist_ok=True)
        self._fetch_inputs(
            job,
            staged["objects"],
            directory,
            staged.get("theta"),
            self.directory / str(w - 1) / "published" / "rank-0" if w else None,
        )
        artifacts = launch_island(
            job,
            directory,
            backend="cpu" if self.cfg.device == "cpu" else "cuda",
            trace=True,
            devices=self._devices(),
        )
        preimages = json.loads(artifacts.leaves.read_bytes())
        summary = json.loads((artifacts.directory / "rank-0/summary.json").read_bytes())[
            "commitments"
        ]
        start = next(s for s in view["starts"] if s["hotkey"] == self.kp.ss58)
        commit = {
            "w": w,
            "hotkey": self.kp.ss58,
            "leaf_scheme": "ht-leaf-v1",
            "n_leaves": len(preimages),
            "metrics_root": MerkleTree(
                [bytes.fromhex(p["loss_f32"] + p["norm_f32"]) for p in preimages]
            ).root.hex(),
            "tokens": len(job.sample_ids) * self.manifest.training.model.seq_len,
            "delta_bytes": artifacts.delta.stat().st_size,
            "ef_in_hash": start["ef_hash"],
            **{
                k: summary[k]
                for k in ("leaves_root", "final_theta_hash", "ef_out_hash", "delta_hash")
            },
        }
        self.api.call(
            "POST",
            self.url + "/commit",
            json=envelope_v2.seal(self.kp, "CommitV2", self.run_id, commit, schedule.d_final),
        )
        from hypertrain.gpu_ops.work_screen import artifact_ref

        # Same d_final -> wall clock mapping as the master (store.py: genesis + (d - 1) * 3).
        put = self._bounded_put(
            self.manifest.training.beacon.genesis_time + (schedule.d_final - 1) * 3, None
        )
        # state/ef/delta are committed by ref only: no round verifier reads them from the store
        # (leaves_v2 reads refs[-1]; the delta reaches the master via relay retrieval; audits
        # replay from master-staged inputs; disputes serve local checkpoints).
        refs = [artifact_ref(path) for path in (artifacts.state, artifacts.ef, artifacts.delta)]
        refs.append(_upload_ref(artifacts.leaves, put))
        descriptor = WorkProof(
            admission_id=status["record"]["admission_id"],
            challenge_hash=sha256_hex(canonical(commit)),
            leaves_root=commit["leaves_root"],
            delta_hash=commit["delta_hash"],
            artifact_refs=refs,
        )
        self.api.call(
            "POST",
            self.url + "/leaves",
            json=envelope_v2.seal(self.kp, "WorkProof", self.run_id, descriptor, schedule.d_final),
        )
        assigned = self.api.call("GET", self.url + f"/rounds/{w}/relay/{self.kp.ss58}")
        chunks = chunk_manifest(artifacts.delta)
        grant = self.api.call(
            "POST",
            self.url + "/upload-grant?commit_hash=" + sha256_hex(canonical(commit)),
            json=relay_envelope.seal(
                self.kp, "UploadChunkManifest", self.run_id, chunks, schedule.d_upload
            ),
        )
        from hypertrain.protocol.relay_envelope import RelayEnvelope

        async def upload() -> RelayEnvelope:
            async with httpx.AsyncClient(follow_redirects=False, timeout=60) as client:
                return await RelayClient(
                    RelayRegistryV1.model_validate(assigned["registry"]), client
                ).upload(
                    artifacts.delta,
                    grant_raw=relay_envelope.parse_envelope(grant),
                    assignment=relay_envelope.parse_envelope(assigned["assignment"]),
                    round_open=view["round_open"],
                )

        receipt = anyio.run(upload)
        acceptance = self.api.call(
            "POST", "/v2/relay-receipts", content=canonical(receipt.model_dump(mode="json"))
        )
        delta = {
            "w": w,
            "hotkey": self.kp.ss58,
            "delta_hash": commit["delta_hash"],
            "size": artifacts.delta.stat().st_size,
            "uri": "sha256:" + commit["delta_hash"],
            "format": "ht-sparse-v1"
            if self.manifest.training.outer.opt == "sparseloco"
            else "ht-dense-int8-v1",
            "chunks": [
                {"off": c.off, "len": c.len, "sha256": c.chunk_sha256} for c in chunks.chunks
            ],
            "grant_hash": sha256_hex(canonical(grant["body"])),
            "master_acceptance_hash": sha256_hex(canonical(acceptance["body"])),
        }
        self.api.call(
            "POST",
            self.url + "/delta",
            json=envelope_v2.seal(
                self.kp, "DeltaManifestV2", self.run_id, delta, schedule.d_upload
            ),
        )
        return "UPLOADED"

    def watch(self, job_path: Path, timeout: float = 30) -> int:
        from hypertrain.auditor.island_bisect import IslandParty
        from hypertrain.data.store import LocalFSStore
        from hypertrain.miner.dispute_watch import (
            DisputeWatch,
            HttpDisputeTransport,
            PublishedStates,
        )
        from hypertrain.protocol.messages_v2 import IslandJobV1

        job = IslandJobV1.model_validate_json(job_path.read_bytes())
        if job.run_id != self.run_id:
            raise MinerError("cross-run watch job")
        published = job_path.parent / "published"
        states = PublishedStates(job, published, LocalFSStore(self.directory / "state-objects"))
        party = IslandParty.published(self.kp.ss58, job, published)
        client = self.api.c
        client.headers["X-Dispute-Signature"] = self.kp.sign(
            f"hypertrain/watch/2|{self.run_id}|{self.kp.ss58}".encode()
        ).hex()
        watcher = DisputeWatch(
            self.directory / "watch",
            self.kp,
            HttpDisputeTransport(client, self.api.base + self.url),
            run_id=self.run_id,
            coordinator=self.manifest.training.coord_pubkey,
            party=lambda _: party,
            state_serve=states.serve,
            beacon=lambda: self.api.call("GET", self.url)["now_round"],
            job=job,
        )
        try:
            return watcher.run_once(timeout=timeout)
        finally:
            watcher.close()
