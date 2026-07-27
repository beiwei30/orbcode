use std::path::{Path, PathBuf};

use orbcode_session_store::SessionStore;

mod adapters;
mod command;
mod events;
pub(crate) use command::HookCommandProgress;
pub(crate) use events::{
    run_permission_denied_hook_commands, run_post_tool_failure_hook_commands,
    run_post_tool_hook_commands, run_pre_tool_hook_commands, run_stop_failure_hook_commands,
    run_stop_hook_commands, run_subagent_start_hook_commands, run_subagent_stop_hook_commands,
    run_user_prompt_submit_hook_commands,
};

pub(crate) struct HookCommandContext<'a> {
    session_id: &'a str,
    transcript_store: &'a SessionStore,
    cwd: PathBuf,
}

impl<'a> HookCommandContext<'a> {
    pub(crate) fn new(session_id: &'a str, transcript_store: &'a SessionStore, cwd: &Path) -> Self {
        Self {
            session_id,
            transcript_store,
            cwd: cwd.to_path_buf(),
        }
    }

    fn transcript_path(&self) -> String {
        self.transcript_store
            .path(self.session_id)
            .display()
            .to_string()
    }

    fn agent_transcript_path(&self, child_session_id: &str) -> String {
        self.transcript_store
            .path(child_session_id)
            .display()
            .to_string()
    }

    fn cwd_display(&self) -> String {
        self.cwd.display().to_string()
    }
}
