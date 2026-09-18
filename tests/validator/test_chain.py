from types import SimpleNamespace

import pytest

from cortex.protocol import ProtocolError
from cortex.protocol.crypto import encode_hotkey
from cortex.validator.chain import BittensorChain, close_subtensor
from cortex.validator.service import DispatchNotBroadcast, DispatchUncertain


def test_close_subtensor_handles_official_sdk_fallback_cleanup_bug():
    from bittensor import Subtensor

    subtensor = Subtensor(
        network="finney", fallback_endpoints=["wss://fallback.invalid"], mock=True
    )

    close_subtensor(subtensor)


def test_close_subtensor_does_not_mask_unrelated_attribute_errors():
    class BrokenSubtensor:
        def close(self):
            raise AttributeError("unrelated SDK failure")

    with pytest.raises(AttributeError, match="unrelated SDK failure"):
        close_subtensor(BrokenSubtensor())


@pytest.fixture
def sdk(monkeypatch):
    """Replace the public SDK boundary; no network and no extrinsics in CI."""

    class SdkRequestError(Exception):
        pass

    class Subtensor:
        enabled = True
        version = 4
        min_weights = 1
        max_weight = 1.0
        permits = [True]
        blocks_since = 101
        rate_limit = 100
        success = True
        extrinsic = object()
        receipt = SimpleNamespace(is_success=True, error_message=None)
        error = None
        exception = None
        calls = []
        encrypted_calls = []

        def get_uid_for_hotkey_on_subnet(self, hotkey, netuid, *, block):
            return 0

        def get_current_block(self):
            return 123

        def commit_reveal_enabled(self, *, netuid, block):
            return self.enabled

        def min_allowed_weights(self, netuid, *, block):
            return self.min_weights

        def max_weight_limit(self, netuid, *, block):
            return self.max_weight

        def get_subnet_validator_permits(self, netuid, *, block):
            return self.permits

        def blocks_since_last_update(self, netuid, uid, *, block):
            return self.blocks_since

        def weights_rate_limit(self, netuid, *, block):
            return self.rate_limit

        def query_subtensor(self, *, name, params, block):
            assert name == "CommitRevealWeightsVersion"
            return SimpleNamespace(value=self.version)

        def get_subnet_hyperparameters(self, netuid, *, block):
            return SimpleNamespace(commit_reveal_period=1)

        def get_epoch_schedule_state(self, netuid, *, block):
            return SimpleNamespace(
                last_epoch_block=100,
                pending_epoch_at=200,
                subnet_epoch_index=5,
                tempo=100,
                blocks_since_last_step=23,
                current_block=123,
            )

        def sign_and_send_extrinsic(self, **kwargs):
            self.calls.append(kwargs)
            if self.exception is not None:
                raise self.exception
            return SimpleNamespace(
                success=self.success,
                extrinsic=self.extrinsic,
                extrinsic_receipt=self.receipt,
                error=self.error,
            )

    class Pallet:
        def __init__(self, subtensor):
            self.subtensor = subtensor

        def set_mechanism_weights(self, **kwargs):
            return {"method": "set_mechanism_weights", **kwargs}

        def commit_timelocked_mechanism_weights(self, **kwargs):
            return {"method": "commit_timelocked_mechanism_weights", **kwargs}

    def encrypted_commit(**kwargs):
        subtensor.encrypted_calls.append(kwargs)
        return b"exact-sealed-vector", 77

    wallet = SimpleNamespace(
        hotkey=SimpleNamespace(
            public_key=bytes([1]) * 32, ss58_address=encode_hotkey(bytes([1]) * 32)
        )
    )
    subtensor = Subtensor()
    monkeypatch.setattr("bittensor.core.extrinsics.pallets.SubtensorModule", Pallet)
    monkeypatch.setattr("bittensor_drand.get_encrypted_commit_v2", encrypted_commit)
    return SimpleNamespace(
        chain=BittensorChain(subtensor, wallet),
        subtensor=subtensor,
        request_error=SdkRequestError,
        wallet=wallet,
    )


async def test_dispatch_passes_the_exact_sealed_vector_to_the_official_sdk(sdk):
    vector = ((1, 32768), (2, 19660), (3, 13107))
    assert await sdk.chain.submit(541, vector, 3)
    assert sdk.subtensor.encrypted_calls == [
        {
            "uids": [1, 2, 3],
            "weights": [32768, 19660, 13107],
            "version_key": 3,
            "last_epoch_block": 100,
            "pending_epoch_at": 200,
            "subnet_epoch_index": 5,
            "tempo": 100,
            "blocks_since_last_step": 23,
            "current_block": 123,
            "subnet_reveal_period_epochs": 1,
            "block_time": 12.0,
            "hotkey": bytes([1]) * 32,
        }
    ]
    assert sdk.subtensor.calls == [
        {
            "call": {
                "method": "commit_timelocked_mechanism_weights",
                "netuid": 541,
                "mecid": 0,
                "commit": b"exact-sealed-vector",
                "reveal_round": 77,
                "commit_reveal_version": 4,
            },
            "wallet": sdk.wallet,
            "sign_with": "hotkey",
            "use_nonce": True,
            "nonce_key": "hotkey",
            "period": 128,
            "raise_error": False,
            "wait_for_inclusion": True,
            "wait_for_finalization": True,
        }
    ]


async def test_crv4_reads_chain_global_version_without_a_subnet_key(sdk):
    def global_version(*, name, params, block):
        assert name == "CommitRevealWeightsVersion"
        if params:
            raise ValueError("Storage function requires 0 parameters, 1 given")
        return SimpleNamespace(value=4)

    sdk.subtensor.query_subtensor = global_version

    assert await sdk.chain.submit(541, ((0, 65535),), 3)
    assert len(sdk.subtensor.calls) == 1


async def test_plain_weights_only_when_chain_disables_commit_reveal(sdk):
    sdk.subtensor.enabled = False
    assert await sdk.chain.submit(541, ((1, 32768), (2, 32768)), 3)
    assert sdk.subtensor.calls[0]["call"] == {
        "method": "set_mechanism_weights",
        "netuid": 541,
        "mecid": 0,
        "dests": [1, 2],
        "weights": [32768, 32768],
        "version_key": 3,
    }
    assert sdk.subtensor.encrypted_calls == []


async def test_preflight_pins_validator_uid_to_the_same_tip_as_chain_limits(sdk):
    calls = []

    def uid_at_block(hotkey, netuid, *, block):
        calls.append((hotkey, netuid, block))
        return 0

    sdk.subtensor.get_uid_for_hotkey_on_subnet = uid_at_block

    await sdk.chain.preflight(541, ((0, 65535),), 3)

    assert calls == [(sdk.wallet.hotkey.ss58_address, 541, 123)]


async def test_wrong_commit_reveal_version_aborts_without_dispatch(sdk):
    sdk.subtensor.version = 3
    with pytest.raises(DispatchNotBroadcast, match="version must be 4"):
        await sdk.chain.submit(541, ((0, 65535),), 3)
    assert sdk.subtensor.calls == []


async def test_unknown_commit_reveal_hyperparameters_abort_without_dispatch(sdk):
    sdk.subtensor.get_subnet_hyperparameters = lambda netuid, block: SimpleNamespace(
        commit_reveal_period=None
    )

    with pytest.raises(DispatchNotBroadcast, match="commit-reveal hyperparameters"):
        await sdk.chain.submit(541, ((0, 65535),), 3)

    assert sdk.subtensor.calls == []


async def test_inconsistent_commit_reveal_schedule_aborts_without_dispatch(sdk):
    sdk.subtensor.get_epoch_schedule_state = lambda netuid, block: SimpleNamespace(
        last_epoch_block=100,
        pending_epoch_at=200,
        subnet_epoch_index=5,
        tempo=100,
        blocks_since_last_step=23,
        current_block=block - 1,
    )

    with pytest.raises(DispatchNotBroadcast, match="commit-reveal schedule"):
        await sdk.chain.submit(541, ((0, 65535),), 3)

    assert sdk.subtensor.calls == []


async def test_unknown_commit_reveal_state_never_downgrades_to_public_weights(sdk):
    sdk.subtensor.enabled = None
    with pytest.raises(ProtocolError, match="commit-reveal state"):
        await sdk.chain.submit(541, ((0, 65535),), 3)
    assert sdk.subtensor.calls == []


async def test_vector_shorter_than_live_chain_minimum_never_dispatches(sdk):
    sdk.subtensor.min_weights = 2

    with pytest.raises(DispatchNotBroadcast, match="minimum requires 2"):
        await sdk.chain.submit(541, ((0, 0), (1, 65535)), 3)

    assert sdk.subtensor.calls == []


@pytest.mark.parametrize("maximum", [None, 0.5, 65535])
async def test_incompatible_live_chain_maximum_never_dispatches(sdk, maximum):
    sdk.subtensor.max_weight = maximum

    with pytest.raises(DispatchNotBroadcast, match="maximum weight limit"):
        await sdk.chain.submit(541, ((0, 65535),), 3)

    assert sdk.subtensor.calls == []


async def test_validator_without_live_permit_never_dispatches(sdk):
    sdk.subtensor.permits = [False]

    with pytest.raises(DispatchNotBroadcast, match="validator permit"):
        await sdk.chain.submit(541, ((0, 65535),), 3)

    assert sdk.subtensor.calls == []


async def test_live_weight_rate_limit_never_dispatches(sdk):
    sdk.subtensor.blocks_since = 100

    with pytest.raises(DispatchNotBroadcast, match="rate limit"):
        await sdk.chain.submit(541, ((0, 65535),), 3)

    assert sdk.subtensor.calls == []


async def test_sdk_error_without_a_receipt_is_ambiguous(sdk):
    sdk.subtensor.success = False
    sdk.subtensor.extrinsic = None
    sdk.subtensor.receipt = None
    sdk.subtensor.error = OSError("drand unavailable")

    with pytest.raises(DispatchUncertain, match="ambiguous"):
        await sdk.chain.submit(541, ((0, 65535),), 3)
    assert len(sdk.subtensor.calls) == 1


async def test_ambiguous_dispatch_exposes_sdk_extrinsic_identity_for_reconciliation(sdk):
    sdk.subtensor.success = False
    sdk.subtensor.receipt = None
    sdk.subtensor.error = TimeoutError("RPC disconnected after broadcast")
    sdk.subtensor.extrinsic = SimpleNamespace(
        extrinsic_hash=bytes.fromhex("ab" * 32),
        value={"signature": {"nonce": 17}},
    )

    with pytest.raises(DispatchUncertain) as caught:
        await sdk.chain.submit(541, ((0, 65535),), 3)

    assert caught.value.extrinsic_hash == "0x" + "ab" * 32
    assert caught.value.nonce == 17


async def test_finalized_dispatch_rejection_can_return_false(sdk):
    sdk.subtensor.success = False
    sdk.subtensor.receipt = SimpleNamespace(is_success=False, error_message={"name": "Rejected"})
    sdk.subtensor.error = {"name": "Rejected"}

    assert await sdk.chain.submit(541, ((0, 65535),), 3) is False


async def test_known_failure_before_extrinsic_creation_can_return_false(sdk):
    sdk.subtensor.success = False
    sdk.subtensor.extrinsic = None
    sdk.subtensor.receipt = None

    assert await sdk.chain.submit(541, ((0, 65535),), 3) is False


async def test_created_extrinsic_without_a_receipt_is_ambiguous(sdk):
    sdk.subtensor.success = False
    sdk.subtensor.receipt = None

    with pytest.raises(DispatchUncertain, match="ambiguous"):
        await sdk.chain.submit(541, ((0, 65535),), 3)


async def test_dispatch_error_without_a_receipt_is_ambiguous(sdk):
    sdk.subtensor.success = False
    sdk.subtensor.receipt = None
    sdk.subtensor.error = TimeoutError("RPC disconnected after broadcast")

    with pytest.raises(DispatchUncertain, match="ambiguous"):
        await sdk.chain.submit(541, ((0, 65535),), 3)


async def test_sdk_exception_during_dispatch_is_ambiguous(sdk):
    sdk.subtensor.exception = TimeoutError("RPC disconnected after broadcast")

    with pytest.raises(DispatchUncertain, match="ambiguous"):
        await sdk.chain.submit(541, ((0, 65535),), 3)


async def test_sdk_error_response_with_a_receipt_is_still_ambiguous(sdk):
    sdk.subtensor.success = False
    sdk.subtensor.receipt = SimpleNamespace(is_success=False, error_message={"name": "Rejected"})
    sdk.subtensor.error = sdk.request_error("RPC failed while reading finalized events")

    with pytest.raises(DispatchUncertain, match="ambiguous"):
        await sdk.chain.submit(541, ((0, 65535),), 3)


async def test_sdk_request_error_during_dispatch_is_ambiguous(sdk):
    sdk.subtensor.exception = sdk.request_error("cannot read account nonce")

    with pytest.raises(DispatchUncertain, match="ambiguous"):
        await sdk.chain.submit(541, ((0, 65535),), 3)


async def test_success_without_a_final_receipt_is_ambiguous(sdk):
    sdk.subtensor.receipt = None

    with pytest.raises(DispatchUncertain, match="ambiguous"):
        await sdk.chain.submit(541, ((0, 65535),), 3)


async def test_unreadable_receipt_is_ambiguous(sdk):
    class Receipt:
        @property
        def is_success(self):
            raise TimeoutError("RPC disconnected while reading events")

    sdk.subtensor.success = False
    sdk.subtensor.receipt = Receipt()

    with pytest.raises(DispatchUncertain, match="ambiguous"):
        await sdk.chain.submit(541, ((0, 65535),), 3)


async def test_verify_only_preflight_does_not_build_or_submit_an_extrinsic(sdk):
    await sdk.chain.preflight(541, ((0, 65535),), 3)

    assert sdk.subtensor.calls == []


async def test_unregistered_validator_is_known_not_broadcast(sdk):
    sdk.subtensor.get_uid_for_hotkey_on_subnet = lambda hotkey, netuid, *, block: None

    with pytest.raises(DispatchNotBroadcast, match="not registered"):
        await sdk.chain.submit(541, ((0, 65535),), 3)
    assert sdk.subtensor.calls == []


async def test_reorganization_during_historical_snapshot_aborts():
    hashes = iter(["0x" + "01" * 32, "0x" + "02" * 32])
    key = encode_hotkey(bytes([1]) * 32)
    subtensor = SimpleNamespace(
        get_block_hash=lambda block: next(hashes),
        neurons_lite=lambda netuid, *, block: [],
        get_subnet_owner_hotkey=lambda netuid, *, block: key,
        get_subnet_epoch_index=lambda netuid, *, block: 1,
    )
    with pytest.raises(ProtocolError, match="reorganized"):
        await BittensorChain(subtensor, None).snapshot(123, 541)


async def test_snapshot_uses_one_hash_for_integer_stakes_uids_permits_and_epoch():
    calls = []
    digest = "0x" + "04" * 32
    owner = encode_hotkey(bytes([1]) * 32)
    miner = encode_hotkey(bytes([2]) * 32)

    def neurons_lite(netuid, *, block):
        calls.append(("neurons_lite", netuid, block))
        return [
            SimpleNamespace(uid=1, hotkey=miner, stake=18, validator_permit=False),
            SimpleNamespace(uid=0, hotkey=owner, stake=1_000_000_001, validator_permit=True),
        ]

    def subnet_owner(netuid, *, block):
        calls.append(("get_subnet_owner_hotkey", netuid, block))
        return owner

    def epoch_index(netuid, *, block):
        calls.append(("get_subnet_epoch_index", netuid, block))
        return 12

    subtensor = SimpleNamespace(
        get_block_hash=lambda _: digest,
        neurons_lite=neurons_lite,
        get_subnet_owner_hotkey=subnet_owner,
        get_subnet_epoch_index=epoch_index,
    )
    view = await BittensorChain(subtensor, None).snapshot(99, 541)
    assert view.epoch == 12 and view.validator_permits == frozenset({0})
    assert [(row.uid, row.stake) for row in view.rows] == [(1, 18), (0, 1_000_000_001)]
    assert calls == [
        ("neurons_lite", 541, 99),
        ("get_subnet_owner_hotkey", 541, 99),
        ("get_subnet_epoch_index", 541, 99),
    ]
