#!/usr/bin/env bash
set -euo pipefail

# Capture live TS CLI stream-json output for structural comparison against
# the Rust goldens.
#
# Live captures go to *.ts.capture.jsonl — they are NOT the checked-in test
# goldens (*.ts.golden.jsonl). The checked-in goldens are hand-crafted with
# matching response content so they can be byte-compared after normalization.
# Live captures have real API responses, so they differ in content; this
# script shows a normalized structural diff instead of asserting equality.
#
# Usage:
#   scripts/generate-ts-goldens.sh [scenario]
#
#   scripts/generate-ts-goldens.sh simple-text      # one scenario
#   scripts/generate-ts-goldens.sh                  # all scenarios
#
# Environment:
#   TS_CLI     Path to the TypeScript Claude Code CLI binary.
#              Default: "claude" (must be on PATH).
#
# Prerequisites:
#   - TypeScript Claude Code CLI installed (npm install -g @anthropic-ai/claude-code)
#   - A valid Anthropic API key (configured in the CLI or via ANTHROPIC_API_KEY)
#
# To regenerate the RUST goldens:
#   ORBCODE_UPDATE_STREAM_JSON_GOLDENS=1 cargo test -p orbcode --test compat_stream_json -- --test-threads=1
#
# To run the byte-equal automated tests (uses checked-in hand-crafted goldens):
#   cargo test -p orbcode-compat-fixtures compat_ts_rs

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "${script_dir}/.." && pwd)"
fixtures_dir="${repo_root}/compat-fixtures/fixtures/stream_json"

TS_CLI="${TS_CLI:-claude}"

if ! command -v "$TS_CLI" &>/dev/null; then
    echo "Error: TS CLI not found at '${TS_CLI}'."
    echo "Install: npm install -g @anthropic-ai/claude-code"
    echo "Or set TS_CLI=/path/to/claude"
    exit 1
fi

echo "==> Using TS CLI: $(command -v "$TS_CLI")"
echo "==> TS CLI version: $("$TS_CLI" --version 2>/dev/null || echo 'unknown')"

# Map kebab-case scenario names to prompts and capture files.
declare -A SCENARIOS
SCENARIOS[simple-text]="say hi"
SCENARIOS[tool-round-trip]="#tool:bash {\"command\":\"echo hi\"}"

declare -A CAPTURE_FILES
CAPTURE_FILES[simple-text]="simple_text.ts.capture.jsonl"
CAPTURE_FILES[tool-round-trip]="tool_round_trip.ts.capture.jsonl"

declare -A RS_GOLDEN_FILES
RS_GOLDEN_FILES[simple-text]="simple_text.jsonl"
RS_GOLDEN_FILES[tool-round-trip]="tool_round_trip.jsonl"

declare -A EXTRA_FLAGS
EXTRA_FLAGS[simple-text]=""
EXTRA_FLAGS[tool-round-trip]="--permission-mode default --allowedTools Bash"

run_scenario() {
    local name=$1
    local prompt="${SCENARIOS[$name]}"
    local capfile="${fixtures_dir}/${CAPTURE_FILES[$name]}"
    local rs_golden="${fixtures_dir}/${RS_GOLDEN_FILES[$name]}"
    local extra="${EXTRA_FLAGS[$name]:-}"

    echo "==> Capturing TS output: ${name} -> ${CAPTURE_FILES[$name]}"

    # Run the TS CLI in headless stream-json mode.
    # --verbose is required for stream-json with --print.
    # --include-partial-messages enables streaming deltas (content_block_delta).
    # --no-session-persistence avoids polluting the session list.
    # --bare skips hooks/plugins for cleaner output.
    # shellcheck disable=SC2086
    "$TS_CLI" \
        --output-format stream-json \
        --verbose \
        --include-partial-messages \
        --no-session-persistence \
        --bare \
        ${extra} \
        -p "${prompt}" \
        > "${capfile}" 2>/dev/null \
        || true

    if [[ ! -s "${capfile}" ]]; then
        echo "    Warning: produced empty output. Check TS CLI / API key configuration."
        return 1
    fi

    local lines
    lines=$(wc -l < "${capfile}" | tr -d ' ')
    echo "    Captured ${lines} lines -> ${capfile}"

    # Show a raw diff against the RS golden if both exist.  Normalization lives
    # in the `compat_ts_rs` test (run below), not in a standalone binary, so this
    # is an eyeball aid only — expect volatile fields (ids, timestamps) to differ.
    if [[ -f "${rs_golden}" ]]; then
        echo "    Raw diff vs RS golden (${RS_GOLDEN_FILES[$name]}):"
        diff "${rs_golden}" "${capfile}" \
             && echo "      (identical)" \
             || echo "      (differences above are expected — content varies with live API)"
    fi
}

if [[ $# -gt 0 ]]; then
    scenario="$1"
    if [[ -z "${SCENARIOS[$scenario]+x}" ]]; then
        echo "Unknown scenario: ${scenario}"
        echo "Available: ${!SCENARIOS[*]}"
        exit 1
    fi
    run_scenario "$scenario"
else
    echo "==> Capturing all TS stream-json scenarios..."
    for name in "${!SCENARIOS[@]}"; do
        run_scenario "$name"
    done
fi

echo ""
echo "Done. Live captures saved to *.ts.capture.jsonl (gitignored)."
echo "Checked-in goldens (*.ts.golden.jsonl) are NOT modified."
echo ""
echo "Automated byte-equal tests (uses hand-crafted goldens):"
echo "  cargo test -p orbcode-compat-fixtures compat_ts_rs"
