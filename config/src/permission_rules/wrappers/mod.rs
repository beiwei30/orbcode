// Each wrapper parses sequential CLI flags by checking flag tokens in order.
// Several branches legitimately share a body (e.g. `return None` for both
// `--` and `--help`) because they encode distinct semantic cases. Collapsing
// them with `||` would obscure the per-flag intent.
#![allow(clippy::if_same_then_else)]

pub(super) mod container_cli;
pub(super) mod editor_env;
pub(super) mod env_runners;
pub(super) mod env_wrap;
pub(super) mod namespace;
pub(super) mod node_runners;
pub(super) mod package_runner;
pub(super) mod pager;
pub(super) mod privilege;
pub(super) mod remote_shell;
pub(super) mod scheduling;
pub(super) mod system_runners;
pub(super) mod vcs_cmd;
