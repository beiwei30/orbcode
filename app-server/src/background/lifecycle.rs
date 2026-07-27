use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::Utc;
#[cfg(test)]
use orbcode_config::PermissionMode;
use uuid::Uuid;

use orbcode_core::CoreError;

use super::{BackgroundJobRecord, BackgroundJobStatus, BackgroundManager, CreateBackgroundJob};

impl BackgroundManager {
    pub async fn create_job(
        &self,
        request: CreateBackgroundJob,
    ) -> Result<BackgroundJobRecord, CoreError> {
        self.ensure_dirs().await?;
        let job_id = Uuid::new_v4().to_string();
        let now = Utc::now();
        let log_path = self.logs_dir.join(format!("{job_id}.log"));
        tokio::fs::write(&log_path, "").await?;

        let record = BackgroundJobRecord {
            job_id: job_id.clone(),
            session_id: request.session_id,
            prompt: request.prompt,
            cwd: request.cwd.display().to_string(),
            provider: request.provider,
            fallback_provider: request.fallback_provider,
            model: request.model,
            permission_mode: request.permission_mode,
            status: BackgroundJobStatus::Queued,
            created_at: now,
            updated_at: now,
            started_at: None,
            finished_at: None,
            pid: None,
            log_path: log_path.display().to_string(),
            error: None,
            exit_code: None,
            signal: None,
            last_log_offset: 0,
            cancellation_reason: None,
        };
        self.save_job(&record).await?;
        let token = Arc::new(AtomicBool::new(false));
        self.cancel_tokens
            .lock()
            .expect("cancel_tokens lock")
            .insert(job_id, token);
        Ok(record)
    }

    /// Returns the cancellation token for a job, if one exists in memory.
    /// Tokens are only present for jobs created by this `BackgroundManager`
    /// instance; a resumed manager does not re-populate them from disk.
    pub fn cancel_token(&self, job_id: &str) -> Option<Arc<AtomicBool>> {
        self.cancel_tokens
            .lock()
            .expect("cancel_tokens lock")
            .get(job_id)
            .cloned()
    }

    /// Signal a job's cancellation token, causing any subscriber holding a
    /// clone of the `Arc<AtomicBool>` to observe cancellation. Returns `true`
    /// if a token was found and signalled, `false` if the job had no in-memory
    /// token (already finished, or created by a previous process).
    pub fn signal_cancel(&self, job_id: &str) -> bool {
        if let Some(token) = self
            .cancel_tokens
            .lock()
            .expect("cancel_tokens lock")
            .get(job_id)
        {
            token.store(true, Ordering::SeqCst);
            true
        } else {
            false
        }
    }

    /// Remove a job's cancellation token from the in-memory map. Called when a
    /// job reaches a terminal state so the token is no longer held.
    fn remove_cancel_token(&self, job_id: &str) {
        self.cancel_tokens
            .lock()
            .expect("cancel_tokens lock")
            .remove(job_id);
    }

    pub async fn mark_running(
        &self,
        job_id: &str,
        pid: Option<u32>,
    ) -> Result<BackgroundJobRecord, CoreError> {
        self.transition(job_id, BackgroundJobStatus::Running, |record| {
            let now = Utc::now();
            record.updated_at = now;
            record.started_at = Some(now);
            record.pid = pid;
            record.error = None;
        })
        .await
    }

    pub async fn mark_completed(&self, job_id: &str) -> Result<BackgroundJobRecord, CoreError> {
        self.finish(job_id, BackgroundJobStatus::Completed, None, Some(0), None)
            .await
    }

    pub async fn mark_completed_with_exit(
        &self,
        job_id: &str,
        exit_code: Option<i32>,
        signal: Option<i32>,
    ) -> Result<BackgroundJobRecord, CoreError> {
        self.finish(
            job_id,
            BackgroundJobStatus::Completed,
            None,
            exit_code,
            signal,
        )
        .await
    }

    pub async fn mark_failed(
        &self,
        job_id: &str,
        error: impl Into<String>,
    ) -> Result<BackgroundJobRecord, CoreError> {
        self.finish(
            job_id,
            BackgroundJobStatus::Failed,
            Some(error.into()),
            Some(1),
            None,
        )
        .await
    }

    pub async fn mark_failed_with_exit(
        &self,
        job_id: &str,
        error: impl Into<String>,
        exit_code: Option<i32>,
        signal: Option<i32>,
    ) -> Result<BackgroundJobRecord, CoreError> {
        self.finish(
            job_id,
            BackgroundJobStatus::Failed,
            Some(error.into()),
            exit_code,
            signal,
        )
        .await
    }

    pub async fn mark_cancelled(
        &self,
        job_id: &str,
        reason: Option<String>,
    ) -> Result<BackgroundJobRecord, CoreError> {
        self.finish(job_id, BackgroundJobStatus::Cancelled, reason, None, None)
            .await
    }

    pub async fn mark_cancelled_with_signal(
        &self,
        job_id: &str,
        reason: Option<String>,
        signal: Option<i32>,
    ) -> Result<BackgroundJobRecord, CoreError> {
        self.finish(job_id, BackgroundJobStatus::Cancelled, reason, None, signal)
            .await
    }

    async fn finish(
        &self,
        job_id: &str,
        status: BackgroundJobStatus,
        error: Option<String>,
        exit_code: Option<i32>,
        signal: Option<i32>,
    ) -> Result<BackgroundJobRecord, CoreError> {
        let offset = self.log_len(job_id).await;
        let result = self
            .transition(job_id, status, |record| {
                let now = Utc::now();
                record.updated_at = now;
                record.finished_at = Some(now);
                record.error = error.clone();
                record.exit_code = exit_code;
                record.signal = signal;
                record.last_log_offset = offset;
                if status == BackgroundJobStatus::Cancelled {
                    record.cancellation_reason = error.clone();
                }
            })
            .await;
        if result.is_ok() {
            self.remove_cancel_token(job_id);
        }
        result
    }

    /// Apply a validated lifecycle transition. Reads the current record without
    /// orphan reconciliation, rejects illegal moves via
    /// [`BackgroundJobStatus::can_transition_to`], then persists the result.
    async fn transition<F>(
        &self,
        job_id: &str,
        next: BackgroundJobStatus,
        mut updater: F,
    ) -> Result<BackgroundJobRecord, CoreError>
    where
        F: FnMut(&mut BackgroundJobRecord),
    {
        let mut record = self.read_record(job_id).await?;
        if !record.status.can_transition_to(next) {
            return Err(CoreError::Config(format!(
                "invalid background job transition: {} -> {next} ({job_id})",
                record.status
            )));
        }
        record.status = next;
        updater(&mut record);
        self.save_job(&record).await?;
        Ok(record)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::Ordering;

    use orbcode_protocol::ProviderId;
    use uuid::Uuid;

    use super::*;
    use crate::background::CreateBackgroundJob;

    fn manager(label: &str) -> BackgroundManager {
        let home = std::env::temp_dir().join(format!(
            "orbcode-background-{label}-{}",
            Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&home).expect("create home");
        BackgroundManager::new(home)
    }

    fn test_job(prompt: &str) -> CreateBackgroundJob {
        CreateBackgroundJob {
            session_id: "session-1".to_string(),
            prompt: prompt.to_string(),
            cwd: PathBuf::from("/tmp"),
            provider: ProviderId::Anthropic,
            fallback_provider: None,
            model: "claude-sonnet-4-20250514".to_string(),
            permission_mode: None,
        }
    }

    #[tokio::test]
    async fn appends_logs_and_finishes_jobs() {
        let manager = manager("logs");
        let record = manager
            .create_job(test_job("hello"))
            .await
            .expect("create job");
        manager
            .mark_running(&record.job_id, Some(123))
            .await
            .expect("mark running");
        manager
            .append_log(&record.job_id, "hello world")
            .await
            .expect("append log");
        manager
            .mark_completed(&record.job_id)
            .await
            .expect("mark completed");

        let log = manager.read_log(&record.job_id).await.expect("read log");
        assert_eq!(log, "hello world");
        let loaded = manager.load_job(&record.job_id).await.expect("load job");
        assert_eq!(loaded.status, BackgroundJobStatus::Completed);
    }

    #[tokio::test]
    async fn rejects_illegal_transition_at_runtime() {
        let manager = manager("illegal-transition");
        let record = manager
            .create_job(test_job("transition"))
            .await
            .expect("create job");
        manager
            .mark_running(&record.job_id, None)
            .await
            .expect("mark running");
        manager
            .mark_completed(&record.job_id)
            .await
            .expect("mark completed");

        // Completed -> Failed is not a legal transition.
        let err = manager.mark_failed(&record.job_id, "boom").await;
        assert!(err.is_err(), "expected illegal transition to be rejected");
        let loaded = manager.load_job(&record.job_id).await.expect("load job");
        assert_eq!(loaded.status, BackgroundJobStatus::Completed);
    }

    #[tokio::test]
    async fn mark_cancelled_records_cancellation_reason() {
        let manager = manager("cancel-reason");
        let record = manager
            .create_job(test_job("cancel"))
            .await
            .expect("create job");
        manager
            .mark_running(&record.job_id, None)
            .await
            .expect("mark running");
        let cancelled = manager
            .mark_cancelled(&record.job_id, Some("user stopped".to_string()))
            .await
            .expect("mark cancelled");
        assert_eq!(cancelled.status, BackgroundJobStatus::Cancelled);
        assert_eq!(
            cancelled.cancellation_reason.as_deref(),
            Some("user stopped")
        );
    }

    #[tokio::test]
    async fn mark_completed_sets_exit_code_zero() {
        let manager = manager("exit-completed");
        let record = manager
            .create_job(test_job("done"))
            .await
            .expect("create job");
        manager
            .mark_running(&record.job_id, Some(100))
            .await
            .expect("mark running");
        let completed = manager
            .mark_completed(&record.job_id)
            .await
            .expect("mark completed");
        assert_eq!(completed.exit_code, Some(0));
        assert_eq!(completed.signal, None);
    }

    #[tokio::test]
    async fn mark_failed_sets_exit_code_one() {
        let manager = manager("exit-failed");
        let record = manager
            .create_job(test_job("fail"))
            .await
            .expect("create job");
        manager
            .mark_running(&record.job_id, None)
            .await
            .expect("mark running");
        let failed = manager
            .mark_failed(&record.job_id, "something went wrong")
            .await
            .expect("mark failed");
        assert_eq!(failed.exit_code, Some(1));
        assert_eq!(failed.signal, None);
    }

    #[tokio::test]
    async fn mark_failed_with_explicit_exit_code_and_signal() {
        let manager = manager("exit-explicit");
        let record = manager
            .create_job(test_job("explicit"))
            .await
            .expect("create job");
        manager
            .mark_running(&record.job_id, None)
            .await
            .expect("mark running");
        let failed = manager
            .mark_failed_with_exit(&record.job_id, "killed", Some(137), Some(9))
            .await
            .expect("mark failed with exit");
        assert_eq!(failed.exit_code, Some(137));
        assert_eq!(failed.signal, Some(9));
    }

    #[tokio::test]
    async fn mark_cancelled_with_signal_records_signal() {
        let manager = manager("exit-signal");
        let record = manager
            .create_job(test_job("signal"))
            .await
            .expect("create job");
        manager
            .mark_running(&record.job_id, Some(42))
            .await
            .expect("mark running");
        let cancelled = manager
            .mark_cancelled_with_signal(&record.job_id, Some("SIGTERM".to_string()), Some(15))
            .await
            .expect("mark cancelled with signal");
        assert_eq!(cancelled.exit_code, None);
        assert_eq!(cancelled.signal, Some(15));
        assert_eq!(cancelled.cancellation_reason.as_deref(), Some("SIGTERM"));
    }

    #[tokio::test]
    async fn summary_includes_exit_info() {
        let manager = manager("summary-exit");
        let record = manager
            .create_job(test_job("summary test"))
            .await
            .expect("create job");
        manager
            .mark_running(&record.job_id, None)
            .await
            .expect("mark running");
        let completed = manager
            .mark_completed(&record.job_id)
            .await
            .expect("mark completed");
        let summary = completed.summary();
        assert_eq!(summary.exit_code, Some(0));
        assert_eq!(summary.signal, None);
    }

    #[tokio::test]
    async fn exit_code_survives_serde_round_trip() {
        let manager = manager("exit-serde");
        let record = manager
            .create_job(test_job("serde"))
            .await
            .expect("create job");
        manager
            .mark_running(&record.job_id, None)
            .await
            .expect("mark running");
        manager
            .mark_failed_with_exit(&record.job_id, "oops", Some(42), Some(15))
            .await
            .expect("mark failed");

        let loaded = manager.load_job(&record.job_id).await.expect("reload");
        assert_eq!(loaded.exit_code, Some(42));
        assert_eq!(loaded.signal, Some(15));
    }

    #[tokio::test]
    async fn record_persists_model_and_permission_mode() {
        let manager = manager("metadata-persist");
        let record = manager
            .create_job(CreateBackgroundJob {
                model: "claude-opus-4-20250514".to_string(),
                permission_mode: Some(PermissionMode::BypassPermissions),
                ..test_job("metadata test")
            })
            .await
            .expect("create job");
        assert_eq!(record.model, "claude-opus-4-20250514");
        assert_eq!(
            record.permission_mode,
            Some(PermissionMode::BypassPermissions)
        );

        let loaded = manager.load_job(&record.job_id).await.expect("reload");
        assert_eq!(loaded.model, "claude-opus-4-20250514");
        assert_eq!(
            loaded.permission_mode,
            Some(PermissionMode::BypassPermissions)
        );
    }

    #[tokio::test]
    async fn summary_includes_provider_and_model() {
        let manager = manager("summary-provider-model");
        let record = manager
            .create_job(CreateBackgroundJob {
                model: "claude-opus-4-20250514".to_string(),
                ..test_job("ps display test")
            })
            .await
            .expect("create job");
        let summary = record.summary();
        assert_eq!(summary.provider, ProviderId::Anthropic);
        assert_eq!(summary.model, "claude-opus-4-20250514");
        assert!(summary.to_string().contains("anthropic"));
        assert!(summary.to_string().contains("claude-opus-4-20250514"));
    }

    #[tokio::test]
    async fn cancel_token_created_at_job_creation() {
        let manager = manager("cancel-token-create");
        let record = manager
            .create_job(test_job("cancel token test"))
            .await
            .expect("create job");
        let token = manager
            .cancel_token(&record.job_id)
            .expect("token should exist");
        assert!(!token.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn signal_cancel_sets_token_flag() {
        let manager = manager("cancel-token-signal");
        let record = manager
            .create_job(test_job("cancel signal test"))
            .await
            .expect("create job");
        let token = manager
            .cancel_token(&record.job_id)
            .expect("token should exist");
        assert!(!token.load(Ordering::SeqCst));

        assert!(manager.signal_cancel(&record.job_id));
        assert!(token.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn cancel_token_removed_on_terminal_transition() {
        let manager = manager("cancel-token-cleanup");
        let record = manager
            .create_job(test_job("cleanup test"))
            .await
            .expect("create job");
        assert!(manager.cancel_token(&record.job_id).is_some());

        manager
            .mark_running(&record.job_id, None)
            .await
            .expect("mark running");
        assert!(manager.cancel_token(&record.job_id).is_some());

        manager
            .mark_completed(&record.job_id)
            .await
            .expect("mark completed");
        assert!(
            manager.cancel_token(&record.job_id).is_none(),
            "token should be removed after terminal transition"
        );
    }

    #[test]
    fn signal_cancel_noop_for_unknown_job() {
        let manager = manager("cancel-token-noop");
        assert!(!manager.signal_cancel("nonexistent-job-id"));
    }

    #[tokio::test]
    async fn cancel_token_absent_after_process_restart() {
        let home = std::env::temp_dir().join(format!(
            "orbcode-background-cancel-resume-{}",
            Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&home).expect("create home");
        let manager = BackgroundManager::new(home.clone());
        let record = manager
            .create_job(test_job("restart test"))
            .await
            .expect("create job");
        assert!(manager.cancel_token(&record.job_id).is_some());

        let resumed = BackgroundManager::new(home);
        assert!(
            resumed.cancel_token(&record.job_id).is_none(),
            "resumed manager should not have in-memory tokens from previous process"
        );
        assert!(
            !resumed.signal_cancel(&record.job_id),
            "signalling on resumed manager is a no-op"
        );
    }

    #[tokio::test]
    async fn creates_and_lists_jobs() {
        let manager = manager("list");
        let record = manager
            .create_job(CreateBackgroundJob {
                fallback_provider: Some(ProviderId::OpenAi),
                ..test_job("long prompt")
            })
            .await
            .expect("create job");
        assert_eq!(record.status, BackgroundJobStatus::Queued);

        let jobs = manager.list_jobs().await.expect("list jobs");
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].job_id, record.job_id);
    }
}
