"""Benchmark runner for parity checkpoints: thin cloze/generation evaluator + lm-eval adapter.

Backends:
  cloze   : in-process scorer on local JSONL items {"task", "id", "context", "choices", "gold"}
            (multiple choice, acc + acc_norm by byte length) or {"task", "id", "question",
            "answer"} (generative, greedy, exact match on the final number; GSM8K style).
            Used for the CPU smoke; items are read at eval time, never mirrored into the repo.
  lm-eval : ``HypertrainLM`` adapter for lm-evaluation-harness == LM_EVAL_VERSION (run with
            ``uv run --with lm-eval==0.4.13``); deferred to the gated GPU proxy. Datasets are
            fetched by the harness at eval time and used for evaluation only.
Task set: MMLU (cloze below 3B), HellaSwag, ARC-Challenge/Easy, GSM8K.
Every result carries ``scale_flag``: below MIN_INFORMATIVE_PARAMS it is "non-informative scale"
(benchmarks sit at chance; the numbers only prove the plumbing).
"""

from __future__ import annotations

import json
import math
import re
from collections.abc import Sequence
from pathlib import Path
from typing import Any

import torch
from torch import Tensor

from hypertrain.data.tokenizer import ByteTokenizer
from hypertrain.trainer.config import ModelConfig
from hypertrain.trainer.model import forward

LM_EVAL_VERSION = "0.4.13"
LM_EVAL_TASKS = ("mmlu_cloze", "hellaswag", "arc_challenge", "arc_easy", "gsm8k")
MIN_INFORMATIVE_PARAMS = 1_000_000_000
NON_INFORMATIVE = "non-informative scale"

Params = dict[str, Tensor]


class ByteLM:
    """Log-likelihood and greedy generation for a hypertrain decoder on the 259-byte tokenizer."""

    def __init__(self, cfg: ModelConfig, theta: Params) -> None:
        self.cfg, self.theta, self.tok = cfg, theta, ByteTokenizer()
        self.n_params = sum(x.numel() for x in theta.values())

    def loglikelihood(self, context: str, continuation: str) -> tuple[float, bool]:
        """(sum log p(continuation | context), is_greedy) over continuation bytes."""
        ctx = [self.tok.bos_id, *self.tok.encode(context)]
        cont = self.tok.encode(continuation)
        if not cont:
            return 0.0, True
        full = (ctx + cont)[-(self.cfg.seq_len + 1) :]
        lp = token_logprobs(self.cfg, self.theta, full)
        tail = lp[-len(cont) :]
        tgt = torch.tensor(full[-len(cont) :])
        got = tail.gather(1, tgt[:, None]).squeeze(1)
        return float(got.sum()), bool((tail.argmax(1) == tgt).all())

    def generate(self, prompt: str, max_new: int = 32, stop: Sequence[str] = ("\n",)) -> str:
        ids = [self.tok.bos_id, *self.tok.encode(prompt)]
        out: list[int] = []
        for _ in range(max_new):
            window = (ids + out)[-self.cfg.seq_len :] + [self.tok.pad_id]
            nxt = int(token_logprobs(self.cfg, self.theta, window)[-1].argmax())
            if nxt >= 256:
                break
            out.append(nxt)
            text = self.tok.decode(out)
            if any(s in text for s in stop):
                return text.split(stop[0])[0]
        return self.tok.decode(out)


@torch.no_grad()
def token_logprobs(cfg: ModelConfig, theta: Params, ids: Sequence[int]) -> Tensor:
    """Row t-1 = log p(. | ids[:t]) for t = 1..T: returns [len(ids)-1, vocab].

    ``forward`` (trainer, read-only here) returns only the loss, so the logits are captured by
    giving the model module a functional namespace whose cross_entropy records its input; the
    math is the training forward unchanged.
    """
    import types

    import torch.nn.functional as F

    import hypertrain.trainer.model as model_mod

    captured: list[Tensor] = []

    def capture(logits: Tensor, target: Tensor, *a: Any, **k: Any) -> Tensor:
        captured.append(logits.detach())
        return F.cross_entropy(logits, target, *a, **k)

    shim: Any = types.SimpleNamespace(
        **{n: getattr(F, n) for n in dir(F) if not n.startswith("__")}
    )
    shim.cross_entropy = capture
    tokens = torch.tensor([list(ids)], dtype=torch.int64)
    model_mod.F = shim
    try:
        forward(cfg, theta, tokens)
    finally:
        model_mod.F = F
    return torch.log_softmax(captured[0].float(), dim=-1)


_NUM = re.compile(r"-?\d[\d,]*\.?\d*")


def _final_number(s: str) -> str | None:
    if "####" in s:
        s = s.split("####")[-1]
    m = _NUM.findall(s)
    return m[-1].replace(",", "").rstrip(".") if m else None


def load_items(path: Path) -> list[dict[str, Any]]:
    return [json.loads(x) for x in Path(path).read_text().splitlines() if x.strip()]


def run_cloze(lm: ByteLM, items: Sequence[dict[str, Any]]) -> dict[str, Any]:
    """Score items per task; returns per-task metrics and per-item 0/1 correctness."""
    per: dict[str, dict[str, list[Any]]] = {}
    for it in items:
        t = per.setdefault(it["task"], {"ids": [], "acc": [], "acc_norm": []})
        t["ids"].append(it["id"])
        if "choices" in it:
            lls = [lm.loglikelihood(it["context"], " " + c)[0] for c in it["choices"]]
            norm = [
                ll / max(1, len((" " + c).encode()))
                for ll, c in zip(lls, it["choices"], strict=True)
            ]
            t["acc"].append(int(max(range(len(lls)), key=lls.__getitem__) == it["gold"]))
            t["acc_norm"].append(int(max(range(len(norm)), key=norm.__getitem__) == it["gold"]))
        else:
            gen = lm.generate(it["question"] + "\nAnswer:")
            ok = _final_number(gen) == _final_number(str(it["answer"]))
            t["acc"].append(int(ok))
            t["acc_norm"].append(int(ok))
    tasks = {
        k: {
            "n": len(v["acc"]),
            "acc": 100.0 * sum(v["acc"]) / len(v["acc"]),
            "acc_norm": 100.0 * sum(v["acc_norm"]) / len(v["acc_norm"]),
            "items": dict(zip(v["ids"], v["acc_norm"], strict=True)),
        }
        for k, v in per.items()
    }
    mean = sum(x["acc_norm"] for x in tasks.values()) / len(tasks) if tasks else math.nan
    return {"tasks": tasks, "bench_mean": mean}


def scale_flag(n_params: int) -> str:
    return NON_INFORMATIVE if n_params < MIN_INFORMATIVE_PARAMS else "informative"


def evaluate(
    cfg: ModelConfig,
    theta: Params,
    items_path: Path,
    out_path: Path,
    meta: dict[str, Any] | None = None,
) -> dict[str, Any]:
    """Run the cloze backend and write the results JSON (with the scale flag) to ``out_path``."""
    lm = ByteLM(cfg, theta)
    res = run_cloze(lm, load_items(items_path))
    doc = {
        "backend": "cloze",
        "lm_eval_version_pinned": LM_EVAL_VERSION,
        "n_params": lm.n_params,
        "scale_flag": scale_flag(lm.n_params),
        "note": "results meaningless below 1B params (chance level); plumbing check only"
        if lm.n_params < MIN_INFORMATIVE_PARAMS
        else "",
        "items_file": str(items_path),
        **res,
        **(meta or {}),
    }
    Path(out_path).write_text(json.dumps(doc, indent=2) + "\n")
    return doc


def lm_eval_adapter(cfg: ModelConfig, theta: Params) -> Any:
    """lm-evaluation-harness LM wrapping ByteLM (needs lm-eval==LM_EVAL_VERSION installed)."""
    from lm_eval.api.model import LM  # type: ignore[import-not-found]

    inner = ByteLM(cfg, theta)

    class HypertrainLM(LM):
        def loglikelihood(self, requests: Any) -> list[tuple[float, bool]]:
            return [inner.loglikelihood(*r.args) for r in requests]

        def loglikelihood_rolling(self, requests: Any) -> list[float]:
            return [inner.loglikelihood("", r.args[0])[0] for r in requests]

        def generate_until(self, requests: Any) -> list[str]:
            out = []
            for r in requests:
                ctx, kw = r.args
                stop = kw.get("until", ["\n"])
                out.append(inner.generate(ctx, kw.get("max_gen_toks", 64), stop))
            return out

    return HypertrainLM()
