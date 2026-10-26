"""Mutation check of the elastic core (ADR 0002): each mutant must fail its named test.

A mutant replaces one exact snippet of one file, the named test runs, and the file is restored.
The check fails when a test still passes under its mutant (pytest exit 1 is the only kill: a
collection error is not), when a snippet is not found exactly once, or when the suite fails
without mutants. The child never writes bytecode and the module's cached bytecode is removed
around each run, so a test always runs the mutant and nothing of it survives the restore; SIGTERM
and SIGHUP restore the file too. Run from hypertrain/:

    uv run --frozen python scripts/check_elastic_mutants.py
"""

from __future__ import annotations

import importlib.util
import os
import signal
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SRC = "src/hypertrain/elastic/"
TESTS = "tests/elastic/"


@dataclass(frozen=True, slots=True)
class Mutant:
    name: str
    path: str
    old: str
    new: str
    test: str


MUTANTS = (
    Mutant(
        "checkpoint write not atomic",
        SRC + "store.py",
        'tmp = path.parent / f".{sha}.{uuid.uuid4().hex}.tmp"',
        "tmp = path",
        "test_store.py::test_put_killed_before_fsync_publishes_nothing",
    ),
    Mutant(
        "checkpoint write not synced before rename",
        SRC + "store.py",
        "                os.fsync(fd)\n",
        "",
        "test_store.py::test_put_killed_before_fsync_publishes_nothing",
    ),
    Mutant(
        "torn object under its key trusted",
        SRC + "store.py",
        "if self._intact(path, sha):",
        "if path.exists():",
        "test_store.py::test_put_rewrites_a_torn_object_under_its_key",
    ),
    Mutant(
        "lease never expires",
        SRC + "membership.py",
        "return member.status is Status.ACTIVE and self.clock.now_ms() < member.lease.expires_ms",
        "return member.status is Status.ACTIVE",
        "test_membership.py::test_lease_lapses_without_a_heartbeat",
    ),
    Mutant(
        "rejoin reuses a token (no fencing)",
        SRC + "membership.py",
        "self._tokens += 1",
        "self._tokens = 1",
        "test_membership.py::test_rejoin_fences_every_older_token",
    ),
    Mutant(
        "double assignment: capacity not consumed",
        SRC + "assignment.py",
        "left[member] -= 1",
        "left[member] -= 0",
        "test_assignment.py::test_plan_gives_each_pending_shard_to_one_member_within_its_capacity",
    ),
    Mutant(
        "double assignment: a held shard is planned again",
        SRC + "coordinator.py",
        "if s.holder is None and not s.done]",
        "if not s.done]",
        "test_coordinator.py::test_a_departure_moves_only_the_departed_members_shards",
    ),
    Mutant(
        "a holder that never progresses keeps its shard",
        SRC + "coordinator.py",
        "if s.holder is not None and now - s.since_ms >= self.progress_ms",
        "if False",
        "test_coordinator.py::test_a_holder_that_stops_progressing_loses_its_shard_unpenalised",
    ),
    Mutant(
        "an accepted link does not restart the progress deadline",
        SRC + "coordinator.py",
        "s.head, s.step, s.since_ms = key, manifest.step, self.clock.now_ms()",
        "s.head, s.step = key, manifest.step",
        "test_coordinator.py::test_a_holder_that_stops_progressing_loses_its_shard_unpenalised",
    ),
    Mutant(
        "an unadmitted key joins and replaces the live holder",
        SRC + "coordinator.py",
        "if not self.admit(member, owner, signer):",
        "if False:",
        "test_coordinator.py::test_only_an_admitted_key_joins_or_replaces_a_member",
    ),
    Mutant(
        "a non-holder extends a shard",
        SRC + "coordinator.py",
        "if s.holder != Holder(member, token):",
        "if s.holder is None:",
        "test_coordinator.py::test_only_the_live_holder_extends_a_shard",
    ),
    Mutant(
        "a stale lease reaches the shard checks",
        SRC + "coordinator.py",
        "        m = self.membership.check(member, token)\n        s = self._shards.get(shard)",
        "        m = self.membership.get(member)\n        assert m is not None\n"
        "        s = self._shards.get(shard)",
        "test_coordinator.py::test_a_replacement_host_resumes_the_shard_from_the_accepted_head",
    ),
    Mutant(
        "head not compare-and-set",
        SRC + "checkpoint.py",
        "if manifest.parent != head:",
        "if False:",
        "test_coordinator.py::test_a_commit_must_extend_the_accepted_head",
    ),
    Mutant(
        "retried commit refused",
        SRC + "coordinator.py",
        "if done is not None and (done[0], done[2]) == (shard, member):",
        "if False:",
        "test_coordinator.py::test_a_retried_commit_is_acknowledged_again_without_new_credit",
    ),
    Mutant(
        "retried commit credited twice",
        SRC + "coordinator.py",
        "            return Accepted(shard, key, done[1], True)",
        "            self._shards[shard].links.append(self._shards[shard].links[-1])\n"
        "            return Accepted(shard, key, done[1], True)",
        "test_coordinator.py::test_a_retried_commit_is_acknowledged_again_without_new_credit",
    ),
    Mutant(
        "a retry after its lease ended is refused",
        SRC + "coordinator.py",
        "        done = self._accepted.get(key)\n",
        "        self.membership.check(member, token)\n        done = self._accepted.get(key)\n",
        "test_coordinator.py::test_a_retried_commit_is_acknowledged_after_its_lease_and_round_ended",
    ),
    Mutant(
        "accepted links forgotten at a round boundary",
        SRC + "coordinator.py",
        "        self._departures = []\n",
        "        self._departures = []\n        self._accepted.clear()\n",
        "test_coordinator.py::test_a_retried_commit_is_acknowledged_after_its_lease_and_round_ended",
    ),
    Mutant(
        "accepted work dropped at a departure",
        SRC + "coordinator.py",
        "        for s in released:\n            s.holder = None\n",
        "        for s in released:\n            s.holder, s.head, s.step = None, None, 0\n",
        "test_coordinator.py::test_a_lapsed_lease_releases_its_shard_and_head_without_a_tick",
    ),
    Mutant(
        "replacement host does not inherit the shards",
        SRC + "coordinator.py",
        "            for s in self._held(replaced.token):\n"
        "                s.holder, s.since_ms = Holder(member, lease.token), self.clock.now_ms()\n",
        "",
        "test_coordinator.py::test_a_replacement_host_resumes_the_shard_from_the_accepted_head",
    ),
    Mutant(
        "resume trusts a spliced chain",
        SRC + "checkpoint.py",
        "            if not manifest.binds(unit):\n"
        '                raise CheckpointError("WRONG_UNIT", key)\n'
        "            if out",
        "            if out",
        "test_checkpoint.py::test_resume_refuses_a_chain_spliced_from_another_shard",
    ),
    Mutant(
        "manifest written before its state",
        SRC + "checkpoint.py",
        "        if not self.store.exists(state):\n"
        '            raise CheckpointError("MISSING_STATE", state)\n',
        "",
        "test_checkpoint.py::test_a_manifest_is_never_written_before_its_state",
    ),
    Mutant(
        "a holder commits a link another member produced",
        SRC + "coordinator.py",
        "if manifest.producer != member or manifest.lease != token:",
        "if False:",
        "test_coordinator.py::test_a_holder_cannot_commit_a_link_another_member_produced",
    ),
    Mutant(
        "a head whose state was never uploaded is accepted",
        SRC + "checkpoint.py",
        "        if not self.store.exists(manifest.state):\n"
        '            raise CheckpointError("MISSING_STATE", manifest.state)\n',
        "",
        "test_coordinator.py::test_a_head_whose_state_was_never_uploaded_is_refused",
    ),
    Mutant(
        "a link past the end of its unit is accepted",
        SRC + "checkpoint.py",
        ") and self.step <= unit.steps",
        ")",
        "test_checkpoint.py::test_a_link_past_the_end_of_its_unit_is_refused",
    ),
    Mutant(
        "a torn state surfaces as a store error",
        SRC + "checkpoint.py",
        "            return step, self.store.get(key)\n"
        "        except ObjectNotFound:\n"
        '            raise CheckpointError("MISSING_STATE", key) from None\n'
        "        except CorruptObjectError:\n",
        "            return step, self.store.get(key)\n"
        "        except ObjectNotFound:\n"
        '            raise CheckpointError("MISSING_STATE", key) from None\n'
        "        except ZeroDivisionError:\n",
        "test_checkpoint.py::test_a_torn_state_is_a_checkpoint_error",
    ),
    Mutant(
        "a leave from another run accepted",
        SRC + "coordinator.py",
        "if signed.notice.run_id != self.run_id:",
        "if False:",
        "test_coordinator.py::test_a_leave_signed_for_another_run_is_refused",
    ),
    Mutant(
        "a leave with unbounded heads accepted",
        SRC + "notice.py",
        "heads: Annotated[list[ShardHead], Field(max_length=MAX_HEADS)]",
        "heads: list[ShardHead]",
        "test_notice.py::test_the_leave_signature_covers_every_field",
    ),
    Mutant(
        "a round opens on a base that is not stored",
        SRC + "coordinator.py",
        "            if not self.checkpoints.store.exists(u.base):\n",
        "            if False:\n",
        "test_coordinator.py::test_open_round_refuses_units_no_manifest_can_bind",
    ),
    Mutant(
        "departures after the last commit are dropped",
        SRC + "coordinator.py",
        "closed = self.settle() if self._shards else None",
        "closed = None",
        "test_coordinator.py::test_a_departure_after_the_last_commit_is_in_the_closing_settlement",
    ),
    Mutant(
        "an unreachable IMDS raises a transport error",
        SRC + "notice.py",
        "        except httpx.HTTPError as e:\n",
        "        except ZeroDivisionError as e:\n",
        "test_notice.py::test_an_unreachable_imds_raises_a_notice_error",
    ),
    Mutant(
        "forged leave signature accepted",
        SRC + "notice.py",
        "if not check_sig(",
        "if False and check_sig(",
        "test_coordinator.py::test_a_forged_leave_signature_is_refused",
    ),
    Mutant(
        "leave from an unregistered key accepted",
        SRC + "coordinator.py",
        "if notice.signer != m.signer:",
        "if False:",
        "test_coordinator.py::test_a_leave_signed_by_a_key_the_member_did_not_register_is_refused",
    ),
    Mutant(
        "retried leave refused",
        SRC + "coordinator.py",
        "if done is not None and done[0] == (notice.member, notice.signer):",
        "if False:",
        "test_coordinator.py::test_a_signed_leave_releases_the_shards_at_once_without_penalty",
    ),
    Mutant(
        "drain flushes past the deadline",
        SRC + "drain.py",
        "fits = notice.deadline_ms - started >= self.flush_ms + self.margin_ms",
        "fits = True",
        "test_drain.py::test_a_late_notice_leaves_at_once_without_flushing",
    ),
    Mutant(
        "drain ignores the measured flush time",
        SRC + "drain.py",
        "        self.flush_ms = ms\n",
        "        del ms\n",
        "test_drain.py::test_the_budget_follows_the_last_measured_flush",
    ),
    Mutant(
        "a failed leave is retried with a second poll and flush",
        SRC + "drain.py",
        "        if self._pending is None:\n",
        "        if True:\n",
        "test_drain.py::test_a_failed_leave_is_retried_by_the_next_check_and_the_drain_runs_once",
    ),
    Mutant(
        "no leave when the flush fails",
        SRC + "drain.py",
        "            finally:\n                self._pending = (notice, flushed, started)\n",
        "            except BaseException:\n                raise\n            else:\n"
        "                self._pending = (notice, flushed, started)\n",
        "test_drain.py::test_a_failed_flush_still_leaves_and_raises",
    ),
    Mutant(
        "hibernate read as a two-minute notice",
        SRC + "notice.py",
        '0 if action == "hibernate" else ',
        "",
        "test_notice.py::test_a_hibernate_notice_leaves_no_time",
    ),
    Mutant(
        "expired IMDS session not renewed",
        SRC + "notice.py",
        "if r.status_code == 401:",
        "if False:",
        "test_notice.py::test_an_expired_session_is_renewed_once",
    ),
    Mutant(
        "a lapse is penalised",
        SRC + "coordinator.py",
        "        released = self._held(m.lease.token)\n",
        "        self._penalties[m.owner] += kind is Exit.LAPSE\n"
        "        released = self._held(m.lease.token)\n",
        "test_sim.py::test_lapses_and_notices_never_penalise_an_honest_miner",
    ),
    Mutant(
        "a valid link refused once (accepted work silently wasted)",
        SRC + "coordinator.py",
        "        if s.holder != Holder(member, token):\n",
        "        if key not in self.__dict__.setdefault('_seen', set()):\n"
        "            self.__dict__['_seen'].add(key)\n"
        '            raise CoordinatorError("FLAKY", key)\n'
        "        if s.holder != Holder(member, token):\n",
        "test_sim.py::test_every_scenario_ends_in_the_uninterrupted_state[none-0]",
    ),
    Mutant(
        "a draining miner stops heartbeating",
        SRC + "sim.py",
        "            if self.now >= self.next_hb_ms:\n"
        "                self._heartbeat()  # the lease must outlive the flush\n",
        "",
        "test_sim.py::test_a_slow_flush_keeps_its_lease_and_leaves_with_a_notice",
    ),
    Mutant(
        "a miner sends its leave twice",
        SRC + "sim.py",
        "            self.drain.check()\n            if self.left:\n                return\n",
        "            self.drain.check()\n",
        "test_sim.py::test_a_notice_too_short_to_flush_leaves_at_once_and_once",
    ),
    Mutant(
        "miner ignores the checkpoint interval",
        SRC + "sim.py",
        "if w.step - w.committed_step >= cfg.ckpt_every or w.step == w.unit.steps:",
        "if w.step == w.unit.steps:",
        "test_sim.py::test_each_loss_wastes_at_most_one_checkpoint_interval[loss]",
    ),
    Mutant(
        "replacement resumes from the base, not the head",
        SRC + "sim.py",
        "self.sim.checkpoints.resume(a.unit, a.head)",
        "self.sim.checkpoints.resume(a.unit, None)",
        "test_sim.py::test_resuming_the_same_heads_again_and_again_changes_nothing",
    ),
)


def pytest(target: str) -> int:
    cmd = [sys.executable, "-B", "-m", "pytest", "-q", "-x", "-p", "no:cacheprovider", target]
    env = {**os.environ, "PYTHONDONTWRITEBYTECODE": "1"}
    run = subprocess.run(  # noqa: S603  (fixed argv: this interpreter and a test id)
        cmd, cwd=ROOT, env=env, capture_output=True, timeout=900, check=False
    )
    return run.returncode


def check(m: Mutant) -> str:
    path = ROOT / m.path
    original = path.read_bytes()
    text = original.decode()
    if text.count(m.old) != 1:
        return f"snippet found {text.count(m.old)} times"
    cache = Path(importlib.util.cache_from_source(str(path)))
    try:
        path.write_text(text.replace(m.old, m.new))
        cache.unlink(missing_ok=True)
        code = pytest(TESTS + m.test)
    finally:
        path.write_bytes(original)
        cache.unlink(missing_ok=True)
    return "killed" if code == 1 else f"SURVIVED (pytest exit {code})"


def main() -> int:
    for sig in (signal.SIGTERM, signal.SIGHUP):
        signal.signal(sig, lambda signum, _frame: sys.exit(128 + signum))
    if pytest(TESTS) != 0:
        print("the elastic suite fails without mutants")
        return 1
    rows = [(m, check(m)) for m in MUTANTS]
    print("| mutant | file | caught by | result |\n|---|---|---|---|")
    for m, result in rows:
        print(f"| {m.name} | {Path(m.path).name} | `{m.test}` | {result} |")
    survivors = [m.name for m, result in rows if result != "killed"]
    print(f"\n{len(rows) - len(survivors)}/{len(rows)} mutants killed")
    return 1 if survivors else 0


if __name__ == "__main__":
    raise SystemExit(main())
