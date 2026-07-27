#!/usr/bin/env bash
#
# Canonical local verification script for orbcode.
# Runs the same checks as CI so local failures surface before push.
#
# Usage:
#   scripts/check.sh                       # full: fmt → clippy → check → test
#   scripts/check.sh --quick               # fast: fmt → clippy → check (no tests)
#   scripts/check.sh --release             # full pipeline in release profile
#   scripts/check.sh --crate orbcode-config  # test only the specified crate
#   scripts/check.sh --pty-e2e             # ONLY the #[ignore]d PTY e2e tests (serial)
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${REPO_ROOT}"

QUICK=false
PROFILE_FLAG=""
CRATE=""
PTY_E2E_ONLY=false

usage() {
    cat >&2 <<'EOF'
Usage: scripts/check.sh [OPTIONS]

Options:
  --quick           Skip tests (fmt + clippy + check only)
  --release         Use the release profile
  --crate <name>    Run tests for a single crate only (e.g. orbcode-config)
  --pty-e2e         Run ONLY the load-sensitive #[ignore]d PTY e2e tests, serially
                    (for a dedicated, uncontended CI job)
  --help            Show this help message

Examples:
  scripts/check.sh                       # full workspace check (PTY e2e NOT run)
  scripts/check.sh --quick               # fast pre-push sanity check
  scripts/check.sh --crate orbcode-core    # iterate on one crate's tests
  scripts/check.sh --pty-e2e             # isolated PTY e2e run for a dedicated CI job
  scripts/check.sh --release --crate orbcode
EOF
    exit 1
}

while [ $# -gt 0 ]; do
    case "$1" in
        --quick)   QUICK=true; shift ;;
        --release) PROFILE_FLAG="--release"; shift ;;
        --pty-e2e) PTY_E2E_ONLY=true; shift ;;
        --crate)
            if [ -z "${2:-}" ]; then
                echo "error: --crate requires a crate name" >&2
                usage
            fi
            CRATE="$2"; shift 2 ;;
        --help|-h) usage ;;
        *)
            echo "error: unknown option '$1'" >&2
            usage
            ;;
    esac
done

step() { printf '\n\033[1;34m==> %s\033[0m\n' "$1"; }

# Run the three #[ignore]d PTY smoke tests ONCE, serially, wall-clock bounded by
# a portable watchdog (so the run always COMPLETES rather than hanging, even
# where `timeout`/`gtimeout` is absent). Returns the test exit code.
run_pty_e2e_once() {
    local timeout_secs="$1"
    cargo test -p orbcode --test tui_remote_pty_e2e ${PROFILE_FLAG} \
        -- --ignored --test-threads=1 &
    local test_pid=$!
    # The watchdog's stdio is detached to /dev/null so its `sleep` child can never
    # hold this script's stdout open (which would stall a piped caller).
    (
        sleep "$timeout_secs"
        kill -0 "$test_pid" 2>/dev/null && kill -TERM "$test_pid" 2>/dev/null
        sleep 5
        kill -0 "$test_pid" 2>/dev/null && kill -KILL "$test_pid" 2>/dev/null
    ) >/dev/null 2>&1 &
    local watchdog_pid=$!
    local rc=0
    wait "$test_pid" || rc=$?
    # Test finished first — stop the watchdog subshell AND its `sleep` child.
    pkill -P "$watchdog_pid" 2>/dev/null || true
    kill "$watchdog_pid" 2>/dev/null || true
    wait "$watchdog_pid" 2>/dev/null || true
    return "$rc"
}

# The PTY e2e tests drive a real pseudo-terminal with blocking I/O. They are
# #[ignore]d so the default parallel `cargo test --workspace` neither runs nor
# hangs on them, and are run here serially on a dedicated runner. They are also
# TIMING-FLAKY: the child TUI's first-frame render / cursor-position (DSR)
# handshake occasionally does not complete in time (reproduced ~1 in 9 runs even
# on an idle machine), which the watchdog turns into a bounded failure rather
# than a hang. To keep the gate reliably green while still catching a GENUINE
# regression (which fails every attempt), retry a few times; a real break never
# passes. The build is done first, OUTSIDE the per-attempt watchdog, so a slow
# compile is never mistaken for a test hang.
run_pty_e2e() {
    step "PTY e2e (ignored, serial)"
    cargo test -p orbcode --test tui_remote_pty_e2e ${PROFILE_FLAG} --no-run
    local attempts=3
    local timeout_secs=120
    local attempt
    for attempt in $(seq 1 "$attempts"); do
        if run_pty_e2e_once "$timeout_secs"; then
            if [ "$attempt" -gt 1 ]; then
                echo "PTY e2e passed on attempt ${attempt}/${attempts} (earlier attempts flaked)."
            fi
            return 0
        fi
        echo "PTY e2e attempt ${attempt}/${attempts} failed or timed out (bound ${timeout_secs}s); retrying..." >&2
    done
    echo "error: PTY e2e failed on all ${attempts} attempts. These tests are timing-" >&2
    echo "       flaky, but a genuine regression fails every retry — investigate." >&2
    return 1
}

if [ "$PTY_E2E_ONLY" = true ]; then
    run_pty_e2e
    printf '\n\033[1;32m✓ PTY e2e passed.\033[0m\n'
    exit 0
fi

step "rustfmt --check"
cargo fmt --all --check

step "clippy"
cargo clippy --workspace --all-targets ${PROFILE_FLAG}

step "cargo check"
cargo check --workspace ${PROFILE_FLAG}

# Mirrors the feature-isolation job in .github/workflows/release.yml: exercises
# the `in-process`-off path so a stale `dep:` reference in a feature list fails
# locally instead of in CI.
step "cargo check (no default features)"
cargo check --workspace --no-default-features ${PROFILE_FLAG}

step "public API surface audit"
"${REPO_ROOT}/scripts/audit-public-surface.sh"

step "brand audit"
"${REPO_ROOT}/scripts/audit-brand.sh"

if [ "$QUICK" = true ]; then
    printf '\n\033[1;32m✓ Quick check passed (fmt + clippy + check).\033[0m\n'
    exit 0
fi

if [ -n "$CRATE" ]; then
    step "cargo test -p ${CRATE}"
    cargo test -p "${CRATE}" ${PROFILE_FLAG}
else
    step "cargo test"
    cargo test --workspace ${PROFILE_FLAG}
fi

# NOTE: the #[ignore]d PTY e2e tests are intentionally NOT run here. They hang
# under the CPU contention this parallel run leaves behind, which would make the
# canonical gate itself hang. Run them in a dedicated, uncontended job via
# `scripts/check.sh --pty-e2e`.
printf '\n\033[1;32m✓ All checks passed. (PTY e2e run separately: scripts/check.sh --pty-e2e)\033[0m\n'
