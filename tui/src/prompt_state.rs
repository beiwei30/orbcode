use std::time::Instant;

use crate::bottom_pane::vim::{FindKind, IndentDirection, OperatorKind, TextObjectScope};

pub(crate) struct ActiveThinkingState {
    pub(crate) text: String,
    pub(crate) is_streaming: bool,
    pub(crate) completed_at: Option<Instant>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct InputSelectionState {
    pub(crate) anchor: usize,
    pub(crate) focus: usize,
}

impl InputSelectionState {
    pub(crate) fn normalized_range(&self) -> (usize, usize) {
        if self.anchor <= self.focus {
            (self.anchor, self.focus)
        } else {
            (self.focus, self.anchor)
        }
    }

    pub(crate) fn is_collapsed(&self) -> bool {
        self.anchor == self.focus
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NormalPending {
    Find(FindKind),
    Go,
    Operator {
        op: OperatorKind,
        count: usize,
    },
    OperatorFind {
        op: OperatorKind,
        count: usize,
        kind: FindKind,
    },
    OperatorTextObject {
        op: OperatorKind,
        count: usize,
        scope: TextObjectScope,
    },
    OperatorGo {
        op: OperatorKind,
        count: usize,
    },
    Replace {
        count: usize,
    },
    Indent {
        direction: IndentDirection,
        count: usize,
    },
}
