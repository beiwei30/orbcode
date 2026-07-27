use std::path::PathBuf;

use orbcode_config::{AppConfig, PermissionRuleSettingKind};

use super::RuntimePermissionRuleEdits;
use super::rules::parsed_permission_rule_strings;
use crate::permissions::{PermissionContext, PermissionRule};

pub(crate) fn permission_context_from_edits(
    config: &AppConfig,
    edits: RuntimePermissionRuleEdits,
    additional_directories: Vec<PathBuf>,
    allow_all: bool,
) -> PermissionContext {
    let mut config = config.clone();
    config
        .allowed_tools
        .retain(|rule| !edits.removed_allow.iter().any(|removed| removed == rule));
    config
        .disallowed_tools
        .retain(|rule| !edits.removed_deny.iter().any(|removed| removed == rule));
    push_unique_rules(&mut config.allowed_tools, edits.added_allow);
    push_unique_rules(&mut config.disallowed_tools, edits.added_deny);
    push_unique_rules(&mut config.allowed_tools, edits.session_allow);
    push_unique_rules(&mut config.disallowed_tools, edits.session_deny);

    let mut permissions = PermissionContext::from_config(&config);
    for rule in edits.remembered_allow {
        let parsed = PermissionRule::for_tool(&rule.tool_name, &rule.rule);
        if !permissions
            .allowed_rules
            .iter()
            .any(|existing| existing == &parsed)
        {
            permissions.allowed_rules.push(parsed);
        }
    }
    permissions.additional_directories = additional_directories;
    if allow_all {
        permissions.allow_tools = true;
        permissions.allow_network = true;
        permissions.provider_allow_network = true;
    }
    permissions
}

pub(crate) fn record_runtime_permission_rule_add(
    edits: &mut RuntimePermissionRuleEdits,
    config: &AppConfig,
    kind: PermissionRuleSettingKind,
    rule: String,
) {
    match kind {
        PermissionRuleSettingKind::Allow => {
            edits.removed_allow.retain(|existing| existing != &rule);
            if !config
                .allowed_tools
                .iter()
                .any(|existing| existing == &rule)
                && !edits.added_allow.iter().any(|existing| existing == &rule)
            {
                edits.added_allow.push(rule);
            }
        }
        PermissionRuleSettingKind::Deny => {
            edits.removed_deny.retain(|existing| existing != &rule);
            if !config
                .disallowed_tools
                .iter()
                .any(|existing| existing == &rule)
                && !edits.added_deny.iter().any(|existing| existing == &rule)
            {
                edits.added_deny.push(rule);
            }
        }
    }
}

pub(crate) fn record_runtime_permission_rule_remove(
    edits: &mut RuntimePermissionRuleEdits,
    config: &AppConfig,
    kind: PermissionRuleSettingKind,
    rule: String,
) {
    match kind {
        PermissionRuleSettingKind::Allow => {
            edits.added_allow.retain(|existing| existing != &rule);
            if config
                .allowed_tools
                .iter()
                .any(|existing| existing == &rule)
                && !edits.removed_allow.iter().any(|existing| existing == &rule)
            {
                edits.removed_allow.push(rule);
            }
        }
        PermissionRuleSettingKind::Deny => {
            edits.added_deny.retain(|existing| existing != &rule);
            if config
                .disallowed_tools
                .iter()
                .any(|existing| existing == &rule)
                && !edits.removed_deny.iter().any(|existing| existing == &rule)
            {
                edits.removed_deny.push(rule);
            }
        }
    }
}

pub(crate) fn record_session_permission_rule_add(
    edits: &mut RuntimePermissionRuleEdits,
    kind: PermissionRuleSettingKind,
    rule: String,
) -> bool {
    let rules = session_rules_mut(edits, kind);
    if rules.iter().any(|existing| existing == &rule) {
        return false;
    }
    rules.push(rule);
    true
}

pub(crate) fn record_session_permission_rule_remove(
    edits: &mut RuntimePermissionRuleEdits,
    kind: PermissionRuleSettingKind,
    rule: &str,
) -> bool {
    let rules = session_rules_mut(edits, kind);
    let before = rules.len();
    rules.retain(|existing| existing != rule);
    before != rules.len()
}

pub(crate) fn settings_permission_rules(
    config: &AppConfig,
    edits: &RuntimePermissionRuleEdits,
    kind: PermissionRuleSettingKind,
) -> Vec<String> {
    let removed = removed_rules(edits, kind);
    let rules = match kind {
        PermissionRuleSettingKind::Allow => {
            parsed_permission_rule_strings(&config.settings.allowed_tools)
        }
        PermissionRuleSettingKind::Deny => {
            parsed_permission_rule_strings(&config.settings.disallowed_tools)
        }
    };
    rules
        .into_iter()
        .filter(|rule| !removed.iter().any(|existing| existing == rule))
        .collect()
}

pub(crate) fn startup_permission_rules(
    config: &AppConfig,
    edits: &RuntimePermissionRuleEdits,
    kind: PermissionRuleSettingKind,
) -> Vec<String> {
    let removed = removed_rules(edits, kind);
    let settings = match kind {
        PermissionRuleSettingKind::Allow => {
            parsed_permission_rule_strings(&config.settings.allowed_tools)
        }
        PermissionRuleSettingKind::Deny => {
            parsed_permission_rule_strings(&config.settings.disallowed_tools)
        }
    }
    .into_iter()
    .filter(|rule| !removed.iter().any(|existing| existing == rule))
    .collect::<Vec<_>>();
    let configured = match kind {
        PermissionRuleSettingKind::Allow => &config.allowed_tools,
        PermissionRuleSettingKind::Deny => &config.disallowed_tools,
    };
    parsed_permission_rule_strings(configured)
        .into_iter()
        .filter(|rule| !removed.iter().any(|existing| existing == rule))
        .filter(|rule| !settings.iter().any(|existing| existing == rule))
        .collect()
}

pub(crate) fn session_permission_rules(
    edits: RuntimePermissionRuleEdits,
    kind: PermissionRuleSettingKind,
) -> Vec<String> {
    match kind {
        PermissionRuleSettingKind::Allow => edits.session_allow,
        PermissionRuleSettingKind::Deny => edits.session_deny,
    }
}

pub(crate) fn runtime_added_permission_rules(
    edits: RuntimePermissionRuleEdits,
    kind: PermissionRuleSettingKind,
) -> Vec<String> {
    match kind {
        PermissionRuleSettingKind::Allow => edits.added_allow,
        PermissionRuleSettingKind::Deny => edits.added_deny,
    }
}

fn push_unique_rules(target: &mut Vec<String>, rules: Vec<String>) {
    for rule in rules {
        if !target.iter().any(|existing| existing == &rule) {
            target.push(rule);
        }
    }
}

fn session_rules_mut(
    edits: &mut RuntimePermissionRuleEdits,
    kind: PermissionRuleSettingKind,
) -> &mut Vec<String> {
    match kind {
        PermissionRuleSettingKind::Allow => &mut edits.session_allow,
        PermissionRuleSettingKind::Deny => &mut edits.session_deny,
    }
}

fn removed_rules(
    edits: &RuntimePermissionRuleEdits,
    kind: PermissionRuleSettingKind,
) -> &Vec<String> {
    match kind {
        PermissionRuleSettingKind::Allow => &edits.removed_allow,
        PermissionRuleSettingKind::Deny => &edits.removed_deny,
    }
}
