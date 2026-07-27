use std::path::Path;

use chrono::Utc;
use orbcode_core::CoreError;
use orbcode_protocol::BackgroundTaskView;
#[cfg(test)]
use orbcode_protocol::StreamEvent;
#[cfg(test)]
use orbcode_tools::subscribe_progress_stream;
use orbcode_tools::{
    BackgroundTaskKind, BackgroundTaskRecord, BackgroundTaskStatus, background_jobs_dir,
    write_background_task_record,
};
#[cfg(test)]
use tokio::sync::mpsc;

use orbcode_tools::task_record_to_view;

pub async fn reconcile_orphaned_agents(home_dir: &Path) -> Result<Vec<String>, CoreError> {
    let jobs_dir = background_jobs_dir(home_dir);
    if !tokio::fs::try_exists(&jobs_dir).await? {
        return Ok(Vec::new());
    }
    let mut entries = tokio::fs::read_dir(&jobs_dir).await?;
    let mut orphaned_ids = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Ok(contents) = tokio::fs::read_to_string(&path).await else {
            continue;
        };
        let Ok(mut record) = serde_json::from_str::<BackgroundTaskRecord>(&contents) else {
            continue;
        };
        if !matches!(
            record.task_kind,
            BackgroundTaskKind::LocalAgent | BackgroundTaskKind::Workflow
        ) {
            continue;
        }
        if record.status != BackgroundTaskStatus::Running {
            continue;
        }
        let now = Utc::now().to_rfc3339();
        record.status = BackgroundTaskStatus::Orphaned;
        record.updated_at = now.clone();
        record.finished_at = Some(now);
        record.error = Some("process restarted; agent was orphaned".to_string());
        if let Err(error) = write_background_task_record(home_dir, &record).await {
            eprintln!(
                "reconcile_orphaned_agents: failed to write {}: {error}",
                record.job_id
            );
            continue;
        }
        orphaned_ids.push(record.job_id);
    }
    Ok(orphaned_ids)
}

pub async fn list_local_task_views(home_dir: &Path) -> Result<Vec<BackgroundTaskView>, CoreError> {
    let jobs_dir = background_jobs_dir(home_dir);
    if !tokio::fs::try_exists(&jobs_dir).await? {
        return Ok(Vec::new());
    }
    let mut entries = tokio::fs::read_dir(&jobs_dir).await?;
    let mut views = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Ok(contents) = tokio::fs::read_to_string(&path).await else {
            continue;
        };
        let Ok(record) = serde_json::from_str::<BackgroundTaskRecord>(&contents) else {
            continue;
        };
        if !matches!(
            record.task_kind,
            BackgroundTaskKind::LocalAgent | BackgroundTaskKind::Workflow
        ) {
            continue;
        }
        views.push(task_record_to_view(&record));
    }
    views.sort_by_key(|v| std::cmp::Reverse(v.updated_at));
    Ok(views)
}

#[cfg(test)]
pub fn background_agent_progress_stream(
    task_id: &str,
) -> Option<mpsc::UnboundedReceiver<StreamEvent>> {
    let mut broadcast_rx = subscribe_progress_stream(task_id)?;
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        loop {
            match broadcast_rx.recv().await {
                Ok(event) => {
                    if tx.send(event).is_err() {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
    Some(rx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbcode_tools::{
        background_log_path, read_background_task_record, register_progress_stream,
    };
    use std::path::PathBuf;

    fn test_home(label: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        std::env::temp_dir().join(format!(
            "orbcode-bg-agent-{label}-{unique}-{}",
            std::process::id()
        ))
    }

    fn running_local_agent_record(home: &Path, job_id: &str) -> BackgroundTaskRecord {
        let log_path = background_log_path(home, job_id);
        BackgroundTaskRecord {
            job_id: job_id.to_string(),
            session_id: "session-1".to_string(),
            prompt: "test prompt".to_string(),
            cwd: "/tmp".to_string(),
            status: BackgroundTaskStatus::Running,
            created_at: Utc::now().to_rfc3339(),
            updated_at: Utc::now().to_rfc3339(),
            started_at: Some(Utc::now().to_rfc3339()),
            finished_at: None,
            pid: None,
            log_path: log_path.display().to_string(),
            error: None,
            task_kind: BackgroundTaskKind::LocalAgent,
            tool_use_id: Some("tu-1".to_string()),
            child_session_id: Some("session-1:agent-aaa".to_string()),
            agent_type: Some("general-purpose".to_string()),
            model: Some("claude-sonnet-4-20250514".to_string()),
            permission_mode: None,
            result: None,
            exit_code: None,
            signal: None,
            extra: serde_json::Map::new(),
        }
    }

    #[tokio::test]
    async fn reconcile_marks_running_local_agent_as_orphaned() {
        let home = test_home("orphan-basic");
        tokio::fs::create_dir_all(background_jobs_dir(&home))
            .await
            .expect("create jobs dir");
        let record = running_local_agent_record(&home, "agent-orphan-1");
        write_background_task_record(&home, &record)
            .await
            .expect("write record");

        let orphaned = reconcile_orphaned_agents(&home).await.expect("reconcile");
        assert_eq!(orphaned, vec!["agent-orphan-1"]);

        let loaded = read_background_task_record(&home, "agent-orphan-1")
            .await
            .expect("read")
            .expect("record exists");
        assert_eq!(loaded.status, BackgroundTaskStatus::Orphaned);
        assert!(loaded.finished_at.is_some());
        assert_eq!(
            loaded.error.as_deref(),
            Some("process restarted; agent was orphaned")
        );
    }

    #[tokio::test]
    async fn reconcile_ignores_non_local_agent_records() {
        let home = test_home("orphan-skip-kind");
        tokio::fs::create_dir_all(background_jobs_dir(&home))
            .await
            .expect("create jobs dir");
        let mut record = running_local_agent_record(&home, "bg-job-1");
        record.task_kind = BackgroundTaskKind::BackgroundJob;
        write_background_task_record(&home, &record)
            .await
            .expect("write record");

        let orphaned = reconcile_orphaned_agents(&home).await.expect("reconcile");
        assert!(orphaned.is_empty());

        let loaded = read_background_task_record(&home, "bg-job-1")
            .await
            .expect("read")
            .expect("record exists");
        assert_eq!(loaded.status, BackgroundTaskStatus::Running);
    }

    #[tokio::test]
    async fn reconcile_ignores_already_terminal_agents() {
        let home = test_home("orphan-skip-terminal");
        tokio::fs::create_dir_all(background_jobs_dir(&home))
            .await
            .expect("create jobs dir");
        let mut record = running_local_agent_record(&home, "agent-done-1");
        record.status = BackgroundTaskStatus::Completed;
        write_background_task_record(&home, &record)
            .await
            .expect("write record");

        let orphaned = reconcile_orphaned_agents(&home).await.expect("reconcile");
        assert!(orphaned.is_empty());
    }

    #[tokio::test]
    async fn progress_stream_returns_none_when_no_active_agent() {
        assert!(background_agent_progress_stream("nonexistent-task").is_none());
    }

    #[tokio::test]
    async fn progress_stream_receives_events_from_broadcast() {
        let task_id = format!(
            "agent-progress-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let tx = register_progress_stream(&task_id, 16);
        let mut rx = background_agent_progress_stream(&task_id).expect("subscribe");

        let event = StreamEvent::AssistantDelta {
            session_id: "s1".to_string(),
            delta: "hello".to_string(),
        };
        tx.send(event.clone()).expect("send");

        let received = rx.recv().await.expect("recv");
        assert_eq!(received, event);

        drop(tx);
        orbcode_tools::unregister_progress_stream(&task_id);
    }

    #[tokio::test]
    async fn concurrent_agents_progress_isolated() {
        let id_a = format!("agent-iso-a-{}", std::process::id());
        let id_b = format!("agent-iso-b-{}", std::process::id());
        let tx_a = register_progress_stream(&id_a, 16);
        let tx_b = register_progress_stream(&id_b, 16);
        let mut rx_a = background_agent_progress_stream(&id_a).expect("subscribe a");
        let mut rx_b = background_agent_progress_stream(&id_b).expect("subscribe b");

        let event_a = StreamEvent::AssistantDelta {
            session_id: "a".to_string(),
            delta: "from a".to_string(),
        };
        let event_b = StreamEvent::AssistantDelta {
            session_id: "b".to_string(),
            delta: "from b".to_string(),
        };
        tx_a.send(event_a.clone()).expect("send a");
        tx_b.send(event_b.clone()).expect("send b");

        let got_a = rx_a.recv().await.expect("recv a");
        let got_b = rx_b.recv().await.expect("recv b");
        assert_eq!(got_a, event_a);
        assert_eq!(got_b, event_b);

        drop(tx_a);
        drop(tx_b);
        orbcode_tools::unregister_progress_stream(&id_a);
        orbcode_tools::unregister_progress_stream(&id_b);
    }
}
