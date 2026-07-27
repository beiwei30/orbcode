use std::fs;
use std::process::Command;

/// Drops three poisoned transcripts (truncated tail, unparseable JSON,
/// leftover atomic-write tmp) into an isolated `CLAUDE_CONFIG_DIR`, runs
/// `orbcode doctor`, and asserts the `session_storage` check reports WARN
/// with the matching recovery hints. Guards the CLI → AppServer →
/// SessionManager → SessionStore wiring that `storage_health()` and
/// `session_storage_check()` unit tests alone don't exercise.
#[test]
fn doctor_session_storage_check_surfaces_recovery_hints() {
    let scratch = tempfile::tempdir().expect("temp scratch dir");
    let home = scratch.path().join("home");
    let cwd = scratch.path().join("cwd");
    fs::create_dir_all(&home).expect("create home dir");
    fs::create_dir_all(&cwd).expect("create cwd dir");

    // Canonicalize: on macOS /var -> /private/var, /tmp -> /private/tmp.
    // Orb Code sanitizes the realpath, so the project dir under home must
    // match what doctor will scan.
    let cwd_real = fs::canonicalize(&cwd).expect("canonicalize cwd");
    let project_name = sanitize_path(&cwd_real.display().to_string());
    let project_dir = home.join("projects").join(&project_name);
    fs::create_dir_all(&project_dir).expect("create project dir");

    // Scenario (a): truncated trailing record (no newline) — exercises
    // partial-line recovery in decode_session_transcript_with_outcome.
    fs::write(
        project_dir.join("s-trunc.jsonl"),
        concat!(
            r#"{"type":"user","uuid":"u1","timestamp":"2026-05-28T00:00:00.000Z","#,
            r#""sessionId":"s-trunc","message":{"role":"user","content":"hi"}}"#,
            "\n",
            r#"{"type":"assistant","uuid":"a1","timestamp":"2026-"#,
        ),
    )
    .expect("write truncated transcript");

    // Scenario (b): completely unparseable transcript — counted as corrupt.
    fs::write(project_dir.join("s-corrupt.jsonl"), "this is not json\n")
        .expect("write corrupt transcript");

    // Scenario (c): leftover *.jsonl.<uuid>.tmp from a crashed atomic write.
    fs::write(project_dir.join("s-good.jsonl.deadbeefcafebabe.tmp"), b"")
        .expect("write stray tmp file");

    // A clean transcript so totals are non-trivial.
    fs::write(
        project_dir.join("s-good.jsonl"),
        concat!(
            r#"{"type":"user","uuid":"u1","timestamp":"2026-05-28T00:00:00.000Z","#,
            r#""sessionId":"s-good","message":{"role":"user","content":"hi"}}"#,
            "\n",
        ),
    )
    .expect("write clean transcript");

    let binary = env!("CARGO_BIN_EXE_orbcode");
    let output = Command::new(binary)
        .current_dir(&cwd_real)
        .env("CLAUDE_CONFIG_DIR", &home)
        .env("ANTHROPIC_BASE_URL", "stub://anthropic")
        .env("ORBCODE_PROVIDER", "anthropic")
        .env_remove("ORBCODE_HOME")
        .env_remove("CLAUDE_CODE_USE_OPENAI")
        .arg("doctor")
        .output()
        .expect("run orbcode doctor");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Doctor exits non-zero only on Fail. We expect Warn -> exit 0.
    assert!(
        output.status.success(),
        "doctor should exit 0 when issues are recoverable\nstatus: {:?}\nstderr:\n{}\nstdout:\n{}",
        output.status.code(),
        stderr,
        stdout,
    );

    let line = stdout
        .lines()
        .find(|line| {
            let trimmed = line.trim_start();
            trimmed.starts_with("PASS session_storage")
                || trimmed.starts_with("WARN session_storage")
                || trimmed.starts_with("FAIL session_storage")
        })
        .unwrap_or_else(|| panic!("session_storage line missing from doctor output:\n{stdout}"));

    let assert_contains = |needle: &str, label: &str| {
        assert!(
            line.contains(needle),
            "{label}: expected substring '{needle}' in session_storage line:\n{line}\nfull stdout:\n{stdout}",
        );
    };

    assert_contains("WARN", "status is WARN");
    assert_contains(&project_name, "project dir in detail");
    assert_contains("corrupt", "corrupt hint (scenario b)");
    assert_contains("mid-line", "trailing-partial hint (scenario a)");
    assert_contains("tmp", "stray tmp hint (scenario c)");
    assert_contains("3 transcript(s)", "transcript count");
    assert_contains("1 recoverable", "recoverable count");
}

/// Mirror of `orbcode_config::claude_home::sanitize_path` for paths short
/// enough to skip the hash suffix. The integration test never produces
/// a > 200 char path so the simple form suffices.
fn sanitize_path(name: &str) -> String {
    const MAX: usize = 200;
    let sanitized: String = name
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect();
    assert!(
        sanitized.len() <= MAX,
        "test fixture path exceeds sanitize length cap ({} > {MAX}); shorten the tempdir prefix",
        sanitized.len(),
    );
    sanitized
}
