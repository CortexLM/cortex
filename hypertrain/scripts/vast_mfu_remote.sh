#!/usr/bin/env bash
# Remote MFU sweep for scripts/vast_e2e.py --remote-script scripts/vast_mfu_remote.sh.
# Env: HT_MFU_WORLDS (default "1 2", capped at GPUs present), HT_MFU_REDUCTION (zero2o),
# HT_MFU_TRACE=1, HT_ATTN_BACKEND, HT_COMPILE. Every step: OUT/<name>.log + OUT/steps.tsv.
set -uo pipefail
HT=${HT_E2E_SRC:-/workspace/ht}
OUT=${HT_E2E_OUT:-/workspace/out}
mkdir -p "$OUT"
cd "$HT"
export CUBLAS_WORKSPACE_CONFIG=:4096:8 CUDA_DISABLE_PTX_JIT=1 OMP_NUM_THREADS=1 PYTHONHASHSEED=0
[ -n "${HT_ATTN_BACKEND:-}" ] && export HT_ATTN_BACKEND
[ -n "${HT_COMPILE:-}" ] && export HT_COMPILE
# vastai/pytorch ships its venv at /venv/main and has no bare `python` on PATH.
PY=${HT_MFU_PYTHON:-}
for cand in /venv/main/bin/python python3 python; do
  [ -n "$PY" ] && break
  command -v "$cand" >/dev/null 2>&1 && PY=$(command -v "$cand")
done
[ -n "$PY" ] || { echo "FATAL: no python interpreter found" >&2; exit 1; }
echo "$PY" >"$OUT/python.txt"
# pip may be absent from the venv (uv-made venvs); fall back to uv targeting the same interpreter.
pipi() {
  if "$PY" -m pip --version >/dev/null 2>&1; then "$PY" -m pip install "$@"
  elif command -v uv >/dev/null 2>&1; then uv pip install --python "$PY" "$@"
  else echo "FATAL: neither pip nor uv available for $PY" >&2; return 1; fi
}

step() {
  local name=$1; shift
  echo "+ $*" >"$OUT/$name.log"
  "$@" >>"$OUT/$name.log" 2>&1
  local rc=$?
  printf '%s\t%s\n' "$name" "$rc" >>"$OUT/steps.tsv"
  return $rc
}
: >"$OUT/steps.tsv"

step nvidia_smi nvidia-smi
step topo nvidia-smi topo -m
cp "$OUT/topo.log" "$OUT/topo.txt"

torch_ok() {
  "$PY" - <<'PYEOF'
import sys
import torch
v = tuple(int(x) for x in torch.__version__.split("+")[0].split(".")[:2])
arch = torch.cuda.get_arch_list() if torch.cuda.is_available() else []
print("torch", torch.__version__, "cuda", torch.version.cuda, "archs", arch)
if v < (2, 14):
    sys.exit("torch < 2.14")
if "sm_120" not in arch:
    sys.exit("no sm_120 support")
PYEOF
}
if ! step torch_check torch_ok; then
  echo "image torch unusable (see torch_check.log); installing torch==2.14.0 cu130" >&2
  step torch_install pipi torch==2.14.0 \
    --index-url https://download.pytorch.org/whl/cu130 || exit 1
  step torch_recheck torch_ok || { echo "FATAL: torch still unusable" >&2; exit 1; }
fi
# Install with the package's real dependencies (torch==2.14.0 is already satisfied by the
# image), then prove the import chain run_mfu_dist.py needs (live: sr25519 was missing).
step pip_install pipi -e "$HT" || exit 1
step import_check "$PY" -c "import hypertrain.protocol.example, hypertrain.trainer.island" || exit 1

NGPU=$(nvidia-smi -L | grep -c '^GPU')
rc_all=0
for N in ${HT_MFU_WORLDS:-1 2}; do
  if [ "$N" -gt "$NGPU" ]; then
    echo "skip dp$N: only $NGPU GPUs present" | tee -a "$OUT/skipped.txt"
    continue
  fi
  if [ "$N" -eq 1 ]; then PROFILE=mfu-dense-1b; else PROFILE=mfu-dense-1b-dp$N; fi
  if ! "$PY" -c "import json,sys; sys.exit('$PROFILE' not in json.load(open('scripts/mfu_profiles.json'))['profiles'])"; then
    echo "FAIL dp$N: profile $PROFILE missing from scripts/mfu_profiles.json" \
      | tee -a "$OUT/skipped.txt"
    rc_all=1
    continue
  fi
  # HT_MFU_COMPILE="1 0": one run per value, compiled (HT_COMPILE=1) then eager.
  # HT_MFU_P2P="1 0": NCCL_P2P_DISABLE per run (--nccl-pcie only setdefaults it).
  # HT_MFU_MB="8 6": per-rank micro_batch overrides (0 = profile value).
  for M in ${HT_MFU_MB:-0}; do
  for C in ${HT_MFU_COMPILE:-0}; do
  for P in ${HT_MFU_P2P:-1}; do
    TAG="dp$N"
    EXTRA=()
    [ "$C" = 1 ] && TAG="$TAG-compile" && EXTRA=(--compile)
    [ "$P" = 0 ] && TAG="$TAG-p2p"
    [ "$M" != 0 ] && TAG="$TAG-mb$M" && EXTRA+=(--micro-batch "$M")
    [ "${HT_MFU_TRACE:-0}" = 1 ] && EXTRA+=(--trace "$OUT/trace-$TAG")
    step "mfu-$TAG" env NCCL_P2P_DISABLE="$P" \
      "$PY" -m torch.distributed.run --standalone --nproc-per-node "$N" \
      scripts/run_mfu_dist.py --profile "$PROFILE" --device cuda \
      --reduction "${HT_MFU_REDUCTION:-zero2o}" --act-ckpt --nccl-pcie \
      --out "$OUT/mfu-$TAG.json" "${EXTRA[@]}" || rc_all=1
  done
  done
  done
done
cat "$OUT/steps.tsv"
exit $rc_all
