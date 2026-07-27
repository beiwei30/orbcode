use orbcode_app_server_protocol::ResponseResult;
use serde::Deserialize;
use serde_json::Value;

use super::{core_error, success, try_parse};
use crate::AppServer;

impl AppServer {
    pub(super) async fn handle_diagnostics_status(&self, params: Option<Value>) -> ResponseResult {
        #[derive(Deserialize)]
        struct Params {
            session_id: String,
        }
        let p: Params = try_parse!(params);
        match self.status_overview(&p.session_id).await {
            Ok(overview) => success(overview),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_diagnostics_memory(&self, _params: Option<Value>) -> ResponseResult {
        match self.memory_overview().await {
            Ok(overview) => success(overview),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_diagnostics_doctor(&self, _params: Option<Value>) -> ResponseResult {
        match self.doctor_report().await {
            Ok(report) => {
                let checks: Vec<Value> = report
                    .checks
                    .iter()
                    .map(|c| {
                        serde_json::json!({
                            "name": c.name,
                            "status": format!("{:?}", c.status),
                            "detail": c.detail,
                        })
                    })
                    .collect();
                success(serde_json::json!({ "checks": checks }))
            }
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_diagnostics_cleanup_child_sessions(
        &self,
        params: Option<Value>,
    ) -> ResponseResult {
        #[derive(Deserialize)]
        struct Params {
            #[serde(default = "default_dry_run")]
            dry_run: bool,
            #[serde(default)]
            stale_running_cutoff_ms: Option<i64>,
        }

        fn default_dry_run() -> bool {
            true
        }

        let p: Params = try_parse!(params);
        match self
            .cleanup_orphan_child_sessions(p.dry_run, p.stale_running_cutoff_ms)
            .await
        {
            Ok(result) => success(result),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_diagnostics_hooks(&self, _params: Option<Value>) -> ResponseResult {
        let discovery = self.hook_discovery().await;
        success(discovery)
    }

    pub(super) async fn handle_diagnostics_diff(&self, _params: Option<Value>) -> ResponseResult {
        match self.workspace_diff().await {
            Ok(diff) => success(serde_json::json!({
                "cwd": diff.cwd,
                "status": diff.status,
                "staged_diff": diff.staged_diff,
                "unstaged_diff": diff.unstaged_diff,
                "untracked_files": diff.untracked_files,
            })),
            Err(e) => core_error(e),
        }
    }

    pub(super) fn handle_diagnostics_advanced(&self, _params: Option<Value>) -> ResponseResult {
        let capabilities: Vec<Value> = self
            .advanced_capabilities()
            .into_iter()
            .map(|c| {
                serde_json::json!({
                    "name": c.name,
                    "summary": c.summary,
                    "status": format!("{:?}", c.status),
                })
            })
            .collect();
        success(capabilities)
    }

    pub(super) async fn handle_diagnostics_last_request(
        &self,
        _params: Option<Value>,
    ) -> ResponseResult {
        let snapshot = self.last_provider_request_snapshot().await;
        match snapshot {
            Some(s) => success(serde_json::json!({
                "provider": s.provider.as_str(),
                "source": s.source,
                "session_id": s.session_id,
                "model": s.model,
                "base_url": s.base_url,
                "captured_at": s.captured_at,
                "recent_activity_json": s.recent_activity_json,
                "previous_turn_json": s.previous_turn_json,
                "body_json": s.body_json,
            })),
            None => success(serde_json::Value::Null),
        }
    }

    pub(super) async fn handle_diagnostics_pre_user_instructions(
        &self,
        params: Option<Value>,
    ) -> ResponseResult {
        #[derive(Deserialize)]
        struct Params {
            session_id: String,
        }
        let p: Params = try_parse!(params);
        let preview = self.pre_user_instructions_preview(&p.session_id).await;
        success(serde_json::json!({ "preview": preview }))
    }
}
