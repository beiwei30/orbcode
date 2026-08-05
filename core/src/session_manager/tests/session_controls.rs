use std::sync::{Arc, atomic::AtomicBool};

use orbcode_config::{AppConfigOverrides, PermissionMode};
use orbcode_protocol::{EffortLevel, MessageRole, TranscriptMessage, TurnContext};
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
async fn explicit_session_mode_overrides_global_allow_all() {
    let manager = test_manager().await;
    let (session, _) = manager.start_or_resume(None).await.expect("session");

    manager.set_allow_all(true);
    assert!(
        manager
            .permission_context_for_session(&session.session_id)
            .allow_tools,
        "an untouched session must honor the global SDK permission control"
    );

    manager
        .set_session_permission_mode(&session.session_id, PermissionMode::Plan)
        .await
        .expect("set explicit session mode");
    assert!(
        !manager
            .permission_context_for_session(&session.session_id)
            .allow_tools,
        "an explicit session mode must remain isolated from global allow-all"
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
async fn stale_session_control_mutation_is_typed_and_side_effect_free() {
    let manager = test_manager().await;
    let error = manager
        .set_session_model("missing-session", Some("sonnet".to_string()))
        .await
        .expect_err("stale session must be rejected");
    assert!(matches!(error, CoreError::SessionNotFound(_)));
}

#[tokio::test]
async fn clear_session_discards_previous_session_controls() {
    let manager = test_manager().await;
    let (previous, _) = manager.start_or_resume(None).await.expect("session");
    manager
        .set_session_permission_mode(&previous.session_id, PermissionMode::Plan)
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
            .session_permission_mode(&next.session_id)
            .expect("new session controls"),
        PermissionMode::Default
    );
}
