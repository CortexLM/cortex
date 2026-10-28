"""Helpers private to the admission_http package of the ledger-invariants lot. Not collected.

`Gate` drives the signed admission routes of the v2 `network` fixture through its real FastAPI
app, with a metagraph the test can change. Every request here is something an honest or a
mistaken participant sends; the tests assert what the service refuses and what stays unchanged.
"""

from __future__ import annotations

import shutil
import sqlite3
from dataclasses import dataclass
from pathlib import Path
from types import ModuleType
from typing import Any

import httpx
from fastapi.testclient import TestClient

from hypertrain.challenge.admission import Admission
from hypertrain.challenge.store import ChallengeStore
from hypertrain.miner import admission as miner_admission
from hypertrain.miner.admission import sign_join, sign_rotation
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages_v2 import (
    HardwareHint,
    JoinRequest,
    RotateRequest,
    WorkProof,
    WorkScreenV2,
)

from .support import World, load_fixture_module

ADMISSION_FIXTURE = ("li_admission_fixture", "challenge/test_admission_v2.py")

# Seeds clear of every key the fixtures use (0x07-0x09, 0x50-0x5d, 0x71-0x74, bytes(range(32))).
HOTS = [Keypair(bytes([130 + i]) * 32) for i in range(40)]
COLDS = [Keypair(bytes([170 + i]) * 32) for i in range(40)]
STRANGER = Keypair(bytes([250]) * 32)

ADMISSION_TABLES = (
    "admissions_v2",
    "admission_history",
    "admission_reservations",
    "admission_quota",
    "admission_trials",
    "admission_trial_results",
    "admission_outbox",
    "admission_transitions",
    "admission_pending",
    "escrow_events",
)


class PreconditionError(Exception):
    """A scenario step that must hold for the assertion after it to mean anything.

    It is not an AssertionError, so an xfail(raises=AssertionError) cannot hide it.
    """


def precondition(ok: bool, message: str) -> None:
    if not ok:
        raise PreconditionError(message)


class Subnet:
    """The master's metagraph for netuid 100, as a MockTransport the test can edit."""

    def __init__(self, registered: list[str]) -> None:
        self.registered = set(registered)

    def handler(self, request: httpx.Request) -> httpx.Response:
        hotkeys = {key: index for index, key in enumerate(sorted(self.registered))}
        return httpx.Response(200, json={"netuid": 100, "hotkeys": hotkeys})

    def transport(self) -> httpx.MockTransport:
        return httpx.MockTransport(self.handler)


def trial_challenge_headers(run_id: str, admission_id: str, owner: Keypair) -> dict[str, str]:
    """The header pair of the owner's challenge GET on a base that has not moved the request
    into a signed body (ADR 0010 Part 3 as first merged, domain `trial-challenge/2`). Only
    `Gate.challenge` uses it, and only when the service network has no `owner_request`.
    """
    message = f"hypertrain/trial-challenge/2|{run_id}|{admission_id}".encode()
    return {"x-hotkey": owner.ss58, "x-trial-signature": owner.sign(message).hex()}


@dataclass
class Gate:
    """The service world of one test, its metagraph, and the signed requests of a participant."""

    world: World
    subnet: Subnet

    @property
    def network(self) -> Any:
        return self.world.network

    @property
    def client(self) -> TestClient:
        client: TestClient = self.network.client
        return client

    @property
    def store(self) -> ChallengeStore:
        return self.world.store

    @property
    def run_id(self) -> str:
        return self.world.run_id

    @property
    def url(self) -> str:
        url: str = self.network.url
        return url

    @property
    def admission(self) -> Admission:
        admission: Admission = self.store._services(self.run_id)[1]
        return admission

    @property
    def policy(self) -> Any:
        return self.admission.policy

    def push(self, rnd: int) -> None:
        self.network.push(rnd)

    def join_request(
        self,
        hot: Keypair,
        cold: Keypair,
        label: str = "request",
        *,
        expires: int = 10000,
        run_id: str | None = None,
        policy_hash: str | None = None,
    ) -> JoinRequest:
        return sign_join(
            hot,
            cold,
            run_id=run_id or self.run_id,
            request_id=sha256_hex(label.encode()),
            expires_beacon=expires,
            policy_hash=policy_hash or self.network.manifest.network.admission_policy_hash,
            hardware_hint=HardwareHint(device_name="cpu", device_count=1, driver="cpu"),
        )

    def post_join(self, request: JoinRequest | dict[str, Any]) -> httpx.Response:
        body = request.body() if isinstance(request, JoinRequest) else request
        return self.client.post(self.url + "/join", content=canonicalize(body))

    def join(
        self, hot: Keypair, cold: Keypair, label: str = "request", **kw: Any
    ) -> httpx.Response:
        return self.post_join(self.join_request(hot, cold, label, **kw))

    def admission_id(self, hot: Keypair, cold: Keypair, label: str = "request") -> str:
        response = self.join(hot, cold, label)
        precondition(response.status_code == 200, f"join refused: {response.text}")
        admission_id: str = response.json()["admission_id"]
        return admission_id

    def challenge(
        self, admission_id: str, owner: Keypair | None = None, *, as_hotkey: Keypair | None = None
    ) -> httpx.Response:
        """Ask for the admission's trial as `owner` (or `as_hotkey`); without a key, as a
        registered stranger. A base whose owner requests are signed bodies takes a POST of the
        owner's ChallengeRequest (`Network.challenge`); an earlier one takes the GET with the
        owner's signed header pair.
        """
        signer = as_hotkey or owner or STRANGER
        if hasattr(self.network, "owner_request"):
            response: httpx.Response = self.network.challenge(admission_id, signer)
            return response
        headers = trial_challenge_headers(self.run_id, admission_id, signer)
        return self.client.get(f"{self.url}/join/{admission_id}/challenge", headers=headers)

    def rotation_request(
        self, cold: Keypair, old_hot: Keypair, new_hot: Keypair | str, label: str = "rotation"
    ) -> dict[str, Any]:
        """The signed rotation as its owner sends it. `new_hot` is a Keypair when its holder takes
        part in the rotation and a bare address when the coldkey names a hotkey whose key it does
        not hold. A base whose request carries the new hotkey's consent (ADR 0010 Part 1) gets it
        signed by the Keypair; for a bare address it gets a well-formed consent of a key that is
        not that hotkey, so an unconsented rotation stays unconsented. A base without the field
        takes the three-field body.
        """
        address = new_hot if isinstance(new_hot, str) else new_hot.ss58
        expires = 10000
        operation_id = sha256_hex(label.encode())
        if "new_hot_sig" not in RotateRequest.model_fields:
            request = RotateRequest.model_validate(
                {"hotkey": old_hot.ss58, "new_hotkey": address, "operation_id": operation_id}
            )
            return sign_rotation(cold, self.run_id, request, expires)
        consent = miner_admission.sign_rotation_consent(
            new_hot if isinstance(new_hot, Keypair) else STRANGER,
            run_id=self.run_id,
            coldkey=cold.ss58,
            hotkey=old_hot.ss58,
            operation_id=operation_id,
            expires_beacon=expires,
        )
        request = RotateRequest.model_validate({**consent.body(), "new_hotkey": address})
        return sign_rotation(cold, self.run_id, request, expires)

    def rotate(
        self, cold: Keypair, old_hot: Keypair, new_hot: Keypair | str, label: str = "rotation"
    ) -> httpx.Response:
        return self.client.post(
            f"{self.url}/admission/{old_hot.ss58}/rotate",
            json=self.rotation_request(cold, old_hot, new_hot, label),
        )

    def recover(self, hot: Keypair, commit_hash: str) -> httpx.Response:
        receipt = {"w": 0, "commit_hash": commit_hash, "received_round": self.network.now}
        return self.client.post(
            f"{self.url}/admission/recover", json=self.network.signed(hot, "Receipt", receipt)
        )

    def snapshot(self) -> dict[str, list[tuple[Any, ...]]]:
        """Every row of the admission tables and of the escrow event log."""
        with self.store._lock:
            return table_rows(self.store._db)

    def row(self, hotkey: Keypair) -> dict[str, Any]:
        with self.store._lock:
            found = self.store._db.execute(
                "SELECT * FROM admissions_v2 WHERE hotkey=?", (hotkey.ss58,)
            ).fetchone()
        precondition(found is not None, f"no admission for {hotkey.ss58}")
        return dict(found)

    def count(self, table: str, where: str = "1", params: tuple[Any, ...] = ()) -> int:
        return self.world.count(table, where, params)


def detail(response: httpx.Response) -> str:
    """The `detail` of an error answer, or the whole body when there is none."""
    body = response.json()
    return str(body["detail"]) if isinstance(body, dict) and "detail" in body else str(body)


def table_rows(db: sqlite3.Connection) -> dict[str, list[tuple[Any, ...]]]:
    return {
        name: sorted(tuple(row) for row in db.execute(f'SELECT * FROM "{name}"'))
        for name in ADMISSION_TABLES
    }


@dataclass
class TrialRig:
    """One genuine shadow trial of the in-process `Admission`, cloned per test.

    `challenged` is the state after the service issued the challenge and before it computed its
    reference; `referenced` is the state after the reference was committed and before any
    proof. `proof` and `screen` are the miner's genuine pair for that challenge.
    """

    module: ModuleType
    setup: Any
    admission_id: str
    proof: WorkProof
    screen: WorkScreenV2
    challenged: Path
    referenced: Path

    def open(self, stage: str, root: Path) -> Admission:
        source = self.challenged if stage == "challenged" else self.referenced
        shutil.copytree(source, root, ignore=shutil.ignore_patterns("miner-*"))
        opened: Admission = self.module.service(self.setup, root)
        return opened

    def submit(
        self,
        admission: Admission,
        proof: WorkProof | None = None,
        screen: WorkScreenV2 | None = None,
        now: int = 2,
    ) -> Any:
        return self.module.submit(admission, proof or self.proof, screen or self.screen, now)


def build_rig(root: Path) -> TrialRig:
    """Run the one real trial: a join, a challenge, the service reference and the miner's work."""
    module = load_fixture_module(*ADMISSION_FIXTURE)
    setup = module.setup()
    referenced = root / "referenced"
    referenced.mkdir()
    admission = module.service(setup, referenced)
    admission_id = module.apply(admission, setup)
    admission.challenge(admission_id, now=2)
    challenged = root / "challenged"
    shutil.copytree(referenced, challenged, ignore=shutil.ignore_patterns("ledger.db*"))
    copy = sqlite3.connect(challenged / "ledger.db")
    admission.escrow.db.backup(copy)
    copy.close()
    admission.prepare_reference(admission_id, now=2)
    proof, screen = module.current_proof(admission, admission_id, 2)
    admission.escrow.db.close()
    return TrialRig(module, setup, admission_id, proof, screen, challenged, referenced)
