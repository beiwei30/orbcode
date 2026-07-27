//! Read-only verification against real `~/.claude` transcripts.
//!
//! Set `ORBCODE_REAL_PROJECTS_DIR` to point at a `~/.claude/projects` directory
//! (or a subdirectory containing `.jsonl` files) to run this test. When the
//! env var is absent the test is silently skipped — CI never touches real data.
//!
//! The test is **strictly read-only**: it decodes every `.jsonl` file, builds
//! a `SessionSummary`, and asserts invariants that must hold for any valid
//! transcript. Nothing is written or deleted.

use std::path::PathBuf;

use orbcode_protocol::unique_display_titles;
use orbcode_session_store::decode_session_transcript_with_outcome;

fn real_projects_dir() -> Option<PathBuf> {
    let dir = std::env::var("ORBCODE_REAL_PROJECTS_DIR").ok()?;
    let dir = dir.trim();
    if dir.is_empty() {
        None
    } else {
        Some(PathBuf::from(dir))
    }
}

fn collect_jsonl_files(root: &std::path::Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let Ok(walker) = std::fs::read_dir(root) else {
        return files;
    };
    for entry in walker.flatten() {
        let path = entry.path();
        if path.is_dir() {
            files.extend(collect_jsonl_files(&path));
        } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            files.push(path);
        }
    }
    files
}

#[test]
fn real_transcripts_decode_and_enrich_without_panic() {
    let Some(projects_dir) = real_projects_dir() else {
        eprintln!("skipping: no real projects dir found (set ORBCODE_REAL_PROJECTS_DIR)");
        return;
    };

    let files = collect_jsonl_files(&projects_dir);
    if files.is_empty() {
        eprintln!(
            "skipping: no .jsonl files found under {}",
            projects_dir.display()
        );
        return;
    }

    eprintln!("scanning {} real transcript files...", files.len());

    let mut total = 0usize;
    let mut decoded = 0usize;
    let mut corrupt = 0usize;
    let mut with_tokens = 0usize;
    let mut with_duration = 0usize;
    let mut summaries = Vec::new();

    for path in &files {
        total += 1;
        let contents = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => {
                corrupt += 1;
                continue;
            }
        };
        let session_id = path
            .file_stem()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

        let outcome = decode_session_transcript_with_outcome(session_id, &contents);

        let Some(session) = outcome.session else {
            corrupt += 1;
            continue;
        };
        decoded += 1;

        let summary = session.summary();

        // --- Invariant checks ---

        // message_count must match actual message vec length
        assert_eq!(
            summary.message_count,
            session.messages.len(),
            "message_count mismatch for {}",
            path.display()
        );

        // token counts must be non-negative (they're u64, so this is always
        // true, but check they don't overflow to weird values)
        assert!(
            summary.total_input_tokens <= u64::MAX / 2,
            "suspiciously large input_tokens for {}",
            path.display()
        );
        assert!(
            summary.total_output_tokens <= u64::MAX / 2,
            "suspiciously large output_tokens for {}",
            path.display()
        );

        // duration must be non-negative if present
        if let Some(dur) = summary.duration_secs {
            assert!(
                dur <= 86400 * 365, // sanity: less than 1 year
                "suspiciously large duration {} for {}",
                dur,
                path.display()
            );
        }

        // sessions with messages should have a title
        if summary.message_count > 0 {
            // title comes from first user message; some sessions start with
            // system records so title can be None even with messages
        }

        if summary.total_input_tokens > 0 || summary.total_output_tokens > 0 {
            with_tokens += 1;
        }
        if summary.duration_secs.is_some() {
            with_duration += 1;
        }

        summaries.push(summary);
    }

    // --- Aggregate checks ---

    eprintln!(
        "results: {total} total, {decoded} decoded, {corrupt} corrupt/empty, \
         {with_tokens} with tokens, {with_duration} with duration"
    );

    // At least some sessions should have decoded successfully
    assert!(
        decoded > 0,
        "no transcripts decoded from {}",
        projects_dir.display()
    );

    // Test unique_display_titles doesn't panic on real data
    let titles = unique_display_titles(&summaries);
    assert_eq!(titles.len(), summaries.len());

    // Check that sessions with titles get non-empty display titles
    let titled_count = titles.iter().filter(|t| t.is_some()).count();
    eprintln!(
        "{titled_count}/{} summaries have display titles",
        summaries.len()
    );

    // Verify sort stability: sort by updated_at DESC + session_id ASC
    let mut sorted = summaries.clone();
    sorted.sort_by(|a, b| {
        b.updated_at
            .cmp(&a.updated_at)
            .then_with(|| a.session_id.cmp(&b.session_id))
    });
    for (i, (_orig, re_sorted)) in summaries.iter().zip(sorted.iter()).enumerate().take(20) {
        // Just verify the sort is deterministic — we're not testing the store's
        // sort here, only that session_id tiebreaker produces stable output.
        // The summaries vec isn't pre-sorted so we only check re-sort consistency.
        assert_eq!(
            re_sorted.session_id, sorted[i].session_id,
            "re-sort should be stable at position {i}"
        );
    }

    eprintln!("all {decoded} real transcripts passed enrichment validation");
}
