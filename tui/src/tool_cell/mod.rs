use std::collections::HashMap;

use ratatui::prelude::Style;
use serde_json::Value;

pub(crate) mod live_state;
pub(crate) mod render;
pub(crate) mod summary;
pub(crate) mod utils;

#[derive(Clone, Debug)]
pub(crate) struct ToolCell {
    pub(crate) tool_use_id: String,
    pub(crate) tool_name: String,
    pub(crate) title: String,
    pub(crate) title_style: Style,
    pub(crate) status_line: String,
    pub(crate) detail_lines: Vec<String>,
    pub(crate) collapsed_preview_lines: Vec<String>,
    pub(crate) prompt: Option<String>,
    pub(crate) progress_messages: Vec<Value>,
    pub(crate) response: Option<String>,
    pub(crate) collapsed_preview_limit: usize,
    pub(crate) is_error: bool,
    pub(crate) is_active: bool,
}

pub(crate) type ToolResultIndex = HashMap<String, ToolResultRecord>;

#[derive(Clone, Debug)]
pub(crate) struct ToolResultRecord {
    pub(crate) content: String,
    pub(crate) is_error: bool,
    pub(crate) metadata: Option<String>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ToolUseSpec<'a> {
    pub(crate) id: &'a str,
    pub(crate) name: &'a str,
    pub(crate) input: &'a str,
}
