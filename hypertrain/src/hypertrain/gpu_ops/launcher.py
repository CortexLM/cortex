"""Phase A launcher: journal-resumable, one CREATE per host, rescue-before-DELETE, parsed absence.

Usage: python -m hypertrain.gpu_ops.launcher run --config CFG.json --run-dir DIR [--live]
Provider writes are refused unless base_url is loopback (mock) or --live is passed.
"""

from __future__ import annotations

import argparse
import io
import json
import os
import select
import shlex
import subprocess
import sys
import tarfile
import time
import urllib.parse
import uuid
from decimal import Decimal
from pathlib import Path
from typing import Any

from hypertrain.gpu_ops import budget
from hypertrain.gpu_ops.journal import Journal, Record, durable_write, flock, fsha, sha256
from hypertrain.gpu_ops.provider import Provider, Response, list_rows, loopback, read_api_key
from hypertrain.gpu_ops.remote import Ssh, ensure_identity
from hypertrain.gpu_ops.verdict import compute_verdict

EXIT = {"ok": 0, "admission_rejected": 2, "failed_cleaned": 3, "liability_open": 4, "refused": 5}
MIN_PUT_SPACING = 5.0
PUT_ENDPOINT = "PUT /api/v0/asks/N/"
REMOTE_SHARD = "data/phase-a/shard.u32"


class Reject(Exception):
    pass


def crash_hook(point: str, base: str) -> None:
    if loopback(base) and os.environ.get("HT_GPU_CRASH_AT") == point:
        os._exit(137)


class Orchestrator:
    def __init__(
        self, cfg: dict[str, Any], j: Journal, p: Provider, run_dir: Path, actor: str = "launcher"
    ) -> None:
        self.cfg, self.j, self.p, self.dir, self.actor = cfg, j, p, run_dir, actor
        self.base = cfg["base_url"]
        self.roles = [h["role"] for h in cfg["hosts"]]
        self.host = {h["role"]: h for h in cfg["hosts"]}
        self.poll = float(cfg.get("poll_seconds", 15))
        spacing = max(MIN_PUT_SPACING, float(cfg.get("create_min_interval_seconds", 5)))
        p.pace.update(
            {PUT_ENDPOINT: spacing, "*": float(cfg.get("provider_min_interval_seconds", 1))}
        )
        puts = j.all("provider_call", method="PUT")
        if puts:
            p.last[PUT_ENDPOINT] = time.monotonic() - max(0.0, time.time() - puts[-1]["unix"])
        (run_dir / "logs").mkdir(mode=0o700, exist_ok=True)

    def admitted(self) -> Record | None:
        return self.j.last("admitted")

    def deadline(self) -> float:
        rec = self.admitted()
        assert rec is not None
        return float(rec["deadline_unix"])

    def receipt(self, role: str) -> Record | None:
        return self.j.last("receipt", role=role)

    def cleanup_started(self) -> bool:
        return self.j.last("supervisor_cleanup_started") is not None

    def sleep(self, seconds: float) -> None:
        self.p.sleep(seconds)

    def offer(self, role: str, tag: str) -> dict[str, Any]:
        h = self.host[role]
        q = {
            "machine_id": {"eq": h["machine_id"]},
            "rentable": {"eq": True},
            "rented": {"eq": False},
        }
        r = self.p.call(
            "GET", "/api/v0/bundles/?" + urllib.parse.urlencode({"q": json.dumps(q)}), tag
        )
        offers = r.parsed.get("offers") if r.ok() else None
        if not isinstance(offers, list):
            raise Reject(f"offer_unknown:{role}:HTTP{r.status}")
        cap = Decimal(str(h["max_dph_total"]))
        fit = [
            o
            for o in offers
            if isinstance(o, dict)
            and o.get("machine_id") == h["machine_id"]
            and o.get("gpu_name") == "RTX 5090"
            and o.get("num_gpus") == 1
            and type(o.get("id")) is int
            and (d := budget.dec(o.get("dph_total"))) is not None
            and d <= cap
        ]
        self.j.append(
            "offer_resolved", role=role, seen=len(offers), eligible=[o["id"] for o in fit]
        )
        if not fit:
            raise Reject(f"offer_gone:{role}")
        return min(fit, key=lambda o: (Decimal(str(o["dph_total"])), o["id"]))

    def inventory(self, tag: str) -> list[dict[str, Any]] | None:
        q = urllib.parse.urlencode(
            {"limit": 25, "select_cols": json.dumps(["id", "label", "machine_id"])}
        )
        return list_rows(self.p.call("GET", "/api/v1/instances/?" + q, tag))

    def admit(self) -> None:
        cfg = self.cfg
        if len({h["machine_id"] for h in cfg["hosts"]}) != len(cfg["hosts"]):
            raise Reject("machine_ids_not_distinct")
        snap = json.loads(Path(cfg["budget_snapshot"]).read_text())
        acct = self.p.call("GET", "/api/v0/users/current/", "admit-account")
        credit = budget.dec(acct.parsed.get("credit")) if acct.ok() else None
        if credit is None or acct.parsed.get("id") != snap["account_id"]:
            raise Reject("account_unverified")
        rows = self.inventory("admit-inventory")
        if rows is None:
            raise Reject("inventory_unknown")
        if rows and not cfg.get("allow_existing_instances"):
            raise Reject("existing_instances_present")
        offers = [self.offer(r, "admit-offer-" + r) for r in self.roles]
        plan = budget.evaluate(
            snapshot0_credit=Decimal(str(snap["credit_usd"])),
            current_credit=credit,
            offers=offers,
            hard_deadline_seconds=int(cfg["hard_deadline_seconds"])
            + int(cfg.get("hard_grace_seconds", 1800)),
            disk_gb=int(cfg["disk_gb"]),
            egress_gb=int(cfg.get("egress_gb", 2)),
            phase_cap=Decimal(str(cfg.get("phase_cap_usd", budget.PHASE_A_CAP_USD))),
        )
        quotes = {
            r: {
                k: o.get(k)
                for k in (
                    "id",
                    "machine_id",
                    "host_id",
                    "dph_total",
                    "storage_cost",
                    "inet_up_cost",
                    "inet_down_cost",
                    "driver_version",
                    "geolocation",
                )
            }
            for r, o in zip(self.roles, offers, strict=True)
        }
        if not plan["admit"]:
            self.j.append("admission_rejected", reason="budget", plan=plan, quotes=quotes)
            raise Reject("over_cap:" + ",".join(k for k, v in plan["checks"].items() if not v))
        self.j.append(
            "admitted",
            deadline_unix=time.time() + int(cfg["hard_deadline_seconds"]),
            plan=plan,
            quotes=quotes,
            baseline_ids=sorted(i["id"] for i in rows),
            account_id=snap["account_id"],
        )

    def ensure_supervisor(self) -> Record:
        try:
            with flock(self.dir / "supervisor.lock", blocking=False):
                pass
        except BlockingIOError:
            ready = self.j.last("supervisor_ready")
            assert ready is not None
            return ready
        err = open(self.dir / "supervisor.stderr", "ab")
        proc = subprocess.Popen(  # noqa: S603  (this interpreter + own module)
            [sys.executable, "-B", "-m", "hypertrain.gpu_ops.supervisor", str(self.dir)],
            stdout=subprocess.PIPE,
            stderr=err,
            stdin=subprocess.DEVNULL,
            start_new_session=True,
            env=os.environ.copy(),
        )
        err.close()
        assert proc.stdout is not None
        if not select.select([proc.stdout], [], [], 60)[0]:
            raise Reject("supervisor_not_ready")
        line = proc.stdout.readline().decode().strip()
        proc.stdout.close()
        ready = self.j.last("supervisor_ready")
        if line != "READY" or not ready or ready["supervisor_pid"] != proc.pid:
            raise Reject("supervisor_not_ready:" + line[:100])
        return ready

    def recover(self, intent: Record, attempts: int = 6) -> tuple[str, int | None]:
        """('found', id) | ('absent', None) | ('unknown', None). 429/malformed lists are unknown."""
        q = urllib.parse.urlencode(
            {
                "limit": 25,
                "select_cols": json.dumps(["id", "label", "machine_id"]),
                "select_filters": json.dumps({"label": {"eq": intent["label"]}}),
            }
        )
        for _ in range(attempts):
            r = self.p.call("GET", "/api/v1/instances/?" + q, "recover-" + intent["role"])
            rows = list_rows(r)
            if rows is not None:
                hits = [
                    i["id"]
                    for i in rows
                    if i.get("label") == intent["label"] and i["id"] not in intent["baseline_ids"]
                ]
                if len(hits) > 1:
                    raise Reject("ownership_ambiguous:" + intent["role"])
                return ("found", hits[0]) if hits else ("absent", None)
            self.j.append("recover_unknown", role=intent["role"], http_status=r.status)
            self.sleep(r.retry_after if r.retry_after is not None else self.poll)
        return ("unknown", None)

    def create(self, role: str) -> Record:
        rec = self.receipt(role)
        if rec:
            return rec
        intent = self.j.last("create_intent", role=role)
        if intent:
            state, iid = self.recover(intent)
            if state == "found":
                return self.j.append(
                    "receipt", role=role, instance_id=iid, source="recovered", label=intent["label"]
                )
            self.j.append("create_unresolved", role=role, state=state)
            raise Reject(f"create_unresolved_no_retry:{role}:{state}")
        h = self.host[role]
        with flock(self.dir / "create.lock"):
            if self.cleanup_started():
                raise Reject("supervisor_cleanup_started")
            if time.time() > self.deadline() - float(self.cfg.get("create_margin_seconds", 1800)):
                raise Reject("deadline_too_close_for_create")
            admitted = self.admitted()
            assert admitted is not None
            label = f"ht-phase-a-{self.dir.name[-10:]}-{role}-{uuid.uuid4().hex[:10]}"
            offer_id = admitted["quotes"][role]["id"]
            intent = self.j.append(
                "create_intent",
                role=role,
                offer_id=offer_id,
                machine_id=h["machine_id"],
                label=label,
                baseline_ids=admitted["baseline_ids"],
                image=self.cfg["image"],
            )
            crash_hook("before_put:" + role, self.base)
            r = self.p.call(
                "PUT",
                f"/api/v0/asks/{offer_id}/",
                "put-" + role,
                body={
                    "image": self.cfg["image"],
                    "disk": int(self.cfg["disk_gb"]),
                    "runtype": "ssh",
                    "target_state": "running",
                    "cancel_unavail": True,
                    "label": label,
                    "onstart": "true",
                },
            )
            crash_hook("after_put:" + role, self.base)
            self.j.append("create_response", role=role, http_status=r.status, raw_file=r.raw_file)
            new = r.parsed.get("new_contract") if r.ok() else None
            if r.ok() and r.parsed.get("success") is True and type(new) is int:
                return self.j.append(
                    "receipt", role=role, instance_id=new, source="PUT_response", label=label
                )
            if r.status == 429 and r.retry_after is not None:
                self.sleep(r.retry_after)
            state, iid = self.recover(intent)
            if state == "found":
                return self.j.append(
                    "receipt", role=role, instance_id=iid, source="recovered_after_PUT", label=label
                )
            self.j.append(
                "create_failed" if state == "absent" else "liability_unresolved",
                role=role,
                http_status=r.status,
            )
            raise Reject(f"create_failed_no_retry:{role}:HTTP{r.status}:{state}")

    def show(self, iid: int, tag: str) -> tuple[Response, Any]:
        r = self.p.call("GET", f"/api/v0/instances/{iid}/?owner=me", tag)
        return r, (r.parsed.get("instances", ...) if r.ok() else ...)

    def boot(self, role: str) -> Record:
        ready = self.j.last("instance_ready", role=role)
        if ready:
            return ready
        rec = self.receipt(role)
        assert rec is not None
        limit = min(
            time.time() + float(self.cfg.get("boot_timeout_seconds", 1800)), self.deadline()
        )
        while time.time() < limit:
            if self.cleanup_started():
                raise Reject("supervisor_cleanup_started")
            r, st = self.show(rec["instance_id"], "show-" + role)
            if isinstance(st, dict):
                if st.get("id") != rec["instance_id"] or st.get("label") != rec["label"]:
                    raise Reject("receipt_status_mismatch:" + role)
                if (
                    st.get("actual_status") == "running"
                    and st.get("ssh_host")
                    and st.get("ssh_port")
                ):
                    return self.j.append(
                        "instance_ready",
                        role=role,
                        instance_id=rec["instance_id"],
                        ssh_host=st["ssh_host"],
                        ssh_port=st["ssh_port"],
                        machine_id=st.get("machine_id"),
                        driver_version=st.get("driver_version"),
                    )
                if (
                    st.get("actual_status") in ("exited", "offline")
                    or st.get("cur_state") == "stopped"
                ):
                    raise Reject("instance_failed:" + role)
            self.sleep(r.retry_after if r.retry_after is not None else self.poll)
        raise Reject("boot_timeout:" + role)

    def attach(self, role: str) -> None:
        if self.j.last("ssh_attached", role=role):
            return
        rec = self.receipt(role)
        assert rec is not None
        pub = (ensure_identity(self.dir).with_suffix(".pub")).read_text().strip()
        r = self.p.call(
            "POST",
            f"/api/v0/instances/{rec['instance_id']}/ssh/",
            "attach-" + role,
            body={"ssh_key": pub},
        )
        if not r.ok() or r.parsed.get("success") is not True:
            raise Reject(f"ssh_attach_failed:{role}:HTTP{r.status}")
        self.j.append("ssh_attached", role=role, instance_id=rec["instance_id"])

    def ssh(self, role: str) -> Ssh:
        st = self.j.last("instance_ready", role=role)
        assert st is not None
        return Ssh(
            host=st["ssh_host"],
            port=st["ssh_port"],
            user=self.cfg.get("ssh_user", "root"),
            identity=ensure_identity(self.dir),
            known_hosts=self.dir / f"known_hosts-{role}",
            journal=self.j,
            role=role,
            timeout=int(self.cfg.get("ssh_timeout_seconds", 3600)),
            sleep=self.p.sleep,
            retry_seconds=self.poll,
        )

    def trust(self, role: str) -> None:
        if self.j.last("ssh_trusted", role=role) and (self.dir / f"known_hosts-{role}").exists():
            self.ssh(role).trust()
            return
        mode = self.ssh(role).trust(attempts=int(self.cfg.get("keyscan_attempts", 10)))
        self.j.append(
            "ssh_trusted",
            role=role,
            mode=mode,
            known_hosts_sha256=fsha(self.dir / f"known_hosts-{role}"),
        )

    def remote_root(self, role: str) -> str:
        return str(self.cfg["remote_root"]).format(role=role)

    def stage_tar(self) -> Path:
        out = self.dir / "stage.tar"
        if out.exists():
            return out
        tree = Path(self.cfg["tree"])
        buf = io.BytesIO()
        with tarfile.open(fileobj=buf, mode="w") as tar:
            for rel, src in sorted(stage_files(tree, self.cfg).items()):
                data = src.read_bytes()
                info = tarfile.TarInfo(rel)
                info.size, info.mode, info.mtime = len(data), 0o644, 0
                tar.addfile(info, io.BytesIO(data))
        durable_write(out, buf.getvalue())
        self.j.append("stage_built", tar_sha256=fsha(out), files=len(stage_files(tree, self.cfg)))
        return out

    def stage(self, role: str) -> None:
        if self.j.last("staged", role=role):
            return
        tar, ssh, root = self.stage_tar(), self.ssh(role), self.remote_root(role)
        logs = self.dir / "logs"
        q = shlex.quote
        if ssh.run(f"mkdir -p {q(root)}/out", "mkdir", logs).returncode:
            raise Reject("stage_mkdir:" + role)
        ssh.put(tar, f"{root}/stage.tar")
        want = fsha(tar)
        if ssh.remote_sha256(f"{root}/stage.tar", logs) != want:
            raise Reject("stage_sha_mismatch:" + role)
        if ssh.run(
            f"cd {q(root)} && tar -xf stage.tar --no-same-owner", "extract", logs
        ).returncode:
            raise Reject("stage_extract:" + role)
        setup = self.cfg.get("setup_command", "")
        if setup and ssh.run(f"cd {q(root)} && {setup}", "setup", logs).returncode:
            raise Reject("stage_setup:" + role)
        self.j.append("staged", role=role, tar_sha256=want)

    def jobs(self) -> list[tuple[str, str, bool]]:
        out = []
        for i, role in enumerate(self.roles):
            for n in range(int(self.cfg.get("runs_per_host", 2))):
                out.append((role, f"run{n + 1}", False))
            if i == 0:
                out.append((role, "neg", True))
        return out

    def run_job(self, role: str, name: str, negative: bool) -> None:
        if self.j.last("job_done", role=role, name=name):
            return
        if time.time() > self.deadline() - float(self.cfg.get("cleanup_margin_seconds", 900)):
            raise Reject("deadline_margin_reached")
        if self.cleanup_started():
            raise Reject("supervisor_cleanup_started")
        root, q = self.remote_root(role), shlex.quote
        argv = [
            "env",
            "CUBLAS_WORKSPACE_CONFIG=:4096:8",
            "CUDA_DISABLE_PTX_JIT=1",
            "PYTHONPATH=src",
            "PYTHONDONTWRITEBYTECODE=1",
            self.cfg.get("remote_python", "python3"),
            "experiments/gpu_phase_a/run_phase_a.py",
            "--profile",
            self.cfg["profile"],
            "--device",
            self.cfg.get("device", "cuda"),
            "--out",
            f"out/{name}.json",
            "--shard",
            REMOTE_SHARD,
            "--shard-sha256",
            fsha(shard_path(self.cfg)),
        ] + (["--negative"] if negative else [])
        if "@" in self.cfg["image"]:
            argv += ["--image-digest", self.cfg["image"].rsplit("@", 1)[1]]
        self.j.append("job_started", role=role, name=name, argv=argv)
        cmd = f"cd {q(root)} && {shlex.join(argv)} > out/{name}.log 2>&1; echo EXIT=$?"
        proc = self.ssh(role).run(cmd, "job-" + name, self.dir / "logs")
        code = proc.stdout.strip().rsplit("EXIT=", 1)[-1] if "EXIT=" in proc.stdout else None
        self.j.append("job_done", role=role, name=name, ssh_rc=proc.returncode, exit=code)

    def rescue(self, role: str) -> str:
        if self.j.last("rescued", role=role):
            return "verified"
        if not self.j.last("ssh_trusted", role=role):
            status = (
                "not_applicable_no_ssh"
                if not self.j.last("staged", role=role)
                else "failed_no_trust"
            )
            self.j.append("rescue_attempt", role=role, status=status)
            return status
        try:
            ssh, root, logs = self.ssh(role), self.remote_root(role), self.dir / "logs"
            tarname = f"{root}/rescue-{role}.tar"
            if ssh.run(
                f"cd {shlex.quote(root)} && tar -cf {shlex.quote(tarname)} out", "rescue-tar", logs
            ).returncode:
                raise Reject("rescue_tar")
            remote_sha = ssh.remote_sha256(tarname, logs)
            dest = self.dir / "rescue" / role
            dest.mkdir(parents=True, exist_ok=True, mode=0o700)
            local = dest / f"rescue-{time.time_ns()}.tar"
            local_sha = ssh.get(tarname, local)
            if local_sha != remote_sha:
                raise Reject("rescue_sha_mismatch")
            files = {}
            with tarfile.open(local) as tar:
                for m in tar.getmembers():
                    if (
                        m.isfile()
                        and not Path(m.name).is_absolute()
                        and ".." not in Path(m.name).parts
                    ):
                        f = tar.extractfile(m)
                        assert f is not None
                        data = f.read()
                        (dest / Path(m.name).name).write_bytes(data)
                        files[m.name] = sha256(data)
            self.j.append("rescued", role=role, tar=str(local), tar_sha256=local_sha, files=files)
            return "verified"
        except Exception as e:  # rescue failure is journaled; DELETE then needs force
            self.j.append(
                "rescue_attempt", role=role, status="failed", error=f"{type(e).__name__}:{e}"[:300]
            )
            return "failed"

    def delete(self, role: str) -> bool:
        rec = self.receipt(role)
        assert rec is not None
        for attempt in range(int(self.cfg.get("delete_attempts", 6))):
            r = self.p.call(
                "DELETE", f"/api/v0/instances/{rec['instance_id']}/", "delete-" + role, body={}
            )
            ok = r.ok() and r.parsed.get("success") is True
            self.j.append(
                "delete_ack",
                role=role,
                instance_id=rec["instance_id"],
                http_status=r.status,
                success=ok,
                attempt=attempt,
            )
            if ok:
                return True
            self.sleep(r.retry_after if r.retry_after is not None else self.poll)
        return False

    def absent_once(self, iid: int, tag: str) -> str:
        q = urllib.parse.urlencode({"limit": 25, "select_cols": json.dumps(["id", "label"])})
        rows = list_rows(self.p.call("GET", "/api/v1/instances/?" + q, "absence-list-" + tag))
        if rows is None:
            return "unknown"
        listed = any(i["id"] == iid for i in rows)
        _, shown = self.show(iid, "absence-show-" + tag)
        if shown is not None and not isinstance(shown, dict):
            return "unknown"
        return "absent" if not listed and shown is None else "present"

    def confirm_absence(self, role: str) -> bool:
        if self.j.last("absence_confirmed", role=role):
            return True
        rec = self.receipt(role)
        assert rec is not None
        for _ in range(int(self.cfg.get("absence_attempts", 20))):
            state = self.absent_once(rec["instance_id"], role)
            self.j.append("absence_check", role=role, state=state)
            if state == "absent":
                self.j.append("absence_confirmed", role=role, instance_id=rec["instance_id"])
                return True
            self.sleep(self.poll)
        return False

    def owned(self) -> tuple[list[str], list[str]]:
        owned, unresolved = [], []
        for role in self.roles:
            if self.receipt(role):
                owned.append(role)
                continue
            intent = self.j.last("create_intent", role=role)
            if not intent:
                continue
            state, iid = self.recover(intent)
            if state == "found":
                self.j.append(
                    "receipt",
                    role=role,
                    instance_id=iid,
                    source=f"recovered_by_{self.actor}",
                    label=intent["label"],
                )
                owned.append(role)
            elif state == "unknown":
                unresolved.append(role)
        return owned, unresolved

    def cleanup(self, force: bool) -> dict[str, Any]:
        with flock(self.dir / "cleanup.lock"):
            if self.actor == "supervisor":
                with flock(self.dir / "create.lock"):
                    self.j.append("supervisor_cleanup_started")
            owned, unresolved = self.owned()
            live = [r for r in owned if not self.j.last("absence_confirmed", role=r)]
            rescue = {r: self.rescue(r) for r in live}
            ok_rescue = all(
                s == "verified" or s.startswith("not_applicable") for s in rescue.values()
            )
            if not ok_rescue and not force:
                self.j.append("delete_withheld_rescue_pending", rescue=rescue)
                return {"all_absent": False, "rescue": rescue, "unresolved": unresolved}
            if not ok_rescue:
                self.j.append("rescue_incomplete_forced_delete", rescue=rescue)
            for r in live:
                if not any(a.get("success") for a in self.j.all("delete_ack", role=r)):
                    self.delete(r)
            absent = all(self.confirm_absence(r) for r in owned)
            ok = absent and not unresolved
            if not ok:
                self.j.append("liability_open", owned=owned, unresolved=unresolved)
            return {"all_absent": ok, "rescue": rescue, "unresolved": unresolved}

    def close(self, code: int, outcome: str) -> Record:
        with flock(self.dir / "close.lock"):
            done = self.j.last("transaction_closed")
            if done:
                return done
            acct = self.p.call("GET", "/api/v0/users/current/", "final-account")
            credit = acct.parsed.get("credit") if acct.ok() else None
            rows = self.inventory("final-inventory")
            ours = {r["instance_id"] for r in self.j.all("receipt")}
            inv = {
                "parsed": rows is not None,
                "total_rows": None if rows is None else len(rows),
                "owned_instances": None
                if rows is None
                else sum(1 for i in rows if i["id"] in ours),
            }
            evidence = self.evidence(outcome, credit, inv)
            out = Path(self.cfg["evidence_path"])
            out.write_text(json.dumps(evidence, indent=1, sort_keys=True))
            return self.j.append(
                "transaction_closed",
                exit_code=code,
                outcome=outcome,
                actor=self.actor,
                final_credit=credit,
                inventory=inv,
                evidence=str(out),
                evidence_sha256=fsha(out),
            )

    def evidence(self, outcome: str, credit: Any, inv: dict[str, Any]) -> dict[str, Any]:
        snap = json.loads(Path(self.cfg["budget_snapshot"]).read_text())
        runs = []
        for role, name, _neg in self.jobs():
            f = self.dir / "rescue" / role / f"{name}.json"
            res = json.loads(f.read_text()) if f.exists() else None
            ready = self.j.last("instance_ready", role=role) or {}
            runs.append(
                {"role": role, "name": name, "result": res, "machine_id": ready.get("machine_id")}
            )
        verdict = compute_verdict(runs, len(self.roles), int(self.cfg.get("runs_per_host", 2)))
        if outcome != "completed" and verdict["verdict"] == "PASS":
            verdict = {"verdict": "CENSORED", "reason": outcome}
        debit = None
        if credit is not None:
            debit = str(max(Decimal(0), Decimal(str(snap["credit_usd"])) - Decimal(str(credit))))
        admitted = self.admitted() or {}
        cap = (admitted.get("plan") or {}).get("phase_cap_usd") or str(budget.PHASE_A_CAP_USD)
        return {
            "task": "todo 12 Phase A cross-host BF16 determinism",
            "outcome": outcome,
            **verdict,
            "image": self.cfg["image"],
            "image_digest": self.cfg["image"].rsplit("@", 1)[-1]
            if "@" in self.cfg["image"]
            else None,
            "runs": runs,
            "hosts": [
                {
                    "role": r,
                    **{
                        k: (self.j.last("instance_ready", role=r) or {}).get(k)
                        for k in ("instance_id", "machine_id", "driver_version")
                    },
                }
                for r in self.roles
            ],
            "cost": {
                "cap_usd": cap,
                "basis": "snapshot-0 credit minus final credit (prepaid, conservative)",
                "lines": [{"item": "whole_debit_phase_a", "usd": debit}],
                "total_usd": debit,
            },
            "inventory": inv,
            "journal": str(self.j.path),
        }


def shard_path(cfg: dict[str, Any]) -> Path:
    return Path(cfg["tree"]) / cfg["shard"]


def stage_files(tree: Path, cfg: dict[str, Any]) -> dict[str, Path]:
    files = {
        str(p.relative_to(tree)): p
        for p in sorted((tree / "src/hypertrain").rglob("*.py"))
        if "__pycache__" not in p.parts
    }
    for rel in (
        "experiments/gpu_phase_a/run_phase_a.py",
        "experiments/gpu_phase_a/phase_a.json",
        "docker/phase-a/requirements.txt",
    ):
        files[rel] = tree / rel
    files[REMOTE_SHARD] = shard_path(cfg)
    return files


def run(args: argparse.Namespace) -> int:
    cfg_raw = Path(args.config).read_bytes()
    cfg = json.loads(cfg_raw)
    run_dir = Path(args.run_dir).resolve()
    run_dir.mkdir(mode=0o700, parents=True, exist_ok=True)
    if not loopback(cfg["base_url"]) and not args.live:
        print(json.dumps({"verdict": "REFUSED_NON_LOOPBACK_WITHOUT_LIVE"}))
        return EXIT["refused"]
    j = Journal(run_dir)
    opened = j.last("transaction_open")
    if not opened:
        durable_write(run_dir / "config.json", cfg_raw)
        j.append("transaction_open", config_sha256=sha256(cfg_raw), live=bool(args.live))
    elif opened["config_sha256"] != sha256(cfg_raw) or opened["live"] != bool(args.live):
        print(json.dumps({"verdict": "REFUSED_CONFIG_CHANGED_ON_RESUME"}))
        return EXIT["refused"]
    else:
        j.append("launcher_resume")
    closed = j.last("transaction_closed")
    if closed:
        print(json.dumps({"verdict": "ALREADY_CLOSED", "exit_code": closed["exit_code"]}))
        return int(closed["exit_code"])
    p = Provider(cfg["base_url"], read_api_key(cfg["key_file"]), run_dir, j, bool(args.live))
    orch = Orchestrator(cfg, j, p, run_dir)
    if not orch.admitted():
        try:
            orch.admit()
        except Reject as e:
            j.append("admission_failed", reason=str(e))
            orch.close(EXIT["admission_rejected"], "admission_rejected:" + str(e))
            print(json.dumps({"verdict": "CENSORED", "reason": str(e)}))
            return EXIT["admission_rejected"]
    outcome = "completed"
    try:
        orch.ensure_supervisor()
        for role in orch.roles:
            orch.create(role)
        for role in orch.roles:
            orch.attach(role)
        for role in orch.roles:
            orch.boot(role)
            orch.trust(role)
            orch.stage(role)
        for role, name, neg in orch.jobs():
            orch.run_job(role, name, neg)
    except Reject as e:
        outcome = "failed:" + str(e)
        j.append("transaction_failure", reason=str(e))
    except Exception as e:  # unexpected: still rescue and delete owned rentals
        outcome = f"failed:{type(e).__name__}:{str(e)[:300]}"
        j.append("transaction_failure", reason=outcome)
    result = orch.cleanup(force=False)
    if result["all_absent"]:
        code = EXIT["ok"] if outcome == "completed" else EXIT["failed_cleaned"]
        code = int(orch.close(code, outcome)["exit_code"])
    else:
        code = EXIT["liability_open"]
        j.append("launcher_exit_liability_open", outcome=outcome, cleanup=result)
    print(json.dumps({"outcome": outcome, "exit_code": code, "run_dir": str(run_dir)}))
    return code


def main() -> int:
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    r = sub.add_parser("run")
    r.add_argument("--config", required=True)
    r.add_argument("--run-dir", required=True)
    r.add_argument("--live", action="store_true")
    args = ap.parse_args()
    return run(args)


if __name__ == "__main__":
    sys.exit(main())
