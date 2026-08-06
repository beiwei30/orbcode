use std::collections::BTreeMap;

use orbcode_app_server_protocol::{
    AgentDefinition, AgentHookCommand, AgentHookMatcher, AgentLoadWarning, AgentPermissionMode,
    AgentSource, AgentWarningKind, AuthMethod, AuthOverview, AuthStatusEntry, ContextWindowOptions,
    DiscoveredHook, EditorModeSetting, HookDiscovery, HookDiscoveryWarning, HookLayer,
    HookProvenance, HookValidationStatus, MaxOutputTokenOptions, McpAnnotations, McpAuth,
    McpAuthOverview, McpContent, McpDiagnosticCheck, McpDiagnosticStatus, McpOAuthOverview,
    McpOAuthStatusEntry, McpPluginSource, McpPrompt, McpPromptArgument, McpPromptMessage,
    McpPromptResult, McpResourceContent, McpResourceSummary, McpServerInput, McpServerOverview,
    McpServerSource, McpServerStatus, McpServerTrust, McpToolResult, McpToolSpec, McpTransport,
    PermissionContext, PermissionMode, PermissionRuleOverview, PermissionRuleUpdateResult,
    SandboxFilesystemLocalSettings, SandboxLocalSettings, SandboxNetworkLocalSettings,
    SandboxSettingsUpdate, SkillDefinition, SkillSource, StatusAuthOverview, StatusAuthStatusEntry,
    ThemeSetting, TokenWarningOptions,
};

const REDACTED: &str = "<redacted>";

pub(crate) fn effective_model_selection_to_wire(
    selection: orbcode_config::EffectiveModelSelection,
) -> orbcode_app_server_protocol::EffectiveModelSelection {
    use orbcode_app_server_protocol::{
        EffectiveModelSelection, ModelSelectionSource, PersistedModelSetting,
        ProviderModelSelection, RuntimeModelOverride, SettingSource,
    };

    let source_to_wire = |source: orbcode_config::SettingOrigin| match source {
        orbcode_config::SettingOrigin::Defaults => SettingSource::Defaults,
        orbcode_config::SettingOrigin::User => SettingSource::User,
        orbcode_config::SettingOrigin::Project => SettingSource::Project,
        orbcode_config::SettingOrigin::Local => SettingSource::Local,
        orbcode_config::SettingOrigin::Managed => SettingSource::Managed,
        orbcode_config::SettingOrigin::CliFlags => SettingSource::Cli,
        orbcode_config::SettingOrigin::EnvVars => SettingSource::Environment,
        orbcode_config::SettingOrigin::SessionOverride => SettingSource::Session,
    };
    let runtime_override = match selection.runtime_override {
        orbcode_config::RuntimeModelOverride::Inherit => RuntimeModelOverride::Inherit,
        orbcode_config::RuntimeModelOverride::Default => RuntimeModelOverride::Default,
        orbcode_config::RuntimeModelOverride::Model(model) => RuntimeModelOverride::Model(model),
    };
    let source = match selection.source {
        orbcode_config::ModelSelectionSource::Runtime => ModelSelectionSource::Runtime,
        orbcode_config::ModelSelectionSource::Environment => ModelSelectionSource::Environment,
        orbcode_config::ModelSelectionSource::Persisted => ModelSelectionSource::Persisted,
        orbcode_config::ModelSelectionSource::ProviderDefault => {
            ModelSelectionSource::ProviderDefault
        }
    };
    EffectiveModelSelection {
        persisted: PersistedModelSetting {
            value: selection.persisted.value,
            source: selection.persisted.source.map(source_to_wire),
            locked: selection.persisted.locked,
        },
        runtime_override,
        requested_model: selection.requested_model,
        source,
        provider: selection.provider,
        resolution: ProviderModelSelection {
            requested_setting: selection.resolution.requested_setting,
            family: selection
                .resolution
                .family
                .map(|family| family.alias().to_string()),
            model: selection.resolution.model,
            request_model: selection.resolution.request_model,
            display_label: selection.resolution.display_label,
            display_name: selection.resolution.display_name,
            capabilities: selection
                .resolution
                .capabilities
                .into_iter()
                .map(|capability| capability.as_str().to_string())
                .collect(),
        },
    }
}

pub(crate) fn context_window_options_to_wire(
    options: orbcode_config::ContextWindowOptions,
) -> ContextWindowOptions {
    ContextWindowOptions {
        disable_1m_context: options.disable_1m_context,
        max_context_tokens_override: options.max_context_tokens_override,
        auto_compact_window_override: options.auto_compact_window_override,
    }
}

pub(crate) fn max_output_token_options_to_wire(
    options: orbcode_config::MaxOutputTokenOptions,
) -> MaxOutputTokenOptions {
    MaxOutputTokenOptions {
        max_output_tokens_override: options.max_output_tokens_override,
    }
}

pub(crate) fn token_warning_options_to_wire(
    options: orbcode_config::TokenWarningOptions,
) -> TokenWarningOptions {
    TokenWarningOptions {
        auto_compact_enabled: options.auto_compact_enabled,
        auto_compact_percent_override: options.auto_compact_percent_override,
        blocking_limit_override: options.blocking_limit_override,
    }
}

pub(crate) fn theme_setting_to_wire(theme: orbcode_config::ThemeSetting) -> ThemeSetting {
    match theme {
        orbcode_config::ThemeSetting::Auto => ThemeSetting::Auto,
        orbcode_config::ThemeSetting::Dark => ThemeSetting::Dark,
        orbcode_config::ThemeSetting::Light => ThemeSetting::Light,
        orbcode_config::ThemeSetting::DarkDaltonized => ThemeSetting::DarkDaltonized,
        orbcode_config::ThemeSetting::LightDaltonized => ThemeSetting::LightDaltonized,
        orbcode_config::ThemeSetting::DarkAnsi => ThemeSetting::DarkAnsi,
        orbcode_config::ThemeSetting::LightAnsi => ThemeSetting::LightAnsi,
    }
}

pub(crate) fn theme_setting_from_wire(theme: ThemeSetting) -> orbcode_config::ThemeSetting {
    match theme {
        ThemeSetting::Auto => orbcode_config::ThemeSetting::Auto,
        ThemeSetting::Dark => orbcode_config::ThemeSetting::Dark,
        ThemeSetting::Light => orbcode_config::ThemeSetting::Light,
        ThemeSetting::DarkDaltonized => orbcode_config::ThemeSetting::DarkDaltonized,
        ThemeSetting::LightDaltonized => orbcode_config::ThemeSetting::LightDaltonized,
        ThemeSetting::DarkAnsi => orbcode_config::ThemeSetting::DarkAnsi,
        ThemeSetting::LightAnsi => orbcode_config::ThemeSetting::LightAnsi,
    }
}

pub(crate) fn editor_mode_setting_to_wire(
    mode: orbcode_config::EditorModeSetting,
) -> EditorModeSetting {
    match mode {
        orbcode_config::EditorModeSetting::Normal => EditorModeSetting::Normal,
        orbcode_config::EditorModeSetting::Vim => EditorModeSetting::Vim,
    }
}

pub(crate) fn editor_mode_setting_from_wire(
    mode: EditorModeSetting,
) -> orbcode_config::EditorModeSetting {
    match mode {
        EditorModeSetting::Normal => orbcode_config::EditorModeSetting::Normal,
        EditorModeSetting::Vim => orbcode_config::EditorModeSetting::Vim,
    }
}

pub(crate) fn sandbox_local_settings_to_wire(
    settings: orbcode_config::SandboxLocalSettings,
) -> SandboxLocalSettings {
    SandboxLocalSettings {
        enabled: settings.enabled,
        auto_allow_bash_if_sandboxed: settings.auto_allow_bash_if_sandboxed,
        allow_unsandboxed_commands: settings.allow_unsandboxed_commands,
        excluded_commands: settings.excluded_commands,
        filesystem: SandboxFilesystemLocalSettings {
            allow_write: settings.filesystem.allow_write,
            deny_write: settings.filesystem.deny_write,
            deny_read: settings.filesystem.deny_read,
            allow_read: settings.filesystem.allow_read,
        },
        network: SandboxNetworkLocalSettings {
            allowed_domains: settings.network.allowed_domains,
            allow_unix_sockets: settings.network.allow_unix_sockets,
            allow_all_unix_sockets: settings.network.allow_all_unix_sockets,
            allow_local_binding: settings.network.allow_local_binding,
            http_proxy_port: settings.network.http_proxy_port,
            socks_proxy_port: settings.network.socks_proxy_port,
        },
    }
}

pub(crate) fn sandbox_settings_update_from_wire(
    update: SandboxSettingsUpdate,
) -> orbcode_config::SandboxSettingsUpdate {
    orbcode_config::SandboxSettingsUpdate {
        enabled: update.enabled,
        auto_allow_bash_if_sandboxed: update.auto_allow_bash_if_sandboxed,
        allow_unsandboxed_commands: update.allow_unsandboxed_commands,
    }
}

pub(crate) fn auth_overview_to_wire(overview: orbcode_config::AuthOverview) -> AuthOverview {
    AuthOverview {
        store_path: overview.store_path,
        entries: overview
            .entries
            .into_iter()
            .map(auth_status_entry_to_wire)
            .collect(),
    }
}

pub(crate) fn auth_status_entry_to_wire(entry: orbcode_config::AuthStatusEntry) -> AuthStatusEntry {
    AuthStatusEntry {
        provider: entry.provider,
        method: auth_method_to_wire(entry.method),
        source_summary: entry.source_summary,
        persisted: entry.persisted,
        usable: entry.usable,
        active: entry.active,
    }
}

pub(crate) fn status_auth_overview_to_wire(
    overview: orbcode_config::AuthOverview,
) -> StatusAuthOverview {
    StatusAuthOverview {
        store_path: overview.store_path,
        entries: overview
            .entries
            .into_iter()
            .map(|entry| StatusAuthStatusEntry {
                provider: entry.provider,
                method: auth_method_to_wire(entry.method),
                source_summary: entry.source_summary,
                persisted: entry.persisted,
                usable: entry.usable,
                active: entry.active,
                updated_at: entry.updated_at,
            })
            .collect(),
    }
}

pub(crate) fn auth_method_from_wire(method: AuthMethod) -> orbcode_config::AuthMethod {
    match method {
        AuthMethod::ApiKey => orbcode_config::AuthMethod::ApiKey,
        AuthMethod::OAuthDevice => orbcode_config::AuthMethod::OAuthDevice,
        AuthMethod::ChatGpt => orbcode_config::AuthMethod::ChatGpt,
    }
}

fn auth_method_to_wire(method: orbcode_config::AuthMethod) -> AuthMethod {
    match method {
        orbcode_config::AuthMethod::ApiKey => AuthMethod::ApiKey,
        orbcode_config::AuthMethod::OAuthDevice => AuthMethod::OAuthDevice,
        orbcode_config::AuthMethod::ChatGpt => AuthMethod::ChatGpt,
    }
}

pub(crate) fn skill_definition_to_wire(
    definition: orbcode_tools::SkillDefinition,
) -> SkillDefinition {
    SkillDefinition {
        name: definition.name,
        description: definition.description,
        when_to_use: definition.when_to_use,
        allowed_tools: definition.allowed_tools,
        model: definition.model,
        assets: definition.assets,
        path: definition.path,
        body: definition.body,
        source: match definition.source {
            orbcode_tools::SkillSource::User => SkillSource::User,
            orbcode_tools::SkillSource::Project => SkillSource::Project,
            orbcode_tools::SkillSource::Plugin { plugin_id } => SkillSource::Plugin { plugin_id },
            orbcode_tools::SkillSource::Bundled => SkillSource::Bundled,
            orbcode_tools::SkillSource::Mcp { server_id } => SkillSource::Mcp { server_id },
        },
    }
}

pub(crate) fn agent_definition_to_wire(
    definition: orbcode_config::AgentDefinition,
) -> AgentDefinition {
    AgentDefinition {
        agent_type: definition.agent_type,
        description: definition.description,
        prompt: definition.prompt,
        tools: definition.tools,
        disallowed_tools: definition.disallowed_tools,
        model: definition.model,
        permission_mode: definition
            .permission_mode
            .map(agent_permission_mode_to_wire),
        skills: definition.skills,
        mcp_server_names: definition.mcp_server_names,
        hooks: definition
            .hooks
            .into_iter()
            .map(|(event, matchers)| {
                (
                    event,
                    matchers
                        .into_iter()
                        .map(agent_hook_matcher_to_wire)
                        .collect(),
                )
            })
            .collect(),
        source: agent_source_to_wire(definition.source),
        path: definition.path,
    }
}

pub(crate) fn agent_load_warning_to_wire(
    warning: orbcode_config::AgentLoadWarning,
) -> AgentLoadWarning {
    AgentLoadWarning {
        kind: match warning.kind {
            orbcode_config::AgentWarningKind::MissingField => AgentWarningKind::MissingField,
            orbcode_config::AgentWarningKind::DuplicateName => AgentWarningKind::DuplicateName,
        },
        source: agent_source_to_wire(warning.source),
        path: warning.path,
        agent_type: warning.agent_type,
        message: warning.message,
    }
}

fn agent_permission_mode_to_wire(mode: orbcode_config::PermissionMode) -> AgentPermissionMode {
    match mode {
        orbcode_config::PermissionMode::Default => AgentPermissionMode::Default,
        orbcode_config::PermissionMode::AcceptEdits => AgentPermissionMode::AcceptEdits,
        orbcode_config::PermissionMode::BypassPermissions => AgentPermissionMode::BypassPermissions,
        orbcode_config::PermissionMode::DontAsk => AgentPermissionMode::DontAsk,
        orbcode_config::PermissionMode::Plan => AgentPermissionMode::Plan,
        orbcode_config::PermissionMode::Auto => AgentPermissionMode::Auto,
    }
}

pub(crate) fn permission_mode_from_wire(mode: PermissionMode) -> orbcode_config::PermissionMode {
    match mode {
        PermissionMode::Default => orbcode_config::PermissionMode::Default,
        PermissionMode::AcceptEdits => orbcode_config::PermissionMode::AcceptEdits,
        PermissionMode::BypassPermissions => orbcode_config::PermissionMode::BypassPermissions,
        PermissionMode::DontAsk => orbcode_config::PermissionMode::DontAsk,
        PermissionMode::Plan => orbcode_config::PermissionMode::Plan,
        PermissionMode::Auto => orbcode_config::PermissionMode::Auto,
    }
}

pub(crate) fn permission_mode_to_wire(mode: orbcode_config::PermissionMode) -> PermissionMode {
    match mode {
        orbcode_config::PermissionMode::Default => PermissionMode::Default,
        orbcode_config::PermissionMode::AcceptEdits => PermissionMode::AcceptEdits,
        orbcode_config::PermissionMode::BypassPermissions => PermissionMode::BypassPermissions,
        orbcode_config::PermissionMode::DontAsk => PermissionMode::DontAsk,
        orbcode_config::PermissionMode::Plan => PermissionMode::Plan,
        orbcode_config::PermissionMode::Auto => PermissionMode::Auto,
    }
}

fn agent_source_to_wire(source: orbcode_config::AgentSource) -> AgentSource {
    match source {
        orbcode_config::AgentSource::BuiltIn => AgentSource::BuiltIn,
        orbcode_config::AgentSource::UserSettings => AgentSource::UserSettings,
        orbcode_config::AgentSource::ProjectSettings => AgentSource::ProjectSettings,
        orbcode_config::AgentSource::Plugin { plugin_id } => AgentSource::Plugin { plugin_id },
    }
}

fn agent_hook_matcher_to_wire(matcher: orbcode_config::HookMatcher) -> AgentHookMatcher {
    AgentHookMatcher {
        matcher: matcher.matcher,
        hooks: matcher
            .hooks
            .into_iter()
            .map(|hook| match hook {
                orbcode_config::HookCommand::Command {
                    command,
                    r#if,
                    timeout,
                } => AgentHookCommand::Command {
                    command,
                    r#if,
                    timeout,
                },
                orbcode_config::HookCommand::Unsupported => AgentHookCommand::Unsupported,
            })
            .collect(),
    }
}

pub(crate) fn hook_discovery_to_wire(discovery: orbcode_config::HookDiscovery) -> HookDiscovery {
    HookDiscovery {
        hooks: discovery
            .hooks
            .into_iter()
            .map(|hook| DiscoveredHook {
                event: hook.event,
                provenance: hook_provenance_to_wire(hook.provenance),
                matcher: hook.matcher,
                command: hook.command,
                trusted: hook.trusted,
                validation: match hook.validation {
                    orbcode_config::HookValidationStatus::Valid => HookValidationStatus::Valid,
                    orbcode_config::HookValidationStatus::Invalid(reason) => {
                        HookValidationStatus::Invalid(reason)
                    }
                },
            })
            .collect(),
        warnings: discovery
            .warnings
            .into_iter()
            .map(|warning| HookDiscoveryWarning {
                provenance: hook_provenance_to_wire(warning.provenance),
                event: warning.event,
                message: warning.message,
            })
            .collect(),
    }
}

fn hook_provenance_to_wire(provenance: orbcode_config::HookProvenance) -> HookProvenance {
    match provenance {
        orbcode_config::HookProvenance::BuiltIn => HookProvenance::BuiltIn,
        orbcode_config::HookProvenance::Settings(layer) => HookProvenance::Settings(match layer {
            orbcode_config::HookLayer::User => HookLayer::User,
            orbcode_config::HookLayer::Project => HookLayer::Project,
            orbcode_config::HookLayer::Local => HookLayer::Local,
            orbcode_config::HookLayer::Managed => HookLayer::Managed,
        }),
        orbcode_config::HookProvenance::Skill { name } => HookProvenance::Skill { name },
        orbcode_config::HookProvenance::Agent { name } => HookProvenance::Agent { name },
        orbcode_config::HookProvenance::Plugin { plugin_id } => {
            HookProvenance::Plugin { plugin_id }
        }
    }
}

pub(crate) fn permission_context_to_wire(
    context: orbcode_core::PermissionContext,
) -> PermissionContext {
    PermissionContext {
        cwd: context.cwd,
        allow_network: context.allow_network,
        provider_allow_network: context.provider_allow_network,
        allow_tools: context.allow_tools,
        allowed_rules: context
            .allowed_rules
            .into_iter()
            .map(permission_rule_to_wire)
            .collect(),
        denied_rules: context
            .denied_rules
            .into_iter()
            .map(permission_rule_to_wire)
            .collect(),
        ask_rules: context
            .ask_rules
            .into_iter()
            .map(permission_rule_to_wire)
            .collect(),
        additional_directories: context.additional_directories,
    }
}

fn permission_rule_to_wire(rule: orbcode_config::PermissionRule) -> PermissionRuleOverview {
    PermissionRuleOverview {
        raw: rule.raw,
        tool_name: rule.tool_name,
        rule_content: rule.rule_content,
    }
}

pub(crate) fn mcp_server_config_from_input(input: McpServerInput) -> orbcode_mcp::McpServerConfig {
    orbcode_mcp::McpServerConfig {
        id: input.id,
        transport: mcp_transport_from_wire(input.transport),
        endpoint: input.endpoint,
        args: input.args,
        env: input.env,
        cwd: input.cwd,
        headers: input.headers,
        enabled: input.enabled,
        status: mcp_status_from_wire(input.status),
        error: input.error,
        summary: input.summary,
        auth: mcp_auth_from_wire(input.auth),
        trust: mcp_trust_from_wire(input.trust),
        transport_type_hint: input.transport_type_hint,
        source: input.source.map(mcp_source_from_wire),
    }
}

pub(crate) fn mcp_server_overview_from_config(
    config: orbcode_mcp::McpServerConfig,
) -> McpServerOverview {
    McpServerOverview {
        id: config.id,
        transport: mcp_transport_to_wire(config.transport),
        endpoint: redact_endpoint(&config.endpoint),
        args: redact_values(config.args),
        env: redact_map_values(config.env),
        cwd: config.cwd,
        headers: redact_map_values(config.headers),
        enabled: config.enabled,
        status: mcp_status_to_wire(config.status),
        error: config
            .error
            .map(|_| "MCP server error (details redacted)".to_string()),
        summary: config.summary,
        auth: mcp_auth_overview(config.auth),
        trust: mcp_trust_to_wire(config.trust),
        transport_type_hint: config.transport_type_hint,
        source: config.source.map(mcp_source_to_wire),
    }
}

pub(crate) fn mcp_oauth_overview_to_wire(
    overview: orbcode_mcp::McpOAuthOverview,
) -> McpOAuthOverview {
    McpOAuthOverview {
        store_path: overview.store_path,
        entries: overview
            .entries
            .into_iter()
            .map(|entry| McpOAuthStatusEntry {
                server_id: entry.server_id,
                source_summary: entry.source_summary,
                usable: entry.usable,
                expired: entry.expired,
                has_refresh_token: entry.has_refresh_token,
                has_token_endpoint: entry.has_token_endpoint,
                expires_at: entry.expires_at,
                scopes: entry.scopes,
                updated_at: entry.updated_at,
            })
            .collect(),
    }
}

pub(crate) fn mcp_prompt_result_to_wire(result: orbcode_mcp::McpPromptResult) -> McpPromptResult {
    McpPromptResult {
        description: result.description,
        messages: result
            .messages
            .into_iter()
            .map(|message| McpPromptMessage {
                role: message.role,
                content: McpContent {
                    kind: message.content.kind,
                    text: message.content.text,
                    binary: message.content.binary,
                    is_binary: message.content.is_binary,
                    mime_type: message.content.mime_type,
                    annotations: message
                        .content
                        .annotations
                        .map(|annotations| McpAnnotations {
                            audience: annotations.audience,
                            priority: annotations.priority,
                            extra: annotations.extra,
                        }),
                },
            })
            .collect(),
    }
}

pub(crate) fn mcp_tool_spec_to_wire(spec: orbcode_mcp::McpToolSpec) -> McpToolSpec {
    McpToolSpec {
        name: spec.name,
        summary: spec.summary,
    }
}

pub(crate) fn mcp_resource_summary_to_wire(
    resource: orbcode_mcp::McpResourceSummary,
) -> McpResourceSummary {
    McpResourceSummary {
        uri: resource.uri,
        name: resource.name,
        mime_type: resource.mime_type,
        description: resource.description,
        annotations: resource.annotations.map(|annotations| McpAnnotations {
            audience: annotations.audience,
            priority: annotations.priority,
            extra: annotations.extra,
        }),
    }
}

pub(crate) fn mcp_resource_content_to_wire(
    resource: orbcode_mcp::McpResourceContent,
) -> McpResourceContent {
    McpResourceContent {
        uri: resource.uri,
        mime_type: resource.mime_type,
        contents: resource.contents,
        blob: resource.blob,
        is_binary: resource.is_binary,
        annotations: resource.annotations.map(|annotations| McpAnnotations {
            audience: annotations.audience,
            priority: annotations.priority,
            extra: annotations.extra,
        }),
    }
}

pub(crate) fn mcp_prompt_to_wire(prompt: orbcode_mcp::McpPrompt) -> McpPrompt {
    McpPrompt {
        name: prompt.name,
        description: prompt.description,
        arguments: prompt
            .arguments
            .into_iter()
            .map(|argument| McpPromptArgument {
                name: argument.name,
                description: argument.description,
                required: argument.required,
            })
            .collect(),
        skill: prompt.skill,
        extra: prompt.extra,
    }
}

pub(crate) fn mcp_tool_result_to_wire(result: orbcode_mcp::McpToolResult) -> McpToolResult {
    McpToolResult {
        server_id: result.server_id,
        tool_name: result.tool_name,
        output: result.output,
        is_error: result.is_error,
    }
}

pub(crate) fn mcp_diagnostic_check_to_wire(
    check: orbcode_mcp::McpDiagnosticCheck,
) -> McpDiagnosticCheck {
    McpDiagnosticCheck {
        name: check.name,
        status: match check.status {
            orbcode_mcp::McpDiagnosticStatus::Pass => McpDiagnosticStatus::Pass,
            orbcode_mcp::McpDiagnosticStatus::Warn => McpDiagnosticStatus::Warn,
            orbcode_mcp::McpDiagnosticStatus::Fail => McpDiagnosticStatus::Fail,
        },
        detail: check.detail,
    }
}

pub(crate) fn permission_rule_update_to_wire(
    update: orbcode_config::PermissionRuleSettingsUpdate,
    kind: orbcode_app_server_protocol::PermissionRuleKind,
    target: orbcode_app_server_protocol::PermissionRuleTargetScope,
    effective_rules: orbcode_app_server_protocol::EffectivePermissionRules,
) -> PermissionRuleUpdateResult {
    PermissionRuleUpdateResult {
        path: update.path,
        rule: update.rule,
        changed: update.changed,
        kind,
        target,
        source: match target {
            orbcode_app_server_protocol::PermissionRuleTargetScope::Settings => {
                orbcode_app_server_protocol::SettingSource::User
            }
            orbcode_app_server_protocol::PermissionRuleTargetScope::Session => {
                orbcode_app_server_protocol::SettingSource::Session
            }
        },
        effective_rules,
    }
}

fn redact_values(values: Vec<String>) -> Vec<String> {
    values.into_iter().map(|_| REDACTED.to_string()).collect()
}

fn redact_map_values(values: BTreeMap<String, String>) -> BTreeMap<String, String> {
    values
        .into_keys()
        .map(|key| (key, REDACTED.to_string()))
        .collect()
}

fn redact_endpoint(endpoint: &str) -> String {
    let appears_sensitive = endpoint.contains('?')
        || endpoint.contains('#')
        || endpoint.split_once("://").is_some_and(|(_, remainder)| {
            remainder.split('/').next().is_some_and(|v| v.contains('@'))
        });
    if !appears_sensitive {
        return endpoint.to_string();
    }

    let Ok(mut parsed) = url::Url::parse(endpoint) else {
        return "<redacted-endpoint>".to_string();
    };

    if !parsed.username().is_empty() {
        let _ = parsed.set_username(REDACTED);
    }
    if parsed.password().is_some() {
        let _ = parsed.set_password(Some(REDACTED));
    }
    if parsed.query().is_some() {
        let query_keys = parsed
            .query_pairs()
            .map(|(key, _)| key.into_owned())
            .collect::<Vec<_>>();
        parsed.set_query(None);
        let mut query = parsed.query_pairs_mut();
        for key in query_keys {
            query.append_pair(&key, REDACTED);
        }
    }
    parsed.set_fragment(None);
    parsed.into()
}

fn mcp_auth_from_wire(auth: McpAuth) -> orbcode_mcp::McpAuth {
    match auth {
        McpAuth::None => orbcode_mcp::McpAuth::None,
        McpAuth::BearerEnv { env_var } => orbcode_mcp::McpAuth::BearerEnv { env_var },
        McpAuth::Header { name, value } => orbcode_mcp::McpAuth::Header { name, value },
    }
}

fn mcp_auth_overview(auth: orbcode_mcp::McpAuth) -> McpAuthOverview {
    match auth {
        orbcode_mcp::McpAuth::None => McpAuthOverview::None,
        orbcode_mcp::McpAuth::BearerEnv { env_var } => McpAuthOverview::BearerEnv { env_var },
        orbcode_mcp::McpAuth::Header { name, .. } => McpAuthOverview::Header {
            name,
            value: REDACTED.to_string(),
        },
    }
}

fn mcp_transport_from_wire(transport: McpTransport) -> orbcode_mcp::McpTransport {
    match transport {
        McpTransport::Stdio => orbcode_mcp::McpTransport::Stdio,
        McpTransport::Http => orbcode_mcp::McpTransport::Http,
        McpTransport::Https => orbcode_mcp::McpTransport::Https,
        McpTransport::StreamableHttp => orbcode_mcp::McpTransport::StreamableHttp,
        McpTransport::WebSocket => orbcode_mcp::McpTransport::WebSocket,
    }
}

pub(crate) fn mcp_transport_to_wire(transport: orbcode_mcp::McpTransport) -> McpTransport {
    match transport {
        orbcode_mcp::McpTransport::Stdio => McpTransport::Stdio,
        orbcode_mcp::McpTransport::Http => McpTransport::Http,
        orbcode_mcp::McpTransport::Https => McpTransport::Https,
        orbcode_mcp::McpTransport::StreamableHttp => McpTransport::StreamableHttp,
        orbcode_mcp::McpTransport::WebSocket => McpTransport::WebSocket,
    }
}

fn mcp_status_from_wire(status: McpServerStatus) -> orbcode_mcp::McpServerStatus {
    match status {
        McpServerStatus::Disabled => orbcode_mcp::McpServerStatus::Disabled,
        McpServerStatus::Starting => orbcode_mcp::McpServerStatus::Starting,
        McpServerStatus::Ready => orbcode_mcp::McpServerStatus::Ready,
        McpServerStatus::Failed => orbcode_mcp::McpServerStatus::Failed,
        McpServerStatus::Unauthorized => orbcode_mcp::McpServerStatus::Unauthorized,
        McpServerStatus::Restarting => orbcode_mcp::McpServerStatus::Restarting,
        McpServerStatus::Stopped => orbcode_mcp::McpServerStatus::Stopped,
    }
}

fn mcp_status_to_wire(status: orbcode_mcp::McpServerStatus) -> McpServerStatus {
    match status {
        orbcode_mcp::McpServerStatus::Disabled => McpServerStatus::Disabled,
        orbcode_mcp::McpServerStatus::Starting => McpServerStatus::Starting,
        orbcode_mcp::McpServerStatus::Ready => McpServerStatus::Ready,
        orbcode_mcp::McpServerStatus::Failed => McpServerStatus::Failed,
        orbcode_mcp::McpServerStatus::Unauthorized => McpServerStatus::Unauthorized,
        orbcode_mcp::McpServerStatus::Restarting => McpServerStatus::Restarting,
        orbcode_mcp::McpServerStatus::Stopped => McpServerStatus::Stopped,
    }
}

pub(crate) fn mcp_trust_from_wire(trust: McpServerTrust) -> orbcode_mcp::McpServerTrust {
    match trust {
        McpServerTrust::Unknown => orbcode_mcp::McpServerTrust::Unknown,
        McpServerTrust::Trusted => orbcode_mcp::McpServerTrust::Trusted,
        McpServerTrust::Denied => orbcode_mcp::McpServerTrust::Denied,
    }
}

pub(crate) fn mcp_trust_to_wire(trust: orbcode_mcp::McpServerTrust) -> McpServerTrust {
    match trust {
        orbcode_mcp::McpServerTrust::Unknown => McpServerTrust::Unknown,
        orbcode_mcp::McpServerTrust::Trusted => McpServerTrust::Trusted,
        orbcode_mcp::McpServerTrust::Denied => McpServerTrust::Denied,
    }
}

fn mcp_source_from_wire(source: McpServerSource) -> orbcode_mcp::McpServerSource {
    match source {
        McpServerSource::Plugin(plugin) => {
            orbcode_mcp::McpServerSource::Plugin(orbcode_mcp::McpPluginSource {
                plugin_id: plugin.plugin_id,
                plugin_name: plugin.plugin_name,
                server_name: plugin.server_name,
                source: plugin.source,
            })
        }
    }
}

fn mcp_source_to_wire(source: orbcode_mcp::McpServerSource) -> McpServerSource {
    match source {
        orbcode_mcp::McpServerSource::Plugin(plugin) => McpServerSource::Plugin(McpPluginSource {
            plugin_id: plugin.plugin_id,
            plugin_name: plugin.plugin_name,
            server_name: plugin.server_name,
            source: plugin.source,
        }),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use super::{mcp_oauth_overview_to_wire, mcp_server_overview_from_config, redact_endpoint};

    #[test]
    fn endpoint_redaction_removes_userinfo_query_and_fragment_values() {
        let endpoint = "https://user-canary:password-canary@example.com/mcp?token=token-canary&mode=safe#fragment-canary";
        let redacted = redact_endpoint(endpoint);
        for canary in [
            "user-canary",
            "password-canary",
            "token-canary",
            "fragment-canary",
        ] {
            assert!(!redacted.contains(canary), "leaked {canary}: {redacted}");
        }
        assert!(redacted.contains("example.com/mcp"));
        assert!(redacted.contains("token="));
        assert!(redacted.contains("mode="));
    }

    #[test]
    fn endpoint_without_credential_fields_is_byte_preserved() {
        let endpoint = "https://example.com/mcp";
        assert_eq!(redact_endpoint(endpoint), endpoint);
    }

    #[test]
    fn mcp_oauth_overview_preserves_non_secret_metadata() {
        let overview = mcp_oauth_overview_to_wire(orbcode_mcp::McpOAuthOverview {
            store_path: PathBuf::from("/tmp/oauth_tokens.json"),
            entries: vec![orbcode_mcp::McpOAuthStatusEntry {
                server_id: "remote".to_string(),
                source_summary: "stored:<redacted>".to_string(),
                usable: true,
                expired: false,
                has_refresh_token: true,
                has_token_endpoint: true,
                expires_at: Some(1_700_000_000),
                scopes: vec!["tools.read".to_string()],
                updated_at: Some(1_699_999_000),
            }],
        });

        let entry = &overview.entries[0];
        assert!(entry.has_token_endpoint);
        assert_eq!(entry.expires_at, Some(1_700_000_000));
        assert_eq!(entry.scopes, ["tools.read"]);
        assert_eq!(entry.updated_at, Some(1_699_999_000));
    }

    #[test]
    fn mcp_server_overview_projection_contains_no_secret_values() {
        let config = orbcode_mcp::McpServerConfig {
            id: "secret-bearing-server".to_string(),
            transport: orbcode_mcp::McpTransport::StreamableHttp,
            endpoint: "https://url-user-canary:url-password-canary@example.com/mcp?token=url-query-canary#url-fragment-canary".to_string(),
            args: vec!["arg-canary".to_string()],
            env: BTreeMap::from([("TOKEN".to_string(), "env-canary".to_string())]),
            cwd: None,
            headers: BTreeMap::from([(
                "Authorization".to_string(),
                "header-canary".to_string(),
            )]),
            enabled: true,
            status: orbcode_mcp::McpServerStatus::Failed,
            error: Some("error-canary".to_string()),
            summary: "safe summary".to_string(),
            auth: orbcode_mcp::McpAuth::Header {
                name: "X-Auth".to_string(),
                value: "auth-header-canary".to_string(),
            },
            trust: orbcode_mcp::McpServerTrust::Unknown,
            transport_type_hint: None,
            source: None,
        };

        let serialized = serde_json::to_string(&mcp_server_overview_from_config(config))
            .expect("serialize overview");
        for canary in [
            "url-user-canary",
            "url-password-canary",
            "url-query-canary",
            "url-fragment-canary",
            "arg-canary",
            "env-canary",
            "header-canary",
            "auth-header-canary",
            "error-canary",
        ] {
            assert!(
                !serialized.contains(canary),
                "leaked {canary}: {serialized}"
            );
        }
    }
}
