//! Output-style discovery and request-time prompt integration.
//!
//! Output styles let users (and plugins) replace the default "tone" of
//! responses by injecting an instruction block into the request-time system
//! prompt. This module loads styles from four layers and resolves the
//! currently active style for a session.
//!
//! Precedence (lowest to highest, later layers win on name collision):
//! 1. Built-in styles (`default`, `Explanatory`, `Learning`).
//! 2. Plugin styles contributed by enabled plugins; these are namespaced
//!    `pluginName:styleName` so they cannot shadow user/project styles.
//! 3. User styles from `<home>/output-styles/*.md`.
//! 4. Project styles from `<cwd>/.claude/output-styles/*.md`.
//!
//! Each definition carries `OutputStyleSource` metadata so downstream
//! surfaces (`/output-style` listing, diagnostics) can report where a style
//! came from. The active style is resolved against the same precedence used
//! by `load_output_style_setting`.

use std::path::{Path, PathBuf};

use crate::ConfigError;
use crate::plugins::{LoadedPlugin, PluginRegistry, load_plugin_registry};
#[cfg(test)]
use crate::policy::{SettingsLayers, SettingsSource};

pub const DEFAULT_OUTPUT_STYLE_NAME: &str = "default";

/// Where an output-style definition was loaded from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OutputStyleSource {
    BuiltIn,
    UserSettings,
    ProjectSettings,
    Plugin { plugin_id: String },
}

impl OutputStyleSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::BuiltIn => "built-in",
            Self::UserSettings => "user",
            Self::ProjectSettings => "project",
            Self::Plugin { .. } => "plugin",
        }
    }
}

/// Why an output-style file produced a non-fatal diagnostic during loading.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputStyleWarningKind {
    /// Frontmatter contained a key the loader does not recognise. The known
    /// keys are kept; the file still loads.
    UnknownField,
    /// The file opened a `---` frontmatter block that was never closed, so it
    /// was skipped as malformed.
    InvalidFile,
}

/// A non-fatal diagnostic raised while loading output styles.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputStyleLoadWarning {
    pub kind: OutputStyleWarningKind,
    pub source: OutputStyleSource,
    pub path: Option<PathBuf>,
    pub message: String,
}

/// Result of loading output styles with diagnostics.
#[derive(Clone, Debug, Default)]
pub struct OutputStyleLoadOutcome {
    pub definitions: Vec<OutputStyleDefinition>,
    pub warnings: Vec<OutputStyleLoadWarning>,
}

/// Frontmatter keys the output-style parser recognises. Anything else is
/// reported via an [`OutputStyleWarningKind::UnknownField`] warning.
const KNOWN_OUTPUT_STYLE_FIELDS: &[&str] = &["name", "description"];

/// Outcome of parsing a single output-style markdown file.
enum OutputStyleParseOutcome {
    Parsed {
        definition: Box<OutputStyleDefinition>,
        unknown_fields: Vec<String>,
    },
    /// Frontmatter delimiter opened but never closed.
    Invalid(String),
}

/// Parsed output-style definition. `body` is the markdown content below any
/// frontmatter; it is what gets appended to the request-time system prompt
/// when this style is active. For built-in styles `path` is `None`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputStyleDefinition {
    pub name: String,
    pub description: String,
    pub body: String,
    pub source: OutputStyleSource,
    pub path: Option<PathBuf>,
}

impl OutputStyleDefinition {
    pub fn is_default(&self) -> bool {
        self.name.eq_ignore_ascii_case(DEFAULT_OUTPUT_STYLE_NAME)
    }

    /// Returns the instruction block that should be folded into the system
    /// prompt when this style is active, or `None` when the style has no
    /// body content (e.g. the built-in `default`).
    pub fn system_prompt_section(&self) -> Option<String> {
        let trimmed = self.body.trim();
        if trimmed.is_empty() {
            return None;
        }
        Some(format!(
            "\n\n## Output style: {name}\n\n{body}",
            name = self.name,
            body = trimmed,
        ))
    }
}

/// Returns the always-available built-in output-style definitions. Bodies
/// mirror the published behaviour of the corresponding TypeScript styles.
pub fn built_in_output_style_definitions() -> Vec<OutputStyleDefinition> {
    vec![
        OutputStyleDefinition {
            name: DEFAULT_OUTPUT_STYLE_NAME.to_string(),
            description: "Claude completes coding tasks efficiently and provides concise responses"
                .to_string(),
            body: String::new(),
            source: OutputStyleSource::BuiltIn,
            path: None,
        },
        OutputStyleDefinition {
            name: "Explanatory".to_string(),
            description: "Claude explains its implementation choices and codebase patterns"
                .to_string(),
            body: concat!(
                "Complete the coding task as requested, then explain your work.\n",
                "After finishing the implementation, add a short 'Insights' section ",
                "describing the non-obvious choices you made, the patterns you ",
                "followed in the surrounding codebase, and any trade-offs that ",
                "future maintainers would benefit from understanding."
            )
            .to_string(),
            source: OutputStyleSource::BuiltIn,
            path: None,
        },
        OutputStyleDefinition {
            name: "Learning".to_string(),
            description:
                "Claude pauses and asks you to write small pieces of code for hands-on practice"
                    .to_string(),
            body: concat!(
                "Work collaboratively with the user so they can learn while you ",
                "implement. When you reach a small, focused, educational ",
                "implementation step, stop and insert a `TODO(human)` marker ",
                "describing what the user should write themselves. Keep the ",
                "TODOs small and targeted so they can complete each one in a ",
                "few minutes. After the user fills in a TODO, continue with ",
                "the remaining work."
            )
            .to_string(),
            source: OutputStyleSource::BuiltIn,
            path: None,
        },
    ]
}

/// Loads all output-style definitions from built-in + plugin + user +
/// project layers, applying the precedence described in the module
/// header.
pub async fn load_output_style_definitions(
    home_dir: &Path,
    cwd: &Path,
) -> Result<Vec<OutputStyleDefinition>, ConfigError> {
    Ok(load_output_style_definitions_with_warnings(home_dir, cwd)
        .await?
        .definitions)
}

/// Like [`load_output_style_definitions`] but also returns non-fatal
/// diagnostics: files carrying unknown frontmatter keys, and malformed files
/// (unterminated frontmatter) that were skipped.
pub async fn load_output_style_definitions_with_warnings(
    home_dir: &Path,
    cwd: &Path,
) -> Result<OutputStyleLoadOutcome, ConfigError> {
    let mut definitions = built_in_output_style_definitions();
    let mut warnings: Vec<OutputStyleLoadWarning> = Vec::new();
    if let Ok(registry) = load_plugin_registry(home_dir, cwd).await {
        for definition in plugin_output_style_definitions(&registry) {
            upsert_definition(&mut definitions, definition);
        }
    }

    for (source, dir) in [
        (
            OutputStyleSource::UserSettings,
            home_dir.join("output-styles"),
        ),
        (
            OutputStyleSource::ProjectSettings,
            cwd.join(".claude").join("output-styles"),
        ),
    ] {
        let dir_outcome = load_output_styles_from_dir(&dir, source).await?;
        warnings.extend(dir_outcome.warnings);
        for definition in dir_outcome.definitions {
            upsert_definition(&mut definitions, definition);
        }
    }

    Ok(OutputStyleLoadOutcome {
        definitions,
        warnings,
    })
}

/// Converts enabled-plugin output-style files into definitions whose names
/// are namespaced (`pluginName:styleName`).
pub fn plugin_output_style_definitions(registry: &PluginRegistry) -> Vec<OutputStyleDefinition> {
    let mut results = Vec::new();
    for plugin in registry.enabled() {
        for path in &plugin.contributions.output_style_files {
            let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            let Ok(contents) = std::fs::read_to_string(path) else {
                continue;
            };
            let Some(mut definition) = parse_output_style_markdown(
                path,
                stem,
                &contents,
                OutputStyleSource::Plugin {
                    plugin_id: plugin.id.clone(),
                },
            ) else {
                continue;
            };
            definition.name = namespaced_plugin_style_name(plugin, &definition.name);
            results.push(definition);
        }
    }
    results
}

fn namespaced_plugin_style_name(plugin: &LoadedPlugin, bare: &str) -> String {
    format!("{}:{}", plugin.name, bare)
}

/// Resolved active style: the requested name plus the definition we used to
/// satisfy it. `requested` is what the settings cascade returned; when it
/// matches no loaded definition `definition` falls back to the built-in
/// default so callers always have something to inject.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedOutputStyle {
    pub requested: String,
    pub definition: OutputStyleDefinition,
    pub matched: bool,
}

impl ResolvedOutputStyle {
    pub fn system_prompt_section(&self) -> Option<String> {
        self.definition.system_prompt_section()
    }
}

/// Picks the active style from `definitions`. Matching is case-insensitive
/// to mirror the lookup used by `output_style_options`.
pub fn resolve_active_output_style(
    definitions: &[OutputStyleDefinition],
    requested: &str,
) -> ResolvedOutputStyle {
    if let Some(definition) = definitions
        .iter()
        .find(|definition| definition.name.eq_ignore_ascii_case(requested))
    {
        return ResolvedOutputStyle {
            requested: requested.to_string(),
            definition: definition.clone(),
            matched: true,
        };
    }
    let fallback = definitions
        .iter()
        .find(|definition| definition.is_default())
        .cloned()
        .unwrap_or_else(|| built_in_output_style_definitions().remove(0));
    ResolvedOutputStyle {
        requested: requested.to_string(),
        definition: fallback,
        matched: false,
    }
}

async fn load_output_styles_from_dir(
    dir: &Path,
    source: OutputStyleSource,
) -> Result<OutputStyleLoadOutcome, ConfigError> {
    let mut outcome = OutputStyleLoadOutcome::default();
    if !tokio::fs::try_exists(dir).await? {
        return Ok(outcome);
    }
    let mut entries = tokio::fs::read_dir(dir).await?;
    let mut markdown_paths = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("md") {
            continue;
        }
        markdown_paths.push(path);
    }
    markdown_paths.sort();

    for path in markdown_paths {
        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        let contents = tokio::fs::read_to_string(&path).await?;
        match parse_output_style_markdown_detailed(&path, stem, &contents, source.clone()) {
            OutputStyleParseOutcome::Parsed {
                definition,
                unknown_fields,
            } => {
                if !unknown_fields.is_empty() {
                    outcome.warnings.push(OutputStyleLoadWarning {
                        kind: OutputStyleWarningKind::UnknownField,
                        source: source.clone(),
                        path: Some(path.clone()),
                        message: format!(
                            "ignoring unknown output-style field(s) [{}] in {}",
                            unknown_fields.join(", "),
                            path.display(),
                        ),
                    });
                }
                upsert_definition(&mut outcome.definitions, *definition);
            }
            OutputStyleParseOutcome::Invalid(reason) => {
                outcome.warnings.push(OutputStyleLoadWarning {
                    kind: OutputStyleWarningKind::InvalidFile,
                    source: source.clone(),
                    path: Some(path.clone()),
                    message: format!("skipping {}: {reason}", path.display()),
                });
            }
        }
    }
    outcome
        .definitions
        .sort_by(|left, right| left.name.cmp(&right.name));
    Ok(outcome)
}

fn upsert_definition(
    definitions: &mut Vec<OutputStyleDefinition>,
    definition: OutputStyleDefinition,
) {
    if let Some(index) = definitions
        .iter()
        .position(|existing| existing.name.eq_ignore_ascii_case(&definition.name))
    {
        definitions[index] = definition;
    } else {
        definitions.push(definition);
    }
}

/// Parses an output-style markdown file. The file may use YAML frontmatter
/// with `name` and `description` keys; missing frontmatter is tolerated and
/// the file stem becomes the style name.
pub fn parse_output_style_markdown(
    path: &Path,
    file_stem: &str,
    contents: &str,
    source: OutputStyleSource,
) -> Option<OutputStyleDefinition> {
    match parse_output_style_markdown_detailed(path, file_stem, contents, source) {
        OutputStyleParseOutcome::Parsed { definition, .. } => Some(*definition),
        OutputStyleParseOutcome::Invalid(_) => None,
    }
}

/// Parses an output-style file, reporting unknown frontmatter keys and
/// rejecting files whose frontmatter delimiter is never closed. A file with no
/// frontmatter at all is valid: the stem becomes the name and the first
/// non-empty line seeds the description.
fn parse_output_style_markdown_detailed(
    path: &Path,
    file_stem: &str,
    contents: &str,
    source: OutputStyleSource,
) -> OutputStyleParseOutcome {
    let trimmed = contents.trim_start_matches('\u{feff}');
    let mut unknown_fields: Vec<String> = Vec::new();
    let (name, description, body) = match split_frontmatter(trimmed) {
        Some((frontmatter, body)) => {
            let mut name: Option<String> = None;
            let mut description: Option<String> = None;
            for (key, value) in parse_frontmatter_fields(frontmatter) {
                match key.as_str() {
                    "name" => name = Some(value),
                    "description" => description = Some(value),
                    _ => {
                        if !KNOWN_OUTPUT_STYLE_FIELDS.contains(&key.as_str())
                            && !unknown_fields.contains(&key)
                        {
                            unknown_fields.push(key);
                        }
                    }
                }
            }
            (
                name.map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| file_stem.to_string()),
                description
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| derive_description(file_stem, body)),
                body.trim().to_string(),
            )
        }
        None => {
            // An opening `---` with no closing delimiter is malformed; a file
            // with no delimiter at all is a plain-markdown style.
            if opens_frontmatter(trimmed) {
                return OutputStyleParseOutcome::Invalid(
                    "frontmatter block opened with `---` but never closed".to_string(),
                );
            }
            (
                file_stem.to_string(),
                derive_description(file_stem, trimmed),
                trimmed.trim().to_string(),
            )
        }
    };

    OutputStyleParseOutcome::Parsed {
        definition: Box::new(OutputStyleDefinition {
            name,
            description,
            body,
            source,
            path: Some(path.to_path_buf()),
        }),
        unknown_fields,
    }
}

fn opens_frontmatter(contents: &str) -> bool {
    contents.starts_with("---\n") || contents.starts_with("---\r\n")
}

fn derive_description(name: &str, body: &str) -> String {
    body.lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with("---"))
        .map(|line| line.trim_start_matches('#').trim().to_string())
        .filter(|line| !line.is_empty())
        .unwrap_or_else(|| format!("Custom {name} output style"))
}

fn split_frontmatter(contents: &str) -> Option<(&str, &str)> {
    let rest = contents
        .strip_prefix("---\n")
        .or_else(|| contents.strip_prefix("---\r\n"))?;
    let separator_index = rest.find("\n---")?;
    let frontmatter = &rest[..separator_index];
    let after_separator = &rest[separator_index + "\n---".len()..];
    let body = after_separator
        .strip_prefix('\n')
        .or_else(|| after_separator.strip_prefix("\r\n"))
        .unwrap_or(after_separator);
    Some((frontmatter, body))
}

fn parse_frontmatter_fields(frontmatter: &str) -> Vec<(String, String)> {
    let mut entries = Vec::new();
    for raw_line in frontmatter.lines() {
        let line = raw_line.trim_end();
        if line.is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let Some(colon) = line.find(':') else {
            continue;
        };
        let key = line[..colon].trim().to_string();
        if key.is_empty() {
            continue;
        }
        let value = strip_quotes(line[colon + 1..].trim()).to_string();
        entries.push((key, value));
    }
    entries
}

fn strip_quotes(value: &str) -> &str {
    let trimmed = value.trim();
    let bytes = trimmed.as_bytes();
    if bytes.len() >= 2 {
        let first = bytes[0];
        let last = bytes[bytes.len() - 1];
        if first == last && (first == b'"' || first == b'\'') {
            return &trimmed[1..trimmed.len() - 1];
        }
    }
    trimmed
}

/// Returns the output style name forced by managed policy, if any.
///
/// When the managed layer sets `"outputStyle"`, that value is locked (the key
/// appears in `managed_locked_keys`) and user/project choices are overridden.
/// This function extracts the raw string so callers can resolve it against the
/// loaded definitions without re-parsing the managed JSON.
#[cfg(test)]
pub(crate) fn managed_forced_output_style(layers: &SettingsLayers) -> Option<String> {
    let layer = layers.get(SettingsSource::Managed)?;
    let object = layer.raw.as_ref()?;
    object
        .get("outputStyle")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn fixture_path(name: &str) -> PathBuf {
        PathBuf::from(name)
    }

    async fn write_text(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.unwrap();
        }
        tokio::fs::write(path, contents).await.unwrap();
    }

    #[test]
    fn builtin_styles_include_default_explanatory_and_learning() {
        let definitions = built_in_output_style_definitions();
        let names: Vec<&str> = definitions
            .iter()
            .map(|definition| definition.name.as_str())
            .collect();
        assert_eq!(names, vec!["default", "Explanatory", "Learning"]);
        assert!(definitions[0].body.is_empty());
        assert!(!definitions[1].body.is_empty());
    }

    #[test]
    fn default_style_emits_no_system_prompt_section() {
        let definitions = built_in_output_style_definitions();
        assert!(definitions[0].system_prompt_section().is_none());
    }

    #[test]
    fn explanatory_style_emits_named_system_prompt_section() {
        let definitions = built_in_output_style_definitions();
        let section = definitions[1]
            .system_prompt_section()
            .expect("section present");
        assert!(section.starts_with("\n\n## Output style: Explanatory"));
        assert!(section.contains("Insights"));
    }

    #[test]
    fn parse_with_frontmatter_extracts_name_and_description() {
        let contents = concat!(
            "---\n",
            "name: Concise\n",
            "description: \"Short replies only\"\n",
            "---\n",
            "Respond in <=3 sentences.\n",
        );
        let definition = parse_output_style_markdown(
            &fixture_path("Concise.md"),
            "Concise",
            contents,
            OutputStyleSource::UserSettings,
        )
        .expect("parses");
        assert_eq!(definition.name, "Concise");
        assert_eq!(definition.description, "Short replies only");
        assert_eq!(definition.body, "Respond in <=3 sentences.");
        assert_eq!(definition.source, OutputStyleSource::UserSettings);
    }

    #[test]
    fn parse_without_frontmatter_uses_stem_and_first_line() {
        let contents = "# Verbose\n\nAlways include reasoning.\n";
        let definition = parse_output_style_markdown(
            &fixture_path("Verbose.md"),
            "Verbose",
            contents,
            OutputStyleSource::ProjectSettings,
        )
        .expect("parses");
        assert_eq!(definition.name, "Verbose");
        assert_eq!(definition.description, "Verbose");
        assert!(definition.body.contains("Always include reasoning"));
    }

    #[test]
    fn blank_frontmatter_name_falls_back_to_exact_file_stem() {
        let contents = concat!(
            "---\n",
            "name: \"   \"\n",
            "description: \"   \"\n",
            "---\n",
            "# First Line\n",
            "Body.\n",
        );
        let definition = parse_output_style_markdown(
            &fixture_path("Review Notes.v1.md"),
            "Review Notes.v1",
            contents,
            OutputStyleSource::UserSettings,
        )
        .expect("parses");
        assert_eq!(definition.name, "Review Notes.v1");
        assert_eq!(definition.description, "First Line");
    }

    #[test]
    fn resolve_active_falls_back_to_default_when_unknown() {
        let definitions = built_in_output_style_definitions();
        let resolved = resolve_active_output_style(&definitions, "Mystery");
        assert!(!resolved.matched);
        assert!(resolved.definition.is_default());
        assert_eq!(resolved.requested, "Mystery");
    }

    #[test]
    fn resolve_active_is_case_insensitive() {
        let definitions = built_in_output_style_definitions();
        let resolved = resolve_active_output_style(&definitions, "explanatory");
        assert!(resolved.matched);
        assert_eq!(resolved.definition.name, "Explanatory");
    }

    #[test]
    fn unknown_frontmatter_fields_are_reported() {
        let contents = concat!(
            "---\n",
            "name: Fancy\n",
            "description: a style\n",
            "color: blue\n",
            "verbosity: high\n",
            "---\n",
            "body\n",
        );
        let outcome = parse_output_style_markdown_detailed(
            &fixture_path("Fancy.md"),
            "Fancy",
            contents,
            OutputStyleSource::UserSettings,
        );
        match outcome {
            OutputStyleParseOutcome::Parsed {
                definition,
                unknown_fields,
            } => {
                assert_eq!(definition.name, "Fancy");
                assert_eq!(
                    unknown_fields,
                    vec!["color".to_string(), "verbosity".to_string()]
                );
            }
            OutputStyleParseOutcome::Invalid(reason) => panic!("unexpected invalid: {reason}"),
        }
    }

    #[test]
    fn unterminated_frontmatter_is_invalid() {
        let contents = "---\nname: Broken\ndescription: oops\nstill in frontmatter\n";
        let outcome = parse_output_style_markdown_detailed(
            &fixture_path("Broken.md"),
            "Broken",
            contents,
            OutputStyleSource::UserSettings,
        );
        assert!(matches!(outcome, OutputStyleParseOutcome::Invalid(_)));
        // The lossy wrapper drops it entirely.
        assert!(
            parse_output_style_markdown(
                &fixture_path("Broken.md"),
                "Broken",
                contents,
                OutputStyleSource::UserSettings,
            )
            .is_none()
        );
    }

    #[tokio::test]
    async fn loader_warns_on_unknown_fields_and_skips_invalid_files() {
        let temp = tempdir().expect("tempdir");
        let home = temp.path().join("home");
        let cwd = temp.path().join("project");
        let user_dir = home.join("output-styles");
        tokio::fs::create_dir_all(&user_dir).await.unwrap();
        tokio::fs::write(
            user_dir.join("Fancy.md"),
            "---\nname: Fancy\ndescription: d\ncolor: blue\n---\nbody",
        )
        .await
        .unwrap();
        tokio::fs::write(
            user_dir.join("Broken.md"),
            "---\nname: Broken\ndescription: d\nno closing delimiter\n",
        )
        .await
        .unwrap();

        let outcome = load_output_style_definitions_with_warnings(&home, &cwd)
            .await
            .unwrap();
        assert!(
            outcome
                .definitions
                .iter()
                .any(|definition| definition.name == "Fancy")
        );
        assert!(
            !outcome
                .definitions
                .iter()
                .any(|definition| definition.name == "Broken"),
            "invalid file should be skipped"
        );
        let kinds: Vec<_> = outcome.warnings.iter().map(|w| w.kind).collect();
        assert!(kinds.contains(&OutputStyleWarningKind::UnknownField));
        assert!(kinds.contains(&OutputStyleWarningKind::InvalidFile));
    }

    #[tokio::test]
    async fn loader_warnings_include_source_path_and_message_context() {
        let temp = tempdir().expect("tempdir");
        let home = temp.path().join("home");
        let cwd = temp.path().join("project");
        let user_path = home.join("output-styles").join("Fancy.md");
        let project_path = cwd.join(".claude").join("output-styles").join("Broken.md");
        write_text(
            &user_path,
            "---\nname: Fancy\ndescription: d\ncolor: blue\n---\nbody",
        )
        .await;
        write_text(
            &project_path,
            "---\nname: Broken\ndescription: d\nno closing delimiter\n",
        )
        .await;

        let outcome = load_output_style_definitions_with_warnings(&home, &cwd)
            .await
            .unwrap();

        let unknown = outcome
            .warnings
            .iter()
            .find(|warning| warning.kind == OutputStyleWarningKind::UnknownField)
            .expect("unknown-field warning");
        assert_eq!(unknown.source, OutputStyleSource::UserSettings);
        assert_eq!(unknown.path.as_deref(), Some(user_path.as_path()));
        assert!(unknown.message.contains("color"));
        assert!(unknown.message.contains(&user_path.display().to_string()));

        let invalid = outcome
            .warnings
            .iter()
            .find(|warning| warning.kind == OutputStyleWarningKind::InvalidFile)
            .expect("invalid-file warning");
        assert_eq!(invalid.source, OutputStyleSource::ProjectSettings);
        assert_eq!(invalid.path.as_deref(), Some(project_path.as_path()));
        assert!(invalid.message.contains("frontmatter block opened"));
        assert!(
            invalid
                .message
                .contains(&project_path.display().to_string())
        );
    }

    #[tokio::test]
    async fn empty_output_styles_dir_produces_no_warnings() {
        let temp = tempdir().expect("tempdir");
        let home = temp.path().join("home");
        let cwd = temp.path().join("project");
        tokio::fs::create_dir_all(home.join("output-styles"))
            .await
            .unwrap();
        let outcome = load_output_style_definitions_with_warnings(&home, &cwd)
            .await
            .unwrap();
        assert!(outcome.warnings.is_empty());
        // Built-ins still present.
        assert!(
            outcome
                .definitions
                .iter()
                .any(super::OutputStyleDefinition::is_default)
        );
    }

    #[tokio::test]
    async fn project_styles_override_user_styles() {
        let temp = tempdir().expect("tempdir");
        let home = temp.path().join("home");
        let cwd = temp.path().join("project");
        let user_dir = home.join("output-styles");
        let project_dir = cwd.join(".claude").join("output-styles");
        tokio::fs::create_dir_all(&user_dir).await.unwrap();
        tokio::fs::create_dir_all(&project_dir).await.unwrap();
        tokio::fs::write(
            user_dir.join("Concise.md"),
            "---\nname: Concise\ndescription: user-version\n---\nUser body.",
        )
        .await
        .unwrap();
        tokio::fs::write(
            project_dir.join("Concise.md"),
            "---\nname: Concise\ndescription: project-version\n---\nProject body.",
        )
        .await
        .unwrap();

        let definitions = load_output_style_definitions(&home, &cwd).await.unwrap();
        let concise = definitions
            .iter()
            .find(|definition| definition.name == "Concise")
            .expect("Concise present");
        assert_eq!(concise.description, "project-version");
        assert_eq!(concise.source, OutputStyleSource::ProjectSettings);
        assert!(concise.body.contains("Project body"));
    }

    #[tokio::test]
    async fn duplicate_names_within_one_source_use_sorted_path_order() {
        let temp = tempdir().expect("tempdir");
        let user_dir = temp.path().join("home").join("output-styles");
        write_text(
            &user_dir.join("a.md"),
            "---\nname: Shared\ndescription: first\n---\nFirst body.",
        )
        .await;
        write_text(
            &user_dir.join("b.md"),
            "---\nname: Shared\ndescription: second\n---\nSecond body.",
        )
        .await;

        let outcome = load_output_styles_from_dir(&user_dir, OutputStyleSource::UserSettings)
            .await
            .unwrap();

        assert_eq!(
            outcome
                .definitions
                .iter()
                .filter(|definition| definition.name == "Shared")
                .count(),
            1
        );
        let shared = outcome
            .definitions
            .iter()
            .find(|definition| definition.name == "Shared")
            .expect("shared style");
        let expected_path = user_dir.join("b.md");
        assert_eq!(shared.description, "second");
        assert_eq!(shared.path.as_deref(), Some(expected_path.as_path()));
        assert!(shared.body.contains("Second body"));
    }

    #[tokio::test]
    async fn user_styles_override_builtin_with_same_name() {
        let temp = tempdir().expect("tempdir");
        let home = temp.path().join("home");
        let cwd = temp.path().join("project");
        let user_dir = home.join("output-styles");
        tokio::fs::create_dir_all(&user_dir).await.unwrap();
        tokio::fs::write(
            user_dir.join("Explanatory.md"),
            "---\nname: Explanatory\ndescription: my override\n---\nDifferent guidance.",
        )
        .await
        .unwrap();

        let definitions = load_output_style_definitions(&home, &cwd).await.unwrap();
        let explanatory = definitions
            .iter()
            .find(|definition| definition.name == "Explanatory")
            .expect("Explanatory present");
        assert_eq!(explanatory.source, OutputStyleSource::UserSettings);
        assert!(explanatory.body.contains("Different guidance"));
    }

    #[tokio::test]
    async fn plugin_styles_are_namespaced_and_cannot_shadow_user_styles() {
        let temp = tempdir().expect("tempdir");
        let home = temp.path().join("home");
        let cwd = temp.path().join("project");
        let plugin_root = temp.path().join("cache").join("demo");
        tokio::fs::create_dir_all(plugin_root.join(".claude-plugin"))
            .await
            .unwrap();
        tokio::fs::write(
            plugin_root.join(".claude-plugin").join("plugin.json"),
            r#"{"name":"demo","version":"1.0.0"}"#,
        )
        .await
        .unwrap();
        tokio::fs::create_dir_all(plugin_root.join("output-styles"))
            .await
            .unwrap();
        tokio::fs::write(
            plugin_root.join("output-styles").join("Explanatory.md"),
            "---\nname: Explanatory\ndescription: plugin override\n---\nplugin body",
        )
        .await
        .unwrap();
        tokio::fs::create_dir_all(home.join("plugins"))
            .await
            .unwrap();
        let index = format!(
            r#"{{"version":2,"plugins":{{"demo@market":[{{"scope":"user","installPath":"{}","version":"1.0.0"}}]}}}}"#,
            plugin_root.display(),
        );
        tokio::fs::write(home.join("plugins").join("installed_plugins.json"), index)
            .await
            .unwrap();
        tokio::fs::create_dir_all(&home).await.unwrap();
        tokio::fs::write(
            home.join("settings.json"),
            r#"{"enabledPlugins":{"demo@market":true}}"#,
        )
        .await
        .unwrap();

        let definitions = load_output_style_definitions(&home, &cwd).await.unwrap();
        // Built-in Explanatory still wins because plugin name was namespaced.
        let builtin_explanatory = definitions
            .iter()
            .find(|definition| definition.name == "Explanatory")
            .expect("built-in Explanatory present");
        assert_eq!(builtin_explanatory.source, OutputStyleSource::BuiltIn);

        let plugin_style = definitions
            .iter()
            .find(|definition| definition.name == "demo:Explanatory")
            .expect("plugin style surfaced");
        assert_eq!(
            plugin_style.source,
            OutputStyleSource::Plugin {
                plugin_id: "demo@market".to_string()
            }
        );
        assert!(plugin_style.body.contains("plugin body"));
    }

    #[tokio::test]
    async fn project_style_overrides_plugin_style_with_same_namespaced_name() {
        let temp = tempdir().expect("tempdir");
        let home = temp.path().join("home");
        let cwd = temp.path().join("project");
        let plugin_root = temp.path().join("cache").join("demo");
        write_text(
            &plugin_root.join(".claude-plugin").join("plugin.json"),
            r#"{"name":"demo","version":"1.0.0"}"#,
        )
        .await;
        write_text(
            &plugin_root.join("output-styles").join("Concise.md"),
            "---\nname: Concise\ndescription: plugin version\n---\nPlugin body.",
        )
        .await;
        let index = format!(
            r#"{{"version":2,"plugins":{{"demo@market":[{{"scope":"user","installPath":"{}","version":"1.0.0"}}]}}}}"#,
            plugin_root.display(),
        );
        write_text(&home.join("plugins").join("installed_plugins.json"), &index).await;
        write_text(
            &home.join("settings.json"),
            r#"{"enabledPlugins":{"demo@market":true}}"#,
        )
        .await;
        write_text(
            &cwd.join(".claude").join("output-styles").join("Concise.md"),
            "---\nname: demo:Concise\ndescription: project version\n---\nProject body.",
        )
        .await;

        let definitions = load_output_style_definitions(&home, &cwd).await.unwrap();
        assert_eq!(
            definitions
                .iter()
                .filter(|definition| definition.name == "demo:Concise")
                .count(),
            1
        );
        let style = definitions
            .iter()
            .find(|definition| definition.name == "demo:Concise")
            .expect("namespaced style");
        assert_eq!(style.source, OutputStyleSource::ProjectSettings);
        assert_eq!(style.description, "project version");
        assert!(style.body.contains("Project body"));
    }

    #[test]
    fn managed_forced_output_style_extracts_from_managed_layer() {
        use crate::policy::{SettingsLayer, SettingsLayers};
        use serde_json::json;

        let managed_raw = json!({"outputStyle": "Explanatory"});
        let layers = SettingsLayers {
            layers: vec![
                SettingsLayer {
                    source: SettingsSource::User,
                    primary_path: PathBuf::from("/home/settings.json"),
                    contributing_paths: Vec::new(),
                    raw: None,
                    errors: Vec::new(),
                },
                SettingsLayer {
                    source: SettingsSource::Managed,
                    primary_path: PathBuf::from("/managed/managed-settings.json"),
                    contributing_paths: vec![PathBuf::from("/managed/managed-settings.json")],
                    raw: Some(managed_raw.as_object().unwrap().clone()),
                    errors: Vec::new(),
                },
            ],
        };
        assert_eq!(
            managed_forced_output_style(&layers),
            Some("Explanatory".to_string())
        );
    }

    #[test]
    fn managed_forced_output_style_returns_none_when_absent() {
        use crate::policy::{SettingsLayer, SettingsLayers};
        use serde_json::json;

        let managed_raw = json!({"model": "opus"});
        let layers = SettingsLayers {
            layers: vec![SettingsLayer {
                source: SettingsSource::Managed,
                primary_path: PathBuf::from("/managed/managed-settings.json"),
                contributing_paths: vec![PathBuf::from("/managed/managed-settings.json")],
                raw: Some(managed_raw.as_object().unwrap().clone()),
                errors: Vec::new(),
            }],
        };
        assert_eq!(managed_forced_output_style(&layers), None);
    }
}
