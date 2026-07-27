use std::path::PathBuf;

use chrono::Utc;
use tokio::time::{Duration, sleep};

use orbcode_core::CoreError;

use super::{
    BackgroundJobDetail, BackgroundJobRecord, BackgroundJobStatus, BackgroundJobSummary,
    BackgroundManager, BackgroundTaskOutput, BackgroundTaskOutputResponse, BackgroundTaskProgress,
    BackgroundTaskRetrievalStatus, ProgressSummary, process_is_alive, truncate_prompt,
};

impl BackgroundManager {
    pub async fn list_jobs(&self) -> Result<Vec<BackgroundJobSummary>, CoreError> {
        self.ensure_dirs().await?;
        let mut dir = tokio::fs::read_dir(&self.jobs_dir).await?;
        let mut jobs = Vec::new();

        while let Some(entry) = dir.next_entry().await? {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let contents = tokio::fs::read_to_string(&path).await?;
            let Ok(record) = serde_json::from_str::<BackgroundJobRecord>(&contents) else {
                continue;
            };
            let record = self.reconcile_orphan(record).await?;
            jobs.push(record.summary());
        }

        jobs.sort_by_key(|job| std::cmp::Reverse(job.updated_at));
        Ok(jobs)
    }

    pub async fn load_job(&self, job_id: &str) -> Result<BackgroundJobRecord, CoreError> {
        let record = self.read_record(job_id).await?;
        self.reconcile_orphan(record).await
    }

    pub async fn job_detail(&self, job_id: &str) -> Result<BackgroundJobDetail, CoreError> {
        let record = self.read_record(job_id).await?;
        let log = self.read_log(job_id).await.unwrap_or_default();
        let log_bytes = log.len() as u64;
        let log_tail: Vec<String> = log
            .lines()
            .rev()
            .take(10)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .map(String::from)
            .collect();
        let start = record.started_at.unwrap_or(record.created_at);
        let elapsed_ms = if record.status.is_active() {
            (Utc::now() - start).num_milliseconds()
        } else {
            record.finished_at.map_or_else(
                || (record.updated_at - start).num_milliseconds(),
                |finished| (finished - start).num_milliseconds(),
            )
        };
        let progress_summary = if record.status.is_active() {
            Some(ProgressSummary {
                last_lines: log_tail.clone(),
                elapsed_ms,
                output_bytes: log_bytes,
            })
        } else {
            None
        };
        Ok(BackgroundJobDetail {
            job_id: record.job_id,
            session_id: record.session_id,
            prompt: record.prompt,
            cwd: record.cwd,
            provider: record.provider,
            fallback_provider: record.fallback_provider,
            model: record.model,
            permission_mode: record.permission_mode,
            status: record.status,
            created_at: record.created_at,
            updated_at: record.updated_at,
            started_at: record.started_at,
            finished_at: record.finished_at,
            pid: record.pid,
            exit_code: record.exit_code,
            signal: record.signal,
            error: record.error,
            cancellation_reason: record.cancellation_reason,
            log_tail,
            progress_summary,
            elapsed_ms,
        })
    }

    pub async fn task_output(
        &self,
        job_id: &str,
        block: bool,
        timeout_ms: u64,
    ) -> Result<BackgroundTaskOutputResponse, CoreError> {
        let mut record = self.read_record(job_id).await?;
        if block {
            let started = std::time::Instant::now();
            while record.status.is_active() && started.elapsed() < Duration::from_millis(timeout_ms)
            {
                sleep(Duration::from_millis(100)).await;
                record = self.read_record(job_id).await?;
            }
        }

        let retrieval_status = if record.status.is_active() {
            if block {
                BackgroundTaskRetrievalStatus::Timeout
            } else {
                BackgroundTaskRetrievalStatus::NotReady
            }
        } else {
            BackgroundTaskRetrievalStatus::Success
        };
        let task = self.task_output_payload(record).await?;
        Ok(BackgroundTaskOutputResponse {
            retrieval_status,
            task: Some(task),
        })
    }

    /// Detect and persist orphaned jobs: a record left `Running` whose pid is no
    /// longer alive (its owning process crashed) is rewritten to `Orphaned` so
    /// readers see a stable terminal state. Non-`Running` records pass through
    /// untouched.
    pub(super) async fn reconcile_orphan(
        &self,
        mut record: BackgroundJobRecord,
    ) -> Result<BackgroundJobRecord, CoreError> {
        if record.status != BackgroundJobStatus::Running {
            return Ok(record);
        }
        let Some(pid) = record.pid else {
            return Ok(record);
        };
        if process_is_alive(pid).await {
            return Ok(record);
        }
        if !record
            .status
            .can_transition_to(BackgroundJobStatus::Orphaned)
        {
            return Ok(record);
        }
        let now = Utc::now();
        record.status = BackgroundJobStatus::Orphaned;
        record.updated_at = now;
        record.finished_at = Some(now);
        record.last_log_offset = self.log_len(&record.job_id).await;
        if record.exit_code.is_none() && record.signal.is_none() {
            record.signal = Some(9);
        }
        if record.error.is_none() {
            record.error = Some(format!("process {pid} is no longer alive; marked orphaned"));
        }
        self.save_job(&record).await?;
        Ok(record)
    }

    pub(super) async fn log_len(&self, job_id: &str) -> u64 {
        let path = self.logs_dir.join(format!("{job_id}.log"));
        tokio::fs::metadata(&path)
            .await
            .map_or(0, |metadata| metadata.len())
    }

    async fn task_output_payload(
        &self,
        record: BackgroundJobRecord,
    ) -> Result<BackgroundTaskOutput, CoreError> {
        let path = PathBuf::from(&record.log_path);
        let output = if tokio::fs::try_exists(&path).await? {
            tokio::fs::read_to_string(&path).await?
        } else {
            String::new()
        };
        let log_bytes = tokio::fs::metadata(&path)
            .await
            .map_or(0, |metadata| metadata.len());
        let result = if record.status.is_active() {
            None
        } else {
            Some(output.clone())
        };
        let progress_summary = if record.status.is_active() {
            let start = record.started_at.unwrap_or(record.created_at);
            let elapsed_ms = (Utc::now() - start).num_milliseconds();
            let last_lines: Vec<String> = output
                .lines()
                .rev()
                .take(5)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .map(String::from)
                .collect();
            Some(ProgressSummary {
                last_lines,
                elapsed_ms,
                output_bytes: log_bytes,
            })
        } else {
            None
        };
        Ok(BackgroundTaskOutput {
            task_id: record.job_id,
            task_type: "background_job".to_string(),
            status: record.status,
            description: truncate_prompt(&record.prompt),
            output,
            output_path: record.log_path,
            error: record.error,
            result,
            progress: BackgroundTaskProgress {
                active: record.status.is_active(),
                created_at: record.created_at,
                updated_at: record.updated_at,
                started_at: record.started_at,
                finished_at: record.finished_at,
                pid: record.pid,
                log_bytes,
            },
            progress_summary,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use orbcode_protocol::ProviderId;
    use tokio::time::{Duration, sleep};
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
    async fn task_output_polls_and_survives_new_manager() {
        let home = std::env::temp_dir().join(format!(
            "orbcode-background-resume-{}",
            Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&home).expect("create home");
        let manager = BackgroundManager::new(home.clone());
        let record = manager
            .create_job(test_job("collect durable output"))
            .await
            .expect("create job");
        manager
            .mark_running(&record.job_id, Some(456))
            .await
            .expect("mark running");

        let finisher = {
            let manager = manager.clone();
            let job_id = record.job_id.clone();
            tokio::spawn(async move {
                sleep(Duration::from_millis(50)).await;
                manager
                    .append_log(&job_id, "finished\n")
                    .await
                    .expect("append log");
                manager
                    .mark_completed(&job_id)
                    .await
                    .expect("mark completed");
            })
        };

        let resumed = BackgroundManager::new(home);
        let output = resumed
            .task_output(&record.job_id, true, 1_000)
            .await
            .expect("poll output");
        finisher.await.expect("finish job");

        assert_eq!(
            output.retrieval_status,
            BackgroundTaskRetrievalStatus::Success
        );
        let task = output.task.expect("task output");
        assert_eq!(task.status, BackgroundJobStatus::Completed);
        assert_eq!(task.output, "finished\n");
        assert_eq!(task.result.as_deref(), Some("finished\n"));
        assert!(!task.progress.active);
        assert_eq!(task.progress.log_bytes, "finished\n".len() as u64);
    }

    #[tokio::test]
    async fn cancelled_state_survives_new_manager() {
        let home = std::env::temp_dir().join(format!(
            "orbcode-background-cancel-{}",
            Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&home).expect("create home");
        let manager = BackgroundManager::new(home.clone());
        let record = manager
            .create_job(test_job("cancel durable output"))
            .await
            .expect("create job");
        manager
            .mark_running(&record.job_id, None)
            .await
            .expect("mark running");
        manager
            .mark_cancelled(&record.job_id, Some("termination requested".to_string()))
            .await
            .expect("mark cancelled");

        let resumed = BackgroundManager::new(home);
        let loaded = resumed.load_job(&record.job_id).await.expect("load job");
        assert_eq!(loaded.status, BackgroundJobStatus::Cancelled);
        assert_eq!(loaded.error.as_deref(), Some("termination requested"));

        let output = resumed
            .task_output(&record.job_id, false, 0)
            .await
            .expect("read cancelled output");
        assert_eq!(
            output.retrieval_status,
            BackgroundTaskRetrievalStatus::Success
        );
        let task = output.task.expect("task output");
        assert_eq!(task.status, BackgroundJobStatus::Cancelled);
        assert_eq!(task.error.as_deref(), Some("termination requested"));
        assert!(!task.progress.active);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn orphan_detection_marks_dead_running_job() {
        let home = std::env::temp_dir().join(format!(
            "orbcode-background-orphan-{}",
            Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&home).expect("create home");
        let manager = BackgroundManager::new(home.clone());
        let record = manager
            .create_job(test_job("orphan me"))
            .await
            .expect("create job");

        // Spawn a real child, capture its pid, then reap it so the pid is
        // guaranteed dead.
        let mut child = tokio::process::Command::new("true")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn helper process");
        let dead_pid = child.id().expect("child pid");
        child.wait().await.expect("reap helper process");

        manager
            .mark_running(&record.job_id, Some(dead_pid))
            .await
            .expect("mark running");

        // load_job reconciles the dead Running record into Orphaned.
        let reconciled = manager.load_job(&record.job_id).await.expect("load job");
        assert_eq!(reconciled.status, BackgroundJobStatus::Orphaned);
        assert!(reconciled.status.is_terminal());
        assert!(reconciled.finished_at.is_some());
        assert!(reconciled.error.is_some());

        // A fresh manager (process restart) sees the persisted terminal state.
        let resumed = BackgroundManager::new(home);
        let loaded = resumed.load_job(&record.job_id).await.expect("reload job");
        assert_eq!(loaded.status, BackgroundJobStatus::Orphaned);
        let jobs = resumed.list_jobs().await.expect("list jobs");
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].status, BackgroundJobStatus::Orphaned);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn orphan_detection_sets_signal() {
        let home = std::env::temp_dir().join(format!(
            "orbcode-background-orphan-sig-{}",
            Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&home).expect("create home");
        let manager = BackgroundManager::new(home);
        let record = manager
            .create_job(test_job("orphan signal"))
            .await
            .expect("create job");
        let mut child = tokio::process::Command::new("true")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn helper process");
        let dead_pid = child.id().expect("child pid");
        child.wait().await.expect("reap helper process");
        manager
            .mark_running(&record.job_id, Some(dead_pid))
            .await
            .expect("mark running");
        let reconciled = manager.load_job(&record.job_id).await.expect("load job");
        assert_eq!(reconciled.status, BackgroundJobStatus::Orphaned);
        assert_eq!(reconciled.signal, Some(9));
    }

    #[tokio::test]
    async fn progress_summary_present_for_running_job() {
        let manager = manager("progress-running");
        let record = manager
            .create_job(test_job("progress summary test"))
            .await
            .expect("create job");
        manager
            .mark_running(&record.job_id, Some(999))
            .await
            .expect("mark running");
        manager
            .append_log(
                &record.job_id,
                "line 1\nline 2\nline 3\nline 4\nline 5\nline 6\nline 7\n",
            )
            .await
            .expect("append log");

        let output = manager
            .task_output(&record.job_id, false, 0)
            .await
            .expect("task output");
        let task = output.task.expect("task");
        assert_eq!(task.status, BackgroundJobStatus::Running);
        let summary = task.progress_summary.expect("should have progress_summary");
        assert_eq!(
            summary.last_lines,
            vec!["line 3", "line 4", "line 5", "line 6", "line 7"]
        );
        assert!(summary.elapsed_ms >= 0);
        assert!(summary.output_bytes > 0);
    }

    #[tokio::test]
    async fn progress_summary_absent_for_completed_job() {
        let manager = manager("progress-completed");
        let record = manager
            .create_job(test_job("completed summary test"))
            .await
            .expect("create job");
        manager
            .mark_running(&record.job_id, None)
            .await
            .expect("mark running");
        manager
            .append_log(&record.job_id, "done\n")
            .await
            .expect("append log");
        manager
            .mark_completed(&record.job_id)
            .await
            .expect("mark completed");

        let output = manager
            .task_output(&record.job_id, false, 0)
            .await
            .expect("task output");
        let task = output.task.expect("task");
        assert_eq!(task.status, BackgroundJobStatus::Completed);
        assert!(
            task.progress_summary.is_none(),
            "completed job should not have progress_summary"
        );
    }

    #[tokio::test]
    async fn progress_summary_fewer_than_five_lines() {
        let manager = manager("progress-short");
        let record = manager
            .create_job(test_job("short log test"))
            .await
            .expect("create job");
        manager
            .mark_running(&record.job_id, Some(100))
            .await
            .expect("mark running");
        manager
            .append_log(&record.job_id, "only two\nlines here\n")
            .await
            .expect("append log");

        let output = manager
            .task_output(&record.job_id, false, 0)
            .await
            .expect("task output");
        let summary = output
            .task
            .expect("task")
            .progress_summary
            .expect("summary");
        assert_eq!(summary.last_lines, vec!["only two", "lines here"]);
    }

    #[tokio::test]
    async fn job_detail_running_has_progress_summary_and_log_tail() {
        let manager = manager("detail-running");
        let record = manager
            .create_job(test_job("detail running test"))
            .await
            .expect("create job");
        manager
            .mark_running(&record.job_id, Some(999))
            .await
            .expect("mark running");
        manager
            .append_log(
                &record.job_id,
                "line 1\nline 2\nline 3\nline 4\nline 5\nline 6\nline 7\nline 8\nline 9\nline 10\nline 11\nline 12\n",
            )
            .await
            .expect("append log");

        let detail = manager
            .job_detail(&record.job_id)
            .await
            .expect("job detail");
        assert_eq!(detail.status, BackgroundJobStatus::Running);
        assert_eq!(detail.provider, ProviderId::Anthropic);
        assert_eq!(detail.model, "claude-sonnet-4-20250514");
        assert_eq!(detail.log_tail.len(), 10);
        assert_eq!(detail.log_tail[0], "line 3");
        assert_eq!(detail.log_tail[9], "line 12");
        assert!(detail.progress_summary.is_some());
        let summary = detail.progress_summary.unwrap();
        assert!(summary.elapsed_ms >= 0);
        assert!(summary.output_bytes > 0);
        assert!(detail.elapsed_ms >= 0);
    }

    #[tokio::test]
    async fn job_detail_completed_no_progress_summary() {
        let manager = manager("detail-completed");
        let record = manager
            .create_job(test_job("detail completed test"))
            .await
            .expect("create job");
        manager
            .mark_running(&record.job_id, None)
            .await
            .expect("mark running");
        manager
            .append_log(&record.job_id, "done\n")
            .await
            .expect("append log");
        manager
            .mark_completed(&record.job_id)
            .await
            .expect("mark completed");

        let detail = manager
            .job_detail(&record.job_id)
            .await
            .expect("job detail");
        assert_eq!(detail.status, BackgroundJobStatus::Completed);
        assert!(detail.progress_summary.is_none());
        assert_eq!(detail.log_tail, vec!["done"]);
        assert!(detail.elapsed_ms >= 0);
        assert_eq!(detail.exit_code, Some(0));
    }
}
