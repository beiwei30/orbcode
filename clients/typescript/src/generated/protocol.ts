// Auto-generated from Rust protocol DTOs — do not edit manually.
// Regenerate with: UPDATE_SCHEMA=1 cargo test -p orbcode-app-server-protocol

/** Messages sent from a client to the server. */
export type ClientMessage = ClientRequestEnvelope & {
  type: "request";
} | ServerRequestResponse & {
  type: "response";
};

/** A request envelope sent by the client. */
export interface ClientRequestEnvelope {
  id: string;
  method: string;
  params?: unknown;
}

/** Messages sent from the server to a client. */
export type ServerMessage = ServerResponseEnvelope & {
  type: "response";
} | ServerNotificationEnvelope & {
  type: "notification";
} | ServerRequestEnvelope & {
  type: "request";
};

/** Response envelope returned by the server for a client request. */
export interface ServerResponseEnvelope {
  id: string;
  result: ResponseResult;
}

/** Outcome of a request -- either success or an error. */
export type ResponseResult = {
  data?: unknown;
  status: "success";
} | ProtocolError & {
  status: "error";
};

/** Unsolicited server notification (stream events, progress, etc.). */
export interface ServerNotificationEnvelope {
  method: string;
  params: unknown;
}

/** A request initiated by the server that expects a client response. */
export interface ServerRequestEnvelope {
  id: string;
  method: string;
  params: unknown;
}

/** Parameters sent by the client in the `initialize` request. */
export interface InitializeParams {
  protocol_version: string;
  client_info: ClientInfo;
  capabilities?: ClientCapabilities;
}

/** Result returned by the server for the `initialize` request. */
export interface InitializeResult {
  protocol_version: string;
  server_info: ServerInfo;
  capabilities: ServerCapabilities;
}

/** Capabilities declared by the client during the handshake. */
export interface ClientCapabilities {
  streaming?: boolean;
  experimental_methods?: boolean;
}

/** Capabilities advertised by the server. */
export interface ServerCapabilities {
  streaming: boolean;
  stable_methods: string[];
  experimental_methods: string[];
  server_notification_methods: string[];
  server_request_methods: string[];
}

export type BootstrapParams = {
  session_id?: string | null;
  cwd?: string | null;
  additional_directories?: string[];
  session_mcp_servers?: McpServerConfig[];
  read_only?: boolean;
};

/** Structured error returned inside [`ResponseResult::Error`](crate::ResponseResult). */
export interface ProtocolError {
  code: ErrorCode;
  message: string;
  data?: unknown;
}

/** Error codes for the app-server protocol. */
export type ErrorCode = "parse_error" | "invalid_request" | "method_not_found" | "invalid_params" | "internal_error" | "session_not_found" | "active_turn" | "no_active_turn" | "permission_denied" | "provider_failed" | "config_error" | "tool_error" | "mcp_error" | "overloaded";

/** Client's decision in response to a [`PermissionRequest`].

This is a wire-serializable enum mirroring `core`'s `PermissionDecision`,
kept here so the protocol crate stays free of core dependencies. */
export type PermissionDecisionWire = {
  decision: "approve";
} | {
  decision: "deny";
} | {
  rules?: string[];
  decision: "approve_always";
};

/** Parameters for the [`method::PERMISSION_RESPOND`](crate::method::PERMISSION_RESPOND)
method sent by the client in response to a server-initiated permission
request. */
export interface PermissionResponseParams {
  request_id: string;
  decision: PermissionDecisionWire;
}

/** Client's decision in response to a [`McpTrustApprovalRequest`]. */
export type McpTrustDecisionWire = "trust" | "deny";

/** Parameters for the [`method::SERVER_REQUEST_MCP_TRUST`](crate::method::SERVER_REQUEST_MCP_TRUST)
response sent by the client. */
export interface McpTrustResponseParams {
  request_id: string;
  decision: McpTrustDecisionWire;
}

/** Server-initiated request asking the connected client to prompt the user
with a question and relay the answer back.

Sent via [`method::SERVER_REQUEST_ASK_USER`](crate::method::SERVER_REQUEST_ASK_USER).

The full tool-level integration is wired: the `AskUserQuestion` tool
pauses execution via `ToolContext::ask_user_tx`, the event pump in
`MessageProcessor::pump_events` sends this as a server-request, and the
client's response is routed back to the tool via
`AppServer::resolve_ask_user_question`. Cancellation (disconnect,
timeout, turn cancel) resolves with `None`. */
export interface AskUserQuestionRequest {
  session_id?: string;
  request_id: string;
  question: string;
  options?: string[];
}

/** Client's response to an [`AskUserQuestionRequest`]. */
export type AskUserQuestionResponse = {
  request_id: string;
  answer?: string | null;
};

/** Identifies the connecting client. */
export interface ClientInfo {
  name: string;
  version: string;
}

export type McpAuth = {
  kind: "none";
} | {
  env_var: string;
  kind: "bearer_env";
} | {
  name: string;
  value: string;
  kind: "header";
};

export interface McpPluginSource {
  plugin_id: string;
  plugin_name: string;
  server_name: string;
  source: string;
}

export type McpServerConfig = {
  id: string;
  transport: McpTransport;
  endpoint: string;
  args?: string[];
  env?: Record<string, unknown>;
  cwd?: string | null;
  headers?: Record<string, unknown>;
  enabled: boolean;
  status?: McpServerStatus;
  error?: string | null;
  summary: string;
  auth: McpAuth;
  trust?: McpServerTrust;
  transport_type_hint?: string | null;
  source?: McpServerSource | null;
};

export type McpServerSource = McpPluginSource & {
  kind: "plugin";
};

export type McpServerStatus = "disabled" | "starting" | "ready" | "failed" | "unauthorized" | "restarting" | "stopped";

export type McpServerTrust = "unknown" | "trusted" | "denied";

export type McpTransport = "stdio" | "http" | "https" | "streamable_http" | "web_socket";

/** Identifies the server. */
export interface ServerInfo {
  name: string;
  version: string;
}

/** Client's response to a server-initiated request. */
export interface ServerRequestResponse {
  id: string;
  result: ResponseResult;
}

// Method constants
export const STABLE_METHODS = [
  "initialize",
  "session/bootstrap",
  "session/list",
  "session/rename",
  "session/fork",
  "session/clear",
  "session/rewind",
  "session/record_message",
  "session/compact",
  "session/compact_decision",
  "session/find_by_title",
  "turn/submit",
  "turn/steer",
  "turn/cancel",
  "turn/interrupt",
  "permission/respond",
  "permission/overview",
  "permission/mode",
  "permission/set_mode",
  "permission/add_rule",
  "permission/remove_rule",
  "permission/add_session_rule",
  "permission/remove_session_rule",
  "permission/add_directory",
  "permission/validate_directory",
  "settings/model_name",
  "settings/model_options",
  "settings/set_model",
  "settings/providers",
  "settings/theme",
  "settings/set_theme",
  "settings/effort",
  "settings/set_effort",
  "settings/output_style",
  "settings/set_output_style",
  "settings/sandbox",
  "settings/update_sandbox",
  "settings/keybindings",
  "settings/load_keybindings",
  "settings/editor_mode",
  "settings/set_editor_mode",
  "settings/output_style_options",
  "settings/active_output_style",
  "settings/is_locked",
  "settings/set_auto_memory",
  "settings/ensure_memory_file",
  "settings/add_sandbox_excluded",
  "settings/allow_all",
  "settings/set_allow_all",
  "context/preview",
  "context/overview",
  "usage/overview",
  "usage/cost",
  "usage/stats",
  "mcp/list_servers",
  "mcp/server_trust",
  "mcp/set_trust",
  "mcp/list_tools",
  "mcp/list_resources",
  "mcp/read_resource",
  "mcp/list_prompts",
  "mcp/get_prompt",
  "mcp/invoke_tool",
  "mcp/diagnose",
  "mcp/upsert_server",
  "mcp/remove_server",
  "mcp/capabilities",
  "mcp/slash_suggestions",
  "mcp/oauth_overview",
  "mcp/logout_oauth_token",
  "tools/list",
  "tools/invoke",
  "tools/skills",
  "tools/agents",
  "tools/plan",
  "tools/task_list",
  "tools/enter_plan",
  "tools/agents_with_warnings",
  "auth/overview",
  "auth/login",
  "auth/logout",
  "diagnostics/status",
  "diagnostics/memory",
  "diagnostics/doctor",
  "diagnostics/hooks",
  "diagnostics/diff",
  "diagnostics/advanced",
  "diagnostics/cleanup_child_sessions",
  "diagnostics/last_request",
  "diagnostics/pre_user_instructions",
] as const;

export const EXPERIMENTAL_METHODS = [
  "session/acp_load_preflight",
  "session/acp_load_setup",
  "session/acp_resume_setup",
  "session/acp_delete",
  "background/create",
  "background/list",
  "background/detail",
  "background/cancel",
  "background/log",
  "background/events",
  "background/list_summary",
  "background/subscribe",
  "workflow/list",
  "workflow/start",
  "workflow/start_dynamic",
  "workflow/resume",
] as const;

export const SERVER_NOTIFICATION_METHODS = [
  "stream/event",
] as const;

export const SERVER_REQUEST_METHODS = [
  "permission/request",
  "mcp_trust/request",
  "ask_user/request",
] as const;
