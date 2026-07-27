#!/usr/bin/env bash
# Local tmux smoke for the native scrollback TUI path.
#
# This is intentionally not wired into CI: it needs a local tmux binary and a
# host terminal environment. It gives a repeatable check for the remaining
# manual tmux surface by running orbcode inside a real tmux pane and validating
# the prompt-mode smoke output.
#
# Usage:
#   scripts/tui-native-scrollback-tmux-smoke.sh
#   scripts/tui-native-scrollback-tmux-smoke.sh --release
#   scripts/tui-native-scrollback-tmux-smoke.sh --interactive
#   scripts/tui-native-scrollback-tmux-smoke.sh --small-height-prompt-smoke
#   scripts/tui-native-scrollback-tmux-smoke.sh --resize-smoke
#   scripts/tui-native-scrollback-tmux-smoke.sh --resize-streaming-smoke
#   scripts/tui-native-scrollback-tmux-smoke.sh --final-answer-smoke
#   scripts/tui-native-scrollback-tmux-smoke.sh --metrics-smoke
#   scripts/tui-native-scrollback-tmux-smoke.sh --metrics-fixture-smoke
#   scripts/tui-native-scrollback-tmux-smoke.sh --mouse-precondition-smoke
#   scripts/tui-native-scrollback-tmux-smoke.sh --mouse-drag-smoke
#   scripts/tui-native-scrollback-tmux-smoke.sh --mouse-wheel-smoke
#   scripts/tui-native-scrollback-tmux-smoke.sh --transcript-pager-smoke
#   scripts/tui-native-scrollback-tmux-smoke.sh --transcript-pager-deferred-smoke

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${repo_root}"

profile="debug"
cargo_profile_args=()
target_dir_segment="debug"
interactive=0
small_height_prompt_smoke=0
resize_smoke=0
resize_streaming_smoke=0
final_answer_smoke=0
metrics_smoke=0
metrics_fixture_smoke=0
mouse_precondition_smoke=0
mouse_drag_smoke=0
mouse_wheel_smoke=0
transcript_pager_smoke=0
transcript_pager_deferred_smoke=0
case "${1:-}" in
    "")
        ;;
    --release)
        profile="release"
        cargo_profile_args=(--release)
        target_dir_segment="release"
        ;;
    --interactive)
        interactive=1
        ;;
    --small-height-prompt-smoke)
        small_height_prompt_smoke=1
        ;;
    --resize-smoke)
        resize_smoke=1
        ;;
    --resize-streaming-smoke)
        resize_streaming_smoke=1
        ;;
    --final-answer-smoke)
        final_answer_smoke=1
        ;;
    --metrics-smoke)
        metrics_smoke=1
        ;;
    --metrics-fixture-smoke)
        metrics_fixture_smoke=1
        ;;
    --mouse-precondition-smoke)
        mouse_precondition_smoke=1
        ;;
    --mouse-drag-smoke)
        mouse_drag_smoke=1
        ;;
    --mouse-wheel-smoke)
        mouse_wheel_smoke=1
        ;;
    --transcript-pager-smoke)
        transcript_pager_smoke=1
        ;;
    --transcript-pager-deferred-smoke)
        transcript_pager_deferred_smoke=1
        ;;
    --help|-h)
        sed -n '2,21p' "$0"
        exit 0
        ;;
    *)
        echo "ERROR: unknown argument: $1" >&2
        exit 2
        ;;
esac

if ! command -v tmux >/dev/null 2>&1; then
    echo "ERROR: tmux is required for this local smoke." >&2
    exit 127
fi
if [[ "${mouse_drag_smoke}" -eq 1 || "${mouse_wheel_smoke}" -eq 1 ]] && ! command -v expect >/dev/null 2>&1; then
    echo "ERROR: expect is required for mouse event smokes." >&2
    exit 127
fi

echo "==> cargo build -p orbcode ${cargo_profile_args[*]-}"
cargo build -p orbcode ${cargo_profile_args[@]+"${cargo_profile_args[@]}"}

bin_suffix=""
case "$(uname -s 2>/dev/null || echo unknown)" in
    MINGW*|MSYS*|CYGWIN*) bin_suffix=".exe" ;;
esac
binary="${repo_root}/target/${target_dir_segment}/orbcode${bin_suffix}"
if [[ ! -x "${binary}" ]]; then
    echo "ERROR: built binary not found at ${binary}" >&2
    exit 1
fi

scratch="$(mktemp -d)"
if [[ "${interactive}" -eq 1 ]]; then
    server_name="orbcode-scrollback-manual-$$"
elif [[ "${small_height_prompt_smoke}" -eq 1 ]]; then
    server_name="orbcode-scrollback-small-height-$$"
elif [[ "${resize_smoke}" -eq 1 ]]; then
    server_name="orbcode-scrollback-resize-$$"
elif [[ "${resize_streaming_smoke}" -eq 1 ]]; then
    server_name="orbcode-scrollback-resize-streaming-$$"
elif [[ "${metrics_smoke}" -eq 1 ]]; then
    server_name="orbcode-scrollback-metrics-$$"
elif [[ "${metrics_fixture_smoke}" -eq 1 ]]; then
    server_name="orbcode-scrollback-metrics-fixture-$$"
elif [[ "${mouse_precondition_smoke}" -eq 1 ]]; then
    server_name="orbcode-scrollback-mouse-$$"
elif [[ "${mouse_drag_smoke}" -eq 1 ]]; then
    server_name="orbcode-scrollback-drag-$$"
elif [[ "${mouse_wheel_smoke}" -eq 1 ]]; then
    server_name="orbcode-scrollback-wheel-$$"
elif [[ "${transcript_pager_smoke}" -eq 1 ]]; then
    server_name="orbcode-scrollback-transcript-$$"
else
    server_name="orbcode-scrollback-smoke-$$"
fi
session_name="orbcode-scrollback-smoke"
target="${session_name}:0.0"
raw_output="${scratch}/pane.raw"
runner="${scratch}/run-tui-smoke.sh"
metrics_file="${scratch}/render-metrics.jsonl"

cleanup() {
    tmux -L "${server_name}" kill-server >/dev/null 2>&1 || true
    rm -rf "${scratch}"
}
if [[ "${interactive}" -eq 0 ]]; then
    trap cleanup EXIT
fi

last_nonempty_line() {
    awk 'NF { line = $0 } END { print line }'
}

contains_non_session_shell_chrome() {
    grep -Eq 'run-tui-smoke\.sh|bash-[0-9]+\.[0-9]+[$]|[$] bash '
}

contains_transcript_table_chrome() {
    grep -Eq '[│┌┐└┘├┤┬┴┼]'
}

mkdir -p "${scratch}/home" "${scratch}/cwd"
cat >"${scratch}/home/settings.json" <<'JSON'
{"env":{"ANTHROPIC_API_KEY":"stub-key"}}
JSON

metrics_env_lines=""
if [[ "${metrics_smoke}" -eq 1 ]]; then
    metrics_env_lines=$(cat <<EOF
export ORBCODE_TUI_RENDER_METRICS=1
export ORBCODE_TUI_RENDER_METRICS_PATH="${metrics_file}"
EOF
)
fi

if [[ "${interactive}" -eq 1 ]]; then
    cat >"${runner}" <<EOF
#!/usr/bin/env bash
set -euo pipefail
export ORBCODE_HOME="${scratch}/home"
export HOME="${scratch}/home"
export ANTHROPIC_BASE_URL="stub://anthropic"
export ORBCODE_PROVIDER="anthropic"
export RUST_LOG="warn"
export TERM="xterm-256color"
export ORBCODE_TUI_RENDER_METRICS=1
export ORBCODE_TUI_MANUAL_SCROLLBACK_FIXTURE=1
cd "${scratch}/cwd"
exec "${binary}" tui
EOF
elif [[ "${resize_smoke}" -eq 1 || "${resize_streaming_smoke}" -eq 1 || "${final_answer_smoke}" -eq 1 || "${metrics_fixture_smoke}" -eq 1 || "${mouse_precondition_smoke}" -eq 1 || "${mouse_drag_smoke}" -eq 1 || "${mouse_wheel_smoke}" -eq 1 || "${transcript_pager_smoke}" -eq 1 || "${transcript_pager_deferred_smoke}" -eq 1 ]]; then
    cat >"${runner}" <<EOF
#!/usr/bin/env bash
set -euo pipefail
export ORBCODE_HOME="${scratch}/home"
export HOME="${scratch}/home"
export ANTHROPIC_BASE_URL="stub://anthropic"
export ORBCODE_PROVIDER="anthropic"
export RUST_LOG="warn"
export TERM="xterm-256color"
export ORBCODE_TUI_RENDER_METRICS=1
export ORBCODE_TUI_RENDER_METRICS_PATH="${metrics_file}"
export ORBCODE_TUI_MANUAL_SCROLLBACK_FIXTURE=1
$(if [[ "${resize_streaming_smoke}" -eq 1 ]]; then printf '%s\n' 'export ORBCODE_TUI_RESIZE_STREAMING_FIXTURE=1'; fi)
$(if [[ "${transcript_pager_deferred_smoke}" -eq 1 ]]; then printf '%s\n' 'export ORBCODE_TUI_PAGER_DEFERRED_FIXTURE=1'; fi)
$(if [[ "${final_answer_smoke}" -eq 1 ]]; then printf '%s\n' 'export ORBCODE_TUI_FINAL_ANSWER_FIXTURE=1'; fi)
cd "${scratch}/cwd"
exec "${binary}" tui
EOF
else
    cat >"${runner}" <<EOF
#!/usr/bin/env bash
set -euo pipefail
export ORBCODE_HOME="${scratch}/home"
export HOME="${scratch}/home"
export ANTHROPIC_BASE_URL="stub://anthropic"
export ORBCODE_PROVIDER="anthropic"
export RUST_LOG="warn"
export TERM="xterm-256color"
export ORBCODE_TUI_PTY_SMOKE_EXIT_AFTER_FIRST_FRAME=1
export ORBCODE_TUI_PTY_SMOKE_HISTORY_SUMMARY=1
${metrics_env_lines}
cd "${scratch}/cwd"
set +e
"${binary}" tui
status=\$?
set -e
printf '\\n__ORBCODE_TMUX_SMOKE_DONE:%s__\\n' "\${status}"
EOF
fi
chmod +x "${runner}"

if [[ "${interactive}" -eq 1 ]]; then
    echo "==> launching interactive tmux validation session ${server_name}"
    tmux -L "${server_name}" -f /dev/null new-session -d -s "${session_name}" -x 100 -y 30 \
        "bash --noprofile --norc"
    tmux -L "${server_name}" set-option -g mouse on
    tmux -L "${server_name}" send-keys -t "${target}" -l "bash $(printf '%q' "${runner}")"
    tmux -L "${server_name}" send-keys -t "${target}" C-m
    cat <<EOF
Interactive tmux validation session is ready.

Attach:
  tmux -L ${server_name} attach -t ${session_name}

Checks to perform inside the attached pane:
  1. Confirm prompt mode does not enter alternate screen.
  2. Use mouse wheel and drag selection over the preloaded scrollback fixture;
     this session has tmux mouse mode enabled, and tmux/terminal should handle
     both natively.
  3. Resize the pane and confirm the preloaded summary history visibly reflows
     while the inline session remains usable.
  4. Inspect render metrics output for bounded prompt redraw work.

Cleanup when finished:
  tmux -L ${server_name} kill-server
  rm -rf ${scratch}
EOF
    exit 0
fi

if [[ "${small_height_prompt_smoke}" -eq 1 ]]; then
    cat >"${runner}" <<EOF
#!/usr/bin/env bash
set -euo pipefail
export ORBCODE_HOME="${scratch}/home"
export HOME="${scratch}/home"
export ANTHROPIC_BASE_URL="stub://anthropic"
export ORBCODE_PROVIDER="anthropic"
export RUST_LOG="warn"
export TERM="xterm-256color"
cd "${scratch}/cwd"
exec "${binary}" tui
EOF
    chmod +x "${runner}"

    count_blank_gap() {
        local text="$1"
        local end_regex="$2"
        awk -v end_regex="${end_regex}" '
            /apply\./ { seen = 1; blanks = 0; next }
            seen && $0 ~ end_regex { print blanks; found = 1; exit }
            seen && $0 ~ /^[[:space:]]*$/ { blanks++; next }
            END { if (!found) print -1 }
        ' <<<"${text}"
    }

    echo "==> launching tmux small-height prompt smoke server ${server_name}"
    tmux -L "${server_name}" -f /dev/null new-session -d -s "${session_name}" -x 84 -y 16 \
        "bash --noprofile --norc"
    tmux -L "${server_name}" send-keys -t "${target}" -l "bash $(printf '%q' "${runner}")"
    tmux -L "${server_name}" send-keys -t "${target}" C-m

    capture=""
    saw_tui=0
    for _ in {1..120}; do
        capture="$(tmux -L "${server_name}" capture-pane -p -J -S -200 -t "${target}" 2>/dev/null || true)"
        current="$(tmux -L "${server_name}" display-message -p -t "${target}" '#{pane_current_command} #{pane_dead}' 2>/dev/null || true)"
        if [[ "${current}" == "orbcode 0" ]] &&
            grep -q 'Orb Code v' <<<"${capture}" &&
            grep -q '^❯' <<<"${capture}"; then
            saw_tui=1
            break
        fi
        sleep 0.1
    done
    if [[ "${saw_tui}" -ne 1 ]]; then
        echo "ERROR: small-height prompt smoke did not observe the TUI prompt." >&2
        echo "----- captured pane -----" >&2
        printf '%s\n' "${capture}" >&2
        exit 1
    fi

    tmux -L "${server_name}" send-keys -t "${target}" -l "/al"
    suggestion_capture=""
    saw_suggestions=0
    for _ in {1..120}; do
        suggestion_capture="$(tmux -L "${server_name}" capture-pane -p -J -S -200 -t "${target}" 2>/dev/null || true)"
        if grep -q '/allow-all' <<<"${suggestion_capture}" &&
            grep -q '/permissions' <<<"${suggestion_capture}"; then
            saw_suggestions=1
            break
        fi
        sleep 0.1
    done
    if [[ "${saw_suggestions}" -ne 1 ]]; then
        echo "ERROR: small-height prompt smoke did not observe slash suggestions." >&2
        echo "----- captured pane -----" >&2
        printf '%s\n' "${suggestion_capture}" >&2
        exit 1
    fi

    tmux -L "${server_name}" send-keys -t "${target}" -l "low-all on"
    tmux -L "${server_name}" send-keys -t "${target}" C-m

    allow_capture=""
    idle_gap=-1
    saw_allow=0
    for _ in {1..120}; do
        allow_capture="$(tmux -L "${server_name}" capture-pane -p -J -S - -t "${target}" 2>/dev/null || true)"
        idle_gap="$(count_blank_gap "${allow_capture}" '^─')"
        if grep -q 'apply\.' <<<"${allow_capture}" &&
            [[ "${idle_gap}" -ge 0 ]]; then
            saw_allow=1
            break
        fi
        sleep 0.1
    done
    if [[ "${saw_allow}" -ne 1 ]]; then
        echo "ERROR: small-height prompt smoke did not observe /allow-all output." >&2
        echo "----- captured pane -----" >&2
        printf '%s\n' "${allow_capture}" >&2
        exit 1
    fi
    if [[ "${idle_gap}" -ne 1 ]]; then
        echo "ERROR: /allow-all output left ${idle_gap} blank rows before the prompt surface; expected exactly 1." >&2
        echo "----- captured pane -----" >&2
        printf '%s\n' "${allow_capture}" >&2
        exit 1
    fi
    allow_divider_count="$(grep -c '^─' <<<"${allow_capture}" || true)"
    if [[ "${allow_divider_count}" -lt 2 ]]; then
        echo "ERROR: /allow-all prompt surface rendered only ${allow_divider_count} divider rows." >&2
        echo "----- captured pane -----" >&2
        printf '%s\n' "${allow_capture}" >&2
        exit 1
    fi
    banner_count="$(grep -c 'Orb Code v' <<<"${allow_capture}" || true)"
    allow_match_count="$(grep -c '/allow-all' <<<"${allow_capture}" || true)"
    if [[ "${banner_count}" -gt 1 || "${allow_match_count}" -gt 1 ]]; then
        echo "ERROR: small-height prompt smoke leaked transient redraw snapshots into scrollback." >&2
        echo "banner_count=${banner_count} allow_match_count=${allow_match_count}" >&2
        echo "----- captured pane -----" >&2
        printf '%s\n' "${allow_capture}" >&2
        exit 1
    fi

    prompt='summarize @~/github/sample-repo structure'
    tmux -L "${server_name}" send-keys -t "${target}" -l "${prompt}"
    tmux -L "${server_name}" send-keys -t "${target}" C-m

    prompt_capture=""
    prompt_gap=-1
    saw_prompt=0
    for _ in {1..120}; do
        prompt_capture="$(tmux -L "${server_name}" capture-pane -p -J -S -200 -t "${target}" 2>/dev/null || true)"
        prompt_gap="$(count_blank_gap "${prompt_capture}" "^› ${prompt}")"
        if grep -q "› ${prompt}" <<<"${prompt_capture}" &&
            [[ "${prompt_gap}" -ge 0 ]]; then
            saw_prompt=1
            break
        fi
        sleep 0.1
    done
    if [[ "${saw_prompt}" -ne 1 ]]; then
        echo "ERROR: small-height prompt smoke did not observe submitted prompt." >&2
        echo "----- captured pane -----" >&2
        printf '%s\n' "${prompt_capture}" >&2
        exit 1
    fi
    if [[ "${prompt_gap}" -ne 1 ]]; then
        echo "ERROR: submitted prompt left ${prompt_gap} blank rows after /allow-all output; expected exactly 1." >&2
        echo "----- captured pane -----" >&2
        printf '%s\n' "${prompt_capture}" >&2
        exit 1
    fi
    prompt_divider_count="$(grep -c '^─' <<<"${prompt_capture}" || true)"
    if [[ "${prompt_divider_count}" -lt 2 ]]; then
        echo "ERROR: submitted prompt surface rendered only ${prompt_divider_count} divider rows." >&2
        echo "----- captured pane -----" >&2
        printf '%s\n' "${prompt_capture}" >&2
        exit 1
    fi

    echo "OK: tmux small-height prompt smoke kept /allow-all and the next prompt adjacent."
    exit 0
fi

if [[ "${resize_smoke}" -eq 1 ]]; then
    echo "==> launching tmux resize smoke server ${server_name}"
    tmux -L "${server_name}" -f /dev/null new-session -d -s "${session_name}" -x 100 -y 30 \
        "bash --noprofile --norc"
    tmux -L "${server_name}" send-keys -t "${target}" -l "bash $(printf '%q' "${runner}")"
    tmux -L "${server_name}" send-keys -t "${target}" C-m

    saw_tui=0
    for _ in {1..120}; do
        current="$(tmux -L "${server_name}" display-message -p -t "${target}" '#{pane_current_command} #{pane_dead}' 2>/dev/null || true)"
        if [[ "${current}" == "orbcode 0" ]]; then
            saw_tui=1
            break
        fi
        sleep 0.1
    done
    if [[ "${saw_tui}" -ne 1 ]]; then
        echo "ERROR: resize smoke did not observe the TUI process running." >&2
        exit 1
    fi
    fixture_capture=""
    saw_fixture=0
    for _ in {1..120}; do
        fixture_capture="$(tmux -L "${server_name}" capture-pane -p -J -S -300 -t "${target}")"
        if grep -q 'Manual scrollback fixture loaded for tmux validation' <<<"${fixture_capture}"; then
            saw_fixture=1
            break
        fi
        sleep 0.1
    done
    if [[ "${saw_fixture}" -ne 1 ]]; then
        echo "ERROR: resize smoke did not observe the manual scrollback fixture." >&2
        echo "----- captured pane -----" >&2
        printf '%s\n' "${fixture_capture}" >&2
        exit 1
    fi

    tmux -L "${server_name}" resize-window -t "${session_name}:0" -x 72 -y 20
    resized="$(tmux -L "${server_name}" display-message -p -t "${target}" '#{pane_width}x#{pane_height} #{pane_current_command} #{pane_dead}')"
    if [[ "${resized}" != "72x20 orbcode 0" ]]; then
        echo "ERROR: TUI did not remain running after shrink resize: ${resized}" >&2
        exit 1
    fi
    narrow_capture=""
    saw_narrow_reflow=0
    for _ in {1..120}; do
        narrow_capture="$(tmux -L "${server_name}" capture-pane -p -J -S -300 -t "${target}")"
        if grep -q '^  confirm retained summary history reflows while the live prompt$' <<<"${narrow_capture}" &&
            grep -q '^  remains usable$' <<<"${narrow_capture}"; then
            saw_narrow_reflow=1
            break
        fi
        sleep 0.1
    done
    if [[ "${saw_narrow_reflow}" -ne 1 ]]; then
        echo "ERROR: shrink resize did not show retained summary history rewrapped at 72 columns." >&2
        echo "----- captured pane -----" >&2
        printf '%s\n' "${narrow_capture}" >&2
        exit 1
    fi

    tmux -L "${server_name}" resize-window -t "${session_name}:0" -x 100 -y 30
    restored="$(tmux -L "${server_name}" display-message -p -t "${target}" '#{pane_width}x#{pane_height} #{pane_current_command} #{pane_dead}')"
    if [[ "${restored}" != "100x30 orbcode 0" ]]; then
        echo "ERROR: TUI did not remain running after restore resize: ${restored}" >&2
        exit 1
    fi
    wide_capture=""
    saw_wide_reflow=0
    for _ in {1..120}; do
        wide_capture="$(tmux -L "${server_name}" capture-pane -p -J -S -300 -t "${target}")"
        if grep -q 'manual scrollback fixture summary 47: resize this tmux pane and confirm retained summary history$' <<<"${wide_capture}" &&
            grep -q '^  reflows while the live prompt remains usable$' <<<"${wide_capture}"; then
            saw_wide_reflow=1
            break
        fi
        sleep 0.1
    done
    if [[ "${saw_wide_reflow}" -ne 1 ]]; then
        echo "ERROR: restore resize did not show retained summary history rewrapped at 100 columns." >&2
        echo "----- captured pane -----" >&2
        printf '%s\n' "${wide_capture}" >&2
        exit 1
    fi

    echo "OK: tmux resize smoke kept the fixture-backed TUI running and rewrapped retained summary history across pane resizes."
    exit 0
fi

if [[ "${resize_streaming_smoke}" -eq 1 ]]; then
    echo "==> launching tmux resize-while-streaming smoke server ${server_name}"
    tmux -L "${server_name}" -f /dev/null new-session -d -s "${session_name}" -x 100 -y 30 \
        "bash --noprofile --norc"
    tmux -L "${server_name}" send-keys -t "${target}" -l "bash $(printf '%q' "${runner}")"
    tmux -L "${server_name}" send-keys -t "${target}" C-m

    capture=""
    saw_active_thinking=0
    for _ in {1..120}; do
        capture="$(tmux -L "${server_name}" capture-pane -p -J -S -300 -t "${target}" 2>/dev/null || true)"
        if grep -q 'active resize smoke thought' <<<"${capture}" &&
            grep -q 'Thinking' <<<"${capture}"; then
            saw_active_thinking=1
            break
        fi
        sleep 0.1
    done
    if [[ "${saw_active_thinking}" -ne 1 ]]; then
        echo "ERROR: resize-while-streaming smoke did not observe active thinking fixture." >&2
        echo "----- captured pane -----" >&2
        printf '%s\n' "${capture}" >&2
        exit 1
    fi

    tmux -L "${server_name}" resize-window -t "${session_name}:0" -x 72 -y 22
    sleep 0.2
    tmux -L "${server_name}" resize-window -t "${session_name}:0" -x 48 -y 18
    sleep 0.2
    tmux -L "${server_name}" resize-window -t "${session_name}:0" -x 100 -y 30
    sleep 0.2
    tmux -L "${server_name}" resize-window -t "${session_name}:0" -x 100 -y 22
    sleep 0.2
    tmux -L "${server_name}" resize-window -t "${session_name}:0" -x 100 -y 16
    sleep 0.2
    tmux -L "${server_name}" resize-window -t "${session_name}:0" -x 100 -y 12
    sleep 0.2
    tmux -L "${server_name}" resize-window -t "${session_name}:0" -x 100 -y 30

    resized_capture=""
    saw_restored_active=0
    for _ in {1..120}; do
        resized_capture="$(tmux -L "${server_name}" capture-pane -p -J -S - -t "${target}" 2>/dev/null || true)"
        active_thinking_count="$(grep -c 'active resize smoke thought' <<<"${resized_capture}" || true)"
        live_prompt_count="$(grep -c '^❯' <<<"${resized_capture}" || true)"
        if [[ "${active_thinking_count}" == "1" && "${live_prompt_count}" == "1" ]]; then
            saw_restored_active=1
            break
        fi
        sleep 0.1
    done
    if [[ "${saw_restored_active}" -ne 1 ]]; then
        echo "ERROR: resize-while-streaming smoke expected one active thinking preview and one live prompt after repeated resizes; saw active_thinking=${active_thinking_count:-0} live_prompt=${live_prompt_count:-0}." >&2
        echo "----- captured scrollback -----" >&2
        printf '%s\n' "${resized_capture}" >&2
        exit 1
    fi
    request_status_count="$(grep -c 'Thinking' <<<"${resized_capture}" || true)"
    if [[ "${request_status_count}" -gt 2 ]]; then
        echo "ERROR: resize-while-streaming smoke found stacked Thinking/status rows after repeated resizes: ${request_status_count}." >&2
        echo "----- captured scrollback -----" >&2
        printf '%s\n' "${resized_capture}" >&2
        exit 1
    fi
    committed_marker_count="$(grep -c 'resize streaming committed history marker' <<<"${resized_capture}" || true)"
    if [[ "${committed_marker_count}" != "1" ]]; then
        echo "ERROR: resize-while-streaming smoke expected one committed history marker after repeated resizes; saw ${committed_marker_count}." >&2
        echo "----- captured scrollback -----" >&2
        printf '%s\n' "${resized_capture}" >&2
        exit 1
    fi

    tmux -L "${server_name}" send-keys -t "${target}" C-o
    streaming_pager_capture=""
    streaming_pager_visible_capture=""
    saw_streaming_pager=0
    for _ in {1..120}; do
        streaming_pager_state="$(tmux -L "${server_name}" display-message -p -t "${target}" '#{alternate_on} #{pane_current_command} #{pane_dead} #{pane_in_mode}')"
        streaming_pager_capture="$(tmux -L "${server_name}" capture-pane -p -J -t "${target}" 2>/dev/null || true)"
        streaming_pager_visible_capture="${streaming_pager_capture}"
        if [[ "${streaming_pager_state}" == "1 orbcode 0 0" ]] &&
            grep -q 'active resize smoke thought' <<<"${streaming_pager_capture}" &&
            ! contains_non_session_shell_chrome <<<"${streaming_pager_visible_capture}"; then
            saw_streaming_pager=1
            break
        fi
        sleep 0.1
    done
    if [[ "${saw_streaming_pager}" -ne 1 ]]; then
        echo "ERROR: resize-while-streaming smoke did not open the app transcript pager during active streaming." >&2
        echo "----- pane state -----" >&2
        printf '%s\n' "${streaming_pager_state}" >&2
        echo "----- captured pager -----" >&2
        printf '%s\n' "${streaming_pager_capture}" >&2
        echo "----- visible pager -----" >&2
        printf '%s\n' "${streaming_pager_visible_capture}" >&2
        exit 1
    fi

    tmux -L "${server_name}" send-keys -t "${target}" C-o
    streaming_restore_capture=""
    saw_streaming_restore=0
    for _ in {1..120}; do
        streaming_restore_state="$(tmux -L "${server_name}" display-message -p -t "${target}" '#{alternate_on} #{pane_current_command} #{pane_dead} #{pane_in_mode}')"
        streaming_restore_capture="$(tmux -L "${server_name}" capture-pane -p -J -S - -t "${target}" 2>/dev/null || true)"
        live_prompt_count="$(grep -c '^❯' <<<"${streaming_restore_capture}" || true)"
        if [[ "${streaming_restore_state}" == "0 orbcode 0 0" ]] &&
            grep -q 'active resize smoke thought' <<<"${streaming_restore_capture}" &&
            [[ "${live_prompt_count}" == "1" ]]; then
            saw_streaming_restore=1
            break
        fi
        sleep 0.1
    done
    if [[ "${saw_streaming_restore}" -ne 1 ]]; then
        echo "ERROR: resize-while-streaming smoke did not restore the live prompt after closing the app transcript pager." >&2
        echo "----- pane state -----" >&2
        printf '%s\n' "${streaming_restore_state}" >&2
        echo "----- captured primary -----" >&2
        printf '%s\n' "${streaming_restore_capture}" >&2
        exit 1
    fi
    streaming_committed_marker_count="$(grep -c 'resize streaming committed history marker' <<<"${streaming_restore_capture}" || true)"
    if [[ "${streaming_committed_marker_count}" -gt 1 ]]; then
        echo "ERROR: resize-while-streaming smoke found duplicated committed marker after closing the app transcript pager; saw ${streaming_committed_marker_count}." >&2
        echo "----- captured primary -----" >&2
        printf '%s\n' "${streaming_restore_capture}" >&2
        exit 1
    fi
    streaming_prompt_line="$(grep '^❯' <<<"${streaming_restore_capture}" | tail -n 1 || true)"
    if contains_transcript_table_chrome <<<"${streaming_prompt_line}"; then
        echo "ERROR: resize-while-streaming smoke found transcript table content on the restored prompt line." >&2
        echo "----- prompt line -----" >&2
        printf '%s\n' "${streaming_prompt_line}" >&2
        echo "----- captured primary -----" >&2
        printf '%s\n' "${streaming_restore_capture}" >&2
        exit 1
    fi
    streaming_mode_line="$(grep '^--' <<<"${streaming_restore_capture}" | tail -n 1 || true)"
    if contains_transcript_table_chrome <<<"${streaming_mode_line}"; then
        echo "ERROR: resize-while-streaming smoke found transcript table content on the restored status line." >&2
        echo "----- status line -----" >&2
        printf '%s\n' "${streaming_mode_line}" >&2
        echo "----- captured primary -----" >&2
        printf '%s\n' "${streaming_restore_capture}" >&2
        exit 1
    fi

    echo "OK: tmux resize-while-streaming smoke kept active thinking live-only across repeated pane resizes."
    exit 0
fi

if [[ "${final_answer_smoke}" -eq 1 ]]; then
    echo "==> launching tmux final-answer native scrollback smoke server ${server_name}"
    tmux -L "${server_name}" -f /dev/null new-session -d -s "${session_name}" -x 100 -y 34 \
        "bash --noprofile --norc"
    tmux -L "${server_name}" send-keys -t "${target}" -l "bash $(printf '%q' "${runner}")"
    tmux -L "${server_name}" send-keys -t "${target}" C-m

    final_capture=""
    saw_final=0
    for _ in {1..160}; do
        final_capture="$(tmux -L "${server_name}" capture-pane -p -J -S - -t "${target}" 2>/dev/null || true)"
        if grep -q 'Final answer fixture completed for tmux validation' <<<"${final_capture}" &&
            grep -q 'final answer fixture head' <<<"${final_capture}" &&
            grep -q 'final answer fixture tail' <<<"${final_capture}"; then
            saw_final=1
            break
        fi
        sleep 0.1
    done
    if [[ "${saw_final}" -ne 1 ]]; then
        echo "ERROR: final-answer smoke did not observe the completed fixture in tmux capture." >&2
        echo "----- captured scrollback -----" >&2
        printf '%s\n' "${final_capture}" >&2
        exit 1
    fi

    head_count="$(grep -c 'final answer fixture head' <<<"${final_capture}" || true)"
    tail_count="$(grep -c 'final answer fixture tail' <<<"${final_capture}" || true)"
    if [[ "${head_count}" -ne 1 || "${tail_count}" -ne 1 ]]; then
        echo "ERROR: final-answer smoke expected head/tail exactly once; saw head=${head_count} tail=${tail_count}." >&2
        echo "----- captured scrollback -----" >&2
        printf '%s\n' "${final_capture}" >&2
        exit 1
    fi

    final_between="$(awk '
        /final answer fixture head/ { in_answer = 1 }
        in_answer { print }
        /final answer fixture tail/ { in_answer = 0 }
    ' <<<"${final_capture}")"
    if grep -q '^❯' <<<"${final_between}" ||
        grep -q -- '-- NORMAL --' <<<"${final_between}" ||
        grep -q -- '-- INSERT --' <<<"${final_between}" ||
        grep -q 'Thinking' <<<"${final_between}"; then
        echo "ERROR: final-answer smoke found live chrome between final answer head and tail." >&2
        echo "----- final answer span -----" >&2
        printf '%s\n' "${final_between}" >&2
        exit 1
    fi

    final_blank_gap="$(awk '
        /final answer fixture head/ { in_answer = 1; current = 0; max = 0 }
        in_answer {
            if ($0 ~ /^[[:space:]]*$/) {
                current += 1
            } else {
                if (current > max) {
                    max = current
                }
                current = 0
            }
        }
        /final answer fixture tail/ { in_answer = 0 }
        END {
            if (current > max) {
                max = current
            }
            print max + 0
        }
    ' <<<"${final_capture}")"
    if [[ "${final_blank_gap}" -gt 2 ]]; then
        echo "ERROR: final-answer smoke found a large blank gap between head and tail: ${final_blank_gap}." >&2
        echo "----- final answer span -----" >&2
        printf '%s\n' "${final_between}" >&2
        exit 1
    fi

    live_prompt_count="$(grep -c '^❯' <<<"${final_capture}" || true)"
    if [[ "${live_prompt_count}" -ne 1 ]]; then
        echo "ERROR: final-answer smoke expected one live prompt after completion; saw ${live_prompt_count}." >&2
        echo "----- captured scrollback -----" >&2
        printf '%s\n' "${final_capture}" >&2
        exit 1
    fi

    for height in 32 29 25 24 25 28 34; do
        tmux -L "${server_name}" resize-window -t "${session_name}:0" -x 100 -y "${height}"
        sleep 0.15
    done

    resized_final_capture=""
    saw_resized_final=0
    for _ in {1..120}; do
        resized_final_capture="$(tmux -L "${server_name}" capture-pane -p -J -S - -t "${target}" 2>/dev/null || true)"
        resized_head_count="$(grep -c 'final answer fixture head' <<<"${resized_final_capture}" || true)"
        resized_tail_count="$(grep -c 'final answer fixture tail' <<<"${resized_final_capture}" || true)"
        resized_body_tail_count="$(grep -c 'final answer fixture body line 24' <<<"${resized_final_capture}" || true)"
        resized_live_prompt_count="$(grep -c '^❯' <<<"${resized_final_capture}" || true)"
        if [[ "${resized_head_count}" -eq 1 &&
            "${resized_tail_count}" -eq 1 &&
            "${resized_body_tail_count}" -eq 1 &&
            "${resized_live_prompt_count}" -eq 1 ]]; then
            saw_resized_final=1
            break
        fi
        sleep 0.1
    done
    if [[ "${saw_resized_final}" -ne 1 ]]; then
        echo "ERROR: final-answer smoke expected completed answer and live prompt exactly once after shrink/grow; saw head=${resized_head_count:-0} tail=${resized_tail_count:-0} body24=${resized_body_tail_count:-0} live_prompt=${resized_live_prompt_count:-0}." >&2
        echo "----- captured scrollback -----" >&2
        printf '%s\n' "${resized_final_capture}" >&2
        exit 1
    fi

    echo "OK: tmux final-answer smoke kept committed head/tail contiguous and unique across shrink/grow."
    exit 0
fi

if [[ "${mouse_precondition_smoke}" -eq 1 ]]; then
    echo "==> launching tmux mouse precondition smoke server ${server_name}"
    tmux -L "${server_name}" -f /dev/null new-session -d -s "${session_name}" -x 100 -y 30 \
        "bash --noprofile --norc"
    tmux -L "${server_name}" set-option -g mouse on
    tmux -L "${server_name}" send-keys -t "${target}" -l "bash $(printf '%q' "${runner}")"
    tmux -L "${server_name}" send-keys -t "${target}" C-m

    saw_tui=0
    for _ in {1..120}; do
        current="$(tmux -L "${server_name}" display-message -p -t "${target}" '#{pane_current_command} #{pane_dead}' 2>/dev/null || true)"
        if [[ "${current}" == "orbcode 0" ]]; then
            saw_tui=1
            break
        fi
        sleep 0.1
    done
    if [[ "${saw_tui}" -ne 1 ]]; then
        echo "ERROR: mouse precondition smoke did not observe the TUI process running." >&2
        exit 1
    fi

    capture=""
    saw_fixture=0
    for _ in {1..120}; do
        capture="$(tmux -L "${server_name}" capture-pane -p -J -S -300 -t "${target}" 2>/dev/null || true)"
        if grep -q 'Manual scrollback fixture loaded for tmux validation' <<<"${capture}"; then
            saw_fixture=1
            break
        fi
        sleep 0.1
    done
    if [[ "${saw_fixture}" -ne 1 ]]; then
        echo "ERROR: mouse precondition smoke did not observe the manual scrollback fixture." >&2
        echo "----- captured pane -----" >&2
        printf '%s\n' "${capture}" >&2
        exit 1
    fi

    if [[ "$(tmux -L "${server_name}" show-options -gqv mouse)" != "on" ]]; then
        echo "ERROR: tmux mouse option is not enabled in the mouse precondition smoke." >&2
        exit 1
    fi

    pane_flags="$(tmux -L "${server_name}" display-message -p -t "${target}" '#{alternate_on} #{mouse_any_flag} #{mouse_button_flag} #{mouse_all_flag} #{pane_in_mode}')"
    if [[ "${pane_flags}" != "0 0 0 0 0" ]]; then
        echo "ERROR: prompt pane does not expose native tmux mouse preconditions: ${pane_flags}" >&2
        exit 1
    fi

    wheel_binding="$(tmux -L "${server_name}" list-keys -T root WheelUpPane)"
    if [[ "${wheel_binding}" != *'#{alternate_on}'* ||
        "${wheel_binding}" != *'copy-mode -e'* ||
        "${wheel_binding}" != *'send-keys -M'* ]]; then
        echo "ERROR: tmux WheelUpPane binding does not match native scroll precondition." >&2
        echo "${wheel_binding}" >&2
        exit 1
    fi

    drag_binding="$(tmux -L "${server_name}" list-keys -T root MouseDrag1Pane)"
    if [[ "${drag_binding}" != *'mouse_any_flag'* ||
        "${drag_binding}" != *'copy-mode -M'* ||
        "${drag_binding}" != *'send-keys -M'* ]]; then
        echo "ERROR: tmux MouseDrag1Pane binding does not match native selection precondition." >&2
        echo "${drag_binding}" >&2
        exit 1
    fi

    echo "OK: tmux mouse precondition smoke verified prompt pane flags and native Wheel/Drag bindings."
    exit 0
fi

if [[ "${mouse_drag_smoke}" -eq 1 ]]; then
    echo "==> launching tmux mouse drag smoke server ${server_name}"
    tmux -L "${server_name}" -f /dev/null new-session -d -s "${session_name}" -x 100 -y 30 \
        "bash --noprofile --norc"
    tmux -L "${server_name}" set-option -g mouse on
    tmux -L "${server_name}" send-keys -t "${target}" -l "bash $(printf '%q' "${runner}")"
    tmux -L "${server_name}" send-keys -t "${target}" C-m

    capture=""
    saw_fixture=0
    for _ in {1..120}; do
        capture="$(tmux -L "${server_name}" capture-pane -p -J -S -300 -t "${target}" 2>/dev/null || true)"
        if grep -q 'Manual scrollback fixture loaded for tmux validation' <<<"${capture}"; then
            saw_fixture=1
            break
        fi
        sleep 0.1
    done
    if [[ "${saw_fixture}" -ne 1 ]]; then
        echo "ERROR: mouse drag smoke did not observe the manual scrollback fixture." >&2
        echo "----- captured pane -----" >&2
        printf '%s\n' "${capture}" >&2
        exit 1
    fi

    pane_flags="$(tmux -L "${server_name}" display-message -p -t "${target}" '#{alternate_on} #{mouse_any_flag} #{mouse_button_flag} #{mouse_all_flag} #{pane_in_mode}')"
    if [[ "${pane_flags}" != "0 0 0 0 0" ]]; then
        echo "ERROR: prompt pane does not expose native tmux drag preconditions: ${pane_flags}" >&2
        exit 1
    fi

    expect_log="${scratch}/mouse-drag-expect.log"
    expect <<EOF >"${expect_log}" 2>&1
set timeout 3
spawn -noecho env TERM=xterm-256color tmux -L "${server_name}" attach -t "${session_name}"
after 800
send "\033\[<32;20;10M"
after 800
close
wait
EOF

    drag_state="$(tmux -L "${server_name}" display-message -p -t "${target}" '#{pane_in_mode} #{pane_mode} #{alternate_on} #{mouse_any_flag} #{pane_current_command} #{pane_dead}')"
    if [[ "${drag_state}" != "1 copy-mode 0 0 orbcode 0" ]]; then
        echo "ERROR: SGR drag event did not put tmux in native copy-mode: ${drag_state}" >&2
        echo "----- expect log -----" >&2
        cat "${expect_log}" >&2
        exit 1
    fi

    echo "OK: tmux mouse drag smoke entered native copy-mode from an attached client SGR drag event."
    exit 0
fi

if [[ "${mouse_wheel_smoke}" -eq 1 ]]; then
    echo "==> launching tmux mouse wheel smoke server ${server_name}"
    tmux -L "${server_name}" -f /dev/null new-session -d -s "${session_name}" -x 100 -y 30 \
        "bash --noprofile --norc"
    tmux -L "${server_name}" set-option -g mouse on
    tmux -L "${server_name}" send-keys -t "${target}" -l "bash $(printf '%q' "${runner}")"
    tmux -L "${server_name}" send-keys -t "${target}" C-m

    capture=""
    saw_fixture=0
    for _ in {1..120}; do
        capture="$(tmux -L "${server_name}" capture-pane -p -J -S -300 -t "${target}" 2>/dev/null || true)"
        if grep -q 'Manual scrollback fixture loaded for tmux validation' <<<"${capture}"; then
            saw_fixture=1
            break
        fi
        sleep 0.1
    done
    if [[ "${saw_fixture}" -ne 1 ]]; then
        echo "ERROR: mouse wheel smoke did not observe the manual scrollback fixture." >&2
        echo "----- captured pane -----" >&2
        printf '%s\n' "${capture}" >&2
        exit 1
    fi

    pane_flags="$(tmux -L "${server_name}" display-message -p -t "${target}" '#{alternate_on} #{mouse_any_flag} #{mouse_button_flag} #{mouse_all_flag} #{pane_in_mode}')"
    if [[ "${pane_flags}" != "0 0 0 0 0" ]]; then
        echo "ERROR: prompt pane does not expose native tmux wheel preconditions: ${pane_flags}" >&2
        exit 1
    fi

    expect_log="${scratch}/mouse-wheel-expect.log"
    expect <<EOF >"${expect_log}" 2>&1
set timeout 3
spawn -noecho env TERM=xterm-256color tmux -L "${server_name}" attach -t "${session_name}"
after 800
send "\033\[<64;10;10M"
after 800
close
wait
EOF

    wheel_state="$(tmux -L "${server_name}" display-message -p -t "${target}" '#{pane_in_mode} #{pane_mode} #{alternate_on} #{mouse_any_flag} #{pane_current_command} #{pane_dead}')"
    if [[ "${wheel_state}" != "1 copy-mode 0 0 orbcode 0" ]]; then
        echo "ERROR: SGR wheel event did not put tmux in native copy-mode: ${wheel_state}" >&2
        echo "----- expect log -----" >&2
        cat "${expect_log}" >&2
        exit 1
    fi

    echo "OK: tmux mouse wheel smoke entered native copy-mode from an attached client SGR wheel event."
    exit 0
fi

if [[ "${transcript_pager_smoke}" -eq 1 ]]; then
    echo "==> launching tmux transcript pager smoke server ${server_name}"
    tmux -L "${server_name}" -f /dev/null new-session -d -s "${session_name}" -x 100 -y 30 \
        "bash --noprofile --norc"
    tmux -L "${server_name}" send-keys -t "${target}" -l "bash $(printf '%q' "${runner}")"
    tmux -L "${server_name}" send-keys -t "${target}" C-m

    capture=""
    saw_fixture=0
    for _ in {1..120}; do
        capture="$(tmux -L "${server_name}" capture-pane -p -J -S -300 -t "${target}" 2>/dev/null || true)"
        if grep -q 'Manual scrollback fixture loaded for tmux validation' <<<"${capture}"; then
            saw_fixture=1
            break
        fi
        sleep 0.1
    done
    if [[ "${saw_fixture}" -ne 1 ]]; then
        echo "ERROR: transcript pager smoke did not observe the manual scrollback fixture." >&2
        echo "----- captured pane -----" >&2
        printf '%s\n' "${capture}" >&2
        exit 1
    fi

    tmux -L "${server_name}" send-keys -t "${target}" C-o
    pager_capture=""
    pager_visible_capture=""
    pager_last_visible_line=""
    saw_pager=0
    for _ in {1..120}; do
        pane_state="$(tmux -L "${server_name}" display-message -p -t "${target}" '#{alternate_on} #{pane_current_command} #{pane_dead} #{pane_in_mode}')"
        pager_capture="$(tmux -L "${server_name}" capture-pane -p -J -t "${target}" 2>/dev/null || true)"
        pager_visible_capture="${pager_capture}"
        pager_last_visible_line="$(last_nonempty_line <<<"${pager_visible_capture}")"
        if [[ "${pane_state}" == "1 orbcode 0 0" ]] &&
            grep -q 'manual scrollback fixture summary 47' <<<"${pager_capture}" &&
            [[ "${pager_last_visible_line}" == *'reflows while the live prompt remains usable' ]] &&
            ! contains_non_session_shell_chrome <<<"${pager_visible_capture}"; then
            saw_pager=1
            break
        fi
        sleep 0.1
    done
    if [[ "${saw_pager}" -ne 1 ]]; then
        echo "ERROR: transcript pager smoke did not open the app transcript pager in tmux." >&2
        echo "----- pane state -----" >&2
        printf '%s\n' "${pane_state}" >&2
        echo "----- captured pane -----" >&2
        printf '%s\n' "${pager_capture}" >&2
        echo "----- visible pane -----" >&2
        printf '%s\n' "${pager_visible_capture}" >&2
        echo "----- last visible line -----" >&2
        printf '%s\n' "${pager_last_visible_line}" >&2
        exit 1
    fi

    tmux -L "${server_name}" send-keys -t "${target}" C-o
    restored_state=""
    restored_capture=""
    saw_restore=0
    for _ in {1..120}; do
        restored_state="$(tmux -L "${server_name}" display-message -p -t "${target}" '#{alternate_on} #{pane_current_command} #{pane_dead} #{pane_in_mode}')"
        restored_capture="$(tmux -L "${server_name}" capture-pane -p -J -S -300 -t "${target}" 2>/dev/null || true)"
        live_prompt_count="$(grep -c '^❯' <<<"${restored_capture}" || true)"
        if [[ "${restored_state}" == "0 orbcode 0 0" ]] &&
            grep -q 'manual scrollback fixture summary 47' <<<"${restored_capture}" &&
            [[ "${live_prompt_count}" -eq 1 ]]; then
            saw_restore=1
            break
        fi
        sleep 0.1
    done
    if [[ "${saw_restore}" -ne 1 ]]; then
        echo "ERROR: transcript pager smoke did not close the app transcript pager and restore prompt after Ctrl-O." >&2
        echo "----- pane state -----" >&2
        printf '%s\n' "${restored_state}" >&2
        echo "----- captured pane -----" >&2
        printf '%s\n' "${restored_capture}" >&2
        exit 1
    fi
    restored_fixture_tail_count="$(grep -c 'manual scrollback fixture summary 47' <<<"${restored_capture}" || true)"
    if [[ "${restored_fixture_tail_count}" -ne 1 ]]; then
        echo "ERROR: transcript pager smoke expected fixture tail once after close; saw ${restored_fixture_tail_count}." >&2
        echo "----- captured pane -----" >&2
        printf '%s\n' "${restored_capture}" >&2
        exit 1
    fi
    live_prompt_line="$(grep '^❯' <<<"${restored_capture}" | tail -n 1 || true)"
    if contains_transcript_table_chrome <<<"${live_prompt_line}"; then
        echo "ERROR: transcript pager smoke found transcript table content on the restored prompt line." >&2
        echo "----- prompt line -----" >&2
        printf '%s\n' "${live_prompt_line}" >&2
        echo "----- captured pane -----" >&2
        printf '%s\n' "${restored_capture}" >&2
        exit 1
    fi
    mode_line="$(grep '^--' <<<"${restored_capture}" | tail -n 1 || true)"
    if contains_transcript_table_chrome <<<"${mode_line}"; then
        echo "ERROR: transcript pager smoke found transcript table content on the restored status line." >&2
        echo "----- status line -----" >&2
        printf '%s\n' "${mode_line}" >&2
        echo "----- captured pane -----" >&2
        printf '%s\n' "${restored_capture}" >&2
        exit 1
    fi

    echo "OK: tmux transcript pager smoke opened app pager and restored inline prompt with Ctrl-O."
    exit 0
fi

if [[ "${transcript_pager_deferred_smoke}" -eq 1 ]]; then
    echo "==> launching tmux transcript pager deferred-history smoke server ${server_name}"
    tmux -L "${server_name}" -f /dev/null new-session -d -s "${session_name}" -x 100 -y 30 \
        "bash --noprofile --norc"
    tmux -L "${server_name}" send-keys -t "${target}" -l "bash $(printf '%q' "${runner}")"
    tmux -L "${server_name}" send-keys -t "${target}" C-m

    capture=""
    saw_fixture=0
    for _ in {1..120}; do
        capture="$(tmux -L "${server_name}" capture-pane -p -J -S -300 -t "${target}" 2>/dev/null || true)"
        if grep -q 'Manual scrollback fixture loaded for tmux validation' <<<"${capture}"; then
            saw_fixture=1
            break
        fi
        sleep 0.1
    done
    if [[ "${saw_fixture}" -ne 1 ]]; then
        echo "ERROR: transcript pager deferred smoke did not observe the manual scrollback fixture." >&2
        echo "----- captured pane -----" >&2
        printf '%s\n' "${capture}" >&2
        exit 1
    fi

    tmux -L "${server_name}" send-keys -t "${target}" C-o
    pager_capture=""
    pager_visible_capture=""
    pager_last_visible_line=""
    pane_state=""
    saw_static_pager=0
    for _ in {1..120}; do
        pane_state="$(tmux -L "${server_name}" display-message -p -t "${target}" '#{alternate_on} #{pane_current_command} #{pane_dead} #{pane_in_mode}')"
        pager_capture="$(tmux -L "${server_name}" capture-pane -p -J -t "${target}" 2>/dev/null || true)"
        pager_visible_capture="${pager_capture}"
        pager_last_visible_line="$(last_nonempty_line <<<"${pager_visible_capture}")"
        if [[ "${pane_state}" == "1 orbcode 0 0" ]] &&
            grep -q 'manual scrollback fixture summary 47' <<<"${pager_capture}" &&
            [[ "${pager_last_visible_line}" == *'reflows while the live prompt remains usable' ]] &&
            ! contains_non_session_shell_chrome <<<"${pager_visible_capture}"; then
            saw_static_pager=1
            break
        fi
        sleep 0.1
    done
    if [[ "${saw_static_pager}" -ne 1 ]]; then
        echo "ERROR: transcript pager deferred smoke did not open the app transcript pager in tmux." >&2
        echo "----- pane state -----" >&2
        printf '%s\n' "${pane_state}" >&2
        echo "----- captured pager -----" >&2
        printf '%s\n' "${pager_capture}" >&2
        echo "----- visible pager -----" >&2
        printf '%s\n' "${pager_visible_capture}" >&2
        echo "----- last visible line -----" >&2
        printf '%s\n' "${pager_last_visible_line}" >&2
        exit 1
    fi
    tmux -L "${server_name}" send-keys -t "${target}" C-o
    restored_capture=""
    restored_state=""
    saw_restore=0
    for _ in {1..120}; do
        restored_state="$(tmux -L "${server_name}" display-message -p -t "${target}" '#{alternate_on} #{pane_current_command} #{pane_dead} #{pane_in_mode}')"
        restored_capture="$(tmux -L "${server_name}" capture-pane -p -J -S - -t "${target}" 2>/dev/null || true)"
        live_prompt_count="$(grep -c '^❯' <<<"${restored_capture}" || true)"
        if [[ "${restored_state}" == "0 orbcode 0 0" ]] &&
            grep -q 'manual scrollback fixture summary 47' <<<"${restored_capture}" &&
            [[ "${live_prompt_count}" -eq 1 ]]; then
            saw_restore=1
            break
        fi
        sleep 0.1
    done
    if [[ "${saw_restore}" -ne 1 ]]; then
        echo "ERROR: transcript pager deferred smoke did not close the app transcript pager and restore prompt after Ctrl-O." >&2
        echo "----- pane state -----" >&2
        printf '%s\n' "${restored_state}" >&2
        echo "----- captured primary -----" >&2
        printf '%s\n' "${restored_capture}" >&2
        exit 1
    fi

    restored_fixture_tail_count="$(grep -c 'manual scrollback fixture summary 47' <<<"${restored_capture}" || true)"
    if [[ "${restored_fixture_tail_count}" -ne 1 ]]; then
        echo "ERROR: transcript pager deferred smoke expected fixture tail once after close; saw ${restored_fixture_tail_count}." >&2
        echo "----- captured primary -----" >&2
        printf '%s\n' "${restored_capture}" >&2
        exit 1
    fi

    if grep -q 'q/Esc close' <<<"${restored_capture}"; then
        echo "ERROR: transcript pager deferred smoke found pager chrome in primary scrollback after close." >&2
        echo "----- captured primary -----" >&2
        printf '%s\n' "${restored_capture}" >&2
        exit 1
    fi
    live_prompt_line="$(grep '^❯' <<<"${restored_capture}" | tail -n 1 || true)"
    if contains_transcript_table_chrome <<<"${live_prompt_line}"; then
        echo "ERROR: transcript pager deferred smoke found transcript table content on the restored prompt line." >&2
        echo "----- prompt line -----" >&2
        printf '%s\n' "${live_prompt_line}" >&2
        echo "----- captured primary -----" >&2
        printf '%s\n' "${restored_capture}" >&2
        exit 1
    fi
    mode_line="$(grep '^--' <<<"${restored_capture}" | tail -n 1 || true)"
    if contains_transcript_table_chrome <<<"${mode_line}"; then
        echo "ERROR: transcript pager deferred smoke found transcript table content on the restored status line." >&2
        echo "----- status line -----" >&2
        printf '%s\n' "${mode_line}" >&2
        echo "----- captured primary -----" >&2
        printf '%s\n' "${restored_capture}" >&2
        exit 1
    fi

    echo "OK: tmux transcript pager deferred smoke opened app pager and restored inline prompt with Ctrl-O."
    exit 0
fi

if [[ "${metrics_fixture_smoke}" -eq 1 ]]; then
    echo "==> launching tmux metrics fixture smoke server ${server_name}"
    tmux -L "${server_name}" -f /dev/null new-session -d -s "${session_name}" -x 100 -y 30 \
        "bash --noprofile --norc"
    tmux -L "${server_name}" send-keys -t "${target}" -l "bash $(printf '%q' "${runner}")"
    tmux -L "${server_name}" send-keys -t "${target}" C-m

    capture=""
    saw_fixture=0
    for _ in {1..120}; do
        capture="$(tmux -L "${server_name}" capture-pane -p -J -S -300 -t "${target}" 2>/dev/null || true)"
        if grep -q 'Manual scrollback fixture loaded for tmux validation' <<<"${capture}"; then
            saw_fixture=1
            break
        fi
        sleep 0.1
    done
    if [[ "${saw_fixture}" -ne 1 ]]; then
        echo "ERROR: metrics fixture smoke did not observe the manual scrollback fixture." >&2
        echo "----- captured pane -----" >&2
        printf '%s\n' "${capture}" >&2
        exit 1
    fi

    scrollback_capture=""
    saw_retained_fixture=0
    for _ in {1..120}; do
        scrollback_capture="$(tmux -L "${server_name}" capture-pane -p -J -S - -t "${target}" 2>/dev/null || true)"
        oldest_summary_count="$(grep -c 'manual scrollback fixture summary 00:' <<<"${scrollback_capture}" || true)"
        summary_count="$(grep -c 'manual scrollback fixture summary 47:' <<<"${scrollback_capture}" || true)"
        if [[ "${oldest_summary_count}" == "1" && "${summary_count}" == "1" ]]; then
            saw_retained_fixture=1
            break
        fi
        sleep 0.1
    done
    if [[ "${saw_retained_fixture}" -ne 1 ]]; then
        echo "ERROR: metrics fixture smoke expected summaries 00 and 47 exactly once in tmux scrollback; saw summary00=${oldest_summary_count:-0} summary47=${summary_count:-0}." >&2
        echo "----- captured scrollback -----" >&2
        printf '%s\n' "${scrollback_capture}" >&2
        exit 1
    fi
    first_prompt_count="$(grep -c 'manual scrollback fixture prompt 00:' <<<"${scrollback_capture}" || true)"
    active_prompt_count="$(grep -c '^❯' <<<"${scrollback_capture}" || true)"
    if [[ "${first_prompt_count}" != "1" ]]; then
        echo "ERROR: metrics fixture smoke expected the first user prompt exactly once in tmux scrollback; saw ${first_prompt_count}." >&2
        echo "----- captured scrollback -----" >&2
        printf '%s\n' "${scrollback_capture}" >&2
        exit 1
    fi
    if [[ "${active_prompt_count}" != "1" ]]; then
        echo "ERROR: metrics fixture smoke expected exactly one live input prompt; saw ${active_prompt_count}." >&2
        echo "----- captured scrollback -----" >&2
        printf '%s\n' "${scrollback_capture}" >&2
        exit 1
    fi
    fixture_scrollback="$(awk '
        /manual scrollback fixture prompt 00:/ { in_fixture = 1 }
        in_fixture { print }
        /manual scrollback fixture summary 47:/ { saw_last = 1 }
        saw_last && /live prompt remains usable/ { exit }
    ' <<<"${scrollback_capture}")"
    fixture_max_blank_gap="$(awk '
        NF {
            if (seen && blanks > max) max = blanks
            blanks = 0
            seen = 1
            next
        }
        seen { blanks++ }
        END { print max + 0 }
    ' <<<"${fixture_scrollback}")"
    if [[ -z "${fixture_scrollback}" ]]; then
        echo "ERROR: metrics fixture smoke could not isolate committed fixture scrollback." >&2
        echo "----- captured scrollback -----" >&2
        printf '%s\n' "${scrollback_capture}" >&2
        exit 1
    fi
    if [[ "${fixture_max_blank_gap}" -gt 2 ]]; then
        echo "ERROR: metrics fixture smoke found a large blank gap in committed fixture history: ${fixture_max_blank_gap}." >&2
        echo "----- committed fixture scrollback -----" >&2
        printf '%s\n' "${fixture_scrollback}" >&2
        exit 1
    fi
    if grep -Eq '^─|^❯|-- INSERT --|ctx:|Orb Code v|Manual scrollback fixture loaded' <<<"${fixture_scrollback}"; then
        echo "ERROR: metrics fixture smoke found live viewport chrome in committed tmux scrollback." >&2
        echo "----- committed fixture scrollback -----" >&2
        printf '%s\n' "${fixture_scrollback}" >&2
        exit 1
    fi

    metrics=""
    saw_bounded_metrics=0
    for _ in {1..120}; do
        metrics="$(cat "${metrics_file}" 2>/dev/null || true)"
        if [[ "${metrics}" == *'"history_flush"'* ]] &&
            grep -Eq '"visible_line_count":[01]' <<<"${metrics}" &&
            grep -Eq '"total_line_count":[01]' <<<"${metrics}" &&
            grep -Eq '"bytes":[1-9][0-9]*' <<<"${metrics}"; then
            saw_bounded_metrics=1
            break
        fi
        sleep 0.1
    done
    if [[ "${saw_bounded_metrics}" -ne 1 ]]; then
        echo "ERROR: metrics fixture smoke did not record bounded prompt metrics after history emission." >&2
        echo "----- metrics -----" >&2
        printf '%s\n' "${metrics}" >&2
        exit 1
    fi

    echo "OK: tmux metrics fixture smoke recorded bounded prompt rows after history emission."
    exit 0
fi

echo "==> launching isolated tmux server ${server_name}"
tmux -L "${server_name}" -f /dev/null new-session -d -s "${session_name}" -x 100 -y 30 \
    "bash --noprofile --norc"
tmux -L "${server_name}" pipe-pane -o -t "${target}" "cat > '${raw_output}'"
tmux -L "${server_name}" send-keys -t "${target}" -l "bash $(printf '%q' "${runner}")"
tmux -L "${server_name}" send-keys -t "${target}" C-m

capture=""
raw=""
for _ in {1..120}; do
    capture="$(tmux -L "${server_name}" capture-pane -p -J -S -200 -t "${target}" 2>/dev/null || true)"
    raw="$(cat "${raw_output}" 2>/dev/null || true)"
    if [[ "${capture}" == *"__ORBCODE_TMUX_SMOKE_DONE:"* || "${raw}" == *"__ORBCODE_TMUX_SMOKE_DONE:"* ]]; then
        break
    fi
    sleep 0.1
done

if [[ "${capture}" != *"__ORBCODE_TMUX_SMOKE_DONE:0"* && "${raw}" != *"__ORBCODE_TMUX_SMOKE_DONE:0"* ]]; then
    echo "ERROR: tmux smoke did not finish successfully." >&2
    echo "----- captured pane -----" >&2
    printf '%s\n' "${capture}" >&2
    exit 1
fi

summary="PTY smoke finalized summary scrollback line"
if [[ "${capture}" != *"${summary}"* && "${raw}" != *"${summary}"* ]]; then
    echo "ERROR: finalized summary history did not appear in tmux pane output." >&2
    echo "----- captured pane -----" >&2
    printf '%s\n' "${capture}" >&2
    exit 1
fi

for sequence_name in \
    "alternate-screen-enter:\x1b[?1049h" \
    "alternate-screen-leave:\x1b[?1049l" \
    "mouse-button-events:\x1b[?1000h" \
    "mouse-drag-events:\x1b[?1002h" \
    "mouse-all-motion:\x1b[?1003h"
do
    name="${sequence_name%%:*}"
    escaped="${sequence_name#*:}"
    sequence="$(printf '%b' "${escaped}")"
    if [[ "${raw}" == *"${sequence}"* ]]; then
        echo "ERROR: prompt-mode tmux smoke emitted forbidden ${name} sequence." >&2
        exit 1
    fi
done

if [[ "${metrics_smoke}" -eq 1 ]]; then
    if [[ ! -s "${metrics_file}" ]]; then
        echo "ERROR: metrics smoke did not write render metrics to ${metrics_file}." >&2
        exit 1
    fi
    if ! grep -q '"type":"tui_render_frame"' "${metrics_file}"; then
        echo "ERROR: metrics smoke output does not contain tui_render_frame records." >&2
        exit 1
    fi
    if ! grep -Eq '"bytes":[1-9][0-9]*' "${metrics_file}"; then
        echo "ERROR: metrics smoke output does not report positive output bytes." >&2
        exit 1
    fi
    if ! grep -Fq '"redraw_reasons":["pty_smoke_first_frame"]' "${metrics_file}"; then
        echo "ERROR: metrics smoke output does not include the PTY smoke redraw reason." >&2
        exit 1
    fi
    echo "OK: tmux metrics smoke wrote render metrics for the prompt first frame."
else
    echo "OK: tmux prompt smoke rendered summary history without alt-screen or mouse-capture sequences."
fi
