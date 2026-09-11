#!/bin/bash
# Force Harbor --path onto the filtered $PROOF_TASKS tree.
# Installed first on PATH for the script harness only. The real binary is
# $PROOF_HARBOR_REAL (captured before this wrapper is prepended).
set -euo pipefail
real="${PROOF_HARBOR_REAL:?PROOF_HARBOR_REAL is the wrapped harbor binary}"
tasks="${PROOF_TASKS:?PROOF_TASKS is the filtered task tree}"
args=()
have_path=0
is_run=0
while [ $# -gt 0 ]; do
    case "$1" in
        run)
            is_run=1
            args+=("$1")
            shift
            ;;
        --path|-p)
            if [ $# -lt 2 ]; then
                echo "rlm_fc_in_guest_harbor: harbor wrapper: $1 needs a value" >&2
                exit 2
            fi
            if [ "$2" != "$tasks" ]; then
                echo "rlm_fc_in_guest_harbor: rewriting harbor $1 to filtered PROOF_TASKS" >&2
            fi
            args+=(--path "$tasks")
            have_path=1
            shift 2
            ;;
        --path=*)
            if [ "${1#--path=}" != "$tasks" ]; then
                echo "rlm_fc_in_guest_harbor: rewriting harbor --path to filtered PROOF_TASKS" >&2
            fi
            args+=(--path "$tasks")
            have_path=1
            shift
            ;;
        *)
            args+=("$1")
            shift
            ;;
    esac
done
if [ "$is_run" -eq 1 ] && [ "$have_path" -eq 0 ]; then
    echo "rlm_fc_in_guest_harbor: injecting harbor --path=\$PROOF_TASKS (filtered)" >&2
    args+=(--path "$tasks")
fi
exec "$real" "${args[@]}"
