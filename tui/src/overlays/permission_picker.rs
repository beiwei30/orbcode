use anyhow::Result;

use crate::commands::permissions::{
    apply_permission_rule_update as apply_permission_rule_settings_update,
    permission_rule_update_command,
};
use crate::numeric::saturating_u16;
use crate::render::permission_labels::{
    canonical_permission_tool_name, human_tool_name, string_value_any, suggested_permission_rule,
};
use crate::state::TuiState;

use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PermissionPickerKeyAction {
    None,
    Status(String),
    Close {
        status: String,
    },
    AddRule {
        command: String,
        scope: PermissionRuleScope,
        kind: PermissionRuleSettingKind,
        rule: String,
    },
    RemoveRule {
        command: String,
        scope: PermissionRuleScope,
        kind: PermissionRuleSettingKind,
        rule: String,
    },
}

#[derive(Clone, Debug)]
pub(crate) struct PermissionPickerState {
    pub(crate) command: String,
    pub(crate) overview: PermissionOverview,
    pub(crate) recent_denied: Vec<RecentlyDeniedPermission>,
    pub(crate) tab: PermissionPickerTab,
    pub(crate) focus: PermissionPickerFocus,
    pub(crate) search_query: String,
    pub(crate) search_active: bool,
    pub(crate) items: Vec<PermissionPickerItem>,
    pub(crate) selected: usize,
    pub(crate) adding: Option<PermissionAddDraft>,
    pub(crate) add_destination: Option<PermissionAddDestinationState>,
    pub(crate) rule_details: Option<PermissionRuleDetailsState>,
    pub(crate) lines_cache: PermissionPickerLinesCache,
}

pub(crate) type PermissionPickerLinesCache = LinesCache<PermissionPickerLinesCacheKey>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PermissionPickerLinesCacheKey {
    width: usize,
    selected: usize,
    focus: PermissionPickerFocus,
    search_query: String,
    search_active: bool,
    adding: Option<PermissionAddDraft>,
    add_destination: Option<PermissionAddDestinationState>,
    rule_details: Option<PermissionRuleDetailsState>,
    allow_all: bool,
    allow_tools: bool,
    allow_network: bool,
    provider_allow_network: bool,
    additional_directories: usize,
    configured_additional_directories: usize,
    session_additional_directories: usize,
    recent_denied: Vec<RecentlyDeniedPermission>,
    tab: PermissionPickerTab,
    items: Vec<PermissionPickerItem>,
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub(crate) enum PermissionPickerTab {
    RecentlyDenied,
    Allow,
    Ask,
    Deny,
    Workspace,
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub(crate) enum PermissionPickerFocus {
    Header,
    Content,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PermissionPickerItem {
    Rule(PermissionPickerRuleItem),
    RecentlyDenied(RecentlyDeniedPermission),
    AddRule(PermissionRuleSettingKind),
    Directory { path: String, source: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PermissionPickerRuleItem {
    pub(crate) kind: PermissionRuleSettingKind,
    pub(crate) rule: String,
    pub(crate) source: String,
    pub(crate) scope: Option<PermissionRuleScope>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RecentlyDeniedPermission {
    pub(crate) tool_name: String,
    pub(crate) detail: String,
    pub(crate) suggested_rule: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PermissionAddDraft {
    pub(crate) scope: PermissionRuleScope,
    pub(crate) kind: PermissionRuleSettingKind,
    pub(crate) rule: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PermissionAddDestinationState {
    pub(crate) draft: PermissionAddDraft,
    pub(crate) selected: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PermissionRuleDetailsState {
    pub(crate) rule: PermissionPickerRuleItem,
    pub(crate) selected: usize,
}

impl TuiState {
    pub(crate) fn open_permission_picker(&mut self, command: &str, overview: PermissionOverview) {
        self.overlay = Some(OverlayState::PermissionPicker(PermissionPickerState::new(
            command,
            overview,
            self.recent_denied_permissions.clone(),
        )));
        self.set_status_line("Permissions: ←/→ tabs, ↓ select, Esc close.");
    }

    pub(crate) fn record_recent_denied_permission(&mut self, request: PermissionRequest) {
        let denied = recently_denied_permission_from_request(&request);
        self.recent_denied_permissions
            .retain(|existing| existing != &denied);
        self.recent_denied_permissions.push(denied);
        const RECENT_DENIED_LIMIT: usize = 20;
        if self.recent_denied_permissions.len() > RECENT_DENIED_LIMIT {
            let excess = self.recent_denied_permissions.len() - RECENT_DENIED_LIMIT;
            self.recent_denied_permissions.drain(0..excess);
        }
    }

    pub(crate) async fn apply_permission_rule_update(
        &mut self,
        app_server: &AppClient,
        command: impl Into<String>,
        action: PermissionRuleAction,
        scope: PermissionRuleScope,
        kind: PermissionRuleSettingKind,
        rule: String,
    ) -> Result<()> {
        let command = command.into();
        let normalized_rule = match normalize_permission_rule_for_edit(&rule) {
            Ok(rule) => rule,
            Err(message) => {
                self.set_status_line(format!("Invalid permission rule: {message}"));
                return Ok(());
            }
        };
        let (summary, detail, overview) = apply_permission_rule_settings_update(
            app_server,
            &self.session_id,
            action,
            scope,
            kind,
            &normalized_rule,
        )
        .await?;
        if let Some(OverlayState::PermissionPicker(picker)) = self.overlay.as_mut() {
            picker.adding = None;
            picker.add_destination = None;
            picker.rule_details = None;
            match action {
                PermissionRuleAction::Add => {
                    picker.refresh_overview_and_focus_rule(overview, kind, &normalized_rule, scope);
                }
                PermissionRuleAction::Remove => {
                    picker.refresh_overview(overview);
                }
            }
        }
        let output_command =
            permission_rule_update_command(&command, action, scope, kind, &normalized_rule);
        self.push_local_slash_command_output(output_command, summary.clone(), Some(detail));
        self.set_status_line(summary);
        Ok(())
    }
}

impl PermissionPickerState {
    pub(crate) fn new(
        command: impl Into<String>,
        overview: PermissionOverview,
        recent_denied: Vec<RecentlyDeniedPermission>,
    ) -> Self {
        let tab = PermissionPickerTab::Allow;
        let items = permission_picker_items(&overview, &recent_denied, tab, "");
        let selected = items
            .iter()
            .position(permission_picker_item_is_selectable)
            .unwrap_or(0);
        Self {
            command: command.into(),
            overview,
            recent_denied,
            tab,
            focus: PermissionPickerFocus::Header,
            search_query: String::new(),
            search_active: false,
            items,
            selected,
            adding: None,
            add_destination: None,
            rule_details: None,
            lines_cache: PermissionPickerLinesCache::default(),
        }
    }

    pub(crate) fn refresh_overview(&mut self, overview: PermissionOverview) {
        self.overview = overview;
        self.refresh_items();
        self.selected = self.selected.min(self.items.len().saturating_sub(1));
        self.lines_cache.invalidate();
    }

    pub(crate) fn refresh_overview_and_focus_rule(
        &mut self,
        overview: PermissionOverview,
        kind: PermissionRuleSettingKind,
        rule: &str,
        scope: PermissionRuleScope,
    ) {
        self.overview = overview;
        self.tab = permission_picker_tab_for_kind(kind);
        self.focus = PermissionPickerFocus::Content;
        self.search_active = false;
        self.search_query.clear();
        self.refresh_items();
        self.selected = self
            .items
            .iter()
            .position(|item| {
                matches!(
                    item,
                    PermissionPickerItem::Rule(candidate)
                        if candidate.kind == kind
                            && candidate.rule == rule
                            && candidate.scope == Some(scope)
                )
            })
            .or_else(|| {
                self.items
                    .iter()
                    .position(permission_picker_item_is_selectable)
            })
            .unwrap_or(0);
        self.lines_cache.invalidate();
    }

    fn refresh_items(&mut self) {
        self.items = permission_picker_items(
            &self.overview,
            &self.recent_denied,
            self.tab,
            &self.search_query,
        );
    }

    pub(crate) fn set_tab(&mut self, tab: PermissionPickerTab) {
        if self.tab == tab {
            return;
        }
        self.tab = tab;
        self.adding = None;
        self.add_destination = None;
        self.rule_details = None;
        self.search_active = false;
        self.search_query.clear();
        self.refresh_items();
        self.selected = self
            .items
            .iter()
            .position(permission_picker_item_is_selectable)
            .unwrap_or(0);
        self.lines_cache.invalidate();
    }

    pub(crate) fn select_next_tab(&mut self) {
        self.set_tab(permission_picker_next_tab(self.tab));
    }

    pub(crate) fn select_previous_tab(&mut self) {
        self.set_tab(permission_picker_previous_tab(self.tab));
    }

    pub(crate) fn selected_item(&self) -> Option<&PermissionPickerItem> {
        self.items.get(self.selected)
    }

    pub(crate) fn item_count(&self) -> usize {
        self.items.len()
    }

    pub(crate) fn start_add(
        &mut self,
        scope: PermissionRuleScope,
        kind: PermissionRuleSettingKind,
    ) {
        self.adding = Some(PermissionAddDraft {
            scope,
            kind,
            rule: String::new(),
        });
        self.lines_cache.invalidate();
    }

    pub(crate) fn cancel_add(&mut self) {
        self.adding = None;
        self.add_destination = None;
        self.lines_cache.invalidate();
    }

    pub(crate) fn set_search_query(&mut self, query: String) {
        self.search_query = query;
        self.refresh_items();
        self.selected = self.selected.min(self.items.len().saturating_sub(1));
        self.lines_cache.invalidate();
    }

    pub(crate) fn clear_search(&mut self) {
        self.search_active = false;
        if !self.search_query.is_empty() {
            self.search_query.clear();
            self.refresh_items();
            self.selected = self.selected.min(self.items.len().saturating_sub(1));
        }
        self.lines_cache.invalidate();
    }

    pub(crate) fn begin_search(&mut self, query: String) {
        if !permission_picker_tab_has_search(self.tab) {
            return;
        }
        self.focus = PermissionPickerFocus::Content;
        self.search_active = true;
        self.set_search_query(query);
    }

    pub(crate) fn cached_lines(&mut self, width: usize) -> &[StyledLine] {
        let key = PermissionPickerLinesCacheKey {
            width,
            selected: self.selected,
            focus: self.focus,
            search_query: self.search_query.clone(),
            search_active: self.search_active,
            adding: self.adding.clone(),
            add_destination: self.add_destination.clone(),
            rule_details: self.rule_details.clone(),
            allow_all: self.overview.allow_all,
            allow_tools: self.overview.permissions.allow_tools,
            allow_network: self.overview.permissions.allow_network,
            provider_allow_network: self.overview.permissions.provider_allow_network,
            additional_directories: self.overview.permissions.additional_directories.len(),
            configured_additional_directories: self
                .overview
                .configured_additional_directories
                .len(),
            session_additional_directories: self.overview.session_additional_directories.len(),
            recent_denied: self.recent_denied.clone(),
            tab: self.tab,
            items: self.items.clone(),
        };
        let mut lines_cache = std::mem::take(&mut self.lines_cache);
        lines_cache.refresh(key, || permission_picker_lines(self, width));
        self.lines_cache = lines_cache;
        &self.lines_cache.lines
    }
}

pub(crate) fn apply_permission_picker_key(
    picker: &mut PermissionPickerState,
    key_event: &KeyEvent,
) -> PermissionPickerKeyAction {
    if picker.rule_details.is_some() {
        return apply_permission_rule_details_key(picker, key_event);
    }
    if picker.add_destination.is_some() {
        return apply_permission_add_destination_key(picker, key_event);
    }
    if picker.adding.is_some() {
        return apply_permission_add_input_key(picker, key_event);
    }

    match key_event.code {
        // `q` must be typeable into the search box, so only treat it as
        // clear/close when the search is NOT active (it then falls through to
        // the search char-input arm below). Esc always clears/closes.
        KeyCode::Esc | KeyCode::Char('q') if !picker.search_active => {
            if picker.search_active || !picker.search_query.is_empty() {
                picker.clear_search();
                PermissionPickerKeyAction::Status("Cleared permission search.".to_string())
            } else {
                PermissionPickerKeyAction::Close {
                    status: "Closed permissions.".to_string(),
                }
            }
        }
        KeyCode::Esc if picker.search_active => {
            picker.clear_search();
            PermissionPickerKeyAction::Status("Cleared permission search.".to_string())
        }
        KeyCode::Left if !picker.search_active => {
            picker.select_previous_tab();
            PermissionPickerKeyAction::Status("Permissions: switched tab.".to_string())
        }
        KeyCode::Right | KeyCode::Tab if !picker.search_active => {
            picker.select_next_tab();
            PermissionPickerKeyAction::Status("Permissions: switched tab.".to_string())
        }
        KeyCode::Backspace if picker.search_active => {
            let mut query = picker.search_query.clone();
            query.pop();
            picker.set_search_query(query);
            PermissionPickerKeyAction::None
        }
        KeyCode::Enter | KeyCode::Down if picker.search_active => {
            picker.search_active = false;
            picker.focus = PermissionPickerFocus::Content;
            picker.lines_cache.invalidate();
            PermissionPickerKeyAction::None
        }
        KeyCode::Up if picker.search_active => {
            picker.search_active = false;
            picker.focus = PermissionPickerFocus::Header;
            picker.lines_cache.invalidate();
            PermissionPickerKeyAction::None
        }
        KeyCode::Char(character)
            if picker.search_active
                && !key_event.modifiers.contains(KeyModifiers::CONTROL)
                && !key_event.modifiers.contains(KeyModifiers::ALT) =>
        {
            let mut query = picker.search_query.clone();
            query.push(character);
            picker.set_search_query(query);
            PermissionPickerKeyAction::None
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if picker.focus == PermissionPickerFocus::Header {
                // Already on tabs.
            } else if picker.selected == 0 {
                picker.focus = PermissionPickerFocus::Header;
                picker.lines_cache.invalidate();
            } else {
                picker.selected -= 1;
            }
            PermissionPickerKeyAction::None
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if picker.focus == PermissionPickerFocus::Header {
                picker.focus = PermissionPickerFocus::Content;
                picker.lines_cache.invalidate();
            } else if picker.selected + 1 < picker.item_count() {
                picker.selected += 1;
            }
            PermissionPickerKeyAction::None
        }
        KeyCode::PageUp => {
            picker.selected = picker.selected.saturating_sub(8);
            PermissionPickerKeyAction::None
        }
        KeyCode::PageDown => {
            picker.selected = (picker.selected + 8).min(picker.item_count().saturating_sub(1));
            PermissionPickerKeyAction::None
        }
        KeyCode::Home => {
            picker.selected = 0;
            PermissionPickerKeyAction::None
        }
        KeyCode::End => {
            picker.selected = picker.item_count().saturating_sub(1);
            PermissionPickerKeyAction::None
        }
        KeyCode::Enter | KeyCode::Char(' ') => select_permission_picker_item(picker),
        KeyCode::Char(character)
            if !key_event.modifiers.contains(KeyModifiers::CONTROL)
                && !key_event.modifiers.contains(KeyModifiers::ALT)
                && permission_picker_search_start_char(character) =>
        {
            picker.begin_search(if character == '/' {
                String::new()
            } else {
                character.to_string()
            });
            if picker.search_active {
                PermissionPickerKeyAction::Status("Filtering permission rules.".to_string())
            } else {
                PermissionPickerKeyAction::None
            }
        }
        _ => PermissionPickerKeyAction::None,
    }
}

fn apply_permission_rule_details_key(
    picker: &mut PermissionPickerState,
    key_event: &KeyEvent,
) -> PermissionPickerKeyAction {
    match key_event.code {
        KeyCode::Esc => {
            picker.rule_details = None;
            PermissionPickerKeyAction::Status("Cancelled permission rule delete.".to_string())
        }
        KeyCode::Up | KeyCode::Down | KeyCode::Left | KeyCode::Right => {
            if let Some(details) = picker.rule_details.as_mut()
                && details.rule.scope.is_some()
            {
                details.selected = 1usize.saturating_sub(details.selected);
            }
            PermissionPickerKeyAction::None
        }
        KeyCode::Enter | KeyCode::Char(' ') => {
            if let Some(details) = picker.rule_details.clone() {
                if details.rule.scope.is_none() || details.selected == 1 {
                    picker.rule_details = None;
                    PermissionPickerKeyAction::Status(
                        "Cancelled permission rule delete.".to_string(),
                    )
                } else if let Some(scope) = details.rule.scope {
                    PermissionPickerKeyAction::RemoveRule {
                        command: picker.command.clone(),
                        scope,
                        kind: details.rule.kind,
                        rule: details.rule.rule,
                    }
                } else {
                    PermissionPickerKeyAction::None
                }
            } else {
                PermissionPickerKeyAction::None
            }
        }
        _ => PermissionPickerKeyAction::None,
    }
}

fn apply_permission_add_destination_key(
    picker: &mut PermissionPickerState,
    key_event: &KeyEvent,
) -> PermissionPickerKeyAction {
    match key_event.code {
        KeyCode::Esc => {
            picker.add_destination = None;
            PermissionPickerKeyAction::Status("Cancelled permission rule add.".to_string())
        }
        KeyCode::Up | KeyCode::Down | KeyCode::Left | KeyCode::Right => {
            if let Some(destination) = picker.add_destination.as_mut() {
                destination.selected = 1usize.saturating_sub(destination.selected);
                destination.draft.scope =
                    permission_picker_destination_options()[destination.selected].2;
            }
            PermissionPickerKeyAction::None
        }
        KeyCode::Enter | KeyCode::Char(' ') => {
            if let Some(destination) = picker.add_destination.as_ref() {
                PermissionPickerKeyAction::AddRule {
                    command: picker.command.clone(),
                    scope: destination.draft.scope,
                    kind: destination.draft.kind,
                    rule: destination.draft.rule.clone(),
                }
            } else {
                PermissionPickerKeyAction::None
            }
        }
        _ => PermissionPickerKeyAction::None,
    }
}

fn apply_permission_add_input_key(
    picker: &mut PermissionPickerState,
    key_event: &KeyEvent,
) -> PermissionPickerKeyAction {
    match key_event.code {
        KeyCode::Esc => {
            picker.cancel_add();
            PermissionPickerKeyAction::Status("Cancelled permission rule add.".to_string())
        }
        KeyCode::Backspace => {
            if let Some(draft) = picker.adding.as_mut() {
                draft.rule.pop();
                picker.lines_cache.invalidate();
            }
            PermissionPickerKeyAction::None
        }
        KeyCode::Char('u') if key_event.modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(draft) = picker.adding.as_mut() {
                draft.rule.clear();
                picker.lines_cache.invalidate();
            }
            PermissionPickerKeyAction::None
        }
        KeyCode::Enter => {
            if let Some(draft) = picker.adding.clone() {
                if draft.rule.trim().is_empty() {
                    return PermissionPickerKeyAction::Status(
                        "Enter a permission rule before adding.".to_string(),
                    );
                }
                match normalize_permission_rule_for_edit(&draft.rule) {
                    Ok(rule) => {
                        picker.adding = None;
                        picker.add_destination = Some(PermissionAddDestinationState {
                            draft: PermissionAddDraft { rule, ..draft },
                            selected: 0,
                        });
                        picker.lines_cache.invalidate();
                        PermissionPickerKeyAction::Status(
                            "Choose where this rule should be saved.".to_string(),
                        )
                    }
                    Err(message) => PermissionPickerKeyAction::Status(format!(
                        "Invalid permission rule: {message}"
                    )),
                }
            } else {
                PermissionPickerKeyAction::None
            }
        }
        KeyCode::Char(character)
            if !key_event.modifiers.contains(KeyModifiers::CONTROL)
                && !key_event.modifiers.contains(KeyModifiers::ALT) =>
        {
            if let Some(draft) = picker.adding.as_mut() {
                draft.rule.push(character);
                picker.lines_cache.invalidate();
            }
            PermissionPickerKeyAction::None
        }
        _ => PermissionPickerKeyAction::None,
    }
}

fn select_permission_picker_item(picker: &mut PermissionPickerState) -> PermissionPickerKeyAction {
    if picker.focus == PermissionPickerFocus::Header {
        picker.focus = PermissionPickerFocus::Content;
        picker.lines_cache.invalidate();
        return PermissionPickerKeyAction::None;
    }

    let Some(item) = picker.selected_item().cloned() else {
        return PermissionPickerKeyAction::None;
    };
    match item {
        PermissionPickerItem::AddRule(kind) => {
            picker.start_add(PermissionRuleScope::Settings, kind);
            PermissionPickerKeyAction::Status(
                "Enter a permission rule, then choose where to save it.".to_string(),
            )
        }
        PermissionPickerItem::Rule(rule) => {
            picker.rule_details = Some(PermissionRuleDetailsState { rule, selected: 1 });
            picker.lines_cache.invalidate();
            PermissionPickerKeyAction::None
        }
        PermissionPickerItem::RecentlyDenied(denied) => {
            let label = denied.suggested_rule.as_deref().unwrap_or(&denied.detail);
            PermissionPickerKeyAction::Status(format!(
                "Recently denied: {}.",
                truncate_chars(label, 80)
            ))
        }
        PermissionPickerItem::Directory { source, .. } => {
            PermissionPickerKeyAction::Status(format!("Workspace directory from {source}."))
        }
    }
}

pub(crate) const PERMISSION_PICKER_BODY_ROWS: usize = 10;
pub(crate) const PERMISSION_PICKER_PANEL_HEIGHT: usize = 18;

pub(crate) const PERMISSION_PICKER_TABS: [PermissionPickerTab; 5] = [
    PermissionPickerTab::RecentlyDenied,
    PermissionPickerTab::Allow,
    PermissionPickerTab::Ask,
    PermissionPickerTab::Deny,
    PermissionPickerTab::Workspace,
];

pub(crate) fn permission_picker_items(
    overview: &PermissionOverview,
    recent_denied: &[RecentlyDeniedPermission],
    tab: PermissionPickerTab,
    query: &str,
) -> Vec<PermissionPickerItem> {
    let mut items = Vec::new();
    match tab {
        PermissionPickerTab::RecentlyDenied => {
            let lower_query = query.to_lowercase();
            for item in recent_denied.iter().rev() {
                if lower_query.is_empty()
                    || item.tool_name.to_lowercase().contains(&lower_query)
                    || item.detail.to_lowercase().contains(&lower_query)
                    || item
                        .suggested_rule
                        .as_deref()
                        .is_some_and(|rule| rule.to_lowercase().contains(&lower_query))
                {
                    items.push(PermissionPickerItem::RecentlyDenied(item.clone()));
                }
            }
        }
        PermissionPickerTab::Allow => {
            if query.is_empty() {
                items.push(PermissionPickerItem::AddRule(
                    PermissionRuleSettingKind::Allow,
                ));
            }
            let mut rules = Vec::new();
            push_permission_picker_rule_items(
                &mut rules,
                PermissionRuleSettingKind::Allow,
                &overview.settings_allowed_rules,
                "settings",
                Some(PermissionRuleScope::Settings),
            );
            push_permission_picker_rule_items(
                &mut rules,
                PermissionRuleSettingKind::Allow,
                &overview.edited_allowed_rules,
                "settings edit",
                Some(PermissionRuleScope::Settings),
            );
            push_permission_picker_rule_items(
                &mut rules,
                PermissionRuleSettingKind::Allow,
                &overview.startup_allowed_rules,
                "env/CLI",
                None,
            );
            push_permission_picker_rule_items(
                &mut rules,
                PermissionRuleSettingKind::Allow,
                &overview.runtime_allowed_rules,
                "session",
                Some(PermissionRuleScope::Session),
            );
            push_sorted_permission_picker_rules(&mut items, rules, query);
        }
        PermissionPickerTab::Deny => {
            if query.is_empty() {
                items.push(PermissionPickerItem::AddRule(
                    PermissionRuleSettingKind::Deny,
                ));
            }
            let mut rules = Vec::new();
            push_permission_picker_rule_items(
                &mut rules,
                PermissionRuleSettingKind::Deny,
                &overview.settings_denied_rules,
                "settings",
                Some(PermissionRuleScope::Settings),
            );
            push_permission_picker_rule_items(
                &mut rules,
                PermissionRuleSettingKind::Deny,
                &overview.edited_denied_rules,
                "settings edit",
                Some(PermissionRuleScope::Settings),
            );
            push_permission_picker_rule_items(
                &mut rules,
                PermissionRuleSettingKind::Deny,
                &overview.startup_denied_rules,
                "env/CLI",
                None,
            );
            push_permission_picker_rule_items(
                &mut rules,
                PermissionRuleSettingKind::Deny,
                &overview.runtime_denied_rules,
                "session",
                Some(PermissionRuleScope::Session),
            );
            push_sorted_permission_picker_rules(&mut items, rules, query);
        }
        PermissionPickerTab::Workspace => {
            push_permission_picker_directory_items(
                &mut items,
                &overview.configured_additional_directories,
                "settings",
            );
            push_permission_picker_directory_items(
                &mut items,
                &overview.session_additional_directories,
                "session",
            );
            items.push(PermissionPickerItem::Directory {
                path: "Add directory…".to_string(),
                source: "use /add-dir".to_string(),
            });
        }
        PermissionPickerTab::Ask => {}
    }
    items
}

fn push_sorted_permission_picker_rules(
    items: &mut Vec<PermissionPickerItem>,
    mut rules: Vec<PermissionPickerItem>,
    query: &str,
) {
    let lower_query = query.to_lowercase();
    rules.sort_by(|left, right| {
        let left = match left {
            PermissionPickerItem::Rule(rule) => rule.rule.to_lowercase(),
            _ => String::new(),
        };
        let right = match right {
            PermissionPickerItem::Rule(rule) => rule.rule.to_lowercase(),
            _ => String::new(),
        };
        left.cmp(&right)
    });
    for item in rules {
        if let PermissionPickerItem::Rule(rule) = &item
            && (lower_query.is_empty() || rule.rule.to_lowercase().contains(&lower_query))
        {
            items.push(item);
        }
    }
}

fn push_permission_picker_rule_items(
    items: &mut Vec<PermissionPickerItem>,
    kind: PermissionRuleSettingKind,
    rules: &[String],
    source: &str,
    scope: Option<PermissionRuleScope>,
) {
    for rule in rules {
        items.push(PermissionPickerItem::Rule(PermissionPickerRuleItem {
            kind,
            rule: rule.clone(),
            source: source.to_string(),
            scope,
        }));
    }
}

fn push_permission_picker_directory_items(
    items: &mut Vec<PermissionPickerItem>,
    directories: &[PathBuf],
    source: &str,
) {
    for path in directories {
        let rendered = path.display().to_string();
        if !items.iter().any(|item| {
            matches!(
                item,
                PermissionPickerItem::Directory { path, .. } if path == &rendered
            )
        }) {
            items.push(PermissionPickerItem::Directory {
                path: rendered,
                source: source.to_string(),
            });
        }
    }
}

pub(crate) fn permission_picker_item_is_selectable(item: &PermissionPickerItem) -> bool {
    match item {
        PermissionPickerItem::Rule(_)
        | PermissionPickerItem::RecentlyDenied(_)
        | PermissionPickerItem::AddRule(_) => true,
        PermissionPickerItem::Directory { .. } => true,
    }
}

pub(crate) fn permission_picker_lines(
    picker: &PermissionPickerState,
    width: usize,
) -> Vec<StyledLine> {
    let mut lines = permission_picker_dialog_title_lines(width);
    if let Some(details) = &picker.rule_details {
        lines.extend(permission_picker_rule_details_lines(details, width));
        return lines;
    }
    if let Some(destination) = &picker.add_destination {
        lines.extend(permission_picker_add_destination_lines(destination, width));
        return lines;
    }
    if let Some(draft) = &picker.adding {
        lines.extend(permission_picker_add_input_lines(draft, width));
        return lines;
    }

    lines.push(permission_picker_tabs_line(picker.tab, width));
    lines.push(Line::default());

    lines.push(Line::from(Span::styled(
        truncate_display_width(permission_picker_tab_description(picker.tab), width.max(1)),
        inactive_style(),
    )));
    lines.push(Line::default());

    lines.extend(permission_picker_body_lines(picker, width));
    lines.push(Line::default());
    lines.push(permission_picker_footer_line(picker));
    debug_assert_eq!(lines.len(), PERMISSION_PICKER_PANEL_HEIGHT);
    lines
}

pub(crate) fn permission_picker_dialog_inner_width(width: usize) -> usize {
    width.saturating_sub(2).max(1)
}

pub(crate) fn permission_picker_outer_height(inner_line_count: usize) -> u16 {
    saturating_u16(inner_line_count.saturating_add(2))
}

pub(crate) fn permission_picker_dialog_inner_area(area: Rect) -> Rect {
    Rect {
        x: area.x.saturating_add(1),
        y: area.y.saturating_add(1),
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    }
}

pub(crate) fn permission_picker_dialog_block() -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(subtle_style())
}

fn permission_picker_dialog_title_lines(width: usize) -> Vec<StyledLine> {
    vec![
        Line::from(Span::styled(
            "Permissions",
            inactive_style().add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled("─".repeat(width.max(1)), subtle_style())),
    ]
}

fn permission_picker_tabs_line(selected: PermissionPickerTab, width: usize) -> StyledLine {
    let muted = empty_transcript_placeholder_style();
    let separator = if width < 60 { " " } else { "     " };
    let mut spans = Vec::new();
    for (index, tab) in PERMISSION_PICKER_TABS.iter().copied().enumerate() {
        if index > 0 {
            spans.push(Span::raw(separator));
        }
        let title = format!(" {} ", permission_picker_tab_title(tab));
        let style = if tab == selected {
            inactive_style().add_modifier(Modifier::REVERSED | Modifier::BOLD)
        } else {
            muted
        };
        spans.push(Span::styled(title, style));
    }
    Line::from(spans)
}

fn permission_picker_body_lines(picker: &PermissionPickerState, width: usize) -> Vec<StyledLine> {
    let muted = empty_transcript_placeholder_style();
    let mut lines = Vec::new();
    if permission_picker_tab_has_search(picker.tab) {
        lines.extend(permission_picker_search_box_lines(picker, width));
        lines.push(Line::default());
    }
    if picker.tab == PermissionPickerTab::Workspace {
        lines.push(Line::from(vec![
            Span::styled("    ", muted),
            Span::styled(
                pad_or_truncate(
                    &picker.overview.permissions.cwd.display().to_string(),
                    width.saturating_sub(38).max(1),
                ),
                inactive_style(),
            ),
            Span::styled(" (Original working directory)", muted),
        ]));
    }

    let item_budget = PERMISSION_PICKER_BODY_ROWS.saturating_sub(lines.len());
    append_permission_picker_items(&mut lines, picker, width, item_budget);
    while lines.len() < PERMISSION_PICKER_BODY_ROWS {
        lines.push(Line::default());
    }
    lines.truncate(PERMISSION_PICKER_BODY_ROWS);
    lines
}

fn append_permission_picker_items(
    lines: &mut Vec<StyledLine>,
    picker: &PermissionPickerState,
    width: usize,
    item_budget: usize,
) {
    if item_budget == 0 {
        return;
    }
    let muted = empty_transcript_placeholder_style();
    if picker.items.is_empty() {
        lines.push(Line::from(Span::styled(
            permission_picker_empty_message(picker.tab),
            muted,
        )));
        return;
    }

    let reserve_more_line = picker.items.len() > item_budget && item_budget > 1;
    let visible_count = if reserve_more_line {
        item_budget - 1
    } else {
        item_budget
    }
    .min(picker.items.len());
    let start = slash_command_view_start(picker.selected, picker.items.len(), visible_count);
    for (offset, item) in picker
        .items
        .iter()
        .skip(start)
        .take(visible_count)
        .enumerate()
    {
        let index = start + offset;
        lines.push(permission_picker_item_line(
            item,
            index,
            picker.focus == PermissionPickerFocus::Content && index == picker.selected,
            width,
        ));
    }
    if start + visible_count < picker.items.len() && lines.len() < PERMISSION_PICKER_BODY_ROWS {
        lines.push(Line::from(vec![
            Span::styled("  ↓ ", muted),
            Span::styled(
                format!("{} more below", picker.items.len() - start - visible_count),
                muted,
            ),
        ]));
    }
}

fn permission_picker_tab_title(tab: PermissionPickerTab) -> &'static str {
    match tab {
        PermissionPickerTab::RecentlyDenied => "Recently denied",
        PermissionPickerTab::Allow => "Allow",
        PermissionPickerTab::Ask => "Ask",
        PermissionPickerTab::Deny => "Deny",
        PermissionPickerTab::Workspace => "Workspace",
    }
}

fn permission_picker_tab_description(tab: PermissionPickerTab) -> &'static str {
    match tab {
        PermissionPickerTab::RecentlyDenied => {
            "Tool requests denied this session will appear here when tracked."
        }
        PermissionPickerTab::Allow => "Orb Code won't ask before using allowed tools.",
        PermissionPickerTab::Ask => {
            "Orb Code will always ask for confirmation before using these tools."
        }
        PermissionPickerTab::Deny => "Orb Code will always reject requests to use denied tools.",
        PermissionPickerTab::Workspace => {
            "Orb Code can read files in the workspace, and make edits when auto-accept edits is on."
        }
    }
}

fn permission_picker_empty_message(tab: PermissionPickerTab) -> &'static str {
    match tab {
        PermissionPickerTab::RecentlyDenied => "  No recently denied requests yet.",
        PermissionPickerTab::Allow => "  No allow rules configured.",
        PermissionPickerTab::Ask => "  No ask rules configured.",
        PermissionPickerTab::Deny => "  No deny rules configured.",
        PermissionPickerTab::Workspace => "  No workspace directories found.",
    }
}

fn permission_picker_footer_line(picker: &PermissionPickerState) -> StyledLine {
    let muted = empty_transcript_placeholder_style();
    if picker.focus == PermissionPickerFocus::Header {
        return Line::from(vec![
            Span::styled("←/→", inactive_style().add_modifier(Modifier::BOLD)),
            Span::styled(" to switch · ", muted),
            Span::styled("↓", inactive_style().add_modifier(Modifier::BOLD)),
            Span::styled(" to select · ", muted),
            Span::styled("Esc", inactive_style().add_modifier(Modifier::BOLD)),
            Span::styled(" to cancel", muted),
        ]);
    }
    if picker.search_active {
        return Line::from(vec![
            Span::styled("Type", inactive_style().add_modifier(Modifier::BOLD)),
            Span::styled(" to filter · ", muted),
            Span::styled("Enter/↓", inactive_style().add_modifier(Modifier::BOLD)),
            Span::styled(" select · ", muted),
            Span::styled("↑", inactive_style().add_modifier(Modifier::BOLD)),
            Span::styled(" tabs · ", muted),
            Span::styled("Esc", inactive_style().add_modifier(Modifier::BOLD)),
            Span::styled(" clear", muted),
        ]);
    }
    Line::from(vec![
        Span::styled("↑↓", inactive_style().add_modifier(Modifier::BOLD)),
        Span::styled(" navigate · ", muted),
        Span::styled("Enter", inactive_style().add_modifier(Modifier::BOLD)),
        Span::styled(" select · ", muted),
        Span::styled("Type", inactive_style().add_modifier(Modifier::BOLD)),
        Span::styled(" to search · ", muted),
        Span::styled("←/→", inactive_style().add_modifier(Modifier::BOLD)),
        Span::styled(" switch · ", muted),
        Span::styled("Esc", inactive_style().add_modifier(Modifier::BOLD)),
        Span::styled(" cancel", muted),
    ])
}

fn permission_picker_search_box_lines(
    picker: &PermissionPickerState,
    width: usize,
) -> Vec<StyledLine> {
    let muted = empty_transcript_placeholder_style();
    let box_width = permission_picker_box_width(width);
    let value_width = box_width.saturating_sub(4).max(1);
    let value = if picker.search_query.is_empty() {
        "⌕ Search…"
    } else {
        &picker.search_query
    };
    let style = if picker.search_query.is_empty() {
        muted
    } else {
        inactive_style()
    };
    vec![
        Line::from(Span::styled(
            format!("  ╭{}╮", "─".repeat(box_width.saturating_sub(2))),
            muted,
        )),
        Line::from(vec![
            Span::styled("  │ ", muted),
            Span::styled(pad_or_truncate(value, value_width), style),
            Span::styled(" │", muted),
        ]),
        Line::from(Span::styled(
            format!("  ╰{}╯", "─".repeat(box_width.saturating_sub(2))),
            muted,
        )),
    ]
}

fn permission_picker_add_input_lines(draft: &PermissionAddDraft, width: usize) -> Vec<StyledLine> {
    let muted = empty_transcript_placeholder_style();
    let box_width = permission_picker_box_width(width);
    let value_width = box_width.saturating_sub(4).max(1);
    let value = if draft.rule.is_empty() {
        "Enter permission rule…"
    } else {
        draft.rule.as_str()
    };
    let value_style = if draft.rule.is_empty() {
        muted
    } else {
        inactive_style()
    };
    vec![
        Line::from(Span::styled(
            "Add permission rule",
            inactive_style().add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            "  Permission rules are a tool name, optionally followed by a specifier.",
            inactive_style(),
        )),
        Line::from(vec![
            Span::styled("  e.g., ", inactive_style()),
            Span::styled("WebFetch", inactive_style().add_modifier(Modifier::BOLD)),
            Span::styled(" or ", inactive_style()),
            Span::styled("Bash(ls:*)", inactive_style().add_modifier(Modifier::BOLD)),
        ]),
        Line::default(),
        Line::from(Span::styled(
            format!("  ╭{}╮", "─".repeat(box_width.saturating_sub(2))),
            muted,
        )),
        Line::from(vec![
            Span::styled("  │ ", muted),
            Span::styled(pad_or_truncate(value, value_width), value_style),
            Span::styled(" │", muted),
        ]),
        Line::from(Span::styled(
            format!("  ╰{}╯", "─".repeat(box_width.saturating_sub(2))),
            muted,
        )),
        Line::default(),
        Line::from(Span::styled("  Enter to submit · Esc to cancel", muted)),
    ]
}

fn permission_picker_add_destination_lines(
    destination: &PermissionAddDestinationState,
    width: usize,
) -> Vec<StyledLine> {
    let muted = empty_transcript_placeholder_style();
    let title = format!("Add {} permission rule", destination.draft.kind.as_str());
    let mut lines = vec![
        Line::from(Span::styled(
            title,
            inactive_style().add_modifier(Modifier::BOLD),
        )),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                pad_or_truncate(&destination.draft.rule, width.saturating_sub(4).max(1)),
                inactive_style().add_modifier(Modifier::BOLD),
            ),
        ]),
    ];
    if let Some(description) = permission_picker_rule_description(&destination.draft.rule) {
        lines.push(Line::from(Span::styled(format!("  {description}"), muted)));
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        "  Where should this rule be saved?",
        inactive_style(),
    )));
    for (index, (label, description, _scope)) in
        permission_picker_destination_options().iter().enumerate()
    {
        let selected = index == destination.selected;
        let marker = if selected { "❯ " } else { "  " };
        let style = if selected {
            permission_picker_highlight_style()
        } else {
            inactive_style()
        };
        lines.push(Line::from(vec![
            Span::styled("  ", muted),
            Span::styled(marker, style),
            Span::styled(format!("{}.", index + 1), style),
            Span::styled(" ", muted),
            Span::styled(*label, style),
            Span::styled("  ", muted),
            Span::styled(*description, muted),
        ]));
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        "  ↑↓ choose · Enter save · Esc cancel",
        muted,
    )));
    lines
}

fn permission_picker_rule_details_lines(
    details: &PermissionRuleDetailsState,
    width: usize,
) -> Vec<StyledLine> {
    let muted = empty_transcript_placeholder_style();
    let rule = &details.rule;
    let editable = rule.scope.is_some();
    let title = if editable {
        format!("Delete {} tool?", permission_rule_behavior_label(rule.kind))
    } else {
        "Rule details".to_string()
    };
    let mut lines = vec![
        Line::from(Span::styled(
            title,
            inactive_style().add_modifier(Modifier::BOLD),
        )),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                pad_or_truncate(&rule.rule, width.saturating_sub(4).max(1)),
                inactive_style().add_modifier(Modifier::BOLD),
            ),
        ]),
    ];
    if let Some(description) = permission_picker_rule_description(&rule.rule) {
        lines.push(Line::from(Span::styled(format!("  {description}"), muted)));
    }
    lines.push(Line::from(Span::styled(
        format!("  From {}", permission_rule_source_display(&rule.source)),
        muted,
    )));
    lines.push(Line::default());
    if editable {
        lines.push(Line::from(Span::styled(
            "  Are you sure you want to delete this permission rule?",
            inactive_style(),
        )));
        for (index, label) in ["Yes", "No"].iter().enumerate() {
            let selected = index == details.selected;
            let marker = if selected { "❯ " } else { "  " };
            let style = if selected {
                permission_picker_highlight_style()
            } else {
                inactive_style()
            };
            lines.push(Line::from(vec![
                Span::styled("  ", muted),
                Span::styled(marker, style),
                Span::styled(*label, style),
            ]));
        }
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            "  ↑↓ choose · Enter confirm · Esc cancel",
            muted,
        )));
    } else {
        lines.push(Line::from(Span::styled(
            "  This rule is read-only in the Rust permissions UI.",
            muted,
        )));
        lines.push(Line::from(Span::styled("  Esc to cancel", muted)));
    }
    lines
}

pub(crate) fn permission_picker_destination_options()
-> [(&'static str, &'static str, PermissionRuleScope); 2] {
    [
        (
            "User settings",
            "Saved in ~/.claude/settings.json",
            PermissionRuleScope::Settings,
        ),
        (
            "Session only",
            "Current session only",
            PermissionRuleScope::Session,
        ),
    ]
}

fn permission_picker_box_width(width: usize) -> usize {
    width.saturating_sub(4).clamp(1, 72)
}

pub(crate) fn recently_denied_permission_from_request(
    request: &PermissionRequest,
) -> RecentlyDeniedPermission {
    RecentlyDeniedPermission {
        tool_name: human_tool_name(&request.tool_name),
        detail: permission_request_detail_label(request),
        suggested_rule: suggested_permission_rule(request),
    }
}

fn permission_request_detail_label(request: &PermissionRequest) -> String {
    let parsed = serde_json::from_str::<Value>(&request.tool_input).ok();
    match canonical_permission_tool_name(&request.tool_name).as_str() {
        "bash" => parsed
            .as_ref()
            .and_then(|value| string_value_any(value, &["command", "cmd", "script"]))
            .unwrap_or_else(|| request.tool_input.clone()),
        "file-read" | "file-write" | "file-edit" | "notebook-edit" => parsed
            .as_ref()
            .and_then(|value| string_value_any(value, &["file_path", "filePath", "path"]))
            .unwrap_or_else(|| request.summary()),
        "grep" | "glob" => parsed
            .as_ref()
            .and_then(|value| string_value_any(value, &["path", "base"]))
            .unwrap_or_else(|| request.summary()),
        _ => request.summary(),
    }
}

fn permission_picker_rule_description(rule: &str) -> Option<String> {
    let trimmed = rule.trim();
    if let Some(content) = trimmed
        .strip_prefix("Bash(")
        .and_then(|rest| rest.strip_suffix(')'))
    {
        if let Some(prefix) = content.strip_suffix(":*") {
            return Some(format!("Any Bash command starting with {prefix}"));
        }
        return Some(format!("The Bash command {content}"));
    }
    let tool_name = trimmed.split('(').next().unwrap_or(trimmed).trim();
    if !tool_name.is_empty() && !trimmed.contains('(') {
        return Some(format!("Any use of the {tool_name} tool"));
    }
    None
}

fn permission_rule_behavior_label(kind: PermissionRuleSettingKind) -> &'static str {
    match kind {
        PermissionRuleSettingKind::Allow => "allowed",
        PermissionRuleSettingKind::Deny => "denied",
    }
}

fn permission_rule_source_display(source: &str) -> &str {
    match source {
        "settings" | "settings edit" => "User settings",
        "session" => "Session",
        "env/CLI" => "env/CLI",
        other => other,
    }
}

pub(crate) fn permission_picker_tab_has_search(tab: PermissionPickerTab) -> bool {
    matches!(
        tab,
        PermissionPickerTab::Allow | PermissionPickerTab::Ask | PermissionPickerTab::Deny
    )
}

pub(crate) fn permission_picker_search_start_char(character: char) -> bool {
    character == '/'
        || (display_width(character) > 0 && !matches!(character, 'j' | 'k' | 'm' | 'i' | 'r' | ' '))
}

pub(crate) fn permission_picker_next_tab(tab: PermissionPickerTab) -> PermissionPickerTab {
    let index = PERMISSION_PICKER_TABS
        .iter()
        .position(|candidate| *candidate == tab)
        .unwrap_or(0);
    PERMISSION_PICKER_TABS[(index + 1) % PERMISSION_PICKER_TABS.len()]
}

pub(crate) fn permission_picker_previous_tab(tab: PermissionPickerTab) -> PermissionPickerTab {
    let index = PERMISSION_PICKER_TABS
        .iter()
        .position(|candidate| *candidate == tab)
        .unwrap_or(0);
    PERMISSION_PICKER_TABS
        [(index + PERMISSION_PICKER_TABS.len() - 1) % PERMISSION_PICKER_TABS.len()]
}

pub(crate) fn permission_picker_tab_for_kind(
    kind: PermissionRuleSettingKind,
) -> PermissionPickerTab {
    match kind {
        PermissionRuleSettingKind::Allow => PermissionPickerTab::Allow,
        PermissionRuleSettingKind::Deny => PermissionPickerTab::Deny,
    }
}

fn permission_picker_item_line(
    item: &PermissionPickerItem,
    index: usize,
    selected: bool,
    width: usize,
) -> StyledLine {
    let muted = empty_transcript_placeholder_style();
    let marker = if selected { "❯ " } else { "  " };
    let style = if selected {
        permission_picker_highlight_style()
    } else {
        inactive_style()
    };
    match item {
        PermissionPickerItem::Rule(rule) => {
            let prefix = format!("{marker}{}.", index + 1);
            let available_rule_width = width
                .saturating_sub(display_width_str(&prefix))
                .saturating_sub(6)
                .max(1);
            Line::from(vec![
                Span::styled("  ", muted),
                Span::styled(prefix, style),
                Span::styled(" ", muted),
                Span::styled(pad_or_truncate(&rule.rule, available_rule_width), style),
            ])
        }
        PermissionPickerItem::RecentlyDenied(denied) => {
            let prefix = format!("{marker}{}.", index + 1);
            let label = denied
                .suggested_rule
                .as_deref()
                .unwrap_or(denied.detail.as_str());
            let available_label_width = width
                .saturating_sub(display_width_str(&prefix))
                .saturating_sub(display_width_str(&denied.tool_name))
                .saturating_sub(10)
                .max(1);
            Line::from(vec![
                Span::styled("  ", muted),
                Span::styled(prefix, style),
                Span::styled(" ", muted),
                Span::styled(pad_or_truncate(label, available_label_width), style),
                Span::styled("  ", muted),
                Span::styled(denied.tool_name.clone(), muted),
            ])
        }
        PermissionPickerItem::AddRule(_) => {
            let label = "Add a new rule…";
            Line::from(vec![
                Span::styled("  ", muted),
                Span::styled(marker.to_string(), style),
                Span::styled(format!("{}.", index + 1), style),
                Span::styled(" ", muted),
                Span::styled(truncate_chars(label, width.saturating_sub(8).max(1)), style),
            ])
        }
        PermissionPickerItem::Directory { path, source } => {
            let prefix = format!("{marker}{}.", index + 1);
            let source_label = if path == "Add directory…" {
                ""
            } else {
                source.as_str()
            };
            let source_width = display_width_str(source_label);
            let available_path_width = width
                .saturating_sub(display_width_str(&prefix))
                .saturating_sub(source_width)
                .saturating_sub(8)
                .max(1);
            let mut spans = vec![
                Span::styled("  ", muted),
                Span::styled(prefix, style),
                Span::styled(" ", muted),
                Span::styled(pad_or_truncate(path, available_path_width), style),
            ];
            if !source_label.is_empty() {
                spans.push(Span::styled("  ", muted));
                spans.push(Span::styled(source_label.to_string(), muted));
            }
            Line::from(spans)
        }
    }
}

fn permission_picker_highlight_style() -> Style {
    highlight_style()
}

pub(crate) fn permission_picker_cursor(
    picker: &PermissionPickerState,
    area: Rect,
) -> Option<(u16, u16)> {
    if area.height == 0 || area.width == 0 {
        return None;
    }
    if picker.search_active && permission_picker_tab_has_search(picker.tab) {
        let value_width = permission_picker_box_width(area.width as usize)
            .saturating_sub(6)
            .max(1);
        let cursor_col = display_width_str(&truncate_chars(&picker.search_query, value_width));
        return Some((
            area.x
                .saturating_add(4)
                .saturating_add(saturating_u16(cursor_col))
                .min(area.x.saturating_add(area.width.saturating_sub(1))),
            area.y
                .saturating_add(7)
                .min(area.y.saturating_add(area.height.saturating_sub(1))),
        ));
    }
    let draft = picker.adding.as_ref()?;
    let input_row = permission_picker_add_input_line_index(picker);
    let value_width = permission_picker_box_width(area.width as usize)
        .saturating_sub(4)
        .max(1);
    let cursor_col = display_width_str(&truncate_chars(&draft.rule, value_width));
    Some((
        area.x
            .saturating_add(4)
            .saturating_add(saturating_u16(cursor_col))
            .min(area.x.saturating_add(area.width.saturating_sub(1))),
        area.y
            .saturating_add(input_row)
            .min(area.y.saturating_add(area.height.saturating_sub(1))),
    ))
}

pub(crate) fn permission_picker_add_input_line_index(picker: &PermissionPickerState) -> u16 {
    if picker.adding.is_some() {
        return 7;
    }
    0
}
