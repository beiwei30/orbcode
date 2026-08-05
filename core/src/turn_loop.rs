use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use orbcode_protocol::TranscriptMessage;
use tokio::sync::Mutex;
use tokio::time::{Duration, sleep};
use uuid::Uuid;

use crate::{
    CoreError, TurnInteractionContext, agent_loop::no_tool::NoToolTurnReason,
    hooks::model_visible_context_message,
};

#[derive(Clone)]
pub(crate) struct ActiveTurnHandle {
    pub(crate) turn_id: Uuid,
    pub(crate) cancel_flag: Arc<AtomicBool>,
    pub(crate) interaction: TurnInteractionContext,
}

#[derive(Clone, Default)]
pub(crate) struct ActiveTurnRegistry {
    active_turns: Arc<Mutex<HashMap<String, ActiveTurnHandle>>>,
}

impl ActiveTurnRegistry {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) async fn insert(
        &self,
        session_id: &str,
        turn_id: Uuid,
        cancel_flag: Arc<AtomicBool>,
        interaction: TurnInteractionContext,
    ) -> Result<(), CoreError> {
        let mut active_turns = self.active_turns.lock().await;
        if active_turns.contains_key(session_id) {
            return Err(CoreError::ActiveTurn(session_id.to_string()));
        }
        active_turns.insert(
            session_id.to_string(),
            ActiveTurnHandle {
                turn_id,
                cancel_flag,
                interaction,
            },
        );
        Ok(())
    }

    pub(crate) async fn cancel(&self, session_id: &str) -> bool {
        let active_turns = self.active_turns.lock().await;
        if let Some(active_turn) = active_turns.get(session_id) {
            active_turn.cancel_flag.store(true, Ordering::SeqCst);
            true
        } else {
            false
        }
    }

    pub(crate) async fn interrupt(&self, session_id: &str) -> bool {
        let mut active_turns = self.active_turns.lock().await;
        if let Some(active_turn) = active_turns.remove(session_id) {
            active_turn.cancel_flag.store(true, Ordering::SeqCst);
            true
        } else {
            false
        }
    }

    pub(crate) async fn cancel_owner(&self, owner_id: &str) -> Vec<String> {
        let active_turns = self.active_turns.lock().await;
        let mut sessions = Vec::new();
        for (session_id, active_turn) in active_turns.iter() {
            if active_turn.interaction.owner_id == owner_id {
                active_turn.cancel_flag.store(true, Ordering::SeqCst);
                sessions.push(session_id.clone());
            }
        }
        sessions
    }

    pub(crate) async fn is_active(&self, session_id: &str, turn_id: Uuid) -> bool {
        let active_turns = self.active_turns.lock().await;
        active_turns
            .get(session_id)
            .is_some_and(|active_turn| active_turn.turn_id == turn_id)
    }

    pub(crate) async fn has_active_session(&self, session_id: &str) -> bool {
        self.active_turns.lock().await.contains_key(session_id)
    }

    pub(crate) async fn interaction_context(
        &self,
        session_id: &str,
    ) -> Option<(Uuid, TurnInteractionContext)> {
        self.active_turns
            .lock()
            .await
            .get(session_id)
            .map(|active_turn| (active_turn.turn_id, active_turn.interaction.clone()))
    }

    pub(crate) async fn clear_if_matching(&self, session_id: &str, turn_id: Uuid) {
        let mut active_turns = self.active_turns.lock().await;
        if active_turns
            .get(session_id)
            .is_some_and(|active_turn| active_turn.turn_id == turn_id)
        {
            active_turns.remove(session_id);
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TurnLoopOutcome {
    Continue,
    StopHookContinue,
    AutoContinue(NoToolTurnReason),
    Finished,
    Cancelled,
}

#[derive(Default)]
pub(crate) struct TurnLoopState {
    pub(crate) synthetic_messages: Vec<TranscriptMessage>,
    pub(crate) auto_continue_attempts: usize,
    pub(crate) stop_hook_active: bool,
    pub(crate) auto_compacted_for_prompt: bool,
    pub(crate) lightweight_compacted_for_prompt: bool,
    pub(crate) provider_request_count: usize,
    pub(crate) context_cache: Option<crate::context::fingerprint::TurnContextCache>,
}

impl TurnLoopState {
    pub(crate) fn push_synthetic_context_message(&mut self, content: String) {
        self.synthetic_messages
            .push(model_visible_context_message(content));
    }
}

pub(crate) async fn wait_for_turn_cancellation(cancel_flag: Arc<AtomicBool>) {
    while !cancel_flag.load(Ordering::SeqCst) {
        sleep(Duration::from_millis(10)).await;
    }
}
