use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use orbcode_protocol::{FileChangeSummary, ToolResultMetadata};
use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::{
    ToolContext, ToolError, ToolOutcome, ToolRegistry,
    fs_text::resolve_path,
    payload::{parse_payload, required_field_keys, string_field_any, usize_field_keys},
    permissions::require_tools,
};

/// Jupyter notebooks are written with a single-space indent (matches the
/// reference `IPYNB_INDENT = 1`) so diffs stay close to how Jupyter itself
/// rewrites the file.
const IPYNB_INDENT: &[u8] = b" ";

impl ToolRegistry {
    pub(crate) async fn notebook_edit(
        &self,
        input: &str,
        context: &ToolContext,
    ) -> Result<ToolOutcome, ToolError> {
        require_tools(context)?;
        let payload = parse_payload(input)?;

        let path = resolve_path(
            &context.cwd,
            &required_field_keys(
                &payload,
                &[
                    "notebook_path",
                    "notebookPath",
                    "path",
                    "file_path",
                    "filePath",
                ],
                input,
            )?,
        )?;

        if path.extension().and_then(|ext| ext.to_str()) != Some("ipynb") {
            return Err(ToolError::InvalidInput(
                "File must be a Jupyter notebook (.ipynb file). For editing other file types, use the FileEdit tool.".into(),
            ));
        }

        let new_source =
            string_field_any(&payload, &["new_source", "newSource", "source"]).unwrap_or_default();
        let mut cell_type = string_field_any(&payload, &["cell_type", "cellType"]);
        let cell_id = string_field_any(&payload, &["cell_id", "cellId"]);
        let cell_number = usize_field_keys(&payload, &["cell_number", "cellNumber"]);
        let edit_mode = string_field_any(&payload, &["edit_mode", "editMode"])
            .unwrap_or_else(|| "replace".to_string());

        if !matches!(
            edit_mode.as_str(),
            "replace" | "insert" | "delete" | "append"
        ) {
            return Err(ToolError::InvalidInput(
                "Edit mode must be replace, insert, or delete.".into(),
            ));
        }

        if edit_mode == "insert" && cell_type.is_none() {
            return Err(ToolError::InvalidInput(
                "Cell type is required when using edit_mode=insert.".into(),
            ));
        }
        if let Some(kind) = cell_type.as_deref()
            && !matches!(kind, "code" | "markdown")
        {
            return Err(ToolError::InvalidInput(
                "Cell type must be code or markdown.".into(),
            ));
        }

        if !tokio::fs::try_exists(&path).await? {
            return Err(ToolError::ExecutionFailed(
                "Notebook file does not exist.".into(),
            ));
        }

        let raw = tokio::fs::read_to_string(&path).await?;
        let mut notebook = serde_json::from_str::<Value>(&raw)
            .map_err(|_| ToolError::ExecutionFailed("Notebook is not valid JSON.".into()))?;

        let nbformat = notebook
            .get("nbformat")
            .and_then(Value::as_u64)
            .unwrap_or(4);
        let nbformat_minor = notebook
            .get("nbformat_minor")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let language = notebook
            .get("metadata")
            .and_then(|meta| meta.get("language_info"))
            .and_then(|info| info.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("python")
            .to_string();

        let cells_len = notebook
            .get("cells")
            .and_then(Value::as_array)
            .map(Vec::len)
            .ok_or_else(|| {
                ToolError::ExecutionFailed("Notebook does not contain a cells array.".into())
            })?;

        let has_addressing = cell_id.is_some() || cell_number.is_some();
        if !has_addressing && !matches!(edit_mode.as_str(), "insert" | "append") {
            return Err(ToolError::ExecutionFailed(
                "Cell ID must be specified when not inserting a new cell.".into(),
            ));
        }

        // Resolve the addressed cell into a base index following the reference
        // tool: cell_id matches the `id` field first, then the `cell-N` fallback;
        // cell_number is a direct 0-based index.
        let mut cell_index = if let Some(id) = cell_id.as_deref() {
            match find_cell_by_id(&notebook, id) {
                Some(index) => index,
                None => match parse_cell_id(id) {
                    Some(index) => {
                        if index >= cells_len {
                            return Err(ToolError::ExecutionFailed(format!(
                                "Cell with index {index} does not exist in notebook."
                            )));
                        }
                        index
                    }
                    None => {
                        return Err(ToolError::ExecutionFailed(format!(
                            "Cell with ID \"{id}\" not found in notebook."
                        )));
                    }
                },
            }
        } else if let Some(index) = cell_number {
            if index >= cells_len {
                return Err(ToolError::ExecutionFailed(format!(
                    "Cell with index {index} does not exist in notebook."
                )));
            }
            index
        } else {
            // insert/append with no addressing: insert defaults to the start.
            0
        };

        if edit_mode == "append" {
            cell_index = cells_len;
        } else if edit_mode == "insert" && has_addressing {
            cell_index += 1;
        }

        // Replacing one-past-the-end is an append in disguise; the reference
        // tool rewrites it to an insert and defaults the new cell to code.
        let mut effective_mode = edit_mode.clone();
        if effective_mode == "replace" && cell_index == cells_len {
            effective_mode = "insert".to_string();
            if cell_type.is_none() {
                cell_type = Some("code".to_string());
            }
        }

        let generates_ids = nbformat > 4 || (nbformat == 4 && nbformat_minor >= 5);
        let new_cell_id = if generates_ids && matches!(effective_mode.as_str(), "insert" | "append")
        {
            Some(generate_cell_id())
        } else {
            None
        };

        let cells = notebook
            .get_mut("cells")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| {
                ToolError::ExecutionFailed("Notebook does not contain a cells array.".into())
            })?;

        let result_cell_type;
        let result_cell_id;
        match effective_mode.as_str() {
            "delete" => {
                cells.remove(cell_index);
                result_cell_type = cell_type.clone().unwrap_or_else(|| "code".to_string());
                result_cell_id = cell_id.clone();
            }
            "insert" | "append" => {
                let kind = cell_type.clone().unwrap_or_else(|| "code".to_string());
                let cell = build_new_cell(&kind, &new_source, new_cell_id.as_deref());
                cells.insert(cell_index, cell);
                result_cell_type = kind;
                result_cell_id = new_cell_id.clone();
            }
            _ => {
                let target = cells.get_mut(cell_index).ok_or_else(|| {
                    ToolError::ExecutionFailed(format!(
                        "Cell with index {cell_index} does not exist in notebook."
                    ))
                })?;
                let current_type = target
                    .get("cell_type")
                    .and_then(Value::as_str)
                    .unwrap_or("code")
                    .to_string();
                if let Some(object) = target.as_object_mut() {
                    object.insert("source".to_string(), Value::String(new_source.clone()));
                    if current_type == "code" {
                        object.insert("execution_count".to_string(), Value::Null);
                        object.insert("outputs".to_string(), Value::Array(Vec::new()));
                    }
                    if let Some(kind) = cell_type.as_deref()
                        && kind != current_type
                    {
                        object.insert("cell_type".to_string(), Value::String(kind.to_string()));
                    }
                }
                result_cell_type = cell_type.clone().unwrap_or(current_type);
                result_cell_id = cell_id.clone();
            }
        }

        let serialized = to_ipynb_string(&notebook)?;
        tokio::fs::write(&path, serialized).await?;

        let path_display = path.display().to_string();
        let output = match effective_mode.as_str() {
            "delete" => format!(
                "Deleted cell {}",
                result_cell_id
                    .clone()
                    .unwrap_or_else(|| cell_index.to_string())
            ),
            "insert" | "append" => format!(
                "Inserted cell {} with {new_source}",
                result_cell_id
                    .clone()
                    .unwrap_or_else(|| cell_index.to_string())
            ),
            _ => format!(
                "Updated cell {} with {new_source}",
                result_cell_id
                    .clone()
                    .unwrap_or_else(|| cell_index.to_string())
            ),
        };

        let metadata = ToolResultMetadata {
            file_changes: Some(FileChangeSummary {
                paths: vec![path_display.clone()],
                operation: Some(effective_mode.clone()),
                git: None,
            }),
            ..Default::default()
        };
        let mut metadata_value = metadata.to_value();
        if let Some(object) = metadata_value.as_object_mut() {
            object.insert(
                "notebook".to_string(),
                json!({
                    "editMode": effective_mode,
                    "cellType": result_cell_type,
                    "language": language,
                    "cellId": result_cell_id,
                }),
            );
        }

        Ok(ToolOutcome {
            name: "notebook-edit".to_string(),
            summary: format!("Edited notebook {path_display}."),
            output,
            metadata: Some(metadata_value),
            changed_paths: vec![path],
        })
    }
}

fn find_cell_by_id(notebook: &Value, id: &str) -> Option<usize> {
    notebook
        .get("cells")?
        .as_array()?
        .iter()
        .position(|cell| cell.get("id").and_then(Value::as_str) == Some(id))
}

/// Parse the synthetic `cell-N` identifier into its 0-based index.
fn parse_cell_id(cell_id: &str) -> Option<usize> {
    cell_id.strip_prefix("cell-")?.parse::<usize>().ok()
}

fn build_new_cell(kind: &str, source: &str, id: Option<&str>) -> Value {
    let mut cell = Map::new();
    cell.insert("cell_type".to_string(), Value::String(kind.to_string()));
    if let Some(id) = id {
        cell.insert("id".to_string(), Value::String(id.to_string()));
    }
    cell.insert("source".to_string(), Value::String(source.to_string()));
    cell.insert("metadata".to_string(), Value::Object(Map::new()));
    if kind == "code" {
        cell.insert("execution_count".to_string(), Value::Null);
        cell.insert("outputs".to_string(), Value::Array(Vec::new()));
    }
    Value::Object(cell)
}

fn to_ipynb_string(value: &Value) -> Result<String, ToolError> {
    let mut buffer = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(IPYNB_INDENT);
    let mut serializer = serde_json::Serializer::with_formatter(&mut buffer, formatter);
    value.serialize(&mut serializer).map_err(ToolError::Json)?;
    String::from_utf8(buffer).map_err(|error| ToolError::ExecutionFailed(error.to_string()))
}

/// Generate a notebook cell id roughly matching the reference base36 form
/// (`Math.random().toString(36).substring(2, 15)`).
fn generate_cell_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos() as u64);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut value = nanos ^ counter.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let mut id = String::with_capacity(13);
    const ALPHABET: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    for _ in 0..13 {
        id.push(ALPHABET[(value % 36) as usize] as char);
        value /= 36;
        if value == 0 {
            value = nanos.rotate_left(7).wrapping_add(0x1234_5678);
        }
    }
    id
}
