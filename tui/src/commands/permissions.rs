use anyhow::Result;
use orbcode_app_server_client::{AppClient, PermissionOverview};
use orbcode_config::{PermissionRuleSettingKind, normalize_permission_rule_for_edit};

use crate::commands::utils::{slash_command_display_path, split_first_word};
use crate::render::slash_output::render_permission_overview;

pub(crate) async fn run_permissions_slash_command(
    app_server: &AppClient,
    session_id: &str,
    args: &str,
) -> Result<(String, Option<String>)> {
    let (action_word, rest) = split_first_word(args).ok_or_else(permissions_usage_error)?;
    let action = parse_permission_rule_action(action_word)?;
    let (scope, kind, rule) = parse_permission_rule_update_args(rest)?;
    let normalized_rule = normalize_permission_rule_for_edit(rule)
        .map_err(|message| permissions_invalid_rule_error(rule, &message))?;
    let (summary, detail, _) = apply_permission_rule_update(
        app_server,
        session_id,
        action,
        scope,
        kind,
        &normalized_rule,
    )
    .await?;
    Ok((summary, Some(detail)))
}

pub(crate) async fn apply_permission_rule_update(
    app_server: &AppClient,
    session_id: &str,
    action: PermissionRuleAction,
    scope: PermissionRuleScope,
    kind: PermissionRuleSettingKind,
    normalized_rule: &str,
) -> Result<(String, String, PermissionOverview)> {
    let overview_before: PermissionOverview = serde_json::from_value(
        app_server
            .permission_overview()
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?,
    )?;
    let sources_before = permission_rule_sources(&overview_before, kind, normalized_rule);
    let kind_str = kind.as_str();
    let update_value = match action {
        PermissionRuleAction::Add => match scope {
            PermissionRuleScope::Settings => app_server
                .add_permission_rule(kind_str, normalized_rule)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?,
            PermissionRuleScope::Session => app_server
                .add_session_permission_rule(session_id, kind_str, normalized_rule)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?,
        },
        PermissionRuleAction::Remove => match scope {
            PermissionRuleScope::Settings => app_server
                .remove_permission_rule(kind_str, normalized_rule)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?,
            PermissionRuleScope::Session => app_server
                .remove_session_permission_rule(session_id, kind_str, normalized_rule)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?,
        },
    };
    let update_path = update_value["path"]
        .as_str()
        .map(std::path::PathBuf::from)
        .unwrap_or_default();
    let update_rule = update_value["rule"].as_str().unwrap_or("").to_string();
    let update_changed = update_value["changed"].as_bool().unwrap_or(false);
    let overview: PermissionOverview = serde_json::from_value(
        app_server
            .permission_overview()
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?,
    )?;
    let sources_after = permission_rule_sources(&overview, kind, &update_rule);
    let display_path = slash_command_display_path(&update_path, &overview.permissions.cwd);
    let scope_prefix = match scope {
        PermissionRuleScope::Settings => "",
        PermissionRuleScope::Session => "session ",
    };
    let title_scope_prefix = match scope {
        PermissionRuleScope::Settings => "",
        PermissionRuleScope::Session => "Session ",
    };
    let kind_title = permission_rule_kind_title(kind);
    let summary = match (action, update_changed) {
        (PermissionRuleAction::Add, true) => {
            format!("Added {scope_prefix}{} permission rule.", kind.as_str())
        }
        (PermissionRuleAction::Add, false) => {
            format!("{title_scope_prefix}{kind_title} permission rule already exists.")
        }
        (PermissionRuleAction::Remove, true) => {
            format!("Removed {scope_prefix}{} permission rule.", kind.as_str())
        }
        (PermissionRuleAction::Remove, false) => {
            format!(
                "No {} {} permission rule matched.",
                permission_scope_label(scope),
                kind.as_str()
            )
        }
    };
    let location = match scope {
        PermissionRuleScope::Settings => format!("Settings: {display_path}"),
        PermissionRuleScope::Session => "Scope: current session only".to_string(),
    };
    let mut detail_lines = vec![format!("Rule: {}", update_rule), location];
    if let Some(note) = permission_rule_change_note(
        action,
        scope,
        kind,
        update_changed,
        &sources_before,
        &sources_after,
        &display_path,
    ) {
        detail_lines.push(note);
    }
    detail_lines.push(String::new());
    detail_lines.push(render_permission_overview(&overview));
    let detail = detail_lines.join("\n");
    Ok((summary, detail, overview))
}

pub(crate) const PERMISSIONS_USAGE: &str =
    "usage: /permissions add|remove [settings|session] allow|deny <rule>";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PermissionRuleAction {
    Add,
    Remove,
}

impl PermissionRuleAction {
    fn as_str(self) -> &'static str {
        match self {
            Self::Add => "add",
            Self::Remove => "remove",
        }
    }
}

pub(crate) fn permission_rule_update_command(
    command: &str,
    action: PermissionRuleAction,
    scope: PermissionRuleScope,
    kind: PermissionRuleSettingKind,
    rule: &str,
) -> String {
    let command = command
        .split_whitespace()
        .next()
        .filter(|command| !command.is_empty())
        .unwrap_or("/permissions");
    format!(
        "{command} {} {} {} {rule}",
        action.as_str(),
        permission_scope_label(scope),
        kind.as_str()
    )
}

fn parse_permission_rule_action(action: &str) -> Result<PermissionRuleAction> {
    match action {
        "add" => Ok(PermissionRuleAction::Add),
        "remove" | "rm" => Ok(PermissionRuleAction::Remove),
        _ => Err(permissions_parse_error(format!(
            "unknown permissions action `{action}`. expected `add` or `remove`."
        ))),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PermissionRuleScope {
    Settings,
    Session,
}

fn parse_permission_rule_update_args(
    args: &str,
) -> Result<(PermissionRuleScope, PermissionRuleSettingKind, &str)> {
    let (first, rest) = split_first_word(args).ok_or_else(|| {
        permissions_parse_error(
            "missing permission rule kind. expected `allow` or `deny`.".to_string(),
        )
    })?;
    let (scope, kind_word, rule) = match first {
        "settings" | "--settings" => {
            let (kind, rule) = split_first_word(rest).ok_or_else(|| {
                permissions_parse_error(
                    "missing permission rule kind after `settings`. expected `allow` or `deny`."
                        .to_string(),
                )
            })?;
            (PermissionRuleScope::Settings, kind, rule)
        }
        "session" | "--session" => {
            let (kind, rule) = split_first_word(rest).ok_or_else(|| {
                permissions_parse_error(
                    "missing permission rule kind after `session`. expected `allow` or `deny`."
                        .to_string(),
                )
            })?;
            (PermissionRuleScope::Session, kind, rule)
        }
        "env" | "cli" | "env/CLI" | "startup" | "configured" | "runtime" => {
            return Err(permissions_parse_error(format!(
                "permission source `{first}` is not editable here. use `settings` or `session`."
            )));
        }
        kind => (PermissionRuleScope::Settings, kind, rest),
    };
    let kind = match kind_word {
        "allow" => PermissionRuleSettingKind::Allow,
        "deny" => PermissionRuleSettingKind::Deny,
        _ => {
            return Err(permissions_parse_error(format!(
                "unknown permission rule kind `{kind_word}`. expected `allow` or `deny`."
            )));
        }
    };
    let rule = rule.trim();
    if rule.is_empty() {
        return Err(permissions_parse_error(format!(
            "missing permission rule after `{kind_word}`."
        )));
    }
    Ok((scope, kind, rule))
}

fn permissions_usage_error() -> anyhow::Error {
    anyhow::anyhow!(permissions_usage_with_examples())
}

fn permissions_parse_error(message: String) -> anyhow::Error {
    anyhow::anyhow!("{message}\n{}", permissions_usage_with_examples())
}

fn permissions_invalid_rule_error(rule: &str, message: &str) -> anyhow::Error {
    anyhow::anyhow!(
        "invalid permission rule `{}`: {message}\n{}",
        rule.trim(),
        permissions_usage_with_examples()
    )
}

fn permissions_usage_with_examples() -> String {
    [
        PERMISSIONS_USAGE,
        "examples:",
        "  /permissions add allow Bash(cargo test:*)",
        "  /permissions add settings deny Bash(rm:*)",
        "  /permissions add session allow Read(src/**)",
        "  /permissions remove session allow Read(src/**)",
    ]
    .join("\n")
}

pub(crate) fn permission_scope_label(scope: PermissionRuleScope) -> &'static str {
    match scope {
        PermissionRuleScope::Settings => "settings",
        PermissionRuleScope::Session => "session",
    }
}

fn permission_rule_sources(
    overview: &PermissionOverview,
    kind: PermissionRuleSettingKind,
    rule: &str,
) -> Vec<&'static str> {
    let mut sources = Vec::new();
    let (settings_rules, startup_rules, edited_rules, runtime_rules) = match kind {
        PermissionRuleSettingKind::Allow => (
            &overview.settings_allowed_rules,
            &overview.startup_allowed_rules,
            &overview.edited_allowed_rules,
            &overview.runtime_allowed_rules,
        ),
        PermissionRuleSettingKind::Deny => (
            &overview.settings_denied_rules,
            &overview.startup_denied_rules,
            &overview.edited_denied_rules,
            &overview.runtime_denied_rules,
        ),
    };
    push_permission_rule_source(&mut sources, settings_rules, rule, "settings");
    push_permission_rule_source(&mut sources, edited_rules, rule, "settings edit");
    push_permission_rule_source(&mut sources, startup_rules, rule, "env/CLI");
    push_permission_rule_source(&mut sources, runtime_rules, rule, "session");
    sources
}

fn push_permission_rule_source(
    sources: &mut Vec<&'static str>,
    rules: &[String],
    rule: &str,
    source: &'static str,
) {
    if rules.iter().any(|existing| existing == rule) && !sources.iter().any(|item| item == &source)
    {
        sources.push(source);
    }
}

fn permission_rule_change_note(
    action: PermissionRuleAction,
    scope: PermissionRuleScope,
    kind: PermissionRuleSettingKind,
    changed: bool,
    sources_before: &[&'static str],
    sources_after: &[&'static str],
    settings_path: &str,
) -> Option<String> {
    match (action, changed) {
        (PermissionRuleAction::Add, true) if !sources_before.is_empty() => Some(format!(
            "Already active from: {}. Added a {} copy.",
            sources_before.join(", "),
            permission_scope_label(scope)
        )),
        (PermissionRuleAction::Add, false) => Some(format!(
            "No changes made: this {} {} rule already exists.",
            permission_scope_label(scope),
            kind.as_str()
        )),
        (PermissionRuleAction::Remove, true) if !sources_after.is_empty() => Some(format!(
            "Still active from: {}. Remove those sources separately if needed.",
            sources_after.join(", ")
        )),
        (PermissionRuleAction::Remove, false) if sources_before.is_empty() => Some(format!(
            "No changes made: no active {} rule matched this rule.",
            kind.as_str()
        )),
        (PermissionRuleAction::Remove, false) => Some(permission_remove_noop_note(
            scope,
            kind,
            sources_before,
            settings_path,
        )),
        _ => None,
    }
}

fn permission_remove_noop_note(
    scope: PermissionRuleScope,
    kind: PermissionRuleSettingKind,
    sources_before: &[&'static str],
    settings_path: &str,
) -> String {
    let source_list = sources_before.join(", ");
    match scope {
        PermissionRuleScope::Settings => format!(
            "No changes made: matching active source(s): {source_list}. Settings edits only modify {settings_path}; use `/permissions remove session {} <rule>` for session rules. Env/CLI and project-local or managed settings are read-only here.",
            kind.as_str()
        ),
        PermissionRuleScope::Session => format!(
            "No changes made: matching active source(s): {source_list}. Session edits only modify current-session rules; use `/permissions remove settings {} <rule>` for home settings rules. Env/CLI and project-local or managed settings are read-only here.",
            kind.as_str()
        ),
    }
}

fn permission_rule_kind_title(kind: PermissionRuleSettingKind) -> &'static str {
    match kind {
        PermissionRuleSettingKind::Allow => "Allow",
        PermissionRuleSettingKind::Deny => "Deny",
    }
}
