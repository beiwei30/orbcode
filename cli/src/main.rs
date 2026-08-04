use std::io::{IsTerminal, Write};
use std::sync::Arc;

use anyhow::{Result, bail};
use clap::Parser;
use orbcode_app_server::{AppServer, SessionStatus};
use orbcode_app_server_client::AppClient;

mod acp_sdk;
mod args;
mod build_info;
mod commands;
mod control;
mod exit_code;
mod headless;
mod stream_json;

use args::{Cli, CliInputFormat, CliOutputFormat, Command, GlobalOptions};
use exit_code::HeadlessOutcome;

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();

    let cli = Cli::parse();
    let continue_latest = cli.continue_latest;
    let print_mode = cli.print;
    let resume_value = cli.resume.clone();
    let session_id_override = cli.session_id.clone();
    let mut positional_prompt = cli.prompt.clone();
    let output_format = cli.output_format.unwrap_or(CliOutputFormat::Text);
    let input_format = cli.input_format.unwrap_or(CliInputFormat::Text);
    let verbose = cli.verbose;

    if continue_latest
        && !matches!(
            &cli.command,
            None | Some(Command::Tui | Command::Prompt { session: None, .. })
        )
        && !print_mode
    {
        invalid_cli_input(
            "--continue can only be used with the TUI or with prompt without --session",
        );
    }

    if print_mode {
        if cli.command.is_some() {
            invalid_cli_input("--print cannot be combined with a subcommand");
        }
        if output_format == CliOutputFormat::StreamJson && !verbose {
            invalid_cli_input("--output-format stream-json requires --verbose");
        }
        if matches!(input_format, CliInputFormat::Text) && positional_prompt.is_none() {
            if !std::io::stdin().is_terminal() {
                use std::io::Read as _;
                let mut piped = String::new();
                std::io::stdin()
                    .read_to_string(&mut piped)
                    .expect("reading piped stdin");
                let piped = piped.trim().to_string();
                if piped.is_empty() {
                    invalid_cli_input("--print requires a prompt; stdin was piped but empty");
                }
                positional_prompt = Some(piped);
            } else {
                invalid_cli_input(
                    "--print is one-shot headless mode and needs a prompt; \
                     pass it as a positional arg, use --input-format stream-json, \
                     or drop --print to resume interactively",
                );
            }
        }
    } else if cli.output_format.is_some() || cli.input_format.is_some() {
        let is_attach = matches!(cli.command, Some(Command::Attach { .. }));
        if cli.input_format.is_some() {
            invalid_cli_input("--input-format is only valid with -p/--print");
        }
        if cli.output_format.is_some() && !is_attach {
            invalid_cli_input("--output-format is only valid with -p/--print or attach");
        }
        if is_attach && output_format == CliOutputFormat::StreamJson && !verbose {
            invalid_cli_input("--output-format stream-json requires --verbose");
        }
    } else if positional_prompt.is_some() && cli.command.is_none() {
        invalid_cli_input(
            "positional argument is only accepted with -p/--print; \
             use a subcommand (e.g. `orbcode prompt <text>`) or pass `--print` explicitly",
        );
    }

    let global_options = GlobalOptions::from_cli(&cli);
    let cwd = std::env::current_dir()?;
    let overrides = global_options.as_overrides()?;
    let command = cli.command.unwrap_or(Command::Tui);

    if let Command::Remote { endpoint, token } = command {
        let remote_client = Arc::new(connect_remote_client(&endpoint, &token).await?);
        orbcode_tui::run_tui(remote_client, None).await?;
        return Ok(());
    }

    let app_server = AppServer::new(cwd, overrides).await?;
    let client = Arc::new(
        AppClient::new(app_server.clone())
            .await
            .map_err(|e| anyhow::anyhow!("protocol init: {e}"))?,
    );

    if print_mode {
        let session =
            pick_requested_session(&client, session_id_override, resume_value, continue_latest)
                .await?;
        headless::run_print_mode(
            app_server,
            &client,
            session,
            positional_prompt,
            output_format,
            input_format,
            verbose,
        )
        .await?;
        return Ok(());
    }

    match command {
        Command::Remote { .. } => unreachable!("remote command returns before local AppServer"),
        Command::Tui => {
            let requested_session = pick_requested_session(
                &client,
                session_id_override.clone(),
                resume_value.clone(),
                continue_latest,
            )
            .await?;
            let session_id = orbcode_tui::run_tui(Arc::clone(&client), requested_session).await?;
            print_resume_hint(&session_id);
        }
        Command::Resume { session_id } => {
            let session_id = orbcode_tui::run_tui(Arc::clone(&client), Some(session_id)).await?;
            print_resume_hint(&session_id);
        }
        Command::Fork {
            session_id,
            title,
            note,
            prompt,
            tui,
        } => commands::run_fork(app_server, &client, session_id, title, note, prompt, tui).await?,
        Command::Prompt {
            prompt,
            mut session,
            bg,
        } => {
            if continue_latest {
                if session.is_some() {
                    bail!("--continue cannot be combined with prompt --session");
                }
                session = Some(latest_available_session_id(&client).await?);
            }
            if let Some(global_session) = session_id_override.clone() {
                if session.is_some() {
                    bail!("--session-id cannot be combined with prompt --session");
                }
                session = Some(global_session);
            }
            if let Some(resume_value) = resume_value.clone() {
                if session.is_some() {
                    bail!("--resume cannot be combined with --session/--continue/--session-id");
                }
                session = Some(if resume_value.is_empty() {
                    latest_available_session_id(&client).await?
                } else {
                    resume_value
                });
            }
            if bg {
                commands::start_background_prompt(
                    app_server,
                    &client,
                    global_options,
                    session,
                    prompt,
                )
                .await?;
            } else {
                let session_id =
                    headless::run_headless_prompt(app_server, &client, session, prompt).await?;
                print_resume_hint(&session_id);
            }
        }
        Command::Sessions { json } => commands::print_sessions(&client, json).await?,
        Command::Rename { session_id, title } => {
            commands::run_rename_session(&client, session_id, title).await?
        }
        Command::Ps => commands::print_background_jobs(&client).await?,
        Command::Logs { job_id, follow } => {
            commands::print_background_log(&client, job_id, follow).await?
        }
        Command::Attach { job_id } => {
            if output_format == CliOutputFormat::StreamJson {
                commands::attach_stream_json(&client, job_id).await?
            } else {
                commands::print_background_log(&client, job_id, true).await?
            }
        }
        Command::Kill { job_id } => commands::cancel_background_job(&client, job_id).await?,
        Command::Providers => commands::print_providers(&client).await?,
        Command::Context => commands::print_context(&client).await?,
        Command::Tools => commands::print_tools(&client).await?,
        Command::Tool {
            name,
            input,
            session,
        } => commands::run_tool(&client, session, name, input.unwrap_or_default()).await?,
        Command::Auth { command } => commands::run_auth(app_server, &client, command).await?,
        Command::Mcp { command } => commands::run_mcp(app_server, &client, command).await?,
        Command::Doctor { command } => commands::run_doctor(&client, command).await?,
        Command::Advanced => commands::print_advanced_capabilities(&client).await?,
        Command::Acp => acp_sdk::run_acp_adapter(app_server).await?,
        Command::BgWorker {
            job_id,
            session_id,
            prompt,
        } => {
            headless::run_background_worker(app_server, &client, job_id, session_id, prompt).await?
        }
        Command::Serve {
            stdio,
            socket,
            websocket,
            auth_token: cli_token,
            allowed_origins,
        } => {
            if let Some(path) = socket {
                #[cfg(not(unix))]
                {
                    let _ = path;
                    invalid_cli_input("--socket is only supported on Unix");
                }
                #[cfg(unix)]
                {
                    let token = cli_token.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                    let config = orbcode_app_server_transport::StdioTransportConfig {
                        auth_token: Some(token.clone()),
                        ..Default::default()
                    };
                    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel::<()>();
                    let path_for_info = path.display().to_string();
                    let token_for_info = token;
                    let info_task = tokio::spawn(async move {
                        if ready_rx.await.is_ok() {
                            print_serve_connection_info(serde_json::json!({
                                "transport": "socket",
                                "path": path_for_info,
                                "auth_token": &token_for_info,
                            }));
                        }
                    });
                    orbcode_app_server_transport::run_unix_socket_transport_with_ready(
                        &path,
                        app_server,
                        config,
                        Some(ready_tx),
                    )
                    .await
                    .map_err(|e| anyhow::anyhow!("transport error: {e}"))?;
                    info_task.abort();
                }
            } else if let Some(addr_str) = websocket {
                let addr: std::net::SocketAddr = addr_str
                    .parse()
                    .map_err(|e| anyhow::anyhow!("invalid --websocket address: {e}"))?;
                let token = cli_token.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                let ws_config = orbcode_app_server_transport::WebSocketTransportConfig {
                    auth_token: Some(token.clone()),
                    allowed_origins,
                    ..Default::default()
                };
                let (bound_tx, bound_rx) = tokio::sync::oneshot::channel::<std::net::SocketAddr>();
                let token_for_info = token;
                let info_task = tokio::spawn(async move {
                    if let Ok(bound_addr) = bound_rx.await {
                        print_serve_connection_info(serde_json::json!({
                            "transport": "websocket",
                            "addr": bound_addr.to_string(),
                            "auth_token": &token_for_info,
                        }));
                    }
                });
                orbcode_app_server_transport::run_websocket_transport_with_bound_addr(
                    addr,
                    app_server,
                    ws_config,
                    Some(bound_tx),
                )
                .await
                .map_err(|e| anyhow::anyhow!("transport error: {e}"))?;
                info_task.abort();
            } else if stdio {
                eprintln!("{}", serde_json::json!({"transport": "stdio"}));
                let config = orbcode_app_server_transport::StdioTransportConfig::default();
                orbcode_app_server_transport::run_stdio_transport(app_server, config)
                    .await
                    .map_err(|e| anyhow::anyhow!("transport error: {e}"))?;
            } else {
                invalid_cli_input("serve requires --stdio, --socket <PATH>, or --websocket <ADDR>");
            }
        }
    }

    Ok(())
}

fn print_resume_hint(session_id: &str) {
    eprintln!("\nTo continue this session, run orbcode resume {session_id}");
}

fn print_serve_connection_info(value: serde_json::Value) {
    if std::io::stdout().is_terminal() {
        match value["transport"].as_str() {
            Some("socket") => tracing::info!(
                transport = "socket",
                path = value["path"].as_str().unwrap_or(""),
                auth_token = value["auth_token"].as_str().unwrap_or(""),
                "serve connection info"
            ),
            Some("websocket") => tracing::info!(
                transport = "websocket",
                addr = value["addr"].as_str().unwrap_or(""),
                auth_token = value["auth_token"].as_str().unwrap_or(""),
                "serve connection info"
            ),
            _ => tracing::info!(%value, "serve connection info"),
        }
        return;
    }

    let mut stdout = std::io::stdout().lock();
    let _ = writeln!(stdout, "{value}");
    let _ = stdout.flush();
}

async fn connect_remote_client(endpoint: &str, token: &str) -> Result<AppClient> {
    if is_websocket_endpoint(endpoint) {
        AppClient::connect_websocket(endpoint, token)
            .await
            .map_err(|e| anyhow::anyhow!("remote websocket connect: {e}"))
    } else {
        #[cfg(not(unix))]
        {
            let _ = token;
            return Err(anyhow::anyhow!(
                "Unix socket transport is not supported on this platform"
            ));
        }
        #[cfg(unix)]
        AppClient::connect_socket(std::path::Path::new(endpoint), token)
            .await
            .map_err(|e| anyhow::anyhow!("remote socket connect: {e}"))
    }
}

fn is_websocket_endpoint(endpoint: &str) -> bool {
    endpoint.starts_with("ws://")
        || endpoint.starts_with("wss://")
        || (!endpoint.starts_with('/') && endpoint.contains(':'))
}

async fn pick_requested_session(
    client: &AppClient,
    session_id_override: Option<String>,
    resume_value: Option<String>,
    continue_latest: bool,
) -> Result<Option<String>> {
    let mut chosen: Option<String> = None;
    if let Some(session_id) = session_id_override {
        chosen = Some(session_id);
    }
    if let Some(resume_value) = resume_value {
        if chosen.is_some() {
            bail!("--resume cannot be combined with --session-id");
        }
        chosen = Some(if resume_value.is_empty() {
            latest_available_session_id(client).await?
        } else {
            resume_value
        });
    }
    if continue_latest {
        if chosen.is_some() {
            bail!("--continue cannot be combined with --resume/--session-id");
        }
        chosen = Some(latest_available_session_id(client).await?);
    }
    Ok(chosen)
}

async fn latest_available_session_id(client: &AppClient) -> Result<String> {
    let cwd = std::env::current_dir()
        .ok()
        .and_then(|p| p.to_str().map(String::from));
    let available: Vec<_> = client
        .list_sessions()
        .await?
        .into_iter()
        .filter(|s| matches!(s.status, SessionStatus::Available))
        .collect();
    let chosen = cwd
        .as_deref()
        .and_then(|cwd| available.iter().find(|s| s.cwd.as_deref() == Some(cwd)))
        .or_else(|| available.first());
    chosen
        .map(|s| s.session_id.clone())
        .ok_or_else(|| anyhow::anyhow!("No conversation found to continue"))
}

pub(crate) fn invalid_cli_input(message: &str) -> ! {
    eprintln!("error: {message}");
    std::process::exit(HeadlessOutcome::InvalidCliInput.code());
}

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with_target(false)
        .compact()
        .try_init();
}

#[cfg(test)]
mod tests {
    use crate::headless::context_compacted_log_line;

    #[test]
    fn context_compacted_log_line_reports_provider_summary() {
        assert_eq!(
            context_compacted_log_line(12, 5, 2, true, None),
            "context compacted in 12 ms (5 -> 2 messages, provider summary)"
        );
    }

    #[test]
    fn context_compacted_log_line_reports_fallback_reason() {
        assert_eq!(
            context_compacted_log_line(7, 4, 2, false, Some("missing provider")),
            "context compacted in 7 ms (4 -> 2 messages, local fallback): missing provider"
        );
    }
}
