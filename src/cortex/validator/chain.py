"""Bittensor SDK adapter with fail-closed preflight and dispatch."""

import asyncio
import functools
import logging
from dataclasses import dataclass
from typing import Any, cast

from cortex.protocol import MetagraphRow, ProtocolError
from cortex.protocol.crypto import decode_hotkey
from cortex.protocol.scale import uint

from .service import ChainSnapshot, DispatchNotBroadcast, DispatchUncertain


@dataclass(frozen=True)
class _DispatchPlan:
    commit_reveal: bool
    reveal_period: int | None = None
    schedule: Any = None


def _dispatch_identity(response: object) -> tuple[str | None, int | None]:
    extrinsic = getattr(response, "extrinsic", None)
    try:
        raw_hash = getattr(extrinsic, "extrinsic_hash", None)
    except Exception:
        raw_hash = None
    extrinsic_hash: str | None
    if isinstance(raw_hash, bytes) and len(raw_hash) == 32:
        extrinsic_hash = "0x" + raw_hash.hex()
    elif isinstance(raw_hash, str) and len(raw_hash.removeprefix("0x")) == 64:
        candidate = raw_hash.removeprefix("0x").lower()
        extrinsic_hash = (
            "0x" + candidate if all(c in "0123456789abcdef" for c in candidate) else None
        )
    else:
        extrinsic_hash = None
    nonce: int | None = None
    try:
        value = getattr(extrinsic, "value", None)
        signature = value.get("signature") if isinstance(value, dict) else None
        nonce_candidate = signature.get("nonce") if isinstance(signature, dict) else None
        nonce_candidate = getattr(nonce_candidate, "value", nonce_candidate)
        if type(nonce_candidate) is int and 0 <= nonce_candidate <= 2**64 - 1:
            nonce = nonce_candidate
    except Exception:
        pass
    return extrinsic_hash, nonce


def close_subtensor(subtensor: Any) -> None:
    """Close the SDK client despite bittensor 10.5's fallback cleanup defect."""
    try:
        subtensor.close()
    except AttributeError as error:
        substrate = getattr(subtensor, "substrate", None)
        cache_names = (
            "get_runtime_for_version",
            "get_parent_block_hash",
            "get_block_runtime_info",
            "get_block_runtime_version_for",
            "supports_rpc_method",
            "_get_block_hash",
            "_cached_get_block_number",
        )
        retry_cache = any(
            isinstance(getattr(substrate, name, None), functools.partial) for name in cache_names
        )
        if not retry_cache or str(error) != (
            "'functools.partial' object has no attribute 'cache_clear'"
        ):
            raise
        logging.warning("Bittensor fallback connection closed with SDK cache cleanup defect")


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
        neurons = self.subtensor.neurons_lite(netuid, block=block)
        owner = self.subtensor.get_subnet_owner_hotkey(netuid, block=block)
        epoch = self.subtensor.get_subnet_epoch_index(netuid, block=block)
        uint(epoch, 8)
        after = self.subtensor.get_block_hash(block)
        if before != after:
            raise ProtocolError("chain reorganized during metagraph read")
        if owner is None or not isinstance(neurons, list):
            raise ProtocolError("incomplete chain owner/validator metadata")
        rows = []
        permits = set()
        for neuron in neurons:
            if type(neuron.validator_permit) is not bool:
                raise ProtocolError("invalid validator permit metadata")
            stake = int(neuron.stake)
            uint(stake, 8)
            row = MetagraphRow(decode_hotkey(neuron.hotkey), neuron.uid, stake)
            row.encode()
            rows.append(row)
            if neuron.validator_permit:
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

    async def preflight(
        self, netuid: int, vector: tuple[tuple[int, int], ...], version_key: int
    ) -> None:
        try:
            await asyncio.to_thread(self._preflight, netuid, vector, version_key)
        except Exception as error:
            raise DispatchNotBroadcast(str(error)) from error

    def _preflight(
        self, netuid: int, vector: tuple[tuple[int, int], ...], version_key: int
    ) -> _DispatchPlan:
        if not vector or tuple(sorted(dict(vector).items())) != vector:
            raise ProtocolError("invalid chain vector")
        for uid, value in vector:
            uint(uid, 2)
            uint(value, 2)
        uint(netuid, 2)
        uint(version_key, 8)
        tip = self.subtensor.get_current_block()
        uint(tip, 8)
        uid = self.subtensor.get_uid_for_hotkey_on_subnet(
            self.wallet.hotkey.ss58_address, netuid, block=tip
        )
        if type(uid) is not int:
            raise ProtocolError("validator hotkey is not registered")
        uint(uid, 2)
        min_allowed = self.subtensor.min_allowed_weights(netuid, block=tip)
        if type(min_allowed) is not int or not 0 <= min_allowed <= 65535:
            raise ProtocolError("unknown chain minimum weight count")
        positive_weights = sum(value > 0 for _, value in vector)
        if positive_weights < min_allowed:
            raise ProtocolError(
                f"sealed vector has {positive_weights} positive entries; "
                f"chain minimum requires {min_allowed}"
            )
        maximum = self.subtensor.max_weight_limit(netuid, block=tip)
        if type(maximum) is not float or maximum != 1.0:
            raise ProtocolError("chain maximum weight limit is incompatible")
        permits = self.subtensor.get_subnet_validator_permits(netuid, block=tip)
        if (
            not isinstance(permits, list)
            or uid >= len(permits)
            or any(type(permit) is not bool for permit in permits)
        ):
            raise ProtocolError("unknown chain validator permit state")
        if not permits[uid]:
            raise ProtocolError("validator hotkey has no validator permit")
        blocks_since = self.subtensor.blocks_since_last_update(netuid, uid, block=tip)
        rate_limit = self.subtensor.weights_rate_limit(netuid, block=tip)
        if (
            type(blocks_since) is not int
            or type(rate_limit) is not int
            or blocks_since < 0
            or rate_limit < 0
        ):
            raise ProtocolError("unknown chain weight rate limit state")
        if blocks_since <= rate_limit:
            raise ProtocolError("chain weight rate limit has not elapsed")
        enabled = self.subtensor.commit_reveal_enabled(netuid=netuid, block=tip)
        if type(enabled) is not bool:
            raise ProtocolError("unknown chain commit-reveal state")
        if not enabled:
            return _DispatchPlan(False)
        version = self.subtensor.query_subtensor(
            name="CommitRevealWeightsVersion", params=[], block=tip
        )
        version_value = getattr(version, "value", None)
        if type(version_value) is not int or version_value != 4:
            raise ProtocolError("commit_reveal_version must be 4")
        hyperparameters = self.subtensor.get_subnet_hyperparameters(netuid, block=tip)
        reveal_period = getattr(hyperparameters, "commit_reveal_period", None)
        if type(reveal_period) is not int or reveal_period < 1:
            raise ProtocolError("unknown chain commit-reveal hyperparameters")
        schedule = self.subtensor.get_epoch_schedule_state(netuid, block=tip)
        schedule_values = (
            getattr(schedule, "last_epoch_block", None),
            getattr(schedule, "pending_epoch_at", None),
            getattr(schedule, "subnet_epoch_index", None),
            getattr(schedule, "tempo", None),
            getattr(schedule, "blocks_since_last_step", None),
            getattr(schedule, "current_block", None),
        )
        if any(type(value) is not int or value < 0 for value in schedule_values):
            raise ProtocolError("unknown chain commit-reveal schedule")
        if cast(int, schedule_values[3]) < 1 or cast(int, schedule_values[5]) != tip:
            raise ProtocolError("incompatible chain commit-reveal schedule")
        return _DispatchPlan(True, reveal_period, schedule)

    def _submit(self, netuid: int, vector: tuple[tuple[int, int], ...], version_key: int) -> bool:
        try:
            plan = self._preflight(netuid, vector, version_key)
        except Exception as error:
            raise DispatchNotBroadcast(str(error)) from error

        uids = [uid for uid, _ in vector]
        weights = [weight for _, weight in vector]
        try:
            from bittensor.core.extrinsics.pallets import SubtensorModule

            pallet = SubtensorModule(self.subtensor)
            if plan.commit_reveal:
                from bittensor_drand import get_encrypted_commit_v2  # type: ignore[import-untyped]

                schedule = plan.schedule
                commit, reveal_round = get_encrypted_commit_v2(
                    uids=uids,
                    weights=weights,
                    version_key=version_key,
                    last_epoch_block=schedule.last_epoch_block,
                    pending_epoch_at=schedule.pending_epoch_at,
                    subnet_epoch_index=schedule.subnet_epoch_index,
                    tempo=schedule.tempo,
                    blocks_since_last_step=schedule.blocks_since_last_step,
                    current_block=schedule.current_block,
                    subnet_reveal_period_epochs=plan.reveal_period,
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
                    netuid=netuid,
                    mecid=0,
                    dests=uids,
                    weights=weights,
                    version_key=version_key,
                )
        except Exception as error:
            raise DispatchNotBroadcast(str(error)) from error

        response = None
        try:
            response = self.subtensor.sign_and_send_extrinsic(
                call=call,
                wallet=self.wallet,
                sign_with="hotkey",
                use_nonce=True,
                nonce_key="hotkey",
                period=128,
                raise_error=False,
                wait_for_inclusion=True,
                wait_for_finalization=True,
            )
            if type(response.success) is not bool:
                raise ValueError("invalid chain dispatch response")
            receipt = getattr(response, "extrinsic_receipt", None)
            extrinsic = getattr(response, "extrinsic", None)
            response_error = getattr(response, "error", None)
            if response.success:
                if receipt is None or response_error is not None or receipt.is_success is not True:
                    raise ValueError("ambiguous chain dispatch outcome requires reconciliation")
                return True
            if isinstance(response_error, BaseException):
                raise ValueError("ambiguous chain dispatch outcome requires reconciliation")
            if receipt is None:
                if extrinsic is None and response_error is None:
                    return False
                raise ValueError("ambiguous chain dispatch outcome requires reconciliation")
            receipt_success = receipt.is_success
            receipt_error = receipt.error_message
            if receipt_success is False and not isinstance(receipt_error, BaseException):
                return False
            raise ValueError("ambiguous chain dispatch outcome requires reconciliation")
        except Exception as error:
            extrinsic_hash, nonce = _dispatch_identity(response)
            raise DispatchUncertain(
                "ambiguous chain dispatch outcome requires reconciliation",
                extrinsic_hash=extrinsic_hash,
                nonce=nonce,
            ) from error
