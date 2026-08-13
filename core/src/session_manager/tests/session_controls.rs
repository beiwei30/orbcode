use std::sync::{Arc, atomic::AtomicBool};

use orbcode_config::{AppConfigOverrides, PermissionMode};
use orbcode_protocol::{
    ApprovalPolicy, ApprovalReviewer, EffortLevel, MessageRole, ModelPermissionPreset, SandboxMode,
    TranscriptMessage, TurnContext,
};
use uuid::Uuid;

use super::support::*;
use super::*;

#[tokio::test]
async fn session_controls_are_isolated_and_drive_the_next_provider_request() {
    let manager = test_manager_with_overrides(AppConfigOverrides {
        allow_tools: Some(true),
        ..AppConfigOverrides::default()
    })
    .await;
    let (first, _) = manager.start_or_resume(None).await.expect("first session");
    let (second, _) = manager.start_or_resume(None).await.expect("second session");
    for session in [&first, &second] {
        manager
            .append_message(
                &session.session_id,
                TranscriptMessage::new(MessageRole::System, "session initialized"),
            )
            .await
            .expect("persist session");
    }

    manager
        .set_session_permission_mode(&first.session_id, PermissionMode::Plan)
        .await
        .expect("set first mode");
    manager
        .set_session_model(&first.session_id, Some("sonnet".to_string()))
        .await
        .expect("set first model");
    manager
        .set_session_effort(&first.session_id, Some(EffortLevel::High))
        .await
        .expect("set first effort");

    assert_eq!(
        manager
            .session_permission_mode(&second.session_id)
            .expect("second mode"),
        PermissionMode::Default
    );
    assert_ne!(
        manager
            .session_model_options(&second.session_id)
            .expect("second models")
            .iter()
            .find(|option| option.current)
            .and_then(|option| option.value.as_deref()),
        Some("sonnet")
    );
    assert_ne!(
        manager
            .session_effort_level(&second.session_id)
            .expect("second effort"),
        Some(EffortLevel::High)
    );

    let first_request = manager
        .provider_request_for_session(
            &first.session_id,
            "plan this",
            TurnContext::default(),
            &[],
            manager.session_exposes_tools(&first.session_id),
            true,
        )
        .await
        .expect("first provider request");
    let expected_model = manager
        .effective_config_for_session(&first.session_id)
        .provider_model_name(manager.config.default_provider);
    assert_eq!(first_request.model, expected_model);
    assert_eq!(first_request.effort, Some(EffortLevel::High));
    for restricted in ["Bash", "Write", "Edit", "Agent", "WebFetch"] {
        assert!(
            !first_request
                .tools
                .iter()
                .any(|tool| tool.name == restricted),
            "plan mode must not expose restricted tool {restricted}"
        );
    }

    let second_request = manager
        .provider_request_for_session(
            &second.session_id,
            "do this",
            TurnContext::default(),
            &[],
            manager.session_exposes_tools(&second.session_id),
            true,
        )
        .await
        .expect("second provider request");
    assert!(
        second_request.tools.iter().any(|tool| tool.name == "Bash"),
        "the other session must retain its own tool visibility"
    );
}

#[tokio::test]
async fn active_turn_rejects_session_control_changes_without_mutation() {
    let manager = test_manager().await;
    let (session, _) = manager.start_or_resume(None).await.expect("session");
    let turn_id = Uuid::new_v4();
    manager
        .active_turns
        .insert(
            &session.session_id,
            turn_id,
            Arc::new(AtomicBool::new(false)),
            crate::TurnInteractionContext::default(),
        )
        .await
        .expect("mark active");

    let error = manager
        .set_session_permission_mode(&session.session_id, PermissionMode::Plan)
        .await
        .expect_err("active session control must be rejected");
    assert!(matches!(error, CoreError::ActiveTurn(_)));
    assert_eq!(
        manager
            .session_permission_mode(&session.session_id)
            .expect("mode remains available"),
        PermissionMode::Default
    );

    manager
        .active_turns
        .clear_if_matching(&session.session_id, turn_id)
        .await;
}

#[tokio::test]
async fn permission_preset_is_isolated_between_sessions() {
    let manager = test_manager_with_overrides(AppConfigOverrides {
        provider_allow_network: Some(false),
        ..AppConfigOverrides::default()
    })
    .await;
    let (full_session, _) = manager.start_or_resume(None).await.expect("full session");
    let (ask_session, _) = manager.start_or_resume(None).await.expect("ask session");

    manager
        .set_session_permission_preset(&full_session.session_id, ModelPermissionPreset::FullAccess)
        .await
        .expect("set Full Access");
    let preset_permissions = manager.permission_context_for_session(&full_session.session_id);
    assert!(
        preset_permissions.allow_tools,
        "Ask for approval permits workspace-scoped tools"
    );
    assert!(
        !preset_permissions.provider_allow_network,
        "a session preset must not change provider transport permissions"
    );
    let ask_permissions = manager.permission_context_for_session(&ask_session.session_id);
    assert!(!ask_permissions.allow_network);

    manager
        .set_session_permission_mode(&ask_session.session_id, PermissionMode::Plan)
        .await
        .expect("set explicit session mode");
    let session_permissions = manager.permission_context_for_session(&ask_session.session_id);
    assert!(
        !session_permissions.allow_tools,
        "an explicit session mode must remain isolated from another session's Full Access"
    );
    assert!(
        !session_permissions.provider_allow_network,
        "an explicit session mode must retain its configured provider network permission"
    );
}

#[tokio::test]
async fn permission_presets_atomically_update_session_runtime_controls() {
    let manager = test_manager_with_overrides(AppConfigOverrides {
        provider_allow_network: Some(false),
        ..AppConfigOverrides::default()
    })
    .await;
    let (first, _) = manager.start_or_resume(None).await.expect("first session");
    let (second, _) = manager.start_or_resume(None).await.expect("second session");

    manager
        .set_session_permission_preset(&first.session_id, ModelPermissionPreset::ApproveForMe)
        .await
        .expect("set approve preset");
    let approve_config = manager.effective_config_for_session(&first.session_id);
    let approve_policy = manager
        .session_permission_policy(&first.session_id)
        .expect("approve policy");
    assert_eq!(approve_config.sandbox_mode, SandboxMode::WorkspaceWrite);
    assert!(!approve_config.sandbox_allow_network);
    assert!(approve_config.allow_tools);
    assert!(!approve_config.allow_network);
    assert!(!approve_config.provider_allow_network);
    assert_eq!(
        approve_policy.approval_policy,
        ApprovalPolicy::OnBoundaryRequest
    );
    assert_eq!(
        approve_policy.approval_reviewer,
        ApprovalReviewer::AutoReview
    );

    manager
        .set_session_permission_preset(&first.session_id, ModelPermissionPreset::FullAccess)
        .await
        .expect("set full access");
    let full_config = manager.effective_config_for_session(&first.session_id);
    assert_eq!(full_config.sandbox_mode, SandboxMode::DangerFullAccess);
    assert!(full_config.sandbox_allow_network);
    assert!(full_config.allow_network);
    assert!(!full_config.provider_allow_network);

    assert_eq!(
        manager
            .session_permission_preset(&second.session_id)
            .expect("second preset"),
        Some(ModelPermissionPreset::AskForApproval)
    );
    assert_eq!(
        manager
            .effective_config_for_session(&second.session_id)
            .sandbox_mode,
        SandboxMode::WorkspaceWrite
    );
}

#[tokio::test]
async fn implicit_ask_preserves_explicit_restrictive_permission_overrides() {
    let manager = test_manager_with_overrides(AppConfigOverrides {
        sandbox_mode: Some(SandboxMode::ReadOnly),
        allow_tools: Some(false),
        ..AppConfigOverrides::default()
    })
    .await;
    let (session, _) = manager.start_or_resume(None).await.expect("session");

    let config = manager.effective_config_for_session(&session.session_id);
    assert_eq!(config.sandbox_mode, SandboxMode::ReadOnly);
    assert!(!config.allow_tools);
    assert!(!manager.session_exposes_tools(&session.session_id));

    let next = manager
        .clear_session(&session.session_id)
        .await
        .expect("clear session");
    let cleared_config = manager.effective_config_for_session(&next.session_id);
    assert_eq!(cleared_config.sandbox_mode, SandboxMode::ReadOnly);
    assert!(!cleared_config.allow_tools);

    manager
        .set_session_permission_preset(&next.session_id, ModelPermissionPreset::AskForApproval)
        .await
        .expect("select Ask preset explicitly");
    let selected_config = manager.effective_config_for_session(&next.session_id);
    assert_eq!(selected_config.sandbox_mode, SandboxMode::WorkspaceWrite);
    assert!(selected_config.allow_tools);
}

#[tokio::test]
async fn managed_policy_disables_full_access_without_silent_downgrade() {
    let mut manager = test_manager().await;
    manager.config.policy.disable_bypass_permissions_mode = true;
    let (session, _) = manager.start_or_resume(None).await.expect("session");

    assert_eq!(
        manager.permission_preset_disabled_reason(ModelPermissionPreset::FullAccess),
        Some("Full Access is disabled by managed policy".to_string())
    );
    let error = manager
        .set_session_permission_preset(&session.session_id, ModelPermissionPreset::FullAccess)
        .await
        .expect_err("managed Full Access must be rejected");
    assert!(matches!(error, CoreError::PermissionDenied(_)));
    assert_eq!(
        manager
            .session_permission_preset(&session.session_id)
            .expect("preset remains available"),
        Some(ModelPermissionPreset::AskForApproval)
    );
}

#[tokio::test]
async fn explicit_session_model_overrides_global_model_control() {
    let manager = test_manager().await;
    let (session, _) = manager.start_or_resume(None).await.expect("session");

    manager.set_runtime_model_override(Some("haiku".to_string()));
    assert_eq!(
        manager
            .effective_config_for_session(&session.session_id)
            .provider_model_setting()
            .as_deref(),
        Some("haiku"),
        "an untouched session must honor the global SDK model control"
    );

    manager
        .set_session_model(&session.session_id, Some("sonnet".to_string()))
        .await
        .expect("set explicit session model");
    assert_eq!(
        manager
            .effective_config_for_session(&session.session_id)
            .provider_model_setting()
            .as_deref(),
        Some("sonnet"),
        "an explicit session model must remain isolated from global controls"
    );
}

#[tokio::test]
async fn clearing_session_model_override_resumes_layered_global_model() {
    let manager = test_manager().await;
    let (session, _) = manager.start_or_resume(None).await.expect("session");

    manager.set_runtime_model_override(Some("haiku".to_string()));
    manager
        .set_session_model(&session.session_id, Some("sonnet".to_string()))
        .await
        .expect("set session model");
    manager
        .clear_session_model_override(&session.session_id)
        .await
        .expect("clear session model");

    assert_eq!(
        manager
            .effective_config_for_session(&session.session_id)
            .provider_model_setting()
            .as_deref(),
        Some("haiku")
    );
}

#[tokio::test]
async fn invalid_session_model_does_not_mutate_existing_selection() {
    let manager = test_manager().await;
    let (session, _) = manager.start_or_resume(None).await.expect("session");
    let explicit_model = "claude-haiku-4-5-20251001";
    manager
        .set_session_model(&session.session_id, Some(explicit_model.to_string()))
        .await
        .expect("set valid explicit model id");

    let error = manager
        .set_session_model(&session.session_id, Some("bad\nmodel".to_string()))
        .await
        .expect_err("invalid model must fail");
    assert!(error.to_string().contains("control characters"));
    assert_eq!(
        manager
            .effective_config_for_session(&session.session_id)
            .provider_model_setting()
            .as_deref(),
        Some(explicit_model)
    );
}

#[tokio::test]
async fn settings_write_failure_leaves_effective_runtime_state_unchanged() {
    let manager = test_manager().await;
    let settings_path = manager.config.home_dir.join("settings.json");
    if tokio::fs::try_exists(&settings_path)
        .await
        .expect("inspect settings path")
    {
        tokio::fs::remove_file(&settings_path)
            .await
            .expect("remove test settings file");
    }
    tokio::fs::create_dir(&settings_path)
        .await
        .expect("replace settings file with directory");

    let before_model = manager.effective_config().effective_model_selection();
    let before_preferences = manager.client_preferences();

    manager
        .set_persisted_model_setting(Some("sonnet".to_string()))
        .await
        .expect_err("model write must fail");
    manager
        .set_theme_setting(orbcode_config::ThemeSetting::Dark)
        .await
        .expect_err("theme write must fail");
    manager
        .set_editor_mode_setting(orbcode_config::EditorModeSetting::Vim)
        .await
        .expect_err("editor write must fail");

    assert_eq!(
        manager.effective_config().effective_model_selection(),
        before_model
    );
    assert_eq!(manager.client_preferences(), before_preferences);
}

#[tokio::test]
async fn stale_session_control_mutation_is_typed_and_side_effect_free() {
    let manager = test_manager().await;
    let error = manager
        .set_session_model("missing-session", Some("sonnet".to_string()))
        .await
        .expect_err("stale session must be rejected");
    assert!(matches!(error, CoreError::SessionNotFound(_)));
}

#[tokio::test]
async fn clear_session_inherits_permission_preset_but_discards_other_controls() {
    let manager = test_manager().await;
    let (previous, _) = manager.start_or_resume(None).await.expect("session");
    manager
        .set_session_permission_preset(&previous.session_id, ModelPermissionPreset::ApproveForMe)
        .await
        .expect("set previous mode");

    let next = manager
        .clear_session(&previous.session_id)
        .await
        .expect("clear session");

    assert!(matches!(
        manager.session_permission_mode(&previous.session_id),
        Err(CoreError::SessionNotFound(_))
    ));
    assert_eq!(
        manager
            .session_permission_preset(&next.session_id)
            .expect("new session controls"),
        Some(ModelPermissionPreset::ApproveForMe)
    );
}

#[tokio::test]
async fn clear_session_removes_only_that_sessions_remembered_denials() {
    let manager = test_manager().await;
    let (first, _) = manager.start_or_resume(None).await.expect("first session");
    let (second, _) = manager.start_or_resume(None).await.expect("second session");
    let denied_input = r#"{"command":"printf denied"}"#;
    for session in [&first, &second] {
        manager
            .permission_runtime
            .remember_denied_tool_call_for_session(&session.session_id, "bash", denied_input)
            .await;
    }

    manager
        .clear_session(&first.session_id)
        .await
        .expect("clear first session");

    assert!(
        !manager
            .permission_runtime
            .matches_denied_tool_call_for_session(&first.session_id, "bash", denied_input)
            .await
    );
    assert!(
        manager
            .permission_runtime
            .matches_denied_tool_call_for_session(&second.session_id, "bash", denied_input)
            .await
    );
}
