#!/usr/bin/env python3
"""Produce the reproducible Clippy inventory used by maintenance Slice 0.

The report is written to stdout. Cargo/Clippy diagnostics are captured in
memory; this script never rewrites source files or checked-in snapshots.
"""

from __future__ import annotations

import argparse
import collections
import json
from pathlib import Path
import subprocess
import sys
from typing import Any, Iterable


SELECTED_LINTS = frozenset(
    {
        "clippy::cast_lossless",
        "clippy::cast_possible_truncation",
        "clippy::cast_possible_wrap",
        "clippy::cast_precision_loss",
        "clippy::cast_sign_loss",
        "clippy::doc_markdown",
        "clippy::manual_let_else",
        "clippy::map_unwrap_or",
        "clippy::match_same_arms",
        "clippy::missing_errors_doc",
        "clippy::must_use_candidate",
        "clippy::needless_pass_by_value",
        "clippy::too_many_lines",
        "clippy::unwrap_used",
    }
)

CLASSIFICATION_ORDER = {
    "production-library": 0,
    "production-other-target": 1,
    "test": 2,
    "compatibility": 3,
    "fixture": 4,
    "generated": 5,
    "dependency": 6,
}

REPORT_VERSION = 2


def run(command: list[str], *, cwd: Path, capture: bool = True) -> str:
    print(f"+ {' '.join(command)}", file=sys.stderr, flush=True)
    completed = subprocess.run(
        command,
        cwd=cwd,
        check=False,
        stdout=subprocess.PIPE if capture else None,
        stderr=None,
        text=True,
    )
    if completed.returncode != 0:
        raise SystemExit(completed.returncode)
    return completed.stdout if completed.stdout is not None else ""


def tool_output(command: list[str], *, cwd: Path) -> str:
    return run(command, cwd=cwd).strip().replace("\n", " | ")


def workspace_packages(root: Path) -> dict[str, str]:
    metadata = json.loads(
        run(
            ["cargo", "metadata", "--format-version=1", "--no-deps"],
            cwd=root,
        )
    )
    return {package["id"]: package["name"] for package in metadata["packages"]}


def clippy_messages(root: Path, arguments: list[str]) -> list[dict[str, Any]]:
    command = ["cargo", "clippy", *arguments, "--message-format=json"]
    command.extend(["--", "-W", "clippy::pedantic", "-W", "clippy::unwrap_used"])
    output = run(command, cwd=root)
    messages: list[dict[str, Any]] = []
    for line in output.splitlines():
        try:
            message = json.loads(line)
        except json.JSONDecodeError:
            continue
        if message.get("reason") == "compiler-message":
            messages.append(message)
    return messages


def normalized_path(raw_path: str, root: Path) -> tuple[str, bool]:
    path = Path(raw_path)
    if not path.is_absolute():
        path = root / path
    try:
        return path.resolve().relative_to(root).as_posix(), True
    except ValueError:
        return path.as_posix(), False


def primary_span(message: dict[str, Any]) -> dict[str, Any] | None:
    spans = message["message"].get("spans", [])
    return next((span for span in spans if span.get("is_primary")), None)


def diagnostic_rows(
    messages: Iterable[dict[str, Any]],
    *,
    root: Path,
    packages: dict[str, str],
    selected_only: bool,
) -> list[dict[str, Any]]:
    rows: list[dict[str, Any]] = []
    for envelope in messages:
        diagnostic = envelope["message"]
        code = (diagnostic.get("code") or {}).get("code")
        if diagnostic.get("level") != "warning" or code is None:
            continue
        if selected_only and code not in SELECTED_LINTS:
            continue
        span = primary_span(envelope)
        if span is None:
            continue
        path, in_workspace = normalized_path(span["file_name"], root)
        target = envelope.get("target", {})
        rows.append(
            {
                "lint": code,
                "crate": packages.get(envelope.get("package_id"), "<dependency>"),
                "target": target.get("name", "<unknown>"),
                "path": path,
                "in_workspace": in_workspace,
                "line": int(span["line_start"]),
                "column": int(span["column_start"]),
            }
        )
    return rows


def location_key(row: dict[str, Any]) -> tuple[Any, ...]:
    return (
        row["lint"],
        row["crate"],
        row["path"],
        row["line"],
        row["column"],
    )


def classify(
    row: dict[str, Any],
    production_library_keys: set[tuple[Any, ...]],
    production_other_keys: set[tuple[Any, ...]],
) -> str:
    path = row["path"]
    parts = Path(path).parts
    if not row["in_workspace"]:
        return "dependency"
    if "target" in parts or any(part in {"generated", "gen"} for part in parts):
        return "generated"
    if "fixtures" in parts or "fixture" in parts:
        return "fixture"
    if parts and parts[0] == "compat-fixtures":
        return "compatibility"
    key = location_key(row)
    if key in production_library_keys:
        return "production-library"
    if key in production_other_keys:
        return "production-other-target"
    return "test"


def deduplicate(rows: Iterable[dict[str, Any]]) -> list[dict[str, Any]]:
    by_key: dict[tuple[Any, ...], dict[str, Any]] = {}
    for row in rows:
        key = location_key(row)
        existing = by_key.get(key)
        row_rank = (CLASSIFICATION_ORDER[row["classification"]], row["target"])
        existing_rank = None
        if existing is not None:
            existing_rank = (
                CLASSIFICATION_ORDER[existing["classification"]],
                existing["target"],
            )
        # A shared integration-test support file can be compiled into several
        # targets, whose Cargo JSON messages arrive in an unspecified order.
        # Prefer the lexicographically first target after classification so the
        # displayed target is stable without counting the source location twice.
        if existing_rank is None or row_rank < existing_rank:
            by_key[key] = row
    return list(by_key.values())


def count_table(rows: Iterable[dict[str, Any]], keys: tuple[str, ...]) -> list[tuple[tuple[str, ...], int]]:
    counts: collections.Counter[tuple[str, ...]] = collections.Counter(
        tuple(str(row[key]) for key in keys) for row in rows
    )
    return sorted(counts.items(), key=lambda item: (-item[1], item[0]))


def print_table(headers: list[str], values: Iterable[Iterable[Any]]) -> None:
    print("| " + " | ".join(headers) + " |")
    print("| " + " | ".join("---" for _ in headers) + " |")
    for value in values:
        print("| " + " | ".join(str(item) for item in value) + " |")


def emit_report(
    *,
    root: Path,
    production_library_rows: list[dict[str, Any]],
    production_other_rows: list[dict[str, Any]],
    all_rows: list[dict[str, Any]],
    production_library_selected_rows: list[dict[str, Any]],
    production_other_selected_rows: list[dict[str, Any]],
    all_selected_rows: list[dict[str, Any]],
) -> None:
    production_library_keys = {
        location_key(row) for row in production_library_rows
    }
    production_other_keys = {location_key(row) for row in production_other_rows}
    combined = []
    for row in all_rows:
        row = dict(row)
        row["classification"] = classify(
            row, production_library_keys, production_other_keys
        )
        combined.append(row)
    for row in production_library_rows + production_other_rows:
        row = dict(row)
        row["classification"] = classify(
            row, production_library_keys, production_other_keys
        )
        combined.append(row)
    combined = deduplicate(combined)
    selected_keys = {
        location_key(row)
        for row in (
            production_library_selected_rows
            + production_other_selected_rows
            + all_selected_rows
        )
    }
    combined.sort(
        key=lambda row: (
            CLASSIFICATION_ORDER[row["classification"]],
            row["lint"],
            row["crate"],
            row["path"],
            row["line"],
            row["column"],
        )
    )
    selected_combined = [row for row in combined if location_key(row) in selected_keys]
    production_clean = [
        row for row in combined if row["classification"] == "production-library"
    ]
    production_selected_clean = [
        row
        for row in selected_combined
        if row["classification"] == "production-library"
    ]
    production_other_clean = [
        row
        for row in combined
        if row["classification"] == "production-other-target"
    ]
    production_other_selected_clean = [
        row
        for row in selected_combined
        if row["classification"] == "production-other-target"
    ]

    print("# Rust maintenance lint inventory")
    print()
    print(f"- Commit: `{tool_output(['git', 'rev-parse', 'HEAD'], cwd=root)}`")
    print(
        "- origin/main: "
        f"`{tool_output(['git', 'rev-parse', 'origin/main'], cwd=root)}`"
    )
    status = tool_output(["git", "status", "--short", "--branch"], cwd=root)
    print(f"- Git status: `{status}`")
    script_blob = tool_output(
        ["git", "hash-object", "scripts/rust-maintenance-report.py"], cwd=root
    )
    print(f"- Report script: `v{REPORT_VERSION}` (Git blob `{script_blob}`)")
    print(f"- rustc: `{tool_output(['rustc', '--version', '--verbose'], cwd=root)}`")
    print(f"- cargo: `{tool_output(['cargo', '--version'], cwd=root)}`")
    print(f"- Clippy: `{tool_output(['cargo', 'clippy', '--version'], cwd=root)}`")
    print()
    print("Only the selected Slice 0 lint families are retained below. Counts are")
    print("deduplicated by lint, crate, path, line, and column.")

    print()
    print("## Scope totals")
    print()
    print_table(
        ["scope", "all warnings", "selected warnings", "unwrap_used"],
        [
            (
                "workspace-lib-command-raw",
                len(
                    deduplicate(
                        {**row, "classification": "production-library"}
                        for row in production_library_rows
                    )
                ),
                len(
                    deduplicate(
                        {**row, "classification": "production-library"}
                        for row in production_library_selected_rows
                    )
                ),
                sum(
                    row["lint"] == "clippy::unwrap_used"
                    for row in production_library_selected_rows
                ),
            ),
            (
                "classified-production-library",
                len(production_clean),
                len(production_selected_clean),
                sum(
                    row["lint"] == "clippy::unwrap_used"
                    for row in production_selected_clean
                ),
            ),
            (
                "workspace-bin-command-raw",
                len(
                    deduplicate(
                        {**row, "classification": "production-other-target"}
                        for row in production_other_rows
                    )
                ),
                len(
                    deduplicate(
                        {**row, "classification": "production-other-target"}
                        for row in production_other_selected_rows
                    )
                ),
                sum(
                    row["lint"] == "clippy::unwrap_used"
                    for row in production_other_selected_rows
                ),
            ),
            (
                "classified-production-other-target",
                len(production_other_clean),
                len(production_other_selected_clean),
                sum(
                    row["lint"] == "clippy::unwrap_used"
                    for row in production_other_selected_clean
                ),
            ),
            (
                "workspace-all-targets",
                len(combined),
                len(selected_combined),
                sum(row["lint"] == "clippy::unwrap_used" for row in selected_combined),
            ),
        ],
    )

    print()
    print("## Production library lint counts")
    print()
    print_table(
        ["lint", "count"],
        (
            (key[0].removeprefix("clippy::"), count)
            for key, count in count_table(production_selected_clean, ("lint",))
        ),
    )

    print()
    print("## All-target classification counts")
    print()
    print_table(
        ["classification", "all warnings", "selected warnings", "unwrap_used"],
        [
            (
                classification,
                sum(row["classification"] == classification for row in combined),
                sum(
                    row["classification"] == classification
                    for row in selected_combined
                ),
                sum(
                    row["classification"] == classification
                    and row["lint"] == "clippy::unwrap_used"
                    for row in selected_combined
                ),
            )
            for classification in CLASSIFICATION_ORDER
        ],
    )

    print()
    print("## Classification and lint counts")
    print()
    print_table(
        ["classification", "lint", "count"],
        (
            (key[0], key[1].removeprefix("clippy::"), count)
            for key, count in count_table(selected_combined, ("classification", "lint"))
        ),
    )

    print()
    print("## Classification and crate counts")
    print()
    print_table(
        ["classification", "crate", "count"],
        (
            (key[0], key[1], count)
            for key, count in count_table(
                selected_combined, ("classification", "crate")
            )
        ),
    )

    print()
    print("## Selected lint locations")
    print()
    print_table(
        ["classification", "lint", "crate", "target", "location"],
        (
            (
                row["classification"],
                row["lint"].removeprefix("clippy::"),
                row["crate"],
                row["target"],
                f"{row['path']}:{row['line']}:{row['column']}",
            )
            for row in selected_combined
        ),
    )


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--repo",
        type=Path,
        help="workspace root (defaults to the parent of this script directory)",
    )
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    root = (args.repo or Path(__file__).resolve().parent.parent).resolve()
    if not (root / "Cargo.toml").is_file():
        raise SystemExit(f"error: no Cargo.toml at workspace root: {root}")

    packages = workspace_packages(root)
    production_library_messages = clippy_messages(
        root, ["--workspace", "--lib", "--no-deps"]
    )
    production_other_messages = clippy_messages(
        root, ["--workspace", "--bins", "--no-deps"]
    )
    all_messages = clippy_messages(root, ["--workspace", "--all-targets"])
    production_library_rows = diagnostic_rows(
        production_library_messages,
        root=root,
        packages=packages,
        selected_only=False,
    )
    production_other_rows = diagnostic_rows(
        production_other_messages,
        root=root,
        packages=packages,
        selected_only=False,
    )
    all_rows = diagnostic_rows(
        all_messages, root=root, packages=packages, selected_only=False
    )
    production_library_selected_rows = [
        row for row in production_library_rows if row["lint"] in SELECTED_LINTS
    ]
    production_other_selected_rows = [
        row for row in production_other_rows if row["lint"] in SELECTED_LINTS
    ]
    all_selected_rows = [row for row in all_rows if row["lint"] in SELECTED_LINTS]
    emit_report(
        root=root,
        production_library_rows=production_library_rows,
        production_other_rows=production_other_rows,
        all_rows=all_rows,
        production_library_selected_rows=production_library_selected_rows,
        production_other_selected_rows=production_other_selected_rows,
        all_selected_rows=all_selected_rows,
    )


if __name__ == "__main__":
    main()
