use std::path::{Path, PathBuf};

use crate::ToolError;

pub(crate) const UTF8_BOM: &str = "\u{FEFF}";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LineEnding {
    Lf,
    CrLf,
    Cr,
}

pub(crate) fn detect_line_ending(content: &str) -> LineEnding {
    let bytes = content.as_bytes();
    let mut crlf = 0usize;
    let mut lf = 0usize;
    let mut cr = 0usize;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\r' {
            if i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
                crlf += 1;
                i += 2;
            } else {
                cr += 1;
                i += 1;
            }
        } else if bytes[i] == b'\n' {
            lf += 1;
            i += 1;
        } else {
            i += 1;
        }
    }
    if crlf > lf && crlf > cr {
        LineEnding::CrLf
    } else if cr > lf && cr > crlf {
        LineEnding::Cr
    } else {
        LineEnding::Lf
    }
}

pub(crate) fn convert_line_endings(text: &str, target: LineEnding) -> String {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    match target {
        LineEnding::Lf => normalized,
        LineEnding::CrLf => normalized.replace('\n', "\r\n"),
        LineEnding::Cr => normalized.replace('\n', "\r"),
    }
}

pub(crate) fn has_utf8_bom(content: &str) -> bool {
    content.starts_with(UTF8_BOM)
}

pub(crate) fn strip_utf8_bom(content: &str) -> &str {
    content.strip_prefix(UTF8_BOM).unwrap_or(content)
}

const MAX_FILE_READ_SIZE_BYTES: u64 = 256 * 1024;
const MAX_FILE_EDIT_SIZE_BYTES: u64 = 1024 * 1024 * 1024;
const DEFAULT_FILE_READ_MAX_OUTPUT_TOKENS: u32 = 25_000;
const BINARY_CHECK_SIZE: usize = 8192;

const BINARY_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "bmp", "ico", "webp", "tiff", "tif", "mp4", "mov", "avi", "mkv",
    "webm", "wmv", "flv", "m4v", "mpeg", "mpg", "mp3", "wav", "ogg", "flac", "aac", "m4a", "wma",
    "aiff", "opus", "zip", "tar", "gz", "bz2", "7z", "rar", "xz", "z", "tgz", "iso", "exe", "dll",
    "so", "dylib", "bin", "o", "a", "obj", "lib", "app", "msi", "deb", "rpm", "pdf", "doc", "docx",
    "xls", "xlsx", "ppt", "pptx", "odt", "ods", "odp", "ttf", "otf", "woff", "woff2", "eot", "pyc",
    "pyo", "class", "jar", "war", "ear", "node", "wasm", "rlib", "sqlite", "sqlite3", "db", "mdb",
    "idx", "psd", "ai", "eps", "sketch", "fig", "xd", "blend", "3ds", "max", "swf", "fla", "lockb",
    "dat", "data",
];

pub(crate) fn resolve_path(cwd: &Path, raw: &str) -> Result<PathBuf, ToolError> {
    let path = PathBuf::from(raw);
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(cwd.join(path))
    }
}

pub(crate) async fn create_parent_dir(path: &Path) -> Result<(), ToolError> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    Ok(())
}

pub(crate) fn slice_lines(
    contents: &str,
    start_line: Option<usize>,
    end_line: Option<usize>,
) -> String {
    let lines = source_lines(contents);
    if lines.is_empty() {
        return String::new();
    }
    let start = start_line.unwrap_or(1).saturating_sub(1).min(lines.len());
    let end = end_line.unwrap_or(lines.len()).min(lines.len());
    if start >= end {
        return String::new();
    }
    lines[start..end].concat()
}

pub(crate) fn source_lines(source: &str) -> Vec<String> {
    if source.is_empty() {
        return vec![String::new()];
    }
    let lines = source
        .split_inclusive('\n')
        .map(str::to_string)
        .collect::<Vec<_>>();
    if lines.is_empty() {
        vec![source.to_string()]
    } else {
        lines
    }
}

pub(crate) async fn validate_file_read_size(path: &Path) -> Result<(), ToolError> {
    let metadata = tokio::fs::metadata(path).await?;
    if metadata.len() <= MAX_FILE_READ_SIZE_BYTES {
        return Ok(());
    }
    Err(ToolError::ExecutionFailed(format!(
        "File content ({}) exceeds maximum allowed size ({}). Use offset and limit parameters to read specific portions of the file, or search for specific content instead of reading the whole file.",
        format_file_size(metadata.len()),
        format_file_size(MAX_FILE_READ_SIZE_BYTES)
    )))
}

pub(crate) async fn validate_file_edit_size(path: &Path) -> Result<(), ToolError> {
    let metadata = tokio::fs::metadata(path).await?;
    if metadata.len() <= MAX_FILE_EDIT_SIZE_BYTES {
        return Ok(());
    }
    Err(ToolError::ExecutionFailed(format!(
        "File is too large to edit ({}). Maximum editable file size is {}.",
        format_file_size(metadata.len()),
        format_file_size(MAX_FILE_EDIT_SIZE_BYTES)
    )))
}

pub(crate) async fn validate_text_file(path: &Path, action: &str) -> Result<(), ToolError> {
    if let Some(extension) = binary_extension(path) {
        return Err(binary_file_error(action, Some(&extension)));
    }

    let sample = read_file_prefix(path, BINARY_CHECK_SIZE).await?;
    if is_binary_content(&sample) {
        return Err(binary_file_error(action, None));
    }

    Ok(())
}

pub(crate) async fn read_text_file(path: &Path, action: &str) -> Result<String, ToolError> {
    match tokio::fs::read_to_string(path).await {
        Ok(contents) => Ok(contents),
        Err(error) if error.kind() == std::io::ErrorKind::InvalidData => {
            Err(binary_file_error(action, None))
        }
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn validate_file_read_tokens(content: &str, path: &Path) -> Result<(), ToolError> {
    let token_count = rough_file_read_token_estimate(content, path);
    if token_count <= DEFAULT_FILE_READ_MAX_OUTPUT_TOKENS {
        return Ok(());
    }
    Err(ToolError::ExecutionFailed(format!(
        "File content ({token_count} tokens) exceeds maximum allowed tokens ({DEFAULT_FILE_READ_MAX_OUTPUT_TOKENS}). Use offset and limit parameters to read specific portions of the file, or search for specific content instead of reading the whole file.",
    )))
}

fn rough_file_read_token_estimate(content: &str, path: &Path) -> u32 {
    let bytes_per_token = match path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("json" | "jsonl" | "jsonc") => 2,
        _ => 4,
    };
    (content.chars().count() as u32).saturating_add(bytes_per_token / 2) / bytes_per_token
}

fn binary_extension(path: &Path) -> Option<String> {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())?
        .to_ascii_lowercase();
    BINARY_EXTENSIONS
        .contains(&extension.as_str())
        .then(|| format!(".{extension}"))
}

fn binary_file_error(action: &str, extension: Option<&str>) -> ToolError {
    let appears = extension.map_or_else(
        || "The file appears to contain binary data.".to_string(),
        |extension| format!("The file appears to be a binary {extension} file."),
    );
    ToolError::ExecutionFailed(format!(
        "This tool cannot {action} binary files. {appears} Please use appropriate tools for binary file analysis."
    ))
}

async fn read_file_prefix(path: &Path, max_bytes: usize) -> Result<Vec<u8>, ToolError> {
    use tokio::io::AsyncReadExt;

    let mut file = tokio::fs::File::open(path).await?;
    let mut buffer = vec![0; max_bytes];
    let bytes_read = file.read(&mut buffer).await?;
    buffer.truncate(bytes_read);
    Ok(buffer)
}

fn is_binary_content(buffer: &[u8]) -> bool {
    let check_size = buffer.len().min(BINARY_CHECK_SIZE);
    if check_size == 0 {
        return false;
    }

    let mut non_printable = 0usize;
    for byte in buffer.iter().take(check_size).copied() {
        if byte == 0 {
            return true;
        }
        if byte < 32 && byte != b'\t' && byte != b'\n' && byte != b'\r' {
            non_printable += 1;
        }
    }

    non_printable * 10 > check_size
}

fn format_file_size(size_in_bytes: u64) -> String {
    let kb = size_in_bytes as f64 / 1024.0;
    if kb < 1.0 {
        return format!("{size_in_bytes} bytes");
    }
    if kb < 1024.0 {
        return format_size_unit(kb, "KB");
    }
    let mb = kb / 1024.0;
    if mb < 1024.0 {
        return format_size_unit(mb, "MB");
    }
    format_size_unit(mb / 1024.0, "GB")
}

fn format_size_unit(value: f64, unit: &str) -> String {
    let formatted = format!("{value:.1}");
    format!("{}{unit}", formatted.trim_end_matches(".0"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn generate_lines(count: usize) -> String {
        (0..count)
            .map(|i| format!("line {i:06}: stable content for slice performance testing\n"))
            .collect()
    }

    #[test]
    fn regression_budget_slice_lines_output_bounded_by_range() {
        let contents = generate_lines(10_000);
        let sliced = slice_lines(&contents, Some(501), Some(510));
        let sliced_lines: Vec<&str> = sliced.split_inclusive('\n').collect();

        assert_eq!(sliced_lines.len(), 10, "should return exactly 10 lines");
        assert!(
            sliced.contains("line 000500:"),
            "should include line 501 (0-indexed 500)"
        );
        assert!(
            !sliced.contains("line 000499:"),
            "should not include line 500"
        );
        assert!(
            !sliced.contains("line 000510:"),
            "should not include line 511"
        );
        assert!(
            sliced.len() < contents.len() / 100,
            "sliced output ({} bytes) should be much smaller than full input ({} bytes)",
            sliced.len(),
            contents.len()
        );
    }

    #[test]
    fn regression_budget_source_lines_round_trips_with_concat() {
        let contents = generate_lines(500);
        let lines = source_lines(&contents);
        let reconstructed: String = lines.concat();
        assert_eq!(reconstructed, contents);
    }

    #[test]
    fn regression_budget_slice_full_range_equals_source() {
        let contents = generate_lines(500);
        assert_eq!(slice_lines(&contents, None, None), contents);
        assert_eq!(slice_lines(&contents, Some(1), Some(500)), contents);
    }

    #[test]
    fn regression_budget_slice_empty_input() {
        assert_eq!(slice_lines("", None, None), "");
        assert_eq!(slice_lines("", Some(1), Some(10)), "");
    }

    #[test]
    fn regression_budget_source_lines_single_line_no_newline() {
        let lines = source_lines("hello");
        assert_eq!(lines, vec!["hello"]);
    }

    #[test]
    fn detect_line_ending_lf() {
        assert_eq!(detect_line_ending("a\nb\nc\n"), LineEnding::Lf);
    }

    #[test]
    fn detect_line_ending_crlf() {
        assert_eq!(detect_line_ending("a\r\nb\r\nc\r\n"), LineEnding::CrLf);
    }

    #[test]
    fn detect_line_ending_cr() {
        assert_eq!(detect_line_ending("a\rb\rc\r"), LineEnding::Cr);
    }

    #[test]
    fn detect_line_ending_defaults_to_lf_on_no_newlines() {
        assert_eq!(detect_line_ending("hello"), LineEnding::Lf);
    }

    #[test]
    fn detect_line_ending_mixed_majority_wins() {
        assert_eq!(detect_line_ending("a\r\nb\r\nc\nd\r\n"), LineEnding::CrLf);
    }

    #[test]
    fn convert_line_endings_lf_to_crlf() {
        assert_eq!(
            convert_line_endings("a\nb\n", LineEnding::CrLf),
            "a\r\nb\r\n"
        );
    }

    #[test]
    fn convert_line_endings_crlf_to_lf() {
        assert_eq!(convert_line_endings("a\r\nb\r\n", LineEnding::Lf), "a\nb\n");
    }

    #[test]
    fn convert_line_endings_mixed_to_crlf() {
        assert_eq!(
            convert_line_endings("a\nb\r\nc\rd\n", LineEnding::CrLf),
            "a\r\nb\r\nc\r\nd\r\n"
        );
    }

    #[test]
    fn convert_line_endings_lf_to_cr() {
        assert_eq!(convert_line_endings("a\nb\n", LineEnding::Cr), "a\rb\r");
    }

    #[test]
    fn convert_line_endings_no_newlines_unchanged() {
        assert_eq!(convert_line_endings("hello", LineEnding::CrLf), "hello");
    }

    #[test]
    fn has_utf8_bom_detects_bom() {
        assert!(has_utf8_bom("\u{FEFF}hello"));
    }

    #[test]
    fn has_utf8_bom_no_bom() {
        assert!(!has_utf8_bom("hello"));
    }

    #[test]
    fn strip_utf8_bom_removes_bom() {
        assert_eq!(strip_utf8_bom("\u{FEFF}hello"), "hello");
    }

    #[test]
    fn strip_utf8_bom_no_bom_unchanged() {
        assert_eq!(strip_utf8_bom("hello"), "hello");
    }

    #[test]
    #[ignore = "manual stress test for large file line slicing allocation"]
    fn slice_lines_stress_measures_allocation_for_large_files() {
        const LINE_COUNT: usize = 200_000;
        const SLICE_SIZE: usize = 50;
        const ITERATIONS: usize = 100;

        let contents = generate_lines(LINE_COUNT);
        let start_line = LINE_COUNT / 2;
        let end_line = start_line + SLICE_SIZE;

        let started = Instant::now();
        let mut output_bytes = 0;
        for _ in 0..ITERATIONS {
            let sliced = slice_lines(&contents, Some(start_line), Some(end_line));
            output_bytes = sliced.len();
        }
        let duration = started.elapsed();

        eprintln!(
            "lines={LINE_COUNT} slice_size={SLICE_SIZE} \
             input_bytes={} output_bytes={output_bytes} \
             iterations={ITERATIONS} total_us={} avg_us={}",
            contents.len(),
            duration.as_micros(),
            duration.as_micros() / ITERATIONS as u128
        );
    }
}
