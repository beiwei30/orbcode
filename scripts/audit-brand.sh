#!/usr/bin/env bash
#
# Guard against the pre-rebrand name leaking back in.
#
# The project was renamed cc-rs -> orbcode (packages cc-rs-* -> orbcode-*, crate
# paths cc_rs_* -> orbcode_*, env prefix CC_RS_* -> ORBCODE_*). The old names are
# a HARD BREAK: nothing accepts them any more. This script fails if any of them
# reappear outside the explicitly-allowed historical material.
#
# It also asserts the opposite direction: that the TypeScript-CLI compatibility
# names are still PRESENT. Those are not our branding, they are the contract that
# lets us share ~/.claude with the TypeScript CLI, and an over-eager rename would
# silently break byte compatibility.
#
# Usage:
#   scripts/audit-brand.sh
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${REPO_ROOT}"

FAILED=0

# ── Paths exempt from the scan, each with its reason ───────────────────────────
#
# compat-fixtures/fixtures/**  captured TypeScript-CLI payloads. One carries a
#                              "Compiling cc-rs-core" bash-progress line; it must
#                              stay exactly as captured.
# session-store/src/transcript_schema.rs
#                              contains the regression test that decodes a
#                              PRE-RENAME transcript line, proving old sessions
#                              still load. The old values are the fixture.
#
# The TUI render fixtures used to need exemptions of their own: they carried a
# pre-rename directory name from the machine the goldens were captured on, plus
# the captured conversation text that went with it. That scenario has been
# replaced with a synthetic one, so tui/testdata/** and the tui test fixtures are
# back under the guard — no masking, no per-file exemption.
EXEMPT_PATHS=(
    ':(exclude)compat-fixtures/fixtures/**'
    ':(exclude)session-store/src/transcript_schema.rs'
    ':(exclude)scripts/audit-brand.sh'
)

scan() {
    local pattern="$1"
    local label="$2"
    local hits
    # -I skips binary files. `--untracked` matters: without it a brand-new file
    # is invisible to the guard until it is committed, which is exactly when you
    # least want a blind spot.
    hits=$(git grep -I -n --untracked --fixed-strings "$pattern" -- . "${EXEMPT_PATHS[@]}" || true)
    if [ -n "$hits" ]; then
        echo "error: found '${label}' — the old name is a hard break, not a supported alias:" >&2
        echo "$hits" | sed 's/^/  /' >&2
        echo "" >&2
        FAILED=1
    fi
}

scan_regex() {
    local pattern="$1"
    local label="$2"
    local hits
    hits=$(git grep -I -n --untracked -E "$pattern" -- . "${EXEMPT_PATHS[@]}" || true)
    if [ -n "$hits" ]; then
        echo "error: found '${label}':" >&2
        echo "$hits" | sed 's/^/  /' >&2
        echo "" >&2
        FAILED=1
    fi
}

echo "==> scanning for pre-rebrand names"
scan 'CC_RS_' 'CC_RS_ (env prefix)'
scan 'cc_rs' 'cc_rs (crate path / identifier)'
scan 'cc-rs' 'cc-rs (product, package, binary)'
# Catch-all for casings the three literal scans miss. This exists because a
# PascalCase `CcRsClient` in the TypeScript client survived the whole rebrand:
# it matches none of `cc-rs`, `cc_rs`, `CC_RS_`. The separator is optional so
# CcRs/ccRs/CCRS are covered; `.` is deliberately NOT a separator here, to avoid
# firing on a hypothetical source file named `…cc.rs`.
scan_regex '[Cc][Cc][-_]?[Rr][Ss]' 'a pre-rebrand name in some other casing'

# ── Positive controls: TypeScript-CLI compatibility must NOT be renamed away ───
echo "==> verifying TypeScript-CLI compatibility names survive"
REQUIRED=(
    'CLAUDE_CONFIG_DIR'
    'CLAUDE_CODE_OAUTH_TOKEN'
    'ANTHROPIC_API_KEY'
    'claude_code_version'
    'x-claude-code-session-id'
    'managed-settings.json'
    '.credentials.json'
    'history.jsonl'
    'settings.json'
    '.claude'
)
# Scoped to SOURCE, not prose: if these only survived in README/docs, a rename
# that gutted the actual implementation would still pass. Markdown is excluded
# for exactly that reason — and so is THIS script, whose REQUIRED list would
# otherwise satisfy its own check.
#
# `--untracked` matches the scan direction above. Without it this loop only sees
# the index, so a tree whose files are all present but not yet added reports
# every compatibility name as "disappeared" — a wall of false alarms in exactly
# the situation (fresh checkout-by-copy, pre-commit) where the guard is most
# likely to be run. The cost is that an untracked scratch copy could satisfy the
# check on its own; that is far less likely than the false alarm it prevents, and
# ignored paths (target/, node_modules/) stay excluded either way.
for name in "${REQUIRED[@]}"; do
    if ! git grep -I -q --untracked --fixed-strings "$name" -- \
            '*.rs' '*.ts' '*.toml' '*.json' '*.sh' \
            ':(exclude)compat-fixtures/fixtures/**' \
            ':(exclude)scripts/audit-brand.sh'; then
        echo "error: '${name}' has disappeared from the source tree." >&2
        echo "       It is a TypeScript-CLI compatibility name, not orbcode branding." >&2
        echo "       Renaming it breaks on-disk/wire compatibility. Restore it." >&2
        FAILED=1
    fi
done

if [ "$FAILED" -ne 0 ]; then
    echo "Brand audit FAILED." >&2
    exit 1
fi

echo "✓ Brand audit passed."
