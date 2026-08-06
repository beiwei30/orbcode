use crate::commands::dispatch::SlashCommandOutcome;
use crate::tests::support::*;
use orbcode_app_server_client::{AppClient, SessionGoalSetParams};
use orbcode_protocol::{SessionGoalStatus, StreamEvent};

fn mock_overrides(scenario: &str) -> HashMap<String, String> {
    let mut env = orbcode_app_server::sealed_provider_env_overrides();
    env.insert(
        "ANTHROPIC_BASE_URL".to_string(),
        format!("mock://anthropic?scenario={scenario}"),
    );
    env.insert("ANTHROPIC_API_KEY".to_string(), "test-key".to_string());
    env
}

async fn goal_tui(label: &str) -> (AppServer, Arc<AppClient>, TuiState) {
    let home_dir = test_temp_path(&format!("{label}-home"));
    let cwd = test_temp_path(&format!("{label}-workspace"));
    tokio::fs::create_dir_all(&home_dir)
        .await
        .expect("create home");
    tokio::fs::create_dir_all(&cwd).await.expect("create cwd");
    let server = AppServer::new(
        cwd,
        orbcode_app_server::AppConfigOverrides {
            home_dir: Some(home_dir),
            env_overrides: mock_overrides("success"),
            ..orbcode_app_server::AppConfigOverrides::default()
        },
    )
    .await
    .expect("create app server");
    let client = Arc::new(
        AppClient::new_interactive_persistent_goals(server.clone())
            .await
            .expect("create goal-capable TUI client"),
    );
    let bootstrap = client.bootstrap(None).await.expect("bootstrap session");
    let state = TuiState::new(Some(Arc::clone(&client)), bootstrap);
    (server, client, state)
}

async fn drain_terminal(mut events: mpsc::UnboundedReceiver<StreamEvent>) -> StreamEvent {
    tokio::time::timeout(Duration::from_secs(10), async move {
        while let Some(event) = events.recv().await {
            if event.is_terminal() {
                return event;
            }
        }
        panic!("goal stream closed without terminal event");
    })
    .await
    .expect("goal stream terminal timeout")
}

#[tokio::test]
async fn goal_slash_command_manages_and_renders_persisted_goal() {
    let (server, client, mut state) = goal_tui("goal-command").await;
    let session_id = state.session_id.clone();
    let (local_command_tx, _local_command_rx) = mpsc::unbounded_channel();

    let outcome = state
        .handle_command(
            &client,
            "/goal create --budget 5000 finish the durable task",
            &local_command_tx,
        )
        .await
        .expect("create goal");
    let SlashCommandOutcome::GoalTurnStarted(events) = outcome else {
        panic!("creating an active goal should start a turn");
    };
    let terminal = drain_terminal(events).await;
    assert!(
        matches!(terminal, StreamEvent::TurnFinished { .. }),
        "{terminal:?}"
    );

    let created = client
        .get_goal(&session_id)
        .await
        .expect("get created goal")
        .expect("created goal");
    assert_eq!(created.objective, "finish the durable task");
    assert_eq!(created.token_budget, Some(5000));
    assert_eq!(created.status, SessionGoalStatus::Active);

    let reconnect = Arc::new(
        AppClient::new_interactive_persistent_goals(server)
            .await
            .expect("reconnect goal-capable client"),
    );
    let bootstrap = reconnect
        .bootstrap(Some(&session_id))
        .await
        .expect("reload persisted session");
    let mut resumed_state = TuiState::new(Some(Arc::clone(&reconnect)), bootstrap);
    resumed_state
        .handle_command(&reconnect, "/goal", &local_command_tx)
        .await
        .expect("show goal");
    let rendered = resumed_state
        .messages
        .last()
        .expect("goal slash output")
        .content
        .clone();
    assert!(rendered.contains("finish the durable task"), "{rendered}");
    assert!(rendered.contains("Tokens: "), "{rendered}");
    assert!(rendered.contains("Elapsed: "), "{rendered}");

    resumed_state
        .handle_command(&reconnect, "/goal pause", &local_command_tx)
        .await
        .expect("pause goal");
    resumed_state
        .handle_command(
            &reconnect,
            "/goal edit --budget 7000 finish and verify the durable task",
            &local_command_tx,
        )
        .await
        .expect("edit goal");
    resumed_state
        .handle_command(&reconnect, "/goal budget none", &local_command_tx)
        .await
        .expect("clear goal budget");
    let edited = reconnect
        .get_goal(&session_id)
        .await
        .expect("get edited goal")
        .expect("edited goal");
    assert_eq!(edited.status, SessionGoalStatus::Paused);
    assert_eq!(edited.objective, "finish and verify the durable task");
    assert_eq!(edited.token_budget, None);

    let outcome = resumed_state
        .handle_command(&reconnect, "/goal resume", &local_command_tx)
        .await
        .expect("resume goal");
    let SlashCommandOutcome::GoalTurnStarted(events) = outcome else {
        panic!("resuming the goal should start a turn");
    };
    drain_terminal(events).await;
    resumed_state
        .handle_command(&reconnect, "/goal pause", &local_command_tx)
        .await
        .expect("pause resumed goal");
    resumed_state
        .handle_command(&reconnect, "/goal clear", &local_command_tx)
        .await
        .expect("clear goal");
    assert!(
        reconnect
            .get_goal(&session_id)
            .await
            .expect("get cleared goal")
            .is_none()
    );
    assert!(
        !reconnect
            .cancel_turn(&session_id)
            .await
            .expect("cleanup check")
    );
}

#[tokio::test]
async fn queued_followup_wins_before_automatic_goal_continuation() {
    let (_server, client, mut state) = goal_tui("goal-followup-order").await;
    let session_id = state.session_id.clone();
    let goal = client
        .set_goal(SessionGoalSetParams {
            session_id: session_id.clone(),
            expected_revision: None,
            replace: false,
            objective: Some("continue after user work".to_string()),
            status: Some(SessionGoalStatus::Active),
            token_budget: Some(Some(10_000)),
        })
        .await
        .expect("set active goal");
    assert!(goal.last_goal_turn_id.is_none());

    state.queue_followup("user follow-up goes first".to_string());
    let mut turn_events = None;
    state
        .submit_queued_followups_if_idle(&client, &mut turn_events)
        .await
        .expect("submit queued follow-up");
    state
        .continue_goal_if_idle(&client, &mut turn_events)
        .await
        .expect("skip goal while user turn is active");
    assert!(
        client
            .get_goal(&session_id)
            .await
            .expect("get unstarted goal")
            .expect("goal")
            .last_goal_turn_id
            .is_none()
    );

    drain_terminal(turn_events.take().expect("queued user turn stream")).await;
    state
        .continue_goal_if_idle(&client, &mut turn_events)
        .await
        .expect("continue goal after user turn");
    let started = client
        .get_goal(&session_id)
        .await
        .expect("get started goal")
        .expect("goal");
    assert!(started.last_goal_turn_id.is_some());
    drain_terminal(turn_events.take().expect("goal turn stream")).await;

    let current = client
        .get_goal(&session_id)
        .await
        .expect("get current goal")
        .expect("goal");
    client
        .set_goal(SessionGoalSetParams {
            session_id: session_id.clone(),
            expected_revision: Some(current.revision),
            replace: false,
            objective: None,
            status: Some(SessionGoalStatus::Paused),
            token_budget: None,
        })
        .await
        .expect("pause test goal");
    assert!(
        client
            .clear_goal(&session_id)
            .await
            .expect("clear test goal")
    );
    assert!(
        !client
            .cancel_turn(&session_id)
            .await
            .expect("cleanup check")
    );
}
