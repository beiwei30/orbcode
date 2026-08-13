use std::future::Future;
use std::pin::Pin;
use std::sync::OnceLock;

use anyhow::Result;

use crate::clipboard::copy_text_to_clipboard;
use crate::commands::branch::BRANCH;
use crate::commands::builtin_prompts::builtin_prompt_body;
use crate::commands::dispatch::SlashCommandOutcome;
use crate::commands::effort::run_effort_slash_command;
use crate::commands::goal::GOAL;
use crate::commands::persisted_system::run_persisted_system_slash_command;
use crate::commands::review::REVIEW;
use crate::commands::tui_local::latest_assistant_text;
use crate::commands::utils::parse_model_argument;
use crate::commands::{CommandContext, CommandRegistry, SlashCommand};
use crate::editor_mode::editor_mode_next_setting;
use crate::overlays::{HelpOverlayState, OverlayState};
use crate::slash_commands::{
    AsyncLocalSlashCommand, BuiltinPromptSlashCommand, LocalOutputSlashCommand,
    TuiLocalSlashCommand,
};

pub(crate) fn command_registry() -> &'static CommandRegistry {
    static REGISTRY: OnceLock<CommandRegistry> = OnceLock::new();
    REGISTRY.get_or_init(build_command_registry)
}

fn build_command_registry() -> CommandRegistry {
    let mut r = CommandRegistry::new();

    // --- Fully extracted commands ---
    r.register(&["branch"], &BRANCH);
    r.register(&["clear", "new", "reset"], &CLEAR);
    r.register(&["help", "?"], &HELP);
    r.register(&["model"], &MODEL);
    r.register(&["effort"], &EFFORT);
    r.register(&["theme"], &THEME);
    r.register(&["vim"], &VIM);
    r.register(&["doctor"], &DOCTOR);
    r.register(&["status"], &STATUS);
    r.register(&["context", "ctx"], &CONTEXT);
    r.register(&["usage"], &USAGE);
    r.register(&["exit", "quit"], &EXIT);
    r.register(&["init"], &INIT);
    r.register(&["copy"], &COPY);
    r.register(&["goal"], &GOAL);
    r.register(&["review"], &REVIEW);

    // --- TuiLocal wrapper commands ---
    r.register(&["add-dir", "add-directory"], &ADD_DIR);
    r.register(&["compact"], &COMPACT);
    r.register(&["config"], &CONFIG);
    r.register(&["files"], &FILES);
    r.register(&["fork"], &FORK);
    r.register(&["keybindings"], &KEYBINDINGS);
    r.register(&["login"], &LOGIN);
    r.register(&["logout"], &LOGOUT);
    r.register(&["output-style"], &OUTPUT_STYLE);
    r.register(&["permissions"], &PERMISSIONS);
    r.register(&["plan"], &PLAN);
    r.register(&["release-notes"], &RELEASE_NOTES);
    r.register(&["rename"], &RENAME);
    r.register(&["resume", "session"], &RESUME);
    r.register(&["rewind", "checkpoint"], &REWIND);
    r.register(&["sandbox", "sandbox-toggle"], &SANDBOX);
    r.register(&["sessions"], &SESSIONS);

    r.register(&["jobs", "background"], &JOBS);

    // --- AsyncLocal wrapper commands ---
    r.register(&["agents"], &AGENTS);
    r.register(&["cost"], &COST);
    r.register(&["diff"], &DIFF);
    r.register(&["hooks"], &HOOKS);
    r.register(&["instructions"], &INSTRUCTIONS);
    r.register(&["memory"], &MEMORY);
    r.register(&["skills"], &SKILLS);
    r.register(&["stats"], &STATS);

    // --- LocalOutput commands ---
    r.register(&["trace", "last-request", "llm-request"], &LAST_REQUEST);
    r.register(&["tools"], &TOOLS);
    r.register(&["mcp"], &MCP);

    // --- Persisted-system commands ---
    r.register(&["tool"], &TOOL);

    r
}

// ============================================================================
// Fully extracted command implementations
// ============================================================================

static CLEAR: ClearCommand = ClearCommand;
struct ClearCommand;
impl SlashCommand for ClearCommand {
    fn execute<'a>(
        &self,
        ctx: CommandContext<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<SlashCommandOutcome>> + 'a>> {
        Box::pin(async move {
            if !ctx.args.is_empty() {
                return Err(anyhow::anyhow!("unknown slash command"));
            }
            let previous_session_id = ctx.state.session_id.clone();
            let previous_usage = orbcode_protocol::get_current_usage(&ctx.state.messages);
            let bootstrap = ctx
                .app_server
                .clear_session(&previous_session_id)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            *ctx.state = crate::state::TuiState::new(ctx.state.client.clone(), bootstrap);
            ctx.state.refresh_permission_mode(ctx.app_server).await;
            ctx.state.clear_session_info = Some(crate::state::ClearSessionInfo {
                session_id: previous_session_id,
                usage: previous_usage,
            });
            ctx.state.transcript_ui.emission.needs_scrollback_clear = true;
            ctx.state.pending_history_flush = true;
            ctx.state.set_status_line("Conversation cleared.");
            Ok(SlashCommandOutcome::Handled)
        })
    }
}

static HELP: HelpCommand = HelpCommand;
struct HelpCommand;
impl SlashCommand for HelpCommand {
    fn execute<'a>(
        &self,
        ctx: CommandContext<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<SlashCommandOutcome>> + 'a>> {
        Box::pin(async move {
            if !ctx.args.is_empty() {
                return Err(anyhow::anyhow!("unknown slash command"));
            }
            ctx.state.overlay = Some(OverlayState::Help(HelpOverlayState::default()));
            ctx.state
                .push_local_slash_command_output(ctx.line, "Opened help.", None);
            ctx.state.set_status_line("Help: ↑↓ scroll, Esc close.");
            Ok(SlashCommandOutcome::Handled)
        })
    }
}

static JOBS: JobsCommand = JobsCommand;
struct JobsCommand;
impl SlashCommand for JobsCommand {
    fn execute<'a>(
        &self,
        ctx: CommandContext<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<SlashCommandOutcome>> + 'a>> {
        Box::pin(async move {
            ctx.state.open_background_jobs_overlay(ctx.app_server).await;
            Ok(SlashCommandOutcome::Handled)
        })
    }
}

static MODEL: ModelCommand = ModelCommand;
struct ModelCommand;
impl SlashCommand for ModelCommand {
    fn execute<'a>(
        &self,
        ctx: CommandContext<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<SlashCommandOutcome>> + 'a>> {
        Box::pin(async move {
            if ctx.args.is_empty() {
                ctx.state
                    .open_model_picker(ctx.line, ctx.app_server)
                    .await?;
                return Ok(SlashCommandOutcome::Handled);
            }
            ctx.state
                .finish_model_selection(
                    ctx.app_server,
                    ctx.line,
                    parse_model_argument(ctx.args),
                    None,
                )
                .await?;
            Ok(SlashCommandOutcome::Handled)
        })
    }
}

static EFFORT: EffortCommand = EffortCommand;
struct EffortCommand;
impl SlashCommand for EffortCommand {
    fn execute<'a>(
        &self,
        ctx: CommandContext<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<SlashCommandOutcome>> + 'a>> {
        Box::pin(async move {
            let message =
                run_effort_slash_command(ctx.app_server, &ctx.state.session_id, ctx.args).await?;
            ctx.state.refresh_status_effort(ctx.app_server).await;
            ctx.state
                .push_local_slash_command_output(ctx.line, message.clone(), None);
            ctx.state.set_status_line(message);
            Ok(SlashCommandOutcome::Handled)
        })
    }
}

static THEME: ThemeCommand = ThemeCommand;
struct ThemeCommand;
impl SlashCommand for ThemeCommand {
    fn execute<'a>(
        &self,
        ctx: CommandContext<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<SlashCommandOutcome>> + 'a>> {
        Box::pin(async move {
            if !ctx.args.is_empty() {
                return Err(anyhow::anyhow!("unknown slash command"));
            }
            ctx.state
                .open_theme_picker(ctx.line, ctx.app_server)
                .await?;
            Ok(SlashCommandOutcome::Handled)
        })
    }
}

static VIM: VimCommand = VimCommand;
struct VimCommand;
impl SlashCommand for VimCommand {
    fn execute<'a>(
        &self,
        ctx: CommandContext<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<SlashCommandOutcome>> + 'a>> {
        Box::pin(async move {
            if !ctx.args.is_empty() {
                return Err(anyhow::anyhow!("unknown slash command"));
            }
            let message = ctx
                .state
                .set_editor_mode_setting(
                    ctx.app_server,
                    editor_mode_next_setting(ctx.state.editor_mode),
                )
                .await?;
            ctx.state
                .push_local_slash_command_output(ctx.line, message.clone(), None);
            ctx.state.set_status_line(message);
            Ok(SlashCommandOutcome::Handled)
        })
    }
}

static DOCTOR: DoctorCommand = DoctorCommand;
struct DoctorCommand;
impl SlashCommand for DoctorCommand {
    fn execute<'a>(
        &self,
        ctx: CommandContext<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<SlashCommandOutcome>> + 'a>> {
        Box::pin(async move {
            if !ctx.args.is_empty() {
                return Err(anyhow::anyhow!("usage: /doctor"));
            }
            ctx.state.start_async_local_slash_command(
                AsyncLocalSlashCommand::Doctor,
                ctx.app_server,
                ctx.local_command_tx,
            );
            Ok(SlashCommandOutcome::Handled)
        })
    }
}

static STATUS: StatusCommand = StatusCommand;
struct StatusCommand;
impl SlashCommand for StatusCommand {
    fn execute<'a>(
        &self,
        ctx: CommandContext<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<SlashCommandOutcome>> + 'a>> {
        Box::pin(async move {
            if !ctx.args.is_empty() {
                return Err(anyhow::anyhow!("usage: /status"));
            }
            ctx.state.start_async_local_slash_command(
                AsyncLocalSlashCommand::Status,
                ctx.app_server,
                ctx.local_command_tx,
            );
            Ok(SlashCommandOutcome::Handled)
        })
    }
}

static CONTEXT: ContextCommand = ContextCommand;
struct ContextCommand;
impl SlashCommand for ContextCommand {
    fn execute<'a>(
        &self,
        ctx: CommandContext<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<SlashCommandOutcome>> + 'a>> {
        Box::pin(async move {
            let full = match ctx.args.trim() {
                "" => false,
                "--full" => true,
                other => {
                    return Err(anyhow::anyhow!(
                        "unsupported /context argument `{other}`; expected --full"
                    ));
                }
            };
            ctx.state.start_context_slash_command(
                ctx.line.to_string(),
                full,
                ctx.app_server,
                ctx.local_command_tx,
            );
            Ok(SlashCommandOutcome::Handled)
        })
    }
}

static USAGE: UsageCommand = UsageCommand;
struct UsageCommand;
impl SlashCommand for UsageCommand {
    fn execute<'a>(
        &self,
        ctx: CommandContext<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<SlashCommandOutcome>> + 'a>> {
        Box::pin(async move {
            if !ctx.args.is_empty() {
                return Err(anyhow::anyhow!("usage: /usage"));
            }
            ctx.state.start_async_local_slash_command(
                AsyncLocalSlashCommand::Usage,
                ctx.app_server,
                ctx.local_command_tx,
            );
            Ok(SlashCommandOutcome::Handled)
        })
    }
}

static EXIT: ExitCommand = ExitCommand;
struct ExitCommand;
impl SlashCommand for ExitCommand {
    fn execute<'a>(
        &self,
        ctx: CommandContext<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<SlashCommandOutcome>> + 'a>> {
        Box::pin(async move {
            if !ctx.args.is_empty() {
                return Err(anyhow::anyhow!("usage: /exit"));
            }
            ctx.state
                .push_local_slash_command_output(ctx.line, "Exiting.", None);
            Ok(SlashCommandOutcome::Exit)
        })
    }
}

static INIT: InitCommand = InitCommand;
struct InitCommand;
impl SlashCommand for InitCommand {
    fn execute<'a>(
        &self,
        ctx: CommandContext<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<SlashCommandOutcome>> + 'a>> {
        Box::pin(async move {
            if !ctx.args.is_empty() {
                return Err(anyhow::anyhow!("usage: /init"));
            }
            ctx.state.push_local_slash_command_output(
                ctx.line,
                "Analyzing your codebase to set up CLAUDE.md...",
                None,
            );
            Ok(SlashCommandOutcome::PromptToSubmit(
                builtin_prompt_body(BuiltinPromptSlashCommand::Init).to_string(),
            ))
        })
    }
}

static COPY: CopyCommand = CopyCommand;
struct CopyCommand;
impl SlashCommand for CopyCommand {
    fn execute<'a>(
        &self,
        ctx: CommandContext<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<SlashCommandOutcome>> + 'a>> {
        Box::pin(async move {
            if !ctx.args.is_empty() {
                return Err(anyhow::anyhow!("unknown slash command"));
            }
            let text = latest_assistant_text(&ctx.state.messages)
                .ok_or_else(|| anyhow::anyhow!("No assistant response available to copy yet."))?;
            let char_count = text.chars().count();
            copy_text_to_clipboard(&text)?;
            let message = format!("Copied last assistant response ({char_count} chars).");
            ctx.state
                .push_local_slash_command_output(ctx.line, message.clone(), None);
            ctx.state.set_status_line(message);
            Ok(SlashCommandOutcome::Handled)
        })
    }
}

// ============================================================================
// TuiLocal wrapper commands — delegate to existing run_tui_local_slash_command
// ============================================================================

macro_rules! tui_local_command {
    ($static_name:ident, $struct_name:ident, $variant:ident) => {
        static $static_name: $struct_name = $struct_name;
        struct $struct_name;
        impl SlashCommand for $struct_name {
            fn execute<'a>(
                &self,
                ctx: CommandContext<'a>,
            ) -> Pin<Box<dyn Future<Output = Result<SlashCommandOutcome>> + 'a>> {
                Box::pin(async move {
                    ctx.state
                        .run_tui_local_slash_command(
                            TuiLocalSlashCommand::$variant,
                            ctx.args,
                            ctx.line,
                            ctx.app_server,
                            ctx.local_command_tx,
                        )
                        .await?;
                    Ok(SlashCommandOutcome::Handled)
                })
            }
        }
    };
}

tui_local_command!(ADD_DIR, AddDirCommand, AddDir);
tui_local_command!(COMPACT, CompactCommand, Compact);
tui_local_command!(CONFIG, ConfigCommand, Config);
tui_local_command!(FILES, FilesCommand, Files);
tui_local_command!(FORK, ForkCommand, Fork);
tui_local_command!(KEYBINDINGS, KeybindingsCommand, Keybindings);
tui_local_command!(LOGIN, LoginCommand, Login);
tui_local_command!(LOGOUT, LogoutCommand, Logout);
tui_local_command!(OUTPUT_STYLE, OutputStyleCommand, OutputStyle);
tui_local_command!(PERMISSIONS, PermissionsCommand, Permissions);
tui_local_command!(PLAN, PlanCommand, Plan);
tui_local_command!(RELEASE_NOTES, ReleaseNotesCommand, ReleaseNotes);
tui_local_command!(RENAME, RenameCommand, Rename);
tui_local_command!(RESUME, ResumeCommand, Resume);
tui_local_command!(REWIND, RewindCommand, Rewind);
tui_local_command!(SANDBOX, SandboxCommand, Sandbox);
tui_local_command!(SESSIONS, SessionsCommand, Sessions);

// ============================================================================
// AsyncLocal wrapper commands — delegate to start_async_local_slash_command
// ============================================================================

macro_rules! async_local_command {
    ($static_name:ident, $struct_name:ident, $variant:ident) => {
        static $static_name: $struct_name = $struct_name;
        struct $struct_name;
        impl SlashCommand for $struct_name {
            fn execute<'a>(
                &self,
                ctx: CommandContext<'a>,
            ) -> Pin<Box<dyn Future<Output = Result<SlashCommandOutcome>> + 'a>> {
                Box::pin(async move {
                    if !ctx.args.is_empty() {
                        return Err(anyhow::anyhow!("this command takes no arguments"));
                    }
                    ctx.state.start_async_local_slash_command(
                        AsyncLocalSlashCommand::$variant,
                        ctx.app_server,
                        ctx.local_command_tx,
                    );
                    Ok(SlashCommandOutcome::Handled)
                })
            }
        }
    };
}

async_local_command!(AGENTS, AgentsCommand, Agents);
async_local_command!(COST, CostCommand, Cost);
async_local_command!(DIFF, DiffCommand, Diff);
async_local_command!(HOOKS, HooksCommand, Hooks);
async_local_command!(INSTRUCTIONS, InstructionsCommand, Instructions);
static MEMORY: MemoryCommand = MemoryCommand;
struct MemoryCommand;
impl SlashCommand for MemoryCommand {
    fn execute<'a>(
        &self,
        ctx: CommandContext<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<SlashCommandOutcome>> + 'a>> {
        Box::pin(async move {
            match ctx.args.trim() {
                "" => {
                    ctx.state.start_async_local_slash_command(
                        AsyncLocalSlashCommand::Memory,
                        ctx.app_server,
                        ctx.local_command_tx,
                    );
                }
                "auto on" => {
                    ctx.app_server
                        .set_auto_memory_enabled(true)
                        .await
                        .map_err(|e| anyhow::anyhow!("{e}"))?;
                    let message = "Auto-memory enabled.";
                    ctx.state
                        .push_local_slash_command_output(ctx.line, message, None);
                    ctx.state.set_status_line(message);
                }
                "auto off" => {
                    ctx.app_server
                        .set_auto_memory_enabled(false)
                        .await
                        .map_err(|e| anyhow::anyhow!("{e}"))?;
                    let message = "Auto-memory disabled.";
                    ctx.state
                        .push_local_slash_command_output(ctx.line, message, None);
                    ctx.state.set_status_line(message);
                }
                _ => {
                    return Err(anyhow::anyhow!("usage: /memory [auto on|off]"));
                }
            }
            Ok(SlashCommandOutcome::Handled)
        })
    }
}
async_local_command!(SKILLS, SkillsCommand, Skills);
async_local_command!(STATS, StatsCommand, Stats);

// ============================================================================
// LocalOutput commands
// ============================================================================

static LAST_REQUEST: LastRequestCommand = LastRequestCommand;
struct LastRequestCommand;
impl SlashCommand for LastRequestCommand {
    fn execute<'a>(
        &self,
        ctx: CommandContext<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<SlashCommandOutcome>> + 'a>> {
        Box::pin(async move {
            ctx.state
                .run_local_output_slash_command(
                    LocalOutputSlashCommand::LastRequest,
                    ctx.args,
                    ctx.line,
                    ctx.app_server,
                )
                .await?;
            Ok(SlashCommandOutcome::Handled)
        })
    }
}

static TOOLS: ToolsCommand = ToolsCommand;
struct ToolsCommand;
impl SlashCommand for ToolsCommand {
    fn execute<'a>(
        &self,
        ctx: CommandContext<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<SlashCommandOutcome>> + 'a>> {
        Box::pin(async move {
            ctx.state
                .run_local_output_slash_command(
                    LocalOutputSlashCommand::Tools,
                    ctx.args,
                    ctx.line,
                    ctx.app_server,
                )
                .await?;
            Ok(SlashCommandOutcome::Handled)
        })
    }
}

static MCP: McpCommand = McpCommand;
struct McpCommand;
impl SlashCommand for McpCommand {
    fn execute<'a>(
        &self,
        ctx: CommandContext<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<SlashCommandOutcome>> + 'a>> {
        Box::pin(async move {
            if LocalOutputSlashCommand::McpInspection.handles_args(ctx.args) {
                ctx.state
                    .run_local_output_slash_command(
                        LocalOutputSlashCommand::McpInspection,
                        ctx.args,
                        ctx.line,
                        ctx.app_server,
                    )
                    .await?;
                return Ok(SlashCommandOutcome::Handled);
            }
            // `read` and `call` subcommands fall through to persisted-system
            if let Some(result) =
                run_persisted_system_slash_command("mcp", ctx.args, ctx.app_server).await?
            {
                let status = result.status;
                ctx.state
                    .push_local_slash_command_output(ctx.line, status.clone(), None);
                ctx.state
                    .push_persisted_system_message(ctx.app_server, result.note)
                    .await?;
                ctx.state.set_status_line(status);
                return Ok(SlashCommandOutcome::Handled);
            }
            Err(anyhow::anyhow!("unknown slash command"))
        })
    }
}

// ============================================================================
// Persisted-system commands
// ============================================================================

static TOOL: ToolCommand = ToolCommand;
struct ToolCommand;
impl SlashCommand for ToolCommand {
    fn execute<'a>(
        &self,
        ctx: CommandContext<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<SlashCommandOutcome>> + 'a>> {
        Box::pin(async move {
            if let Some(result) =
                run_persisted_system_slash_command("tool", ctx.args, ctx.app_server).await?
            {
                let status = result.status;
                ctx.state
                    .push_local_slash_command_output(ctx.line, status.clone(), None);
                ctx.state
                    .push_persisted_system_message(ctx.app_server, result.note)
                    .await?;
                ctx.state.set_status_line(status);
                return Ok(SlashCommandOutcome::Handled);
            }
            Err(anyhow::anyhow!("unknown slash command"))
        })
    }
}
