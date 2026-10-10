"""One bounded outer arithmetic file contract; no wallet, SQL or job scheduler."""

from __future__ import annotations

import os
import stat
import sys
from collections.abc import Iterator
from contextlib import contextmanager
from pathlib import Path
from typing import Annotated

from pydantic import Field, JsonValue

from hypertrain.aggregator.tape_v2 import TapeBodyV2, VerifiedInput, compute_body
from hypertrain.challenge.trust_v2 import FundedStatus, ReplayEvidence, SettlementStatus
from hypertrain.data.store import LocalFSStore
from hypertrain.protocol.envelope_v2 import load_json
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages import Hex64
from hypertrain.protocol.messages_v2 import (
    AggregationPolicyV2,
    CommitV2,
    DeltaManifestV2,
    RosterEntryV2,
    RunManifestV2,
    StartStateV2,
    WireModel,
)


class GenesisRequest(WireModel):
    operation: Annotated[str, Field(pattern="^genesis$")] = "genesis"
    manifest: RunManifestV2
    profile: dict[str, JsonValue]
    manifest_envelope: dict[str, JsonValue]
    receipt: dict[str, JsonValue]
    opening: dict[str, JsonValue]
    accepted: Annotated[int, Field(strict=True, ge=1)]

    def validate_authority(self) -> list[RosterEntryV2]:
        from hypertrain.challenge.store import ChallengeStore
        from hypertrain.protocol import envelope_v2
        from hypertrain.protocol.messages import Receipt
        from hypertrain.protocol.messages_v2 import RoundOpenV2

        _geometry(self.manifest)
        run = self.manifest.run_id()
        manifest_env = envelope_v2.parse_envelope(canonicalize(self.manifest_envelope))
        if (
            manifest_env.run_id != run
            or manifest_env.type != "RunManifestV2"
            or manifest_env.body != self.manifest.body()
            or not envelope_v2.verify_envelope(self.manifest_envelope)
        ):
            raise ValueError("capacity genesis manifest authority differs")
        receipt = envelope_v2.Intake(run, {"Receipt": manifest_env.signer.__eq__}).accept(
            canonicalize(self.receipt), self.accepted
        )
        opening = envelope_v2.Intake(
            run, {"RoundOpenV2": self.manifest.training.coord_pubkey.__eq__}
        ).accept(canonicalize(self.opening), self.accepted)
        assert isinstance(receipt, Receipt) and isinstance(opening, RoundOpenV2)
        paths = (
            "aggregator/core.py",
            "aggregator/tape_v2.py",
            "aggregator/rollback_v2.py",
            "trainer/compress.py",
            "trainer/config.py",
            "trainer/model.py",
            "miner/island_launch.py",
            "aggregator/capacity_worker.py",
            "auditor/replay.py",
        )
        source = ChallengeStore._service_implementation_v2(
            Path(__file__).resolve().parents[1], paths
        )
        if (
            self.profile.get("run_id") != run
            or self.profile.get("implementation_hash") != source
            or receipt.commit_hash != sha256_hex(canonicalize(self.profile))
            or receipt.w != 0
            or receipt.received_round != self.accepted
            or opening.w != 0
            or opening.d_final <= self.accepted
            or opening.policy_hashes.body()
            != {k: getattr(self.manifest.network, k) for k in opening.policy_hashes.body()}
            or len({x.hotkey for x in opening.roster}) != len(opening.roster)
            or opening.roster_hash != sha256_hex(canonicalize([x.body() for x in opening.roster]))
        ):
            raise ValueError("capacity genesis authority differs")
        return opening.roster


class GenesisResult(WireModel):
    request_hash: Hex64
    starts: Annotated[list[StartStateV2], Field(min_length=4, max_length=32)]
    state_root: Hex64
    outer_state: Hex64
    outer_hashes: dict[str, Hex64]


def collect_genesis(
    directory: Path, request: GenesisRequest
) -> tuple[GenesisResult, dict[str, bytes]]:
    roster = request.validate_authority()
    with _directory_fd(directory) as root:
        raw = _read_at(root, Path("result.json"), 1 << 20)
        result = GenesisResult.model_validate(load_json(raw, max_bytes=1 << 20))
        if (
            canonicalize(result.body()) != raw
            or result.request_hash != sha256_hex(canonicalize(request.body()))
            or [s.hotkey for s in result.starts] != [r.hotkey for r in roster]
        ):
            raise ValueError("capacity genesis result binding differs")
        first = result.starts[0]
        for start in result.starts:
            proof = sha256_hex(
                bytes.fromhex(request.manifest.run_id() + start.opt_state_hash + start.ef_hash)
            )
            if (
                start.run_id != request.manifest.run_id()
                or start.w != 0
                or start.global_step0 != 0
                or start.parent_anchor_hash != proof
                or start.anchor_verdict_hash != proof
                or start.model_dump(exclude={"hotkey"}) != first.model_dump(exclude={"hotkey"})
            ):
                raise ValueError("capacity genesis start authority differs")
        if (
            set(result.outer_hashes) != {"theta_hash", "outer_state_hash", "center_hash"}
            or result.outer_hashes["theta_hash"] != first.theta_hash
        ):
            raise ValueError("capacity genesis outer metadata differs")
        objects = {}
        for key in {result.outer_state, first.state_object_sha256, first.ef_object_sha256}:
            data = _read_at(root, Path("objects") / key[:2] / key, 65536)
            if sha256_hex(data) != key:
                raise ValueError("capacity genesis object hash differs")
            objects[key] = data
        return result, objects


def _genesis(root: int, request: GenesisRequest) -> None:
    from hypertrain.aggregator.core import OuterState
    from hypertrain.auditor.replay import AnchorCache, optimizer_hash, pack_state, tensor_root
    from hypertrain.trainer.compress import state_hash
    from hypertrain.trainer.config import TrainConfig
    from hypertrain.trainer.model import init_params

    roster = request.validate_authority()
    theta = init_params(TrainConfig.from_manifest_v2(request.manifest).model)
    cache = AnchorCache()
    starts = []

    def put(data: bytes) -> str:
        if len(data) > 65536:
            raise ValueError("capacity genesis object too large")
        key = sha256_hex(data)
        _write_at(root, Path("objects") / key[:2] / key, data)
        return key

    for entry in roster:
        anchor = cache.genesis(request.manifest, entry.hotkey, theta)
        starts.append(
            StartStateV2(
                run_id=request.manifest.run_id(),
                w=0,
                hotkey=entry.hotkey,
                theta_hash=state_hash(theta),
                state_object_sha256=put(pack_state(theta, anchor.state)),
                opt_state_hash=optimizer_hash(anchor.state),
                ef_object_sha256=put(pack_state(anchor.ef)),
                ef_hash=state_hash(anchor.ef),
                parent_anchor_hash=anchor.anchor_hash,
                global_step0=anchor.state.step,
                anchor_verdict_hash=anchor.proof_hash,
            )
        )
    outer = OuterState.init({k: v.detach().cpu().numpy() for k, v in theta.items()})
    result = GenesisResult(
        request_hash=sha256_hex(canonicalize(request.body())),
        starts=starts,
        state_root=tensor_root(theta, anchor.state),
        outer_state=put(outer.to_bytes()),
        outer_hashes=outer.hashes(),
    )
    _write_at(root, Path("result.json"), canonicalize(result.body()))
    print("CAPACITY_GENESIS_COMPLETE", flush=True)


def _geometry(manifest: RunManifestV2) -> None:
    model, inner, layout = (
        manifest.training.model,
        manifest.training.inner,
        manifest.training.reference_spec.layout,
    )
    if (
        (
            model.arch,
            model.n_layers,
            model.d_model,
            model.n_heads,
            model.n_kv_heads,
            model.d_ff,
            model.vocab,
            model.seq_len,
            model.param_count,
            model.compute_dtype,
        )
        != ("decoder", 1, 8, 2, 2, 8, 16, 2, 728, "fp32")
        or (inner.H, inner.J, inner.micro_batch, inner.grad_accum, inner.opt, inner.state_policy)
        != (2, 1, 1, 1, "adamw", "carry")
        or (layout.pp, layout.n_gpus, layout.dp_size, layout.ep_size, layout.zero1)
        != (1, 1, 1, 1, False)
        or manifest.training.outer.opt != "nesterov"
    ):
        raise ValueError("capacity outer geometry is unqualified")


class OuterInput(WireModel):
    roster: RosterEntryV2
    commit: CommitV2
    delta_manifest: DeltaManifestV2
    replay: ReplayEvidence
    funding: FundedStatus
    settlement: SettlementStatus
    assignment_hash: Hex64
    clean_finalizations: Annotated[int, Field(strict=True, ge=0)]

    def work(self) -> VerifiedInput:
        return VerifiedInput(**{name: getattr(self, name) for name in type(self).model_fields})


class OuterRequest(WireModel):
    manifest: RunManifestV2
    w: Annotated[int, Field(strict=True, ge=0)]
    original_roster: Annotated[list[RosterEntryV2], Field(min_length=4, max_length=32)]
    inputs: Annotated[list[OuterInput], Field(min_length=4, max_length=32)]
    prev_state: Hex64 | None
    predecessor_tape_hash: Hex64
    reference_reward_units: Annotated[int, Field(strict=True, ge=0)]
    objects: Annotated[list[Hex64], Field(max_length=100)]

    @classmethod
    def parse(cls, raw: bytes) -> OuterRequest:
        request = cls.model_validate(load_json(raw, max_bytes=1 << 20))
        _geometry(request.manifest)
        if canonicalize(request.body()) != raw:
            raise ValueError("noncanonical capacity request")
        original = {entry.hotkey: entry for entry in request.original_roster}
        if len(original) != len(request.original_roster) or any(
            original.get(work.roster.hotkey) != work.roster for work in request.inputs
        ):
            raise ValueError("capacity original roster differs")
        if len({work.roster.hotkey for work in request.inputs}) != len(request.inputs):
            raise ValueError("duplicate capacity input")
        if len(set(request.objects)) != len(request.objects):
            raise ValueError("duplicate capacity object")
        if request.w != 0 and request.prev_state is None:
            raise ValueError("capacity predecessor missing")
        return request


class OuterResult(WireModel):
    request_hash: Hex64
    tape_body: TapeBodyV2
    objects: Annotated[list[Hex64], Field(min_length=1, max_length=2)]
    replayed: Annotated[bool, Field(strict=True)]


@contextmanager
def _directory_fd(path: Path, *, root_fd: int | None = None, create: bool = False) -> Iterator[int]:
    """Walk directories without links and pin the resulting inode."""
    parts = path.parts if root_fd is not None else path.absolute().parts[1:]
    if any(part in ("..", "/") for part in parts):
        raise ValueError("capacity relative path escapes directory")
    fd = os.dup(root_fd) if root_fd is not None else os.open("/", os.O_RDONLY | os.O_DIRECTORY)
    try:
        for part in parts:
            if part == ".":
                continue
            if create:
                try:
                    os.mkdir(part, mode=0o700, dir_fd=fd)
                except FileExistsError:
                    pass
            child = os.open(part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=fd)
            os.close(fd)
            fd = child
        yield fd
    except OSError as exc:
        raise ValueError("capacity directory confinement failed") from exc
    finally:
        os.close(fd)


def _read_at(root_fd: int, relative: Path, limit: int) -> bytes:
    with _directory_fd(relative.parent, root_fd=root_fd) as parent:
        try:
            fd = os.open(relative.name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=parent)
        except OSError as exc:
            raise ValueError("capacity object confinement failed") from exc
        with os.fdopen(fd, "rb") as stream:
            if not stat.S_ISREG(os.fstat(stream.fileno()).st_mode):
                raise ValueError("capacity object is not a regular file")
            raw = stream.read(limit + 1)
    if len(raw) > limit:
        raise ValueError("capacity bytes exceed limit")
    return raw


def _write_at(root_fd: int, relative: Path, data: bytes) -> None:
    """Exclusive descriptor-relative create; an existing hash object must match."""
    with _directory_fd(relative.parent, root_fd=root_fd, create=True) as parent:
        try:
            fd = os.open(
                relative.name,
                os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW,
                0o600,
                dir_fd=parent,
            )
        except FileExistsError:
            if _read_at(parent, Path(relative.name), len(data)) != data:
                raise ValueError("capacity existing output differs") from None
            return
        with os.fdopen(fd, "wb") as stream:
            stream.write(data)
            stream.flush()
            os.fsync(stream.fileno())
        os.fsync(parent)


def bounded_bytes(path: Path, limit: int) -> bytes:
    with _directory_fd(path.parent) as parent:
        return _read_at(parent, Path(path.name), limit)


def collect_result(directory: Path, request: OuterRequest) -> tuple[TapeBodyV2, dict[str, bytes]]:
    """Parent consumes bounded metadata/bytes only; no tensor parser or arithmetic."""
    with _directory_fd(directory) as root_fd:
        return _collect_result(root_fd, request)


def _collect_result(root_fd: int, request: OuterRequest) -> tuple[TapeBodyV2, dict[str, bytes]]:
    raw = _read_at(root_fd, Path("result.json"), 1 << 20)
    result = OuterResult.model_validate(load_json(raw, max_bytes=1 << 20))
    if (
        canonicalize(result.body()) != raw
        or not result.replayed
        or result.request_hash != sha256_hex(canonicalize(request.body()))
    ):
        raise ValueError("capacity result binding differs")
    body = result.tape_body
    manifest = request.manifest
    expected = sorted(request.inputs, key=lambda x: x.roster.hotkey.encode())
    from hypertrain.aggregator.tape_v2 import _input

    records = [_input(work.work(), manifest, request.w) for work in expected]
    from hypertrain.aggregator.weighted_v2 import Candidate, allocate_weights

    policy_hash = manifest.network.aggregation_policy_hash
    policy_raw = _read_at(root_fd, Path("objects") / policy_hash[:2] / policy_hash, 65536)
    if sha256_hex(policy_raw) != manifest.network.aggregation_policy_hash:
        raise ValueError("capacity policy hash differs")
    policy = AggregationPolicyV2.model_validate_json(policy_raw)
    allocation = allocate_weights(
        [Candidate(r.roster, r.commit_hash, r.delta_manifest_hash) for r in records], policy
    )
    if (
        body.run_id != manifest.run_id()
        or body.w != request.w
        or body.inputs != records
        or body.excluded
        or body.input_root != sha256_hex(canonicalize([r.body() for r in records]))
        or body.predecessor_tape_hash != request.predecessor_tape_hash
        or request.prev_state is not None
        and body.prev_state != request.prev_state
        or body.policy_hash != manifest.network.aggregation_policy_hash
        or body.allocation != allocation
        or body.policy_hashes.body()
        != {k: getattr(manifest.network, k) for k in body.policy_hashes.body()}
        or set(result.objects)
        != ({body.out_state, body.prev_state} if request.prev_state is None else {body.out_state})
    ):
        raise ValueError("capacity output authority differs")
    objects = {}
    for key in result.objects:
        data = _read_at(root_fd, Path("objects") / key[:2] / key, 65536)
        if sha256_hex(data) != key:
            raise ValueError("capacity output hash differs")
        objects[key] = data
    return body, objects


def execute(directory: Path) -> None:
    """Child owns init/load/decode, original-object verification and two arithmetic passes."""
    with _directory_fd(directory) as root_fd:
        _execute(root_fd)


def _execute(root_fd: int) -> None:
    import hypertrain.trainer  # noqa: F401
    from hypertrain.aggregator.core import OuterState, load_state
    from hypertrain.trainer.compress import validate_payload
    from hypertrain.trainer.config import TrainConfig
    from hypertrain.trainer.model import init_params, param_shapes

    raw = _read_at(root_fd, Path("request.json"), 1 << 20)
    parsed = load_json(raw, max_bytes=1 << 20)
    if isinstance(parsed, dict) and parsed.get("operation") == "genesis":
        request_genesis = GenesisRequest.model_validate(parsed)
        if canonicalize(request_genesis.body()) != raw:
            raise ValueError("noncanonical genesis request")
        _genesis(root_fd, request_genesis)
        return
    request = OuterRequest.parse(raw)
    config = TrainConfig.from_manifest_v2(request.manifest)
    shapes = param_shapes(config.model)
    deltas = {x.commit.delta_hash for x in request.inputs}

    class Objects(LocalFSStore):
        def __init__(self) -> None:
            pass

        def get(self, key: str) -> bytes:
            from hypertrain.data.store import _check_key

            _check_key(key)
            data = _read_at(root_fd, Path("objects") / key[:2] / key, 65536)
            if sha256_hex(data) != key:
                raise ValueError("capacity original object hash differs")
            if (
                key in deltas
                and validate_payload(data, shapes, max_payload_bytes=65536) != config.compress.codec
            ):
                raise ValueError("capacity delta codec differs")
            return data

        def put(self, data: bytes) -> str:
            if len(data) > 65536:
                raise ValueError("capacity output bytes exceed limit")
            key = sha256_hex(data)
            _write_at(root_fd, Path("objects") / key[:2] / key, data)
            return key

    store = Objects()
    for key in request.objects:
        store.get(key)
    manifest = request.manifest
    policy = AggregationPolicyV2.model_validate_json(
        store.get(manifest.network.aggregation_policy_hash)
    )
    economics = store.get(manifest.network.economics_policy_hash)
    previous = request.prev_state
    output_objects = []
    if previous is None:
        theta = init_params(config.model)
        previous = store.put(
            OuterState.init({k: v.detach().cpu().numpy() for k, v in theta.items()}).to_bytes()
        )
        output_objects.append(previous)
    previous_state = load_state(store, previous)
    if {name: tuple(value.shape) for name, value in previous_state.theta.items()} != shapes:
        raise ValueError("capacity predecessor geometry differs from manifest")
    works = [x.work() for x in request.inputs]
    body, state = compute_body(
        store,
        manifest,
        policy,
        economics,
        w=request.w,
        prev_state=previous,
        predecessor_tape_hash=request.predecessor_tape_hash,
        inputs=works,
        reference_reward_units=request.reference_reward_units,
    )
    state_bytes = state.to_bytes()
    if store.put(state_bytes) != body.out_state:
        raise ValueError("capacity output object differs")
    replayed_body, replayed_state = compute_body(
        store,
        manifest,
        policy,
        economics,
        w=request.w,
        prev_state=previous,
        predecessor_tape_hash=request.predecessor_tape_hash,
        inputs=works,
        reference_reward_units=request.reference_reward_units,
    )
    if replayed_body != body or replayed_state.to_bytes() != state_bytes:
        raise ValueError("capacity independent replay differs")
    output_objects.append(body.out_state)
    result = OuterResult(
        request_hash=sha256_hex(raw),
        tape_body=body,
        objects=list(dict.fromkeys(output_objects)),
        replayed=True,
    )
    _write_at(root_fd, Path("result.json"), canonicalize(result.body()))
    print("CAPACITY_OUTER_REPLAY_COMPLETE", flush=True)


if __name__ == "__main__":
    if len(sys.argv) != 2:
        raise SystemExit("capacity worker requires one private attempt directory")
    execute(Path(sys.argv[1]))
