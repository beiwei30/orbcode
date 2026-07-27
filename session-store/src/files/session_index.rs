use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use orbcode_protocol::SessionSummary;
use serde::{Deserialize, Serialize};

const INDEX_VERSION: u32 = 1;
const INDEX_FILENAME: &str = "_session_index.json";

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct SessionIndex {
    version: u32,
    entries: HashMap<String, IndexEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct IndexEntry {
    mtime_secs: i64,
    byte_size: u64,
    summary: SessionSummary,
}

impl SessionIndex {
    fn new() -> Self {
        Self {
            version: INDEX_VERSION,
            entries: HashMap::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn new_for_test() -> Self {
        Self::new()
    }

    pub(crate) fn get(&self, session_id: &str) -> Option<&SessionSummary> {
        self.entries.get(session_id).map(|e| &e.summary)
    }

    pub(crate) fn is_fresh(&self, session_id: &str, mtime: SystemTime, byte_size: u64) -> bool {
        let Some(entry) = self.entries.get(session_id) else {
            return false;
        };
        entry.byte_size == byte_size && entry.mtime_secs == system_time_to_epoch_secs(mtime)
    }

    pub(crate) fn upsert(
        &mut self,
        session_id: String,
        summary: SessionSummary,
        mtime: SystemTime,
        byte_size: u64,
    ) {
        self.entries.insert(
            session_id,
            IndexEntry {
                mtime_secs: system_time_to_epoch_secs(mtime),
                byte_size,
                summary,
            },
        );
    }

    pub(crate) fn retain_session_ids(&mut self, valid_ids: &[&str]) {
        self.entries
            .retain(|id, _| valid_ids.contains(&id.as_str()));
    }

    pub(crate) fn entries_len(&self) -> usize {
        self.entries.len()
    }
}

pub(crate) fn index_path(project_dir: &Path) -> PathBuf {
    project_dir.join(INDEX_FILENAME)
}

pub(crate) async fn load_index(project_dir: &Path) -> SessionIndex {
    let path = index_path(project_dir);
    let Ok(contents) = tokio::fs::read_to_string(&path).await else {
        return SessionIndex::new();
    };
    match serde_json::from_str::<SessionIndex>(&contents) {
        Ok(index) if index.version == INDEX_VERSION => index,
        _ => SessionIndex::new(),
    }
}

pub(crate) async fn save_index(project_dir: &Path, index: &SessionIndex) {
    let path = index_path(project_dir);
    let Ok(json) = serde_json::to_string(index) else {
        return;
    };
    let _ = tokio::fs::write(&path, json.as_bytes()).await;
}

fn system_time_to_epoch_secs(time: SystemTime) -> i64 {
    time.duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbcode_protocol::SessionStatus;

    fn test_summary(id: &str) -> SessionSummary {
        SessionSummary {
            session_id: id.into(),
            title: Some("test".into()),
            custom_title: None,
            message_count: 1,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            cwd: None,
            git_branch: None,
            model: None,
            provider: None,
            transcript_path: None,
            status: SessionStatus::Available,
            total_input_tokens: 0,
            total_output_tokens: 0,
            duration_secs: None,
        }
    }

    #[test]
    fn fresh_check_matches_exact_metadata() {
        let mut index = SessionIndex::new();
        let mtime = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1000);
        index.upsert("s1".into(), test_summary("s1"), mtime, 500);

        assert!(index.is_fresh("s1", mtime, 500));
        assert!(!index.is_fresh("s1", mtime, 501));
        let later = mtime + std::time::Duration::from_secs(1);
        assert!(!index.is_fresh("s1", later, 500));
        assert!(!index.is_fresh("s2", mtime, 500));
    }

    #[test]
    fn retain_removes_stale_entries() {
        let mut index = SessionIndex::new();
        let mtime = SystemTime::UNIX_EPOCH;
        index.upsert("keep".into(), test_summary("keep"), mtime, 100);
        index.upsert("drop".into(), test_summary("drop"), mtime, 200);

        index.retain_session_ids(&["keep"]);
        assert!(index.get("keep").is_some());
        assert!(index.get("drop").is_none());
    }

    #[test]
    fn roundtrip_serialization() {
        let mut index = SessionIndex::new();
        let mtime = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(12345);
        index.upsert("s1".into(), test_summary("s1"), mtime, 999);

        let json = serde_json::to_string(&index).unwrap();
        let loaded: SessionIndex = serde_json::from_str(&json).unwrap();
        assert!(loaded.is_fresh("s1", mtime, 999));
        assert_eq!(loaded.get("s1").unwrap().session_id, "s1");
    }
}
