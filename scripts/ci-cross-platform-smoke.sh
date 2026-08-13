#!/usr/bin/env bash
#
# CI cross-platform smoke tests.
#
# Runs sandbox host validation through a prebuilt orbcode binary, so CI
# does not compile a second Rust test harness. Each platform unlocks a different
# subset:
#
#   Linux  — bubblewrap sandbox baseline (requires `bwrap` on PATH)
#   macOS  — seatbelt (sandbox-exec) baseline (built-in on macOS)
#   Windows — no sandbox runner yet; packaged path behavior is covered by
#             smoke-release.sh
#
# Usage:
#   scripts/ci-cross-platform-smoke.sh --binary target/debug/orbcode
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${REPO_ROOT}"

binary=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        --binary)
            if [[ $# -lt 2 || -z "$2" ]]; then
                echo "ERROR: --binary requires a path" >&2
                exit 2
            fi
            binary="$2"
            shift 2
            ;;
        *)
            echo "Usage: scripts/ci-cross-platform-smoke.sh --binary <path>" >&2
            exit 1
            ;;
    esac
done

if [[ -z "${binary}" ]]; then
    echo "ERROR: --binary is required" >&2
    exit 2
fi
if [[ "${binary}" != /* ]]; then
    binary="${REPO_ROOT}/${binary}"
fi
if [[ ! -x "${binary}" ]]; then
    echo "ERROR: executable orbcode binary not found at ${binary}" >&2
    exit 1
fi

step() { printf '\n\033[1;34m==> %s\033[0m\n' "$1"; }
warn() { printf '\033[1;33m    ⚠ %s\033[0m\n' "$1"; }
pass() { printf '\033[1;32m    ✓ %s\033[0m\n' "$1"; }

python_command=""
if command -v python3 >/dev/null 2>&1; then
    python_command="python3"
elif command -v python >/dev/null 2>&1; then
    python_command="python"
fi

json_command() {
    local value="$1"
    value="${value//\\/\\\\}"
    value="${value//\"/\\\"}"
    value="${value//$'\n'/\\n}"
    printf '{"command":"%s"}' "${value}"
}

invoke_bash() {
    local cwd="$1"
    local home="$2"
    local mode="$3"
    local network="$4"
    local command="$5"
    shift 5
    (
        cd "${cwd}"
        env \
            -u ORBCODE_HOME \
            -u CLAUDE_CODE_USE_OPENAI \
            CLAUDE_CONFIG_DIR="${home}" \
            ANTHROPIC_BASE_URL="stub://anthropic" \
            PROVIDER_TYPE="anthropic" \
            "${binary}" \
            --sandbox-mode "${mode}" \
            --sandbox-network "${network}" \
            --allow-tools true \
            "$@" \
            tool bash "$(json_command "${command}")"
    )
}

baseline_smoke() {
    local scratch="$1"
    local output
    output="$(invoke_bash \
        "${scratch}/workspace" \
        "${scratch}/home" \
        read-only \
        false \
        "printf cross-platform")" || return 1
    [[ "${output}" == *"cross-platform"* ]]
}

start_loopback_listener() {
    local port_file="$1"
    local accepted_file="$2"
    "${python_command}" - "${port_file}" "${accepted_file}" <<'PY' &
import pathlib
import socket
import sys

port_file = pathlib.Path(sys.argv[1])
accepted_file = pathlib.Path(sys.argv[2])
listener = socket.socket()
listener.bind(("127.0.0.1", 0))
listener.listen(1)
listener.settimeout(3)
port_file.write_text(str(listener.getsockname()[1]), encoding="utf-8")
try:
    connection, _ = listener.accept()
except TimeoutError:
    accepted_file.write_text("false", encoding="utf-8")
else:
    connection.close()
    accepted_file.write_text("true", encoding="utf-8")
finally:
    listener.close()
PY
    listener_pid=$!
}

wait_for_port() {
    local port_file="$1"
    local remaining=100
    while (( remaining > 0 )); do
        if [[ -s "${port_file}" ]]; then
            return 0
        fi
        sleep 0.05
        remaining=$((remaining - 1))
    done
    return 1
}

OS="$(uname -s)"
FAILED=0
scratch="$(mktemp -d)"
trap 'rm -rf "${scratch}"' EXIT
mkdir -p "${scratch}/workspace" "${scratch}/home" "${scratch}/outside"

case "$OS" in
    Linux)
        step "Linux bubblewrap sandbox host tests"
        if command -v bwrap >/dev/null 2>&1; then
            if ! baseline_smoke "${scratch}"; then
                warn "bubblewrap baseline FAILED"
                FAILED=1
            else
                pass "bubblewrap baseline"
            fi

            blocked="${scratch}/workspace/read-only-blocked.txt"
            if invoke_bash \
                "${scratch}/workspace" \
                "${scratch}/home" \
                read-only \
                false \
                "printf blocked > '${blocked}'" \
                >/dev/null 2>&1 || [[ -e "${blocked}" ]]; then
                warn "read-only write boundary FAILED"
                FAILED=1
            else
                pass "read-only write boundary"
            fi

            extra="${scratch}/extra"
            mkdir -p "${extra}"
            inside="${scratch}/workspace/workspace-write.txt"
            extra_file="${extra}/additional-dir-write.txt"
            if invoke_bash \
                "${scratch}/workspace" \
                "${scratch}/home" \
                workspace-write \
                false \
                "printf cwd > '${inside}' && printf extra > '${extra_file}'" \
                --add-dir "${extra}" \
                >/dev/null 2>&1 \
                && [[ "$(cat "${inside}" 2>/dev/null || true)" == "cwd" ]] \
                && [[ "$(cat "${extra_file}" 2>/dev/null || true)" == "extra" ]]; then
                pass "workspace and additional-directory writes"
            else
                warn "workspace write boundary FAILED"
                FAILED=1
            fi

            outside_file="${scratch}/outside/workspace-escape.txt"
            if invoke_bash \
                "${scratch}/workspace" \
                "${scratch}/home" \
                workspace-write \
                false \
                "printf blocked > '${outside_file}'" \
                >/dev/null 2>&1 || [[ -e "${outside_file}" ]]; then
                warn "workspace escape boundary FAILED"
                FAILED=1
            else
                pass "workspace escape boundary"
            fi

            symlink_target="${scratch}/outside/symlink-escape.txt"
            symlink_path="${scratch}/workspace/symlink-escape.txt"
            ln -s "${symlink_target}" "${symlink_path}"
            if invoke_bash \
                "${scratch}/workspace" \
                "${scratch}/home" \
                workspace-write \
                false \
                "printf blocked > '${symlink_path}'" \
                >/dev/null 2>&1 || [[ -e "${symlink_target}" ]]; then
                warn "workspace symlink escape boundary FAILED"
                FAILED=1
            else
                pass "workspace symlink escape boundary"
            fi

            if [[ -z "${python_command}" ]]; then
                warn "Python not found — network boundary validation FAILED"
                FAILED=1
            else
                allowed_port_file="${scratch}/allowed.port"
                allowed_accepted_file="${scratch}/allowed.accepted"
                start_loopback_listener "${allowed_port_file}" "${allowed_accepted_file}"
                allowed_listener_pid="${listener_pid}"
                if ! wait_for_port "${allowed_port_file}"; then
                    warn "network-allowed listener startup FAILED"
                    FAILED=1
                else
                    allowed_port="$(cat "${allowed_port_file}")"
                    if invoke_bash \
                        "${scratch}/workspace" \
                        "${scratch}/home" \
                        read-only \
                        true \
                        "python3 -c \"import socket; s=socket.create_connection(('127.0.0.1', ${allowed_port}), 1); s.close()\"" \
                        >/dev/null 2>&1 \
                        && wait "${allowed_listener_pid}" \
                        && [[ "$(cat "${allowed_accepted_file}" 2>/dev/null || true)" == "true" ]]; then
                        pass "network allowed boundary"
                    else
                        wait "${allowed_listener_pid}" 2>/dev/null || true
                        warn "network allowed boundary FAILED"
                        FAILED=1
                    fi
                fi

                blocked_port_file="${scratch}/blocked.port"
                blocked_accepted_file="${scratch}/blocked.accepted"
                start_loopback_listener "${blocked_port_file}" "${blocked_accepted_file}"
                blocked_listener_pid="${listener_pid}"
                if ! wait_for_port "${blocked_port_file}"; then
                    warn "network-blocked listener startup FAILED"
                    FAILED=1
                else
                    blocked_port="$(cat "${blocked_port_file}")"
                    if invoke_bash \
                        "${scratch}/workspace" \
                        "${scratch}/home" \
                        read-only \
                        false \
                        "python3 -c \"import socket; s=socket.create_connection(('127.0.0.1', ${blocked_port}), 1); s.close()\"" \
                        >/dev/null 2>&1; then
                        warn "network blocked boundary FAILED"
                        FAILED=1
                    fi
                    wait "${blocked_listener_pid}" 2>/dev/null || true
                    if [[ "$(cat "${blocked_accepted_file}" 2>/dev/null || true)" == "false" ]]; then
                        pass "network blocked boundary"
                    else
                        warn "network blocked boundary FAILED"
                        FAILED=1
                    fi
                fi
            fi
        else
            warn "bwrap not found — bubblewrap host validation FAILED"
            FAILED=1
        fi
        ;;

    Darwin)
        step "macOS seatbelt sandbox host tests"
        if command -v sandbox-exec >/dev/null 2>&1; then
            if baseline_smoke "${scratch}"; then
                pass "seatbelt baseline"
            else
                warn "seatbelt baseline FAILED"
                FAILED=1
            fi
        else
            warn "sandbox-exec not found — seatbelt host validation FAILED"
            FAILED=1
        fi
        ;;

    MINGW*|MSYS*|CYGWIN*|Windows_NT)
        step "Windows sandbox host tests"
        pass "sandbox runner not enabled; packaged path smoke already passed"
        ;;

    *)
        warn "Unknown OS '$OS' — no sandbox host validation ran"
        ;;
esac

echo ""
if [ "$FAILED" -eq 0 ]; then
    printf '\033[1;32m✓ Cross-platform smoke passed.\033[0m\n'
else
    printf '\033[1;31m✗ Cross-platform smoke had failures.\033[0m\n'
    exit 1
fi
