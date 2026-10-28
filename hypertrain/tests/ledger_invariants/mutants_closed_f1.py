"""Mutants that re-open the hole F1/D52 closed (a client event takes an id the service derives for
its own event, ADR 0014 part 1, fix fe5ce134 and the part of 5c6e02ee that makes every lookup and
every write name its key space): each one brings back, in one place, the shared key space of the
client ids and the ids the service derives.

The pins were strict expected failures until the fix landed and now hold the fix. These mutants
prove it on the merged tree: reverting one behaviour of the fix in a private copy of src must fail
them. Three mutants revert the shared mechanism (the key function, its prefix and the legacy-chain
fallback of the lookup); the others revert one operation family at a time, either its lookup or the
key its event is written under, because a service event written under a bare id lets a client
event of that id occupy it, and a lookup under a bare id finds that client event and refuses the
service.

The keyed origin ids of 5c6e02ee (the genesis, bootstrap, reward and shadow origin ids, derived
under the id key) have no mutant here. The nearest are the domain checks of the genesis and
bootstrap origin ids, mutated in mutants_credit.py (credit-genesis-origin-id-unbound) and
mutants_gaps_bootstrap.py; no mutant of the lot reverts the keyed derivation of the reward and
shadow origin ids.

Run from hypertrain/: .venv/bin/python scripts/check_ledger_invariant_mutants.py --only <ids>
"""

from __future__ import annotations

LEDGER = "src/hypertrain/ledger/escrow_v2.py"
DISPUTES = "src/hypertrain/challenge/disputes_v2.py"

OPIDS = "tests/ledger_invariants/test_opids.py::"
OPIDS_HTTP = "tests/ledger_invariants/test_opids_2.py::"
WEIGHTS = "tests/ledger_invariants/test_weights.py::"
ESCROW = "tests/ledger_invariants/test_gaps_escrow.py::"
BOOTSTRAP = "tests/ledger_invariants/test_gaps_bootstrap.py::"
DISPUTE_GAPS = "tests/ledger_invariants/test_gaps_disputes.py::"

REWARD_ID = OPIDS + "test_a_client_event_never_blocks_the_reward_finalize_id"
SHADOW_ID = OPIDS + "test_a_client_event_never_blocks_the_shadow_finalize_id"
PAYOUT_ID = OPIDS + "test_a_client_event_never_blocks_the_weights_payout_id"
SETTLEMENT_ID = OPIDS + "test_a_client_event_never_blocks_the_dispute_settlement_id"
MATURITY_ID = OPIDS_HTTP + "test_a_client_event_never_blocks_the_maturity_id_over_http"
PAY_ID = WEIGHTS + "test_a_client_event_carrying_the_pay_id_never_blocks_the_payout[True]"
SLASH_ID = ESCROW + "test_a_client_event_never_blocks_the_dispute_slash_id"
BEACON_CLOSE = ESCROW + "test_a_client_event_never_stops_the_beacon_from_closing_a_blocked_dispute"
CONVICTED = (
    ESCROW + "test_a_client_event_never_blocks_the_settlement_of_a_convicted_miner_over_http"
)
SQUATTED_MATURITY = (
    BOOTSTRAP + "test_a_squatted_maturity_id_never_blocks_another_owners_origin_or_the_next_push"
)
STUCK_CONTEST = (
    DISPUTE_GAPS + "test_a_client_event_cannot_stop_the_beacon_from_closing_a_stuck_contest"
)
# CONVICTED has two variants. The legacy fallback mutant fails both, the slash lookup mutant fails
# [resolution] only. The runner stops at the first failing test, so each mutant lists first the
# variant it is to prove.

MUTANTS = [
    {
        "id": "closed-f1-server-key-is-the-bare-id",
        "invariants": ["ESC-04", "SG-11"],
        "path": LEDGER,
        "old": (
            "        return SERVER_SCOPE"
            ' + hmac.digest(self.id_key, operation_id.encode(), "sha256").hex()\n'
        ),
        "new": "        return operation_id\n",
        "tests": [SLASH_ID, STUCK_CONTEST],
        "why": (
            "an event the service derives is stored and looked up under the bare id again, so a"
            " client event signed under that id makes the service operation fail"
            " OPERATION_CONFLICT."
        ),
    },
    {
        "id": "closed-f1-legacy-fallback-ignores-the-request",
        "invariants": ["ESC-04", "SG-11"],
        "path": LEDGER,
        "old": (
            '                "SELECT request,receipt FROM escrow_events WHERE operation_id=?'
            ' AND request=?",\n'
            "                (operation_id, request),\n"
        ),
        "new": (
            '                "SELECT request,receipt FROM escrow_events WHERE operation_id=?",\n'
            "                (operation_id,),\n"
        ),
        "tests": [CONVICTED + "[timeout]", BEACON_CLOSE],
        "why": (
            "the lookup that reads chains written before the key space accepts any event under the"
            " bare id, so a client event on the id the service derived is taken for the service's"
            " own and refused as a different body."
        ),
    },
    {
        "id": "closed-f1-server-keys-are-plain-hex64",
        "invariants": ["ESC-04", "SG-11"],
        "path": LEDGER,
        "old": 'SERVER_SCOPE = "srv:"\n',
        "new": 'SERVER_SCOPE = ""\n',
        "tests": [SLASH_ID, SQUATTED_MATURITY],
        "why": (
            "a server key without its prefix is a 64-hex digest again, a key space a client id can"
            " name, so a client that reaches the stored key signs an event the service then"
            " collides with."
        ),
    },
    {
        "id": "closed-f1-finalize-looks-up-the-bare-id",
        "invariants": ["ESC-04", "BND-24"],
        "path": LEDGER,
        "old": (
            "            old = self._existing(operation, request, server=True)\n"
            "            if old is not None:\n"
            "                return old\n"
            "            if origins is None:\n"
        ),
        "new": (
            "            old = self._existing(operation, request, server=False)\n"
            "            if old is not None:\n"
            "                return old\n"
            "            if origins is None:\n"
        ),
        "tests": [REWARD_ID, SHADOW_ID],
        "why": (
            "the reward and the shadow finalize look their event up under the bare public id, find"
            " the event a client signed under it and refuse the round OPERATION_CONFLICT."
        ),
    },
    {
        "id": "closed-f1-reward-finalize-stored-under-the-bare-id",
        "invariants": ["ESC-04", "BND-24"],
        "path": LEDGER,
        "old": (
            "                    return self._event(\n"
            '                        operation, "REWARD_FINALIZE", request, origins[0].owner,'
            " server=True\n"
            "                    )\n"
        ),
        "new": (
            "                    return self._event(\n"
            '                        operation, "REWARD_FINALIZE", request, origins[0].owner,'
            " server=False\n"
            "                    )\n"
        ),
        "tests": [REWARD_ID],
        "why": (
            "the reward finalize is written under the bare public id, which a client event may"
            " already hold, so its insert hits the unique event key and the round is not"
            " finalized."
        ),
    },
    {
        "id": "closed-f1-shadow-finalize-stored-under-the-bare-id",
        "invariants": ["ESC-04", "BND-24"],
        "path": LEDGER,
        "old": (
            "                    receipt = self._event(\n"
            '                        operation, "SHADOW_REWARD_FINALIZE", request,'
            " origins[0].owner, server=True\n"
            "                    )\n"
        ),
        "new": (
            "                    receipt = self._event(\n"
            '                        operation, "SHADOW_REWARD_FINALIZE", request,'
            " origins[0].owner, server=False\n"
            "                    )\n"
        ),
        "tests": [SHADOW_ID],
        "why": (
            "the shadow finalize is written under the bare public id, which a client event may"
            " already hold, so its insert hits the unique event key and the reservation is not"
            " finalized."
        ),
    },
    {
        "id": "closed-f1-pay-looks-up-the-bare-id",
        "invariants": ["ESC-04", "WGT-13"],
        "path": LEDGER,
        "old": "            old = self._owner_operation(op, signer, server=True)\n",
        "new": "            old = self._owner_operation(op, signer, server=False)\n",
        "tests": [PAYOUT_ID, PAY_ID],
        "why": (
            "the payout looks its event up under the bare id derived from public values, finds the"
            " client event signed under it and get_weights fails for the epoch."
        ),
    },
    {
        "id": "closed-f1-pay-stored-under-the-bare-id",
        "invariants": ["ESC-04", "WGT-13"],
        "path": LEDGER,
        "old": (
            '            return self._event(op.operation_id, "PAY", op.model_dump_json(),'
            " op.owner, server=True)\n"
        ),
        "new": (
            '            return self._event(op.operation_id, "PAY", op.model_dump_json(),'
            " op.owner, server=False)\n"
        ),
        "tests": [PAY_ID],
        "why": (
            "the payout is written under the bare id derived from public values, which a client"
            " event may already hold, so its insert hits the unique event key and the epoch is"
            " not paid."
        ),
    },
    {
        "id": "closed-f1-mature-looks-up-the-bare-id",
        "invariants": ["ESC-04", "BND-24"],
        "path": LEDGER,
        "old": (
            "            old = self._existing(operation_id, request, server=True)\n"
            "            if old is not None:\n"
            "                return old\n"
            '            owner = ""\n'
        ),
        "new": (
            "            old = self._existing(operation_id, request, server=False)\n"
            "            if old is not None:\n"
            "                return old\n"
            '            owner = ""\n'
        ),
        "tests": [MATURITY_ID, SQUATTED_MATURITY],
        "why": (
            "the maturity looks its event up under the bare id derived from the origin, finds the"
            " client event signed under it and the origin stays pending."
        ),
    },
    {
        "id": "closed-f1-mature-stored-under-the-bare-id",
        "invariants": ["ESC-04", "BND-24"],
        "path": LEDGER,
        "old": (
            '            return self._event(operation_id, "MATURE", request, owner, server=True)\n'
        ),
        "new": (
            '            return self._event(operation_id, "MATURE", request, owner, server=False)\n'
        ),
        "tests": [MATURITY_ID],
        "why": (
            "the maturity is written under the bare id derived from the origin, which a client"
            " event may already hold, so its insert hits the unique event key and the origin"
            " never vests."
        ),
    },
    {
        "id": "closed-f1-settlement-release-is-a-client-event",
        "invariants": ["ESC-04", "DS-07"],
        "path": LEDGER,
        "old": (
            '        """The service\'s release of a settled lock: its id is derived, so it is a'
            ' server event."""\n'
            "        return self._release(op, signer, now_beacon, server=True)\n"
        ),
        "new": (
            '        """The service\'s release of a settled lock: its id is derived, so it is a'
            ' server event."""\n'
            "        return self._release(op, signer, now_beacon, server=False)\n"
        ),
        "tests": [SETTLEMENT_ID, STUCK_CONTEST],
        "why": (
            "the release of a settled lock is keyed as a client event, so a client event signed"
            " under the public settlement id of the dispute keeps the bond locked."
        ),
    },
    {
        "id": "closed-f1-dispute-settles-with-the-owners-release",
        "invariants": ["ESC-04", "DIA-14"],
        "path": DISPUTES,
        "old": "self.escrow.release_settlement(release, signer=lock.owner, now_beacon=now)",
        "new": "self.escrow.release(release, signer=lock.owner, now_beacon=now)",
        "tests": [BEACON_CLOSE],
        "why": (
            "the dispute settles its bond through the owner's release API, which keys the event"
            " under the public settlement id, so a client event on that id stops the beacon from"
            " closing the contest."
        ),
    },
    {
        "id": "closed-f1-slash-looks-up-the-bare-id",
        "invariants": ["ESC-04", "SG-11"],
        "path": LEDGER,
        "old": "            old = self._existing(op.operation_id, request, server=True)\n",
        "new": "            old = self._existing(op.operation_id, request, server=False)\n",
        "tests": [CONVICTED + "[resolution]", SLASH_ID],
        "why": (
            "the slash of a convicted miner looks its event up under the bare public id, finds the"
            " client event signed under it and the conviction cannot burn the bond."
        ),
    },
    {
        "id": "closed-f1-slash-stored-under-the-bare-id",
        "invariants": ["ESC-04", "SG-11"],
        "path": LEDGER,
        "old": (
            '            return self._event(op.operation_id, "SLASH", request, op.owner,'
            " server=True)\n"
        ),
        "new": (
            '            return self._event(op.operation_id, "SLASH", request, op.owner,'
            " server=False)\n"
        ),
        "tests": [SLASH_ID],
        "why": (
            "the slash is written under the bare public id, which a client event may already hold,"
            " so its insert hits the unique event key and the convicted miner's bond is never"
            " burned."
        ),
    },
]
