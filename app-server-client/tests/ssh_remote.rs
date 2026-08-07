use std::time::Duration;

use orbcode_app_server_client::{
    ChildStdioTransportConfig, ClientError, SshOption, SshRemoteConfig, SshRemoteConnection,
    SshRemoteError,
};
use tokio::time::timeout;

const FAKE_SSH: &str = env!("CARGO_BIN_EXE_orbcode-fake-ssh");

fn config(target: &str) -> SshRemoteConfig {
    let mut child = ChildStdioTransportConfig::default();
    child.graceful_shutdown_timeout = Duration::from_millis(75);
    child.terminate_timeout = Duration::from_millis(75);

    let mut config = SshRemoteConfig::new(target).with_ssh_program(FAKE_SSH);
    config.initialize_timeout = Duration::from_secs(2);
    config.child = child;
    config
}

#[tokio::test]
async fn fake_ssh_pins_argv_and_relays_canonical_protocol() {
    let temp = tempfile::tempdir().expect("tempdir");
    let argv_log = temp.path().join("ssh argv.json");
    let mut config = config("normal.example");
    config.remote_cwd = Some("/srv/project with spaces".into());
    config.remote_orbcode_path = "/opt/orb code/orbcode".into();
    config.options = vec![
        format!("IdentityFile={}", argv_log.display())
            .parse::<SshOption>()
            .expect("identity option"),
        "Port=2222".parse().expect("port option"),
        "ProxyJump=bastion.example".parse().expect("jump option"),
    ];

    let connection = SshRemoteConnection::connect(config)
        .await
        .expect("connect fake SSH");
    let sessions = connection
        .client()
        .list_sessions()
        .await
        .expect("session/list over SSH child stdio");
    assert!(sessions.0.is_empty());

    let args: Vec<String> =
        serde_json::from_slice(&std::fs::read(&argv_log).expect("read argv log"))
            .expect("decode argv log");
    assert_eq!(
        args,
        vec![
            "-o",
            &format!("IdentityFile={}", argv_log.display()),
            "-o",
            "Port=2222",
            "-o",
            "ProxyJump=bastion.example",
            "--",
            "normal.example",
            "cd '/srv/project with spaces' && exec '/opt/orb code/orbcode' 'serve' '--stdio'",
        ]
    );
    assert!(args.iter().all(|argument| !argument.contains("token")));

    let diagnostics = connection
        .child()
        .shutdown()
        .await
        .expect("shutdown fake SSH");
    assert!(diagnostics.success);
}

#[tokio::test]
async fn ssh_failures_are_distinct_from_protocol_initialize_failures() {
    let cases = [
        ("host-key.example", "host-key"),
        ("auth.example", "auth"),
        ("missing.example", "missing"),
        ("connection.example", "connection"),
        ("unknown-ssh.example", "ssh"),
        ("protocol.example", "protocol"),
    ];

    for (target, expected) in cases {
        let error = match SshRemoteConnection::connect(config(target)).await {
            Ok(_) => panic!("{target} unexpectedly connected"),
            Err(error) => error,
        };
        let matches_expected = matches!(
            (expected, &error),
            ("host-key", SshRemoteError::HostKey { .. })
                | ("auth", SshRemoteError::Authentication { .. })
                | ("missing", SshRemoteError::RemoteBinaryNotFound { .. })
                | ("connection", SshRemoteError::Connection { .. })
                | ("ssh", SshRemoteError::SshExited { .. })
                | ("protocol", SshRemoteError::ProtocolInitialize { .. })
        );
        assert!(matches_expected, "unexpected {target} error: {error:?}");
        assert!(error.diagnostics().is_some());
    }
}

#[tokio::test]
async fn initialize_timeout_terminates_and_reaps_ssh_child() {
    let mut config = config("hang.example");
    config.initialize_timeout = Duration::from_millis(100);
    let error = match SshRemoteConnection::connect(config).await {
        Ok(_) => panic!("hanging fake SSH unexpectedly connected"),
        Err(error) => error,
    };
    assert!(matches!(error, SshRemoteError::InitializeTimeout { .. }));
    assert!(error.diagnostics().is_some());
}

#[tokio::test]
async fn cancel_closes_ssh_child_and_releases_pending_protocol_call() {
    let connection = SshRemoteConnection::connect(config("pending.example"))
        .await
        .expect("connect pending fake SSH");
    let request = connection.client().list_sessions();
    let shutdown = async {
        tokio::time::sleep(Duration::from_millis(50)).await;
        connection.child().shutdown().await
    };
    let (request, diagnostics) = timeout(Duration::from_secs(2), async {
        tokio::join!(request, shutdown)
    })
    .await
    .expect("cancel should not hang");

    assert!(matches!(
        request,
        Err(ClientError::Cancelled | ClientError::Transport(_))
    ));
    assert!(diagnostics.expect("SSH diagnostics").success);
}

#[tokio::test]
async fn missing_local_ssh_and_invalid_remote_values_are_actionable() {
    let launch = SshRemoteConnection::connect(
        config("normal.example").with_ssh_program("/definitely/missing/orbcode-test-ssh"),
    )
    .await;
    assert!(matches!(launch, Err(SshRemoteError::Launch(_))));

    let invalid_targets = ["", "-oProxyCommand=bad", "host\nother"];
    for target in invalid_targets {
        let result = SshRemoteConnection::connect(config(target)).await;
        assert!(matches!(result, Err(SshRemoteError::InvalidConfig(_))));
    }

    let mut relative_cwd = config("normal.example");
    relative_cwd.remote_cwd = Some("relative/path".into());
    assert!(matches!(
        SshRemoteConnection::connect(relative_cwd).await,
        Err(SshRemoteError::InvalidConfig(_))
    ));
}
