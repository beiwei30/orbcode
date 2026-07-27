use orbcode_config::parse_tool_rule_list;
use serde_json::json;

use super::SessionPermissionRule;
use crate::permissions::PermissionRule;

pub(crate) fn compact_session_permission_rules(
    rules: &[SessionPermissionRule],
) -> Vec<SessionPermissionRule> {
    let mut compacted = Vec::new();
    for rule in rules {
        if compacted.iter().any(|existing: &SessionPermissionRule| {
            session_permission_rule_covers(&existing.tool_name, &existing.rule, &rule.rule)
        }) {
            continue;
        }
        compacted.retain(|existing| {
            !session_permission_rule_covers(&rule.tool_name, &rule.rule, &existing.rule)
        });
        compacted.push(rule.clone());
    }
    compacted
}

pub(crate) fn parsed_permission_rule_strings(values: &[String]) -> Vec<String> {
    parse_tool_rule_list(values)
        .into_iter()
        .map(|rule| PermissionRule::parse(&rule).raw)
        .collect()
}

pub(crate) fn session_permission_rule_covers(
    tool_name: &str,
    broader_rule: &str,
    candidate_rule: &str,
) -> bool {
    if broader_rule == candidate_rule {
        return true;
    }
    if !tool_name.eq_ignore_ascii_case("bash") {
        return false;
    }
    let candidate = PermissionRule::for_tool(tool_name, candidate_rule);
    let Some(command) = candidate.rule_content.as_deref() else {
        return false;
    };
    let input = json!({ "command": command }).to_string();
    PermissionRule::for_tool(tool_name, broader_rule).matches_tool_call(tool_name, &input)
}
