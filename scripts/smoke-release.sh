#!/usr/bin/env bash
# Release-readiness smoke pipeline for the orbcode binary.
#
# Validates that build metadata (git SHA, target, profile, build timestamp,
# providers) is embedded into the packaged binary and that --version, -V,
# and `doctor` all agree. Then runs the black-box build_smoke integration
# tests. Run locally and from CI before publishing a binary.
#
# Usage:
#   scripts/smoke-release.sh             # debug build + smoke tests (fast)
#   scripts/smoke-release.sh --release   # release build + smoke tests
#   scripts/smoke-release.sh --release --package
#                                        # also build + verify the packaged
#                                        # release archive for the host target
#
# Exit codes: 0 = all checks pass; non-zero = first failing check.

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${repo_root}"

profile="debug"
cargo_profile_args=()
target_dir_segment="debug"
do_package=0
for arg in "$@"; do
    case "${arg}" in
        --release)
            profile="release"
            cargo_profile_args=(--release)
            target_dir_segment="release"
            ;;
        --package)
            do_package=1
            ;;
        *)
            echo "ERROR: unknown argument: ${arg}" >&2
            exit 2
            ;;
    esac
done

echo "==> smoke-release.sh (profile=${profile})"
echo "    repo: ${repo_root}"

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

echo "==> --version (long form) parses"
version_long="$("${binary}" --version)"
echo "${version_long}"
require_substring() {
    local haystack="$1" needle="$2" label="$3"
    if ! grep -qF -- "${needle}" <<<"${haystack}"; then
        echo "ERROR: ${label} missing '${needle}'" >&2
        echo "----- output -----" >&2
        echo "${haystack}" >&2
        exit 1
    fi
}

# First line shape: "orbcode <semver> (<sha>[+dirty])"
first_line="$(head -n1 <<<"${version_long}")"
if ! grep -Eq '^orbcode [0-9]+\.[0-9]+\.[0-9]+ \([a-f0-9]+(\+dirty)?\)$' <<<"${first_line}"; then
    echo "ERROR: --version first line does not match expected shape: ${first_line}" >&2
    exit 1
fi
require_substring "${version_long}" "target:"    "--version"
require_substring "${version_long}" "profile:"   "--version"
require_substring "${version_long}" "built:"     "--version"
require_substring "${version_long}" "providers:" "--version"
for provider in anthropic openai gemini grok; do
    require_substring "${version_long}" "${provider}" "--version providers"
done

# Profile baked into the binary should match what we just built.
require_substring "${version_long}" "profile:   ${profile}" "--version profile"

echo "==> -V (short form) parses"
version_short="$("${binary}" -V)"
echo "${version_short}"
if ! grep -Eq '^orbcode [0-9]+\.[0-9]+\.[0-9]+ \([a-f0-9]+(\+dirty)? [a-z0-9_]+-[a-z0-9_-]+\)$' \
     <<<"${version_short}"; then
    echo "ERROR: -V output does not match '<name> <ver> (<sha> <target>)' shape" >&2
    exit 1
fi

scratch="$(mktemp -d)"
trap 'rm -rf "${scratch}"' EXIT
isolated_env=(env -u ORBCODE_HOME -u PROVIDER_TYPE -u ORBCODE_PROVIDER -u CLAUDE_CODE_USE_OPENAI)

echo "==> --help lists packaged CLI smoke commands"
help_out="$("${binary}" --help)"
for command_name in "Commands:" "prompt" "sessions" "providers" "doctor" "--version"; do
    require_substring "${help_out}" "${command_name}" "--help"
done

echo "==> orbcode doctor build_info row matches --version"
doctor_out="$(
    "${isolated_env[@]}" \
        CLAUDE_CONFIG_DIR="${scratch}/doctor-home" \
        ANTHROPIC_BASE_URL="stub://anthropic" \
        PROVIDER_TYPE="anthropic" \
        "${binary}" doctor 2>&1
)" || {
    echo "ERROR: orbcode doctor exited non-zero" >&2
    echo "${doctor_out}" >&2
    exit 1
}
build_info_row="$(grep "build_info" <<<"${doctor_out}" || true)"
if [[ -z "${build_info_row}" ]]; then
    echo "ERROR: doctor output missing build_info row" >&2
    echo "${doctor_out}" >&2
    exit 1
fi
echo "${build_info_row}"
for field in "version=" "sha=" "target=" "profile=${profile}" "built=" "providers="; do
    require_substring "${build_info_row}" "${field}" "doctor build_info"
done

# Cross-check: the SHA in --version must equal the SHA in doctor.
sha_from_version="$(sed -nE 's/^orbcode [0-9.]+ \(([a-f0-9]+)(\+dirty)?\)$/\1/p' <<<"${first_line}")"
sha_from_doctor="$(sed -nE 's/.*sha=([a-f0-9]+)(\+dirty)?.*/\1/p' <<<"${build_info_row}")"
if [[ "${sha_from_version}" != "${sha_from_doctor}" ]]; then
    echo "ERROR: SHA mismatch: --version=${sha_from_version} doctor=${sha_from_doctor}" >&2
    exit 1
fi
echo "    sha=${sha_from_version} (matches across --version and doctor)"

echo "==> orbcode providers command smoke"
providers_out="$(
    "${isolated_env[@]}" \
        CLAUDE_CONFIG_DIR="${scratch}/providers-home" \
        ANTHROPIC_BASE_URL="stub://anthropic" \
        PROVIDER_TYPE="anthropic" \
        "${binary}" providers 2>&1
)" || {
    echo "ERROR: orbcode providers exited non-zero" >&2
    echo "${providers_out}" >&2
    exit 1
}
echo "${providers_out}" | sed -n '1,5p'
for field in "active chain:" "provider=anthropic" "permissions=" "anthropic" "openai" "model=" "capabilities="; do
    require_substring "${providers_out}" "${field}" "providers"
done

echo "==> orbcode sessions command smoke"
sessions_out="$(
    "${isolated_env[@]}" \
        CLAUDE_CONFIG_DIR="${scratch}/sessions-home" \
        "${binary}" sessions 2>&1
)" || {
    echo "ERROR: orbcode sessions exited non-zero" >&2
    echo "${sessions_out}" >&2
    exit 1
}
echo "${sessions_out}"
require_substring "${sessions_out}" "no persisted sessions" "sessions"
sessions_json_out="$(
    "${isolated_env[@]}" \
        CLAUDE_CONFIG_DIR="${scratch}/sessions-home" \
        "${binary}" sessions --json 2>&1
)" || {
    echo "ERROR: orbcode sessions --json exited non-zero" >&2
    echo "${sessions_json_out}" >&2
    exit 1
}
if [[ -n "${sessions_json_out}" ]]; then
    echo "ERROR: empty orbcode sessions --json should emit no rows" >&2
    echo "${sessions_json_out}" >&2
    exit 1
fi

# Portable timeout: GNU `timeout` is absent on stock macOS and on the Git-bash
# shell used by Windows CI, so poll the child ourselves. Returns 124 on kill.
run_with_timeout() {
    local secs="$1"
    shift
    "$@" &
    local pid=$!
    local waited=0
    while kill -0 "${pid}" 2>/dev/null; do
        if (( waited >= secs )); then
            kill -TERM "${pid}" 2>/dev/null || true
            sleep 1
            kill -KILL "${pid}" 2>/dev/null || true
            wait "${pid}" 2>/dev/null || true
            return 124
        fi
        sleep 1
        waited=$((waited + 1))
    done
    wait "${pid}"
    return $?
}

echo "==> TUI startup smoke (headless: no controlling terminal)"
# Launch the interactive TUI with stdin detached from any TTY. The runtime
# boots the AppServer, then `setup_terminal()` fails fast because raw mode
# needs a real terminal. The contract we assert: the binary reaches the TUI
# runtime and exits promptly instead of hanging or panic-looping.
tui_out="${scratch}/tui.out"
tui_err="${scratch}/tui.err"
set +e
(
    unset ORBCODE_HOME PROVIDER_TYPE ORBCODE_PROVIDER CLAUDE_CODE_USE_OPENAI
    export CLAUDE_CONFIG_DIR="${scratch}/tui-home"
    export ANTHROPIC_BASE_URL="stub://anthropic"
    export PROVIDER_TYPE="anthropic"
    run_with_timeout 60 "${binary}" tui
) </dev/null >"${tui_out}" 2>"${tui_err}"
tui_code=$?
set -e
if [[ "${tui_code}" -eq 124 ]]; then
    echo "ERROR: TUI did not exit within timeout (possible hang)" >&2
    echo "----- tui stderr -----" >&2
    cat "${tui_err}" >&2
    exit 1
fi
echo "    TUI exited with code ${tui_code} (no hang)"
if [[ -s "${tui_err}" ]]; then
    echo "    stderr: $(head -n1 "${tui_err}")"
fi

if [[ "${do_package}" -eq 1 ]]; then
    echo "==> packaging release artifact for host target"
    pkg_out="${scratch}/dist"
    "${repo_root}/scripts/package-release.sh" --no-build --out-dir "${pkg_out}"
    eval "$("${repo_root}/scripts/package-release.sh" --print-name)"
    archive_path="${pkg_out}/${archive_file}"
    checksum_path="${archive_path}.sha256"
    if [[ ! -f "${archive_path}" ]]; then
        echo "ERROR: expected archive missing: ${archive_path}" >&2
        exit 1
    fi
    echo "==> verifying sha256 checksum of packaged archive"
    if command -v sha256sum >/dev/null 2>&1; then
        ( cd "${pkg_out}" && sha256sum -c "${archive_file}.sha256" )
    elif command -v shasum >/dev/null 2>&1; then
        ( cd "${pkg_out}" && shasum -a 256 -c "${archive_file}.sha256" )
    else
        echo "ERROR: no sha256 tool to verify checksum" >&2
        exit 1
    fi
    echo "    archive: ${archive_path}"
fi

echo "==> cargo test -p orbcode --test build_smoke"
cargo test -p orbcode --test build_smoke ${cargo_profile_args[@]+"${cargo_profile_args[@]}"}

echo
echo "OK: build metadata embedded; packaged binary smoke pipeline green."
