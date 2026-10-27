"""Helpers of the absence scenarios: a rostered miner that does not finish its round, and the
non-sanction contract such a miner is owed.

An absent miner is not a cheater. `standing()` reads, from the master's own tables, everything a
sanction would touch (admission state, strikes, pending dispute, transitions, disputes, escrow
slashes, the admission lock, eligibility), so a scenario can compare it before and after the
round. The classes below are driven through the real miner and the real master. The wire doubles
of ac_cheats (no-show, late commit, withdrawal after a stage) play the absences; the only double
defined here is the honest eviction, a launch that dies the way a SIGKILL leaves it.
"""

from __future__ import annotations

import shutil
import tempfile
import time
from collections.abc import Callable
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

import ac_cheats
from ac_world import MinerOutcome, Settlement, Step, Weights, World

from hypertrain.miner.island_launch import confined
from hypertrain.protocol.messages import Finalize
from hypertrain.protocol.messages_v2 import RoundOpenV2

ROSTER = list(range(5))
HONEST = ROSTER[:4]
ABSENT = ROSTER[4]

NO_SHOW = "no-show-before-commit"
LATE_COMMIT = "late-commit"
NO_LEAVES = "commit-without-leaves"
NO_DELTA = "commit-without-delta"
EVICTED_LATE = "evicted-then-back-after-d-commit"
CLASSES = (NO_SHOW, LATE_COMMIT, NO_LEAVES, NO_DELTA, EVICTED_LATE)
SEAMS = {EVICTED_LATE: "launch+time"}  # the classes that have no ac_cheats double of their own


class Evicted(Exception):
    """The host was reclaimed: the launch process is gone."""


@dataclass
class EvictedMidLaunch:
    """The first launch dies after its attempt directory was prepared and before publication.

    A SIGKILLed launch cannot clean up after itself, so the `attempt-*` directory stays beside
    the (absent) `published/` one; the double leaves it the same way and then raises.
    """

    cheat: int = 8
    name: str = "evicted-mid-launch"
    seam: str = "launch"
    attempts: list[Path] = field(default_factory=list)

    def launch(self, real: Callable[..., Any], ctx: Any) -> Callable[..., Any]:
        def killed(job: Any, directory: Path, **kwargs: Any) -> Any:
            attempt = Path(tempfile.mkdtemp(prefix="attempt-", dir=directory))
            for relative in job.object_paths.values():
                target = attempt / relative
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(confined(directory, relative), target)
            self.attempts.append(attempt)
            raise Evicted("host reclaimed during the launch")

        return killed

    def wire(self, miner: Any, ctx: Any) -> None:
        return None


@dataclass(frozen=True)
class Run:
    """Round 0 of five rostered miners: four honest, one absent in the given class."""

    opening: RoundOpenV2
    honest: list[MinerOutcome]
    absent: list[MinerOutcome]


def run_class(world: World, name: str) -> Run:
    """Open round 0, run the four honest miners, then the absent one the way `name` describes.

    The late commit moves the shared master clock, so the absent miner always runs last.
    """
    opening = world.open_round(0, ROSTER)
    honest = world.run_roster({i: None for i in HONEST})
    if name == EVICTED_LATE:
        first = world.run_miner(ABSENT, EvictedMidLaunch())
        world.advance_beacon(opening.d_commit)
        absent = [first, world.run_miner(ABSENT)]
    else:
        absent = [world.run_miner(ABSENT, ac_cheats.BY_NAME[name])]
    return Run(opening, honest, absent)


def finish_round(world: World, settled: Settlement) -> Settlement:
    """Aggregate and finalize the round of `settled` through the admin routes, without scheduling
    audits again.

    World.settle() always starts by scheduling the audits, which the master refuses once the
    audit lease horizon has passed. With the fixture's round offsets the horizon lies more than one
    lease bound after d_audit, and an AuditJobV2 may not outlive its creation beacon by more than
    that bound, so a lease can only be issued at the d_audit beacon itself. A scenario that must
    wait for an absence to become certain therefore audits at d_audit and finishes here.
    """
    n, svc, w = world.network, world.svc, settled.w
    started = time.monotonic()

    def step(name: str, path: str, body: Any = None) -> Step:
        try:
            response = n.client.post(n.url + path, json=body, headers=svc.admin())
        except Exception as error:  # noqa: BLE001 - an uncaught server error is data here
            return Step(name, 500, str(error), None, None, type(error).__name__)
        answer = response.json()
        detail = str(answer.get("detail", answer)) if response.status_code >= 400 else None
        return Step(name, response.status_code, detail, answer)

    steps = [*settled.steps, step("aggregate", f"/admin/rounds/{w}/aggregate")]
    # Read the exclusions after the aggregate: the master may decide an absence while aggregating.
    excluded = {
        row[0]
        for row in n.store._db.execute(
            "SELECT id FROM records_v2 WHERE run_id=? AND kind='excluded'", (world.run_id,)
        )
    }
    included = sorted(
        e.hotkey for e in world.opening(w).roster if f"{w}:{e.hotkey}" not in excluded
    )
    finalize_hash = None
    if steps[-1].ok:
        final = Finalize(
            w=w,
            final_theta_hash_w1=steps[-1].body["theta_hash"],
            included=included,
            entitlements_root="0" * 64,
        )
        steps.append(
            step("finalize", f"/admin/rounds/{w}/finalize", n.signed(svc.COORD, "Finalize", final))
        )
        if steps[-1].ok and steps[-1].body.get("status") == "SETTLED":
            finalize_hash = world.finalize_hash(w)
    return Settlement(
        w,
        tuple(steps),
        settled.verdicts,
        tuple(included),
        finalize_hash,
        settled.seconds + time.monotonic() - started,
    )


def standing(world: World, index: int) -> dict[str, Any]:
    """Everything a sanction of miner `index` would change, as plain data."""
    n, rid = world.network, world.run_id
    db, hotkey = n.store._db, world.hotkey(index)
    admission = db.execute(
        "SELECT admission_id,state,strikes,pending_dispute,reason,clean_count,screen_until "
        "FROM admissions_v2 WHERE run_id=? AND hotkey=?",
        (rid, hotkey),
    ).fetchone()
    admission_id = admission["admission_id"]
    return {
        "admission": dict(admission),
        "transitions": db.execute(
            "SELECT COUNT(*) FROM admission_transitions WHERE admission_id=?", (admission_id,)
        ).fetchone()[0],
        "pending": db.execute(
            "SELECT COUNT(*) FROM admission_pending WHERE admission_id=?", (admission_id,)
        ).fetchone()[0],
        "disputes": db.execute(
            "SELECT COUNT(*) FROM disputes_v2 WHERE miner=?", (hotkey,)
        ).fetchone()[0],
        "slashes": db.execute("SELECT COUNT(*) FROM escrow_events WHERE kind='SLASH'").fetchone()[
            0
        ],
        "locked": world.observe().escrow_by_owner[world.svc.COLD[index].ss58]["admission_locked"],
        "eligible": n.client.get(n.url + "/admission/" + hotkey).json()["eligible"],
    }


def evidence(
    world: World,
    test: str,
    name: str,
    run: Run,
    settlement: Settlement,
    paid: Weights | None = None,
    hole: str | None = None,
) -> dict[str, Any]:
    """The raw material of one absence row of the ADR table, in plain JSON types."""
    seen = world.observe()
    absent, honest = world.hotkey(ABSENT), [world.hotkey(i) for i in HONEST]
    double = ac_cheats.BY_NAME.get(name)
    seam = double.seam if double is not None else SEAMS.get(name, "launch")
    first_failure = next((s for s in settlement.steps if not s.ok and s.status != 204), None)
    verdict = next((v for v in settlement.verdicts if v.hotkey == absent), None)
    row = standing(world, ABSENT)["admission"]
    return {
        "test": test,
        "cheat": 8,
        "name": name,
        "seam": seam,
        "intake": [
            {
                "status": o.status,
                "http_status": o.http_status,
                "endpoint": o.endpoint,
                "detail": o.detail,
                "stage": o.stage,
                "error": o.error,
            }
            for o in run.absent
        ],
        "audit_verdict": None if verdict is None else verdict.result,
        "first_bad_leaf": None if verdict is None else verdict.first_bad_leaf,
        "excluded_reason": seen.excluded.get(f"0:{absent}", {}).get("reason"),
        "settled": settlement.settled,
        "stalled_at": settlement.stalled_at,
        "master_answer": None
        if first_failure is None
        else {
            "step": first_failure.name,
            "status": first_failure.status,
            "detail": first_failure.detail,
        },
        "leases": [
            {"hotkey_is_absent": r["hotkey"] == absent, "state": r["state"], "result": r["result"]}
            for r in seen.leases
        ],
        "absent_weight": None if paid is None else paid.weights.get(absent, 0.0),
        "honest_weights": None if paid is None else [paid.weights.get(h, 0.0) for h in honest],
        "absent_admission": {k: row[k] for k in ("state", "strikes", "pending_dispute")},
        "honest_unsanctioned": all(
            seen.admissions[h]["state"] == "ACTIVE" and seen.admissions[h]["strikes"] == 0
            for h in honest
        ),
        "hole": hole,
    }
