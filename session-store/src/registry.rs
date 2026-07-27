use std::path::PathBuf;

use chrono::Utc;

use crate::SessionStoreError;

#[derive(Debug, serde::Serialize, PartialEq, Eq)]
pub struct LiveSessionRegistryEntry {
    pid: u32,
    #[serde(rename = "sessionId")]
    session_id: String,
    cwd: String,
    #[serde(rename = "startedAt")]
    started_at: i64,
    kind: &'static str,
    entrypoint: &'static str,
}

impl LiveSessionRegistryEntry {
    pub fn new(
        pid: u32,
        session_id: impl Into<String>,
        cwd: impl Into<String>,
        started_at: i64,
        kind: &'static str,
    ) -> Self {
        Self {
            pid,
            session_id: session_id.into(),
            cwd: cwd.into(),
            started_at,
            kind,
            entrypoint: "orbcode",
        }
    }
}

#[derive(Clone)]
pub struct LiveSessionRegistryStore {
    sessions_dir: PathBuf,
    cwd: PathBuf,
}

impl LiveSessionRegistryStore {
    pub fn new(sessions_dir: PathBuf, cwd: PathBuf) -> Self {
        Self { sessions_dir, cwd }
    }

    pub async fn register(
        &self,
        session_id: &str,
        kind: &'static str,
    ) -> Result<(), SessionStoreError> {
        self.register_with_cwd(session_id, kind, self.cwd.clone())
            .await
    }

    pub async fn register_with_cwd(
        &self,
        session_id: &str,
        kind: &'static str,
        cwd: PathBuf,
    ) -> Result<(), SessionStoreError> {
        tokio::fs::create_dir_all(&self.sessions_dir).await?;
        let pid = std::process::id();
        let path = self.sessions_dir.join(format!("{pid}.json"));
        let entry = LiveSessionRegistryEntry::new(
            pid,
            session_id,
            cwd.display().to_string(),
            Utc::now().timestamp_millis(),
            kind,
        );
        tokio::fs::write(path, serde_json::to_string(&entry)?).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn live_session_registry_entry_serializes_claude_compatible_fields() {
        let entry =
            LiveSessionRegistryEntry::new(42, "session-1", "/tmp/project", 1_700, "interactive");

        assert_eq!(
            serde_json::to_value(entry).expect("serialize registry entry"),
            json!({
                "pid": 42,
                "sessionId": "session-1",
                "cwd": "/tmp/project",
                "startedAt": 1_700,
                "kind": "interactive",
                "entrypoint": "orbcode",
            })
        );
    }
}
