"""Coordinator repair evidence, never a replacement miner commitment or reward."""

from __future__ import annotations

import json
import os
import shutil
import struct
import subprocess
import sys
import tempfile
from collections.abc import Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import Annotated, Any, Literal

import hypertrain.trainer  # noqa: F401 - determinism before torch in fresh repair worker

# isort: split
import torch
from pydantic import BaseModel, ConfigDict, Field

from hypertrain.aggregator.core import OuterState, TapeError, get_object, load_delta, load_state
from hypertrain.aggregator.tape_v2 import (
    Exclusion,
    TapeBodyV2,
    TapeV2,
    VerifiedInput,
    _aggregate_body,
    _input,
    _verify_original,
)
from hypertrain.auditor.replay import (
    AnchorCache,
    VerifiedAnchor,
    optimizer_hash,
    pack_state,
)
from hypertrain.data.store import Store
from hypertrain.protocol.envelope_v2 import load_json, parse_envelope, tape_signing_message
from hypertrain.protocol.envelope_v2 import verify_envelope as verify_source
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import Keypair, decode_hotkey, verify
from hypertrain.protocol.messages import SS58, Hex64
from hypertrain.protocol.messages_v2 import (
    U64,
    AggregationPolicyV2,
    EconomicsPolicyV2,
    RunManifestV2,
    WireModel,
)
from hypertrain.trainer.compress import state_hash
from hypertrain.trainer.config import TrainConfig
from hypertrain.trainer.island import Comm, IslandAssignment, train_island
from hypertrain.trainer.loop import SampleFn
from hypertrain.trainer.optim import OptState


def execute_repair_context(
    store: Any,
    manifest: RunManifestV2,
    context: dict[str, Any],
    signer: str,
    *,
    key: Keypair | None = None,
    tape: RepairTape | None = None,
) -> RepairResult:
    """Fresh qualified torchrun computes/replays a coordinator-authenticated repair context."""
    from hypertrain.protocol.jcs import canonicalize

    raw = canonicalize(context)
    message = b"hypertrain/rollback/context/1|" + sha256_hex(canonicalize(context["body"])).encode()
    if context["signer"] != signer or not verify(
        decode_hotkey(signer), message, bytes.fromhex(context["sig"])
    ):
        raise TapeError("repair context authority mismatch")
    body = context["body"]
    if body["run_id"] != manifest.run_id() or body["backend"] not in ("cpu", "cuda"):
        raise TapeError("repair context runtime/run mismatch")
    if body["qualification_hash"] != sha256_hex(canonicalize(body["qualification"])):
        raise TapeError("repair context qualification object differs")
    _qualification_body(manifest, body)
    if body["manifest"] != manifest.body():
        raise TapeError("repair context embedded manifest differs")
    if body["w"] > 0 and body.get("history"):
        last = body["history"][-1]
        prior_raw = get_object(store, last["tape_hash"])
        prior = (
            RepairTape.from_bytes(prior_raw).body.arithmetic
            if "repair_context" in last
            else TapeV2.from_bytes(prior_raw).body
        )
        if len(body["history"]) != body["w"] or (
            prior.w != body["w"] - 1
            or prior.out_state != body["prev_state"]
            or last["tape_hash"] != body["predecessor_tape_hash"]
        ):
            raise TapeError("repair authenticated predecessor history endpoint differs")
    if body["timeout_seconds"] < 1 or body["timeout_seconds"] > 600:
        raise TapeError("repair context timeout budget differs")
    from hypertrain.protocol.messages_v2 import DisputePolicyV2

    dispute_policy = DisputePolicyV2.model_validate_json(
        get_object(store, manifest.network.dispute_policy_hash)
    )
    proof_ids = set()
    for proof in body["exclusion_proofs"]:
        resolution = proof["resolution"]
        evidence = proof["referee_evidence"]
        if (
            not verify_source(resolution)
            or resolution["type"] != "ResolutionV2"
            or resolution["run_id"] != manifest.run_id()
            or resolution["signer"] not in dispute_policy.referees
            or resolution["body"]["reason"] not in ("FRAUD", "WITHHELD", "PARTY_TIMEOUT")
            or resolution["body"]["evidence_hash"] != sha256_hex(canonicalize(evidence))
            or resolution["body"]["loser"] != evidence["loser"]
            or resolution["body"]["reason"] != evidence["reason"]
            or resolution["body"]["dispute_id"] != evidence["dispute_id"]
            or resolution["body"]["transcript_hash"] != evidence["transcript_hash"]
        ):
            raise TapeError("repair exclusion independent authority differs")
        proof_ids.add((resolution["body"]["loser"], sha256_hex(canonicalize(resolution["body"]))))
    if not {(e["hotkey"], e["evidence_hash"]) for e in body["excluded"]} <= proof_ids:
        raise TapeError("repair exclusions lack exact independent authority")
    for source in body["sources"]:
        for receipt, envelope_hash in zip(
            source["receipts"],
            (
                source["commit_envelope_hash"],
                source["delta_envelope_hash"],
            ),
            strict=True,
        ):
            original = parse_envelope(get_object(store, envelope_hash))
            if (
                not verify_source(receipt)
                or receipt["signer"] != signer
                or (
                    receipt["type"] != "Receipt"
                    or receipt["run_id"] != manifest.run_id()
                    or receipt["body"]["w"] != body["w"]
                    or receipt["body"]["commit_hash"] != sha256_hex(canonicalize(original.body))
                )
            ):
                raise TapeError("repair historical intake receipt differs")
        if source["acceptance_hash"] != sha256_hex(canonicalize(source["receipts"])):
            raise TapeError("repair acceptance receipt hash differs")
    if tape is not None and (
        tape.signer != signer
        or not verify(decode_hotkey(signer), repair_message(tape.body), bytes.fromhex(tape.sig))
    ):
        raise TapeError("repair candidate signature mismatch")
    context_hash = store.put(raw)
    with tempfile.TemporaryDirectory(prefix="hypertrain-repair-") as tmp:
        env = os.environ.copy()
        env["CUBLAS_WORKSPACE_CONFIG"] = ":4096:8"
        env["OMP_NUM_THREADS"] = "1"
        argv = [
            sys.executable,
            "-m",
            "torch.distributed.run",
            "--standalone",
            f"--nproc-per-node={manifest.training.reference_spec.layout.n_gpus}",
            "--max-restarts=0",
            "-m",
            "hypertrain.aggregator.rollback_v2",
            str(store.root),
            context_hash,
            tmp,
            signer,
        ]
        try:
            subprocess.run(  # noqa: S603 - interpreter-owned torchrun, signed context only
                argv,
                env=env,
                check=True,
                capture_output=True,
                timeout=body["timeout_seconds"],
            )
        except subprocess.CalledProcessError as error:
            raise TapeError(
                "qualified repair worker failed: " + error.stderr.decode()[-6000:]
            ) from error
        computed = RepairBody.model_validate_json((Path(tmp) / "body.json").read_bytes())
    state = load_state(store, computed.arithmetic.out_state)
    if tape is not None:
        if computed != tape.body:
            raise TapeError("fresh independent repair context replay differs")
    else:
        if key is None or key.ss58 != signer:
            raise TapeError("repair context signing authority unavailable")
        tape = RepairTape(
            body=computed, signer=signer, sig=key.sign(repair_message(computed)).hex()
        )
    return RepairResult(tape, state, context_anchors(store, computed))


def _qualification_body(manifest: RunManifestV2, body: dict[str, Any]) -> None:
    selection = body["qualification"]["execution_backend"]
    if selection["backend"] != body["backend"]:
        raise TapeError("repair frozen backend selection differs")
    if body["backend"] == "cpu":
        if selection["authority_hash"] is not None:
            raise TapeError("CPU repair cannot substitute CUDA qualification")
    else:
        reviewed = body["qualification"]["reviewed_qualification"]
        ref = manifest.training.reference_spec
        if sha256_hex(canonicalize(reviewed)) != selection["authority_hash"] or (
            reviewed["run_id"] != manifest.run_id()
            or reviewed["backend"] != "cuda"
            or reviewed["reference_hash"] != body["reference_hash"]
            or reviewed["layout_hash"] != ref.layout.model_dump_json()
        ):
            raise TapeError("repair exact reviewed CUDA qualification differs")


def context_anchors(store: Store, body: RepairBody) -> tuple[VerifiedAnchor, ...]:
    from hypertrain.auditor.replay import unpack_state

    anchors = []
    for e in body.recomputed:
        theta, state = unpack_state(get_object(store, e.final_state_object))
        ef, _ = unpack_state(get_object(store, e.final_ef_object))
        if state is None:
            raise TapeError("repair output optimizer absent")
        if any(
            not bool(torch.isfinite(x).all())
            for values in (theta, state.m, state.v, ef)
            for x in values.values()
        ):
            raise TapeError("nonfinite repair output artifact")
        if (
            optimizer_hash(state) != e.final_optimizer_hash
            or state_hash(theta) != e.final_theta_hash
            or state_hash(ef) != e.ef_out_hash
        ):
            raise TapeError("repair output artifact commitments differ")
        proof = sha256_hex(
            canonicalize(
                [
                    "ht-rollback-anchor/1",
                    body.arithmetic.run_id,
                    body.arithmetic.w,
                    body.arithmetic.predecessor_tape_hash,
                    body.arithmetic.prev_state,
                    body.source_tape_hash,
                    body.qualification_hash,
                    body.reference_hash,
                    body.layout_hash,
                    body.backend,
                    e.body(),
                ]
            )
        )
        anchors.append(
            VerifiedAnchor(
                body.arithmetic.run_id,
                e.hotkey,
                body.arithmetic.w,
                body.layout_hash,
                state,
                ef,
                proof,
                sha256_hex(b"hypertrain/rollback/anchor/1|" + bytes.fromhex(proof)),
                theta,
                body.backend,
            )
        )
    return tuple(anchors)


def replay_history_context(
    store: Any,
    manifest: RunManifestV2,
    context: dict[str, Any],
    signer: str,
) -> tuple[VerifiedAnchor, ...]:
    """Authenticated ordinary/repair history, independently carried from deterministic genesis."""
    message = b"hypertrain/rollback/context/1|" + sha256_hex(canonicalize(context["body"])).encode()
    if (
        context["signer"] != signer
        or not verify(decode_hotkey(signer), message, bytes.fromhex(context["sig"]))
        or context["body"]["manifest"] != manifest.body()
    ):
        raise TapeError("predecessor history authority differs")
    context_hash = store.put(canonicalize(context))
    with tempfile.TemporaryDirectory(prefix="hypertrain-history-") as tmp:
        env = os.environ.copy()
        env["CUBLAS_WORKSPACE_CONFIG"] = ":4096:8"
        env["OMP_NUM_THREADS"] = "1"
        process = subprocess.run(  # noqa: S603 - fixed interpreter, authenticated context
            [
                sys.executable,
                "-m",
                "torch.distributed.run",
                "--standalone",
                f"--nproc-per-node={manifest.training.reference_spec.layout.n_gpus}",
                "--max-restarts=0",
                "-m",
                "hypertrain.aggregator.rollback_v2",
                str(store.root),
                context_hash,
                tmp,
                signer,
                "history",
            ],
            env=env,
            capture_output=True,
            timeout=context["body"]["timeout_seconds"],
        )
        if process.returncode:
            raise TapeError("predecessor history replay failed: " + process.stderr.decode()[-6000:])
        records = json.loads((Path(tmp) / "anchors.json").read_bytes())
    from hypertrain.auditor.replay import unpack_state

    anchors = []
    for record in records:
        theta, state = unpack_state(get_object(store, record["state_object"]))
        ef, _ = unpack_state(get_object(store, record["ef_object"]))
        if state is None:
            raise TapeError("predecessor history optimizer absent")
        anchors.append(
            VerifiedAnchor(
                manifest.run_id(),
                record["hotkey"],
                record["w"],
                record["layout_hash"],
                state,
                ef,
                record["proof_hash"],
                record["anchor_hash"],
                theta,
                record["backend"],
            )
        )
    return tuple(anchors)


def _replay_history(
    store: Store,
    manifest: RunManifestV2,
    entries: Sequence[dict[str, Any]],
    signer: str,
    comm: Comm,
    get_sample: SampleFn,
    backend: Literal["cpu", "cuda"],
) -> tuple[VerifiedAnchor, ...]:
    """No hash assertion can substitute for actual inner work and outer predecessor replay."""
    from hypertrain.aggregator.checkpoint import network_inputs
    from hypertrain.aggregator.tape_v2 import replay_tape
    from hypertrain.auditor.replay import audit_island, unpack_state
    from hypertrain.protocol.messages_v2 import AuditJobV2, RoundOpenV2
    from hypertrain.trainer.model import init_params

    if not entries or len(entries) > 16:
        raise TapeError("bounded complete predecessor history required")
    cache = AnchorCache()
    previous = None
    previous_hash = "0" * 64
    policy = AggregationPolicyV2.model_validate_json(
        get_object(store, manifest.network.aggregation_policy_hash)
    )
    economics = get_object(store, manifest.network.economics_policy_hash)
    for w, item in enumerate(entries):
        tape_hash = item["tape_hash"]
        raw = get_object(store, tape_hash)
        if "repair_context" in item:
            candidate = RepairTape.from_bytes(raw)
            c = item["repair_context"]
            message = (
                b"hypertrain/rollback/context/1|" + sha256_hex(canonicalize(c["body"])).encode()
            )
            if c["signer"] != signer or not verify(
                decode_hotkey(signer), message, bytes.fromhex(c["sig"])
            ):
                raise TapeError("predecessor repair context authority differs")
            if candidate.signer != signer or not verify(
                decode_hotkey(signer), repair_message(candidate.body), bytes.fromhex(candidate.sig)
            ):
                raise TapeError("predecessor repair signature differs")
            b = c["body"]
            sources = []
            for record in b["sources"]:
                a = record["anchor"]
                if w == 0:
                    anchor = cache.genesis(
                        manifest,
                        a["hotkey"],
                        init_params(TrainConfig.from_manifest_v2(manifest).model),
                    )
                else:
                    anchor = cache.entries[
                        (manifest.run_id(), a["hotkey"], w - 1, AnchorCache.layout_hash(manifest))
                    ]
                if (a["anchor_hash"], a["proof_hash"], a["state_object"], a["ef_object"]) != (
                    anchor.anchor_hash,
                    anchor.proof_hash,
                    sha256_hex(pack_state(anchor.theta, anchor.state)),
                    sha256_hex(pack_state(anchor.ef)),
                ):
                    raise TapeError("predecessor repair carry differs from replayed history")
                sources.append(
                    RepairSource(
                        network_inputs([record["work"]])[0],
                        get_object(store, record["commit_envelope_hash"]),
                        get_object(store, record["delta_envelope_hash"]),
                        record["acceptance_hash"],
                        tuple(record["samples"]),
                        anchor,
                    )
                )
            body, state, anchors = _compute_repair(
                store,
                manifest,
                policy,
                economics,
                RepairAuthority(
                    signer,
                    b["source_tape_hash"],
                    b["qualification_hash"],
                    b["reference_hash"],
                    b["layout_hash"],
                    backend,
                ),
                comm,
                get_sample,
                w=w,
                prev_state=b["prev_state"],
                predecessor_tape_hash=previous_hash,
                sources=sources,
                excluded=[Exclusion.model_validate(e) for e in b["excluded"]],
                reference_reward_units=b["reference_reward_units"],
            )
            if candidate.body != body:
                raise TapeError("predecessor repair replay differs")
            for anchor in anchors:
                cache.entries[(manifest.run_id(), anchor.hotkey, w, anchor.layout_hash)] = anchor
            arithmetic = body.arithmetic
        else:
            tape = TapeV2.from_bytes(raw)
            arithmetic = tape.body
            opening = parse_envelope(item["round_open"])
            if (
                not verify_source(item["round_open"])
                or opening.signer != signer
                or opening.type != "RoundOpenV2"
                or opening.run_id != manifest.run_id()
                or opening.body["w"] != w
            ):
                raise TapeError("predecessor original round authority differs")
            inputs = network_inputs(item["inputs"])
            opening_model = RoundOpenV2.model_validate(opening.body)
            if sorted((x.roster for x in inputs), key=lambda r: r.hotkey) != sorted(
                opening_model.roster, key=lambda r: r.hotkey
            ):
                raise TapeError("predecessor original full roster differs")
            replay_tape(
                store,
                tape,
                manifest,
                policy,
                economics,
                signer=signer,
                w=w,
                prev_state=arithmetic.prev_state,
                predecessor_tape_hash=previous_hash,
                inputs=inputs,
                reference_reward_units=item["reference_reward_units"],
            )
            jobs = {
                job["start_state"]["hotkey"]: AuditJobV2.model_validate(job)
                for job in item["audit_jobs"]
            }
            if set(jobs) != {x.roster.hotkey for x in inputs}:
                raise TapeError("predecessor audit history incomplete")
            for work in inputs:
                job = jobs[work.roster.hotkey]
                if w == 0:
                    cache.genesis(
                        manifest,
                        work.roster.hotkey,
                        init_params(TrainConfig.from_manifest_v2(manifest).model),
                    )
                if (
                    job.manifest != manifest
                    or job.start_state.w != w
                    or job.commit_envelope["body"] != work.commit.model_dump(mode="json")
                    or parse_envelope(job.challenge_envelope).signer != signer
                ):
                    raise TapeError("predecessor audit authority/commit differs")
                theta, supplied = unpack_state(
                    get_object(store, job.start_state.state_object_sha256)
                )
                prior_state = load_state(store, arithmetic.prev_state)
                if any(
                    not torch.equal(theta[name].detach().cpu(), torch.from_numpy(value.copy()))
                    for name, value in prior_state.theta.items()
                ) or set(theta) != set(prior_state.theta):
                    raise TapeError("predecessor exact original global theta bytes differ")
                if (
                    supplied is None
                    or state_hash(theta) != arithmetic.prev_hashes["theta_hash"]
                    or job.start_state.theta_hash != opening.body["theta_hash"]
                ):
                    raise TapeError("predecessor original global theta differs")
                _finite_state(
                    comm, theta, supplied, unpack_state(get_object(store, job.ef_in.sha256))[0]
                )
                outcome, result = audit_island(
                    job,
                    comm,
                    get_sample,
                    get_object(store, job.start_state.state_object_sha256),
                    get_object(store, job.ef_in.sha256),
                    get_object(store, job.v0.sha256),
                    cache,
                    device=backend,
                    now_round=job.created_beacon,
                )
                _finite_state(comm, result.final_theta, result.final_state, result.ef_out)
                if outcome.result != "MATCH":
                    raise TapeError("predecessor actual carried audit differs")
        if (
            arithmetic.w != w
            or arithmetic.predecessor_tape_hash != previous_hash
            or (previous is not None and arithmetic.prev_state != previous)
        ):
            raise TapeError("predecessor history outer lineage differs")
        if w == 0:
            genesis = init_params(TrainConfig.from_manifest_v2(manifest).model)
            initial = OuterState.init({n: x.detach().cpu().numpy() for n, x in genesis.items()})
            if (
                state_hash(genesis) != arithmetic.prev_hashes["theta_hash"]
                or get_object(store, arithmetic.prev_state) != initial.to_bytes()
            ):
                raise TapeError("predecessor history global genesis differs")
        previous, previous_hash = arithmetic.out_state, tape_hash
    return tuple(a for a in cache.entries.values() if a.w == len(entries) - 1)


def _context_worker(
    store_path: str, context_hash: str, output: str, signer: str, mode: str = "repair"
) -> None:
    from datetime import timedelta

    import numpy as np
    import torch.distributed as dist

    from hypertrain.aggregator.checkpoint import network_inputs
    from hypertrain.auditor.replay import unpack_state
    from hypertrain.data.store import LocalFSStore
    from hypertrain.protocol.hashing import MerkleTree
    from hypertrain.trainer.determinism import apply_reference_env
    from hypertrain.trainer.island import DistComm

    store = LocalFSStore(Path(store_path))
    context: dict[str, Any] = json.loads(get_object(store, context_hash))
    message = b"hypertrain/rollback/context/1|" + sha256_hex(canonicalize(context["body"])).encode()
    if context["signer"] != signer or not verify(
        decode_hotkey(signer), message, bytes.fromhex(context["sig"])
    ):
        raise TapeError("worker repair context authority mismatch")
    body = context["body"]
    manifest = RunManifestV2.model_validate(body["manifest"])
    _qualification_body(manifest, body)
    backend = body["backend"]
    ref = manifest.training.reference_spec
    if backend == "cuda":
        rank = int(os.environ["LOCAL_RANK"])
        torch.cuda.set_device(rank)
        torch.set_default_device(torch.device("cuda", rank))
        if os.environ.get("HT_IMAGE_DIGEST") != ref.image_digest:
            raise TapeError("repair runtime image mismatch")
        if torch.__version__ != "2.14.0+cu130":
            raise TapeError("repair unqualified CUDA runtime")
        smi = shutil.which("nvidia-smi")
        if smi is None:
            raise TapeError("repair GPU driver inspector absent")
        drivers = subprocess.run(  # noqa: S603 - PATH-resolved fixed GPU inspection command
            [smi, "--query-gpu=driver_version", "--format=csv,noheader"],
            check=True,
            capture_output=True,
            text=True,
            timeout=10,
        ).stdout.splitlines()
        if len(drivers) != ref.layout.n_gpus or drivers[rank].strip() not in ref.driver_allowlist:
            raise TapeError("repair unqualified CUDA driver")
        if torch.cuda.get_device_properties(rank).multi_processor_count != ref.sm_count:
            raise TapeError("repair unqualified CUDA SM profile")
    apply_reference_env(ref.env.model_dump())
    dist.init_process_group("gloo" if backend == "cpu" else "nccl", timeout=timedelta(minutes=5))
    try:
        sources = []
        for item in body["sources"]:
            a = item["anchor"]
            theta, state = unpack_state(get_object(store, a["state_object"]))
            ef, _ = unpack_state(get_object(store, a["ef_object"]))
            if state is None:
                raise TapeError("repair source optimizer absent")
            anchor = VerifiedAnchor(
                manifest.run_id(),
                a["hotkey"],
                a["w"],
                a["layout_hash"],
                state,
                ef,
                a["proof_hash"],
                a["anchor_hash"],
                theta,
                a["backend"],
            )
            if a["w"] == -1:
                from hypertrain.trainer.model import init_params

                expected = AnchorCache().genesis(
                    manifest,
                    a["hotkey"],
                    init_params(TrainConfig.from_manifest_v2(manifest).model),
                )
                if (
                    anchor.anchor_hash != expected.anchor_hash
                    or anchor.proof_hash != expected.proof_hash
                    or pack_state(anchor.theta, anchor.state)
                    != pack_state(expected.theta, expected.state)
                    or pack_state(anchor.ef) != pack_state(expected.ef)
                ):
                    raise TapeError("repair independently reconstructed genesis differs")
            sources.append(
                RepairSource(
                    network_inputs([item["work"]])[0],
                    get_object(store, item["commit_envelope_hash"]),
                    get_object(store, item["delta_envelope_hash"]),
                    item["acceptance_hash"],
                    tuple(item["samples"]),
                    anchor,
                )
            )
        dataset = manifest.training.dataset
        dtype = "<u2" if dataset.sample_format.startswith("u16") else "<u4"
        rows = np.frombuffer(get_object(store, body["samples_hash"]), dtype=dtype).reshape(
            dataset.n_samples,
            manifest.training.model.seq_len + 1,
        )
        proofs = json.loads(get_object(store, body["proofs_hash"]))

        def get_sample(i: int) -> np.ndarray:
            row = rows[i]
            if not MerkleTree.verify(
                row.tobytes(),
                i,
                [bytes.fromhex(p) for p in proofs[i]],
                bytes.fromhex(dataset.merkle_root),
                dataset.n_samples,
            ):
                raise TapeError("repair dataset proof differs")
            return row.astype(np.uint32)

        if mode == "history":
            anchors = _replay_history(
                store, manifest, body["history"], signer, DistComm(), get_sample, backend
            )
            records = [
                {
                    "hotkey": a.hotkey,
                    "w": a.w,
                    "layout_hash": a.layout_hash,
                    "proof_hash": a.proof_hash,
                    "anchor_hash": a.anchor_hash,
                    "backend": a.backend,
                    "state_object": store.put(pack_state(a.theta, a.state)),
                    "ef_object": store.put(pack_state(a.ef)),
                }
                for a in anchors
            ]
            if dist.get_rank() == 0:
                (Path(output) / "anchors.json").write_bytes(canonicalize(records))
            return

        if body["w"] > 0 and body.get("history"):
            verified = {
                a.hotkey: a
                for a in _replay_history(
                    store,
                    manifest,
                    body["history"],
                    signer,
                    DistComm(),
                    get_sample,
                    backend,
                )
            }
            for source in sources:
                expected = verified[source.work.roster.hotkey]
                if (
                    source.anchor.anchor_hash,
                    source.anchor.proof_hash,
                    pack_state(source.anchor.theta, source.anchor.state),
                    pack_state(source.anchor.ef),
                ) != (
                    expected.anchor_hash,
                    expected.proof_hash,
                    pack_state(expected.theta, expected.state),
                    pack_state(expected.ef),
                ):
                    raise TapeError(
                        "first repair carry differs from authenticated predecessor history"
                    )

        authority = RepairAuthority(
            signer,
            body["source_tape_hash"],
            body["qualification_hash"],
            body["reference_hash"],
            body["layout_hash"],
            backend,
        )
        computed, outer_state, _ = _compute_repair(
            store,
            manifest,
            AggregationPolicyV2.model_validate_json(
                get_object(store, manifest.network.aggregation_policy_hash)
            ),
            get_object(store, manifest.network.economics_policy_hash),
            authority,
            DistComm(),
            get_sample,
            w=body["w"],
            prev_state=body["prev_state"],
            predecessor_tape_hash=body["predecessor_tape_hash"],
            sources=sources,
            excluded=[Exclusion.model_validate(e) for e in body["excluded"]],
            reference_reward_units=body["reference_reward_units"],
        )
        store.put(outer_state.to_bytes())
        if dist.get_rank() == 0:
            (Path(output) / "body.json").write_bytes(canonicalize(computed.body()))
    finally:
        dist.destroy_process_group()


@dataclass(frozen=True, slots=True)
class RepairSource:
    """L0 accepted journal facts; receipt hash must come from durable intake, not a request."""

    work: VerifiedInput
    commit_envelope: bytes
    delta_envelope: bytes
    acceptance_hash: str
    sample_ids: tuple[int, ...]
    anchor: VerifiedAnchor


@dataclass(frozen=True, slots=True)
class RepairAuthority:
    """Independent L0 pins, resolved from run authority and qualified runtime registry."""

    coordinator: str
    source_tape_hash: str
    qualification_hash: str
    reference_hash: str
    layout_hash: str
    backend: Literal["cpu", "cuda"]


class RecomputedWork(WireModel):
    hotkey: SS58
    source_commit_envelope: Hex64
    source_delta_envelope: Hex64
    acceptance_hash: Hex64
    source_anchor_hash: Hex64
    source_anchor_proof: Hex64
    start_theta_hash: Hex64
    start_optimizer_hash: Hex64
    start_ef_hash: Hex64
    global_step0: U64
    sample_ids: Annotated[list[U64], Field(min_length=1, max_length=1_000_000)]
    delta_hash: Hex64
    leaves_root: Hex64
    final_theta_hash: Hex64
    final_state_object: Hex64
    final_ef_object: Hex64
    final_optimizer_hash: Hex64
    ef_out_hash: Hex64


class RepairBody(WireModel):
    v: Literal["ht-rollback-replay/1"]
    purpose: Literal["model-repair-no-reward"]
    source_tape_hash: Hex64
    qualification_hash: Hex64
    reference_hash: Hex64
    layout_hash: Hex64
    backend: Literal["cpu", "cuda"]
    arithmetic: TapeBodyV2
    recomputed: Annotated[list[RecomputedWork], Field(min_length=4, max_length=1024)]


def repair_message(body: RepairBody) -> bytes:
    return (
        b"hypertrain/rollback/replay/1|"
        + body.digest().encode()
        + b"|"
        + body.arithmetic.run_id.encode()
    )


class RepairTape(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True)

    body: RepairBody
    signer: SS58
    sig: Annotated[str, Field(pattern=r"^[0-9a-f]{128}$")]

    def to_bytes(self) -> bytes:
        return canonicalize(self.model_dump(mode="json"), allow_float=False)

    @classmethod
    def from_bytes(cls, raw: bytes) -> RepairTape:
        tape = cls.model_validate(load_json(raw, max_bytes=1 << 20))
        if tape.to_bytes() != raw:
            raise TapeError("noncanonical repair tape")
        return tape


@dataclass(frozen=True, slots=True)
class RepairResult:
    tape: RepairTape
    state: OuterState
    anchors: tuple[VerifiedAnchor, ...]


def _finite_state(
    comm: Comm,
    theta: dict[str, torch.Tensor],
    state: OptState,
    ef: dict[str, torch.Tensor],
) -> None:
    """Every exact-layout rank rejects invalid decoded tensors before publication."""
    finite = all(
        bool(torch.isfinite(x).all())
        for values in (theta, state.m, state.v, ef)
        for x in values.values()
    )
    flag = torch.tensor([int(finite)], dtype=torch.int64, device=next(iter(theta.values())).device)
    if any(int(x[0]) != 1 for x in comm.all_gather(flag)):
        raise TapeError("nonfinite repair theta/optimizer/EF")


def _compute_repair(
    store: Store,
    manifest: RunManifestV2,
    policy: AggregationPolicyV2,
    economics: bytes,
    authority: RepairAuthority,
    comm: Comm,
    get_sample: SampleFn,
    *,
    w: int,
    prev_state: str,
    predecessor_tape_hash: str,
    sources: Sequence[RepairSource],
    excluded: Sequence[Exclusion],
    reference_reward_units: int,
) -> tuple[RepairBody, OuterState, tuple[VerifiedAnchor, ...]]:
    """Every rank executes real H-step carried training; caller owns bounded reservation."""
    cfg = TrainConfig.from_manifest_v2(manifest)
    reference = manifest.training.reference_spec
    econ = EconomicsPolicyV2.model_validate(load_json(economics))
    if (
        policy.digest() != manifest.network.aggregation_policy_hash
        or econ.digest() != manifest.network.economics_policy_hash
        or authority.reference_hash != sha256_hex(canonicalize(reference.model_dump(mode="json")))
        or authority.layout_hash != AnchorCache.layout_hash(manifest)
        or cfg.inner.state_policy != "carry"
        or comm.world != reference.layout.n_gpus
        or (authority.backend == "cpu" and econ.ledger_mode != "test")
    ):
        raise TapeError("repair policy/layout/runtime not qualified")
    source_tape = TapeV2.from_bytes(get_object(store, authority.source_tape_hash))
    if (
        source_tape.body.run_id != manifest.run_id()
        or source_tape.body.w != w
        or source_tape.signer != authority.coordinator
        or not verify(
            decode_hotkey(authority.coordinator),
            tape_signing_message(source_tape.body.body(), manifest.run_id()),
            bytes.fromhex(source_tape.sig),
        )
    ):
        raise TapeError("source tape authority/run/round mismatch")
    ordered = sorted(sources, key=lambda s: s.work.roster.hotkey.encode())
    records = [_input(s.work, manifest, w) for s in ordered]
    exclusions = sorted(excluded, key=lambda e: e.hotkey.encode())
    ids = [r.roster.hotkey for r in records]
    omitted = [e.hotkey for e in exclusions]
    original_ids = {r.roster.hotkey for r in source_tape.body.inputs} | {
        e.hotkey for e in source_tape.body.excluded
    }
    if (
        len(set(ids)) != len(ids)
        or len(set(omitted)) != len(omitted)
        or set(ids) & set(omitted)
        or set(ids) | set(omitted) != original_ids
        or any(r not in source_tape.body.inputs for r in records)
        or any(e not in exclusions for e in source_tape.body.excluded)
    ):
        raise TapeError("repair source/exclusion set differs from accepted tape")
    previous = load_state(store, prev_state)
    theta = {n: torch.from_numpy(x.copy()).to(authority.backend) for n, x in previous.theta.items()}
    for source in ordered:
        anchor = source.anchor
        _finite_state(comm, theta, anchor.state, anchor.ef)
        _finite_state(comm, anchor.theta, anchor.state, anchor.ef)
    evidence = []
    anchors = []
    deltas = {}
    for source, record in zip(ordered, records, strict=True):
        work, anchor = source.work, source.anchor
        _verify_original(store, manifest, econ, work, record, reference_reward_units)
        for raw, kind, original in (
            (source.commit_envelope, "CommitV2", work.commit),
            (source.delta_envelope, "DeltaManifestV2", work.delta_manifest),
        ):
            env = parse_envelope(raw)
            if (
                not verify_source(raw)
                or env.type != kind
                or env.run_id != manifest.run_id()
                or env.signer != work.roster.hotkey
                or env.body != original.model_dump(mode="json")
            ):
                raise TapeError("original miner source signature/authority differs")
        if (
            (anchor.run_id, anchor.hotkey, anchor.w, anchor.layout_hash)
            != (manifest.run_id(), work.roster.hotkey, w - 1, authority.layout_hash)
            or anchor.backend not in ("genesis", authority.backend)
            or anchor.state.step != w * cfg.inner.H
            or len(source.sample_ids) != manifest.training.batch_samples()
        ):
            raise TapeError("repair anchor/layout/step/assignment mismatch")
        # Same byte encoding as challenge.store.assignment_hash; no dependency on service.
        assignment_hash = sha256_hex(
            b"ht-assignment-v1"
            + bytes.fromhex(manifest.run_id())
            + struct.pack(">QQ", w, work.roster.slot)
            + b"".join(struct.pack("<Q", i) for i in source.sample_ids)
        )
        if assignment_hash != work.assignment_hash:
            raise TapeError("repair assignment differs from accepted work")
        carry = OptState(
            {n: x.to(authority.backend) for n, x in anchor.state.m.items()},
            {n: x.to(authority.backend) for n, x in anchor.state.v.items()},
            anchor.state.step,
        )
        result = train_island(
            cfg,
            reference.layout,
            comm,
            theta,
            IslandAssignment(
                manifest.run_id(), w, source.sample_ids, anchor.state.step, reference.layout.n_gpus
            ),
            get_sample,
            ef_in={n: x.to(authority.backend) for n, x in anchor.ef.items()},
            carry=carry,
        )
        _finite_state(comm, result.final_theta, result.final_state, result.ef_out)
        delta_hash = store.put(result.delta_payload)
        e = RecomputedWork(
            hotkey=work.roster.hotkey,
            source_commit_envelope=sha256_hex(source.commit_envelope),
            source_delta_envelope=sha256_hex(source.delta_envelope),
            acceptance_hash=source.acceptance_hash,
            source_anchor_hash=anchor.anchor_hash,
            source_anchor_proof=anchor.proof_hash,
            start_theta_hash=state_hash(theta),
            start_optimizer_hash=optimizer_hash(anchor.state),
            start_ef_hash=state_hash(anchor.ef),
            global_step0=anchor.state.step,
            sample_ids=list(source.sample_ids),
            delta_hash=delta_hash,
            leaves_root=result.leaves_root,
            final_theta_hash=result.final_theta_hash,
            final_state_object=store.put(pack_state(result.final_theta, result.final_state)),
            final_ef_object=store.put(pack_state(result.ef_out)),
            final_optimizer_hash=optimizer_hash(result.final_state),
            ef_out_hash=result.ef_out_hash,
        )
        evidence.append(e)
        proof = sha256_hex(
            canonicalize(
                [
                    "ht-rollback-anchor/1",
                    manifest.run_id(),
                    w,
                    predecessor_tape_hash,
                    prev_state,
                    authority.source_tape_hash,
                    authority.qualification_hash,
                    authority.reference_hash,
                    authority.layout_hash,
                    authority.backend,
                    e.body(),
                ]
            )
        )
        anchors.append(
            VerifiedAnchor(
                manifest.run_id(),
                work.roster.hotkey,
                w,
                authority.layout_hash,
                result.final_state.clone(),
                {n: x.clone() for n, x in result.ef_out.items()},
                proof,
                sha256_hex(b"hypertrain/rollback/anchor/1|" + bytes.fromhex(proof)),
                {n: x.clone() for n, x in result.final_theta.items()},
                authority.backend,
            )
        )
        deltas[work.roster.hotkey] = load_delta(store, delta_hash, previous.theta)
    arithmetic, state = _aggregate_body(
        manifest,
        policy,
        previous,
        w=w,
        prev_state=prev_state,
        predecessor_tape_hash=predecessor_tape_hash,
        records=records,
        exclusions=exclusions,
        deltas=deltas,
    )
    body = RepairBody(
        v="ht-rollback-replay/1",
        purpose="model-repair-no-reward",
        source_tape_hash=authority.source_tape_hash,
        qualification_hash=authority.qualification_hash,
        reference_hash=authority.reference_hash,
        layout_hash=authority.layout_hash,
        backend=authority.backend,
        arithmetic=arithmetic,
        recomputed=evidence,
    )
    return body, state, tuple(anchors)


def make_repair_tape(
    store: Store,
    manifest: RunManifestV2,
    policy: AggregationPolicyV2,
    economics: bytes,
    key: Keypair,
    authority: RepairAuthority,
    comm: Comm,
    get_sample: SampleFn,
    *,
    w: int,
    prev_state: str,
    predecessor_tape_hash: str,
    sources: Sequence[RepairSource],
    excluded: Sequence[Exclusion],
    reference_reward_units: int,
) -> RepairResult:
    if key.ss58 != authority.coordinator:
        raise TapeError("repair signer is not pinned coordinator")
    body, state, anchors = _compute_repair(
        store,
        manifest,
        policy,
        economics,
        authority,
        comm,
        get_sample,
        w=w,
        prev_state=prev_state,
        predecessor_tape_hash=predecessor_tape_hash,
        sources=sources,
        excluded=excluded,
        reference_reward_units=reference_reward_units,
    )
    store.put(state.to_bytes())
    tape = RepairTape(body=body, signer=key.ss58, sig=key.sign(repair_message(body)).hex())
    return RepairResult(tape, state, anchors)


def replay_repair_tape(
    store: Store,
    tape: RepairTape,
    manifest: RunManifestV2,
    policy: AggregationPolicyV2,
    economics: bytes,
    authority: RepairAuthority,
    comm: Comm,
    get_sample: SampleFn,
    *,
    w: int,
    prev_state: str,
    predecessor_tape_hash: str,
    sources: Sequence[RepairSource],
    excluded: Sequence[Exclusion],
    reference_reward_units: int,
) -> RepairResult:
    """Accept only independently recomputed bytes under caller-pinned source authorities."""
    if tape.signer != authority.coordinator or not verify(
        decode_hotkey(authority.coordinator), repair_message(tape.body), bytes.fromhex(tape.sig)
    ):
        raise TapeError("repair signature/authority differs")
    body, state, anchors = _compute_repair(
        store,
        manifest,
        policy,
        economics,
        authority,
        comm,
        get_sample,
        w=w,
        prev_state=prev_state,
        predecessor_tape_hash=predecessor_tape_hash,
        sources=sources,
        excluded=excluded,
        reference_reward_units=reference_reward_units,
    )
    if body != tape.body or get_object(store, body.arithmetic.out_state) != state.to_bytes():
        raise TapeError("independent repair replay differs")
    return RepairResult(tape, state, anchors)


if __name__ == "__main__":
    _context_worker(*sys.argv[1:])
