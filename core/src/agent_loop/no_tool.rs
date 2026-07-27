use orbcode_protocol::{TranscriptBlock, TranscriptMessage};

const MAX_AUTO_CONTINUE_ATTEMPTS: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NoToolTurnDecision {
    AutoContinue(NoToolTurnReason),
    Finish(NoToolTurnReason),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NoToolTurnReason {
    AutoContinueLimitReached,
    ToolUseStopReason,
    NonRepoPrompt,
    ThinkingOnly,
    ThinPlanningReply,
    PlanningCue,
    MaxOutput,
    SubstantiveAnswer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) struct AssistantMessageShape {
    pub(crate) visible_text_chars: usize,
    pub(crate) thinking_chars: usize,
    pub(crate) visible_text_lines: usize,
    pub(crate) thinking_lines: usize,
    pub(crate) has_tool_blocks: bool,
    pub(crate) has_structured_formatting: bool,
    has_substantive_markers: bool,
}

#[cfg(test)]
pub(crate) fn should_auto_continue_without_tools(
    prompt: &str,
    message: &TranscriptMessage,
    stop_reason: Option<&str>,
    auto_continue_attempts: usize,
) -> bool {
    matches!(
        decide_no_tool_turn_action(prompt, message, stop_reason, auto_continue_attempts),
        NoToolTurnDecision::AutoContinue(_)
    )
}

#[cfg(test)]
pub(crate) fn should_auto_continue_without_tools_after_tool_use(
    prompt: &str,
    message: &TranscriptMessage,
    stop_reason: Option<&str>,
    auto_continue_attempts: usize,
) -> bool {
    matches!(
        decide_no_tool_turn_action(prompt, message, stop_reason, auto_continue_attempts),
        NoToolTurnDecision::AutoContinue(_)
    )
}

pub(crate) fn decide_no_tool_turn_action(
    prompt: &str,
    message: &TranscriptMessage,
    stop_reason: Option<&str>,
    auto_continue_attempts: usize,
) -> NoToolTurnDecision {
    if auto_continue_attempts >= MAX_AUTO_CONTINUE_ATTEMPTS {
        return NoToolTurnDecision::Finish(NoToolTurnReason::AutoContinueLimitReached);
    }

    if matches!(stop_reason, Some(reason) if reason.eq_ignore_ascii_case("tool_use")) {
        return NoToolTurnDecision::Finish(NoToolTurnReason::ToolUseStopReason);
    }

    if is_max_output_stop_reason(stop_reason) {
        return NoToolTurnDecision::AutoContinue(NoToolTurnReason::MaxOutput);
    }

    if !prompt_suggests_repo_or_code_work(prompt) {
        return NoToolTurnDecision::Finish(NoToolTurnReason::NonRepoPrompt);
    }

    let shape = assistant_message_shape(message);
    if shape.has_tool_blocks {
        return NoToolTurnDecision::Finish(NoToolTurnReason::ToolUseStopReason);
    }

    if shape.visible_text_chars == 0 && shape.thinking_chars > 0 {
        return NoToolTurnDecision::AutoContinue(NoToolTurnReason::ThinkingOnly);
    }

    if shape.visible_text_chars > 0
        && shape.thinking_chars > 0
        && shape.visible_text_chars <= 200
        && shape.visible_text_lines <= 3
        && !shape.has_structured_formatting
    {
        return NoToolTurnDecision::AutoContinue(NoToolTurnReason::ThinPlanningReply);
    }

    if shape.visible_text_chars >= 700
        || shape.visible_text_lines >= 18
        || (shape.has_substantive_markers
            && (shape.visible_text_chars >= 120 || shape.visible_text_lines >= 4))
        || (shape.has_structured_formatting
            && shape.visible_text_chars >= 260
            && shape.visible_text_lines >= 6)
    {
        return NoToolTurnDecision::Finish(NoToolTurnReason::SubstantiveAnswer);
    }

    let cues = planning_text_cues(message);
    if cues.iter().any(|cue| looks_like_planning_only_reply(cue)) {
        return NoToolTurnDecision::AutoContinue(NoToolTurnReason::PlanningCue);
    }

    NoToolTurnDecision::Finish(NoToolTurnReason::SubstantiveAnswer)
}

fn is_max_output_stop_reason(stop_reason: Option<&str>) -> bool {
    matches!(stop_reason, Some(reason) if matches!(
        reason.to_ascii_lowercase().as_str(),
        "max_tokens" | "max_output" | "length"
    ))
}

pub(crate) fn assistant_message_shape(message: &TranscriptMessage) -> AssistantMessageShape {
    let mut shape = AssistantMessageShape::default();

    for block in effective_blocks(message) {
        match block {
            TranscriptBlock::Text { text } => {
                let trimmed = text.trim();
                if trimmed.is_empty() {
                    continue;
                }
                shape.visible_text_chars += trimmed.chars().count();
                shape.visible_text_lines += trimmed
                    .lines()
                    .filter(|line| !line.trim().is_empty())
                    .count();
                let lower = trimmed.to_ascii_lowercase();
                shape.has_structured_formatting |= trimmed.contains('\n')
                    || trimmed.contains('|')
                    || trimmed.contains("- ")
                    || trimmed.contains("* ")
                    || trimmed.contains("1. ")
                    || trimmed.contains("##")
                    || trimmed.contains("```");
                shape.has_substantive_markers |= [
                    "summary",
                    "conclusion",
                    "report",
                    "assessment",
                    "comparison",
                    "results",
                    "recommendation",
                    "overall",
                    "implemented",
                    "missing",
                    "completeness",
                    "coverage",
                ]
                .iter()
                .any(|needle| lower.contains(needle))
                    || [
                        "结论",
                        "总结",
                        "报告",
                        "评估",
                        "对比",
                        "比较",
                        "结果",
                        "建议",
                        "已实现",
                        "缺口",
                        "完整度",
                        "覆盖",
                        "分析",
                    ]
                    .iter()
                    .any(|needle| trimmed.contains(needle));
            }
            TranscriptBlock::Thinking { text, .. } => {
                let trimmed = text.trim();
                if trimmed.is_empty() {
                    continue;
                }
                shape.thinking_chars += trimmed.chars().count();
                shape.thinking_lines += trimmed
                    .lines()
                    .filter(|line| !line.trim().is_empty())
                    .count();
            }
            TranscriptBlock::ToolUse { .. } | TranscriptBlock::ToolResult { .. } => {
                shape.has_tool_blocks = true;
            }
            _ => {}
        }
    }

    shape
}

fn prompt_suggests_repo_or_code_work(prompt: &str) -> bool {
    let lower = prompt.to_ascii_lowercase();
    [
        "repo",
        "repository",
        "project",
        "workspace",
        "code",
        "file",
        "directory",
        "folder",
        "implement",
        "implementation",
        "compare",
        "completeness",
        "analyze",
        "assessment",
        "rust",
        "typescript",
        "orbcode",
        "项目",
        "仓库",
        "代码",
        "文件",
        "目录",
        "实现",
        "完整度",
        "评估",
        "对比",
        "分析",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn planning_text_cues(message: &TranscriptMessage) -> Vec<String> {
    let mut cues = Vec::new();

    for block in effective_blocks(message) {
        match block {
            TranscriptBlock::Text { text } | TranscriptBlock::Thinking { text, .. } => {
                push_planning_cues(&mut cues, &text);
            }
            _ => {}
        }
    }

    cues
}

fn looks_like_planning_only_reply(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return false;
    }

    let lower = trimmed.to_ascii_lowercase();
    let english_planning = [
        "i'll ",
        "i’ll ",
        "let me ",
        "first, i'll",
        "first i'll",
        "now let me",
        "i'm going to",
    ]
    .iter()
    .any(|needle| lower.contains(needle));
    let chinese_planning = [
        "我来",
        "我先",
        "我会",
        "让我先",
        "让我",
        "先让我",
        "我会先",
        "我先来",
    ]
    .iter()
    .any(|needle| trimmed.contains(needle));

    if !(english_planning || chinese_planning) {
        return false;
    }

    [
        "inspect",
        "explore",
        "explor",
        "start",
        "begin",
        "investigate",
        "investigat",
        "check",
        "look at",
        "look",
        "review",
        "analyze",
        "analy",
        "compare",
        "compar",
        "assess",
        "read",
        "understand",
        "understanding",
        "structure",
        "directory",
        "key file",
        "search",
        "查看",
        "看看",
        "探索",
        "开始",
        "调查",
        "检查",
        "分析",
        "评估",
        "对比",
        "阅读",
        "搜索",
        "了解",
        "结构",
        "实现情况",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn push_planning_cues(cues: &mut Vec<String>, text: &str) {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return;
    }

    if trimmed.chars().count() <= 220 {
        cues.push(trimmed.to_string());
    }

    for line in trimmed.lines() {
        let line = line.trim();
        if !line.is_empty() && line.chars().count() <= 220 {
            cues.push(line.to_string());
        }
    }

    if let Some(sentence) = last_sentence_fragment(trimmed)
        && sentence.chars().count() <= 220
    {
        cues.push(sentence);
    }
}

fn last_sentence_fragment(text: &str) -> Option<String> {
    text.rsplit(['.', '!', '?', '。', '！', '？', '\n'])
        .map(str::trim)
        .find(|part| !part.is_empty())
        .map(str::to_string)
}

pub(crate) fn auto_continue_nudge(
    prompt: &str,
    attempt: usize,
    reason: NoToolTurnReason,
) -> String {
    if matches!(reason, NoToolTurnReason::MaxOutput) {
        return concat!(
            "Continue exactly where the previous assistant response stopped because it hit the max output limit. ",
            "Do not restart, summarize, or repeat completed text; continue from the last sentence."
        )
        .to_string();
    }

    if prompt_suggests_repo_or_code_work(prompt) && attempt >= 5 {
        return concat!(
            "Continue immediately. Your previous replies stopped at planning only.\n",
            "Your very next assistant response must contain at least one tool_use block and must not be only thinking or planning prose.\n",
            "For repository inspection, start with one of these concrete actions now:\n",
            "- bash with {\"command\":\"ls -la && find . -maxdepth 2 -type d | sort | head -200\"}\n",
            "- glob with {\"pattern\":\"**/*.rs\",\"path\":\".\"}\n",
            "- file-read with {\"file_path\":\"Cargo.toml\"}\n",
            "- file-read with {\"file_path\":\"README.md\"}\n",
            "After the tool results arrive, continue the analysis. No apology, no recap."
        )
        .to_string();
    }

    if attempt >= 3 {
        return concat!(
            "Continue immediately. Your previous replies stopped at planning only.\n",
            "Your next assistant response must include a tool_use block or the final answer.\n",
            "Do not provide more planning text without acting."
        )
        .to_string();
    }

    "Continue immediately. Your previous reply stopped at planning only. Do not stop after stating a plan. Either use the available tools now or provide the final answer in this same turn. No recap or apology.".to_string()
}

fn effective_blocks(message: &TranscriptMessage) -> Vec<TranscriptBlock> {
    if message.blocks.is_empty() {
        if message.content.is_empty() {
            Vec::new()
        } else {
            vec![TranscriptBlock::Text {
                text: message.content.clone(),
            }]
        }
    } else {
        message.blocks.clone()
    }
}

#[cfg(test)]
mod tests {
    use orbcode_protocol::MessageRole;

    use super::*;

    #[test]
    fn auto_continue_detection_ignores_non_repo_conclusions() {
        let message = TranscriptMessage::new(
            MessageRole::Assistant,
            "Rust ownership is a compile-time memory safety system.".to_string(),
        );

        assert!(!should_auto_continue_without_tools(
            "Explain Rust ownership briefly.",
            &message,
            Some("end_turn"),
            0,
        ));
    }

    #[test]
    fn auto_continue_detection_matches_thinking_only_chinese_planning_reply() {
        let message = TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![TranscriptBlock::Thinking {
                text: "用户要求我评估各个 crate 的测试覆盖情况。\n\n让我开始探索。".to_string(),
                signature: None,
            }],
        );

        assert!(should_auto_continue_without_tools(
            "评估一下这个仓库中各个 crate 的测试覆盖情况",
            &message,
            Some("end_turn"),
            1,
        ));
    }

    #[test]
    fn auto_continue_detection_matches_investigation_wording() {
        let message = TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![TranscriptBlock::Thinking {
                text: "用户要求我评估各个 crate 的测试覆盖情况。\n\n让我立即开始调查。".to_string(),
                signature: None,
            }],
        );

        assert!(should_auto_continue_without_tools(
            "评估一下这个仓库中各个 crate 的测试覆盖情况",
            &message,
            Some("end_turn"),
            2,
        ));
    }

    #[test]
    fn auto_continue_detection_matches_long_english_thinking_reply() {
        let message = TranscriptMessage::from_blocks(
                MessageRole::Assistant,
                vec![TranscriptBlock::Thinking {
                    text: "The user wants me to evaluate how well each crate in this workspace is covered by tests. Let me start by exploring the directory structure and understanding what is already there.".to_string(),
                    signature: None,
                }],
            );

        assert!(should_auto_continue_without_tools(
            "评估一下这个仓库中各个 crate 的测试覆盖情况",
            &message,
            Some("end_turn"),
            3,
        ));
    }

    #[test]
    fn auto_continue_detection_matches_thinking_only_message_without_planning_prefix() {
        let message = TranscriptMessage::from_blocks(
            MessageRole::Assistant,
            vec![TranscriptBlock::Thinking {
                text: "用户想让我评估这个仓库里各个 crate 的测试覆盖情况。".to_string(),
                signature: None,
            }],
        );

        assert!(should_auto_continue_without_tools(
            "评估一下这个仓库中各个 crate 的测试覆盖情况",
            &message,
            Some("end_turn"),
            4,
        ));
    }

    #[test]
    fn auto_continue_detection_accepts_structured_chinese_report_after_tool_use() {
        let message = TranscriptMessage::new(
            MessageRole::Assistant,
            concat!(
                "基于对代码库的分析，我来汇总各个 crate 的测试覆盖情况：\n\n",
                "## 覆盖概览\n",
                "| crate | 测试数 | 行覆盖 |\n",
                "| --- | --- | --- |\n",
                "| orbcode-core | 412 | 78% |\n",
                "| orbcode-mcp | 128 | 64% |\n\n",
                "## 关键结论\n",
                "1. 核心 crate 的单元测试已经比较充分。\n",
                "2. MCP 传输层仍缺少端到端用例。\n",
                "3. 整体覆盖大约在 70% 到 80% 之间，交互路径明显高于平均值。\n"
            )
            .to_string(),
        );

        assert!(!should_auto_continue_without_tools_after_tool_use(
            "评估一下这个仓库中各个 crate 的测试覆盖情况",
            &message,
            Some("end_turn"),
            2,
        ));
    }

    #[test]
    fn auto_continue_detection_accepts_long_structured_answer_after_tool_use() {
        let message = TranscriptMessage::from_blocks(
                MessageRole::Assistant,
                vec![
                    TranscriptBlock::Thinking {
                        text: "我已经收集了足够的信息。让我给出完整结论。".to_string(),
                        signature: None,
                    },
                    TranscriptBlock::Text {
                        text: concat!(
                            "# Workspace Test Coverage Report\n\n",
                            "## Executive Summary\n",
                            "Unit tests cover the core interactive path well, but the MCP transport layer and the tool adapters still have gaps.\n\n",
                            "## Coverage By Crate\n",
                            "- orbcode-core: 412 tests\n",
                            "- orbcode-tui: 366 tests\n",
                            "- orbcode-mcp: 128 tests\n",
                            "- orbcode-session-store: 96 tests\n\n",
                            "## Conclusion\n",
                            "Overall coverage sits around 70-80%, while the primary TUI workflow is materially better covered than the average subsystem.\n"
                        )
                        .to_string(),
                    },
                ],
            );

        assert!(!should_auto_continue_without_tools_after_tool_use(
            "评估一下这个仓库中各个 crate 的测试覆盖情况",
            &message,
            Some("end_turn"),
            3,
        ));
    }
}
