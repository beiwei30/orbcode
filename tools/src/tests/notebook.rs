use super::*;

const TYPICAL_IPYNB: &str = include_str!("../fixtures/typical.ipynb");
const IMAGE_IPYNB: &str = include_str!("../fixtures/with_image.ipynb");
const MARKDOWN_IPYNB: &str = include_str!("../fixtures/with_markdown.ipynb");

async fn seed(context: &ToolContext, name: &str, contents: &str) -> PathBuf {
    let path = context.cwd.join(name);
    tokio::fs::write(&path, contents)
        .await
        .expect("seed notebook");
    path
}

async fn read_back(path: &std::path::Path) -> Value {
    let raw = tokio::fs::read_to_string(path)
        .await
        .expect("read notebook");
    serde_json::from_str(&raw).expect("parse notebook")
}

fn cells(notebook: &Value) -> &Vec<Value> {
    notebook["cells"].as_array().expect("cells array")
}

// ----- typical.ipynb: 4 edit modes -----

#[tokio::test]
async fn typical_replace_resets_outputs_and_execution_count() {
    let registry = ToolRegistry::foundation();
    let context = test_context("nb-typical-replace").await;
    let path = seed(&context, "typical.ipynb", TYPICAL_IPYNB).await;
    registry
        .invoke(
            "notebook-edit",
            r#"{"notebook_path":"typical.ipynb","cell_id":"setup-cell","new_source":"print('hi')","edit_mode":"replace"}"#,
            &context,
        )
        .await
        .expect("replace cell");
    let notebook = read_back(&path).await;
    let cell = &cells(&notebook)[1];
    assert_eq!(cell["source"], json!("print('hi')"));
    assert_eq!(cell["execution_count"], Value::Null);
    assert_eq!(cell["outputs"], json!([]));
    assert_eq!(cell["cell_type"], json!("code"));
}

#[tokio::test]
async fn typical_insert_adds_cell_after_target_and_keeps_others() {
    let registry = ToolRegistry::foundation();
    let context = test_context("nb-typical-insert").await;
    let path = seed(&context, "typical.ipynb", TYPICAL_IPYNB).await;
    registry
        .invoke(
            "notebook-edit",
            r#"{"notebook_path":"typical.ipynb","cell_id":"intro-cell","new_source":"y = 2","cell_type":"code","edit_mode":"insert"}"#,
            &context,
        )
        .await
        .expect("insert cell");
    let notebook = read_back(&path).await;
    let cells = cells(&notebook);
    assert_eq!(cells.len(), 4);
    let inserted = &cells[1];
    assert_eq!(inserted["source"], json!("y = 2"));
    assert_eq!(inserted["cell_type"], json!("code"));
    assert!(inserted["id"].as_str().is_some_and(|id| !id.is_empty()));
    assert_eq!(cells[2]["execution_count"], json!(1));
    assert!(!cells[2]["outputs"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn typical_delete_removes_addressed_cell() {
    let registry = ToolRegistry::foundation();
    let context = test_context("nb-typical-delete").await;
    let path = seed(&context, "typical.ipynb", TYPICAL_IPYNB).await;
    registry
        .invoke(
            "notebook-edit",
            r#"{"notebook_path":"typical.ipynb","cell_id":"compute-cell","new_source":"","edit_mode":"delete"}"#,
            &context,
        )
        .await
        .expect("delete cell");
    let notebook = read_back(&path).await;
    let cells = cells(&notebook);
    assert_eq!(cells.len(), 2);
    assert!(
        cells
            .iter()
            .all(|cell| cell["id"].as_str() != Some("compute-cell"))
    );
}

#[tokio::test]
async fn typical_append_adds_cell_at_end() {
    let registry = ToolRegistry::foundation();
    let context = test_context("nb-typical-append").await;
    let path = seed(&context, "typical.ipynb", TYPICAL_IPYNB).await;
    registry
        .invoke(
            "notebook-edit",
            &json!({"notebook_path":"typical.ipynb","new_source":"## End","cell_type":"markdown","edit_mode":"append"}).to_string(),
            &context,
        )
        .await
        .expect("append cell");
    let notebook = read_back(&path).await;
    let cells = cells(&notebook);
    assert_eq!(cells.len(), 4);
    let last = cells.last().unwrap();
    assert_eq!(last["cell_type"], json!("markdown"));
    assert_eq!(last["source"], json!("## End"));
    assert!(!last.as_object().unwrap().contains_key("outputs"));
}

// ----- with_image.ipynb: 4 edit modes (image preservation) -----

fn image_data(cell: &Value) -> Option<&str> {
    cell["outputs"][0]["data"]["image/png"].as_str()
}

#[tokio::test]
async fn image_replace_clears_image_output() {
    let registry = ToolRegistry::foundation();
    let context = test_context("nb-image-replace").await;
    let path = seed(&context, "with_image.ipynb", IMAGE_IPYNB).await;
    registry
        .invoke(
            "notebook-edit",
            r#"{"notebook_path":"with_image.ipynb","cell_id":"plot-cell","new_source":"pass","edit_mode":"replace"}"#,
            &context,
        )
        .await
        .expect("replace image cell");
    let notebook = read_back(&path).await;
    let cell = &cells(&notebook)[1];
    assert_eq!(cell["source"], json!("pass"));
    assert_eq!(cell["outputs"], json!([]));
}

#[tokio::test]
async fn image_insert_preserves_existing_image_output() {
    let registry = ToolRegistry::foundation();
    let context = test_context("nb-image-insert").await;
    let path = seed(&context, "with_image.ipynb", IMAGE_IPYNB).await;
    registry
        .invoke(
            "notebook-edit",
            r#"{"notebook_path":"with_image.ipynb","cell_id":"plot-intro","new_source":"note","cell_type":"markdown","edit_mode":"insert"}"#,
            &context,
        )
        .await
        .expect("insert cell");
    let notebook = read_back(&path).await;
    let cells = cells(&notebook);
    assert_eq!(cells.len(), 3);
    assert!(image_data(&cells[2]).is_some_and(|data| data.starts_with("iVBOR")));
}

#[tokio::test]
async fn image_delete_keeps_remaining_image_cell() {
    let registry = ToolRegistry::foundation();
    let context = test_context("nb-image-delete").await;
    let path = seed(&context, "with_image.ipynb", IMAGE_IPYNB).await;
    registry
        .invoke(
            "notebook-edit",
            r#"{"notebook_path":"with_image.ipynb","cell_id":"plot-intro","new_source":"","edit_mode":"delete"}"#,
            &context,
        )
        .await
        .expect("delete cell");
    let notebook = read_back(&path).await;
    let cells = cells(&notebook);
    assert_eq!(cells.len(), 1);
    assert!(image_data(&cells[0]).is_some_and(|data| data.starts_with("iVBOR")));
}

#[tokio::test]
async fn image_append_keeps_image_cell_intact() {
    let registry = ToolRegistry::foundation();
    let context = test_context("nb-image-append").await;
    let path = seed(&context, "with_image.ipynb", IMAGE_IPYNB).await;
    registry
        .invoke(
            "notebook-edit",
            r#"{"notebook_path":"with_image.ipynb","new_source":"print('done')","cell_type":"code","edit_mode":"append"}"#,
            &context,
        )
        .await
        .expect("append cell");
    let notebook = read_back(&path).await;
    let cells = cells(&notebook);
    assert_eq!(cells.len(), 3);
    assert!(image_data(&cells[1]).is_some_and(|data| data.starts_with("iVBOR")));
    assert_eq!(cells[2]["source"], json!("print('done')"));
}

// ----- with_markdown.ipynb: 4 edit modes -----

#[tokio::test]
async fn markdown_replace_updates_prose_cell() {
    let registry = ToolRegistry::foundation();
    let context = test_context("nb-md-replace").await;
    let path = seed(&context, "with_markdown.ipynb", MARKDOWN_IPYNB).await;
    registry
        .invoke(
            "notebook-edit",
            &json!({"notebook_path":"with_markdown.ipynb","cell_id":"section-one","new_source":"## Renamed","edit_mode":"replace"}).to_string(),
            &context,
        )
        .await
        .expect("replace markdown cell");
    let notebook = read_back(&path).await;
    let cell = &cells(&notebook)[1];
    assert_eq!(cell["cell_type"], json!("markdown"));
    assert_eq!(cell["source"], json!("## Renamed"));
    assert!(!cell.as_object().unwrap().contains_key("outputs"));
}

#[tokio::test]
async fn markdown_insert_at_start_without_cell_id() {
    let registry = ToolRegistry::foundation();
    let context = test_context("nb-md-insert").await;
    let path = seed(&context, "with_markdown.ipynb", MARKDOWN_IPYNB).await;
    registry
        .invoke(
            "notebook-edit",
            r#"{"notebook_path":"with_markdown.ipynb","new_source":"setup = True","cell_type":"code","edit_mode":"insert"}"#,
            &context,
        )
        .await
        .expect("insert at start");
    let notebook = read_back(&path).await;
    let cells = cells(&notebook);
    assert_eq!(cells.len(), 5);
    assert_eq!(cells[0]["cell_type"], json!("code"));
    assert_eq!(cells[0]["source"], json!("setup = True"));
    assert_eq!(cells[1]["id"], json!("title"));
}

#[tokio::test]
async fn markdown_delete_by_cell_number() {
    let registry = ToolRegistry::foundation();
    let context = test_context("nb-md-delete").await;
    let path = seed(&context, "with_markdown.ipynb", MARKDOWN_IPYNB).await;
    registry
        .invoke(
            "notebook-edit",
            r#"{"notebook_path":"with_markdown.ipynb","cell_number":2,"new_source":"","edit_mode":"delete"}"#,
            &context,
        )
        .await
        .expect("delete by cell_number");
    let notebook = read_back(&path).await;
    let cells = cells(&notebook);
    assert_eq!(cells.len(), 3);
    assert!(
        cells
            .iter()
            .all(|cell| cell["id"].as_str() != Some("lonely-code"))
    );
}

#[tokio::test]
async fn markdown_append_code_cell_at_end() {
    let registry = ToolRegistry::foundation();
    let context = test_context("nb-md-append").await;
    let path = seed(&context, "with_markdown.ipynb", MARKDOWN_IPYNB).await;
    registry
        .invoke(
            "notebook-edit",
            r#"{"notebook_path":"with_markdown.ipynb","new_source":"z = 9","cell_type":"code","edit_mode":"append"}"#,
            &context,
        )
        .await
        .expect("append code cell");
    let notebook = read_back(&path).await;
    let cells = cells(&notebook);
    assert_eq!(cells.len(), 5);
    let last = cells.last().unwrap();
    assert_eq!(last["cell_type"], json!("code"));
    assert_eq!(last["execution_count"], Value::Null);
    assert_eq!(last["outputs"], json!([]));
}

// ----- serialization parity + metadata -----

#[tokio::test]
async fn serializes_with_single_space_indent_and_ts_field_order() {
    let registry = ToolRegistry::foundation();
    let context = test_context("nb-serialize").await;
    let path = seed(&context, "typical.ipynb", TYPICAL_IPYNB).await;
    registry
        .invoke(
            "notebook-edit",
            r#"{"notebook_path":"typical.ipynb","new_source":"q = 1","cell_type":"code","edit_mode":"append"}"#,
            &context,
        )
        .await
        .expect("append");
    let raw = tokio::fs::read_to_string(&path).await.expect("read raw");
    assert!(raw.contains("\n \"cells\""));
    assert!(!raw.contains("\n  \"cells\""));
    let cell_type_at = raw.find("\"cell_type\": \"code\"").expect("code cell");
    let tail = &raw[cell_type_at..];
    let order: Vec<&str> = ["id", "source", "metadata", "execution_count", "outputs"]
        .into_iter()
        .filter_map(|key| tail.find(&format!("\"{key}\"")).map(|pos| (pos, key)))
        .map(|(_, key)| key)
        .collect();
    let last_block = raw.rsplit("\"cell_type\": \"code\"").next().unwrap();
    let id_pos = last_block.find("\"id\"");
    let source_pos = last_block.find("\"source\"");
    let exec_pos = last_block.find("\"execution_count\"");
    assert!(id_pos < source_pos);
    assert!(source_pos < exec_pos);
    assert!(!order.is_empty());
}

#[tokio::test]
async fn populates_unified_file_change_metadata() {
    let registry = ToolRegistry::foundation();
    let context = test_context("nb-metadata").await;
    seed(&context, "typical.ipynb", TYPICAL_IPYNB).await;
    let outcome = registry
        .invoke(
            "notebook-edit",
            r#"{"notebook_path":"typical.ipynb","cell_id":"setup-cell","new_source":"print('hi')","edit_mode":"replace"}"#,
            &context,
        )
        .await
        .expect("replace");
    let metadata = outcome.metadata.expect("metadata present");
    let parsed: orbcode_protocol::ToolResultMetadata =
        serde_json::from_value(metadata.clone()).expect("parse unified metadata");
    let file_changes = parsed.file_changes.expect("file changes");
    assert_eq!(file_changes.operation.as_deref(), Some("replace"));
    assert_eq!(file_changes.paths.len(), 1);
    assert!(file_changes.paths[0].ends_with("typical.ipynb"));
    assert_eq!(metadata["notebook"]["editMode"], json!("replace"));
    assert_eq!(metadata["notebook"]["language"], json!("python"));
}

// ----- validation / error parity with the reference tool -----

#[tokio::test]
async fn rejects_non_ipynb_path() {
    let registry = ToolRegistry::foundation();
    let context = test_context("nb-err-ext").await;
    let error = registry
        .invoke(
            "notebook-edit",
            r#"{"notebook_path":"notes.txt","new_source":"x"}"#,
            &context,
        )
        .await
        .expect_err("non-ipynb rejected");
    assert_eq!(
        error.to_string(),
        "invalid tool input: File must be a Jupyter notebook (.ipynb file). For editing other file types, use the FileEdit tool."
    );
}

#[tokio::test]
async fn rejects_invalid_edit_mode() {
    let registry = ToolRegistry::foundation();
    let context = test_context("nb-err-mode").await;
    seed(&context, "typical.ipynb", TYPICAL_IPYNB).await;
    let error = registry
        .invoke(
            "notebook-edit",
            r#"{"notebook_path":"typical.ipynb","cell_id":"setup-cell","new_source":"x","edit_mode":"swap"}"#,
            &context,
        )
        .await
        .expect_err("invalid mode rejected");
    assert_eq!(
        error.to_string(),
        "invalid tool input: Edit mode must be replace, insert, or delete."
    );
}

#[tokio::test]
async fn rejects_insert_without_cell_type() {
    let registry = ToolRegistry::foundation();
    let context = test_context("nb-err-insert-type").await;
    seed(&context, "typical.ipynb", TYPICAL_IPYNB).await;
    let error = registry
        .invoke(
            "notebook-edit",
            r#"{"notebook_path":"typical.ipynb","new_source":"x","edit_mode":"insert"}"#,
            &context,
        )
        .await
        .expect_err("insert without cell_type rejected");
    assert_eq!(
        error.to_string(),
        "invalid tool input: Cell type is required when using edit_mode=insert."
    );
}

#[tokio::test]
async fn rejects_missing_notebook_file() {
    let registry = ToolRegistry::foundation();
    let context = test_context("nb-err-missing").await;
    let error = registry
        .invoke(
            "notebook-edit",
            r#"{"notebook_path":"ghost.ipynb","cell_id":"a","new_source":"x"}"#,
            &context,
        )
        .await
        .expect_err("missing file rejected");
    assert_eq!(
        error.to_string(),
        "tool execution failed: Notebook file does not exist."
    );
}

#[tokio::test]
async fn rejects_invalid_json_notebook() {
    let registry = ToolRegistry::foundation();
    let context = test_context("nb-err-json").await;
    seed(&context, "broken.ipynb", "{ not json").await;
    let error = registry
        .invoke(
            "notebook-edit",
            r#"{"notebook_path":"broken.ipynb","cell_id":"a","new_source":"x"}"#,
            &context,
        )
        .await
        .expect_err("invalid json rejected");
    assert_eq!(
        error.to_string(),
        "tool execution failed: Notebook is not valid JSON."
    );
}

#[tokio::test]
async fn rejects_replace_without_cell_id() {
    let registry = ToolRegistry::foundation();
    let context = test_context("nb-err-no-id").await;
    seed(&context, "typical.ipynb", TYPICAL_IPYNB).await;
    let error = registry
        .invoke(
            "notebook-edit",
            r#"{"notebook_path":"typical.ipynb","new_source":"x","edit_mode":"replace"}"#,
            &context,
        )
        .await
        .expect_err("replace without cell_id rejected");
    assert_eq!(
        error.to_string(),
        "tool execution failed: Cell ID must be specified when not inserting a new cell."
    );
}

#[tokio::test]
async fn rejects_unknown_cell_id() {
    let registry = ToolRegistry::foundation();
    let context = test_context("nb-err-unknown-id").await;
    seed(&context, "typical.ipynb", TYPICAL_IPYNB).await;
    let error = registry
        .invoke(
            "notebook-edit",
            r#"{"notebook_path":"typical.ipynb","cell_id":"nope","new_source":"x","edit_mode":"replace"}"#,
            &context,
        )
        .await
        .expect_err("unknown cell_id rejected");
    assert_eq!(
        error.to_string(),
        "tool execution failed: Cell with ID \"nope\" not found in notebook."
    );
}

#[tokio::test]
async fn rejects_cell_index_out_of_range() {
    let registry = ToolRegistry::foundation();
    let context = test_context("nb-err-range").await;
    seed(&context, "typical.ipynb", TYPICAL_IPYNB).await;
    let error = registry
        .invoke(
            "notebook-edit",
            r#"{"notebook_path":"typical.ipynb","cell_id":"cell-99","new_source":"x","edit_mode":"replace"}"#,
            &context,
        )
        .await
        .expect_err("out of range rejected");
    assert_eq!(
        error.to_string(),
        "tool execution failed: Cell with index 99 does not exist in notebook."
    );
}
