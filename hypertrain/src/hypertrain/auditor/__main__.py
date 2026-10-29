"""`python -m hypertrain.auditor --api URL --run-id HEX --token-file F --keyfile F --data-dir D`.

Leases audit jobs for ONE run, replays them on the local verified shards and posts verdicts.
Jobs of any other run are handed back (retry) so they are never replayed on the wrong data.

`--contract 2 --api URL --run-id HEX --keyfile F`: signed v2 Receipt lease, master-executed
replay (`/execute`), then the auditor signs exactly that ReplayVerdict (`/complete`). The
auditor never computes or chooses the verdict; jobs not coordinator-signed for this run and key
are refused unexecuted (the lease then expires server-side).

Exit 0: settled (--once) or no lease granted (empty queue, or the global running-lease cap).
Exit 2: refused job, malformed lease, HTTP error/timeout, or the server's 409 "one live lease
per auditor" after a restart mid-job (that lease is left to expire server-side). The CLI does
not poll for work: the supervisor re-invokes it on its own audit-scheduling or exit events. A
503 (beacon pause) on /lease or /complete is retried every RETRY_S, /complete only until its
lease's wall deadline; /execute is never re-posted.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import logging
import os
import secrets
import sys
import time
from collections.abc import Callable
from pathlib import Path
from typing import Any

import httpx

import hypertrain.trainer  # noqa: F401  (determinism before torch)
from hypertrain.auditor.worker import Auditor, HttpApi
from hypertrain.miner.cli import shard_sampler
from hypertrain.miner.core import load_keyfile
from hypertrain.protocol import envelope_v2
from hypertrain.protocol.envelope import body_digest
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages import Receipt, ReplayEnv, ReplayVerdict, RunManifest
from hypertrain.protocol.messages_v2 import AuditJobV2, RunManifestV2


class RunScopedApi(HttpApi):
    def __init__(self, client: httpx.Client, token: str, base: str, run_id: str) -> None:
        super().__init__(client, token, base)
        self.run_id = run_id

    def lease(self) -> dict[str, Any] | None:
        raw = super().lease()
        if raw is not None and RunManifest.model_validate(raw["manifest"]).run_id() != self.run_id:
            self.fail(str(raw["id"]), str(raw["lease"]), "auditor serves another run", True)
            return None
        return raw


class RefusedJob(RuntimeError):
    pass


MARGIN_S = 60.0
RETRY_S = 3.0  # one beacon round
LEASE_RETRY_S = 300.0  # a /lease 503 outlasting this is an outage: exit, the supervisor restarts


def audit_v2(
    client: httpx.Client,
    base: str,
    run_id: str,
    key: Keypair,
    once: bool,
    clock: Callable[[], float] = time.time,
    sleep: Callable[[float], None] = time.sleep,
) -> int:
    root = f"{base}/v2/runs/{run_id}"

    def post(path: str, body: dict[str, Any], timeout: float, until: float = 0.0) -> httpx.Response:
        """Post once; while the master answers 503 and `until` (wall) is ahead, post again."""
        while True:
            r = client.post(root + path, json=body, timeout=timeout)
            if r.status_code != 503 or clock() + RETRY_S >= until:
                r.raise_for_status()
                return r
            logging.warning("%s: 503 %s, retry in %.0f s", path, r.text[:200], RETRY_S)
            sleep(RETRY_S)

    def now() -> int:
        r = client.get(root, timeout=60)
        r.raise_for_status()
        return int(r.json()["now_round"])

    r = client.get(root, timeout=60)
    r.raise_for_status()
    manifest = RunManifestV2.model_validate(r.json()["manifest_envelope"]["body"])
    if r.json()["contract"] != 2 or manifest.run_id() != run_id:
        raise RefusedJob("served v2 manifest hashes to another run_id")
    while True:
        give_up = clock() + LEASE_RETRY_S
        while True:
            t = now()
            # Unique Receipt replay ID per lease request; w is not lease-bound server-side.
            ask = Receipt(w=0, commit_hash=secrets.token_hex(32), received_round=t)
            r = client.post(
                root + "/worker/lease",
                json=envelope_v2.seal(key, "Receipt", run_id, ask, t + 100),
                timeout=60,
            )
            # Empty queue: 204, or 503 from an admitted service run (store.lease_v2).
            if r.status_code == 204 or (
                r.status_code == 503 and "no authenticated queued" in r.text
            ):
                return 0
            if r.status_code != 503 or clock() + RETRY_S >= give_up:
                break
            logging.warning("/worker/lease: 503 %s, retry in %.0f s", r.text[:200], RETRY_S)
            sleep(RETRY_S)
        r.raise_for_status()
        env = envelope_v2.parse_envelope(r.json())
        if (
            env.type != "AuditJobV2"
            or env.run_id != run_id
            or env.signer != manifest.training.coord_pubkey
            or not envelope_v2.verify_envelope(r.json())
        ):
            raise RefusedJob("lease is not a coordinator-signed job of this run")
        job = AuditJobV2.model_validate(env.body)
        if job.run_id != run_id or job.manifest != manifest or job.auditor_id != key.ss58:
            raise RefusedJob("leased job binds another run, manifest or auditor")
        w, exp = job.start_state.w, job.lease_expires
        receipt = Receipt(w=w, commit_hash=job.lease_nonce, received_round=now())
        # Server LeaseGuard cancels replay at this wall deadline (store.execute_audit_v2).
        beacon = manifest.training.beacon
        deadline = beacon.genesis_time + (min(exp, job.absolute_deadline) - 1) * beacon.period
        r = post(
            f"/worker/jobs/{job.job_id}/execute",
            envelope_v2.seal(key, "Receipt", run_id, receipt, exp),
            max(deadline - clock(), 0.0) + MARGIN_S,
        )
        verdict = ReplayVerdict.model_validate(r.json()["verdict"])
        challenge = envelope_v2.parse_envelope(job.challenge_envelope)
        if verdict.challenge_hash != body_digest(challenge.body):
            raise RefusedJob("executed verdict answers another challenge")
        done = post(
            f"/worker/jobs/{job.job_id}/complete",
            envelope_v2.seal(key, "ReplayVerdict", run_id, verdict, exp),
            MARGIN_S,
            until=deadline,
        ).json()
        print(json.dumps({"job": job.job_id, "result": done["result"]}), flush=True)
        if once:
            return 0


def replay_env(image_digest: str, device: str) -> ReplayEnv:
    import torch

    if device == "cpu":
        return ReplayEnv(
            image_digest=image_digest,
            driver="cpu",
            gpu_uuid_sha256=hashlib.sha256(b"cpu").hexdigest(),
            sm_count=1,
        )
    props = torch.cuda.get_device_properties(0)
    version = Path("/proc/driver/nvidia/version")
    parts = version.read_text().split() if version.is_file() else []
    driver = next((p for p in parts if p[:1].isdigit() and "." in p), "unknown")
    return ReplayEnv(
        image_digest=image_digest,
        driver=driver,
        gpu_uuid_sha256=hashlib.sha256(str(props.uuid).encode()).hexdigest(),
        sm_count=props.multi_processor_count,
    )


def main(argv: list[str] | None = None) -> int:
    p = argparse.ArgumentParser(prog="python -m hypertrain.auditor")
    p.add_argument("--api", required=True)
    p.add_argument("--run-id", required=True)
    p.add_argument("--contract", type=int, choices=[1, 2], default=1, help="2: signed v2 routes")
    p.add_argument("--token-file", type=Path, help="worker bearer token (contract 1)")
    p.add_argument("--keyfile", type=Path, required=True)
    p.add_argument("--data-dir", type=Path, help="verified shards (contract 1)")
    p.add_argument("--device", choices=["cuda", "cpu"], default="cuda")
    p.add_argument("--image-digest", default=os.environ.get("HYPERTRAIN_IMAGE_DIGEST", ""))
    p.add_argument("--once", action="store_true", help="settle at most one job, then exit")
    a = p.parse_args(argv)
    if a.contract == 1 and (a.token_file is None or a.data_dir is None):
        p.error("contract 1 requires --token-file and --data-dir")
    logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(message)s")
    if a.contract == 2:
        with httpx.Client() as v2:
            try:
                return audit_v2(v2, a.api.rstrip("/"), a.run_id, load_keyfile(a.keyfile), a.once)
            except (RefusedJob, ValueError, httpx.HTTPError) as error:
                logging.error("%s", error)
                return 2
    if a.device == "cuda":
        import torch

        torch.set_default_device("cuda")
    client = httpx.Client()
    base = a.api.rstrip("/")
    r = client.get(f"{base}/v1/runs/{a.run_id}", timeout=60)
    r.raise_for_status()
    manifest = RunManifest.model_validate(r.json()["manifest_envelope"]["body"])
    if manifest.run_id() != a.run_id:
        logging.error("served manifest hashes to another run_id")
        return 2
    api = RunScopedApi(client, a.token_file.read_text().strip(), base, a.run_id)
    auditor = Auditor(
        api,
        load_keyfile(a.keyfile),
        replay_env(a.image_digest, a.device),
        shard_sampler(a.data_dir, manifest),
    )
    while True:
        result = auditor.run_once()
        if result is not None:
            print(json.dumps({"result": result}), flush=True)
        if a.once or result is None:
            return 0


if __name__ == "__main__":
    sys.exit(main())
