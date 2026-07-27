use std::ffi::OsStr;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::{SecondsFormat, Utc};
use ratatui::layout::{Rect, Size};
use serde_json::{Value, json};

const TERMINAL_TRACE_ENV: &str = "ORBCODE_TUI_TERMINAL_TRACE";
const MAX_CAPTURE_BYTES: usize = 32 * 1024;

static TRACE_PATH: OnceLock<Option<PathBuf>> = OnceLock::new();
static TRACE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub(crate) fn enabled() -> bool {
    trace_path().is_some()
}

pub(crate) fn record_event(kind: &str, metadata: Value) {
    append_trace(json!({
        "type": "tui_terminal_trace",
        "kind": kind,
        "metadata": metadata,
    }));
}

pub(crate) fn record_bytes(kind: &str, metadata: Value, bytes: &[u8]) {
    let capture_len = bytes.len().min(MAX_CAPTURE_BYTES);
    append_trace(json!({
        "type": "tui_terminal_trace",
        "kind": kind,
        "metadata": metadata,
        "bytes": {
            "len": bytes.len(),
            "captured_len": capture_len,
            "truncated": bytes.len() > capture_len,
            "contains_viewport_chrome": contains_viewport_chrome(bytes),
            "ansi": escaped_bytes(&bytes[..capture_len]),
        },
    }));
}

pub(crate) fn rect(area: Rect) -> Value {
    json!({
        "x": area.x,
        "y": area.y,
        "width": area.width,
        "height": area.height,
        "top": area.top(),
        "bottom": area.bottom(),
    })
}

pub(crate) fn size(size: Size) -> Value {
    json!({
        "width": size.width,
        "height": size.height,
    })
}

fn append_trace(mut payload: Value) {
    let Some(path) = trace_path() else {
        return;
    };
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
        && std::fs::create_dir_all(parent).is_err()
    {
        return;
    }
    let sequence = TRACE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    payload["sequence"] = json!(sequence);
    payload["timestamp"] = json!(Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true));

    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) else {
        return;
    };
    let _ = writeln!(file, "{payload}");
}

fn trace_path() -> Option<&'static PathBuf> {
    TRACE_PATH
        .get_or_init(|| {
            std::env::var_os(TERMINAL_TRACE_ENV)
                .as_deref()
                .and_then(trace_path_from_value)
        })
        .as_ref()
}

fn trace_path_from_value(value: &OsStr) -> Option<PathBuf> {
    if value.is_empty() {
        return None;
    }
    if value == OsStr::new("1") {
        return Some(std::env::temp_dir().join(format!(
            "orbcode-tui-terminal-trace-{}.jsonl",
            std::process::id()
        )));
    }
    Some(PathBuf::from(value))
}

fn contains_viewport_chrome(bytes: &[u8]) -> bool {
    let text = String::from_utf8_lossy(bytes);
    text.contains("\n❯")
        || text.contains("\r\n❯")
        || text.contains("ctx:")
        || text.contains("INSERT")
        || text.contains("interrupt")
}

fn escaped_bytes(bytes: &[u8]) -> String {
    let mut escaped = String::new();
    for ch in String::from_utf8_lossy(bytes).chars() {
        match ch {
            '\x1b' => escaped.push_str("\\x1b"),
            '\r' => escaped.push_str("\\r"),
            '\n' => escaped.push_str("\\n"),
            '\t' => escaped.push_str("\\t"),
            ch if ch.is_control() => {
                use std::fmt::Write as _;
                let _ = write!(escaped, "\\u{{{:x}}}", ch as u32);
            }
            ch => escaped.push(ch),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escaped_bytes_makes_terminal_control_sequences_visible() {
        assert_eq!(escaped_bytes(b"\x1b[2J\r\n"), "\\x1b[2J\\r\\n");
    }

    #[test]
    fn chrome_detector_ignores_box_table_borders() {
        assert!(!contains_viewport_chrome(
            "┌────┬────┐\n│ 数据 │ 值 │\n└────┴────┘".as_bytes()
        ));
    }

    #[test]
    fn chrome_detector_matches_status_bar() {
        assert!(contains_viewport_chrome(
            "-- INSERT --  esc to interrupt ctx:20%".as_bytes()
        ));
    }
}
