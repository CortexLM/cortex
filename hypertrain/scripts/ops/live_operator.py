"""One-shot coordinator/admin commands (open, audits, finalize) on the production master.

Runs inside the master container. Every value it sends is rebuilt from accepted master state
with the same models, hashes and seals the store validates; nothing here relaxes a guard.
Miner and auditor work stay with their existing CLIs.
"""

from __future__ import annotations

import argparse
import json
import sqlite3
from contextlib import closing
from pathlib import Path

import httpx

import hypertrain.trainer  # noqa: F401
from hypertrain.aggregator.core import load_state
from hypertrain.auditor.replay import AnchorCache, optimizer_hash, pack_state
from hypertrain.data.store import LocalFSStore, put_large
from hypertrain.miner.core import load_keyfile
from hypertrain.protocol import envelope_v2
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages import Finalize, f32hex
from hypertrain.protocol.messages_v2 import (
    PolicyHashes,
    RosterEntryV2,
    RoundOpenV2,
    RunManifestV2,
    StartStateV2,
)
from hypertrain.protocol.relay_messages import RelayRegistryV1
from hypertrain.trainer.compress import state_hash
from hypertrain.trainer.config import TrainConfig
from hypertrain.trainer.model import init_params


def index_key(blob: bytes) -> str:
    """Key put_large(blob, store.put) returns, computed with a hash-only sink (no writes)."""
    return put_large(blob, lambda part: sha256_hex(part))


class Master:
    def __init__(self, args: argparse.Namespace) -> None:
        if (
            not (args.state_dir / "challenge.db").is_file()
            or not (args.state_dir / "objects").is_dir()
        ):
            raise SystemExit(
                "state-dir must contain an existing challenge.db and objects directory"
            )
        self.run_id, self.state = args.run_id, args.state_dir
        self.objects = LocalFSStore(args.state_dir / "objects")
        self.coord = load_keyfile(args.coord_key)
        token = args.admin_token_file.read_text().strip()
        self.admin = {"Authorization": "Bearer " + token}
        self.client = httpx.Client(base_url=args.master, timeout=600)
        self.root = f"/v2/runs/{self.run_id}"

    def query(self, sql: str, *params: object) -> sqlite3.Row | None:
        uri = (self.state / "challenge.db").as_uri() + "?mode=ro"
        with closing(sqlite3.connect(uri, uri=True)) as db:
            db.row_factory = sqlite3.Row
            return db.execute(sql, params).fetchone()

    def record(self, kind: str, identity: str) -> dict:
        row = self.query(
            "SELECT data FROM records_v2 WHERE run_id=? AND kind=? AND id=?",
            self.run_id,
            kind,
            identity,
        )
        if row is None:
            raise SystemExit(f"missing accepted {kind}:{identity}")
        return json.loads(row[0])

    def manifest(self) -> RunManifestV2:
        row = self.query("SELECT manifest FROM runs WHERE run_id=?", self.run_id)
        assert row is not None
        manifest = RunManifestV2.model_validate(envelope_v2.load_json(row[0]))
        if manifest.run_id() != self.run_id or manifest.training.coord_pubkey != self.coord.ss58:
            raise SystemExit("run or coordinator key differs from the stored manifest")
        return manifest

    def call(self, method: str, path: str, body: object = None, admin: bool = False) -> dict:
        response = self.client.request(
            method,
            self.root + path,
            content=None if body is None else canonicalize(body),
            headers=self.admin if admin else {},
        )
        if response.is_error:
            raise SystemExit(f"{method} {path}: {response.status_code} {response.text[:500]}")
        return response.json()


def refuse_service_run(m: Master) -> None:
    """Service-bounded runs need the 4-actor capacity reservation; out of scope here."""
    if m.query(
        "SELECT 1 FROM records_v2 WHERE run_id=? AND kind='service-admission' AND id='run'",
        m.run_id,
    ):
        raise SystemExit("run has a service-admission record; this helper refuses it")


def check_open_shape(w: int, hotkeys: list[str], offsets: list[int]) -> None:
    if w < 0 or not hotkeys or len(set(hotkeys)) != len(hotkeys):
        raise SystemExit("need w >= 0 and unique hotkeys")
    if (
        len(offsets) != 6
        or offsets[0] < 1
        or any(a >= b for a, b in zip(offsets, offsets[1:], strict=False))
    ):
        raise SystemExit("offsets: six strictly increasing positive beacon offsets")
    if not 2 <= offsets[5] - offsets[3] <= 200 or offsets[4] <= offsets[2]:
        raise SystemExit("offsets: need 2 <= d_final-d_audit <= 200 and d_upload > d_commit")


def open_round(m: Master, w: int, hotkeys: list[str], offsets: list[int]) -> dict:
    """RoundOpenV2 rebuilt the way open_round_v2 recomputes it (store.py:4892-4939)."""
    refuse_service_run(m)
    manifest = m.manifest()
    check_open_shape(w, hotkeys, offsets)
    cfg = TrainConfig.from_manifest_v2(manifest)
    now = m.call("GET", "")["now_round"]
    d = [now + o for o in offsets]
    theta, prev_final = init_params(cfg.model), "0" * 64
    if w:
        import torch

        applied = m.record("applied", str(w - 1))
        theta = {
            k: torch.from_numpy(v.copy())
            for k, v in load_state(m.objects, applied["out_state"]).theta.items()
        }
        prev_final = sha256_hex(canonicalize(m.record("finalize", str(w - 1))["body"]))
    starts, roster = [], []
    for slot, hot in enumerate(hotkeys):
        status = m.call("GET", "/admission/" + hot)
        # honored: the challenge assigns live_phase via admission.OWNER_SOLO_HOTKEYS
        # (owner-solo override skips trial/screen), so `eligible` alone is authoritative.
        if not status["eligible"]:
            raise SystemExit("admission is not eligible: " + hot)
        # open_round_v2 refuses the same entry: the screen must cover the round up to d_final.
        screen_until = status["record"]["screen_until"]
        if screen_until < d[5]:
            raise SystemExit(
                f"screen of {hot} ends at round {screen_until}, before d_final {d[5]}: "
                "renew it with a trial first, or open a shorter round"
            )
        cache = AnchorCache()
        if w:
            a = m.record("anchor", f"{w - 1}:{hot}")
            anchor = cache.restore(
                Path(a["path"]),
                manifest,
                a["anchor_hash"],
                a["proof_hash"],
                a["state_root"],
                a["ef_hash"],
                expected_hotkey=hot,
                expected_round=w - 1,
                expected_backend=a["backend"],
            )
        else:
            anchor = cache.genesis(manifest, hot, theta)
        starts.append(
            StartStateV2(
                run_id=m.run_id,
                w=w,
                hotkey=hot,
                theta_hash=state_hash(theta),
                state_object_sha256=index_key(pack_state(theta, anchor.state)),
                opt_state_hash=optimizer_hash(anchor.state),
                ef_object_sha256=index_key(pack_state(anchor.ef)),
                ef_hash=state_hash(anchor.ef),
                parent_anchor_hash=anchor.anchor_hash,
                global_step0=anchor.state.step,
                anchor_verdict_hash=anchor.proof_hash,
            )
        )
        rec = status["record"]
        roster.append(
            {
                "hotkey": hot,
                "slot": slot,
                "q_i": f32hex(1),
                "admission_id": rec["admission_id"],
                "coldkey_group": rec["coldkey"],
                "state": rec["state"],
                "eligible_weight": 4194304,
            }
        )
    current = m.query(
        "SELECT data FROM records_v2 WHERE run_id=? AND kind='registry' AND id='current'",
        m.run_id,
    )
    registry = (
        RelayRegistryV1.model_validate(json.loads(current[0])["body"])
        if current is not None
        else RelayRegistryV1.model_validate_json(
            m.objects.get(manifest.network.relay_registry_hash)
        )
    )
    opening = RoundOpenV2(
        w=w,
        prev_final_hash=prev_final,
        theta_hash=state_hash(theta),
        outer_state_hash="0" * 64,
        center_hash="0" * 64,
        roster_hash=sha256_hex(canonicalize(roster)),
        honeypot_commit="0" * 64,
        d_open=d[0],
        d_assign=d[1],
        d_commit=d[2],
        d_audit=d[3],
        d_upload=d[4],
        d_final=d[5],
        contract_version=2,
        policy_hashes=PolicyHashes.model_validate(
            {k: getattr(manifest.network, k) for k in PolicyHashes.model_fields}
        ),
        registry_epoch=registry.epoch,
        start_state_index_hash=sha256_hex(canonicalize([s.body() for s in starts])),
        audit_mode="anchored-full",
        roster=[RosterEntryV2.model_validate(r) for r in roster],
    )
    signed = envelope_v2.seal(m.coord, "RoundOpenV2", m.run_id, opening, opening.d_final)
    return m.call("POST", "/admin/rounds", signed, True)


def included(m: Master, w: int, opening: RoundOpenV2) -> list[str]:
    """Same set verified_inputs_v2 yields: roster minus 'excluded' records (store.py:7574-7581)."""
    out = []
    for entry in opening.roster:
        identity = f"{w}:{entry.hotkey}"
        if m.query(
            "SELECT 1 FROM records_v2 WHERE run_id=? AND kind='excluded' AND id=?",
            m.run_id,
            identity,
        ):
            continue
        m.record("commit", identity)  # verified_inputs_v2 requires it; refuse before POST
        out.append(entry.hotkey)
    return sorted(out)


FINALIZE_EXPIRY_MARGIN = 1000  # the margin the trial Finalize of the store already uses


def finalize_expiry(d_final: int, now: int) -> int:
    """Last round of the Finalize envelope: a finalize after d_final must still be accepted.

    The stored commit, verdicts and challenge are judged at their acceptance beacon, so a late
    finalize can settle; the Finalize itself is a fresh request and has to be unexpired when posted.
    """
    return max(d_final, now) + FINALIZE_EXPIRY_MARGIN


def finalize(m: Master, w: int) -> dict:
    """One aggregate, then Finalize bound to the applied theta (store.py:7994-7998)."""
    refuse_service_run(m)
    opening = RoundOpenV2.model_validate(m.record("round", str(w))["body"])
    applied = m.call("POST", f"/admin/rounds/{w}/aggregate", admin=True)
    keep = included(m, w, opening)  # after the aggregate: it may leave out a member without a delta
    final = Finalize(
        w=w,
        final_theta_hash_w1=applied["theta_hash"],
        included=keep,
        entitlements_root="0" * 64,
    )
    expiry = finalize_expiry(opening.d_final, m.call("GET", "")["now_round"])
    signed = envelope_v2.seal(m.coord, "Finalize", m.run_id, final, expiry)
    return m.call("POST", f"/admin/rounds/{w}/finalize", signed, True)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--master", required=True)
    parser.add_argument("--run-id", required=True)
    parser.add_argument("--state-dir", type=Path, required=True)
    parser.add_argument("--coord-key", type=Path, required=True)
    parser.add_argument("--admin-token-file", type=Path, required=True)
    sub = parser.add_subparsers(dest="command", required=True)
    o = sub.add_parser("open")
    o.add_argument("--w", type=int, required=True)
    o.add_argument("--hotkey", action="append", required=True)
    o.add_argument("--offsets", required=True, help="d_open..d_final beacon offsets, 6 ints")
    sub.add_parser("audits").add_argument("--w", type=int, required=True)
    sub.add_parser("finalize").add_argument("--w", type=int, required=True)
    args = parser.parse_args(argv)
    if args.command == "open":
        try:
            offsets = [int(x) for x in args.offsets.split(",")]
        except ValueError:
            raise SystemExit("offsets must be integers") from None
        check_open_shape(args.w, args.hotkey, offsets)
    m = Master(args)
    if args.command == "open":
        out = open_round(m, args.w, args.hotkey, offsets)
    elif args.command == "audits":
        out = m.call("POST", f"/admin/rounds/{args.w}/audits", admin=True)
    else:
        out = finalize(m, args.w)
    print(json.dumps(out, sort_keys=True)[:4000])
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
