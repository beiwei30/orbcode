use orbcode_app_server_protocol::{
    AddDirectoryParams, PermissionDecisionWire, PermissionModeResult, PermissionResponseParams,
    PermissionRuleKind, PermissionRuleParams, PermissionSetModeParams, ResponseResult, SentResult,
    SessionPermissionRuleParams, ValidateDirectoryParams,
};
use orbcode_config::PermissionRuleSettingKind;
use serde_json::Value;

use super::{core_error, invalid_params, success, success_empty, try_parse};
use crate::AppServer;
use crate::permissions::PermissionRuleMutation;
use crate::protocol_conversion::{
    permission_mode_from_wire, permission_mode_to_wire, permission_rule_update_to_wire,
};

fn rule_kind_from_wire(kind: PermissionRuleKind) -> PermissionRuleSettingKind {
    match kind {
        PermissionRuleKind::Allow => PermissionRuleSettingKind::Allow,
        PermissionRuleKind::Deny => PermissionRuleSettingKind::Deny,
    }
}

/// Convert the wire-level [`PermissionDecisionWire`] to core's
/// [`PermissionDecision`](orbcode_core::PermissionDecision).
pub(crate) fn wire_to_core(
    wire: PermissionDecisionWire,
) -> Result<orbcode_core::PermissionDecision, ResponseResult> {
    match wire {
        PermissionDecisionWire::Approve => Ok(orbcode_core::PermissionDecision::Approve),
        PermissionDecisionWire::Deny => Ok(orbcode_core::PermissionDecision::Deny),
        PermissionDecisionWire::ApproveAlways { mut rules } => {
            if rules.len() == 1 {
                let Some(rule) = rules.pop() else {
                    return Err(invalid_params("approve_always requires one rule"));
                };
                Ok(orbcode_core::PermissionDecision::ApproveAlways(rule))
            } else if rules.len() > 1 {
                Ok(orbcode_core::PermissionDecision::ApproveAlwaysMany(rules))
            } else {
                Err(invalid_params("approve_always requires at least one rule"))
            }
        }
        _ => Err(invalid_params("unsupported permission decision")),
    }
}

impl AppServer {
    pub(super) async fn handle_permission_respond(&self, params: Option<Value>) -> ResponseResult {
        let p: PermissionResponseParams = try_parse!(params);
        let decision = match wire_to_core(p.decision) {
            Ok(d) => d,
            Err(resp) => return resp,
        };
        let sent = self
            .respond_to_permission_request(&p.request_id, decision)
            .await;
        success(SentResult { sent })
    }

    pub(super) async fn handle_permission_overview(
        &self,
        _params: Option<Value>,
    ) -> ResponseResult {
        let overview = self.permission_overview().await;
        success(overview)
    }

    pub(super) fn handle_permission_mode(&self, _params: Option<Value>) -> ResponseResult {
        let mode = self.permission_mode();
        success(PermissionModeResult {
            mode: permission_mode_to_wire(mode),
        })
    }

    pub(super) async fn handle_permission_set_mode(&self, params: Option<Value>) -> ResponseResult {
        let p: PermissionSetModeParams = try_parse!(params);
        match self
            .set_permission_mode(permission_mode_from_wire(p.mode))
            .await
        {
            Ok(()) => success_empty(),
            Err(error) => core_error(error),
        }
    }

    pub(super) async fn handle_permission_add_rule(&self, params: Option<Value>) -> ResponseResult {
        let p: PermissionRuleParams = try_parse!(params);
        let kind = rule_kind_from_wire(p.kind);
        let target = p.target_scope();
        match self
            .mutate_permission_rule(PermissionRuleMutation::Add, target, None, kind, &p.rule)
            .await
        {
            Ok(update) => success(permission_rule_update_to_wire(
                update,
                p.kind,
                target,
                self.permission_overview().await.effective_rules,
            )),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_permission_remove_rule(
        &self,
        params: Option<Value>,
    ) -> ResponseResult {
        let p: PermissionRuleParams = try_parse!(params);
        let kind = rule_kind_from_wire(p.kind);
        let target = p.target_scope();
        match self
            .mutate_permission_rule(PermissionRuleMutation::Remove, target, None, kind, &p.rule)
            .await
        {
            Ok(update) => success(permission_rule_update_to_wire(
                update,
                p.kind,
                target,
                self.permission_overview().await.effective_rules,
            )),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_permission_add_session_rule(
        &self,
        params: Option<Value>,
    ) -> ResponseResult {
        let p: SessionPermissionRuleParams = try_parse!(params);
        let kind = rule_kind_from_wire(p.kind);
        let target = p.target_scope();
        match self
            .mutate_permission_rule(
                PermissionRuleMutation::Add,
                target,
                Some(&p.session_id),
                kind,
                &p.rule,
            )
            .await
        {
            Ok(update) => success(permission_rule_update_to_wire(
                update,
                p.kind,
                target,
                self.permission_overview().await.effective_rules,
            )),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_permission_remove_session_rule(
        &self,
        params: Option<Value>,
    ) -> ResponseResult {
        let p: SessionPermissionRuleParams = try_parse!(params);
        let kind = rule_kind_from_wire(p.kind);
        let target = p.target_scope();
        match self
            .mutate_permission_rule(
                PermissionRuleMutation::Remove,
                target,
                Some(&p.session_id),
                kind,
                &p.rule,
            )
            .await
        {
            Ok(update) => success(permission_rule_update_to_wire(
                update,
                p.kind,
                target,
                self.permission_overview().await.effective_rules,
            )),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_permission_add_directory(
        &self,
        params: Option<Value>,
    ) -> ResponseResult {
        let p: AddDirectoryParams = try_parse!(params);
        match self.add_directory(&p.session_id, p.directory).await {
            Ok(added) => success(added),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_permission_validate_directory(
        &self,
        params: Option<Value>,
    ) -> ResponseResult {
        let p: ValidateDirectoryParams = try_parse!(params);
        match self.validate_add_directory(&p.path).await {
            Ok(candidate) => success(candidate),
            Err(e) => core_error(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use orbcode_app_server_protocol::{ErrorCode, PermissionDecisionWire, ResponseResult};
    use orbcode_core::PermissionDecision;

    use super::wire_to_core;

    #[test]
    fn wire_to_core_approve() {
        let result = wire_to_core(PermissionDecisionWire::Approve).unwrap();
        assert_eq!(result, PermissionDecision::Approve);
    }

    #[test]
    fn wire_to_core_deny() {
        let result = wire_to_core(PermissionDecisionWire::Deny).unwrap();
        assert_eq!(result, PermissionDecision::Deny);
    }

    #[test]
    fn wire_to_core_approve_always_single_rule() {
        let result = wire_to_core(PermissionDecisionWire::ApproveAlways {
            rules: vec!["Bash(*)".to_string()],
        })
        .unwrap();
        assert_eq!(
            result,
            PermissionDecision::ApproveAlways("Bash(*)".to_string())
        );
    }

    #[test]
    fn wire_to_core_approve_always_multiple_rules() {
        let rules = vec!["Bash(*)".to_string(), "Read(*)".to_string()];
        let result = wire_to_core(PermissionDecisionWire::ApproveAlways {
            rules: rules.clone(),
        })
        .unwrap();
        assert_eq!(result, PermissionDecision::ApproveAlwaysMany(rules));
    }

    #[test]
    fn wire_to_core_approve_always_empty_rules_errors() {
        let result = wire_to_core(PermissionDecisionWire::ApproveAlways { rules: vec![] });
        match result {
            Err(ResponseResult::Error(e)) => {
                assert_eq!(e.code, ErrorCode::InvalidParams);
                assert!(e.message.contains("at least one rule"));
            }
            other => panic!("expected InvalidParams error, got: {other:?}"),
        }
    }
}
