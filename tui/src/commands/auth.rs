use anyhow::Result;
use orbcode_app_server_client::AppClient;
use orbcode_config::{AuthMethod, AuthOverview, AuthStatusEntry};
use orbcode_protocol::ProviderId;

use crate::render::slash_output::render_auth_overview;

pub(crate) async fn run_login_slash_command(
    app_server: &AppClient,
    _client: Option<&AppClient>,
    args: &str,
) -> Result<(String, Option<String>)> {
    let args = args.trim();
    if args.is_empty() || args == "status" {
        let value = app_server
            .auth_overview()
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let overview: AuthOverview = serde_json::from_value(value)?;
        return Ok((
            "Auth status loaded.".to_string(),
            Some(render_auth_overview(&overview)),
        ));
    }

    let (provider, env_var) = parse_login_env_var_args(args)?;
    let value = app_server
        .auth_login(
            provider.as_str(),
            &AuthMethod::ApiKey.to_string(),
            None,
            Some(env_var.as_str()),
        )
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let entry: AuthStatusEntry = serde_json::from_value(value)?;
    let value = app_server
        .auth_overview()
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let overview: AuthOverview = serde_json::from_value(value)?;
    Ok((
        format!(
            "Stored auth metadata for {} via {}.",
            entry.provider, entry.source_summary
        ),
        Some(render_auth_overview(&overview)),
    ))
}

fn parse_login_env_var_args(args: &str) -> Result<(ProviderId, String)> {
    let tokens = args.split_whitespace().collect::<Vec<_>>();
    let mut provider = None;
    let mut env_var = None;
    let mut index = 0usize;
    while index < tokens.len() {
        match tokens[index] {
            "--provider" => {
                index += 1;
                let value = tokens
                    .get(index)
                    .ok_or_else(|| anyhow::anyhow!("usage: /login <provider> --env-var VAR"))?;
                provider = Some(parse_provider_argument(value)?);
            }
            "--env-var" => {
                index += 1;
                let value = tokens
                    .get(index)
                    .ok_or_else(|| anyhow::anyhow!("usage: /login <provider> --env-var VAR"))?;
                env_var = Some((*value).to_string());
            }
            "--token" => {
                return Err(anyhow::anyhow!(
                    "TUI /login does not accept --token because slash commands are recorded; use /login <provider> --env-var VAR or `orbcode auth login --token`."
                ));
            }
            token if token.starts_with("--") => {
                return Err(anyhow::anyhow!(
                    "unknown login option `{token}`. usage: /login <provider> --env-var VAR"
                ));
            }
            value => {
                if provider.is_some() {
                    return Err(anyhow::anyhow!("usage: /login <provider> --env-var VAR"));
                }
                provider = Some(parse_provider_argument(value)?);
            }
        }
        index += 1;
    }

    let provider =
        provider.ok_or_else(|| anyhow::anyhow!("usage: /login <provider> --env-var VAR"))?;
    let env_var = env_var
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("usage: /login <provider> --env-var VAR"))?;
    Ok((provider, env_var))
}

pub(crate) async fn run_logout_slash_command(
    app_server: &AppClient,
    _client: Option<&AppClient>,
    args: &str,
) -> Result<(String, Option<String>)> {
    let provider = parse_logout_provider_arg(args)?;
    let value = app_server
        .auth_logout(provider.map(orbcode_protocol::ProviderId::as_str))
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let removed = value["removed"].as_u64().unwrap_or(0) as usize;
    let value = app_server
        .auth_overview()
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let overview: AuthOverview = serde_json::from_value(value)?;
    let scope = provider.map_or_else(
        || "all providers".to_string(),
        |provider| provider.to_string(),
    );
    Ok((
        format!("Removed {removed} persisted auth entry(s) for {scope}."),
        Some(render_auth_overview(&overview)),
    ))
}

fn parse_logout_provider_arg(args: &str) -> Result<Option<ProviderId>> {
    let args = args.trim();
    if args.is_empty() {
        return Ok(None);
    }
    let tokens = args.split_whitespace().collect::<Vec<_>>();
    match tokens.as_slice() {
        [provider] => Ok(Some(parse_provider_argument(provider)?)),
        ["--provider", provider] => Ok(Some(parse_provider_argument(provider)?)),
        _ => Err(anyhow::anyhow!("usage: /logout [provider]")),
    }
}

fn parse_provider_argument(value: &str) -> Result<ProviderId> {
    ProviderId::parse(value).ok_or_else(|| {
        anyhow::anyhow!(
            "unknown provider `{value}`. expected one of: anthropic, openai, gemini, grok"
        )
    })
}
