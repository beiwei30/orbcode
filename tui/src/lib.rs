mod app;
mod background_agent_panel;
mod bottom_pane;
mod chat;
mod clipboard;
mod commands;
mod custom_terminal;
mod dynamic_slash_commands;
mod editor_mode;
mod embedded_progress;
mod external_editor;
mod followups;
mod history_cell;
mod key_input;
mod keybindings;
mod line_cache;
mod mouse_interaction;
mod numeric;
mod overlays;
mod pickers;
mod prompt_state;
mod render;
mod render_metrics;
mod slash_commands;
mod state;
mod streaming;
mod syntax_highlight;
mod task_panel;
mod terminal_trace;
mod tool_cell;
mod transcript_task_cards;
mod tui_runtime;
mod tui_theme;
mod workspace_display;

pub use app::run_tui;

#[cfg(test)]
mod tests;
