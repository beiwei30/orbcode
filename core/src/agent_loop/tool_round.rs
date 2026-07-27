use orbcode_model_provider::{ProviderResponse, ProviderStreamAccumulator, ProviderStreamEvent};
use orbcode_protocol::{ProviderId, TranscriptBlock};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SequentialToolRoundOutcome {
    Continue,
    Denied {
        /// Tool uses after the denied one that never ran and therefore have no
        /// result yet (the denied tool already has its denial result).
        remaining_tool_uses: Vec<ToolRoundToolUse>,
    },
    Cancelled {
        remaining_tool_uses: Vec<ToolRoundToolUse>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ToolRoundToolUse {
    pub(crate) tool_use_id: String,
    pub(crate) tool_name: String,
    pub(crate) tool_input: String,
}

impl ToolRoundToolUse {
    pub(crate) fn new(
        tool_use_id: impl Into<String>,
        tool_name: impl Into<String>,
        tool_input: impl Into<String>,
    ) -> Self {
        Self {
            tool_use_id: tool_use_id.into(),
            tool_name: tool_name.into(),
            tool_input: tool_input.into(),
        }
    }

    fn from_block(block: &TranscriptBlock) -> Option<Self> {
        match block {
            TranscriptBlock::ToolUse { id, name, input } => {
                Some(Self::new(id.clone(), name.clone(), input.clone()))
            }
            _ => None,
        }
    }
}

fn collect_tool_round_tool_uses(blocks: &[TranscriptBlock]) -> Vec<ToolRoundToolUse> {
    blocks
        .iter()
        .filter_map(ToolRoundToolUse::from_block)
        .collect()
}

fn resolve_tool_round_tool_uses(
    blocks: &[TranscriptBlock],
    streamed_tool_uses: Option<Vec<ToolRoundToolUse>>,
) -> Vec<ToolRoundToolUse> {
    let block_tool_uses = collect_tool_round_tool_uses(blocks);
    match streamed_tool_uses {
        Some(streamed_tool_uses) if streamed_tool_uses == block_tool_uses => streamed_tool_uses,
        _ => block_tool_uses,
    }
}

#[derive(Debug)]
pub(crate) struct ToolRoundStreamResult {
    pub(crate) response: ProviderResponse,
    scheduler: ToolRoundScheduler,
}

impl ToolRoundStreamResult {
    pub(crate) fn into_tool_round_response(self) -> ToolRoundResponse {
        ToolRoundResponse {
            response: self.response,
            streamed_tool_uses: Some(self.scheduler.into_tool_uses()),
        }
    }
}

#[derive(Debug)]
pub(crate) struct ToolRoundResponse {
    pub(crate) response: ProviderResponse,
    streamed_tool_uses: Option<Vec<ToolRoundToolUse>>,
}

#[derive(Debug)]
pub(crate) struct ResolvedToolRoundResponse {
    pub(crate) response: ProviderResponse,
    pub(crate) scheduler: ToolRoundScheduler,
}

impl ToolRoundResponse {
    #[cfg(test)]
    pub(crate) fn from_response(response: ProviderResponse) -> Self {
        Self {
            response,
            streamed_tool_uses: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn from_response_and_streamed_tool_uses(
        response: ProviderResponse,
        streamed_tool_uses: Vec<ToolRoundToolUse>,
    ) -> Self {
        Self {
            response,
            streamed_tool_uses: Some(streamed_tool_uses),
        }
    }

    pub(crate) fn resolve_for_blocks(
        mut self,
        blocks: &[TranscriptBlock],
    ) -> ResolvedToolRoundResponse {
        let scheduler =
            ToolRoundScheduler::from_tool_uses(self.resolve_tool_uses_for_blocks(blocks));
        ResolvedToolRoundResponse {
            response: self.response,
            scheduler,
        }
    }

    fn resolve_tool_uses_for_blocks(
        &mut self,
        blocks: &[TranscriptBlock],
    ) -> Vec<ToolRoundToolUse> {
        resolve_tool_round_tool_uses(blocks, self.streamed_tool_uses.take())
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ToolRoundStreamCollector {
    accumulator: ProviderStreamAccumulator,
    scheduler: ToolRoundScheduler,
}

impl ToolRoundStreamCollector {
    pub(crate) fn new(provider: ProviderId, fallback_from: Option<ProviderId>) -> Self {
        Self {
            accumulator: ProviderStreamAccumulator::new(provider, fallback_from),
            scheduler: ToolRoundScheduler::new(),
        }
    }

    pub(crate) fn apply(&mut self, event: &ProviderStreamEvent) -> ToolRoundStreamUpdate {
        let completed = collect_completed_tool_use_from_stream_event(&mut self.accumulator, event);
        match completed {
            Some(tool_use) => {
                ToolRoundStreamUpdate::ready(self.scheduler.accept_tool_use(tool_use))
            }
            None => ToolRoundStreamUpdate::default(),
        }
    }

    pub(crate) fn into_response(self) -> ProviderResponse {
        self.accumulator.into_response()
    }

    pub(crate) fn into_result(self) -> ToolRoundStreamResult {
        ToolRoundStreamResult {
            response: self.accumulator.into_response(),
            scheduler: self.scheduler,
        }
    }

    pub(crate) fn response_snapshot(&self) -> ProviderResponse {
        self.accumulator.clone().into_response()
    }

    pub(crate) fn block(&self, index: usize) -> Option<&TranscriptBlock> {
        self.accumulator.block(index)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ToolRoundStreamUpdate {
    ready_item: Option<ToolRoundReadyItem>,
}

impl ToolRoundStreamUpdate {
    fn ready(ready_item: ToolRoundReadyItem) -> Self {
        Self {
            ready_item: Some(ready_item),
        }
    }

    pub(crate) fn into_ready_item(self) -> Option<ToolRoundReadyItem> {
        self.ready_item
    }
}

fn collect_completed_tool_use_from_stream_event(
    accumulator: &mut ProviderStreamAccumulator,
    event: &ProviderStreamEvent,
) -> Option<ToolRoundToolUse> {
    let stopped_index = match event {
        ProviderStreamEvent::ContentBlockStop { index } => Some(*index),
        _ => None,
    };
    accumulator.apply(event);
    stopped_index
        .and_then(|index| accumulator.block(index))
        .and_then(ToolRoundToolUse::from_block)
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ToolRoundItem {
    index: usize,
    tool_use_id: String,
    tool_name: String,
    tool_input: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ToolRoundReadyItem {
    item: ToolRoundItem,
}

impl ToolRoundReadyItem {
    pub(crate) fn tool_use_id(&self) -> &str {
        &self.item.tool_use_id
    }

    pub(crate) fn tool_name(&self) -> &str {
        &self.item.tool_name
    }

    pub(crate) fn tool_input(&self) -> &str {
        &self.item.tool_input
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ToolRoundExecutionOutcome {
    Continue,
    Denied,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ToolRoundSchedulingDecision {
    Waiting,
    Continue,
    Stop(SequentialToolRoundOutcome),
}

impl ToolRoundSchedulingDecision {
    fn terminal_outcome(self) -> Option<SequentialToolRoundOutcome> {
        match self {
            Self::Waiting | Self::Continue => None,
            Self::Stop(outcome) => Some(outcome),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ToolRoundScheduler {
    tool_uses: Vec<ToolRoundToolUse>,
    next_index: usize,
    next_commit_index: usize,
    completed: Vec<Option<ToolRoundExecutionOutcome>>,
}

impl ToolRoundScheduler {
    #[cfg(test)]
    fn sequential(tool_uses: &[ToolRoundToolUse]) -> Self {
        Self::from_tool_uses(tool_uses.to_vec())
    }

    pub(crate) fn from_tool_uses(tool_uses: Vec<ToolRoundToolUse>) -> Self {
        let mut scheduler = Self::new();
        for tool_use in tool_uses {
            scheduler.append_tool_use(tool_use);
        }
        scheduler
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.tool_uses.is_empty()
    }

    pub(crate) fn new() -> Self {
        Self {
            tool_uses: Vec::new(),
            next_index: 0,
            next_commit_index: 0,
            completed: Vec::new(),
        }
    }

    fn append_tool_use(&mut self, tool_use: ToolRoundToolUse) {
        self.tool_uses.push(tool_use);
        self.completed.push(None);
    }

    pub(crate) fn accept_tool_use(&mut self, tool_use: ToolRoundToolUse) -> ToolRoundReadyItem {
        self.append_tool_use(tool_use);
        self.next_ready()
            .expect("accepted tool use should produce a ready item")
    }

    fn into_tool_uses(self) -> Vec<ToolRoundToolUse> {
        self.tool_uses
    }

    pub(crate) fn next_ready(&mut self) -> Option<ToolRoundReadyItem> {
        let index = self.next_index;
        let tool_use = self.tool_uses.get(index)?;
        self.next_index += 1;
        Some(ToolRoundReadyItem {
            item: ToolRoundItem {
                index,
                tool_use_id: tool_use.tool_use_id.clone(),
                tool_name: tool_use.tool_name.clone(),
                tool_input: tool_use.tool_input.clone(),
            },
        })
    }

    pub(crate) fn record_execution_outcome(
        &mut self,
        ready_item: ToolRoundReadyItem,
        outcome: ToolRoundExecutionOutcome,
    ) -> Option<SequentialToolRoundOutcome> {
        self.record_scheduling_outcome(ready_item.item, outcome)
            .terminal_outcome()
    }

    fn record_scheduling_outcome(
        &mut self,
        item: ToolRoundItem,
        outcome: ToolRoundExecutionOutcome,
    ) -> ToolRoundSchedulingDecision {
        if let Some(slot) = self.completed.get_mut(item.index) {
            debug_assert!(slot.is_none(), "tool round item recorded more than once");
            *slot = Some(outcome);
        }
        self.advance_committed_outcomes()
    }

    fn advance_committed_outcomes(&mut self) -> ToolRoundSchedulingDecision {
        let mut advanced = false;
        while let Some(outcome) = self
            .completed
            .get(self.next_commit_index)
            .copied()
            .flatten()
        {
            match outcome {
                ToolRoundExecutionOutcome::Continue => {
                    self.completed[self.next_commit_index] = None;
                    self.next_commit_index += 1;
                    advanced = true;
                }
                ToolRoundExecutionOutcome::Denied => {
                    return ToolRoundSchedulingDecision::Stop(SequentialToolRoundOutcome::Denied {
                        // Skip the denied tool itself — it already has its denial
                        // result; only the tools after it are unanswered.
                        remaining_tool_uses: self.tool_uses[self.next_commit_index + 1..].to_vec(),
                    });
                }
                ToolRoundExecutionOutcome::Cancelled => {
                    return ToolRoundSchedulingDecision::Stop(
                        SequentialToolRoundOutcome::Cancelled {
                            remaining_tool_uses: self.tool_uses[self.next_commit_index..].to_vec(),
                        },
                    );
                }
            }
        }
        if advanced {
            ToolRoundSchedulingDecision::Continue
        } else {
            ToolRoundSchedulingDecision::Waiting
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbcode_model_provider::{ProviderContentBlockDelta, ProviderContentBlockStart};
    use orbcode_protocol::{ProviderId, TokenUsage};

    fn tool_use(
        id: impl Into<String>,
        name: impl Into<String>,
        input: impl Into<String>,
    ) -> ToolRoundToolUse {
        ToolRoundToolUse::new(id, name, input)
    }

    fn provider_response(blocks: Vec<TranscriptBlock>) -> ProviderResponse {
        ProviderResponse {
            provider: ProviderId::Anthropic,
            fallback_from: None,
            content: String::new(),
            blocks,
            stop_reason: Some("tool_use".to_string()),
            usage: TokenUsage::default(),
            deltas: Vec::new(),
        }
    }

    #[test]
    fn agent_loop_tool_round_scheduler_preserves_provider_order() {
        let tool_uses = vec![
            tool_use(
                "tool-first".to_string(),
                "bash".to_string(),
                r#"{"command":"printf first"}"#.to_string(),
            ),
            tool_use(
                "tool-second".to_string(),
                "Read".to_string(),
                r#"{"file_path":"notes.txt"}"#.to_string(),
            ),
        ];
        let mut scheduler = ToolRoundScheduler::sequential(&tool_uses);

        let first = scheduler.next_ready().expect("first item");
        let second = scheduler.next_ready().expect("second item");

        assert_eq!(first.tool_use_id(), "tool-first");
        assert_eq!(first.tool_name(), "bash");
        assert_eq!(first.tool_input(), r#"{"command":"printf first"}"#);
        assert_eq!(second.tool_use_id(), "tool-second");
        assert_eq!(second.tool_name(), "Read");
        assert_eq!(second.tool_input(), r#"{"file_path":"notes.txt"}"#);
        assert_eq!(scheduler.next_ready(), None);
    }

    #[test]
    fn agent_loop_tool_round_items_own_execution_inputs() {
        let item = {
            let tool_uses = vec![tool_use(
                "tool-owned",
                "bash",
                r#"{"command":"printf owned"}"#,
            )];
            let mut scheduler = ToolRoundScheduler::sequential(&tool_uses);
            scheduler.next_ready().expect("ready item")
        };

        assert_eq!(item.tool_use_id(), "tool-owned");
        assert_eq!(item.tool_name(), "bash");
        assert_eq!(item.tool_input(), r#"{"command":"printf owned"}"#);
    }

    #[test]
    fn agent_loop_tool_round_scheduler_owns_tool_uses() {
        let mut scheduler = {
            let tool_uses = vec![tool_use(
                "tool-scheduler-owned",
                "bash",
                r#"{"command":"printf scheduler"}"#,
            )];
            ToolRoundScheduler::sequential(&tool_uses)
        };

        let item = scheduler.next_ready().expect("ready item");
        assert_eq!(item.tool_use_id(), "tool-scheduler-owned");
        assert_eq!(item.tool_name(), "bash");
        assert_eq!(item.tool_input(), r#"{"command":"printf scheduler"}"#);
    }

    #[test]
    fn agent_loop_tool_round_scheduler_accepts_later_tool_uses() {
        let mut scheduler = ToolRoundScheduler::new();

        assert_eq!(scheduler.next_ready(), None);

        let item = scheduler.accept_tool_use(tool_use(
            "tool-later",
            "bash",
            r#"{"command":"printf later"}"#,
        ));
        assert_eq!(item.tool_use_id(), "tool-later");
        assert_eq!(item.tool_name(), "bash");
        assert_eq!(item.tool_input(), r#"{"command":"printf later"}"#);
        assert_eq!(scheduler.next_ready(), None);
        assert_eq!(
            scheduler.record_execution_outcome(item, ToolRoundExecutionOutcome::Continue),
            None
        );
    }

    #[test]
    fn agent_loop_tool_round_scheduler_accepts_later_tool_uses_in_order() {
        let mut scheduler = ToolRoundScheduler::new();

        let first = scheduler.accept_tool_use(tool_use(
            "tool-first",
            "bash",
            r#"{"command":"printf first"}"#,
        ));
        let second = scheduler.accept_tool_use(tool_use(
            "tool-second",
            "Read",
            r#"{"file_path":"notes.txt"}"#,
        ));

        assert_eq!(first.tool_use_id(), "tool-first");
        assert_eq!(second.tool_use_id(), "tool-second");
    }

    #[test]
    fn agent_loop_tool_round_collects_only_tool_use_blocks() {
        let blocks = vec![
            TranscriptBlock::Text {
                text: "before".to_string(),
            },
            TranscriptBlock::ToolUse {
                id: "tool-first".to_string(),
                name: "bash".to_string(),
                input: r#"{"command":"printf first"}"#.to_string(),
            },
            TranscriptBlock::ToolResult {
                tool_use_id: "tool-first".to_string(),
                content: "first".to_string(),
                is_error: false,
                metadata: None,
            },
            TranscriptBlock::ToolUse {
                id: "tool-second".to_string(),
                name: "Read".to_string(),
                input: r#"{"file_path":"notes.txt"}"#.to_string(),
            },
        ];

        assert_eq!(
            collect_tool_round_tool_uses(&blocks),
            vec![
                tool_use("tool-first", "bash", r#"{"command":"printf first"}"#),
                tool_use("tool-second", "Read", r#"{"file_path":"notes.txt"}"#),
            ]
        );
    }

    #[test]
    fn agent_loop_tool_round_uses_matching_streamed_tool_uses() {
        let blocks = vec![TranscriptBlock::ToolUse {
            id: "tool-stream".to_string(),
            name: "bash".to_string(),
            input: r#"{"command":"printf hi"}"#.to_string(),
        }];
        let streamed_tool_uses = vec![tool_use(
            "tool-stream",
            "bash",
            r#"{"command":"printf hi"}"#,
        )];
        let response = ToolRoundResponse {
            response: provider_response(blocks.clone()),
            streamed_tool_uses: Some(streamed_tool_uses.clone()),
        };

        assert_eq!(
            response
                .resolve_for_blocks(&blocks)
                .scheduler
                .into_tool_uses(),
            streamed_tool_uses
        );
    }

    #[test]
    fn agent_loop_tool_round_falls_back_to_blocks_when_streamed_tool_uses_differ() {
        let blocks = vec![TranscriptBlock::ToolUse {
            id: "tool-final".to_string(),
            name: "bash".to_string(),
            input: r#"{"command":"printf final"}"#.to_string(),
        }];
        let streamed_tool_uses = vec![tool_use(
            "tool-stream",
            "bash",
            r#"{"command":"printf stream"}"#,
        )];
        let response = ToolRoundResponse {
            response: provider_response(blocks.clone()),
            streamed_tool_uses: Some(streamed_tool_uses),
        };

        assert_eq!(
            response
                .resolve_for_blocks(&blocks)
                .scheduler
                .into_tool_uses(),
            vec![tool_use(
                "tool-final",
                "bash",
                r#"{"command":"printf final"}"#
            )]
        );
    }

    #[test]
    fn agent_loop_tool_round_collects_completed_stream_tool_use() {
        let mut collector = ToolRoundStreamCollector::new(ProviderId::Anthropic, None);

        assert_eq!(
            collector
                .apply(&ProviderStreamEvent::MessageStart {
                    provider: ProviderId::Anthropic,
                    fallback_from: None,
                    usage: TokenUsage::default(),
                })
                .into_ready_item(),
            None
        );
        assert_eq!(
            collector
                .apply(&ProviderStreamEvent::ContentBlockStart {
                    index: 0,
                    block: ProviderContentBlockStart::ToolUse {
                        id: "tool-stream".to_string(),
                        name: "bash".to_string(),
                        input: "{}".to_string(),
                    },
                })
                .into_ready_item(),
            None
        );
        assert_eq!(
            collector
                .apply(&ProviderStreamEvent::ContentBlockDelta {
                    index: 0,
                    delta: ProviderContentBlockDelta::InputJson(
                        r#"{"command":"printf "#.to_string()
                    ),
                })
                .into_ready_item(),
            None
        );
        assert_eq!(
            collector
                .apply(&ProviderStreamEvent::ContentBlockDelta {
                    index: 0,
                    delta: ProviderContentBlockDelta::InputJson(r#"hi"}"#.to_string()),
                })
                .into_ready_item(),
            None
        );

        let ready_item = collector
            .apply(&ProviderStreamEvent::ContentBlockStop { index: 0 })
            .into_ready_item()
            .expect("completed stream tool use");
        assert_eq!(ready_item.tool_use_id(), "tool-stream");
        assert_eq!(ready_item.tool_name(), "bash");
        assert_eq!(ready_item.tool_input(), r#"{"command":"printf hi"}"#);
        let result = collector.into_result();
        let tool_round_response = result.into_tool_round_response();
        assert_eq!(
            tool_round_response.streamed_tool_uses,
            Some(vec![tool_use(
                "tool-stream",
                "bash",
                r#"{"command":"printf hi"}"#
            )])
        );
        assert_eq!(
            collect_tool_round_tool_uses(&tool_round_response.response.blocks),
            vec![tool_use(
                "tool-stream",
                "bash",
                r#"{"command":"printf hi"}"#
            )]
        );
    }

    #[test]
    fn agent_loop_tool_round_scheduler_maps_terminal_outcomes() {
        let tool_uses = vec![
            tool_use(
                "tool-first".to_string(),
                "bash".to_string(),
                r#"{"command":"printf first"}"#.to_string(),
            ),
            tool_use(
                "tool-second".to_string(),
                "bash".to_string(),
                r#"{"command":"printf second"}"#.to_string(),
            ),
        ];
        let mut scheduler = ToolRoundScheduler::sequential(&tool_uses);

        let first = scheduler.next_ready().expect("first item");
        assert_eq!(
            scheduler.record_execution_outcome(first, ToolRoundExecutionOutcome::Continue),
            None
        );

        let second = scheduler.next_ready().expect("second item");
        assert_eq!(
            scheduler.record_execution_outcome(second, ToolRoundExecutionOutcome::Cancelled),
            Some(SequentialToolRoundOutcome::Cancelled {
                remaining_tool_uses: vec![tool_use(
                    "tool-second",
                    "bash",
                    r#"{"command":"printf second"}"#
                )]
            })
        );
    }

    #[test]
    fn agent_loop_tool_round_scheduler_waits_for_prior_outcomes() {
        let tool_uses = vec![
            tool_use(
                "tool-first".to_string(),
                "bash".to_string(),
                r#"{"command":"printf first"}"#.to_string(),
            ),
            tool_use(
                "tool-second".to_string(),
                "bash".to_string(),
                r#"{"command":"printf second"}"#.to_string(),
            ),
        ];
        let mut scheduler = ToolRoundScheduler::sequential(&tool_uses);

        let first = scheduler.next_ready().expect("first item");
        let second = scheduler.next_ready().expect("second item");

        assert_eq!(
            scheduler.record_execution_outcome(second, ToolRoundExecutionOutcome::Cancelled),
            None
        );
        assert_eq!(
            scheduler.record_execution_outcome(first, ToolRoundExecutionOutcome::Continue),
            Some(SequentialToolRoundOutcome::Cancelled {
                remaining_tool_uses: vec![tool_use(
                    "tool-second",
                    "bash",
                    r#"{"command":"printf second"}"#
                )]
            })
        );
    }
}
