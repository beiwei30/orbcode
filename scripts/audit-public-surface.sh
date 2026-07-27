#!/usr/bin/env bash
#
# Audit the workspace's public API surface.
#
# Extracts every `pub use`, `pub mod`, `pub struct`, `pub enum`, `pub fn`,
# `pub trait`, `pub type`, and `pub const` declaration from each crate's
# lib.rs, normalises them into a sorted allow-list format, and compares
# against the checked-in golden file (public-api-allow-list.txt).
#
# Usage:
#   scripts/audit-public-surface.sh            # diff mode (CI / check.sh)
#   scripts/audit-public-surface.sh --update   # regenerate the golden file
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
GOLDEN="${REPO_ROOT}/public-api-allow-list.txt"

UPDATE=false
if [ "${1:-}" = "--update" ]; then
    UPDATE=true
fi

CRATES=(
    protocol
    config
    model-provider
    session-store
    core
    app-server
    app-server-transport
    tools
    mcp
    tui
    cli
)

# Extract public surface from a single crate's lib.rs.
# Output: one line per public item in the format:
#   crate-name<TAB>kind<TAB>path
extract_surface() {
    local crate_name="$1"
    local lib_rs="${REPO_ROOT}/${crate_name}/src/lib.rs"

    if [ ! -f "$lib_rs" ]; then
        return
    fi

    # Step 1: join multi-line pub use statements into single lines.
    local joined
    joined=$(awk '
    /^pub use / || /^pub\(crate\) use / {
        buf = $0
        while (buf !~ /;[[:space:]]*$/) {
            if ((getline line) <= 0) break
            buf = buf " " line
        }
        print buf
        next
    }
    { print }
    ' "$lib_rs")

    # Step 2: extract declarations using sed + awk for portability
    echo "$joined" | while IFS= read -r line; do
        # pub use with glob: pub use module::*;
        if echo "$line" | grep -qE '^pub use [a-zA-Z_].*::\*;'; then
            path=$(echo "$line" | sed 's/^pub use //; s/;[[:space:]]*$//')
            printf '%s\tpub_use_glob\t%s\n' "$crate_name" "$path"
            continue
        fi

        # pub use with brace group: pub use path::{A, B, C};
        if echo "$line" | grep -qE '^pub use [a-zA-Z_].*[{]'; then
            # Extract prefix (everything before {) using parameter expansion
            temp="${line#pub use }"
            prefix="${temp%%\{*}"
            # Extract items (between { and })
            items="${temp#*\{}"
            items="${items%%\}*}"
            # Split on comma and emit each
            echo "$items" | tr ',' '\n' | while IFS= read -r item; do
                item=$(echo "$item" | sed 's/^[[:space:]]*//; s/[[:space:]]*$//')
                if [ -n "$item" ]; then
                    printf '%s\tpub_use\t%s%s\n' "$crate_name" "$prefix" "$item"
                fi
            done
            continue
        fi

        # Simple pub use: pub use path::item;
        if echo "$line" | grep -qE '^pub use [a-zA-Z_]'; then
            path=$(echo "$line" | sed 's/^pub use //; s/;[[:space:]]*$//')
            printf '%s\tpub_use\t%s\n' "$crate_name" "$path"
            continue
        fi

        # pub mod name
        if echo "$line" | grep -qE '^pub mod [a-zA-Z_]'; then
            name=$(echo "$line" | sed 's/^pub mod //; s/;[[:space:]]*$//; s/[[:space:]]*{.*//')
            printf '%s\tpub_mod\t%s\n' "$crate_name" "$name"
            continue
        fi

        # pub struct Name
        if echo "$line" | grep -qE '^pub struct [A-Za-z_]'; then
            name=$(echo "$line" | sed 's/^pub struct //; s/[^A-Za-z0-9_].*//')
            printf '%s\tpub_struct\t%s\n' "$crate_name" "$name"
            continue
        fi

        # pub enum Name
        if echo "$line" | grep -qE '^pub enum [A-Za-z_]'; then
            name=$(echo "$line" | sed 's/^pub enum //; s/[^A-Za-z0-9_].*//')
            printf '%s\tpub_enum\t%s\n' "$crate_name" "$name"
            continue
        fi

        # pub async fn name or pub fn name
        if echo "$line" | grep -qE '^pub (async )?fn [a-z_]'; then
            name=$(echo "$line" | sed 's/^pub \(async \)\{0,1\}fn //; s/[^A-Za-z0-9_].*//')
            printf '%s\tpub_fn\t%s\n' "$crate_name" "$name"
            continue
        fi

        # pub trait Name
        if echo "$line" | grep -qE '^pub trait [A-Za-z_]'; then
            name=$(echo "$line" | sed 's/^pub trait //; s/[^A-Za-z0-9_].*//')
            printf '%s\tpub_trait\t%s\n' "$crate_name" "$name"
            continue
        fi

        # pub type Name
        if echo "$line" | grep -qE '^pub type [A-Za-z_]'; then
            name=$(echo "$line" | sed 's/^pub type //; s/[^A-Za-z0-9_].*//')
            printf '%s\tpub_type\t%s\n' "$crate_name" "$name"
            continue
        fi

        # pub const NAME
        if echo "$line" | grep -qE '^pub const [A-Za-z_]'; then
            name=$(echo "$line" | sed 's/^pub const //; s/[^A-Za-z0-9_].*//')
            printf '%s\tpub_const\t%s\n' "$crate_name" "$name"
            continue
        fi
    done
}

# Generate the full sorted surface
generate_surface() {
    for crate_name in "${CRATES[@]}"; do
        extract_surface "$crate_name"
    done | sort
}

if [ "$UPDATE" = true ]; then
    {
        echo "# Public API surface allow-list for the orbcode workspace."
        echo "# Auto-generated by: scripts/audit-public-surface.sh --update"
        echo "#"
        echo "# To verify: scripts/audit-public-surface.sh"
        echo "# To update after intentional changes: scripts/audit-public-surface.sh --update"
        echo "#"
        echo "# Format: crate-name<TAB>kind<TAB>path"
        echo ""
        generate_surface
    } > "$GOLDEN"
    echo "Updated: ${GOLDEN}"
    exit 0
fi

# Diff mode: compare current surface against golden file.
if [ ! -f "$GOLDEN" ]; then
    echo "error: golden file not found: ${GOLDEN}" >&2
    echo "       Run: scripts/audit-public-surface.sh --update" >&2
    exit 1
fi

ACTUAL=$(generate_surface)
EXPECTED=$(grep -v '^#' "$GOLDEN" | grep -v '^$' | sort)

if [ "$ACTUAL" != "$EXPECTED" ]; then
    echo "Public API surface mismatch!" >&2
    echo "" >&2
    echo "--- expected (public-api-allow-list.txt)" >&2
    echo "+++ actual (current lib.rs files)" >&2
    echo "" >&2
    diff <(echo "$EXPECTED") <(echo "$ACTUAL") >&2 || true
    echo "" >&2
    echo "To update the allow-list after intentional changes:" >&2
    echo "  scripts/audit-public-surface.sh --update" >&2
    exit 1
fi
