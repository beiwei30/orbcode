#!/usr/bin/env bash
# Release-readiness smoke pipeline for the orbcode binary.
#
# Validates that build metadata (git SHA, target, profile, build timestamp,
# providers) is embedded into the packaged binary and that --version, -V,
# and `doctor` all agree. It also exercises the packaged-binary behavior covered
# by cli/tests/build_smoke.rs without compiling a separate Rust test harness.
# Run locally and from CI before publishing a binary.
#
# Usage:
#   scripts/smoke-release.sh             # debug build + smoke tests (fast)
#   scripts/smoke-release.sh --release   # release build + smoke tests
#   scripts/smoke-release.sh --release --package
#                                        # also build + verify the packaged
#                                        # release archive for the host target
#   scripts/smoke-release.sh --release --no-build --target <triple>
#                                        # validate a prebuilt target binary
#
# Exit codes: 0 = all checks pass; non-zero = first failing check.

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${repo_root}"

profile="debug"
cargo_profile_args=()
target_dir_segment="debug"
do_package=0
do_build=1
target=""
binary_override=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        --release)
            profile="release"
            cargo_profile_args=(--release)
            target_dir_segment="release"
            shift
            ;;
        --package)
            do_package=1
            shift
            ;;
        --no-build)
            do_build=0
            shift
            ;;
        --target)
            if [[ $# -lt 2 || -z "$2" ]]; then
                echo "ERROR: --target requires a target triple" >&2
                exit 2
            fi
            target="$2"
            shift 2
            ;;
        --binary)
            if [[ $# -lt 2 || -z "$2" ]]; then
                echo "ERROR: --binary requires a path" >&2
                exit 2
            fi
            binary_override="$2"
            do_build=0
            shift 2
            ;;
        *)
            echo "ERROR: unknown argument: $1" >&2
            exit 2
            ;;
    esac
done

echo "==> smoke-release.sh (profile=${profile})"
echo "    repo: ${repo_root}"

target_args=()
if [[ -n "${target}" ]]; then
    target_args=(--target "${target}")
fi

if [[ "${do_build}" -eq 1 ]]; then
    echo "==> cargo build -p orbcode ${cargo_profile_args[*]-} ${target_args[*]-}"
    cargo build -p orbcode \
        ${cargo_profile_args[@]+"${cargo_profile_args[@]}"} \
        ${target_args[@]+"${target_args[@]}"}
else
    echo "==> reusing prebuilt orbcode binary"
fi

bin_suffix=""
case "$(uname -s 2>/dev/null || echo unknown)" in
    MINGW*|MSYS*|CYGWIN*) bin_suffix=".exe" ;;
esac
if [[ -n "${target}" && "${target}" == *windows* ]]; then
    bin_suffix=".exe"
fi
if [[ -n "${binary_override}" ]]; then
    if [[ "${binary_override}" = /* ]]; then
        binary="${binary_override}"
    else
        binary="${repo_root}/${binary_override}"
    fi
elif [[ -n "${target}" ]]; then
    binary="${repo_root}/target/${target}/${target_dir_segment}/orbcode${bin_suffix}"
else
    binary="${repo_root}/target/${target_dir_segment}/orbcode${bin_suffix}"
fi
if [[ ! -x "${binary}" ]]; then
    echo "ERROR: executable orbcode binary not found at ${binary}" >&2
    exit 1
fi
echo "    binary: ${binary}"

echo "==> --version (long form) parses"
version_long="$("${binary}" --version)"
version_long="${version_long//$'\r'/}"
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
if [[ -n "${target}" ]]; then
    require_substring "${version_long}" "target:    ${target}" "--version target"
fi

echo "==> -V (short form) parses"
version_short="$("${binary}" -V)"
version_short="${version_short//$'\r'/}"
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
help_out="${help_out//$'\r'/}"
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
doctor_out="${doctor_out//$'\r'/}"
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
providers_out="${providers_out//$'\r'/}"
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
sessions_out="${sessions_out//$'\r'/}"
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
sessions_json_out="${sessions_json_out//$'\r'/}"
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
# runtime and exits promptly instead of hanging or panic-looping. Windows keeps
# the previous release workflow behavior because its Git Bash process/TTY
# emulation cannot reliably exercise this contract.
case "$(uname -s 2>/dev/null || echo unknown)" in
    MINGW*|MSYS*|CYGWIN*)
        echo "    skipped on Windows (Git Bash has no native TTY contract)"
        ;;
    *)
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
        ;;
esac

echo "==> packaged prompt, transcript, and project-path smoke"
prompt_home="${scratch}/prompt-home"
prompt_cwd="${scratch}/deep/nested/project"
mkdir -p "${prompt_home}" "${prompt_cwd}"
prompt_text="smoke transcript path separator"
prompt_out="${scratch}/prompt.out"
prompt_err="${scratch}/prompt.err"
(
    cd "${prompt_cwd}"
    "${isolated_env[@]}" \
        CLAUDE_CONFIG_DIR="${prompt_home}" \
        ANTHROPIC_BASE_URL="stub://anthropic" \
        PROVIDER_TYPE="anthropic" \
        "${binary}" prompt "${prompt_text}"
) >"${prompt_out}" 2>"${prompt_err}" || {
        echo "ERROR: packaged prompt smoke exited non-zero" >&2
        echo "----- stdout -----" >&2
        cat "${prompt_out}" >&2
        echo "----- stderr -----" >&2
        cat "${prompt_err}" >&2
        exit 1
    }
prompt_stdout="$(cat "${prompt_out}")"
prompt_stderr="$(cat "${prompt_err}")"
prompt_stdout="${prompt_stdout//$'\r'/}"
prompt_stderr="${prompt_stderr//$'\r'/}"
require_substring "${prompt_stdout}" "session " "packaged prompt session header"
require_substring \
    "${prompt_stdout}" \
    "Anthropic compatibility stub response" \
    "packaged prompt stub response"
require_substring \
    "${prompt_stderr}" \
    "completed with anthropic" \
    "packaged prompt completion"

projects_dir="${prompt_home}/projects"
if [[ ! -d "${projects_dir}" ]]; then
    echo "ERROR: prompt smoke did not create ${projects_dir}" >&2
    exit 1
fi
shopt -s nullglob
project_dirs=("${projects_dir}"/*)
shopt -u nullglob
if [[ "${#project_dirs[@]}" -ne 1 || ! -d "${project_dirs[0]}" ]]; then
    echo "ERROR: expected exactly one project directory under ${projects_dir}" >&2
    exit 1
fi
project_slug="$(basename "${project_dirs[0]}")"
if [[ ! "${project_slug}" =~ ^[A-Za-z0-9-]+$ ]]; then
    echo "ERROR: project slug contains an unsanitized path separator: ${project_slug}" >&2
    exit 1
fi

shopt -s nullglob
transcripts=("${project_dirs[0]}"/*.jsonl)
shopt -u nullglob
if [[ "${#transcripts[@]}" -lt 1 ]]; then
    echo "ERROR: no transcript was persisted under ${project_dirs[0]}" >&2
    exit 1
fi
transcript="${transcripts[0]}"
if [[ ! -s "${transcript}" ]]; then
    echo "ERROR: persisted transcript is empty: ${transcript}" >&2
    exit 1
fi
if ! grep -qF -- "${prompt_text}" "${transcript}"; then
    echo "ERROR: persisted transcript does not contain the prompt" >&2
    exit 1
fi

python_command=""
if command -v python3 >/dev/null 2>&1; then
    python_command="python3"
elif command -v python >/dev/null 2>&1; then
    python_command="python"
else
    echo "ERROR: Python is required to validate persisted transcript JSONL" >&2
    exit 1
fi
"${python_command}" - "${transcript}" <<'PY'
import json
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
for line_number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
    if line.strip():
        try:
            json.loads(line)
        except json.JSONDecodeError as error:
            raise SystemExit(f"{path}:{line_number}: invalid JSON: {error}") from error
PY
echo "    project slug=${project_slug}; transcript JSONL valid"

if [[ "${do_package}" -eq 1 ]]; then
    echo "==> packaging release artifact for host target"
    pkg_out="${scratch}/dist"
    package_target_args=()
    if [[ -n "${target}" ]]; then
        package_target_args=(--target "${target}")
    fi
    "${repo_root}/scripts/package-release.sh" \
        --no-build \
        "${package_target_args[@]}" \
        --out-dir "${pkg_out}"
    # package-release.sh emits shell assignments for the stable artifact name.
    # shellcheck disable=SC1090
    eval "$(
        "${repo_root}/scripts/package-release.sh" \
            --print-name \
            "${package_target_args[@]}"
    )"
    # Assigned by the evaluated --print-name output above.
    # shellcheck disable=SC2154
    archive_path="${pkg_out}/${archive_file}"
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

echo
echo "OK: build metadata embedded; packaged binary smoke pipeline green."
