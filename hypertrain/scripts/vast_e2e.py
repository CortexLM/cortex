"""One-host Vast.ai end-to-end harness for Hypertrain (2x RTX 5090). Stdlib only.

Flow: credit check -> offer search -> worst-case cost admission -> ONE create -> ssh ready ->
upload current tree -> scripts/vast_e2e_remote.sh -> rescue /workspace/out -> DELETE -> parsed
absence. A watchdog thread fires at the wall deadline (minus the cleanup reserve) and runs the
same rescue+delete. Every provider call is journaled. `--dry-run` stops after the cost calc.
The API key is read from --key-file and never printed or journaled.
"""

from __future__ import annotations

import argparse
import json
import os
import shlex
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
EVIDENCE = Path(
    "/root/distributed-decision-training/.omo/evidence/hypertrain-complete-20261010/vast"
)
ROOT = Path(__file__).resolve().parents[1]
UPLOAD = ["src", "tests", "pyproject.toml", "uv.lock", "scripts", "README.md", "LICENSE"]
DISK_GB = 40
EXCLUDED_MACHINES = (137465, 43459)
HOURS_PER_MONTH = Decimal(720)


class Refused(Exception):
    """Admission refusal: nothing was created."""


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


def offer_query() -> dict[str, Any]:
    return {
        "gpu_name": {"eq": "RTX 5090"},
        "num_gpus": {"eq": 2},
        "verified": {"eq": True},
        "rentable": {"eq": True},
        "rented": {"eq": False},
        "reliability": {"gt": 0.98},
        "inet_down": {"gte": 500},
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
        and o.get("num_gpus") == 2
        and o.get("verified") is not False
        and o.get("rentable") is not False
        and o.get("rented") is not True
        and (r := dec(o.get("reliability", o.get("reliability2")))) is not None
        and r > Decimal("0.98")
        and (d := dec(o.get("inet_down"))) is not None
        and d >= 500
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
    def __init__(self, args: argparse.Namespace, api: Api) -> None:
        self.a, self.api = args, api
        self.out = Path(args.evidence_dir)
        self.label = f"hypertrain-e2e-{uuid.uuid4().hex[:8]}"
        self.instance_id: int | None = None
        self.create_attempted = False
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
        q = urllib.parse.urlencode({"q": json.dumps(offer_query())})
        r = self.api.call("GET", "/api/v0/bundles/?" + q)
        offers = r.body.get("offers") if r.ok() else None
        if not isinstance(offers, list):
            raise Refused(f"offer search failed (HTTP {r.status})")
        fit = [o for o in offers if eligible(o)]
        if not fit:
            raise Refused(f"no eligible 2x RTX 5090 offer among {len(offers)}")
        best = min(fit, key=lambda o: (dec(o["dph_total"]), o["id"]))
        cost = worst_case(best, float(self.a.max_minutes))
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
        return best

    def create(self, offer: dict[str, Any]) -> None:
        self.api.log("create_intent", offer_id=offer["id"], label=self.label, image=IMAGE)
        self.create_attempted = True
        r = self.api.call(
            "PUT",
            f"/api/v0/asks/{offer['id']}/",
            body={
                "client_id": "me",
                "image": IMAGE,
                "disk": DISK_GB,
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

    def wait_ready(self) -> None:
        key = self.identity().with_suffix(".pub").read_text().strip()
        attached = False
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
        sources = [str(ROOT / p) for p in UPLOAD if (ROOT / p).exists()]
        argv = [
            "rsync",
            "-az",
            "--delete",
            "--exclude=__pycache__",
            "-e",
            ssh,
            *sources,
            f"root@{self.ssh[0]}:/workspace/ht/",
        ]
        rc = subprocess.run(argv, capture_output=True, timeout=600, check=False).returncode  # noqa: S603
        self.api.log("upload", rc=rc, sources=[Path(x).name for x in sources])
        return rc

    def run_remote(self) -> int:
        return self.sh(
            "bash /workspace/ht/scripts/vast_e2e_remote.sh", timeout=self.deadline - time.time()
        )

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
                self.result["absent"] = True
                return
            self.result["deleted"] = self.delete()
            self.result["absent"] = self.confirm_absent()
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

    def confirm_absent(self) -> bool:
        for _ in range(int(self.a.absence_attempts)):
            rows = self.instances()
            if rows is not None and not any(
                i["id"] == self.instance_id or i.get("label") == self.label for i in rows
            ):
                return True
            self.api.sleep(max(5.0, self.api.min_gap))
        return False

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
            self.create(offer)
            self.wait_ready()
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
            self.result["remote_ok"] = rc == 0 and not self.result.get("watchdog")
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
    p.add_argument("--evidence-dir", type=Path, default=EVIDENCE)
    p.add_argument(
        "--cleanup-reserve",
        type=float,
        default=600.0,
        help="seconds before the wall deadline at which the watchdog fires",
    )
    p.add_argument("--min-gap", type=float, default=5.0, help="seconds between provider calls")
    p.add_argument("--delete-attempts", type=int, default=6)
    p.add_argument("--absence-attempts", type=int, default=12)
    a = p.parse_args(argv)
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
    api = Api(a.api_base, key, a.evidence_dir / "journal.jsonl", min_gap=a.min_gap, sleep=sleep)
    api.log(
        "start",
        dry_run=a.dry_run,
        max_usd=a.max_usd,
        max_minutes=a.max_minutes,
        argv=shlex.join(sys.argv[1:] if argv is None else argv),
        pid=os.getpid(),
    )
    try:
        return harness(a, api).run()
    except Refused as e:
        api.log("refused", reason=str(e))
        print(f"REFUSED: {e}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
