# Teacher labelling (stage B/C soft labels)

Offline, once: an OpenAI-compatible chat endpoint labels multiple-choice questions; the
probabilities are quantised into the record's `teacher_q` field (DESIGN A9/A11). Training
and the audited step never call a teacher.

## Setup

```sh
cp teacher.example.toml teacher.toml        # edit
export OPENROUTER_API_KEY=...                # or api_key_file = "/path/to/key"
python -m hypertrain.teacher estimate --config teacher.toml --data-cache cache --limit 1000
python -m hypertrain.teacher label    --config teacher.toml --data-cache cache --limit 1000 --max-usd 5 --out label-report.json
python -m hypertrain.teacher build-records --config teacher.toml --data-cache cache --out data/od-b-v1
```

`build-records` reads the label cache only (no network) and fails if a label is missing.
Exit codes: `label` returns 1 if any item FAILED or was left unlabelled (budget stop).

## How a label is made

Prompt: text, question, options as letters A.. plus a last option "cannot be determined from
the text". The model answers one letter (`max_tokens 1`, `logprobs`, `top_logprobs`). Letter
probabilities from the first non-blank content token's alternatives are renormalised, then averaged over cyclic
rotations of the option order (`opendecision.teacher.cyclic_average`) to cancel position bias.
Output: K_q+1 probabilities, unknown last; `pack_record` puts unknown at index `n_options`.
No logprobs, or no letter among them: the item is FAILED, skipped and counted. Never defaulted.

## Config keys (`teacher.example.toml`)

| key | default | note |
|---|---|---|
| `base_url` | `https://openrouter.ai/api/v1` | any OpenAI-compatible `/chat/completions` base |
| `model` | `deepseek/deepseek-v4.1-flash` | must support `logprobs` |
| `api_key_env` / `api_key_file` | `OPENROUTER_API_KEY` / unset | file wins; key is never logged |
| `headers` | `{}` | e.g. `HTTP-Referer`, `X-Title` |
| `temperature`, `top_logprobs` | 0, 20 | |
| `max_concurrency`, `timeout_s` | 8, 60 | |
| `retries`, `backoff_s` | 5, 1.0 | 429/5xx/transport; `Retry-After` honoured |
| `max_usd` | 10 | hard cap, reserved before each call; `--max-usd` overrides |
| `price_prompt`, `price_completion` | 3.56e-8, 1e-6 USD/token | fallback when response has no `usage.cost` |
| `reasoning` | `{ enabled = false }` | verbatim `reasoning` request field; reasoning tokens would consume `max_tokens=1`. `{}` omits it |
| `provider` | `{ require_parameters = true }` on openrouter.ai, else omitted | verbatim `provider` field; OpenRouter routes logprobs only to some providers. `{}` omits it |
| `rotations` | 0 | 0 = all K+1; N caps calls per question |
| `cache_dir` | `teacher-cache` | one JSON per sha256(endpoint, model, prompt, order); atomic writes |

## Cost

One call per (question, rotation): K+1 calls per question. Prices above are OpenRouter's
listing for the default model on 2026-10-08 (context 1,048,576). Run `estimate` first; it is
offline and uses ~4 chars/token. Cached rotations cost nothing on re-runs.

## Gaps and licence flags

- Tulu-3 SFT (25% of od-b-v1) has no options; DESIGN A9 defines no conversion. The adapter
  raises `NotImplementedError`; the design owner must specify one. Stage C sources beyond
  od-b-v1 (snli, banking77, helpsteer3, squad-v2, typed-decisions) have no adapter yet.
- A9 names an Apache-2.0 teacher panel (Qwen3, Mistral-Small); the default here is a single
  DeepSeek model by the user's request. **For the D5 licence lane:** the DeepSeek model
  licence and the terms on its outputs (use for training a student) are unverified here.
