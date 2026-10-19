"""One-host Vast.ai end-to-end harness for Hypertrain (2x RTX 5090). Stdlib only.

Flow: credit check -> offer search -> worst-case cost admission -> ONE create -> ssh ready ->
upload current tree -> scripts/vast_e2e_remote.sh -> rescue /workspace/out -> DELETE -> parsed
absence. A watchdog thread fires at the wall deadline (minus the cleanup reserve) and runs the
same rescue+delete. Every provider call is journaled. `--dry-run` stops after the cost calc.
The API key is read from --key-file and never printed or journaled.
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import os
import shlex
import signal
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid
from collections.abc import Callable
from decimal import Decimal
from pathlib import Path
from typing import Any

IMAGE = (
    "ghcr.io/cortexlm/hypertrain-gpu@sha256:"
    "c16a28dd29df300eca0cfb5c5aeacb8947681708634e3cbc8639b530321fe2e2"
)
ROOT = Path(__file__).resolve().parents[1]
EVIDENCE = ROOT / ".evidence" / "vast"  # = --evidence-dir default; siblings use EVIDENCE.parent
UPLOAD = ["src", "tests", "pyproject.toml", "uv.lock", "scripts", "README.md", "LICENSE"]
DISK_GB = 40
# 142018: GPUs at 100% util by a hidden tenant; 146840/141607: image pull >15 min (2026-10-12)
EXCLUDED_MACHINES = (137465, 43459, 142018, 146840, 141607)
HOURS_PER_MONTH = Decimal(720)
OFFER_PAGES = 8  # offer-search pages (of offer_query()["limit"]) before admission refuses


class Refused(Exception):
    """Admission refusal: nothing was created."""


class OfferGone(Exception):
    """CREATE refused (offer taken meanwhile) and a parsed listing shows no instance."""


class Response:
    def __init__(self, status: int | None, body: Any, retry_after: float | None, error: str):
        self.status, self.body, self.retry_after, self.error = status, body, retry_after, error

    def ok(self) -> bool:
        return self.status is not None and 200 <= self.status < 300 and isinstance(self.body, dict)


class Api:
    """Paced (>= min_gap between calls), journaled, 429-aware client."""

    def __init__(
        self,
        base: str,
        key: str,
        journal: Path,
        *,
        min_gap: float = 5.0,
        sleep: Callable[[float], None] = time.sleep,
        max_429: int = 6,
    ) -> None:
        self.base, self._key, self.journal = base.rstrip("/"), key, journal
        self.min_gap, self.sleep, self.max_429 = min_gap, sleep, max_429
        self._last = -1e18
        self._lock = threading.Lock()

    def log(self, event: str, **fields: Any) -> None:
        line = json.dumps({"ts": time.time(), "event": event, **fields}, sort_keys=True)
        if self._key and self._key in line:  # defence in depth: never persist the key
            line = line.replace(self._key, "<redacted>")
        with self._lock, self.journal.open("a") as f:
            f.write(line + "\n")

    def call(self, method: str, path: str, body: Any = None, timeout: float = 30) -> Response:
        for attempt in range(self.max_429 + 1):
            r = self._once(method, path, body, timeout, attempt)
            if r.status != 429 or attempt == self.max_429:
                return r
            wait = r.retry_after if r.retry_after is not None else min(60.0, 5.0 * 2**attempt)
            self.log("backoff_429", method=method, path=path.split("?")[0], seconds=wait)
            self.sleep(wait)
        raise AssertionError("unreachable")

    def _once(self, method: str, path: str, body: Any, timeout: float, attempt: int) -> Response:
        gap = self.min_gap - (time.monotonic() - self._last)
        if gap > 0:
            self.sleep(gap)
        request = urllib.request.Request(  # noqa: S310 - operator-chosen API base
            self.base + path,
            data=json.dumps(body).encode() if body is not None else None,
            method=method,
            headers={"Authorization": "Bearer " + self._key, "Content-Type": "application/json"},
        )
        status, raw, retry, error = None, b"", None, ""
        t0 = time.time()
        try:
            with urllib.request.urlopen(request, timeout=timeout) as resp:  # noqa: S310
                status, raw = resp.status, resp.read(4 << 20)
        except urllib.error.HTTPError as e:
            status, raw = e.code, e.read(4 << 20)
            header = e.headers.get("Retry-After")
            retry = float(header) if header and header.replace(".", "", 1).isdigit() else None
        except Exception as e:  # network failure is journaled, never a silent pass
            error = f"{type(e).__name__}: {str(e)[:300]}"
        self._last = time.monotonic()
        try:
            parsed = json.loads(raw) if raw else None
        except ValueError:
            parsed = None
        self.log(
            "provider_call",
            method=method,
            path=path,
            status=status,
            attempt=attempt,
            seconds=round(time.time() - t0, 3),
            error=error,
            body_keys=sorted(body) if isinstance(body, dict) else None,
        )
        return Response(status, parsed, retry, error)


def dec(value: Any) -> Decimal | None:
    if isinstance(value, bool) or not isinstance(value, int | float | str):
        return None
    try:
        d = Decimal(str(value))
    except ArithmeticError:
        return None
    return d if d.is_finite() and d >= 0 else None


MINER_FLOW_IMAGE = (
    "docker.io/vastai/pytorch:2.14.0-cu130-cuda-13.2-mini-py312-2026-09-08@sha256:"
    "450710d89268fcd55632f28fdd80508ee920a53075b3f8e9f828bdc154e3d856"
)
NUM_GPUS = 2
# mfu profile: dp all_gathers cross PCIe (a pcie_bw ~26 host gave 11% MFU vs 37% at 53.8).
# Every image is cuda-13.2: cuda_max_good >= 13.2 (a 13.0 host answered create with HTTP 400).
MIN_PCIE_BW: Decimal | None = None
MIN_CUDA = Decimal("13.2")
# Bitwise replay is only claimed on drivers qualified by genuine CUDA CI (single source of truth).
_ci_spec = importlib.util.spec_from_file_location(
    "genuine_cuda_ci", Path(__file__).with_name("genuine_cuda_ci.py")
)
assert _ci_spec and _ci_spec.loader
_ci = importlib.util.module_from_spec(_ci_spec)
_ci_spec.loader.exec_module(_ci)
QUALIFIED_DRIVERS: tuple[str, ...] = tuple(_ci.DRIVERS)
ALLOWED_DRIVERS: tuple[str, ...] | None = QUALIFIED_DRIVERS  # None: --any-driver


def offer_query() -> dict[str, Any]:
    return {
        "gpu_name": {"eq": "RTX 5090"},
        "num_gpus": {"eq": NUM_GPUS},
        "verified": {"eq": True},
        "rentable": {"eq": True},
        "rented": {"eq": False},
        "reliability": {"gt": 0.98},
        "inet_down": {"gte": 1000},
        **({"pcie_bw": {"gte": float(MIN_PCIE_BW)}} if MIN_PCIE_BW is not None else {}),
        # No driver_version filter: the API answers 400 "driver_vers in None" (2026-10-19);
        # eligible() applies ALLOWED_DRIVERS to the returned offers.
        "type": "on-demand",
        "order": [["dph_total", "asc"]],
        "limit": 64,
    }


def eligible(o: Any) -> bool:
    """Client-side re-check: the server filter is not trusted."""
    return (
        isinstance(o, dict)
        and type(o.get("id")) is int
        and o.get("machine_id") not in EXCLUDED_MACHINES
        and o.get("gpu_name") == "RTX 5090"
        and o.get("num_gpus") == NUM_GPUS
        and o.get("verified") is not False
        and o.get("rentable") is not False
        and o.get("rented") is not True
        and (r := dec(o.get("reliability", o.get("reliability2")))) is not None
        and r > Decimal("0.98")
        and (d := dec(o.get("inet_down"))) is not None
        and d >= 1000
        and (MIN_PCIE_BW is None or ((b := dec(o.get("pcie_bw"))) is not None and b >= MIN_PCIE_BW))
        and (c := dec(o.get("cuda_max_good"))) is not None
        and c >= MIN_CUDA
        and (ALLOWED_DRIVERS is None or o.get("driver_version") in ALLOWED_DRIVERS)
        and dec(o.get("dph_total")) is not None
        and dec(o.get("storage_cost")) is not None
    )


def worst_case(offer: dict[str, Any], minutes: float, disk_gb: int = DISK_GB) -> Decimal:
    hours = Decimal(str(minutes)) / 60
    dph, storage = dec(offer["dph_total"]), dec(offer["storage_cost"])
    assert dph is not None and storage is not None
    return dph * hours + storage * disk_gb * hours / HOURS_PER_MONTH


def list_rows(r: Response) -> list[dict[str, Any]] | None:
    """Complete instance list or None (UNKNOWN: 429, malformed, paginated never mean absent)."""
    b = r.body if r.ok() else None
    if not isinstance(b, dict) or b.get("next_token"):
        return None
    rows = b.get("instances")
    if not isinstance(rows, list) or not all(
        isinstance(i, dict) and type(i.get("id")) is int for i in rows
    ):
        return None
    return rows


class Harness:
    # Profile hooks (subclasses such as scripts/vast_parity.py override these; defaults = e2e).
    label_prefix = "hypertrain-e2e"
    disk_gb = DISK_GB
    upload_paths: list[str] = UPLOAD
    rsync_excludes: list[str] = []

    @property
    def offer_desc(self) -> str:
        return f"{NUM_GPUS}x RTX 5090"

    def offer_query(self) -> dict[str, Any]:
        return offer_query()

    def eligible(self, o: Any) -> bool:
        return eligible(o)

    def rank(self, o: dict[str, Any]) -> tuple[Any, ...]:
        return (dec(o["dph_total"]), o["id"])

    def __init__(self, args: argparse.Namespace, api: Api) -> None:
        self.a, self.api = args, api
        self.out = Path(args.evidence_dir)
        self.label = f"{self.label_prefix}-{uuid.uuid4().hex[:8]}"
        self.instance_id: int | None = None
        self.create_attempted = False
        self.retired: list[tuple[int, str]] = []  # stuck hosts boot() deleted: (id, label)
        self.ssh: tuple[str, int] | None = None
        self.cancel = threading.Event()
        self.cleanup_lock = threading.Lock()
        self.cleaned = False
        self.result: dict[str, Any] = {"label": self.label}
        self.child: subprocess.Popen[bytes] | None = None
        self.deadline = time.time() + float(args.max_minutes) * 60 - float(args.cleanup_reserve)

    def admit(self) -> dict[str, Any]:
        acct = self.api.call("GET", "/api/v0/users/current/")
        credit = dec(acct.body.get("credit")) if acct.ok() else None
        if credit is None:
            raise Refused(f"account credit unknown (HTTP {acct.status})")
        need = Decimal(str(self.a.max_usd)) + 5
        self.api.log("credit", credit=str(credit), required=str(need))
        if credit < need:
            raise Refused(f"credit {credit} < max_usd + 5 = {need}")
        # Drivers are filtered client-side (the API refuses a driver filter), so the cheapest
        # page may hold no qualified offer: page on by price until one is eligible.
        query, offers, seen, capped = self.offer_query(), [], set(), False
        skip = set(self.a.exclude_offer)  # here, not in eligible(): profiles override that
        for page in range(OFFER_PAGES):
            q = urllib.parse.urlencode({"q": json.dumps(query)})
            r = self.api.call("GET", "/api/v0/bundles/?" + q)
            got = r.body.get("offers") if r.ok() else None
            if not isinstance(got, list):
                raise Refused(f"offer search failed (HTTP {r.status})")
            new = [o for o in got if not (isinstance(o, dict) and o.get("id") in seen)]
            seen |= {o.get("id") for o in got if isinstance(o, dict)}
            offers += new
            pool = [o for o in offers if not isinstance(o, dict) or o.get("id") not in skip]
            fit = [o for o in pool if self.eligible(o)]
            prices = [p for o in got if isinstance(o, dict) and (p := dec(o.get("dph_total")))]
            limit = query.get("limit")
            if fit or not new or not prices or limit is None or len(got) < limit:
                break
            capped = page == OFFER_PAGES - 1
            # gte, not gt: offers at the boundary price may continue on the next page.
            query = {**query, "dph_total": {"gte": float(max(prices))}}
        if not fit:
            alone = 0  # offers a qualified driver would have made eligible
            if ALLOWED_DRIVERS is not None:
                alone = sum(
                    isinstance(o, dict)
                    and any(self.eligible(o | {"driver_version": v}) for v in ALLOWED_DRIVERS)
                    for o in pool
                )
            raise Refused(
                f"no eligible {self.offer_desc} offer among {len(offers)} "
                f"({alone} rejected by driver allow-list)"
                + (
                    f"; search stopped after {OFFER_PAGES} pages of {query.get('limit')}: "
                    "cheaper-first market not exhausted"
                    if capped
                    else ""
                )
            )
        best = min(fit, key=self.rank)
        cost = worst_case(best, float(self.a.max_minutes), self.disk_gb)
        quote = {
            k: best.get(k)
            for k in (
                "id",
                "machine_id",
                "host_id",
                "dph_total",
                "storage_cost",
                "reliability",
                "inet_down",
                "driver_version",
                "geolocation",
                "gpu_name",
                "num_gpus",
                "cpu_name",
                "cpu_cores_effective",
                "cpu_ghz",
                "cpu_ram",
            )
        }
        quote["worst_case_usd"] = str(cost.quantize(Decimal("0.0001")))
        self.api.log("offer_chosen", seen=len(offers), eligible=len(fit), quote=quote)
        print(
            json.dumps(
                {
                    "chosen_offer": quote,
                    "max_minutes": self.a.max_minutes,
                    "max_usd": self.a.max_usd,
                    "credit": str(credit),
                },
                sort_keys=True,
            )
        )
        if cost > Decimal(str(self.a.max_usd)):
            raise Refused(f"worst case {cost:.4f} USD > max_usd {self.a.max_usd}")
        # Later attempts (after a host that never booted) take the next cheapest offers.
        self.fallbacks = [o for o in sorted(fit, key=self.rank) if o is not best]
        return best

    def create(self, offer: dict[str, Any]) -> None:
        if self.cancel.is_set():
            raise RuntimeError("cancelled before create")
        self.api.log("create_intent", offer_id=offer["id"], label=self.label, image=IMAGE)
        self.create_attempted = True
        r = self.api.call(
            "PUT",
            f"/api/v0/asks/{offer['id']}/",
            body={
                "client_id": "me",
                "image": IMAGE,
                "disk": self.disk_gb,
                "runtype": "ssh",
                "label": self.label,
                "target_state": "running",
                "cancel_unavail": True,
            },
        )
        new = r.body.get("new_contract") if r.ok() else None
        if r.ok() and r.body.get("success") is True and type(new) is int:
            self.instance_id = new
        else:
            self.instance_id = self.find_by_label()
        if self.instance_id is None:
            if r.status == 400 and self.instances() == []:
                raise OfferGone(f"offer {offer['id']} refused (HTTP 400), nothing created")
            raise RuntimeError(f"create unresolved (HTTP {r.status})")
        self.api.log("created", instance_id=self.instance_id)

    def instances(self) -> list[dict[str, Any]] | None:
        q = urllib.parse.urlencode({"select_filters": json.dumps({"label": {"eq": self.label}})})
        return list_rows(self.api.call("GET", "/api/v1/instances/?" + q))

    def find_by_label(self) -> int | None:
        for _ in range(3):
            rows = self.instances()
            if rows is not None:
                hits = [i["id"] for i in rows if i.get("label") == self.label]
                if len(hits) > 1:
                    raise RuntimeError("label ownership ambiguous")
                return hits[0] if hits else None
        return None

    def boot(self, offer: dict[str, Any]) -> None:
        """Create + wait for ssh; a vanished offer or a stuck host moves to the next offer."""
        tried: set[int | None] = set()
        for attempt in range(1 + int(self.a.boot_retries)):
            tried.add(offer.get("machine_id"))
            try:
                self.create(offer)
            except OfferGone:
                self.api.log("offer_gone", attempt=attempt, offer_id=offer["id"])
            else:
                real, self.deadline = (
                    self.deadline,
                    min(self.deadline, time.time() + float(self.a.boot_timeout)),
                )
                try:
                    self.wait_ready()
                    return
                except RuntimeError:
                    if time.time() >= real or self.cancel.is_set():
                        raise
                finally:
                    self.deadline = real
                self.api.log("boot_timeout", attempt=attempt, instance_id=self.instance_id)
                if not (self.delete() and self.confirm_absent()):
                    raise RuntimeError("stuck host not confirmed absent")
                assert self.instance_id is not None
                self.retired.append((self.instance_id, self.label))
            self.instance_id, self.create_attempted = None, False
            self.label = f"{self.label_prefix}-{uuid.uuid4().hex[:8]}"
            self.result["label"] = self.label
            nxt = [o for o in self.fallbacks if o.get("machine_id") not in tried]
            if not nxt:
                break
            offer = nxt[0]
            cost = worst_case(offer, float(self.a.max_minutes), self.disk_gb)
            if cost > Decimal(str(self.a.max_usd)):
                break
        raise RuntimeError("no host became ssh-ready")

    def wait_ready(self) -> None:
        key = self.identity().with_suffix(".pub").read_text().strip()
        attached, last_error = False, None
        while time.time() < self.deadline and not self.cancel.is_set():
            rows = self.instances()
            row = next((i for i in rows or [] if i["id"] == self.instance_id), None)
            if row is not None:
                if (
                    row.get("actual_status") in ("exited", "offline")
                    or row.get("cur_state") == "stopped"
                ):
                    raise RuntimeError(f"instance failed: {row.get('actual_status')}")
                if not attached:
                    r = self.api.call(
                        "POST", f"/api/v0/instances/{self.instance_id}/ssh/", body={"ssh_key": key}
                    )
                    attached = r.ok() and r.body.get("success") is not False
                host, port = row.get("ssh_host"), row.get("ssh_port")
                if (
                    attached
                    and row.get("actual_status") == "running"
                    and isinstance(host, str)
                    and type(port) is int
                ):
                    if self.ssh_probe(host, port):
                        self.ssh = (host, port)
                        self.api.log(
                            "ssh_ready", host=host, port=port, driver=row.get("driver_version")
                        )
                        return
                    err = getattr(self, "probe_error", "")
                    if err != last_error:
                        self.api.log("ssh_probe_failed", host=host, port=port, stderr=err)
                        last_error = err
                    if "Permission denied" in err:
                        attached = False  # key attached while loading can be lost at start
            self.api.sleep(max(5.0, self.api.min_gap))
        raise RuntimeError("instance not ssh-ready before deadline")

    def identity(self) -> Path:
        key = self.out / "id_ed25519"
        if not key.exists():
            subprocess.run(  # noqa: S603
                [  # noqa: S607
                    "ssh-keygen",
                    "-q",
                    "-t",
                    "ed25519",
                    "-N",
                    "",
                    "-C",
                    self.label,
                    "-f",
                    str(key),
                ],
                check=True,
                capture_output=True,
            )
        return key

    def ssh_argv(self, host: str, port: int) -> list[str]:
        return [
            "ssh",
            "-p",
            str(port),
            "-i",
            str(self.identity()),
            "-o",
            "IdentitiesOnly=yes",
            "-o",
            "BatchMode=yes",
            "-o",
            "StrictHostKeyChecking=accept-new",
            "-o",
            f"UserKnownHostsFile={self.out / 'known_hosts'}",
            "-o",
            "GlobalKnownHostsFile=/dev/null",
            "-o",
            "ConnectTimeout=20",
            "-o",
            "ServerAliveInterval=15",
            f"root@{host}",
        ]

    def sh(
        self,
        command: str,
        *,
        stdin: bytes | None = None,
        timeout: float,
        stdout_path: Path | None = None,
    ) -> int:
        """Run a command on the host; killable by the watchdog via self.child."""
        assert self.ssh is not None
        argv = [*self.ssh_argv(*self.ssh), command]
        log = (self.out / "remote.log").open("ab")
        sink = stdout_path.open("wb") if stdout_path else log
        try:
            self.child = subprocess.Popen(  # noqa: S603
                argv,
                stdin=subprocess.PIPE if stdin else None,
                stdout=sink,
                stderr=log,
            )
            if self.cancel.is_set() and not self.cleaned:  # signal landed before self.child
                self.child.kill()
            try:
                self.child.communicate(stdin, timeout=max(1.0, timeout))
            except subprocess.TimeoutExpired:
                self.child.kill()
                self.child.wait()
            return self.child.returncode
        finally:
            self.child = None
            log.close()
            if stdout_path:
                sink.close()

    def ssh_probe(self, host: str, port: int) -> bool:
        p = subprocess.run(  # noqa: S603
            [*self.ssh_argv(host, port), "true"],
            capture_output=True,
            timeout=60,
            check=False,
        )
        self.probe_error = "" if p.returncode == 0 else p.stderr.decode(errors="replace")[-300:]
        return p.returncode == 0

    def upload(self) -> int:
        assert self.ssh is not None
        if (
            self.sh(
                "mkdir -p /workspace/ht /workspace/out && (command -v rsync >/dev/null || "
                "(apt-get -qq update && apt-get -qq install -y rsync >/dev/null))",
                timeout=300,
            )
            != 0
        ):
            raise RuntimeError("remote rsync unavailable")
        ssh = shlex.join(self.ssh_argv(*self.ssh)[:-1])
        sources = [str(ROOT / p) for p in self.upload_paths if (ROOT / p).exists()]
        argv = [
            "rsync",
            "-az",
            "--delete",
            "--exclude=__pycache__",
            *(f"--exclude={x}" for x in self.rsync_excludes),
            "-e",
            ssh,
            *sources,
            f"root@{self.ssh[0]}:/workspace/ht/",
        ]
        rc = subprocess.run(argv, capture_output=True, timeout=600, check=False).returncode  # noqa: S603
        self.api.log("upload", rc=rc, sources=[Path(x).name for x in sources])
        return rc

    def run_remote(self) -> int:
        env = [shlex.quote(kv) for kv in self.a.remote_env]
        script = shlex.quote(f"/workspace/ht/scripts/{self.a.remote_script}")
        return self.sh(" ".join(["env", *env, "bash", script]), timeout=self.deadline - time.time())

    def rescue(self) -> int:
        dest = self.out / "out"
        dest.mkdir(parents=True, exist_ok=True)
        archive = self.out / "out.tar.gz"
        rc = self.sh("tar -C /workspace/out -czf - .", timeout=300, stdout_path=archive)
        if rc == 0:
            rc = subprocess.run(  # noqa: S603
                ["tar", "-xzf", str(archive), "-C", str(dest)],  # noqa: S607
                check=False,
            ).returncode
        return rc

    def cleanup(self, reason: str) -> None:
        with self.cleanup_lock:
            if self.cleaned:
                return
            self.cleaned = True
            self.cancel.set()
            self.api.log("cleanup_start", reason=reason, instance_id=self.instance_id)
            if self.child is not None and self.child.poll() is None:
                self.child.kill()
            if self.ssh is not None:
                try:
                    rc = self.rescue()
                except Exception as e:  # rescue failure must not block delete
                    rc, self.result["rescue_error"] = -1, repr(e)[:300]
                self.result["rescue_rc"] = rc
                self.api.log("rescued", rc=rc)
            if self.instance_id is None and self.create_attempted:
                self.instance_id = self.find_by_label()
            if self.instance_id is None and not self.create_attempted:
                self.result["absent"] = self.retired_absent()
                if self.retired:  # every host was stuck: still end on a parsed absence
                    self.result["deleted"] = True
                    self.api.log("cleanup_done", deleted=True, absent=self.result["absent"])
                return
            self.result["deleted"] = self.delete()
            self.result["absent"] = self.confirm_absent() and self.retired_absent()
            self.api.log(
                "cleanup_done", deleted=self.result["deleted"], absent=self.result["absent"]
            )

    def delete(self) -> bool:
        if self.instance_id is None:
            return False
        for attempt in range(int(self.a.delete_attempts)):
            r = self.api.call("DELETE", f"/api/v0/instances/{self.instance_id}/")
            if r.ok() and r.body.get("success") is not False:
                return True
            self.api.log("delete_retry", attempt=attempt, status=r.status)
            self.api.sleep(min(60.0, 5.0 * 2**attempt))
        return False

    def confirm_absent(self, iid: int | None = None, label: str | None = None) -> bool:
        iid, label = iid or self.instance_id, label or self.label
        q = urllib.parse.urlencode({"select_filters": json.dumps({"label": {"eq": label}})})
        for _ in range(int(self.a.absence_attempts)):
            rows = list_rows(self.api.call("GET", "/api/v1/instances/?" + q))
            if rows is not None and not any(
                i["id"] == iid or i.get("label") == label for i in rows
            ):
                return True
            self.api.sleep(max(5.0, self.api.min_gap))
        return False

    def retired_absent(self) -> bool:
        """Re-list each host boot() deleted: its absence was seen once, the run proves it last."""
        return all([self.confirm_absent(i, label) for i, label in self.retired])  # list: check all

    def watchdog(self) -> None:
        if not self.cancel.wait(max(0.0, self.deadline - time.time())):
            self.api.log("watchdog_fired", deadline=self.deadline)
            self.result["watchdog"] = True
            self.cleanup("watchdog")

    def run(self) -> int:
        offer = self.admit()
        if self.a.dry_run:
            return 0
        dog = threading.Thread(target=self.watchdog, daemon=True)
        dog.start()
        rc = 1
        try:
            self.boot(offer)
            if self.upload() != 0:
                raise RuntimeError("upload failed")
            rc = self.run_remote()
            self.result["remote_rc"] = rc
        except Exception as e:
            self.result["error"] = repr(e)[:500]
            self.api.log("run_error", error=self.result["error"])
            rc = 1
        finally:
            self.cleanup("finally")
            dog.join(timeout=5)
            self.result["remote_ok"] = rc == 0 and not ({"watchdog", "signal"} & self.result.keys())
            (self.out / "harness-result.json").write_text(json.dumps(self.result, indent=2))
        if not self.result.get("absent"):
            print(f"ALERT: instance {self.instance_id} absence NOT confirmed", file=sys.stderr)
            return 3
        return 0 if self.result["remote_ok"] else 1


def parse(argv: list[str] | None = None) -> argparse.Namespace:
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--key-file", type=Path, required=True)
    p.add_argument("--max-usd", type=float, required=True)
    p.add_argument("--max-minutes", type=float, required=True)
    p.add_argument("--dry-run", action="store_true")
    p.add_argument("--api-base", default="https://console.vast.ai")
    p.add_argument("--evidence-dir", type=Path, default=ROOT / ".evidence" / "vast")
    p.add_argument(
        "--cleanup-reserve",
        type=float,
        default=600.0,
        help="seconds before the wall deadline at which the watchdog fires",
    )
    p.add_argument("--min-gap", type=float, default=5.0, help="seconds between provider calls")
    p.add_argument(
        "--boot-timeout",
        type=float,
        default=900.0,
        help="seconds a host may take to become ssh-ready before it is replaced",
    )
    p.add_argument("--boot-retries", type=int, default=2, help="replacement hosts to try")
    p.add_argument(
        "--exclude-offer",
        type=int,
        action="append",
        default=[],
        metavar="ID",
        help="never rent this offer id (repeatable), e.g. a host whose ssh never accepted the key",
    )
    p.add_argument("--delete-attempts", type=int, default=6)
    p.add_argument("--absence-attempts", type=int, default=12)
    p.add_argument(
        "--profile",
        choices=("gpu-e2e", "miner-flow", "mfu", "miner-cycle", "gpu8"),
        default="gpu-e2e",
        help="gpu-e2e: --num-gpus x RTX 5090, pinned GPU image, scripts/vast_e2e_remote.sh; "
        "gpu8: gpu-e2e on 8x RTX 5090; "
        "miner-flow: 1x RTX 5090, vastai/pytorch image, scripts/vast_miner_flow_remote.sh; "
        "miner-cycle: --num-gpus (default 1) x RTX 5090, vastai/pytorch image, "
        "scripts/vast_miner_cycle_remote.sh; "
        "mfu: --num-gpus x RTX 5090, vastai/pytorch image, pcie_bw >= --min-pcie-bw, "
        "scripts/vast_mfu_remote.sh",
    )
    p.add_argument(
        "--num-gpus",
        type=int,
        choices=(1, 2, 4, 8),
        help="GPUs per host, every profile; default 2 (miner-flow, miner-cycle: 1; gpu8: 8)",
    )
    drivers = p.add_mutually_exclusive_group()
    drivers.add_argument(
        "--driver",
        action="append",
        metavar="VERSION",
        help="only this driver_version (repeatable); must be in scripts/genuine_cuda_ci.py DRIVERS",
    )
    drivers.add_argument(
        "--any-driver",
        action="store_true",
        help="no driver_version filter; for --profile mfu, where bitwise replay is not compared",
    )
    p.add_argument("--min-pcie-bw", type=float, default=50.0, help="mfu profile, GB/s")
    p.add_argument("--remote-env", action="append", default=[], metavar="KEY=VAL")
    a = p.parse_args(argv)
    a.remote_script = {
        "miner-flow": "vast_miner_flow_remote.sh",
        "mfu": "vast_mfu_remote.sh",
        "miner-cycle": "vast_miner_cycle_remote.sh",
    }.get(a.profile, "vast_e2e_remote.sh")
    gpus = {"miner-flow": 1, "miner-cycle": 1, "gpu8": 8}.get(a.profile, 2)
    if a.profile in ("miner-flow", "gpu8") and a.num_gpus not in (None, gpus):
        p.error(f"--profile {a.profile} supports only --num-gpus {gpus}")
    a.num_gpus = a.num_gpus or gpus
    if a.remote_script == "vast_e2e_remote.sh":
        # Appended: env(1) keeps the last KEY=VAL, so --remote-env cannot contradict the host.
        a.remote_env = [*a.remote_env, f"HT_E2E_GPUS={a.num_gpus}"]
    if unknown := sorted(set(a.driver or ()) - set(QUALIFIED_DRIVERS)):
        p.error(f"--driver {', '.join(unknown)} not in scripts/genuine_cuda_ci.py DRIVERS")
    if any("=" not in kv or not kv.partition("=")[0].isidentifier() for kv in a.remote_env):
        p.error("--remote-env must be KEY=VAL with an identifier KEY")
    if (
        a.max_usd <= 0
        or a.max_minutes <= 0
        or a.min_gap < 5.0
        and a.api_base.startswith("https://console.vast.ai")
    ):
        p.error("positive budget/minutes and >=5s spacing against the real API required")
    return a


def main(
    argv: list[str] | None = None,
    harness: type[Harness] = Harness,
    sleep: Callable[[float], None] = time.sleep,
) -> int:
    a = parse(argv)
    a.evidence_dir.mkdir(parents=True, exist_ok=True)
    key = a.key_file.read_text().strip()
    if not key:
        print("empty key file", file=sys.stderr)
        return 2
    global IMAGE, NUM_GPUS, MIN_PCIE_BW, ALLOWED_DRIVERS
    NUM_GPUS = a.num_gpus
    ALLOWED_DRIVERS = None if a.any_driver else tuple(a.driver or QUALIFIED_DRIVERS)
    if a.profile in ("miner-flow", "miner-cycle", "mfu"):
        IMAGE = MINER_FLOW_IMAGE
    if a.profile == "mfu":
        MIN_PCIE_BW = Decimal(str(a.min_pcie_bw))
    api = Api(a.api_base, key, a.evidence_dir / "journal.jsonl", min_gap=a.min_gap, sleep=sleep)
    h = harness(a, api)

    def stop(signum: int, _frame: Any) -> None:
        # A service stop/restart: cancel like the watchdog does, never raise, so a signal can
        # not abort a rescue/delete under way. No api.log: this thread may hold its lock.
        h.result["signal"] = signum
        h.cancel.set()
        child = h.child
        if not h.cleaned and child is not None and child.poll() is None:
            child.kill()  # once cleanup runs, the child is the rescue: let it finish

    saved = {s: signal.signal(s, stop) for s in (signal.SIGTERM, signal.SIGHUP)}
    api.log(
        "start",
        dry_run=a.dry_run,
        max_usd=a.max_usd,
        max_minutes=a.max_minutes,
        argv=shlex.join(sys.argv[1:] if argv is None else argv),
        pid=os.getpid(),
    )
    try:
        return h.run()
    except Refused as e:
        api.log("refused", reason=str(e))
        print(f"REFUSED: {e}", file=sys.stderr)
        return 2
    finally:
        for s, old in saved.items():
            signal.signal(s, old)


if __name__ == "__main__":
    sys.exit(main())
