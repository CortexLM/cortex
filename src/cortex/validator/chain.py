"""Bittensor SDK adapter with exact u16 submissions and CRv4 fail-closed dispatch."""

import asyncio
from importlib import import_module
from typing import Any

from cortex.protocol import MetagraphRow, ProtocolError
from cortex.protocol.crypto import decode_hotkey
from cortex.protocol.scale import uint

from .service import ChainSnapshot


class BittensorChain:
    """Optional ``chain`` dependency; no chain calls are made by constructing this adapter."""

    def __init__(self, subtensor: Any, wallet: Any, *, block_time: float = 12.0):
        self.subtensor = subtensor
        self.wallet = wallet
        self.block_time = block_time

    async def current_block(self) -> int:
        block = await asyncio.to_thread(self.subtensor.get_current_block)
        uint(block, 8)
        return block

    async def current_epoch(self, netuid: int) -> int:
        epoch = await asyncio.to_thread(self.subtensor.get_subnet_epoch_index, netuid)
        uint(epoch, 8)
        return epoch

    async def snapshot(self, block: int, netuid: int) -> ChainSnapshot:
        return await asyncio.to_thread(self._snapshot, block, netuid)

    def _snapshot(self, block: int, netuid: int) -> ChainSnapshot:
        uint(block, 8)
        before = self.subtensor.get_block_hash(block)
        raw = self.subtensor.substrate.runtime_call(
            "NeuronInfoRuntimeApi", "get_neurons_lite", [netuid], block_hash=before
        )
        neurons = getattr(raw, "value", raw)
        owner_value = self.subtensor.substrate.query(
            "SubtensorModule", "SubnetOwnerHotkey", [netuid], block_hash=before
        )
        owner = getattr(owner_value, "value", owner_value)
        epoch_value = self.subtensor.substrate.query(
            "SubtensorModule", "SubnetEpochIndex", [netuid], block_hash=before
        )
        epoch = getattr(epoch_value, "value", epoch_value)
        uint(epoch, 8)
        after = self.subtensor.get_block_hash(block)
        if before != after:
            raise ProtocolError("chain reorganized during metagraph read")
        if owner is None or not isinstance(neurons, list):
            raise ProtocolError("incomplete chain owner/validator metadata")
        rows = []
        permits = set()
        for neuron in neurons:
            if type(neuron["validator_permit"]) is not bool:
                raise ProtocolError("invalid validator permit metadata")
            stake = 0
            for _, amount in neuron["stake"]:
                uint(amount, 8)
                stake += amount
            row = MetagraphRow(decode_hotkey(neuron["hotkey"]), neuron["uid"], stake)
            row.encode()
            rows.append(row)
            if neuron["validator_permit"]:
                permits.add(row.uid)
        return ChainSnapshot(
            block,
            bytes.fromhex(before.removeprefix("0x")),
            tuple(rows),
            decode_hotkey(owner),
            frozenset(permits),
            epoch,
        )

    async def submit(
        self, netuid: int, vector: tuple[tuple[int, int], ...], version_key: int
    ) -> bool:
        return await asyncio.to_thread(self._submit, netuid, vector, version_key)

    def _submit(self, netuid: int, vector: tuple[tuple[int, int], ...], version_key: int) -> bool:
        # SDK set_weights re-normalizes by maximum and would change the sealed u16
        # vector. Compose exact pallet calls, retaining the SDK's signing/dispatch.
        from bittensor.core.extrinsics.pallets import SubtensorModule

        if not vector or tuple(sorted(dict(vector).items())) != vector:
            raise ProtocolError("invalid chain vector")
        for uid, value in vector:
            uint(uid, 2)
            uint(value, 2)
        uint(netuid, 2)
        uint(version_key, 8)
        if (
            self.subtensor.get_uid_for_hotkey_on_subnet(self.wallet.hotkey.ss58_address, netuid)
            is None
        ):
            raise ProtocolError("validator hotkey is not registered")
        tip = self.subtensor.get_current_block()
        enabled = self.subtensor.commit_reveal_enabled(netuid=netuid, block=tip)
        if type(enabled) is not bool:
            raise ProtocolError("unknown chain commit-reveal state")
        pallet = SubtensorModule(self.subtensor)
        uids = [uid for uid, _ in vector]
        values = [value for _, value in vector]
        if enabled:
            # Optional native extension; it has no typing metadata.
            get_encrypted_commit_v2 = import_module("bittensor_drand").get_encrypted_commit_v2

            version = self.subtensor.query_subtensor(
                name="CommitRevealWeightsVersion", params=[netuid], block=tip
            )
            if getattr(version, "value", version) != 4:
                raise ProtocolError("commit_reveal_version must be 4")
            hyperparameters = self.subtensor.get_subnet_hyperparameters(netuid, block=tip)
            schedule = self.subtensor.get_epoch_schedule_state(netuid, block=tip)
            # Failure to obtain a timelock never falls back to public set_weights.
            commit, reveal_round = get_encrypted_commit_v2(
                uids=uids,
                weights=values,
                version_key=version_key,
                last_epoch_block=schedule.last_epoch_block,
                pending_epoch_at=schedule.pending_epoch_at,
                subnet_epoch_index=schedule.subnet_epoch_index,
                tempo=schedule.tempo,
                blocks_since_last_step=schedule.blocks_since_last_step,
                current_block=schedule.current_block,
                subnet_reveal_period_epochs=hyperparameters.commit_reveal_period,
                block_time=self.block_time,
                hotkey=self.wallet.hotkey.public_key,
            )
            call = pallet.commit_timelocked_mechanism_weights(
                netuid=netuid,
                mecid=0,
                commit=commit,
                reveal_round=reveal_round,
                commit_reveal_version=4,
            )
        else:
            call = pallet.set_mechanism_weights(
                netuid=netuid, mecid=0, dests=uids, weights=values, version_key=version_key
            )
        response = self.subtensor.sign_and_send_extrinsic(
            call=call,
            wallet=self.wallet,
            wait_for_inclusion=True,
            wait_for_finalization=True,
            use_nonce=True,
            sign_with="hotkey",
            nonce_key="hotkey",
            raise_error=False,
        )
        return bool(response.success)
