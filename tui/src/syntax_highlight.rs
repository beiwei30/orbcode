use ratatui::style::Color;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::text::Span;
use std::sync::OnceLock;
use std::sync::RwLock;
use syntect::easy::HighlightLines;
use syntect::highlighting::Color as SyntectColor;
use syntect::highlighting::FontStyle;
use syntect::highlighting::Style as SyntectStyle;
use syntect::highlighting::Theme;
use syntect::parsing::SyntaxReference;
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;
use two_face::theme::EmbeddedThemeName;

static SYNTAX_SET: OnceLock<SyntaxSet> = OnceLock::new();
static THEME: OnceLock<RwLock<Theme>> = OnceLock::new();

const ANSI_ALPHA_INDEX: u8 = 0x00;
const ANSI_ALPHA_DEFAULT: u8 = 0x01;
const OPAQUE_ALPHA: u8 = 0xff;
const MAX_HIGHLIGHT_BYTES: usize = 512 * 1024;
const MAX_HIGHLIGHT_LINES: usize = 10_000;

fn syntax_set() -> &'static SyntaxSet {
    SYNTAX_SET.get_or_init(two_face::syntax::extra_newlines)
}

fn theme_lock() -> &'static RwLock<Theme> {
    THEME.get_or_init(|| {
        let themes = two_face::theme::extra();
        RwLock::new(themes.get(EmbeddedThemeName::Ansi).clone())
    })
}

pub(crate) fn exceeds_highlight_limits(total_bytes: usize, total_lines: usize) -> bool {
    total_bytes > MAX_HIGHLIGHT_BYTES || total_lines > MAX_HIGHLIGHT_LINES
}

pub(crate) fn highlight_code_to_styled_spans(
    code: &str,
    lang: &str,
) -> Option<Vec<Vec<Span<'static>>>> {
    if code.is_empty() || exceeds_highlight_limits(code.len(), code.lines().count()) {
        return None;
    }

    let syntax = find_syntax(lang)?;
    let theme = match theme_lock().read() {
        Ok(theme) => theme,
        Err(poisoned) => poisoned.into_inner(),
    };
    let mut highlighter = HighlightLines::new(syntax, &theme);
    let mut lines = Vec::new();

    for line in LinesWithEndings::from(code) {
        let ranges = highlighter.highlight_line(line, syntax_set()).ok()?;
        let mut spans = Vec::new();
        for (style, text) in ranges {
            let text = text.trim_end_matches(['\n', '\r']);
            if text.is_empty() {
                continue;
            }
            spans.push(Span::styled(text.to_string(), convert_style(style)));
        }
        if spans.is_empty() {
            spans.push(Span::raw(String::new()));
        }
        lines.push(spans);
    }

    Some(lines)
}

fn find_syntax(lang: &str) -> Option<&'static SyntaxReference> {
    let lang = lang.trim_start_matches('.').trim();
    if lang.is_empty() {
        return None;
    }

    let patched = match lang {
        "csharp" | "c-sharp" => "c#",
        "golang" => "go",
        "python3" => "python",
        "shell" => "bash",
        "yml" => "yaml",
        _ => lang,
    };

    let syntax_set = syntax_set();
    if let Some(syntax) = syntax_set.find_syntax_by_token(patched) {
        return Some(syntax);
    }
    if let Some(syntax) = syntax_set.find_syntax_by_name(patched) {
        return Some(syntax);
    }

    let lower = patched.to_ascii_lowercase();
    if let Some(syntax) = syntax_set
        .syntaxes()
        .iter()
        .find(|syntax| syntax.name.to_ascii_lowercase() == lower)
    {
        return Some(syntax);
    }

    syntax_set.find_syntax_by_extension(lang)
}

fn convert_style(syn_style: SyntectStyle) -> Style {
    let mut style = Style::default();
    if let Some(fg) = convert_syntect_color(syn_style.foreground) {
        style = style.fg(fg);
    }
    if syn_style.font_style.contains(FontStyle::BOLD) {
        style.add_modifier |= Modifier::BOLD;
    }
    style
}

fn convert_syntect_color(color: SyntectColor) -> Option<Color> {
    match color.a {
        ANSI_ALPHA_INDEX => Some(ansi_palette_color(color.r)),
        ANSI_ALPHA_DEFAULT => None,
        OPAQUE_ALPHA => Some(Color::Rgb(color.r, color.g, color.b)),
        _ => Some(Color::Rgb(color.r, color.g, color.b)),
    }
}

fn ansi_palette_color(index: u8) -> Color {
    match index {
        0x00 => Color::Black,
        0x01 => Color::Red,
        0x02 => Color::Green,
        0x03 => Color::Yellow,
        0x04 => Color::Blue,
        0x05 => Color::Magenta,
        0x06 => Color::Cyan,
        0x07 => Color::Gray,
        value => Color::Indexed(value),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlights_common_file_extensions() {
        for (extension, code) in [
            ("rs", "fn main() { println!(\"hi\"); }\n"),
            ("md", "# Title\n\n```rust\nfn main() {}\n```\n"),
            ("json", "{ \"name\": \"orbcode\", \"ok\": true }\n"),
            ("toml", "name = \"orbcode\"\n"),
            ("py", "def main():\n    print('hi')\n"),
            ("ts", "export const value: number = 1;\n"),
        ] {
            let lines = highlight_code_to_styled_spans(code, extension)
                .unwrap_or_else(|| panic!("{extension} should be highlighted"));
            assert!(!lines.is_empty(), "{extension} should produce lines");
        }
    }

    #[test]
    fn falls_back_for_unknown_or_large_inputs() {
        assert!(highlight_code_to_styled_spans("hello", "not-a-language").is_none());

        let many_lines = "x\n".repeat(MAX_HIGHLIGHT_LINES + 1);
        assert!(highlight_code_to_styled_spans(&many_lines, "rs").is_none());
    }
}
