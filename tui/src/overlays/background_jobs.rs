use std::collections::{HashMap, HashSet};

use crate::render::styled_wrap::{ensure_source_range_visible, project_styled_lines};
use orbcode_protocol::{
    BackgroundTaskView, BackgroundTaskViewStatus, SessionRecord, TranscriptMessage,
    WorkflowStepView, WorkflowStepViewStatus,
};

use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum BackgroundJobsOverlayAction {
    None,
    Close,
    CancelJob { job_index: usize },
    RequestRefresh,
    OpenChildSession { session_id: String },
    CopyWorkflowStepOutput { output: String },
    SetStatus { message: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BackgroundJobsView {
    List,
    Detail,
    ChildSession,
}

#[derive(Clone, Debug)]
pub(crate) struct BackgroundJobsChildSessionView {
    session_id: String,
    title: Option<String>,
    messages: Vec<TranscriptMessage>,
}

#[derive(Clone, Debug)]
pub(crate) struct BackgroundJobsOverlayState {
    pub(crate) jobs: Vec<BackgroundTaskView>,
    pub(crate) current_session_id: String,
    pub(crate) selected: usize,
    pub(crate) view: BackgroundJobsView,
    pub(crate) detail: Option<BackgroundTaskView>,
    pub(crate) detail_step_selected: usize,
    pub(crate) collapsed_workflow_steps: HashSet<String>,
    pub(crate) child_session: Option<BackgroundJobsChildSessionView>,
    pub(crate) scroll: usize,
    pub(crate) max_scroll: usize,
    detail_scroll: usize,
    ensure_selection_visible: bool,
    selected_source_range: Option<(usize, usize)>,
    source_line_by_visual_row: Vec<usize>,
    pub(crate) lines_cache: BackgroundJobsLinesCache,
    pub(crate) needs_refresh: bool,
}

type BackgroundJobsLinesCache = LinesCache<BackgroundJobsCacheKey>;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct BackgroundJobsCacheKey {
    width: usize,
    view: BackgroundJobsView,
    selected: usize,
    job_count: usize,
    detail_job_id: Option<String>,
    detail_step_selected: usize,
    collapsed_workflow_steps: Vec<String>,
}

impl BackgroundJobsOverlayState {
    pub(crate) fn new(jobs: Vec<BackgroundTaskView>, current_session_id: String) -> Self {
        Self {
            jobs,
            current_session_id,
            selected: 0,
            view: BackgroundJobsView::List,
            detail: None,
            detail_step_selected: 0,
            collapsed_workflow_steps: HashSet::new(),
            child_session: None,
            scroll: 0,
            max_scroll: 0,
            detail_scroll: 0,
            ensure_selection_visible: true,
            selected_source_range: None,
            source_line_by_visual_row: Vec::new(),
            lines_cache: BackgroundJobsLinesCache::default(),
            needs_refresh: false,
        }
    }

    pub(crate) fn update_jobs(&mut self, jobs: Vec<BackgroundTaskView>) {
        self.jobs = jobs;
        if self.selected >= self.jobs.len() && !self.jobs.is_empty() {
            self.selected = self.jobs.len() - 1;
        }
        self.lines_cache.invalidate();
    }

    pub(crate) fn set_detail(&mut self, detail: BackgroundTaskView) {
        let same_detail = self
            .detail
            .as_ref()
            .is_some_and(|current| current.task_id == detail.task_id);
        let step_count = detail.workflow_steps.as_ref().map_or(0, Vec::len);
        self.detail = Some(detail);
        self.view = BackgroundJobsView::Detail;
        if same_detail {
            self.detail_step_selected = self.detail_step_selected.min(step_count.saturating_sub(1));
            self.prune_collapsed_workflow_steps();
            self.ensure_selected_workflow_step_visible();
        } else {
            self.detail_step_selected = 0;
            self.collapsed_workflow_steps.clear();
            self.scroll = 0;
        }
        self.lines_cache.invalidate();
    }

    pub(crate) fn set_child_session(&mut self, session: SessionRecord) {
        let title = session.display_title().map(str::to_string);
        self.child_session = Some(BackgroundJobsChildSessionView {
            session_id: session.session_id,
            title,
            messages: session.messages,
        });
        self.view = BackgroundJobsView::ChildSession;
        self.detail_scroll = self.scroll;
        self.scroll = 0;
        self.lines_cache.invalidate();
    }

    fn cache_key(&self, width: usize) -> BackgroundJobsCacheKey {
        let mut collapsed_workflow_steps = self
            .collapsed_workflow_steps
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        collapsed_workflow_steps.sort();
        BackgroundJobsCacheKey {
            width,
            view: self.view,
            selected: self.selected,
            job_count: self.jobs.len(),
            detail_job_id: self.detail.as_ref().map(|d| d.task_id.clone()),
            detail_step_selected: self.detail_step_selected,
            collapsed_workflow_steps,
        }
    }

    pub(crate) fn cached_lines(&mut self, width: usize) -> &[StyledLine] {
        let width = width.max(1);
        let key = self.cache_key(width);
        let jobs = &self.jobs;
        let selected = self.selected;
        let view = self.view;
        let detail = &self.detail;
        let detail_step_selected = self.detail_step_selected;
        let collapsed_workflow_steps = &self.collapsed_workflow_steps;
        let child_session = &self.child_session;
        let current_session_id = &self.current_session_id;
        let mut projection_metadata = None;
        let rebuilt = self.lines_cache.refresh(key, || {
            let logical_lines = match view {
                BackgroundJobsView::List => {
                    background_jobs_list_lines(jobs, selected, current_session_id, width)
                }
                BackgroundJobsView::Detail => background_job_detail_lines(
                    detail.as_ref(),
                    width,
                    detail_step_selected,
                    collapsed_workflow_steps,
                ),
                BackgroundJobsView::ChildSession => {
                    background_jobs_child_session_lines(child_session.as_ref())
                }
            };
            let selected_source_range = selected_background_source_range(view, &logical_lines);
            let projection = project_styled_lines(&logical_lines, width);
            projection_metadata =
                Some((selected_source_range, projection.source_line_by_visual_row));
            projection.visual_rows
        });
        if rebuilt
            && let Some((selected_source_range, source_line_by_visual_row)) = projection_metadata
        {
            self.selected_source_range = selected_source_range;
            self.source_line_by_visual_row = source_line_by_visual_row;
            self.ensure_selection_visible = true;
        }
        &self.lines_cache.lines
    }

    pub(crate) fn cached_visible_lines(
        &mut self,
        width: usize,
        content_height: usize,
    ) -> Vec<StyledLine> {
        if content_height == 0 {
            return Vec::new();
        }
        let scroll = self.scroll.min(self.max_scroll);
        self.cached_lines(width)
            .iter()
            .skip(scroll)
            .take(content_height)
            .cloned()
            .collect()
    }

    pub(crate) fn selected_job(&self) -> Option<&BackgroundTaskView> {
        self.jobs.get(self.selected)
    }

    fn workflow_steps(&self) -> Option<&[WorkflowStepView]> {
        self.detail
            .as_ref()
            .and_then(|detail| detail.workflow_steps.as_deref())
    }

    fn prune_collapsed_workflow_steps(&mut self) {
        let Some(steps) = self.workflow_steps() else {
            self.collapsed_workflow_steps.clear();
            return;
        };
        let valid = steps
            .iter()
            .map(|step| step.step_key.clone())
            .collect::<HashSet<_>>();
        self.collapsed_workflow_steps
            .retain(|step_key| valid.contains(step_key));
    }

    fn ensure_selected_workflow_step_visible(&mut self) {
        let Some(next_selected) = (|| {
            let steps = self.workflow_steps()?;
            if steps.is_empty() {
                return Some(0);
            }
            let selected = self.detail_step_selected.min(steps.len() - 1);
            if workflow_step_is_visible(steps, selected, &self.collapsed_workflow_steps) {
                return Some(selected);
            }
            Some(
                nearest_visible_workflow_step_index(
                    steps,
                    selected,
                    &self.collapsed_workflow_steps,
                )
                .unwrap_or(0),
            )
        })() else {
            return;
        };
        self.detail_step_selected = next_selected;
    }
}

fn selected_background_source_range(
    view: BackgroundJobsView,
    lines: &[StyledLine],
) -> Option<(usize, usize)> {
    let selected = lines.iter().position(|line| {
        line.spans
            .iter()
            .any(|span| span.content.as_ref().contains('▸'))
    })?;
    let end = if view == BackgroundJobsView::List {
        selected
            .saturating_add(1)
            .min(lines.len().saturating_sub(1))
    } else {
        selected
    };
    Some((selected, end))
}

pub(crate) fn apply_background_jobs_key(
    state: &mut BackgroundJobsOverlayState,
    key_event: &KeyEvent,
) -> BackgroundJobsOverlayAction {
    match state.view {
        BackgroundJobsView::List => apply_list_key(state, key_event),
        BackgroundJobsView::Detail => apply_detail_key(state, key_event),
        BackgroundJobsView::ChildSession => apply_child_session_key(state, key_event),
    }
}

fn apply_list_key(
    state: &mut BackgroundJobsOverlayState,
    key_event: &KeyEvent,
) -> BackgroundJobsOverlayAction {
    match key_event.code {
        KeyCode::Esc | KeyCode::Char('q') => BackgroundJobsOverlayAction::Close,
        KeyCode::Char('c') if key_event.modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(job) = state.selected_job()
                && job.status.is_active()
            {
                return BackgroundJobsOverlayAction::CancelJob {
                    job_index: state.selected,
                };
            }
            BackgroundJobsOverlayAction::Close
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if !state.jobs.is_empty() {
                state.selected = state.selected.saturating_sub(1);
                state.lines_cache.invalidate();
            }
            BackgroundJobsOverlayAction::None
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if !state.jobs.is_empty() {
                state.selected = (state.selected + 1).min(state.jobs.len() - 1);
                state.lines_cache.invalidate();
            }
            BackgroundJobsOverlayAction::None
        }
        KeyCode::Home | KeyCode::Char('g') => {
            state.selected = 0;
            state.lines_cache.invalidate();
            BackgroundJobsOverlayAction::None
        }
        KeyCode::End | KeyCode::Char('G') => {
            if !state.jobs.is_empty() {
                state.selected = state.jobs.len() - 1;
                state.lines_cache.invalidate();
            }
            BackgroundJobsOverlayAction::None
        }
        KeyCode::Enter => {
            if state.selected_job().is_some() {
                state.view = BackgroundJobsView::Detail;
                state.detail = None;
                state.detail_step_selected = 0;
                state.scroll = 0;
                state.lines_cache.invalidate();
                BackgroundJobsOverlayAction::RequestRefresh
            } else {
                BackgroundJobsOverlayAction::None
            }
        }
        KeyCode::Char('d') => {
            if let Some(job) = state.selected_job()
                && job.status.is_active()
            {
                return BackgroundJobsOverlayAction::CancelJob {
                    job_index: state.selected,
                };
            }
            BackgroundJobsOverlayAction::None
        }
        KeyCode::Char('r') => {
            state.needs_refresh = true;
            BackgroundJobsOverlayAction::RequestRefresh
        }
        _ => BackgroundJobsOverlayAction::None,
    }
}

fn apply_detail_key(
    state: &mut BackgroundJobsOverlayState,
    key_event: &KeyEvent,
) -> BackgroundJobsOverlayAction {
    match key_event.code {
        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Backspace => {
            state.view = BackgroundJobsView::List;
            state.detail = None;
            state.detail_step_selected = 0;
            state.child_session = None;
            state.scroll = 0;
            state.lines_cache.invalidate();
            BackgroundJobsOverlayAction::None
        }
        KeyCode::Char('c') if key_event.modifiers.contains(KeyModifiers::CONTROL) => {
            state.view = BackgroundJobsView::List;
            state.detail = None;
            state.detail_step_selected = 0;
            state.child_session = None;
            state.scroll = 0;
            state.lines_cache.invalidate();
            BackgroundJobsOverlayAction::None
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if has_workflow_steps(state) {
                select_workflow_step(state, -1);
            } else {
                scroll_by(state, -1);
            }
            BackgroundJobsOverlayAction::None
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if has_workflow_steps(state) {
                select_workflow_step(state, 1);
            } else {
                scroll_by(state, 1);
            }
            BackgroundJobsOverlayAction::None
        }
        KeyCode::PageUp | KeyCode::Char('b') => {
            scroll_by(state, -(HELP_OVERLAY_PAGE_STEP as isize));
            BackgroundJobsOverlayAction::None
        }
        KeyCode::PageDown | KeyCode::Char('f') => {
            scroll_by(state, HELP_OVERLAY_PAGE_STEP as isize);
            BackgroundJobsOverlayAction::None
        }
        KeyCode::Char(' ') => {
            toggle_selected_workflow_step_group(state);
            BackgroundJobsOverlayAction::None
        }
        KeyCode::Left | KeyCode::Char('h') => {
            if !set_selected_workflow_step_collapsed(state, true) {
                select_parent_workflow_step(state);
            }
            BackgroundJobsOverlayAction::None
        }
        KeyCode::Right | KeyCode::Char('l') => {
            if !set_selected_workflow_step_collapsed(state, false) {
                select_first_child_workflow_step(state);
            }
            BackgroundJobsOverlayAction::None
        }
        KeyCode::Enter => selected_workflow_step_child_session(state)
            .map(|session_id| BackgroundJobsOverlayAction::OpenChildSession { session_id })
            .unwrap_or_else(|| {
                toggle_selected_workflow_step_group(state);
                BackgroundJobsOverlayAction::None
            }),
        KeyCode::Char('y') => selected_workflow_step_output(state)
            .map(|output| BackgroundJobsOverlayAction::CopyWorkflowStepOutput { output })
            .unwrap_or_else(|| BackgroundJobsOverlayAction::SetStatus {
                message: "Selected workflow step has no output to copy.".to_string(),
            }),
        KeyCode::Char('d') => {
            if let Some(detail) = &state.detail
                && detail.status.is_active()
            {
                return BackgroundJobsOverlayAction::CancelJob {
                    job_index: state.selected,
                };
            }
            BackgroundJobsOverlayAction::None
        }
        KeyCode::Home | KeyCode::Char('g') => {
            state.scroll = 0;
            state.ensure_selection_visible = false;
            BackgroundJobsOverlayAction::None
        }
        KeyCode::End | KeyCode::Char('G') => {
            state.scroll = state.max_scroll;
            state.ensure_selection_visible = false;
            BackgroundJobsOverlayAction::None
        }
        _ => BackgroundJobsOverlayAction::None,
    }
}

fn apply_child_session_key(
    state: &mut BackgroundJobsOverlayState,
    key_event: &KeyEvent,
) -> BackgroundJobsOverlayAction {
    match key_event.code {
        KeyCode::Esc | KeyCode::Backspace => {
            state.view = BackgroundJobsView::Detail;
            state.child_session = None;
            state.scroll = state.detail_scroll;
            state.lines_cache.invalidate();
            BackgroundJobsOverlayAction::None
        }
        KeyCode::Char('q') => BackgroundJobsOverlayAction::Close,
        KeyCode::Char('c') if key_event.modifiers.contains(KeyModifiers::CONTROL) => {
            state.view = BackgroundJobsView::Detail;
            state.child_session = None;
            state.scroll = state.detail_scroll;
            state.lines_cache.invalidate();
            BackgroundJobsOverlayAction::None
        }
        KeyCode::Up | KeyCode::Char('k') => {
            scroll_by(state, -1);
            BackgroundJobsOverlayAction::None
        }
        KeyCode::Down | KeyCode::Char('j') => {
            scroll_by(state, 1);
            BackgroundJobsOverlayAction::None
        }
        KeyCode::PageUp | KeyCode::Char('b') => {
            scroll_by(state, -(HELP_OVERLAY_PAGE_STEP as isize));
            BackgroundJobsOverlayAction::None
        }
        KeyCode::PageDown | KeyCode::Char('f' | ' ') => {
            scroll_by(state, HELP_OVERLAY_PAGE_STEP as isize);
            BackgroundJobsOverlayAction::None
        }
        KeyCode::Char('y') => selected_workflow_step_output(state)
            .map(|output| BackgroundJobsOverlayAction::CopyWorkflowStepOutput { output })
            .unwrap_or_else(|| BackgroundJobsOverlayAction::SetStatus {
                message: "Selected workflow step has no output to copy.".to_string(),
            }),
        KeyCode::Home | KeyCode::Char('g') => {
            state.scroll = 0;
            state.ensure_selection_visible = false;
            BackgroundJobsOverlayAction::None
        }
        KeyCode::End | KeyCode::Char('G') => {
            state.scroll = state.max_scroll;
            state.ensure_selection_visible = false;
            BackgroundJobsOverlayAction::None
        }
        _ => BackgroundJobsOverlayAction::None,
    }
}

fn has_workflow_steps(state: &BackgroundJobsOverlayState) -> bool {
    state
        .detail
        .as_ref()
        .and_then(|detail| detail.workflow_steps.as_ref())
        .is_some_and(|steps| !steps.is_empty())
}

fn workflow_step_index_by_key(steps: &[WorkflowStepView]) -> HashMap<&str, usize> {
    steps
        .iter()
        .enumerate()
        .map(|(index, step)| (step.step_key.as_str(), index))
        .collect()
}

fn workflow_step_has_collapsed_ancestor(
    steps: &[WorkflowStepView],
    index: usize,
    by_key: &HashMap<&str, usize>,
    collapsed_workflow_steps: &HashSet<String>,
) -> bool {
    let mut parent_key = steps.get(index).and_then(|step| step.parent_key.as_deref());
    while let Some(key) = parent_key {
        if collapsed_workflow_steps.contains(key) {
            return true;
        }
        parent_key = by_key
            .get(key)
            .and_then(|parent_index| steps.get(*parent_index))
            .and_then(|step| step.parent_key.as_deref());
    }
    false
}

fn workflow_step_is_visible(
    steps: &[WorkflowStepView],
    index: usize,
    collapsed_workflow_steps: &HashSet<String>,
) -> bool {
    if index >= steps.len() {
        return false;
    }
    let by_key = workflow_step_index_by_key(steps);
    !workflow_step_has_collapsed_ancestor(steps, index, &by_key, collapsed_workflow_steps)
}

fn visible_workflow_step_indices(
    steps: &[WorkflowStepView],
    collapsed_workflow_steps: &HashSet<String>,
) -> Vec<usize> {
    let by_key = workflow_step_index_by_key(steps);
    steps
        .iter()
        .enumerate()
        .filter_map(|(index, _)| {
            (!workflow_step_has_collapsed_ancestor(steps, index, &by_key, collapsed_workflow_steps))
                .then_some(index)
        })
        .collect()
}

fn nearest_visible_workflow_step_index(
    steps: &[WorkflowStepView],
    preferred_index: usize,
    collapsed_workflow_steps: &HashSet<String>,
) -> Option<usize> {
    let visible = visible_workflow_step_indices(steps, collapsed_workflow_steps);
    visible
        .iter()
        .rev()
        .copied()
        .find(|index| *index <= preferred_index)
        .or_else(|| visible.first().copied())
}

fn selected_visible_workflow_step_index(
    steps: &[WorkflowStepView],
    selected_index: usize,
    collapsed_workflow_steps: &HashSet<String>,
) -> Option<usize> {
    if workflow_step_is_visible(steps, selected_index, collapsed_workflow_steps) {
        return Some(selected_index);
    }
    nearest_visible_workflow_step_index(steps, selected_index, collapsed_workflow_steps)
}

fn workflow_step_has_children(steps: &[WorkflowStepView], index: usize) -> bool {
    let Some(step) = steps.get(index) else {
        return false;
    };
    steps
        .iter()
        .any(|candidate| candidate.parent_key.as_deref() == Some(step.step_key.as_str()))
}

fn selected_workflow_step_child_session(state: &BackgroundJobsOverlayState) -> Option<String> {
    let steps = state
        .detail
        .as_ref()
        .and_then(|detail| detail.workflow_steps.as_ref())?;
    let selected = selected_visible_workflow_step_index(
        steps,
        state.detail_step_selected,
        &state.collapsed_workflow_steps,
    )?;
    let step = steps.get(selected)?;
    step.child_session_id
        .as_ref()
        .filter(|session_id| !session_id.trim().is_empty())
        .cloned()
}

fn selected_workflow_step_output(state: &BackgroundJobsOverlayState) -> Option<String> {
    let steps = state
        .detail
        .as_ref()
        .and_then(|detail| detail.workflow_steps.as_ref())?;
    let selected = selected_visible_workflow_step_index(
        steps,
        state.detail_step_selected,
        &state.collapsed_workflow_steps,
    )?;
    steps
        .get(selected)?
        .output
        .as_ref()
        .filter(|output| !output.trim().is_empty())
        .cloned()
}

fn select_workflow_step(state: &mut BackgroundJobsOverlayState, delta: isize) {
    let Some(next_selected) = (|| {
        let steps = state.workflow_steps()?;
        let visible = visible_workflow_step_indices(steps, &state.collapsed_workflow_steps);
        if visible.is_empty() {
            return None;
        }
        let current = visible
            .iter()
            .position(|index| *index == state.detail_step_selected)
            .or_else(|| {
                nearest_visible_workflow_step_index(
                    steps,
                    state.detail_step_selected,
                    &state.collapsed_workflow_steps,
                )
                .and_then(|nearest| visible.iter().position(|index| *index == nearest))
            })
            .unwrap_or(0);
        let next = if delta < 0 {
            current.saturating_sub(delta.unsigned_abs())
        } else {
            current.saturating_add(delta as usize)
        };
        Some(visible[next.min(visible.len() - 1)])
    })() else {
        return;
    };
    state.detail_step_selected = next_selected;
    state.lines_cache.invalidate();
}

fn toggle_selected_workflow_step_group(state: &mut BackgroundJobsOverlayState) -> bool {
    let Some((step_key, is_collapsed)) = (|| {
        let steps = state.workflow_steps()?;
        let selected = selected_visible_workflow_step_index(
            steps,
            state.detail_step_selected,
            &state.collapsed_workflow_steps,
        )?;
        if !workflow_step_has_children(steps, selected) {
            return None;
        }
        let step_key = steps[selected].step_key.clone();
        Some((
            step_key.clone(),
            state.collapsed_workflow_steps.contains(&step_key),
        ))
    })() else {
        return false;
    };
    if is_collapsed {
        state.collapsed_workflow_steps.remove(&step_key);
    } else {
        state.collapsed_workflow_steps.insert(step_key);
        state.ensure_selected_workflow_step_visible();
    }
    state.lines_cache.invalidate();
    true
}

fn set_selected_workflow_step_collapsed(
    state: &mut BackgroundJobsOverlayState,
    collapsed: bool,
) -> bool {
    let Some(step_key) = (|| {
        let steps = state.workflow_steps()?;
        let selected = selected_visible_workflow_step_index(
            steps,
            state.detail_step_selected,
            &state.collapsed_workflow_steps,
        )?;
        workflow_step_has_children(steps, selected).then(|| steps[selected].step_key.clone())
    })() else {
        return false;
    };
    let changed = if collapsed {
        state.collapsed_workflow_steps.insert(step_key)
    } else {
        state.collapsed_workflow_steps.remove(&step_key)
    };
    if collapsed {
        state.ensure_selected_workflow_step_visible();
    }
    if changed {
        state.lines_cache.invalidate();
    }
    changed
}

fn select_parent_workflow_step(state: &mut BackgroundJobsOverlayState) -> bool {
    let Some(parent_index) = (|| {
        let steps = state.workflow_steps()?;
        let selected = selected_visible_workflow_step_index(
            steps,
            state.detail_step_selected,
            &state.collapsed_workflow_steps,
        )?;
        let parent_key = steps[selected].parent_key.as_deref()?;
        workflow_step_index_by_key(steps).get(parent_key).copied()
    })() else {
        return false;
    };
    state.detail_step_selected = parent_index;
    state.lines_cache.invalidate();
    true
}

fn select_first_child_workflow_step(state: &mut BackgroundJobsOverlayState) -> bool {
    let Some(child_index) = (|| {
        let steps = state.workflow_steps()?;
        let selected = selected_visible_workflow_step_index(
            steps,
            state.detail_step_selected,
            &state.collapsed_workflow_steps,
        )?;
        let selected_key = steps[selected].step_key.as_str();
        steps
            .iter()
            .position(|step| step.parent_key.as_deref() == Some(selected_key))
    })() else {
        return false;
    };
    state.detail_step_selected = child_index;
    state.lines_cache.invalidate();
    true
}

fn scroll_by(state: &mut BackgroundJobsOverlayState, delta: isize) {
    let next = if delta < 0 {
        state.scroll.saturating_sub(delta.unsigned_abs())
    } else {
        state.scroll.saturating_add(delta as usize)
    };
    state.scroll = next.min(state.max_scroll);
    state.ensure_selection_visible = false;
}

pub(crate) fn sync_background_jobs_overlay_bounds(
    state: &mut BackgroundJobsOverlayState,
    area: Rect,
) {
    let content_height = background_jobs_content_height(area);
    let line_count = state.cached_lines(area.width.max(1) as usize).len();
    state.max_scroll = line_count.saturating_sub(content_height);

    if state.ensure_selection_visible
        && let Some((source_start, source_end)) = state.selected_source_range
    {
        state.scroll = ensure_source_range_visible(
            &state.source_line_by_visual_row,
            line_count,
            state.scroll,
            content_height,
            source_start,
            source_end,
        );
        state.ensure_selection_visible = false;
    }

    state.scroll = state.scroll.min(state.max_scroll);
}

pub(crate) fn background_jobs_content_height(area: Rect) -> usize {
    area.height.saturating_sub(1) as usize
}

pub(crate) fn draw_background_jobs_overlay(
    frame: &mut Frame,
    state: &mut BackgroundJobsOverlayState,
    area: Rect,
) {
    frame.render_widget(Clear, area);
    if area.width == 0 || area.height == 0 {
        return;
    }

    let content_height = background_jobs_content_height(area);
    let visible_lines = state.cached_visible_lines(area.width.max(1) as usize, content_height);
    let content_area = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: area.height.saturating_sub(1),
    };
    frame.render_widget(Paragraph::new(visible_lines), content_area);

    let footer_area = Rect {
        x: area.x,
        y: area.bottom().saturating_sub(1),
        width: area.width,
        height: 1,
    };
    let footer_spans = match state.view {
        BackgroundJobsView::List => vec![
            Span::styled("jk", inactive_style().add_modifier(Modifier::BOLD)),
            Span::styled(" select · ", subtle_style()),
            Span::styled("enter", inactive_style().add_modifier(Modifier::BOLD)),
            Span::styled(" detail · ", subtle_style()),
            Span::styled("d", inactive_style().add_modifier(Modifier::BOLD)),
            Span::styled(" cancel · ", subtle_style()),
            Span::styled("r", inactive_style().add_modifier(Modifier::BOLD)),
            Span::styled(" refresh · ", subtle_style()),
            Span::styled("q", inactive_style().add_modifier(Modifier::BOLD)),
            Span::styled(" close", subtle_style()),
        ],
        BackgroundJobsView::Detail => vec![
            Span::styled("jk", inactive_style().add_modifier(Modifier::BOLD)),
            Span::styled(" step · ", subtle_style()),
            Span::styled("h/l", inactive_style().add_modifier(Modifier::BOLD)),
            Span::styled(" fold · ", subtle_style()),
            Span::styled("space", inactive_style().add_modifier(Modifier::BOLD)),
            Span::styled(" toggle · ", subtle_style()),
            Span::styled("enter", inactive_style().add_modifier(Modifier::BOLD)),
            Span::styled(" child · ", subtle_style()),
            Span::styled("pg", inactive_style().add_modifier(Modifier::BOLD)),
            Span::styled(" scroll · ", subtle_style()),
            Span::styled("d", inactive_style().add_modifier(Modifier::BOLD)),
            Span::styled(" cancel · ", subtle_style()),
            Span::styled("esc", inactive_style().add_modifier(Modifier::BOLD)),
            Span::styled(" back · ", subtle_style()),
            Span::styled("q", inactive_style().add_modifier(Modifier::BOLD)),
            Span::styled(" close", subtle_style()),
        ],
        BackgroundJobsView::ChildSession => vec![
            Span::styled("jk", inactive_style().add_modifier(Modifier::BOLD)),
            Span::styled(" scroll · ", subtle_style()),
            Span::styled("pg", inactive_style().add_modifier(Modifier::BOLD)),
            Span::styled(" scroll · ", subtle_style()),
            Span::styled("y", inactive_style().add_modifier(Modifier::BOLD)),
            Span::styled(" copy · ", subtle_style()),
            Span::styled("esc", inactive_style().add_modifier(Modifier::BOLD)),
            Span::styled(" back · ", subtle_style()),
            Span::styled("q", inactive_style().add_modifier(Modifier::BOLD)),
            Span::styled(" close", subtle_style()),
        ],
    };
    frame.render_widget(Paragraph::new(Line::from(footer_spans)), footer_area);
}

fn status_badge(status: BackgroundTaskViewStatus) -> Span<'static> {
    let (text, style) = match status {
        BackgroundTaskViewStatus::Running => ("● running", accent_style()),
        BackgroundTaskViewStatus::Queued => ("◌ queued", subtle_style()),
        BackgroundTaskViewStatus::PermissionPending => ("◌ pending", subtle_style()),
        BackgroundTaskViewStatus::Interrupting => ("● stopping", accent_style()),
        BackgroundTaskViewStatus::Completed => ("✓ completed", emphasis_style()),
        BackgroundTaskViewStatus::Failed => ("✗ failed", warning_style()),
        BackgroundTaskViewStatus::Cancelled => ("⊘ cancelled", inactive_style()),
        BackgroundTaskViewStatus::Orphaned => ("? orphaned", warning_style()),
        _ => ("? unknown", subtle_style()),
    };
    Span::styled(text, style)
}

fn format_elapsed(ms: i64) -> String {
    if ms < 1000 {
        format!("{ms}ms")
    } else if ms < 60_000 {
        format!("{:.0}s", ms as f64 / 1000.0)
    } else if ms < 3_600_000 {
        let minutes = ms / 60_000;
        let seconds = (ms % 60_000) / 1000;
        format!("{minutes}m{seconds:02}s")
    } else {
        let hours = ms / 3_600_000;
        let minutes = (ms % 3_600_000) / 60_000;
        format!("{hours}h{minutes:02}m")
    }
}

fn short_job_id(id: &str) -> &str {
    if id.len() > 8 { &id[..8] } else { id }
}

pub(crate) fn background_jobs_list_lines(
    jobs: &[BackgroundTaskView],
    selected: usize,
    current_session_id: &str,
    width: usize,
) -> Vec<StyledLine> {
    let prompt_max = width.saturating_sub(6).max(20);
    let mut lines: Vec<StyledLine> = Vec::new();

    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            "BACKGROUND JOBS",
            inactive_style().add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        ),
    ]));
    lines.push(Line::from(vec![]));

    if jobs.is_empty() {
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled("No background jobs.", subtle_style()),
        ]));
        return lines;
    }

    for (i, job) in jobs.iter().enumerate() {
        let is_selected = i == selected;
        let is_current_session = job.session_id == current_session_id;
        let pointer = if is_selected { "▸ " } else { "  " };
        let pointer_style = if is_selected {
            accent_style()
        } else {
            Style::default()
        };

        let id_str = format!("{} ", short_job_id(&job.task_id));
        let elapsed = format_elapsed(job.elapsed_ms());
        let model = job.model.as_deref().unwrap_or("");
        let model_str = if model.is_empty() {
            String::new()
        } else {
            format!(" [{model}]")
        };
        let session_marker = if is_current_session { " ★" } else { "" };
        let prompt_text = job.description.replace('\n', " ");
        let prompt_preview = if prompt_text.chars().count() > prompt_max {
            let truncated: String = prompt_text.chars().take(prompt_max).collect();
            format!("  {truncated}…")
        } else {
            format!("  {prompt_text}")
        };

        let row_style = if is_selected {
            highlight_style()
        } else {
            Style::default()
        };

        lines.push(Line::from(vec![
            Span::styled(pointer, pointer_style),
            Span::styled(id_str, row_style.add_modifier(Modifier::DIM)),
            status_badge(job.status),
            Span::styled(model_str, row_style),
            Span::styled(format!("  {elapsed}"), subtle_style()),
            Span::styled(session_marker, accent_style()),
        ]));
        lines.push(Line::from(vec![
            Span::raw("    "),
            Span::styled(prompt_preview, subtle_style()),
        ]));
    }

    lines
}

pub(crate) fn background_job_detail_lines(
    detail: Option<&BackgroundTaskView>,
    _width: usize,
    selected_step: usize,
    collapsed_workflow_steps: &HashSet<String>,
) -> Vec<StyledLine> {
    let mut lines: Vec<StyledLine> = Vec::new();

    let Some(detail) = detail else {
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled("Loading job detail...", subtle_style()),
        ]));
        return lines;
    };

    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            "JOB DETAIL",
            inactive_style().add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        ),
    ]));
    lines.push(Line::from(vec![]));

    let label_style = inactive_style().add_modifier(Modifier::BOLD);

    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled("ID:      ", label_style),
        Span::styled(detail.task_id.clone(), Style::default()),
    ]));
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled("Status:  ", label_style),
        status_badge(detail.status),
    ]));
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled("Model:   ", label_style),
        Span::styled(detail.model.clone().unwrap_or_default(), Style::default()),
    ]));
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled("Elapsed: ", label_style),
        Span::styled(format_elapsed(detail.elapsed_ms()), Style::default()),
    ]));
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled("CWD:     ", label_style),
        Span::styled(detail.cwd.clone(), subtle_style()),
    ]));

    if let Some(ref perm) = detail.permission_mode {
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled("Perms:   ", label_style),
            Span::styled(perm.clone(), Style::default()),
        ]));
    }

    lines.push(Line::from(vec![]));
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled("Prompt:", label_style),
    ]));
    for prompt_line in detail.description.lines() {
        lines.push(Line::from(vec![
            Span::raw("    "),
            Span::styled(prompt_line.to_string(), Style::default()),
        ]));
    }

    if let Some(ref error) = detail.error {
        lines.push(Line::from(vec![]));
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled("Error:", warning_style().add_modifier(Modifier::BOLD)),
        ]));
        for error_line in error.lines() {
            lines.push(Line::from(vec![
                Span::raw("    "),
                Span::styled(error_line.to_string(), warning_style()),
            ]));
        }
    }

    if let Some(ref reason) = detail.cancellation_reason {
        lines.push(Line::from(vec![]));
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(
                "Cancellation reason:",
                inactive_style().add_modifier(Modifier::BOLD),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::raw("    "),
            Span::styled(reason.clone(), inactive_style()),
        ]));
    }

    if let Some(ref workflow_steps) = detail.workflow_steps
        && !workflow_steps.is_empty()
    {
        let visible_indices =
            visible_workflow_step_indices(workflow_steps, collapsed_workflow_steps);
        let selected_index = selected_visible_workflow_step_index(
            workflow_steps,
            selected_step,
            collapsed_workflow_steps,
        )
        .unwrap_or(0);
        lines.push(Line::from(vec![]));
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!(
                    "Workflow steps ({}/{} visible):",
                    visible_indices.len(),
                    workflow_steps.len()
                ),
                label_style,
            ),
        ]));
        for index in visible_indices {
            let step = &workflow_steps[index];
            let is_selected = index == selected_index;
            let is_collapsed = collapsed_workflow_steps.contains(&step.step_key);
            let has_children = workflow_step_has_children(workflow_steps, index);
            lines.push(workflow_step_tree_line(
                step,
                is_selected,
                has_children,
                is_collapsed,
            ));
        }

        lines.push(Line::from(vec![]));
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled("Selected step:", label_style),
        ]));
        lines.extend(workflow_selected_step_lines(
            &workflow_steps[selected_index],
        ));
    }

    if detail.workflow_steps.as_ref().is_none_or(Vec::is_empty)
        && let Some(ref progress_events) = detail.progress_events
        && !progress_events.is_empty()
    {
        lines.push(Line::from(vec![]));
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!("Progress ({} events):", progress_events.len()),
                label_style,
            ),
        ]));
        for event in progress_events {
            let step = event.step_key.as_deref().unwrap_or("-");
            let detail = event
                .message
                .as_deref()
                .or(event.output.as_deref())
                .unwrap_or("");
            let kind = event.kind.as_deref().unwrap_or("");
            let suffix = if detail.is_empty() {
                String::new()
            } else {
                format!("  {detail}")
            };
            lines.push(Line::from(vec![
                Span::raw("    "),
                Span::styled(event.event.clone(), Style::default()),
                Span::styled(format!("  {step}"), subtle_style()),
                Span::styled(format!("  {kind}"), subtle_style()),
                Span::styled(suffix, subtle_style()),
            ]));
        }
    }

    if let Some(ref log_tail) = detail.log_tail
        && !log_tail.is_empty()
    {
        lines.push(Line::from(vec![]));
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(format!("Log (last {} lines):", log_tail.len()), label_style),
        ]));
        for log_line in log_tail {
            lines.push(Line::from(vec![
                Span::raw("    "),
                Span::styled(log_line.clone(), subtle_style()),
            ]));
        }
    }

    lines
}

fn workflow_step_tree_line(
    step: &WorkflowStepView,
    selected: bool,
    has_children: bool,
    is_collapsed: bool,
) -> StyledLine {
    let pointer = if selected { "▸ " } else { "  " };
    let indent = "  ".repeat(step.depth as usize);
    let collapse_marker = if has_children {
        if is_collapsed { "[+]" } else { "[-]" }
    } else {
        "   "
    };
    let style = if selected {
        highlight_style()
    } else {
        Style::default()
    };
    Line::from(vec![
        Span::raw("  "),
        Span::styled(pointer, if selected { accent_style() } else { style }),
        Span::styled(indent, subtle_style()),
        Span::styled(collapse_marker, subtle_style()),
        Span::raw(" "),
        Span::styled(step.step_key.clone(), style.add_modifier(Modifier::DIM)),
        Span::styled(
            format!("  [{}]", step.status),
            workflow_step_status_style(step.status),
        ),
        Span::styled(format!("  {}", step.kind), subtle_style()),
        Span::styled(format!("  {}", step.label), style),
    ])
}

fn workflow_selected_step_lines(step: &WorkflowStepView) -> Vec<StyledLine> {
    let mut lines = Vec::new();
    let label_style = inactive_style().add_modifier(Modifier::BOLD);
    lines.push(Line::from(vec![
        Span::raw("    "),
        Span::styled("Key:    ", label_style),
        Span::styled(step.step_key.clone(), Style::default()),
    ]));
    lines.push(Line::from(vec![
        Span::raw("    "),
        Span::styled("Kind:   ", label_style),
        Span::styled(step.kind.clone(), Style::default()),
    ]));
    lines.push(Line::from(vec![
        Span::raw("    "),
        Span::styled("Status: ", label_style),
        Span::styled(
            step.status.to_string(),
            workflow_step_status_style(step.status),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::raw("    "),
        Span::styled("Label:  ", label_style),
        Span::styled(step.label.clone(), Style::default()),
    ]));
    if let Some(child_session_id) = step
        .child_session_id
        .as_ref()
        .filter(|value| !value.trim().is_empty())
    {
        lines.push(Line::from(vec![
            Span::raw("    "),
            Span::styled("Child:  ", label_style),
            Span::styled(child_session_id.clone(), Style::default()),
        ]));
        lines.push(Line::from(vec![
            Span::raw("    "),
            Span::styled("Enter opens agent step output.", subtle_style()),
        ]));
    }
    if let Some(error) = step.error.as_ref().filter(|value| !value.trim().is_empty()) {
        lines.push(Line::from(vec![]));
        lines.push(Line::from(vec![
            Span::raw("    "),
            Span::styled("Error:", warning_style().add_modifier(Modifier::BOLD)),
        ]));
        for line in error.lines() {
            lines.push(Line::from(vec![
                Span::raw("      "),
                Span::styled(line.to_string(), warning_style()),
            ]));
        }
    }
    if let Some(output) = step
        .output
        .as_ref()
        .filter(|value| !value.trim().is_empty())
    {
        lines.push(Line::from(vec![]));
        lines.push(Line::from(vec![
            Span::raw("    "),
            Span::styled("Output:", label_style),
        ]));
        lines.push(Line::from(vec![
            Span::raw("    "),
            Span::styled("y copies selected step output.", subtle_style()),
        ]));
        for line in output.lines() {
            lines.push(Line::from(vec![
                Span::raw("      "),
                Span::styled(line.to_string(), Style::default()),
            ]));
        }
    } else if step.error.is_none() {
        lines.push(Line::from(vec![
            Span::raw("    "),
            Span::styled("Output: ", label_style),
            Span::styled("(not available yet)", subtle_style()),
        ]));
    }
    lines
}

fn background_jobs_child_session_lines(
    child_session: Option<&BackgroundJobsChildSessionView>,
) -> Vec<StyledLine> {
    let mut lines = Vec::new();
    let Some(child_session) = child_session else {
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled("Loading child session...", subtle_style()),
        ]));
        return lines;
    };
    let label_style = inactive_style().add_modifier(Modifier::BOLD);
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            "AGENT STEP OUTPUT",
            inactive_style().add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        ),
    ]));
    lines.push(Line::from(vec![]));
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled("Session: ", label_style),
        Span::styled(child_session.session_id.clone(), subtle_style()),
    ]));
    if let Some(title) = child_session.title.as_ref() {
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled("Title:   ", label_style),
            Span::styled(title.clone(), Style::default()),
        ]));
    }
    lines.push(Line::from(vec![]));

    if child_session.messages.is_empty() {
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled("No child session messages are available.", subtle_style()),
        ]));
        return lines;
    }

    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled("y copies selected step output.", subtle_style()),
    ]));
    lines.push(Line::from(vec![]));

    for message in &child_session.messages {
        let role = format!("{:?}", message.role).to_lowercase();
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(format!("{role}:"), label_style),
        ]));
        if message.content.trim().is_empty() {
            lines.push(Line::from(vec![
                Span::raw("    "),
                Span::styled("(empty)", subtle_style()),
            ]));
        } else {
            for line in message.content.lines() {
                lines.push(Line::from(vec![
                    Span::raw("    "),
                    Span::styled(line.to_string(), Style::default()),
                ]));
            }
        }
        lines.push(Line::from(vec![]));
    }
    lines
}

fn workflow_step_status_style(status: WorkflowStepViewStatus) -> Style {
    match status {
        WorkflowStepViewStatus::Completed => emphasis_style(),
        WorkflowStepViewStatus::Failed => warning_style(),
        WorkflowStepViewStatus::Cancelled => inactive_style(),
        WorkflowStepViewStatus::Running => accent_style(),
        WorkflowStepViewStatus::Pending => subtle_style(),
        _ => subtle_style(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use orbcode_protocol::{
        BackgroundTaskProgressEvent, BackgroundTaskViewKind, MessageRole, ProviderId,
    };

    fn make_view(id: &str, status: BackgroundTaskViewStatus) -> BackgroundTaskView {
        BackgroundTaskView {
            task_id: id.to_string(),
            session_id: "sess-1".to_string(),
            kind: BackgroundTaskViewKind::BackgroundJob,
            status,
            description: "Run the tests".to_string(),
            cwd: "/tmp".to_string(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            started_at: None,
            finished_at: None,
            pid: Some(12345),
            exit_code: None,
            signal: None,
            error: None,
            model: Some("claude-sonnet-4-6".to_string()),
            provider: Some(ProviderId::Anthropic),
            permission_mode: None,
            agent_type: None,
            child_session_id: None,
            cancellation_reason: None,
            label: None,
            log_tail: None,
            progress_events: None,
            workflow_steps: None,
        }
    }

    fn make_workflow_step(
        step_key: &str,
        parent_key: Option<&str>,
        depth: u32,
        kind: &str,
        label: &str,
        output: Option<&str>,
    ) -> WorkflowStepView {
        WorkflowStepView {
            step_key: step_key.to_string(),
            parent_key: parent_key.map(str::to_string),
            depth,
            kind: kind.to_string(),
            label: label.to_string(),
            status: WorkflowStepViewStatus::Completed,
            started_at: Some(Utc::now()),
            finished_at: Some(Utc::now()),
            output: output.map(str::to_string),
            error: None,
            child_session_id: None,
        }
    }

    fn lines_text(lines: &[StyledLine]) -> String {
        lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn list_lines_empty_shows_placeholder() {
        let lines = background_jobs_list_lines(&[], 0, "", 80);
        let text = lines_text(&lines);
        assert!(text.contains("No background jobs"));
    }

    #[test]
    fn list_lines_shows_jobs() {
        let jobs = vec![
            make_view("aaaa1111", BackgroundTaskViewStatus::Running),
            make_view("bbbb2222", BackgroundTaskViewStatus::Completed),
        ];
        let lines = background_jobs_list_lines(&jobs, 0, "sess-1", 80);
        let text = lines_text(&lines);
        assert!(text.contains("aaaa1111"));
        assert!(text.contains("running"));
        assert!(text.contains("completed"));
    }

    #[test]
    fn key_j_moves_selection_down() {
        let jobs = vec![
            make_view("a", BackgroundTaskViewStatus::Running),
            make_view("b", BackgroundTaskViewStatus::Running),
        ];
        let mut state = BackgroundJobsOverlayState::new(jobs, "test-session".to_string());
        assert_eq!(state.selected, 0);
        apply_background_jobs_key(
            &mut state,
            &KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
        );
        assert_eq!(state.selected, 1);
        apply_background_jobs_key(
            &mut state,
            &KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
        );
        assert_eq!(state.selected, 1);
    }

    #[test]
    fn key_k_moves_selection_up() {
        let jobs = vec![
            make_view("a", BackgroundTaskViewStatus::Running),
            make_view("b", BackgroundTaskViewStatus::Running),
        ];
        let mut state = BackgroundJobsOverlayState::new(jobs, "test-session".to_string());
        state.selected = 1;
        apply_background_jobs_key(
            &mut state,
            &KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE),
        );
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn key_d_cancels_active_job() {
        let jobs = vec![make_view("a", BackgroundTaskViewStatus::Running)];
        let mut state = BackgroundJobsOverlayState::new(jobs, "test-session".to_string());
        let action = apply_background_jobs_key(
            &mut state,
            &KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
        );
        assert_eq!(
            action,
            BackgroundJobsOverlayAction::CancelJob { job_index: 0 }
        );
    }

    #[test]
    fn key_d_does_nothing_for_completed_job() {
        let jobs = vec![make_view("a", BackgroundTaskViewStatus::Completed)];
        let mut state = BackgroundJobsOverlayState::new(jobs, "test-session".to_string());
        let action = apply_background_jobs_key(
            &mut state,
            &KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
        );
        assert_eq!(action, BackgroundJobsOverlayAction::None);
    }

    #[test]
    fn key_q_closes() {
        let mut state = BackgroundJobsOverlayState::new(vec![], "test-session".to_string());
        let action = apply_background_jobs_key(
            &mut state,
            &KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE),
        );
        assert_eq!(action, BackgroundJobsOverlayAction::Close);
    }

    #[test]
    fn enter_requests_detail_refresh() {
        let jobs = vec![make_view("a", BackgroundTaskViewStatus::Running)];
        let mut state = BackgroundJobsOverlayState::new(jobs, "test-session".to_string());
        let action = apply_background_jobs_key(
            &mut state,
            &KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        );
        assert_eq!(action, BackgroundJobsOverlayAction::RequestRefresh);
        assert_eq!(state.view, BackgroundJobsView::Detail);
        assert!(state.detail.is_none());
        let loading = background_job_detail_lines(state.detail.as_ref(), 80, 0, &HashSet::new());
        let text = lines_text(&loading);
        assert!(text.contains("Loading job detail..."));
    }

    #[test]
    fn detail_view_back_on_esc() {
        let jobs = vec![make_view("a", BackgroundTaskViewStatus::Running)];
        let mut state = BackgroundJobsOverlayState::new(jobs, "test-session".to_string());
        state.view = BackgroundJobsView::Detail;
        state.detail = Some(BackgroundTaskView {
            task_id: "a".to_string(),
            session_id: "s".to_string(),
            kind: BackgroundTaskViewKind::BackgroundJob,
            status: BackgroundTaskViewStatus::Running,
            description: "test".to_string(),
            cwd: "/tmp".to_string(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            started_at: None,
            finished_at: None,
            pid: None,
            exit_code: None,
            signal: None,
            error: None,
            model: Some("opus".to_string()),
            provider: Some(ProviderId::Anthropic),
            permission_mode: None,
            agent_type: None,
            child_session_id: None,
            cancellation_reason: None,
            label: None,
            log_tail: Some(vec!["line1".to_string()]),
            progress_events: None,
            workflow_steps: None,
        });
        let action =
            apply_background_jobs_key(&mut state, &KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(action, BackgroundJobsOverlayAction::None);
        assert_eq!(state.view, BackgroundJobsView::List);
        assert!(state.detail.is_none());
    }

    #[test]
    fn detail_lines_contain_metadata() {
        let detail = BackgroundTaskView {
            task_id: "abc123".to_string(),
            session_id: "s".to_string(),
            kind: BackgroundTaskViewKind::BackgroundJob,
            status: BackgroundTaskViewStatus::Running,
            description: "Fix the bug".to_string(),
            cwd: "/home/user/project".to_string(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            started_at: None,
            finished_at: None,
            pid: None,
            exit_code: None,
            signal: None,
            error: None,
            model: Some("claude-sonnet-4-6".to_string()),
            provider: Some(ProviderId::Anthropic),
            permission_mode: Some("default".to_string()),
            agent_type: None,
            child_session_id: None,
            cancellation_reason: None,
            label: None,
            log_tail: Some(vec!["output line".to_string()]),
            progress_events: Some(vec![BackgroundTaskProgressEvent {
                timestamp: Utc::now(),
                event: "step_started".to_string(),
                step_key: Some("step.0".to_string()),
                kind: Some("log".to_string()),
                message: Some("done ok".to_string()),
                output: None,
                child_session_id: None,
            }]),
            workflow_steps: None,
        };
        let lines = background_job_detail_lines(Some(&detail), 80, 0, &HashSet::new());
        let text = lines_text(&lines);
        assert!(text.contains("abc123"));
        assert!(text.contains("claude-sonnet-4-6"));
        assert!(text.contains("Fix the bug"));
        assert!(text.contains("step_started"));
        assert!(text.contains("done ok"));
        assert!(text.contains("output line"));
    }

    #[test]
    fn detail_lines_render_selected_workflow_step_output() {
        let mut detail = make_view("workflow-1", BackgroundTaskViewStatus::Completed);
        detail.kind = BackgroundTaskViewKind::Workflow;
        detail.description = "Nested workflow".to_string();
        detail.progress_events = Some(vec![BackgroundTaskProgressEvent {
            timestamp: Utc::now(),
            event: "step_completed".to_string(),
            step_key: Some("step.0".to_string()),
            kind: Some("phase".to_string()),
            message: Some("flat progress should be hidden".to_string()),
            output: Some("flat output should be hidden".to_string()),
            child_session_id: None,
        }]);
        detail.workflow_steps = Some(vec![
            make_workflow_step("step.0", None, 0, "phase", "Plan", Some("phase output")),
            make_workflow_step(
                "step.0.0",
                Some("step.0"),
                1,
                "log",
                "first",
                Some("first output"),
            ),
            make_workflow_step(
                "step.0.1",
                Some("step.0"),
                1,
                "log",
                "second",
                Some("second line 1\nsecond line 2"),
            ),
        ]);

        let lines = background_job_detail_lines(Some(&detail), 80, 2, &HashSet::new());
        let text = lines_text(&lines);
        assert!(text.contains("Workflow steps (3/3 visible):"));
        assert!(text.contains("step.0.1"));
        assert!(text.contains("Selected step:"));
        assert!(text.contains("second line 1"));
        assert!(text.contains("second line 2"));
        assert!(text.contains("y copies selected step output."));
        assert!(!text.contains("flat progress should be hidden"));
        assert!(!text.contains("flat output should be hidden"));
    }

    #[test]
    fn detail_lines_render_workflow_step_child_session_hint() {
        let mut detail = make_view("workflow-1", BackgroundTaskViewStatus::Completed);
        detail.kind = BackgroundTaskViewKind::Workflow;
        let mut agent = make_workflow_step(
            "step.0",
            None,
            0,
            "agent",
            "Inspect implementation",
            Some("done"),
        );
        agent.child_session_id = Some("session-1:workflow-1:agent-a".to_string());
        detail.workflow_steps = Some(vec![agent]);

        let lines = background_job_detail_lines(Some(&detail), 80, 0, &HashSet::new());
        let text = lines_text(&lines);
        assert!(text.contains("Child:"));
        assert!(text.contains("session-1:workflow-1:agent-a"));
        assert!(text.contains("Enter opens agent step output."));
    }

    #[test]
    fn child_session_view_renders_messages() {
        let mut session = SessionRecord::new();
        session.session_id = "session-1:workflow-1:agent-a".to_string();
        session.title = Some("Agent: inspect".to_string());
        session.push_message(TranscriptMessage::new(MessageRole::User, "inspect prompt"));
        session.push_message(TranscriptMessage::new(
            MessageRole::Assistant,
            "agent output line",
        ));

        let mut state = BackgroundJobsOverlayState::new(
            vec![make_view("workflow-1", BackgroundTaskViewStatus::Completed)],
            "s".to_string(),
        );
        state.set_child_session(session);

        assert_eq!(state.view, BackgroundJobsView::ChildSession);
        let text = lines_text(state.cached_lines(100));
        assert!(text.contains("AGENT STEP OUTPUT"));
        assert!(text.contains("inspect prompt"));
        assert!(text.contains("agent output line"));
    }

    #[test]
    fn child_session_esc_returns_to_workflow_detail() {
        let mut detail = make_view("workflow-1", BackgroundTaskViewStatus::Completed);
        detail.kind = BackgroundTaskViewKind::Workflow;
        detail.workflow_steps = Some(vec![make_workflow_step(
            "step.0",
            None,
            0,
            "agent",
            "Inspect implementation",
            Some("done"),
        )]);
        let mut state = BackgroundJobsOverlayState::new(
            vec![make_view("workflow-1", BackgroundTaskViewStatus::Completed)],
            "s".to_string(),
        );
        state.set_detail(detail);
        let mut session = SessionRecord::new();
        session.session_id = "session-1:workflow-1:agent-a".to_string();
        session.push_message(TranscriptMessage::new(
            MessageRole::Assistant,
            "agent output",
        ));
        state.set_child_session(session);

        let action =
            apply_background_jobs_key(&mut state, &KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(action, BackgroundJobsOverlayAction::None);
        assert_eq!(state.view, BackgroundJobsView::Detail);
        assert!(state.detail.is_some());
        assert!(state.child_session.is_none());
    }

    #[test]
    fn child_session_q_closes_overlay() {
        let mut state = BackgroundJobsOverlayState::new(
            vec![make_view("workflow-1", BackgroundTaskViewStatus::Completed)],
            "s".to_string(),
        );
        let mut session = SessionRecord::new();
        session.session_id = "session-1:workflow-1:agent-a".to_string();
        state.set_child_session(session);

        let action = apply_background_jobs_key(
            &mut state,
            &KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE),
        );
        assert_eq!(action, BackgroundJobsOverlayAction::Close);
    }

    #[test]
    fn detail_keys_select_workflow_steps() {
        let mut detail = make_view("workflow-1", BackgroundTaskViewStatus::Running);
        detail.kind = BackgroundTaskViewKind::Workflow;
        detail.workflow_steps = Some(vec![
            make_workflow_step("step.0", None, 0, "phase", "Plan", None),
            make_workflow_step("step.0.0", Some("step.0"), 1, "log", "first", None),
            make_workflow_step("step.0.1", Some("step.0"), 1, "log", "second", None),
        ]);
        let mut state = BackgroundJobsOverlayState::new(
            vec![make_view("workflow-1", BackgroundTaskViewStatus::Running)],
            "s".to_string(),
        );
        state.set_detail(detail);
        state.scroll = 4;

        apply_background_jobs_key(
            &mut state,
            &KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
        );
        assert_eq!(state.detail_step_selected, 1);
        assert_eq!(state.scroll, 4);

        apply_background_jobs_key(
            &mut state,
            &KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
        );
        apply_background_jobs_key(
            &mut state,
            &KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
        );
        assert_eq!(state.detail_step_selected, 2);

        apply_background_jobs_key(
            &mut state,
            &KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE),
        );
        assert_eq!(state.detail_step_selected, 1);
    }

    #[test]
    fn y_copies_selected_workflow_step_output() {
        let mut detail = make_view("workflow-1", BackgroundTaskViewStatus::Completed);
        detail.kind = BackgroundTaskViewKind::Workflow;
        detail.workflow_steps = Some(vec![
            make_workflow_step("step.0", None, 0, "agent", "First", Some("first output")),
            make_workflow_step(
                "step.1",
                None,
                0,
                "agent",
                "Second",
                Some("second line 1\nsecond line 2"),
            ),
        ]);
        let mut state = BackgroundJobsOverlayState::new(
            vec![make_view("workflow-1", BackgroundTaskViewStatus::Completed)],
            "s".to_string(),
        );
        state.set_detail(detail);
        state.detail_step_selected = 1;

        let action = apply_background_jobs_key(
            &mut state,
            &KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE),
        );
        assert_eq!(
            action,
            BackgroundJobsOverlayAction::CopyWorkflowStepOutput {
                output: "second line 1\nsecond line 2".to_string()
            }
        );
    }

    #[test]
    fn y_copies_selected_workflow_step_output_from_child_session_view() {
        let mut detail = make_view("workflow-1", BackgroundTaskViewStatus::Completed);
        detail.kind = BackgroundTaskViewKind::Workflow;
        let mut agent = make_workflow_step(
            "step.0",
            None,
            0,
            "agent",
            "Inspect implementation",
            Some("selected step output"),
        );
        agent.child_session_id = Some("session-1:workflow-1:agent-a".to_string());
        detail.workflow_steps = Some(vec![agent]);

        let mut state = BackgroundJobsOverlayState::new(
            vec![make_view("workflow-1", BackgroundTaskViewStatus::Completed)],
            "s".to_string(),
        );
        state.set_detail(detail);

        let mut session = SessionRecord::new();
        session.session_id = "session-1:workflow-1:agent-a".to_string();
        session.push_message(TranscriptMessage::new(
            MessageRole::Assistant,
            "child transcript output",
        ));
        state.set_child_session(session);

        let action = apply_background_jobs_key(
            &mut state,
            &KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE),
        );
        assert_eq!(
            action,
            BackgroundJobsOverlayAction::CopyWorkflowStepOutput {
                output: "selected step output".to_string()
            }
        );
        assert_eq!(state.view, BackgroundJobsView::ChildSession);
    }

    #[test]
    fn y_reports_when_selected_workflow_step_has_no_output() {
        let mut detail = make_view("workflow-1", BackgroundTaskViewStatus::Running);
        detail.kind = BackgroundTaskViewKind::Workflow;
        detail.workflow_steps = Some(vec![make_workflow_step(
            "step.0", None, 0, "agent", "Pending", None,
        )]);
        let mut state = BackgroundJobsOverlayState::new(
            vec![make_view("workflow-1", BackgroundTaskViewStatus::Running)],
            "s".to_string(),
        );
        state.set_detail(detail);

        let action = apply_background_jobs_key(
            &mut state,
            &KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE),
        );
        assert_eq!(
            action,
            BackgroundJobsOverlayAction::SetStatus {
                message: "Selected workflow step has no output to copy.".to_string()
            }
        );
    }

    #[test]
    fn space_toggles_selected_workflow_group() {
        let mut detail = make_view("workflow-1", BackgroundTaskViewStatus::Running);
        detail.kind = BackgroundTaskViewKind::Workflow;
        detail.workflow_steps = Some(vec![
            make_workflow_step("step.0", None, 0, "phase", "Plan", Some("phase output")),
            make_workflow_step(
                "step.0.0",
                Some("step.0"),
                1,
                "agent",
                "child hidden when collapsed",
                Some("child output"),
            ),
            make_workflow_step(
                "step.1",
                None,
                0,
                "agent",
                "Sibling",
                Some("sibling output"),
            ),
        ]);
        let mut state = BackgroundJobsOverlayState::new(
            vec![make_view("workflow-1", BackgroundTaskViewStatus::Running)],
            "s".to_string(),
        );
        state.set_detail(detail);

        let action = apply_background_jobs_key(
            &mut state,
            &KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
        );
        assert_eq!(action, BackgroundJobsOverlayAction::None);
        assert!(state.collapsed_workflow_steps.contains("step.0"));
        let text = lines_text(state.cached_lines(100));
        assert!(text.contains("Workflow steps (2/3 visible):"));
        assert!(text.contains("[+]"));
        assert!(!text.contains("child hidden when collapsed"));
        assert!(text.contains("Sibling"));

        apply_background_jobs_key(
            &mut state,
            &KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
        );
        assert!(!state.collapsed_workflow_steps.contains("step.0"));
        let text = lines_text(state.cached_lines(100));
        assert!(text.contains("Workflow steps (3/3 visible):"));
        assert!(text.contains("child hidden when collapsed"));
    }

    #[test]
    fn workflow_step_navigation_skips_collapsed_descendants() {
        let mut detail = make_view("workflow-1", BackgroundTaskViewStatus::Running);
        detail.kind = BackgroundTaskViewKind::Workflow;
        detail.workflow_steps = Some(vec![
            make_workflow_step("step.0", None, 0, "phase", "Plan", None),
            make_workflow_step("step.0.0", Some("step.0"), 1, "agent", "Hidden child", None),
            make_workflow_step("step.1", None, 0, "agent", "Visible sibling", None),
        ]);
        let mut state = BackgroundJobsOverlayState::new(
            vec![make_view("workflow-1", BackgroundTaskViewStatus::Running)],
            "s".to_string(),
        );
        state.set_detail(detail);

        apply_background_jobs_key(
            &mut state,
            &KeyEvent::new(KeyCode::Left, KeyModifiers::NONE),
        );
        assert_eq!(state.detail_step_selected, 0);
        assert!(state.collapsed_workflow_steps.contains("step.0"));

        apply_background_jobs_key(
            &mut state,
            &KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
        );
        assert_eq!(state.detail_step_selected, 2);

        apply_background_jobs_key(
            &mut state,
            &KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE),
        );
        assert_eq!(state.detail_step_selected, 0);
    }

    #[test]
    fn left_right_collapse_expand_and_move_between_parent_child() {
        let mut detail = make_view("workflow-1", BackgroundTaskViewStatus::Running);
        detail.kind = BackgroundTaskViewKind::Workflow;
        detail.workflow_steps = Some(vec![
            make_workflow_step("step.0", None, 0, "phase", "Plan", None),
            make_workflow_step("step.0.0", Some("step.0"), 1, "agent", "Child", None),
        ]);
        let mut state = BackgroundJobsOverlayState::new(
            vec![make_view("workflow-1", BackgroundTaskViewStatus::Running)],
            "s".to_string(),
        );
        state.set_detail(detail);

        apply_background_jobs_key(
            &mut state,
            &KeyEvent::new(KeyCode::Right, KeyModifiers::NONE),
        );
        assert_eq!(state.detail_step_selected, 1);

        apply_background_jobs_key(
            &mut state,
            &KeyEvent::new(KeyCode::Left, KeyModifiers::NONE),
        );
        assert_eq!(state.detail_step_selected, 0);

        apply_background_jobs_key(
            &mut state,
            &KeyEvent::new(KeyCode::Left, KeyModifiers::NONE),
        );
        assert!(state.collapsed_workflow_steps.contains("step.0"));

        apply_background_jobs_key(
            &mut state,
            &KeyEvent::new(KeyCode::Right, KeyModifiers::NONE),
        );
        assert!(!state.collapsed_workflow_steps.contains("step.0"));
        assert_eq!(state.detail_step_selected, 0);
    }

    #[test]
    fn enter_opens_selected_workflow_step_child_session() {
        let mut detail = make_view("workflow-1", BackgroundTaskViewStatus::Completed);
        detail.kind = BackgroundTaskViewKind::Workflow;
        let mut agent =
            make_workflow_step("step.1", None, 0, "agent", "Inspect implementation", None);
        agent.child_session_id = Some("session-1:workflow-1:agent-a".to_string());
        detail.workflow_steps = Some(vec![
            make_workflow_step("step.0", None, 0, "phase", "Plan", None),
            agent,
        ]);
        let mut state = BackgroundJobsOverlayState::new(
            vec![make_view("workflow-1", BackgroundTaskViewStatus::Completed)],
            "s".to_string(),
        );
        state.set_detail(detail);

        let action = apply_background_jobs_key(
            &mut state,
            &KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        );
        assert_eq!(action, BackgroundJobsOverlayAction::None);

        state.detail_step_selected = 1;
        let action = apply_background_jobs_key(
            &mut state,
            &KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        );
        assert_eq!(
            action,
            BackgroundJobsOverlayAction::OpenChildSession {
                session_id: "session-1:workflow-1:agent-a".to_string()
            }
        );
    }

    #[test]
    fn ctrl_c_cancels_active_job_in_list() {
        let jobs = vec![make_view("a", BackgroundTaskViewStatus::Running)];
        let mut state = BackgroundJobsOverlayState::new(jobs, "test-session".to_string());
        let action = apply_background_jobs_key(
            &mut state,
            &KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        );
        assert_eq!(
            action,
            BackgroundJobsOverlayAction::CancelJob { job_index: 0 }
        );
    }

    #[test]
    fn ctrl_c_closes_when_no_active_job() {
        let jobs = vec![make_view("a", BackgroundTaskViewStatus::Completed)];
        let mut state = BackgroundJobsOverlayState::new(jobs, "test-session".to_string());
        let action = apply_background_jobs_key(
            &mut state,
            &KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        );
        assert_eq!(action, BackgroundJobsOverlayAction::Close);
    }

    #[test]
    fn format_elapsed_formats_correctly() {
        assert_eq!(format_elapsed(500), "500ms");
        assert_eq!(format_elapsed(5000), "5s");
        assert_eq!(format_elapsed(65000), "1m05s");
        assert_eq!(format_elapsed(3_661_000), "1h01m");
    }

    #[test]
    fn update_jobs_clamps_selected() {
        let jobs = vec![
            make_view("a", BackgroundTaskViewStatus::Running),
            make_view("b", BackgroundTaskViewStatus::Running),
        ];
        let mut state = BackgroundJobsOverlayState::new(jobs, "test-session".to_string());
        state.selected = 1;
        state.update_jobs(vec![make_view("a", BackgroundTaskViewStatus::Running)]);
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn narrow_workflow_projection_reaches_bottom_and_keeps_selection_visible() {
        for width in [20_u16, 40, 80] {
            let mut detail = make_view("workflow-1", BackgroundTaskViewStatus::Running);
            detail.kind = BackgroundTaskViewKind::Workflow;
            detail.description = "A workflow prompt that wraps at narrow widths".repeat(2);
            detail.workflow_steps = Some(vec![
                make_workflow_step(
                    "step.0",
                    None,
                    0,
                    "phase",
                    "Plan a deliberately long workflow label",
                    Some("phase output "),
                ),
                make_workflow_step(
                    "step.0.0",
                    Some("step.0"),
                    1,
                    "agent",
                    "Inspect a deeply nested implementation with a long label",
                    Some(&"nested output ".repeat(12)),
                ),
                make_workflow_step(
                    "step.1",
                    None,
                    0,
                    "agent",
                    "Finish the workflow",
                    Some(&"final output ".repeat(12)),
                ),
            ]);
            let mut state = BackgroundJobsOverlayState::new(
                vec![make_view("workflow-1", BackgroundTaskViewStatus::Running)],
                "s".to_string(),
            );
            state.set_detail(detail);
            let area = Rect::new(0, 0, width, 9);

            apply_background_jobs_key(&mut state, &KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
            apply_background_jobs_key(
                &mut state,
                &KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
            );
            apply_background_jobs_key(
                &mut state,
                &KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
            );
            sync_background_jobs_overlay_bounds(&mut state, area);

            let visible = lines_text(
                &state.cached_visible_lines(width as usize, background_jobs_content_height(area)),
            );
            assert!(visible.contains('▸'), "width {width}: {visible}");
            let expected_max_scroll = state
                .cached_lines(width as usize)
                .len()
                .saturating_sub(background_jobs_content_height(area));
            assert_eq!(state.max_scroll, expected_max_scroll);

            state.scroll = state.max_scroll;
            let last_visible = state
                .cached_visible_lines(width as usize, background_jobs_content_height(area))
                .last()
                .cloned();
            assert_eq!(
                last_visible,
                state.cached_lines(width as usize).last().cloned()
            );
        }
    }

    #[test]
    fn workflow_refresh_and_child_round_trip_preserve_selected_step_anchor() {
        let mut detail = make_view("workflow-1", BackgroundTaskViewStatus::Running);
        detail.kind = BackgroundTaskViewKind::Workflow;
        let mut selected = make_workflow_step(
            "step.1",
            None,
            0,
            "agent",
            "Selected child with long output",
            Some(&"initial output ".repeat(20)),
        );
        selected.child_session_id = Some("child-session".to_string());
        detail.workflow_steps = Some(vec![
            make_workflow_step("step.0", None, 0, "phase", "First", None),
            selected,
        ]);
        let mut state = BackgroundJobsOverlayState::new(
            vec![make_view("workflow-1", BackgroundTaskViewStatus::Running)],
            "s".to_string(),
        );
        state.set_detail(detail.clone());
        state.detail_step_selected = 1;
        let area = Rect::new(0, 0, 24, 8);
        sync_background_jobs_overlay_bounds(&mut state, area);
        let anchored_scroll = state.scroll;
        assert!(anchored_scroll > 0);

        detail.workflow_steps.as_mut().unwrap()[1].output = Some("grown output ".repeat(40));
        state.set_detail(detail);
        sync_background_jobs_overlay_bounds(&mut state, area);
        let visible =
            lines_text(&state.cached_visible_lines(24, background_jobs_content_height(area)));
        assert!(
            visible.contains('▸'),
            "selected step lost after growth: {visible}"
        );

        let detail_scroll = state.scroll;
        let mut child = SessionRecord::new();
        child.session_id = "child-session".to_string();
        child.push_message(TranscriptMessage::new(
            MessageRole::Assistant,
            "child output",
        ));
        state.set_child_session(child);
        apply_background_jobs_key(&mut state, &KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(state.scroll, detail_scroll);
        sync_background_jobs_overlay_bounds(&mut state, area);
        let visible =
            lines_text(&state.cached_visible_lines(24, background_jobs_content_height(area)));
        assert!(
            visible.contains('▸'),
            "selected step lost after child view: {visible}"
        );
    }
}
