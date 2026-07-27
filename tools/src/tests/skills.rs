use super::*;

#[tokio::test]
async fn skill_tool_loads_project_skill_prompts() {
    let registry = ToolRegistry::foundation();
    let context = test_context("skill").await;
    let skill_dir = context.cwd.join(".claude/skills/demo");
    std::fs::create_dir_all(&skill_dir).expect("create skill dir");
    std::fs::write(
        skill_dir.join("SKILL.md"),
        concat!(
            "---\n",
            "name: demo\n",
            "description: \"Demo skill\"\n",
            "---\n\n",
            "Use these arguments: $ARGUMENTS\n"
        ),
    )
    .expect("write skill");

    let result = registry
        .invoke("Skill", r#"{"skill":"demo","args":"alpha beta"}"#, &context)
        .await
        .expect("invoke skill");
    assert!(result.output.contains("Skill: demo"));
    assert!(result.output.contains("Demo skill"));
    assert!(result.output.contains("Use these arguments: alpha beta"));
}

#[tokio::test]
async fn lsp_tool_finds_definitions_and_references() {
    let registry = ToolRegistry::foundation();
    let context = test_context("lsp").await;
    let source_dir = context.cwd.join("src");
    std::fs::create_dir_all(&source_dir).expect("create source dir");
    let file_path = source_dir.join("lib.rs");
    std::fs::write(
        &file_path,
        concat!(
            "fn helper() {}\n\n",
            "fn caller() {\n",
            "    helper();\n",
            "}\n"
        ),
    )
    .expect("write lsp fixture");

    let definition = registry
        .invoke(
            "LSP",
            &json!({
                "operation": "goToDefinition",
                "filePath": file_path.display().to_string(),
                "line": 4,
                "character": 6
            })
            .to_string(),
            &context,
        )
        .await
        .expect("invoke goToDefinition");
    let definition_payload: Value =
        serde_json::from_str(&definition.output).expect("parse definition");
    assert_eq!(definition_payload["symbol"], "helper");
    assert!(
        definition_payload["result"]
            .as_str()
            .expect("definition result")
            .contains("fn helper()")
    );

    let references = registry
        .invoke(
            "lsp",
            &json!({
                "operation": "findReferences",
                "filePath": file_path.display().to_string(),
                "line": 4,
                "character": 6
            })
            .to_string(),
            &context,
        )
        .await
        .expect("invoke findReferences");
    let references_payload: Value =
        serde_json::from_str(&references.output).expect("parse references");
    assert!(
        references_payload["result"]
            .as_str()
            .expect("references result")
            .contains("helper();")
    );
}
