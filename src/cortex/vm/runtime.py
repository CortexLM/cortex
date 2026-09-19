"""Durable acceptance, capacity and teardown state machine for the VM host."""

from __future__ import annotations

import asyncio
import sqlite3
import uuid
from pathlib import Path
from typing import Protocol

from pydantic import ValidationError

from cortex.errors import ServiceError
from cortex.proof.artifacts import check_env, verify_artifact
from cortex.rlm.models import VmResult
from cortex.state import secure_sqlite_path

from .models import (
    MAX_ARTIFACT,
    ExecuteRequest,
    GuestOutput,
    Resources,
    VmError,
    VmRecord,
    VmSpec,
)
from .setup import SetupEvidence, SetupExport


class Hypervisor(Protocol):
    async def ready(self) -> None: ...
    async def boot(self, vm_id: str, spec: VmSpec) -> int: ...
    async def execute(self, vm_id: str, spec: VmSpec, job: ExecuteRequest) -> GuestOutput: ...
    async def teardown(self, vm_id: str, retain: bool) -> bool: ...


class Orchestrator:
    def __init__(
        self,
        path: Path,
        hypervisor: Hypervisor,
        *,
        max_experiments: int = 1,
        max_topics: int = 64,
        caps: Resources | None = None,
    ):
        if not 1 <= max_experiments <= 128 or not 1 <= max_topics <= 1024:
            raise ValueError("invalid VM capacity")
        self.hypervisor = hypervisor
        self.max_experiments, self.max_topics = max_experiments, max_topics
        self.caps = caps or Resources(disk_mib=1048576)
        database = secure_sqlite_path(path)
        self.db = sqlite3.connect(database, isolation_level=None)
        self.db.row_factory = sqlite3.Row
        self.db.execute("PRAGMA journal_mode=WAL")
        self.db.execute("PRAGMA synchronous=FULL")
        self.db.executescript("""
            CREATE TABLE IF NOT EXISTS vms (
                vm_id TEXT PRIMARY KEY, topic_id TEXT NOT NULL, kind TEXT NOT NULL,
                state TEXT NOT NULL, spec TEXT NOT NULL, pid INTEGER
            );
            CREATE TABLE IF NOT EXISTS jobs (
                execution_id TEXT PRIMARY KEY, commitment TEXT NOT NULL,
                topic_vm_id TEXT NOT NULL, vm_id TEXT, state TEXT NOT NULL,
                result TEXT, error TEXT
            );
            CREATE TABLE IF NOT EXISTS setup_evidence (
                execution_id TEXT PRIMARY KEY, evidence TEXT NOT NULL
            );
        """)
        self._lock = asyncio.Lock()
        self._tasks: dict[str, asyncio.Task[VmResult]] = {}
        self._topic_locks: dict[str, asyncio.Lock] = {}

    def _record(self, row) -> VmRecord:
        return VmRecord(
            vm_id=row["vm_id"],
            spec=VmSpec.model_validate_json(row["spec"]),
            state=row["state"],
            pid=row["pid"],
        )

    def get(self, vm_id: str) -> VmRecord:
        row = self.db.execute("SELECT * FROM vms WHERE vm_id=?", (vm_id,)).fetchone()
        if row is None:
            raise VmError("vm not found", 404)
        return self._record(row)

    def by_topic(self, topic_id: str) -> VmRecord:
        row = self.db.execute(
            "SELECT * FROM vms WHERE topic_id=? AND kind='topic' AND state='running'", (topic_id,)
        ).fetchone()
        if row is None:
            raise VmError("topic VM not found", 404)
        return self._record(row)

    def _validate_capacity(self, spec: VmSpec) -> None:
        resources = spec.resources
        if (
            resources.vcpus > self.caps.vcpus
            or resources.mem_mib > self.caps.mem_mib
            or resources.disk_mib > self.caps.disk_mib
        ):
            raise VmError("VM resources exceed host ceilings")
        active = self.db.execute(
            "SELECT count(*) FROM vms WHERE kind=? AND state IN ('booting','running','uncertain')",
            (spec.kind,),
        ).fetchone()[0]
        maximum = self.max_experiments if spec.kind == "experiment" else self.max_topics
        if active >= maximum:
            raise VmError("VM capacity exhausted")
        if (
            spec.kind == "topic"
            and self.db.execute(
                "SELECT 1 FROM vms WHERE topic_id=? AND kind='topic' "
                "AND state IN ('booting','running','uncertain')",
                (spec.topic_id,),
            ).fetchone()
        ):
            raise VmError("topic already has a VM", 409)

    async def create(self, spec: VmSpec) -> VmRecord:
        await self.hypervisor.ready()
        async with self._lock:
            self.db.execute("BEGIN IMMEDIATE")
            try:
                self._validate_capacity(spec)
                vm_id = f"vm-{uuid.uuid4().hex}"
                self.db.execute(
                    "INSERT INTO vms VALUES (?,?,?,?,?,NULL)",
                    (vm_id, spec.topic_id, spec.kind, "booting", spec.model_dump_json()),
                )
                self.db.execute("COMMIT")
            except BaseException:
                self.db.execute("ROLLBACK")
                raise
        try:
            pid = await self.hypervisor.boot(vm_id, spec)
            self.db.execute("UPDATE vms SET state='running',pid=? WHERE vm_id=?", (pid, vm_id))
        except BaseException:
            # An interrupted boot is owned by its durable record until cleanup is confirmed.
            await asyncio.shield(self.teardown(vm_id, retain=True))
            raise
        return self.get(vm_id)

    def _prepare_job(
        self, topic_vm_id: str, job: ExecuteRequest
    ) -> tuple[VmRecord, ExecuteRequest]:
        topic = self.get(topic_vm_id)
        if (
            topic.spec.topic_id != job.context.topic_id
            or topic.spec.image_digest != job.context.image_digest
            or topic.spec.kind != "topic"
        ):
            raise VmError("topic_mismatch", 409)
        self._validate_job(job)
        # The production backend resolves and hashes the operator pack before a VM can be rented.
        prepare = getattr(self.hypervisor, "prepare", None)
        if prepare is not None:
            job = prepare(job)
        return topic, job

    async def execute(self, topic_vm_id: str, job: ExecuteRequest) -> VmResult:
        topic, job = self._prepare_job(topic_vm_id, job)
        async with self._lock:
            row = self.db.execute(
                "SELECT * FROM jobs WHERE execution_id=?", (job.execution_id,)
            ).fetchone()
            if row:
                if row["commitment"] != job.commitment() or row["topic_vm_id"] != topic_vm_id:
                    raise VmError("execution id reused with different request", 409)
                if row["state"] == "succeeded":
                    return VmResult.model_validate_json(row["result"])
                if row["state"] == "failed":
                    raise VmError(row["error"])
                if job.execution_id not in self._tasks:
                    raise VmError("interrupted job requires host recovery")
            else:
                if topic.state != "running":
                    raise VmError("topic VM is not running")
                self.db.execute(
                    "INSERT INTO jobs VALUES (?,?,?,NULL,'accepted',NULL,NULL)",
                    (job.execution_id, job.commitment(), topic_vm_id),
                )
                self._tasks[job.execution_id] = asyncio.create_task(self._execute(topic, job))
                # Harvest exceptions even when every HTTP waiter has disconnected.
                self._tasks[job.execution_id].add_done_callback(self._harvest)

                def forget(completed: asyncio.Task[VmResult]) -> None:
                    self._tasks.pop(job.execution_id, None)

                self._tasks[job.execution_id].add_done_callback(forget)
            task = self._tasks[job.execution_id]
        return await asyncio.shield(task)

    async def reconcile(self, topic_vm_id: str, job: ExecuteRequest) -> VmResult:
        """Recover exact successful evidence without starting or repairing a VM job."""

        topic, job = self._prepare_job(topic_vm_id, job)
        async with self._lock:
            row = self.db.execute(
                "SELECT * FROM jobs WHERE execution_id=?", (job.execution_id,)
            ).fetchone()
            if row is None:
                raise VmError("execution not found", 404)
            if row["commitment"] != job.commitment() or row["topic_vm_id"] != topic_vm_id:
                raise VmError("execution id reused with different request", 409)
            if row["state"] != "succeeded":
                raise VmError("execution has no confirmed successful result")
            if not isinstance(row["result"], str) or not isinstance(row["vm_id"], str):
                raise VmError("stored execution evidence unavailable")
            try:
                result = VmResult.model_validate_json(row["result"])
                executed_vm = self.get(row["vm_id"])
            except ValidationError:
                raise VmError("stored execution evidence invalid") from None
            if (
                result.topic_id != job.context.topic_id
                or result.job_id != job.context.job_id
                or result.image_digest != job.context.image_digest
                or result.artifact_digest != job.context.artifact_digest
                or result.execution_id != job.execution_id
                or result.exit_code != 0
                or not result.sandboxed
                or executed_vm.spec.topic_id != topic.spec.topic_id
                or executed_vm.spec.image_digest != topic.spec.image_digest
            ):
                raise VmError("stored execution evidence binding mismatch")
            if result.network_enabled and (
                job.dedicated or job.action.phase in {"experiment", "preflight"}
            ):
                raise VmError("stored evaluation evidence requires a networkless VM")
            if job.dedicated:
                if executed_vm.spec.kind != "experiment" or executed_vm.vm_id == topic_vm_id:
                    raise VmError("stored execution requires a dedicated experiment VM")
                if executed_vm.state != "destroyed":
                    raise VmError("TeardownUnconfirmed")
            elif executed_vm.vm_id != topic_vm_id or executed_vm.spec.kind != "topic":
                raise VmError("stored execution topic VM mismatch")
            return result

    @staticmethod
    def _harvest(task: asyncio.Task) -> None:
        if not task.cancelled():
            task.exception()

    @staticmethod
    def _validate_job(job: ExecuteRequest) -> None:
        try:
            byok_params = (
                job.params
                if job.action.phase == "experiment"
                else {key: value for key, value in job.params.items() if key != "miner_byok"}
            )
            check_env(byok_params, job.env)
            names = [f"PROOF_PARAM_{name.upper().replace('-', '_')}" for name in job.params]
            if len(names) != len(set(names)):
                raise VmError("signed params collide as environment names", 400)
            if job.artifact_b64:
                verify_artifact(
                    job.artifact(), job.context.artifact_digest or "", limit=MAX_ARTIFACT
                )
        except ServiceError as exc:
            raise VmError(exc.reason, exc.status) from None

    async def _execute(self, topic: VmRecord, job: ExecuteRequest) -> VmResult:
        vm = topic
        owned = False
        try:
            if job.dedicated:
                vm = await self.create(
                    VmSpec(
                        topic_id=topic.spec.topic_id,
                        image_digest=topic.spec.image_digest,
                        kind="experiment",
                        resources=topic.spec.resources,
                    )
                )
                owned = True
            self.db.execute(
                "UPDATE jobs SET vm_id=?,state='running' WHERE execution_id=?",
                (vm.vm_id, job.execution_id),
            )
            lock = self._topic_locks.setdefault(vm.vm_id, asyncio.Lock())
            async with lock, asyncio.timeout(job.action.timeout_seconds + 30):
                output = await self.hypervisor.execute(vm.vm_id, vm.spec, job)
            if output.context != job.context or output.execution_id != job.execution_id:
                raise VmError("guest evidence binding mismatch")
            if output.exit_code != 0:
                raise VmError("guest command failed")
            if job.action.phase in {"experiment", "preflight"} and output.measurement is None:
                raise VmError("guest report required")
            measurement = output.measurement
            if output.setup_export is not None:
                if job.context.purpose != "setup" or job.action.phase != "setup":
                    raise VmError("setup export outside setup job")
                export = SetupExport.model_validate(output.setup_export)
                raw, evidence = export.verified()
                if output.report_digest != evidence.setup_report_digest:
                    raise VmError("setup evidence digest mismatch")
                install = getattr(self.hypervisor, "install_setup", None)
                if install is None:
                    raise VmError("generated setup packs are not supported by this host")
                install(raw, evidence.environment_digest)
                self.db.execute(
                    "INSERT INTO setup_evidence VALUES (?,?)",
                    (job.execution_id, evidence.model_dump_json()),
                )
            result = VmResult(
                topic_id=job.context.topic_id,
                job_id=job.context.job_id,
                image_digest=vm.spec.image_digest,
                artifact_digest=job.context.artifact_digest,
                sandboxed=True,
                network_enabled=(
                    getattr(self.hypervisor, "network_enabled", lambda spec: False)(vm.spec)
                ),
                execution_id=job.execution_id,
                report_digest=output.report_digest,
                exit_code=output.exit_code,
                stdout_tail=output.stdout_tail,
                metrics=measurement.metrics if measurement else [],
                rule_checks=measurement.rule_checks if measurement else [],
                produced_artifacts=measurement.produced_artifacts if measurement else [],
                flops_used=measurement.flops_used if measurement else 0,
            )
            if owned:
                confirmed = await self.teardown(vm.vm_id, retain=False)
                owned = False  # Teardown failure retains the uncertain record and capacity.
                if not confirmed:
                    raise VmError("TeardownUnconfirmed")
            self.db.execute(
                "UPDATE jobs SET state='succeeded',result=? WHERE execution_id=?",
                (result.model_dump_json(), job.execution_id),
            )
            return result
        except BaseException as exc:
            if owned:
                await asyncio.shield(self.teardown(vm.vm_id, retain=True))
            reason = exc.reason if isinstance(exc, VmError) else "guest execution unavailable"
            self.db.execute(
                "UPDATE jobs SET state='failed',error=? WHERE execution_id=?",
                (reason, job.execution_id),
            )
            if isinstance(exc, asyncio.CancelledError):
                raise
            raise VmError(reason) from None

    def setup_evidence(self, execution_id: str) -> SetupEvidence | None:
        row = self.db.execute(
            "SELECT evidence FROM setup_evidence WHERE execution_id=?", (execution_id,)
        ).fetchone()
        return SetupEvidence.model_validate_json(row[0]) if row else None

    async def teardown(self, vm_id: str, *, retain: bool) -> bool:
        try:
            confirmed = await self.hypervisor.teardown(vm_id, retain)
        except Exception:
            confirmed = False
        self.db.execute(
            "UPDATE vms SET state=? WHERE vm_id=?",
            (("retained" if retain else "destroyed") if confirmed else "uncertain", vm_id),
        )
        return confirmed

    async def recover(self) -> None:
        """Reap pre-restart VMs; never rerun an accepted paid job automatically."""
        rows = self.db.execute(
            "SELECT vm_id FROM vms WHERE state IN ('booting','running','uncertain')"
        ).fetchall()
        for row in rows:
            await self.teardown(row["vm_id"], retain=True)
        self.db.execute(
            "UPDATE jobs SET state='failed',error='host restarted during job' "
            "WHERE state IN ('accepted','running')"
        )

    async def close(self) -> None:
        if self._tasks:
            await asyncio.gather(*self._tasks.values(), return_exceptions=True)
        self.db.close()
