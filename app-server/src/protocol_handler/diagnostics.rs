use orbcode_app_server_protocol::{
    AdvancedCapabilitiesResult, AdvancedCapabilityOverview, ChildSessionOrphanCleanupResult,
    DiagnosticsCleanupChildSessionsParams, LastProviderRequestResult, PreUserInstructionsResult,
    ProviderRequestDebugOverview, ResponseResult, SessionIdParams,
};
use serde_json::Value;

use super::{core_error, success, try_parse};
use crate::AppServer;
use crate::protocol_conversion::hook_discovery_to_wire;

impl AppServer {
    pub(super) async fn handle_diagnostics_status(&self, params: Option<Value>) -> ResponseResult {
        let p: SessionIdParams = try_parse!(params);
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
            Ok(report) => success(report),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_diagnostics_cleanup_child_sessions(
        &self,
        params: Option<Value>,
    ) -> ResponseResult {
        let p: DiagnosticsCleanupChildSessionsParams = try_parse!(params);
        match self
            .cleanup_orphan_child_sessions(p.dry_run, p.stale_running_cutoff_ms)
            .await
        {
            Ok(result) => success(ChildSessionOrphanCleanupResult {
                dry_run: result.dry_run,
                scoped_cwds: result.scoped_cwds,
                inspected_metadata: result.inspected_metadata,
                orphan_metadata: result.orphan_metadata,
                eligible_metadata: result.eligible_metadata,
                stale_running_metadata: result.stale_running_metadata,
                skipped_running_metadata: result.skipped_running_metadata,
                removed_metadata: result.removed_metadata,
                removed_transcripts: result.removed_transcripts,
                orphan_child_session_ids: result.orphan_child_session_ids,
            }),
            Err(e) => core_error(e),
        }
    }

    pub(super) async fn handle_diagnostics_hooks(&self, _params: Option<Value>) -> ResponseResult {
        let discovery = self.hook_discovery().await;
        success(hook_discovery_to_wire(discovery))
    }

    pub(super) async fn handle_diagnostics_diff(&self, _params: Option<Value>) -> ResponseResult {
        match self.workspace_diff().await {
            Ok(diff) => success(diff),
            Err(e) => core_error(e),
        }
    }

    pub(super) fn handle_diagnostics_advanced(&self, _params: Option<Value>) -> ResponseResult {
        let capabilities = self
            .advanced_capabilities()
            .into_iter()
            .map(|capability| AdvancedCapabilityOverview {
                name: capability.name.to_string(),
                summary: capability.summary.to_string(),
                status: format!("{:?}", capability.status),
            })
            .collect();
        success(AdvancedCapabilitiesResult(capabilities))
    }

    pub(super) async fn handle_diagnostics_last_request(
        &self,
        _params: Option<Value>,
    ) -> ResponseResult {
        let snapshot = self.last_provider_request_snapshot().await;
        match snapshot {
            Some(snapshot) => success(LastProviderRequestResult(Some(
                ProviderRequestDebugOverview {
                    provider: snapshot.provider.as_str().to_string(),
                    source: snapshot.source,
                    session_id: snapshot.session_id,
                    model: snapshot.model,
                    base_url: snapshot.base_url,
                    captured_at: snapshot.captured_at,
                    recent_activity_json: snapshot.recent_activity_json,
                    previous_turn_json: snapshot.previous_turn_json,
                    body_json: snapshot.body_json,
                },
            ))),
            None => success(LastProviderRequestResult(None)),
        }
    }

    pub(super) async fn handle_diagnostics_pre_user_instructions(
        &self,
        params: Option<Value>,
    ) -> ResponseResult {
        let p: SessionIdParams = try_parse!(params);
        let preview = self.pre_user_instructions_preview(&p.session_id).await;
        success(PreUserInstructionsResult { preview })
    }
}
