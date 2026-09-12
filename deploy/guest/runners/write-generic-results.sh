# write-generic-results.sh — POSIX helper for a custom-family Evaluate.
#
# After `report.json` exists, write `generic-custom-v1` results JSON bound
# to the scored `primary_value` / `claim_holds` and the guest identity env.
# File name: `results.json`, or `PROOF_PARAM_RESULTS_PATH` when it is one
# safe `*.json` segment. Harbor / trial contracts write their own document
# (see `rlm_fc_in_guest_harbor/harness/summarize.py`).
#
# Source or append this from an adaptor `run`. Missing `report.json` is a
# no-op (the guest still fail-closes Evaluate with no results file).

if [ -f "${PROOF_OUTPUT_DIR:?}/report.json" ]; then
    _results_name=results.json
    case "${PROOF_PARAM_RESULTS_PATH:-}" in
        [A-Za-z0-9][A-Za-z0-9._-]*.json)
            case "${PROOF_PARAM_RESULTS_PATH}" in
                */* | .* | *.*.*.*) ;;
                *) _results_name="${PROOF_PARAM_RESULTS_PATH}" ;;
            esac
            ;;
    esac
    _pv=$(sed -n 's/.*"primary_value"[[:space:]]*:[[:space:]]*\([^,}[:space:]]*\).*/\1/p' \
        "$PROOF_OUTPUT_DIR/report.json" | sed -n '1p')
    _ch=$(sed -n 's/.*"claim_holds"[[:space:]]*:[[:space:]]*\([^,}[:space:]]*\).*/\1/p' \
        "$PROOF_OUTPUT_DIR/report.json" | sed -n '1p')
    [ -n "$_ch" ] || _ch=false
    [ -n "$_pv" ] || _pv=0
    cat > "$PROOF_OUTPUT_DIR/$_results_name" <<EOF
{"schema_version":1,"contract":"generic-custom-v1","topic_id":"${PROOF_TOPIC_ID}","custom_id":"${PROOF_CUSTOM_ID}","submission_digest":"${PROOF_SUBMISSION_DIGEST}","artifact_digest":"${PROOF_ARTIFACT_DIGEST}","primary_value":${_pv},"claim_holds":${_ch},"display":{"ok":true}}
EOF
    unset _results_name _pv _ch
fi
