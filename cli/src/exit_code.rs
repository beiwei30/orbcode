//! Granular, documented exit codes for the headless `-p/--print` path.
//!
//! The TypeScript CLI (`src/cli/print.ts`) collapses every headless outcome to
//! `0` (success) or `1` via `gracefulShutdownSync(lastMessage.is_error ? 1 : 0)`
//! and carries the real granularity in the `result.subtype` field. Orb Code keeps
//! those `subtype` strings byte-compatible with the TS SDK schema, but *also*
//! maps each outcome to a distinct numeric exit code so CI pipelines and editor
//! integrations can branch on `$?` without parsing stdout.
//!
//! Exit-code <-> meaning table (kept in lockstep with [`HeadlessOutcome`]):
//!
//! | code | outcome            | result.subtype           | meaning                                              |
//! |------|--------------------|--------------------------|------------------------------------------------------|
//! | 0    | `Success`          | `success`                | turn completed normally (`end_turn`)                 |
//! | 1    | `ExecutionError`   | `error_during_execution` | model / provider / tool error (generic)              |
//! | 2    | `InvalidCliInput`  | (no result emitted)      | bad flag combination / missing prompt (pre-flight)   |
//! | 3    | `AuthFailure`      | `error_during_execution` | credentials rejected (provider auth error / `401`)   |
//! | 4    | `PermissionDenied` | `error_during_execution` | a tool call was denied by permission policy          |
//! | 5    | `Cancelled`        | `error_during_execution` | turn interrupted (Ctrl-C / cancel)                   |
//! | 6    | `MaxTurns`         | `error_max_turns`        | agent loop hit the turn ceiling (not yet emitted)    |
//! | 7    | `MaxBudget`        | `error_max_budget_usd`   | budget ceiling hit (wired by budget-enforcement)     |
//!
//! Codes 5 and 6 are pinned by unit tests but are not yet reachable from a
//! headless subprocess, so they have no end-to-end coverage: the `-p/--print`
//! path (`run_print_mode`) installs no SIGINT handler — Ctrl-C terminates with
//! the default signal disposition (130), not 5 — and no stub/mock provider
//! scenario emits a `TurnCancelled` on that path; max-turns has neither a
//! `--max-turns` flag nor any core terminal signal. The [`tests`] module locks
//! their code/subtype/is_error contract until those runtime paths are wired.

use orbcode_app_server::StreamErrorCategory;

/// Exit code reserved for budget exhaustion. The headless loop does not yet
/// detect budget exhaustion; this constant is wired by the budget-enforcement
/// branch, which will classify the turn as [`HeadlessOutcome::MaxBudget`].
#[allow(dead_code)]
pub const MAX_BUDGET_EXIT_CODE: i32 = HeadlessOutcome::MaxBudget.code();

/// Terminal classification of a headless `-p/--print` run. Each variant owns a
/// distinct numeric exit code and the SDK-compatible `result.subtype` string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeadlessOutcome {
    Success,
    ExecutionError,
    InvalidCliInput,
    AuthFailure,
    PermissionDenied,
    Cancelled,
    /// Reserved: the agent loop does not yet surface a max-turns terminal state.
    #[allow(dead_code)]
    MaxTurns,
    /// Reserved placeholder; wired by the budget-enforcement branch.
    MaxBudget,
}

impl HeadlessOutcome {
    /// Numeric process exit code. Kept in lockstep with the module table.
    pub const fn code(self) -> i32 {
        match self {
            Self::Success => 0,
            Self::ExecutionError => 1,
            Self::InvalidCliInput => 2,
            Self::AuthFailure => 3,
            Self::PermissionDenied => 4,
            Self::Cancelled => 5,
            Self::MaxTurns => 6,
            Self::MaxBudget => 7,
        }
    }

    /// SDK-compatible `result.subtype`. Mirrors the strings the TypeScript
    /// QueryEngine writes so stream-json stays schema-compatible: `Success` maps
    /// to `success`; max-turns / max-budget have dedicated TS subtypes; every
    /// other error falls back to the catch-all `error_during_execution`.
    pub const fn result_subtype(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::MaxTurns => "error_max_turns",
            Self::MaxBudget => "error_max_budget_usd",
            Self::ExecutionError
            | Self::InvalidCliInput
            | Self::AuthFailure
            | Self::PermissionDenied
            | Self::Cancelled => "error_during_execution",
        }
    }

    /// Whether this outcome is an error (anything but `Success`).
    pub const fn is_error(self) -> bool {
        !matches!(self, Self::Success)
    }
}

/// Classify the terminal outcome of a headless turn from the signals the
/// `-p/--print` loop collected.
///
/// Precedence (first match wins): a fatal provider/model error outranks a
/// cancellation, which outranks a terminal tool permission denial, which
/// outranks a clean success. Auth errors are split out of the generic
/// execution-error bucket so callers can distinguish "fix your credentials" from
/// a transient model error.
///
/// `permission_denied_terminal` means a tool was denied *and* the turn produced
/// no successful model output as a result, so the denial blocked all progress. A
/// denial the model recovered from (it still produced a final answer) is *not*
/// terminal: it is recorded in `result.permission_denials` but the run is a
/// success, matching the TypeScript CLI.
pub fn classify_outcome(
    error_category: Option<StreamErrorCategory>,
    had_error: bool,
    was_cancelled: bool,
    permission_denied_terminal: bool,
) -> HeadlessOutcome {
    if had_error {
        return match error_category {
            Some(StreamErrorCategory::Auth) => HeadlessOutcome::AuthFailure,
            _ => HeadlessOutcome::ExecutionError,
        };
    }
    if was_cancelled {
        return HeadlessOutcome::Cancelled;
    }
    if permission_denied_terminal {
        return HeadlessOutcome::PermissionDenied;
    }
    HeadlessOutcome::Success
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_distinct_and_match_table() {
        let outcomes = [
            HeadlessOutcome::Success,
            HeadlessOutcome::ExecutionError,
            HeadlessOutcome::InvalidCliInput,
            HeadlessOutcome::AuthFailure,
            HeadlessOutcome::PermissionDenied,
            HeadlessOutcome::Cancelled,
            HeadlessOutcome::MaxTurns,
            HeadlessOutcome::MaxBudget,
        ];
        let mut codes: Vec<i32> = outcomes.iter().map(|o| o.code()).collect();
        let total = codes.len();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), total, "every outcome must own a distinct code");
        assert_eq!(HeadlessOutcome::Success.code(), 0);
        assert_eq!(HeadlessOutcome::MaxBudget.code(), MAX_BUDGET_EXIT_CODE);
    }

    #[test]
    fn only_success_is_non_error() {
        assert!(!HeadlessOutcome::Success.is_error());
        assert!(HeadlessOutcome::ExecutionError.is_error());
        assert!(HeadlessOutcome::AuthFailure.is_error());
        assert!(HeadlessOutcome::PermissionDenied.is_error());
        assert!(HeadlessOutcome::Cancelled.is_error());
    }

    #[test]
    fn subtypes_match_typescript_strings() {
        assert_eq!(HeadlessOutcome::Success.result_subtype(), "success");
        assert_eq!(
            HeadlessOutcome::ExecutionError.result_subtype(),
            "error_during_execution"
        );
        assert_eq!(
            HeadlessOutcome::AuthFailure.result_subtype(),
            "error_during_execution"
        );
        assert_eq!(
            HeadlessOutcome::MaxTurns.result_subtype(),
            "error_max_turns"
        );
        assert_eq!(
            HeadlessOutcome::MaxBudget.result_subtype(),
            "error_max_budget_usd"
        );
    }

    #[test]
    fn classification_precedence() {
        // A fatal error outranks every other signal.
        assert_eq!(
            classify_outcome(Some(StreamErrorCategory::ServerError), true, true, true),
            HeadlessOutcome::ExecutionError
        );
        // Auth errors split out of the generic execution-error bucket.
        assert_eq!(
            classify_outcome(Some(StreamErrorCategory::Auth), true, false, false),
            HeadlessOutcome::AuthFailure
        );
        // Cancellation outranks a permission denial.
        assert_eq!(
            classify_outcome(None, false, true, true),
            HeadlessOutcome::Cancelled
        );
        assert_eq!(
            classify_outcome(None, false, false, true),
            HeadlessOutcome::PermissionDenied
        );
        assert_eq!(
            classify_outcome(None, false, false, false),
            HeadlessOutcome::Success
        );
    }

    /// Exit codes 5 (`Cancelled`) and 6 (`MaxTurns`) are pinned here at the unit
    /// level because neither is reachable from a headless subprocess as the CLI
    /// stands, so an end-to-end test cannot drive them (see the module docs):
    ///
    /// - `Cancelled` (5): `run_print_mode` installs no `ctrl_c` handler, so
    ///   Ctrl-C exits with 130, and no stub/mock provider scenario cancels a
    ///   turn — nothing emits `TurnCancelled` on the `-p` path.
    /// - `MaxTurns` (6): there is no `--max-turns` flag and core emits no
    ///   max-turns terminal signal; it is a reserved seam.
    ///
    /// These assertions lock the code/subtype/is_error contract so the mappings
    /// stay correct for the day those runtime paths are wired.
    #[test]
    fn cancel_and_max_turns_mappings_are_pinned() {
        assert_eq!(HeadlessOutcome::Cancelled.code(), 5);
        assert_eq!(
            HeadlessOutcome::Cancelled.result_subtype(),
            "error_during_execution"
        );
        assert!(HeadlessOutcome::Cancelled.is_error());

        assert_eq!(HeadlessOutcome::MaxTurns.code(), 6);
        assert_eq!(
            HeadlessOutcome::MaxTurns.result_subtype(),
            "error_max_turns"
        );
        assert!(HeadlessOutcome::MaxTurns.is_error());

        // A cancelled turn classifies as Cancelled whenever no fatal error
        // preceded it, independent of any non-terminal permission denial.
        assert_eq!(
            classify_outcome(None, false, true, false),
            HeadlessOutcome::Cancelled
        );
    }
}
