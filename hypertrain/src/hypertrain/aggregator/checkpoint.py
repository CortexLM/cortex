"""Checkpoints: provisional until every audit in their round range closes, then final.

Layout of a checkpoint directory:
  model.safetensors  theta (float32, sorted names)
  lineage.json       signed global event tapes for rounds [w_start, w_end] (+ regional tapes)
  MANIFEST.json      signed {status, run_id, rounds, files: {name: sha256}, theta_hash,
                     included hotkeys, license, dataset attribution}
"""

from __future__ import annotations

import json
from collections.abc import Iterable, Mapping, Sequence
from pathlib import Path
from typing import Any

import numpy as np
from safetensors.numpy import load as st_load
from safetensors.numpy import save as st_save

from hypertrain.aggregator.core import (
    AggregatorError,
    tape_key,
    th,
    verify_tape,
)
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import KeyError_, Keypair, decode_hotkey, verify

MANIFEST = "MANIFEST.json"
MODEL = "model.safetensors"
LINEAGE = "lineage.json"
CKPT_DOMAIN = b"hypertrain/1|Checkpoint|"


class CheckpointError(AggregatorError):
    pass


def _msg(body: Mapping[str, Any]) -> bytes:
    return CKPT_DOMAIN + sha256_hex(canonicalize(body, allow_float=False)).encode()


def _write_manifest(d: Path, kp: Keypair, body: Mapping[str, Any]) -> None:
    doc = {"body": dict(body), "signer": kp.ss58, "sig": kp.sign(_msg(body)).hex()}
    tmp = d / (MANIFEST + ".tmp")
    tmp.write_bytes(canonicalize(doc, allow_float=False))
    tmp.replace(d / MANIFEST)


def _included(tapes: Sequence[Mapping[str, Any]], regional: Mapping[str, Any]) -> list[str]:
    ids: set[str] = set()
    for t in [*tapes, *regional.values()]:
        ids.update(i["id"] for i in t["body"]["inputs"] if not i["id"].startswith("region:"))
    return sorted(ids, key=lambda s: s.encode("utf-8"))


def write_checkpoint(
    d: Path,
    kp: Keypair,
    theta: Mapping[str, Any],
    tapes: Sequence[Mapping[str, Any]],
    *,
    license: str,
    dataset: Mapping[str, str],
    regional_tapes: Sequence[Mapping[str, Any]] = (),
    journal_anchor: Mapping[str, Any] | None = None,
) -> dict[str, Any]:
    """Write a PROVISIONAL checkpoint for the consecutive global tapes `tapes`.

    `journal_anchor` (Aggregator.journal.anchor) is signed into the manifest; pass it back as
    Aggregator(anchor=...) on restart so a truncated aggregator journal fails closed."""
    if not tapes:
        raise CheckpointError("a checkpoint needs at least one global tape")
    d.mkdir(parents=True, exist_ok=False)
    model = st_save(
        {n: np.ascontiguousarray(theta[n], dtype="<f4").reshape(theta[n].shape) for n in theta}
    )
    reg = {tape_key(t): dict(t) for t in regional_tapes}
    lineage = canonicalize({"tapes": [dict(t) for t in tapes], "regional": reg}, allow_float=False)
    (d / MODEL).write_bytes(model)
    (d / LINEAGE).write_bytes(lineage)
    body = {
        "v": "ht-ckpt-v1",
        "status": "provisional",
        "run_id": tapes[0]["body"]["run_id"],
        "rounds": [tapes[0]["body"]["w"], tapes[-1]["body"]["w"]],
        "theta_hash": th({n: np.asarray(theta[n], dtype=np.float32) for n in theta}),
        "included": _included(tapes, reg),
        "license": license,
        "dataset": dict(dataset),
        "files": {MODEL: sha256_hex(model), LINEAGE: sha256_hex(lineage)},
    }
    if journal_anchor is not None:
        body["journal_anchor"] = dict(journal_anchor)
    _write_manifest(d, kp, body)
    return body


def read_manifest(d: Path) -> dict[str, Any]:
    doc: dict[str, Any] = json.loads((d / MANIFEST).read_bytes())
    return doc


def finalize_checkpoint(
    d: Path,
    kp: Keypair,
    open_audit_rounds: Iterable[int],
    journal_anchor: Mapping[str, Any] | None = None,
) -> dict[str, Any]:
    """Mark final; refuses while any audit for a round in the checkpoint range is still open.
    `journal_anchor` (taken after finalize_round) re-anchors the journal head past the finals."""
    errors = verify_checkpoint(d, require_final=False, signer=kp.ss58)
    if errors:
        raise CheckpointError("refusing to finalize an invalid checkpoint: " + "; ".join(errors))
    body = read_manifest(d)["body"]
    lo, hi = body["rounds"]
    blocking = sorted({w for w in open_audit_rounds if lo <= w <= hi})
    if blocking:
        raise CheckpointError(
            f"audits still open for rounds {blocking}; checkpoint stays provisional"
        )
    body = {**body, "status": "final"}
    if journal_anchor is not None:
        body["journal_anchor"] = dict(journal_anchor)
    _write_manifest(d, kp, body)
    return body


def verify_checkpoint(
    d: Path, *, require_final: bool = True, signer: str | None = None
) -> list[str]:
    """Return a list of problems (empty = valid): sha256 files, signature, lineage, provenance."""
    try:
        doc = read_manifest(d)
        body, who, sig = doc["body"], doc["signer"], bytes.fromhex(doc["sig"])
        if set(doc) != {"body", "signer", "sig"}:
            return ["manifest has unexpected fields"]
        if not verify(decode_hotkey(who), _msg(body), sig):
            return ["manifest signature does not verify"]
    except (OSError, ValueError, KeyError, TypeError, KeyError_) as exc:
        return [f"manifest unreadable: {type(exc).__name__}"]
    errs: list[str] = []
    if signer is not None and who != signer:
        errs.append("manifest signer is not the expected coordinator")
    if require_final and body.get("status") != "final":
        errs.append(f"checkpoint status is {body.get('status')!r}, not final")
    files = body.get("files", {})
    on_disk = sorted(p.name for p in d.iterdir() if p.name != MANIFEST)
    if on_disk != sorted(files):
        errs.append(f"files on disk {on_disk} differ from manifest {sorted(files)}")
    for name, want in files.items():
        p = d / name
        if not p.is_file() or sha256_hex(p.read_bytes()) != want:
            errs.append(f"{name}: sha256 mismatch")
    if errs:
        return errs
    if not str(body.get("license", "")).strip():
        errs.append("license missing")
    ds = body.get("dataset", {})
    for k in ("name", "license", "attribution", "merkle_root"):
        if not str(ds.get(k, "")).strip():
            errs.append(f"dataset {k} missing")
    try:
        lin = json.loads((d / LINEAGE).read_bytes())
        tapes, reg = lin["tapes"], lin["regional"]
        theta = st_load((d / MODEL).read_bytes())
    except (ValueError, KeyError, TypeError) as exc:
        return [*errs, f"lineage/model unreadable: {type(exc).__name__}"]
    errs += _check_lineage(body, who, tapes, reg, theta)
    return errs


def _check_lineage(
    body: Mapping[str, Any],
    who: str,
    tapes: Sequence[Mapping[str, Any]],
    reg: Mapping[str, Any],
    theta: Mapping[str, Any],
) -> list[str]:
    errs: list[str] = []
    if not tapes:
        return ["lineage has no tapes"]
    for h, t in reg.items():
        if not verify_tape(t, who) or tape_key(t) != h or t["body"]["kind"] != "regional":
            errs.append(f"regional tape {h[:12]} invalid")
    for j, t in enumerate(tapes):
        b = t["body"]
        if not verify_tape(t, who) or b["kind"] != "global" or b["run_id"] != body["run_id"]:
            errs.append(f"tape {j} signature/kind/run_id invalid")
            continue
        if j and (
            b["w"] != tapes[j - 1]["body"]["w"] + 1
            or b["prev_state"] != tapes[j - 1]["body"]["out_state"]
        ):
            errs.append(f"tape {j} does not chain from tape {j - 1}")
        for hashes in b.get("regional_tapes", {}).values():
            errs += [f"tape {j}: regional tape {h[:12]} missing" for h in hashes if h not in reg]
    if [tapes[0]["body"]["w"], tapes[-1]["body"]["w"]] != list(body["rounds"]):
        errs.append("manifest rounds differ from lineage")
    if tapes[-1]["body"]["theta_hash"] != body["theta_hash"]:
        errs.append("last tape theta_hash differs from manifest")
    if th(theta) != body["theta_hash"]:
        errs.append("model.safetensors theta hash differs from manifest")
    if _included(tapes, reg) != body["included"]:
        errs.append("included hotkeys differ from lineage inputs")
    return errs


def require_network_roster(roster: Sequence[Any]) -> None:
    """Guard the complete accepted roster before every v2 compute/make/replay call."""
    if len(roster) > 16:
        raise CheckpointError("ROSTER_LIMIT_16: complete roster requires runtime qualification")


def network_inputs(data: Sequence[Mapping[str, Any]]) -> list[Any]:
    from hypertrain.aggregator.tape_v2 import VerifiedInput
    from hypertrain.challenge.trust_v2 import FundedStatus, ReplayEvidence, SettlementStatus
    from hypertrain.protocol.messages_v2 import CommitV2, DeltaManifestV2, RosterEntryV2

    return [
        VerifiedInput(
            RosterEntryV2.model_validate(x["roster"]),
            CommitV2.model_validate(x["commit"]),
            DeltaManifestV2.model_validate(x["delta_manifest"]),
            ReplayEvidence(**x["replay"]),
            FundedStatus(**x["funding"]),
            SettlementStatus(**x["settlement"]),
            x["assignment_hash"],
            x["clean_finalizations"],
        )
        for x in data
    ]


def _network_subjects(
    item: Mapping[str, Any],
    tape: Any,
    manifest: Any,
    signer: str,
    previous: Mapping[str, Any] | None,
) -> None:
    from hypertrain.protocol import envelope_v2
    from hypertrain.protocol.messages import Finalize
    from hypertrain.protocol.messages_v2 import RoundOpenV2

    if "repair_context" in item:
        _repair_subjects(item, tape, manifest, signer, previous)
        return
    run_id = manifest.run_id()
    for field, expected in (("round_open", "RoundOpenV2"), ("finalize", "Finalize")):
        try:
            env = envelope_v2.parse_envelope(item[field])
        except ValueError as error:
            raise CheckpointError(f"v2 {field} invalid envelope: {error}") from error
        if (
            env.type != expected
            or env.run_id != run_id
            or env.signer != signer
            or (not envelope_v2.verify_envelope(env.model_dump()))
        ):
            raise CheckpointError(f"v2 {field} authority/subject mismatch")
    opening = RoundOpenV2.model_validate(item["round_open"]["body"])
    final = Finalize.model_validate(item["finalize"]["body"])
    contributors = [work["roster"]["hotkey"] for work in item["inputs"]]
    roster = {entry.hotkey: entry.body() for entry in opening.roster}
    if len(contributors) != len(set(contributors)) or any(
        work["roster"] != roster.get(work["roster"]["hotkey"]) for work in item["inputs"]
    ):
        raise CheckpointError("v2 contributors differ from accepted round roster")
    expected_included = sorted(contributors, key=lambda hot: hot.encode("utf-8"))
    if (
        final.w != opening.w
        or tape.body.w != opening.w
        or tape.body.run_id != run_id
        or (
            final.included != expected_included
            or final.final_theta_hash_w1 != tape.body.out_hashes["theta_hash"]
        )
    ):
        raise CheckpointError("v2 finality round/contributor/output subject mismatch")
    if opening.theta_hash != tape.body.prev_hashes["theta_hash"] or (
        tape.body.prev_state != item["prev_state"]
        or tape.body.predecessor_tape_hash != item["predecessor_tape_hash"]
    ):
        raise CheckpointError("v2 round/tape state lineage mismatch")
    if previous is not None:
        if (
            opening.w != previous["w"] + 1
            or item["prev_state"] != previous["out_state"]
            or (
                item["predecessor_tape_hash"] != previous["tape_hash"]
                or opening.prev_final_hash != previous["final_hash"]
            )
        ):
            raise CheckpointError("v2 checkpoint rounds/predecessors are not contiguous")
    elif opening.w == 0 and item["predecessor_tape_hash"] != "0" * 64:
        raise CheckpointError("v2 genesis predecessor mismatch")


def _repair_subjects(
    item: Mapping[str, Any],
    tape: Any,
    manifest: Any,
    signer: str,
    previous: Mapping[str, Any] | None,
) -> None:
    from hypertrain.protocol import envelope_v2
    from hypertrain.protocol.messages import Rollback
    from hypertrain.protocol.messages_v2 import RoundOpenV2

    opening = envelope_v2.parse_envelope(item["round_open"])
    rollback = envelope_v2.parse_envelope(item["rollback"])
    if any(
        not envelope_v2.verify_envelope(e.model_dump())
        or e.signer != signer
        or e.run_id != manifest.run_id()
        for e in (opening, rollback)
    ) or (opening.type != "RoundOpenV2" or rollback.type != "Rollback"):
        raise CheckpointError("repair checkpoint subject authority differs")
    original_open = RoundOpenV2.model_validate(opening.body)
    request = Rollback.model_validate(rollback.body)
    context = item["repair_context"]["body"]
    roster = {r.hotkey: r.body() for r in original_open.roster}
    inputs = [s["work"] for s in context["sources"]]
    if any(work["roster"]["hotkey"] in request.excluded for work in item["inputs"]):
        raise CheckpointError("repair excluded identity still included")
    if context["w"] == request.w + 1 and previous is None:
        raise CheckpointError("repair checkpoint requires both repaired rounds")
    if set(request.excluded) != {e["hotkey"] for e in context["excluded"]} or (
        set(request.cause_hashes) != {e["evidence_hash"] for e in context["excluded"]}
    ):
        raise CheckpointError("repair checkpoint exact exclusion/cause set differs")
    if any(x["roster"] != roster.get(x["roster"]["hotkey"]) for x in inputs) or (
        context["w"] != original_open.w
        or tape.body.w != original_open.w
        or original_open.w not in (request.w, request.w + 1)
        or context["prev_state"] != tape.body.prev_state
        or context["predecessor_tape_hash"] != tape.body.predecessor_tape_hash
        or item["prev_state"] != tape.body.prev_state
        or item["predecessor_tape_hash"] != tape.body.predecessor_tape_hash
        or request.recomputed != ["agg_w", "step_w", "agg_w1", "step_w1"]
        or inputs != item["inputs"]
    ):
        raise CheckpointError("repair checkpoint original roster/round/lineage differs")
    if original_open.w == request.w + 1 and (
        request.new_theta_hash_w2 != tape.body.out_hashes["theta_hash"]
        or request.new_outer_state_hash != tape.body.out_state
    ):
        raise CheckpointError("repair checkpoint terminal output differs")
    if previous is not None and (
        original_open.w != previous["w"] + 1
        or item["prev_state"] != previous["out_state"]
        or item["predecessor_tape_hash"] != previous["tape_hash"]
    ):
        raise CheckpointError("repair checkpoint predecessor differs")
    if previous is None and original_open.w == 0 and item["predecessor_tape_hash"] != "0" * 64:
        raise CheckpointError("repair genesis predecessor differs")


def _network_tape(store: Any, item: Mapping[str, Any]) -> Any:
    from types import SimpleNamespace

    from hypertrain.aggregator.rollback_v2 import RepairTape
    from hypertrain.aggregator.tape_v2 import TapeV2

    if "repair_context" in item:
        repair = RepairTape.from_bytes(store.get(item["tape_hash"]))
        return SimpleNamespace(body=repair.body.arithmetic, repair=repair)
    return TapeV2.from_bytes(store.get(item["tape_hash"]))


def _network_replay(
    store: Any,
    item: Mapping[str, Any],
    tape: Any,
    manifest: Any,
    policy: Any,
    economics: bytes,
    signer: str,
    prior_anchors: Any,
) -> tuple[Any, Any]:
    from hypertrain.aggregator.rollback_v2 import execute_repair_context
    from hypertrain.aggregator.tape_v2 import replay_tape

    if "repair_context" not in item:
        state = replay_tape(
            store,
            tape,
            manifest,
            policy,
            economics,
            signer=signer,
            w=tape.body.w,
            prev_state=item["prev_state"],
            predecessor_tape_hash=item["predecessor_tape_hash"],
            inputs=network_inputs(item["inputs"]),
            reference_reward_units=item["reference_reward_units"],
        )
        return state, None
    from hypertrain.auditor.replay import pack_state

    context = item["repair_context"]
    if tape.body.w > 0 and prior_anchors is None:
        from hypertrain.aggregator.rollback_v2 import replay_history_context

        if not context["body"].get("history") or len(context["body"]["history"]) != tape.body.w:
            raise CheckpointError(
                "first non-genesis repair requires complete authenticated predecessor history"
            )
        last = _network_tape(store, context["body"]["history"][-1])
        if (last.body.out_state, context["body"]["history"][-1]["tape_hash"]) != (
            item["prev_state"],
            item["predecessor_tape_hash"],
        ):
            raise CheckpointError("authenticated predecessor history global endpoint differs")
        prior_anchors = replay_history_context(store, manifest, context, signer)
    from hypertrain.aggregator.tape_v2 import TapeV2

    original_tape = TapeV2.from_bytes(store.get(context["body"]["source_tape_hash"]))
    if tape.body.w == item["rollback"]["body"]["w"] and (
        original_tape.body.prev_state != item["prev_state"]
        or original_tape.body.predecessor_tape_hash != item["predecessor_tape_hash"]
        or original_tape.body.prev_hashes["theta_hash"] != item["round_open"]["body"]["theta_hash"]
    ):
        raise CheckpointError("repair original round predecessor differs")
    if tape.body.w == item["rollback"]["body"]["w"] + 1 and (
        original_tape.body.out_hashes["theta_hash"] != item["rollback"]["body"]["old_theta_hash_w2"]
    ):
        raise CheckpointError("repair original terminal theta differs")
    if prior_anchors is not None:
        anchors = {a.hotkey: a for a in prior_anchors}
        for source in context["body"]["sources"]:
            a = source["anchor"]
            expected = anchors.get(a["hotkey"])
            if expected is None or (
                a["anchor_hash"] != expected.anchor_hash
                or a["proof_hash"] != expected.proof_hash
                or a["state_object"] != sha256_hex(pack_state(expected.theta, expected.state))
                or a["ef_object"] != sha256_hex(pack_state(expected.ef))
            ):
                raise CheckpointError(
                    "repair checkpoint carried anchor differs from independent replay"
                )
    result = execute_repair_context(store, manifest, context, signer, tape=tape.repair)
    return result.state, result.anchors


def write_network_checkpoint(
    directory: Path, key: Keypair, store: Any, manifest: Any, rounds: Sequence[Mapping[str, Any]]
) -> dict[str, Any]:
    """Replay each full accepted roster and tape before emitting a signed v2 checkpoint."""
    from hypertrain.aggregator.tape_v2 import TapeV2
    from hypertrain.protocol.messages_v2 import AggregationPolicyV2, RoundOpenV2

    for item in rounds:
        require_network_roster(RoundOpenV2.model_validate(item["round_open"]["body"]).roster)
    if not rounds:
        raise CheckpointError("checkpoint requires accepted finalized rounds")
    policy = AggregationPolicyV2.model_validate_json(
        store.get(manifest.network.aggregation_policy_hash)
    )
    economics = store.get(manifest.network.economics_policy_hash)
    state = None
    previous = None
    prior_anchors = None
    pending_repair = None
    for item in rounds:
        from hypertrain.protocol import envelope_v2

        opening = item["round_open"]
        if (
            opening["signer"] != key.ss58
            or opening["run_id"] != manifest.run_id()
            or not envelope_v2.verify_envelope(opening)
        ):
            raise CheckpointError("round opening authority mismatch")
        tape = _network_tape(store, item)
        if "repair_context" in item:
            request_body = item["rollback"]["body"]
            if pending_repair is None:
                if tape.body.w != request_body["w"]:
                    raise CheckpointError("checkpoint repair pair must start at excluded round")
                pending_repair = request_body
            else:
                if request_body != pending_repair or tape.body.w != pending_repair["w"] + 1:
                    raise CheckpointError("checkpoint repair pair authority differs")
                pending_repair = None
        elif pending_repair is not None:
            raise CheckpointError("checkpoint repair pair interrupted")
        _network_subjects(item, tape, manifest, key.ss58, previous)
        require_network_roster(item["round_open"]["body"]["roster"])
        state, prior_anchors = _network_replay(
            store,
            item,
            tape,
            manifest,
            policy,
            economics,
            key.ss58,
            prior_anchors,
        )
        final = item.get("rollback", item.get("finalize"))
        from hypertrain.protocol import envelope_v2

        if (
            not envelope_v2.verify_envelope(final)
            or final["signer"] != key.ss58
            or (
                "repair_context" not in item
                and final["body"]["final_theta_hash_w1"] != tape.body.out_hashes["theta_hash"]
            )
        ):
            raise CheckpointError("unaccepted finality linkage")
        previous = {
            "w": tape.body.w,
            "out_state": tape.body.out_state,
            "tape_hash": item["tape_hash"],
            "final_hash": sha256_hex(canonicalize(final["body"])),
        }
    if pending_repair is not None:
        raise CheckpointError("checkpoint incomplete repair pair")
    assert state is not None
    directory.mkdir(parents=True, exist_ok=False)
    model = st_save(
        {n: np.ascontiguousarray(a, dtype="<f4").reshape(a.shape) for n, a in state.theta.items()}
    )
    lineage = canonicalize({"manifest": manifest.body(), "rounds": list(rounds)})
    (directory / MODEL).write_bytes(model)
    (directory / LINEAGE).write_bytes(lineage)
    # Copy content-addressed inputs only; independent verifier reads original bytes.
    from hypertrain.data.store import LocalFSStore

    backing = LocalFSStore(directory / "objects")
    keys = {manifest.network.aggregation_policy_hash, manifest.network.economics_policy_hash}
    if any("repair_context" in item for item in rounds):
        keys.add(manifest.network.dispute_policy_hash)
    for item in rounds:
        tape = _network_tape(store, item)
        keys.update((item["tape_hash"], item["prev_state"], tape.body.out_state))
        if "repair_context" in item:
            context = item["repair_context"]["body"]
            source_tape = TapeV2.from_bytes(store.get(context["source_tape_hash"]))
            keys.update(
                (
                    context["source_tape_hash"],
                    source_tape.body.prev_state,
                    source_tape.body.out_state,
                    context["samples_hash"],
                    context["proofs_hash"],
                )
            )
            for record in source_tape.body.inputs:
                keys.update((record.commit_hash, record.delta_manifest_hash))
                original_commit = json.loads(store.get(record.commit_hash))
                keys.add(original_commit["delta_hash"])
            for source in context["sources"]:
                keys.update(
                    (
                        source["commit_envelope_hash"],
                        source["delta_envelope_hash"],
                        source["anchor"]["state_object"],
                        source["anchor"]["ef_object"],
                    )
                )
            for e in tape.repair.body.recomputed:
                keys.update((e.delta_hash, e.final_state_object, e.final_ef_object))
            for prior in context.get("history", []):
                historical = _network_tape(store, prior)
                keys.update(
                    (prior["tape_hash"], historical.body.prev_state, historical.body.out_state)
                )
                if "repair_context" in prior:
                    c = prior["repair_context"]["body"]
                    keys.add(c["source_tape_hash"])
                    for s in c["sources"]:
                        keys.update(
                            (
                                s["commit_envelope_hash"],
                                s["delta_envelope_hash"],
                                s["anchor"]["state_object"],
                                s["anchor"]["ef_object"],
                            )
                        )
                for work in prior["inputs"]:
                    keys.update(
                        (
                            sha256_hex(canonicalize(work["commit"])),
                            sha256_hex(canonicalize(work["delta_manifest"])),
                            work["commit"]["delta_hash"],
                        )
                    )
                for job in prior.get("audit_jobs", []):
                    keys.update(
                        (
                            job["start_state"]["state_object_sha256"],
                            job["ef_in"]["sha256"],
                            job["v0"]["sha256"],
                        )
                    )
        for work in item["inputs"]:
            keys.update(
                (
                    sha256_hex(canonicalize(work["commit"])),
                    sha256_hex(canonicalize(work["delta_manifest"])),
                    work["commit"]["delta_hash"],
                )
            )
    for identity in keys:
        backing.put(store.get(identity))
    body = {
        "v": "ht-ckpt-v2",
        "status": "final",
        "run_id": manifest.run_id(),
        "rounds": [rounds[0]["round_open"]["body"]["w"], rounds[-1]["round_open"]["body"]["w"]],
        "theta_hash": th(state.theta),
        "included": sorted(work["roster"]["hotkey"] for work in rounds[-1]["inputs"]),
        "license": "Apache-2.0",
        "files": {MODEL: sha256_hex(model), LINEAGE: sha256_hex(lineage)},
    }
    message = b"hypertrain/checkpoint/2|" + sha256_hex(canonicalize(body)).encode()
    (directory / MANIFEST).write_bytes(
        canonicalize({"body": body, "signer": key.ss58, "sig": key.sign(message).hex()})
    )
    return body


def verify_network_checkpoint(directory: Path, signer: str) -> list[str]:
    from hypertrain.data.store import LocalFSStore
    from hypertrain.protocol.messages_v2 import AggregationPolicyV2, RoundOpenV2, RunManifestV2

    doc = read_manifest(directory)
    body = doc["body"]
    message = b"hypertrain/checkpoint/2|" + sha256_hex(canonicalize(body)).encode()
    if doc["signer"] != signer or not verify(
        decode_hotkey(signer), message, bytes.fromhex(doc["sig"])
    ):
        return ["v2 checkpoint authority/signature mismatch"]
    lineage = json.loads((directory / LINEAGE).read_bytes())
    # COMPLETE rosters, before object reads or replay cost.
    for item in lineage["rounds"]:
        require_network_roster(RoundOpenV2.model_validate(item["round_open"]["body"]).roster)
    for name, digest in body["files"].items():
        if sha256_hex((directory / name).read_bytes()) != digest:
            return [f"{name}: hash mismatch"]
    manifest = RunManifestV2.model_validate(lineage["manifest"])
    if body["run_id"] != manifest.run_id():
        return ["v2 checkpoint run mismatch"]
    if not lineage["rounds"] or body["v"] != "ht-ckpt-v2" or body["status"] != "final":
        return ["v2 checkpoint version/status/empty lineage mismatch"]
    expected_range = [
        lineage["rounds"][0]["round_open"]["body"]["w"],
        lineage["rounds"][-1]["round_open"]["body"]["w"],
    ]
    if body["rounds"] != expected_range or body["included"] != sorted(
        work["roster"]["hotkey"] for work in lineage["rounds"][-1]["inputs"]
    ):
        return ["v2 checkpoint manifest round/contributor subject mismatch"]
    store = LocalFSStore(directory / "objects")
    policy = AggregationPolicyV2.model_validate_json(
        store.get(manifest.network.aggregation_policy_hash)
    )
    econ = store.get(manifest.network.economics_policy_hash)
    previous = None
    prior_anchors = None
    pending_repair = None
    for item in lineage["rounds"]:
        from hypertrain.protocol import envelope_v2

        if (
            not envelope_v2.verify_envelope(item["round_open"])
            or item["round_open"]["signer"] != signer
        ):
            return ["v2 round opening authority mismatch"]
        tape = _network_tape(store, item)
        if "repair_context" in item:
            request_body = item["rollback"]["body"]
            if pending_repair is None:
                if tape.body.w != request_body["w"]:
                    return ["checkpoint repair pair must start at excluded round"]
                pending_repair = request_body
            else:
                if request_body != pending_repair or tape.body.w != pending_repair["w"] + 1:
                    return ["checkpoint repair pair authority differs"]
                pending_repair = None
        elif pending_repair is not None:
            return ["checkpoint repair pair interrupted"]
        try:
            _network_subjects(item, tape, manifest, signer, previous)
        except CheckpointError as error:
            return [str(error)]
        require_network_roster(item["round_open"]["body"]["roster"])
        try:
            state, prior_anchors = _network_replay(
                store,
                item,
                tape,
                manifest,
                policy,
                econ,
                signer,
                prior_anchors,
            )
        except (ValueError, CheckpointError) as error:
            return [str(error)]
        final = item.get("rollback", item.get("finalize"))
        if (
            not envelope_v2.verify_envelope(final)
            or final["signer"] != signer
            or final["run_id"] != manifest.run_id()
            or (
                "repair_context" not in item
                and final["body"]["final_theta_hash_w1"] != tape.body.out_hashes["theta_hash"]
            )
        ):
            return ["v2 checkpoint finality mismatch"]
        previous = {
            "w": tape.body.w,
            "out_state": tape.body.out_state,
            "tape_hash": item["tape_hash"],
            "final_hash": sha256_hex(canonicalize(final["body"])),
        }
    if pending_repair is not None:
        return ["checkpoint incomplete repair pair"]
    if th(st_load((directory / MODEL).read_bytes())) != th(state.theta) or body["theta_hash"] != th(
        state.theta
    ):
        return ["checkpoint model differs from independently replayed tape"]
    return []
