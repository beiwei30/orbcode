use crate::tests::support::*;

#[test]
fn assistant_markdown_renders_headings_and_lists_without_raw_markers() {
    let rendered = render_text_block_lines(
        &MessageRole::Assistant,
        "# Title\n- item one\n1. item two\nplain **bold** `code`",
        80,
    );
    let lines = rendered
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>();

    assert!(lines.iter().any(|line| line.contains("Title")));
    assert!(lines.iter().any(|line| line.contains("• item one")));
    assert!(lines.iter().any(|line| line.contains("1. item two")));
    assert!(!lines.iter().any(|line| line.contains("# Title")));
}

#[test]
fn assistant_markdown_styles_bold_and_code_spans() {
    let rendered = render_text_block_lines(&MessageRole::Assistant, "plain **bold** `code`", 80);
    let spans = rendered
        .iter()
        .flat_map(|line| line.spans.iter())
        .collect::<Vec<_>>();

    assert!(
        spans
            .iter()
            .any(|span| span.content == "bold" && span.style.add_modifier.contains(Modifier::BOLD))
    );
    assert!(
        spans
            .iter()
            .any(|span| span.content == "code" && span.style.fg == Some(TOOL_BLUE))
    );
}

#[test]
fn assistant_markdown_renders_tables() {
    let rendered = render_text_block_lines(
        &MessageRole::Assistant,
        "| Area | Status |\n| --- | --- |\n| TUI | Partial |\n| Tools | Missing |",
        80,
    );
    let lines = rendered
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>();

    assert!(lines.iter().any(|line| line.contains('┌')));
    assert!(lines.iter().any(|line| line.contains("Area")));
    assert!(lines.iter().any(|line| line.contains("Partial")));
    assert!(lines.iter().any(|line| line.contains('┘')));
}

#[test]
fn assistant_markdown_tables_fit_narrow_width() {
    let rendered = render_text_block_lines(
        &MessageRole::Assistant,
        "| Command | TypeScript | Rust |\n| --- | --- | --- |\n| server / ssh / open / plugin / agents / auto-mode | ✅ | 未实现 |",
        36,
    );
    let widths = rendered
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .map(|line| display_width_str(&line))
        .collect::<Vec<_>>();

    assert!(widths.iter().all(|width| *width <= 36));
    assert!(rendered.iter().any(|line| {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>()
            .contains('│')
    }));
}

#[test]
fn assistant_markdown_tables_keep_aligned_display_width_with_cjk_content() {
    let rendered = render_text_block_lines(
        &MessageRole::Assistant,
        "| 能力 | 状态 | 说明 |\n| --- | --- | --- |\n| OpenAI/Gemini 实际调用 | Stub | generate() 返回固定 stub 响应 |\n| 多数 Tool | 缺失 | Agent、LSP、Plan Mode、Skill 等 40+ 工具 |",
        72,
    );
    let widths = rendered
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .filter(|line| line.contains('│') || line.contains('┌') || line.contains('└'))
        .map(|line| display_width_str(&line))
        .collect::<Vec<_>>();

    assert!(!widths.is_empty());
    assert!(widths.iter().all(|width| *width <= 72));
    assert!(widths.windows(2).all(|pair| pair[0] == pair[1]));
}

#[test]
fn assistant_markdown_tables_keep_aligned_display_width_with_inline_markdown() {
    let rendered = render_text_block_lines(
        &MessageRole::Assistant,
        "| Crate | 说明 |\n| --- | --- |\n| core | `session_manager.rs` 是核心，**turn loop**、Agent 与权限都在这里 |\n| tools | 支持 `bash`、`glob` 与 **web-fetch**，仍有大量工具待补齐 |",
        72,
    );
    let widths = rendered
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .filter(|line| line.contains('│') || line.contains('┌') || line.contains('└'))
        .map(|line| display_width_str(&line))
        .collect::<Vec<_>>();

    assert!(!widths.is_empty());
    assert!(widths.iter().all(|width| *width <= 72));
    assert!(widths.windows(2).all(|pair| pair[0] == pair[1]));
}
