//! Full TypeScript-compatible settings precedence resolution.
//!
//! The TypeScript client merges settings from low-to-high priority sources and
//! tracks which source set each value. This module reproduces that model for
//! the complete eight-layer hierarchy:
//!
//! `defaults → user → project → local → managed → CLI flags → env vars →
//! session override`
//!
//! Later layers override earlier ones. Each resolved leaf value carries a
//! [`SourceAttribution`] naming the layer, the originating file (when the layer
//! is file-backed), and a JSON pointer to the value. Unknown and deprecated
//! top-level keys produce non-fatal [`SettingWarning`]s so startup can surface
//! actionable diagnostics without aborting.
//!
//! Note: the [`SettingsSource`](crate::policy::SettingsSource) enum in
//! [`crate::policy`] models only the four *file-backed* layers used for policy
//! resolution. [`SettingOrigin`] here is the broader runtime hierarchy that
//! also covers defaults, CLI flags, env vars, and the session override.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde_json::{Map, Value};

/// The eight settings precedence layers, ordered low-to-high priority. The
/// `Ord`/`PartialOrd` derives follow declaration order, so a later variant
/// compares greater and therefore wins during resolution.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SettingOrigin {
    Defaults,
    User,
    Project,
    Local,
    Managed,
    CliFlags,
    EnvVars,
    SessionOverride,
}

impl SettingOrigin {
    /// All layers in ascending priority order.
    pub const ORDER: [SettingOrigin; 8] = [
        SettingOrigin::Defaults,
        SettingOrigin::User,
        SettingOrigin::Project,
        SettingOrigin::Local,
        SettingOrigin::Managed,
        SettingOrigin::CliFlags,
        SettingOrigin::EnvVars,
        SettingOrigin::SessionOverride,
    ];

    /// 0-based priority index; higher wins.
    pub fn priority(self) -> u8 {
        match self {
            Self::Defaults => 0,
            Self::User => 1,
            Self::Project => 2,
            Self::Local => 3,
            Self::Managed => 4,
            Self::CliFlags => 5,
            Self::EnvVars => 6,
            Self::SessionOverride => 7,
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Defaults => "built-in defaults",
            Self::User => "user settings",
            Self::Project => "shared project settings",
            Self::Local => "project local settings",
            Self::Managed => "enterprise managed settings",
            Self::CliFlags => "command line arguments",
            Self::EnvVars => "environment variables",
            Self::SessionOverride => "current session",
        }
    }

    pub fn short_label(self) -> &'static str {
        match self {
            Self::Defaults => "defaults",
            Self::User => "user",
            Self::Project => "project",
            Self::Local => "local",
            Self::Managed => "managed",
            Self::CliFlags => "cli",
            Self::EnvVars => "env",
            Self::SessionOverride => "session",
        }
    }

    /// Only the three on-disk user-owned layers are editable in-app. Defaults,
    /// managed, CLI, env, and session override are all read-only surfaces.
    pub fn is_editable(self) -> bool {
        matches!(self, Self::User | Self::Project | Self::Local)
    }

    pub fn is_read_only(self) -> bool {
        !self.is_editable()
    }
}

/// Where a single resolved value came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceAttribution {
    pub origin: SettingOrigin,
    /// File that contributed the value, when the layer is file-backed.
    pub path: Option<PathBuf>,
    /// RFC 6901 JSON pointer to the value within the layer object. The empty
    /// string denotes the document root.
    pub pointer: String,
}

/// A resolved value paired with its winning source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttributedValue {
    pub value: Value,
    pub attribution: SourceAttribution,
}

/// One layer's raw contribution prior to resolution.
#[derive(Clone, Debug)]
pub struct LayerInput {
    pub origin: SettingOrigin,
    pub path: Option<PathBuf>,
    pub values: Map<String, Value>,
}

impl LayerInput {
    pub fn new(origin: SettingOrigin, path: Option<PathBuf>, values: Map<String, Value>) -> Self {
        Self {
            origin,
            path,
            values,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingWarningKind {
    UnknownKey,
    DeprecatedKey,
}

/// A non-fatal diagnostic about a setting key. Surfaced through `/status` so
/// users can find typos or stale keys without the client refusing to start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingWarning {
    pub origin: SettingOrigin,
    pub path: Option<PathBuf>,
    pub key: String,
    pub kind: SettingWarningKind,
    pub message: String,
}

/// Effective settings with per-value source attribution and load-time warnings.
#[derive(Clone, Debug, Default)]
pub struct ResolvedSettings {
    /// Flattened leaf pointer → winning value/source. Objects are recursed;
    /// arrays and scalars are treated as opaque leaves.
    leaves: BTreeMap<String, AttributedValue>,
    /// Top-level key → winning source, for quick `/status`-style lookups.
    top_level: BTreeMap<String, SourceAttribution>,
    pub warnings: Vec<SettingWarning>,
}

impl ResolvedSettings {
    /// Resolve layers in ascending priority. Later layers overwrite earlier
    /// ones at the leaf level, so a high-priority layer that only sets one
    /// nested field does not clobber sibling fields from lower layers.
    pub fn resolve(layers: &[LayerInput]) -> Self {
        let mut ordered: Vec<&LayerInput> = layers.iter().collect();
        ordered.sort_by_key(|layer| layer.origin.priority());

        let mut resolved = ResolvedSettings::default();
        for layer in ordered {
            for (key, value) in &layer.values {
                let pointer = format!("/{}", escape_pointer_token(key));
                flatten_into(
                    &mut resolved.leaves,
                    layer.origin,
                    layer.path.as_ref(),
                    &pointer,
                    value,
                );
                resolved.top_level.insert(
                    key.clone(),
                    SourceAttribution {
                        origin: layer.origin,
                        path: layer.path.clone(),
                        pointer,
                    },
                );
            }
            resolved.warnings.extend(collect_key_warnings(
                layer.origin,
                layer.path.as_ref(),
                &layer.values,
            ));
        }
        resolved
    }

    /// Source that ultimately set a top-level key, if any layer did.
    pub fn top_level_source(&self, key: &str) -> Option<&SourceAttribution> {
        self.top_level.get(key)
    }

    /// Winning value/source for a JSON pointer (leaf granularity).
    pub fn attributed(&self, pointer: &str) -> Option<&AttributedValue> {
        self.leaves.get(pointer)
    }

    /// Number of resolved leaf values. Useful for diagnostics.
    pub fn leaf_count(&self) -> usize {
        self.leaves.len()
    }

    pub fn warnings(&self) -> &[SettingWarning] {
        &self.warnings
    }
}

fn flatten_into(
    leaves: &mut BTreeMap<String, AttributedValue>,
    origin: SettingOrigin,
    path: Option<&PathBuf>,
    pointer: &str,
    value: &Value,
) {
    match value {
        Value::Object(object) if !object.is_empty() => {
            for (key, child) in object {
                let child_pointer = format!("{pointer}/{}", escape_pointer_token(key));
                flatten_into(leaves, origin, path, &child_pointer, child);
            }
        }
        _ => {
            leaves.insert(
                pointer.to_string(),
                AttributedValue {
                    value: value.clone(),
                    attribution: SourceAttribution {
                        origin,
                        path: path.cloned(),
                        pointer: pointer.to_string(),
                    },
                },
            );
        }
    }
}

/// Escape `~` and `/` per RFC 6901 so pointers round-trip for odd key names.
fn escape_pointer_token(token: &str) -> String {
    token.replace('~', "~0").replace('/', "~1")
}

fn collect_key_warnings(
    origin: SettingOrigin,
    path: Option<&PathBuf>,
    values: &Map<String, Value>,
) -> Vec<SettingWarning> {
    let mut warnings = Vec::new();
    for key in values.keys() {
        if let Some(replacement) = deprecated_key_replacement(key) {
            warnings.push(SettingWarning {
                origin,
                path: path.cloned(),
                key: key.clone(),
                kind: SettingWarningKind::DeprecatedKey,
                message: format!(
                    "`{key}` in {} is deprecated; use `{replacement}` instead",
                    origin.display_name()
                ),
            });
        } else if !is_known_top_level_key(key) {
            warnings.push(SettingWarning {
                origin,
                path: path.cloned(),
                key: key.clone(),
                kind: SettingWarningKind::UnknownKey,
                message: format!(
                    "unknown setting `{key}` in {} will be ignored",
                    origin.display_name()
                ),
            });
        }
    }
    warnings
}

/// Deprecated top-level keys mapped to their modern replacement.
fn deprecated_key_replacement(key: &str) -> Option<&'static str> {
    match key {
        "ignorePatterns" => Some("respectGitignore"),
        "autoUpdaterStatus" => Some("autoUpdates"),
        "allowedTools" => Some("permissions.allow"),
        "disallowedTools" => Some("permissions.deny"),
        _ => None,
    }
}

/// TypeScript-recognized top-level setting keys. Keys outside this set produce
/// an `UnknownKey` warning. The list intentionally errs toward inclusion of
/// keys Orb Code reads today plus the documented TypeScript schema fields.
fn is_known_top_level_key(key: &str) -> bool {
    const KNOWN: &[&str] = &[
        "$schema",
        "additionalDirectories",
        "advisorModel",
        "allowManagedHooksOnly",
        "allowManagedMcpServersOnly",
        "allowManagedPermissionRulesOnly",
        "allowedHttpHookUrls",
        "allowedMcpServers",
        "alwaysThinkingEnabled",
        "apiKeyHelper",
        "attribution",
        "autoMemoryDirectory",
        "autoMemoryEnabled",
        "autoMode",
        "autoUpdates",
        "availableModels",
        "awsAuthRefresh",
        "awsCredentialExport",
        "blockedMarketplaces",
        "cleanupPeriodDays",
        "companyAnnouncements",
        "defaultMode",
        "defaultShell",
        "deniedMcpServers",
        "disableAllHooks",
        "disableBypassPermissionsMode",
        "disabledMcpjsonServers",
        "editorMode",
        "effortLevel",
        "enableAllProjectMcpServers",
        "enabledMcpjsonServers",
        "enabledPlugins",
        "env",
        "extraKnownMarketplaces",
        "feedbackSurveyRate",
        "fileSuggestion",
        "forceLoginMethod",
        "forceLoginOrgUUID",
        "gcpAuthRefresh",
        "hooks",
        "httpHookAllowedEnvVars",
        "includeCoAuthoredBy",
        "includeGitInstructions",
        "language",
        "maxBudgetUsd",
        "maxBudgetUsdStrictUnknownPricing",
        "model",
        "modelOverrides",
        "modelType",
        "otelHeadersHelper",
        "outputStyle",
        "permissions",
        "plansDirectory",
        "pluginConfigs",
        "respectGitignore",
        "sandbox",
        "showThinkingSummaries",
        "skipWebFetchPreflight",
        "spinnerTipsEnabled",
        "statusLine",
        "statusLineEnabled",
        "strictKnownMarketplaces",
        "strictPluginOnlyCustomization",
        "theme",
        "useAutoModeDuringPlan",
        "verbose",
        "worktree",
    ];
    KNOWN.contains(&key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn layer(origin: SettingOrigin, value: Value) -> LayerInput {
        LayerInput::new(
            origin,
            file_path_for(origin),
            value.as_object().cloned().unwrap_or_default(),
        )
    }

    fn file_path_for(origin: SettingOrigin) -> Option<PathBuf> {
        origin
            .is_editable()
            .then(|| PathBuf::from(format!("/{}/settings.json", origin.short_label())))
    }

    #[test]
    fn order_is_low_to_high_priority() {
        assert_eq!(SettingOrigin::ORDER.len(), 8);
        for window in SettingOrigin::ORDER.windows(2) {
            assert!(window[0].priority() < window[1].priority());
        }
        assert!(SettingOrigin::SessionOverride > SettingOrigin::Managed);
        assert!(SettingOrigin::Managed > SettingOrigin::Local);
    }

    #[test]
    fn editable_layers_are_user_project_local_only() {
        assert!(SettingOrigin::User.is_editable());
        assert!(SettingOrigin::Project.is_editable());
        assert!(SettingOrigin::Local.is_editable());
        for origin in [
            SettingOrigin::Defaults,
            SettingOrigin::Managed,
            SettingOrigin::CliFlags,
            SettingOrigin::EnvVars,
            SettingOrigin::SessionOverride,
        ] {
            assert!(origin.is_read_only(), "{origin:?} should be read-only");
        }
    }

    #[test]
    fn higher_layer_overrides_top_level_value() {
        let layers = vec![
            layer(SettingOrigin::User, json!({"theme": "dark"})),
            layer(SettingOrigin::Project, json!({"theme": "light"})),
        ];
        let resolved = ResolvedSettings::resolve(&layers);
        let attributed = resolved.attributed("/theme").expect("theme leaf");
        assert_eq!(attributed.value, json!("light"));
        assert_eq!(attributed.attribution.origin, SettingOrigin::Project);
    }

    #[test]
    fn nested_leaves_merge_without_clobbering_siblings() {
        // User sets two nested keys; Local overrides only one. The other must
        // remain attributed to User rather than disappearing.
        let layers = vec![
            layer(
                SettingOrigin::User,
                json!({"permissions": {"defaultMode": "ask", "language": "en"}}),
            ),
            layer(
                SettingOrigin::Local,
                json!({"permissions": {"defaultMode": "acceptEdits"}}),
            ),
        ];
        let resolved = ResolvedSettings::resolve(&layers);
        let mode = resolved
            .attributed("/permissions/defaultMode")
            .expect("mode leaf");
        assert_eq!(mode.value, json!("acceptEdits"));
        assert_eq!(mode.attribution.origin, SettingOrigin::Local);

        let language = resolved
            .attributed("/permissions/language")
            .expect("language leaf");
        assert_eq!(language.value, json!("en"));
        assert_eq!(language.attribution.origin, SettingOrigin::User);
    }

    #[test]
    fn session_override_wins_over_managed() {
        let layers = vec![
            layer(SettingOrigin::Managed, json!({"model": "managed-model"})),
            layer(
                SettingOrigin::SessionOverride,
                json!({"model": "session-model"}),
            ),
        ];
        let resolved = ResolvedSettings::resolve(&layers);
        let model = resolved.attributed("/model").expect("model");
        assert_eq!(model.value, json!("session-model"));
        assert_eq!(model.attribution.origin, SettingOrigin::SessionOverride);
    }

    #[test]
    fn each_of_eight_layers_can_win_one_key() {
        // Give every layer a unique top-level key so each layer is the winner
        // for exactly one attribution.
        let layers = vec![
            layer(SettingOrigin::Defaults, json!({"d": 0})),
            layer(SettingOrigin::User, json!({"u": 1})),
            layer(SettingOrigin::Project, json!({"p": 2})),
            layer(SettingOrigin::Local, json!({"l": 3})),
            layer(SettingOrigin::Managed, json!({"m": 4})),
            layer(SettingOrigin::CliFlags, json!({"c": 5})),
            layer(SettingOrigin::EnvVars, json!({"e": 6})),
            layer(SettingOrigin::SessionOverride, json!({"s": 7})),
        ];
        let resolved = ResolvedSettings::resolve(&layers);
        assert_eq!(
            resolved.attributed("/d").unwrap().attribution.origin,
            SettingOrigin::Defaults
        );
        assert_eq!(
            resolved.attributed("/u").unwrap().attribution.origin,
            SettingOrigin::User
        );
        assert_eq!(
            resolved.attributed("/p").unwrap().attribution.origin,
            SettingOrigin::Project
        );
        assert_eq!(
            resolved.attributed("/l").unwrap().attribution.origin,
            SettingOrigin::Local
        );
        assert_eq!(
            resolved.attributed("/m").unwrap().attribution.origin,
            SettingOrigin::Managed
        );
        assert_eq!(
            resolved.attributed("/c").unwrap().attribution.origin,
            SettingOrigin::CliFlags
        );
        assert_eq!(
            resolved.attributed("/e").unwrap().attribution.origin,
            SettingOrigin::EnvVars
        );
        assert_eq!(
            resolved.attributed("/s").unwrap().attribution.origin,
            SettingOrigin::SessionOverride
        );
    }

    #[test]
    fn full_chain_resolves_to_highest_layer_for_shared_key() {
        let mut layers = Vec::new();
        for (index, origin) in SettingOrigin::ORDER.iter().enumerate() {
            layers.push(layer(*origin, json!({ "model": format!("model-{index}") })));
        }
        let resolved = ResolvedSettings::resolve(&layers);
        let model = resolved.attributed("/model").expect("model");
        assert_eq!(model.value, json!("model-7"));
        assert_eq!(model.attribution.origin, SettingOrigin::SessionOverride);
    }

    #[test]
    fn unknown_key_produces_warning() {
        let layers = vec![layer(SettingOrigin::User, json!({"definitelyNotReal": 1}))];
        let resolved = ResolvedSettings::resolve(&layers);
        assert_eq!(resolved.warnings.len(), 1);
        let warning = &resolved.warnings[0];
        assert_eq!(warning.kind, SettingWarningKind::UnknownKey);
        assert_eq!(warning.key, "definitelyNotReal");
        assert_eq!(warning.origin, SettingOrigin::User);
        assert!(warning.message.contains("unknown setting"));
    }

    #[test]
    fn deprecated_key_produces_warning_with_replacement() {
        let layers = vec![layer(SettingOrigin::Project, json!({"ignorePatterns": []}))];
        let resolved = ResolvedSettings::resolve(&layers);
        assert_eq!(resolved.warnings.len(), 1);
        assert_eq!(resolved.warnings[0].kind, SettingWarningKind::DeprecatedKey);
        assert!(resolved.warnings[0].message.contains("respectGitignore"));
    }

    #[test]
    fn known_keys_do_not_warn() {
        let layers = vec![layer(
            SettingOrigin::User,
            json!({"model": "x", "theme": "dark", "permissions": {"allow": []}, "env": {}}),
        )];
        let resolved = ResolvedSettings::resolve(&layers);
        assert!(resolved.warnings.is_empty(), "{:?}", resolved.warnings);
    }

    #[test]
    fn attribution_records_file_path_for_file_layers() {
        let layers = vec![layer(SettingOrigin::User, json!({"theme": "dark"}))];
        let resolved = ResolvedSettings::resolve(&layers);
        let theme = resolved.attributed("/theme").unwrap();
        assert_eq!(
            theme.attribution.path,
            Some(PathBuf::from("/user/settings.json"))
        );
    }

    #[test]
    fn pointer_escapes_special_characters() {
        let layers = vec![layer(SettingOrigin::User, json!({"env": {"A/B": "v"}}))];
        let resolved = ResolvedSettings::resolve(&layers);
        assert!(resolved.attributed("/env/A~1B").is_some());
    }
}
