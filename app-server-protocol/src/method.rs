// ---------------------------------------------------------------------------
// Lifecycle
// ---------------------------------------------------------------------------
pub const INITIALIZE: &str = "initialize";

// ---------------------------------------------------------------------------
// Session
// ---------------------------------------------------------------------------
pub const SESSION_BOOTSTRAP: &str = "session/bootstrap";
pub const SESSION_LIST: &str = "session/list";
pub const SESSION_RENAME: &str = "session/rename";
pub const SESSION_FORK: &str = "session/fork";
pub const SESSION_CLEAR: &str = "session/clear";
pub const SESSION_REWIND: &str = "session/rewind";
pub const SESSION_RECORD_MESSAGE: &str = "session/record_message";
pub const SESSION_COMPACT: &str = "session/compact";
pub const SESSION_COMPACT_DECISION: &str = "session/compact_decision";
pub const SESSION_FIND_BY_TITLE: &str = "session/find_by_title";
pub const SESSION_ACP_LOAD_PREFLIGHT: &str = "session/acp_load_preflight";
pub const SESSION_ACP_LOAD_SETUP: &str = "session/acp_load_setup";
pub const SESSION_ACP_RESUME_SETUP: &str = "session/acp_resume_setup";
pub const SESSION_ACP_DELETE: &str = "session/acp_delete";
pub const SESSION_ACP_CLOSE: &str = "session/acp_close";
pub const SESSION_CONTROL_STATE: &str = "session/control_state";
pub const SESSION_SET_PERMISSION_MODE: &str = "session/set_permission_mode";
pub const SESSION_SET_MODEL: &str = "session/set_model";
pub const SESSION_SET_EFFORT: &str = "session/set_effort";
pub const SESSION_GOAL_GET: &str = "session/goal/get";
pub const SESSION_GOAL_SET: &str = "session/goal/set";
pub const SESSION_GOAL_CLEAR: &str = "session/goal/clear";
pub const SESSION_GOAL_CONTINUE: &str = "session/goal/continue";

// ---------------------------------------------------------------------------
// Turn
// ---------------------------------------------------------------------------
pub const TURN_SUBMIT: &str = "turn/submit";
pub const TURN_STEER: &str = "turn/steer";
pub const TURN_CANCEL: &str = "turn/cancel";
pub const TURN_INTERRUPT: &str = "turn/interrupt";

// ---------------------------------------------------------------------------
// Permission
// ---------------------------------------------------------------------------
pub const PERMISSION_RESPOND: &str = "permission/respond";
pub const PERMISSION_OVERVIEW: &str = "permission/overview";
pub const PERMISSION_MODE: &str = "permission/mode";
pub const PERMISSION_SET_MODE: &str = "permission/set_mode";
pub const PERMISSION_ADD_RULE: &str = "permission/add_rule";
pub const PERMISSION_REMOVE_RULE: &str = "permission/remove_rule";
pub const PERMISSION_ADD_SESSION_RULE: &str = "permission/add_session_rule";
pub const PERMISSION_REMOVE_SESSION_RULE: &str = "permission/remove_session_rule";
pub const PERMISSION_ADD_DIRECTORY: &str = "permission/add_directory";
pub const PERMISSION_VALIDATE_DIRECTORY: &str = "permission/validate_directory";

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------
pub const SETTINGS_MODEL_NAME: &str = "settings/model_name";
pub const SETTINGS_MODEL_OPTIONS: &str = "settings/model_options";
pub const SETTINGS_SET_MODEL: &str = "settings/set_model";
pub const SETTINGS_SET_THINKING_BUDGET: &str = "settings/set_thinking_budget";
pub const SETTINGS_PROVIDERS: &str = "settings/providers";
pub const SETTINGS_THEME: &str = "settings/theme";
pub const SETTINGS_SET_THEME: &str = "settings/set_theme";
pub const SETTINGS_EFFORT: &str = "settings/effort";
pub const SETTINGS_SET_EFFORT: &str = "settings/set_effort";
pub const SETTINGS_OUTPUT_STYLE: &str = "settings/output_style";
pub const SETTINGS_SET_OUTPUT_STYLE: &str = "settings/set_output_style";
pub const SETTINGS_SANDBOX: &str = "settings/sandbox";
pub const SETTINGS_UPDATE_SANDBOX: &str = "settings/update_sandbox";
pub const SETTINGS_KEYBINDINGS: &str = "settings/keybindings";
pub const SETTINGS_LOAD_KEYBINDINGS: &str = "settings/load_keybindings";
pub const SETTINGS_EDITOR_MODE: &str = "settings/editor_mode";
pub const SETTINGS_SET_EDITOR_MODE: &str = "settings/set_editor_mode";
pub const SETTINGS_OUTPUT_STYLE_OPTIONS: &str = "settings/output_style_options";
pub const SETTINGS_ACTIVE_OUTPUT_STYLE: &str = "settings/active_output_style";
pub const SETTINGS_IS_LOCKED: &str = "settings/is_locked";
pub const SETTINGS_SET_AUTO_MEMORY: &str = "settings/set_auto_memory";
pub const SETTINGS_ENSURE_MEMORY_FILE: &str = "settings/ensure_memory_file";
pub const SETTINGS_ADD_SANDBOX_EXCLUDED: &str = "settings/add_sandbox_excluded";
pub const SETTINGS_ALLOW_ALL: &str = "settings/allow_all";
pub const SETTINGS_SET_ALLOW_ALL: &str = "settings/set_allow_all";

// ---------------------------------------------------------------------------
// Context / Usage
// ---------------------------------------------------------------------------
pub const CONTEXT_PREVIEW: &str = "context/preview";
pub const CONTEXT_OVERVIEW: &str = "context/overview";
pub const USAGE_OVERVIEW: &str = "usage/overview";
pub const USAGE_COST: &str = "usage/cost";
pub const USAGE_STATS: &str = "usage/stats";

// ---------------------------------------------------------------------------
// MCP
// ---------------------------------------------------------------------------
pub const MCP_LIST_SERVERS: &str = "mcp/list_servers";
pub const MCP_STATUS: &str = "mcp/status";
pub const MCP_SERVER_TRUST: &str = "mcp/server_trust";
pub const MCP_SET_TRUST: &str = "mcp/set_trust";
pub const MCP_LIST_TOOLS: &str = "mcp/list_tools";
pub const MCP_LIST_RESOURCES: &str = "mcp/list_resources";
pub const MCP_READ_RESOURCE: &str = "mcp/read_resource";
pub const MCP_LIST_PROMPTS: &str = "mcp/list_prompts";
pub const MCP_GET_PROMPT: &str = "mcp/get_prompt";
pub const MCP_INVOKE_TOOL: &str = "mcp/invoke_tool";
pub const MCP_DIAGNOSE: &str = "mcp/diagnose";
pub const MCP_UPSERT_SERVER: &str = "mcp/upsert_server";
pub const MCP_REMOVE_SERVER: &str = "mcp/remove_server";
pub const MCP_CAPABILITIES: &str = "mcp/capabilities";
pub const MCP_SLASH_SUGGESTIONS: &str = "mcp/slash_suggestions";
pub const MCP_OAUTH_OVERVIEW: &str = "mcp/oauth_overview";
pub const MCP_LOGOUT_OAUTH_TOKEN: &str = "mcp/logout_oauth_token";

// ---------------------------------------------------------------------------
// Tools
// ---------------------------------------------------------------------------
pub const TOOLS_LIST: &str = "tools/list";
pub const TOOLS_INVOKE: &str = "tools/invoke";
pub const TOOLS_SKILLS: &str = "tools/skills";
pub const TOOLS_AGENTS: &str = "tools/agents";
pub const TOOLS_PLAN: &str = "tools/plan";
pub const TOOLS_TASK_LIST: &str = "tools/task_list";
pub const TOOLS_ENTER_PLAN: &str = "tools/enter_plan";
pub const TOOLS_AGENTS_WITH_WARNINGS: &str = "tools/agents_with_warnings";
pub const TOOLS_SEED_READ_STATE: &str = "tools/seed_read_state";

// ---------------------------------------------------------------------------
// Background
// ---------------------------------------------------------------------------
pub const BACKGROUND_CREATE: &str = "background/create";
pub const BACKGROUND_LIST: &str = "background/list";
pub const BACKGROUND_DETAIL: &str = "background/detail";
pub const BACKGROUND_CANCEL: &str = "background/cancel";
pub const BACKGROUND_LOG: &str = "background/log";
pub const BACKGROUND_EVENTS: &str = "background/events";
pub const BACKGROUND_LIST_SUMMARY: &str = "background/list_summary";
pub const BACKGROUND_SUBSCRIBE: &str = "background/subscribe";
pub const BACKGROUND_CANCEL_ASYNC: &str = "background/cancel_async";

// ---------------------------------------------------------------------------
// Workflows
// ---------------------------------------------------------------------------
pub const WORKFLOW_LIST: &str = "workflow/list";
pub const WORKFLOW_START: &str = "workflow/start";
pub const WORKFLOW_START_DYNAMIC: &str = "workflow/start_dynamic";
pub const WORKFLOW_RESUME: &str = "workflow/resume";

// ---------------------------------------------------------------------------
// Auth
// ---------------------------------------------------------------------------
pub const AUTH_OVERVIEW: &str = "auth/overview";
pub const AUTH_LOGIN: &str = "auth/login";
pub const AUTH_LOGOUT: &str = "auth/logout";

// ---------------------------------------------------------------------------
// Diagnostics
// ---------------------------------------------------------------------------
pub const DIAGNOSTICS_STATUS: &str = "diagnostics/status";
pub const DIAGNOSTICS_MEMORY: &str = "diagnostics/memory";
pub const DIAGNOSTICS_DOCTOR: &str = "diagnostics/doctor";
pub const DIAGNOSTICS_HOOKS: &str = "diagnostics/hooks";
pub const DIAGNOSTICS_DIFF: &str = "diagnostics/diff";
pub const DIAGNOSTICS_ADVANCED: &str = "diagnostics/advanced";
pub const DIAGNOSTICS_CLEANUP_CHILD_SESSIONS: &str = "diagnostics/cleanup_child_sessions";
pub const DIAGNOSTICS_LAST_REQUEST: &str = "diagnostics/last_request";
pub const DIAGNOSTICS_PRE_USER_INSTRUCTIONS: &str = "diagnostics/pre_user_instructions";

// ---------------------------------------------------------------------------
// Notifications (server -> client, no response expected)
// ---------------------------------------------------------------------------
pub const NOTIFICATION_STREAM_EVENT: &str = "stream/event";

// ---------------------------------------------------------------------------
// Server-initiated requests (server -> client, response expected)
// ---------------------------------------------------------------------------
pub const SERVER_REQUEST_PERMISSION: &str = "permission/request";
pub const SERVER_REQUEST_MCP_TRUST: &str = "mcp_trust/request";
pub const SERVER_REQUEST_ASK_USER: &str = "ask_user/request";

/// Returns all methods the client can call (client -> server requests).
///
/// This excludes server-initiated notifications and server-initiated requests.
/// It is the union of [`stable_client_request_methods`] and
/// [`experimental_client_request_methods`].
pub fn client_request_methods() -> Vec<&'static str> {
    let mut all = stable_client_request_methods();
    all.extend(experimental_client_request_methods());
    all
}

/// Returns client request methods that are considered **stable** --
/// their wire shape and semantics are committed and will not change in
/// backward-incompatible ways without a protocol version bump.
pub fn stable_client_request_methods() -> Vec<&'static str> {
    vec![
        // Lifecycle
        INITIALIZE,
        // Session
        SESSION_BOOTSTRAP,
        SESSION_LIST,
        SESSION_RENAME,
        SESSION_FORK,
        SESSION_CLEAR,
        SESSION_REWIND,
        SESSION_RECORD_MESSAGE,
        SESSION_COMPACT,
        SESSION_COMPACT_DECISION,
        SESSION_FIND_BY_TITLE,
        // Turn
        TURN_SUBMIT,
        TURN_STEER,
        TURN_CANCEL,
        TURN_INTERRUPT,
        // Permission
        PERMISSION_RESPOND,
        PERMISSION_OVERVIEW,
        PERMISSION_MODE,
        PERMISSION_SET_MODE,
        PERMISSION_ADD_RULE,
        PERMISSION_REMOVE_RULE,
        PERMISSION_ADD_SESSION_RULE,
        PERMISSION_REMOVE_SESSION_RULE,
        PERMISSION_ADD_DIRECTORY,
        PERMISSION_VALIDATE_DIRECTORY,
        // Settings
        SETTINGS_MODEL_NAME,
        SETTINGS_MODEL_OPTIONS,
        SETTINGS_SET_MODEL,
        SETTINGS_SET_THINKING_BUDGET,
        SETTINGS_PROVIDERS,
        SETTINGS_THEME,
        SETTINGS_SET_THEME,
        SETTINGS_EFFORT,
        SETTINGS_SET_EFFORT,
        SETTINGS_OUTPUT_STYLE,
        SETTINGS_SET_OUTPUT_STYLE,
        SETTINGS_SANDBOX,
        SETTINGS_UPDATE_SANDBOX,
        SETTINGS_KEYBINDINGS,
        SETTINGS_LOAD_KEYBINDINGS,
        SETTINGS_EDITOR_MODE,
        SETTINGS_SET_EDITOR_MODE,
        SETTINGS_OUTPUT_STYLE_OPTIONS,
        SETTINGS_ACTIVE_OUTPUT_STYLE,
        SETTINGS_IS_LOCKED,
        SETTINGS_SET_AUTO_MEMORY,
        SETTINGS_ENSURE_MEMORY_FILE,
        SETTINGS_ADD_SANDBOX_EXCLUDED,
        SETTINGS_ALLOW_ALL,
        SETTINGS_SET_ALLOW_ALL,
        // Context / Usage
        CONTEXT_PREVIEW,
        CONTEXT_OVERVIEW,
        USAGE_OVERVIEW,
        USAGE_COST,
        USAGE_STATS,
        // MCP
        MCP_LIST_SERVERS,
        MCP_STATUS,
        MCP_SERVER_TRUST,
        MCP_SET_TRUST,
        MCP_LIST_TOOLS,
        MCP_LIST_RESOURCES,
        MCP_READ_RESOURCE,
        MCP_LIST_PROMPTS,
        MCP_GET_PROMPT,
        MCP_INVOKE_TOOL,
        MCP_DIAGNOSE,
        MCP_UPSERT_SERVER,
        MCP_REMOVE_SERVER,
        MCP_CAPABILITIES,
        MCP_SLASH_SUGGESTIONS,
        MCP_OAUTH_OVERVIEW,
        MCP_LOGOUT_OAUTH_TOKEN,
        // Tools
        TOOLS_LIST,
        TOOLS_INVOKE,
        TOOLS_SKILLS,
        TOOLS_AGENTS,
        TOOLS_PLAN,
        TOOLS_TASK_LIST,
        TOOLS_ENTER_PLAN,
        TOOLS_AGENTS_WITH_WARNINGS,
        TOOLS_SEED_READ_STATE,
        // Auth
        AUTH_OVERVIEW,
        AUTH_LOGIN,
        AUTH_LOGOUT,
        // Diagnostics
        DIAGNOSTICS_STATUS,
        DIAGNOSTICS_MEMORY,
        DIAGNOSTICS_DOCTOR,
        DIAGNOSTICS_HOOKS,
        DIAGNOSTICS_DIFF,
        DIAGNOSTICS_ADVANCED,
        DIAGNOSTICS_CLEANUP_CHILD_SESSIONS,
        DIAGNOSTICS_LAST_REQUEST,
        DIAGNOSTICS_PRE_USER_INSTRUCTIONS,
    ]
}

/// Returns client request methods that are considered **experimental** --
/// their wire shape or semantics may change between releases without a
/// protocol version bump. Clients should handle these gracefully if the
/// server drops or alters them.
pub fn experimental_client_request_methods() -> Vec<&'static str> {
    vec![
        // Session
        SESSION_ACP_LOAD_PREFLIGHT,
        SESSION_ACP_LOAD_SETUP,
        SESSION_ACP_RESUME_SETUP,
        SESSION_ACP_DELETE,
        SESSION_ACP_CLOSE,
        SESSION_CONTROL_STATE,
        SESSION_SET_PERMISSION_MODE,
        SESSION_SET_MODEL,
        SESSION_SET_EFFORT,
        SESSION_GOAL_GET,
        SESSION_GOAL_SET,
        SESSION_GOAL_CLEAR,
        SESSION_GOAL_CONTINUE,
        // Background
        BACKGROUND_CREATE,
        BACKGROUND_LIST,
        BACKGROUND_DETAIL,
        BACKGROUND_CANCEL,
        BACKGROUND_LOG,
        BACKGROUND_EVENTS,
        BACKGROUND_LIST_SUMMARY,
        BACKGROUND_SUBSCRIBE,
        BACKGROUND_CANCEL_ASYNC,
        // Workflows
        WORKFLOW_LIST,
        WORKFLOW_START,
        WORKFLOW_START_DYNAMIC,
        WORKFLOW_RESUME,
    ]
}

/// Experimental methods that additionally require the connection's dedicated
/// `persistent_goals` capability bit.
pub fn persistent_goal_client_request_methods() -> Vec<&'static str> {
    vec![
        SESSION_GOAL_GET,
        SESSION_GOAL_SET,
        SESSION_GOAL_CLEAR,
        SESSION_GOAL_CONTINUE,
    ]
}

/// Returns server-initiated notification methods (server -> client, no
/// response expected).
pub fn server_notification_methods() -> Vec<&'static str> {
    vec![NOTIFICATION_STREAM_EVENT]
}

/// Returns server-initiated request methods (server -> client, response
/// expected).
pub fn server_request_methods() -> Vec<&'static str> {
    vec![
        SERVER_REQUEST_PERMISSION,
        SERVER_REQUEST_MCP_TRUST,
        SERVER_REQUEST_ASK_USER,
    ]
}

/// Returns a `Vec` of every method constant defined in this module.
///
/// This combines [`client_request_methods`], [`server_notification_methods`],
/// and [`server_request_methods`].
pub fn all_methods() -> Vec<&'static str> {
    let mut all = client_request_methods();
    all.extend(server_notification_methods());
    all.extend(server_request_methods());
    all
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_methods_is_nonempty() {
        let methods = all_methods();
        assert!(!methods.is_empty());
    }

    #[test]
    fn all_methods_contains_expected_entries() {
        let methods = all_methods();
        assert!(methods.contains(&INITIALIZE));
        assert!(methods.contains(&SESSION_BOOTSTRAP));
        assert!(methods.contains(&SESSION_ACP_LOAD_PREFLIGHT));
        assert!(methods.contains(&SESSION_ACP_LOAD_SETUP));
        assert!(methods.contains(&SESSION_ACP_RESUME_SETUP));
        assert!(methods.contains(&SESSION_ACP_DELETE));
        assert!(methods.contains(&SESSION_ACP_CLOSE));
        assert!(methods.contains(&SESSION_CONTROL_STATE));
        assert!(methods.contains(&SESSION_SET_PERMISSION_MODE));
        assert!(methods.contains(&SESSION_SET_MODEL));
        assert!(methods.contains(&SESSION_SET_EFFORT));
        assert!(methods.contains(&TURN_SUBMIT));
        assert!(methods.contains(&PERMISSION_RESPOND));
        assert!(methods.contains(&SETTINGS_MODEL_NAME));
        assert!(methods.contains(&MCP_LIST_SERVERS));
        assert!(methods.contains(&TOOLS_LIST));
        assert!(methods.contains(&BACKGROUND_CREATE));
        assert!(methods.contains(&BACKGROUND_SUBSCRIBE));
        assert!(methods.contains(&WORKFLOW_LIST));
        assert!(methods.contains(&WORKFLOW_START));
        assert!(methods.contains(&WORKFLOW_START_DYNAMIC));
        assert!(methods.contains(&WORKFLOW_RESUME));
        assert!(methods.contains(&AUTH_OVERVIEW));
        assert!(methods.contains(&DIAGNOSTICS_STATUS));
        assert!(methods.contains(&NOTIFICATION_STREAM_EVENT));
        assert!(methods.contains(&SERVER_REQUEST_PERMISSION));
        assert!(methods.contains(&SERVER_REQUEST_MCP_TRUST));
        assert!(methods.contains(&SERVER_REQUEST_ASK_USER));
    }

    #[test]
    fn all_methods_no_duplicates() {
        let methods = all_methods();
        let mut seen = std::collections::HashSet::new();
        for m in &methods {
            assert!(seen.insert(m), "duplicate method: {m}");
        }
    }

    #[test]
    fn method_constants_are_valid_strings() {
        for m in all_methods() {
            assert!(!m.is_empty(), "method constant must not be empty");
            assert!(!m.starts_with('/'), "method must not start with slash: {m}");
            // Each method should be lowercase with slashes and underscores only
            for ch in m.chars() {
                assert!(
                    ch.is_ascii_lowercase() || ch == '/' || ch == '_',
                    "unexpected character '{ch}' in method: {m}"
                );
            }
        }
    }

    #[test]
    fn method_count_matches_expected() {
        // Current count: 1 lifecycle + 23 session + 4 turn + 10 permission +
        // 25 settings + 5 context/usage + 17 mcp + 9 tools + 9 background +
        // 4 workflows + 3 auth + 9 diagnostics + 1 notification +
        // 3 server requests = 123
        assert_eq!(all_methods().len(), 123);
    }

    #[test]
    fn all_methods_is_union_of_categories() {
        let mut combined = client_request_methods();
        combined.extend(server_notification_methods());
        combined.extend(server_request_methods());
        assert_eq!(all_methods(), combined);
    }

    #[test]
    fn client_request_methods_is_union_of_stable_and_experimental() {
        let mut combined = stable_client_request_methods();
        combined.extend(experimental_client_request_methods());
        assert_eq!(client_request_methods(), combined);
    }

    #[test]
    fn stable_and_experimental_do_not_overlap() {
        let stable: std::collections::HashSet<_> =
            stable_client_request_methods().into_iter().collect();
        for m in experimental_client_request_methods() {
            assert!(
                !stable.contains(m),
                "experimental method {m} must not appear in stable methods"
            );
        }
    }

    #[test]
    fn stable_methods_no_duplicates() {
        let methods = stable_client_request_methods();
        let mut seen = std::collections::HashSet::new();
        for m in &methods {
            assert!(seen.insert(m), "duplicate stable method: {m}");
        }
    }

    #[test]
    fn experimental_methods_no_duplicates() {
        let methods = experimental_client_request_methods();
        let mut seen = std::collections::HashSet::new();
        for m in &methods {
            assert!(seen.insert(m), "duplicate experimental method: {m}");
        }
    }

    #[test]
    fn client_request_methods_excludes_server_only() {
        let client = client_request_methods();
        assert!(
            !client.contains(&NOTIFICATION_STREAM_EVENT),
            "client requests must not contain server notification methods"
        );
        assert!(
            !client.contains(&SERVER_REQUEST_PERMISSION),
            "client requests must not contain server request methods"
        );
        assert!(
            !client.contains(&SERVER_REQUEST_MCP_TRUST),
            "client requests must not contain server request methods"
        );
        assert!(
            !client.contains(&SERVER_REQUEST_ASK_USER),
            "client requests must not contain server request methods"
        );
    }

    #[test]
    fn server_notification_methods_contents() {
        let notifs = server_notification_methods();
        assert_eq!(notifs, vec![NOTIFICATION_STREAM_EVENT]);
    }

    #[test]
    fn server_request_methods_contents() {
        let reqs = server_request_methods();
        assert_eq!(
            reqs,
            vec![
                SERVER_REQUEST_PERMISSION,
                SERVER_REQUEST_MCP_TRUST,
                SERVER_REQUEST_ASK_USER,
            ]
        );
    }

    #[test]
    fn server_methods_do_not_overlap_with_client_methods() {
        let client: std::collections::HashSet<_> = client_request_methods().into_iter().collect();
        for m in server_notification_methods() {
            assert!(
                !client.contains(m),
                "server notification {m} must not appear in client requests"
            );
        }
        for m in server_request_methods() {
            assert!(
                !client.contains(m),
                "server request {m} must not appear in client requests"
            );
        }
    }

    #[test]
    fn client_request_methods_count() {
        // Stable and experimental client methods are counted independently
        // below; this assertion locks their union.
        assert_eq!(client_request_methods().len(), 119);
    }

    #[test]
    fn stable_client_request_methods_count() {
        // Includes the stable thinking-budget, MCP-status, and read-state
        // controls added for SDK/headless convergence.
        assert_eq!(stable_client_request_methods().len(), 93);
    }

    #[test]
    fn experimental_client_request_methods_count() {
        // 13 session + 9 background (including async cancellation) + 4 workflows.
        assert_eq!(experimental_client_request_methods().len(), 26);
    }

    #[test]
    fn persistent_goal_method_family_is_pinned() {
        assert_eq!(
            persistent_goal_client_request_methods(),
            vec![
                SESSION_GOAL_GET,
                SESSION_GOAL_SET,
                SESSION_GOAL_CLEAR,
                SESSION_GOAL_CONTINUE,
            ]
        );
    }
}
