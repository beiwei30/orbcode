use std::path::PathBuf;

use orbcode_app_server_protocol::{
    PermissionDecisionWire, PermissionResponseParams, ResponseResult,
};
use orbcode_config::{PermissionMode, PermissionRuleSettingKind};
use serde::Deserialize;
use serde_json::Value;

use super::{core_error, invalid_params, success, success_empty, try_parse};
use crate::AppServer;

/// Convert the wire-level [`PermissionDecisionWire`] to core's
/// [`PermissionDecision`](orbcode_core::PermissionDecision).
pub(crate) fn wire_to_core(
    wire: PermissionDecisionWire,
) -> Result<orbcode_core::PermissionDecision, ResponseResult> {
    match wire {
        PermissionDecisionWire::Approve => Ok(orbcode_core::PermissionDecision::Approve),
        PermissionDecisionWire::Deny => Ok(orbcode_core::PermissionDecision::Deny),
        PermissionDecisionWire::ApproveAlways { rules } => {
            if rules.len() == 1 {
                Ok(orbcode_core::PermissionDecision::ApproveAlways(
                    rules.into_iter().next().unwrap(),
                ))
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
        success(serde_json::json!({ "sent": sent }))
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
        success(serde_json::json!({ "mode": mode.as_str() }))
    }

    pub(super) fn handle_permission_set_mode(&self, params: Option<Value>) -> ResponseResult {
        #[derive(Deserialize)]
        struct Params {
            mode: String,
        }
        let p: Params = try_parse!(params);
        let Some(mode) = PermissionMode::parse(&p.mode) else {
            return invalid_params(format!("unknown permission mode: {}", p.mode));
        };
        self.set_permission_mode(mode);
        success_empty()
    }

    pub(super) async fn handle_permission_add_rule(&self, params: Option<Value>) -> ResponseResult {
        #[derive(Deserialize)]
        struct Params {
            kind: String,
            rule: String,
        }
        let p: Params = try_parse!(params);
        let kind = match p.kind.as_str() {
            "allow" => PermissionRuleSettingKind::Allow,
            "deny" => PermissionRuleSettingKind::Deny,
            other => return invalid_params(format!("unknown rule kind: {other}")),
        };
        match self.add_permission_rule(kind, &p.rule).await {
            Ok(update) => success(update),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_permission_remove_rule(
        &self,
        params: Option<Value>,
    ) -> ResponseResult {
        #[derive(Deserialize)]
        struct Params {
            kind: String,
            rule: String,
        }
        let p: Params = try_parse!(params);
        let kind = match p.kind.as_str() {
            "allow" => PermissionRuleSettingKind::Allow,
            "deny" => PermissionRuleSettingKind::Deny,
            other => return invalid_params(format!("unknown rule kind: {other}")),
        };
        match self.remove_permission_rule(kind, &p.rule).await {
            Ok(update) => success(update),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_permission_add_session_rule(
        &self,
        params: Option<Value>,
    ) -> ResponseResult {
        #[derive(Deserialize)]
        struct Params {
            session_id: String,
            kind: String,
            rule: String,
        }
        let p: Params = try_parse!(params);
        let kind = match p.kind.as_str() {
            "allow" => PermissionRuleSettingKind::Allow,
            "deny" => PermissionRuleSettingKind::Deny,
            other => return invalid_params(format!("unknown rule kind: {other}")),
        };
        match self
            .add_session_permission_rule(&p.session_id, kind, &p.rule)
            .await
        {
            Ok(update) => success(update),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_permission_remove_session_rule(
        &self,
        params: Option<Value>,
    ) -> ResponseResult {
        #[derive(Deserialize)]
        struct Params {
            session_id: String,
            kind: String,
            rule: String,
        }
        let p: Params = try_parse!(params);
        let kind = match p.kind.as_str() {
            "allow" => PermissionRuleSettingKind::Allow,
            "deny" => PermissionRuleSettingKind::Deny,
            other => return invalid_params(format!("unknown rule kind: {other}")),
        };
        match self
            .remove_session_permission_rule(&p.session_id, kind, &p.rule)
            .await
        {
            Ok(update) => success(update),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_permission_add_directory(
        &self,
        params: Option<Value>,
    ) -> ResponseResult {
        #[derive(Deserialize)]
        struct Params {
            session_id: String,
            directory: PathBuf,
        }
        let p: Params = try_parse!(params);
        match self.add_directory(&p.session_id, p.directory).await {
            Ok(added) => success(serde_json::json!({ "path": added.path })),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_permission_validate_directory(
        &self,
        params: Option<Value>,
    ) -> ResponseResult {
        #[derive(Deserialize)]
        struct Params {
            path: String,
        }
        let p: Params = try_parse!(params);
        match self.validate_add_directory(&p.path).await {
            Ok(candidate) => success(serde_json::json!({ "path": candidate.path })),
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
