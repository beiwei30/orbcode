use std::collections::HashMap;
use std::env;
use std::fmt;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use orbcode_protocol::ProviderId;
use serde::{Deserialize, Serialize};

use crate::ConfigError;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuthMethod {
    ApiKey,
    OAuthDevice,
}

impl fmt::Display for AuthMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::ApiKey => "api_key",
            Self::OAuthDevice => "oauth_device",
        };
        f.write_str(value)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AuthStatusEntry {
    pub provider: ProviderId,
    pub method: AuthMethod,
    pub source_summary: String,
    pub persisted: bool,
    pub usable: bool,
    pub active: bool,
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AuthOverview {
    pub store_path: PathBuf,
    pub entries: Vec<AuthStatusEntry>,
}

impl AuthOverview {
    pub fn has_provider(&self, provider: ProviderId) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.provider == provider && entry.usable)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ClaudeAiOAuth {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: Option<i64>,
    pub scopes: Vec<String>,
    pub subscription_type: Option<String>,
}

#[derive(Clone)]
pub struct AuthManager {
    home_dir: PathBuf,
    /// Test-only seal mirroring `AppConfig::env_overrides`. Empty entries
    /// block the matching `std::env::var` lookup so a developer's shell
    /// env never leaks into auth fixtures.
    env_overrides: HashMap<String, String>,
    /// When set by managed policy (`forceLoginMethod`), `login` rejects any
    /// attempt to authenticate with a different method.
    forced_method: Option<AuthMethod>,
}

/// Map a managed `forceLoginMethod` policy string onto an [`AuthMethod`].
/// `claudeai` forces the OAuth device flow; `console`/`apiKey` force API keys.
/// Unknown values yield `None` (no enforcement).
pub fn parse_forced_login_method(value: &str) -> Option<AuthMethod> {
    match value.trim() {
        "claudeai" | "claude_ai" | "claude-ai" | "oauth" | "oauth_device" => {
            Some(AuthMethod::OAuthDevice)
        }
        "console" | "apiKey" | "api_key" | "api-key" => Some(AuthMethod::ApiKey),
        _ => None,
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct StoredAuthEntry {
    provider: ProviderId,
    method: AuthMethod,
    source: StoredAuthSource,
    updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum StoredAuthSource {
    EnvVar { env_var: String },
    StoredApiKey { api_key: String },
    StoredAuthToken { auth_token: String },
    StoredHint { hint: String },
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
struct StoredAuthState {
    entries: Vec<StoredAuthEntry>,
}

impl AuthManager {
    pub fn new(home_dir: PathBuf) -> Self {
        Self {
            home_dir,
            env_overrides: HashMap::new(),
            forced_method: None,
        }
    }

    /// Constrain `login` to the method the managed policy forces. A `None`
    /// argument leaves login unconstrained.
    pub fn with_forced_login_method(mut self, forced_method: Option<AuthMethod>) -> Self {
        self.forced_method = forced_method;
        self
    }

    /// Apply a `resolve_env`-style seal to env-derived auth entries. Empty
    /// values mark a key as "explicitly cleared" so the matching
    /// `std::env::var` lookup is skipped. Intended for tests that need
    /// deterministic auth state regardless of the developer's shell.
    pub fn with_env_overrides(mut self, env_overrides: HashMap<String, String>) -> Self {
        self.env_overrides = env_overrides;
        self
    }

    pub async fn overview(&self) -> Result<AuthOverview, ConfigError> {
        let store = self.load_state().await?;
        let mut entries = store
            .entries
            .into_iter()
            .map(|entry| AuthStatusEntry {
                provider: entry.provider,
                method: entry.method,
                source_summary: stored_source_summary(&entry.source),
                persisted: true,
                usable: stored_source_usable(&entry.source),
                active: false,
                updated_at: Some(entry.updated_at),
            })
            .collect::<Vec<_>>();
        entries.extend(env_entries(&self.env_overrides));
        if let Some(entry) = claude_ai_oauth_entry(&self.home_dir).await? {
            entries.push(entry);
        }
        mark_active_entries(&mut entries);
        entries.sort_by(|left, right| {
            left.provider
                .as_str()
                .cmp(right.provider.as_str())
                .then(left.source_summary.cmp(&right.source_summary))
        });
        Ok(AuthOverview {
            store_path: self.store_path(),
            entries,
        })
    }

    pub async fn login(
        &self,
        provider: ProviderId,
        method: AuthMethod,
        token: Option<String>,
        env_var: Option<String>,
    ) -> Result<AuthStatusEntry, ConfigError> {
        if let Some(forced) = self.forced_method
            && forced != method
        {
            return Err(ConfigError::Config(format!(
                "login method is locked by managed policy to {forced}; cannot log in with {method}"
            )));
        }
        let source = match (
            token
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty()),
            env_var
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty()),
        ) {
            (Some(token), None) => match method {
                AuthMethod::ApiKey => StoredAuthSource::StoredApiKey { api_key: token },
                AuthMethod::OAuthDevice => StoredAuthSource::StoredAuthToken { auth_token: token },
            },
            (None, Some(env_var)) => StoredAuthSource::EnvVar { env_var },
            (None, None) => {
                return Err(ConfigError::Config(
                    "auth login requires either --token or --env-var".into(),
                ));
            }
            (Some(_), Some(_)) => {
                return Err(ConfigError::Config(
                    "auth login accepts only one of --token or --env-var".into(),
                ));
            }
        };

        let mut store = self.load_state().await?;
        let entry = StoredAuthEntry {
            provider,
            method,
            source,
            updated_at: Utc::now(),
        };

        store
            .entries
            .retain(|existing| existing.provider != provider);
        store.entries.push(entry.clone());
        self.save_state(&store).await?;

        Ok(AuthStatusEntry {
            provider: entry.provider,
            method: entry.method,
            source_summary: stored_source_summary(&entry.source),
            persisted: true,
            usable: stored_source_usable(&entry.source),
            active: true,
            updated_at: Some(entry.updated_at),
        })
    }

    pub async fn logout(&self, provider: Option<ProviderId>) -> Result<usize, ConfigError> {
        let mut store = self.load_state().await?;
        let previous = store.entries.len();
        store.entries.retain(|entry| match provider {
            Some(provider) => entry.provider != provider,
            None => false,
        });
        let mut removed = previous.saturating_sub(store.entries.len());
        if matches!(provider, None | Some(ProviderId::Anthropic))
            && remove_claude_ai_oauth(&self.home_dir).await?
        {
            removed += 1;
        }
        self.save_state(&store).await?;
        Ok(removed)
    }

    fn store_path(&self) -> PathBuf {
        self.home_dir.join("auth.json")
    }

    async fn load_state(&self) -> Result<StoredAuthState, ConfigError> {
        let path = self.store_path();
        tokio::fs::create_dir_all(&self.home_dir).await?;
        if !tokio::fs::try_exists(&path).await? {
            return Ok(StoredAuthState::default());
        }
        let contents = tokio::fs::read_to_string(path).await?;
        Ok(serde_json::from_str(&contents)?)
    }

    async fn save_state(&self, store: &StoredAuthState) -> Result<(), ConfigError> {
        let path = self.store_path();
        let tmp_path = path.with_extension("json.tmp");
        tokio::fs::create_dir_all(&self.home_dir).await?;
        tokio::fs::write(&tmp_path, serde_json::to_string_pretty(store)?).await?;
        #[cfg(unix)]
        tokio::fs::set_permissions(
            &tmp_path,
            std::os::unix::fs::PermissionsExt::from_mode(0o600),
        )
        .await?;
        tokio::fs::rename(tmp_path, path).await?;
        Ok(())
    }
}

pub fn stored_api_key(home_dir: &Path, provider: ProviderId) -> Option<String> {
    let store = load_stored_auth_state(home_dir)?;
    store
        .entries
        .into_iter()
        .find(|entry| entry.provider == provider)
        .and_then(|entry| match entry.source {
            StoredAuthSource::StoredApiKey { api_key } => Some(api_key),
            _ => None,
        })
}

pub fn stored_auth_token(home_dir: &Path, provider: ProviderId) -> Option<String> {
    let store = load_stored_auth_state(home_dir)?;
    store
        .entries
        .into_iter()
        .find(|entry| entry.provider == provider)
        .and_then(|entry| match entry.source {
            StoredAuthSource::StoredAuthToken { auth_token } => Some(auth_token),
            _ => None,
        })
}

fn load_stored_auth_state(home_dir: &Path) -> Option<StoredAuthState> {
    let contents = std::fs::read_to_string(home_dir.join("auth.json")).ok()?;
    serde_json::from_str(&contents).ok()
}

fn stored_source_summary(source: &StoredAuthSource) -> String {
    match source {
        StoredAuthSource::EnvVar { env_var } => format!("env:{env_var}"),
        StoredAuthSource::StoredApiKey { api_key } => format!("stored:{}", mask_secret(api_key)),
        StoredAuthSource::StoredAuthToken { auth_token } => {
            format!("stored:{}", mask_secret(auth_token))
        }
        StoredAuthSource::StoredHint { hint } => format!("stored:{hint} (metadata only)"),
    }
}

fn stored_source_usable(source: &StoredAuthSource) -> bool {
    !matches!(source, StoredAuthSource::StoredHint { .. })
}

struct AuthEnvMatch {
    matched_key: String,
}

fn resolve_auth_env(env_overrides: &HashMap<String, String>, key: &str) -> Option<AuthEnvMatch> {
    let keys = crate::env_compat::resolve_keys(key);
    let mut sealed = false;
    for k in &keys {
        if let Some(value) = env_overrides.get(*k) {
            if !value.trim().is_empty() {
                return Some(AuthEnvMatch {
                    matched_key: k.to_string(),
                });
            }
            sealed = true;
        }
    }
    if sealed {
        return None;
    }
    for k in &keys {
        if let Some(value) = env::var(k).ok().filter(|v| !v.trim().is_empty()) {
            let _ = value;
            return Some(AuthEnvMatch {
                matched_key: k.to_string(),
            });
        }
    }
    None
}

fn env_entries(env_overrides: &HashMap<String, String>) -> Vec<AuthStatusEntry> {
    env_auth_sources()
        .into_iter()
        .filter_map(|(provider, env_var)| {
            resolve_auth_env(env_overrides, env_var).map(|m| AuthStatusEntry {
                provider,
                method: env_auth_method(env_var),
                source_summary: format!("env:{}", m.matched_key),
                persisted: false,
                usable: true,
                active: false,
                updated_at: None,
            })
        })
        .collect()
}

fn mark_active_entries(entries: &mut [AuthStatusEntry]) {
    for provider in ProviderId::ALL {
        let active_index = entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.provider == provider && entry.usable)
            .min_by_key(|(_, entry)| auth_source_precedence(entry))
            .map(|(index, _)| index);
        if let Some(active_index) = active_index {
            entries[active_index].active = true;
        }
    }
}

fn auth_source_precedence(entry: &AuthStatusEntry) -> u8 {
    match entry.provider {
        ProviderId::Anthropic => anthropic_source_precedence(entry),
        ProviderId::OpenAi => openai_source_precedence(entry),
        ProviderId::Gemini => {
            if env_summary_matches(&entry.source_summary, "GEMINI_API_KEY") {
                0
            } else {
                100
            }
        }
        ProviderId::Grok => {
            if env_summary_matches(&entry.source_summary, "XAI_API_KEY") {
                0
            } else if env_summary_matches(&entry.source_summary, "GROK_API_KEY") {
                1
            } else {
                100
            }
        }
        _ => 100,
    }
}

fn env_summary_matches(summary: &str, legacy_key: &str) -> bool {
    let Some(key) = summary.strip_prefix("env:") else {
        return false;
    };
    let group = crate::env_compat::resolve_keys(legacy_key);
    group.contains(&key)
}

fn anthropic_source_precedence(entry: &AuthStatusEntry) -> u8 {
    if env_summary_matches(&entry.source_summary, "ANTHROPIC_AUTH_TOKEN") {
        return 0;
    }
    if env_summary_matches(&entry.source_summary, "ANTHROPIC_API_KEY") {
        return 1;
    }
    if env_summary_matches(&entry.source_summary, "CLAUDE_CODE_OAUTH_TOKEN") {
        return 3;
    }
    if entry.method == AuthMethod::ApiKey && entry.source_summary.starts_with("stored:") {
        return 2;
    }
    if entry
        .source_summary
        .starts_with("credentials:claude.ai oauth")
    {
        return 4;
    }
    if entry.method == AuthMethod::OAuthDevice && entry.source_summary.starts_with("stored:") {
        return 5;
    }
    100
}

fn openai_source_precedence(entry: &AuthStatusEntry) -> u8 {
    if env_summary_matches(&entry.source_summary, "OPENAI_API_KEY") {
        return 0;
    }
    if entry.method == AuthMethod::ApiKey && entry.source_summary.starts_with("stored:") {
        return 1;
    }
    100
}

fn env_auth_sources() -> Vec<(ProviderId, &'static str)> {
    vec![
        (ProviderId::Anthropic, "ANTHROPIC_API_KEY"),
        (ProviderId::Anthropic, "ANTHROPIC_AUTH_TOKEN"),
        (ProviderId::Anthropic, "CLAUDE_CODE_OAUTH_TOKEN"),
        (ProviderId::OpenAi, "OPENAI_API_KEY"),
        (ProviderId::Gemini, "GEMINI_API_KEY"),
        (ProviderId::Grok, "XAI_API_KEY"),
        (ProviderId::Grok, "GROK_API_KEY"),
    ]
}

fn env_auth_method(env_var: &str) -> AuthMethod {
    match env_var {
        "ANTHROPIC_AUTH_TOKEN" | "CLAUDE_CODE_OAUTH_TOKEN" => AuthMethod::OAuthDevice,
        _ => AuthMethod::ApiKey,
    }
}

pub fn load_claude_ai_oauth(home_dir: &Path) -> Option<ClaudeAiOAuth> {
    let contents = std::fs::read_to_string(home_dir.join(".credentials.json")).ok()?;
    parse_claude_ai_oauth(&contents)
}

pub fn usable_claude_ai_oauth_token(home_dir: &Path) -> Option<String> {
    let oauth = load_claude_ai_oauth(home_dir)?;
    if oauth_usable_for_inference(&oauth) {
        Some(oauth.access_token)
    } else {
        None
    }
}

async fn claude_ai_oauth_entry(home_dir: &Path) -> Result<Option<AuthStatusEntry>, ConfigError> {
    let path = home_dir.join(".credentials.json");
    if !tokio::fs::try_exists(&path).await? {
        return Ok(None);
    }
    let contents = tokio::fs::read_to_string(path).await?;
    let Some(oauth) = parse_claude_ai_oauth(&contents) else {
        return Ok(None);
    };
    let usable = oauth_usable_for_inference(&oauth);
    let mut status = if oauth_expired(oauth.expires_at) {
        "expired".to_string()
    } else if !has_scope(&oauth, "user:inference") {
        "missing user:inference scope".to_string()
    } else {
        "ready".to_string()
    };
    if !has_scope(&oauth, "user:profile") {
        status.push_str("; missing user:profile scope");
    }
    if let Some(subscription_type) = oauth.subscription_type.as_deref() {
        write!(status, "; subscription:{subscription_type}")
            .expect("writing to String cannot fail");
    }
    Ok(Some(AuthStatusEntry {
        provider: ProviderId::Anthropic,
        method: AuthMethod::OAuthDevice,
        source_summary: format!("credentials:claude.ai oauth ({status})"),
        persisted: true,
        usable,
        active: false,
        updated_at: None,
    }))
}

async fn remove_claude_ai_oauth(home_dir: &Path) -> Result<bool, ConfigError> {
    let path = home_dir.join(".credentials.json");
    if !tokio::fs::try_exists(&path).await? {
        return Ok(false);
    }
    let contents = tokio::fs::read_to_string(&path).await?;
    let mut value = serde_json::from_str::<serde_json::Value>(&contents)?;
    let Some(object) = value.as_object_mut() else {
        return Ok(false);
    };
    if object.remove("claudeAiOauth").is_none() {
        return Ok(false);
    }
    if object.is_empty() {
        tokio::fs::remove_file(path).await?;
    } else {
        let tmp_path = path.with_extension("json.tmp");
        let permissions = tokio::fs::metadata(&path)
            .await
            .ok()
            .map(|metadata| metadata.permissions());
        tokio::fs::write(&tmp_path, serde_json::to_string_pretty(&value)?).await?;
        if let Some(permissions) = permissions {
            tokio::fs::set_permissions(&tmp_path, permissions).await?;
        }
        tokio::fs::rename(tmp_path, path).await?;
    }
    Ok(true)
}

/// On-disk shape of `~/.claude/.credentials.json`. Only the `claudeAiOauth`
/// key is typed; the rest is preserved as-is by `remove_claude_ai_oauth`.
#[derive(Deserialize)]
struct CredentialsFile {
    #[serde(rename = "claudeAiOauth")]
    claude_ai_oauth: Option<RawClaudeAiOAuth>,
}

/// Raw deserialization target for the `claudeAiOauth` object inside the
/// credentials file. Fields are optional to tolerate partial/malformed entries.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawClaudeAiOAuth {
    access_token: Option<String>,
    refresh_token: Option<String>,
    expires_at: Option<i64>,
    #[serde(default)]
    scopes: Vec<String>,
    subscription_type: Option<String>,
}

fn parse_claude_ai_oauth(contents: &str) -> Option<ClaudeAiOAuth> {
    let file: CredentialsFile = serde_json::from_str(contents).ok()?;
    let raw = file.claude_ai_oauth?;
    let access_token = raw
        .access_token
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())?;
    let refresh_token = raw
        .refresh_token
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    Some(ClaudeAiOAuth {
        access_token,
        refresh_token,
        expires_at: raw.expires_at,
        scopes: raw.scopes,
        subscription_type: raw.subscription_type,
    })
}

fn oauth_usable_for_inference(oauth: &ClaudeAiOAuth) -> bool {
    !oauth_expired(oauth.expires_at) && has_scope(oauth, "user:inference")
}

fn oauth_expired(expires_at: Option<i64>) -> bool {
    let Some(expires_at) = expires_at else {
        return false;
    };
    let expires_with_buffer = Utc::now().timestamp_millis() + 5 * 60 * 1000;
    expires_with_buffer >= expires_at
}

fn has_scope(oauth: &ClaudeAiOAuth, scope: &str) -> bool {
    oauth.scopes.iter().any(|candidate| candidate == scope)
}

fn mask_secret(secret: &str) -> String {
    // Slice by chars, not bytes: a multibyte token (e.g. a stored key with
    // non-ASCII) would panic on `&secret[..4]` / `&secret[len-2..]` at a
    // non-char-boundary offset.
    let chars: Vec<char> = secret.chars().collect();
    if chars.len() <= 6 {
        return "*".repeat(chars.len());
    }
    let prefix: String = chars[..4].iter().collect();
    let suffix: String = chars[chars.len() - 2..].iter().collect();
    format!("{prefix}***{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration as ChronoDuration;

    #[test]
    fn mask_secret_is_char_boundary_safe() {
        // A multibyte token must not panic on `&secret[..4]` / `&secret[len-2..]`.
        let masked = mask_secret("sk-café-secret-café");
        assert!(masked.starts_with("sk-c"));
        assert!(masked.contains("***"));
        // Short (<= 6 chars) multibyte secret masks by char count, not byte len.
        assert_eq!(mask_secret("café"), "****");
    }

    fn temp_home(label: &str) -> PathBuf {
        let home =
            std::env::temp_dir().join(format!("orbcode-auth-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).expect("create home");
        home
    }

    /// Seal the auth-related env vars so a developer's shell never leaks
    /// into tests that assert on `AuthManager::overview` shape.
    fn sealed_auth_env_overrides() -> HashMap<String, String> {
        let legacy = [
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "CLAUDE_CODE_OAUTH_TOKEN",
            "OPENAI_API_KEY",
            "GEMINI_API_KEY",
            "XAI_API_KEY",
            "GROK_API_KEY",
        ];
        legacy
            .into_iter()
            .map(|key| (key.to_string(), String::new()))
            .chain(crate::env_compat::canonical_keys().map(|key| (key.to_string(), String::new())))
            .collect()
    }

    fn sealed_auth_manager(home: PathBuf) -> AuthManager {
        AuthManager::new(home).with_env_overrides(sealed_auth_env_overrides())
    }

    #[tokio::test]
    async fn stores_masked_auth_entries() {
        let manager = AuthManager::new(temp_home("store"));
        let entry = manager
            .login(
                ProviderId::Anthropic,
                AuthMethod::ApiKey,
                Some("sk-ant-secret".to_string()),
                None,
            )
            .await
            .expect("login");
        assert!(entry.source_summary.contains("stored:sk-a***et"));

        let overview = manager.overview().await.expect("overview");
        assert!(overview.has_provider(ProviderId::Anthropic));
    }

    #[tokio::test]
    async fn token_login_persists_usable_api_key() {
        let home = temp_home("api-key-token-login");
        let manager = AuthManager::new(home.clone());

        let entry = manager
            .login(
                ProviderId::Anthropic,
                AuthMethod::ApiKey,
                Some("sk-ant-secret".to_string()),
                None,
            )
            .await
            .expect("login");

        assert_eq!(entry.source_summary, "stored:sk-a***et");
        assert!(entry.usable);
        assert_eq!(
            stored_api_key(&home, ProviderId::Anthropic).as_deref(),
            Some("sk-ant-secret")
        );
    }

    #[tokio::test]
    async fn oauth_token_login_persists_usable_bearer_token() {
        let home = temp_home("oauth-token-login");
        let manager = AuthManager::new(home.clone());

        let entry = manager
            .login(
                ProviderId::Anthropic,
                AuthMethod::OAuthDevice,
                Some("oauth-token".to_string()),
                None,
            )
            .await
            .expect("login");

        assert_eq!(entry.source_summary, "stored:oaut***en");
        assert!(entry.usable);
        assert_eq!(
            stored_auth_token(&home, ProviderId::Anthropic).as_deref(),
            Some("oauth-token")
        );
    }

    #[tokio::test]
    async fn legacy_stored_hints_are_metadata_only() {
        let home = temp_home("legacy-hint");
        std::fs::write(
            home.join("auth.json"),
            r#"{"entries":[{"provider":"anthropic","method":"api_key","source":{"kind":"stored_hint","hint":"sk-a***et"},"updated_at":"2026-01-01T00:00:00Z"}]}"#,
        )
        .expect("auth store");

        let overview = sealed_auth_manager(home.clone())
            .overview()
            .await
            .expect("overview");
        let entry = overview.entries.first().expect("entry");

        assert!(!entry.usable);
        assert!(entry.source_summary.contains("metadata only"));
        assert!(!overview.has_provider(ProviderId::Anthropic));
        assert_eq!(stored_api_key(&home, ProviderId::Anthropic), None);
    }

    #[test]
    fn active_auth_source_marks_precedence_winner() {
        let mut entries = vec![
            AuthStatusEntry {
                provider: ProviderId::Anthropic,
                method: AuthMethod::OAuthDevice,
                source_summary: "credentials:claude.ai oauth (ready)".to_string(),
                persisted: true,
                usable: true,
                active: false,
                updated_at: None,
            },
            AuthStatusEntry {
                provider: ProviderId::Anthropic,
                method: AuthMethod::ApiKey,
                source_summary: "stored:sk-a***et".to_string(),
                persisted: true,
                usable: true,
                active: false,
                updated_at: None,
            },
            AuthStatusEntry {
                provider: ProviderId::Anthropic,
                method: AuthMethod::OAuthDevice,
                source_summary: "env:ANTHROPIC_AUTH_TOKEN".to_string(),
                persisted: false,
                usable: true,
                active: false,
                updated_at: None,
            },
            AuthStatusEntry {
                provider: ProviderId::Anthropic,
                method: AuthMethod::ApiKey,
                source_summary: "env:ANTHROPIC_API_KEY".to_string(),
                persisted: false,
                usable: true,
                active: false,
                updated_at: None,
            },
        ];

        mark_active_entries(&mut entries);

        let active = entries
            .iter()
            .filter(|entry| entry.active)
            .collect::<Vec<_>>();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].source_summary, "env:ANTHROPIC_AUTH_TOKEN");
    }

    #[test]
    fn active_auth_source_ignores_blocked_entries() {
        let mut entries = vec![
            AuthStatusEntry {
                provider: ProviderId::Anthropic,
                method: AuthMethod::OAuthDevice,
                source_summary: "credentials:claude.ai oauth (expired)".to_string(),
                persisted: true,
                usable: false,
                active: false,
                updated_at: None,
            },
            AuthStatusEntry {
                provider: ProviderId::Anthropic,
                method: AuthMethod::OAuthDevice,
                source_summary: "stored:oaut***en".to_string(),
                persisted: true,
                usable: true,
                active: false,
                updated_at: None,
            },
        ];

        mark_active_entries(&mut entries);

        assert!(!entries[0].active);
        assert!(entries[1].active);
    }

    #[tokio::test]
    async fn logout_removes_persisted_entries() {
        let manager = AuthManager::new(temp_home("logout"));
        manager
            .login(
                ProviderId::OpenAi,
                AuthMethod::ApiKey,
                None,
                Some("OPENAI_API_KEY".to_string()),
            )
            .await
            .expect("login");

        let removed = manager
            .logout(Some(ProviderId::OpenAi))
            .await
            .expect("logout");
        assert_eq!(removed, 1);
        assert!(
            !manager
                .overview()
                .await
                .expect("overview")
                .has_provider(ProviderId::OpenAi)
        );
    }

    #[tokio::test]
    async fn logout_anthropic_removes_claude_ai_oauth_credentials() {
        let home = temp_home("logout-oauth");
        std::fs::write(
            home.join(".credentials.json"),
            r#"{"claudeAiOauth":{"accessToken":"oauth-token","expiresAt":null,"scopes":["user:inference"]},"other":"kept"}"#,
        )
        .expect("credentials");
        let manager = sealed_auth_manager(home.clone());

        let removed = manager
            .logout(Some(ProviderId::Anthropic))
            .await
            .expect("logout");

        assert_eq!(removed, 1);
        let credentials =
            std::fs::read_to_string(home.join(".credentials.json")).expect("credentials remain");
        assert!(!credentials.contains("claudeAiOauth"));
        assert!(credentials.contains("other"));
        assert!(
            !manager
                .overview()
                .await
                .expect("overview")
                .has_provider(ProviderId::Anthropic)
        );
    }

    #[tokio::test]
    async fn logout_other_provider_keeps_claude_ai_oauth_credentials() {
        let home = temp_home("logout-other-provider-oauth");
        std::fs::write(
            home.join(".credentials.json"),
            r#"{"claudeAiOauth":{"accessToken":"oauth-token","expiresAt":null,"scopes":["user:inference"]}}"#,
        )
        .expect("credentials");
        let manager = AuthManager::new(home.clone());

        let removed = manager
            .logout(Some(ProviderId::OpenAi))
            .await
            .expect("logout");

        assert_eq!(removed, 0);
        assert!(
            std::fs::read_to_string(home.join(".credentials.json"))
                .expect("credentials")
                .contains("claudeAiOauth")
        );
        assert!(
            manager
                .overview()
                .await
                .expect("overview")
                .has_provider(ProviderId::Anthropic)
        );
    }

    #[tokio::test]
    async fn overview_reports_expired_oauth_as_blocked() {
        let home = temp_home("expired-oauth");
        let expired_at = (Utc::now() - ChronoDuration::minutes(10)).timestamp_millis();
        std::fs::write(
            home.join(".credentials.json"),
            format!(
                r#"{{
                    "claudeAiOauth": {{
                        "accessToken": "oauth-token",
                        "refreshToken": "refresh-token",
                        "expiresAt": {expired_at},
                        "scopes": ["user:inference", "user:profile"],
                        "subscriptionType": "pro"
                    }}
                }}"#
            ),
        )
        .expect("credentials");

        let overview = sealed_auth_manager(home)
            .overview()
            .await
            .expect("overview");
        let entry = overview
            .entries
            .iter()
            .find(|entry| entry.source_summary.contains("claude.ai oauth"))
            .expect("oauth entry");

        assert!(!entry.usable);
        assert!(entry.source_summary.contains("expired"));
        assert!(!overview.has_provider(ProviderId::Anthropic));
    }

    #[tokio::test]
    async fn overview_reports_missing_oauth_scope() {
        let home = temp_home("missing-scope-oauth");
        let expires_at = (Utc::now() + ChronoDuration::hours(1)).timestamp_millis();
        std::fs::write(
            home.join(".credentials.json"),
            format!(
                r#"{{
                    "claudeAiOauth": {{
                        "accessToken": "oauth-token",
                        "refreshToken": "refresh-token",
                        "expiresAt": {expires_at},
                        "scopes": ["user:profile"],
                        "subscriptionType": null
                    }}
                }}"#
            ),
        )
        .expect("credentials");

        let overview = AuthManager::new(home).overview().await.expect("overview");
        let entry = overview
            .entries
            .iter()
            .find(|entry| entry.source_summary.contains("claude.ai oauth"))
            .expect("oauth entry");

        assert!(!entry.usable);
        assert!(
            entry
                .source_summary
                .contains("missing user:inference scope")
        );
    }

    #[tokio::test]
    async fn overview_reports_profile_scope_and_subscription_status() {
        let home = temp_home("profile-subscription-oauth");
        let expires_at = (Utc::now() + ChronoDuration::hours(1)).timestamp_millis();
        std::fs::write(
            home.join(".credentials.json"),
            format!(
                r#"{{
                    "claudeAiOauth": {{
                        "accessToken": "oauth-token",
                        "refreshToken": "refresh-token",
                        "expiresAt": {expires_at},
                        "scopes": ["user:inference"],
                        "subscriptionType": "pro"
                    }}
                }}"#
            ),
        )
        .expect("credentials");

        let overview = AuthManager::new(home).overview().await.expect("overview");
        let entry = overview
            .entries
            .iter()
            .find(|entry| entry.source_summary.contains("claude.ai oauth"))
            .expect("oauth entry");

        assert!(entry.usable);
        assert!(entry.source_summary.contains("missing user:profile scope"));
        assert!(entry.source_summary.contains("subscription:pro"));
        assert!(overview.has_provider(ProviderId::Anthropic));
    }

    #[test]
    fn parse_forced_login_method_maps_policy_strings() {
        assert_eq!(
            parse_forced_login_method("claudeai"),
            Some(AuthMethod::OAuthDevice)
        );
        assert_eq!(
            parse_forced_login_method("console"),
            Some(AuthMethod::ApiKey)
        );
        assert_eq!(
            parse_forced_login_method("apiKey"),
            Some(AuthMethod::ApiKey)
        );
        assert_eq!(parse_forced_login_method("something-else"), None);
    }

    #[tokio::test]
    async fn forced_login_method_rejects_mismatched_method() {
        let home = temp_home("forced-login");
        let manager =
            AuthManager::new(home).with_forced_login_method(Some(AuthMethod::OAuthDevice));

        let error = manager
            .login(
                ProviderId::Anthropic,
                AuthMethod::ApiKey,
                Some("sk-ant-secret".to_string()),
                None,
            )
            .await
            .expect_err("api key login must be rejected under forced oauth");
        assert!(error.to_string().contains("locked by managed policy"));
    }

    #[tokio::test]
    async fn forced_login_method_allows_matching_method() {
        let home = temp_home("forced-login-match");
        let manager =
            AuthManager::new(home).with_forced_login_method(Some(AuthMethod::OAuthDevice));

        let entry = manager
            .login(
                ProviderId::Anthropic,
                AuthMethod::OAuthDevice,
                Some("oauth-token".to_string()),
                None,
            )
            .await
            .expect("matching oauth login succeeds");
        assert!(entry.usable);
    }

    #[test]
    fn usable_oauth_token_requires_unexpired_inference_scope() {
        let home = temp_home("usable-oauth");
        let expires_at = (Utc::now() + ChronoDuration::hours(1)).timestamp_millis();
        std::fs::write(
            home.join(".credentials.json"),
            format!(
                r#"{{
                    "claudeAiOauth": {{
                        "accessToken": "oauth-token",
                        "refreshToken": "refresh-token",
                        "expiresAt": {expires_at},
                        "scopes": ["user:inference"],
                        "subscriptionType": "max"
                    }}
                }}"#
            ),
        )
        .expect("credentials");

        assert_eq!(
            usable_claude_ai_oauth_token(&home).as_deref(),
            Some("oauth-token")
        );
    }

    #[tokio::test]
    async fn canonical_env_auth_token_is_marked_active_over_stored_key() {
        let home = temp_home("canonical-env-precedence");
        let mut overrides = sealed_auth_env_overrides();
        overrides.insert(
            "ORBCODE_ANTHROPIC_AUTH_TOKEN".to_string(),
            "sk-canonical".to_string(),
        );
        let manager = AuthManager::new(home.clone()).with_env_overrides(overrides);
        manager
            .login(
                ProviderId::Anthropic,
                AuthMethod::ApiKey,
                Some("sk-stored".to_string()),
                None,
            )
            .await
            .expect("login");
        let overview = manager.overview().await.expect("overview");
        let active = overview
            .entries
            .iter()
            .find(|e| e.provider == ProviderId::Anthropic && e.active)
            .expect("should have an active Anthropic entry");
        assert!(
            active
                .source_summary
                .contains("ORBCODE_ANTHROPIC_AUTH_TOKEN"),
            "canonical env entry should be active, got: {}",
            active.source_summary
        );
    }

    #[tokio::test]
    async fn canonical_env_openai_key_is_marked_active() {
        let home = temp_home("canonical-env-openai");
        let mut overrides = sealed_auth_env_overrides();
        overrides.insert(
            "ORBCODE_OPENAI_API_KEY".to_string(),
            "sk-openai".to_string(),
        );
        let manager = AuthManager::new(home).with_env_overrides(overrides);
        let overview = manager.overview().await.expect("overview");
        let active = overview
            .entries
            .iter()
            .find(|e| e.provider == ProviderId::OpenAi && e.active);
        assert!(
            active.is_some(),
            "canonical env:ORBCODE_OPENAI_API_KEY should produce an active entry"
        );
        assert!(
            active
                .unwrap()
                .source_summary
                .contains("ORBCODE_OPENAI_API_KEY"),
            "source_summary should show canonical key"
        );
    }
}
