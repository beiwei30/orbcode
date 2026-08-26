#!/usr/bin/env python3
"""Guard the narrow Rust-maintenance boundaries closed by Slice 8.

This is deliberately not a global Clippy snapshot. It checks only boundaries
whose current exceptions have been semantically reviewed:

* string-typed ``permission_mode`` declarations;
* raw ``Option<Option<T>>`` spellings; and
* production ``tokio::spawn`` owner/count anchors.

The spawn allow-list records the owning function rather than a source line, so
unrelated edits do not move the baseline. A new spawn, a removed spawn, or a
move to another owner requires an explicit disposition update.
"""

from __future__ import annotations

import argparse
import bisect
import collections
from dataclasses import dataclass
from pathlib import Path
import re
import sys
import unittest


PERMISSION_STRING_RE = re.compile(
    r"\bpermission_mode\s*:\s*(?:Option\s*<\s*String\s*>|String)"
)
NESTED_OPTION_RE = re.compile(r"\bOption\s*<\s*Option\s*<")
SPAWN_RE = re.compile(r"\btokio\s*::\s*spawn\s*\(")
FUNCTION_RE = re.compile(r"\bfn\s+([A-Za-z_][A-Za-z0-9_]*)\s*(?:<[^>{};]*>)?\s*\(")
RAW_STRING_RE = re.compile(r"(?:br|rb|cr|rc|r)(?P<hashes>#{0,255})\"")
CHARACTER_RE = re.compile(r"'(?:\\(?:.|u\{[0-9A-Fa-f_]+\})|[^'\\\n])'")
CFG_TEST_RE = re.compile(r"#\s*\[\s*cfg\s*\(\s*test\s*\)\s*]")
FILE_CFG_TEST_RE = re.compile(r"#\s*!\[\s*cfg\s*\(\s*test\s*\)\s*]")
TEST_ATTRIBUTE_RE = re.compile(r"#\s*\[\s*(?:(?:tokio|async_std)\s*::\s*)?test\b")

SPAWN_ALLOW_LIST = "scripts/rust-maintenance-spawn-allow-list.txt"
SPAWN_DISPOSITION_RE = re.compile(r"(?:complete|deferred|best-effort):[a-z0-9-]+$")


def normalize_line(line: str) -> str:
    return " ".join(line.strip().split())


EXPECTED_PERMISSION_STRINGS = collections.Counter(
    {
        (
            "protocol/src/background_task_view.rs",
            "pub permission_mode: Option<String>,",
        ): 1,
        (
            "session-store/src/child_sessions.rs",
            "pub permission_mode: Option<String>,",
        ): 2,
        (
            "config/src/agents.rs",
            "let mut permission_mode: Option<String> = None;",
        ): 1,
        ("cli/src/stream_json.rs", "permission_mode: String,"): 1,
        ("protocol/src/control.rs", "pub permission_mode: String,"): 1,
    }
)


EXPECTED_NESTED_OPTIONS = collections.Counter(
    {
        (
            "app-server-protocol/src/contracts.rs",
            "pub token_budget: Option<Option<u64>>,",
        ): 1,
        (
            "app-server-protocol/src/contracts.rs",
            "fn deserialize_present_option<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>",
        ): 1,
        (
            "core/src/session_manager/session_goal.rs",
            "pub token_budget: Option<Option<u64>>,",
        ): 1,
        (
            "core/src/session_manager/session_goal.rs",
            "pub stop_reason: Option<Option<String>>,",
        ): 1,
        (
            "core/src/session_manager/session_goal.rs",
            "fn validated_budget(value: Option<Option<u64>>) -> Result<Option<u64>, GoalError> {",
        ): 1,
        (
            "tui/src/commands/goal.rs",
            "token_budget: Option<Option<u64>>,",
        ): 2,
        (
            "tui/src/commands/goal.rs",
            "fn budget_allows_resume(update: Option<Option<u64>>, goal: &SessionGoal) -> bool {",
        ): 1,
        (
            "tui/src/commands/goal.rs",
            "fn parse_objective_and_budget(args: &str) -> Result<(Option<Option<u64>>, String)> {",
        ): 1,
        (
            "session-store/src/transcript_schema.rs",
            "type PresentJsonValue = Option<Option<Value>>;",
        ): 1,
        (
            "tui/src/overlays/mod.rs",
            "pub(crate) type EffortOverrideSelection = Option<Option<EffortLevel>>;",
        ): 1,
    }
)


@dataclass(frozen=True)
class FunctionRange:
    name: str
    start: int
    end: int
    is_test: bool


@dataclass(frozen=True)
class SourceAnalysis:
    path: str
    source: str
    masked: str
    functions: tuple[FunctionRange, ...]
    excluded_ranges: tuple[tuple[int, int], ...]
    test_only: bool


def mask_non_code(source: str) -> str:
    """Replace comments and literals with spaces while preserving offsets."""

    masked = list(source)
    length = len(source)
    index = 0

    def blank(start: int, end: int) -> None:
        for offset in range(start, end):
            if masked[offset] != "\n":
                masked[offset] = " "

    while index < length:
        if source.startswith("//", index):
            end = source.find("\n", index + 2)
            end = length if end < 0 else end
            blank(index, end)
            index = end
            continue

        if source.startswith("/*", index):
            depth = 1
            end = index + 2
            while end < length and depth:
                if source.startswith("/*", end):
                    depth += 1
                    end += 2
                elif source.startswith("*/", end):
                    depth -= 1
                    end += 2
                else:
                    end += 1
            blank(index, end)
            index = end
            continue

        raw = RAW_STRING_RE.match(source, index)
        if raw is not None:
            delimiter = '"' + raw.group("hashes")
            content_start = raw.end()
            close = source.find(delimiter, content_start)
            end = length if close < 0 else close + len(delimiter)
            blank(index, end)
            index = end
            continue

        quote_index = index
        if source[index] in {"b", "c"} and index + 1 < length and source[index + 1] == '"':
            quote_index += 1
        if source[quote_index] == '"':
            end = quote_index + 1
            escaped = False
            while end < length:
                char = source[end]
                end += 1
                if char == '"' and not escaped:
                    break
                if char == "\\" and not escaped:
                    escaped = True
                else:
                    escaped = False
            blank(index, end)
            index = end
            continue

        if source[index] == "'":
            character = CHARACTER_RE.match(source, index)
            if character is not None:
                end = character.end()
                blank(index, end)
                index = end
                continue

        index += 1

    return "".join(masked)


def matching_braces(masked: str) -> dict[int, int]:
    stack: list[int] = []
    pairs: dict[int, int] = {}
    for index, char in enumerate(masked):
        if char == "{":
            stack.append(index)
        elif char == "}" and stack:
            opening = stack.pop()
            pairs[opening] = index
    return pairs


def next_body_open(masked: str, start: int) -> int | None:
    opening = masked.find("{", start)
    terminator = masked.find(";", start)
    if opening < 0 or (terminator >= 0 and terminator < opening):
        return None
    return opening


def cfg_test_ranges(masked: str, braces: dict[int, int]) -> list[tuple[int, int]]:
    ranges: list[tuple[int, int]] = []
    for match in CFG_TEST_RE.finditer(masked):
        opening = next_body_open(masked, match.end())
        if opening is not None and opening in braces:
            ranges.append((opening, braces[opening]))
    return ranges


def function_ranges(masked: str, braces: dict[int, int]) -> list[FunctionRange]:
    test_ranges = cfg_test_ranges(masked, braces)
    item_boundaries = [
        index for index, char in enumerate(masked) if char == "}" or char == ";"
    ]
    ranges: list[FunctionRange] = []
    for match in FUNCTION_RE.finditer(masked):
        opening = next_body_open(masked, match.end())
        if opening is None or opening not in braces:
            continue
        end = braces[opening]
        boundary_index = bisect.bisect_left(item_boundaries, match.start()) - 1
        boundary = item_boundaries[boundary_index] if boundary_index >= 0 else -1
        prefix = masked[boundary + 1 : match.start()]
        is_test = bool(TEST_ATTRIBUTE_RE.search(prefix) or CFG_TEST_RE.search(prefix))
        is_test = is_test or any(start < opening < finish for start, finish in test_ranges)
        ranges.append(FunctionRange(match.group(1), opening, end, is_test))
    return ranges


def owner_for(position: int, functions: list[FunctionRange]) -> FunctionRange | None:
    owners = [function for function in functions if function.start < position < function.end]
    return min(owners, key=lambda function: function.end - function.start, default=None)


def is_test_path(path: Path) -> bool:
    parts = path.parts
    return "tests" in parts or path.name.startswith("test_") or path.name.endswith("_test.rs")


def analyze_source(relative_path: Path, source: str) -> SourceAnalysis:
    path = relative_path.as_posix()
    if is_test_path(relative_path):
        return SourceAnalysis(path, source, "", (), (), True)
    masked = mask_non_code(source)
    if FILE_CFG_TEST_RE.search(masked):
        return SourceAnalysis(path, source, masked, (), (), True)
    braces = matching_braces(masked)
    functions = tuple(function_ranges(masked, braces))
    excluded_ranges = cfg_test_ranges(masked, braces)
    excluded_ranges.extend(
        (function.start, function.end) for function in functions if function.is_test
    )
    return SourceAnalysis(
        path,
        source,
        masked,
        functions,
        tuple(excluded_ranges),
        False,
    )


def production_spawns_in(analysis: SourceAnalysis) -> collections.Counter[tuple[str, str]]:
    owners: collections.Counter[tuple[str, str]] = collections.Counter()
    if analysis.test_only:
        return owners
    for match in SPAWN_RE.finditer(analysis.masked):
        owner = owner_for(match.start(), list(analysis.functions))
        if owner is not None and owner.is_test:
            continue
        owners[(analysis.path, owner.name if owner is not None else "<module>")] += 1
    return owners


def production_spawn_owners(relative_path: Path, source: str) -> collections.Counter[tuple[str, str]]:
    return production_spawns_in(analyze_source(relative_path, source))


def rust_sources(root: Path) -> list[Path]:
    return sorted(path for path in root.glob("*/src/**/*.rs") if path.is_file())


def workspace_analyses(root: Path) -> list[SourceAnalysis]:
    return [
        analyze_source(path.relative_to(root), path.read_text(encoding="utf-8"))
        for path in rust_sources(root)
    ]


def line_matches_in(
    analyses: list[SourceAnalysis], pattern: re.Pattern[str]
) -> collections.Counter[tuple[str, str]]:
    matches: collections.Counter[tuple[str, str]] = collections.Counter()
    for analysis in analyses:
        if analysis.test_only:
            continue
        for match in pattern.finditer(analysis.masked):
            if any(
                start < match.start() < end for start, end in analysis.excluded_ranges
            ):
                continue
            line_start = analysis.source.rfind("\n", 0, match.start()) + 1
            line_end = analysis.source.find("\n", match.end())
            line_end = len(analysis.source) if line_end < 0 else line_end
            matches[
                (analysis.path, normalize_line(analysis.source[line_start:line_end]))
            ] += 1
    return matches


def line_matches(root: Path, pattern: re.Pattern[str]) -> collections.Counter[tuple[str, str]]:
    return line_matches_in(workspace_analyses(root), pattern)


def production_spawns_in_workspace(
    analyses: list[SourceAnalysis],
) -> collections.Counter[tuple[str, str]]:
    owners: collections.Counter[tuple[str, str]] = collections.Counter()
    for analysis in analyses:
        owners.update(production_spawns_in(analysis))
    return owners


def all_production_spawns(root: Path) -> collections.Counter[tuple[str, str]]:
    return production_spawns_in_workspace(workspace_analyses(root))


def parse_spawn_allow_list(path: Path) -> dict[tuple[str, str], tuple[int, str]]:
    expected: dict[tuple[str, str], tuple[int, str]] = {}
    for line_number, raw_line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        if not raw_line or raw_line.startswith("#"):
            continue
        fields = raw_line.split("\t")
        if len(fields) != 4:
            raise ValueError(f"{path}:{line_number}: expected four tab-separated fields")
        file_name, owner, raw_count, disposition = fields
        try:
            count = int(raw_count)
        except ValueError as error:
            raise ValueError(f"{path}:{line_number}: invalid count {raw_count!r}") from error
        if count < 1 or SPAWN_DISPOSITION_RE.fullmatch(disposition) is None:
            raise ValueError(
                f"{path}:{line_number}: count must be positive and disposition must use "
                "complete:, deferred:, or best-effort:"
            )
        key = (file_name, owner)
        if key in expected:
            raise ValueError(f"{path}:{line_number}: duplicate owner {file_name}:{owner}")
        expected[key] = (count, disposition)
    return expected


def report_counter_difference(
    label: str,
    expected: collections.Counter[tuple[str, str]],
    actual: collections.Counter[tuple[str, str]],
) -> bool:
    if actual == expected:
        print(f"✓ {label}: {sum(actual.values())} reviewed occurrence(s)")
        return True
    print(f"error: {label} changed; classify the boundary before updating the audit:", file=sys.stderr)
    for key in sorted(set(expected) | set(actual)):
        if expected[key] != actual[key]:
            print(
                f"  {key[0]}: {key[1]} (expected {expected[key]}, actual {actual[key]})",
                file=sys.stderr,
            )
    return False


def audit_spawns(
    root: Path, actual: collections.Counter[tuple[str, str]] | None = None
) -> bool:
    allow_path = root / SPAWN_ALLOW_LIST
    if not allow_path.is_file():
        print(f"error: spawn allow-list not found: {allow_path}", file=sys.stderr)
        return False
    try:
        expected = parse_spawn_allow_list(allow_path)
    except ValueError as error:
        print(f"error: {error}", file=sys.stderr)
        return False
    actual = actual if actual is not None else all_production_spawns(root)
    expected_counts = collections.Counter({key: value[0] for key, value in expected.items()})
    if actual != expected_counts:
        print(
            "error: production tokio::spawn owners changed; add a lifecycle disposition "
            "before updating the allow-list:",
            file=sys.stderr,
        )
        for key in sorted(set(expected_counts) | set(actual)):
            if expected_counts[key] != actual[key]:
                print(
                    f"  {key[0]}::{key[1]} (expected {expected_counts[key]}, actual {actual[key]})",
                    file=sys.stderr,
                )
        return False
    print(
        f"✓ production tokio::spawn owners: {sum(actual.values())} spawn(s) "
        f"across {len(actual)} reviewed owner anchor(s)"
    )
    return True


class ScannerTests(unittest.TestCase):
    def test_boundary_scanner_excludes_test_only_declarations(self) -> None:
        source = """
struct Production {
    permission_mode: Option<String>,
    update: Option<Option<u8>>,
}

#[cfg(test)]
mod tests {
    struct Fixture {
        permission_mode: Option<String>,
        update: Option<Option<u8>>,
    }
}
"""
        analysis = analyze_source(Path("crate/src/lib.rs"), source)
        self.assertEqual(
            line_matches_in([analysis], PERMISSION_STRING_RE),
            collections.Counter(
                {("crate/src/lib.rs", "permission_mode: Option<String>,"): 1}
            ),
        )
        self.assertEqual(
            line_matches_in([analysis], NESTED_OPTION_RE),
            collections.Counter(
                {("crate/src/lib.rs", "update: Option<Option<u8>>,"): 1}
            ),
        )

    def test_spawn_scanner_ignores_tests_comments_and_literals(self) -> None:
        source = r'''
async fn production() {
    tokio::spawn(async move {});
    let _literal = r#"tokio::spawn(async move { fake() })"#;
    // tokio::spawn(async move {});
    /* tokio::spawn(async move {}); */
}

#[cfg(test)]
mod tests {
    async fn helper() {
        tokio::spawn(async move {});
    }
}

#[tokio::test]
async fn standalone_test() {
    tokio::spawn(async move {});
}

#[cfg(not(test))]
mod production_build {
    async fn production_when_not_testing() {
        tokio::spawn(async move {});
    }
}

fn two_spawns() {
    tokio::spawn(async move {});
    tokio::spawn(async move {});
}
'''
        self.assertEqual(
            production_spawn_owners(Path("crate/src/lib.rs"), source),
            collections.Counter(
                {
                    ("crate/src/lib.rs", "production"): 1,
                    ("crate/src/lib.rs", "production_when_not_testing"): 1,
                    ("crate/src/lib.rs", "two_spawns"): 2,
                }
            ),
        )

    def test_dedicated_test_paths_are_excluded(self) -> None:
        source = "fn helper() { tokio::spawn(async move {}); }"
        self.assertFalse(production_spawn_owners(Path("crate/src/tests/helper.rs"), source))

    def test_mask_preserves_offsets_and_braces(self) -> None:
        source = 'fn f() { let _ = "}"; /* { nested /* } */ } */ call(); }'
        masked = mask_non_code(source)
        self.assertEqual(len(source), len(masked))
        braces = matching_braces(masked)
        opening = masked.index("{")
        self.assertEqual(source[braces[opening]], "}")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--repo",
        type=Path,
        help="workspace root (defaults to the parent of this script directory)",
    )
    parser.add_argument(
        "--list-spawns",
        action="store_true",
        help="print detected production spawn owner/count anchors",
    )
    parser.add_argument(
        "--self-test",
        action="store_true",
        help="run focused scanner tests",
    )
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    if args.self_test:
        suite = unittest.defaultTestLoader.loadTestsFromTestCase(ScannerTests)
        result = unittest.TextTestRunner(verbosity=2).run(suite)
        raise SystemExit(0 if result.wasSuccessful() else 1)

    root = (args.repo or Path(__file__).resolve().parent.parent).resolve()
    if not (root / "Cargo.toml").is_file():
        raise SystemExit(f"error: no Cargo.toml at workspace root: {root}")

    if args.list_spawns:
        analyses = workspace_analyses(root)
        for (path, owner), count in sorted(
            production_spawns_in_workspace(analyses).items()
        ):
            print(f"{path}\t{owner}\t{count}\tUNCLASSIFIED")
        return

    analyses = workspace_analyses(root)
    spawns = production_spawns_in_workspace(analyses)
    checks = [
        report_counter_difference(
            "string-typed permission_mode boundaries",
            EXPECTED_PERMISSION_STRINGS,
            line_matches_in(analyses, PERMISSION_STRING_RE),
        ),
        report_counter_difference(
            "raw nested-option boundaries",
            EXPECTED_NESTED_OPTIONS,
            line_matches_in(analyses, NESTED_OPTION_RE),
        ),
        audit_spawns(root, spawns),
    ]
    if not all(checks):
        raise SystemExit(1)
    print("Rust maintenance boundary audit passed.")


if __name__ == "__main__":
    main()
