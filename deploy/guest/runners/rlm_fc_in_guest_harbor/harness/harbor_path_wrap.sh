#!/bin/bash
# Force Harbor --path onto the filtered task tree.
# Generated copies bake `real` and `tasks`. This file must not read
# PROOF_HARBOR_REAL (never a miner-usable env) and must not fall back to
# an unfiltered pack path.
set -euo pipefail
unset PROOF_HARBOR_REAL || true
if [ -z "${real:-}" ] || [ -z "${tasks:-}" ]; then
    echo "rlm_fc_in_guest_harbor: harbor wrapper missing real binary or filtered tasks" >&2
    exit 2
fi
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
