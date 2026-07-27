//! Config-side hook discovery.
//!
//! This module owns the *configuration* shape of hooks (the matcher/command
//! types parsed out of settings files) and the cross-layer discovery that walks
//! the `User → Project → Local → Managed` settings cascade plus any
//! skill/agent/plugin-contributed hooks. Discovery applies the same trust
//! gating the session boundary enforces (managed-only policy,
//! untrusted-project local hooks) and performs load-time command-shape
//! validation, while preserving each hook's source so diagnostics can answer
//! "which layer/extension contributed this hook, and is it trusted/valid".
//!
//! Lifecycle *execution* — environment construction, timeout, stdout/stderr
//! capture, and hook-result schema validation — lives in `orbcode-core` and is
//! intentionally out of scope here.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::policy::{EffectivePolicy, SettingsLayers, SettingsSource};

/// One `matcher` entry inside a hook event array, carrying the matcher pattern
/// and the commands to run when it matches.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct HookMatcher {
    #[serde(default)]
    pub matcher: Option<String>,
    #[serde(default)]
    pub hooks: Vec<HookCommand>,
}

/// A single hook action. Only `command` hooks are modeled; any other `type`
/// deserializes to [`HookCommand::Unsupported`] so an unknown action never
/// fails the whole settings parse.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type")]
pub enum HookCommand {
    #[serde(rename = "command")]
    Command {
        command: String,
        #[serde(default)]
        r#if: Option<String>,
        #[serde(default)]
        timeout: Option<f64>,
    },
    #[serde(other)]
    Unsupported,
}

/// Which settings layer a hook definition came from. Mirrors the
/// `User → Project → Local → Managed` precedence used everywhere else.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookLayer {
    User,
    Project,
    Local,
    Managed,
}

impl HookLayer {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Project => "project",
            Self::Local => "local",
            Self::Managed => "managed",
        }
    }

    fn from_source(source: SettingsSource) -> Self {
        match source {
            SettingsSource::User => Self::User,
            SettingsSource::Project => Self::Project,
            SettingsSource::Local => Self::Local,
            SettingsSource::Managed => Self::Managed,
        }
    }
}

/// Where a discovered hook came from. Settings hooks carry their layer;
/// skill/agent/plugin hooks carry the contributing extension's identifier so
/// source metadata survives discovery.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookProvenance {
    BuiltIn,
    Settings(HookLayer),
    Skill { name: String },
    Agent { name: String },
    Plugin { plugin_id: String },
}

impl HookProvenance {
    /// Short, diagnostic-safe label. Never includes managed file paths.
    pub fn label(&self) -> String {
        match self {
            Self::BuiltIn => "built-in".to_string(),
            Self::Settings(layer) => layer.as_str().to_string(),
            Self::Skill { name } => format!("skill:{name}"),
            Self::Agent { name } => format!("agent:{name}"),
            Self::Plugin { plugin_id } => format!("plugin:{plugin_id}"),
        }
    }
}

/// Outcome of load-time command-shape validation for a single hook command.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookValidationStatus {
    Valid,
    Invalid(String),
}

impl HookValidationStatus {
    pub fn is_valid(&self) -> bool {
        matches!(self, Self::Valid)
    }
}

/// A single discovered hook command with full provenance, trust, and
/// validation status. One entry is produced per command (a matcher with N
/// commands yields N entries) so diagnostics can address each command.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiscoveredHook {
    pub event: String,
    pub provenance: HookProvenance,
    pub matcher: Option<String>,
    /// The command text, or `<unsupported>` for non-command hook actions.
    pub command: String,
    /// Whether this hook would actually be registered after trust gating.
    pub trusted: bool,
    pub validation: HookValidationStatus,
}

impl DiscoveredHook {
    /// Render a single diagnostic line: source, event, matcher, command, and
    /// trust/validation status.
    pub fn summary_line(&self) -> String {
        let matcher = self.matcher.as_deref().unwrap_or("*");
        let trust = if self.trusted { "trusted" } else { "untrusted" };
        let validity = match &self.validation {
            HookValidationStatus::Valid => "valid".to_string(),
            HookValidationStatus::Invalid(reason) => format!("invalid: {reason}"),
        };
        format!(
            "[{}] {} ({}) -> {} ({trust}, {validity})",
            self.provenance.label(),
            self.event,
            matcher,
            self.command
        )
    }
}

/// A non-fatal problem found while discovering hooks (malformed hooks block or
/// an invalid command shape). Discovery keeps going past these.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HookDiscoveryWarning {
    pub provenance: HookProvenance,
    pub event: String,
    pub message: String,
}

impl HookDiscoveryWarning {
    pub fn summary_line(&self) -> String {
        if self.event.is_empty() {
            format!("[{}] {}", self.provenance.label(), self.message)
        } else {
            format!(
                "[{}] {}: {}",
                self.provenance.label(),
                self.event,
                self.message
            )
        }
    }
}

/// Hooks contributed by a skill, agent, or plugin, fed into discovery alongside
/// the settings cascade. The caller decides `trusted` (e.g. a project-sourced
/// agent's hooks are untrusted when the project is untrusted).
#[derive(Clone, Debug)]
pub struct ContributedHookSource {
    pub provenance: HookProvenance,
    pub trusted: bool,
    pub hooks: BTreeMap<String, Vec<HookMatcher>>,
}

/// The full result of cross-layer hook discovery.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct HookDiscovery {
    pub hooks: Vec<DiscoveredHook>,
    pub warnings: Vec<HookDiscoveryWarning>,
}

impl HookDiscovery {
    pub fn is_empty(&self) -> bool {
        self.hooks.is_empty()
    }

    pub fn trusted_count(&self) -> usize {
        self.hooks.iter().filter(|hook| hook.trusted).count()
    }

    pub fn invalid_count(&self) -> usize {
        self.hooks
            .iter()
            .filter(|hook| !hook.validation.is_valid())
            .count()
    }
}

/// Discover hooks across the four settings layers plus any contributed
/// skill/agent/plugin sources.
///
/// Trust gating mirrors the session boundary:
/// - when `policy.allow_managed_hooks_only` is set, only `Managed`-layer hooks
///   are trusted; user/project/local and all contributed hooks are surfaced but
///   marked untrusted;
/// - `Local`-layer hooks are untrusted when `trusted_project` is `false`.
///
/// Every discovered command is validated for shape (a `command` hook with an
/// empty command, or an unsupported hook action, is `Invalid`) and an invalid
/// command also emits a warning. Source metadata is preserved on every entry.
pub fn discover_hooks(
    layers: &SettingsLayers,
    policy: &EffectivePolicy,
    trusted_project: bool,
    contributed: &[ContributedHookSource],
) -> HookDiscovery {
    let mut discovery = HookDiscovery::default();

    for source in [
        SettingsSource::User,
        SettingsSource::Project,
        SettingsSource::Local,
        SettingsSource::Managed,
    ] {
        let layer = HookLayer::from_source(source);
        let provenance = HookProvenance::Settings(layer);
        let Some(settings_layer) = layers.get(source) else {
            continue;
        };
        let Some(raw) = settings_layer.raw.as_ref() else {
            continue;
        };
        let Some(hooks_value) = raw.get("hooks") else {
            continue;
        };
        let parsed: BTreeMap<String, Vec<HookMatcher>> =
            match serde_json::from_value(hooks_value.clone()) {
                Ok(parsed) => parsed,
                Err(error) => {
                    discovery.warnings.push(HookDiscoveryWarning {
                        provenance,
                        event: String::new(),
                        message: format!("malformed hooks block: {error}"),
                    });
                    continue;
                }
            };
        let trusted = settings_layer_trusted(layer, policy, trusted_project);
        collect_hooks(&parsed, &provenance, trusted, &mut discovery);
    }

    for source in contributed {
        // Contributed hooks are not managed settings, so the managed-only
        // policy gates them out just like user/project/local hooks.
        let trusted = source.trusted && !policy.allow_managed_hooks_only;
        collect_hooks(&source.hooks, &source.provenance, trusted, &mut discovery);
    }

    discovery
}

fn settings_layer_trusted(
    layer: HookLayer,
    policy: &EffectivePolicy,
    trusted_project: bool,
) -> bool {
    if policy.allow_managed_hooks_only && layer != HookLayer::Managed {
        return false;
    }
    if layer == HookLayer::Local && !trusted_project {
        return false;
    }
    true
}

fn collect_hooks(
    hooks: &BTreeMap<String, Vec<HookMatcher>>,
    provenance: &HookProvenance,
    trusted: bool,
    discovery: &mut HookDiscovery,
) {
    for (event, matchers) in hooks {
        for matcher in matchers {
            for command in &matcher.hooks {
                let (command_summary, validation) = inspect_command(command);
                if let HookValidationStatus::Invalid(reason) = &validation {
                    discovery.warnings.push(HookDiscoveryWarning {
                        provenance: provenance.clone(),
                        event: event.clone(),
                        message: reason.clone(),
                    });
                }
                discovery.hooks.push(DiscoveredHook {
                    event: event.clone(),
                    provenance: provenance.clone(),
                    matcher: matcher.matcher.clone(),
                    command: command_summary,
                    trusted,
                    validation,
                });
            }
        }
    }
}

/// Validate a single hook command's shape and return its display summary.
fn inspect_command(command: &HookCommand) -> (String, HookValidationStatus) {
    match command {
        HookCommand::Command { command, .. } => {
            if command.trim().is_empty() {
                (
                    "<empty>".to_string(),
                    HookValidationStatus::Invalid("command is empty".to_string()),
                )
            } else {
                (command.clone(), HookValidationStatus::Valid)
            }
        }
        HookCommand::Unsupported => (
            "<unsupported>".to_string(),
            HookValidationStatus::Invalid("unsupported hook type (expected `command`)".to_string()),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::{SettingsLayer, SettingsLayers};
    use serde_json::{Map, Value};
    use std::path::PathBuf;

    fn layer(source: SettingsSource, hooks_json: &str) -> SettingsLayer {
        let value: Value = serde_json::from_str(hooks_json).expect("valid json");
        let raw = value.as_object().cloned().unwrap_or_else(Map::new);
        SettingsLayer {
            source,
            primary_path: PathBuf::from(format!("/{}.json", source.short_label())),
            contributing_paths: Vec::new(),
            raw: Some(raw),
            errors: Vec::new(),
        }
    }

    fn layers(layers: Vec<SettingsLayer>) -> SettingsLayers {
        SettingsLayers { layers }
    }

    #[test]
    fn discovers_hooks_across_all_four_layers_with_source_metadata() {
        let layers = layers(vec![
            layer(
                SettingsSource::User,
                r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"echo user"}]}]}}"#,
            ),
            layer(
                SettingsSource::Project,
                r#"{"hooks":{"PreToolUse":[{"matcher":"Read","hooks":[{"type":"command","command":"echo project"}]}]}}"#,
            ),
            layer(
                SettingsSource::Local,
                r#"{"hooks":{"PostToolUse":[{"matcher":"Edit","hooks":[{"type":"command","command":"echo local"}]}]}}"#,
            ),
            layer(
                SettingsSource::Managed,
                r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"echo managed"}]}]}}"#,
            ),
        ]);
        let policy = EffectivePolicy::default();

        let discovery = discover_hooks(&layers, &policy, true, &[]);

        assert_eq!(discovery.hooks.len(), 4);
        assert!(discovery.warnings.is_empty());
        // Layers are walked in precedence order, so provenance follows it.
        let provenances: Vec<HookProvenance> = discovery
            .hooks
            .iter()
            .map(|hook| hook.provenance.clone())
            .collect();
        assert_eq!(
            provenances,
            vec![
                HookProvenance::Settings(HookLayer::User),
                HookProvenance::Settings(HookLayer::Project),
                HookProvenance::Settings(HookLayer::Local),
                HookProvenance::Settings(HookLayer::Managed),
            ]
        );
        // All trusted when project is trusted and no managed-only policy.
        assert!(discovery.hooks.iter().all(|hook| hook.trusted));
        assert_eq!(discovery.trusted_count(), 4);
    }

    #[test]
    fn untrusted_project_marks_local_hooks_untrusted() {
        let layers = layers(vec![
            layer(
                SettingsSource::User,
                r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"echo user"}]}]}}"#,
            ),
            layer(
                SettingsSource::Local,
                r#"{"hooks":{"PreToolUse":[{"matcher":"Read","hooks":[{"type":"command","command":"echo local"}]}]}}"#,
            ),
        ]);
        let policy = EffectivePolicy::default();

        let discovery = discover_hooks(&layers, &policy, false, &[]);

        let user = discovery
            .hooks
            .iter()
            .find(|hook| hook.provenance == HookProvenance::Settings(HookLayer::User))
            .expect("user hook");
        let local = discovery
            .hooks
            .iter()
            .find(|hook| hook.provenance == HookProvenance::Settings(HookLayer::Local))
            .expect("local hook");
        assert!(user.trusted, "user hook stays trusted");
        assert!(
            !local.trusted,
            "local hook untrusted when project untrusted"
        );
    }

    #[test]
    fn managed_only_policy_marks_non_managed_hooks_untrusted() {
        let layers = layers(vec![
            layer(
                SettingsSource::User,
                r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"echo user"}]}]}}"#,
            ),
            layer(
                SettingsSource::Managed,
                r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"echo managed"}]}]}}"#,
            ),
        ]);
        let policy = EffectivePolicy {
            allow_managed_hooks_only: true,
            ..EffectivePolicy::default()
        };

        let contributed = vec![ContributedHookSource {
            provenance: HookProvenance::Agent {
                name: "worker".to_string(),
            },
            trusted: true,
            hooks: BTreeMap::from([(
                "PreToolUse".to_string(),
                vec![HookMatcher {
                    matcher: Some("Bash".to_string()),
                    hooks: vec![HookCommand::Command {
                        command: "echo agent".to_string(),
                        r#if: None,
                        timeout: None,
                    }],
                }],
            )]),
        }];

        let discovery = discover_hooks(&layers, &policy, true, &contributed);

        let user = discovery
            .hooks
            .iter()
            .find(|hook| hook.provenance == HookProvenance::Settings(HookLayer::User))
            .expect("user hook");
        let managed = discovery
            .hooks
            .iter()
            .find(|hook| hook.provenance == HookProvenance::Settings(HookLayer::Managed))
            .expect("managed hook");
        let agent = discovery
            .hooks
            .iter()
            .find(|hook| matches!(hook.provenance, HookProvenance::Agent { .. }))
            .expect("agent hook");
        assert!(!user.trusted, "user hook gated by managed-only policy");
        assert!(managed.trusted, "managed hook stays trusted");
        assert!(
            !agent.trusted,
            "contributed hook gated by managed-only policy"
        );
    }

    #[test]
    fn validates_command_shape_and_emits_warnings() {
        let layers = layers(vec![layer(
            SettingsSource::User,
            r#"{"hooks":{"PreToolUse":[
                {"matcher":"Bash","hooks":[{"type":"command","command":"  "}]},
                {"matcher":"Read","hooks":[{"type":"webhook","url":"https://x"}]}
            ]}}"#,
        )]);
        let policy = EffectivePolicy::default();

        let discovery = discover_hooks(&layers, &policy, true, &[]);

        assert_eq!(discovery.hooks.len(), 2);
        assert_eq!(discovery.invalid_count(), 2);
        assert_eq!(discovery.warnings.len(), 2);
        let messages: Vec<&str> = discovery
            .warnings
            .iter()
            .map(|warning| warning.message.as_str())
            .collect();
        assert!(messages.iter().any(|message| message.contains("empty")));
        assert!(
            messages
                .iter()
                .any(|message| message.contains("unsupported"))
        );
    }

    #[test]
    fn malformed_hooks_block_warns_without_aborting() {
        let layers = layers(vec![
            layer(SettingsSource::User, r#"{"hooks":"not-an-object"}"#),
            layer(
                SettingsSource::Project,
                r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"echo ok"}]}]}}"#,
            ),
        ]);
        let policy = EffectivePolicy::default();

        let discovery = discover_hooks(&layers, &policy, true, &[]);

        // The malformed user layer warns but the valid project layer is still
        // discovered.
        assert_eq!(discovery.hooks.len(), 1);
        assert_eq!(
            discovery.hooks[0].provenance,
            HookProvenance::Settings(HookLayer::Project)
        );
        assert_eq!(discovery.warnings.len(), 1);
        assert!(discovery.warnings[0].message.contains("malformed"));
        assert_eq!(
            discovery.warnings[0].provenance,
            HookProvenance::Settings(HookLayer::User)
        );
    }

    #[test]
    fn preserves_skill_agent_plugin_provenance() {
        let layers = layers(vec![]);
        let policy = EffectivePolicy::default();
        let matcher = |command: &str| {
            BTreeMap::from([(
                "PreToolUse".to_string(),
                vec![HookMatcher {
                    matcher: Some("Bash".to_string()),
                    hooks: vec![HookCommand::Command {
                        command: command.to_string(),
                        r#if: None,
                        timeout: None,
                    }],
                }],
            )])
        };
        let contributed = vec![
            ContributedHookSource {
                provenance: HookProvenance::Skill {
                    name: "deploy".to_string(),
                },
                trusted: true,
                hooks: matcher("echo skill"),
            },
            ContributedHookSource {
                provenance: HookProvenance::Agent {
                    name: "worker".to_string(),
                },
                trusted: true,
                hooks: matcher("echo agent"),
            },
            ContributedHookSource {
                provenance: HookProvenance::Plugin {
                    plugin_id: "demo@market".to_string(),
                },
                trusted: true,
                hooks: matcher("echo plugin"),
            },
        ];

        let discovery = discover_hooks(&layers, &policy, true, &contributed);

        let labels: Vec<String> = discovery
            .hooks
            .iter()
            .map(|hook| hook.provenance.label())
            .collect();
        assert_eq!(
            labels,
            vec![
                "skill:deploy".to_string(),
                "agent:worker".to_string(),
                "plugin:demo@market".to_string(),
            ]
        );
        assert!(discovery.hooks.iter().all(|hook| hook.trusted));
    }
}
