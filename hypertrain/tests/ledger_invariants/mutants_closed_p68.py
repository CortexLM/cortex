"""Mutants that re-open what ADR 0014 parts 6 and 8 closed: relay custody released without an
operator, and the dispute watch and the object upload signed in their body (a59005e9, dae84220,
67535a7f, 5bd7537f, fad3023c, 6f6e9c90, b61c6389).

Five pins were strict expected failures until those lots landed. Part 6: the relay the entry point
builds asks the master for the settlement of a grant (`settlement_from_master`), so a master
release of a retained upload is acted on, and an upload the master released no longer counts
against the retained-upload cap (`Relay.held`). Part 8: the dispute watch is `POST .../disputes/
watch` with a signed body whose expiry is checked at the current round; a signed upload's admission
check waits for the store lock on a worker thread; the long polls of a watcher run on a pool of
their own. Each mutant reverts one behaviour in a private copy of src, so that the runner proves a
pin fails without it:

- LV-03 + TRN-02: the shipped relay does not refuse a release for want of a settlement journal
  (the settlement view not wired in relay/app.py) and a released upload frees its slot (the cap
  counts every record again, or counts a record whose custody was released);
- SG-05 + HTP-03: a dispute watch captured in an earlier round is refused later (the signed body
  is checked at round 1, so it never expires);
- LV-02 + ADM-04: a signed upload never takes the store lock on the event loop (the admission
  check runs on the loop thread);
- DS-08: waiting dispute polls never starve the other routes (the polls run on the shared pool of
  the request threads again).

Each pin fails alone under its mutants, by its own assertion.

What no pin or mutant covers here: the per-party and per-role caps of the watch pool
(429, WATCH_PER_PARTY and WATCH_MINERS: the DS-08 pin accepts a 429 as well as a 200), the horizon
a signed watch may be dated ahead (REQUEST_HORIZON), the 405 of the retired header-signed routes
and the refusal of an unadmitted signer of an upload (held by the route tables and the intake scan
of test_http_auth.py), and the rotation and pruning of the relay journal.

Run from hypertrain/: .venv/bin/python scripts/check_ledger_invariant_mutants.py --only <ids>
"""

from __future__ import annotations

CORE = "src/hypertrain/relay/core.py"
RELAY_APP = "src/hypertrain/relay/app.py"
APP = "src/hypertrain/challenge/app.py"

HTTP = "tests/ledger_invariants/test_http_auth_2.py::"
WATCH_REPLAY = HTTP + "test_a_dispute_watch_captured_in_an_earlier_round_is_refused_later"
UPLOAD_OFF_LOOP = HTTP + "test_a_signed_object_upload_never_takes_the_store_lock_on_the_event_loop"
SHIPPED_RELAY = (
    HTTP + "test_the_shipped_relay_does_not_refuse_a_release_for_want_of_a_settlement_journal"
)
SLOT_FREED = HTTP + "test_a_released_upload_frees_its_slot_in_the_retained_upload_cap"
POLLS = (
    "tests/ledger_invariants/test_exit_disputes.py::"
    "test_waiting_dispute_polls_never_starve_the_other_routes"
)

POLL_SLOT = (
    "        claim_watch(poll.party, pinned)\n"
    "        try:\n"
    "            future = watch_pool.submit("
    "disputes.events, poll.party, poll.cursor, timeout=poll.wait)\n"
    "        except BaseException:\n"
    "            release_watch(poll.party, pinned)\n"
    "            raise\n"
    "        # The slot lasts as long as the thread: "
    "a client that disconnects does not stop the wait.\n"
    "        future.add_done_callback(lambda _: release_watch(poll.party, pinned))\n"
    "        events = await asyncio.wrap_future(future)\n"
)

MUTANTS = [
    {
        "id": "closed-p68-the-retained-upload-cap-counts-every-record",
        "invariants": ["LV-03", "TRN-02"],
        "path": CORE,
        "old": "sum(self.held(u, now) for u in state.uploads.values()) >= self.max_uploads",
        "new": "len(state.uploads) >= self.max_uploads",
        "tests": [SLOT_FREED],
        "why": (
            "The cap counts every upload the journal still holds, released or not, so finished "
            "rounds keep the relay from granting the next upload."
        ),
    },
    {
        "id": "closed-p68-a-released-upload-is-still-held",
        "invariants": ["LV-03", "TRN-02"],
        "path": CORE,
        "old": (
            "return any(p.delete_after is None and p.horizon >= now for p in record.pins.values())"
        ),
        "new": "return any(p.horizon >= now for p in record.pins.values())",
        "tests": [SLOT_FREED],
        "why": (
            "An upload whose custody the master released is held until its horizon, so its slot "
            "in the cap is not freed by the release."
        ),
    },
    {
        "id": "closed-p68-the-shipped-relay-has-no-settlement-view",
        "invariants": ["LV-03", "TRN-02"],
        "path": RELAY_APP,
        "old": (
            "        relay.settlement = settlement_from_master(relay, client, config.master_url)\n"
        ),
        "new": "",
        "tests": [SHIPPED_RELAY],
        "why": (
            "The relay the entry point builds is not given the master's settlement view, so it "
            "refuses every release for want of a settlement journal and holds the bytes for ever."
        ),
    },
    {
        "id": "closed-p68-a-captured-watch-never-expires",
        "invariants": ["SG-05", "HTP-03"],
        "path": APP,
        "old": 'Intake(run_id, {"DisputeWatchRequest": poll.party.__eq__}).accept(raw, now)',
        "new": 'Intake(run_id, {"DisputeWatchRequest": poll.party.__eq__}).accept(raw, 1)',
        "tests": [WATCH_REPLAY],
        "why": (
            "The signed body of a dispute watch is checked against round 1 and not the current "
            "one, so a request captured once opens the watch of its party for ever."
        ),
    },
    {
        "id": "closed-p68-the-upload-admission-waits-for-the-lock-on-the-event-loop",
        "invariants": ["LV-02", "ADM-04"],
        "path": APP,
        "old": "        now, limit = await run(admitted)\n",
        "new": "        now, limit = admitted()\n",
        "tests": [UPLOAD_OFF_LOOP],
        "why": (
            "The admission check of a signed upload runs on the event loop thread, so a busy "
            "store freezes every route of the service while it waits for the lock."
        ),
    },
    {
        "id": "closed-p68-waiting-polls-run-on-the-shared-request-pool",
        "invariants": ["DS-08"],
        "path": APP,
        "old": POLL_SLOT,
        "new": (
            "        events = await run(\n"
            "            lambda: disputes.events(poll.party, poll.cursor, timeout=poll.wait)\n"
            "        )\n"
        ),
        "tests": [POLLS],
        "why": (
            "The long polls of a watcher take the threads of the shared request pool, so a few "
            "parked polls make every other route wait."
        ),
    },
]
