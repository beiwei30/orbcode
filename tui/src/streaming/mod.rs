//! Streaming-render helpers layered on top of Orb Code's assistant-message
//! emission pipeline (`history_cell::state`).
//!
//! When incremental streaming commit is active, these helpers refine *which*
//! rendered lines are eligible to commit to scrollback (`table_holdback`) so
//! non-incremental content such as markdown tables is not torn while it is
//! still streaming.

pub(crate) mod table_holdback;
