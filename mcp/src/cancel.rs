use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Debug, Default)]
pub struct McpCancellationToken {
    flag: Option<Arc<AtomicBool>>,
}

impl McpCancellationToken {
    pub fn from_flag(flag: Arc<AtomicBool>) -> Self {
        Self { flag: Some(flag) }
    }

    pub fn is_cancelled(&self) -> bool {
        self.flag
            .as_ref()
            .is_some_and(|f| f.load(Ordering::Relaxed))
    }

    pub async fn cancelled(&self) {
        let Some(flag) = &self.flag else {
            std::future::pending::<()>().await;
            return;
        };
        loop {
            if flag.load(Ordering::Relaxed) {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }
}
