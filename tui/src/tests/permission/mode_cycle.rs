use super::super::support::*;
use orbcode_app_server_client::{AppClient, PermissionMode};

fn preset_options(full_access_disabled: bool) -> Vec<PermissionPresetOption> {
    [
        ModelPermissionPreset::AskForApproval,
        ModelPermissionPreset::ApproveForMe,
        ModelPermissionPreset::FullAccess,
    ]
    .into_iter()
    .map(|value| PermissionPresetOption {
        value,
        label: format!("{value:?}"),
        description: String::new(),
        current: value == ModelPermissionPreset::AskForApproval,
        disabled_reason: (full_access_disabled && value == ModelPermissionPreset::FullAccess)
            .then(|| "disabled by policy".to_string()),
    })
    .collect()
}

async fn mode_cycle_state(name: &str) -> (Arc<AppClient>, TuiState) {
    let home_dir = test_temp_path(&format!("{name}-home"));
    let cwd = test_temp_path(&format!("{name}-workspace"));
    tokio::fs::create_dir_all(&home_dir)
        .await
        .expect("create home");
    tokio::fs::create_dir_all(&cwd).await.expect("create cwd");
    let server = AppServer::new(
        cwd,
        orbcode_app_server::AppConfigOverrides {
            home_dir: Some(home_dir),
            ..orbcode_app_server::AppConfigOverrides::default()
        },
    )
    .await
    .expect("create app server");
    let client = Arc::new(AppClient::new(server).await.expect("create client"));
    let bootstrap = client.bootstrap(None).await.expect("bootstrap");
    let mut state = TuiState::new(Some(Arc::clone(&client)), bootstrap);
    state.refresh_permission_mode(&client).await;
    (client, state)
}

async fn press(
    state: &mut TuiState,
    client: &AppClient,
    key: KeyEvent,
    turn_events: &mut Option<mpsc::UnboundedReceiver<StreamEvent>>,
) {
    let (local_command_tx, _local_command_rx) = mpsc::unbounded_channel();
    state
        .handle_key(client, key, turn_events, &local_command_tx)
        .await
        .expect("handle key");
}

#[test]
fn cycle_skips_full_access_when_managed_policy_disables_it() {
    assert_eq!(
        InteractivePermissionMode::ApproveForMe.next_available(&preset_options(true)),
        InteractivePermissionMode::Plan
    );
    assert_eq!(
        InteractivePermissionMode::ApproveForMe.next_available(&preset_options(false)),
        InteractivePermissionMode::FullAccess
    );
}

#[tokio::test]
async fn shift_tab_cycles_ask_approve_full_plan_and_back_to_ask() {
    let (client, mut state) = mode_cycle_state("shift-tab-cycle").await;
    let mut turn_events = None;
    let shift_tab = || KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT);
    state.input = "draft stays intact".to_string();
    state.input_cursor = state.input.len();
    let initial_message_count = state.messages.len();

    assert_eq!(
        state.status.permission_mode,
        InteractivePermissionMode::AskForApproval
    );

    press(&mut state, &client, shift_tab(), &mut turn_events).await;
    assert_eq!(
        state.status.permission_mode,
        InteractivePermissionMode::AskForApproval
    );
    assert_eq!(
        client
            .session_control_state(&state.session_id)
            .await
            .expect("ask controls")
            .active_permission_preset,
        Some(ModelPermissionPreset::AskForApproval)
    );
    press(&mut state, &client, shift_tab(), &mut turn_events).await;
    assert_eq!(
        state.status.permission_mode,
        InteractivePermissionMode::ApproveForMe
    );
    assert_eq!(
        client
            .session_control_state(&state.session_id)
            .await
            .expect("approve controls")
            .active_permission_preset,
        Some(ModelPermissionPreset::ApproveForMe)
    );
    press(&mut state, &client, shift_tab(), &mut turn_events).await;
    assert!(state.overlay.is_none());
    assert_eq!(
        state.status.permission_mode,
        InteractivePermissionMode::FullAccess
    );
    assert_eq!(
        client
            .session_control_state(&state.session_id)
            .await
            .expect("full access controls")
            .active_permission_preset,
        Some(ModelPermissionPreset::FullAccess)
    );
    press(&mut state, &client, shift_tab(), &mut turn_events).await;
    let controls = client
        .session_control_state(&state.session_id)
        .await
        .expect("plan controls");
    assert_eq!(controls.permission_mode, PermissionMode::Plan);
    assert_eq!(controls.active_permission_preset, None);
    assert_eq!(
        state.status.permission_mode,
        InteractivePermissionMode::Plan
    );

    press(&mut state, &client, shift_tab(), &mut turn_events).await;
    assert_eq!(
        state.status.permission_mode,
        InteractivePermissionMode::AskForApproval
    );
    assert_eq!(
        client
            .session_control_state(&state.session_id)
            .await
            .expect("ask controls")
            .active_permission_preset,
        Some(ModelPermissionPreset::AskForApproval)
    );
    assert_eq!(state.input, "draft stays intact");
    assert_eq!(state.messages.len(), initial_message_count);
    assert!(state.status_line.is_empty());
    assert!(state.footer_right_text().ends_with("Ask for approval"));
}

#[tokio::test]
async fn shift_tab_does_not_change_modes_during_an_active_turn() {
    let (client, mut state) = mode_cycle_state("shift-tab-active-turn").await;
    let (_turn_tx, turn_rx) = mpsc::unbounded_channel();
    let mut turn_events = Some(turn_rx);

    press(
        &mut state,
        &client,
        KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT),
        &mut turn_events,
    )
    .await;

    assert!(state.status_line.contains("active turn"));
    assert_eq!(
        client
            .session_control_state(&state.session_id)
            .await
            .expect("unchanged controls")
            .active_permission_preset,
        Some(ModelPermissionPreset::AskForApproval)
    );
}
