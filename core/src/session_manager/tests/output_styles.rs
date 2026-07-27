use orbcode_config::OutputStyleSource;
use orbcode_protocol::TurnContext;

use super::support::test_manager;

#[tokio::test]
async fn refresh_output_styles_picks_up_project_style_and_resolves_active() {
    let manager = test_manager().await;

    // Default style after init: built-in `default`, no injection.
    assert_eq!(manager.active_output_style().definition.name, "default");
    assert!(
        manager
            .active_output_style()
            .system_prompt_section()
            .is_none()
    );

    let cwd = manager.config().cwd.clone();
    let project_styles = cwd.join(".claude").join("output-styles");
    tokio::fs::create_dir_all(&project_styles)
        .await
        .expect("create project output-styles dir");
    tokio::fs::write(
        project_styles.join("Verbose.md"),
        concat!(
            "---\n",
            "name: Verbose\n",
            "description: Speak in full paragraphs\n",
            "---\n",
            "Always reason out loud before producing code.\n",
        ),
    )
    .await
    .expect("write project style fixture");
    let local_settings_dir = cwd.join(".claude");
    tokio::fs::create_dir_all(&local_settings_dir)
        .await
        .unwrap();
    tokio::fs::write(
        local_settings_dir.join("settings.local.json"),
        r#"{"outputStyle":"Verbose"}"#,
    )
    .await
    .expect("write local settings");

    manager.refresh_output_styles().await;

    let active = manager.active_output_style();
    assert!(active.matched);
    assert_eq!(active.definition.name, "Verbose");
    assert_eq!(active.definition.source, OutputStyleSource::ProjectSettings);
    let section = active
        .system_prompt_section()
        .expect("section present for non-default style");
    assert!(section.contains("Always reason out loud"));

    let context = TurnContext {
        cwd: cwd.display().to_string(),
        current_date: "2026-05-28".to_string(),
        ..Default::default()
    };
    let request = manager
        .provider_request_for_messages("new-session", "", context, Vec::new(), true, true)
        .await;
    assert!(request.system_prompt.contains("## Output style: Verbose"));
    assert!(
        request
            .system_prompt
            .contains("Always reason out loud before producing code")
    );
}

#[tokio::test]
async fn append_system_prompt_is_concatenated_into_request() {
    let mut manager = test_manager().await;
    manager.config.append_system_prompt = Some("Follow the house style guide.".to_string());
    let cwd = manager.config.cwd.clone();
    let context = TurnContext {
        cwd: cwd.display().to_string(),
        current_date: "2026-05-28".to_string(),
        ..Default::default()
    };
    let request = manager
        .provider_request_for_messages("append-sys", "", context, Vec::new(), true, true)
        .await;
    assert!(
        request
            .system_prompt
            .contains("Follow the house style guide."),
        "--append-system-prompt must reach the request system prompt"
    );
}

#[tokio::test]
async fn unknown_active_style_falls_back_to_default_with_no_injection() {
    let manager = test_manager().await;
    let cwd = manager.config().cwd.clone();
    let local_settings_dir = cwd.join(".claude");
    tokio::fs::create_dir_all(&local_settings_dir)
        .await
        .unwrap();
    tokio::fs::write(
        local_settings_dir.join("settings.local.json"),
        r#"{"outputStyle":"NoSuchStyle"}"#,
    )
    .await
    .expect("write local settings");

    manager.refresh_output_styles().await;

    let active = manager.active_output_style();
    assert!(!active.matched);
    assert!(active.definition.is_default());

    let context = TurnContext {
        cwd: cwd.display().to_string(),
        current_date: "2026-05-28".to_string(),
        ..Default::default()
    };
    let request = manager
        .provider_request_for_messages("new-session", "", context, Vec::new(), true, true)
        .await;
    assert!(!request.system_prompt.contains("## Output style:"));
}
