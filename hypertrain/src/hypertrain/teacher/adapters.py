"""HF row -> OD question schema (A9/A10) and the to_record used by datasets.build.

DESIGN A9/A10 name the sources and the record shape but do not define per-source prompt
wording; the mappings below are the minimal natural ones (state text, instruction, options,
qtype=choice, gold index). Tulu-3 SFT rows have no options and no conversion is defined.
"""

from __future__ import annotations

from collections.abc import Callable
from dataclasses import dataclass
from typing import Any

import numpy as np
import numpy.typing as npt

from hypertrain.data.tokenizer import Tokenizer
from hypertrain.datasets.registry import OFFSET, Source
from hypertrain.teacher.config import TeacherConfig, TeacherError
from hypertrain.teacher.labeller import LabelCache, Question, label_question

CHOICE = 0  # opendecision.model.QTYPES["choice"]
Row = dict[str, Any]


def _mnli(r: Row) -> Question:
    # HF multi_nli label: 0 entailment, 1 neutral, 2 contradiction
    return Question(
        str(r["premise"]),
        f'Given the text, is this statement true? "{r["hypothesis"]}"',
        ("entailment", "neutral", "contradiction"),
        CHOICE,
        int(r["label"]) if r.get("label", -1) in (0, 1, 2) else None,
    )


def _boolq(r: Row) -> Question:
    return Question(
        str(r["passage"]),
        f"{str(r['question']).rstrip('?')}?",
        ("yes", "no"),
        CHOICE,
        0 if r["answer"] else 1,
    )


def _choices(r: Row) -> tuple[list[str], int | None]:
    c = r["choices"]
    labels, texts = list(c["label"]), [str(t) for t in c["text"]]
    key = r.get("answerKey")
    return texts, (labels.index(key) if key in labels else None)


def _csqa(r: Row) -> Question:
    opts, y = _choices(r)
    return Question(str(r["question"]), "Choose the best answer.", tuple(opts), CHOICE, y)


def _qasc(r: Row) -> Question:
    opts, y = _choices(r)
    state = " ".join(str(r[k]) for k in ("fact1", "fact2") if r.get(k))
    return Question(state or str(r["question"]), str(r["question"]), tuple(opts), CHOICE, y)


def _cosmos(r: Row) -> Question:
    opts = tuple(str(r[f"answer{i}"]) for i in range(4))
    lab = r.get("label")
    return Question(
        str(r["context"]),
        str(r["question"]),
        opts,
        CHOICE,
        int(lab) if lab in (0, 1, 2, 3) else None,
    )


def _tulu(r: Row) -> Question:
    raise NotImplementedError(
        "tulu-3-sft: DESIGN A9 defines no conversion of chat rows to OD questions "
        "(no options, no gold, no letter-choice teacher prompt); the design owner must "
        "specify one before od-b-v1 can be labelled"
    )


ADAPTERS: dict[str, Callable[[Row], Question]] = {
    "multi-nli": _mnli,
    "boolq": _boolq,
    "commonsense-qa": _csqa,
    "qasc": _qasc,
    "cosmos-qa": _cosmos,
    "tulu-3-sft": _tulu,
}


def to_question(source: Source, row: Row) -> Question:
    try:
        f = ADAPTERS[source.id]
    except KeyError:
        raise NotImplementedError(
            f"no teacher adapter for source {source.id!r} (only od-b-v1 sources)"
        ) from None
    return f(row)


@dataclass
class RecordStats:
    ok: int = 0
    failed: int = 0
    too_long: int = 0


class RecordBuilder:
    """Callable matching datasets.build.ToRecord. Reads the label cache only; no network."""

    def __init__(
        self,
        tokenizer: Tokenizer,
        record: dict[str, int],
        cfg: TeacherConfig,
        cache: LabelCache,
    ) -> None:
        from opendecision.records import RecordShape, qmax_for

        self.tok, self.cfg, self.cache = tokenizer, cfg, cache
        self.shape = RecordShape(**record)
        self.qmax = qmax_for(tokenizer.vocab_size + OFFSET)
        self.stats = RecordStats()

    def _ids(self, text: str) -> list[int]:
        return [t + OFFSET for t in self.tok.encode(text)]

    def __call__(self, source: Source, row: Row) -> npt.NDArray[np.uint32] | None:
        from opendecision.records import pack_record

        q = to_question(source, row)
        if len(q.options) > self.shape.n_options:
            self.stats.too_long += 1
            return None
        res = label_question(q, self.cfg, self.cache, None)
        if res.status == "missing":
            raise TeacherError(
                f"{source.id}: no cached teacher label for a question; run `label` first"
            )
        if res.probs is None:
            self.stats.failed += 1
            return None
        ex = {
            "state": self._ids(q.state),
            "instr": [self._ids(q.instruction)],
            "opts": [[self._ids(o) for o in q.options]],
            "qtype": [q.qtype],
            "y": [q.gold],
            "teacher": [res.probs],
        }
        try:
            rec = pack_record(ex, self.shape, self.qmax)
        except ValueError:  # state/instruction/option longer than the record field: skip, counted
            self.stats.too_long += 1
            return None
        self.stats.ok += 1
        return rec.astype(np.uint32)
