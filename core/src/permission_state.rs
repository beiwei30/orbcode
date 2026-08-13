use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, Ordering},
    },
};

use orbcode_config::{AppConfig, PermissionRuleSettingKind, bash_command_allowed_by_rules};
use orbcode_protocol::PermissionRequest;
use tokio::{
    sync::{Mutex, oneshot},
    time::{Duration, sleep},
};

use crate::permissions::{PermissionContext, PermissionRule};

mod edits;
mod rules;
use rules::{compact_session_permission_rules, session_permission_rule_covers};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PermissionDecision {
    Approve,
    ApproveAlways(String),
    ApproveAlwaysMany(Vec<String>),
    Deny,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SessionPermissionRule {
    pub(crate) tool_name: String,
    pub(crate) rule: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SessionDeniedToolCall {
    pub(crate) tool_name: String,
    pub(crate) tool_input: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct RuntimePermissionRuleEdits {
    pub(crate) added_allow: Vec<String>,
    pub(crate) added_deny: Vec<String>,
    pub(crate) removed_allow: Vec<String>,
    pub(crate) removed_deny: Vec<String>,
    pub(crate) session_allow: Vec<String>,
    pub(crate) session_deny: Vec<String>,
}

#[derive(Clone, Default)]
pub(crate) struct PermissionRuntimeState {
    pending_permissions: Arc<Mutex<HashMap<String, oneshot::Sender<PermissionDecision>>>>,
    permission_rules: Arc<Mutex<HashMap<String, Vec<SessionPermissionRule>>>>,
    denied_tool_calls: Arc<Mutex<HashMap<String, Vec<SessionDeniedToolCall>>>>,
    approval_review_counts: Arc<Mutex<HashMap<String, usize>>>,
    runtime_permission_rule_edits: Arc<RwLock<RuntimePermissionRuleEdits>>,
}

impl PermissionRuntimeState {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn permission_context(
        &self,
        config: &AppConfig,
        additional_directories: Vec<PathBuf>,
    ) -> PermissionContext {
        edits::permission_context_from_edits(config, self.edits(), additional_directories)
    }

    pub(crate) async fn respond_to_permission_request(
        &self,
        request_id: &str,
        decision: PermissionDecision,
    ) -> bool {
        let mut pending = self.pending_permissions.lock().await;
        if let Some(sender) = pending.remove(request_id) {
            let _ = sender.send(decision);
            true
        } else {
            false
        }
    }

    pub(crate) async fn await_permission_decision(
        &self,
        request: &PermissionRequest,
        notify_registered: impl FnOnce() -> bool,
        cancel_flag: Arc<AtomicBool>,
        poll_interval: Duration,
    ) -> Option<PermissionDecision> {
        let (sender, mut receiver) = oneshot::channel();
        {
            let mut pending = self.pending_permissions.lock().await;
            pending.insert(request.request_id.clone(), sender);
        }
        if !notify_registered() {
            let mut pending = self.pending_permissions.lock().await;
            pending.remove(&request.request_id);
            return None;
        }

        loop {
            if cancel_flag.load(Ordering::SeqCst) {
                let mut pending = self.pending_permissions.lock().await;
                pending.remove(&request.request_id);
                return None;
            }

            tokio::select! {
                result = &mut receiver => {
                    let mut pending = self.pending_permissions.lock().await;
                    pending.remove(&request.request_id);
                    return Some(result.unwrap_or(PermissionDecision::Deny));
                }
                _ = sleep(poll_interval) => {}
            }
        }
    }

    pub(crate) async fn begin_permission_turn(&self, session_id: &str) {
        self.approval_review_counts
            .lock()
            .await
            .insert(session_id.to_string(), 0);
    }

    pub(crate) async fn try_begin_approval_review(&self, session_id: &str, limit: usize) -> bool {
        let mut counts = self.approval_review_counts.lock().await;
        let count = counts.entry(session_id.to_string()).or_insert(0);
        if *count >= limit {
            return false;
        }
        *count += 1;
        true
    }

    pub(crate) async fn end_permission_turn(&self, session_id: &str) {
        self.approval_review_counts.lock().await.remove(session_id);
    }

    pub(crate) fn record_runtime_permission_rule_add(
        &self,
        config: &AppConfig,
        kind: PermissionRuleSettingKind,
        rule: String,
    ) {
        let mut edits = self.write_edits();
        edits::record_runtime_permission_rule_add(&mut edits, config, kind, rule);
    }

    pub(crate) fn record_runtime_permission_rule_remove(
        &self,
        config: &AppConfig,
        kind: PermissionRuleSettingKind,
        rule: String,
    ) {
        let mut edits = self.write_edits();
        edits::record_runtime_permission_rule_remove(&mut edits, config, kind, rule);
    }

    pub(crate) fn record_session_permission_rule_add(
        &self,
        kind: PermissionRuleSettingKind,
        rule: String,
    ) -> bool {
        let mut edits = self.write_edits();
        edits::record_session_permission_rule_add(&mut edits, kind, rule)
    }

    pub(crate) fn record_unscoped_session_permission_rule_remove(
        &self,
        kind: PermissionRuleSettingKind,
        rule: &str,
    ) -> bool {
        let mut edits = self.write_edits();
        edits::record_session_permission_rule_remove(&mut edits, kind, rule)
    }

    pub(crate) async fn record_session_permission_rule_remove(
        &self,
        session_id: &str,
        kind: PermissionRuleSettingKind,
        rule: &str,
    ) -> bool {
        let mut changed = self.record_unscoped_session_permission_rule_remove(kind, rule);
        if kind == PermissionRuleSettingKind::Allow {
            let mut rules_by_session = self.permission_rules.lock().await;
            let mut remove_session_entry = false;
            if let Some(rules) = rules_by_session.get_mut(session_id) {
                let before = rules.len();
                rules.retain(|existing| {
                    PermissionRule::for_tool(&existing.tool_name, &existing.rule).raw != rule
                });
                changed |= rules.len() != before;
                remove_session_entry = rules.is_empty();
            }
            if remove_session_entry {
                rules_by_session.remove(session_id);
            }
        }
        changed
    }

    pub(crate) async fn runtime_permission_rules_for_session(
        &self,
        session_id: &str,
    ) -> Vec<String> {
        let rules = self.permission_rules.lock().await;
        compact_session_permission_rules(
            rules.get(session_id).map(Vec::as_slice).unwrap_or_default(),
        )
        .into_iter()
        .map(|rule| PermissionRule::for_tool(&rule.tool_name, &rule.rule).raw)
        .collect()
    }

    pub(crate) fn settings_permission_rules(
        &self,
        config: &AppConfig,
        kind: PermissionRuleSettingKind,
    ) -> Vec<String> {
        let edits = self.read_edits();
        edits::settings_permission_rules(config, &edits, kind)
    }

    pub(crate) fn startup_permission_rules(
        &self,
        config: &AppConfig,
        kind: PermissionRuleSettingKind,
    ) -> Vec<String> {
        let edits = self.read_edits();
        edits::startup_permission_rules(config, &edits, kind)
    }

    pub(crate) fn session_permission_rules(&self, kind: PermissionRuleSettingKind) -> Vec<String> {
        edits::session_permission_rules(self.edits(), kind)
    }

    pub(crate) fn set_session_permission_rules(&self, allow: Vec<String>, deny: Vec<String>) {
        let mut edits = self.write_edits();
        edits.session_allow = allow;
        edits.session_deny = deny;
    }

    pub(crate) fn runtime_added_permission_rules(
        &self,
        kind: PermissionRuleSettingKind,
    ) -> Vec<String> {
        edits::runtime_added_permission_rules(self.edits(), kind)
    }

    pub(crate) async fn remember_permission_rule_for_session(
        &self,
        session_id: &str,
        tool_name: &str,
        rule: &str,
    ) {
        if rule.trim().is_empty() {
            return;
        }

        let mut rules_by_session = self.permission_rules.lock().await;
        let rules = rules_by_session.entry(session_id.to_string()).or_default();
        let candidate = SessionPermissionRule {
            tool_name: tool_name.to_string(),
            rule: PermissionRule::for_tool(tool_name, rule).raw,
        };
        if !rules.iter().any(|existing| {
            session_permission_rule_covers(&existing.tool_name, &existing.rule, &candidate.rule)
        }) {
            rules.retain(|existing| {
                !session_permission_rule_covers(
                    &candidate.tool_name,
                    &candidate.rule,
                    &existing.rule,
                )
            });
            if !rules.iter().any(|existing| existing == &candidate) {
                rules.push(candidate);
            }
        }
        *rules = compact_session_permission_rules(rules);
    }

    pub(crate) async fn remember_denied_tool_call_for_session(
        &self,
        session_id: &str,
        tool_name: &str,
        tool_input: &str,
    ) {
        let mut denied_by_session = self.denied_tool_calls.lock().await;
        let denied = denied_by_session.entry(session_id.to_string()).or_default();
        let candidate = SessionDeniedToolCall {
            tool_name: tool_name.to_string(),
            tool_input: tool_input.to_string(),
        };
        if !denied.iter().any(|existing| existing == &candidate) {
            denied.push(candidate);
        }
    }

    pub(crate) async fn matches_denied_tool_call_for_session(
        &self,
        session_id: &str,
        tool_name: &str,
        tool_input: &str,
    ) -> bool {
        let denied_by_session = self.denied_tool_calls.lock().await;
        denied_by_session
            .get(session_id)
            .into_iter()
            .flatten()
            .any(|call| {
                call.tool_name.eq_ignore_ascii_case(tool_name) && call.tool_input == tool_input
            })
    }

    pub(crate) async fn matches_permission_rule_for_session(
        &self,
        session_id: &str,
        tool_name: &str,
        tool_input: &str,
    ) -> bool {
        if tool_name.eq_ignore_ascii_case("bash")
            && orbcode_tools::bash_input_requests_sandbox_escalation(tool_input)
        {
            return false;
        }
        let rules_by_session = self.permission_rules.lock().await;
        let parsed = rules_by_session
            .get(session_id)
            .into_iter()
            .flatten()
            .map(|rule| PermissionRule::for_tool(&rule.tool_name, &rule.rule))
            .collect::<Vec<_>>();
        if tool_name.eq_ignore_ascii_case("bash") {
            bash_command_allowed_by_rules(&parsed, tool_name, tool_input).is_some()
        } else {
            parsed
                .iter()
                .any(|rule| rule.matches_tool_call(tool_name, tool_input))
        }
    }

    pub(crate) async fn remove_session_state(&self, session_id: &str) {
        self.permission_rules.lock().await.remove(session_id);
        self.denied_tool_calls.lock().await.remove(session_id);
        self.approval_review_counts.lock().await.remove(session_id);
    }

    #[cfg(test)]
    async fn remember_permission_rule(&self, tool_name: &str, rule: &str) {
        self.remember_permission_rule_for_session("test-session", tool_name, rule)
            .await;
    }

    #[cfg(test)]
    async fn matches_permission_rule(&self, tool_name: &str, tool_input: &str) -> bool {
        self.matches_permission_rule_for_session("test-session", tool_name, tool_input)
            .await
    }

    #[cfg(test)]
    async fn runtime_permission_rules(&self) -> Vec<String> {
        self.runtime_permission_rules_for_session("test-session")
            .await
    }

    fn edits(&self) -> RuntimePermissionRuleEdits {
        match self.runtime_permission_rule_edits.read() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    fn read_edits(&self) -> std::sync::RwLockReadGuard<'_, RuntimePermissionRuleEdits> {
        match self.runtime_permission_rule_edits.read() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn write_edits(&self) -> std::sync::RwLockWriteGuard<'_, RuntimePermissionRuleEdits> {
        match self.runtime_permission_rule_edits.write() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn permission_request_is_registered_before_notification() {
        let runtime = PermissionRuntimeState::new();
        let runtime_for_notification = runtime.clone();
        let request = PermissionRequest {
            request_id: "permission-request".to_string(),
            session_id: "session".to_string(),
            tool_use_id: "tool-use".to_string(),
            tool_name: "bash".to_string(),
            tool_input: r#"{"command":"printf hi"}"#.to_string(),
            requires_tools_permission: true,
            requires_network_permission: false,
        };

        let decision = runtime
            .await_permission_decision(
                &request,
                || {
                    let pending = runtime_for_notification
                        .pending_permissions
                        .try_lock()
                        .expect("pending permissions lock");
                    assert!(pending.contains_key(&request.request_id));
                    false
                },
                Arc::new(AtomicBool::new(false)),
                Duration::from_millis(1),
            )
            .await;

        assert_eq!(decision, None);
        assert!(runtime.pending_permissions.lock().await.is_empty());
    }

    #[tokio::test]
    async fn automatic_review_budget_is_per_session_and_resets_each_turn() {
        let runtime = PermissionRuntimeState::new();
        runtime.begin_permission_turn("session-a").await;
        runtime.begin_permission_turn("session-b").await;

        assert!(runtime.try_begin_approval_review("session-a", 2).await);
        assert!(runtime.try_begin_approval_review("session-a", 2).await);
        assert!(!runtime.try_begin_approval_review("session-a", 2).await);
        assert!(runtime.try_begin_approval_review("session-b", 2).await);

        runtime.end_permission_turn("session-a").await;
        runtime.begin_permission_turn("session-a").await;
        assert!(runtime.try_begin_approval_review("session-a", 2).await);
    }

    #[tokio::test]
    async fn remembered_rules_compact_overlapping_bash_prefixes() {
        let runtime = PermissionRuntimeState::new();

        runtime
            .remember_permission_rule("bash", "cat orbcode/core/src/lib.rs")
            .await;
        runtime.remember_permission_rule("bash", "cat:*").await;
        runtime
            .remember_permission_rule("bash", "cat orbcode/tui/src/lib.rs")
            .await;

        assert_eq!(runtime.runtime_permission_rules().await, vec!["cat:*"]);
    }

    #[tokio::test]
    async fn remembered_grouped_bash_rules_match_later_compound_commands() {
        let runtime = PermissionRuntimeState::new();

        runtime.remember_permission_rule("bash", "grep:*").await;
        runtime.remember_permission_rule("bash", "head:*").await;

        assert_eq!(
            runtime.runtime_permission_rules().await,
            vec!["grep:*", "head:*"]
        );
        assert!(
            runtime
                .matches_permission_rule(
                    "bash",
                    &serde_json::json!({
                        "command": r#"grep -n "ApproveAlways" orbcode/core/src/session_manager/mod.rs | head -3"#
                    })
                    .to_string(),
                )
                .await
        );
    }

    #[tokio::test]
    async fn remembered_rules_and_denials_are_session_isolated() {
        let runtime = PermissionRuntimeState::new();
        let input = r#"{"command":"cargo test"}"#;
        runtime
            .remember_permission_rule_for_session("session-a", "bash", "cargo test:*")
            .await;
        runtime
            .remember_denied_tool_call_for_session("session-a", "bash", input)
            .await;

        assert!(
            runtime
                .matches_permission_rule_for_session("session-a", "bash", input)
                .await
        );
        assert!(
            !runtime
                .matches_permission_rule_for_session("session-b", "bash", input)
                .await
        );
        assert!(
            runtime
                .matches_denied_tool_call_for_session("session-a", "bash", input)
                .await
        );
        assert!(
            !runtime
                .matches_denied_tool_call_for_session("session-b", "bash", input)
                .await
        );
    }

    #[tokio::test]
    async fn exact_compound_bash_rule_matches_without_escalation() {
        let runtime = PermissionRuntimeState::new();
        let command = r#"echo "=== app-server ===" && find orbcode/app-server/src -name "*.rs" | xargs wc -l 2>/dev/null | tail -1"#;
        let escaped_rule = r#"echo "=== app-server ===" && find orbcode/app-server/src -name "\*.rs" | xargs wc -l 2>/dev/null | tail -1"#;

        runtime.remember_permission_rule("bash", escaped_rule).await;

        assert!(
            runtime
                .matches_permission_rule(
                    "bash",
                    &serde_json::json!({ "command": command }).to_string()
                )
                .await
        );
        assert!(
            !runtime
                .matches_permission_rule(
                    "bash",
                    r#"{"command":"echo different && find orbcode/app-server/src -name \"*.rs\""}"#,
                )
                .await
        );
        assert!(
            !runtime
                .matches_permission_rule(
                    "bash",
                    &serde_json::json!({
                        "command": command,
                        "sandbox_permissions": "require_escalated"
                    })
                    .to_string()
                )
                .await
        );
    }

    #[tokio::test]
    async fn remembered_file_permission_rule_matches_related_file_tools() {
        let runtime = PermissionRuntimeState::new();

        runtime
            .remember_permission_rule("Write", "File(notes/**)")
            .await;

        assert!(
            runtime
                .matches_permission_rule("Read", r#"{"file_path":"notes/hello.txt"}"#)
                .await
        );
        assert!(
            runtime
                .matches_permission_rule(
                    "Edit",
                    r#"{"file_path":"notes/hello.txt","old_string":"a","new_string":"b"}"#
                )
                .await
        );
        assert!(
            !runtime
                .matches_permission_rule("Read", r#"{"file_path":"other/hello.txt"}"#)
                .await
        );
    }

    #[tokio::test]
    async fn remembered_mcp_rules_match_adapter_tool_inputs() {
        let runtime = PermissionRuntimeState::new();

        runtime
            .remember_permission_rule("call-mcp-tool", "mcp__docs__inspect")
            .await;

        assert!(
            runtime
                .matches_permission_rule(
                    "call-mcp-tool",
                    r#"{"server_id":"docs","tool_name":"inspect","input":{"mode":"brief"}}"#
                )
                .await
        );
        assert!(
            !runtime
                .matches_permission_rule(
                    "call-mcp-tool",
                    r#"{"server_id":"docs","tool_name":"other"}"#
                )
                .await
        );
    }
}
