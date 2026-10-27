"""Helpers of the multi-round scenarios of the audit-cheaters lot.

They sit on top of `ac_world.World` and add what a scenario spanning several rounds needs and the
world does not offer: requests posted straight to the master with what they left behind, a round
opening that reaches the master's own anchor check without the operator tooling's pre-check, and
the get_weights answer of a later epoch.
"""

from __future__ import annotations

import json
from collections.abc import Callable
from dataclasses import dataclass
from typing import Any

from ac_cheats import lie
from ac_world import Weights, World

from hypertrain.protocol.envelope import body_digest
from hypertrain.protocol.hashing import sha256_hex

# Tables a request can write; a refused request must leave both row counts unchanged.
PERSISTED = ("accepted_v2", "records_v2")


@dataclass(frozen=True)
class Answer:
    """The master's answer to one request: HTTP status, its detail on a refusal, its JSON body."""

    status: int
    detail: str | None
    body: Any

    @classmethod
    def of(cls, response: Any) -> Answer:
        body = response.json()
        detail = None
        if response.status_code >= 400:
            detail = str(body.get("detail", body)) if isinstance(body, dict) else response.text
        return cls(response.status_code, detail, body)

    @classmethod
    def of_request(cls, send: Callable[[], Any]) -> Answer:
        """The answer to `send()`; the TestClient re-raises an unhandled server error, which is a
        500 answer for a test that asserts the refusal, not an error of the test itself."""
        try:
            return cls.of(send())
        except Exception as error:  # noqa: BLE001 - recorded as the answer, asserted by the caller
            return cls(500, f"{type(error).__name__}: {error}", None)


@dataclass(frozen=True)
class Probe:
    """One request posted to the master and the persisted row counts around it."""

    answer: Answer
    before: dict[str, int]
    after: dict[str, int]

    @property
    def changed_nothing(self) -> bool:
        return self.before == self.after


def persisted(world: World) -> dict[str, int]:
    db = world.network.store._db
    return {table: db.execute(f"SELECT COUNT(*) FROM {table}").fetchone()[0] for table in PERSISTED}


def record_ids(world: World, kind: str) -> set[str]:
    """Identities of the records of this kind the master holds for the run ('w:hotkey' or 'w')."""
    rows = world.network.store._db.execute(
        "SELECT id FROM records_v2 WHERE run_id=? AND kind=?", (world.run_id, kind)
    )
    return {row[0] for row in rows}


def stored_envelope(world: World, kind: str, w: int, hotkey: str) -> dict[str, Any]:
    """The signed envelope the master accepted as `kind` ('commit' or 'delta') for round w."""
    return world.network.store._record_v2(world.run_id, kind, f"{w}:{hotkey}")


def accepted_receipt(world: World, envelope: dict[str, Any]) -> dict[str, Any]:
    """The coordinator receipt the master stored when it accepted this envelope's body."""
    row = world.network.store._db.execute(
        "SELECT receipt FROM accepted_v2 WHERE digest=? AND receipt!='{}'",
        (body_digest(envelope["body"]),),
    ).fetchone()
    assert row is not None, "the envelope was never accepted"
    return json.loads(row[0])


def post_training(world: World, action: str, envelope: dict[str, Any]) -> Probe:
    """POST a signed envelope to /accept, /commit or /delta, as a miner's retry would."""
    before = persisted(world)
    answer = Answer.of_request(
        lambda: world.network.client.post(f"{world.network.url}/{action}", json=envelope)
    )
    return Probe(answer, before, persisted(world))


def commit_resigned_with_another_claim(world: World, index: int, w: int) -> dict[str, Any]:
    """The miner's accepted round-w commit with another final_theta_hash, signed by the miner."""
    stored = stored_envelope(world, "commit", w, world.hotkey(index))
    body = {**stored["body"], "final_theta_hash": lie(stored["body"]["final_theta_hash"])}
    return world.network.signed(world.svc.HOT[index], "CommitV2", body)


def post_round_opening_of_previous_roster(world: World, w: int, prev_final: str) -> Probe:
    """Post the coordinator's round-`w` opening for the very roster of round w-1.

    World.open_round computes every rostered miner's start state itself before it posts, so a
    miner without an anchor never reaches the master. This opening carries a start-state index
    that is deliberately not computed: the master must refuse it on its own anchor lookup, which
    precedes the index comparison.
    """
    import torch

    from hypertrain.aggregator.core import load_state
    from hypertrain.trainer.compress import state_hash

    n, svc, rid = world.network, world.svc, world.run_id
    applied = n.store._record_v2(rid, "applied", str(w - 1))
    theta = {
        k: torch.from_numpy(v.copy())
        for k, v in load_state(n.store.objects, applied["out_state"]).theta.items()
    }
    opening = world.opening(w - 1).model_copy(
        update={
            "w": w,
            "prev_final_hash": prev_final,
            "theta_hash": state_hash(theta),
            "d_open": n.now + 1,
            "d_assign": n.now + 2,
            "d_commit": n.now + 30,
            "d_audit": n.now + 31,
            "d_upload": n.now + 100,
            "d_final": n.now + 131,  # d_final - d_audit = 100, the audit lease of H <= 30
            "start_state_index_hash": sha256_hex(b"start-state-index-not-computed"),
        }
    )
    before = persisted(world)
    answer = Answer.of_request(
        lambda: n.client.post(
            n.url + "/admin/rounds",
            json=n.signed(svc.COORD, "RoundOpenV2", opening),
            headers=svc.admin(),
        )
    )
    return Probe(answer, before, persisted(world))


def weights_of_the_next_epoch(world: World) -> Weights:
    """get_weights for the first epoch after the one already answered.

    An answered epoch is final, so the rewards that did not fit in its mass (one epoch pays at
    most 10^6 units, oldest maturity first) are paid by the following epoch. Everything that must
    happen at the current time (opening a round, admissions) has to be done before this call:
    jumping an epoch ahead lets the fixture admissions go stale.
    """
    n = world.network
    n.push(n.now + n.store.params.epoch_seconds // 3 + 2)
    return world.weights()
