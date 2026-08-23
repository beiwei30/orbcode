#!/usr/bin/env bash
# Prove normal/release Orb Code builds cannot activate ChatGPT OAuth test inputs.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${REPO_ROOT}"

binary=""
target=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        --binary)
            [[ $# -ge 2 && -n "$2" ]] || {
                echo "error: --binary requires a path" >&2
                exit 2
            }
            binary="$2"
            shift 2
            ;;
        --target)
            [[ $# -ge 2 && -n "$2" ]] || {
                echo "error: --target requires a Rust target triple" >&2
                exit 2
            }
            target="$2"
            shift 2
            ;;
        --help|-h)
            echo "usage: scripts/audit-chatgpt-auth-release.sh [--binary PATH] [--target TRIPLE]"
            exit 0
            ;;
        *)
            echo "error: unknown argument: $1" >&2
            exit 2
            ;;
    esac
done

echo "==> default/release dependency feature graph"
if [[ -n "${target}" ]]; then
    feature_graph="$(cargo tree -p orbcode --edges normal,build,features --target "${target}")"
else
    feature_graph="$(cargo tree -p orbcode --edges normal,build,features)"
fi
if grep -Fq 'orbcode-config feature "oauth-test-support"' <<<"${feature_graph}"; then
    echo "error: normal orbcode feature graph activates oauth-test-support" >&2
    exit 1
fi

if [[ -z "${binary}" ]]; then
    echo "==> build ordinary release binary"
    if [[ -n "${target}" ]]; then
        cargo build -p orbcode --release --target "${target}"
    else
        cargo build -p orbcode --release
    fi
    target_dir="${CARGO_TARGET_DIR:-${REPO_ROOT}/target}"
    if [[ "${target_dir}" != /* ]]; then
        target_dir="${REPO_ROOT}/${target_dir}"
    fi
    suffix=""
    if rustc -vV | grep -Fq 'host: x86_64-pc-windows' || [[ "${target}" == *-windows-* ]]; then
        suffix=".exe"
    fi
    if [[ -n "${target}" ]]; then
        binary="${target_dir}/${target}/release/orbcode${suffix}"
    else
        binary="${target_dir}/release/orbcode${suffix}"
    fi
elif [[ "${binary}" != /* ]]; then
    binary="${REPO_ROOT}/${binary}"
fi

if [[ ! -x "${binary}" && ! -f "${binary}" ]]; then
    echo "error: release binary not found: ${binary}" >&2
    exit 1
fi

test_prefix="ORBCODE""_TEST_OPENAI_"
echo "==> release binary excludes test-variable names"
if LC_ALL=C grep -aFq "${test_prefix}" "${binary}"; then
    echo "error: release binary contains ChatGPT OAuth test-variable names" >&2
    exit 1
fi

release_binary="${binary}"
if command -v cygpath >/dev/null 2>&1; then
    release_binary="$(cygpath -w "${binary}")"
fi

scenario_targets=(
    --test chatgpt_auth_harness_e2e
    --test chatgpt_auth_browser_e2e
    --test chatgpt_auth_device_e2e
    --test chatgpt_auth_failure_e2e
    --test chatgpt_auth_state_e2e
    --test chatgpt_responses_cli_e2e
    --test chatgpt_auth_security_e2e
)
echo "==> cross-platform ChatGPT auth scenario cluster"
if [[ -n "${target}" ]]; then
    cargo test -p orbcode --target "${target}" "${scenario_targets[@]}" -- --test-threads=4
else
    cargo test -p orbcode "${scenario_targets[@]}" -- --test-threads=4
fi

echo "==> release binary ignores every test endpoint override"
if [[ -n "${target}" ]]; then
    ORBCODE_RELEASE_BIN="${release_binary}" cargo test -p orbcode \
        --test chatgpt_auth_security_e2e --target "${target}" \
        release_binary_ignores_test_endpoint_overrides -- \
        --ignored --exact --test-threads=1
else
    ORBCODE_RELEASE_BIN="${release_binary}" cargo test -p orbcode \
        --test chatgpt_auth_security_e2e \
        release_binary_ignores_test_endpoint_overrides -- \
        --ignored --exact --test-threads=1
fi

echo "ChatGPT auth release-boundary audit passed."
