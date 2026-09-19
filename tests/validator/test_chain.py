import sys
from types import ModuleType, SimpleNamespace

import pytest

from cortex.protocol import ProtocolError
from cortex.protocol.crypto import encode_hotkey
from cortex.validator.chain import BittensorChain


@pytest.fixture
def sdk(monkeypatch):
    """Replace SDK/RPC and drand boundaries; no network and no extrinsics in CI."""
    calls = []
    encrypted = []

    class Pallet:
        def __init__(self, subtensor):
            pass

        def set_mechanism_weights(self, **kwargs):
            calls.append(("set", kwargs))
            return kwargs

        def commit_timelocked_mechanism_weights(self, **kwargs):
            calls.append(("commit", kwargs))
            return kwargs

    pallet_module = ModuleType("bittensor.core.extrinsics.pallets")
    pallet_module.SubtensorModule = Pallet
    monkeypatch.setitem(sys.modules, "bittensor.core.extrinsics.pallets", pallet_module)

    def encrypt(**kwargs):
        encrypted.append(kwargs)
        return b"encrypted-commit", 500

    drand = ModuleType("bittensor_drand")
    drand.get_encrypted_commit_v2 = encrypt
    monkeypatch.setitem(sys.modules, "bittensor_drand", drand)

    class Subtensor:
        enabled = True
        version = 4
        sent = []

        def get_uid_for_hotkey_on_subnet(self, hotkey, netuid):
            return 0

        def get_current_block(self):
            return 123

        def commit_reveal_enabled(self, *, netuid, block):
            return self.enabled

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
            self.sent.append(kwargs)
            return SimpleNamespace(success=True)

    wallet = SimpleNamespace(
        hotkey=SimpleNamespace(
            public_key=bytes([1]) * 32, ss58_address=encode_hotkey(bytes([1]) * 32)
        )
    )
    subtensor = Subtensor()
    return SimpleNamespace(
        chain=BittensorChain(subtensor, wallet),
        subtensor=subtensor,
        calls=calls,
        encrypted=encrypted,
        drand=drand,
    )


async def test_crv4_encrypts_exact_sealed_u16_values_without_sdk_renormalization(sdk):
    vector = ((1, 32768), (2, 19660), (3, 13107))
    assert await sdk.chain.submit(541, vector, 3)
    assert sdk.encrypted[0]["weights"] == [32768, 19660, 13107]
    assert sdk.encrypted[0]["uids"] == [1, 2, 3]
    assert "merkle_root" not in sdk.encrypted[0]
    assert sdk.calls == [
        (
            "commit",
            dict(
                netuid=541,
                mecid=0,
                commit=b"encrypted-commit",
                reveal_round=500,
                commit_reveal_version=4,
            ),
        )
    ]
    assert sdk.subtensor.sent[0]["sign_with"] == "hotkey"
    assert sdk.subtensor.sent[0]["wait_for_finalization"] is True


async def test_plain_weights_only_when_chain_disables_commit_reveal(sdk):
    sdk.subtensor.enabled = False
    assert await sdk.chain.submit(541, ((1, 32768), (2, 32768)), 3)
    assert sdk.encrypted == []
    assert sdk.calls == [
        ("set", dict(netuid=541, mecid=0, dests=[1, 2], weights=[32768, 32768], version_key=3))
    ]


async def test_wrong_commit_reveal_version_aborts_without_dispatch(sdk):
    sdk.subtensor.version = 3
    with pytest.raises(ProtocolError, match="version must be 4"):
        await sdk.chain.submit(541, ((0, 65535),), 3)
    assert sdk.calls == []
    assert sdk.encrypted == []


async def test_unknown_commit_reveal_state_never_downgrades_to_public_weights(sdk):
    sdk.subtensor.enabled = None
    with pytest.raises(ProtocolError, match="commit-reveal state"):
        await sdk.chain.submit(541, ((0, 65535),), 3)
    assert sdk.calls == []


async def test_drand_outage_never_falls_back_to_public_weights(sdk):
    def outage(**kwargs):
        raise OSError("drand unavailable")

    sdk.drand.get_encrypted_commit_v2 = outage
    with pytest.raises(OSError):
        await sdk.chain.submit(541, ((0, 65535),), 3)
    assert sdk.calls == []


async def test_reorganization_during_historical_snapshot_aborts():
    hashes = iter(["0x" + "01" * 32, "0x" + "02" * 32])
    key = encode_hotkey(bytes([1]) * 32)
    subtensor = SimpleNamespace(
        get_block_hash=lambda block: next(hashes),
        substrate=SimpleNamespace(
            runtime_call=lambda *args, **kwargs: [],
            query=lambda module, name, *args, **kwargs: 1 if name == "SubnetEpochIndex" else key,
        ),
    )
    with pytest.raises(ProtocolError, match="reorganized"):
        await BittensorChain(subtensor, None).snapshot(123, 541)


async def test_snapshot_uses_one_hash_for_integer_stakes_uids_permits_and_epoch():
    calls = []
    digest = "0x" + "04" * 32
    owner = encode_hotkey(bytes([1]) * 32)
    miner = encode_hotkey(bytes([2]) * 32)

    def runtime(api, method, params, *, block_hash):
        calls.append((api, method, params, block_hash))
        return SimpleNamespace(
            value=[
                {
                    "uid": 1,
                    "hotkey": miner,
                    "stake": [(owner, 7), (miner, 11)],
                    "validator_permit": False,
                },
                {
                    "uid": 0,
                    "hotkey": owner,
                    "stake": [(owner, 1_000_000_001)],
                    "validator_permit": True,
                },
            ]
        )

    def query(module, name, params, *, block_hash):
        calls.append((module, name, params, block_hash))
        return SimpleNamespace(value=12 if name == "SubnetEpochIndex" else owner)

    subtensor = SimpleNamespace(
        get_block_hash=lambda _: digest,
        substrate=SimpleNamespace(runtime_call=runtime, query=query),
    )
    view = await BittensorChain(subtensor, None).snapshot(99, 541)
    assert view.epoch == 12 and view.validator_permits == frozenset({0})
    assert [(row.uid, row.stake) for row in view.rows] == [(1, 18), (0, 1_000_000_001)]
    assert {call[-1] for call in calls} == {digest}
    assert len(calls) == 3
