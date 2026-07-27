//! Shared TypeScript-vs-Rust golden compatibility fixtures.
//!
//! This crate bundles the fixture corpus (`fixtures/`) used to prove the Rust
//! port reads and reproduces the TypeScript CLI's on-disk and on-wire formats,
//! plus the normalization utilities that make those comparisons stable across
//! machines. The assertions that consume these fixtures live in the crates that
//! own the behavior under test (for example
//! `orbcode-session-store/tests/compat_transcripts.rs`).

pub mod fixtures;
pub mod normalize;

pub use fixtures::{
    Fixture, FixtureCategory, category_dir, fixtures_root, load_category, load_dir, load_named,
};
pub use normalize::{
    API_KEY_SOURCE_SENTINEL, CWD_SENTINEL, DURATION_SENTINEL, MODEL_SENTINEL, OPAQUE_SENTINEL,
    SEQUENCE_SENTINEL, TIMESTAMP_SENTINEL, TOOLS_SENTINEL, UUID_SENTINEL, VERSION_SENTINEL,
    is_iso_timestamp, normalize_jsonl, normalize_line, normalize_path_separators,
    normalize_stream_json, normalize_string, normalize_value, replace_uuids,
};
