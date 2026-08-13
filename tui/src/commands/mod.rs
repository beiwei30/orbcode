use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

use anyhow::Result;
use orbcode_app_server_client::AppClient;
use tokio::sync::mpsc;

use crate::commands::async_local::LocalCommandEnvelope;
use crate::commands::dispatch::SlashCommandOutcome;
use crate::state::TuiState;

pub(crate) mod async_local;
pub(crate) mod auth;
pub(crate) mod branch;
pub(crate) mod builtin_prompts;
pub(crate) mod compact;
pub(crate) mod dispatch;
pub(crate) mod effort;
pub(crate) mod goal;
pub(crate) mod local_output;
pub(crate) mod persisted_system;
pub(crate) mod plan;
pub(crate) mod registry;
pub(crate) mod release_notes;
pub(crate) mod review;
pub(crate) mod tui_local;
pub(crate) mod utils;

pub(crate) struct CommandContext<'a> {
    pub(crate) state: &'a mut TuiState,
    pub(crate) app_server: &'a AppClient,
    pub(crate) line: &'a str,
    pub(crate) args: &'a str,
    pub(crate) local_command_tx: &'a mpsc::UnboundedSender<LocalCommandEnvelope>,
}

pub(crate) trait SlashCommand: Sync {
    fn execute<'a>(
        &self,
        ctx: CommandContext<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<SlashCommandOutcome>> + 'a>>;
}

pub(crate) struct CommandRegistry {
    commands: HashMap<&'static str, &'static dyn SlashCommand>,
}

impl CommandRegistry {
    pub(crate) fn new() -> Self {
        Self {
            commands: HashMap::new(),
        }
    }

    pub(crate) fn register(&mut self, names: &[&'static str], command: &'static dyn SlashCommand) {
        for &name in names {
            self.commands.insert(name, command);
        }
    }

    pub(crate) fn lookup(&self, name: &str) -> Option<&&'static dyn SlashCommand> {
        self.commands.get(name)
    }
}
