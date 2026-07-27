use serde_json::{Value, json};

use crate::{ToolError, ToolOutcome};

pub fn tool_result_metadata(outcome: &ToolOutcome) -> String {
    let mut metadata = json!({
        "status": "completed",
        "toolName": outcome.name,
        "summary": outcome.summary,
        "changedPaths": outcome
            .changed_paths
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>(),
        "content": [
            {
                "type": "text",
                "text": outcome.output,
            }
        ],
    });
    merge_tool_extra_metadata(&mut metadata, outcome.metadata.clone());
    metadata.to_string()
}

pub fn tool_error_result_metadata(tool_name: &str, error: &ToolError) -> Option<String> {
    let mut metadata = error.metadata()?;
    if let Some(object) = metadata.as_object_mut() {
        object.entry("status".to_string()).or_insert_with(|| {
            json!(if error.is_interrupted() {
                "interrupted"
            } else {
                "failed"
            })
        });
        object
            .entry("toolName".to_string())
            .or_insert_with(|| json!(tool_name));
    }
    Some(metadata.to_string())
}

fn merge_tool_extra_metadata(metadata: &mut Value, extra: Option<Value>) {
    let Some(extra) = extra else {
        return;
    };
    let Some(metadata_object) = metadata.as_object_mut() else {
        return;
    };
    let Some(extra_object) = extra.as_object() else {
        metadata_object.insert("extra".to_string(), extra);
        return;
    };
    for (key, value) in extra_object {
        metadata_object.insert(key.clone(), value.clone());
    }
}

pub fn post_tool_response(outcome: &ToolOutcome) -> Value {
    let mut response = json!({
        "success": true,
        "toolName": outcome.name,
        "summary": outcome.summary,
        "output": outcome.output,
        "changedPaths": outcome
            .changed_paths
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>(),
    });
    merge_tool_extra_metadata(&mut response, outcome.metadata.clone());
    response
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn tool_result_metadata_merges_extra_metadata() {
        let metadata = tool_result_metadata(&ToolOutcome {
            name: "bash".to_string(),
            summary: "ran command".to_string(),
            output: "ok".to_string(),
            metadata: Some(json!({"bash": {"exitCode": 0}})),
            changed_paths: vec![PathBuf::from("src/main.rs")],
        });
        let parsed: Value = serde_json::from_str(&metadata).expect("metadata should parse");

        assert_eq!(parsed["status"], json!("completed"));
        assert_eq!(parsed["toolName"], json!("bash"));
        assert_eq!(parsed["summary"], json!("ran command"));
        assert_eq!(parsed["changedPaths"], json!(["src/main.rs"]));
        assert_eq!(parsed["content"][0]["text"], json!("ok"));
        assert_eq!(parsed["bash"]["exitCode"], json!(0));
    }

    #[test]
    fn tool_result_metadata_preserves_empty_output_content() {
        let metadata = tool_result_metadata(&ToolOutcome {
            name: "file-read".to_string(),
            summary: "Read /tmp/empty.txt.".to_string(),
            output: String::new(),
            metadata: None,
            changed_paths: vec![PathBuf::from("/tmp/empty.txt")],
        });
        let parsed: Value = serde_json::from_str(&metadata).expect("metadata should parse");

        assert_eq!(parsed["content"][0]["text"], json!(""));
    }

    #[test]
    fn tool_error_result_metadata_adds_status_and_tool_name() {
        let metadata = tool_error_result_metadata(
            "bash",
            &ToolError::ExecutionFailedWithMetadata {
                message: "exit 1".to_string(),
                metadata: json!({"exitCode": 1}),
            },
        )
        .expect("metadata should exist");
        let parsed: Value = serde_json::from_str(&metadata).expect("metadata should parse");

        assert_eq!(parsed["status"], json!("failed"));
        assert_eq!(parsed["toolName"], json!("bash"));
        assert_eq!(parsed["exitCode"], json!(1));
    }

    #[test]
    fn post_tool_response_merges_extra_metadata() {
        let response = post_tool_response(&ToolOutcome {
            name: "write".to_string(),
            summary: "updated file".to_string(),
            output: "done".to_string(),
            metadata: Some(json!({"artifact": "file"})),
            changed_paths: vec![PathBuf::from("README.md")],
        });

        assert_eq!(response["success"], json!(true));
        assert_eq!(response["toolName"], json!("write"));
        assert_eq!(response["output"], json!("done"));
        assert_eq!(response["changedPaths"], json!(["README.md"]));
        assert_eq!(response["artifact"], json!("file"));
    }
}
