# Multi-GPU training and MFU (PCIe, no NVLink)

How to run the 1B dense island on several RTX 5090s over PCIe, what each knob does, and what was
measured. Every configuration below keeps bitwise replay: the leaf root of a second pass equals
the first (`replay_match: true`).

## Running it

On a Vast host, through the guarded harness (one instance, deleted and confirmed absent on exit):

```bash
uv run --frozen python scripts/vast_e2e.py --profile mfu --num-gpus 2 --min-pcie-bw 50 \
  --key-file ~/.config/vast.key --max-usd 3 --max-minutes 50 \
  --remote-env HT_MFU_WORLDS=2 --remote-env HT_MFU_COMPILE=1 --remote-env HT_MFU_MB=8
```

`scripts/vast_mfu_remote.sh` sweeps `HT_MFU_WORLDS` x `HT_MFU_MB` x `HT_MFU_COMPILE` x
`HT_MFU_P2P` and writes one `mfu-<tag>.json` per run. On any host, the same run directly:

```bash
torchrun --standalone --nproc-per-node 2 scripts/run_mfu_dist.py --profile mfu-dense-1b-dp2 \
  --device cuda --reduction zero2o --act-ckpt --nccl-pcie --compile --micro-batch 8 \
  --out mfu-dp2.json
```

Profiles live in `scripts/mfu_profiles.json`. The JSON records MFU against the vendor bf16 dense
peak, tokens/s, per-step times, `rank_peak_mem_gib` (whole round), `rank_step_peak_mem_gib`
(inside training steps only), the NCCL environment and the `HT_*` engine flags.

## Knobs

| Knob | Effect | Replay |
|------|--------|--------|
| `--reduction zero2o` | Gradients reduced to their ZeRO-2 owner in buckets issued from post-accumulate grad hooks, so communication overlaps the rest of backward. | Bitwise equal to `zero2` (same adds, same order). |
| `--act-ckpt` (`HT_ACT_CKPT=1`) | Per-layer activation checkpointing. | Same kernels on recompute: bitwise equal. |
| sdpa-flash | Selected on (sm_120, torch 2.14.0+cu130) from the qualification record in `tests/trainer/data/attn_qualify_sm120.json`. When the flash kernel rejects the inputs (dtype, head dim, `enable_gqa`), the first accepting path of `attention.PATHS` is probed once per (device, dtype, group, head dim) and logged as `attention backend: <path>`; inputs whose layout the chosen kernel still rejects use math. | Qualified by cross-process equality (flash path only). A fallback path is a function of hardware, torch and geometry, so ranks agree, but it is not qualified. |
| `--compile` (`HT_COMPILE=1`) | One Inductor graph per layer. | Leaves differ from eager (fusion changes rounding), so a compiled run replays only under the same flag. Both modes replay bitwise. |
| `--micro-batch N` | Overrides the profile's per-rank micro_batch. | Part of the run; recorded as `HT_MFU_MICRO_BATCH`. |
| `--nccl-pcie` | setdefaults `NCCL_P2P_DISABLE=1`, Ring, Simple, 4-8 channels, 8 MiB buffers. | Collectives are copies: no numeric effect. |
| `PYTORCH_CUDA_ALLOC_CONF=expandable_segments:True` | Set by default in `run_mfu_dist.py`; avoided a fragmentation OOM at 29.9 GiB on a 31.36 GiB card. | Allocator layout only. |

The RoPE cos/sin tables are now cached per (config, length, dtype, device). The pageable
host-to-device copy used to run once per layer and per checkpoint recompute and stalled the GPU.
The bytes are identical, so leaves are unchanged.

## Measured (1B profile, 2026-10-12/13)

| Run | MFU | Tokens/s | Step | In-step peak / rank |
|-----|----:|---------:|-----:|--------------------:|
| dp2, compile, micro_batch 12 | 54.58 % | 37,243 | 1.311 s | 24.41 GiB |
| dp2, compile, micro_batch 10 | 53.16 % | 36,275 | 1.128 s | 22.58 GiB |
| dp2, compile, micro_batch 8 | 52.09 % | 35,543 | 0.922 s | 20.75 GiB |
| dp2, compile, micro_batch 6 | 46.61 % | 31,800 | 0.773 s | 20.04 GiB |
| dp2, compile, micro_batch 4 | 37.20 % | 25,379 | 0.636 s | ~20 GiB |
| dp2, eager + RoPE cache, micro_batch 4 | 35.52 % | 24,234 | 0.656 s | - |
| dp2, eager, micro_batch 4 (before RoPE cache) | 34.02 % | 23,216 | 0.684 s | - |
| dp4, compile, micro_batch 4 | 24.02 % | 32,778 | 0.990 s | 18.81 GiB |
| dp2 before this work | 7.40 % | | | |

Recommended: dp2, `zero2o`, activation checkpointing, `--compile`, micro_batch 8 to 12, on a
host with `pcie_bw` >= 50.

## What limits it

- **Host PCIe.** The same dp2 config measured 11.2-11.5 % on a host where NCCL took ~3.8 s of GPU
  time per step whether P2P was on or off, against 37.2 % on a `pcie_bw` 53.8 (PCIe Gen5) host.
  `--profile mfu` therefore filters offers on `pcie_bw` and requires `cuda_max_good >= 13.2`
  (the image is CUDA 13.2; a 13.0 host refused create with HTTP 400). Every profile also keeps
  only the drivers in `scripts/genuine_cuda_ci.py` `DRIVERS`; MFU does not compare replays, so
  add `--any-driver` to widen the market. Evidence goes to `.evidence/vast/` unless
  `--evidence-dir` is given.
- **Round-end memory, not step memory.** `max_memory_allocated` over a round (~29.9 GiB at
  micro_batch 4) is dominated by the round end: full weights and optimizer state, delta, error
  feedback and the compressed payload. Inside the steps the peak is ~20 GiB, which is why
  micro_batch 8-12 fits.
- **dp4 scaling.** dp4 reaches 1.3x the dp2 throughput: NCCL CUDA time rises from 1.55 s to
  4.03 s per 3 steps, because each rank all_gathers (world - 1) shards over PCIe. A
  reduce-scatter-shaped owner reduction and a larger micro_batch at dp4 are the next steps.
- **Elementwise work.** In the compiled trace, CUDA time outside GEMM/attention/NCCL (fp32 adds,
  fills, casts, checkpoint recompute) is still the largest bucket.

## All GPU proofs on one host (`--profile proofs`)

One 2x RTX 5090 instance, one upload, one DELETE and one parsed absence proof at the end. The
image is the digest-pinned `IMAGE` of `scripts/vast_e2e.py`; Vast pulls it before ssh is ready,
so the per-step seconds in `out/steps.tsv` (name, rc, seconds) exclude the pull.

```bash
uv run --frozen python scripts/vast_e2e.py --profile proofs --key-file /var/lib/openbot/.vast-key \
  --max-usd 8 --max-minutes 45 --dry-run
```

`--dry-run` prints the chosen offer, then the step list (`PROOF_PLAN`), the pytest node ids
(`PROOF_TESTS`), the caps and the worst-case cost, without creating anything (it does make read-only GET calls: credit and up to 8 offer pages). `remote_env` is printed with values masked (`KEY=***`). A real run exits 4 when it passed on a driver outside `QUALIFIED_DRIVERS`: the result is valid but backs no bitwise-replay claim. Steps: the
gpu-e2e flow (network round, auditor replay, repeat; `HT_ATTN_BACKEND=math` because the fixture
model is fp32 with head_dim 4), then with `HT_ATTN_BACKEND` unset: `select_backend` must pick
sdpa-flash, the GPU-only tests (a skip fails), and a 1B bf16 dp2 round plus replay through
`run_mfu_dist.py`, checked by `check_gpu_evidence.py --kind mfu` and `attention_backend ==
sdpa-flash`. `--boot-retries` defaults to 0 here.

Drivers: the default is `--any-driver`, and the offer quote and `harness-result.json` carry
`driver_qualified` (false when the driver is not in `scripts/genuine_cuda_ci.py` `DRIVERS`, so
no bitwise claim is made). `--eight-gpu-qualified` rents 8x RTX 5090 with a qualified driver
only; nothing else selects 8 GPUs for this profile.
