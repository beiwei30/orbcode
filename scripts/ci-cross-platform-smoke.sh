#!/usr/bin/env bash
#
# CI cross-platform smoke tests.
#
# Runs the platform-specific #[ignore] sandbox host-validation tests that are
# normally skipped during `cargo test --workspace`. Each platform unlocks a
# different subset:
#
#   Linux  — bubblewrap sandbox baseline (requires `bwrap` on PATH)
#   macOS  — seatbelt (sandbox-exec) baseline (built-in on macOS)
#   Windows — path-normalization build_smoke only (sandbox runner not yet in CI)
#
# Also runs the path-normalization build_smoke test on every platform.
#
# Usage:
#   scripts/ci-cross-platform-smoke.sh              # auto-detect OS
#   scripts/ci-cross-platform-smoke.sh --release     # release profile
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${REPO_ROOT}"

PROFILE_FLAG=""
for arg in "$@"; do
    case "$arg" in
        --release) PROFILE_FLAG="--release" ;;
        *)
            echo "Usage: scripts/ci-cross-platform-smoke.sh [--release]" >&2
            exit 1
            ;;
    esac
done

step() { printf '\n\033[1;34m==> %s\033[0m\n' "$1"; }
warn() { printf '\033[1;33m    ⚠ %s\033[0m\n' "$1"; }
pass() { printf '\033[1;32m    ✓ %s\033[0m\n' "$1"; }

OS="$(uname -s)"
FAILED=0

step "Path-normalization smoke (all platforms)"
if cargo test -p orbcode --test build_smoke ${PROFILE_FLAG} -- project_dir_path_uses_sanitized_separators; then
    pass "path normalization"
else
    warn "path normalization FAILED"
    FAILED=1
fi

case "$OS" in
    Linux)
        step "Linux bubblewrap sandbox host tests"
        if command -v bwrap >/dev/null 2>&1; then
            export ORBCODE_RUN_LINUX_SANDBOX_HOST_TESTS=1
            if cargo test -p orbcode-tools ${PROFILE_FLAG} -- --ignored linux_bubblewrap_host; then
                pass "bubblewrap host validation"
            else
                warn "bubblewrap host validation FAILED"
                FAILED=1
            fi
        else
            warn "bwrap not found — skipping bubblewrap host tests (install bubblewrap to enable)"
        fi
        ;;

    Darwin)
        step "macOS seatbelt sandbox host tests"
        if command -v sandbox-exec >/dev/null 2>&1; then
            export ORBCODE_RUN_MACOS_SANDBOX_HOST_TESTS=1
            if cargo test -p orbcode-tools ${PROFILE_FLAG} -- --ignored macos_seatbelt_host; then
                pass "seatbelt host validation"
            else
                warn "seatbelt host validation FAILED"
                FAILED=1
            fi
        else
            warn "sandbox-exec not found — skipping seatbelt host tests"
        fi
        ;;

    MINGW*|MSYS*|CYGWIN*|Windows_NT)
        step "Windows sandbox host tests (path smoke only — sandbox runner not yet in CI)"
        pass "path normalization covers Windows path separator handling"
        ;;

    *)
        warn "Unknown OS '$OS' — only path-normalization smoke ran"
        ;;
esac

echo ""
if [ "$FAILED" -eq 0 ]; then
    printf '\033[1;32m✓ Cross-platform smoke passed.\033[0m\n'
else
    printf '\033[1;31m✗ Cross-platform smoke had failures.\033[0m\n'
    exit 1
fi
