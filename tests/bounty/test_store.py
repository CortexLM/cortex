"""Persistence guarantees that span independent service processes."""

import sqlite3
from concurrent.futures import ThreadPoolExecutor
from threading import Barrier

import pytest

from cortex.bounty import BountyStore
from cortex.bounty.store import StoreError


def test_two_connections_cannot_race_the_same_hotkey_rate_limit(tmp_path):
    path = tmp_path / "bounty.sqlite3"
    stores = [BountyStore(path), BountyStore(path)]
    barrier = Barrier(2)
    pairing = {"miner_hotkey": "ab" * 32, "account_id": "account"}

    def submit(index):
        barrier.wait(timeout=5)
        try:
            stores[index].insert_report(pairing, f"title {index}", "body", "reproduce", 100)
            return 201
        except StoreError as exc:
            return exc.status

    try:
        with ThreadPoolExecutor(max_workers=2) as pool:
            outcomes = list(pool.map(submit, range(2)))

        assert sorted(outcomes) == [201, 429]
        assert len(stores[0].list_reports()) == 1
    finally:
        for store in stores:
            store.close()


def test_two_connections_cannot_consume_the_same_pairing_nonce(tmp_path):
    path = tmp_path / "bounty.sqlite3"
    stores = [BountyStore(path), BountyStore(path)]
    barrier = Barrier(2)
    stores[0].grant_pair("account", "ab" * 32, expires_at=200, now=100)

    def bind(index):
        barrier.wait(timeout=5)
        try:
            result = stores[index].bind_pair("account", "ab" * 32, "12" * 16, 100, b"s" * 32)
            return 201, result
        except StoreError as exc:
            return exc.status, {"error": str(exc)}

    try:
        with ThreadPoolExecutor(max_workers=2) as pool:
            outcomes = list(pool.map(bind, range(2)))

        assert sorted(status for status, _ in outcomes) == [201, 409]
        refusal = next(body for status, body in outcomes if status == 409)
        assert refusal == {"error": "nonce reused"}
        accepted = next(body for status, body in outcomes if status == 201)
        assert stores[0].lookup_session(accepted["session"], b"s" * 32)["account_id"] == "account"
    finally:
        for store in stores:
            store.close()


def test_failed_session_insert_rolls_back_the_grant_and_nonce_across_restart(tmp_path):
    path = tmp_path / "bounty.sqlite3"
    store = BountyStore(path)
    store.grant_pair("account", "ab" * 32, expires_at=200, now=100)
    with sqlite3.connect(path) as connection:
        connection.execute(
            "CREATE TRIGGER fail_session BEFORE INSERT ON bounty_sessions "
            "BEGIN SELECT RAISE(ABORT, 'session write failed'); END"
        )
    try:
        with pytest.raises(sqlite3.IntegrityError, match="session write failed"):
            store.bind_pair("account", "ab" * 32, "12" * 16, 100, b"s" * 32)
    finally:
        store.close()
    with sqlite3.connect(path) as connection:
        connection.execute("DROP TRIGGER fail_session")

    restarted = BountyStore(path)
    try:
        result = restarted.bind_pair("account", "ab" * 32, "12" * 16, 100, b"s" * 32)

        assert restarted.lookup_session(result["session"], b"s" * 32)["account_id"] == "account"
        with pytest.raises(StoreError, match="nonce reused") as repeated:
            restarted.bind_pair("account", "ab" * 32, "12" * 16, 100, b"s" * 32)
        assert repeated.value.status == 409
        with pytest.raises(StoreError, match="pairing not authorized") as spent_grant:
            restarted.bind_pair("account", "ab" * 32, "34" * 16, 100, b"s" * 32)
        assert spent_grant.value.status == 403
    finally:
        restarted.close()


def test_two_connections_cannot_consume_one_pair_grant_with_different_nonces(tmp_path):
    path = tmp_path / "bounty.sqlite3"
    stores = [BountyStore(path), BountyStore(path)]
    barrier = Barrier(2)
    stores[0].grant_pair("account", "ab" * 32, expires_at=200, now=100)

    def bind(index):
        barrier.wait(timeout=5)
        try:
            stores[index].bind_pair("account", "ab" * 32, f"{index + 1:02x}" * 16, 100, b"s" * 32)
            return 201
        except StoreError as exc:
            return exc.status

    try:
        with ThreadPoolExecutor(max_workers=2) as pool:
            outcomes = list(pool.map(bind, range(2)))

        assert sorted(outcomes) == [201, 403]
    finally:
        for store in stores:
            store.close()
