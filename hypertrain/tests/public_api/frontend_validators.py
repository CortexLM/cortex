"""Hand port of the frontend validators (hypertrain.ts isRun, isRound, isCluster,
isParticipant, isPoint, isSnapshot) plus the B4 encoder/decoder union for `isArchitecture`."""

from __future__ import annotations

import math
from collections.abc import Callable, Sequence
from typing import Any

STATUSES = ("training", "syncing", "offline", "finished")
QTYPES = ("choice", "score", "noul")
ARCH_NUMS = ("parameters", "layers", "dModel", "heads", "mlpHidden", "contextLength", "vocabSize")


def is_obj(v: Any) -> bool:
    return isinstance(v, dict)


def _num(v: Any) -> bool:
    return isinstance(v, (int, float)) and not isinstance(v, bool) and math.isfinite(v)


def nums(o: dict[str, Any], keys: Sequence[str]) -> bool:
    return all(_num(o.get(k)) for k in keys)


def strs(o: dict[str, Any], keys: Sequence[str]) -> bool:
    return all(isinstance(o.get(k), str) for k in keys)


def one_of(v: Any, allowed: Sequence[str]) -> bool:
    return isinstance(v, str) and v in allowed


def arr_of(v: Any, guard: Callable[[Any], bool]) -> bool:
    return isinstance(v, list) and all(guard(x) for x in v)


def is_run(v: Any) -> bool:
    return is_obj(v) and _is_run(v, v.get("outerOptimizer"))


def _is_run(v: dict[str, Any], outer: Any) -> bool:
    return (
        strs(v, ["id", "name", "model", "startedAt"])
        and ("endedAt" not in v or isinstance(v["endedAt"], str))
        and one_of(v.get("status"), ["scheduled", "running", "completed", "stopped"])
        and nums(v, ["innerSteps", "totalRounds", "currentRound", "tokensTrained", "targetTokens"])
        and is_obj(outer)
        and outer.get("type") == "nesterov"
        and nums(outer, ["lr", "momentum"])
        and one_of(v.get("pseudoGradDtype"), ["fp32", "fp16", "int8"])
    )


def is_round(v: Any) -> bool:
    return (
        is_obj(v)
        and isinstance(v.get("startedAt"), str)
        and ("syncedAt" not in v or isinstance(v["syncedAt"], str))
        and ("globalLoss" not in v or _num(v["globalLoss"]))
        and nums(
            v,
            [
                "index",
                "participants",
                "computeSeconds",
                "syncSeconds",
                "bytesExchanged",
                "bytesIfDDP",
            ],
        )
    )


def is_cluster(v: Any) -> bool:
    return (
        is_obj(v)
        and strs(v, ["id", "name", "region", "gpuType", "lastSyncAt"])
        and one_of(v.get("status"), STATUSES)
        and nums(
            v,
            [
                "nodes",
                "gpus",
                "innerStepsDone",
                "tokensPerSec",
                "lastSyncRound",
                "tokensContributed",
                "share",
            ],
        )
    )


def is_participant(v: Any) -> bool:
    return (
        is_obj(v)
        and strs(v, ["id", "label", "clusterId", "gpuType"])
        and ("uid" not in v or _num(v["uid"]))
        and ("hotkey" not in v or isinstance(v["hotkey"], str))
        and one_of(v.get("status"), STATUSES)
        and nums(
            v,
            [
                "gpus",
                "joinedRound",
                "roundsContributed",
                "tokensContributed",
                "tokensPerSec",
                "share",
            ],
        )
    )


def is_point(v: Any) -> bool:
    return is_obj(v) and isinstance(v.get("t"), str) and nums(v, ["round", "step", "value"])


def is_decoder_arch(v: dict[str, Any]) -> bool:
    return (
        v.get("family") == "decoder-only transformer"
        and nums(v, ARCH_NUMS)
        and is_obj(v.get("innerOptimizer"))
        and v["innerOptimizer"].get("type") == "adamw"
        and v.get("norm") == "RMSNorm"
        and v.get("activation") == "SwiGLU"
        and v.get("positional") == "RoPE"
        and isinstance(v.get("tiedEmbeddings"), bool)
    )


def is_encoder_arch(v: dict[str, Any]) -> bool:
    dh = v.get("decisionHead")
    return (
        v.get("family") == "encoder + decision head"
        and nums(v, ARCH_NUMS)
        and v.get("norm") == "LayerNorm"
        and v.get("activation") == "GELU"
        and v.get("positional") == "RoPE"
        and v.get("pretrainObjective") == "mlm"
        and v.get("calibration") == "temperature"
        and is_obj(dh)
        and nums(dh, ["layers"])
        and isinstance(dh.get("unknownSlot"), bool)
        and isinstance(dh.get("trainedInThisRun"), bool)
        and arr_of(dh.get("questionTypes"), lambda q: one_of(q, QTYPES))
    )


def is_architecture(v: Any, *, present: bool = True) -> bool:
    """`present=False` models JS `undefined` (key absent); a JSON null is never valid."""
    if not present:
        return True
    return is_obj(v) and (is_decoder_arch(v) or is_encoder_arch(v))


def is_snapshot(v: Any) -> bool:
    if not is_obj(v) or not is_obj(v.get("metrics")):
        return False
    return (
        is_architecture(v.get("architecture"), present="architecture" in v)
        and is_run(v.get("run"))
        and arr_of(v.get("rounds"), is_round)
        and arr_of(v.get("clusters"), is_cluster)
        and arr_of(v.get("participants"), is_participant)
        and arr_of(v["metrics"].get("loss"), is_point)
    )
