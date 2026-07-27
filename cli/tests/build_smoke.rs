//! Black-box smoke tests for the packaged `orbcode` binary.
//!
//! Exercises the release-validation surfaces a packaged build has to get right:
//!   - `orbcode --version` exposes git SHA + target + provider list.
//!   - `orbcode doctor` includes the matching `build_info` row.
//!   - packaged command smoke covers help, providers, and sessions.
//!   - `orbcode prompt` against a stub provider produces a streaming reply.
//!   - the same prompt persists a `.jsonl` transcript under the project dir.

use std::fs;
use std::process::Command;

const ORBCODE_BIN: &str = env!("CARGO_BIN_EXE_orbcode");

#[test]
fn version_long_form_exposes_git_sha_target_and_providers() {
    let output = Command::new(ORBCODE_BIN)
        .arg("--version")
        .output()
        .expect("run orbcode --version");
    assert!(
        output.status.success(),
        "orbcode --version failed: {:?}",
        output.status.code()
    );
    let stdout = String::from_utf8_lossy(&output.stdout);

    // First line must be parseable: `orbcode <version> (<sha>[+dirty])`
    let first_line = stdout.lines().next().expect("at least one line");
    assert!(
        first_line.starts_with("orbcode "),
        "version line should start with binary name; got `{first_line}`"
    );
    assert!(
        first_line.contains('(') && first_line.contains(')'),
        "version line should embed sha in parens; got `{first_line}`"
    );

    for needle in &["target:", "profile:", "built:", "providers:"] {
        assert!(
            stdout.contains(needle),
            "--version output missing `{needle}`:\n{stdout}"
        );
    }
    for provider in &["anthropic", "openai", "gemini", "grok"] {
        assert!(
            stdout.contains(provider),
            "--version output missing provider `{provider}`:\n{stdout}"
        );
    }
}

#[test]
fn version_short_form_is_one_line_with_target() {
    let output = Command::new(ORBCODE_BIN)
        .arg("-V")
        .output()
        .expect("run orbcode -V");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    let trimmed = stdout.trim();
    assert!(
        !trimmed.contains('\n'),
        "-V should be single-line; got `{trimmed}`"
    );
    assert!(trimmed.starts_with("orbcode "));
    // The single-line form embeds `(<sha>[+dirty] <target>)`.
    assert!(
        trimmed.contains("aarch64-")
            || trimmed.contains("x86_64-")
            || trimmed.contains("riscv")
            || trimmed.contains("armv"),
        "-V should include the build target; got `{trimmed}`"
    );
}

#[test]
fn doctor_emits_build_info_row_consistent_with_version() {
    let scratch = tempfile::tempdir().expect("temp scratch");
    let home = scratch.path().join("home");
    let cwd = scratch.path().join("cwd");
    fs::create_dir_all(&home).expect("create home");
    fs::create_dir_all(&cwd).expect("create cwd");

    let output = Command::new(ORBCODE_BIN)
        .current_dir(&cwd)
        .env("CLAUDE_CONFIG_DIR", &home)
        .env("ANTHROPIC_BASE_URL", "stub://anthropic")
        .env("ORBCODE_PROVIDER", "anthropic")
        .env_remove("ORBCODE_HOME")
        .env_remove("CLAUDE_CODE_USE_OPENAI")
        .arg("doctor")
        .output()
        .expect("run orbcode doctor");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "doctor exited non-zero: {:?}\n{stdout}",
        output.status.code()
    );

    let build_info_line = stdout
        .lines()
        .find(|line| line.trim_start().contains("build_info"))
        .unwrap_or_else(|| panic!("doctor output missing build_info row:\n{stdout}"));
    for needle in &[
        "version=",
        "sha=",
        "target=",
        "profile=",
        "built=",
        "providers=",
    ] {
        assert!(
            build_info_line.contains(needle),
            "build_info row missing `{needle}`:\n{build_info_line}"
        );
    }

    // doctor must include the build_info row in its summary counts.
    let summary = stdout
        .lines()
        .find(|line| line.starts_with("doctor summary:"))
        .expect("doctor summary line present");
    assert!(
        summary.contains("pass="),
        "summary line missing pass count: {summary}"
    );
}

#[test]
fn help_lists_packaged_cli_smoke_commands() {
    let output = Command::new(ORBCODE_BIN)
        .arg("--help")
        .output()
        .expect("run orbcode --help");
    assert!(
        output.status.success(),
        "orbcode --help failed: {:?}",
        output.status.code()
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    for needle in &[
        "Usage:",
        "Commands:",
        "prompt",
        "sessions",
        "providers",
        "doctor",
        "--version",
    ] {
        assert!(
            stdout.contains(needle),
            "--help missing `{needle}`:\n{stdout}"
        );
    }
}

#[test]
fn providers_subcommand_smokes_with_isolated_home() {
    let scratch = tempfile::tempdir().expect("temp scratch");
    let home = scratch.path().join("home");
    let cwd = scratch.path().join("cwd");
    fs::create_dir_all(&home).expect("create home");
    fs::create_dir_all(&cwd).expect("create cwd");

    let output = Command::new(ORBCODE_BIN)
        .current_dir(&cwd)
        .env("CLAUDE_CONFIG_DIR", &home)
        .env("ANTHROPIC_BASE_URL", "stub://anthropic")
        .env("ORBCODE_PROVIDER", "anthropic")
        .env_remove("ORBCODE_HOME")
        .env_remove("CLAUDE_CODE_USE_OPENAI")
        .arg("providers")
        .output()
        .expect("run orbcode providers");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "providers exited non-zero: {:?}\nstderr:\n{stderr}\nstdout:\n{stdout}",
        output.status.code()
    );
    for needle in &[
        "active chain:",
        "provider=anthropic",
        "permissions=",
        "anthropic",
        "openai",
        "model=",
        "capabilities=",
    ] {
        assert!(
            stdout.contains(needle),
            "providers output missing `{needle}`:\n{stdout}"
        );
    }
}

#[test]
fn sessions_subcommand_smokes_with_empty_home() {
    let scratch = tempfile::tempdir().expect("temp scratch");
    let home = scratch.path().join("home");
    let cwd = scratch.path().join("cwd");
    fs::create_dir_all(&home).expect("create home");
    fs::create_dir_all(&cwd).expect("create cwd");

    let output = Command::new(ORBCODE_BIN)
        .current_dir(&cwd)
        .env("CLAUDE_CONFIG_DIR", &home)
        .env_remove("ORBCODE_HOME")
        .arg("sessions")
        .output()
        .expect("run orbcode sessions");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "sessions exited non-zero: {:?}\nstdout:\n{stdout}",
        output.status.code()
    );
    assert!(
        stdout.contains("no persisted sessions"),
        "empty sessions output should explain the empty state:\n{stdout}"
    );

    let json_output = Command::new(ORBCODE_BIN)
        .current_dir(&cwd)
        .env("CLAUDE_CONFIG_DIR", &home)
        .env_remove("ORBCODE_HOME")
        .args(["sessions", "--json"])
        .output()
        .expect("run orbcode sessions --json");
    let json_stdout = String::from_utf8_lossy(&json_output.stdout);
    assert!(
        json_output.status.success(),
        "sessions --json exited non-zero: {:?}\nstdout:\n{json_stdout}",
        json_output.status.code()
    );
    assert!(
        json_stdout.trim().is_empty(),
        "empty sessions --json should emit no rows, got:\n{json_stdout}"
    );
}

#[test]
fn prompt_against_stub_provider_streams_assistant_reply() {
    let scratch = tempfile::tempdir().expect("temp scratch");
    let home = scratch.path().join("home");
    let cwd = scratch.path().join("cwd");
    fs::create_dir_all(&home).expect("create home");
    fs::create_dir_all(&cwd).expect("create cwd");

    let output = Command::new(ORBCODE_BIN)
        .current_dir(&cwd)
        .env("CLAUDE_CONFIG_DIR", &home)
        .env("ANTHROPIC_BASE_URL", "stub://anthropic")
        .env("ORBCODE_PROVIDER", "anthropic")
        .env_remove("ORBCODE_HOME")
        .env_remove("CLAUDE_CODE_USE_OPENAI")
        .args(["prompt", "smoke ping"])
        .output()
        .expect("run orbcode prompt");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "orbcode prompt failed\nstatus: {:?}\nstderr:\n{stderr}\nstdout:\n{stdout}",
        output.status.code(),
    );

    // The headless harness prints `session <id>` first, then the streamed
    // assistant reply. Both surfaces must be present.
    assert!(
        stdout.lines().any(|line| line.starts_with("session ")),
        "stdout missing `session <id>` header:\n{stdout}"
    );
    assert!(
        stdout.contains("Anthropic compatibility stub response"),
        "stub assistant reply missing from stdout:\n{stdout}"
    );
    // Provider summary lands on stderr at end-of-turn.
    assert!(
        stderr.contains("completed with anthropic"),
        "stderr missing turn-completion line:\n{stderr}"
    );
}

#[test]
fn prompt_persists_transcript_under_projects_dir() {
    let scratch = tempfile::tempdir().expect("temp scratch");
    let home = scratch.path().join("home");
    let cwd = scratch.path().join("cwd");
    fs::create_dir_all(&home).expect("create home");
    fs::create_dir_all(&cwd).expect("create cwd");
    let cwd_real = fs::canonicalize(&cwd).expect("canonicalize cwd");

    let status = Command::new(ORBCODE_BIN)
        .current_dir(&cwd_real)
        .env("CLAUDE_CONFIG_DIR", &home)
        .env("ANTHROPIC_BASE_URL", "stub://anthropic")
        .env("ORBCODE_PROVIDER", "anthropic")
        .env_remove("ORBCODE_HOME")
        .env_remove("CLAUDE_CODE_USE_OPENAI")
        .args(["prompt", "smoke transcript"])
        .status()
        .expect("run orbcode prompt");
    assert!(
        status.success(),
        "orbcode prompt exited non-zero: {status:?}"
    );

    let projects_dir = home.join("projects");
    let project_subdirs: Vec<_> = fs::read_dir(&projects_dir)
        .unwrap_or_else(|err| panic!("projects dir missing under {}: {err}", home.display()))
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect();
    assert_eq!(
        project_subdirs.len(),
        1,
        "expected exactly one project subdir under {}, got {project_subdirs:?}",
        projects_dir.display()
    );
    let project_dir = &project_subdirs[0];

    let transcripts: Vec<_> = fs::read_dir(project_dir)
        .expect("read project dir")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext == "jsonl")
        })
        .collect();
    assert!(
        !transcripts.is_empty(),
        "no .jsonl transcript persisted under {}",
        project_dir.display()
    );

    let transcript_body = fs::read_to_string(&transcripts[0]).expect("read transcript");
    assert!(
        !transcript_body.trim().is_empty(),
        "transcript at {} is empty",
        transcripts[0].display()
    );
    // Each line must be valid JSON (the storage layer's invariant).
    for (index, line) in transcript_body.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        serde_json::from_str::<serde_json::Value>(line).unwrap_or_else(|err| {
            panic!(
                "transcript line {index} is not valid JSON ({err}): {line}\nfull file:\n{transcript_body}"
            )
        });
    }
    // Sanity: the user prompt should appear somewhere in the transcript.
    assert!(
        transcript_body.contains("smoke transcript"),
        "transcript did not record the user prompt:\n{transcript_body}"
    );
}

#[test]
fn project_dir_path_uses_sanitized_separators() {
    let scratch = tempfile::tempdir().expect("temp scratch");
    let home = scratch.path().join("home");
    let cwd = scratch.path().join("deep").join("nested").join("project");
    fs::create_dir_all(&home).expect("create home");
    fs::create_dir_all(&cwd).expect("create cwd");
    let cwd_real = fs::canonicalize(&cwd).expect("canonicalize cwd");

    let status = Command::new(ORBCODE_BIN)
        .current_dir(&cwd_real)
        .env("CLAUDE_CONFIG_DIR", &home)
        .env("ANTHROPIC_BASE_URL", "stub://anthropic")
        .env("ORBCODE_PROVIDER", "anthropic")
        .env_remove("ORBCODE_HOME")
        .env_remove("CLAUDE_CODE_USE_OPENAI")
        .args(["prompt", "path separator smoke"])
        .status()
        .expect("run orbcode prompt");
    assert!(
        status.success(),
        "orbcode prompt exited non-zero: {status:?}"
    );

    let projects_dir = home.join("projects");
    let project_subdirs: Vec<_> = fs::read_dir(&projects_dir)
        .unwrap_or_else(|err| panic!("projects dir missing under {}: {err}", home.display()))
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(project_subdirs.len(), 1, "expected one project subdir");

    let project_slug = &project_subdirs[0];

    // The project slug must never contain native path separators — sanitize_path
    // replaces both `/` and `\` with `-`, guaranteeing a flat directory name
    // regardless of the host OS.
    assert!(
        !project_slug.contains(std::path::MAIN_SEPARATOR),
        "project slug should not contain native path separator '{}'; got `{project_slug}`",
        std::path::MAIN_SEPARATOR,
    );
    assert!(
        !project_slug.contains('/') || cfg!(unix),
        "project slug should not contain forward slash on Windows; got `{project_slug}`",
    );
    assert!(
        !project_slug.contains('\\'),
        "project slug should never contain backslash; got `{project_slug}`",
    );
    assert!(
        project_slug
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-'),
        "project slug should only contain alphanumeric + dash; got `{project_slug}`",
    );
}
