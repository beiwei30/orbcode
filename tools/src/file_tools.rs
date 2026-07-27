use std::fmt::Write as _;
use std::path::Path;

use orbcode_protocol::{FileChangeSummary, PermissionSummary, ToolResultMetadata};
use serde::Serialize;
use serde_json::Value;

use crate::{
    ToolContext, ToolError, ToolOutcome, ToolRegistry,
    file_state::{FILE_MODIFIED_SINCE_READ_ERROR, FILE_UNEXPECTEDLY_MODIFIED_ERROR, mtime_ms},
    fs_text::{
        UTF8_BOM, convert_line_endings, create_parent_dir, detect_line_ending, has_utf8_bom,
        read_text_file, resolve_path, slice_lines, strip_utf8_bom, validate_file_edit_size,
        validate_file_read_size, validate_file_read_tokens, validate_text_file,
    },
    output::truncate_tool_output,
    payload::{
        bool_field_keys, exact_string_field_keys, parse_payload, required_field_keys, string_field,
        usize_field_keys,
    },
    permissions::require_tools,
};

const MAX_FILE_READ_OUTPUT_CHARS: usize = 100_000;
const MAX_DIFF_LINES: usize = 20;
const DIFF_CONTEXT_LINES: usize = 3;

/// Extension metadata fields appended to the base `ToolResultMetadata` for
/// file-read results.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FileReadExt {
    encoding: &'static str,
    has_bom: bool,
}

/// Extension metadata fields appended to the base `ToolResultMetadata` for
/// file-write results.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FileWriteExt {
    lines_written: usize,
}

/// Extension metadata fields appended to the base `ToolResultMetadata` for
/// file-edit results.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FileEditExt {
    diff: String,
    line_range: LineRange,
    lines_added: usize,
    lines_removed: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    diff_truncated: Option<bool>,
}

#[derive(Serialize)]
struct LineRange {
    start: usize,
    end: usize,
}

/// Merge the fields of a serializable extension struct into an existing
/// metadata JSON object.
fn merge_ext<T: Serialize>(metadata: &mut Value, ext: &T) {
    if let (Value::Object(base), Value::Object(extra)) =
        (metadata, serde_json::to_value(ext).unwrap_or_default())
    {
        base.extend(extra);
    }
}

struct DiffInfo {
    diff: String,
    line_range_start: usize,
    line_range_end: usize,
    lines_added: usize,
    lines_removed: usize,
    truncated: bool,
}

fn compute_edit_diff(
    old: &str,
    new: &str,
    find: &str,
    replace: &str,
    replace_all: bool,
) -> DiffInfo {
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();

    let occurrences: Vec<usize> = find_occurrence_start_lines(old, find);
    let find_line_count = find.lines().count();
    let replace_line_count = replace.lines().count();

    let total_lines_removed = occurrences.len() * find_line_count;
    let total_lines_added = occurrences.len() * replace_line_count;

    if replace_all && occurrences.len() > 2 {
        let first_range = occurrence_line_range(&old_lines, occurrences[0], find_line_count);

        let first_hunk = format_hunk(
            &old_lines,
            &new_lines,
            first_range.0,
            first_range.0 + find_line_count,
            first_range.0,
            first_range.0 + replace_line_count,
        );
        let last_hunk =
            format_hunk_for_last(old, new, find, replace, occurrences[occurrences.len() - 1]);

        let middle_count = occurrences.len() - 2;
        let mut diff = first_hunk;
        write!(diff, "\n...{middle_count} more replacement(s)...\n")
            .expect("writing to String cannot fail");
        diff.push_str(&last_hunk);

        let diff_lines: Vec<&str> = diff.lines().collect();
        let truncated = diff_lines.len() > MAX_DIFF_LINES;
        let final_diff = if truncated {
            diff_lines[..MAX_DIFF_LINES].join("\n")
        } else {
            diff
        };

        let global_start = occurrences[0] + 1;
        let last_occ = occurrences[occurrences.len() - 1];
        let global_end = last_occ + find_line_count;

        return DiffInfo {
            diff: final_diff,
            line_range_start: global_start,
            line_range_end: global_end,
            lines_added: total_lines_added,
            lines_removed: total_lines_removed,
            truncated,
        };
    }

    let first_change_line = occurrences.first().copied().unwrap_or(0);
    let last_occ = occurrences.last().copied().unwrap_or(first_change_line);
    let global_start = first_change_line + 1;
    let global_end = last_occ + find_line_count;

    let hunk = format_unified_diff(
        &old_lines,
        &new_lines,
        first_change_line,
        find_line_count,
        replace_line_count,
    );

    let diff_lines: Vec<&str> = hunk.lines().collect();
    let truncated = diff_lines.len() > MAX_DIFF_LINES;
    let final_diff = if truncated {
        diff_lines[..MAX_DIFF_LINES].join("\n")
    } else {
        hunk
    };

    DiffInfo {
        diff: final_diff,
        line_range_start: global_start,
        line_range_end: global_end,
        lines_added: total_lines_added,
        lines_removed: total_lines_removed,
        truncated,
    }
}

fn find_occurrence_start_lines(content: &str, find: &str) -> Vec<usize> {
    let mut results = Vec::new();
    let mut search_start = 0;
    while let Some(byte_pos) = content[search_start..].find(find) {
        let abs_pos = search_start + byte_pos;
        let line_num = content[..abs_pos].matches('\n').count();
        results.push(line_num);
        search_start = abs_pos + find.len();
    }
    results
}

fn occurrence_line_range(
    _old_lines: &[&str],
    start_line: usize,
    find_line_count: usize,
) -> (usize, usize) {
    (start_line, start_line + find_line_count)
}

fn format_unified_diff(
    old_lines: &[&str],
    new_lines: &[&str],
    change_start: usize,
    old_span: usize,
    new_span: usize,
) -> String {
    let ctx_before_start = change_start.saturating_sub(DIFF_CONTEXT_LINES);
    let ctx_after_end_old = (change_start + old_span + DIFF_CONTEXT_LINES).min(old_lines.len());
    let ctx_after_end_new = (change_start + new_span + DIFF_CONTEXT_LINES).min(new_lines.len());

    let mut out = String::new();
    let old_hunk_start = ctx_before_start + 1;
    let old_hunk_len = ctx_after_end_old - ctx_before_start;
    let new_hunk_start = ctx_before_start + 1;
    let new_hunk_len = ctx_after_end_new - ctx_before_start;

    writeln!(
        out,
        "@@ -{old_hunk_start},{old_hunk_len} +{new_hunk_start},{new_hunk_len} @@"
    )
    .expect("writing to String cannot fail");

    for i in ctx_before_start..change_start {
        if i < old_lines.len() {
            writeln!(out, " {}", old_lines[i]).expect("writing to String cannot fail");
        }
    }
    for i in change_start..change_start + old_span {
        if i < old_lines.len() {
            writeln!(out, "-{}", old_lines[i]).expect("writing to String cannot fail");
        }
    }
    for i in change_start..change_start + new_span {
        if i < new_lines.len() {
            writeln!(out, "+{}", new_lines[i]).expect("writing to String cannot fail");
        }
    }
    let after_old = change_start + old_span;
    let after_new = change_start + new_span;
    for i in 0..DIFF_CONTEXT_LINES {
        let old_idx = after_old + i;
        let new_idx = after_new + i;
        if old_idx < old_lines.len() {
            writeln!(out, " {}", old_lines[old_idx]).expect("writing to String cannot fail");
        } else if new_idx < new_lines.len() {
            writeln!(out, " {}", new_lines[new_idx]).expect("writing to String cannot fail");
        }
    }

    if out.ends_with('\n') {
        out.truncate(out.len() - 1);
    }
    out
}

fn format_hunk(
    old_lines: &[&str],
    new_lines: &[&str],
    old_start: usize,
    old_end: usize,
    new_start: usize,
    new_end: usize,
) -> String {
    let old_span = old_end - old_start;
    let new_span = new_end - new_start;
    format_unified_diff(old_lines, new_lines, old_start, old_span, new_span)
}

fn format_hunk_for_last(
    old: &str,
    new: &str,
    find: &str,
    replace: &str,
    last_occ_line: usize,
) -> String {
    let old_lines: Vec<&str> = old.lines().collect();
    let new_content = new;
    let new_lines: Vec<&str> = new_content.lines().collect();

    let find_line_count = find.lines().count();
    let replace_line_count = replace.lines().count();

    let occurrences = find_occurrence_start_lines(new, replace);
    let new_change_start = occurrences.last().copied().unwrap_or(0);

    let ctx_before_start = last_occ_line.saturating_sub(DIFF_CONTEXT_LINES);
    let ctx_after_end_old =
        (last_occ_line + find_line_count + DIFF_CONTEXT_LINES).min(old_lines.len());
    let ctx_after_end_new =
        (new_change_start + replace_line_count + DIFF_CONTEXT_LINES).min(new_lines.len());

    let mut out = String::new();
    let old_hunk_start = ctx_before_start + 1;
    let old_hunk_len = ctx_after_end_old - ctx_before_start;
    let new_hunk_start = (new_change_start.saturating_sub(DIFF_CONTEXT_LINES)) + 1;
    let new_hunk_len = ctx_after_end_new - new_change_start.saturating_sub(DIFF_CONTEXT_LINES);

    writeln!(
        out,
        "@@ -{old_hunk_start},{old_hunk_len} +{new_hunk_start},{new_hunk_len} @@"
    )
    .expect("writing to String cannot fail");

    for i in ctx_before_start..last_occ_line {
        if i < old_lines.len() {
            writeln!(out, " {}", old_lines[i]).expect("writing to String cannot fail");
        }
    }
    for i in last_occ_line..last_occ_line + find_line_count {
        if i < old_lines.len() {
            writeln!(out, "-{}", old_lines[i]).expect("writing to String cannot fail");
        }
    }
    for i in new_change_start..new_change_start + replace_line_count {
        if i < new_lines.len() {
            writeln!(out, "+{}", new_lines[i]).expect("writing to String cannot fail");
        }
    }
    let after_old = last_occ_line + find_line_count;
    for i in 0..DIFF_CONTEXT_LINES {
        let idx = after_old + i;
        if idx < old_lines.len() {
            writeln!(out, " {}", old_lines[idx]).expect("writing to String cannot fail");
        }
    }

    if out.ends_with('\n') {
        out.truncate(out.len() - 1);
    }
    out
}

fn file_change_metadata(path: &Path, operation: &str, context: &ToolContext) -> Value {
    ToolResultMetadata {
        file_changes: Some(FileChangeSummary {
            paths: vec![path.display().to_string()],
            operation: Some(operation.to_string()),
            git: None,
        }),
        permissions: Some(PermissionSummary {
            tools_allowed: Some(context.allow_tools),
            network_allowed: Some(context.allow_network),
        }),
        ..Default::default()
    }
    .to_value()
}

impl ToolRegistry {
    pub(crate) async fn file_read(
        &self,
        input: &str,
        context: &ToolContext,
    ) -> Result<ToolOutcome, ToolError> {
        require_tools(context)?;
        let payload = parse_payload(input)?;
        let path = resolve_path(
            &context.cwd,
            &required_field_keys(&payload, &["file_path", "filePath", "path"], input)?,
        )?;
        let start_line = usize_field_keys(&payload, &["offset", "start_line"]);
        let limit = usize_field_keys(&payload, &["limit"]);
        validate_text_file(&path, "read").await?;
        if limit.is_none() {
            validate_file_read_size(&path).await?;
        }
        let contents = read_text_file(&path, "read").await?;
        let end_line = if let Some(limit) = limit {
            if limit == 0 {
                Some(0)
            } else {
                Some(start_line.unwrap_or(1).saturating_add(limit - 1))
            }
        } else {
            usize_field_keys(&payload, &["end_line"])
        };
        let rendered = slice_lines(&contents, start_line, end_line);
        validate_file_read_tokens(&rendered, &path)?;
        if let Some(read_state) = &context.read_state {
            let metadata = tokio::fs::metadata(&path).await?;
            // A whole-file read records no range so a later edit can use the
            // content-identity fallback; any offset/limit/end_line makes the
            // recorded read partial (end_line already folds in the limit).
            read_state
                .record_read(
                    &path,
                    mtime_ms(&metadata),
                    contents.clone(),
                    start_line,
                    end_line,
                )
                .await;
        }
        let mut metadata = file_change_metadata(&path, "read", context);
        merge_ext(
            &mut metadata,
            &FileReadExt {
                encoding: "utf-8",
                has_bom: has_utf8_bom(&contents),
            },
        );
        Ok(ToolOutcome {
            name: "file-read".to_string(),
            summary: format!("Read {}.", path.display()),
            output: truncate_tool_output(
                rendered,
                MAX_FILE_READ_OUTPUT_CHARS,
                "File output truncated for transcript safety. Re-run file-read with offset/limit to inspect the omitted portion.",
            ),
            metadata: Some(metadata),
            changed_paths: vec![path],
        })
    }

    pub(crate) async fn file_write(
        &self,
        input: &str,
        context: &ToolContext,
    ) -> Result<ToolOutcome, ToolError> {
        require_tools(context)?;
        let payload = parse_payload(input)?;
        let path = resolve_path(
            &context.cwd,
            &required_field_keys(&payload, &["file_path", "filePath", "path"], input)?,
        )?;
        let raw_content = string_field(&payload, "content")
            .or_else(|| payload.as_str().map(str::to_string))
            .ok_or_else(|| ToolError::InvalidInput("file-write requires `content`".into()))?;
        let existing = tokio::fs::read_to_string(&path).await.ok();
        // Only existing, previously read files can be stale; a brand-new file
        // (or one never read this session) is written unconditionally.
        if let Some(read_state) = &context.read_state
            && let Ok(metadata) = tokio::fs::metadata(&path).await
            && read_state.write_is_stale(&path, mtime_ms(&metadata)).await
        {
            return Err(ToolError::ExecutionFailed(
                FILE_MODIFIED_SINCE_READ_ERROR.to_string(),
            ));
        }
        let contents = if let Some(ref existing) = existing {
            let bom = has_utf8_bom(existing);
            let line_ending = detect_line_ending(existing);
            let has_final_newline = existing.ends_with('\n') || existing.ends_with('\r');
            let mut c = convert_line_endings(&raw_content, line_ending);
            if has_final_newline && !c.is_empty() && !c.ends_with('\n') && !c.ends_with('\r') {
                match line_ending {
                    crate::fs_text::LineEnding::CrLf => c.push_str("\r\n"),
                    crate::fs_text::LineEnding::Cr => c.push('\r'),
                    crate::fs_text::LineEnding::Lf => c.push('\n'),
                }
            }
            if bom { format!("{UTF8_BOM}{c}") } else { c }
        } else {
            let mut c = raw_content;
            if !c.is_empty() && !c.ends_with('\n') {
                c.push('\n');
            }
            c
        };
        create_parent_dir(&path).await?;
        let lines_written = contents.lines().count();
        tokio::fs::write(&path, &contents).await?;
        if let Some(read_state) = &context.read_state
            && let Ok(metadata) = tokio::fs::metadata(&path).await
        {
            read_state
                .record_write(&path, mtime_ms(&metadata), contents)
                .await;
        }
        let mut metadata = file_change_metadata(&path, "write", context);
        merge_ext(&mut metadata, &FileWriteExt { lines_written });
        Ok(ToolOutcome {
            name: "file-write".to_string(),
            summary: format!("Wrote {}.", path.display()),
            output: format!("updated {}", path.display()),
            metadata: Some(metadata),
            changed_paths: vec![path],
        })
    }

    pub(crate) async fn file_edit(
        &self,
        input: &str,
        context: &ToolContext,
    ) -> Result<ToolOutcome, ToolError> {
        require_tools(context)?;
        let payload = parse_payload(input)?;
        let path = resolve_path(
            &context.cwd,
            &required_field_keys(&payload, &["file_path", "filePath", "path"], input)?,
        )?;
        let find = exact_string_field_keys(&payload, &["old_string", "find"])
            .filter(|value| !value.is_empty())
            .ok_or_else(|| ToolError::InvalidInput("file-edit requires `old_string`".into()))?;
        let replace = exact_string_field_keys(&payload, &["new_string", "replace"])
            .ok_or_else(|| ToolError::InvalidInput("file-edit requires `new_string`".into()))?;
        let replace_all = bool_field_keys(&payload, &["replace_all", "all"]).unwrap_or(false);
        validate_file_edit_size(&path).await?;
        validate_text_file(&path, "edit").await?;
        let raw_contents = read_text_file(&path, "edit").await?;
        if let Some(read_state) = &context.read_state {
            let metadata = tokio::fs::metadata(&path).await?;
            if read_state
                .edit_is_stale(&path, mtime_ms(&metadata), &raw_contents)
                .await
            {
                return Err(ToolError::ExecutionFailed(
                    FILE_UNEXPECTEDLY_MODIFIED_ERROR.to_string(),
                ));
            }
        }
        let bom = has_utf8_bom(&raw_contents);
        let contents = if bom {
            strip_utf8_bom(&raw_contents)
        } else {
            &raw_contents
        };
        let line_ending = detect_line_ending(contents);
        let find = convert_line_endings(&find, line_ending);
        let replace = convert_line_endings(&replace, line_ending);
        if !contents.contains(find.as_str()) {
            return Err(ToolError::ExecutionFailed(format!(
                "`{find}` was not found in {}",
                path.display()
            )));
        }
        let matches = contents.matches(find.as_str()).count();
        if matches > 1 && !replace_all {
            return Err(ToolError::ExecutionFailed(format!(
                "Found {matches} matches of the string to replace, but replace_all is false. To replace all occurrences, set replace_all to true. To replace only one occurrence, provide more context to uniquely identify the instance.\nString: {find}"
            )));
        }
        let replaced = if replace_all {
            contents.replace(find.as_str(), replace.as_str())
        } else {
            contents.replacen(find.as_str(), replace.as_str(), 1)
        };
        let diff_info = compute_edit_diff(contents, &replaced, &find, &replace, replace_all);
        let updated = if bom {
            format!("{UTF8_BOM}{replaced}")
        } else {
            replaced
        };
        tokio::fs::write(&path, &updated).await?;
        if let Some(read_state) = &context.read_state
            && let Ok(metadata) = tokio::fs::metadata(&path).await
        {
            read_state
                .record_write(&path, mtime_ms(&metadata), updated)
                .await;
        }
        let mut metadata = file_change_metadata(&path, "edit", context);
        merge_ext(
            &mut metadata,
            &FileEditExt {
                diff: diff_info.diff,
                line_range: LineRange {
                    start: diff_info.line_range_start,
                    end: diff_info.line_range_end,
                },
                lines_added: diff_info.lines_added,
                lines_removed: diff_info.lines_removed,
                diff_truncated: if diff_info.truncated {
                    Some(true)
                } else {
                    None
                },
            },
        );
        Ok(ToolOutcome {
            name: "file-edit".to_string(),
            summary: format!("Edited {}.", path.display()),
            output: format!(
                "replaced {} occurrence(s) in {}",
                if replace_all { "all" } else { "the first" },
                path.display()
            ),
            metadata: Some(metadata),
            changed_paths: vec![path],
        })
    }
}
