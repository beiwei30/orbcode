//! Discovery for slash commands sourced from outside the built-in registry.
//!
//! Three sources feed the dynamic registry:
//! - `<home>/.claude/commands/**/*.md` — user-scoped prompt commands.
//! - `<cwd>/.claude/commands/**/*.md` — project-scoped prompt commands.
//! - `<plugin-root>/commands/**/*.md` — commands contributed by enabled
//!   plugins. Plugin commands are namespaced as `pluginName:relativePath` to
//!   prevent collisions with user/project entries.
//!
//! The discovery pass only reads markdown frontmatter; execution lives in
//! `slash_commands` (see `SlashCommandExecution::PromptExpansion`).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use orbcode_app_server_client::{AppClient, WorkflowSource};
use orbcode_config::{LoadedPlugin, PluginRegistry, load_plugin_registry};
use orbcode_tools::load_skill_definitions;

/// Owned-string spec produced by discovery. Consumed by
/// `register_dynamic_slash_commands` which leaks the strings into
/// `'static` to keep the runtime `SlashCommandSpec` `Copy`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DynamicSlashCommandSpec {
    pub(crate) name: String,
    pub(crate) aliases: Vec<String>,
    pub(crate) description: String,
    pub(crate) argument_hint: Option<String>,
    pub(crate) source: DynamicSlashCommandSource,
    pub(crate) hidden: bool,
    pub(crate) prompt_body: String,
    pub(crate) mcp_prompt: Option<McpPromptInfo>,
    pub(crate) workflow_name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DynamicSlashCommandSource {
    User,
    Project,
    Plugin {
        plugin_id: String,
        plugin_name: String,
    },
    Skill,
    McpPrompt {
        server_id: String,
    },
    Workflow {
        source: WorkflowSource,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct McpPromptInfo {
    pub(crate) server_id: String,
    pub(crate) prompt_name: String,
}

/// Walk user / project / plugin sources and build the dynamic spec list.
/// Duplicate names within the same source are dropped (first wins). Project
/// entries override user entries; plugin entries always namespace and so do
/// not collide.
pub(crate) async fn load_dynamic_slash_commands(
    home_dir: &Path,
    cwd: &Path,
) -> Vec<DynamicSlashCommandSpec> {
    let mut specs: Vec<DynamicSlashCommandSpec> = Vec::new();
    let mut seen_user_project: HashSet<String> = HashSet::new();

    let project_dir = cwd.join(".claude").join("commands");
    for spec in collect_commands_from_root(&project_dir, DynamicSlashCommandSource::Project) {
        if seen_user_project.insert(spec.name.clone()) {
            specs.push(spec);
        }
    }

    // `home_dir` is already the resolved `.claude` directory (either
    // `$ORBCODE_HOME` or `~/.claude`), so we join `commands` directly.
    let user_dir = home_dir.join("commands");
    for spec in collect_commands_from_root(&user_dir, DynamicSlashCommandSource::User) {
        if seen_user_project.insert(spec.name.clone()) {
            specs.push(spec);
        }
    }

    if let Ok(registry) = load_plugin_registry(home_dir, cwd).await {
        for spec in plugin_command_specs(&registry) {
            specs.push(spec);
        }
    }

    for spec in skill_command_specs(home_dir, cwd).await {
        if !seen_user_project.contains(&spec.name) {
            specs.push(spec);
        }
    }

    specs
}

async fn skill_command_specs(home_dir: &Path, cwd: &Path) -> Vec<DynamicSlashCommandSpec> {
    let skills = match load_skill_definitions(home_dir, cwd).await {
        Ok(skills) => skills,
        Err(_) => return Vec::new(),
    };
    skills
        .into_iter()
        .map(|skill| {
            let description = skill
                .when_to_use
                .as_deref()
                .or(skill.description.as_deref())
                .unwrap_or("")
                .to_string();
            let description = if description.is_empty() {
                format!("Run /{}", skill.name)
            } else {
                description
            };
            DynamicSlashCommandSpec {
                name: skill.name,
                aliases: Vec::new(),
                description,
                argument_hint: Some("<args>".to_string()),
                source: DynamicSlashCommandSource::Skill,
                hidden: false,
                prompt_body: skill.body,
                mcp_prompt: None,
                workflow_name: None,
            }
        })
        .collect()
}

fn plugin_command_specs(registry: &PluginRegistry) -> Vec<DynamicSlashCommandSpec> {
    let mut out = Vec::new();
    let mut seen_per_plugin: HashSet<String> = HashSet::new();
    for plugin in registry.enabled() {
        for spec in collect_plugin_commands(plugin) {
            if seen_per_plugin.insert(spec.name.clone()) {
                out.push(spec);
            }
        }
    }
    out
}

fn collect_plugin_commands(plugin: &LoadedPlugin) -> Vec<DynamicSlashCommandSpec> {
    let mut out = Vec::new();
    let base = plugin.root().join("commands");
    if !base.exists() {
        return out;
    }
    for path in &plugin.contributions.command_files {
        let Some(name) = plugin_command_name(plugin, &base, path) else {
            continue;
        };
        if let Some(spec) = parse_command_file(
            path,
            name,
            DynamicSlashCommandSource::Plugin {
                plugin_id: plugin.id.clone(),
                plugin_name: plugin.name.clone(),
            },
        ) {
            out.push(spec);
        }
    }
    out
}

fn plugin_command_name(plugin: &LoadedPlugin, base: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(base).ok()?;
    let mut segments: Vec<String> = relative
        .with_extension("")
        .components()
        .filter_map(|component| {
            component
                .as_os_str()
                .to_str()
                .map(std::string::ToString::to_string)
        })
        .collect();
    if segments.is_empty() {
        return None;
    }
    let leaf = segments.pop()?;
    let namespace = if segments.is_empty() {
        plugin.name.clone()
    } else {
        format!("{}:{}", plugin.name, segments.join(":"))
    };
    Some(format!("{namespace}:{leaf}"))
}

fn collect_commands_from_root(
    root: &Path,
    source: DynamicSlashCommandSource,
) -> Vec<DynamicSlashCommandSpec> {
    let mut out = Vec::new();
    if !root.exists() {
        return out;
    }
    let mut stack: Vec<PathBuf> = vec![root.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                stack.push(path);
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
                continue;
            }
            let Some(name) = local_command_name(root, &path) else {
                continue;
            };
            if let Some(spec) = parse_command_file(&path, name, source.clone()) {
                out.push(spec);
            }
        }
    }
    out.sort_by(|left, right| left.name.cmp(&right.name));
    out
}

fn local_command_name(root: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(root).ok()?;
    let segments: Vec<String> = relative
        .with_extension("")
        .components()
        .filter_map(|component| {
            component
                .as_os_str()
                .to_str()
                .map(std::string::ToString::to_string)
        })
        .collect();
    if segments.is_empty() {
        return None;
    }
    Some(segments.join(":"))
}

fn parse_command_file(
    path: &Path,
    name: String,
    source: DynamicSlashCommandSource,
) -> Option<DynamicSlashCommandSpec> {
    let contents = std::fs::read_to_string(path).ok()?;
    let (frontmatter, body) = split_frontmatter(&contents);
    let mut description = String::new();
    let mut argument_hint: Option<String> = None;
    let mut aliases: Vec<String> = Vec::new();
    let mut hidden = false;
    for (key, value) in frontmatter {
        match key.as_str() {
            "description" => description = value,
            "argument-hint" | "argument_hint" => {
                if !value.is_empty() {
                    argument_hint = Some(value);
                }
            }
            "aliases" => aliases = parse_alias_list(&value),
            "hidden" => {
                hidden = matches!(value.to_ascii_lowercase().as_str(), "true" | "yes" | "1");
            }
            // `disable-model-invocation` only tells the model not to auto-invoke
            // the command. Orb Code slash commands are interactive-only, so it must
            // NOT hide the command from the slash menu (mapping it to `hidden`
            // made such commands vanish).
            "disable-model-invocation" => {}
            _ => {}
        }
    }
    if description.is_empty() {
        description = extract_first_paragraph(body);
    }
    if description.is_empty() {
        description = format!("Run /{name}");
    }
    Some(DynamicSlashCommandSpec {
        name,
        aliases,
        description,
        argument_hint,
        source,
        hidden,
        prompt_body: body.trim_end().to_string(),
        mcp_prompt: None,
        workflow_name: None,
    })
}

fn split_frontmatter(contents: &str) -> (Vec<(String, String)>, &str) {
    let trimmed = contents.trim_start_matches('\u{feff}');
    let Some(rest) = trimmed.strip_prefix("---") else {
        return (Vec::new(), trimmed);
    };
    // Frontmatter opener must terminate on its own line. Tolerate a CRLF
    // (`---\r\n`) opener, not just LF.
    let rest = match rest
        .strip_prefix("\r\n")
        .or_else(|| rest.strip_prefix('\n'))
    {
        Some(rest) => rest,
        None => return (Vec::new(), trimmed),
    };
    let Some(end_index) = find_frontmatter_end(rest) else {
        return (Vec::new(), trimmed);
    };
    let frontmatter_text = &rest[..end_index];
    let body_start = end_index + "---".len();
    // Strip leading CR as well as LF, so a CRLF file does not leak a `\r` into
    // the prompt body's first line.
    let body = rest[body_start..].trim_start_matches(['\r', '\n']);
    (parse_simple_yaml(frontmatter_text), body)
}

fn find_frontmatter_end(text: &str) -> Option<usize> {
    let mut cursor = 0usize;
    while cursor < text.len() {
        let line_end = text[cursor..]
            .find('\n')
            .map_or(text.len(), |offset| cursor + offset);
        let line = text[cursor..line_end].trim_end_matches('\r');
        if line.trim_end() == "---" {
            return Some(cursor);
        }
        if line_end >= text.len() {
            break;
        }
        cursor = line_end + 1;
    }
    None
}

fn parse_simple_yaml(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for raw_line in text.lines() {
        let line = raw_line.trim_end_matches('\r');
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim().to_string();
        let value = strip_yaml_quotes(value.trim()).to_string();
        out.push((key, value));
    }
    out
}

fn strip_yaml_quotes(value: &str) -> &str {
    if value.len() >= 2 {
        let bytes = value.as_bytes();
        let first = bytes[0];
        let last = bytes[value.len() - 1];
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return &value[1..value.len() - 1];
        }
    }
    value
}

fn parse_alias_list(value: &str) -> Vec<String> {
    let trimmed = value.trim();
    let inner = trimmed.trim_start_matches('[').trim_end_matches(']');
    inner
        .split(',')
        .map(|alias| strip_yaml_quotes(alias.trim()).to_string())
        .filter(|alias| !alias.is_empty())
        .collect()
}

fn extract_first_paragraph(body: &str) -> String {
    for raw_line in body.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        return line.to_string();
    }
    String::new()
}

pub(crate) async fn load_mcp_prompt_commands(
    app_server: &AppClient,
) -> Vec<DynamicSlashCommandSpec> {
    let mut specs = Vec::new();
    let servers_value = match app_server.list_mcp_servers().await {
        Ok(value) => value,
        Err(_) => return specs,
    };
    let servers_arr = servers_value.as_array().cloned().unwrap_or_default();
    for server in &servers_arr {
        let server_id = match server["id"].as_str() {
            Some(id) => id,
            None => continue,
        };
        let enabled = server["enabled"].as_bool().unwrap_or(false);
        let trust = server["trust"].as_str().unwrap_or("");
        let status = server["status"].as_str().unwrap_or("");
        if !enabled || trust != "trusted" {
            continue;
        }
        if status != "ready" {
            continue;
        }
        let prompts_value = match app_server.list_mcp_prompts(server_id).await {
            Ok(value) => value,
            Err(_) => continue,
        };
        let prompts_arr = prompts_value.as_array().cloned().unwrap_or_default();
        for prompt in &prompts_arr {
            let prompt_name = match prompt["name"].as_str() {
                Some(n) => n.to_string(),
                None => continue,
            };
            let description = prompt["description"].as_str().unwrap_or("").to_string();
            let arguments = prompt["arguments"].as_array().cloned().unwrap_or_default();
            let argument_hint = if arguments.is_empty() {
                None
            } else {
                let args: Vec<String> = arguments
                    .iter()
                    .map(|arg| {
                        let name = arg["name"].as_str().unwrap_or("arg");
                        if arg["required"].as_bool().unwrap_or(false) {
                            format!("<{name}>")
                        } else {
                            format!("[{name}]")
                        }
                    })
                    .collect();
                Some(args.join(" "))
            };
            specs.push(DynamicSlashCommandSpec {
                name: format!("{server_id}:{prompt_name}"),
                aliases: Vec::new(),
                description,
                argument_hint,
                source: DynamicSlashCommandSource::McpPrompt {
                    server_id: server_id.to_string(),
                },
                hidden: false,
                prompt_body: String::new(),
                mcp_prompt: Some(McpPromptInfo {
                    server_id: server_id.to_string(),
                    prompt_name,
                }),
                workflow_name: None,
            });
        }
    }
    specs
}

pub(crate) async fn load_workflow_commands(app_server: &AppClient) -> Vec<DynamicSlashCommandSpec> {
    let workflows = match app_server.list_workflows().await {
        Ok(workflows) => workflows,
        Err(_) => return Vec::new(),
    };
    workflows
        .into_iter()
        .map(|workflow| DynamicSlashCommandSpec {
            name: format!("workflow:{}", workflow.name),
            aliases: Vec::new(),
            description: workflow.description,
            argument_hint: Some("<args>".to_string()),
            source: DynamicSlashCommandSource::Workflow {
                source: workflow.source,
            },
            hidden: false,
            prompt_body: String::new(),
            mcp_prompt: None,
            workflow_name: Some(workflow.name),
        })
        .collect()
}

/// Substitute `$ARGUMENTS` (full arg string) and `$1..$9` (positional args)
/// into a prompt-expansion body. Missing positional args render as the empty
/// string, matching the TypeScript behavior.
pub(crate) fn expand_prompt_body(body: &str, args: &str) -> String {
    let positional: Vec<&str> = args.split_whitespace().collect();
    let mut out = String::with_capacity(body.len() + args.len());
    let mut chars = body.char_indices().peekable();
    while let Some((index, character)) = chars.next() {
        if character != '$' {
            out.push(character);
            continue;
        }
        let Some(&(_, next_char)) = chars.peek() else {
            out.push(character);
            continue;
        };
        if next_char.is_ascii_digit() {
            chars.next();
            let digit = next_char.to_digit(10).unwrap_or(0) as usize;
            if digit >= 1
                && let Some(arg) = positional.get(digit - 1)
            {
                out.push_str(arg);
            }
            continue;
        }
        if body[index + 1..].starts_with("ARGUMENTS") {
            for _ in 0.."ARGUMENTS".len() {
                chars.next();
            }
            out.push_str(args);
            continue;
        }
        out.push(character);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn parses_frontmatter_and_body() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("greet.md");
        std::fs::write(
            &path,
            "---\ndescription: \"Say hello\"\nargument-hint: <name>\naliases: [hi, hello]\n---\nHi $1!\n",
        )
        .unwrap();
        let spec =
            parse_command_file(&path, "greet".into(), DynamicSlashCommandSource::User).unwrap();
        assert_eq!(spec.description, "Say hello");
        assert_eq!(spec.argument_hint.as_deref(), Some("<name>"));
        assert_eq!(spec.aliases, vec!["hi".to_string(), "hello".to_string()]);
        assert!(!spec.hidden);
        assert_eq!(spec.prompt_body, "Hi $1!");
    }

    #[test]
    fn falls_back_to_first_paragraph_when_description_missing() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("nofront.md");
        std::fs::write(&path, "First line of the prompt.\nMore body.\n").unwrap();
        let spec = parse_command_file(&path, "nofront".into(), DynamicSlashCommandSource::Project)
            .unwrap();
        assert_eq!(spec.description, "First line of the prompt.");
    }

    #[test]
    fn project_and_user_dirs_discover_commands_with_nested_namespaces() {
        let temp = tempdir().unwrap();
        let cwd = temp.path().join("project");
        // `home` here mimics the resolved claude-home (either ORBCODE_HOME or
        // ~/.claude) — it already IS the `.claude` directory, so commands
        // live at `home/commands/...` not `home/.claude/commands/...`.
        let home = temp.path().join("home");
        std::fs::create_dir_all(cwd.join(".claude").join("commands").join("group")).unwrap();
        std::fs::create_dir_all(home.join("commands")).unwrap();
        std::fs::write(
            cwd.join(".claude").join("commands").join("greet.md"),
            "---\ndescription: project greet\n---\nproject body\n",
        )
        .unwrap();
        std::fs::write(
            cwd.join(".claude")
                .join("commands")
                .join("group")
                .join("review.md"),
            "---\ndescription: review nested\n---\nreview body\n",
        )
        .unwrap();
        std::fs::write(
            home.join("commands").join("greet.md"),
            "---\ndescription: user greet\n---\nuser body\n",
        )
        .unwrap();

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let specs = runtime.block_on(load_dynamic_slash_commands(&home, &cwd));

        let names: Vec<_> = specs.iter().map(|spec| spec.name.clone()).collect();
        assert!(names.contains(&"greet".to_string()));
        assert!(names.contains(&"group:review".to_string()));
        let greet = specs.iter().find(|spec| spec.name == "greet").unwrap();
        assert!(matches!(greet.source, DynamicSlashCommandSource::Project));
        assert_eq!(greet.prompt_body, "project body");
    }

    #[test]
    fn expand_prompt_body_replaces_arguments() {
        let body = "Title: $1 :: rest: $ARGUMENTS";
        let expanded = expand_prompt_body(body, "alpha beta gamma");
        assert_eq!(expanded, "Title: alpha :: rest: alpha beta gamma");
    }

    #[test]
    fn expand_prompt_body_handles_missing_positional() {
        let body = "First: $1, Second: $2";
        let expanded = expand_prompt_body(body, "only");
        assert_eq!(expanded, "First: only, Second: ");
    }

    #[test]
    fn hidden_flag_recognized() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("secret.md");
        std::fs::write(&path, "---\ndescription: secret\nhidden: true\n---\nbody\n").unwrap();
        let spec =
            parse_command_file(&path, "secret".into(), DynamicSlashCommandSource::User).unwrap();
        assert!(spec.hidden);
    }

    #[test]
    fn disable_model_invocation_does_not_hide_command() {
        // `disable-model-invocation` blocks only model auto-invocation; the
        // command must stay visible in the interactive slash menu.
        let temp = tempdir().unwrap();
        let path = temp.path().join("manual.md");
        std::fs::write(
            &path,
            "---\ndescription: manual only\ndisable-model-invocation: true\n---\nbody\n",
        )
        .unwrap();
        let spec =
            parse_command_file(&path, "manual".into(), DynamicSlashCommandSource::User).unwrap();
        assert!(
            !spec.hidden,
            "disable-model-invocation must not hide the command"
        );
    }
}
