//! End-to-end integration test: write → list → enrich → gc → verify.
//!
//! Exercises the full pipeline through `TranscriptFileStore` using temp
//! directories — no real `~/.claude` state is touched.

use std::path::PathBuf;

use chrono::Utc;
use orbcode_protocol::{SessionStatus, SessionSummary, unique_display_titles};
use serde_json::json;

fn make_store(dir: &std::path::Path) -> orbcode_session_store::SessionStore {
    orbcode_session_store::SessionStore::new(
        dir.to_path_buf(),
        PathBuf::from("/tmp/e2e-project"),
        "claude-sonnet-4".to_string(),
    )
}

fn write_raw_transcript(dir: &std::path::Path, session_id: &str, lines: &[serde_json::Value]) {
    let content = lines
        .iter()
        .map(|v| serde_json::to_string(v).unwrap())
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    std::fs::write(dir.join(format!("{session_id}.jsonl")), content).unwrap();
}

fn set_mtime_old(path: &std::path::Path) {
    use std::fs;
    use std::time::{Duration, SystemTime};
    let old = SystemTime::UNIX_EPOCH + Duration::from_secs(1_577_836_800);
    let times = fs::FileTimes::new().set_modified(old);
    let file = fs::File::options().write(true).open(path).unwrap();
    file.set_times(times).unwrap();
}

// -----------------------------------------------------------------------
// Full lifecycle: write sessions → list with enrichment → gc → verify
// -----------------------------------------------------------------------

#[tokio::test]
async fn full_lifecycle_write_list_gc() {
    let temp = tempfile::tempdir().unwrap();
    let store = make_store(temp.path());

    // --- Phase 1: Populate ---

    // Session A: two-turn conversation with token usage
    let ts_a1 = "2026-04-01T10:00:00.000Z";
    let ts_a2 = "2026-04-01T10:02:00.000Z";
    write_raw_transcript(
        temp.path(),
        "session-a",
        &[
            json!({
                "type": "user",
                "uuid": "a-u1",
                "timestamp": ts_a1,
                "message": { "role": "user", "content": "explain ownership" },
                "sessionId": "session-a",
                "cwd": "/tmp/project",
            }),
            json!({
                "type": "assistant",
                "uuid": "a-a1",
                "parentUuid": "a-u1",
                "timestamp": ts_a2,
                "message": {
                    "role": "assistant",
                    "content": [{"type": "text", "text": "Ownership is..."}],
                    "model": "claude-sonnet-4",
                    "usage": {
                        "input_tokens": 500,
                        "output_tokens": 200,
                        "cache_creation_input_tokens": 0,
                        "cache_read_input_tokens": 0,
                        "total_tokens": 700,
                    },
                },
                "sessionId": "session-a",
                "cwd": "/tmp/project",
            }),
        ],
    );

    // Session B: same title as A, older timestamp (tests sort + title uniqueness)
    let ts_b1 = "2026-03-01T08:00:00.000Z";
    write_raw_transcript(
        temp.path(),
        "session-b",
        &[json!({
            "type": "user",
            "uuid": "b-u1",
            "timestamp": ts_b1,
            "message": { "role": "user", "content": "explain ownership" },
            "sessionId": "session-b",
            "cwd": "/tmp/project",
        })],
    );

    // Session C: empty (only whitespace) — will be GC target when old
    std::fs::write(temp.path().join("session-c.jsonl"), "\n").unwrap();
    set_mtime_old(&temp.path().join("session-c.jsonl"));

    // Session D: corrupt — will be GC target when old
    std::fs::write(temp.path().join("session-d.jsonl"), "not-json\n").unwrap();
    set_mtime_old(&temp.path().join("session-d.jsonl"));

    // Session E: has content but old — should survive GC
    let ts_e = "2020-06-01T00:00:00.000Z";
    write_raw_transcript(
        temp.path(),
        "session-e",
        &[json!({
            "type": "user",
            "uuid": "e-u1",
            "timestamp": ts_e,
            "message": { "role": "user", "content": "old but valid" },
            "sessionId": "session-e",
            "cwd": "/tmp/old",
        })],
    );
    set_mtime_old(&temp.path().join("session-e.jsonl"));

    // --- Phase 2: List and verify enrichment ---

    let summaries = store
        .load_project_session_summaries()
        .await
        .expect("list summaries");

    assert_eq!(summaries.len(), 5);

    let find = |id: &str| -> &SessionSummary {
        summaries
            .iter()
            .find(|s| s.session_id == id)
            .unwrap_or_else(|| panic!("missing {id}"))
    };

    // Session A: enriched fields
    let a = find("session-a");
    assert_eq!(a.message_count, 2);
    assert_eq!(a.total_input_tokens, 500);
    assert_eq!(a.total_output_tokens, 200);
    assert!(a.duration_secs.is_some());
    assert_eq!(a.duration_secs.unwrap(), 120); // 2 minutes
    assert!(matches!(a.status, SessionStatus::Available));

    // Session B: 1 message, no tokens
    let b = find("session-b");
    assert_eq!(b.message_count, 1);
    assert_eq!(b.total_input_tokens, 0);
    assert_eq!(b.total_output_tokens, 0);

    // Session C: empty
    let c = find("session-c");
    assert_eq!(c.message_count, 0);
    assert!(matches!(c.status, SessionStatus::Corrupt { .. }));

    // Session D: corrupt
    let d = find("session-d");
    assert_eq!(d.message_count, 0);
    assert!(matches!(d.status, SessionStatus::Corrupt { .. }));

    // --- Phase 3: Verify sort stability ---
    // Summaries should be sorted by updated_at DESC, with session_id ASC tiebreaker.
    // A (2026-04-01) > B (2026-03-01) > E (2020-06-01) > C,D (old mtime, alphabetical)
    let ids: Vec<&str> = summaries.iter().map(|s| s.session_id.as_str()).collect();
    assert_eq!(ids[0], "session-a");
    assert_eq!(ids[1], "session-b");
    // E, C, D all have old timestamps — E has 2020-06-01, C and D have
    // 2020-01-01 mtime, so E comes before C/D, then C < D alphabetically.
    assert_eq!(ids[2], "session-e");
    assert_eq!(ids[3], "session-c");
    assert_eq!(ids[4], "session-d");

    // --- Phase 4: Verify display title uniqueness ---
    // A and B both have auto title "explain ownership"
    let titles = unique_display_titles(&summaries);
    let title_a = titles[0].as_deref().unwrap();
    let title_b = titles[1].as_deref().unwrap();
    assert_eq!(title_a, "explain ownership");
    assert!(
        title_b.starts_with("explain ownership ("),
        "expected suffix, got: {title_b}"
    );
    assert!(
        title_b.contains("session-"),
        "suffix should contain id prefix, got: {title_b}"
    );

    // --- Phase 5: GC stale sessions ---
    let gc = store.gc_stale_sessions(1).await.expect("gc stale sessions");

    assert_eq!(gc.removed, 2, "should remove empty + corrupt old sessions");
    assert!(gc.removed_ids.contains(&"session-c".to_string()));
    assert!(gc.removed_ids.contains(&"session-d".to_string()));
    assert!(
        !gc.removed_ids.contains(&"session-e".to_string()),
        "old session with content must survive"
    );
    assert!(!gc.removed_ids.contains(&"session-a".to_string()));
    assert!(!gc.removed_ids.contains(&"session-b".to_string()));

    // Verify filesystem state
    assert!(!temp.path().join("session-c.jsonl").exists());
    assert!(!temp.path().join("session-d.jsonl").exists());
    assert!(temp.path().join("session-a.jsonl").exists());
    assert!(temp.path().join("session-b.jsonl").exists());
    assert!(temp.path().join("session-e.jsonl").exists());

    // Post-GC listing should have 3 sessions
    let post_gc = store
        .load_project_session_summaries()
        .await
        .expect("post-gc list");
    assert_eq!(post_gc.len(), 3);
}

// -----------------------------------------------------------------------
// GC with threshold 0 removes all stale, threshold MAX removes none
// -----------------------------------------------------------------------

#[tokio::test]
async fn gc_threshold_boundary_cases() {
    let temp = tempfile::tempdir().unwrap();
    let store = make_store(temp.path());

    // Write one corrupt old session
    std::fs::write(temp.path().join("stale.jsonl"), "bad\n").unwrap();
    set_mtime_old(&temp.path().join("stale.jsonl"));

    // Write one recent session with content
    write_raw_transcript(
        temp.path(),
        "recent",
        &[json!({
            "type": "user",
            "uuid": "r-u1",
            "timestamp": Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            "message": { "role": "user", "content": "fresh" },
            "sessionId": "recent",
            "cwd": "/tmp",
        })],
    );

    // Very large threshold: nothing should be removed
    let gc_none = store.gc_stale_sessions(99999).await.unwrap();
    assert_eq!(gc_none.removed, 0);
    assert!(temp.path().join("stale.jsonl").exists());

    // Reasonable threshold: old corrupt session is removed
    let gc_some = store.gc_stale_sessions(1).await.unwrap();
    assert_eq!(gc_some.removed, 1);
    assert!(!temp.path().join("stale.jsonl").exists());
    assert!(temp.path().join("recent.jsonl").exists());
}

// -----------------------------------------------------------------------
// Multiple token usages accumulate correctly
// -----------------------------------------------------------------------

#[tokio::test]
async fn multi_turn_token_accumulation() {
    let temp = tempfile::tempdir().unwrap();
    let store = make_store(temp.path());

    write_raw_transcript(
        temp.path(),
        "multi-turn",
        &[
            json!({
                "type": "user",
                "uuid": "u1",
                "timestamp": "2026-05-01T10:00:00.000Z",
                "message": { "role": "user", "content": "q1" },
                "sessionId": "multi-turn",
                "cwd": "/tmp",
            }),
            json!({
                "type": "assistant",
                "uuid": "a1",
                "parentUuid": "u1",
                "timestamp": "2026-05-01T10:01:00.000Z",
                "message": {
                    "role": "assistant",
                    "content": [{"type": "text", "text": "answer1"}],
                    "model": "claude-sonnet-4",
                    "usage": {
                        "input_tokens": 100,
                        "output_tokens": 50,
                        "cache_creation_input_tokens": 0,
                        "cache_read_input_tokens": 0,
                        "total_tokens": 150,
                    },
                },
                "sessionId": "multi-turn",
                "cwd": "/tmp",
            }),
            json!({
                "type": "user",
                "uuid": "u2",
                "parentUuid": "a1",
                "timestamp": "2026-05-01T10:02:00.000Z",
                "message": { "role": "user", "content": "q2" },
                "sessionId": "multi-turn",
                "cwd": "/tmp",
            }),
            json!({
                "type": "assistant",
                "uuid": "a2",
                "parentUuid": "u2",
                "timestamp": "2026-05-01T10:03:00.000Z",
                "message": {
                    "role": "assistant",
                    "content": [{"type": "text", "text": "answer2"}],
                    "model": "claude-sonnet-4",
                    "usage": {
                        "input_tokens": 300,
                        "output_tokens": 120,
                        "cache_creation_input_tokens": 0,
                        "cache_read_input_tokens": 0,
                        "total_tokens": 420,
                    },
                },
                "sessionId": "multi-turn",
                "cwd": "/tmp",
            }),
        ],
    );

    let summaries = store.load_project_session_summaries().await.unwrap();
    let s = &summaries[0];
    assert_eq!(s.session_id, "multi-turn");
    assert_eq!(s.message_count, 4);
    assert_eq!(s.total_input_tokens, 400); // 100 + 300
    assert_eq!(s.total_output_tokens, 170); // 50 + 120
    assert_eq!(s.duration_secs, Some(180)); // 3 minutes
}
