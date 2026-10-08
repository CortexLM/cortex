import json
import math
from pathlib import Path
from typing import Any

import httpx
import numpy as np
import pytest

from hypertrain.data.tokenizer import ByteTokenizer
from hypertrain.datasets.build import build_mix
from hypertrain.datasets.registry import Source, load_registry
from hypertrain.teacher.__main__ import main, run_label
from hypertrain.teacher.adapters import ADAPTERS, RecordBuilder, to_question
from hypertrain.teacher.client import Budget, BudgetExceeded, TeacherClient
from hypertrain.teacher.config import TeacherConfig, TeacherError, load_config
from hypertrain.teacher.labeller import (
    LabelCache,
    Question,
    build_prompt,
    label_question,
    letter_probs,
    rotations,
)

KEY = "sk-secret-key-123"
RECORD = {"state_len": 32, "n_questions": 1, "n_options": 4, "opt_len": 8, "instr_len": 32}


def _resp(letters: dict[str, float], cost: float | None = 0.001) -> dict[str, Any]:
    alts = [{"token": k, "logprob": math.log(v)} for k, v in letters.items()]
    d: dict[str, Any] = {
        "choices": [{"logprobs": {"content": [{"token": "A", "top_logprobs": alts}]}}],
        "usage": {"prompt_tokens": 10, "completion_tokens": 1},
    }
    if cost is not None:
        d["usage"]["cost"] = cost
    return d


def _client(cfg: TeacherConfig, handler: Any, **kw: Any) -> TeacherClient:
    return TeacherClient(
        cfg, KEY, transport=httpx.MockTransport(handler), sleep=lambda _s: None, **kw
    )


def _q(n: int = 2) -> Question:
    return Question("text", "which?", tuple(f"opt{i}" for i in range(n)), 0, 0)


def _biased_handler(bias_letter: str = "A") -> Any:
    """Pure position bias: always 70% on one letter, rest uniform."""

    def h(req: httpx.Request) -> httpx.Response:
        prompt = json.loads(req.content)["messages"][0]["content"]
        k = sum(
            1
            for ln in prompt.splitlines()
            if len(ln) > 2 and ln[1:3] == ". " and ln[0] in "ABCDEFGHIJ"
        )
        rest = 0.3 / (k - 1)
        return httpx.Response(
            200, json=_resp({c: (0.7 if c == bias_letter else rest) for c in "ABCDEFGHIJ"[:k]})
        )

    return h


def test_logprob_mapping_renormalises() -> None:
    alts = [
        ("A", math.log(0.5)),
        (" b", math.log(0.25)),
        ("The", math.log(0.2)),
        ("C", math.log(0.05)),
    ]
    p = letter_probs(alts, 3)
    assert p is not None
    assert np.allclose(p, [0.5 / 0.8, 0.25 / 0.8, 0.05 / 0.8])
    assert letter_probs([("The", -1.0)], 3) is None
    assert letter_probs(None, 3) is None


def test_cyclic_average_cancels_position_bias(tmp_path: Path) -> None:
    cfg = TeacherConfig(cache_dir=str(tmp_path))
    r = label_question(_q(2), cfg, LabelCache(tmp_path), _client(cfg, _biased_handler()))
    assert r.status == "ok" and r.probs is not None and len(r.probs) == 3
    assert np.allclose(r.probs, [1 / 3] * 3, atol=1e-9)


def test_cyclic_average_keeps_content_signal(tmp_path: Path) -> None:
    cfg = TeacherConfig(cache_dir=str(tmp_path))
    q = _q(2)

    def h(req: httpx.Request) -> httpx.Response:
        prompt = json.loads(req.content)["messages"][0]["content"]
        right = next(ln[0] for ln in prompt.splitlines() if ln.endswith(". opt1"))
        return httpx.Response(200, json=_resp({c: (0.8 if c == right else 0.1) for c in "ABC"}))

    r = label_question(q, cfg, LabelCache(tmp_path), _client(cfg, h))
    assert r.probs is not None and np.allclose(r.probs, [0.1, 0.8, 0.1])


def test_prompt_lists_unknown_and_letters() -> None:
    q = _q(2)
    pr = build_prompt(q, rotations(q, 0)[0])
    assert "A. opt0" in pr and "C. cannot be determined from the text" in pr
    assert len(rotations(q, 0)) == 3 and len(rotations(q, 2)) == 2


def test_missing_logprobs_is_failed_and_counted(tmp_path: Path) -> None:
    cfg = TeacherConfig(cache_dir=str(tmp_path), rotations=1)
    cache = LabelCache(tmp_path)

    def h(_r: httpx.Request) -> httpx.Response:
        return httpx.Response(200, json={"choices": [{"message": {"content": "A"}}]})

    c = _client(cfg, h)
    r = label_question(_q(), cfg, cache, c)
    assert r.status == "failed" and r.probs is None
    src = load_registry()[0]["boolq"]
    rep = run_label([(src, _q()), (src, _q(3))], cfg, LabelCache(tmp_path / "x"), c)
    assert rep["failed"] == 2 and rep["ok"] == 0


def test_budget_cap_stops_before_exceeding(tmp_path: Path) -> None:
    cfg = TeacherConfig(cache_dir=str(tmp_path), max_usd=0.0025, rotations=1, max_concurrency=1)
    n = {"c": 0}

    def h(_r: httpx.Request) -> httpx.Response:
        n["c"] += 1
        return httpx.Response(200, json=_resp({"A": 0.5, "B": 0.5}, cost=0.001))

    c = _client(cfg, h)
    src = load_registry()[0]["boolq"]
    qs = [(src, Question("t", f"q{i}", ("a", "b"), 0, 0)) for i in range(10)]
    rep = run_label(qs, cfg, LabelCache(tmp_path), c)
    assert n["c"] == 2 and rep["budget_stopped"] and c.budget.spent <= 0.0025
    assert rep["unlabelled"] == 8


def test_budget_reserve_blocks() -> None:
    b = Budget(0.001)
    with pytest.raises(BudgetExceeded):
        b.reserve(0.002)


def test_429_retry_after_then_success(tmp_path: Path) -> None:
    cfg = TeacherConfig(cache_dir=str(tmp_path), rotations=1)
    waits: list[float] = []
    seq = [
        httpx.Response(429, headers={"Retry-After": "7"}),
        httpx.Response(200, json=_resp({"A": 1.0})),
    ]

    c = TeacherClient(
        cfg, KEY, transport=httpx.MockTransport(lambda _r: seq.pop(0)), sleep=waits.append
    )
    out = c.complete("hi")
    assert waits == [7.0] and out.top_logprobs == [("A", 0.0)]


def test_retries_exhausted_message_has_no_key(tmp_path: Path) -> None:
    cfg = TeacherConfig(retries=1)
    c = _client(cfg, lambda _r: httpx.Response(503, text=f"oops {KEY}"))
    with pytest.raises(TeacherError) as ei:
        c.complete("hi")
    assert KEY not in str(ei.value)
    c2 = _client(cfg, lambda _r: httpx.Response(401, text=f"bad key {KEY}"))
    with pytest.raises(TeacherError) as e2:
        c2.complete("hi")
    assert KEY not in str(e2.value) and "***" in str(e2.value)


def test_cache_hit_makes_zero_http_calls(tmp_path: Path) -> None:
    cfg = TeacherConfig(cache_dir=str(tmp_path), rotations=2)
    cache = LabelCache(tmp_path)
    calls = {"n": 0}

    def h(r: httpx.Request) -> httpx.Response:
        calls["n"] += 1
        return _biased_handler()(r)

    first = label_question(_q(), cfg, cache, _client(cfg, h))
    assert calls["n"] == 2
    again = label_question(_q(), cfg, cache, _client(cfg, h))
    assert calls["n"] == 2 and again.cached == 2 and again.probs == first.probs
    assert not list(tmp_path.rglob("*.tmp"))


def test_cache_key_depends_on_endpoint_and_model() -> None:
    from hypertrain.teacher.labeller import cache_key

    a, b = TeacherConfig(), TeacherConfig(base_url="http://x/v1")
    assert cache_key(a, "p", (0, 1)) != cache_key(b, "p", (0, 1))
    assert cache_key(a, "p", (0, 1)) != cache_key(TeacherConfig(model="m"), "p", (0, 1))


def test_config_endpoint_override_is_used(tmp_path: Path) -> None:
    toml = tmp_path / "t.toml"
    toml.write_text(
        'base_url = "http://localhost:9999/v1"\nmodel = "my/model"\n\n[headers]\nX-Title = "t"\n'
    )
    cfg = load_config(toml)
    assert cfg.base_url == "http://localhost:9999/v1" and cfg.headers == {"X-Title": "t"}
    seen: list[httpx.Request] = []

    def h(r: httpx.Request) -> httpx.Response:
        seen.append(r)
        return httpx.Response(200, json=_resp({"A": 1.0}))

    _client(cfg, h).complete("hi")
    assert str(seen[0].url) == "http://localhost:9999/v1/chat/completions"
    assert json.loads(seen[0].content)["model"] == "my/model"
    assert seen[0].headers["x-title"] == "t" and seen[0].headers["authorization"] == f"Bearer {KEY}"


def test_default_and_example_config() -> None:
    cfg = load_config(Path(__file__).parents[2] / "teacher.example.toml")
    assert (
        cfg.base_url == "https://openrouter.ai/api/v1"
        and cfg.model == "deepseek/deepseek-v4.1-flash"
    )
    with pytest.raises(TeacherError):
        load_config(None, top_logprobs=0)


def test_unknown_config_key_rejected(tmp_path: Path) -> None:
    toml = tmp_path / "t.toml"
    toml.write_text("bogus = 1\n")
    with pytest.raises(TeacherError, match="bogus"):
        load_config(toml)


def test_key_sources(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.delenv("OPENROUTER_API_KEY", raising=False)
    with pytest.raises(TeacherError) as ei:
        TeacherConfig().api_key()
    assert "OPENROUTER_API_KEY" in str(ei.value)
    monkeypatch.setenv("OPENROUTER_API_KEY", KEY)
    assert TeacherConfig().api_key() == KEY
    kf = tmp_path / "k"
    kf.write_text("file-key\n")
    assert TeacherConfig(api_key_file=str(kf)).api_key() == "file-key"


def test_key_never_in_logs(tmp_path: Path, caplog: pytest.LogCaptureFixture) -> None:
    caplog.set_level("DEBUG")
    cfg = TeacherConfig(cache_dir=str(tmp_path), rotations=1)
    label_question(_q(), cfg, LabelCache(tmp_path), _client(cfg, _biased_handler()))
    assert KEY not in caplog.text and KEY not in repr(cfg)


FIXTURES: dict[str, dict[str, Any]] = {
    "multi-nli": {"premise": "A man sleeps.", "hypothesis": "A man is awake.", "label": 2},
    "boolq": {"question": "is the sky blue", "passage": "The sky looks blue.", "answer": True},
    "commonsense-qa": {
        "question": "Where do you buy bread?",
        "choices": {"label": list("ABCDE"), "text": ["bank", "bakery", "zoo", "car", "sea"]},
        "answerKey": "B",
    },
    "qasc": {
        "question": "What melts ice?",
        "fact1": "Heat melts ice.",
        "fact2": "The sun gives heat.",
        "choices": {"label": list("ABCDEFGH"), "text": [f"c{i}" for i in range(8)]},
        "answerKey": "C",
    },
    "cosmos-qa": {
        "context": "Sam ran a marathon.",
        "question": "How does Sam feel?",
        "answer0": "tired",
        "answer1": "bored",
        "answer2": "cold",
        "answer3": "none",
        "label": 0,
    },
}


@pytest.mark.parametrize(
    ("sid", "n_opts", "gold"),
    [
        ("multi-nli", 3, 2),
        ("boolq", 2, 0),
        ("commonsense-qa", 5, 1),
        ("qasc", 8, 2),
        ("cosmos-qa", 4, 0),
    ],
)
def test_adapters(sid: str, n_opts: int, gold: int) -> None:
    src = load_registry()[0][sid]
    q = to_question(src, FIXTURES[sid])
    assert (
        len(q.options) == n_opts and q.gold == gold and q.qtype == 0 and q.state and q.instruction
    )


def test_tulu_not_implemented() -> None:
    src = load_registry()[0]["tulu-3-sft"]
    with pytest.raises(NotImplementedError, match="A9"):
        to_question(src, {"messages": [], "source": "x"})
    assert set(FIXTURES) | {"tulu-3-sft"} == set(ADAPTERS)


def _labelled_builder(
    tmp_path: Path, sid: str = "cosmos-qa"
) -> tuple[RecordBuilder, Source, TeacherConfig, LabelCache]:
    cfg = TeacherConfig(cache_dir=str(tmp_path), rotations=2)
    cache = LabelCache(tmp_path)
    src = load_registry()[0][sid]
    return RecordBuilder(ByteTokenizer(), RECORD, cfg, cache), src, cfg, cache


def test_to_record_packs_with_unknown_at_n_options(tmp_path: Path) -> None:
    rb, src, cfg, cache = _labelled_builder(tmp_path)
    q = to_question(src, FIXTURES["cosmos-qa"])
    r = label_question(q, cfg, cache, _client(cfg, _biased_handler()))
    assert r.probs is not None
    rec = rb(src, FIXTURES["cosmos-qa"])
    assert rec is not None and rec.dtype == np.uint32 and len(rec) == rb.shape.length
    s = rb.shape
    te = rec[-(s.n_options + 1) :].astype(np.int64)
    assert te[s.n_options] > 0 and te[: len(q.options)].sum() > 0
    assert np.all(te[len(q.options) : s.n_options] == 0)
    assert abs(te.sum() / rb.qmax - 1.0) < 1e-3
    assert rec[-(s.n_options + 1) - 1] == 0  # y = gold 0


def test_to_record_missing_label_raises(tmp_path: Path) -> None:
    rb, src, _, _ = _labelled_builder(tmp_path)
    with pytest.raises(TeacherError, match="no cached teacher label"):
        rb(src, FIXTURES["cosmos-qa"])


def test_to_record_failed_label_skipped_and_counted(tmp_path: Path) -> None:
    rb, src, cfg, cache = _labelled_builder(tmp_path)
    c = _client(
        cfg, lambda _r: httpx.Response(200, json={"choices": [{"message": {"content": "A"}}]})
    )
    label_question(to_question(src, FIXTURES["cosmos-qa"]), cfg, cache, c)
    assert rb(src, FIXTURES["cosmos-qa"]) is None and rb.stats.failed == 1


def test_build_records_fails_when_label_missing(tmp_path: Path) -> None:
    rb, _, _, _ = _labelled_builder(tmp_path)
    rows = {"cosmos-qa": [FIXTURES["cosmos-qa"]]}
    mini = tmp_path / "reg.json"
    raw = json.loads(
        (Path(__file__).parents[2] / "src/hypertrain/datasets/registry.json").read_text()
    )
    raw["mixes"] = {
        "m": {
            **raw["mixes"]["od-b-v1"],
            "components": [["cosmos-qa", 1.0]],
            "record": RECORD,
            "decontam_against": ["mmlu-test"],
        }
    }
    mini.write_text(json.dumps(raw))
    with pytest.raises(TeacherError, match="no cached teacher label"):
        build_mix(
            "m",
            tmp_path / "out",
            cache=tmp_path,
            registry_path=mini,
            tokenizer=ByteTokenizer(),
            to_record=rb,
            loader=lambda s: rows.get(s.id, [{"question": "zzz unrelated", "choices": []}]),
        )


def test_cli_estimate_no_network(tmp_path: Path, capsys: pytest.CaptureFixture[str]) -> None:
    import pyarrow as pa
    import pyarrow.parquet as pq

    d = tmp_path / "cache" / "cosmos-qa"
    d.mkdir(parents=True)
    pq.write_table(pa.Table.from_pylist([FIXTURES["cosmos-qa"]] * 2), d / "x.parquet")
    cfg = tmp_path / "c.toml"
    cfg.write_text(f'cache_dir = "{tmp_path / "lc"}"\nrotations = 2\n')
    rc = main(
        [
            "estimate",
            "--config",
            str(cfg),
            "--data-cache",
            str(tmp_path / "cache"),
            "--sources",
            "cosmos-qa",
        ]
    )
    out = capsys.readouterr().out
    assert rc == 0 and '"questions": 2' in out and '"uncached_calls": 4' in out


def _capture(cfg: TeacherConfig, resp: dict[str, Any] | None = None) -> tuple[dict[str, Any], Any]:
    body: dict[str, Any] = {}

    def h(r: httpx.Request) -> httpx.Response:
        body.update(json.loads(r.content))
        return httpx.Response(200, json=resp or _resp({"A": 1.0}))

    return body, _client(cfg, h).complete("hi")


def test_request_has_reasoning_and_provider_by_default() -> None:
    body, _ = _capture(TeacherConfig())
    assert body["reasoning"] == {"enabled": False}
    assert body["provider"] == {"require_parameters": True}
    body2, _ = _capture(TeacherConfig(base_url="http://localhost:1/v1"))
    assert "provider" not in body2 and body2["reasoning"] == {"enabled": False}


def test_empty_tables_omit_fields(tmp_path: Path) -> None:
    toml = tmp_path / "t.toml"
    toml.write_text("reasoning = {}\nprovider = {}\n")
    body, _ = _capture(load_config(toml))
    assert "reasoning" not in body and "provider" not in body


def test_custom_tables_pass_verbatim() -> None:
    body, _ = _capture(TeacherConfig(reasoning={"effort": "low"}, provider={"order": ["x"]}))
    assert body["reasoning"] == {"effort": "low"} and body["provider"] == {"order": ["x"]}


def test_blank_leading_token_skipped() -> None:
    alts = [{"token": "B", "logprob": 0.0}]
    resp = {
        "choices": [
            {
                "logprobs": {
                    "content": [
                        {"token": " ", "top_logprobs": [{"token": " ", "logprob": 0.0}]},
                        {"token": "", "top_logprobs": []},
                        {"token": "B", "top_logprobs": alts},
                    ]
                }
            }
        ],
        "usage": {"cost": 0.0},
    }
    _, out = _capture(TeacherConfig(), resp)
    assert out.top_logprobs == [("B", 0.0)]
    resp["choices"][0]["logprobs"]["content"] = [{"token": " ", "top_logprobs": []}]  # type: ignore[index]
    _, out2 = _capture(TeacherConfig(), resp)
    assert out2.top_logprobs is None
