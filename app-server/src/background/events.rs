use std::path::PathBuf;

use orbcode_core::CoreError;

use super::BackgroundManager;

impl BackgroundManager {
    pub async fn append_log(&self, job_id: &str, chunk: &str) -> Result<(), CoreError> {
        let record = self.read_record(job_id).await?;
        let path = PathBuf::from(record.log_path);
        use tokio::io::AsyncWriteExt;
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await?;
        file.write_all(chunk.as_bytes()).await?;
        Ok(())
    }

    pub async fn read_log(&self, job_id: &str) -> Result<String, CoreError> {
        let record = self.read_record(job_id).await?;
        let path = PathBuf::from(record.log_path);
        if !tokio::fs::try_exists(&path).await? {
            return Ok(String::new());
        }
        Ok(tokio::fs::read_to_string(path).await?)
    }

    pub fn events_path(&self, job_id: &str) -> PathBuf {
        self.logs_dir.join(format!("{job_id}.events.jsonl"))
    }

    pub async fn read_events(&self, job_id: &str) -> Result<String, CoreError> {
        let path = self.events_path(job_id);
        if !tokio::fs::try_exists(&path).await? {
            return Ok(String::new());
        }
        Ok(tokio::fs::read_to_string(path).await?)
    }

    pub async fn append_event_line(
        &self,
        job_id: &str,
        value: &serde_json::Value,
    ) -> Result<(), CoreError> {
        use tokio::io::AsyncWriteExt;
        let path = self.events_path(job_id);
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await?;
        let mut bytes = serde_json::to_vec(value)?;
        bytes.push(b'\n');
        file.write_all(&bytes).await?;
        Ok(())
    }
}
