use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, UNIX_EPOCH};

use orbcode_protocol::{OutputTruncation, ToolResultMetadata};
use serde_json::{Map, Value};
use tokio::process::Command;
use tokio::sync::OnceCell;
use tokio::time::timeout;

use crate::{
    ToolContext, ToolError, ToolOutcome, ToolRegistry,
    fs_text::resolve_path,
    output::truncate_tool_output_with_metadata,
    payload::{
        bool_field_keys, field_or_raw_keys, optional_path_field_keys, parse_payload,
        string_field_keys, usize_field_keys,
    },
    permissions::require_tools,
    process::run_command_output,
};

// ─── Shared ripgrep infrastructure (used by both grep and glob tools) ───────

pub(crate) const VCS_DIRECTORIES_TO_EXCLUDE: &[&str] =
    &[".git", ".svn", ".hg", ".bzr", ".jj", ".sl"];

const DEFAULT_RG_TIMEOUT_MS: u64 = 20_000;
pub(crate) const RG_TIMEOUT_ENV: &str = "CLAUDE_CODE_GLOB_TIMEOUT_SECONDS";
const RG_FORCE_FALLBACK_ENV: &str = "ORBCODE_FORCE_RG_FALLBACK";
pub(crate) const FALLBACK_MAX_FILE_BYTES: u64 = 5 * 1024 * 1024;

#[cfg(test)]
static FORCE_FALLBACK_TEST_FLAG: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

#[cfg(test)]
pub(crate) fn set_force_fallback_for_tests(force: bool) {
    FORCE_FALLBACK_TEST_FLAG.store(force, std::sync::atomic::Ordering::SeqCst);
}

/// Deterministically simulate a ripgrep run outcome in tests so the fallback
/// triggers (timeout, non-1 exit code) can be exercised without depending on a
/// real `rg` binary or wall-clock timing. While a simulation is active the probe
/// reports ripgrep as available so the rg code path is taken, and
/// [`run_ripgrep`] returns the synthetic outcome instead of spawning `rg`.
#[cfg(test)]
#[derive(Clone, Copy, Debug)]
pub(crate) enum SimulatedRipgrep {
    Timeout,
    Exit(i32),
}

#[cfg(test)]
static SIMULATED_RIPGREP: std::sync::Mutex<Option<SimulatedRipgrep>> = std::sync::Mutex::new(None);

#[cfg(test)]
pub(crate) fn set_simulated_ripgrep_for_tests(sim: Option<SimulatedRipgrep>) {
    *SIMULATED_RIPGREP
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = sim;
}

#[cfg(test)]
fn simulated_ripgrep() -> Option<SimulatedRipgrep> {
    *SIMULATED_RIPGREP
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[derive(Clone, Debug)]
pub(crate) struct RipgrepProbe {
    pub(crate) available: bool,
    pub(crate) version: Option<String>,
    pub(crate) error: Option<String>,
}

static RIPGREP_PROBE: OnceCell<RipgrepProbe> = OnceCell::const_new();

pub(crate) fn force_fallback_env() -> bool {
    if std::env::var(RG_FORCE_FALLBACK_ENV).is_ok_and(|value| !value.is_empty() && value != "0") {
        return true;
    }
    #[cfg(test)]
    {
        if FORCE_FALLBACK_TEST_FLAG.load(std::sync::atomic::Ordering::SeqCst) {
            return true;
        }
    }
    false
}

pub(crate) async fn ripgrep_status() -> RipgrepProbe {
    #[cfg(test)]
    if simulated_ripgrep().is_some() {
        return RipgrepProbe {
            available: true,
            version: Some("ripgrep <simulated>".to_string()),
            error: None,
        };
    }
    if force_fallback_env() {
        return RipgrepProbe {
            available: false,
            version: None,
            error: Some(format!("ripgrep disabled via {RG_FORCE_FALLBACK_ENV}")),
        };
    }
    RIPGREP_PROBE
        .get_or_init(|| async {
            let mut command = Command::new("rg");
            command.arg("--version");
            match timeout(Duration::from_secs(5), command.output()).await {
                Ok(Ok(output)) if output.status.success() => {
                    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    let version = stdout.lines().next().map(str::to_string);
                    RipgrepProbe {
                        available: true,
                        version,
                        error: None,
                    }
                }
                Ok(Ok(output)) => RipgrepProbe {
                    available: false,
                    version: None,
                    error: Some(short_stderr(&String::from_utf8_lossy(&output.stderr))),
                },
                Ok(Err(error)) => RipgrepProbe {
                    available: false,
                    version: None,
                    error: Some(error.to_string()),
                },
                Err(_) => RipgrepProbe {
                    available: false,
                    version: None,
                    error: Some("`rg --version` timed out after 5s".into()),
                },
            }
        })
        .await
        .clone()
}

pub(crate) fn rg_timeout_ms(context: &crate::ToolContext) -> u64 {
    context
        .resolve_env(RG_TIMEOUT_ENV)
        .and_then(|raw| raw.parse::<u64>().ok())
        .filter(|seconds| *seconds > 0)
        .map_or(DEFAULT_RG_TIMEOUT_MS, |seconds| {
            seconds.saturating_mul(1000)
        })
}

pub(crate) fn short_stderr(stderr: &str) -> String {
    let trimmed = stderr.trim();
    if trimmed.len() <= 240 {
        trimmed.to_string()
    } else {
        let mut clipped: String = trimmed.chars().take(240).collect();
        clipped.push_str(" …");
        clipped
    }
}

fn is_eagain_error(stderr: &str) -> bool {
    stderr.contains("os error 11") || stderr.contains("Resource temporarily unavailable")
}

#[derive(Debug)]
pub(crate) struct RipgrepRunOutcome {
    pub(crate) stdout: String,
    pub(crate) stderr: String,
    pub(crate) exit_code: Option<i32>,
    pub(crate) eagain_retried: bool,
    pub(crate) timed_out: bool,
}

pub(crate) async fn run_ripgrep(
    base_args: &[&str],
    target: Option<&Path>,
    cwd: &Path,
    context: &ToolContext,
    timeout_ms: u64,
) -> Result<RipgrepRunOutcome, ToolError> {
    #[cfg(test)]
    if let Some(sim) = simulated_ripgrep() {
        return Ok(match sim {
            SimulatedRipgrep::Timeout => RipgrepRunOutcome {
                stdout: String::new(),
                stderr: format!(
                    "ripgrep search timed out after {} seconds",
                    timeout_ms / 1000
                ),
                exit_code: None,
                eagain_retried: false,
                timed_out: true,
            },
            SimulatedRipgrep::Exit(code) => RipgrepRunOutcome {
                stdout: String::new(),
                stderr: "simulated ripgrep failure".to_string(),
                exit_code: Some(code),
                eagain_retried: false,
                timed_out: false,
            },
        });
    }
    let outcome = run_ripgrep_once(base_args, target, cwd, context, timeout_ms, false).await?;
    if outcome.timed_out || !is_eagain_error(&outcome.stderr) || outcome.exit_code == Some(0) {
        return Ok(outcome);
    }
    let mut retried = run_ripgrep_once(base_args, target, cwd, context, timeout_ms, true).await?;
    retried.eagain_retried = true;
    Ok(retried)
}

async fn run_ripgrep_once(
    base_args: &[&str],
    target: Option<&Path>,
    cwd: &Path,
    context: &ToolContext,
    timeout_ms: u64,
    single_threaded: bool,
) -> Result<RipgrepRunOutcome, ToolError> {
    let mut command = Command::new("rg");
    if single_threaded {
        command.arg("-j").arg("1");
    }
    for arg in base_args {
        command.arg(arg);
    }
    if let Some(target) = target {
        command.arg(target);
    }
    command.current_dir(cwd);
    let run = run_command_output(&mut command, context);
    match timeout(Duration::from_millis(timeout_ms), run).await {
        Ok(Ok(output)) => Ok(RipgrepRunOutcome {
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            exit_code: output.status.code(),
            eagain_retried: false,
            timed_out: false,
        }),
        Ok(Err(error)) => Err(error),
        Err(_) => Ok(RipgrepRunOutcome {
            stdout: String::new(),
            stderr: format!(
                "ripgrep search timed out after {} seconds",
                timeout_ms / 1000
            ),
            exit_code: None,
            eagain_retried: false,
            timed_out: true,
        }),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SearchEngine {
    Ripgrep,
    Fallback,
}

impl SearchEngine {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            SearchEngine::Ripgrep => "ripgrep",
            SearchEngine::Fallback => "fallback",
        }
    }
}

pub(crate) fn relative_to_cwd(cwd: &Path, path: impl AsRef<Path>) -> String {
    let path = path.as_ref();
    path.strip_prefix(cwd)
        .ok()
        .filter(|relative| !relative.as_os_str().is_empty())
        .unwrap_or(path)
        .display()
        .to_string()
        .replace('\\', "/")
}

pub(crate) fn path_to_absolute(cwd: &Path, path: &str) -> PathBuf {
    let path = PathBuf::from(path);
    if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    }
}

/// Wrap a tool-specific search block (`glob`/`grep`) with the shared
/// [`ToolResultMetadata`] fields so transcript consumers can read the unified
/// schema while the legacy nested block stays intact for backward compatibility.
pub(crate) fn search_unified_metadata(
    nested_key: &str,
    nested: Value,
    duration: Duration,
    truncated: bool,
    diagnostic: Option<String>,
) -> Value {
    let unified = ToolResultMetadata {
        duration_ms: Some(duration.as_millis() as u64),
        truncation: Some(OutputTruncation {
            truncated,
            original_chars: None,
            omitted_chars: None,
        }),
        diagnostics: diagnostic.into_iter().collect(),
        ..Default::default()
    };
    let mut metadata = unified.to_value();
    if let Some(map) = metadata.as_object_mut() {
        map.insert(nested_key.to_string(), nested);
    }
    metadata
}

#[derive(Debug)]
pub(crate) enum EngineError {
    FallbackRequested(String),
    Fatal(ToolError),
}

// ─── Grep-specific implementation ───────────────────────────────────────────

const DEFAULT_GREP_HEAD_LIMIT: usize = 250;
const MAX_GREP_OUTPUT_CHARS: usize = 20_000;

#[derive(Debug)]
struct GrepInvocation<'a> {
    pattern: &'a str,
    target_input: &'a str,
    target_path: PathBuf,
    file_glob: Option<&'a str>,
    output_mode: &'a str,
    show_line_numbers: bool,
    case_insensitive: bool,
    head_limit: Option<usize>,
}

impl ToolRegistry {
    pub(crate) async fn grep(
        &self,
        input: &str,
        context: &ToolContext,
    ) -> Result<ToolOutcome, ToolError> {
        require_tools(context)?;
        let payload = parse_payload(input)?;
        let pattern = field_or_raw_keys(&payload, &["pattern", "query"], input)?;
        let target =
            optional_path_field_keys(&payload, &["path"]).unwrap_or_else(|| ".".to_string());
        let file_glob = optional_path_field_keys(&payload, &["glob"]);
        let output_mode = string_field_keys(&payload, &["output_mode"])
            .unwrap_or_else(|| "files_with_matches".to_string());
        let show_line_numbers = bool_field_keys(&payload, &["-n"]).unwrap_or(true);
        let case_insensitive = bool_field_keys(&payload, &["-i"]).unwrap_or(false);
        let head_limit = usize_field_keys(&payload, &["head_limit"]);
        let target_path = resolve_path(&context.cwd, &target)?;
        if !target_path.exists() {
            return Err(ToolError::ExecutionFailed(format!(
                "Path does not exist: {target}. Note: file paths are relative to the workspace root: {}.",
                context.cwd.display()
            )));
        }
        // Validate the pattern up front so an invalid regex is rejected with an
        // explicit, engine-independent message before any `rg` process is
        // spawned. ripgrep's default engine is the same `regex` crate, so this
        // matches what rg would accept.
        if let Err(error) = compile_grep_regex(&pattern, case_insensitive) {
            return Err(ToolError::ExecutionFailed(format!(
                "Invalid regex pattern `{pattern}`: {error}"
            )));
        }
        let invocation = GrepInvocation {
            pattern: &pattern,
            target_input: &target,
            target_path: target_path.clone(),
            file_glob: file_glob.as_deref(),
            output_mode: &output_mode,
            show_line_numbers,
            case_insensitive,
            head_limit,
        };
        let probe = ripgrep_status().await;
        if probe.available {
            match run_grep_with_ripgrep(&invocation, context, probe.version.clone()).await {
                Ok(outcome) => return Ok(outcome),
                Err(EngineError::FallbackRequested(diagnostic)) => {
                    return run_grep_fallback(&invocation, context, Some(diagnostic)).await;
                }
                Err(EngineError::Fatal(error)) => return Err(error),
            }
        }
        run_grep_fallback(&invocation, context, probe.error).await
    }
}

async fn run_grep_with_ripgrep(
    invocation: &GrepInvocation<'_>,
    context: &ToolContext,
    ripgrep_version: Option<String>,
) -> Result<ToolOutcome, EngineError> {
    let started = Instant::now();
    let mut args: Vec<String> = vec![
        "--hidden".into(),
        "--color".into(),
        "never".into(),
        "--max-columns".into(),
        "500".into(),
    ];
    for directory in VCS_DIRECTORIES_TO_EXCLUDE {
        args.push("--glob".into());
        args.push(format!("!{directory}"));
    }
    if invocation.show_line_numbers && invocation.output_mode == "content" {
        args.push("-n".into());
    }
    if invocation.case_insensitive {
        args.push("-i".into());
    }
    if let Some(file_glob) = invocation.file_glob {
        for glob_pattern in split_grep_globs(file_glob) {
            args.push("--glob".into());
            args.push(glob_pattern.to_string());
        }
    }
    match invocation.output_mode {
        "content" => {}
        "count" => args.push("--count".into()),
        _ => args.push("-l".into()),
    }
    if invocation.pattern.starts_with('-') {
        args.push("-e".into());
        args.push(invocation.pattern.to_string());
    } else {
        args.push(invocation.pattern.to_string());
    }
    let target_arg = if PathBuf::from(invocation.target_input).is_absolute() {
        invocation.target_path.clone()
    } else {
        PathBuf::from(invocation.target_input)
    };
    let timeout_ms = rg_timeout_ms(context);
    let arg_refs: Vec<&str> = args.iter().map(std::string::String::as_str).collect();
    let outcome = run_ripgrep(
        &arg_refs,
        Some(&target_arg),
        &context.cwd,
        context,
        timeout_ms,
    )
    .await
    .map_err(EngineError::Fatal)?;
    if outcome.timed_out {
        return Err(EngineError::FallbackRequested(format!(
            "ripgrep search timed out after {} seconds (set {RG_TIMEOUT_ENV} to extend the timeout)",
            timeout_ms / 1000
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
    let body = outcome.stdout.trim();
    let result = render_grep_outcome(
        invocation,
        body,
        SearchEngine::Ripgrep,
        started.elapsed(),
        outcome.eagain_retried,
        ripgrep_version,
        None,
        context,
    )
    .await;
    Ok(result)
}

async fn run_grep_fallback(
    invocation: &GrepInvocation<'_>,
    context: &ToolContext,
    diagnostic: Option<String>,
) -> Result<ToolOutcome, ToolError> {
    let started = Instant::now();
    let regex =
        compile_grep_regex(invocation.pattern, invocation.case_insensitive).map_err(|error| {
            ToolError::ExecutionFailed(format!(
                "ripgrep is unavailable and the fallback could not compile pattern `{}`: {error}",
                invocation.pattern
            ))
        })?;
    let params = FallbackGrepParams {
        target_path: invocation.target_path.clone(),
        file_glob: invocation.file_glob.map(str::to_string),
        output_mode: invocation.output_mode.to_string(),
        show_line_numbers: invocation.show_line_numbers,
    };
    let body = tokio::task::spawn_blocking(move || run_fallback_grep(&params, &regex))
        .await
        .map_err(|e| ToolError::ExecutionFailed(e.to_string()))??;
    Ok(render_grep_outcome(
        invocation,
        &body,
        SearchEngine::Fallback,
        started.elapsed(),
        false,
        None,
        diagnostic,
        context,
    )
    .await)
}

#[allow(clippy::too_many_arguments)]
async fn render_grep_outcome(
    invocation: &GrepInvocation<'_>,
    stdout: &str,
    engine: SearchEngine,
    duration: Duration,
    eagain_retried: bool,
    ripgrep_version: Option<String>,
    diagnostic: Option<String>,
    context: &ToolContext,
) -> ToolOutcome {
    let summary = if let Some(glob) = invocation.file_glob {
        format!(
            "Searched `{}` under {} with glob `{glob}`.",
            invocation.pattern,
            invocation.target_path.display()
        )
    } else {
        format!(
            "Searched `{}` under {}.",
            invocation.pattern,
            invocation.target_path.display()
        )
    };

    let GrepRender {
        rendered,
        num_files,
        num_lines,
        num_matches,
        applied_limit,
        filenames,
    } = render_grep_output(
        stdout,
        invocation.output_mode,
        invocation.head_limit,
        context,
    )
    .await;

    let mut final_rendered = rendered;
    if let Some(message) = &diagnostic
        && engine == SearchEngine::Fallback
    {
        write!(
            final_rendered,
            "\n[Grep fallback engaged ({}). Regex match honoring .gitignore within git repos; results may differ slightly from ripgrep.]",
            short_stderr(message)
        )
        .expect("writing to String cannot fail");
    }
    let truncated_output = truncate_tool_output_with_metadata(
        final_rendered,
        MAX_GREP_OUTPUT_CHARS,
        "Grep output truncated for transcript safety. Narrow the pattern or lower head_limit to inspect the omitted portion.",
    );

    let metadata = build_grep_metadata(
        invocation,
        engine,
        duration,
        eagain_retried,
        ripgrep_version,
        diagnostic,
        num_files,
        num_lines,
        num_matches,
        applied_limit,
        &filenames,
    );

    ToolOutcome {
        name: "grep".to_string(),
        summary,
        output: truncated_output.output,
        metadata: Some(metadata),
        changed_paths: Vec::new(),
    }
}

#[allow(clippy::too_many_arguments)]
fn build_grep_metadata(
    invocation: &GrepInvocation<'_>,
    engine: SearchEngine,
    duration: Duration,
    eagain_retried: bool,
    ripgrep_version: Option<String>,
    diagnostic: Option<String>,
    num_files: usize,
    num_lines: Option<usize>,
    num_matches: Option<usize>,
    applied_limit: Option<usize>,
    filenames: &[String],
) -> Value {
    let mut grep = Map::new();
    grep.insert(
        "pattern".to_string(),
        Value::String(invocation.pattern.to_string()),
    );
    grep.insert(
        "path".to_string(),
        Value::String(invocation.target_input.to_string()),
    );
    if let Some(file_glob) = invocation.file_glob {
        grep.insert("glob".to_string(), Value::String(file_glob.to_string()));
    }
    grep.insert(
        "outputMode".to_string(),
        Value::String(invocation.output_mode.to_string()),
    );
    grep.insert(
        "caseInsensitive".to_string(),
        Value::Bool(invocation.case_insensitive),
    );
    grep.insert(
        "showLineNumbers".to_string(),
        Value::Bool(invocation.show_line_numbers),
    );
    grep.insert(
        "engine".to_string(),
        Value::String(engine.as_str().to_string()),
    );
    grep.insert(
        "durationMs".to_string(),
        Value::Number(serde_json::Number::from(duration.as_millis() as u64)),
    );
    grep.insert(
        "numFiles".to_string(),
        Value::Number(serde_json::Number::from(num_files as u64)),
    );
    if let Some(num_lines) = num_lines {
        grep.insert(
            "numLines".to_string(),
            Value::Number(serde_json::Number::from(num_lines as u64)),
        );
    }
    if let Some(num_matches) = num_matches {
        grep.insert(
            "numMatches".to_string(),
            Value::Number(serde_json::Number::from(num_matches as u64)),
        );
    }
    let match_count = num_matches.or(num_lines).unwrap_or(num_files);
    grep.insert(
        "matchCount".to_string(),
        Value::Number(serde_json::Number::from(match_count as u64)),
    );
    if let Some(limit) = applied_limit {
        grep.insert(
            "appliedLimit".to_string(),
            Value::Number(serde_json::Number::from(limit as u64)),
        );
    }
    if invocation.output_mode == "files_with_matches" || invocation.output_mode == "count" {
        grep.insert(
            "filenames".to_string(),
            Value::Array(filenames.iter().cloned().map(Value::String).collect()),
        );
    }
    if eagain_retried {
        grep.insert("ripgrepEagainRetried".to_string(), Value::Bool(true));
    }
    if let Some(version) = ripgrep_version {
        grep.insert("ripgrepVersion".to_string(), Value::String(version));
    }
    if let Some(message) = diagnostic.clone() {
        grep.insert("diagnostic".to_string(), Value::String(message));
    }
    search_unified_metadata(
        "grep",
        Value::Object(grep),
        duration,
        applied_limit.is_some(),
        diagnostic,
    )
}

struct GrepRender {
    rendered: String,
    num_files: usize,
    num_lines: Option<usize>,
    num_matches: Option<usize>,
    applied_limit: Option<usize>,
    filenames: Vec<String>,
}

async fn render_grep_output(
    stdout: &str,
    output_mode: &str,
    head_limit: Option<usize>,
    context: &ToolContext,
) -> GrepRender {
    if stdout.is_empty() {
        let rendered = if output_mode == "files_with_matches" {
            "No files found".to_string()
        } else {
            "No matches found".to_string()
        };
        return GrepRender {
            rendered,
            num_files: 0,
            num_lines: if output_mode == "content" {
                Some(0)
            } else {
                None
            },
            num_matches: if output_mode == "count" {
                Some(0)
            } else {
                None
            },
            applied_limit: None,
            filenames: Vec::new(),
        };
    }

    let mut lines = stdout.lines().map(str::to_string).collect::<Vec<_>>();
    if output_mode == "files_with_matches" {
        sort_paths_newest_first(&mut lines, &context.cwd).await;
        lines = lines
            .into_iter()
            .map(|path| relative_to_cwd(&context.cwd, path_to_absolute(&context.cwd, &path)))
            .collect();
    } else if output_mode == "content" {
        lines = lines
            .into_iter()
            .map(|line| relativize_grep_content_line(&context.cwd, &line))
            .collect();
    } else if output_mode == "count" {
        lines = lines
            .into_iter()
            .map(|line| relativize_grep_count_line(&context.cwd, &line))
            .collect();
    }

    let total_lines = lines.len();
    let effective_limit = match head_limit {
        Some(0) => None,
        Some(limit) => Some(limit),
        None => Some(DEFAULT_GREP_HEAD_LIMIT),
    };
    let applied_limit = effective_limit.filter(|limit| total_lines > *limit);
    if let Some(limit) = effective_limit {
        lines.truncate(limit);
    }

    let (rendered, num_files, num_lines, num_matches, filenames) = match output_mode {
        "files_with_matches" => {
            let mut rendered = format!(
                "Found {} {}{}",
                lines.len(),
                if lines.len() == 1 { "file" } else { "files" },
                applied_limit
                    .map(|limit| format!(" limit: {limit}"))
                    .unwrap_or_default()
            );
            rendered.push('\n');
            rendered.push_str(&lines.join("\n"));
            (rendered, lines.len(), None, None, lines.clone())
        }
        "count" => {
            let content = lines.join("\n");
            let (matches, files) = count_grep_matches(&lines);
            let mut rendered = content;
            write!(
                rendered,
                "\n\nFound {matches} total {} across {files} {}.",
                if matches == 1 {
                    "occurrence"
                } else {
                    "occurrences"
                },
                if files == 1 { "file" } else { "files" },
            )
            .expect("writing to String cannot fail");
            if let Some(limit) = applied_limit {
                write!(rendered, " with pagination = limit: {limit}")
                    .expect("writing to String cannot fail");
            }
            let filenames = lines
                .iter()
                .filter_map(|line| line.rsplit_once(':').map(|(path, _)| path.to_string()))
                .collect::<Vec<_>>();
            (rendered, files, None, Some(matches), filenames)
        }
        _ => {
            let mut rendered = lines.join("\n");
            if let Some(limit) = applied_limit {
                write!(
                    rendered,
                    "\n\n[Showing results with pagination = limit: {limit}]"
                )
                .expect("writing to String cannot fail");
            }
            (rendered, 0, Some(lines.len()), None, Vec::new())
        }
    };

    GrepRender {
        rendered,
        num_files,
        num_lines,
        num_matches,
        applied_limit,
        filenames,
    }
}

async fn sort_paths_newest_first(paths: &mut [String], cwd: &Path) {
    let mut paths_with_mtime = Vec::with_capacity(paths.len());
    for path in paths.iter() {
        let absolute = path_to_absolute(cwd, path);
        let mtime = tokio::fs::metadata(&absolute)
            .await
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |duration| duration.as_nanos());
        paths_with_mtime.push((path.clone(), mtime));
    }
    paths_with_mtime.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    for (path, (sorted, _)) in paths.iter_mut().zip(paths_with_mtime) {
        *path = sorted;
    }
}

fn split_grep_globs(glob: &str) -> Vec<&str> {
    let mut patterns = Vec::new();
    for raw_pattern in glob.split_whitespace() {
        if raw_pattern.contains('{') && raw_pattern.contains('}') {
            patterns.push(raw_pattern);
        } else {
            patterns.extend(raw_pattern.split(',').filter(|pattern| !pattern.is_empty()));
        }
    }
    patterns
}

fn relativize_grep_content_line(cwd: &Path, line: &str) -> String {
    if let Some((path, rest)) = line.split_once(':') {
        format!(
            "{}:{rest}",
            relative_to_cwd(cwd, path_to_absolute(cwd, path))
        )
    } else {
        line.to_string()
    }
}

fn relativize_grep_count_line(cwd: &Path, line: &str) -> String {
    if let Some(index) = line.rfind(':') {
        let (path, count) = line.split_at(index);
        format!(
            "{}{}",
            relative_to_cwd(cwd, path_to_absolute(cwd, path)),
            count
        )
    } else {
        line.to_string()
    }
}

fn count_grep_matches(lines: &[String]) -> (usize, usize) {
    let mut matches = 0usize;
    let mut files = 0usize;
    for line in lines {
        if let Some(index) = line.rfind(':')
            && let Ok(count) = line[index + 1..].parse::<usize>()
        {
            matches += count;
            files += 1;
        }
    }
    (matches, files)
}

/// Collect candidate files for the grep fallback. Directory targets are walked
/// with the `ignore` crate so `.gitignore`/`.ignore` rules are honored the same
/// way ripgrep's grep invocation honors them: hidden files are shown to match
/// `--hidden`, VCS metadata directories are excluded, and gitignore semantics
/// apply within a git repository (matching rg's default `require_git`). A file
/// target is returned verbatim so an explicitly named file is always searched.
fn collect_fallback_grep_files(target: &Path) -> Vec<PathBuf> {
    if target.is_file() {
        return vec![target.to_path_buf()];
    }
    let mut files = Vec::new();
    let walker = ignore::WalkBuilder::new(target)
        .hidden(false)
        .follow_links(false)
        .filter_entry(|entry| {
            entry
                .file_name()
                .to_str()
                .is_none_or(|name| !VCS_DIRECTORIES_TO_EXCLUDE.contains(&name))
        })
        .build();
    for entry in walker.flatten() {
        if entry
            .file_type()
            .is_some_and(|file_type| file_type.is_file())
        {
            files.push(entry.path().to_path_buf());
        }
    }
    files
}

/// Owned parameter bundle for [`run_fallback_grep`] so it can be sent into
/// `spawn_blocking` (no lifetime parameters).
struct FallbackGrepParams {
    target_path: PathBuf,
    file_glob: Option<String>,
    output_mode: String,
    show_line_numbers: bool,
}

fn run_fallback_grep(
    params: &FallbackGrepParams,
    regex: &regex::Regex,
) -> Result<String, ToolError> {
    let target = &params.target_path;
    let mut output = String::new();
    let mut counts: Vec<(String, usize)> = Vec::new();
    for path in collect_fallback_grep_files(target) {
        let path = path.as_path();
        if let Some(glob) = params.file_glob.as_deref() {
            let relative = path
                .strip_prefix(target)
                .map_or_else(|_| path.to_path_buf(), Path::to_path_buf);
            let relative_str = relative.to_string_lossy().replace('\\', "/");
            if !split_grep_globs(glob)
                .iter()
                .any(|pattern| crate::glob_tool::glob_matches(pattern, &relative_str))
            {
                continue;
            }
        }
        let metadata = match std::fs::metadata(path) {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        if metadata.len() > FALLBACK_MAX_FILE_BYTES {
            continue;
        }
        let contents = match std::fs::read_to_string(path) {
            Ok(value) => value,
            Err(_) => continue,
        };
        let mut file_match_count = 0usize;
        let mut content_lines: Vec<(usize, String)> = Vec::new();
        for (idx, line) in contents.lines().enumerate() {
            if regex.is_match(line) {
                file_match_count += 1;
                if params.output_mode == "content" {
                    content_lines.push((idx + 1, line.to_string()));
                }
            }
        }
        if file_match_count == 0 {
            continue;
        }
        let display_path = path.display().to_string();
        match params.output_mode.as_str() {
            "files_with_matches" => {
                output.push_str(&display_path);
                output.push('\n');
            }
            "count" => {
                counts.push((display_path, file_match_count));
            }
            _ => {
                for (line_no, line) in content_lines {
                    if params.show_line_numbers {
                        writeln!(output, "{display_path}:{line_no}:{line}")
                            .expect("writing to String cannot fail");
                    } else {
                        writeln!(output, "{display_path}:{line}")
                            .expect("writing to String cannot fail");
                    }
                }
            }
        }
    }
    if params.output_mode == "count" {
        for (path, count) in counts {
            writeln!(output, "{path}:{count}").expect("writing to String cannot fail");
        }
    }
    Ok(output.trim_end().to_string())
}

fn compile_grep_regex(pattern: &str, case_insensitive: bool) -> Result<regex::Regex, regex::Error> {
    let mut builder = regex::RegexBuilder::new(pattern);
    builder.case_insensitive(case_insensitive);
    builder.build()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    use crate::ToolCancellationToken;
    use orbcode_protocol::SandboxMode;

    async fn minimal_context(cwd: &Path) -> ToolContext {
        let home = cwd.join("home");
        std::fs::create_dir_all(&home).expect("create home");
        let mcp = orbcode_mcp::McpRegistry::load(&home, cwd)
            .await
            .expect("load mcp");
        ToolContext {
            cwd: cwd.to_path_buf(),
            additional_directories: Vec::new(),
            home_dir: home,
            sandbox_mode: SandboxMode::DangerFullAccess,
            sandbox_allow_network: false,
            allow_network: false,
            allow_tools: true,
            mcp,
            progress: None,
            cancellation: ToolCancellationToken::default(),
            read_state: None,
            session_id: None,
            local_shell_tasks: None,
            on_cwd_change: None,
            plans_directory_override: None,
            ask_user_tx: None,
            settings_env: std::collections::BTreeMap::new(),
            skill_definitions: None,
        }
    }

    fn generate_grep_content_stdout(file_count: usize, lines_per_file: usize) -> String {
        let mut output = String::new();
        for file_index in 0..file_count {
            for line_index in 0..lines_per_file {
                writeln!(
                    output,
                    "src/module_{file_index}/file_{file_index}.rs:{}: \
                     let fixture_value_{line_index} = {line_index};",
                    line_index + 1
                )
                .expect("writing to String cannot fail");
            }
        }
        output
    }

    #[test]
    fn regression_budget_split_grep_globs_parses_patterns() {
        assert_eq!(split_grep_globs("*.rs *.ts"), vec!["*.rs", "*.ts"]);
        assert_eq!(split_grep_globs("{*.rs,*.ts}"), vec!["{*.rs,*.ts}"]);
        assert_eq!(split_grep_globs("*.rs,*.ts"), vec!["*.rs", "*.ts"]);
        assert_eq!(split_grep_globs("  *.rs  "), vec!["*.rs"]);
    }

    #[test]
    fn regression_budget_relative_to_cwd_removes_prefix() {
        let cwd = Path::new("/tmp/project");
        assert_eq!(
            relative_to_cwd(cwd, "/tmp/project/src/main.rs"),
            "src/main.rs"
        );
        assert_eq!(
            relative_to_cwd(cwd, "/other/path/file.rs"),
            "/other/path/file.rs"
        );
        assert_eq!(relative_to_cwd(cwd, "relative/file.rs"), "relative/file.rs");
    }

    #[tokio::test]
    async fn regression_budget_render_grep_output_truncates_at_head_limit() {
        let temp = tempfile::tempdir().expect("create temp dir");
        let cwd = temp.path().join("cwd");
        std::fs::create_dir_all(&cwd).expect("create cwd");
        let context = minimal_context(&cwd).await;

        let stdout = generate_grep_content_stdout(50, 10);
        let total_lines = stdout.lines().count();
        assert_eq!(total_lines, 500);

        let result = render_grep_output(&stdout, "content", Some(50), &context).await;

        let output_lines = result.rendered.lines().count();
        assert!(
            output_lines <= 52,
            "output ({output_lines} lines) should be bounded near head_limit=50 \
             (plus footer)"
        );
        assert_eq!(
            result.applied_limit,
            Some(50),
            "applied_limit should report the limit"
        );
        assert!(
            result.rendered.contains("[Showing results with pagination"),
            "should include pagination notice"
        );
    }

    #[tokio::test]
    #[ignore = "manual stress test for grep output rendering with many files"]
    async fn render_grep_output_stress_processes_large_result_set() {
        const FILE_COUNT: usize = 5_000;
        const LINES_PER_FILE: usize = 10;
        const ITERATIONS: usize = 50;

        let temp = tempfile::tempdir().expect("create temp dir");
        let cwd = temp.path().join("cwd");
        std::fs::create_dir_all(&cwd).expect("create cwd");
        let context = minimal_context(&cwd).await;

        let stdout = generate_grep_content_stdout(FILE_COUNT, LINES_PER_FILE);
        let total_lines = stdout.lines().count();

        let started = Instant::now();
        let mut rendered_lines = 0;
        for _ in 0..ITERATIONS {
            let result = render_grep_output(&stdout, "content", Some(250), &context).await;
            rendered_lines = result.rendered.lines().count();
        }
        let duration = started.elapsed();

        eprintln!(
            "files={FILE_COUNT} lines_per_file={LINES_PER_FILE} \
             total_lines={total_lines} rendered_lines={rendered_lines} \
             iterations={ITERATIONS} total_us={} avg_us={}",
            duration.as_micros(),
            duration.as_micros() / ITERATIONS as u128
        );
    }
}
