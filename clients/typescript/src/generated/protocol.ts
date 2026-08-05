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
export type ClientCapabilities = {
  streaming?: boolean;
  experimental_methods?: boolean;
  interactive_questions?: InteractiveQuestionsCapability | null;
};

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
  session_mcp_servers?: McpServerInput[];
  read_only?: boolean;
};

/** Secret-bearing input accepted by MCP upsert and session bootstrap methods. */
export type McpServerInput = {
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

/** Redacted MCP configuration/status returned by read-only protocol methods.

Collection shapes and field names intentionally match protocol 1.0. Values
that may contain credentials are replaced at the app-server boundary. */
export type McpServerOverview = {
  id: string;
  transport: McpTransport;
  endpoint: string;
  args: string[];
  env: Record<string, unknown>;
  cwd?: string | null;
  headers: Record<string, unknown>;
  enabled: boolean;
  status: McpServerStatus;
  error?: string | null;
  summary: string;
  auth: McpAuthOverview;
  trust: McpServerTrust;
  transport_type_hint?: string | null;
  source?: McpServerSource | null;
};

export type McpListServersResult = McpServerOverview[];

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
with one or more questions and relay a typed outcome back.

Sent via [`method::SERVER_REQUEST_ASK_USER`](crate::method::SERVER_REQUEST_ASK_USER).

`question` and `options` are retained for protocol-1.0 clients. Canonical
clients should use `questions`; servers normalize either representation at
the boundary and validate the result before registering pending state. */
export type AskUserQuestionRequest = {
  session_id?: string;
  turn_id?: string | null;
  tool_use_id?: string;
  request_id: string;
  deadline?: string | null;
  validation_error?: AskUserValidationError | null;
  questions?: AskUserQuestionSpec[];
  question?: string;
  options?: string[];
};

/** Client's response to an [`AskUserQuestionRequest`]. */
export type AskUserQuestionResponse = {
  request_id: string;
  outcome?: AskUserResponseOutcome | null;
  answer?: string | null;
};

/** Canonical model-visible specification for one interactive question. */
export interface AskUserQuestionSpec {
  id: string;
  question: string;
  header: string;
  multi_select?: boolean;
  options?: AskUserOption[];
  allow_free_text?: boolean;
  allow_annotation?: boolean;
}

/** Canonical response lifecycle for an interactive question request. */
export type AskUserResponseOutcome = {
  answers: Record<string, unknown>;
  annotations?: Record<string, unknown>;
  outcome: "answered";
} | {
  outcome: "rejected";
} | {
  outcome: "clarify";
} | {
  outcome: "finish_plan_interview";
} | {
  reason: AskUserCancellationReason;
  outcome: "cancelled";
};

export interface InteractiveQuestionsCapability {
  single_select?: boolean;
  multi_select?: boolean;
  free_text?: boolean;
  previews?: boolean;
  annotations?: boolean;
  special_outcomes?: boolean;
}

export interface EmptyParams Record<string, unknown>

/** Successful method result whose response envelope intentionally has no data payload.

Transports normalize an omitted `data` field to JSON `null`, which is the
serde representation of this unit struct. */
export type NoData = null;

export interface SessionIdParams {
  session_id: string;
}

export interface SessionRenameParams {
  session_id: string;
  new_title: string;
}

export type SessionForkParams = {
  session_id: string;
  title?: string | null;
  note?: string | null;
};

export interface SessionRewindParams {
  session_id: string;
  keep_messages: number;
}

export interface SessionRecordMessageParams {
  session_id: string;
  message: string;
}

export interface SessionFindByTitleParams {
  title: string;
}

export type SessionFindByTitleResult = {
  session_id?: string | null;
};

export interface TurnSubmitParams {
  session_id: string;
  prompt: string;
}

export interface TurnSubmitResult {
  subscription_id: string;
}

export interface TurnCancelResult {
  cancelled: boolean;
}

export interface TurnInterruptResult {
  interrupted: boolean;
}

export interface SentResult {
  sent: boolean;
}

export interface PermissionModeResult {
  mode: string;
}

export interface PermissionSetModeParams {
  mode: string;
}

export interface PermissionRuleParams {
  kind: string;
  rule: string;
}

export interface SessionPermissionRuleParams {
  session_id: string;
  kind: string;
  rule: string;
}

export interface PermissionRuleUpdateResult {
  path: string;
  rule: string;
  changed: boolean;
}

export interface AddDirectoryParams {
  session_id: string;
  directory: string;
}

export interface ValidateDirectoryParams {
  path: string;
}

export interface ModelNameResult {
  model_name: string;
}

export type ModelOptionsResult = ModelOptionOverview[];

export type SetModelParams = {
  model?: string | null;
};

export interface SetModelResult {
  display_name: string;
}

export type ProvidersResult = {
  default_provider: string;
  fallback_provider?: string | null;
  max_retries: number;
  resolutions: ProviderResolutionOverview[];
};

export interface ThemeParams {
  theme: string;
}

export interface ThemeResult {
  theme: string;
}

export type EffortParams = {
  session_id: string;
  effort?: string | null;
};

export type EffortResult = {
  effort?: string | null;
};

export interface OutputStyleParams {
  style: string;
}

export interface OutputStyleResult {
  style: string;
}

export interface PathResult {
  path: string;
}

export interface KeybindingsFileResult {
  path: string;
  created: boolean;
}

export interface KeybindingsLoadResult {
  warnings: string[];
}

export interface EditorModeParams {
  mode: string;
}

export interface EditorModeResult {
  editor_mode: string;
}

export type OutputStyleOptionsResult = OutputStyleOptionOverview[];

export interface ActiveOutputStyleResult {
  name: string;
  matched: boolean;
}

export interface SettingKeyParams {
  key: string;
}

export interface SettingLockedResult {
  locked: boolean;
}

export interface EnabledParams {
  enabled: boolean;
}

export interface EnabledResult {
  enabled: boolean;
}

export interface StringPathParams {
  path: string;
}

export interface SandboxExcludedCommandParams {
  command: string;
}

export interface SandboxExcludedCommandResult {
  pattern: string;
  path: string;
}

export interface AllowAllParams {
  allow_all: boolean;
}

export interface AllowAllResult {
  allow_all: boolean;
}

export type ToolsListResult = ToolOverview[];

export interface ToolInvokeParams {
  name: string;
  input?: string;
}

export interface ToolInvokeResult {
  name: string;
  summary: string;
  output: string;
  metadata?: unknown;
  changed_paths: string[];
}

export type SkillDefinitionsParams = {
  session_id?: string | null;
};

export type SkillDefinitionsResult = SkillDefinition[];

export type AgentDefinitionsResult = AgentSummary[];

export interface AgentDefinitionsWithWarningsResult {
  definitions: AgentDefinition[];
  warnings: AgentLoadWarning[];
}

export interface TaskListParams {
  task_list_id: string;
}

export interface TaskListResult {
  task_list_id: string;
  directory: string;
  tasks: TaskOverview[];
}

export interface EnterPlanModeResult {
  name: string;
  summary: string;
  output: string;
  metadata?: unknown;
}

export type AuthLoginParams = {
  provider: string;
  method: AuthMethod;
  token?: string | null;
  env_var?: string | null;
};

export interface AuthLoginResult {
  provider: string;
  method: string;
  source_summary: string;
  persisted: boolean;
  usable: boolean;
  active: boolean;
}

export type AuthLogoutParams = {
  provider?: string | null;
};

export interface AuthLogoutResult {
  removed: number;
}

export type DiagnosticsCleanupChildSessionsParams = {
  dry_run?: boolean;
  stale_running_cutoff_ms?: number | null;
};

export interface ChildSessionOrphanCleanupResult {
  dry_run: boolean;
  scoped_cwds: string[];
  inspected_metadata: number;
  orphan_metadata: number;
  eligible_metadata: number;
  stale_running_metadata: number;
  skipped_running_metadata: number;
  removed_metadata: number;
  removed_transcripts: number;
  orphan_child_session_ids: string[];
}

export type AdvancedCapabilitiesResult = AdvancedCapabilityOverview[];

export type LastProviderRequestResult = ProviderRequestDebugOverview | null;

export interface PreUserInstructionsResult {
  preview: string;
}

export interface BackgroundCreateParams {
  session_id: string;
  prompt: string;
}

export interface BackgroundCreateResult {
  job_id: string;
  session_id: string;
  status: string;
  log_path: string;
}

export interface BackgroundJobParams {
  job_id: string;
}

export interface BackgroundCancelResult {
  job_id: string;
  status: string;
}

export interface BackgroundLogResult {
  log: string;
}

export interface BackgroundEventsResult {
  events: string;
}

export interface BackgroundSubscribeParams {
  task_id: string;
}

export interface BackgroundSubscribeResult {
  subscription_id: string;
}

export interface WorkflowStartParams {
  session_id: string;
  name: string;
  arguments?: string;
}

export interface WorkflowStartDynamicParams {
  session_id: string;
  name: string;
  spec: unknown;
  arguments?: string;
}

export interface WorkflowResumeParams {
  run_id: string;
}

export interface WorkflowTaskResult {
  task_id: string;
}

export interface McpServerIdParams {
  server_id: string;
}

export type McpSessionServerParams = {
  server_id: string;
  session_id?: string | null;
};

export interface McpServerTrustResult {
  trust: McpServerTrust;
}

export interface McpSetTrustParams {
  server_id: string;
  trust: McpServerTrust;
}

export type McpListToolsResult = McpToolSpec[];

export type McpListResourcesResult = McpResourceSummary[];

export type McpReadResourceParams = {
  server_id: string;
  uri: string;
  session_id?: string | null;
};

export type McpResourceContent = {
  uri: string;
  mime_type: string;
  contents: string;
  blob?: string | null;
  is_binary?: boolean;
  annotations?: McpAnnotations | null;
};

export type McpListPromptsResult = McpPrompt[];

export type McpGetPromptParams = {
  server_id: string;
  name: string;
  arguments?: unknown;
  session_id?: string | null;
};

/** Result of `prompts/get`: the rendered prompt messages. */
export interface McpPromptResult {
  description: string;
  messages: McpPromptMessage[];
}

export interface McpInvokeToolParams {
  server_id: string;
  tool_name: string;
  input?: string;
}

export interface McpToolResult {
  server_id: string;
  tool_name: string;
  output: string;
  is_error?: boolean;
}

export type McpDiagnoseResult = McpDiagnosticCheck[];

export interface McpRemoveServerResult {
  removed: boolean;
}

export type McpCapabilitiesResult = McpCapability[];

export type McpOAuthOverviewParams = {
  server_id?: string | null;
};

/** Secret-free OAuth status returned by `mcp/oauth_overview`. */
export interface McpOAuthOverview {
  store_path: string;
  entries: McpOAuthStatusEntry[];
}

export interface McpLogoutOAuthTokenResult {
  logged_out: boolean;
}

export interface AuthOverview {
  store_path: string;
  entries: AuthStatusEntry[];
}

/** Data-only view of the effective permission context.

Permission evaluation and structured bash/path matching remain internal to
core and config; this DTO exposes only the values clients render. */
export interface PermissionContext {
  cwd: string;
  allow_network: boolean;
  provider_allow_network: boolean;
  allow_tools: boolean;
  allowed_rules: PermissionRuleOverview[];
  denied_rules: PermissionRuleOverview[];
  ask_rules: PermissionRuleOverview[];
  additional_directories: string[];
}

export interface SandboxLocalSettings {
  enabled: boolean;
  auto_allow_bash_if_sandboxed: boolean;
  allow_unsandboxed_commands: boolean;
  excluded_commands: string[];
  filesystem: SandboxFilesystemLocalSettings;
  network: SandboxNetworkLocalSettings;
}

export type SandboxSettingsUpdate = {
  enabled?: boolean | null;
  auto_allow_bash_if_sandboxed?: boolean | null;
  allow_unsandboxed_commands?: boolean | null;
};

export interface HookDiscovery {
  hooks: DiscoveredHook[];
  warnings: HookDiscoveryWarning[];
}

export interface AdvancedCapabilityOverview {
  name: string;
  summary: string;
  status: string;
}

export type AgentDefinition = {
  agent_type: string;
  description: string;
  prompt: string;
  tools?: unknown | null;
  disallowed_tools?: unknown | null;
  model?: string | null;
  permission_mode?: AgentPermissionMode | null;
  skills: string[];
  mcp_server_names?: unknown | null;
  hooks: Record<string, unknown>;
  source: AgentSource;
  path?: string | null;
};

export type AgentHookCommand = {
  command: string;
  if?: string | null;
  timeout?: number | null;
  type: "command";
} | {
  type: "Unsupported";
};

export type AgentHookMatcher = {
  matcher?: string | null;
  hooks?: AgentHookCommand[];
};

export type AgentLoadWarning = {
  kind: AgentWarningKind;
  source: AgentSource;
  path?: string | null;
  agent_type?: string | null;
  message: string;
};

export type AgentPermissionMode = "default" | "acceptEdits" | "bypassPermissions" | "dontAsk" | "plan" | "auto";

export type AgentSource = "built_in" | "user_settings" | "project_settings" | {
  plugin: {
    plugin_id: string;
  };
};

export interface AgentSummary {
  agent_type: string;
  description: string;
}

export type AgentWarningKind = "missing_field" | "duplicate_name";

/** A typed answer to one canonical question. */
export type AskUserAnswerValue = {
  text: string;
  kind: "text";
} | {
  option_id: string;
  kind: "selected";
} | {
  option_ids: string[];
  kind: "selected_many";
};

/** Why a pending interactive question was cancelled. */
export type AskUserCancellationReason = "interrupt" | "disconnect" | "timeout" | "client_closed" | "delivery_failed" | "session_closed" | "shutdown";

/** One selectable answer presented for an interactive question. */
export type AskUserOption = {
  id: string;
  label: string;
  description?: string;
  preview?: string | null;
};

/** Stable machine-readable category for request or response validation errors. */
export type AskUserValidationCode = "question_count" | "empty_id" | "duplicate_id" | "empty_question" | "header_too_long" | "option_count" | "empty_label" | "duplicate_label" | "field_too_large" | "request_too_large" | "missing_answer" | "unknown_question" | "unknown_option" | "answer_kind" | "free_text_disabled" | "annotation_disabled" | "malformed_response" | "request_id_mismatch";

/** Validation failure safe to return to a client or model as structured data. */
export interface AskUserValidationError {
  code: AskUserValidationCode;
  message: string;
}

export type AuthMethod = "api_key" | "o_auth_device" | "chatgpt";

export interface AuthStatusEntry {
  provider: string;
  method: AuthMethod;
  source_summary: string;
  persisted: boolean;
  usable: boolean;
  active: boolean;
}

/** Identifies the connecting client. */
export interface ClientInfo {
  name: string;
  version: string;
}

export type DiscoveredHook = {
  event: string;
  provenance: HookProvenance;
  matcher?: string | null;
  command: string;
  trusted: boolean;
  validation: HookValidationStatus;
};

export interface HookDiscoveryWarning {
  provenance: HookProvenance;
  event: string;
  message: string;
}

export type HookLayer = "user" | "project" | "local" | "managed";

export type HookProvenance = "built_in" | {
  settings: HookLayer;
} | {
  skill: {
    name: string;
  };
} | {
  agent: {
    name: string;
  };
} | {
  plugin: {
    plugin_id: string;
  };
};

export type HookValidationStatus = "valid" | {
  invalid: string;
};

export type McpAnnotations = {
  audience?: string[];
  priority?: number | null;
};

/** Authentication input accepted by MCP mutation methods.

Header values are intentionally possible here because this is a write
contract. Read methods use [`McpAuthOverview`] instead. */
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

/** Safe authentication summary returned by read-only MCP methods.

The `value` field is retained for protocol-1.0 shape compatibility but the
app-server always fills it with a redaction marker. */
export type McpAuthOverview = {
  kind: "none";
} | {
  env_var: string;
  kind: "bearer_env";
} | {
  name: string;
  value: string;
  kind: "header";
};

export interface McpCapability {
  transport: McpTransport;
  enabled: boolean;
  note: string;
}

/** A protocol-owned MCP content block. */
export type McpContent = {
  kind: string;
  text?: string | null;
  binary?: string | null;
  is_binary?: boolean;
  mime_type?: string;
  annotations?: McpAnnotations | null;
};

export interface McpDiagnosticCheck {
  name: string;
  status: McpDiagnosticStatus;
  detail: string;
}

export type McpDiagnosticStatus = "pass" | "warn" | "fail";

export type McpOAuthStatusEntry = {
  server_id: string;
  source_summary: string;
  usable: boolean;
  expired: boolean;
  has_refresh_token: boolean;
  has_token_endpoint?: boolean;
  expires_at?: number | null;
  scopes?: string[];
  updated_at?: number | null;
};

export interface McpPluginSource {
  plugin_id: string;
  plugin_name: string;
  server_name: string;
  source: string;
}

export interface McpPrompt {
  name: string;
  description: string;
  arguments?: McpPromptArgument[];
  skill?: boolean;
}

export interface McpPromptArgument {
  name: string;
  description: string;
  required: boolean;
}

export interface McpPromptMessage {
  role: string;
  content: McpContent;
}

export type McpResourceSummary = {
  uri: string;
  name: string;
  mime_type: string;
  description: string;
  annotations?: McpAnnotations | null;
};

export type McpServerSource = McpPluginSource & {
  kind: "plugin";
};

export type McpServerStatus = "disabled" | "starting" | "ready" | "failed" | "unauthorized" | "restarting" | "stopped";

export type McpServerTrust = "unknown" | "trusted" | "denied";

export interface McpToolSpec {
  name: string;
  summary: string;
}

/** Transport selected for an MCP server.

This is a wire DTO. Runtime transport behavior remains owned by
`orbcode-mcp` and is converted at the app-server boundary. */
export type McpTransport = "stdio" | "http" | "https" | "streamable_http" | "web_socket";

export type ModelOptionOverview = {
  value?: string | null;
  label: string;
  description: string;
  current: boolean;
};

export interface OutputStyleOptionOverview {
  value: string;
  label: string;
  description: string;
  current: boolean;
}

export type PermissionRuleOverview = {
  raw: string;
  tool_name: string;
  rule_content?: string | null;
};

export interface ProviderRequestDebugOverview {
  provider: string;
  source: string;
  session_id: string;
  model: string;
  base_url: string;
  captured_at: string;
  recent_activity_json: string;
  previous_turn_json: string;
  body_json: string;
}

export interface ProviderResolutionOverview {
  provider: string;
  model: string;
  request_model: string;
  display_name: string;
  capabilities: string[];
}

export interface SandboxFilesystemLocalSettings {
  allow_write: string[];
  deny_write: string[];
  deny_read: string[];
  allow_read: string[];
}

export type SandboxNetworkLocalSettings = {
  allowed_domains: string[];
  allow_unix_sockets: string[];
  allow_all_unix_sockets?: boolean | null;
  allow_local_binding?: boolean | null;
  http_proxy_port?: number | null;
  socks_proxy_port?: number | null;
};

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

export type SkillDefinition = {
  name: string;
  description?: string | null;
  when_to_use?: string | null;
  allowed_tools: string[];
  model?: string | null;
  assets: string[];
  path: string;
  body: string;
  source: SkillSource;
};

export type SkillSource = "User" | "Project" | "Bundled" | {
  Plugin: {
    plugin_id: string;
  };
} | {
  Mcp: {
    server_id: string;
  };
};

export interface TaskOverview {
  id: string;
  subject: string;
  description: string;
  status: string;
}

export type ToolOverview = {
  name: string;
  summary: string;
  requires_tools_permission: boolean;
  requires_network_permission: boolean;
  provider_hidden: boolean;
  unavailable_reason?: string | null;
};

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
  "session/acp_close",
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
