use std::collections::HashMap;
use std::env;
use std::fmt;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use chrono::{DateTime, Utc};
use orbcode_protocol::ProviderId;
use serde::{Deserialize, Serialize};

use crate::ConfigError;
use crate::OutboundProxyConfig;
use crate::openai_oauth::{
    self, ChatGptBrowserLoginSession, ChatGptDeviceLoginSession, ChatGptOAuthCredentials,
    OpenAiOAuthOptions,
};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuthMethod {
    ApiKey,
    #[serde(alias = "oauth_device")]
    OAuthDevice,
    #[serde(rename = "chatgpt")]
    ChatGpt,
}

impl fmt::Display for AuthMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::ApiKey => "api_key",
            Self::OAuthDevice => "oauth_device",
            Self::ChatGpt => "chatgpt",
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
    openai_oauth: OpenAiOAuthOptions,
    #[cfg(feature = "oauth-test-support")]
    openai_oauth_test_error: Option<String>,
    openai_refresh_lock: Arc<tokio::sync::Mutex<()>>,
    cached_state: Arc<RwLock<StoredAuthState>>,
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
    EnvVar {
        env_var: String,
    },
    StoredApiKey {
        api_key: String,
    },
    StoredAuthToken {
        auth_token: String,
    },
    StoredHint {
        hint: String,
    },
    #[serde(rename = "chatgpt_oauth")]
    ChatGptOAuth {
        credentials: ChatGptOAuthCredentials,
    },
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
struct StoredAuthState {
    entries: Vec<StoredAuthEntry>,
}

impl AuthManager {
    pub fn new(home_dir: PathBuf) -> Self {
        #[cfg(feature = "oauth-test-support")]
        let (openai_oauth, openai_oauth_test_error) =
            match crate::oauth_test_support::from_process_env() {
                Ok(Some(options)) => (options, None),
                Ok(None) => (OpenAiOAuthOptions::default(), None),
                Err(error) => (OpenAiOAuthOptions::default(), Some(error)),
            };
        #[cfg(not(feature = "oauth-test-support"))]
        let openai_oauth = OpenAiOAuthOptions::default();

        Self {
            home_dir,
            env_overrides: HashMap::new(),
            forced_method: None,
            openai_oauth,
            #[cfg(feature = "oauth-test-support")]
            openai_oauth_test_error,
            openai_refresh_lock: Arc::new(tokio::sync::Mutex::new(())),
            cached_state: Arc::new(RwLock::new(StoredAuthState::default())),
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

    /// Override OAuth endpoints and callback behavior. Production callers use
    /// the fixed defaults; this exists so tests can use local fake servers.
    pub fn with_openai_oauth_options(mut self, options: OpenAiOAuthOptions) -> Self {
        self.openai_oauth = options;
        #[cfg(feature = "oauth-test-support")]
        {
            self.openai_oauth_test_error = None;
        }
        self
    }

    /// Apply the same resolved proxy inputs used by provider traffic to
    /// ChatGPT login, token exchange, and refresh requests.
    pub fn with_openai_proxy_config(mut self, proxy_config: OutboundProxyConfig) -> Self {
        self.openai_oauth.proxy_config = proxy_config;
        self
    }

    pub fn chatgpt_codex_base_url(&self) -> &str {
        &self.openai_oauth.codex_base_url
    }

    /// Refresh the in-memory auth snapshot used by synchronous session status
    /// and billing decisions. Runtime file I/O remains on Tokio's async path.
    pub async fn refresh_stored_state(&self) -> Result<(), ConfigError> {
        self.load_state().await.map(drop)
    }

    pub fn cached_api_key(&self, provider: ProviderId) -> Option<String> {
        let store = self
            .cached_state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        stored_api_key_from_state(&store, provider)
    }

    pub fn cached_chatgpt_oauth(&self) -> Option<ChatGptOAuthCredentials> {
        let store = self
            .cached_state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        chatgpt_oauth_from_state(&store)
    }

    pub async fn start_chatgpt_browser_login(
        &self,
    ) -> Result<ChatGptBrowserLoginSession, ConfigError> {
        self.ensure_openai_oauth_test_support_valid()?;
        self.ensure_login_method_allowed(AuthMethod::ChatGpt)?;
        openai_oauth::start_browser_login(self.openai_oauth.clone()).await
    }

    pub async fn complete_chatgpt_browser_login(
        &self,
        session: ChatGptBrowserLoginSession,
    ) -> Result<AuthStatusEntry, ConfigError> {
        self.ensure_openai_oauth_test_support_valid()?;
        let credentials = openai_oauth::complete_browser_login(session).await?;
        self.store_chatgpt_oauth(credentials).await
    }

    pub async fn start_chatgpt_device_login(
        &self,
    ) -> Result<ChatGptDeviceLoginSession, ConfigError> {
        self.ensure_openai_oauth_test_support_valid()?;
        self.ensure_login_method_allowed(AuthMethod::ChatGpt)?;
        openai_oauth::start_device_login(self.openai_oauth.clone()).await
    }

    pub async fn complete_chatgpt_device_login(
        &self,
        session: ChatGptDeviceLoginSession,
    ) -> Result<AuthStatusEntry, ConfigError> {
        self.ensure_openai_oauth_test_support_valid()?;
        let credentials = openai_oauth::complete_device_login(session).await?;
        self.store_chatgpt_oauth(credentials).await
    }

    /// Return current ChatGPT credentials, refreshing and atomically
    /// persisting a rotated refresh token when expiry is near.
    pub async fn resolve_chatgpt_oauth(
        &self,
    ) -> Result<Option<ChatGptOAuthCredentials>, ConfigError> {
        self.ensure_openai_oauth_test_support_valid()?;
        let Some(credentials) = self.load_chatgpt_oauth().await? else {
            return Ok(None);
        };
        ensure_chatgpt_oauth_usable(&credentials)?;
        if !credentials.needs_refresh() {
            return Ok(Some(credentials));
        }

        let _refresh_guard = self.openai_refresh_lock.lock().await;
        let Some(credentials) = self.load_chatgpt_oauth().await? else {
            return Ok(None);
        };
        ensure_chatgpt_oauth_usable(&credentials)?;
        if !credentials.needs_refresh() {
            return Ok(Some(credentials));
        }
        let refreshed = openai_oauth::refresh_credentials(&self.openai_oauth, &credentials).await?;
        self.store_chatgpt_oauth(refreshed.clone()).await?;
        Ok(Some(refreshed))
    }

    /// Force one refresh after the Codex backend rejects an otherwise-current
    /// access token. Callers must bound this recovery to one attempt.
    pub async fn refresh_chatgpt_oauth(
        &self,
    ) -> Result<Option<ChatGptOAuthCredentials>, ConfigError> {
        self.ensure_openai_oauth_test_support_valid()?;
        let _refresh_guard = self.openai_refresh_lock.lock().await;
        let Some(credentials) = self.load_chatgpt_oauth().await? else {
            return Ok(None);
        };
        ensure_chatgpt_oauth_usable(&credentials)?;
        let refreshed = openai_oauth::refresh_credentials(&self.openai_oauth, &credentials).await?;
        self.store_chatgpt_oauth(refreshed.clone()).await?;
        Ok(Some(refreshed))
    }

    fn ensure_login_method_allowed(&self, method: AuthMethod) -> Result<(), ConfigError> {
        if let Some(forced) = self.forced_method
            && forced != method
        {
            return Err(ConfigError::Config(format!(
                "login method is locked by managed policy to {forced}; cannot log in with {method}"
            )));
        }
        Ok(())
    }

    fn ensure_openai_oauth_test_support_valid(&self) -> Result<(), ConfigError> {
        #[cfg(feature = "oauth-test-support")]
        if let Some(error) = &self.openai_oauth_test_error {
            return Err(ConfigError::Config(error.clone()));
        }
        Ok(())
    }

    async fn store_chatgpt_oauth(
        &self,
        credentials: ChatGptOAuthCredentials,
    ) -> Result<AuthStatusEntry, ConfigError> {
        ensure_chatgpt_oauth_usable(&credentials)?;
        let mut store = self.load_state().await?;
        let entry = StoredAuthEntry {
            provider: ProviderId::OpenAi,
            method: AuthMethod::ChatGpt,
            source: StoredAuthSource::ChatGptOAuth { credentials },
            updated_at: Utc::now(),
        };
        store.entries.retain(|existing| {
            !(existing.provider == ProviderId::OpenAi && existing.method == AuthMethod::ChatGpt)
        });
        store.entries.push(entry.clone());
        self.save_state(&store).await?;
        self.status_for_stored_entry(&entry).await
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
        self.ensure_login_method_allowed(method)?;
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
                AuthMethod::ChatGpt => {
                    return Err(ConfigError::Config(
                        "ChatGPT login uses the browser or --device-code flow; --token is not accepted"
                            .into(),
                    ));
                }
            },
            (None, Some(env_var)) if method != AuthMethod::ChatGpt => {
                StoredAuthSource::EnvVar { env_var }
            }
            (None, Some(_)) => {
                return Err(ConfigError::Config(
                    "ChatGPT login cannot be configured through --env-var".into(),
                ));
            }
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
            .retain(|existing| !(existing.provider == provider && existing.method == method));
        store.entries.push(entry.clone());
        self.save_state(&store).await?;
        self.status_for_stored_entry(&entry).await
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

    async fn status_for_stored_entry(
        &self,
        stored: &StoredAuthEntry,
    ) -> Result<AuthStatusEntry, ConfigError> {
        self.overview()
            .await?
            .entries
            .into_iter()
            .find(|entry| {
                entry.persisted
                    && entry.provider == stored.provider
                    && entry.method == stored.method
                    && entry.updated_at == Some(stored.updated_at)
            })
            .ok_or_else(|| {
                ConfigError::Config("saved auth entry was not present in auth status".to_string())
            })
    }

    async fn load_state(&self) -> Result<StoredAuthState, ConfigError> {
        let path = self.store_path();
        tokio::fs::create_dir_all(&self.home_dir).await?;
        if !tokio::fs::try_exists(&path).await? {
            let store = StoredAuthState::default();
            self.replace_cached_state(&store);
            return Ok(store);
        }
        let contents = tokio::fs::read_to_string(path).await?;
        let store = serde_json::from_str(&contents)?;
        self.replace_cached_state(&store);
        Ok(store)
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
        self.replace_cached_state(store);
        Ok(())
    }

    async fn load_chatgpt_oauth(&self) -> Result<Option<ChatGptOAuthCredentials>, ConfigError> {
        let store = self.load_state().await?;
        Ok(chatgpt_oauth_from_state(&store))
    }

    fn replace_cached_state(&self, store: &StoredAuthState) {
        *self
            .cached_state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = store.clone();
    }
}

pub fn stored_api_key(home_dir: &Path, provider: ProviderId) -> Option<String> {
    let store = load_stored_auth_state(home_dir)?;
    stored_api_key_from_state(&store, provider)
}

fn stored_api_key_from_state(store: &StoredAuthState, provider: ProviderId) -> Option<String> {
    store.entries.iter().find_map(|entry| match entry {
        StoredAuthEntry {
            provider: entry_provider,
            source: StoredAuthSource::StoredApiKey { api_key },
            ..
        } if *entry_provider == provider => Some(api_key.clone()),
        _ => None,
    })
}

pub fn stored_auth_token(home_dir: &Path, provider: ProviderId) -> Option<String> {
    let store = load_stored_auth_state(home_dir)?;
    store.entries.into_iter().find_map(|entry| match entry {
        StoredAuthEntry {
            provider: entry_provider,
            source: StoredAuthSource::StoredAuthToken { auth_token },
            ..
        } if entry_provider == provider => Some(auth_token),
        _ => None,
    })
}

pub async fn load_chatgpt_oauth(home_dir: &Path) -> Option<ChatGptOAuthCredentials> {
    let contents = tokio::fs::read_to_string(home_dir.join("auth.json"))
        .await
        .ok()?;
    let store = serde_json::from_str(&contents).ok()?;
    chatgpt_oauth_from_state(&store)
}

fn chatgpt_oauth_from_state(store: &StoredAuthState) -> Option<ChatGptOAuthCredentials> {
    store
        .entries
        .iter()
        .find(|entry| entry.provider == ProviderId::OpenAi && entry.method == AuthMethod::ChatGpt)
        .and_then(|entry| match &entry.source {
            StoredAuthSource::ChatGptOAuth { credentials } => Some(credentials.clone()),
            _ => None,
        })
}

fn ensure_chatgpt_oauth_usable(credentials: &ChatGptOAuthCredentials) -> Result<(), ConfigError> {
    if credentials.is_usable() {
        Ok(())
    } else {
        Err(ConfigError::Config(
            "saved ChatGPT OAuth credentials are incomplete; sign in again with `orbcode auth login --provider openai --method chatgpt`"
                .to_string(),
        ))
    }
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
        StoredAuthSource::ChatGptOAuth { credentials } => {
            let mut status = if credentials.needs_refresh() {
                "refresh required".to_string()
            } else {
                "ready".to_string()
            };
            if let Some(plan_type) = credentials.plan_type.as_deref() {
                write!(status, "; subscription:{plan_type}")
                    .expect("writing to String cannot fail");
            }
            if let Some(email) = credentials.email.as_deref() {
                write!(status, "; {email}").expect("writing to String cannot fail");
            }
            format!("chatgpt oauth ({status})")
        }
    }
}

fn stored_source_usable(source: &StoredAuthSource) -> bool {
    match source {
        StoredAuthSource::StoredHint { .. } => false,
        StoredAuthSource::ChatGptOAuth { credentials } => credentials.is_usable(),
        _ => true,
    }
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
    if entry.method == AuthMethod::ChatGpt && entry.source_summary.starts_with("chatgpt oauth") {
        return 2;
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
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

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

    fn chatgpt_credentials(expires_at: i64) -> ChatGptOAuthCredentials {
        ChatGptOAuthCredentials {
            id_token: "id-token".to_string(),
            access_token: "access-token".to_string(),
            refresh_token: "refresh-token".to_string(),
            expires_at,
            account_id: Some("account-123".to_string()),
            email: Some("dev@example.com".to_string()),
            plan_type: Some("plus".to_string()),
        }
    }

    #[tokio::test]
    async fn chatgpt_credentials_round_trip_and_coexist_with_stored_api_key() {
        let home = temp_home("chatgpt-round-trip");
        let manager = sealed_auth_manager(home.clone());
        manager
            .login(
                ProviderId::OpenAi,
                AuthMethod::ApiKey,
                Some("sk-openai-secret".to_string()),
                None,
            )
            .await
            .expect("API key login");
        manager
            .store_chatgpt_oauth(chatgpt_credentials(
                Utc::now().timestamp_millis() + 60 * 60 * 1000,
            ))
            .await
            .expect("ChatGPT login");

        let loaded = load_chatgpt_oauth(&home)
            .await
            .expect("stored ChatGPT credentials");
        assert_eq!(loaded.account_id.as_deref(), Some("account-123"));
        assert_eq!(
            stored_api_key(&home, ProviderId::OpenAi).as_deref(),
            Some("sk-openai-secret")
        );
        let overview = manager.overview().await.expect("overview");
        let api_key = overview
            .entries
            .iter()
            .find(|entry| {
                entry.provider == ProviderId::OpenAi && entry.method == AuthMethod::ApiKey
            })
            .expect("API key status");
        let chatgpt = overview
            .entries
            .iter()
            .find(|entry| entry.method == AuthMethod::ChatGpt)
            .expect("ChatGPT status");
        assert!(api_key.active);
        assert!(!chatgpt.active);
        assert!(!chatgpt.source_summary.contains("access-token"));
        assert!(!chatgpt.source_summary.contains("refresh-token"));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(home.join("auth.json"))
                .expect("metadata")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[tokio::test]
    async fn concurrent_chatgpt_resolution_refreshes_once_and_persists_rotation() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/oauth/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "new-access-token",
                "refresh_token": "new-refresh-token",
                "expires_in": 3600
            })))
            .expect(1)
            .mount(&server)
            .await;

        let home = temp_home("chatgpt-singleflight");
        let manager =
            sealed_auth_manager(home.clone()).with_openai_oauth_options(OpenAiOAuthOptions {
                issuer: server.uri(),
                ..OpenAiOAuthOptions::default()
            });
        manager
            .store_chatgpt_oauth(chatgpt_credentials(Utc::now().timestamp_millis() - 1))
            .await
            .expect("expired credentials");

        let left = manager.clone();
        let right = manager.clone();
        let (left, right) =
            tokio::join!(left.resolve_chatgpt_oauth(), right.resolve_chatgpt_oauth());
        assert_eq!(
            left.expect("left").expect("credentials").access_token,
            "new-access-token"
        );
        assert_eq!(
            right.expect("right").expect("credentials").access_token,
            "new-access-token"
        );
        let stored = load_chatgpt_oauth(&home)
            .await
            .expect("rotated credentials");
        assert_eq!(stored.id_token, "id-token");
        assert_eq!(stored.refresh_token, "new-refresh-token");
    }

    #[test]
    fn chatgpt_credentials_require_non_empty_request_and_refresh_fields() {
        let valid = chatgpt_credentials(Utc::now().timestamp_millis() + 60 * 60 * 1000);
        assert!(valid.is_usable());

        for invalid in [
            ChatGptOAuthCredentials {
                access_token: " ".to_string(),
                ..valid.clone()
            },
            ChatGptOAuthCredentials {
                refresh_token: String::new(),
                ..valid.clone()
            },
            ChatGptOAuthCredentials {
                account_id: None,
                ..valid.clone()
            },
            ChatGptOAuthCredentials {
                account_id: Some("  ".to_string()),
                ..valid
            },
        ] {
            assert!(!invalid.is_usable());
        }
    }

    #[tokio::test]
    async fn chatgpt_resolution_rejects_incomplete_stored_credentials() {
        let home = temp_home("chatgpt-incomplete");
        std::fs::write(
            home.join("auth.json"),
            r#"{"entries":[{"provider":"openai","method":"chatgpt","source":{"kind":"chatgpt_oauth","credentials":{"id_token":"id","access_token":"access","refresh_token":"refresh","expires_at":4102444800000,"account_id":null,"email":null,"plan_type":"plus"}},"updated_at":"2026-08-03T00:00:00Z"}]}"#,
        )
        .expect("auth store");

        let error = sealed_auth_manager(home)
            .resolve_chatgpt_oauth()
            .await
            .expect_err("incomplete credentials must fail");
        assert!(error.to_string().contains("credentials are incomplete"));
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
