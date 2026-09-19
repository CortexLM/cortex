"""In-guest process execution. This module never offers a control-plane fallback."""

from __future__ import annotations

import asyncio
import hashlib
import io
import os
import re
import signal
import stat
import sys
import tarfile
from dataclasses import dataclass
from pathlib import Path

from pydantic import ValidationError

from cortex.errors import ServiceError
from cortex.proof.artifacts import check_env, verify_artifact
from cortex.rlm.models import ProducedArtifact

from .models import MAX_ARTIFACT, TAIL_BYTES, ExecuteRequest, GuestMeasurement, GuestOutput, VmError
from .setup import GENERATED_RUNNER, SetupManifest, export_setup


@dataclass(frozen=True)
class GuestIdentity:
    vm_id: str
    topic_id: str
    image_digest: str
    kind: str

    @classmethod
    def from_system(cls) -> GuestIdentity:
        """Only the host's kernel boot binding establishes executable guest context."""
        values = dict(
            item.split("=", 1) for item in Path("/proc/cmdline").read_text().split() if "=" in item
        )
        try:
            identity = cls(
                *(values[key] for key in ("proof_vm", "proof_topic", "proof_image", "proof_kind"))
            )
        except KeyError:
            raise VmError("not running in an established Proof VM") from None
        if (
            not re.fullmatch(r"[A-Za-z0-9_.-]{1,96}", identity.vm_id)
            or not re.fullmatch(r"[A-Za-z0-9_.-]{1,96}", identity.topic_id)
            or not re.fullmatch(r"[0-9a-f]{64}", identity.image_digest)
            or identity.kind not in {"topic", "experiment"}
        ):
            raise VmError("invalid Proof VM boot binding")
        return identity


def extract_verified(data: bytes, digest: str, destination: Path) -> None:
    try:
        verify_artifact(data, digest, limit=MAX_ARTIFACT)
    except ServiceError as exc:
        raise VmError(exc.reason, 400) from None
    destination.mkdir(mode=0o700)
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:") as archive:
        for member in archive:
            target = destination / member.name
            if member.isdir():
                target.mkdir(mode=0o700, parents=True, exist_ok=True)
            else:
                target.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
                content = archive.extractfile(member)
                if content is None:
                    raise VmError("artifact member unreadable")
                with target.open("xb") as output:
                    while chunk := content.read(65536):
                        output.write(chunk)
                target.chmod(0o600)


class GuestExecutor:
    def __init__(
        self,
        identity: GuestIdentity,
        *,
        workspace: Path = Path("/workspace"),
        runners_dir: Path = Path("/opt/proof/runners"),
    ):
        self.identity, self.workspace, self.runners_dir = identity, workspace, runners_dir
        workspace.mkdir(parents=True, exist_ok=True, mode=0o700)
        if workspace.is_symlink():
            raise VmError("guest workspace may not be a symlink")

    async def execute(self, request: ExecuteRequest) -> GuestOutput:
        context = request.context
        if (
            context.topic_id != self.identity.topic_id
            or context.image_digest != self.identity.image_digest
            or (request.dedicated and self.identity.kind != "experiment")
        ):
            raise VmError("guest topic or isolation mismatch", 409)
        if request.action.operation == "read_file":
            return self._read_file(request)
        directory = self.workspace / request.execution_id
        directory.mkdir(mode=0o700)
        output_dir = directory / "output"
        output_dir.mkdir(mode=0o700)
        artifact_dir = directory / "artifact"
        if request.artifact_b64:
            extract_verified(request.artifact(), context.artifact_digest or "", artifact_dir)
        pack_dir = directory / "pack"
        if request.action.phase in {"preflight", "experiment"}:
            digest = request.params.get("experiment_pack_digest", "").removeprefix("sha256:")
            if not request.pack_b64:
                raise VmError("experiment pack required")
            extract_verified(request.pack(), digest, pack_dir)
        environment = self._environment(request, directory, output_dir, artifact_dir)
        environment["PROOF_PACK_DIR"] = str(pack_dir)
        environment["PROOF_WALL_BUDGET_S"] = str(request.action.timeout_seconds)
        setup_dir = self.workspace / "setup" / request.context.job_id
        if request.context.purpose == "setup":
            setup_dir.mkdir(mode=0o700, parents=True, exist_ok=True)
            environment["PROOF_SETUP_DIR"] = str(setup_dir)
        if request.params.get("baseline_runner") == GENERATED_RUNNER:
            environment["PYTHONPATH"] = str(pack_dir / "vendor")
        argv = self._argv(request, pack_dir)
        tail = bytearray()
        # Keep enough extra prefix to redact a secret straddling the visible tail boundary.
        tail_limit = TAIL_BYTES + max(
            (len(value.encode()) for value in request.env.values()), default=0
        )
        process = await asyncio.create_subprocess_exec(
            *argv,
            cwd=directory,
            env=environment,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.STDOUT,
            start_new_session=True,
        )

        async def drain():
            if process.stdout is None:
                raise VmError("guest stdout unavailable")
            while chunk := await process.stdout.read(8192):
                tail.extend(chunk)
                del tail[:-tail_limit]

        draining = asyncio.create_task(drain())
        try:
            async with asyncio.timeout(request.action.timeout_seconds):
                await process.wait()
                await draining
        except BaseException:
            self._kill(process)
            await process.wait()
            await asyncio.gather(draining, return_exceptions=True)
            raise VmError("guest command timed out or was interrupted") from None
        finally:
            self._kill(process)  # A successful parent may have left descendants.
        text = tail.decode(errors="replace")
        for value in request.env.values():
            text = text.replace(value, "[redacted]")
        measurement, report_digest = None, hashlib.sha256(bytes(tail)).hexdigest()
        report_path = output_dir / "report.json"
        if report_path.exists():
            raw = self._safe_read(report_path, output_dir, 1_048_576)
            try:
                measurement = GuestMeasurement.model_validate_json(raw)
            except ValidationError:
                raise VmError("invalid measured report.json") from None
            report_digest = hashlib.sha256(raw).hexdigest()
        elif request.action.phase in {"preflight", "experiment"}:
            raise VmError("report.json required; no measured result")
        setup_export = None
        setup_manifest = output_dir / "setup.json"
        if request.context.purpose == "setup" and request.action.phase == "setup":
            if setup_manifest.exists():
                raw = self._safe_read(setup_manifest, output_dir, 8_388_608)
                manifest = SetupManifest.model_validate_json(raw)
                setup_export = export_setup(setup_dir, manifest)
                _, evidence = setup_export.verified()
                report_digest = evidence.setup_report_digest
                measurement = GuestMeasurement(
                    flops_used=0,
                    produced_artifacts=[
                        ProducedArtifact(kind="environment", digest=evidence.environment_digest),
                        ProducedArtifact(
                            kind="private_holdout", digest=evidence.private_holdout_digest
                        ),
                    ],
                )
        return GuestOutput(
            context=context,
            execution_id=request.execution_id,
            exit_code=process.returncode if process.returncode is not None else -1,
            stdout_tail=text[-TAIL_BYTES:],
            measurement=measurement,
            report_digest=report_digest,
            setup_export=setup_export.model_dump() if setup_export else None,
        )

    def _argv(self, request: ExecuteRequest, pack_dir: Path) -> list[str]:
        if request.action.phase not in {"preflight", "experiment"}:
            if request.context.purpose == "evaluate":
                raise VmError("evaluation command requires an operator adaptor")
            return request.action.argv
        runner = request.params.get("baseline_runner") or request.params.get(
            "in_guest_benchmark_runner"
        )
        if runner is None or not re.fullmatch(r"[A-Za-z0-9_.-]{1,96}", runner):
            raise VmError("signed topic selects no installed runner")
        if runner == GENERATED_RUNNER:
            entry = pack_dir / ("inspect.py" if request.action.phase == "preflight" else "run.py")
            self._safe_read(entry, pack_dir, MAX_ARTIFACT)
            # A pack's inspect.py must not shadow Python's standard inspect module.
            bootstrap = (
                "import runpy,sys; root,entry=sys.argv[1:]; sys.argv=[entry]; "
                "sys.path.append(root); runpy.run_path(entry,run_name='__main__')"
            )
            return [sys.executable, "-P", "-c", bootstrap, str(pack_dir), str(entry)]
        entry = (
            self.runners_dir
            / runner
            / ("inspect" if request.action.phase == "preflight" else "run")
        )
        if (
            not entry.is_file()
            or entry.is_symlink()
            or not entry.resolve().is_relative_to(self.runners_dir.resolve())
            or not os.access(entry, os.X_OK)
        ):
            raise VmError("operator adaptor is not installed in pinned guest image")
        # The model cannot create its own report by choosing an arbitrary command.
        return [str(entry)]

    def _environment(self, request, directory, output_dir, artifact_dir):
        try:
            byok_params = (
                request.params
                if request.action.phase == "experiment"
                else {key: value for key, value in request.params.items() if key != "miner_byok"}
            )
            check_env(byok_params, request.env)
        except ServiceError as exc:
            raise VmError(exc.reason, exc.status) from None
        environment = {
            "PATH": "/usr/local/bin:/usr/bin:/bin",
            "LANG": "C.UTF-8",
            "PROOF_TOPIC_ID": request.context.topic_id,
            "PROOF_JOB_ID": request.context.job_id,
            "PROOF_OUTPUT_DIR": str(output_dir),
            "PROOF_ARTIFACT_DIR": str(artifact_dir),
            "PROOF_ARTIFACT_DIGEST": request.context.artifact_digest or "",
            "PROOF_WORK_DIR": str(directory),
            "PROOF_JOB": request.context.purpose,
        }
        for name, value in request.params.items():
            if not re.fullmatch(r"[a-zA-Z0-9_-]{1,64}", name):
                raise VmError("invalid signed param name")
            mapped = "PROOF_PARAM_" + name.upper().replace("-", "_")
            if mapped in environment:
                raise VmError("signed params collide as environment names")
            environment[mapped] = value
        vault = directory / "env"
        vault.mkdir(mode=0o700)
        for name, value in request.env.items():
            fd = os.open(vault / name, os.O_CREAT | os.O_EXCL | os.O_WRONLY | os.O_NOFOLLOW, 0o600)
            with os.fdopen(fd, "w") as stream:
                stream.write(value)
            environment[name] = value
        environment["PROOF_MINER_ENV_DIR"] = str(vault)
        return environment

    def _read_file(self, request: ExecuteRequest) -> GuestOutput:
        path = request.action.path or ""
        target = self.workspace / path.removeprefix("/workspace/")
        raw = self._safe_read(target, self.workspace, TAIL_BYTES)
        text = raw.decode(errors="replace")
        for value in request.env.values():
            text = text.replace(value, "[redacted]")
        return GuestOutput(
            context=request.context,
            execution_id=request.execution_id,
            exit_code=0,
            stdout_tail=text[:TAIL_BYTES],
            report_digest=hashlib.sha256(raw).hexdigest(),
        )

    @staticmethod
    def _safe_read(path: Path, root: Path, limit: int) -> bytes:
        if path.is_symlink() or not path.resolve().is_relative_to(root.resolve()):
            raise VmError("guest file escapes workspace")
        try:
            fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
            with os.fdopen(fd, "rb") as stream:
                if not stat.S_ISREG(os.fstat(stream.fileno()).st_mode):
                    raise VmError("guest output must be a regular file")
                raw = stream.read(limit + 1)
        except OSError:
            raise VmError("guest output file unavailable") from None
        if len(raw) > limit:
            raise VmError("guest output file too large")
        return raw

    @staticmethod
    def _kill(process):
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
