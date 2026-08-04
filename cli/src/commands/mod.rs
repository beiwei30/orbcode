use std::fmt::Write as _;
use std::io::{self, Write};
use std::process::{Command as StdCommand, Stdio};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, bail};
use orbcode_app_server::{
    AppServer, McpAuth, McpOAuthBrowserLoginInput, McpOAuthDeviceLoginInput, McpOAuthTokenInput,
    McpServerConfig, PermissionOverview, SessionStatus,
};
use orbcode_app_server_client::AppClient;
use serde::Deserialize;

use crate::args::{AuthCommand, CliMcpAuthCommand, DoctorCommand, GlobalOptions, McpCommand};
use crate::headless::run_headless_prompt;

/// Helper to convert a `ClientError` into `anyhow::Error`.
fn client_err(e: orbcode_app_server_client::ClientError) -> anyhow::Error {
    anyhow::anyhow!("{e}")
}

pub(crate) async fn run_fork(
    app_server: AppServer,
    client: &Arc<AppClient>,
    session_id: String,
    title: Option<String>,
    note: Option<String>,
    prompt: Option<String>,
    open_tui: bool,
) -> Result<()> {
    let forked = client
        .fork_session(&session_id, title, note)
        .await
        .map_err(client_err)?;
    let forked_id = forked["session_id"]
        .as_str()
        .unwrap_or("unknown")
        .to_string();
    let forked_title = forked["title"]
        .as_str()
        .unwrap_or("Untitled Session")
        .to_string();
    println!("forked {session_id} -> {forked_id}");
    println!("title {forked_title}");

    if let Some(prompt) = prompt {
        run_headless_prompt(app_server, client, Some(forked_id), prompt).await?;
    } else if open_tui {
        orbcode_tui::run_tui(Arc::clone(client), Some(forked_id)).await?;
    }

    Ok(())
}

pub(crate) async fn print_sessions(client: &AppClient, json: bool) -> Result<()> {
    let sessions = client.list_sessions().await?;

    if json {
        for session in &sessions {
            println!("{}", serde_json::to_string(session)?);
        }
        return Ok(());
    }

    if sessions.is_empty() {
        println!("no persisted sessions");
        return Ok(());
    }

    for session in sessions {
        let status = match &session.status {
            SessionStatus::Available => String::new(),
            SessionStatus::Corrupt { reason } => format!("  [corrupt: {reason}]"),
            _ => "  [unknown status]".to_string(),
        };
        let branch = session
            .git_branch
            .as_deref()
            .map(|branch| format!("  @{branch}"))
            .unwrap_or_default();
        let provider = session
            .provider
            .map(|provider| format!("  {}", provider.as_str()))
            .unwrap_or_default();
        let title = session
            .title
            .unwrap_or_else(|| "Untitled Session".to_string());
        println!(
            "{}  {}  msgs={}  {}{}{}{}",
            session.session_id,
            title,
            session.message_count,
            session.updated_at.format("%Y-%m-%d %H:%M"),
            branch,
            provider,
            status,
        );
    }

    Ok(())
}

pub(crate) async fn run_rename_session(
    client: &AppClient,
    session_id: String,
    title: String,
) -> Result<()> {
    client.rename_session(&session_id, &title).await?;
    println!("renamed {session_id} -> {title}");
    Ok(())
}

pub(crate) async fn print_background_jobs(client: &AppClient) -> Result<()> {
    let jobs = client.list_background_jobs().await.map_err(client_err)?;
    let jobs_arr = jobs.as_array().cloned().unwrap_or_default();
    if jobs_arr.is_empty() {
        println!("no background jobs");
        return Ok(());
    }

    for job in &jobs_arr {
        let task_id = job["task_id"].as_str().unwrap_or("");
        let status = job["status"].as_str().unwrap_or("").to_ascii_lowercase();
        let session_id = job["session_id"].as_str().unwrap_or("");
        let description = job["description"].as_str().unwrap_or("");
        println!(
            "{}  {:10} session={} {}",
            task_id,
            status,
            short_id(session_id),
            description
        );
    }

    Ok(())
}

pub(crate) async fn print_background_log(
    client: &AppClient,
    job_id: String,
    follow: bool,
) -> Result<()> {
    let mut printed_len = 0usize;
    loop {
        let log_result = client
            .read_background_log(&job_id)
            .await
            .map_err(client_err)?;
        let contents = log_result["log"].as_str().unwrap_or("");
        if contents.len() > printed_len {
            print!("{}", &contents[printed_len..]);
            io::stdout().flush()?;
            printed_len = contents.len();
        }

        let detail = client
            .background_job_detail(&job_id)
            .await
            .map_err(client_err)?;
        let status_raw = detail["status"].as_str().unwrap_or("");
        let status = status_raw.to_ascii_lowercase();
        let is_active = status == "queued" || status == "running";
        if !follow || !is_active {
            if printed_len == 0 && contents.is_empty() {
                println!("no log output for {job_id}");
            }
            if !is_active {
                println!("\nstatus {status}");
                if let Some(error) = detail["error"].as_str() {
                    println!("detail {error}");
                }
            }
            break;
        }

        tokio::time::sleep(Duration::from_millis(250)).await;
    }

    Ok(())
}

pub(crate) async fn attach_stream_json(client: &AppClient, job_id: String) -> Result<()> {
    let mut printed_len = 0usize;
    loop {
        let events_result = client
            .read_background_events(&job_id)
            .await
            .map_err(client_err)?;
        let contents = events_result["events"].as_str().unwrap_or("");
        if contents.len() > printed_len {
            print!("{}", &contents[printed_len..]);
            io::stdout().flush()?;
            printed_len = contents.len();
        }

        let detail = client
            .background_job_detail(&job_id)
            .await
            .map_err(client_err)?;
        let status = detail["status"].as_str().unwrap_or("").to_ascii_lowercase();
        let is_active = status == "queued" || status == "running";
        if !is_active {
            let events_result = client
                .read_background_events(&job_id)
                .await
                .map_err(client_err)?;
            let contents = events_result["events"].as_str().unwrap_or("");
            if contents.len() > printed_len {
                print!("{}", &contents[printed_len..]);
                io::stdout().flush()?;
            }
            break;
        }

        tokio::time::sleep(Duration::from_millis(250)).await;
    }

    Ok(())
}

pub(crate) async fn cancel_background_job(client: &AppClient, job_id: String) -> Result<()> {
    let result = client
        .cancel_background_job(&job_id)
        .await
        .map_err(client_err)?;
    let rjid = result["job_id"].as_str().unwrap_or(&job_id);
    let status = result["status"]
        .as_str()
        .unwrap_or("unknown")
        .to_ascii_lowercase();
    println!("background job {rjid} -> {status}");
    if let Some(error) = result["error"].as_str() {
        println!("detail {error}");
    }
    Ok(())
}

pub(crate) async fn start_background_prompt(
    app_server: AppServer,
    client: &AppClient,
    global_options: GlobalOptions,
    session: Option<String>,
    prompt: String,
) -> Result<()> {
    let bootstrap = client
        .bootstrap(session.as_deref())
        .await
        .map_err(client_err)?;
    let session_id = bootstrap.session.session_id;
    let job_value = client
        .create_background_job(&session_id, &prompt)
        .await
        .map_err(client_err)?;
    let job_id = job_value["job_id"].as_str().unwrap_or("").to_string();

    let current_exe = std::env::current_exe()?;
    let current_dir = std::env::current_dir()?;
    let mut command = StdCommand::new(current_exe);
    command
        .current_dir(current_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for arg in global_options.as_args() {
        command.arg(arg);
    }
    command
        .arg("bg-worker")
        .arg("--job-id")
        .arg(&job_id)
        .arg("--session-id")
        .arg(&session_id)
        .arg("--prompt")
        .arg(&prompt);

    match command.spawn() {
        Ok(child) => {
            // mark_background_running is a lifecycle operation that stays on
            // AppServer (not yet on the protocol path).
            app_server
                .mark_background_running(&job_id, Some(child.id()))
                .await?;
            println!("queued background job {job_id} for session {session_id}");
            if let Some(log_path) = job_value["log_path"].as_str()
                && !log_path.is_empty()
            {
                println!("output file {log_path}");
            }
            println!("use `orbcode logs {job_id}` to inspect output");
        }
        Err(error) => {
            // fail_background_job is a lifecycle operation that stays on
            // AppServer (not yet on the protocol path).
            let _ = app_server
                .fail_background_job(&job_id, format!("spawn failed: {error}"))
                .await;
            return Err(error.into());
        }
    }

    Ok(())
}

pub(crate) async fn print_providers(client: &AppClient) -> Result<()> {
    // Use the permission_overview to get the full permissions description.
    let perm_overview = client.permission_overview().await.map_err(client_err)?;
    let perm_desc = serde_json::from_value::<PermissionOverview>(perm_overview).map_or_else(
        |_| "unknown".to_string(),
        |overview| overview.permissions.describe(),
    );

    // Get provider data (default/fallback/retries + model resolutions).
    let providers_data = client.supported_providers().await.map_err(client_err)?;

    let default_provider = providers_data["default_provider"]
        .as_str()
        .unwrap_or("unknown");
    let fallback_provider = providers_data["fallback_provider"].as_str();
    let max_retries = providers_data["max_retries"].as_u64().unwrap_or(0);

    #[derive(Deserialize)]
    struct Resolution {
        provider: String,
        display_name: String,
        capabilities: Vec<String>,
    }

    let resolutions: Vec<Resolution> = providers_data["resolutions"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|v| serde_json::from_value(v).ok())
        .collect();

    println!(
        "active chain: provider={} fallback={:?} retries={} permissions={}",
        default_provider,
        fallback_provider.map(String::from),
        max_retries,
        perm_desc,
    );

    for resolution in &resolutions {
        let marker = if resolution.provider == default_provider {
            "default"
        } else if fallback_provider == Some(resolution.provider.as_str()) {
            "fallback"
        } else {
            ""
        };
        let model = format!(
            "model={} capabilities={}",
            resolution.display_name,
            capability_list(&resolution.capabilities)
        );
        println!("{:10} {:9} | {}", resolution.provider, marker, model);
    }

    Ok(())
}

pub(crate) async fn print_context(client: &AppClient) -> Result<()> {
    let context = client.context_preview().await.map_err(client_err)?;
    let cwd = context["cwd"].as_str().unwrap_or("");
    let current_date = context["current_date"].as_str().unwrap_or("");
    let git_branch = context["git_branch"].as_str();
    let git_status = context["git_status"].as_str();
    let claude_md = context["claude_md"].as_str();

    println!("cwd: {cwd}");
    println!("date: {current_date}");
    println!("git branch: {}", git_branch.unwrap_or("not available"));
    println!(
        "git status: {}",
        git_status.unwrap_or("clean or unavailable")
    );
    println!(
        "claude md: {}",
        if claude_md.is_some() {
            "loaded"
        } else {
            "not available"
        }
    );

    let memory_sources = context["memory_sources"].as_array();
    match memory_sources {
        Some(sources) if !sources.is_empty() => {
            println!("memory sources:");
            for source in sources {
                let label = source["label"].as_str().unwrap_or("");
                let status = source["status"].as_str().unwrap_or("unknown");
                let path = source["path"].as_str().unwrap_or("(not configured)");
                let writable = source["writable"].as_bool().unwrap_or(false);
                let access = if writable { "writable" } else { "read-only" };
                let mut line = format!("- {label}: {status} {access} {path}");
                if let Some(scope) = source["scope"].as_str() {
                    write!(line, " scope={scope}").expect("writing to String cannot fail");
                }
                if let Some(reason) = source["skipped_reason"].as_str() {
                    write!(line, " reason={reason}").expect("writing to String cannot fail");
                }
                println!("{line}");
            }
        }
        _ => println!("memory sources: none"),
    }

    if let Some(md) = claude_md {
        println!("claude md contents:\n{md}");
    }
    Ok(())
}

pub(crate) async fn print_tools(client: &AppClient) -> Result<()> {
    let tools_value = client.list_tools().await.map_err(client_err)?;

    #[derive(Deserialize)]
    struct ToolEntry {
        name: String,
        summary: String,
        requires_tools_permission: bool,
        requires_network_permission: bool,
        #[serde(default)]
        provider_hidden: bool,
    }

    let tools: Vec<ToolEntry> = tools_value
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|v| serde_json::from_value(v).ok())
        .collect();

    for tool in &tools {
        let hidden = if tool.provider_hidden { " hidden" } else { "" };
        println!(
            "{:20} tools={} network={}{} {}",
            tool.name,
            tool.requires_tools_permission,
            tool.requires_network_permission,
            hidden,
            tool.summary
        );
    }
    Ok(())
}

pub(crate) async fn run_tool(
    client: &AppClient,
    session: Option<String>,
    name: String,
    input: String,
) -> Result<()> {
    let outcome = client
        .invoke_tool(&name, &input)
        .await
        .map_err(client_err)?;
    let summary = outcome["summary"].as_str().unwrap_or("");
    let output = outcome["output"].as_str().unwrap_or("");
    let tool_name = outcome["name"].as_str().unwrap_or(&name);
    println!("{summary}");
    if !output.is_empty() {
        println!("{output}");
    }
    if let Some(session_id) = session {
        let note = format!("Tool: {tool_name}\n{summary}\n{output}");
        client
            .record_system_message(&session_id, &note)
            .await
            .map_err(client_err)?;
    }
    Ok(())
}

pub(crate) async fn run_auth(
    app_server: AppServer,
    client: &AppClient,
    command: AuthCommand,
) -> Result<()> {
    match command {
        AuthCommand::Status => {
            let overview = client.auth_overview().await.map_err(client_err)?;
            let store_path = overview["store_path"].as_str().unwrap_or("");
            println!("store {store_path}");
            let entries = overview["entries"].as_array();
            match entries {
                Some(entries) if !entries.is_empty() => {
                    for entry in entries {
                        let provider = entry["provider"].as_str().unwrap_or("");
                        let method = entry["method"].as_str().unwrap_or("");
                        let persisted = entry["persisted"].as_bool().unwrap_or(false);
                        let active = entry["active"].as_bool().unwrap_or(false);
                        let usable = entry["usable"].as_bool().unwrap_or(false);
                        let source_summary = entry["source_summary"].as_str().unwrap_or("");
                        println!(
                            "{:10} {:12} {:8} {:8} {}",
                            provider,
                            method,
                            if persisted { "stored" } else { "env" },
                            if active {
                                "active"
                            } else if usable {
                                "ready"
                            } else {
                                "blocked"
                            },
                            source_summary
                        );
                    }
                }
                _ => println!("no auth metadata configured"),
            }
            Ok(())
        }
        AuthCommand::Login {
            provider,
            method,
            token,
            env_var,
            device_code,
        } => {
            if matches!(method, crate::args::CliAuthMethod::ChatGpt) {
                if provider.as_str() != "openai" {
                    bail!("--method chatgpt is only valid with --provider openai");
                }
                if token.is_some() || env_var.is_some() {
                    bail!("ChatGPT login does not accept --token or --env-var");
                }
                if device_code {
                    let session = app_server.start_chatgpt_device_login().await?;
                    println!("open {}", session.verification_uri);
                    println!("code {}", session.user_code);
                    println!(
                        "waiting for authorization; interval={}s",
                        session.interval_secs
                    );
                    io::stdout().flush()?;
                    let entry = app_server.complete_chatgpt_device_login(session).await?;
                    println!(
                        "signed in to {} via {} ({})",
                        entry.provider, entry.method, entry.source_summary
                    );
                    return Ok(());
                }

                let session = app_server.start_chatgpt_browser_login().await?;
                println!("open {}", session.authorization_url);
                println!("listening {}", session.redirect_uri);
                match launch_browser(&session.authorization_url) {
                    BrowserLaunch::Opened => println!("opened system browser"),
                    BrowserLaunch::Disabled => println!(
                        "browser auto-launch disabled (ORBCODE_NO_BROWSER); open the URL above"
                    ),
                    BrowserLaunch::Failed(error) => {
                        println!("could not open system browser ({error}); open the URL above")
                    }
                }
                io::stdout().flush()?;
                let entry = app_server.complete_chatgpt_browser_login(session).await?;
                println!(
                    "signed in to {} via {} ({})",
                    entry.provider, entry.method, entry.source_summary
                );
                return Ok(());
            }
            if device_code {
                bail!("--device-code requires --method chatgpt");
            }
            let provider_str = provider.as_str();
            let method_str: &str = match method {
                crate::args::CliAuthMethod::ApiKey => "api_key",
                crate::args::CliAuthMethod::OAuthDevice => "oauth_device",
                crate::args::CliAuthMethod::ChatGpt => unreachable!("handled above"),
            };
            let entry = client
                .auth_login(
                    provider_str,
                    method_str,
                    token.as_deref(),
                    env_var.as_deref(),
                )
                .await
                .map_err(client_err)?;
            let ep = entry["provider"].as_str().unwrap_or(provider_str);
            let em = entry["method"].as_str().unwrap_or(method_str);
            let es = entry["source_summary"].as_str().unwrap_or("");
            println!("stored auth metadata for {ep} via {em} ({es})");
            Ok(())
        }
        AuthCommand::Logout { provider } => {
            let provider_str = provider.map(super::args::CliProvider::as_str);
            let result = client.auth_logout(provider_str).await.map_err(client_err)?;
            let removed = result["removed"].as_u64().unwrap_or(0);
            println!("removed {removed} persisted auth entry(s)");
            Ok(())
        }
    }
}

pub(crate) async fn run_mcp(
    app_server: AppServer,
    client: &AppClient,
    command: McpCommand,
) -> Result<()> {
    match command {
        McpCommand::Capabilities => print_mcp_capabilities(client).await,
        McpCommand::Servers => print_mcp_servers(client).await,
        McpCommand::Diagnose { server_id } => {
            let checks = client
                .diagnose_mcp_server(&server_id)
                .await
                .map_err(client_err)?;
            let checks_arr = checks.as_array().cloned().unwrap_or_default();
            let mut has_fail = false;
            for check in &checks_arr {
                let status = check["status"].as_str().unwrap_or("");
                let name = check["name"].as_str().unwrap_or("");
                let detail = check["detail"].as_str().unwrap_or("").replace('\n', " ");
                println!("{status:4} {name:10} {detail}");
                if status.eq_ignore_ascii_case("fail") {
                    has_fail = true;
                }
            }
            if has_fail {
                bail!("MCP diagnostics detected failing checks")
            }
            Ok(())
        }
        McpCommand::Auth { command } => run_mcp_auth(app_server, command).await,
        McpCommand::Add {
            server_id,
            transport,
            endpoint,
            summary,
            auth,
            enabled,
        } => {
            let auth = McpAuth::parse(auth.as_deref())?;
            let summary = summary.unwrap_or_else(|| format!("{server_id} ({transport:?})"));
            let config = serde_json::to_value(McpServerConfig {
                id: server_id.clone(),
                transport: transport.into(),
                endpoint,
                args: Vec::new(),
                env: std::collections::BTreeMap::new(),
                cwd: None,
                headers: std::collections::BTreeMap::new(),
                enabled,
                status: if enabled {
                    orbcode_app_server::McpServerStatus::Ready
                } else {
                    orbcode_app_server::McpServerStatus::Disabled
                },
                error: None,
                summary,
                auth,
                trust: orbcode_app_server::McpServerTrust::Trusted,
                transport_type_hint: None,
                source: None,
            })?;
            client.upsert_mcp_server(config).await.map_err(client_err)?;
            println!("upserted MCP server `{server_id}`");
            Ok(())
        }
        McpCommand::Remove { server_id } => {
            let result = client
                .remove_mcp_server(&server_id)
                .await
                .map_err(client_err)?;
            let removed = result["removed"].as_bool().unwrap_or(false);
            println!(
                "{} MCP server `{server_id}`",
                if removed { "removed" } else { "did not find" }
            );
            Ok(())
        }
        McpCommand::Resources { server_id } => {
            let resources = client
                .list_mcp_resources(&server_id)
                .await
                .map_err(client_err)?;
            for resource in resources.as_array().unwrap_or(&Vec::new()) {
                let uri = resource["uri"].as_str().unwrap_or("");
                let mime_type = resource["mime_type"].as_str().unwrap_or("");
                let description = resource["description"].as_str().unwrap_or("");
                println!("{uri}  {mime_type}  {description}");
            }
            Ok(())
        }
        McpCommand::Read { server_id, uri } => {
            let resource = client
                .read_mcp_resource(&server_id, &uri)
                .await
                .map_err(client_err)?;
            let is_binary = resource["is_binary"].as_bool().unwrap_or(false);
            if is_binary {
                let blob = resource["blob"].as_str().unwrap_or_default();
                let mime_type = resource["mime_type"].as_str().unwrap_or("");
                println!(
                    "[binary {} {} base64 bytes]",
                    if mime_type.is_empty() {
                        "application/octet-stream"
                    } else {
                        mime_type
                    },
                    blob.len()
                );
                println!("{blob}");
            } else {
                let contents = resource["contents"].as_str().unwrap_or_default();
                println!("{contents}");
            }
            Ok(())
        }
        McpCommand::Prompts { server_id } => {
            let prompts = client
                .list_mcp_prompts(&server_id)
                .await
                .map_err(client_err)?;
            for prompt in prompts.as_array().unwrap_or(&Vec::new()) {
                let name = prompt["name"].as_str().unwrap_or("");
                let description = prompt["description"].as_str().unwrap_or("");
                let arguments = prompt["arguments"].as_array();
                let args = match arguments {
                    Some(args) if !args.is_empty() => {
                        let rendered = args
                            .iter()
                            .map(|arg| {
                                let n = arg["name"].as_str().unwrap_or("");
                                let required = arg["required"].as_bool().unwrap_or(false);
                                if required {
                                    format!("{n}*")
                                } else {
                                    n.to_string()
                                }
                            })
                            .collect::<Vec<_>>()
                            .join(", ");
                        format!("  args=[{rendered}]")
                    }
                    _ => String::new(),
                };
                println!("{name}  {description}{args}");
            }
            Ok(())
        }
        McpCommand::Prompt {
            server_id,
            prompt_name,
            arguments,
        } => {
            let arguments = match arguments.as_deref().map(str::trim) {
                Some(raw) if !raw.is_empty() => serde_json::from_str(raw).map_err(|error| {
                    anyhow::anyhow!("prompt arguments must be a JSON object: {error}")
                })?,
                _ => serde_json::json!({}),
            };
            let result = client
                .get_mcp_prompt(&server_id, &prompt_name, arguments)
                .await
                .map_err(client_err)?;
            let description = result["description"].as_str().unwrap_or("");
            if !description.is_empty() {
                println!("{description}");
            }
            if let Some(messages) = result["messages"].as_array() {
                for message in messages {
                    let role = message["role"].as_str().unwrap_or("unknown");
                    let content = &message["content"];
                    let is_binary = content["is_binary"].as_bool().unwrap_or(false);
                    if is_binary {
                        let binary = content["binary"].as_str().unwrap_or_default();
                        let mime_type = content["mime_type"].as_str().unwrap_or("");
                        println!(
                            "{role}: [binary {} {} base64 bytes]",
                            if mime_type.is_empty() {
                                "application/octet-stream"
                            } else {
                                mime_type
                            },
                            binary.len()
                        );
                    } else {
                        let text = content["text"].as_str().unwrap_or_default();
                        println!("{role}: {text}");
                    }
                }
            }
            Ok(())
        }
        McpCommand::Tools { server_id } => {
            let tools = client
                .list_mcp_tools(&server_id)
                .await
                .map_err(client_err)?;
            for tool in tools.as_array().unwrap_or(&Vec::new()) {
                let name = tool["name"].as_str().unwrap_or("");
                let summary = tool["summary"].as_str().unwrap_or("");
                println!("{name}  {summary}");
            }
            Ok(())
        }
        McpCommand::Call {
            server_id,
            tool_name,
            input,
            session,
        } => {
            let result = client
                .invoke_mcp_tool(&server_id, &tool_name, input.as_deref().unwrap_or("{}"))
                .await
                .map_err(client_err)?;
            let output = result["output"].as_str().unwrap_or("");
            println!("{output}");
            if let Some(session_id) = session {
                let rsid = result["server_id"].as_str().unwrap_or(&server_id);
                let rtn = result["tool_name"].as_str().unwrap_or(&tool_name);
                let note = format!("MCP Tool: {rsid}::{rtn}\n{output}");
                client
                    .record_system_message(&session_id, &note)
                    .await
                    .map_err(client_err)?;
            }
            Ok(())
        }
        McpCommand::Trust { server_id } => {
            client
                .set_mcp_server_trust(&server_id, "trusted")
                .await
                .map_err(client_err)?;
            println!("marked MCP server `{server_id}` as trusted");
            Ok(())
        }
        McpCommand::Distrust { server_id } => {
            client
                .set_mcp_server_trust(&server_id, "denied")
                .await
                .map_err(client_err)?;
            println!("marked MCP server `{server_id}` as denied");
            Ok(())
        }
        McpCommand::Untrust { server_id } => {
            client
                .set_mcp_server_trust(&server_id, "unknown")
                .await
                .map_err(client_err)?;
            println!("reset trust for MCP server `{server_id}` (next call will need approval)");
            Ok(())
        }
    }
}

async fn run_mcp_auth(app_server: AppServer, command: CliMcpAuthCommand) -> Result<()> {
    match command {
        CliMcpAuthCommand::Status { server_id } => {
            let overview = app_server.mcp_oauth_overview(server_id.as_deref()).await?;
            println!("store {}", overview.store_path.display());
            if overview.entries.is_empty() {
                println!("no MCP OAuth tokens configured");
                return Ok(());
            }
            for entry in overview.entries {
                let scopes = if entry.scopes.is_empty() {
                    "-".to_string()
                } else {
                    entry.scopes.join(",")
                };
                println!(
                    "{:20} {:8} refresh={} token_endpoint={} expires_at={} scopes={} {}",
                    entry.server_id,
                    if entry.usable {
                        "ready"
                    } else if entry.expired {
                        "expired"
                    } else {
                        "blocked"
                    },
                    entry.has_refresh_token,
                    entry.has_token_endpoint,
                    entry
                        .expires_at
                        .map_or_else(|| "-".to_string(), |value| value.to_string()),
                    scopes,
                    entry.source_summary
                );
            }
            Ok(())
        }
        CliMcpAuthCommand::Login {
            server_id,
            access_token,
            refresh_token,
            token_endpoint,
            client_id,
            expires_at,
            scopes,
        } => {
            let entry = app_server
                .store_mcp_oauth_token(
                    &server_id,
                    McpOAuthTokenInput {
                        access_token,
                        refresh_token,
                        token_endpoint,
                        client_id,
                        expires_at,
                        scopes,
                    },
                )
                .await?;
            println!(
                "stored MCP OAuth token for {} ({})",
                entry.server_id, entry.source_summary
            );
            Ok(())
        }
        CliMcpAuthCommand::DeviceLogin {
            server_id,
            device_authorization_endpoint,
            token_endpoint,
            client_id,
            scopes,
        } => {
            let session = app_server
                .start_mcp_oauth_device_login(
                    &server_id,
                    McpOAuthDeviceLoginInput {
                        device_authorization_endpoint,
                        token_endpoint,
                        client_id,
                        scopes,
                    },
                )
                .await?;
            println!("open {}", session.verification_uri);
            println!("code {}", session.user_code);
            if let Some(uri) = session.verification_uri_complete.as_deref() {
                println!("direct {uri}");
            }
            println!(
                "waiting for authorization; expires_at={} interval={}s",
                session.expires_at, session.interval_secs
            );
            io::stdout().flush()?;
            let entry = app_server.complete_mcp_oauth_device_login(session).await?;
            println!(
                "stored MCP OAuth token for {} ({})",
                entry.server_id, entry.source_summary
            );
            Ok(())
        }
        CliMcpAuthCommand::BrowserLogin {
            server_id,
            authorization_endpoint,
            token_endpoint,
            client_id,
            registration_endpoint,
            scopes,
            redirect_port,
        } => {
            let session = app_server
                .start_mcp_oauth_browser_login(
                    &server_id,
                    McpOAuthBrowserLoginInput {
                        authorization_endpoint,
                        token_endpoint,
                        client_id,
                        registration_endpoint,
                        scopes,
                        redirect_port,
                    },
                )
                .await?;
            println!("open {}", session.authorization_url);
            println!("listening {}", session.redirect_uri);
            match launch_browser(&session.authorization_url) {
                BrowserLaunch::Opened => println!("opened system browser"),
                BrowserLaunch::Disabled => {
                    println!(
                        "browser auto-launch disabled (ORBCODE_NO_BROWSER); open the URL above"
                    )
                }
                BrowserLaunch::Failed(error) => {
                    println!("could not open system browser ({error}); open the URL above")
                }
            }
            io::stdout().flush()?;
            let entry = app_server.complete_mcp_oauth_browser_login(session).await?;
            println!(
                "stored MCP OAuth token for {} ({})",
                entry.server_id, entry.source_summary
            );
            Ok(())
        }
        CliMcpAuthCommand::Logout { server_id } => {
            let removed = app_server.logout_mcp_oauth_token(&server_id).await?;
            println!(
                "{} MCP OAuth token for `{server_id}`",
                if removed { "removed" } else { "did not find" }
            );
            Ok(())
        }
    }
}

pub(crate) async fn run_doctor(client: &AppClient, command: Option<DoctorCommand>) -> Result<()> {
    match command {
        None => run_doctor_report(client).await,
        Some(DoctorCommand::CleanupOrphans {
            dry_run,
            yes,
            stale_running_days,
        }) => run_doctor_cleanup_orphans(client, dry_run, yes, stale_running_days).await,
    }
}

async fn run_doctor_report(client: &AppClient) -> Result<()> {
    let report = client.doctor_report().await.map_err(client_err)?;

    #[derive(Deserialize)]
    struct DoctorCheckValue {
        name: String,
        status: String,
        detail: String,
    }

    let checks_value = report["checks"].as_array().cloned().unwrap_or_default();
    let mut checks: Vec<DoctorCheckValue> = checks_value
        .into_iter()
        .filter_map(|v| serde_json::from_value(v).ok())
        .collect();

    // Prepend build_info check.
    checks.insert(
        0,
        DoctorCheckValue {
            name: "build_info".to_string(),
            status: "Pass".to_string(),
            detail: crate::build_info::doctor_detail(),
        },
    );

    let (mut pass, mut warn, mut fail) = (0usize, 0usize, 0usize);
    for check in &checks {
        match check.status.as_str() {
            "Pass" => pass += 1,
            "Warn" => warn += 1,
            "Fail" => fail += 1,
            _ => {}
        }
    }
    println!("doctor summary: pass={pass} warn={warn} fail={fail}");
    for check in &checks {
        let status_label = match check.status.as_str() {
            "Pass" => "PASS",
            "Warn" => "WARN",
            "Fail" => "FAIL",
            other => other,
        };
        println!(
            "{:4} {:18} {}",
            status_label,
            check.name,
            check.detail.replace('\n', " ")
        );
    }
    if fail > 0 {
        bail!("doctor detected failing checks")
    }
    Ok(())
}

async fn run_doctor_cleanup_orphans(
    client: &AppClient,
    dry_run: bool,
    yes: bool,
    stale_running_days: Option<u64>,
) -> Result<()> {
    if dry_run && yes {
        bail!("choose either --dry-run or --yes, not both");
    }
    let stale_running_cutoff_ms = stale_running_days
        .map(|days| (chrono::Utc::now() - chrono::Duration::days(days as i64)).timestamp_millis());
    let effective_dry_run = !yes;
    let result = client
        .cleanup_orphan_child_sessions(effective_dry_run, stale_running_cutoff_ms)
        .await
        .map_err(client_err)?;

    #[derive(Deserialize)]
    struct CleanupResult {
        dry_run: bool,
        scoped_cwds: Vec<String>,
        inspected_metadata: usize,
        orphan_metadata: usize,
        eligible_metadata: usize,
        stale_running_metadata: usize,
        skipped_running_metadata: usize,
        removed_metadata: usize,
        removed_transcripts: usize,
        orphan_child_session_ids: Vec<String>,
    }

    let result: CleanupResult = serde_json::from_value(result)?;
    let mode = if result.dry_run { "dry-run" } else { "applied" };
    println!("doctor orphan cleanup: {mode}");
    println!("scoped cwd(s): {}", result.scoped_cwds.join(", "));
    println!("inspected child metadata: {}", result.inspected_metadata);
    println!(
        "orphan child metadata: {} (eligible {}, stale running {}, running skipped {})",
        result.orphan_metadata,
        result.eligible_metadata,
        result.stale_running_metadata,
        result.skipped_running_metadata
    );
    println!(
        "removed: {} metadata, {} transcript(s)",
        result.removed_metadata, result.removed_transcripts
    );

    if !result.orphan_child_session_ids.is_empty() {
        println!("eligible child session id(s):");
        for child_id in result.orphan_child_session_ids.iter().take(20) {
            println!("  {child_id}");
        }
        if result.orphan_child_session_ids.len() > 20 {
            println!("  ... {} more", result.orphan_child_session_ids.len() - 20);
        }
    }

    if result.dry_run && result.eligible_metadata > 0 {
        let stale_arg = stale_running_days
            .map(|days| format!(" --stale-running-days {days}"))
            .unwrap_or_default();
        println!(
            "rerun with `orbcode doctor cleanup-orphans --yes{stale_arg}` to remove eligible artifacts"
        );
    }

    Ok(())
}

pub(crate) async fn print_advanced_capabilities(client: &AppClient) -> Result<()> {
    let capabilities = client.advanced_capabilities().await.map_err(client_err)?;
    for capability in capabilities.as_array().unwrap_or(&Vec::new()) {
        let name = capability["name"].as_str().unwrap_or("");
        let summary = capability["summary"].as_str().unwrap_or("");
        let status = capability["status"].as_str().unwrap_or("");
        let marker = if status.eq_ignore_ascii_case("implemented") {
            "active"
        } else {
            "deferred"
        };
        println!("{name:24} {marker:10} {summary}");
    }
    Ok(())
}

async fn print_mcp_capabilities(client: &AppClient) -> Result<()> {
    let capabilities = client.mcp_capabilities().await.map_err(client_err)?;
    for capability in capabilities.as_array().unwrap_or(&Vec::new()) {
        let transport = capability["transport"].as_str().unwrap_or("");
        let enabled = capability["enabled"].as_bool().unwrap_or(false);
        let note = capability["note"].as_str().unwrap_or("");
        println!("{transport:10} enabled={enabled} {note}");
    }
    Ok(())
}

async fn print_mcp_servers(client: &AppClient) -> Result<()> {
    let servers = client.list_mcp_servers().await.map_err(client_err)?;
    let servers_arr = servers.as_array().cloned().unwrap_or_default();
    if servers_arr.is_empty() {
        println!("No MCP servers configured.");
        println!(
            "Add one via ~/.claude/settings.json, .mcp.json, or `orbcode mcp add <id> <command>`."
        );
        return Ok(());
    }
    for server in &servers_arr {
        let id = server["id"].as_str().unwrap_or("");
        let transport = server["transport"].as_str().unwrap_or("");
        let status = server["status"].as_str().unwrap_or("");
        let trust = server["trust"].as_str().unwrap_or("");
        let enabled = server["enabled"].as_bool().unwrap_or(false);
        let auth_summary = server["auth"]
            .as_str()
            .or_else(|| server["auth"]["summary"].as_str())
            .unwrap_or("none");
        let endpoint = server["endpoint"].as_str().unwrap_or("");
        let error = server["error"]
            .as_str()
            .map(|e| format!(" error={e}"))
            .unwrap_or_default();
        println!(
            "{id:12} {transport:10} status={status} trust={trust} enabled={enabled} auth={auth_summary} {endpoint}{error}",
        );
    }
    Ok(())
}

enum BrowserLaunch {
    Opened,
    Disabled,
    Failed(String),
}

fn launch_browser(url: &str) -> BrowserLaunch {
    if std::env::var_os("ORBCODE_NO_BROWSER").is_some_and(|value| !value.is_empty()) {
        return BrowserLaunch::Disabled;
    }

    use std::process::{Command, Stdio};
    let mut command = if cfg!(target_os = "macos") {
        let mut command = Command::new("open");
        command.arg(url);
        command
    } else if cfg!(target_os = "windows") {
        let mut command = Command::new("cmd");
        command.args(["/C", "start", "", url]);
        command
    } else {
        let mut command = Command::new("xdg-open");
        command.arg(url);
        command
    };

    match command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
    {
        Ok(status) if status.success() => BrowserLaunch::Opened,
        Ok(status) => BrowserLaunch::Failed(format!("launcher exited with {status}")),
        Err(error) => BrowserLaunch::Failed(error.to_string()),
    }
}

fn short_id(session_id: &str) -> &str {
    session_id.get(..8).unwrap_or(session_id)
}

fn capability_list(capabilities: &[String]) -> String {
    if capabilities.is_empty() {
        "none".to_string()
    } else {
        capabilities.join(",")
    }
}
