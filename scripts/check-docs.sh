#!/usr/bin/env bash
# Verify the bilingual user-manual inventory and repository-local Markdown links.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${REPO_ROOT}"

failures=0

fail() {
    echo "docs audit: $*" >&2
    failures=$((failures + 1))
}

if [[ ! -f README.md || ! -f README.zh-CN.md ]]; then
    fail "README.md and README.zh-CN.md must both exist"
fi

# Every top-level English manual page has a Chinese peer, and vice versa.
# Historical design records under docs/plans are deliberately outside this
# user-manual mirror.
for english in docs/*.md; do
    name="$(basename "${english}")"
    chinese="docs/zh-CN/${name}"
    if [[ ! -f "${chinese}" ]]; then
        fail "missing Chinese peer for ${english}: ${chinese}"
        continue
    fi
    if [[ "${name}" == "README.md" ]]; then
        english_to_chinese="zh-CN/README.md"
    else
        english_to_chinese="zh-CN/${name}"
    fi
    if ! grep -Fq "(${english_to_chinese})" "${english}"; then
        fail "${english} does not link to ${chinese}"
    fi
    if ! grep -Fq "(../${name})" "${chinese}"; then
        fail "${chinese} does not link to ${english}"
    fi
done

for chinese in docs/zh-CN/*.md; do
    name="$(basename "${chinese}")"
    if [[ ! -f "docs/${name}" ]]; then
        fail "missing English peer for ${chinese}: docs/${name}"
    fi
done

# These inventories come directly from the source so adding a command/control
# cannot leave both language guides stale without failing CI.
while IFS= read -r command; do
    [[ -z "${command}" ]] && continue
    for guide in docs/user-guide.md docs/zh-CN/user-guide.md; do
        if ! grep -Fq "\`/${command}\`" "${guide}"; then
            fail "${guide} is missing built-in slash command /${command}"
        fi
    done
done < <(
    sed -n '/pub(crate) const BUILTIN_SLASH_COMMANDS/,/^];/p' \
        tui/src/slash_commands/registry.rs \
        | sed -n 's/^[[:space:]]*name: "\([^"]*\)".*/\1/p'
)

while IFS= read -r control; do
    [[ -z "${control}" ]] && continue
    for guide in docs/stream-json.md docs/zh-CN/stream-json.md; do
        if ! grep -Fq "\`${control}\`" "${guide}"; then
            fail "${guide} is missing stream-JSON control ${control}"
        fi
    done
done < <(
    sed -n '/pub const SUPPORTED_CONTROL_SUBTYPES/,/];/p; /pub const UNSUPPORTED_CONTROL_SUBTYPES/p' \
        protocol/src/control.rs \
        | grep -o '"[a-z_]*"' \
        | tr -d '"'
)

while IFS= read -r command; do
    [[ -z "${command}" || "${command}" == "bg-worker" ]] && continue
    for guide in docs/cli-reference.md docs/zh-CN/cli-reference.md; do
        if ! grep -Fq "\`${command}" "${guide}"; then
            fail "${guide} is missing top-level CLI command ${command}"
        fi
    done
done < <(
    sed -n '/pub(crate) enum Command {/,/^}/p' cli/src/args.rs \
        | sed -n 's/^    \([A-Z][A-Za-z]*\)[,{].*/\1/p' \
        | perl -pe 's/([a-z0-9])([A-Z])/$1-$2/g; $_ = lc $_'
)

# Extract ordinary inline Markdown links. The manual intentionally uses no
# repository-local paths containing a closing parenthesis, keeping this parser
# small and dependency-free beyond Perl (already present on supported hosts).
markdown_files=(README.md README.zh-CN.md)
while IFS= read -r file; do
    markdown_files+=("${file}")
done < <(find docs -type f -name '*.md' -not -path 'docs/plans/*' | sort)

while IFS=$'\t' read -r source target; do
    target="${target#<}"
    target="${target%>}"
    target="${target%%#*}"
    target="${target%%\?*}"

    [[ -z "${target}" ]] && continue
    case "${target}" in
        http://*|https://*|mailto:*|data:*|/*) continue ;;
    esac

    if [[ ! -e "$(dirname "${source}")/${target}" ]]; then
        fail "broken link in ${source}: ${target}"
    fi
done < <(
    perl -ne '
        while (/\[[^]]*\]\(([^)]+)\)/g) {
            print "$ARGV\t$1\n";
        }
    ' "${markdown_files[@]}"
)

if [[ "${failures}" -ne 0 ]]; then
    echo "docs audit failed with ${failures} problem(s)" >&2
    exit 1
fi

echo "Documentation audit passed: bilingual pages and local links are complete."
