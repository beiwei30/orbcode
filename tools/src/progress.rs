use std::sync::Arc;

use serde::Serialize;
use serde_json::Value;
use tokio::io::AsyncReadExt;

use crate::local_shell_task::LocalShellTaskRegistry;
use crate::{ToolContext, ToolError, ToolProgressReporter};

const BASH_PROGRESS_REPORT_INTERVAL_BYTES: usize = 16 * 1024;

pub(crate) async fn emit_tool_progress(
    context: &ToolContext,
    progress: Value,
) -> Result<(), ToolError> {
    if let Some(reporter) = &context.progress {
        reporter.report(progress).await?;
    }
    Ok(())
}

#[derive(Serialize)]
struct BashProgressData {
    #[serde(rename = "type")]
    kind: &'static str,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    bytes: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "exitCode")]
    exit_code: Option<i32>,
}

#[derive(Serialize)]
struct BashProgressEnvelope {
    data: BashProgressData,
}

pub(crate) fn bash_progress_payload(
    status: &str,
    stream: Option<&str>,
    bytes: Option<usize>,
    exit_code: Option<i32>,
) -> Value {
    serde_json::to_value(BashProgressEnvelope {
        data: BashProgressData {
            kind: "bash_progress",
            status: status.to_string(),
            stream: stream.map(ToString::to_string),
            bytes,
            exit_code,
        },
    })
    .unwrap_or_default()
}

/// Read a child stream to EOF while mirroring each chunk into the durable
/// local-shell registry (`append_stdout` / `append_stderr`) and emitting byte
/// progress. Returns the full captured bytes so the caller can still build the
/// transcript-facing result. Registry appends serialise per `task_id` inside
/// the registry, so the stdout and stderr readers can run concurrently.
pub(crate) async fn read_child_stream<R>(
    mut reader: R,
    registry: LocalShellTaskRegistry,
    task_id: String,
    is_stdout: bool,
    progress: Option<Arc<dyn ToolProgressReporter>>,
    stream: &'static str,
) -> Result<Vec<u8>, ToolError>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut output = Vec::new();
    let mut buffer = [0_u8; 4096];
    let mut total_bytes = 0;
    let mut next_report_bytes = 1;

    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        let chunk = &buffer[..read];
        output.extend_from_slice(chunk);
        if is_stdout {
            registry.append_stdout(&task_id, chunk).await?;
        } else {
            registry.append_stderr(&task_id, chunk).await?;
        }
        total_bytes += read;

        if total_bytes >= next_report_bytes {
            if let Some(reporter) = &progress {
                reporter
                    .report(bash_progress_payload(
                        &format!("Streaming {stream}"),
                        Some(stream),
                        Some(total_bytes),
                        None,
                    ))
                    .await?;
            }
            next_report_bytes = total_bytes.saturating_add(BASH_PROGRESS_REPORT_INTERVAL_BYTES);
        }
    }

    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn bash_progress_payload_full() {
        let payload = bash_progress_payload("completed", Some("stdout"), Some(4096), Some(0));
        assert_eq!(
            payload,
            json!({
                "data": {
                    "type": "bash_progress",
                    "status": "completed",
                    "stream": "stdout",
                    "bytes": 4096,
                    "exitCode": 0,
                }
            })
        );
    }

    #[test]
    fn bash_progress_payload_minimal() {
        let payload = bash_progress_payload("running", None, None, None);
        assert_eq!(
            payload,
            json!({
                "data": {
                    "type": "bash_progress",
                    "status": "running",
                }
            })
        );
    }
}
