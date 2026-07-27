use anyhow::Result;
use crossterm::event::MouseEvent;
use orbcode_config::mcp_permission_target;

use crate::clipboard::copy_text_to_clipboard;
use crate::numeric::saturating_u16;
use crate::render::bash_warnings::destructive_bash_command_warning;
use crate::render::permission_labels::{
    bash_permission_requests_sandbox_escalation, bool_value_any, canonical_permission_tool_name,
    file_name_for_display, friendly_bash_permission_rules_label, friendly_permission_rule_label,
    human_field_label, human_tool_name, parent_path_for_display, string_value_any,
    suggested_permission_rules,
};
use crate::render::styled_wrap::{wrap_styled_line, wrap_styled_lines};
use crate::state::TuiState;

use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PermissionRequestKeyAction {
    None,
    ToggleDetails,
    Permission {
        request_id: String,
        decision: PermissionDecision,
    },
}

pub(crate) struct PermissionPanelViewport {
    pub(crate) body: Vec<StyledLine>,
    pub(crate) all_lines: Vec<StyledLine>,
    pub(crate) first_row: usize,
    pub(crate) actual_scroll: usize,
    pub(crate) max_scroll: usize,
}

#[allow(dead_code)]
#[derive(Clone)]
pub(crate) struct PermissionOverlayState {
    pub(crate) request: PermissionRequest,
    pub(crate) selected_option: usize,
    pub(crate) always_allow_rule: String,
    pub(crate) always_allow_rules: Vec<String>,
    pub(crate) editing_rule: bool,
    pub(crate) details_expanded: bool,
    pub(crate) panel_scroll: usize,
    pub(crate) viewport: TranscriptViewportState,
    pub(crate) content_cache: PermissionPanelContentCache,
    /// Further permission requests that arrived while this overlay was open.
    /// Shown one at a time as each is resolved so a concurrent request is not
    /// dropped (which left it to hang until timeout).
    pub(crate) queued: std::collections::VecDeque<PermissionRequest>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct PermissionPanelContentCache {
    key: Option<PermissionPanelContentCacheKey>,
    content: PermissionPanelContent,
    wrapped_body: Vec<StyledLine>,
    #[cfg(test)]
    pub(crate) hits: u64,
    #[cfg(test)]
    pub(crate) misses: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PermissionPanelContentCacheKey {
    inner_width: usize,
    selected_option: usize,
    always_allow_rule: String,
    always_allow_rules: Vec<String>,
    editing_rule: bool,
    details_expanded: bool,
}

pub(crate) struct CachedPermissionPanelContent<'a> {
    pub(crate) content: &'a PermissionPanelContent,
    pub(crate) wrapped_body: &'a [StyledLine],
}

#[derive(Clone, Debug, Default)]
pub(crate) struct PermissionPanelContent {
    pub(crate) body: Vec<StyledLine>,
    pub(crate) edit_cursor: Option<(u16, u16)>,
}

#[allow(dead_code)]
pub(crate) fn permission_panel_content(
    permission: &PermissionOverlayState,
    inner_width: usize,
) -> PermissionPanelContent {
    let request = &permission.request;
    let payload = serde_json::from_str::<Value>(&request.tool_input).ok();
    let mut body = Vec::new();
    let title = permission_panel_title(request);
    body.push(Line::from(vec![Span::styled(
        title.clone(),
        inactive_style().add_modifier(Modifier::BOLD),
    )]));
    body.push(Line::from(vec![Span::styled(
        "─".repeat(inner_width.max(1)),
        subtle_style(),
    )]));

    match canonical_permission_tool_name(&request.tool_name).as_str() {
        "bash" => {
            let command = payload
                .as_ref()
                .and_then(|value| string_value_any(value, &["command", "cmd", "script"]))
                .unwrap_or_else(|| request.tool_input.clone());
            body.push(Line::from(vec![
                Span::styled("Command ", subtle_style()),
                Span::styled(command.clone(), emphasis_style()),
            ]));
            if let Some(description) = payload
                .as_ref()
                .and_then(|value| string_value_any(value, &["description"]))
            {
                body.push(Line::from(vec![
                    Span::styled("Description ", subtle_style()),
                    Span::styled(description, inactive_style()),
                ]));
            }
            if payload
                .as_ref()
                .is_some_and(bash_permission_requests_sandbox_escalation)
            {
                body.push(Line::from(vec![
                    Span::styled("Sandbox ", warning_style().add_modifier(Modifier::BOLD)),
                    Span::styled(
                        "Requests unsandboxed execution for this command",
                        warning_style(),
                    ),
                ]));
                body.push(Line::from(vec![
                    Span::styled("Effect ", warning_style().add_modifier(Modifier::BOLD)),
                    Span::styled(
                        "Bypasses configured filesystem and network sandbox restrictions for this run",
                        warning_style(),
                    ),
                ]));
            }
            if let Some(warning) = destructive_bash_command_warning(&command) {
                body.push(Line::from(vec![
                    Span::styled("Warning ", warning_style().add_modifier(Modifier::BOLD)),
                    Span::styled(warning, warning_style()),
                ]));
            }
        }
        "file-read" => {
            append_file_read_permission_preview(&mut body, request, payload.as_ref());
        }
        "file-write" | "file-edit" => {
            append_file_permission_preview(&mut body, request, payload.as_ref(), inner_width);
        }
        "grep" => {
            append_grep_permission_preview(&mut body, payload.as_ref());
        }
        "call-mcp-tool" | "read-mcp-resource" | "list-mcp-resources" | "list-mcp-tools" => {
            append_mcp_permission_preview(
                &mut body,
                &request.tool_name,
                payload.as_ref(),
                inner_width,
            );
        }
        "Agent" => {
            append_agent_permission_preview(&mut body, payload.as_ref(), inner_width);
        }
        "workflow" => {
            append_workflow_permission_preview(&mut body, payload.as_ref(), inner_width);
        }
        "todo-write" => {
            append_todo_permission_preview(&mut body, payload.as_ref());
        }
        "task-create" | "task-get" | "task-list" | "task-update" | "task-output" | "task-stop" => {
            append_task_permission_preview(
                &mut body,
                &request.tool_name,
                payload.as_ref(),
                inner_width,
            );
        }
        "enter-plan-mode" | "exit-plan-mode" | "verify-plan-execution" => {
            append_plan_permission_preview(
                &mut body,
                &request.tool_name,
                payload.as_ref(),
                inner_width,
            );
        }
        _ => {
            append_generic_permission_preview(&mut body, request, payload.as_ref());
        }
    }

    if permission.details_expanded {
        body.push(Line::default());
        append_permission_request_detail_lines(&mut body, request, inner_width);
    }

    if !body.is_empty() {
        body.push(Line::default());
    }
    body.push(Line::from(vec![Span::styled(
        permission_question(request, payload.as_ref()),
        inactive_style(),
    )]));
    body.push(Line::default());

    let options_start = body.len();
    for (index, option) in permission.option_lines().iter().enumerate() {
        let selected = index == permission.selected_option;
        body.push(Line::from(vec![
            Span::styled(
                if selected { "› " } else { "  " },
                if selected {
                    emphasis_style()
                } else {
                    subtle_style()
                },
            ),
            Span::styled(
                option.clone(),
                if selected {
                    highlight_style()
                } else {
                    inactive_style()
                },
            ),
        ]));
    }

    body.push(Line::default());
    body.push(Line::from(vec![
        Span::styled("↑↓", inactive_style()),
        Span::styled(" to navigate · ", subtle_style()),
        Span::styled("Enter", inactive_style().add_modifier(Modifier::BOLD)),
        Span::styled(" to select · ", subtle_style()),
        Span::styled("PgUp/PgDn", inactive_style().add_modifier(Modifier::BOLD)),
        Span::styled(" to scroll · ", subtle_style()),
        Span::styled("Ctrl-O", inactive_style().add_modifier(Modifier::BOLD)),
        Span::styled(" details · ", subtle_style()),
        Span::styled("Esc", inactive_style().add_modifier(Modifier::BOLD)),
        Span::styled("/", subtle_style()),
        Span::styled("Ctrl-C", inactive_style().add_modifier(Modifier::BOLD)),
        Span::styled(" to cancel", subtle_style()),
    ]));

    let edit_cursor = if permission.editing_rule {
        permission.always_allow_index().map(|index| {
            let prefix = "  2. Yes, and don't ask again for: ";
            (
                saturating_u16(
                    prefix
                        .chars()
                        .count()
                        .saturating_add(permission.always_allow_rule.chars().count()),
                ),
                saturating_u16(options_start.saturating_add(index)),
            )
        })
    } else {
        None
    };

    PermissionPanelContent { body, edit_cursor }
}

impl PermissionOverlayState {
    pub(crate) fn new(request: PermissionRequest) -> Self {
        let always_allow_rules = suggested_permission_rules(&request);
        let always_allow_rule = if always_allow_rules.len() == 1 {
            always_allow_rules[0].clone()
        } else {
            String::new()
        };
        Self {
            request,
            selected_option: 0,
            always_allow_rule,
            always_allow_rules,
            editing_rule: false,
            details_expanded: false,
            panel_scroll: 0,
            viewport: TranscriptViewportState::default(),
            content_cache: PermissionPanelContentCache::default(),
            queued: std::collections::VecDeque::new(),
        }
    }

    /// Enqueue a concurrent request to be shown after the current one resolves,
    /// unless it is already the active request or already queued.
    pub(crate) fn enqueue(&mut self, request: PermissionRequest) {
        if self.request.request_id == request.request_id
            || self
                .queued
                .iter()
                .any(|queued| queued.request_id == request.request_id)
        {
            return;
        }
        self.queued.push_back(request);
    }

    /// Drop a queued request that resolved out-of-band (timeout, or another
    /// client answered it) so it is not shown later as a stale prompt. Returns
    /// whether an entry was removed. Does not affect the currently-shown request.
    pub(crate) fn remove_queued(&mut self, request_id: &str) -> bool {
        let before = self.queued.len();
        self.queued.retain(|queued| queued.request_id != request_id);
        self.queued.len() != before
    }

    /// Build the overlay for the next queued request (if any), carrying the
    /// remaining queue forward. Returns `None` when the queue is empty.
    pub(crate) fn take_next_queued(&mut self) -> Option<Self> {
        let next = self.queued.pop_front()?;
        let mut overlay = Self::new(next);
        overlay.queued = std::mem::take(&mut self.queued);
        Some(overlay)
    }

    fn can_always_allow(&self) -> bool {
        !self.always_allow_rules_for_decision().is_empty()
    }

    pub(crate) fn can_edit_always_allow(&self) -> bool {
        self.always_allow_rules.len() <= 1 && self.can_always_allow()
    }

    pub(crate) fn always_allow_rules_for_decision(&self) -> Vec<String> {
        if self.editing_rule || self.always_allow_rules.len() <= 1 {
            let rule = self.always_allow_rule.trim();
            if rule.is_empty() {
                Vec::new()
            } else {
                vec![rule.to_string()]
            }
        } else {
            self.always_allow_rules
                .iter()
                .map(|rule| rule.trim())
                .filter(|rule| !rule.is_empty())
                .map(ToString::to_string)
                .collect()
        }
    }

    pub(crate) fn option_count(&self) -> usize {
        if self.can_always_allow() { 3 } else { 2 }
    }

    pub(crate) fn always_allow_index(&self) -> Option<usize> {
        if self.can_always_allow() {
            Some(1)
        } else {
            None
        }
    }

    pub(crate) fn deny_index(&self) -> usize {
        self.option_count().saturating_sub(1)
    }

    pub(crate) fn option_lines(&self) -> Vec<String> {
        let mut lines = vec!["1. Yes".to_string()];
        if self.can_always_allow() {
            lines.push(format!(
                "2. Yes, and don't ask again for: {}",
                self.always_allow_label()
            ));
            lines.push("3. No".to_string());
        } else {
            lines.push("2. No".to_string());
        }
        lines
    }

    fn always_allow_label(&self) -> String {
        if self.editing_rule {
            self.always_allow_rule.clone()
        } else if canonical_permission_tool_name(&self.request.tool_name) == "bash" {
            friendly_bash_permission_rules_label(&self.always_allow_rules_for_decision())
        } else {
            friendly_permission_rule_label(&self.always_allow_rule)
        }
    }

    pub(crate) fn has_selection(&self) -> bool {
        self.viewport.selection.is_some()
    }

    pub(crate) fn clear_selection(&mut self) {
        self.viewport.clear_selection();
    }

    pub(crate) fn autoscroll_selection(&mut self, mouse_event: &MouseEvent) {
        if !self.has_selection() {
            return;
        }
        let area = self.viewport.area;
        if area.width == 0 || area.height == 0 {
            return;
        }
        let top_edge = area.y;
        let bottom_edge = area.y.saturating_add(area.height.saturating_sub(1));
        if mouse_event.row <= top_edge {
            if !self.viewport.is_at_top() {
                self.panel_scroll = self.panel_scroll.saturating_add(1);
                self.viewport.set_scroll(self.panel_scroll);
            }
        } else if mouse_event.row >= bottom_edge && !self.viewport.is_at_bottom() {
            self.panel_scroll = self.panel_scroll.saturating_sub(1);
            self.viewport.set_scroll(self.panel_scroll);
        }
    }
}

impl TuiState {
    pub(crate) fn has_permission_selection(&self) -> bool {
        matches!(
            &self.overlay,
            Some(OverlayState::PermissionRequest(permission)) if permission.has_selection()
        )
    }

    pub(crate) fn clear_permission_selection(&mut self) {
        if let Some(OverlayState::PermissionRequest(permission)) = self.overlay.as_mut() {
            permission.clear_selection();
        }
    }

    pub(crate) fn copy_selected_permission_to_clipboard(&self) -> Result<usize> {
        let selected = match &self.overlay {
            Some(OverlayState::PermissionRequest(permission)) => permission
                .viewport
                .selected_text()
                .ok_or_else(|| anyhow::anyhow!("No permission text is selected."))?,
            _ => anyhow::bail!("No permission text is selected."),
        };
        copy_text_to_clipboard(&selected)?;
        Ok(selected.chars().count())
    }
}

impl PermissionOverlayState {
    pub(crate) fn cached_panel_content(
        &mut self,
        inner_width: usize,
    ) -> CachedPermissionPanelContent<'_> {
        let key = PermissionPanelContentCacheKey {
            inner_width,
            selected_option: self.selected_option,
            always_allow_rule: self.always_allow_rule.clone(),
            always_allow_rules: self.always_allow_rules.clone(),
            editing_rule: self.editing_rule,
            details_expanded: self.details_expanded,
        };
        if self.content_cache.key.as_ref() == Some(&key) {
            #[cfg(test)]
            {
                self.content_cache.hits += 1;
            }
        } else {
            #[cfg(test)]
            {
                self.content_cache.misses += 1;
            }
            let content = permission_panel_content(self, inner_width);
            let wrapped_body = wrap_styled_lines(&content.body, inner_width.max(1));
            self.content_cache.key = Some(key);
            self.content_cache.content = content;
            self.content_cache.wrapped_body = wrapped_body;
        }

        CachedPermissionPanelContent {
            content: &self.content_cache.content,
            wrapped_body: &self.content_cache.wrapped_body,
        }
    }
}

fn append_permission_request_detail_lines(
    body: &mut Vec<StyledLine>,
    request: &PermissionRequest,
    inner_width: usize,
) {
    body.push(Line::from(Span::styled(
        "Request details",
        inactive_style().add_modifier(Modifier::BOLD),
    )));
    body.push(Line::from(vec![
        Span::styled("Tool ", subtle_style()),
        Span::styled(human_tool_name(&request.tool_name), inactive_style()),
    ]));
    body.push(Line::from(vec![
        Span::styled("Tool use ID ", subtle_style()),
        Span::styled(request.tool_use_id.clone(), inactive_style()),
    ]));
    body.push(Line::from(vec![
        Span::styled("Request ID ", subtle_style()),
        Span::styled(request.request_id.clone(), inactive_style()),
    ]));
    body.push(Line::from(vec![
        Span::styled("Session ", subtle_style()),
        Span::styled(request.session_id.clone(), inactive_style()),
    ]));
    body.push(Line::from(vec![
        Span::styled("Requires ", subtle_style()),
        Span::styled(
            permission_requirements_label(request),
            warning_style().add_modifier(Modifier::BOLD),
        ),
    ]));
    if !request.tool_input.trim().is_empty() {
        append_preview_text_lines(
            body,
            "Raw input",
            &request.tool_input,
            usize::MAX,
            inner_width,
        );
    }
}

fn permission_requirements_label(request: &PermissionRequest) -> String {
    let mut requirements = Vec::new();
    if request.requires_tools_permission {
        requirements.push("tools permission");
    }
    if request.requires_network_permission {
        requirements.push("network permission");
    }
    if requirements.is_empty() {
        "no elevated permissions".to_string()
    } else {
        requirements.join(" and ")
    }
}

fn permission_panel_title(request: &PermissionRequest) -> String {
    match canonical_permission_tool_name(&request.tool_name).as_str() {
        "bash" => "Bash command".to_string(),
        "file-read" => "Read file".to_string(),
        "file-write" => "Create file".to_string(),
        "file-edit" => "Edit file".to_string(),
        "glob" => "Search files".to_string(),
        "grep" => "Search content".to_string(),
        "notebook-edit" => "Edit notebook".to_string(),
        "web-fetch" => "Fetch URL".to_string(),
        "web-search" => "Search web".to_string(),
        "ask-user-question" => "Ask question".to_string(),
        "todo-write" => "Update todo list".to_string(),
        "task-create" => "Create task".to_string(),
        "task-get" => "Read task".to_string(),
        "task-list" => "List tasks".to_string(),
        "task-update" => "Update task".to_string(),
        "task-output" => "Read task output".to_string(),
        "task-stop" => "Stop task".to_string(),
        "enter-plan-mode" => "Enter plan mode".to_string(),
        "exit-plan-mode" => "Exit plan mode".to_string(),
        "verify-plan-execution" => "Verify plan".to_string(),
        "skill" => "Load skill".to_string(),
        "tool-search" => "Search tools".to_string(),
        "workflow" => "Start workflow".to_string(),
        "lsp" => "Language server".to_string(),
        "list-mcp-resources" => "List MCP resources".to_string(),
        "list-mcp-tools" => "List MCP tools".to_string(),
        "read-mcp-resource" => "Read MCP resource".to_string(),
        "call-mcp-tool" => "Call MCP tool".to_string(),
        "Agent" => "Run agent".to_string(),
        _ => "Tool permission".to_string(),
    }
}

fn permission_question(request: &PermissionRequest, payload: Option<&Value>) -> String {
    match canonical_permission_tool_name(&request.tool_name).as_str() {
        "bash" => "Do you want to run this command?".to_string(),
        "file-read" => "Do you want to read this file?".to_string(),
        "glob" => "Do you want to search files?".to_string(),
        "grep" => {
            if let Some(path) = payload.and_then(|value| string_value_any(value, &["path"])) {
                format!("Do you want to search file contents in {path}?")
            } else {
                "Do you want to search file contents?".to_string()
            }
        }
        "file-write" | "file-edit" => {
            let path = payload
                .and_then(|value| string_value_any(value, &["file_path", "filePath", "path"]))
                .unwrap_or_else(|| "this file".to_string());
            format!("Do you want to edit {path}?")
        }
        "web-fetch" => "Do you want to fetch this URL?".to_string(),
        "web-search" => "Do you want to search the web?".to_string(),
        "call-mcp-tool" => "Do you want to call this MCP tool?".to_string(),
        "read-mcp-resource" => "Do you want to read this MCP resource?".to_string(),
        "Agent" => "Do you want to run this subagent?".to_string(),
        "workflow" => "Do you want to start this workflow?".to_string(),
        "todo-write" => "Do you want to update the todo list?".to_string(),
        "task-create" => "Do you want to create this task?".to_string(),
        "task-update" => "Do you want to update this task?".to_string(),
        "task-output" => "Do you want to read this task output?".to_string(),
        "task-stop" => "Do you want to stop this task?".to_string(),
        "task-get" | "task-list" => "Do you want to inspect tasks?".to_string(),
        "enter-plan-mode" => "Do you want to enter plan mode?".to_string(),
        "exit-plan-mode" => "Do you want to accept this plan?".to_string(),
        "verify-plan-execution" => "Do you want to verify the plan?".to_string(),
        _ => format!(
            "Do you want to allow {}?",
            human_tool_name(&request.tool_name)
        ),
    }
}

fn append_file_read_permission_preview(
    body: &mut Vec<StyledLine>,
    request: &PermissionRequest,
    payload: Option<&Value>,
) {
    let path = payload
        .and_then(|value| string_value_any(value, &["file_path", "filePath", "path"]))
        .unwrap_or_else(|| request.tool_input.clone());

    body.push(Line::from(vec![
        Span::styled("File ", subtle_style()),
        Span::styled(path, inactive_style()),
    ]));
}

fn append_file_permission_preview(
    body: &mut Vec<StyledLine>,
    request: &PermissionRequest,
    payload: Option<&Value>,
    inner_width: usize,
) {
    let path = payload
        .and_then(|value| string_value_any(value, &["file_path", "filePath", "path"]))
        .unwrap_or_else(|| "(unknown path)".to_string());
    let canonical = canonical_permission_tool_name(&request.tool_name);
    let new_text = payload
        .and_then(|value| string_value_any(value, &["content", "new_string", "replace"]))
        .unwrap_or_default();
    let old_text = payload
        .and_then(|value| string_value_any(value, &["old_string", "find"]))
        .unwrap_or_default();
    let added = new_text.lines().count();
    let removed = if canonical == "file-edit" {
        old_text.lines().count()
    } else {
        0
    };

    body.push(Line::from(vec![
        Span::styled(
            file_name_for_display(&path),
            inactive_style().add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(
            format!("+{added}"),
            Style::default().fg(active_palette().success),
        ),
        Span::raw(" "),
        Span::styled(
            format!("-{removed}"),
            Style::default().fg(active_palette().warning),
        ),
    ]));
    body.push(Line::from(Span::styled(
        parent_path_for_display(&path),
        subtle_style(),
    )));
    body.push(Line::default());

    if new_text.is_empty() {
        body.push(Line::from(Span::styled(
            "(empty file content)",
            subtle_style(),
        )));
    } else {
        body.extend(code_preview_lines(&new_text, &path, inner_width, '+'));
    }
}

fn append_grep_permission_preview(body: &mut Vec<StyledLine>, payload: Option<&Value>) {
    let query = payload
        .and_then(|value| string_value_any(value, &["pattern", "query"]))
        .unwrap_or_else(|| "(missing pattern)".to_string());
    let path = payload
        .and_then(|value| string_value_any(value, &["path"]))
        .unwrap_or_else(|| ".".to_string());
    let file_glob = payload.and_then(|value| string_value_any(value, &["glob"]));
    let output_mode = payload
        .and_then(|value| string_value_any(value, &["output_mode"]))
        .map(|value| human_grep_output_mode(&value));

    body.push(Line::from(vec![
        Span::styled("Regex ", subtle_style()),
        Span::styled(query, emphasis_style()),
    ]));
    body.push(Line::from(vec![
        Span::styled("Search in ", subtle_style()),
        Span::styled(path, inactive_style()),
    ]));
    if let Some(file_glob) = file_glob {
        body.push(Line::from(vec![
            Span::styled("Files ", subtle_style()),
            Span::styled(file_glob, inactive_style()),
        ]));
    }
    if let Some(output_mode) = output_mode {
        body.push(Line::from(vec![
            Span::styled("Show ", subtle_style()),
            Span::styled(output_mode, inactive_style()),
        ]));
    }
}

fn append_mcp_permission_preview(
    body: &mut Vec<StyledLine>,
    tool_name: &str,
    payload: Option<&Value>,
    inner_width: usize,
) {
    let server = payload
        .and_then(|value| string_value_any(value, &["server_id", "server"]))
        .unwrap_or_else(|| "(missing server)".to_string());
    body.push(Line::from(vec![
        Span::styled("Server ", subtle_style()),
        Span::styled(server, Style::default().fg(active_palette().tool)),
    ]));

    match canonical_permission_tool_name(tool_name).as_str() {
        "call-mcp-tool" => {
            let mcp_tool = payload
                .and_then(|value| string_value_any(value, &["tool_name", "tool"]))
                .unwrap_or_else(|| "(missing tool)".to_string());
            body.push(Line::from(vec![
                Span::styled("Tool ", subtle_style()),
                Span::styled(mcp_tool, inactive_style().add_modifier(Modifier::BOLD)),
            ]));
            if let Some(input) = payload.and_then(|value| value.get("input")) {
                append_human_readable_value(body, "Arguments", input, 0);
            }
        }
        "read-mcp-resource" => {
            append_optional_payload_string(body, payload, "URI", &["uri"]);
        }
        "list-mcp-resources" => {
            body.push(Line::from(Span::styled(
                "Action List resources exposed by this server",
                inactive_style(),
            )));
        }
        "list-mcp-tools" => {
            body.push(Line::from(Span::styled(
                "Action List tools exposed by this server",
                inactive_style(),
            )));
        }
        _ => {}
    }

    let target = mcp_permission_target(
        tool_name,
        &payload.cloned().unwrap_or(Value::Null).to_string(),
    );
    if let Some(target) = target {
        append_preview_text_lines(body, "Permission target", &target, 1, inner_width);
    }
}

fn append_agent_permission_preview(
    body: &mut Vec<StyledLine>,
    payload: Option<&Value>,
    inner_width: usize,
) {
    let agent_type = payload
        .and_then(|value| string_value_any(value, &["subagent_type", "subagentType"]))
        .unwrap_or_else(|| "general-purpose".to_string());
    let description = payload
        .and_then(|value| string_value_any(value, &["description"]))
        .unwrap_or_else(|| "(missing description)".to_string());
    let prompt = payload
        .and_then(|value| string_value_any(value, &["prompt"]))
        .unwrap_or_default();

    body.push(Line::from(vec![
        Span::styled("Agent ", subtle_style()),
        Span::styled(agent_type, Style::default().fg(active_palette().tool)),
    ]));
    body.push(Line::from(vec![
        Span::styled("Task ", subtle_style()),
        Span::styled(description, inactive_style()),
    ]));
    append_preview_text_lines(body, "Prompt", &prompt, usize::MAX, inner_width);
}

fn append_workflow_permission_preview(
    body: &mut Vec<StyledLine>,
    payload: Option<&Value>,
    inner_width: usize,
) {
    let name = payload
        .and_then(|value| string_value_any(value, &["name"]))
        .unwrap_or_else(|| "(missing name)".to_string());
    body.push(Line::from(vec![
        Span::styled("Workflow ", subtle_style()),
        Span::styled(name, Style::default().fg(active_palette().tool)),
    ]));

    if let Some(arguments) =
        payload.and_then(|value| string_value_any(value, &["arguments", "args"]))
        && !arguments.trim().is_empty()
    {
        body.push(Line::from(vec![
            Span::styled("Arguments ", subtle_style()),
            Span::styled(arguments, inactive_style()),
        ]));
    }

    let spec = payload.and_then(|value| value.get("spec"));
    if let Some(description) = spec.and_then(|value| string_value_any(value, &["description"])) {
        append_preview_text_lines(body, "Description", &description, 2, inner_width);
    }

    let steps = spec
        .and_then(|value| value.get("steps"))
        .and_then(Value::as_array);
    body.push(Line::from(vec![
        Span::styled("Top-level steps ", subtle_style()),
        Span::styled(
            format!("{} step(s)", steps.map_or(0, Vec::len)),
            inactive_style(),
        ),
    ]));
    if let Some(steps) = steps {
        append_workflow_step_preview_lines(body, steps, 5);
    }
}

fn append_todo_permission_preview(body: &mut Vec<StyledLine>, payload: Option<&Value>) {
    let list_name = payload
        .and_then(|value| string_value_any(value, &["list"]))
        .unwrap_or_else(|| "default".to_string());
    let mode = payload
        .and_then(|value| string_value_any(value, &["mode"]))
        .unwrap_or_else(|| "append".to_string());
    let items = payload
        .and_then(|value| value.get("items"))
        .and_then(Value::as_array);

    body.push(Line::from(vec![
        Span::styled("List ", subtle_style()),
        Span::styled(list_name, inactive_style()),
    ]));
    body.push(Line::from(vec![
        Span::styled("Mode ", subtle_style()),
        Span::styled(human_todo_mode(&mode), inactive_style()),
    ]));
    body.push(Line::from(vec![
        Span::styled("Items ", subtle_style()),
        Span::styled(
            format!("{} item(s)", items.map_or(0, Vec::len)),
            inactive_style(),
        ),
    ]));
    if let Some(items) = items {
        append_todo_item_preview_lines(body, items, 5);
    }
}

fn append_task_permission_preview(
    body: &mut Vec<StyledLine>,
    tool_name: &str,
    payload: Option<&Value>,
    inner_width: usize,
) {
    let canonical = canonical_permission_tool_name(tool_name);
    body.push(Line::from(vec![
        Span::styled("Action ", subtle_style()),
        Span::styled(
            human_tool_name(tool_name),
            Style::default().fg(active_palette().tool),
        ),
    ]));

    match canonical.as_str() {
        "task-create" => {
            append_optional_payload_string(body, payload, "Subject", &["subject"]);
            append_optional_payload_string(body, payload, "Owner", &["owner"]);
            if let Some(value) = payload.and_then(|value| string_value_any(value, &["description"]))
            {
                append_preview_text_lines(body, "Description", &value, 2, inner_width);
            }
            append_task_link_counts(body, payload);
        }
        "task-update" => {
            append_optional_payload_string(body, payload, "Task", &["taskId", "task_id"]);
            append_optional_payload_string(body, payload, "Status", &["status"]);
            append_optional_payload_string(body, payload, "Subject", &["subject"]);
            append_optional_payload_string(body, payload, "Owner", &["owner"]);
            if let Some(value) = payload.and_then(|value| string_value_any(value, &["description"]))
            {
                append_preview_text_lines(body, "Description", &value, 2, inner_width);
            }
            append_task_link_counts(body, payload);
        }
        "task-list" => {
            body.push(Line::from(Span::styled(
                "Lists known tasks for this workspace.",
                inactive_style(),
            )));
        }
        "task-output" => {
            append_optional_payload_string(body, payload, "Task", &["task_id", "taskId"]);
            let block = payload
                .and_then(|value| bool_value_any(value, &["block"]))
                .unwrap_or(true);
            body.push(Line::from(vec![
                Span::styled("Wait ", subtle_style()),
                Span::styled(if block { "yes" } else { "no" }, inactive_style()),
            ]));
            append_optional_payload_value(body, payload, "Timeout", &["timeout"]);
        }
        "task-stop" => {
            append_optional_payload_string(
                body,
                payload,
                "Task",
                &["task_id", "taskId", "shell_id"],
            );
            body.push(Line::from(Span::styled(
                "Requests cancellation of a running background task.",
                warning_style(),
            )));
        }
        _ => {
            append_optional_payload_string(body, payload, "Task", &["taskId", "task_id"]);
        }
    }
}

fn append_plan_permission_preview(
    body: &mut Vec<StyledLine>,
    tool_name: &str,
    payload: Option<&Value>,
    inner_width: usize,
) {
    let canonical = canonical_permission_tool_name(tool_name);
    match canonical.as_str() {
        "enter-plan-mode" => {
            body.push(Line::from(Span::styled(
                "Switches the session into plan mode before implementation.",
                inactive_style(),
            )));
        }
        "exit-plan-mode" => {
            let plan = payload
                .and_then(|value| string_value_any(value, &["plan"]))
                .unwrap_or_default();
            append_preview_text_lines(body, "Plan", &plan, 5, inner_width);
            let allowed = payload
                .and_then(|value| value.get("allowedPrompts"))
                .and_then(Value::as_array);
            body.push(Line::from(vec![
                Span::styled("Allowed prompts ", subtle_style()),
                Span::styled(
                    format!("{} item(s)", allowed.map_or(0, Vec::len)),
                    inactive_style(),
                ),
            ]));
            if let Some(allowed) = allowed {
                append_allowed_prompt_preview_lines(body, allowed, 3);
            }
        }
        "verify-plan-execution" => {
            body.push(Line::from(Span::styled(
                "Checks whether the current plan has been completed.",
                inactive_style(),
            )));
        }
        _ => {
            body.push(Line::from(vec![
                Span::styled("Tool ", subtle_style()),
                Span::styled(human_tool_name(tool_name), inactive_style()),
            ]));
        }
    }
}

fn append_optional_payload_string(
    body: &mut Vec<StyledLine>,
    payload: Option<&Value>,
    label: &str,
    keys: &[&str],
) {
    if let Some(value) = payload.and_then(|value| string_value_any(value, keys)) {
        body.push(Line::from(vec![
            Span::styled(format!("{label} "), subtle_style()),
            Span::styled(value, inactive_style()),
        ]));
    }
}

fn append_optional_payload_value(
    body: &mut Vec<StyledLine>,
    payload: Option<&Value>,
    label: &str,
    keys: &[&str],
) {
    if let Some(value) = payload.and_then(|payload| keys.iter().find_map(|key| payload.get(*key))) {
        body.push(Line::from(vec![
            Span::styled(format!("{label} "), subtle_style()),
            Span::styled(human_value(value), inactive_style()),
        ]));
    }
}

fn append_task_link_counts(body: &mut Vec<StyledLine>, payload: Option<&Value>) {
    for (label, keys) in [
        ("Blocks", &["blocks", "addBlocks"][..]),
        ("Blocked by", &["blockedBy", "addBlockedBy"][..]),
    ] {
        let count = payload
            .and_then(|value| keys.iter().find_map(|key| value.get(*key)))
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        if count > 0 {
            body.push(Line::from(vec![
                Span::styled(format!("{label} "), subtle_style()),
                Span::styled(format!("{count} task(s)"), inactive_style()),
            ]));
        }
    }
}

fn append_preview_text_lines(
    body: &mut Vec<StyledLine>,
    label: &str,
    value: &str,
    limit: usize,
    inner_width: usize,
) {
    let lines = value
        .lines()
        .map(collapse_inline_whitespace)
        .filter(|line| !line.is_empty())
        .take(limit)
        .collect::<Vec<_>>();
    if lines.is_empty() {
        body.push(Line::from(vec![
            Span::styled(format!("{label} "), subtle_style()),
            Span::styled("none", subtle_style()),
        ]));
        return;
    }

    body.push(Line::from(Span::styled(
        format!("{label}:"),
        subtle_style(),
    )));
    let indent = "  ";
    let content_width = inner_width.saturating_sub(display_width_str(indent)).max(1);
    for line in lines {
        let content_line = Line::from(Span::styled(line, inactive_style()));
        for wrapped in wrap_styled_line(&content_line, content_width) {
            let mut spans = vec![Span::styled(indent, subtle_style())];
            spans.extend(wrapped.spans);
            body.push(Line::from(spans));
        }
    }
}

fn append_todo_item_preview_lines(body: &mut Vec<StyledLine>, items: &[Value], limit: usize) {
    for (index, item) in items.iter().take(limit).enumerate() {
        let title = match item {
            Value::String(value) => value.clone(),
            Value::Object(map) => map
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("(missing title)")
                .to_string(),
            _ => human_value(item),
        };
        let done = match item {
            Value::Object(map) => map.get("done").and_then(Value::as_bool),
            _ => None,
        };
        let state = match done {
            Some(true) => "done",
            Some(false) => "todo",
            None => "todo",
        };
        body.push(Line::from(vec![
            Span::styled(format!("  {}. ", index + 1), subtle_style()),
            Span::styled(
                truncate_chars(&collapse_inline_whitespace(&title), 100),
                inactive_style(),
            ),
            Span::styled(format!(" ({state})"), subtle_style()),
        ]));
    }
    if items.len() > limit {
        body.push(Line::from(Span::styled(
            format!("  ... {} more", items.len() - limit),
            subtle_style(),
        )));
    }
}

fn append_allowed_prompt_preview_lines(
    body: &mut Vec<StyledLine>,
    prompts: &[Value],
    limit: usize,
) {
    for (index, prompt) in prompts.iter().take(limit).enumerate() {
        let tool = prompt.get("tool").and_then(Value::as_str).unwrap_or("tool");
        let text = prompt
            .get("prompt")
            .and_then(Value::as_str)
            .unwrap_or_default();
        body.push(Line::from(vec![
            Span::styled(format!("  {}. ", index + 1), subtle_style()),
            Span::styled(tool.to_string(), Style::default().fg(active_palette().tool)),
            Span::styled(" ", subtle_style()),
            Span::styled(
                truncate_chars(&collapse_inline_whitespace(text), 100),
                inactive_style(),
            ),
        ]));
    }
    if prompts.len() > limit {
        body.push(Line::from(Span::styled(
            format!("  ... {} more", prompts.len() - limit),
            subtle_style(),
        )));
    }
}

fn append_workflow_step_preview_lines(body: &mut Vec<StyledLine>, steps: &[Value], limit: usize) {
    for (index, step) in steps.iter().take(limit).enumerate() {
        let (kind, label) = workflow_step_summary(step);
        body.push(Line::from(vec![
            Span::styled(format!("  {}. ", index + 1), subtle_style()),
            Span::styled(kind, Style::default().fg(active_palette().tool)),
            Span::styled(" ", subtle_style()),
            Span::styled(truncate_chars(&label, 100), inactive_style()),
        ]));
    }
    if steps.len() > limit {
        body.push(Line::from(Span::styled(
            format!("  ... {} more", steps.len() - limit),
            subtle_style(),
        )));
    }
}

fn workflow_step_summary(step: &Value) -> (&'static str, String) {
    if let Some(agent) = step.get("agent") {
        return (
            "agent",
            string_value_any(agent, &["description"]).unwrap_or_else(|| "(no description)".into()),
        );
    }
    if let Some(phase) = step.get("phase") {
        return (
            "phase",
            string_value_any(phase, &["name"]).unwrap_or_else(|| "(unnamed phase)".into()),
        );
    }
    if step.get("parallel").is_some() {
        return ("parallel", "parallel steps".to_string());
    }
    if step.get("pipeline").is_some() {
        return ("pipeline", "pipeline steps".to_string());
    }
    if let Some(log) = step.get("log") {
        return (
            "log",
            string_value_any(log, &["message"]).unwrap_or_else(|| "(empty log)".into()),
        );
    }
    ("unknown", human_value(step))
}

fn human_todo_mode(mode: &str) -> String {
    match mode {
        "replace" => "replace list".to_string(),
        "append" => "append to list".to_string(),
        other => human_field_label(other).to_ascii_lowercase(),
    }
}

fn append_generic_permission_preview(
    body: &mut Vec<StyledLine>,
    request: &PermissionRequest,
    payload: Option<&Value>,
) {
    let canonical = canonical_permission_tool_name(&request.tool_name);
    body.push(Line::from(vec![
        Span::styled("Tool ", subtle_style()),
        Span::styled(
            human_tool_name(&request.tool_name),
            Style::default()
                .fg(active_palette().tool)
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    match payload {
        Some(Value::Object(map)) if !map.is_empty() => {
            for (key, value) in ordered_permission_fields(&canonical, map) {
                append_human_readable_value(body, &human_field_label(&key), &value, 0);
            }
        }
        Some(value) => {
            append_human_readable_value(body, "Input", value, 0);
        }
        None if !request.tool_input.trim().is_empty() => {
            body.push(Line::from(vec![
                Span::styled("Input ", subtle_style()),
                Span::styled(request.tool_input.clone(), inactive_style()),
            ]));
        }
        None => {
            body.push(Line::from(Span::styled("No input", subtle_style())));
        }
    }
}

fn code_preview_lines(
    code: &str,
    path: &str,
    _inner_width: usize,
    marker: char,
) -> Vec<StyledLine> {
    let lines = code.lines().collect::<Vec<_>>();
    let line_no_width = lines.len().max(1).to_string().chars().count().max(1);
    let extension = Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default();
    let mut rendered = Vec::new();

    for (index, line) in lines.iter().enumerate() {
        let line_number = index + 1;
        let mut spans = vec![
            Span::styled(format!("{line_number:>line_no_width$}"), subtle_style()),
            Span::raw(" "),
            Span::styled(
                marker.to_string(),
                Style::default().fg(active_palette().success),
            ),
            Span::raw("  "),
        ];
        spans.extend(highlight_code_line(line, extension));
        rendered.push(Line::from(spans));
    }

    if rendered.is_empty() {
        rendered.push(Line::from(Span::styled("1 +", subtle_style())));
    }

    rendered
}

fn ordered_permission_fields(
    canonical_tool: &str,
    map: &serde_json::Map<String, Value>,
) -> Vec<(String, Value)> {
    let preferred = match canonical_tool {
        "Agent" => &["description", "prompt", "subagent_type", "subagentType"][..],
        "glob" => &["pattern", "glob", "path", "base"][..],
        "grep" => &[
            "pattern",
            "query",
            "path",
            "glob",
            "output_mode",
            "-n",
            "-i",
            "head_limit",
        ][..],
        "notebook-edit" => &["path", "cell_type", "source"][..],
        "web-fetch" => &["url"][..],
        "web-search" => &["query"][..],
        "ask-user-question" => &["question", "options"][..],
        "todo-write" => &["list", "mode", "items"][..],
        "task-create" => &[
            "subject",
            "description",
            "activeForm",
            "owner",
            "status",
            "blocks",
            "addBlocks",
            "blockedBy",
            "addBlockedBy",
            "metadata",
        ][..],
        "task-get" => &["taskId", "task_id"][..],
        "task-update" => &[
            "taskId",
            "task_id",
            "subject",
            "description",
            "activeForm",
            "owner",
            "status",
            "blocks",
            "addBlocks",
            "blockedBy",
            "addBlockedBy",
            "metadata",
        ][..],
        "task-output" => &["task_id", "taskId", "block", "timeout"][..],
        "task-stop" => &["task_id", "taskId", "shell_id"][..],
        "exit-plan-mode" => &["plan", "allowedPrompts"][..],
        "skill" => &["skill", "args"][..],
        "tool-search" => &["query", "max_results"][..],
        "lsp" => &["operation", "filePath", "file_path", "line", "character"][..],
        "list-mcp-resources" | "list-mcp-tools" => &["server_id"][..],
        "read-mcp-resource" => &["server_id", "uri"][..],
        "call-mcp-tool" => &["server_id", "tool_name", "tool", "input"][..],
        _ => &[][..],
    };

    let mut fields = Vec::new();
    let mut seen = HashSet::new();
    for key in preferred {
        if let Some(value) = map.get(*key) {
            fields.push(((*key).to_string(), value.clone()));
            seen.insert((*key).to_string());
        }
    }
    for (key, value) in map {
        if seen.insert(key.clone()) {
            fields.push((key.clone(), value.clone()));
        }
    }
    fields
}

fn append_human_readable_value(
    body: &mut Vec<StyledLine>,
    label: &str,
    value: &Value,
    indent: usize,
) {
    let prefix = "  ".repeat(indent);
    match value {
        Value::Array(items) if items.is_empty() => {
            append_label_value_line(body, &prefix, label, "none");
        }
        Value::Array(items) => {
            append_label_value_line(body, &prefix, label, &format!("{} item(s)", items.len()));
            for (index, item) in items.iter().enumerate() {
                match item {
                    Value::Object(map) => {
                        body.push(Line::from(vec![
                            Span::raw(format!("{prefix}  ")),
                            Span::styled(format!("{}.", index + 1), subtle_style()),
                        ]));
                        for (key, value) in map {
                            append_human_readable_value(
                                body,
                                &human_field_label(key),
                                value,
                                indent.saturating_add(2),
                            );
                        }
                    }
                    _ => append_label_value_line(
                        body,
                        &format!("{prefix}  "),
                        &format!("{}.", index + 1),
                        &human_value(item),
                    ),
                }
            }
        }
        Value::Object(map) if map.is_empty() => {
            append_label_value_line(body, &prefix, label, "none");
        }
        Value::Object(map) => {
            append_label_value_line(body, &prefix, label, "");
            for (key, value) in map {
                append_human_readable_value(
                    body,
                    &human_field_label(key),
                    value,
                    indent.saturating_add(1),
                );
            }
        }
        _ => append_label_value_line(body, &prefix, label, &human_value(value)),
    }
}

fn append_label_value_line(body: &mut Vec<StyledLine>, prefix: &str, label: &str, value: &str) {
    let mut spans = vec![
        Span::raw(prefix.to_string()),
        Span::styled(format!("{label} "), subtle_style()),
    ];
    if !value.is_empty() {
        spans.push(Span::styled(value.to_string(), inactive_style()));
    }
    body.push(Line::from(spans));
}

fn human_value(value: &Value) -> String {
    match value {
        Value::Null => "none".to_string(),
        Value::Bool(value) => {
            if *value {
                "true".to_string()
            } else {
                "false".to_string()
            }
        }
        Value::Number(value) => value.to_string(),
        Value::String(value) => value.clone(),
        Value::Array(items) => format!("{} item(s)", items.len()),
        Value::Object(map) => format!("{} field(s)", map.len()),
    }
}

fn human_grep_output_mode(value: &str) -> String {
    match value {
        "content" => "matching lines".to_string(),
        "files_with_matches" => "matching files".to_string(),
        "count" => "match counts".to_string(),
        other => human_field_label(other).to_ascii_lowercase(),
    }
}

pub(crate) fn apply_permission_request_key(
    permission: &mut PermissionOverlayState,
    key_event: &KeyEvent,
) -> PermissionRequestKeyAction {
    match key_event.code {
        KeyCode::Char('c') if key_event.modifiers.contains(KeyModifiers::CONTROL) => {
            PermissionRequestKeyAction::Permission {
                request_id: permission.request.request_id.clone(),
                decision: PermissionDecision::Deny,
            }
        }
        KeyCode::Char('o') if key_event.modifiers.contains(KeyModifiers::CONTROL) => {
            PermissionRequestKeyAction::ToggleDetails
        }
        KeyCode::PageUp => {
            permission.panel_scroll = permission.panel_scroll.saturating_add(6);
            PermissionRequestKeyAction::None
        }
        KeyCode::PageDown => {
            permission.panel_scroll = permission.panel_scroll.saturating_sub(6);
            PermissionRequestKeyAction::None
        }
        KeyCode::Home => {
            permission.panel_scroll = usize::MAX / 2;
            PermissionRequestKeyAction::None
        }
        KeyCode::End => {
            permission.panel_scroll = 0;
            PermissionRequestKeyAction::None
        }
        KeyCode::Up => {
            if permission.selected_option > 0 {
                permission.selected_option -= 1;
                permission.editing_rule = false;
            }
            PermissionRequestKeyAction::None
        }
        KeyCode::Down | KeyCode::Tab => {
            if permission.editing_rule {
                permission.editing_rule = false;
            } else if key_event.code == KeyCode::Tab
                && permission.always_allow_index() == Some(permission.selected_option)
                && permission.can_edit_always_allow()
            {
                permission.editing_rule = true;
            } else if permission.selected_option + 1 < permission.option_count() {
                permission.selected_option += 1;
            } else {
                permission.selected_option = 0;
            }
            PermissionRequestKeyAction::None
        }
        KeyCode::Backspace if permission.editing_rule => {
            permission.always_allow_rule.pop();
            PermissionRequestKeyAction::None
        }
        KeyCode::Char(character)
            if permission.editing_rule
                && !key_event.modifiers.contains(KeyModifiers::CONTROL)
                && !key_event.modifiers.contains(KeyModifiers::ALT) =>
        {
            permission.always_allow_rule.push(character);
            PermissionRequestKeyAction::None
        }
        KeyCode::Enter | KeyCode::Char('y' | 'Y') => {
            // While editing an always-allow rule that is currently empty, a
            // confirm must not fall through to a decision: clearing the rule
            // shrinks `option_count()`, so `deny_index()` collapses onto the
            // selected index and Enter would silently emit a Deny (or Approve).
            // Keep the dialog open until the user types a rule or navigates.
            if permission.editing_rule && permission.always_allow_rules_for_decision().is_empty() {
                return PermissionRequestKeyAction::None;
            }
            let decision = if permission.always_allow_index() == Some(permission.selected_option)
                && !permission.always_allow_rules_for_decision().is_empty()
            {
                let rules = permission.always_allow_rules_for_decision();
                if rules.len() == 1 {
                    PermissionDecision::ApproveAlways(rules[0].clone())
                } else {
                    PermissionDecision::ApproveAlwaysMany(rules)
                }
            } else if permission.selected_option == permission.deny_index() {
                // The deny option is selected: a `y`/`Y` shortcut must not
                // override an explicit deny selection into an approve.
                PermissionDecision::Deny
            } else {
                PermissionDecision::Approve
            };
            PermissionRequestKeyAction::Permission {
                request_id: permission.request.request_id.clone(),
                decision,
            }
        }
        KeyCode::Esc | KeyCode::Char('n' | 'N') => PermissionRequestKeyAction::Permission {
            request_id: permission.request.request_id.clone(),
            decision: PermissionDecision::Deny,
        },
        _ => PermissionRequestKeyAction::None,
    }
}

#[cfg(test)]
pub(crate) fn permission_overlay_area(body: &[StyledLine], host_area: Rect) -> Rect {
    let max_width = host_area.width.saturating_sub(2).max(1);
    let preferred_width = host_area.width.saturating_mul(76).saturating_div(100);
    let width = preferred_width.clamp(max_width.min(48), max_width);
    let inner_width = width.saturating_sub(2).max(1) as usize;
    let body_rows = wrap_styled_lines(body, inner_width).len() as u16;
    let max_height = host_area.height.saturating_sub(2).max(3);
    let min_height = 10.min(max_height);
    let height = body_rows.saturating_add(2).clamp(min_height, max_height);
    centered_rect_with_size(width, height, host_area)
}

#[cfg(test)]
fn centered_rect_with_size(width: u16, height: u16, area: Rect) -> Rect {
    let width = width.max(1).min(area.width);
    let height = height.max(1).min(area.height);
    Rect {
        x: area.x.saturating_add(area.width.saturating_sub(width) / 2),
        y: area
            .y
            .saturating_add(area.height.saturating_sub(height) / 2),
        width,
        height,
    }
}

#[cfg(test)]
pub(crate) fn permission_panel_height(body: &[StyledLine], host_area: Rect) -> u16 {
    if host_area.height == 0 {
        return 0;
    }
    let inner_width = host_area.width.saturating_sub(2).max(1) as usize;
    let max_height = host_area.height.max(1);
    let min_height = 8.min(max_height);
    permission_panel_full_height(body, inner_width).clamp(min_height, max_height)
}

#[cfg(test)]
pub(crate) fn permission_panel_height_with_context(body: &[StyledLine], host_area: Rect) -> u16 {
    let full_height = permission_panel_height(body, host_area);
    let context_height = permission_panel_context_height(host_area.height, full_height);
    full_height.min(host_area.height.saturating_sub(context_height).max(1))
}

pub(crate) fn permission_panel_height_with_context_from_wrapped(
    wrapped_body: &[StyledLine],
    host_area: Rect,
) -> u16 {
    let full_height = permission_panel_height_from_wrapped(wrapped_body, host_area);
    let context_height = permission_panel_context_height(host_area.height, full_height);
    full_height.min(host_area.height.saturating_sub(context_height).max(1))
}

fn permission_panel_height_from_wrapped(wrapped_body: &[StyledLine], host_area: Rect) -> u16 {
    if host_area.height == 0 {
        return 0;
    }
    let max_height = host_area.height.max(1);
    let min_height = 8.min(max_height);
    permission_panel_full_height_from_wrapped(wrapped_body).clamp(min_height, max_height)
}

fn permission_panel_context_height(host_height: u16, panel_height: u16) -> u16 {
    const TARGET_CONTEXT_ROWS: u16 = 8;
    const MIN_PANEL_ROWS: u16 = 8;

    if panel_height.saturating_add(TARGET_CONTEXT_ROWS) <= host_height {
        return 0;
    }
    TARGET_CONTEXT_ROWS.min(host_height.saturating_sub(MIN_PANEL_ROWS))
}

#[cfg(test)]
pub(crate) fn permission_panel_full_height(body: &[StyledLine], inner_width: usize) -> u16 {
    saturating_u16(
        wrap_styled_lines(body, inner_width.max(1))
            .len()
            .saturating_add(2),
    )
}

pub(crate) fn permission_panel_full_height_from_wrapped(wrapped_body: &[StyledLine]) -> u16 {
    saturating_u16(wrapped_body.len().saturating_add(2))
}

#[cfg(test)]
pub(crate) fn permission_panel_viewport(
    body: &[StyledLine],
    inner_width: usize,
    inner_height: u16,
    scroll_from_bottom: usize,
) -> PermissionPanelViewport {
    let wrapped = wrap_styled_lines(body, inner_width.max(1));
    permission_panel_viewport_from_wrapped(&wrapped, inner_height, scroll_from_bottom)
}

pub(crate) fn permission_panel_viewport_from_wrapped(
    wrapped: &[StyledLine],
    inner_height: u16,
    scroll_from_bottom: usize,
) -> PermissionPanelViewport {
    let visible_height = inner_height as usize;
    if visible_height == 0 || wrapped.is_empty() {
        return PermissionPanelViewport {
            body: Vec::new(),
            all_lines: Vec::new(),
            first_row: 0,
            actual_scroll: 0,
            max_scroll: 0,
        };
    }
    if wrapped.len() <= visible_height {
        return PermissionPanelViewport {
            body: wrapped.to_vec(),
            all_lines: wrapped.to_vec(),
            first_row: 0,
            actual_scroll: 0,
            max_scroll: 0,
        };
    }

    let max_scroll = wrapped.len().saturating_sub(visible_height);
    let scroll = scroll_from_bottom.min(max_scroll);
    let first_row = max_scroll.saturating_sub(scroll);
    let last_row = first_row.saturating_add(visible_height).min(wrapped.len());
    PermissionPanelViewport {
        body: wrapped[first_row..last_row].to_vec(),
        all_lines: wrapped.to_vec(),
        first_row,
        actual_scroll: scroll,
        max_scroll,
    }
}

pub(crate) fn draw_permission_panel(
    frame: &mut Frame,
    permission: &mut PermissionOverlayState,
    area: Rect,
) -> Option<(u16, u16)> {
    let inner_width = area.width.saturating_sub(2).max(1) as usize;
    let inner_height = area.height.saturating_sub(2);
    let panel_scroll = permission.panel_scroll;
    let (viewport, edit_cursor) = {
        let cached = permission.cached_panel_content(inner_width);
        (
            permission_panel_viewport_from_wrapped(cached.wrapped_body, inner_height, panel_scroll),
            cached.content.edit_cursor,
        )
    };
    let content_area = Rect {
        x: area.x.saturating_add(1),
        y: area.y.saturating_add(1),
        width: area.width.saturating_sub(2),
        height: inner_height,
    };
    permission.viewport.sync(
        content_area,
        viewport.body,
        viewport.all_lines,
        viewport.first_row,
        viewport.actual_scroll,
        viewport.max_scroll,
    );
    frame.render_widget(Clear, area);
    let panel = Paragraph::new(permission.viewport.render_lines()).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded),
    );
    frame.render_widget(panel, area);
    edit_cursor.and_then(|(x, y)| {
        let y = y as usize;
        let visible_end = viewport.first_row.saturating_add(inner_height as usize);
        if y < viewport.first_row || y >= visible_end {
            return None;
        }
        let visible_y = saturating_u16(y.saturating_sub(viewport.first_row));
        Some((
            area.x.saturating_add(1).saturating_add(x),
            area.y
                .saturating_add(1)
                .saturating_add(visible_y)
                .min(area.bottom().saturating_sub(2)),
        ))
    })
}
