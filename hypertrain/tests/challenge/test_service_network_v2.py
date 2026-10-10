"""Signed HTTP integration over the real shared admission/accounting authority."""

from __future__ import annotations

import importlib.util
import json
import shutil
import sys
import time
from dataclasses import dataclass
from pathlib import Path

import httpx
import pytest
from fastapi.testclient import TestClient
from pydantic import BaseModel, ConfigDict, Field, StrictInt

import hypertrain
from hypertrain.challenge.app import Config, create_app
from hypertrain.ledger import Params
from hypertrain.ledger.escrow_v2 import (
    authority_message,
    genesis_allocation_hash,
    genesis_origin_id,
)
from hypertrain.miner.admission import sign_join
from hypertrain.protocol.envelope_v2 import seal
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages import f32hex
from hypertrain.protocol.messages_v2 import (
    AggregationPolicyV2,
    AuditPolicyV2,
    DisputePolicyV2,
    EconomicsPolicyV2,
    HardwareHint,
    OriginAllocation,
    RunManifestV2,
)
from hypertrain.protocol.messages_v2 import (
    TestGenesis as Genesis,
)
from hypertrain.protocol.relay_messages import RelayRegistryV1

_spec = importlib.util.spec_from_file_location(
    "network_escrow_fixture",
    Path(hypertrain.__file__).resolve().parents[2] / "tests/ledger/test_escrow_v2.py",
)
assert _spec is not None and _spec.loader is not None
fixture = importlib.util.module_from_spec(_spec)
sys.modules[_spec.name] = fixture
_spec.loader.exec_module(fixture)
COORD = fixture.COORD
OWNER = Keypair(b"\x71" * 32)
AUDITORS = [fixture.AUDITOR, Keypair(b"\x72" * 32)]
REFEREE = Keypair(b"\x73" * 32)
RELAY = Keypair(b"\x74" * 32)
HOT = [Keypair(bytes([80 + i]) * 32) for i in range(4)]
COLD = [Keypair(bytes([90 + i]) * 32) for i in range(4)]


@dataclass
class Network:
    client: TestClient
    manifest: RunManifestV2
    setup: object
    origin_ids: list[str]
    now: int = 1

    @property
    def store(self):
        return self.client.app.state.store

    @property
    def url(self) -> str:
        return "/v2/runs/" + self.manifest.run_id()

    def push(self, number: int) -> None:
        self.now = number
        self.store.clock = lambda: self.manifest.training.beacon.genesis_time + (number - 1) * 3
        response = self.client.post(
            "/v1/admin/beacon", json=fixture_beacon(number), headers=admin()
        )
        assert response.status_code == 200, response.text

    def signed(self, key, type, body, expiry=10000):
        return seal(key, type, self.manifest.run_id(), body, expiry)

    def join(self, index=0):
        request = sign_join(
            HOT[index],
            COLD[index],
            run_id=self.manifest.run_id(),
            request_id=sha256_hex(f"join-{index}".encode()),
            expires_beacon=10000,
            policy_hash=self.manifest.network.admission_policy_hash,
            hardware_hint=HardwareHint(device_name="cpu", device_count=999, driver="advisory"),
        )
        return self.client.post(self.url + "/join", content=canonicalize(request.body()))


def admin():
    return {"Authorization": "Bearer integration-admin"}


def subnet() -> httpx.MockTransport:
    """Fake master metagraph for netuid 100: fixture miners plus the rotation target."""
    hotkeys = {k.ss58: i for i, k in enumerate([*HOT, REFEREE])}
    return httpx.MockTransport(
        lambda _: httpx.Response(200, json={"netuid": 100, "hotkeys": hotkeys})
    )


class GenuineFile(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True)
    path: str
    sha256: str = Field(pattern=r"^[0-9a-f]{64}$")


class GenuineIndex(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True)
    schema_version: str = Field(alias="schema", pattern=r"^ht-genuine-cuda-context/1$")
    run_id: str = Field(pattern=r"^[0-9a-f]{64}$")
    beacon: StrictInt = Field(ge=1)
    files: dict[str, GenuineFile]
    configs: list[str] = Field(min_length=2, max_length=2)
    results: list[str] = Field(min_length=2, max_length=2)
    snapshot: dict[str, str]
    cases: dict[str, dict[str, str]]
    lifecycle: str
    d2_reservation: str


def genuine_index(path: Path):
    """Owner-pinned private index; no missing-file or synthetic fallback."""
    from hypertrain.gpu_ops.journal import fsha
    from hypertrain.miner.island_launch import confined
    from hypertrain.protocol import envelope_v2
    from hypertrain.protocol.messages import Receipt

    payload = envelope_v2.load_json(path.read_bytes())
    if not isinstance(payload["record"], dict) or not isinstance(payload["receipt"], dict):
        raise ValueError("genuine CUDA index shape differs")
    index = GenuineIndex.model_validate(payload["record"])
    signed = envelope_v2.parse_envelope(payload["receipt"])
    receipt = Receipt.model_validate(signed.body)
    if (
        signed.type != "Receipt"
        or signed.signer != OWNER.ss58
        or signed.run_id != index.run_id
        or not envelope_v2.verify_envelope(payload["receipt"])
        or receipt.commit_hash != sha256_hex(canonicalize(payload["record"]))
        or not receipt.received_round <= index.beacon <= signed.exp_drand
    ):
        raise ValueError("genuine CUDA index owner/run/expiry differs")
    files = {}
    for name, item in index.files.items():
        if (path.parent / item.path).is_symlink():
            raise ValueError("genuine CUDA index symlink refused")
        target = confined(path.parent, item.path)
        if not target.is_file() or fsha(target) != item.sha256:
            raise ValueError("genuine CUDA index file changed: " + name)
        files[name] = target
    manifest = RunManifestV2.model_validate_json(files["manifest"].read_bytes())
    if manifest.run_id() != index.run_id or manifest.training.coord_pubkey != COORD.ss58:
        raise ValueError("genuine CUDA manifest identity differs")
    if (
        manifest.training.reference_spec.image_digest
        != "sha256:c16a28dd29df300eca0cfb5c5aeacb8947681708634e3cbc8639b530321fe2e2"
    ):
        raise ValueError("genuine CUDA candidate image differs")
    if set(manifest.training.auditors) != {key.ss58 for key in AUDITORS}:
        raise ValueError("genuine CUDA auditor identities differ")
    return index, files, manifest


def genuine_network(path: Path, destination: Path, *, accepted: bool = True):
    """Validate bootstrap, then reuse exact same-run signed accepted snapshot."""
    from hypertrain.beacon.core import BeaconRound
    from hypertrain.challenge.store import ChallengeStore
    from hypertrain.data.store import LocalFSStore
    from hypertrain.gpu_ops.journal import fsha
    from hypertrain.protocol import envelope_v2

    index, files, manifest = genuine_index(path)
    params = Params(
        "hypertrain",
        manifest.training.beacon.genesis_time,
        4320,
        1,
        manifest.training.verify.E_vest_rounds,
    )
    config = Config(
        "hypertrain",
        destination / "bootstrap",
        "https://master.test",
        None,
        files["admin_token"],
        None,
        files["coord_key"],
        OWNER.ss58,
        params,
    )

    def verifier(data):
        return BeaconRound(data["round"], data["signature"], data["randomness"], False)

    raw = files["backend_receipt"].read_bytes()
    bootstrap_round = envelope_v2.parse_envelope(envelope_v2.load_json(raw)).body["received_round"]
    app = create_app(
        config,
        verify_beacon=verifier,
        transport=subnet(),
        clock=lambda: manifest.training.beacon.genesis_time + (bootstrap_round - 1) * 3,
    )
    configs, results = (
        tuple(files[n] for n in index.configs),
        tuple(files[n] for n in index.results),
    )
    with TestClient(app) as client:
        initial = app.state.store
        for name, source in files.items():
            if name.startswith("object:"):
                assert initial.objects.put(source.read_bytes()) == name.removeprefix("object:")
        response = client.post(
            "/v1/admin/beacon",
            content=files["bootstrap_beacon"].read_bytes(),
            headers=admin(),
        )
        assert response.status_code == 200, response.text
        initial._bootstrap_backend_v2(manifest, configs, results, raw)
        response = client.post(
            "/v2/admin/runs",
            content=files["manifest_envelope"].read_bytes(),
            headers=admin(),
        )
        assert response.status_code == 201, response.text
        url = "/v2/runs/" + index.run_id
        response = client.put(
            url + "/admin/dataset",
            json=json.loads(files["dataset_intake"].read_bytes()),
            headers=admin(),
        )
        assert response.status_code == 200, response.text
        response = client.post(
            url + "/admin/genesis",
            content=files["genesis_envelope"].read_bytes(),
            headers=admin(),
        )
        assert response.status_code == 200, response.text
        assert initial._backend_v2(manifest) == "cuda"
        authority = initial._record_v2(
            index.run_id, "qualification", manifest.training.reference_spec.image_digest
        )
        if accepted:
            state = destination / "accepted"
            state.mkdir(parents=True)
            for relative, name in index.snapshot.items():
                target = (state / relative).resolve()
                if not target.is_relative_to(state.resolve()):
                    raise ValueError("genuine snapshot path escapes")
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(files[name], target)
                assert fsha(target) == index.files[name].sha256
            store = ChallengeStore(
                state,
                params,
                COORD,
                OWNER.ss58,
                verifier,
                LocalFSStore(state / "objects"),
            )
            store.clock = lambda: manifest.training.beacon.genesis_time + (index.beacon - 1) * 3
            assert store._run_v2(index.run_id) == manifest
            assert (
                store._record_v2(
                    index.run_id,
                    "qualification",
                    manifest.training.reference_spec.image_digest,
                )
                == authority
            )
            assert store._backend_v2(manifest) == "cuda"
            store.require_roster_v2(index.run_id, 0)
            opening = store._record_v2(index.run_id, "round", "0")
            assert envelope_v2.verify_envelope(opening) and opening["signer"] == COORD.ss58
            assert opening == envelope_v2.load_json(files["accepted_round"].read_bytes())
            rows = store._db.execute(
                "SELECT hotkey,state,clean_count FROM admissions_v2 WHERE run_id=?",
                (index.run_id,),
            ).fetchall()
            assert {
                row["hotkey"]
                for row in rows
                if row["state"] == "ACTIVE" and row["clean_count"] >= 12
            } == {key.ss58 for key in HOT}
            trials = store._db.execute(
                "SELECT admission_id,COUNT(*) AS n FROM admission_trials "
                "WHERE finalized_beacon IS NOT NULL GROUP BY admission_id"
            ).fetchall()
            assert len(trials) == 4 and all(row["n"] >= 12 for row in trials)
            state_app = create_app(
                Config(
                    "hypertrain",
                    state,
                    "https://master.test",
                    None,
                    files["admin_token"],
                    None,
                    files["coord_key"],
                    OWNER.ss58,
                    params,
                ),
                _store=store,
                clock=store.clock,
                objects=store.objects,
                verify_beacon=verifier,
                transport=subnet(),
            )
            with TestClient(state_app) as state_client:
                store._genuine_index = index
                store._genuine_files = files
                yield (
                    Network(state_client, manifest, None, [], index.beacon),
                    configs,
                    results,
                    raw,
                )
            store.close_v2_notifications()
            store._db.close()
            return
        initial._genuine_index = index
        initial._genuine_files = files
        current = Network(client, manifest, None, [], index.beacon)
        yield current, configs, results, raw
    initial.close_v2_notifications()
    initial._db.close()


@pytest.mark.parametrize(
    "fault",
    [
        "none",
        "received",
        "accepted",
        "stored",
        "expiry",
        "owner",
        "graph",
        "scope",
        "cutoff",
        "backend",
        "source",
        "workload",
        "extra",
    ],
)
def test_shadow_operator_addressed_contract(tmp_path, fault):
    """Metadata contract only. Never accepted CUDA/store bootstrap authority."""
    import copy

    from pydantic import ValidationError

    from hypertrain.challenge.store import (
        ChallengeError,
        shadow_operator_expected,
        shadow_operator_receipt,
    )
    from hypertrain.protocol.envelope import EnvelopeError

    record = {
        "domain": "shadow-operator-admission-v1",
        "run_id": "11" * 32,
        "owner": OWNER.ss58,
        "admission_hash": "22" * 32,
        "manifest_hash": "11" * 32,
        "economics_policy_hash": "33" * 32,
        "graph_hash": "44" * 32,
        "workload": {"normal": 116, "ceiling": 126},
        "backend_qualification_authority_hash": "55" * 32,
        "source_map_hash": "66" * 32,
        "profile_hash": "77" * 32,
        "hosts": {"h0": {"instance_id": 1}, "h1": {"instance_id": 2}},
        "budget": {"scope": "FULL116_CONTINUATION"},
        "accepted_beacon": 2,
        "cutoff_unix": 100,
        "expires_beacon": 5,
    }
    expected = copy.deepcopy(record)
    received, accepted_at, expiry, signer, clock = 2, 2, 5, OWNER, 99
    if fault == "received":
        received = 3
    elif fault == "accepted":
        record["accepted_beacon"] = 3
    elif fault == "stored":
        accepted_at = 3
    elif fault == "expiry":
        expiry = 6
    elif fault == "owner":
        signer = HOT[0]
    elif fault == "graph":
        record["graph_hash"] = "88" * 32
    elif fault == "scope":
        record["budget"]["scope"] = "FIRST4_HISTORICAL_QUALIFICATION_ONLY"
    elif fault == "cutoff":
        clock = 100
    elif fault == "backend":
        record["backend_qualification_authority_hash"] = "99" * 32
    elif fault == "source":
        record["source_map_hash"] = "aa" * 32
    elif fault == "workload":
        record["workload"]["normal"] = 4
    elif fault == "extra":
        record["trusted"] = True
    receipt = seal(
        signer,
        "Receipt",
        record["run_id"],
        {
            "w": 0,
            "commit_hash": sha256_hex(canonicalize(record)),
            "received_round": received,
        },
        expiry,
    )
    raw = canonicalize({"record": record, "receipt": receipt})

    def validate():
        parsed, digest, original = shadow_operator_receipt(
            raw, OWNER.ss58, record["run_id"], accepted_at
        )
        shadow_operator_expected(parsed, expected, clock)
        return digest, original

    if fault == "none":
        first = validate()
        assert validate() == first
        # Historical recovery at original beacon survives current expiry; never grants new work.
        assert shadow_operator_receipt(raw, OWNER.ss58, record["run_id"], 2)[1] == first[0]
    else:
        with pytest.raises((ChallengeError, EnvelopeError, ValidationError)):
            validate()


@pytest.mark.parametrize("fault", ["missing", "tampered"])
def test_shadow_operator_cpu_and_missing_qualification_refused(network, tmp_path, fault):
    """Real existing Store CPU backend is NOT an operator admission positive."""
    from hypertrain.challenge.store import ChallengeError
    from hypertrain.data.store import StoreError

    run_id = network.manifest.run_id()
    assert network.store._backend_v2(network.manifest) == "cpu"
    if fault == "tampered":
        with network.store._tx():
            network.store._put_record_v2(
                run_id,
                "qualification",
                network.manifest.training.reference_spec.image_digest,
                {"authority_hash": "ab" * 32, "receipt": {}},
            )
        with pytest.raises((ChallengeError, StoreError, ValueError, KeyError)):
            network.store._backend_v2(network.manifest)
    with pytest.raises(ChallengeError):
        network.store._shadow_operator_v2(run_id)
    assert (
        network.store._db.execute(
            "SELECT COUNT(*) FROM records_v2 WHERE kind='shadow-operator'"
        ).fetchone()[0]
        == 0
    )


def test_shadow_operator_retention_transaction_only(network, tmp_path):
    """Persistence unit only: not public intake or accepted economic/GPU authority."""
    from hypertrain.challenge.store import ChallengeError

    store, run_id = network.store, network.manifest.run_id()
    payload = {"metadata_persistence_unit": True}
    digest = sha256_hex(canonicalize(payload))
    events = [tuple(row) for row in store._db.execute("SELECT * FROM escrow_events")]
    store._retain_shadow_operator_v2(run_id, payload, digest, 1, tmp_path / "graph")
    store._retain_shadow_operator_v2(run_id, payload, digest, 1, tmp_path / "graph")
    before = store._record_v2(run_id, "shadow-operator", "run")
    with pytest.raises(ChallengeError, match="immutable"):
        store._retain_shadow_operator_v2(run_id, payload, digest, 2, tmp_path / "graph")
    assert store._record_v2(run_id, "shadow-operator", "run") == before
    assert [tuple(row) for row in store._db.execute("SELECT * FROM escrow_events")] == events
    with pytest.raises(RuntimeError):
        with store._tx():
            store._retain_shadow_operator_v2("22" * 32, payload, digest, 1, tmp_path / "graph")
            raise RuntimeError("rollback")
    assert (
        store._db.execute(
            "SELECT COUNT(*) FROM records_v2 WHERE kind='shadow-operator'"
        ).fetchone()[0]
        == 1
    )


def fixture_beacon(number):
    from hypertrain.beacon.core import FixtureBeacon

    b = FixtureBeacon(current=number).get(number)
    return {"round": b.round, "signature": b.signature, "randomness": b.randomness}


@pytest.fixture
def network(tmp_path: Path, request):
    from hypertrain.beacon.core import BeaconRound

    s = fixture.setup()
    objective = getattr(request, "param", None)
    if objective is not None:
        import numpy as np

        from hypertrain.protocol.hashing import MerkleTree
        from hypertrain.trainer.config import TrainConfig

        od_spec = importlib.util.spec_from_file_location(
            "service_od_fixture",
            Path(__file__).parents[1] / "layout/test_od_island_v2.py",
        )
        od_module = importlib.util.module_from_spec(od_spec)
        od_spec.loader.exec_module(od_module)
        wrapper = od_module.od_manifest(objective, 2, True)
        od_body = wrapper.body()
        od_body["training"]["coord_pubkey"] = COORD.ss58
        od_body["training"]["reference_spec"]["image_digest"] = (
            s.manifest.training.reference_spec.image_digest
        )
        od_body["training"]["reference_spec"]["driver_allowlist"] = ["cpu"]
        od_body["training"]["reference_spec"]["sm_count"] = 1
        od_body["training"]["dataset"].update(
            n_samples=32, assign_unit=1, sample_format="u16[seq_len+1] token ids"
        )
        cfg = TrainConfig.from_manifest_v2(wrapper)
        get = od_module.samples(cfg)
        rows = np.asarray([get(i) for i in range(32)], dtype="<u2")
        tree = MerkleTree([row.tobytes() for row in rows])
        od_body["training"]["dataset"]["merkle_root"] = tree.root.hex()
        od_body["training"]["dataset"]["unit_sha256_root"] = MerkleTree(
            [bytes.fromhex(sha256_hex(row.tobytes())) for row in rows]
        ).root.hex()
        from hypertrain.trainer.compress import state_hash
        from hypertrain.trainer.model import init_params

        od_body["training"]["init_state_hash"] = state_hash(init_params(cfg.model))
        # Trial schedule may use carry once; current objective fixtures use reset.
        s = fixture.Setup(
            RunManifestV2.model_validate(od_body),
            s.policy,
            s.admission_policy,
            rows,
            tree,
        )
        limits = s.admission_policy.artifact_limits.model_copy(
            update={
                "max_object_bytes": 8_000_000,
                "max_trial_bytes": 32_000_000,
                "max_pending_upload_bytes": 64_000_000,
            }
        )
        s = fixture.Setup(
            s.manifest,
            s.policy,
            s.admission_policy.model_copy(update={"artifact_limits": limits}),
            rows,
            tree,
        )
    alloc = [(k.ss58, 10000, 1) for k in COLD]
    econ = EconomicsPolicyV2.model_validate(
        {**s.policy.body(), "genesis_allocation_hash": genesis_allocation_hash(alloc)}
    )
    agg = AggregationPolicyV2(
        arithmetic="flat-cclip-cap-v1",
        order="utf8",
        center="prev_outer_update",
        trust_mode="uniform-verified",
        weight_quantum=16777216,
        miner_cap_units=4194304,
        probation_cap_units=4194304,
        owner_group_caps={},
        preclip_norm=f32hex(1),
        cclip_tau=f32hex(1),
        cclip_iters=2,
    )
    audit = AuditPolicyV2(
        lease_rounds=100,
        max_attempts=2,
        max_concurrent_per_auditor=1,
        max_running_jobs=2,
        max_queued_jobs=32,
        max_anchor_age_rounds=1,
        max_steps_per_attempt=2 * s.manifest.training.inner.H,
    )
    dispute = DisputePolicyV2(
        referees=[REFEREE.ss58],
        max_open=32,
        max_transcript_entries=256,
        max_entry_bytes=65536,
        fanout=2,
        max_referee_reassignments=1,
        max_referee_cost_units=100,
    )
    registry = RelayRegistryV1.model_validate(
        {
            "registry_version": 1,
            "epoch": 0,
            "previous_registry_hash": None,
            "specs": [
                {
                    "id": "local",
                    "region": "local",
                    "https_url": "https://relay.test",
                    "pubkeys": [
                        {
                            "key_id": "k1",
                            "pubkey": RELAY.ss58,
                            "valid_from_round": 0,
                            "valid_until_round": 10000,
                        }
                    ],
                    "max_object_bytes": 1000000,
                    "max_inflight_bytes": 1000000,
                    "codecs": ["ht-sparse-v1", "ht-dense-int8-v1"],
                    "mode": "transport",
                },
            ],
        }
    )
    body = s.manifest.body()
    body["training"]["auditors"] = [k.ss58 for k in AUDITORS]
    body["training"]["beacon"]["genesis_time"] = int(time.time()) - 1
    for field, policy in (
        ("economics_policy_hash", econ),
        ("admission_policy_hash", s.admission_policy),
        ("aggregation_policy_hash", agg),
        ("audit_policy_hash", audit),
        ("dispute_policy_hash", dispute),
        ("relay_registry_hash", registry),
    ):
        body["network"][field] = policy.digest()
    manifest = RunManifestV2.model_validate(body)
    keyfile = tmp_path / "coord.key"
    keyfile.write_bytes(bytes(range(32)))
    token = tmp_path / "admin"
    token.write_text("integration-admin")
    cfg = Config(
        "hypertrain",
        tmp_path / "state",
        "https://master.test",
        None,
        token,
        None,
        keyfile,
        OWNER.ss58,
        Params(
            "hypertrain",
            manifest.training.beacon.genesis_time,
            4320,
            1,
            manifest.training.verify.E_vest_rounds,
        ),
    )

    def verifier(data):
        return BeaconRound(data["round"], data["signature"], data["randomness"], False)

    app = create_app(cfg, verify_beacon=verifier, transport=subnet())
    with TestClient(app) as client:
        n = Network(client, manifest, s, [])
        for policy in (econ, s.admission_policy, agg, audit, dispute, registry):
            assert n.store.objects.put(canonicalize(policy.body())) == policy.digest()
        n.push(1)
        response = client.post(
            "/v2/admin/runs",
            content=canonicalize(seal(OWNER, "RunManifestV2", manifest.run_id(), manifest, 10000)),
            headers=admin(),
        )
        assert response.status_code == 201, response.text
        samples_hash = n.store.objects.put(s.rows.tobytes())
        proofs_hash = n.store.objects.put(
            canonicalize([[p.hex() for p in s.tree.proof(i)] for i in range(len(s.rows))])
        )
        response = client.put(
            n.url + "/admin/dataset",
            json={"samples_hash": samples_hash, "proofs_hash": proofs_hash},
            headers=admin(),
        )
        assert response.status_code == 200, response.text
        origins = [
            OriginAllocation(
                origin_id=genesis_origin_id(manifest.run_id(), econ.genesis_allocation_hash, i),
                owner=k.ss58,
                units=10000,
                mature_at=1,
            )
            for i, k in enumerate(COLD)
        ]
        genesis = Genesis(
            run_id=manifest.run_id(),
            allocation_hash=econ.genesis_allocation_hash,
            total_units=40000,
            origins=origins,
            authority_sig="0" * 128,
        )
        genesis = genesis.model_copy(
            update={"authority_sig": COORD.sign(authority_message(genesis)).hex()}
        )
        response = client.post(
            n.url + "/admin/genesis",
            content=canonicalize(n.signed(COORD, "TestGenesis", genesis)),
            headers=admin(),
        )
        assert response.status_code == 200, response.text
        n.origin_ids = [o.origin_id for o in origins]
        yield n


@pytest.fixture(scope="module")
def shadow_reservation_work(tmp_path_factory):
    """One genuine independent reference/miner pair; clone original custody per negative."""
    import json
    import shutil
    from types import SimpleNamespace

    from hypertrain.auditor.replay import unpack_state
    from hypertrain.data.trial_assignment import trial_samples
    from hypertrain.gpu_ops.work_screen import screen_work
    from hypertrain.protocol.hashing import MerkleTree
    from hypertrain.protocol.messages import Finalize, LeafPreimage
    from hypertrain.protocol.messages_v2 import CommitV2, JoinChallenge, WorkProof
    from hypertrain.trainer.compress import state_hash

    generator = network.__wrapped__(
        tmp_path_factory.mktemp("shadow-reservation-work"), SimpleNamespace(param=None)
    )
    n = next(generator)
    try:
        identity = n.join().json()["admission_id"]
        n.push(2)
        admission = n.store._services(n.manifest.run_id())[1]
        challenge_wire = admission.challenge(identity, now=2)
        challenge = JoinChallenge.model_validate(challenge_wire["body"])
        n.store.trial_reference_v2(n.manifest.run_id(), identity, retain_shadow_custody=True)
        row = n.store._db.execute(
            "SELECT * FROM admissions_v2 WHERE admission_id=?", (identity,)
        ).fetchone()
        samples = trial_samples(n.manifest, identity, challenge.nonce, n.store._beacon_v2(2))
        job, reference_directory = n.store.stage_trial_v2(
            n.manifest.run_id(), challenge, samples, row["trial_epoch"]
        )
        miner_directory = n.store.state_dir / "original-miner" / challenge.nonce
        miner_directory.mkdir(parents=True)
        for relative in job.object_paths.values():
            shutil.copyfile(reference_directory / relative, miner_directory / relative)
        screen, proof = screen_work(job, miner_directory, challenge, now_beacon=2, backend="cpu")
        assert proof == WorkProof.model_validate_json(row["reference_json"])
        for name in ("state.safetensors", "ef.safetensors", "delta.bin", "leaves.json"):
            n.store.objects.put((miner_directory / "published/rank-0" / name).read_bytes())
        proof_wire = n.signed(HOT[0], "WorkProof", proof)
        screen_wire = n.signed(HOT[0], "WorkScreenV2", screen)
        response = n.client.post(
            n.url + "/join/proof",
            json={
                "proof": proof_wire,
                "screen": screen_wire,
            },
        )
        assert response.status_code == 200, response.text
        publication = miner_directory / "published"
        summaries = json.loads((publication / "rank-0/summary.json").read_bytes())["commitments"]
        leaves = [
            LeafPreimage.model_validate(p)
            for p in json.loads((publication / "rank-0/leaves.json").read_bytes())
        ]
        commit = CommitV2(
            w=job.w,
            hotkey=HOT[0].ss58,
            leaf_scheme="ht-leaf-v1",
            n_leaves=len(leaves),
            metrics_root=MerkleTree(
                [bytes.fromhex(p.loss_f32) + bytes.fromhex(p.norm_f32) for p in leaves]
            ).root.hex(),
            tokens=len(job.sample_ids) * n.manifest.training.model.seq_len,
            delta_bytes=(publication / "rank-0/delta.bin").stat().st_size,
            ef_in_hash=state_hash(
                unpack_state((publication / job.object_paths["ef_in"]).read_bytes())[0]
            ),
            **{
                k: summaries[k]
                for k in (
                    "leaves_root",
                    "final_theta_hash",
                    "ef_out_hash",
                    "delta_hash",
                )
            },
        )
        commit_wire = n.signed(HOT[0], "CommitV2", commit)
        with admission.escrow.tx():
            miner = admission.retain_trial_publication(
                "probe",
                job,
                miner_directory,
                screen,
                proof,
                2,
                commit_raw=canonicalize(commit_wire),
            )
        reference_row = n.store._db.execute(
            "SELECT id,data FROM records_v2 WHERE kind='shadow-execution' "
            "AND json_extract(data,'$.operation')='reference'"
        ).fetchone()
        reference = json.loads(reference_row["data"])
        final = Finalize(
            w=job.w,
            included=[HOT[0].ss58],
            entitlements_root=proof.digest(),
            final_theta_hash_w1=commit.final_theta_hash,
        )
        final_wire = n.signed(COORD, "Finalize", final)
        response = n.client.post(
            n.url + "/admin/join/" + identity + "/finalize",
            json=final_wire,
            headers=admin(),
        )
        assert response.status_code == 200, response.text
        custody = {
            "job_hash": miner["job_hash"],
            "reference_operation_hash": reference_row["id"],
            "miner_operation_hash": miner["operation_hash"],
        }
        for prefix, subject in (("reference", reference), ("miner", miner)):
            for key in ("publication_hash", "result_hash"):
                custody[prefix + "_" + key] = subject[key]
        yield (
            n,
            identity,
            {"finalize": final_wire, "commit": commit_wire, "custody": custody},
        )
    finally:
        generator.close()


@pytest.fixture
def reserved_trial(shadow_reservation_work, tmp_path):
    """Clone real accepted SQLite/object/publication bytes; never regenerate work."""
    import json
    import shutil
    import sqlite3

    from hypertrain.challenge.store import ChallengeStore
    from hypertrain.data.store import LocalFSStore

    original, identity, body = shadow_reservation_work
    state = tmp_path / "state"
    state.mkdir()
    with sqlite3.connect(state / "challenge.db") as copied:
        original.store._db.backup(copied)
    for name in ("objects", "trials-v2", "original-miner", "ledger"):
        shutil.copytree(original.store.state_dir / name, state / name)
    store = ChallengeStore(
        state,
        original.store.params,
        COORD,
        OWNER.ss58,
        original.store.verify_beacon,
        LocalFSStore(state / "objects"),
    )
    store.clock = lambda: original.manifest.training.beacon.genesis_time + 3
    with store._tx():
        for row in store._db.execute(
            "SELECT id,data FROM records_v2 WHERE kind='shadow-execution'"
        ).fetchall():
            record = json.loads(row["data"])
            record["directory"] = str(
                state / Path(record["directory"]).relative_to(original.store.state_dir)
            )
            store._db.execute(
                "UPDATE records_v2 SET data=? WHERE kind='shadow-execution' AND id=?",
                (canonicalize(record).decode(), row["id"]),
            )
    cfg = Config(
        "hypertrain",
        state,
        "https://master.test",
        None,
        original.client.app.state.store.state_dir.parent / "admin",
        None,
        original.store.state_dir.parent / "coord.key",
        OWNER.ss58,
        store.params,
    )
    app = create_app(
        cfg,
        verify_beacon=store.verify_beacon,
        _store=store,
        clock=store.clock,
        transport=subnet(),
    )
    with TestClient(app) as client:
        yield (
            Network(client, original.manifest, original.setup, original.origin_ids, 2),
            identity,
            json.loads(canonicalize(body)),
        )


@pytest.fixture(scope="module")
def runtime_store_work(tmp_path_factory):
    original = fixture.setup
    try:
        fixture.setup = lambda mode="production": original("test")
        actual = fixture.actual_reward.__wrapped__(tmp_path_factory)
    finally:
        fixture.setup = original
    base = tmp_path_factory.getbasetemp()
    return (
        fixture,
        actual,
        next(base.glob("real-reward*/published")),
        next(base.glob("real-reference*/published")),
    )


@pytest.fixture
def accepted_runtime_store(runtime_store_work, tmp_path, monkeypatch):
    import json
    import shutil
    from types import SimpleNamespace

    from hypertrain.beacon.core import FixtureBeacon
    from hypertrain.challenge.admission import Admission
    from hypertrain.data.store import LocalFSStore
    from hypertrain.data.trial_assignment import trial_samples
    from hypertrain.gpu_ops.work_screen import screen_work
    from hypertrain.miner.island_launch import validate_artifacts
    from hypertrain.protocol.messages_v2 import IslandJobV1, JoinChallenge

    module, actual, retained, reference = runtime_store_work
    setup = actual[0]
    escrow = module.ledger(setup, tmp_path / "ledger.db")
    objects = LocalFSStore(tmp_path / "objects")
    beacon = FixtureBeacon(current=2)
    job = IslandJobV1.model_validate_json((retained / "job.json").read_bytes())

    def stage(challenge, samples, epoch):
        assert samples == (0,) and epoch == job.w
        directory = tmp_path / "reference"
        publication = directory / "published"
        shutil.copytree(reference, publication)
        bound = job.model_copy(
            update={
                "deadline": job.manifest.training.beacon.genesis_time
                + (challenge.deadline_beacon - 1) * 3,
            }
        )
        (publication / "job.json").write_bytes(canonicalize(bound.body()))
        summary_path = publication / "rank-0/summary.json"
        summary = json.loads(summary_path.read_bytes())
        summary["job_hash"] = sha256_hex(canonicalize(bound.body()))
        summary_path.write_bytes(canonicalize(summary))
        for relative in bound.object_paths.values():
            shutil.copyfile(publication / relative, directory / relative)
        return bound, directory

    def qualified(screen):
        return (
            screen.layout == setup.manifest.training.reference_spec.layout
            and screen.image_digest == setup.manifest.training.reference_spec.image_digest
        )

    admission = Admission(
        escrow,
        setup.admission_policy,
        module.COORD,
        beacon=beacon.get,
        stage_reference=stage,
        objects=objects,
        ip_secret=b"fixture",
        qualified=qualified,
        conservative_bound=lambda: False,
        backend="cpu",
    )
    joined = admission.join(
        canonicalize(
            sign_join(
                module.HOT,
                module.COLD,
                run_id=job.run_id,
                request_id="19" * 32,
                expires_beacon=10000,
                policy_hash=setup.admission_policy.digest(),
                hardware_hint=HardwareHint(device_name="cpu", device_count=1, driver="cpu"),
            ).body()
        ),
        now=1,
        ip_prefix="203.0.113.0/24",
    )
    nonce = next(
        f"{i:064x}"
        for i in range(4096)
        if trial_samples(setup.manifest, joined.admission_id, f"{i:064x}", beacon.get(2)) == (0,)
    )
    monkeypatch.setattr("hypertrain.challenge.admission.secrets.token_hex", lambda n: nonce)
    wire = admission.challenge(joined.admission_id, now=2)
    challenge = JoinChallenge.model_validate(wire["body"])
    monkeypatch.setattr(time, "time", lambda: setup.manifest.training.beacon.genesis_time + 3)
    admission.prepare_reference(
        joined.admission_id,
        now=2,
        trusted_launch=lambda job, directory, challenge, epoch: (
            lambda job, directory, **kwargs: validate_artifacts(job, directory / "published")
        ),
    )
    spec = importlib.util.spec_from_file_location(
        "store_runtime_fixture",
        Path(hypertrain.__file__).resolve().parents[2] / "tests/gpu_ops/test_runtime_custody.py",
    )
    assert spec and spec.loader
    runtime_module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = runtime_module
    spec.loader.exec_module(runtime_module)
    monkeypatch.setattr(runtime_module, "JoinChallenge", lambda **kwargs: challenge)
    source = tmp_path / "retained"
    shutil.copytree(retained, source)
    source_job = job.model_copy(
        update={
            "deadline": setup.manifest.training.beacon.genesis_time
            + (challenge.deadline_beacon - 1) * 3
        }
    )
    (source / "job.json").write_bytes(canonicalize(source_job.body()))
    (tmp_path / "runtime").mkdir()
    import network_gpu_operation as producer

    operation_type = producer.Operation
    with monkeypatch.context() as patch:
        patch.setattr(
            producer,
            "Operation",
            lambda **kwargs: operation_type(**{**kwargs, "binding": challenge.admission_id}),
        )
        n = runtime_module.completed.__wrapped__(
            tmp_path / "runtime",
            monkeypatch,
            SimpleNamespace(),
            (module, actual, source, reference),
        )
    from experiments.gpu_network_v2.orchestrate import NetworkRuntime

    n.runtime = NetworkRuntime(n.runtime.lifecycle, n.runtime.profile)
    artifacts = validate_artifacts(n.spec.job, n.original / "published")
    screen, proof = screen_work(
        n.spec.job,
        n.original / "published",
        challenge,
        now_beacon=2,
        backend="cpu",
        launch=lambda job, directory, **kwargs: artifacts,
    )
    for path in (artifacts.state, artifacts.ef, artifacts.delta, artifacts.leaves):
        objects.put(path.read_bytes())
    admission.proof(
        canonicalize(seal(module.HOT, "WorkProof", job.run_id, proof, 10000)),
        canonicalize(seal(module.HOT, "WorkScreenV2", job.run_id, screen, 10000)),
        now=2,
    )
    return SimpleNamespace(admission=admission, escrow=escrow, n=n, proof=proof, screen=screen)


def test_internal_runtime_custody_retains_only_authenticated_completion(
    accepted_runtime_store,
):
    item = accepted_runtime_store
    before = item.escrow.balances()
    events = [tuple(row) for row in item.escrow.db.execute("SELECT * FROM escrow_events")]
    n = item.n
    first = item.admission.retain_runtime_custody(
        n.runtime,
        n.spec,
        n.directory,
        tree=Path(hypertrain.__file__).resolve().parents[2],
        now=2,
    )
    assert first["status"] == "NOTSTOREACCEPTED"
    assert (
        item.admission.retain_runtime_custody(
            n.runtime,
            n.spec,
            n.directory,
            tree=Path(hypertrain.__file__).resolve().parents[2],
            now=2,
        )
        == first
    )
    assert item.escrow.balances() == before
    assert [tuple(row) for row in item.escrow.db.execute("SELECT * FROM escrow_events")] == events
    assert (
        item.escrow.db.execute(
            "SELECT COUNT(*) FROM records_v2 WHERE kind='runtime-custody'"
        ).fetchone()[0]
        == 1
    )
    assert (
        item.escrow.db.execute(
            "SELECT COUNT(*) FROM records_v2 WHERE kind='shadow-execution'"
        ).fetchone()[0]
        == 0
    )


@pytest.mark.parametrize(
    "fault",
    [
        "owner",
        "challenge",
        "rescue",
        "cancel",
        "reference",
        "proof",
        "changed",
        "lookalike",
    ],
)
def test_internal_runtime_custody_refuses_changed_authority(accepted_runtime_store, fault):
    import threading

    from experiments.gpu_network_v2.orchestrate import Reject

    from hypertrain.challenge.admission_store import AdmissionError
    from hypertrain.protocol.envelope_v2 import SignatureError

    item, cancel = accepted_runtime_store, threading.Event()
    n = item.n
    if fault == "owner":
        old = n.runtime.journal.last("network_operation_authority")
        n.runtime.journal.append(
            "network_operation_authority",
            **{k: v for k, v in old.items() if k not in ("kind", "unix", "pid", "receipt")},
            receipt={**old["receipt"], "sig": "00" * 64},
        )
    elif fault == "rescue":
        n.archive_path.write_bytes(n.archive_path.read_bytes() + b"changed")
    elif fault == "cancel":
        cancel.set()
    elif fault == "reference":
        item.escrow.db.execute("UPDATE admissions_v2 SET reference_json=NULL")
    elif fault == "proof":
        item.escrow.db.execute("UPDATE admission_trial_results SET proof=NULL")
    elif fault == "lookalike":
        n.runtime = {"body": n.custody}
    else:
        if fault == "changed":
            item.admission.retain_runtime_custody(
                n.runtime,
                n.spec,
                n.directory,
                tree=Path(hypertrain.__file__).resolve().parents[2],
                now=2,
            )
            n.custody["body"]["environment"]["extra"] = "changed"
        else:
            n.custody["body"]["trial_authority"]["envelope"]["sig"] = "00" * 64
            trial = n.custody["body"]["trial_authority"]
            trial["sha256"] = sha256_hex(canonicalize(trial["envelope"]))
        n.custody["sha256"] = sha256_hex(canonicalize(n.custody["body"]))
        import network_gpu_operation as producer

        for name in (
            "publication-custody.json",
            "operation-result.json",
            "execution-custody.json",
        ):
            (n.original / name).unlink()
        producer.publish_custody_result(n.spec, n.original, n.result, n.custody, threading.Event())
        n.archive()
    before = item.escrow.balances()
    records = [tuple(row) for row in item.escrow.db.execute("SELECT * FROM records_v2")]
    events = [tuple(row) for row in item.escrow.db.execute("SELECT * FROM escrow_events")]
    with pytest.raises((AdmissionError, Reject, SignatureError)):
        item.admission.retain_runtime_custody(
            n.runtime,
            n.spec,
            n.directory,
            tree=Path(hypertrain.__file__).resolve().parents[2],
            now=2,
            cancel=cancel,
        )
    assert item.escrow.balances() == before
    assert [tuple(row) for row in item.escrow.db.execute("SELECT * FROM records_v2")] == records
    assert [tuple(row) for row in item.escrow.db.execute("SELECT * FROM escrow_events")] == events


def test_internal_runtime_custody_retains_accepted_reference(accepted_runtime_store):
    import io
    import json
    import shutil
    import tarfile
    import threading

    import network_gpu_operation as producer
    from hypertrain.gpu_ops.journal import fsha
    from hypertrain.miner.island_launch import validate_artifacts
    from hypertrain.protocol.messages import Receipt

    item, n = accepted_runtime_store, accepted_runtime_store.n
    spec = n.spec.model_copy(update={"operation": "reference"})
    name = "op-" + sha256_hex(
        (spec.job.run_id + spec.operation + spec.hotkey + spec.binding + spec.job.digest()).encode()
    )
    original = n.directory / name
    original.mkdir()
    shutil.copytree(n.original / "published", original / "published")
    artifacts = validate_artifacts(spec.job, original / "published")
    custody = producer.publication_custody(
        spec, original, artifacts, n.custody["body"]["trial_authority"]["envelope"]
    )
    result = {
        "operation": "reference",
        "binding": spec.binding,
        "hotkey": spec.hotkey,
        "role": spec.role,
        "job_sha256": spec.job.digest(),
        "publication": "published",
        "status": "CAPTURED_NOT_ACCEPTED",
    }
    producer.publish_custody_result(spec, original, result, custody, threading.Event())
    subject = {
        "role": spec.role,
        "binding": spec.binding,
        "operation": spec.operation,
        "hotkey": spec.hotkey,
        "spec_sha256": sha256_hex(spec.model_dump_json().encode()),
    }
    receipt = seal(
        n.owner,
        "Receipt",
        spec.job.run_id,
        Receipt(
            w=spec.job.w,
            commit_hash=sha256_hex(canonicalize(subject)),
            received_round=1,
        ),
        10000,
    )
    n.runtime.accept_operation(spec, canonicalize(receipt), owner=spec.owner, beacon=1)
    n.runtime.journal.append(
        "network_execution_intent",
        role=spec.role,
        name=name,
        phase="workload",
        executions=1,
    )
    n.runtime.journal.append(
        "network_operation_started",
        role=spec.role,
        name=name,
        binding=spec.binding,
        job_sha256=spec.job.digest(),
        operation=spec.operation,
        hotkey=spec.hotkey,
        sources_sha256=sha256_hex(json.dumps(spec.sources, sort_keys=True).encode()),
        cutoff=spec.cutoff,
    )
    archive_path = n.directory / "reference-rescued.tar"
    with tarfile.open(archive_path, "w") as archive:
        for path in sorted(original.rglob("*")):
            if path.is_file():
                raw = path.read_bytes()
                member = tarfile.TarInfo("out/" + name + "/" + str(path.relative_to(original)))
                member.size = len(raw)
                archive.addfile(member, io.BytesIO(raw))
    n.runtime.journal.append(
        "network_rescued",
        role=spec.role,
        tar=str(archive_path),
        tar_sha256=fsha(archive_path),
    )
    n.runtime.journal.append(
        "network_operation_done",
        role=spec.role,
        name=name,
        result_sha256=fsha(original / "operation-result.json"),
        tar_sha256=fsha(archive_path),
    )
    before = item.escrow.balances()
    events = [tuple(row) for row in item.escrow.db.execute("SELECT * FROM escrow_events")]
    retained = item.admission.retain_runtime_custody(
        n.runtime,
        spec,
        n.directory,
        tree=Path(hypertrain.__file__).resolve().parents[2],
        now=2,
    )
    assert retained["status"] == "NOTSTOREACCEPTED"
    assert item.escrow.balances() == before
    assert [tuple(row) for row in item.escrow.db.execute("SELECT * FROM escrow_events")] == events


@pytest.mark.parametrize("fault", ["none", "unsigned", "origin", "challenge"])
def test_store_runtime_custody_wrapper_keeps_acceptance_private(
    reserved_trial, tmp_path, monkeypatch, fault
):
    """Real Store and accepted CPU work; runtime metadata grants no production authority."""
    import json
    from types import SimpleNamespace

    from experiments.gpu_network_v2.orchestrate import NetworkRuntime, Reject

    from hypertrain.challenge.admission_store import AdmissionError
    from hypertrain.challenge.store import ChallengeError
    from hypertrain.protocol.envelope_v2 import SignatureError
    from hypertrain.protocol.messages_v2 import IslandJobV1, JoinChallenge

    # Given genuine accepted reference/proof/finalization and authenticated CPU custody.
    network, identity, body = reserved_trial
    store, run_id = network.store, network.manifest.run_id()
    escrow, admission, _ = store._services(run_id)
    row = store._db.execute(
        "SELECT challenge FROM admissions_v2 WHERE admission_id=?", (identity,)
    ).fetchone()
    challenge = JoinChallenge.model_validate_json(row[0])
    records = store._db.execute(
        "SELECT data FROM records_v2 WHERE kind='shadow-execution'"
    ).fetchall()
    publications = {
        json.loads(r[0])["operation"]: Path(json.loads(r[0])["directory"]) for r in records
    }
    job = IslandJobV1.model_validate_json((publications["probe"] / "job.json").read_bytes())
    spec = importlib.util.spec_from_file_location(
        "public_custody_runtime_fixture",
        Path(hypertrain.__file__).resolve().parents[2] / "tests/gpu_ops/test_runtime_custody.py",
    )
    assert spec and spec.loader
    runtime_fixture = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = runtime_fixture
    spec.loader.exec_module(runtime_fixture)
    monkeypatch.setattr(runtime_fixture, "JoinChallenge", lambda **kwargs: challenge)
    keys = SimpleNamespace(COORD=COORD, HOT=HOT[0], OTHER=OWNER)
    runtime_dir = tmp_path / "runtime-wrapper"
    runtime_dir.mkdir()
    n = runtime_fixture.completed.__wrapped__(
        runtime_dir,
        monkeypatch,
        SimpleNamespace(),
        (keys, None, publications["probe"], publications["reference"]),
    )
    n.runtime = NetworkRuntime(n.runtime.lifecycle, n.runtime.profile)
    assert n.spec.job == job and n.spec.binding == identity
    original_runtime = n.runtime
    if fault == "unsigned":
        n.runtime = {"body": n.custody}
    elif fault == "origin":
        old = n.runtime.journal.last("network_operation_authority")
        n.runtime.journal.append(
            "network_operation_authority",
            **{k: v for k, v in old.items() if k not in ("kind", "unix", "pid", "receipt")},
            receipt={**old["receipt"], "sig": "00" * 64},
        )
    elif fault == "challenge":
        n.spec = n.spec.model_copy(update={"binding": "ff" * 32})
    before = escrow.balances()
    events = [tuple(r) for r in store._db.execute("SELECT * FROM escrow_events")]
    original_records = [tuple(r) for r in store._db.execute("SELECT * FROM records_v2")]
    tree = Path(hypertrain.__file__).resolve().parents[2]

    # When the canonical wrapper consumes original runtime authority, never a public descriptor.
    if fault == "none":
        retained = store.retain_runtime_custody_v2(
            run_id, n.runtime, n.spec, n.directory, tree=tree
        )
        assert retained["status"] == "NOTSTOREACCEPTED"
        assert (
            store.retain_runtime_custody_v2(run_id, n.runtime, n.spec, n.directory, tree=tree)
            == retained
        )
        assert (
            store._db.execute(
                "SELECT COUNT(*) FROM records_v2 WHERE kind='runtime-custody'"
            ).fetchone()[0]
            == 1
        )
    else:
        with pytest.raises((AdmissionError, Reject, SignatureError)):
            store.retain_runtime_custody_v2(run_id, n.runtime, n.spec, n.directory, tree=tree)
        assert [tuple(r) for r in store._db.execute("SELECT * FROM records_v2")] == original_records

    # Then accepted execution custody remains separate; no funds, events or production grant.
    assert escrow.balances() == before
    assert [tuple(r) for r in store._db.execute("SELECT * FROM escrow_events")] == events
    assert [
        tuple(r)
        for r in store._db.execute("SELECT data FROM records_v2 WHERE kind='shadow-execution'")
    ] == [tuple(r) for r in records]
    assert admission.backend == "cpu" and not admission.status(HOT[0].ss58, now=2).eligible
    with pytest.raises(ChallengeError):
        store._shadow_operator_v2(run_id)
    assert type(original_runtime) is NetworkRuntime


def test_shadow_reservation_uses_original_accepted_trial(reserved_trial):
    n, identity, body = reserved_trial
    escrow = n.store._services(n.manifest.run_id())[0]
    before = escrow.balances()
    response = n.client.post(
        n.url + "/admin/join/" + identity + "/shadow-reservation",
        json=body,
        headers=admin(),
    )
    assert response.status_code == 200, response.text
    receipt = response.json()
    assert receipt["shadow_ordinal"] == 0
    assert receipt["reservation_hash"] == sha256_hex(canonicalize(receipt["reservation"]))
    assert escrow.balances() == before
    assert n.store._db.execute("SELECT COUNT(*) FROM escrow_shadow_finalized").fetchone()[0] == 0


@pytest.mark.parametrize(
    "fault",
    [
        "admin",
        "extra",
        "path",
        "signature",
        "finalize-signer",
        "changed-finalize",
        "changed-commit",
        "captured-only",
        "same-execution",
        "publication",
        "nonce",
        "backend",
        "dispute",
        "wrong-admission",
        "wrong-run",
        "missing-execution",
        "expired",
    ],
)
def test_shadow_reservation_rejects_unbound_original_custody(reserved_trial, fault):
    import json

    n, identity, body = reserved_trial
    headers = admin()
    if fault == "admin":
        headers = {}
    elif fault == "extra":
        body["MATCH"] = True
    elif fault == "path":
        body["custody"]["job_hash"] = "../../published"
    elif fault == "signature":
        body["commit"]["sig"] = "0" * 128
    elif fault == "finalize-signer":
        body["finalize"] = n.signed(HOT[0], "Finalize", body["finalize"]["body"])
    elif fault == "changed-finalize":
        body["finalize"]["body"]["final_theta_hash_w1"] = "ff" * 32
        body["finalize"] = n.signed(COORD, "Finalize", body["finalize"]["body"])
    elif fault == "changed-commit":
        body["commit"]["body"]["metrics_root"] = "ff" * 32
        body["commit"] = n.signed(HOT[0], "CommitV2", body["commit"]["body"])
    elif fault == "captured-only":
        body["custody"]["miner_operation_hash"] = n.store.objects.put(
            canonicalize(
                {
                    "status": "CAPTURED_NOT_ACCEPTED",
                    "MATCH": True,
                }
            )
        )
    elif fault == "missing-execution":
        n.store._db.execute(
            "DELETE FROM records_v2 WHERE kind='shadow-execution' AND id=?",
            (body["custody"]["miner_operation_hash"],),
        )
    elif fault == "expired":
        body["commit"] = n.signed(HOT[0], "CommitV2", body["commit"]["body"], expiry=1)
    elif fault == "same-execution":
        body["custody"]["miner_operation_hash"] = body["custody"]["reference_operation_hash"]
    elif fault == "wrong-admission":
        identity = "ff" * 32
    elif fault == "wrong-run":
        body["commit"] = seal(HOT[0], "CommitV2", "ff" * 32, body["commit"]["body"], 10000)
    elif fault == "dispute":
        admission = n.store._services(n.manifest.run_id())[1]
        admission.suspend(
            HOT[0].ss58,
            now=2,
            reason="PENDING_DISPUTE",
            pending_dispute=True,
            dispute_id="fa" * 32,
            evidence_hash="fb" * 32,
        )
    else:
        record = n.store._record_v2(
            n.manifest.run_id(),
            "shadow-execution",
            body["custody"]["reference_operation_hash"],
        )
        publication = Path(record["directory"])
        if fault == "publication":
            (publication / "rank-0/delta.bin").write_bytes(b"changed original bytes")
        elif fault == "backend":
            path = publication / "rank-0/summary.json"
            summary = json.loads(path.read_bytes())
            summary["backend"] = "cuda"
            path.write_bytes(canonicalize(summary))
        elif fault == "nonce":
            row = n.store._db.execute(
                "SELECT epoch,challenge FROM admission_trial_results"
            ).fetchone()
            challenge = json.loads(row["challenge"])
            challenge["nonce"] = "ff" * 32
            n.store._db.execute(
                "UPDATE admission_trial_results SET challenge=? WHERE epoch=?",
                (canonicalize(challenge).decode(), row["epoch"]),
            )
    escrow = n.store._services(n.manifest.run_id())[0]
    before = escrow.balances()
    response = n.client.post(
        n.url + "/admin/join/" + identity + "/shadow-reservation",
        json=body,
        headers=headers,
    )
    assert response.status_code in (401, 403, 409, 422), response.text
    assert escrow.balances() == before
    assert (
        n.store._db.execute(
            "SELECT COUNT(*) FROM records_v2 WHERE kind='shadow-reservation'"
        ).fetchone()[0]
        == 0
    )


def test_shadow_reservation_authenticated_recovery_precedes_new_predicates(
    reserved_trial,
):
    n, identity, body = reserved_trial
    url = n.url + "/admin/join/" + identity + "/shadow-reservation"
    first = n.client.post(url, json=body, headers=admin())
    assert first.status_code == 200, first.text
    admission = n.store._services(n.manifest.run_id())[1]
    admission.suspend(
        HOT[0].ss58,
        now=2,
        reason="PENDING_DISPUTE",
        pending_dispute=True,
        dispute_id="fc" * 32,
        evidence_hash="fd" * 32,
    )
    # Authentic identical semantic recovery ignores mutable new-request eligibility.
    for name, key in (("finalize", COORD), ("commit", HOT[0])):
        body[name] = n.signed(key, body[name]["type"], body[name]["body"], expiry=20000)
    recovered = n.client.post(url, json=body, headers=admin())
    assert recovered.status_code == 200, recovered.text
    assert recovered.json() == first.json()
    body["commit"]["sig"] = "0" * 128
    assert n.client.post(url, json=body, headers=admin()).status_code == 409
    assert (
        n.store._db.execute(
            "SELECT COUNT(*) FROM records_v2 WHERE kind='shadow-reservation'"
        ).fetchone()[0]
        == 1
    )


def test_shadow_reservation_pending_never_reassigns_ordinal(reserved_trial):
    n, identity, body = reserved_trial
    url = n.url + "/admin/join/" + identity + "/shadow-reservation"
    first = n.client.post(url, json=body, headers=admin())
    assert first.status_code == 200, first.text
    # Later genuine signed challenge is allowed; cannot erase historical reservation custody.
    n.push(3)
    admission = n.store._services(n.manifest.run_id())[1]
    admission.challenge(identity, now=3)
    recovered = n.client.post(url, json=body, headers=admin())
    assert recovered.status_code == 200, recovered.text
    assert recovered.json() == first.json()
    # Outcome-only mutation cannot replace or invalidate authenticated historical recovery.
    try:
        n.store._db.execute(
            "UPDATE admission_trial_results SET outcome='MISMATCH' WHERE epoch=?",
            (body["finalize"]["body"]["w"],),
        )
        escrow = n.store._services(n.manifest.run_id())[0]
        before = escrow.balances()
        tables = (
            "records_v2",
            "admission_reservations",
            "beacon_pins_v2",
            "escrow_events",
        )

        def snapshot():
            return {
                t: sorted(tuple(r) for r in n.store._db.execute(f'SELECT * FROM "{t}"'))
                for t in tables
            }

        records = snapshot()
        repeated = n.client.post(url, json=body, headers=admin())
        assert repeated.status_code == 200, repeated.text
        assert repeated.json() == first.json()
        assert escrow.balances() == before and snapshot() == records
    finally:
        n.store._db.execute(
            "UPDATE admission_trial_results SET outcome='MATCH' WHERE epoch=?",
            (body["finalize"]["body"]["w"],),
        )
    assert (
        n.store._db.execute(
            "SELECT COUNT(*) FROM records_v2 WHERE kind='shadow-reservation'"
        ).fetchone()[0]
        == 1
    )


def test_shadow_new_reservation_refuses_nonmatch_original_trial(reserved_trial):
    n, identity, body = reserved_trial
    with n.store._services(n.manifest.run_id())[0].tx():
        n.store._db.execute(
            "UPDATE admission_trial_results SET outcome='MISMATCH' WHERE epoch=?",
            (body["finalize"]["body"]["w"],),
        )
    escrow = n.store._services(n.manifest.run_id())[0]
    before = escrow.balances()
    tables = ("records_v2", "admission_reservations", "beacon_pins_v2", "escrow_events")

    def snapshot():
        return {
            t: sorted(tuple(r) for r in n.store._db.execute(f'SELECT * FROM "{t}"')) for t in tables
        }

    records = snapshot()
    response = n.client.post(
        n.url + "/admin/join/" + identity + "/shadow-reservation",
        json=body,
        headers=admin(),
    )
    assert (
        response.status_code == 409 and "new reservation requires original MATCH" in response.text
    )
    assert escrow.balances() == before and snapshot() == records
    assert (
        n.store._db.execute(
            "SELECT COUNT(*) FROM records_v2 WHERE kind='shadow-reservation'"
        ).fetchone()[0]
        == 0
    )


@pytest.mark.parametrize("refusal", ["pending", "cap"])
def test_shadow_reservation_refusal_rolls_back_freshness(reserved_trial, monkeypatch, refusal):
    from hypertrain.ledger.escrow_v2 import EscrowV2, ShadowReservationEvidence
    from hypertrain.protocol import envelope_v2
    from hypertrain.protocol.envelope import body_digest
    from hypertrain.protocol.messages_v2 import ShadowReservationRequestV1

    n, identity, body = reserved_trial
    store = n.store
    escrow = store._services(n.manifest.run_id())[0]
    epoch = body["finalize"]["body"]["w"]
    trial = store._db.execute("SELECT * FROM admission_trials WHERE epoch=?", (epoch,)).fetchone()
    result = store._db.execute(
        "SELECT challenge FROM admission_trial_results WHERE epoch=?", (epoch,)
    ).fetchone()
    from hypertrain.protocol.messages_v2 import IslandJobV1, JoinChallenge

    challenge = JoinChallenge.model_validate_json(result["challenge"])
    job = IslandJobV1.model_validate_json(store.objects.get(body["custody"]["job_hash"]))
    if refusal == "pending":
        # Original ledger's pending-slot fixture: genuine signed work, distinct semantic key.
        # It claims no second store trial/acceptance; reserve_shadow itself creates the slot.
        request = ShadowReservationRequestV1(
            run_id=n.manifest.run_id(),
            admission_id="22" * 32,
            hotkey=HOT[0].ss58,
            coldkey=COLD[0].ss58,
            trial_epoch=epoch,
            commit_hash=body_digest(body["commit"]["body"]),
            finalize_hash=body_digest(body["finalize"]["body"]),
            custody_hash=body_digest(body["custody"]),
            graph_hash=body_digest({"manifest": n.manifest.body()}),
            economic_admission_hash=escrow.policy.digest(),
            nonce=challenge.nonce,
            assignment_hash=challenge.assignment_hash,
            finalized_beacon=trial["finalized_beacon"],
            mature_at=trial["finalized_beacon"] + n.manifest.training.verify.E_vest_rounds,
        )
        escrow.reserve_shadow(
            request,
            ShadowReservationEvidence(
                finalize=envelope_v2.EnvelopeV2.model_validate(body["finalize"]),
                commit=envelope_v2.EnvelopeV2.model_validate(body["commit"]),
                owner=COLD[0].ss58,
                sample_ids=tuple(job.sample_ids),
                custody_hash=request.custody_hash,
                reference_final_state_hash=body["finalize"]["body"]["final_theta_hash_w1"],
            ),
        )
        expected = "SHADOW_RESERVATION_PENDING"
    else:
        # Narrow cap boundary only: original genesis-issued units remain real and untouched.
        # Lower the disposable intake's cap, not shared policy, balances or cap predicate.
        original = EscrowV2.reserve_shadow

        def capped(intake, request, evidence):
            intake.policy = EconomicsPolicyV2.model_validate(
                {
                    **intake.policy.body(),
                    "max_total_issuance": intake.balances().issued,
                }
            )
            return original(intake, request, evidence)

        monkeypatch.setattr(EscrowV2, "reserve_shadow", capped)
        expected = "TOTAL_ISSUANCE_BUDGET"

    def snapshot():
        return {
            table: sorted(tuple(row) for row in store._db.execute(f'SELECT * FROM "{table}"'))
            for table in (
                "beacon_pins_v2",
                "escrow_events",
                "escrow_origins",
                "escrow_units",
                "escrow_snapshots",
                "admission_reservations",
                "records_v2",
                "admission_trials",
                "admission_trial_results",
                "admissions_v2",
            )
        }

    before, balances = snapshot(), escrow.balances()
    freshness = []
    original_fresh = store._fresh_v2

    def fresh(manifest, operation, rnd=None):
        if operation.startswith("shadow-reservation:"):
            assert store._db.in_transaction
            freshness.append(operation)
        return original_fresh(manifest, operation, rnd)

    monkeypatch.setattr(store, "_fresh_v2", fresh)
    response = n.client.post(
        n.url + "/admin/join/" + identity + "/shadow-reservation",
        json=body,
        headers=admin(),
    )
    assert response.status_code == 409 and expected in response.text, response.text
    assert freshness == [f"shadow-reservation:{identity}:2"]
    assert snapshot() == before
    assert escrow.balances() == balances


@pytest.fixture
def shadow_acceptance(reserved_trial):
    """Sign evidence over retained independent original work; no additional trajectory."""
    import json

    from hypertrain.auditor.replay import unpack_state
    from hypertrain.ledger.escrow_v2 import shadow_origin_id
    from hypertrain.protocol.envelope import body_digest
    from hypertrain.protocol.messages import ReplayEnv, ReplayVerdict
    from hypertrain.protocol.messages_v2 import (
        AuditChallengeV2,
        IslandJobV1,
        JoinChallenge,
        ShadowReplayReceiptV1,
        ShadowReservationV1,
        ShadowRewardFinalizeV1,
        WorkProof,
    )
    from hypertrain.trainer.compress import state_hash

    n, identity, original = reserved_trial
    reserved = n.client.post(
        n.url + "/admin/join/" + identity + "/shadow-reservation",
        json=original,
        headers=admin(),
    )
    assert reserved.status_code == 200, reserved.text
    binding = ShadowReservationV1.model_validate(reserved.json()["reservation"])
    escrow = n.store._services(n.manifest.run_id())[0]
    result = n.store._db.execute(
        "SELECT * FROM admission_trial_results WHERE epoch=?", (binding.trial_epoch,)
    ).fetchone()
    challenge = JoinChallenge.model_validate_json(result["challenge"])
    proof = WorkProof.model_validate_json(result["reference"])
    job = IslandJobV1.model_validate_json(n.store.objects.get(original["custody"]["job_hash"]))
    execution = n.store._record_v2(
        n.manifest.run_id(),
        "shadow-execution",
        original["custody"]["reference_operation_hash"],
    )
    reference_path = Path(execution["directory"])
    theta_hash = state_hash(
        unpack_state((reference_path / "rank-0/state.safetensors").read_bytes())[0]
    )
    assert theta_hash == original["finalize"]["body"]["final_theta_hash_w1"]
    summary = json.loads((reference_path / "rank-0/summary.json").read_bytes())["commitments"]
    assert summary["leaves_root"] == original["commit"]["body"]["leaves_root"]
    audit = AuditChallengeV2(
        w=binding.trial_epoch,
        target=binding.hotkey,
        beacon_round=n.now,
        beacon_sig_sha256=sha256_hex(bytes.fromhex(n.store._beacon_v2(n.now).signature)),
        mode="full",
        segments=[],
        reasons=["probation"],
        serve_deadline=challenge.deadline_beacon,
        anchor_hash=n.store._shadow_anchor_v2(job, binding.hotkey, reference_path),
        audit_mode="anchored-full",
    )
    verdict = ReplayVerdict(
        challenge_hash=body_digest(audit.model_dump(mode="json")),
        first_bad_leaf=None,
        result="MATCH",
        recomputed_leaves_root=summary["leaves_root"],
        replay_env=ReplayEnv(
            image_digest=n.manifest.training.reference_spec.image_digest,
            driver=n.manifest.training.reference_spec.driver_allowlist[0],
            gpu_uuid_sha256=sha256_hex(b"cpu-test-only"),
            sm_count=n.manifest.training.reference_spec.sm_count,
        ),
    )
    signed_challenge = n.store._db.execute(
        "SELECT receipt FROM admission_reservations WHERE reservation=?",
        (f"challenge|{identity}|{challenge.nonce}",),
    ).fetchone()[0]
    receipt = ShadowReplayReceiptV1(
        run_id=n.manifest.run_id(),
        admission_id=identity,
        hotkey=binding.hotkey,
        coldkey=binding.coldkey,
        trial_epoch=binding.trial_epoch,
        shadow_ordinal=binding.shadow_ordinal,
        reservation_hash=binding.digest(),
        challenge_hash=challenge.digest(),
        signed_challenge_hash=sha256_hex(canonicalize(json.loads(signed_challenge))),
        assignment_hash=binding.assignment_hash,
        sample_ids_hash=sha256_hex(canonicalize(job.sample_ids)),
        **original["custody"],
        reference_work_proof_hash=proof.digest(),
        miner_work_proof_hash=proof.digest(),
        commit_hash=binding.commit_hash,
        audit_challenge_hash=body_digest(audit.model_dump(mode="json")),
        verdict_hash=body_digest(verdict.model_dump(mode="json")),
        trial_finalize_hash=binding.finalize_hash,
        final_state_hash=theta_hash,
        backend_qualification_authority_hash=n.store._shadow_backend_authority_v2(
            n.manifest.run_id()
        ),
        completed_beacon=n.now,
    )
    signed_receipt = n.signed(AUDITORS[0], "ShadowReplayReceiptV1", receipt)
    origin = OriginAllocation(
        origin_id=shadow_origin_id(binding),
        owner=binding.coldkey,
        units=escrow.policy.round_reward_units,
        mature_at=binding.mature_at,
    )
    reward = ShadowRewardFinalizeV1(
        run_id=n.manifest.run_id(),
        w=binding.trial_epoch,
        finalize_hash=binding.finalize_hash,
        tape_hash=sha256_hex(canonicalize(signed_receipt)),
        verdict_root=sha256_hex(canonicalize([body_digest(verdict.model_dump(mode="json"))])),
        allocation_hash=sha256_hex(canonicalize([origin.body()])),
        budget_units=escrow.policy.round_reward_units,
        origin_ids=[origin.origin_id],
        mature_at=binding.mature_at,
        authority_sig="0" * 128,
        shadow=True,
        shadow_ordinal=binding.shadow_ordinal,
        reservation_hash=binding.digest(),
    )
    reward = reward.model_copy(
        update={"authority_sig": COORD.sign(authority_message(reward)).hex()}
    )
    return (
        n,
        identity,
        {
            "reservation_hash": binding.digest(),
            "audit_challenge": n.signed(COORD, "AuditChallengeV2", audit),
            "verdict": n.signed(AUDITORS[0], "ReplayVerdict", verdict),
            "auditor_receipt": signed_receipt,
            "reward": reward.body(),
        },
    )


def test_shadow_acceptance_issues_once_and_recovers_lost_response(shadow_acceptance):
    n, identity, body = shadow_acceptance
    escrow = n.store._services(n.manifest.run_id())[0]
    before = escrow.balances()
    url = n.url + "/admin/join/" + identity + "/shadow-reward"
    first = n.client.post(url, json=body, headers=admin())
    assert first.status_code == 200, first.text
    response = first.json()
    assert response["authority_hash"] == sha256_hex(canonicalize(body["auditor_receipt"]))
    assert escrow.balances().issued == before.issued + escrow.policy.round_reward_units
    assert (
        escrow.balances().reward_pending == before.reward_pending + escrow.policy.round_reward_units
    )
    assert escrow.balances().available == before.available
    assert n.store._db.execute("SELECT COUNT(*) FROM escrow_finalized").fetchone()[0] == 0
    events = [tuple(row) for row in n.store._db.execute("SELECT * FROM escrow_events")]
    # A genuine subsequent challenge cannot turn a lost-response retry into another issuance.
    n.push(3)
    n.store._services(n.manifest.run_id())[1].challenge(identity, now=3)
    repeated = n.client.post(url, json=body, headers=admin())
    assert repeated.status_code == 200, repeated.text
    assert repeated.json() == response
    assert [tuple(row) for row in n.store._db.execute("SELECT * FROM escrow_events")] == events
    # Fresh signatures over the exact original bodies recover the same accepted response.
    for name, key in (
        ("audit_challenge", COORD),
        ("verdict", AUDITORS[0]),
        ("auditor_receipt", AUDITORS[0]),
    ):
        body[name] = n.signed(key, body[name]["type"], body[name]["body"], expiry=20000)
    renewed = n.client.post(url, json=body, headers=admin())
    assert renewed.status_code == 200 and renewed.json() == response
    # Authenticated changed bodies are not a lost-response replay.
    from hypertrain.protocol.messages_v2 import ShadowRewardFinalizeV1

    body["reward"]["budget_units"] += 1
    changed = ShadowRewardFinalizeV1.model_validate(body["reward"])
    body["reward"]["authority_sig"] = COORD.sign(authority_message(changed)).hex()
    conflict = n.client.post(url, json=body, headers=admin())
    assert conflict.status_code == 409 and "semantic conflict" in conflict.text
    assert [tuple(row) for row in n.store._db.execute("SELECT * FROM escrow_events")] == events
    escrow.verify()


def test_shadow_operator_cutoff_recovery_uses_original_store_objects(
    shadow_acceptance, monkeypatch
):
    """Real signed CPU store chain; cutoff observer tests routing, not accepted CUDA authority."""
    from hypertrain.challenge.store import ChallengeError

    n, identity, body = shadow_acceptance
    store = n.store
    original = store._shadow_backend_authority_v2
    expired, observed = False, []

    def backend(run_id, *, historical=False):
        observed.append(historical)
        if expired and not historical:
            raise ChallengeError(403, "operator cutoff expired")
        return original(run_id, historical=historical)

    monkeypatch.setattr(store, "_shadow_backend_authority_v2", backend)
    url = n.url + "/admin/join/" + identity + "/shadow-reward"
    first = n.client.post(url, json=body, headers=admin())
    assert first.status_code == 200, first.text
    expired = True
    observed.clear()
    escrow = store._services(n.manifest.run_id())[0]
    events = [tuple(row) for row in store._db.execute("SELECT * FROM escrow_events")]
    balances = escrow.balances()
    recovered = n.client.post(url, json=body, headers=admin())
    assert recovered.status_code == 200 and recovered.json() == first.json()
    assert observed and all(observed)
    assert escrow.balances() == balances
    assert [tuple(row) for row in store._db.execute("SELECT * FROM escrow_events")] == events
    retained = store._record_v2(n.manifest.run_id(), "shadow-evidence", body["reservation_hash"])
    path = (
        store.objects.root / retained["auditor_receipt_hash"][:2] / retained["auditor_receipt_hash"]
    )
    raw = path.read_bytes()
    try:
        path.write_bytes(bytes([raw[0] ^ 1]) + raw[1:])
        refused = n.client.post(url, json=body, headers=admin())
        assert refused.status_code == 409 and "accepted evidence object unavailable" in refused.text
        assert escrow.balances() == balances
        assert [tuple(row) for row in store._db.execute("SELECT * FROM escrow_events")] == events
    finally:
        path.write_bytes(raw)
    # NEW authority call still refuses; no renewal/extra grant after cutoff.
    with pytest.raises(ChallengeError, match="cutoff expired"):
        store._shadow_backend_authority_v2(n.manifest.run_id())


@pytest.mark.parametrize("field", ["auditor_receipt_hash", "audit_challenge_hash", "verdict_hash"])
@pytest.mark.parametrize("fault", ["missing", "tampered"])
def test_shadow_recovery_authenticates_stored_acceptance_objects(shadow_acceptance, field, fault):
    n, identity, body = shadow_acceptance
    url = n.url + "/admin/join/" + identity + "/shadow-reward"
    accepted = n.client.post(url, json=body, headers=admin())
    assert accepted.status_code == 200, accepted.text
    escrow = n.store._services(n.manifest.run_id())[0]
    before = escrow.balances()
    events = [tuple(row) for row in n.store._db.execute("SELECT * FROM escrow_events")]
    record = n.store._record_v2(n.manifest.run_id(), "shadow-evidence", body["reservation_hash"])
    key = record[field]
    path = n.store.objects.root / key[:2] / key
    original = path.read_bytes()
    try:
        if fault == "missing":
            path.unlink()
        else:
            path.write_bytes(bytes([original[0] ^ 1]) + original[1:])
        recovered = n.client.post(url, json=body, headers=admin())
        assert recovered.status_code == 409, recovered.text
        assert "accepted evidence object unavailable" in recovered.text
        assert escrow.balances() == before
        assert [tuple(row) for row in n.store._db.execute("SELECT * FROM escrow_events")] == events
    finally:
        path.write_bytes(original)


@pytest.mark.parametrize(
    "fault",
    [
        "admin",
        "extra",
        "reservation",
        "auditor",
        "verdict-signer",
        "signature",
        "reward-authority",
        "ordinal",
        "domain",
        "assignment",
        "anchor",
        "backend",
        "receipt-state",
        "publication",
        "missing-execution",
        "missing-custody",
        "dispute",
        "allocation",
        "cap",
        "expired",
    ],
)
def test_shadow_acceptance_rejects_without_partial_issuance(shadow_acceptance, monkeypatch, fault):
    from hypertrain.ledger.escrow_v2 import EscrowV2
    from hypertrain.protocol.messages_v2 import ShadowRewardFinalizeV1

    n, identity, body = shadow_acceptance
    headers = admin()
    if fault == "admin":
        headers = {}
    elif fault == "extra":
        body["MATCH"] = True
    elif fault == "reservation":
        body["reservation_hash"] = "ff" * 32
    elif fault == "auditor":
        body["auditor_receipt"] = n.signed(
            HOT[0], "ShadowReplayReceiptV1", body["auditor_receipt"]["body"]
        )
    elif fault == "verdict-signer":
        body["verdict"] = n.signed(AUDITORS[1], "ReplayVerdict", body["verdict"]["body"])
    elif fault == "signature":
        body["verdict"]["sig"] = "0" * 128
    elif fault == "expired":
        body["verdict"] = n.signed(AUDITORS[0], "ReplayVerdict", body["verdict"]["body"], expiry=1)
    elif fault == "reward-authority":
        body["reward"]["authority_sig"] = "0" * 128
    elif fault in ("ordinal", "domain", "allocation"):
        body["reward"][
            {
                "ordinal": "shadow_ordinal",
                "domain": "shadow",
                "allocation": "allocation_hash",
            }[fault]
        ] = 1 if fault == "ordinal" else False if fault == "domain" else "ff" * 32
        if fault != "domain":
            reward = ShadowRewardFinalizeV1.model_validate(body["reward"])
            body["reward"]["authority_sig"] = COORD.sign(authority_message(reward)).hex()
    elif fault in ("assignment", "backend", "receipt-state"):
        field = {
            "assignment": "assignment_hash",
            "backend": "backend_qualification_authority_hash",
            "receipt-state": "final_state_hash",
        }[fault]
        body["auditor_receipt"]["body"][field] = "ff" * 32
        body["auditor_receipt"] = n.signed(
            AUDITORS[0], "ShadowReplayReceiptV1", body["auditor_receipt"]["body"]
        )
    elif fault == "anchor":
        body["audit_challenge"]["body"]["anchor_hash"] = "ff" * 32
        body["audit_challenge"] = n.signed(
            COORD, "AuditChallengeV2", body["audit_challenge"]["body"]
        )
    elif fault == "publication":
        receipt = body["auditor_receipt"]["body"]
        execution = n.store._record_v2(
            n.manifest.run_id(), "shadow-execution", receipt["reference_operation_hash"]
        )
        (Path(execution["directory"]) / "rank-0/delta.bin").write_bytes(
            b"tampered original reference"
        )
    elif fault == "missing-execution":
        n.store._db.execute(
            "DELETE FROM records_v2 WHERE kind='shadow-execution' AND id=?",
            (body["auditor_receipt"]["body"]["miner_operation_hash"],),
        )
    elif fault == "missing-custody":
        n.store._db.execute("DELETE FROM records_v2 WHERE kind='shadow-custody'")
    elif fault == "dispute":
        n.store._services(n.manifest.run_id())[1].suspend(
            HOT[0].ss58,
            now=2,
            reason="PENDING_DISPUTE",
            pending_dispute=True,
            dispute_id="ef" * 32,
            evidence_hash="ed" * 32,
        )
    elif fault == "cap":
        original = EscrowV2.reward_finalize
        original_allocations = EscrowV2.reward_allocations

        def capped_allocations(escrow, reward, evidence):
            origins = original_allocations(escrow, reward, evidence)
            # Exercise original cap after genuine allocation/custody validation, not fake units.
            escrow.policy = escrow.policy.model_copy(
                update={"max_total_issuance": escrow.balances().issued}
            )
            return origins

        def capped(escrow, reward, evidence):
            policy = escrow.policy
            try:
                return original(escrow, reward, evidence)
            finally:
                escrow.policy = policy

        monkeypatch.setattr(EscrowV2, "reward_finalize", capped)
        monkeypatch.setattr(EscrowV2, "reward_allocations", capped_allocations)
    escrow = n.store._services(n.manifest.run_id())[0]
    before = escrow.balances()
    tables = (
        "records_v2",
        "escrow_events",
        "escrow_origins",
        "escrow_units",
        "escrow_snapshots",
        "escrow_shadow_finalized",
        "beacon_pins_v2",
    )

    def snapshot():
        return {
            t: sorted(tuple(r) for r in n.store._db.execute(f'SELECT * FROM "{t}"')) for t in tables
        }

    records = snapshot()
    response = n.client.post(
        n.url + "/admin/join/" + identity + "/shadow-reward", json=body, headers=headers
    )
    assert response.status_code in (401, 403, 409, 422), response.text
    if fault == "cap":
        assert "TOTAL_ISSUANCE_BUDGET" in response.text
    assert escrow.balances() == before and snapshot() == records


def test_shadow_acceptance_failure_rolls_back_and_dispute_blocks_maturity(
    shadow_acceptance, monkeypatch
):
    from hypertrain.challenge.store import ChallengeError

    n, identity, body = shadow_acceptance
    url = n.url + "/admin/join/" + identity + "/shadow-reward"
    escrow, admission, _ = n.store._services(n.manifest.run_id())
    before = escrow.balances()
    tables = (
        "records_v2",
        "escrow_events",
        "escrow_origins",
        "escrow_units",
        "escrow_snapshots",
        "escrow_shadow_finalized",
        "beacon_pins_v2",
    )

    def snapshot():
        return {
            t: sorted(tuple(r) for r in n.store._db.execute(f'SELECT * FROM "{t}"')) for t in tables
        }

    original_records = snapshot()
    original_put = n.store._put_record_v2

    def interrupted(run_id, kind, key, record):
        if kind == "shadow-settlement":
            raise ChallengeError(409, "acceptance transaction interrupted after issuance")
        return original_put(run_id, kind, key, record)

    with monkeypatch.context() as patch:
        patch.setattr(n.store, "_put_record_v2", interrupted)
        failed = n.client.post(url, json=body, headers=admin())
        assert failed.status_code == 409 and "interrupted" in failed.text
    assert escrow.balances() == before
    assert snapshot() == original_records
    assert n.store._db.execute("SELECT COUNT(*) FROM escrow_shadow_finalized").fetchone()[0] == 0
    assert (
        n.store._db.execute(
            "SELECT COUNT(*) FROM records_v2 WHERE kind='shadow-evidence'"
        ).fetchone()[0]
        == 0
    )
    accepted = n.client.post(url, json=body, headers=admin())
    assert accepted.status_code == 200, accepted.text
    pending = escrow.balances().reward_pending
    admission.suspend(
        HOT[0].ss58,
        now=2,
        reason="PENDING_DISPUTE",
        pending_dispute=True,
        dispute_id="ea" * 32,
        evidence_hash="eb" * 32,
    )
    replay = n.client.post(url, json=body, headers=admin())
    assert replay.status_code == 200 and replay.json() == accepted.json()
    n.push(accepted.json()["mature_at"])
    assert escrow.balances().reward_pending == pending
    assert n.store._shadow_settlement_v2(
        n.manifest.run_id(), accepted.json()["origin_ids"][0]
    ).unresolved


@pytest.mark.parametrize("outcome", ["INFRASTRUCTURE", "FRAUD"])
def test_shadow_accepted_response_survives_adverse_trial_outcome(shadow_acceptance, outcome):
    n, identity, body = shadow_acceptance
    run_id = n.manifest.run_id()
    escrow, admission, _ = n.store._services(run_id)
    url = n.url + "/admin/join/" + identity + "/shadow-reward"
    accepted = n.client.post(url, json=body, headers=admin())
    assert accepted.status_code == 200, accepted.text
    before = escrow.balances()
    events = [tuple(row) for row in n.store._db.execute("SELECT * FROM escrow_events")]
    admission.suspend(HOT[0].ss58, now=n.now, reason="INFRASTRUCTURE")
    admission.resume(HOT[0].ss58, now=n.now)
    # Inject a later adverse outcome on the original finalized trial, not accepted work.
    # Original resume only rewrites OPEN trials; historical MATCH bytes remain authenticated.
    with escrow.tx():
        n.store._db.execute(
            "UPDATE admission_trial_results SET outcome=? WHERE epoch=?",
            (outcome, body["reward"]["w"]),
        )
    repeated = n.client.post(url, json=body, headers=admin())
    assert repeated.status_code == 200 and repeated.json() == accepted.json()
    assert escrow.balances() == before
    assert [tuple(row) for row in n.store._db.execute("SELECT * FROM escrow_events")] == events
    forged = {
        **body,
        "verdict": n.signed(HOT[0], "ReplayVerdict", body["verdict"]["body"]),
    }
    rejected = n.client.post(url, json=forged, headers=admin())
    assert rejected.status_code in (403, 409, 422), rejected.text
    assert escrow.balances() == before
    assert [tuple(row) for row in n.store._db.execute("SELECT * FROM escrow_events")] == events
    n.push(accepted.json()["mature_at"])
    assert n.store.latest_beacon()["round"] == accepted.json()["mature_at"]
    assert escrow.balances().reward_pending == before.reward_pending
    assert escrow.balances().available == before.available
    assert n.store._shadow_settlement_v2(run_id, accepted.json()["origin_ids"][0]).unresolved
    assert escrow.balances().conserved()


@pytest.mark.parametrize("beyond", [False, True])
@pytest.mark.parametrize("field", ["serve_deadline", "completed_beacon"])
def test_shadow_acceptance_completion_uses_original_signed_cutoff(shadow_acceptance, field, beyond):
    from hypertrain.protocol.envelope import body_digest
    from hypertrain.protocol.messages_v2 import JoinChallenge, ShadowRewardFinalizeV1

    n, identity, body = shadow_acceptance
    trial = n.store._db.execute(
        "SELECT challenge FROM admission_trial_results WHERE epoch=?",
        (body["reward"]["w"],),
    ).fetchone()
    cutoff = JoinChallenge.model_validate_json(trial[0]).deadline_beacon
    value = cutoff + int(beyond)
    n.push(value)
    body["audit_challenge"]["body"]["serve_deadline"] = value
    if field == "completed_beacon":
        body["auditor_receipt"]["body"][field] = value
    body["verdict"]["body"]["challenge_hash"] = body_digest(body["audit_challenge"]["body"])
    body["auditor_receipt"]["body"]["audit_challenge_hash"] = body["verdict"]["body"][
        "challenge_hash"
    ]
    body["auditor_receipt"]["body"]["verdict_hash"] = body_digest(body["verdict"]["body"])
    for name, key in (
        ("audit_challenge", COORD),
        ("verdict", AUDITORS[0]),
        ("auditor_receipt", AUDITORS[0]),
    ):
        body[name] = n.signed(key, body[name]["type"], body[name]["body"])
    body["reward"]["tape_hash"] = sha256_hex(canonicalize(body["auditor_receipt"]))
    body["reward"]["verdict_root"] = sha256_hex(
        canonicalize([body_digest(body["verdict"]["body"])])
    )
    reward = ShadowRewardFinalizeV1.model_validate(body["reward"])
    body["reward"]["authority_sig"] = COORD.sign(authority_message(reward)).hex()
    escrow = n.store._services(n.manifest.run_id())[0]
    before = escrow.balances()
    events = [tuple(row) for row in n.store._db.execute("SELECT * FROM escrow_events")]
    response = n.client.post(
        n.url + "/admin/join/" + identity + "/shadow-reward", json=body, headers=admin()
    )
    if beyond:
        assert response.status_code == 409 and "binding differs" in response.text
        assert escrow.balances() == before
        assert [tuple(row) for row in n.store._db.execute("SELECT * FROM escrow_events")] == events
    else:
        assert response.status_code == 200, response.text
        assert escrow.balances().issued == before.issued + escrow.policy.round_reward_units


def test_shadow_acceptance_original_vesting_matures_when_undisputed(shadow_acceptance):
    n, identity, body = shadow_acceptance
    escrow = n.store._services(n.manifest.run_id())[0]
    before = escrow.balances()
    accepted = n.client.post(
        n.url + "/admin/join/" + identity + "/shadow-reward", json=body, headers=admin()
    )
    assert accepted.status_code == 200, accepted.text
    maturity = accepted.json()["mature_at"]
    n.push(maturity - 1)
    assert escrow.balances().available == before.available
    n.push(maturity)
    assert escrow.balances().available == before.available + escrow.policy.round_reward_units
    assert escrow.balances().reward_pending == before.reward_pending
    assert escrow.balances().conserved()
    escrow.verify()


def test_public_join_replays_same_identity_after_restart(network):
    # Given: real shared SQLite, dual-signed public application.
    first = network.join()
    assert first.status_code == 200, first.text
    # When: the exact request is retried.
    second = network.join()
    # Then: one persisted identity, original admission identifier.
    assert second.json() == first.json()
    assert network.store._db.execute("SELECT COUNT(*) FROM admissions_v2").fetchone()[0] == 1


def test_forged_coldkey_cannot_join(network):
    # Given
    request = sign_join(
        HOT[0],
        COLD[0],
        run_id=network.manifest.run_id(),
        request_id="ab" * 32,
        expires_beacon=100,
        policy_hash=network.manifest.network.admission_policy_hash,
        hardware_hint=HardwareHint(device_name="cpu", device_count=1, driver="cpu"),
    )
    raw = request.body()
    raw["cold_sig"] = "0" * 128
    # When
    response = network.client.post(network.url + "/join", json=raw)
    # Then
    assert response.status_code == 409
    assert network.store._db.execute("SELECT COUNT(*) FROM admissions_v2").fetchone()[0] == 0


def test_public_hotkey_churn_after_reopen_preserves_origin_and_quota(network, tmp_path):
    """New transport prefix and hotkey cannot reset a funded coldkey's probation."""
    from hypertrain.challenge.store import ChallengeStore
    from hypertrain.protocol.messages_v2 import EscrowLock

    # Given: public dual-signed join and real origin-backed collateral, not ACTIVE.
    first = network.join()
    assert first.status_code == 200, first.text
    identity = first.json()["admission_id"]
    lock = EscrowLock(
        operation_id="bc" * 32,
        owner=COLD[0].ss58,
        units=1000,
        origin_ids=[network.origin_ids[0]],
        admission_id=identity,
        dispute_id=None,
        kind="LOCK_ADMISSION",
    )
    response = network.client.post(
        network.url + "/escrow/lock", json=network.signed(COLD[0], "EscrowLock", lock)
    )
    assert response.status_code == 200, response.text
    original = network.store
    before = original._services(network.manifest.run_id())[0].balances()
    assert before.admission_locked == 1000 and before.conserved()
    tables = (
        "admissions_v2",
        "admission_history",
        "admission_reservations",
        "admission_quota",
        "escrow_events",
        "escrow_units",
    )
    snapshot = {
        table: [tuple(r) for r in original._db.execute(f'SELECT * FROM "{table}"')]
        for table in tables
    }
    namespace = original._db.execute(
        "SELECT value FROM meta WHERE key='admission-prefix-namespace-v2'"
    ).fetchone()[0]
    original.close_v2_notifications()
    original._db.close()
    reopened = ChallengeStore(
        original.state_dir,
        original.params,
        COORD,
        OWNER.ss58,
        original.verify_beacon,
        original.objects,
    )
    reopened.clock = original.clock
    token = tmp_path / "reopen-admin"
    token.write_text("integration-admin")
    cfg = Config(
        "hypertrain",
        original.state_dir,
        "https://master.test",
        None,
        token,
        None,
        tmp_path / "coord.key",
        OWNER.ss58,
        original.params,
    )
    app = create_app(
        cfg,
        verify_beacon=reopened.verify_beacon,
        _store=reopened,
        clock=reopened.clock,
        transport=subnet(),
    )
    request = sign_join(
        HOT[1],
        COLD[0],
        run_id=network.manifest.run_id(),
        request_id="bd" * 32,
        expires_beacon=100,
        policy_hash=network.manifest.network.admission_policy_hash,
        hardware_hint=HardwareHint(device_name="metadata", device_count=999, driver="advisory"),
    )
    try:
        # When: valid dual signatures change hotkey and source prefix after actual reopen.
        with TestClient(app, client=("198.51.100.42", 43123)) as client:
            rejected = client.post(network.url + "/join", json=request.body())
            # Then: same origin remains locked; no new identity/quota debit or reward.
            assert rejected.status_code == 409, rejected.text
            assert "COLDKEY_PROBATION_EXISTS" in rejected.text
            assert {
                table: [tuple(r) for r in reopened._db.execute(f'SELECT * FROM "{table}"')]
                for table in tables
            } == snapshot
            escrow, admission, _ = reopened._services(network.manifest.run_id())
            assert escrow.balances() == before and escrow.balances().conserved()
            assert not admission.status(HOT[0].ss58, now=1).eligible
            assert (
                reopened._db.execute(
                    "SELECT value FROM meta WHERE key='admission-prefix-namespace-v2'"
                ).fetchone()[0]
                == namespace
            )
    finally:
        reopened.close_v2_notifications()
        reopened._db.close()


def test_prefix_quota_stable_after_store_restart(network, monkeypatch):
    import hmac

    from hypertrain.challenge.admission_store import AdmissionError
    from hypertrain.challenge.store import ChallengeError, ChallengeStore
    from hypertrain.protocol.keys import Keypair

    store = network.store
    signer = store.coord
    assert signer is not None
    signatures = [signer.sign(b"hypertrain/ip-prefix/2") for _ in range(2)]
    assert signatures[0] != signatures[1]  # Actual signer, no quota/signature stub.
    escrow, admission, _ = store._services(network.manifest.run_id())
    epoch = network.now // admission.policy.work_screen_epoch_rounds
    prefix = "203.0.113.0/24"
    hashed = hmac.digest(admission.ip_secret, f"{epoch}|{prefix}".encode(), "sha256").hex()
    quota_key = f"ip:{hashed}"
    with escrow.tx():
        for _ in range(admission.policy.ip_prefix_burst):
            admission.store.quota(
                quota_key,
                network.now,
                admission.policy.ip_prefix_per_beacon,
                admission.policy.ip_prefix_burst,
            )
    before = tuple(
        store._db.execute(
            "SELECT beacon,tokens FROM admission_quota WHERE identity=?", (quota_key,)
        ).fetchone()
    )
    store.close_v2_notifications()
    store._db.close()
    reopened = ChallengeStore(
        store.state_dir,
        store.params,
        Keypair(bytes(range(32))),
        owner_hotkey=store.owner_hotkey,
        verify_beacon=store.verify_beacon,
        objects=store.objects,
    )
    monkeypatch.setattr(network.client.app.state, "store", reopened)
    try:
        hot, cold = Keypair(b"\xc1" * 32), Keypair(b"\xc2" * 32)
        request = sign_join(
            hot,
            cold,
            run_id=network.manifest.run_id(),
            request_id="c3" * 32,
            expires_beacon=network.now + 100,
            policy_hash=network.manifest.network.admission_policy_hash,
            hardware_hint=HardwareHint(device_name="metadata", device_count=1, driver="cpu"),
        )
        with pytest.raises(AdmissionError, match="JOIN_QUOTA"):
            reopened.admission_v2(
                network.manifest.run_id(),
                "join",
                canonicalize(request.body()),
                ip_prefix=prefix,
            )
        assert (
            tuple(
                reopened._db.execute(
                    "SELECT beacon,tokens FROM admission_quota WHERE identity=?",
                    (quota_key,),
                ).fetchone()
            )
            == before
        )
        accepted = reopened.admission_v2(
            network.manifest.run_id(),
            "join",
            canonicalize(request.body()),
            ip_prefix="198.51.100.0/24",
        )
        assert accepted["state"] == "APPLIED"
        assert reopened._db.execute("SELECT COUNT(*) FROM admissions_v2").fetchone()[0] == 1
        namespace = reopened._db.execute(
            "SELECT value FROM meta WHERE key='admission-prefix-namespace-v2'"
        ).fetchone()[0]
        quotas = [
            tuple(row)
            for row in reopened._db.execute("SELECT * FROM admission_quota ORDER BY identity")
        ]
        reopened._services_v2.clear()
        reopened._db.execute("DELETE FROM meta WHERE key='admission-prefix-namespace-v2'")
        with pytest.raises(ChallengeError, match="legacy admission prefix namespace unavailable"):
            reopened._services(network.manifest.run_id())
        assert [
            tuple(row)
            for row in reopened._db.execute("SELECT * FROM admission_quota ORDER BY identity")
        ] == quotas
        reopened._db.execute(
            "INSERT INTO meta VALUES('admission-prefix-namespace-v2', ?)", (namespace,)
        )
        reopened.coord = Keypair(b"\xcc" * 32)
        with pytest.raises(ChallengeError, match="prefix namespace differs"):
            reopened._prefix_secret_v2()
        assert (
            reopened._db.execute(
                "SELECT value FROM meta WHERE key='admission-prefix-namespace-v2'"
            ).fetchone()[0]
            == namespace
        )
    finally:
        reopened.close_v2_notifications()
        reopened._db.close()


def test_unfunded_lock_cannot_create_collateral(network):
    from hypertrain.protocol.messages_v2 import EscrowLock

    # Given
    identity = network.join().json()["admission_id"]
    lock = EscrowLock(
        operation_id="ac" * 32,
        owner=COLD[0].ss58,
        units=1000,
        origin_ids=["ff" * 32],
        admission_id=identity,
        dispute_id=None,
        kind="LOCK_ADMISSION",
    )
    # When
    response = network.client.post(
        network.url + "/escrow/lock", json=network.signed(COLD[0], "EscrowLock", lock)
    )
    # Then
    assert response.status_code == 409
    assert network.store._services(network.manifest.run_id())[0].balances().admission_locked == 0


def test_cross_run_signed_rotation_rejects(network):
    from hypertrain.protocol.messages_v2 import RotateRequest

    # Given
    network.join()
    request = RotateRequest(hotkey=HOT[0].ss58, new_hotkey=REFEREE.ss58, operation_id="ae" * 32)
    # When
    response = network.client.post(
        network.url + "/admission/" + HOT[0].ss58 + "/rotate",
        json=seal(COLD[0], "RotateRequest", "ff" * 32, request, 100),
    )
    # Then
    assert response.status_code == 409


def test_second_run_cannot_mix_shared_state_directory(network):
    # Given
    original = network.store._db.execute("SELECT manifest FROM runs").fetchone()[0]
    # When
    response = network.client.post(
        "/v2/admin/runs",
        json=network.signed(OWNER, "RunManifestV2", network.manifest),
        headers=admin(),
    )
    # Then
    assert response.status_code == 409
    assert network.store._db.execute("SELECT manifest FROM runs").fetchone()[0] == original


def test_complete_seventeen_roster_rejects_before_any_mutation(network):
    from hypertrain.protocol.messages_v2 import PolicyHashes, RoundOpenV2

    # Given: valid wire roster, larger than qualified service throughput.
    roster = [
        {
            "hotkey": Keypair(bytes([i + 1]) * 32).ss58,
            "slot": i,
            "q_i": f32hex(1),
            "admission_id": sha256_hex(str(i).encode()),
            "coldkey_group": f"owner{i}",
            "state": "ACTIVE",
            "eligible_weight": 4194304,
        }
        for i in range(17)
    ]
    n = network.manifest.network
    round_open = RoundOpenV2(
        w=0,
        prev_final_hash="0" * 64,
        theta_hash="0" * 64,
        outer_state_hash="0" * 64,
        center_hash="0" * 64,
        roster_hash=sha256_hex(canonicalize(roster)),
        honeypot_commit="0" * 64,
        d_open=2,
        d_assign=3,
        d_commit=20,
        d_audit=21,
        d_upload=22,
        d_final=100,
        contract_version=2,
        policy_hashes=PolicyHashes.model_validate(
            {k: getattr(n, k) for k in PolicyHashes.model_fields}
        ),
        registry_epoch=0,
        start_state_index_hash="0" * 64,
        audit_mode="anchored-full",
        roster=roster,
    )
    before = network.store._db.total_changes
    objects = sorted(
        p.relative_to(network.store.state_dir)
        for p in network.store.state_dir.rglob("*")
        if p.is_file()
    )
    # When
    response = network.client.post(
        network.url + "/admin/rounds",
        json=network.signed(COORD, "RoundOpenV2", round_open),
        headers=admin(),
    )
    # Then: no allocator, no changed SQL/object state, no subset admission.
    assert response.status_code == 409
    assert "ROSTER_LIMIT_16" in response.text
    assert network.store._db.total_changes == before
    assert (
        sorted(
            p.relative_to(network.store.state_dir)
            for p in network.store.state_dir.rglob("*")
            if p.is_file()
        )
        == objects
    )


def test_expired_join_cannot_reserve_nonce(network):
    # Given
    network.push(10)
    request = sign_join(
        HOT[0],
        COLD[0],
        run_id=network.manifest.run_id(),
        request_id="ba" * 32,
        expires_beacon=9,
        policy_hash=network.manifest.network.admission_policy_hash,
        hardware_hint=HardwareHint(device_name="cpu", device_count=1, driver="cpu"),
    )
    # When
    response = network.client.post(network.url + "/join", json=request.body())
    # Then
    assert response.status_code == 409
    assert network.store._db.execute("SELECT COUNT(*) FROM admissions_v2").fetchone()[0] == 0


def test_stale_beacon_preserves_pause_before_join_identity(network):
    # Given: no fresh pushed beacon, explicit clock beyond the two-round lag.
    network.store.clock = lambda: network.manifest.training.beacon.genesis_time + 300
    # When
    response = network.join()
    # Then: durable infrastructure pause, no admission or nonce reservation.
    assert response.status_code == 503
    assert network.store._db.execute("SELECT COUNT(*) FROM admissions_v2").fetchone()[0] == 0
    assert network.store._db.execute("SELECT paused FROM beacon_pins_v2").fetchone()[0] == 1


def test_rotation_and_status_never_call_dense_allocator(network, monkeypatch):
    import hypertrain.aggregator.weighted_v2 as weighted
    from hypertrain.miner.admission import sign_rotation

    # Given
    identity = network.join().json()["admission_id"]
    monkeypatch.setattr(
        weighted,
        "allocate_weights",
        lambda *args: (_ for _ in ()).throw(AssertionError("public allocator")),
    )
    # When
    status = network.client.get(network.url + "/admission/" + HOT[0].ss58)
    from hypertrain.protocol.messages_v2 import RotateRequest

    rotate = sign_rotation(
        COLD[0],
        network.manifest.run_id(),
        RotateRequest(hotkey=HOT[0].ss58, new_hotkey=REFEREE.ss58, operation_id="df" * 32),
        1000,
    )
    response = network.client.post(
        network.url + "/admission/" + HOT[0].ss58 + "/rotate", json=rotate
    )
    # Then
    assert status.status_code == 200, status.text
    assert response.status_code == 200, response.text
    assert response.json()["admission_id"] == identity
    assert network.store._owner_v2(network.manifest.run_id(), REFEREE.ss58) == COLD[0].ss58


def test_other_process_notification_subscription_sees_committed_beacon(network):
    import multiprocessing

    # Given: separate store process, subscribed Condition before trigger.
    ctx = multiprocessing.get_context("fork")
    parent, child = ctx.Pipe()

    def listen():
        from hypertrain.challenge.store import ChallengeStore

        store = ChallengeStore(
            network.store.state_dir,
            network.store.params,
            COORD,
            OWNER.ss58,
            lambda _: None,
            network.store.objects,
        )
        store._services(network.manifest.run_id())
        with store.beacon_arrived:
            child.send("subscribed")
            assert store.beacon_arrived.wait(timeout=5)
        child.send(store._now(store._db))
        store.close_v2_notifications()

    process = ctx.Process(target=listen)
    process.start()
    assert parent.poll(5) and parent.recv() == "subscribed"
    # When
    network.push(2)
    # Then: explicit IPC wakes a separate accepted-authority reader, no delay loop.
    assert parent.poll(5) and parent.recv() == 2
    process.join(timeout=5)
    assert process.exitcode == 0


def test_recovery_wrong_pending_dispute_cannot_clear_suspension(network):
    # Given: actual accepted applicant, exact persisted pending incident.
    identity = network.join().json()["admission_id"]
    _, admission, _ = network.store._services(network.manifest.run_id())
    admission.suspend(
        HOT[0].ss58,
        now=network.now,
        reason="PENDING_DISPUTE",
        pending_dispute=True,
        dispute_id="ea" * 32,
        evidence_hash="eb" * 32,
    )
    before = admission.store.record(HOT[0].ss58)
    # When: a valid hotkey signature cites a different accepted reference.
    response = network.client.post(
        network.url + "/admission/recover",
        json=network.signed(
            HOT[0],
            "Receipt",
            {"w": 0, "commit_hash": "ec" * 32, "received_round": network.now},
        ),
    )
    # Then
    assert response.status_code == 403, response.text
    assert admission.store.record(HOT[0].ss58) == before
    assert admission.store.record(HOT[0].ss58).admission_id == identity


def test_genesis_anchor_identity_and_backend_tamper_reject(network):
    import json

    from hypertrain.auditor.replay import AnchorCache, AuditInputError
    from hypertrain.trainer.config import TrainConfig
    from hypertrain.trainer.model import init_params

    # Given: genuine accepted genesis, same tensor root for distinct identities.
    cache = AnchorCache()
    theta = init_params(TrainConfig.from_manifest_v2(network.manifest).model)
    for key in HOT[:2]:
        anchor = cache.genesis(network.manifest, key.ss58, theta)
        network.store._persist_anchor_v2(network.manifest, anchor, cache)
    first = network.store._record_v2(network.manifest.run_id(), "anchor", "-1:" + HOT[0].ss58)
    second = network.store._record_v2(network.manifest.run_id(), "anchor", "-1:" + HOT[1].ss58)
    assert first["path"] != second["path"]
    restored = network.store._restore_anchor_v2(
        network.manifest.run_id(), HOT[1].ss58, -1, AnchorCache()
    )
    assert restored.hotkey == HOT[1].ss58
    path = Path(first["path"]) / "metadata"
    metadata = json.loads(path.read_bytes())
    metadata["backend"] = "cuda"
    path.write_text(json.dumps(metadata))
    # When / Then: adjacent metadata cannot upgrade accepted CPU/genesis provenance.
    with pytest.raises(AuditInputError, match="authentication"):
        network.store._restore_anchor_v2(network.manifest.run_id(), HOT[0].ss58, -1, AnchorCache())


def test_signed_event_access_and_forged_ack_reject(network):
    from hypertrain.challenge.disputes_v2 import EventAck

    # Given: genuine dual-signed identity; no pending dispute events.
    network.join()
    url = network.url + "/disputes?party=" + HOT[0].ss58
    # When
    denied = network.client.get(url)
    signature = (
        HOT[0].sign(f"hypertrain/watch/2|{network.manifest.run_id()}|{HOT[0].ss58}".encode()).hex()
    )
    accepted = network.client.get(url, headers={"X-Dispute-Signature": signature})
    ack = EventAck(
        run_id=network.manifest.run_id(),
        party=HOT[0].ss58,
        cursor=1,
        event_hash="0" * 64,
        received_beacon=network.now,
        sig="0" * 128,
    )
    rejected = network.client.post(network.url + "/disputes/ack", json=ack.body())
    # Then
    assert denied.status_code == 403
    assert accepted.status_code == 200 and accepted.json() == []
    assert rejected.status_code == 409
    assert network.store._db.execute("SELECT COUNT(*) FROM dispute_acks_v2").fetchone()[0] == 0


def test_registry_rotation_requires_signed_exact_successor_lineage(network):
    from hypertrain.protocol import relay_envelope
    from hypertrain.protocol.relay_messages import RelayRegistryV1

    old = network.store._registry_v2(network.manifest)
    body = old.body()
    body["epoch"], body["previous_registry_hash"] = 1, old.digest()
    successor = RelayRegistryV1.model_validate(body)
    wrong = relay_envelope.seal(
        HOT[0], "RelayRegistryV1", network.manifest.run_id(), successor, 1000
    )
    denied = network.client.post(network.url + "/admin/relay-registry", json=wrong, headers=admin())
    assert denied.status_code == 409
    accepted = network.client.post(
        network.url + "/admin/relay-registry",
        json=relay_envelope.seal(
            COORD, "RelayRegistryV1", network.manifest.run_id(), successor, 1000
        ),
        headers=admin(),
    )
    assert accepted.status_code == 200, accepted.text
    assert network.store._registry_v2(network.manifest).digest() == successor.digest()
    body["epoch"], body["previous_registry_hash"] = 2, "0" * 64
    conflict = network.client.post(
        network.url + "/admin/relay-registry",
        json=relay_envelope.seal(
            COORD,
            "RelayRegistryV1",
            network.manifest.run_id(),
            RelayRegistryV1.model_validate(body),
            1000,
        ),
        headers=admin(),
    )
    assert conflict.status_code == 409
    assert network.store._registry_v2(network.manifest).digest() == successor.digest()


def test_failure_json_flags_cannot_create_relay_or_miner_authority(network):
    before = network.store._db.total_changes
    response = network.client.post(
        network.url + "/admin/relay-failure",
        json={"healthy": True, "fraud": True},
        headers=admin(),
    )
    assert response.status_code == 422
    assert network.store._db.total_changes == before
    assert network.store._services(network.manifest.run_id())[0].balances().burned == 0


def test_integrated_relay_lifecycle_original_custody_gate(network, monkeypatch, tmp_path):
    """Reuse genuine ACTIVE12 snapshot; one live round, no repeated graduation."""
    import json
    import shutil
    import sqlite3

    import anyio
    import httpx

    from hypertrain.data.store import LocalFSStore
    from hypertrain.data.stream_store import StreamStore
    from hypertrain.protocol import relay_envelope
    from hypertrain.protocol.relay_messages import (
        AcceptedUploadAck,
        AvailabilityContext,
        ChunkCustodyAck,
        RelayFailureEvidence,
        RelayReceipt,
        RelayRegistryV1,
        UploadGrant,
    )
    from hypertrain.relay.app import create_app as relay_app
    from hypertrain.relay.core import Relay, Settlement

    spec = importlib.util.spec_from_file_location(
        "relay_lifecycle_e2e",
        Path(__file__).parents[1] / "e2e/test_service_network_v2_e2e.py",
    )
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    alternate_key = Keypair(b"\x76" * 32)
    original_live = module._live_checkpoint
    original_signed = network.signed
    alternate_registry = None

    def publish(reg):
        response = network.client.post(
            network.url + "/admin/relay-registry",
            json=relay_envelope.seal(
                COORD, "RelayRegistryV1", network.manifest.run_id(), reg, 10000
            ),
            headers=admin(),
        )
        assert response.status_code == 200, response.text

    def signed_with_registry(key, type, body, expiry=10000):
        if type == "RoundOpenV2":
            body = body.model_copy(update={"registry_epoch": alternate_registry.epoch})
        return original_signed(key, type, body, expiry)

    def live_with_alternate(n):
        nonlocal alternate_registry
        previous = n.store._registry_v2(n.manifest)
        body = previous.body()
        body.update(epoch=previous.epoch + 1, previous_registry_hash=previous.digest())
        extra = dict(body["specs"][0])
        extra.update(id="fallback", region="z-local", https_url="https://fallback.test")
        extra["pubkeys"] = [
            {
                "key_id": "f1",
                "pubkey": alternate_key.ss58,
                "valid_from_round": 0,
                "valid_until_round": 10000,
            }
        ]
        body["specs"].append(extra)
        alternate_registry = RelayRegistryV1.model_validate(body)
        publish(alternate_registry)
        monkeypatch.setattr(n, "signed", signed_with_registry)
        original_live(n)

    monkeypatch.setattr(module, "_live_checkpoint", live_with_alternate)
    captured = {}
    original_receipt = network.store.relay_receipt_v2

    async def capture(raw, client):
        if not captured:
            db = sqlite3.connect(tmp_path / "before-accept.db")
            network.store._db.backup(db)
            db.close()
            captured["raw"] = raw
        return await original_receipt(raw, client)

    monkeypatch.setattr(network.store, "relay_receipt_v2", capture)
    watched = module._funded_dispute_watch
    real_post = network.client.post
    unresolved_rejections = []

    def post_with_dispute_pin(url, *args, **kwargs):
        response = real_post(url, *args, **kwargs)
        if url == network.url + "/dispute" and response.status_code == 200:
            receipt = RelayReceipt.model_validate(
                relay_envelope.parse_envelope(captured["raw"]).body
            )
            pinned = real_post(
                network.url + f"/admin/relay/{receipt.grant_hash}/release",
                headers=admin(),
            )
            assert pinned.status_code == 409, pinned.text
            unresolved_rejections.append(pinned.status_code)
        return response

    def dispute_pin(n):
        monkeypatch.setattr(network.client, "post", post_with_dispute_pin)
        watched(n)
        monkeypatch.setattr(network.client, "post", real_post)

    monkeypatch.setattr(module, "_funded_dispute_watch", dispute_pin)
    module.test_four_actual_funded_identities_graduate_shadow_only(network, monkeypatch)
    assert unresolved_rejections == [409, 409, 409]
    monkeypatch.setattr(network.store, "relay_receipt_v2", original_receipt)
    receipt_env = relay_envelope.parse_envelope(captured["raw"])
    receipt = RelayReceipt.model_validate(receipt_env.body)
    run_id = network.manifest.run_id()
    grant = UploadGrant.model_validate(
        network.store._record_v2(run_id, "grant", receipt.grant_hash)["body"]
    )
    assigned = network.store._record_v2(run_id, "relay-assignment", f"{grant.w}:{grant.hotkey}")
    registry = network.store._registry_v2(network.manifest)
    real_client = httpx.AsyncClient
    backing_http = real_client()

    async def now():
        return network.now

    async def settled(grant_hash):
        return Settlement(**network.store.relay_settlement_v2(run_id, grant_hash))

    relay = Relay(
        run_id=run_id,
        master=COORD.ss58,
        registry=registry,
        network_manifest_hash=assigned["body"]["manifest_hash"],
        observers={a.ss58 for a in AUDITORS},
        relay_id="local",
        region="local",
        keys={"k1": RELAY},
        active_key="k1",
        backing=StreamStore(LocalFSStore(network.store.state_dir / "relay-backing"), backing_http),
        now=now,
        settlement=settled,
    )
    shutil.copytree(network.store.state_dir / "relay-backing", tmp_path / "relay-before-release")
    app = relay_app(relay, "x" * 32)

    def transport_client(*args, **kwargs):
        kwargs["transport"] = httpx.ASGITransport(app=app)
        return real_client(*args, **kwargs)

    monkeypatch.setattr(httpx, "AsyncClient", transport_client)
    # Release rejects before vesting/grace; signed finality is real live evidence.
    response = network.client.post(
        network.url + f"/admin/relay/{grant.digest()}/release", headers=admin()
    )
    assert response.status_code == 409, response.text
    settlement = network.store.relay_settlement_v2(run_id, grant.digest())
    network.push(max(settlement["vesting_beacon"], settlement["closed_beacon"]) + 100)
    response = network.client.post(
        network.url + f"/admin/relay/{grant.digest()}/release", headers=admin()
    )
    assert response.status_code == 200, response.text
    assert response.json()["type"] == "CustodyRelease"

    async def check_release():
        state = await relay.state()
        pin = state.uploads[grant.digest()].pins[receipt.digest()]
        assert pin.delete_after >= pin.horizon
        assert all(p.delete_after is not None for p in state.uploads[grant.digest()].pins.values())
        assert grant.delta_hash not in await relay.eligible_deletions()

    anyio.run(check_release)

    # Restore actual pre-accept authority, not fabricated health/admission/finality.
    network.store.close_v2_notifications()
    network.store._services_v2.clear()
    db = sqlite3.connect(tmp_path / "before-accept.db")
    db.backup(network.store._db)
    db.close()
    network.now = network.store._now(network.store._db)
    shutil.rmtree(network.store.state_dir / "relay-backing")
    shutil.copytree(tmp_path / "relay-before-release", network.store.state_dir / "relay-backing")

    # Restore both authorities at the same snapshot; preserve original signatures.
    async def custody():
        record = (await relay.state()).uploads[grant.digest()]
        return record.opportunity, list(record.chunks.values())

    opportunity_env, chunk_envs = anyio.run(custody)
    opportunity = AcceptedUploadAck.model_validate(opportunity_env.body)
    chunks = [ChunkCustodyAck.model_validate(e.body) for e in chunk_envs]
    artifacts = [
        opportunity_env.model_dump(mode="json"),
        *[e.model_dump(mode="json") for e in chunk_envs],
    ]
    network.push(opportunity.service_deadline + 1)
    before = network.store._services(run_id)[0].balances()

    def evidence_for(items, *, ids=None, hash_override=None):
        ids = [a.ss58 for a in AUDITORS] if ids is None else ids
        evidence = RelayFailureEvidence(
            run_id=run_id,
            relay_id="local",
            key_id="k1",
            assignment_hash=sha256_hex(canonicalize(assigned["body"])),
            grant_hash=grant.digest(),
            upload_ack_hash=opportunity.digest(),
            chunk_manifest_hash=grant.chunk_manifest_hash,
            chunk_ack_hashes=[c.digest() for c in chunks],
            receipt_hash=None,
            retention_hash=sha256_hex(canonicalize(sorted(c.digest() for c in chunks))),
            request=None,
            response=None,
            observer_ids=ids,
            observed_beacons=[network.now] * len(ids),
            result="CONTRACTUAL_SERVICE_FAILURE",
            availability_context=AvailabilityContext(
                master_healthy=True,
                beacon_healthy=True,
                reference_healthy=True,
                observer_healthy=True,
            ),
            evidence_hash=hash_override or sha256_hex(canonicalize(items)),
        )
        signed = relay_envelope.seal(
            COORD, "RelayFailureEvidence", run_id, evidence, network.now + 20
        )
        return evidence, {
            "evidence": signed,
            "artifacts": items,
            "observations": [
                relay_envelope.seal(a, "RelayFailureEvidence", run_id, evidence, network.now + 20)
                for a in AUDITORS
            ],
        }

    evidence, body = evidence_for(artifacts)
    # Signed booleans alone do not establish observer health.
    denied = network.client.post(network.url + "/admin/relay-failure", json=body, headers=admin())
    assert denied.status_code == 409, denied.text
    for auditor in AUDITORS:
        url = network.url + f"/relay-observers/{grant.digest()}/{auditor.ss58}"
        challenge = network.client.get(url)
        assert challenge.status_code == 200, challenge.text
        forged = network.client.post(
            url, json=network.signed(HOT[0], "Receipt", challenge.json()["body"])
        )
        assert forged.status_code == 409
        healthy = network.client.post(
            url, json=network.signed(auditor, "Receipt", challenge.json()["body"])
        )
        assert healthy.status_code == 200, healthy.text
    # Original opportunity only cannot frame byte custody; mixed scope rejected.
    _, prefix = evidence_for([artifacts[0]])
    assert (
        network.client.post(
            network.url + "/admin/relay-failure", json=prefix, headers=admin()
        ).status_code
        == 422
    )
    mixed = json.loads(json.dumps(body))
    mixed["observations"][1] = mixed["observations"][0]
    assert (
        network.client.post(
            network.url + "/admin/relay-failure", json=mixed, headers=admin()
        ).status_code
        == 403
    )
    stale_body = json.loads(json.dumps(body))
    stale_body["observations"] = [
        relay_envelope.seal(a, "RelayFailureEvidence", run_id, evidence, network.now)
        for a in AUDITORS
    ]
    assert (
        network.client.post(
            network.url + "/admin/relay-failure", json=stale_body, headers=admin()
        ).status_code
        == 403
    )
    altered = evidence.model_copy(update={"assignment_hash": "0" * 64})
    wrong_scope = {
        **body,
        "evidence": relay_envelope.seal(COORD, "RelayFailureEvidence", run_id, altered, 10000),
        "observations": [
            relay_envelope.seal(a, "RelayFailureEvidence", run_id, altered, 10000) for a in AUDITORS
        ],
    }
    assert (
        network.client.post(
            network.url + "/admin/relay-failure", json=wrong_scope, headers=admin()
        ).status_code
        == 422
    )
    accepted = network.client.post(network.url + "/admin/relay-failure", json=body, headers=admin())
    assert accepted.status_code == 200, accepted.text
    assert accepted.json() == {
        "result": "CONTRACTUAL_SERVICE_FAILURE",
        "disabled": True,
        "miner_penalty": 0,
    }
    receipt_artifacts = [artifacts[0], receipt_env.model_dump(mode="json")]
    receipt_evidence = evidence.model_copy(
        update={
            "chunk_ack_hashes": [],
            "receipt_hash": receipt.digest(),
            "retention_hash": receipt.digest(),
            "evidence_hash": sha256_hex(canonicalize(receipt_artifacts)),
        }
    )
    receipt_body = {
        "evidence": relay_envelope.seal(
            COORD, "RelayFailureEvidence", run_id, receipt_evidence, 10000
        ),
        "artifacts": receipt_artifacts,
        "observations": [
            relay_envelope.seal(a, "RelayFailureEvidence", run_id, receipt_evidence, 10000)
            for a in AUDITORS
        ],
    }
    accepted_receipt = network.client.post(
        network.url + "/admin/relay-failure", json=receipt_body, headers=admin()
    )
    assert accepted_receipt.status_code == 200, accepted_receipt.text
    # Rescue invokes actual signed retrieval/chunk bytes; no backdated acceptance.
    rescued = network.client.post(
        network.url + f"/admin/relay-fallback/{evidence.digest()}", headers=admin()
    )
    assert rescued.status_code == 200, rescued.text
    assert rescued.json()["rescued"] and rescued.json()["master_acceptance"] is None
    assert rescued.json()["assignment"] == assigned
    assert network.store.objects.get(grant.delta_hash) == relay.backing.store.get(grant.delta_hash)
    assert network.store._services(run_id)[0].balances() == before

    # Signed successor retains current custody key; current/next validity explicit.
    successor_body = registry.body()
    successor_body.update(epoch=registry.epoch + 1, previous_registry_hash=registry.digest())
    next_key = Keypair(b"\x75" * 32)
    successor_body["specs"][0]["pubkeys"].append(
        {
            "key_id": "k2",
            "pubkey": next_key.ss58,
            "valid_from_round": network.now,
            "valid_until_round": 10000,
        }
    )
    successor = RelayRegistryV1.model_validate(successor_body)
    rotated = network.client.post(
        network.url + "/admin/relay-registry",
        json=relay_envelope.seal(COORD, "RelayRegistryV1", run_id, successor, 10000),
        headers=admin(),
    )
    assert rotated.status_code == 200, rotated.text

    async def rotation_ready():
        rotated_relay = Relay(
            run_id=run_id,
            master=COORD.ss58,
            registry=successor,
            network_manifest_hash=assigned["body"]["manifest_hash"],
            observers={a.ss58 for a in AUDITORS},
            relay_id="local",
            region="local",
            keys={"k1": RELAY, "k2": next_key},
            active_key="k2",
            backing=relay.backing,
            now=now,
            settlement=settled,
        )
        assert await rotated_relay.ready()
        original = (await rotated_relay.state()).uploads[grant.digest()]
        assert original.opportunity == opportunity_env
        assert original.chunks == {str(c.body["index"]): c for c in chunk_envs}

    anyio.run(rotation_ready)
    dropped = successor.body()
    dropped.update(epoch=successor.epoch + 1, previous_registry_hash=successor.digest())
    dropped["specs"][0]["pubkeys"] = [dropped["specs"][0]["pubkeys"][1]]
    rejected = network.client.post(
        network.url + "/admin/relay-registry",
        json=relay_envelope.seal(
            COORD,
            "RelayRegistryV1",
            run_id,
            RelayRegistryV1.model_validate(dropped),
            10000,
        ),
        headers=admin(),
    )
    assert rejected.status_code == 409, rejected.text
    # Timely branch: extend registry via signed successor before granting a miner
    # whose accepted commit exists but whose relay assignment was not yet issued.
    network.store.close_v2_notifications()
    network.store._services_v2.clear()
    db = sqlite3.connect(tmp_path / "before-accept.db")
    db.backup(network.store._db)
    db.close()
    network.now = network.store._now(network.store._db)
    network.push(network.now)
    publish(alternate_registry)
    assigned_other = network.client.get(network.url + f"/rounds/0/relay/{HOT[1].ss58}")
    assert assigned_other.status_code == 200, assigned_other.text
    assigned_other = assigned_other.json()
    assert assigned_other["assignment"]["body"]["fallback_ids"] == ["fallback"]
    path = network.store.state_dir / "jobs-v2/0" / HOT[1].ss58 / "published/rank-0/delta.bin"
    from hypertrain.relay.client import RelayClient, chunk_manifest

    chunks_other = chunk_manifest(path)
    response = network.client.post(
        network.url + "/upload-grant",
        json=relay_envelope.seal(HOT[1], "UploadChunkManifest", run_id, chunks_other, 10000),
    )
    assert response.status_code == 200, response.text
    original_grant = UploadGrant.model_validate(response.json()["body"])
    uncertain = RelayFailureEvidence(
        run_id=run_id,
        relay_id="local",
        key_id="k1",
        assignment_hash=sha256_hex(canonicalize(assigned_other["assignment"]["body"])),
        grant_hash=original_grant.digest(),
        upload_ack_hash="0" * 64,
        chunk_manifest_hash=original_grant.chunk_manifest_hash,
        chunk_ack_hashes=[],
        receipt_hash=None,
        retention_hash="0" * 64,
        request=None,
        response=None,
        observer_ids=[AUDITORS[0].ss58],
        observed_beacons=[network.now],
        result="UNCONFIRMED_AVAILABILITY",
        availability_context=AvailabilityContext(
            master_healthy=False,
            beacon_healthy=False,
            reference_healthy=False,
            observer_healthy=False,
        ),
        evidence_hash=sha256_hex(canonicalize([])),
    )
    response = network.client.post(
        network.url + "/admin/relay-failure",
        json={
            "evidence": relay_envelope.seal(
                COORD, "RelayFailureEvidence", run_id, uncertain, 10000
            ),
            "artifacts": [],
            "observations": [],
        },
        headers=admin(),
    )
    assert response.status_code == 200 and not response.json()["disabled"]
    response = network.client.post(
        network.url + f"/admin/relay-fallback/{uncertain.digest()}", headers=admin()
    )
    assert response.status_code == 200, response.text
    fallback = response.json()
    reassigned = UploadGrant.model_validate(fallback["grant"]["body"])
    assert reassigned.relay_id == "fallback" and reassigned.exp_drand == original_grant.exp_drand
    assert reassigned.delta_hash == original_grant.delta_hash
    assert fallback["assignment"] == assigned_other["assignment"]

    async def actual_fallback():
        alternate = Relay(
            run_id=run_id,
            master=COORD.ss58,
            registry=alternate_registry,
            network_manifest_hash=assigned_other["assignment"]["body"]["manifest_hash"],
            observers={a.ss58 for a in AUDITORS},
            relay_id="fallback",
            region="z-local",
            keys={"f1": alternate_key},
            active_key="f1",
            backing=StreamStore(
                LocalFSStore(network.store.state_dir / "fallback-backing"), backing_http
            ),
            now=now,
        )
        async with real_client(
            transport=httpx.ASGITransport(app=relay_app(alternate, "x" * 32))
        ) as client:
            receipt = await RelayClient(alternate_registry, client).upload(
                path,
                grant_raw=relay_envelope.parse_envelope(fallback["grant"]),
                assignment=relay_envelope.parse_envelope(fallback["assignment"]),
                round_open=network.store._record_v2(run_id, "round", "0"),
            )
            accepted = await network.store.relay_receipt_v2(
                canonicalize(receipt.model_dump(mode="json")), client
            )
            assert accepted["body"]["delta_hash"] == original_grant.delta_hash

    anyio.run(actual_fallback)
    drained = network.client.post(network.url + "/admin/relay-drain/local", headers=admin())
    assert drained.status_code == 200
    new_grant = network.client.post(
        network.url + "/upload-grant",
        json=relay_envelope.seal(HOT[1], "UploadChunkManifest", run_id, chunks_other, 10000),
    )
    assert new_grant.status_code == 409, new_grant.text

    async def drain():
        async with real_client(
            transport=httpx.ASGITransport(app=app), base_url="https://relay.test"
        ) as client:
            assert (
                await client.post(
                    "/internal/drain", headers={"Authorization": "Bearer " + "x" * 32}
                )
            ).status_code == 200
            assert (await client.get("/readyz")).status_code == 503

    anyio.run(drain)
    anyio.run(backing_http.aclose)


@pytest.mark.parametrize("invalid", [None, "partial-receipt", "duplicate", "gap", "contradiction"])
def test_complete_receipt_prefix_rescue_http(network, monkeypatch, tmp_path, invalid):
    """Exact two-chunk receipt/prefix regression; no live-round/graduation sweep."""
    import json

    import anyio
    import httpx

    from hypertrain.data.store import LocalFSStore
    from hypertrain.data.stream_store import StreamStore
    from hypertrain.protocol import envelope_v2, relay_envelope
    from hypertrain.protocol.relay_messages import (
        AcceptedUploadAck,
        AvailabilityContext,
        ChunkCustodyAck,
        RelayAssignment,
        RelayFailureEvidence,
        RelayNetworkManifest,
        RelayReceipt,
        UploadChunk,
        UploadChunkManifest,
        UploadGrant,
    )
    from hypertrain.relay.app import create_app as relay_app
    from hypertrain.relay.core import Relay

    spec = importlib.util.spec_from_file_location(
        "receipt_prefix_shadow",
        Path(__file__).parents[1] / "e2e/test_service_network_v2_e2e.py",
    )
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    # One real accepted MATCH reference supplies observer-health byte authority.
    module.test_actual_shadow_trial_does_not_mutate_production(network)
    run_id = network.manifest.run_id()
    old = network.store._registry_v2(network.manifest)
    body = old.body()
    body.update(epoch=1, previous_registry_hash=old.digest())
    body["specs"][0].update(max_object_bytes=8 << 20, max_inflight_bytes=16 << 20)
    registry = RelayRegistryV1.model_validate(body)
    response = network.client.post(
        network.url + "/admin/relay-registry",
        json=relay_envelope.seal(COORD, "RelayRegistryV1", run_id, registry, 10000),
        headers=admin(),
    )
    assert response.status_code == 200, response.text
    payload = b"a" * (4 << 20) + b"b"
    chunks = UploadChunkManifest(
        size=len(payload),
        chunks=[
            UploadChunk(index=0, off=0, len=4 << 20, chunk_sha256=sha256_hex(payload[:-1])),
            UploadChunk(index=1, off=4 << 20, len=1, chunk_sha256=sha256_hex(payload[-1:])),
        ],
    )
    net = RelayNetworkManifest(
        run_id=run_id,
        base_manifest_hash=network.manifest.training.training_hash(),
        registry_hash=registry.digest(),
        specs_hash=sha256_hex(canonicalize([s.body() for s in registry.specs])),
        assignment_policy="master-observed-median3-v1",
    )
    assignment = RelayAssignment(
        w=0,
        hotkey=HOT[0].ss58,
        primary_id="local",
        fallback_ids=[],
        assignment_epoch=registry.epoch,
        manifest_hash=net.digest(),
        exp_drand=network.now + 20,
    )
    grant = UploadGrant(
        w=0,
        hotkey=HOT[0].ss58,
        relay_id="local",
        assignment_epoch=registry.epoch,
        delta_hash=sha256_hex(payload),
        size=len(payload),
        chunk_manifest_hash=chunks.chunk_manifest_hash(),
        retain_until=network.now + 200,
        nonce="ad" * 32,
        exp_drand=assignment.exp_drand,
    )
    signed_grant = relay_envelope.seal(COORD, "UploadGrant", run_id, grant, grant.exp_drand)
    signed_assignment = relay_envelope.seal(
        COORD, "RelayAssignment", run_id, assignment, assignment.exp_drand
    )
    # Accepted master journal adapter: original typed signatures, not supplied trust flags.
    with network.store._tx():
        network.store._put_record_v2(run_id, "grant", grant.digest(), signed_grant)
        network.store._put_record_v2(run_id, "chunk-manifest", grant.digest(), chunks.body())
        network.store._put_record_v2(
            run_id, "relay-assignment", "0:" + HOT[0].ss58, signed_assignment
        )
    opened = {
        "w": 0,
        **{
            k: "12" * 32
            for k in (
                "prev_final_hash",
                "theta_hash",
                "outer_state_hash",
                "center_hash",
                "roster_hash",
                "honeypot_commit",
                "start_state_index_hash",
            )
        },
        "d_open": 1,
        "d_assign": 2,
        "d_commit": network.now + 10,
        "d_audit": network.now + 11,
        "d_upload": grant.exp_drand,
        "d_final": network.now + 30,
        "contract_version": 2,
        "policy_hashes": {
            k: getattr(network.manifest.network, k)
            for k in (
                "admission_policy_hash",
                "economics_policy_hash",
                "aggregation_policy_hash",
                "dispute_policy_hash",
                "audit_policy_hash",
            )
        },
        "registry_epoch": registry.epoch,
        "audit_mode": "anchored-full",
        "roster": [],
    }
    real_client = httpx.AsyncClient
    backing = real_client()

    async def now():
        return network.now

    relay = Relay(
        run_id=run_id,
        master=COORD.ss58,
        registry=registry,
        network_manifest_hash=net.digest(),
        observers={a.ss58 for a in AUDITORS},
        relay_id="local",
        region="local",
        keys={"k1": RELAY},
        active_key="k1",
        backing=StreamStore(LocalFSStore(tmp_path / "prefix-backing"), backing),
        now=now,
    )

    async def uploaded():
        opportunity = await relay.accept(
            relay_envelope.parse_envelope(signed_grant),
            chunks,
            relay_envelope.parse_envelope(signed_assignment),
            envelope_v2.parse_envelope(network.signed(COORD, "RoundOpenV2", opened)),
        )

        async def source(part):
            yield part

        first = await relay.chunk(grant.digest(), 0, source(payload[:-1]))
        await relay.chunk(grant.digest(), 1, source(payload[-1:]))
        return opportunity, first, await relay.complete(grant.digest())

    opportunity_env, prefix_env, receipt_env = anyio.run(uploaded)
    opportunity = AcceptedUploadAck.model_validate(opportunity_env.body)
    prefix = ChunkCustodyAck.model_validate(prefix_env.body)
    receipt = RelayReceipt.model_validate(receipt_env.body)
    if invalid == "partial-receipt":
        receipt = receipt.model_copy(update={"size": prefix.len, "delta_hash": prefix.chunk_sha256})
        receipt_env = relay_envelope.parse_envelope(
            relay_envelope.seal(RELAY, "RelayReceipt", run_id, receipt, grant.retain_until)
        )
    if invalid in {"gap", "contradiction"}:
        prefix = prefix.model_copy(
            update={"off": 1} if invalid == "gap" else {"chunk_sha256": "ff" * 32}
        )
        prefix_env = relay_envelope.parse_envelope(
            relay_envelope.seal(RELAY, "ChunkCustodyAck", run_id, prefix, grant.retain_until)
        )
    artifacts = [
        opportunity_env.model_dump(mode="json"),
        prefix_env.model_dump(mode="json"),
        receipt_env.model_dump(mode="json"),
    ]
    if invalid == "duplicate":
        artifacts.insert(2, prefix_env.model_dump(mode="json"))
    network.push(grant.exp_drand + 1)
    for auditor in AUDITORS:
        url = network.url + f"/relay-observers/{grant.digest()}/{auditor.ss58}"
        challenge = network.client.get(url)
        assert challenge.status_code == 200, challenge.text
        response = network.client.post(
            url, json=network.signed(auditor, "Receipt", challenge.json()["body"])
        )
        assert response.status_code == 200, response.text
    evidence = RelayFailureEvidence(
        run_id=run_id,
        relay_id="local",
        key_id="k1",
        assignment_hash=assignment.digest(),
        grant_hash=grant.digest(),
        upload_ack_hash=opportunity.digest(),
        chunk_manifest_hash=grant.chunk_manifest_hash,
        chunk_ack_hashes=[prefix.digest()],
        receipt_hash=receipt.digest(),
        retention_hash=receipt.digest(),
        request=None,
        response=None,
        observer_ids=[a.ss58 for a in AUDITORS],
        observed_beacons=[network.now] * 2,
        result="CONTRACTUAL_SERVICE_FAILURE",
        availability_context=AvailabilityContext(
            master_healthy=True,
            beacon_healthy=True,
            reference_healthy=True,
            observer_healthy=True,
        ),
        evidence_hash=sha256_hex(canonicalize(artifacts)),
    )
    failure = network.client.post(
        network.url + "/admin/relay-failure",
        json={
            "evidence": relay_envelope.seal(COORD, "RelayFailureEvidence", run_id, evidence, 10000),
            "artifacts": artifacts,
            "observations": [
                relay_envelope.seal(a, "RelayFailureEvidence", run_id, evidence, 10000)
                for a in AUDITORS
            ],
        },
        headers=admin(),
    )
    if invalid == "partial-receipt":
        assert failure.status_code == 422, failure.text
        anyio.run(backing.aclose)
        return
    assert failure.status_code == 200, failure.text
    app = relay_app(relay, "x" * 32)
    monkeypatch.setattr(
        httpx,
        "AsyncClient",
        lambda *a, **kw: real_client(*a, **{**kw, "transport": httpx.ASGITransport(app=app)}),
    )
    rescued = network.client.post(
        network.url + f"/admin/relay-fallback/{evidence.digest()}", headers=admin()
    )
    if invalid is not None:
        assert rescued.status_code == 422, rescued.text
    else:
        assert rescued.status_code == 200, rescued.text
        assert rescued.json()["grant"] == signed_grant
        assert rescued.json()["assignment"] == signed_assignment
        assert rescued.json()["master_acceptance"] is None
        assert network.store.objects.get(grant.delta_hash) == payload
        records = [
            json.loads(r[0])["body"]
            for r in network.store._db.execute("SELECT data FROM records_v2 WHERE kind='retrieval'")
        ]
        assert len(records) == 1
        assert records[0]["receipt_hash"] == receipt.digest()
        assert records[0]["custody_ack_hashes"] == []
        assert records[0]["size"] == len(payload)
        assert records[0]["retention_hash"] == receipt.digest()
        assert records[0]["deadline_beacon"] == min(network.now + 10, receipt.retain_until)
    anyio.run(backing.aclose)


@pytest.mark.parametrize(
    "fault",
    [
        None,
        "round-mismatch",
        "arbitrary-subject",
        "ambiguous",
        "explicit-identical",
        "wrong-signer",
        "wrong-run",
        "unknown-commit",
    ],
)
def test_upload_grant_exact_subject_with_two_live_rounds(network, tmp_path, fault):
    import anyio
    import httpx

    from hypertrain.data.store import LocalFSStore
    from hypertrain.data.stream_store import StreamStore, spool
    from hypertrain.protocol import envelope_v2, relay_envelope
    from hypertrain.protocol.messages_v2 import CommitV2
    from hypertrain.protocol.relay_messages import (
        RelayAssignment,
        RelayNetworkManifest,
        RelayReceipt,
        RetrievalRequest,
        RetrievalResponse,
        UploadChunk,
        UploadChunkManifest,
        UploadGrant,
    )
    from hypertrain.relay.core import Relay

    assert network.join().status_code == 200
    run_id = network.manifest.run_id()
    registry = network.store._registry_v2(network.manifest)
    relay_net = RelayNetworkManifest(
        run_id=run_id,
        base_manifest_hash=network.manifest.training.training_hash(),
        registry_hash=registry.digest(),
        specs_hash=sha256_hex(canonicalize([s.body() for s in registry.specs])),
        assignment_policy="master-observed-median3-v1",
    )
    payloads = [b"old", b"new"]
    if fault in {"ambiguous", "explicit-identical"}:
        payloads[1] = payloads[0]
    assignments, chunks, commit_hashes = [], [], []
    for w, payload in enumerate(payloads):
        chunk = UploadChunkManifest(
            size=3,
            chunks=[UploadChunk(index=0, off=0, len=3, chunk_sha256=sha256_hex(payload))],
        )
        chunks.append(chunk)
        assignment = RelayAssignment(
            w=w,
            hotkey=HOT[0].ss58,
            primary_id="local",
            fallback_ids=[],
            assignment_epoch=0,
            manifest_hash=relay_net.digest(),
            exp_drand=552 + w * 31,
        )
        signed = relay_envelope.seal(
            COORD, "RelayAssignment", run_id, assignment, assignment.exp_drand
        )
        assignments.append(signed)
        commit = CommitV2.model_validate(
            {
                "w": w + 1 if fault == "round-mismatch" and w == 1 else w,
                "hotkey": HOT[0].ss58,
                "leaf_scheme": "ht-leaf-v1",
                "n_leaves": 2,
                "leaves_root": "01" * 32,
                "metrics_root": "02" * 32,
                "final_theta_hash": "03" * 32,
                "tokens": 8,
                "ef_in_hash": "04" * 32,
                "ef_out_hash": "05" * 32,
                "delta_hash": sha256_hex(payload),
                "delta_bytes": 3,
            }
        )
        commit_hashes.append(sha256_hex(canonicalize(commit.model_dump(mode="json"))))
        committed_envelope = network.signed(HOT[0], "CommitV2", commit)
        if w == 1 and fault == "wrong-signer":
            committed_envelope = network.signed(HOT[1], "CommitV2", commit)
        if w == 1 and fault == "wrong-run":
            committed_envelope = envelope_v2.seal(HOT[0], "CommitV2", "ff" * 32, commit, 10000)
        with network.store._tx():
            network.store.objects.put(payload)
            network.store._put_record_v2(run_id, "relay-assignment", f"{w}:{HOT[0].ss58}", signed)
            network.store._put_record_v2(run_id, "commit", f"{w}:{HOT[0].ss58}", committed_envelope)
    # Exact accepted journal boundary, real signed subjects; no graduation/train.
    network.push(385)
    requested = chunks[1]
    if fault == "arbitrary-subject":
        requested = UploadChunkManifest(
            size=3,
            chunks=[UploadChunk(index=0, off=0, len=3, chunk_sha256=sha256_hex(b"bad"))],
        )
    before = list(
        network.store._db.execute(
            "SELECT id,data FROM records_v2 WHERE kind='relay-assignment' ORDER BY id"
        )
    )
    response = network.client.post(
        network.url + "/upload-grant",
        params={"commit_hash": "00" * 32 if fault == "unknown-commit" else commit_hashes[1]}
        if fault
        in {
            "explicit-identical",
            "wrong-signer",
            "wrong-run",
            "unknown-commit",
            "round-mismatch",
        }
        else {},
        json=relay_envelope.seal(HOT[0], "UploadChunkManifest", run_id, requested, 583),
    )
    if fault not in {None, "explicit-identical"}:
        assert response.status_code == 409, response.text
        assert (
            network.store._db.execute(
                "SELECT COUNT(*) FROM records_v2 WHERE kind='grant'"
            ).fetchone()[0]
            == 0
        )
        return
    assert response.status_code == 200, response.text
    grant1 = UploadGrant.model_validate(response.json()["body"])
    assert grant1.w == 1 and grant1.delta_hash == sha256_hex(payloads[1])
    assert grant1.exp_drand == 583
    old = network.client.post(
        network.url + "/upload-grant",
        params={"commit_hash": commit_hashes[0]} if fault == "explicit-identical" else {},
        json=relay_envelope.seal(HOT[0], "UploadChunkManifest", run_id, chunks[0], 552),
    )
    assert old.status_code == 200, old.text
    grant0 = UploadGrant.model_validate(old.json()["body"])
    assert grant0.w == 0 and grant0.exp_drand == 552
    assert [tuple(r) for r in before] == [
        tuple(r)
        for r in network.store._db.execute(
            "SELECT id,data FROM records_v2 WHERE kind='relay-assignment' ORDER BY id"
        )
    ]

    async def old_custody():
        async with httpx.AsyncClient() as client:

            async def now():
                return network.now

            relay = Relay(
                run_id=run_id,
                master=COORD.ss58,
                registry=registry,
                network_manifest_hash=relay_net.digest(),
                observers={a.ss58 for a in AUDITORS},
                relay_id="local",
                region="local",
                keys={"k1": RELAY},
                active_key="k1",
                backing=StreamStore(LocalFSStore(tmp_path / "old-custody"), client),
                now=now,
            )
            opened = {
                "w": 0,
                **{
                    k: "12" * 32
                    for k in (
                        "prev_final_hash",
                        "theta_hash",
                        "outer_state_hash",
                        "center_hash",
                        "roster_hash",
                        "honeypot_commit",
                        "start_state_index_hash",
                    )
                },
                "d_open": 1,
                "d_assign": 2,
                "d_commit": 400,
                "d_audit": 401,
                "d_upload": 552,
                "d_final": 600,
                "contract_version": 2,
                "policy_hashes": {
                    k: getattr(network.manifest.network, k)
                    for k in (
                        "admission_policy_hash",
                        "economics_policy_hash",
                        "aggregation_policy_hash",
                        "dispute_policy_hash",
                        "audit_policy_hash",
                    )
                },
                "registry_epoch": 0,
                "audit_mode": "anchored-full",
                "roster": [],
            }
            await relay.accept(
                relay_envelope.parse_envelope(old.json()),
                chunks[0],
                relay_envelope.parse_envelope(assignments[0]),
                envelope_v2.parse_envelope(network.signed(COORD, "RoundOpenV2", opened)),
            )

            async def source():
                yield payloads[0]

            await relay.chunk(grant0.digest(), 0, source())
            receipt = RelayReceipt.model_validate((await relay.complete(grant0.digest())).body)
            request = RetrievalRequest(
                request_id="ab" * 32,
                nonce="ac" * 32,
                relay_id="local",
                key_id="k1",
                assignment_hash=sha256_hex(canonicalize(assignments[0]["body"])),
                grant_hash=grant0.digest(),
                custody_ack_hashes=[],
                receipt_hash=receipt.digest(),
                retention_hash=receipt.digest(),
                object_or_chunk_hash=grant0.delta_hash,
                size=3,
                requested_beacon=385,
                deadline_beacon=395,
            )
            served = await relay.retrieve(
                relay_envelope.parse_envelope(
                    relay_envelope.seal(COORD, "RetrievalRequest", run_id, request, 395)
                )
            )
            assert RetrievalResponse.model_validate(served.body).status == "SERVED"
            with spool() as file:
                await relay.backing.get(grant0.delta_hash, 3, file)
                file.seek(0)
                assert file.read() == payloads[0]

    anyio.run(old_custody)


def test_network_miner_upload_grant_sends_accepted_commit_body_subject(
    network, monkeypatch, tmp_path
):
    import json
    from types import SimpleNamespace
    from urllib.parse import parse_qs, urlsplit

    import httpx

    from hypertrain.miner.core import NetworkMiner
    from hypertrain.protocol.envelope import body_digest
    from hypertrain.protocol.messages_v2 import PolicyHashes, RoundOpenV2

    job = fixture.stage(network.setup, tmp_path / "client-job", (0,), 0)
    directory = tmp_path / "published"
    (directory / "rank-0").mkdir(parents=True)
    commitments = {
        k: "01" * 32 for k in ("leaves_root", "final_theta_hash", "ef_out_hash", "delta_hash")
    }
    commitments["delta_hash"] = sha256_hex(b"new")
    (directory / "rank-0/summary.json").write_bytes(canonicalize({"commitments": commitments}))
    files = {}
    for name, raw in (
        ("state", b"state"),
        ("ef", b"ef"),
        ("delta", b"new"),
        ("leaves", canonicalize([{"loss_f32": "00000000", "norm_f32": "00000000"}])),
    ):
        files[name] = directory / name
        files[name].write_bytes(raw)
    import hypertrain.miner.island_launch as launch

    monkeypatch.setattr(
        launch,
        "launch_island",
        lambda *a, **kw: SimpleNamespace(directory=directory, **files),
    )
    opening = RoundOpenV2(
        w=1,
        **{
            k: "12" * 32
            for k in (
                "prev_final_hash",
                "theta_hash",
                "outer_state_hash",
                "center_hash",
                "roster_hash",
                "honeypot_commit",
                "start_state_index_hash",
            )
        },
        d_open=380,
        d_assign=385,
        d_commit=413,
        d_audit=414,
        d_upload=583,
        d_final=600,
        contract_version=2,
        registry_epoch=0,
        audit_mode="anchored-full",
        roster=[],
        policy_hashes=PolicyHashes.model_validate(
            {k: getattr(network.manifest.network, k) for k in PolicyHashes.model_fields}
        ),
    )
    saved = {}

    class CapturedSubject(Exception):
        pass

    def call(method, path, **kwargs):
        if path.endswith("/rounds/1"):
            return {
                "round_open": network.signed(COORD, "RoundOpenV2", opening),
                "assignment": [{"hotkey": HOT[0].ss58, "assignment_hash": "11" * 32}],
                "starts": [{"hotkey": HOT[0].ss58, "ef_hash": "22" * 32}],
            }
        if path.endswith("/job/" + HOT[0].ss58):
            return {"job": job.body(), "objects": {}}
        if path.endswith("/commit"):
            saved["commit"] = kwargs["json"]
            return {}
        if path.endswith("/relay/" + HOT[0].ss58):
            return {}
        if urlsplit(path).path.endswith("/upload-grant"):
            assert parse_qs(urlsplit(path).query)["commit_hash"] == [
                body_digest(saved["commit"]["body"])
            ]
            assert body_digest(saved["commit"]["body"]) != body_digest(saved["commit"])
            assert body_digest(saved["commit"]["body"]) != commitments["delta_hash"]
            raise CapturedSubject()
        return {}

    # Use exact pinned policy object from registered authority, not fixture hash.
    policy = network.store.objects.get(network.manifest.network.economics_policy_hash)

    def wire(request):
        return httpx.Response(200, content=policy if request.method == "GET" else b"{}")

    actor = NetworkMiner.__new__(NetworkMiner)
    actor.manifest, actor.kp, actor.run_id = (
        network.manifest,
        HOT[0],
        network.manifest.run_id(),
    )
    actor.url, actor.directory = network.url, tmp_path / "client"
    actor.cfg = SimpleNamespace(device="cpu")
    actor.status = lambda: {
        "work_screen_hash": "33" * 32,
        "record": {"admission_id": "44" * 32},
    }
    with httpx.Client(transport=httpx.MockTransport(wire)) as client:
        actor.api = SimpleNamespace(call=call, c=client, base="https://master.test")
        with pytest.raises(CapturedSubject):
            actor.run_round(1)
    assert json.loads(canonicalize(saved["commit"]))["body"]["w"] == 1
