use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::json;

use crate::fs_text::resolve_path;
use crate::payload::{parse_payload, string_field, string_field_any, usize_field_keys};
use crate::{
    ToolContext, ToolError, ToolOutcome, ToolRegistry,
    permissions::{ensure_not_cancelled, require_tools},
};

impl ToolRegistry {
    pub(crate) async fn lsp(
        &self,
        input: &str,
        context: &ToolContext,
    ) -> Result<ToolOutcome, ToolError> {
        require_tools(context)?;
        let payload = parse_payload(input)?;
        let operation = string_field(&payload, "operation")
            .ok_or_else(|| ToolError::InvalidInput("lsp requires `operation`".into()))?;
        let file_path_raw = string_field_any(&payload, &["filePath", "file_path"])
            .ok_or_else(|| ToolError::InvalidInput("lsp requires `filePath`".into()))?;
        let line = usize_field_keys(&payload, &["line"]).unwrap_or(1);
        let character = usize_field_keys(&payload, &["character"]).unwrap_or(1);
        let file_path = resolve_path(&context.cwd, &file_path_raw)?;
        let file_contents = tokio::fs::read_to_string(&file_path)
            .await
            .map_err(|error| {
                ToolError::InvalidInput(format!(
                    "unable to read `{}`: {error}",
                    file_path.display()
                ))
            })?;
        let symbol =
            extract_symbol_at_position(&file_contents, line, character).ok_or_else(|| {
                ToolError::InvalidInput(format!(
                    "could not resolve a symbol at {}:{} in {}",
                    line,
                    character,
                    file_path.display()
                ))
            })?;

        let (result, result_count, file_count) = execute_lsp_operation(
            &operation,
            &file_path,
            &file_contents,
            line,
            &symbol,
            context,
        )
        .await?;
        Ok(ToolOutcome {
            name: "lsp".to_string(),
            summary: format!("Completed LSP operation `{operation}` for `{symbol}`."),
            output: serde_json::to_string_pretty(&json!({
                "operation": operation,
                "result": result,
                "filePath": file_path.display().to_string(),
                "resultCount": result_count,
                "fileCount": file_count,
                "symbol": symbol,
                "mode": "heuristic",
            }))?,
            metadata: None,
            changed_paths: Vec::new(),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SearchHit {
    path: PathBuf,
    line_number: usize,
    line_text: String,
}

pub(crate) async fn execute_lsp_operation(
    operation: &str,
    file_path: &Path,
    file_contents: &str,
    line: usize,
    symbol: &str,
    context: &ToolContext,
) -> Result<(String, usize, usize), ToolError> {
    match operation {
        "documentSymbol" => {
            let symbols = collect_document_symbols(file_path, file_contents);
            let file_count = usize::from(!symbols.is_empty());
            Ok((format_search_hits(&symbols), symbols.len(), file_count))
        }
        "workspaceSymbol" => {
            let hits = search_workspace_hits(&context.cwd, symbol, 100, context, |text| {
                likely_definition_line(text, symbol)
            })
            .await?;
            let file_count = hits
                .iter()
                .map(|hit| hit.path.clone())
                .collect::<BTreeSet<_>>()
                .len();
            Ok((format_search_hits(&hits), hits.len(), file_count))
        }
        "goToDefinition" | "goToImplementation" | "prepareCallHierarchy" => {
            let hits = search_workspace_hits(&context.cwd, symbol, 100, context, |text| {
                likely_definition_line(text, symbol)
            })
            .await?;
            let fallback = if hits.is_empty() {
                search_workspace_hits(&context.cwd, symbol, 25, context, |_| true).await?
            } else {
                Vec::new()
            };
            let result_hits = if hits.is_empty() { fallback } else { hits };
            let file_count = result_hits
                .iter()
                .map(|hit| hit.path.clone())
                .collect::<BTreeSet<_>>()
                .len();
            Ok((
                format_search_hits(&result_hits),
                result_hits.len(),
                file_count,
            ))
        }
        "findReferences" | "incomingCalls" => {
            let hits = search_workspace_hits(&context.cwd, symbol, 200, context, |_| true).await?;
            let file_count = hits
                .iter()
                .map(|hit| hit.path.clone())
                .collect::<BTreeSet<_>>()
                .len();
            Ok((format_search_hits(&hits), hits.len(), file_count))
        }
        "hover" => {
            let summary = build_hover_summary(file_path, file_contents, line, symbol);
            Ok((summary, 1, 1))
        }
        "outgoingCalls" => {
            let outgoing = collect_outgoing_calls(file_contents, line);
            let result = if outgoing.is_empty() {
                "No outgoing calls detected in the local block.".to_string()
            } else {
                outgoing.join("\n")
            };
            Ok((result, outgoing.len(), usize::from(!outgoing.is_empty())))
        }
        _ => Err(ToolError::InvalidInput(format!(
            "unsupported lsp operation `{operation}`"
        ))),
    }
}

fn format_search_hits(hits: &[SearchHit]) -> String {
    if hits.is_empty() {
        "No matching symbols found.".to_string()
    } else {
        hits.iter()
            .map(|hit| {
                format!(
                    "{}:{}: {}",
                    hit.path.display(),
                    hit.line_number,
                    hit.line_text.trim()
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn build_hover_summary(file_path: &Path, contents: &str, line: usize, symbol: &str) -> String {
    let lines = contents.lines().collect::<Vec<_>>();
    let line_index = line.saturating_sub(1).min(lines.len().saturating_sub(1));
    let mut doc_lines = Vec::new();
    let mut cursor = line_index;
    while cursor > 0 {
        let candidate = lines[cursor - 1].trim();
        if candidate.starts_with("///")
            || candidate.starts_with("//")
            || candidate.starts_with('#')
            || candidate.starts_with('*')
        {
            doc_lines.push(
                candidate
                    .trim_start_matches('/')
                    .trim_start_matches('*')
                    .trim()
                    .to_string(),
            );
            cursor -= 1;
        } else {
            break;
        }
    }
    doc_lines.reverse();
    let symbol_line = lines
        .get(line_index)
        .copied()
        .unwrap_or_default()
        .trim()
        .to_string();
    if doc_lines.is_empty() {
        format!(
            "Symbol: {symbol}\nLocation: {}:{}\nLine: {}",
            file_path.display(),
            line,
            symbol_line
        )
    } else {
        format!(
            "Symbol: {symbol}\nLocation: {}:{}\nDocumentation:\n{}\n\nLine: {}",
            file_path.display(),
            line,
            doc_lines.join("\n"),
            symbol_line
        )
    }
}

fn collect_document_symbols(file_path: &Path, contents: &str) -> Vec<SearchHit> {
    contents
        .lines()
        .enumerate()
        .filter_map(|(index, line)| {
            let trimmed = line.trim_start();
            let markers = [
                "fn ",
                "pub fn ",
                "async fn ",
                "pub async fn ",
                "struct ",
                "pub struct ",
                "enum ",
                "pub enum ",
                "trait ",
                "pub trait ",
                "impl ",
                "class ",
                "interface ",
                "function ",
                "type ",
                "const ",
                "let ",
            ];
            if markers.iter().any(|marker| trimmed.starts_with(marker)) {
                Some(SearchHit {
                    path: file_path.to_path_buf(),
                    line_number: index + 1,
                    line_text: line.to_string(),
                })
            } else {
                None
            }
        })
        .collect()
}

async fn search_workspace_hits<F>(
    root: &Path,
    symbol: &str,
    limit: usize,
    context: &ToolContext,
    predicate: F,
) -> Result<Vec<SearchHit>, ToolError>
where
    F: Fn(&str) -> bool,
{
    let mut files = collect_workspace_code_files(root, context).await?;
    files.sort();
    let mut hits = Vec::new();
    for path in files {
        ensure_not_cancelled(context)?;
        let contents = match tokio::fs::read_to_string(&path).await {
            Ok(value) => value,
            Err(_) => continue,
        };
        for (index, line) in contents.lines().enumerate() {
            if line.contains(symbol) && predicate(line) {
                hits.push(SearchHit {
                    path: path.clone(),
                    line_number: index + 1,
                    line_text: line.to_string(),
                });
                if hits.len() >= limit {
                    return Ok(hits);
                }
            }
        }
    }
    Ok(hits)
}

async fn collect_workspace_code_files(
    root: &Path,
    context: &ToolContext,
) -> Result<Vec<PathBuf>, ToolError> {
    let mut stack = vec![root.to_path_buf()];
    let mut files = Vec::new();
    while let Some(dir) = stack.pop() {
        ensure_not_cancelled(context)?;
        let mut entries = match tokio::fs::read_dir(&dir).await {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        while let Some(entry) = entries.next_entry().await? {
            ensure_not_cancelled(context)?;
            let path = entry.path();
            let file_name = path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("");
            if entry.file_type().await?.is_dir() {
                if should_skip_workspace_dir(file_name) {
                    continue;
                }
                stack.push(path);
            } else if is_probable_code_file(&path) {
                files.push(path);
            }
        }
    }
    Ok(files)
}

fn should_skip_workspace_dir(name: &str) -> bool {
    matches!(
        name,
        ".git" | "node_modules" | "target" | "dist" | "build" | ".next" | ".turbo"
    )
}

fn is_probable_code_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|value| value.to_str()),
        Some(
            "rs" | "ts"
                | "tsx"
                | "js"
                | "jsx"
                | "go"
                | "py"
                | "java"
                | "c"
                | "cc"
                | "cpp"
                | "h"
                | "hpp"
                | "swift"
                | "kt"
                | "m"
                | "mm"
                | "rb"
                | "php"
                | "cs"
        )
    )
}

fn likely_definition_line(line: &str, symbol: &str) -> bool {
    let trimmed = line.trim();
    let definition_patterns = [
        format!("fn {symbol}"),
        format!("struct {symbol}"),
        format!("enum {symbol}"),
        format!("trait {symbol}"),
        format!("class {symbol}"),
        format!("interface {symbol}"),
        format!("type {symbol}"),
        format!("const {symbol}"),
        format!("let {symbol}"),
        format!("function {symbol}"),
        format!("impl {symbol}"),
    ];
    definition_patterns
        .iter()
        .any(|pattern| trimmed.contains(pattern))
}

pub(crate) fn extract_symbol_at_position(
    contents: &str,
    line: usize,
    character: usize,
) -> Option<String> {
    let lines = contents.lines().collect::<Vec<_>>();
    let line_text = lines.get(line.saturating_sub(1))?;
    let chars = line_text.chars().collect::<Vec<_>>();
    if chars.is_empty() {
        return None;
    }
    let mut index = character
        .saturating_sub(1)
        .min(chars.len().saturating_sub(1));
    if !is_symbol_char(chars[index]) && index > 0 && is_symbol_char(chars[index - 1]) {
        index -= 1;
    }
    if !is_symbol_char(chars[index]) {
        return None;
    }
    let mut start = index;
    while start > 0 && is_symbol_char(chars[start - 1]) {
        start -= 1;
    }
    let mut end = index;
    while end + 1 < chars.len() && is_symbol_char(chars[end + 1]) {
        end += 1;
    }
    Some(chars[start..=end].iter().collect())
}

fn is_symbol_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

fn collect_outgoing_calls(contents: &str, line: usize) -> Vec<String> {
    let lines = contents.lines().collect::<Vec<_>>();
    if lines.is_empty() {
        return Vec::new();
    }
    let start = line.saturating_sub(1).min(lines.len().saturating_sub(1));
    let mut collected = Vec::new();
    let mut brace_depth = 0isize;
    let mut seen_open = false;
    for candidate in lines.iter().skip(start).take(60) {
        for ch in candidate.chars() {
            if ch == '{' {
                brace_depth += 1;
                seen_open = true;
            } else if ch == '}' {
                brace_depth -= 1;
            }
        }
        collected.extend(extract_call_names(candidate));
        if seen_open && brace_depth <= 0 {
            break;
        }
    }
    collected.sort();
    collected.dedup();
    collected
}

fn extract_call_names(line: &str) -> Vec<String> {
    let mut calls = Vec::new();
    let chars = line.chars().collect::<Vec<_>>();
    let mut index = 0usize;
    while index < chars.len() {
        if is_symbol_char(chars[index]) {
            let start = index;
            while index < chars.len() && is_symbol_char(chars[index]) {
                index += 1;
            }
            let token = chars[start..index].iter().collect::<String>();
            if index < chars.len()
                && chars[index] == '('
                && !matches!(
                    token.as_str(),
                    "if" | "for"
                        | "while"
                        | "match"
                        | "return"
                        | "fn"
                        | "function"
                        | "class"
                        | "struct"
                        | "enum"
                        | "trait"
                        | "impl"
                        | "let"
                        | "const"
                        | "new"
                )
            {
                calls.push(token);
            }
        } else {
            index += 1;
        }
    }
    calls
}
