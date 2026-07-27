use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, UNIX_EPOCH};

use serde_json::{Map, Value};

use crate::{
    ToolContext, ToolError, ToolOutcome, ToolRegistry,
    fs_text::resolve_path,
    grep_tool::{
        EngineError, RG_TIMEOUT_ENV, SearchEngine, VCS_DIRECTORIES_TO_EXCLUDE, relative_to_cwd,
        rg_timeout_ms, ripgrep_status, run_ripgrep, search_unified_metadata, short_stderr,
    },
    output::truncate_tool_output_with_metadata,
    payload::{
        optional_path_field_keys, parse_payload, raw_string_payload_or_input, string_field_keys,
    },
    permissions::require_tools,
};

const MAX_GLOB_RESULTS: usize = 100;
const MAX_GLOB_OUTPUT_CHARS: usize = 100_000;
const FALLBACK_MAX_FILES_SCANNED: usize = 50_000;

#[derive(Debug)]
struct GlobInvocation<'a> {
    pattern: &'a str,
    base_input: &'a str,
    search_path: PathBuf,
    relative_base: String,
}

impl ToolRegistry {
    pub(crate) async fn glob(
        &self,
        input: &str,
        context: &ToolContext,
    ) -> Result<ToolOutcome, ToolError> {
        require_tools(context)?;
        let payload = parse_payload(input)?;
        let pattern = string_field_keys(&payload, &["pattern", "glob"])
            .or_else(|| raw_string_payload_or_input(&payload, input))
            .or_else(|| Some("*".to_string()))
            .expect("glob pattern should always have a default");
        let base = optional_path_field_keys(&payload, &["path", "base"])
            .unwrap_or_else(|| ".".to_string());
        let search_path = resolve_path(&context.cwd, &base)?;
        let invocation = GlobInvocation {
            pattern: &pattern,
            base_input: &base,
            search_path: search_path.clone(),
            relative_base: base.clone(),
        };
        let probe = ripgrep_status().await;
        if probe.available {
            match run_glob_with_ripgrep(&invocation, context, probe.version.clone()).await {
                Ok(outcome) => return Ok(outcome),
                Err(EngineError::FallbackRequested(diagnostic)) => {
                    return run_glob_fallback(&invocation, context, Some(diagnostic));
                }
                Err(EngineError::Fatal(error)) => return Err(error),
            }
        }
        run_glob_fallback(&invocation, context, probe.error)
    }
}

async fn run_glob_with_ripgrep(
    invocation: &GlobInvocation<'_>,
    context: &ToolContext,
    ripgrep_version: Option<String>,
) -> Result<ToolOutcome, EngineError> {
    let started = Instant::now();
    let args: Vec<&str> = vec![
        "--files",
        "--glob",
        invocation.pattern,
        "--sort=modified",
        "--no-ignore",
        "--hidden",
    ];
    let outcome = run_ripgrep(
        &args,
        None,
        &invocation.search_path,
        context,
        rg_timeout_ms(context),
    )
    .await
    .map_err(EngineError::Fatal)?;
    if outcome.timed_out {
        return Err(EngineError::FallbackRequested(format!(
            "ripgrep timed out after {} seconds while listing files (set {RG_TIMEOUT_ENV} to extend the timeout)",
            rg_timeout_ms(context) / 1000
        )));
    }
    let exit_code = outcome.exit_code.unwrap_or_default();
    if exit_code != 0 && exit_code != 1 {
        let message = if outcome.stderr.trim().is_empty() {
            outcome.stdout.trim().to_string()
        } else {
            short_stderr(&outcome.stderr)
        };
        return Err(EngineError::FallbackRequested(format!(
            "ripgrep exited {exit_code}: {message}"
        )));
    }
    let mut files = outcome
        .stdout
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| relative_to_cwd(&context.cwd, invocation.search_path.join(line)))
        .collect::<Vec<_>>();
    Ok(format_glob_outcome(
        invocation,
        &mut files,
        SearchEngine::Ripgrep,
        started.elapsed(),
        outcome.eagain_retried,
        ripgrep_version,
        None,
    ))
}

fn run_glob_fallback(
    invocation: &GlobInvocation<'_>,
    context: &ToolContext,
    diagnostic: Option<String>,
) -> Result<ToolOutcome, ToolError> {
    let started = Instant::now();
    if !invocation.search_path.exists() {
        return Err(ToolError::ExecutionFailed(format!(
            "Path does not exist: {}. Note: paths are relative to the workspace root: {}.",
            invocation.base_input,
            context.cwd.display()
        )));
    }
    let mut entries = collect_fallback_files(
        &invocation.search_path,
        invocation.pattern,
        FALLBACK_MAX_FILES_SCANNED,
    )?;
    entries.sort_by(|a, b| {
        a.mtime_ns
            .cmp(&b.mtime_ns)
            .then_with(|| a.absolute_path.cmp(&b.absolute_path))
    });
    let mut files = entries
        .into_iter()
        .map(|entry| relative_to_cwd(&context.cwd, entry.absolute_path))
        .collect::<Vec<_>>();
    Ok(format_glob_outcome(
        invocation,
        &mut files,
        SearchEngine::Fallback,
        started.elapsed(),
        false,
        None,
        diagnostic,
    ))
}

fn format_glob_outcome(
    invocation: &GlobInvocation<'_>,
    files: &mut Vec<String>,
    engine: SearchEngine,
    duration: Duration,
    eagain_retried: bool,
    ripgrep_version: Option<String>,
    diagnostic: Option<String>,
) -> ToolOutcome {
    let total_matches = files.len();
    let truncated = files.len() > MAX_GLOB_RESULTS;
    if truncated {
        files.truncate(MAX_GLOB_RESULTS);
    }
    let mut rendered = if files.is_empty() {
        "No files found".to_string()
    } else {
        files.join("\n")
    };
    if truncated {
        rendered
            .push_str("\n(Results are truncated. Consider using a more specific path or pattern.)");
    }
    if let Some(message) = &diagnostic
        && engine == SearchEngine::Fallback
    {
        write!(
            rendered,
            "\n[Glob fallback engaged ({}). Results may differ from ripgrep.]",
            short_stderr(message)
        )
        .expect("writing to String cannot fail");
    }
    let truncated_output = truncate_tool_output_with_metadata(
        rendered,
        MAX_GLOB_OUTPUT_CHARS,
        "Glob output truncated for transcript safety. Narrow the pattern or path to inspect more results.",
    );
    let metadata = build_glob_metadata(
        invocation,
        files,
        total_matches,
        truncated,
        engine,
        duration,
        eagain_retried,
        ripgrep_version,
        diagnostic,
    );
    ToolOutcome {
        name: "glob".to_string(),
        summary: format!("Matched {total_matches} path(s)."),
        output: truncated_output.output,
        metadata: Some(metadata),
        changed_paths: Vec::new(),
    }
}

#[allow(clippy::too_many_arguments)]
fn build_glob_metadata(
    invocation: &GlobInvocation<'_>,
    files: &[String],
    num_files: usize,
    truncated: bool,
    engine: SearchEngine,
    duration: Duration,
    eagain_retried: bool,
    ripgrep_version: Option<String>,
    diagnostic: Option<String>,
) -> Value {
    let mut glob = Map::new();
    glob.insert(
        "pattern".to_string(),
        Value::String(invocation.pattern.to_string()),
    );
    glob.insert(
        "path".to_string(),
        Value::String(invocation.relative_base.clone()),
    );
    glob.insert(
        "searchPath".to_string(),
        Value::String(invocation.search_path.display().to_string()),
    );
    glob.insert(
        "durationMs".to_string(),
        Value::Number(serde_json::Number::from(duration.as_millis() as u64)),
    );
    glob.insert(
        "numFiles".to_string(),
        Value::Number(serde_json::Number::from(num_files as u64)),
    );
    glob.insert(
        "filenames".to_string(),
        Value::Array(files.iter().cloned().map(Value::String).collect()),
    );
    glob.insert(
        "matchCount".to_string(),
        Value::Number(serde_json::Number::from(num_files as u64)),
    );
    glob.insert("truncated".to_string(), Value::Bool(truncated));
    glob.insert(
        "engine".to_string(),
        Value::String(engine.as_str().to_string()),
    );
    if eagain_retried {
        glob.insert("ripgrepEagainRetried".to_string(), Value::Bool(true));
    }
    if let Some(version) = ripgrep_version {
        glob.insert("ripgrepVersion".to_string(), Value::String(version));
    }
    if let Some(message) = diagnostic.clone() {
        glob.insert("diagnostic".to_string(), Value::String(message));
    }
    search_unified_metadata("glob", Value::Object(glob), duration, truncated, diagnostic)
}

// ─── Glob matching (used by both glob and grep fallback) ────────────────────

#[derive(Debug)]
struct FallbackEntry {
    absolute_path: PathBuf,
    mtime_ns: u128,
}

fn collect_fallback_files(
    base: &Path,
    pattern: &str,
    cap: usize,
) -> Result<Vec<FallbackEntry>, ToolError> {
    let mut entries = Vec::new();
    let walker = walkdir::WalkDir::new(base).follow_links(false).into_iter();
    let walker = walker.filter_entry(|entry| {
        entry
            .file_name()
            .to_str()
            .is_none_or(|name| !VCS_DIRECTORIES_TO_EXCLUDE.contains(&name))
    });
    for entry in walker {
        if entries.len() >= cap {
            break;
        }
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        if !entry.file_type().is_file() {
            continue;
        }
        let absolute = entry.path().to_path_buf();
        let relative_to_base = absolute
            .strip_prefix(base)
            .map_or_else(|_| absolute.clone(), Path::to_path_buf);
        let relative_str = relative_to_base.to_string_lossy().replace('\\', "/");
        if !glob_matches(pattern, &relative_str) {
            continue;
        }
        let mtime_ns = entry
            .metadata()
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |duration| duration.as_nanos());
        entries.push(FallbackEntry {
            absolute_path: absolute,
            mtime_ns,
        });
    }
    Ok(entries)
}

pub(crate) fn glob_matches(pattern: &str, candidate: &str) -> bool {
    let normalized_pattern = pattern.replace('\\', "/");
    let normalized_candidate = candidate.replace('\\', "/");
    let pattern_has_slash = normalized_pattern.contains('/');
    if pattern_has_slash {
        glob_match_path(&normalized_pattern, &normalized_candidate)
    } else {
        let basename = normalized_candidate
            .rsplit('/')
            .next()
            .unwrap_or(&normalized_candidate);
        glob_match_segment(&normalized_pattern, basename)
    }
}

fn glob_match_path(pattern: &str, candidate: &str) -> bool {
    let pattern_segments: Vec<&str> = pattern.split('/').collect();
    let candidate_segments: Vec<&str> = candidate.split('/').collect();
    glob_match_segments(&pattern_segments, &candidate_segments)
}

fn glob_match_segments(pattern_segments: &[&str], candidate_segments: &[&str]) -> bool {
    if pattern_segments.is_empty() {
        return candidate_segments.is_empty();
    }
    let head = pattern_segments[0];
    if head == "**" {
        let rest_pattern = &pattern_segments[1..];
        if rest_pattern.is_empty() {
            return true;
        }
        for index in 0..=candidate_segments.len() {
            if glob_match_segments(rest_pattern, &candidate_segments[index..]) {
                return true;
            }
        }
        return false;
    }
    if candidate_segments.is_empty() {
        return false;
    }
    if !glob_match_segment(head, candidate_segments[0]) {
        return false;
    }
    glob_match_segments(&pattern_segments[1..], &candidate_segments[1..])
}

fn glob_match_segment(pattern: &str, candidate: &str) -> bool {
    if pattern.contains('{') && pattern.contains('}') {
        for option in expand_braces(pattern) {
            if glob_match_segment(&option, candidate) {
                return true;
            }
        }
        return false;
    }
    let pattern_bytes = pattern.as_bytes();
    let candidate_bytes = candidate.as_bytes();
    glob_match_segment_bytes(pattern_bytes, candidate_bytes)
}

fn glob_match_segment_bytes(pattern: &[u8], candidate: &[u8]) -> bool {
    let mut pi = 0usize;
    let mut ci = 0usize;
    let mut star_pi: Option<usize> = None;
    let mut star_ci = 0usize;
    while ci < candidate.len() {
        let pattern_byte = pattern.get(pi).copied();
        match pattern_byte {
            Some(b'*') => {
                star_pi = Some(pi);
                star_ci = ci;
                pi += 1;
            }
            Some(b'?') => {
                pi += 1;
                ci += 1;
            }
            Some(b) if b == candidate[ci] => {
                pi += 1;
                ci += 1;
            }
            _ => {
                if let Some(saved) = star_pi {
                    pi = saved + 1;
                    star_ci += 1;
                    ci = star_ci;
                } else {
                    return false;
                }
            }
        }
    }
    while pi < pattern.len() && pattern[pi] == b'*' {
        pi += 1;
    }
    pi == pattern.len()
}

fn expand_braces(pattern: &str) -> Vec<String> {
    if let Some(open) = pattern.find('{')
        && let Some(close) = pattern[open..].find('}').map(|index| open + index)
    {
        let prefix = &pattern[..open];
        let inside = &pattern[open + 1..close];
        let suffix = &pattern[close + 1..];
        let mut results = Vec::new();
        for option in inside.split(',') {
            let combined = format!("{prefix}{option}{suffix}");
            results.extend(expand_braces(&combined));
        }
        return results;
    }
    vec![pattern.to_string()]
}
