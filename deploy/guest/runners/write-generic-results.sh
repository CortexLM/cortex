# write-generic-results.sh — POSIX helper for a custom-family Evaluate.
#
# After `report.json` exists, write `generic-custom-v1` results JSON bound
# to the scored *root* `primary_value` / `claim_holds` and the guest
# identity env. Nested keys with the same name are ignored.
# File name: `results.json`, or `PROOF_PARAM_RESULTS_PATH` when it is one
# safe `*.json` segment (8–64 chars, no `/`, no leading `.`). Multiple
# dots are allowed (`audit.v1.final.json`). A bad pin is fail-closed
# (no write, no fallback name). Harbor / trial contracts write their own
# document (see `rlm_fc_in_guest_harbor/harness/summarize.py`).
#
# Source or append this from an adaptor `run`. Missing `report.json` is a
# no-op (the guest still fail-closes Evaluate with no results file).

# Root-object JSON atom (number / true / false / null). Nested names ignored.
_proof_root_json_atom() {
    awk -v want="$2" '
    BEGIN {
        depth = 0
        in_str = 0
        esc = 0
        reading_key = 0
        after_colon = 0
        curkey = ""
        keybuf = ""
    }
    {
        n = length($0)
        for (i = 1; i <= n; i++) {
            c = substr($0, i, 1)
            if (in_str) {
                if (esc) { esc = 0; keybuf = keybuf c; continue }
                if (c == "\\") { esc = 1; keybuf = keybuf c; continue }
                if (c == "\"") {
                    in_str = 0
                    if (reading_key) { curkey = keybuf; reading_key = 0 }
                    continue
                }
                keybuf = keybuf c
                continue
            }
            if (c == "\"") {
                in_str = 1
                keybuf = ""
                if (depth == 1 && !after_colon) reading_key = 1
                continue
            }
            if (c == "{" || c == "[") { depth++; after_colon = 0; continue }
            if (c == "}" || c == "]") { depth--; after_colon = 0; continue }
            if (c == ":") { after_colon = 1; continue }
            if (c == ",") { after_colon = 0; curkey = ""; continue }
            if (c ~ /[ \t\r\n]/) continue
            if (after_colon && depth == 1 && curkey == want) {
                val = c
                i++
                while (i <= n) {
                    d = substr($0, i, 1)
                    if (d ~ /[,} \t\r\n\]]/) break
                    val = val d
                    i++
                }
                if (val ~ /^-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][+-]?[0-9]+)?$/ \
                    || val == "true" || val == "false" || val == "null") {
                    print val
                    exit 0
                }
                exit 1
            }
        }
    }
    ' "$1"
}

if [ -f "${PROOF_OUTPUT_DIR:?}/report.json" ]; then
    _results_name=results.json
    if [ -n "${PROOF_PARAM_RESULTS_PATH:-}" ]; then
        _pin="${PROOF_PARAM_RESULTS_PATH}"
        _n=${#_pin}
        case "$_pin" in
            */* | .* | . | ..)
                echo "results_path is not a single safe .json file name" >&2
                exit 2
                ;;
        esac
        if [ "$_n" -lt 8 ] || [ "$_n" -gt 64 ]; then
            echo "results_path is not a single safe .json file name" >&2
            exit 2
        fi
        case "$_pin" in
            *.json | *.JSON) ;;
            *)
                echo "results_path is not a single safe .json file name" >&2
                exit 2
                ;;
        esac
        case "$_pin" in
            *[!A-Za-z0-9._-]*)
                echo "results_path is not a single safe .json file name" >&2
                exit 2
                ;;
        esac
        _results_name="$_pin"
        unset _pin _n
    fi
    _pv="$(_proof_root_json_atom "$PROOF_OUTPUT_DIR/report.json" primary_value)" || _pv=
    _ch="$(_proof_root_json_atom "$PROOF_OUTPUT_DIR/report.json" claim_holds)" || _ch=
    [ -n "$_ch" ] || _ch=false
    if [ -z "$_pv" ]; then
        echo "report.json missing root primary_value" >&2
        exit 2
    fi
    cat > "$PROOF_OUTPUT_DIR/$_results_name" <<EOF
{"schema_version":1,"contract":"generic-custom-v1","topic_id":"${PROOF_TOPIC_ID}","custom_id":"${PROOF_CUSTOM_ID}","submission_digest":"${PROOF_SUBMISSION_DIGEST}","artifact_digest":"${PROOF_ARTIFACT_DIGEST}","primary_value":${_pv},"claim_holds":${_ch},"display":{"ok":true}}
EOF
    unset _results_name _pv _ch
fi
