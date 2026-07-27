//! Normalization helpers that make TypeScript and Rust transcript output
//! comparable. Volatile fields (timestamps, UUIDs, working directories) and
//! platform-specific path separators are collapsed to stable sentinels so a
//! golden fixture authored once stays valid across machines and runs.
//!
//! The strategy is value-aware: each JSONL line is parsed, walked, and
//! re-serialized. Object keys are sorted during the walk so that two
//! semantically equal objects normalize to identical bytes regardless of the
//! key order they were written in. Sorting is done explicitly (not by relying
//! on the `serde_json` map backing) because the workspace enables
//! `serde_json/preserve_order`, which keeps insertion order on serialization.

use serde_json::Value;

pub const TIMESTAMP_SENTINEL: &str = "<TS>";
pub const UUID_SENTINEL: &str = "<UUID>";
pub const CWD_SENTINEL: &str = "<CWD>";
pub const OPAQUE_SENTINEL: &str = "<OPAQUE>";
/// Sentinel for wall-clock duration fields in headless stream-json `result`
/// records (`duration_ms` / `duration_api_ms`), which vary every run.
pub const DURATION_SENTINEL: &str = "<DUR>";
/// Sentinel for the `claude_code_version` reported in the stream-json
/// `system/init` record, so goldens survive version bumps.
pub const VERSION_SENTINEL: &str = "<VERSION>";
/// Sentinel for the monotonic `sequence` counter in stream-json records,
/// which increments every run and so must be folded for golden stability.
pub const SEQUENCE_SENTINEL: &str = "<SEQ>";
/// Sentinel for the `apiKeySource` field in the stream-json `system/init`
/// record. The value domain differs between the TS CLI (`"api_key"`) and
/// the Rust CLI (`"user"`); folding it keeps the golden implementation-neutral.
pub const API_KEY_SOURCE_SENTINEL: &str = "<API_KEY_SOURCE>";
/// Sentinel for `model` string values and `modelUsage` object keys in
/// stream-json records. The TS CLI reports the full API model id (e.g.
/// `"claude-sonnet-4-6-20250514"`) while the Rust CLI uses a display name
/// (e.g. `"Sonnet(anthropic)"`); folding keeps goldens comparable.
pub const MODEL_SENTINEL: &str = "<MODEL>";
/// Sentinel for the `tools` array in the stream-json `system/init` record.
/// Tool names differ between implementations (TS uses `"Bash"` / `"Read"`,
/// Rust uses `"bash"` / `"file-read"`); the whole list is folded.
pub const TOOLS_SENTINEL: &str = "<TOOLS>";

/// Object keys whose value is a wall-clock duration in milliseconds and so must
/// fold to [`DURATION_SENTINEL`] for stream-json comparisons.
const DURATION_KEYS: &[&str] = &["duration_ms", "duration_api_ms"];

/// Object key carrying the running CLI version in the stream-json init record.
const VERSION_KEY: &str = "claude_code_version";

/// Object keys whose string value is a working directory or project path. Their
/// value is replaced wholesale with [`CWD_SENTINEL`] so absolute paths captured
/// on one machine do not leak into the golden comparison.
const CWD_KEYS: &[&str] = &[
    "cwd",
    "projectPath",
    "project_path",
    "worktreePath",
    "originalCwd",
];

/// Object keys whose numeric value is a computed cost in USD. These depend on
/// whether the provider model is recognized by the pricing table and so must be
/// folded for golden stability.
const COST_KEYS: &[&str] = &["total_cost_usd", "costUSD"];

/// Object keys that are implementation-specific and should be dropped from
/// stream-json output during normalization. These are fields present in one
/// implementation (TS or Rust) but absent in the other, or whose value domains
/// are incompatible (e.g. `service_tier`: TS emits `"standard"`, Rust emits
/// `null`). Dropping them produces an implementation-neutral golden.
const STREAM_JSON_DROP_KEYS: &[&str] = &[
    // TS-only init fields
    "agents",
    "analytics_disabled",
    "product_feedback_disabled",
    "fast_mode_state",
    // TS-only message fields
    "stop_details",
    "context_management",
    // TS-only result fields
    "api_error_status",
    "ttft_ms",
    "terminal_reason",
    // TS-only usage / streaming fields
    "cache_creation",
    "inference_geo",
    "iterations",
    "speed",
    "output_tokens_details",
    // TS SDK content_block_delta carries an `index` RS does not emit
    "index",
    // RS-only result fields
    "pricing_known",
    // service_tier: TS emits "standard", RS emits null — value incompatible
    "service_tier",
];

/// Object keys whose string value is an opaque, run-specific blob. Thinking
/// signatures are always opaque; `data` is folded only when it has the long
/// base64-like shape used by redacted thinking and inline image sources.
const OPAQUE_KEYS: &[&str] = &["signature"];

/// Normalize a full JSONL document. Lines that fail to parse as JSON are kept
/// verbatim so the function can be applied to deliberately corrupt fixtures
/// without panicking; only valid JSON lines are rewritten. A single trailing
/// newline is preserved when present so byte comparisons stay faithful.
pub fn normalize_jsonl(input: &str) -> String {
    let trailing_newline = input.ends_with('\n');
    let mut out_lines = Vec::new();
    for line in input.split('\n') {
        if line.is_empty() {
            continue;
        }
        out_lines.push(normalize_line(line));
    }
    let mut joined = out_lines.join("\n");
    if trailing_newline && !joined.is_empty() {
        joined.push('\n');
    }
    joined
}

/// Normalize a single JSON line. Invalid JSON is returned unchanged.
pub fn normalize_line(line: &str) -> String {
    match serde_json::from_str::<Value>(line) {
        Ok(mut value) => {
            normalize_value(&mut value, None);
            serde_json::to_string(&value).unwrap_or_else(|_| line.to_string())
        }
        Err(_) => line.to_string(),
    }
}

/// Normalize an arbitrary JSON value in place. `key` is the object field name
/// the value was found under, when applicable, so working-directory fields can
/// be detected by name rather than by content.
pub fn normalize_value(value: &mut Value, key: Option<&str>) {
    // Canonicalize the two equivalent Anthropic content encodings before
    // recursing: a single text block array `[{"type":"text","text":X}]` and the
    // bare string `X` are identical to the API, but the TypeScript reference
    // emits the string form for plain-text messages while the Rust provider
    // encoder always emits the array form. Collapse the array to the string so
    // goldens authored from either side compare equal.
    if let Some(text) = single_text_block_string(value) {
        *value = Value::String(text);
    }

    match value {
        Value::String(text) => {
            if key.is_some_and(|key| CWD_KEYS.contains(&key)) {
                *text = CWD_SENTINEL.to_string();
                return;
            }
            if key.is_some_and(|key| should_fold_opaque_value(key, text)) {
                *text = OPAQUE_SENTINEL.to_string();
                return;
            }
            *text = normalize_string(text);
        }
        Value::Array(items) => {
            for item in items.iter_mut() {
                normalize_value(item, None);
            }
        }
        Value::Object(map) => {
            // Rebuild the map in sorted key order so the serialized bytes are
            // independent of the input key order even when `serde_json` is built
            // with `preserve_order` (insertion-order IndexMap backing).
            let sorted: std::collections::BTreeMap<String, Value> =
                std::mem::take(map).into_iter().collect();
            let mut rebuilt = serde_json::Map::new();
            for (child_key, mut child_value) in sorted {
                normalize_value(&mut child_value, Some(child_key.as_str()));
                rebuilt.insert(child_key, child_value);
            }
            *map = rebuilt;
        }
        _ => {}
    }
}

fn should_fold_opaque_value(key: &str, text: &str) -> bool {
    OPAQUE_KEYS.contains(&key) || (key == "data" && is_probably_opaque_data_blob(text))
}

fn is_probably_opaque_data_blob(text: &str) -> bool {
    text.len() >= 24
        && text.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'=' | b'_' | b'-')
        })
}

/// If `value` is an array holding exactly one object of the form
/// `{"type":"text","text":<string>}` (and no other keys), return the inner text.
/// Used to canonicalize the equivalent Anthropic message-content encodings.
fn single_text_block_string(value: &Value) -> Option<String> {
    let items = value.as_array()?;
    let [Value::Object(block)] = items.as_slice() else {
        return None;
    };
    if block.len() != 2 {
        return None;
    }
    if block.get("type").and_then(Value::as_str) != Some("text") {
        return None;
    }
    block
        .get("text")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Apply the scalar string transforms: whole-string timestamps collapse to
/// [`TIMESTAMP_SENTINEL`], embedded UUIDs collapse to [`UUID_SENTINEL`], and
/// Windows-style backslash separators become forward slashes.
pub fn normalize_string(text: &str) -> String {
    if is_iso_timestamp(text) {
        return TIMESTAMP_SENTINEL.to_string();
    }
    let replaced = replace_uuids(text);
    normalize_path_separators(&replaced)
}

/// Extended stream-json string normalization. Applies everything from
/// [`normalize_string`] plus patterns that appear in tool result content from
/// the stub provider: 32-char hex agent sub-session identifiers, temp
/// directory paths, and embedded `date=YYYY-MM-DD` stamps.
fn normalize_stream_json_string(text: &str) -> String {
    if is_iso_timestamp(text) {
        return TIMESTAMP_SENTINEL.to_string();
    }
    let replaced = replace_uuids(text);
    let replaced = replace_hex32(&replaced);
    let replaced = replace_temp_paths(&replaced);
    let replaced = replace_inline_dates(&replaced);
    normalize_path_separators(&replaced)
}

/// Replace 32-char lowercase hex runs preceded by `agent-` or `session-` with
/// the UUID sentinel so agent sub-session identifiers stabilize.
fn replace_hex32(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut index = 0;
    while index < bytes.len() {
        if let Some((prefix_end, hex_end)) = hex32_at(bytes, index) {
            out.push_str(&text[index..prefix_end]);
            out.push_str(UUID_SENTINEL);
            index = hex_end;
        } else {
            let ch = text[index..].chars().next().expect("char at index");
            out.push(ch);
            index += ch.len_utf8();
        }
    }
    out
}

fn hex32_at(bytes: &[u8], start: usize) -> Option<(usize, usize)> {
    for prefix in [b"agent-" as &[u8], b"session-" as &[u8]] {
        let prefix_len = prefix.len();
        if start + prefix_len + 32 > bytes.len() {
            continue;
        }
        if &bytes[start..start + prefix_len] != prefix {
            continue;
        }
        let hex_start = start + prefix_len;
        if bytes[hex_start..hex_start + 32]
            .iter()
            .all(u8::is_ascii_hexdigit)
        {
            let hex_end = hex_start + 32;
            if bytes.get(hex_end).is_some_and(u8::is_ascii_hexdigit) {
                continue;
            }
            return Some((hex_start, hex_end));
        }
    }
    None
}

/// Replace OS temp directory paths with `<TMPDIR>` so goldens are stable.
fn replace_temp_paths(text: &str) -> String {
    let mut result = text.to_string();
    for pattern in ["/private/var/folders/", "/tmp/.tmp", "/var/folders/"] {
        while let Some(start) = result.find(pattern) {
            let end = result[start..]
                .find(|c: char| c.is_whitespace() || c == '\n' || c == '"')
                .map_or(result.len(), |i| start + i);
            result.replace_range(start..end, "<TMPDIR>");
        }
    }
    result
}

/// Replace `date=YYYY-MM-DD` stamps with `date=<DATE>` for date stability.
fn replace_inline_dates(text: &str) -> String {
    let mut result = text.to_string();
    let pattern = "date=";
    let mut search_from = 0;
    while let Some(start) = result[search_from..].find(pattern) {
        let abs_start = search_from + start;
        let date_start = abs_start + pattern.len();
        if date_start + 10 <= result.len() {
            let candidate = &result[date_start..date_start + 10];
            if candidate.len() == 10
                && candidate.as_bytes()[4] == b'-'
                && candidate.as_bytes()[7] == b'-'
                && candidate.bytes().filter(u8::is_ascii_digit).count() == 8
            {
                result.replace_range(date_start..date_start + 10, "<DATE>");
                search_from = date_start + 6;
                continue;
            }
        }
        search_from = abs_start + 1;
    }
    result
}

/// Convert backslashes to forward slashes so Windows and POSIX paths compare
/// equal. Applied to every string value; strings without separators are
/// unaffected.
pub fn normalize_path_separators(text: &str) -> String {
    text.replace('\\', "/")
}

/// True when the entire string is an ISO-8601 / RFC-3339 timestamp such as
/// `2026-05-29T01:02:03.456Z` or `2026-05-29T01:02:03+00:00`.
pub fn is_iso_timestamp(text: &str) -> bool {
    let bytes = text.as_bytes();
    // Minimum shape: YYYY-MM-DDTHH:MM:SS -> 19 chars.
    if bytes.len() < 19 {
        return false;
    }
    let digit = |index: usize| bytes.get(index).is_some_and(u8::is_ascii_digit);
    let at = |index: usize, ch: u8| bytes.get(index) == Some(&ch);

    if !(digit(0) && digit(1) && digit(2) && digit(3) && at(4, b'-')) {
        return false;
    }
    if !(digit(5) && digit(6) && at(7, b'-')) {
        return false;
    }
    if !(digit(8) && digit(9) && at(10, b'T')) {
        return false;
    }
    if !(digit(11) && digit(12) && at(13, b':')) {
        return false;
    }
    if !(digit(14) && digit(15) && at(16, b':')) {
        return false;
    }
    if !(digit(17) && digit(18)) {
        return false;
    }

    let mut index = 19;
    // Optional fractional seconds: `.` followed by one or more digits.
    if at(index, b'.') {
        index += 1;
        let start = index;
        while digit(index) {
            index += 1;
        }
        if index == start {
            return false;
        }
    }

    // Timezone: `Z`, or `±HH:MM`, or `±HHMM`.
    match bytes.get(index) {
        Some(b'Z') => index + 1 == bytes.len(),
        Some(b'+' | b'-') => {
            let rest = &text[index + 1..];
            matches!(rest.len(), 4 | 5)
                && rest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || byte == b':')
        }
        None => true,
        _ => false,
    }
}

/// Replace every 8-4-4-4-12 hexadecimal UUID substring with
/// [`UUID_SENTINEL`], leaving the surrounding text intact. This catches both
/// bare UUID values and ids that embed a UUID (for example `orbcode-<uuid>`).
pub fn replace_uuids(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut index = 0;
    while index < bytes.len() {
        if let Some(end) = uuid_at(bytes, index) {
            out.push_str(UUID_SENTINEL);
            index = end;
        } else {
            // Push one UTF-8 character at a time to stay on char boundaries.
            let ch = text[index..].chars().next().expect("char at index");
            out.push(ch);
            index += ch.len_utf8();
        }
    }
    out
}

/// If a UUID begins at `start`, return the byte index just past it.
fn uuid_at(bytes: &[u8], start: usize) -> Option<usize> {
    const GROUPS: [usize; 5] = [8, 4, 4, 4, 12];
    let mut index = start;
    for (group_position, group_len) in GROUPS.iter().enumerate() {
        if group_position > 0 {
            if bytes.get(index) != Some(&b'-') {
                return None;
            }
            index += 1;
        }
        for _ in 0..*group_len {
            match bytes.get(index) {
                Some(byte) if byte.is_ascii_hexdigit() => index += 1,
                _ => return None,
            }
        }
    }
    // Reject if the next byte continues a longer hex/identifier run that would
    // make this a coincidental prefix rather than a standalone UUID.
    if bytes.get(index).is_some_and(u8::is_ascii_hexdigit) {
        return None;
    }
    Some(index)
}

// ---------------------------------------------------------------------------
// Headless stream-json normalization
// ---------------------------------------------------------------------------

/// Normalize a headless stream-json (NDJSON) document into a stable golden form.
///
/// Stream-json records carry the same volatile fields as transcripts (per-record
/// `uuid`, ISO `timestamp`, the `cwd` reported in `system/init`) plus two of
/// their own: the wall-clock `duration_ms` / `duration_api_ms` on the `result`
/// record and the `claude_code_version` on `system/init`. Those are collapsed to
/// [`DURATION_SENTINEL`] and [`VERSION_SENTINEL`] respectively; `uuid`,
/// `timestamp`, `cwd`, and Windows path separators fold exactly as they do for
/// transcripts.
///
/// `tool_progress` `stream_event` records are dropped entirely: they are emitted
/// asynchronously by running tools (status pings, byte-chunked stdout streaming)
/// and so are inherently timing-dependent, which would make a byte-for-byte
/// golden flake. Every other record is kept in stream order.
///
/// Object keys are sorted so the serialized bytes are independent of emission
/// key order. Unlike [`normalize_jsonl`], the equivalent single-text-block /
/// bare-string content encodings are *not* collapsed: the stream-json wire
/// contract is the SDK's array-of-content-blocks shape, and a regression that
/// changed it should be caught rather than normalized away.
pub fn normalize_stream_json(input: &str) -> String {
    let mut out_lines = Vec::new();
    for line in input.split('\n') {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(mut value) = serde_json::from_str::<Value>(line) else {
            // Stream-json stdout is always well-formed NDJSON; keep an
            // unparseable line verbatim rather than panic so a malformed record
            // surfaces in the diff instead of aborting the test.
            out_lines.push(line.to_string());
            continue;
        };
        if is_tool_progress_record(&value) || is_ts_lifecycle_record(&value) {
            continue;
        }
        normalize_stream_json_value(&mut value, None);
        out_lines.push(serde_json::to_string(&value).unwrap_or_else(|_| line.to_string()));
    }
    out_lines.join("\n")
}

/// True for a `stream_event` whose inner `event.type` is `tool_progress`.
fn is_tool_progress_record(value: &Value) -> bool {
    value.get("type").and_then(Value::as_str) == Some("stream_event")
        && value
            .get("event")
            .and_then(|event| event.get("type"))
            .and_then(Value::as_str)
            == Some("tool_progress")
}

/// True for TS SDK streaming lifecycle events that the Rust CLI does not emit.
/// These are dropped so stream-json goldens contain only the shared record types.
fn is_ts_lifecycle_record(value: &Value) -> bool {
    let record_type = value.get("type").and_then(Value::as_str);
    if record_type == Some("system") {
        return value.get("subtype").and_then(Value::as_str) == Some("status");
    }
    if record_type != Some("stream_event") {
        return false;
    }
    let event_type = value
        .get("event")
        .and_then(|event| event.get("type"))
        .and_then(Value::as_str)
        .unwrap_or("");
    matches!(
        event_type,
        "message_start"
            | "content_block_start"
            | "content_block_stop"
            | "message_delta"
            | "message_stop"
    )
}

/// In-place stream-json value normalization. Mirrors [`normalize_value`] for the
/// shared volatile fields but additionally folds the duration and version keys
/// and never collapses single-text-block content arrays.
fn normalize_stream_json_value(value: &mut Value, key: Option<&str>) {
    match value {
        Value::String(text) => {
            if key.is_some_and(|key| CWD_KEYS.contains(&key)) {
                *text = CWD_SENTINEL.to_string();
                return;
            }
            *text = normalize_stream_json_string(text);
        }
        Value::Array(items) => {
            for item in items.iter_mut() {
                normalize_stream_json_value(item, None);
            }
        }
        Value::Object(map) => {
            let sorted: std::collections::BTreeMap<String, Value> =
                std::mem::take(map).into_iter().collect();
            let mut rebuilt = serde_json::Map::new();
            for (child_key, mut child_value) in sorted {
                if child_key == "output_style"
                    || child_key == "outputStyle"
                    || STREAM_JSON_DROP_KEYS.contains(&child_key.as_str())
                {
                    continue;
                }
                if DURATION_KEYS.contains(&child_key.as_str()) {
                    child_value = Value::String(DURATION_SENTINEL.to_string());
                } else if child_key == VERSION_KEY {
                    child_value = Value::String(VERSION_SENTINEL.to_string());
                } else if child_key == "sequence" {
                    child_value = Value::String(SEQUENCE_SENTINEL.to_string());
                } else if COST_KEYS.contains(&child_key.as_str()) {
                    child_value =
                        Value::Number(serde_json::Number::from_f64(0.0).expect("0.0 is valid"));
                } else if child_key == "apiKeySource" {
                    child_value = Value::String(API_KEY_SOURCE_SENTINEL.to_string());
                } else if child_key == "model" && child_value.is_string() {
                    child_value = Value::String(MODEL_SENTINEL.to_string());
                } else if child_key == "tools" && child_value.is_array() {
                    child_value = Value::String(TOOLS_SENTINEL.to_string());
                } else if child_key == "modelUsage" {
                    normalize_model_usage_keys(&mut child_value);
                } else {
                    normalize_stream_json_value(&mut child_value, Some(child_key.as_str()));
                }
                rebuilt.insert(child_key, child_value);
            }
            *map = rebuilt;
        }
        _ => {}
    }
}

/// Fold `modelUsage` object keys (model ids) to [`MODEL_SENTINEL`] so goldens
/// are implementation-neutral. When multiple models are present, keys are
/// suffixed (`<MODEL>_1`, `<MODEL>_2`, …) to avoid object-key collisions.
fn normalize_model_usage_keys(value: &mut Value) {
    if let Value::Object(map) = value {
        let sorted: std::collections::BTreeMap<String, Value> =
            std::mem::take(map).into_iter().collect();
        let mut rebuilt = serde_json::Map::new();
        for (index, (_, mut model_value)) in sorted.into_iter().enumerate() {
            let key = if index == 0 {
                MODEL_SENTINEL.to_string()
            } else {
                format!("{MODEL_SENTINEL}_{index}")
            };
            normalize_stream_json_value(&mut model_value, None);
            rebuilt.insert(key, model_value);
        }
        *map = rebuilt;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn compat_normalize_collapses_timestamp_uuid_and_cwd() {
        let line = json!({
            "type": "user",
            "uuid": "11111111-2222-3333-4444-555555555555",
            "parentUuid": null,
            "timestamp": "2026-05-29T01:02:03.456Z",
            "cwd": "/Users/dev/project",
            "message": { "role": "user", "content": "hi" }
        })
        .to_string();

        let normalized = normalize_line(&line);
        let value: Value = serde_json::from_str(&normalized).expect("normalized json");
        assert_eq!(value["uuid"], json!(UUID_SENTINEL));
        assert_eq!(value["timestamp"], json!(TIMESTAMP_SENTINEL));
        assert_eq!(value["cwd"], json!(CWD_SENTINEL));
        assert_eq!(value["parentUuid"], Value::Null);
        assert_eq!(value["message"]["content"], json!("hi"));
    }

    #[test]
    fn compat_normalize_is_key_order_independent() {
        let a = json!({ "a": 1, "b": 2, "c": 3 }).to_string();
        let b = "{\"c\":3,\"b\":2,\"a\":1}".to_string();
        assert_eq!(normalize_line(&a), normalize_line(&b));
    }

    #[test]
    fn compat_normalize_rewrites_windows_paths_and_embedded_uuid() {
        let value = normalize_string("orbcode-11111111-2222-3333-4444-555555555555");
        assert_eq!(value, format!("orbcode-{UUID_SENTINEL}"));
        assert_eq!(
            normalize_path_separators("C:\\Users\\dev\\file.rs"),
            "C:/Users/dev/file.rs"
        );
    }

    #[test]
    fn compat_normalize_preserves_invalid_lines_and_trailing_newline() {
        let input = "{\"a\":1}\nnot json\n";
        let normalized = normalize_jsonl(input);
        assert_eq!(normalized, "{\"a\":1}\nnot json\n");
    }

    #[test]
    fn compat_iso_timestamp_detection_matches_real_shapes() {
        assert!(is_iso_timestamp("2026-05-29T01:02:03Z"));
        assert!(is_iso_timestamp("2026-05-29T01:02:03.456Z"));
        assert!(is_iso_timestamp("2026-05-29T01:02:03+00:00"));
        assert!(!is_iso_timestamp("2026-05-29"));
        assert!(!is_iso_timestamp("hello world"));
        assert!(!is_iso_timestamp("2026-05-29T01:02:03.Z"));
    }

    #[test]
    fn compat_normalize_collapses_single_text_block_to_string() {
        // The Rust provider encoder emits array content; the TS reference emits
        // a bare string for the same plain-text message. Both must normalize to
        // the same bytes.
        let array_form = json!({ "role": "user", "content": [ { "type": "text", "text": "hi" } ] });
        let string_form = json!({ "role": "user", "content": "hi" });
        assert_eq!(
            normalize_line(&array_form.to_string()),
            normalize_line(&string_form.to_string())
        );

        // A multi-key block (e.g. with cache_control) or a non-text block must
        // not be collapsed.
        let with_cache =
            json!([{ "type": "text", "text": "hi", "cache_control": { "type": "ephemeral" } }]);
        assert!(super::single_text_block_string(&with_cache).is_none());
        let tool_result = json!([{ "type": "tool_result", "tool_use_id": "t1", "content": "ok" }]);
        assert!(super::single_text_block_string(&tool_result).is_none());
    }

    #[test]
    fn compat_uuid_detection_rejects_longer_hex_runs() {
        // 13-char final group must not be treated as a UUID.
        let almost = "11111111-2222-3333-4444-5555555555556";
        assert_eq!(replace_uuids(almost), almost);
    }

    #[test]
    fn compat_normalize_folds_opaque_signature_and_data_blobs() {
        // A `redacted_thinking` block carries an opaque `data` blob and a
        // `thinking` block carries a `signature`; both vary run to run and must
        // collapse to the opaque sentinel. An inline image `source.data` is the
        // same case. Distinct blobs normalize to identical bytes.
        let a = json!({ "content": [
            { "type": "thinking", "thinking": "step", "signature": "sig-AAA" },
            { "type": "redacted_thinking", "data": "QUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFB" },
            { "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": "QUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFB" } },
        ]})
        .to_string();
        let b = json!({ "content": [
            { "type": "thinking", "thinking": "step", "signature": "sig-ZZZ" },
            { "type": "redacted_thinking", "data": "WlpaWlpaWlpaWlpaWlpaWlpaWlpaWlpa" },
            { "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": "WlpaWlpaWlpaWlpaWlpaWlpaWlpaWlpa" } },
        ]})
        .to_string();
        assert_eq!(normalize_line(&a), normalize_line(&b));

        let normalized: Value = serde_json::from_str(&normalize_line(&a)).expect("json");
        let blocks = normalized["content"].as_array().expect("content array");
        assert_eq!(blocks[0]["signature"], json!(OPAQUE_SENTINEL));
        assert_eq!(blocks[1]["data"], json!(OPAQUE_SENTINEL));
        assert_eq!(blocks[2]["source"]["data"], json!(OPAQUE_SENTINEL));
        // The non-opaque siblings are left intact.
        assert_eq!(blocks[2]["source"]["media_type"], json!("image/png"));

        let meaningful_data = json!({ "data": "manual" }).to_string();
        assert_eq!(
            serde_json::from_str::<Value>(&normalize_line(&meaningful_data)).expect("json")["data"],
            json!("manual")
        );
    }

    /// Resolve a fixture path under the bundled `fixtures/transcripts` dir.
    fn transcript_fixture(name: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/transcripts")
            .join(name);
        std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read fixture {}: {e}", path.display()))
    }

    #[test]
    fn compat_normalize_is_idempotent_and_stable_on_new_fixtures() {
        // Each new fixture must fold its volatile fields (timestamps, UUIDs,
        // cwd, opaque blobs) and stay byte-stable under a second pass.
        for name in [
            "redacted_thinking.jsonl",
            "attachment.jsonl",
            "local_command_output.jsonl",
            "compact_boundary_multiturn.jsonl",
            "system_subtypes_and_context.jsonl",
        ] {
            let raw = transcript_fixture(name);
            let once = normalize_jsonl(&raw);
            let twice = normalize_jsonl(&once);
            assert_eq!(once, twice, "{name}: normalization should be idempotent");

            // No raw ISO timestamp, UUID, or sanitized cwd path survives the fold.
            assert!(
                !once.contains("2026-05-"),
                "{name}: raw timestamps should be folded to {TIMESTAMP_SENTINEL}"
            );
            // The cwd *key's* value must fold; absolute paths under other keys
            // (file_path, filePath, attachment path) are content and stay.
            assert!(
                !once.contains("\"cwd\":\"/Users"),
                "{name}: cwd-keyed paths should be folded to {CWD_SENTINEL}"
            );
            assert!(
                once.contains(TIMESTAMP_SENTINEL),
                "{name}: expected at least one folded timestamp"
            );
            assert!(
                once.contains(CWD_SENTINEL),
                "{name}: expected at least one folded cwd"
            );
            // The session id (a UUID) appears on every record, so the UUID
            // sentinel must be present after folding.
            assert!(
                once.contains(UUID_SENTINEL),
                "{name}: expected at least one folded UUID"
            );
        }
    }

    #[test]
    fn compat_normalize_folds_redacted_thinking_and_image_blobs_in_fixtures() {
        // The redacted_thinking and attachment fixtures carry the opaque blobs;
        // after normalization the raw blob bytes must be gone.
        let redacted = normalize_jsonl(&transcript_fixture("redacted_thinking.jsonl"));
        assert!(
            !redacted.contains("EvAFCkYIBRgC"),
            "redacted_thinking data blob should be folded to {OPAQUE_SENTINEL}"
        );
        assert!(redacted.contains(OPAQUE_SENTINEL));

        let attachment = normalize_jsonl(&transcript_fixture("attachment.jsonl"));
        assert!(
            !attachment.contains("iVBORw0KGgo"),
            "inline image base64 data should be folded to {OPAQUE_SENTINEL}"
        );
        assert!(attachment.contains(OPAQUE_SENTINEL));
    }

    #[test]
    fn compat_normalize_rewrites_new_fixture_path_shapes() {
        let line = json!({
            "cwd": "C:\\Users\\dev\\project",
            "message": {
                "content": [
                    {
                        "type": "tool_use",
                        "input": { "file_path": "C:\\Users\\dev\\project\\STATUS.md" }
                    }
                ]
            },
            "toolUseResult": {
                "file": { "filePath": "C:\\Users\\dev\\project\\STATUS.md" }
            },
            "attachment": {
                "path": "C:\\Users\\dev\\project\\screenshot.png"
            }
        })
        .to_string();

        let normalized = normalize_line(&line);
        let value: Value = serde_json::from_str(&normalized).expect("normalized json");
        assert_eq!(value["cwd"], json!(CWD_SENTINEL));
        assert_eq!(
            value["message"]["content"][0]["input"]["file_path"],
            json!("C:/Users/dev/project/STATUS.md")
        );
        assert_eq!(
            value["toolUseResult"]["file"]["filePath"],
            json!("C:/Users/dev/project/STATUS.md")
        );
        assert_eq!(
            value["attachment"]["path"],
            json!("C:/Users/dev/project/screenshot.png")
        );
    }

    // -----------------------------------------------------------------------
    // Stream-json normalization
    // -----------------------------------------------------------------------

    #[test]
    fn compat_stream_json_folds_uuid_timestamp_cwd_duration_and_version() {
        let init = json!({
            "type": "system",
            "subtype": "init",
            "uuid": "11111111-2222-3333-4444-555555555555",
            "session_id": "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
            "cwd": "/private/var/folders/tl/tmp.AbCdEf",
            "claude_code_version": "0.1.0",
            "timestamp": "2026-05-31T05:27:42.392236+00:00",
        });
        let result = json!({
            "type": "result",
            "uuid": "99999999-8888-7777-6666-555555555555",
            "session_id": "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
            "duration_ms": 289,
            "duration_api_ms": 43,
            "timestamp": "2026-05-31T05:27:42.437404+00:00",
        });
        let input = format!("{init}\n{result}\n");

        let normalized = normalize_stream_json(&input);
        let lines: Vec<Value> = normalized
            .lines()
            .map(|line| serde_json::from_str(line).expect("normalized line is json"))
            .collect();

        assert_eq!(lines[0]["uuid"], json!(UUID_SENTINEL));
        assert_eq!(lines[0]["session_id"], json!(UUID_SENTINEL));
        assert_eq!(lines[0]["cwd"], json!(CWD_SENTINEL));
        assert_eq!(lines[0]["claude_code_version"], json!(VERSION_SENTINEL));
        assert_eq!(lines[0]["timestamp"], json!(TIMESTAMP_SENTINEL));
        assert_eq!(lines[1]["duration_ms"], json!(DURATION_SENTINEL));
        assert_eq!(lines[1]["duration_api_ms"], json!(DURATION_SENTINEL));
    }

    #[test]
    fn compat_stream_json_drops_tool_progress_events_only() {
        let started = json!({
            "type": "stream_event",
            "event": { "type": "tool_use_started", "tool_use_id": "toolu-1", "tool_name": "bash" },
        });
        let progress = json!({
            "type": "stream_event",
            "event": { "type": "tool_progress", "tool_use_id": "toolu-1", "progress": { "bytes": 3 } },
        });
        let completed = json!({
            "type": "stream_event",
            "event": { "type": "tool_use_completed", "tool_use_id": "toolu-1", "kind": "success" },
        });
        let input = format!("{started}\n{progress}\n{completed}\n");

        let normalized = normalize_stream_json(&input);
        let kinds: Vec<String> = normalized
            .lines()
            .map(|line| {
                serde_json::from_str::<Value>(line).expect("json")["event"]["type"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect();
        assert_eq!(kinds, vec!["tool_use_started", "tool_use_completed"]);
    }

    #[test]
    fn compat_stream_json_preserves_single_text_block_array() {
        // Unlike `normalize_jsonl`, the stream-json normalizer must NOT collapse
        // a lone text block to a bare string: the SDK wire contract is the
        // array-of-blocks shape and a regression away from it should be caught.
        let assistant = json!({
            "type": "assistant",
            "message": { "role": "assistant", "content": [{ "type": "text", "text": "hi" }] },
        })
        .to_string();
        let normalized = normalize_stream_json(&assistant);
        let value: Value = serde_json::from_str(&normalized).expect("json");
        assert!(
            value["message"]["content"].is_array(),
            "content must stay an array: {value}"
        );
        assert_eq!(value["message"]["content"][0]["text"], json!("hi"));
    }

    #[test]
    fn compat_stream_json_is_idempotent_and_key_order_independent() {
        let a = "{\"type\":\"result\",\"duration_ms\":5,\"uuid\":\"11111111-2222-3333-4444-555555555555\"}";
        let b = "{\"uuid\":\"99999999-8888-7777-6666-555555555555\",\"duration_ms\":900,\"type\":\"result\"}";
        // Different volatile values + different key order normalize identically.
        assert_eq!(normalize_stream_json(a), normalize_stream_json(b));
        // And a second pass is a no-op.
        let once = normalize_stream_json(a);
        assert_eq!(normalize_stream_json(&once), once);
    }

    #[test]
    fn compat_stream_json_folds_api_key_source_and_model() {
        let init = json!({
            "type": "system",
            "subtype": "init",
            "apiKeySource": "api_key",
            "model": "claude-sonnet-4-6-20250514",
        })
        .to_string();
        let normalized = normalize_stream_json(&init);
        let value: Value = serde_json::from_str(&normalized).expect("json");
        assert_eq!(value["apiKeySource"], json!(API_KEY_SOURCE_SENTINEL));
        assert_eq!(value["model"], json!(MODEL_SENTINEL));

        let rust_init = json!({
            "type": "system",
            "subtype": "init",
            "apiKeySource": "user",
            "model": "Sonnet(anthropic)",
        })
        .to_string();
        assert_eq!(
            normalize_stream_json(&init),
            normalize_stream_json(&rust_init),
            "TS and Rust apiKeySource/model values should normalize identically"
        );
    }

    #[test]
    fn compat_stream_json_folds_tools_array() {
        let init = json!({
            "type": "system",
            "tools": ["Agent", "Bash", "Read", "Edit"],
        })
        .to_string();
        let normalized = normalize_stream_json(&init);
        let value: Value = serde_json::from_str(&normalized).expect("json");
        assert_eq!(value["tools"], json!(TOOLS_SENTINEL));

        let rust_init = json!({
            "type": "system",
            "tools": ["Agent", "bash", "file-read", "file-edit"],
        })
        .to_string();
        assert_eq!(
            normalize_stream_json(&init),
            normalize_stream_json(&rust_init),
            "different tool name lists should normalize identically"
        );
    }

    #[test]
    fn compat_stream_json_drops_output_style() {
        let with = json!({
            "type": "system",
            "output_style": "default",
            "subtype": "init",
        })
        .to_string();
        let without = json!({
            "type": "system",
            "subtype": "init",
        })
        .to_string();
        let normalized = normalize_stream_json(&with);
        let value: Value = serde_json::from_str(&normalized).expect("json");
        assert!(
            value.get("output_style").is_none(),
            "output_style should be dropped: {value}"
        );
        assert_eq!(
            normalize_stream_json(&with),
            normalize_stream_json(&without),
            "presence/absence of output_style should not affect normalized output"
        );
    }

    #[test]
    fn compat_stream_json_normalizes_model_usage_keys() {
        let ts_result = json!({
            "type": "result",
            "modelUsage": {
                "claude-sonnet-4-6-20250514": {
                    "inputTokens": 10,
                    "outputTokens": 5,
                    "costUSD": 0.001,
                }
            },
        })
        .to_string();
        let rs_result = json!({
            "type": "result",
            "modelUsage": {
                "claude-sonnet-4-6": {
                    "inputTokens": 10,
                    "outputTokens": 5,
                    "costUSD": 0.002,
                }
            },
        })
        .to_string();
        let ts_norm = normalize_stream_json(&ts_result);
        let rs_norm = normalize_stream_json(&rs_result);
        assert_eq!(
            ts_norm, rs_norm,
            "different model id keys and cost values should normalize identically"
        );

        let value: Value = serde_json::from_str(&ts_norm).expect("json");
        assert!(value["modelUsage"][MODEL_SENTINEL].is_object());
        assert_eq!(value["modelUsage"][MODEL_SENTINEL]["costUSD"], json!(0.0));
    }

    // -----------------------------------------------------------------------
    // TS ↔ RS live diff tests
    // -----------------------------------------------------------------------

    fn stream_json_fixture(name: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/stream_json")
            .join(name);
        std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read fixture {}: {e}", path.display()))
    }

    #[test]
    fn compat_ts_rs_simple_text_normalized_bytes_match() {
        let rs = stream_json_fixture("simple_text.jsonl");
        let ts = stream_json_fixture("simple_text.ts.golden.jsonl");
        let rs_norm = normalize_stream_json(&rs);
        let ts_norm = normalize_stream_json(&ts);
        assert_eq!(
            ts_norm, rs_norm,
            "normalized TS and RS simple_text goldens should be byte-equal"
        );
    }

    #[test]
    fn compat_ts_rs_tool_round_trip_normalized_bytes_match() {
        let rs = stream_json_fixture("tool_round_trip.jsonl");
        let ts = stream_json_fixture("tool_round_trip.ts.golden.jsonl");
        let rs_norm = normalize_stream_json(&rs);
        let ts_norm = normalize_stream_json(&ts);
        assert_eq!(
            ts_norm, rs_norm,
            "normalized TS and RS tool_round_trip goldens should be byte-equal"
        );
    }

    #[test]
    fn compat_ts_golden_normalization_is_idempotent() {
        for name in [
            "simple_text.ts.golden.jsonl",
            "tool_round_trip.ts.golden.jsonl",
        ] {
            let raw = stream_json_fixture(name);
            let once = normalize_stream_json(&raw);
            let twice = normalize_stream_json(&once);
            assert_eq!(
                once, twice,
                "{name}: TS golden normalization should be idempotent"
            );
        }
    }
}
