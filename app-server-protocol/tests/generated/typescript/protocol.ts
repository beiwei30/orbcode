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
  persistent_goals?: boolean;
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

/** Client-owned appearance and editing preferences. Bootstrap uses the legacy
PascalCase wire spelling through field adapters while typed settings
methods use each enum's canonical settings spelling. */
export type BootstrapState = {
  session: SessionRecord;
  bootstrap_event: StreamEvent;
  prompt_history: string[];
  available_tool_count: number;
  configured_mcp_server_count: number;
  enabled_mcp_capability_count: number;
  home_dir: string;
  cwd: string;
  model_display_name: string;
  context_window_options: ContextWindowOptions;
  max_output_token_options: MaxOutputTokenOptions;
  token_warning_options: TokenWarningOptions;
  theme: string;
  editor_mode: string;
  default_provider: string;
  fallback_provider?: string | null;
  max_retries: number;
  permissions: PermissionContext;
  mcp_slash_suggestions: McpSlashSuggestionCatalog;
  statusline_command?: string | null;
  statusline_refresh_interval_secs: number;
};

export type SessionListResult = SessionSummary[];

export type SessionForkResult = {
  session_id: string;
  title?: string | null;
  custom_title?: string | null;
  created_at: string;
  updated_at: string;
  cwd?: string | null;
  git_branch?: string | null;
  model?: string | null;
  provider?: ProviderId | null;
  additional_directories?: string[];
  session_allowed_tools?: string[];
  session_disallowed_tools?: string[];
  session_effort?: EffortLevel | null;
  goal?: SessionGoal | null;
  messages: TranscriptMessage[];
};

/** Canonical session-scoped controls consumed by headless clients and edge
adapters. Values are resolved by app-server; adapters only project them into
their protocol-specific shapes. */
export type SessionControlState = {
  session_id: string;
  permission_mode: PermissionMode;
  model_selection: EffectiveModelSelection;
  model_options: SessionModelOption[];
  effort_level?: string | null;
  effort_options: string[];
};

export interface SetSessionPermissionModeParams {
  session_id: string;
  mode: PermissionMode;
}

export type SetSessionModelParams = {
  session_id: string;
  model?: string | null;
  inherit?: boolean;
};

export type SetSessionEffortParams = {
  session_id: string;
  effort?: string | null;
};

export interface AcpDeleteSessionParams {
  session_id: string;
  cwd: string;
}

export type StreamEvent = {
  summary: SessionSummary;
  event: "session_started";
} | {
  summary: SessionSummary;
  event: "session_loaded";
} | {
  session_id: string;
  provider: ProviderId;
  fallback_provider?: ProviderId | null;
  context: TurnContext;
  event: "request_started";
} | {
  message: TranscriptMessage;
  event: "user_message";
} | {
  session_id: string;
  provider: ProviderId;
  fallback_from?: ProviderId | null;
  event: "assistant_message_started";
} | {
  session_id: string;
  provider: ProviderId;
  event: "thinking_started";
} | {
  session_id: string;
  delta: string;
  event: "thinking_delta";
} | {
  session_id: string;
  provider: ProviderId;
  event: "thinking_completed";
} | {
  session_id: string;
  delta: string;
  event: "assistant_delta";
} | {
  request: PermissionRequest;
  event: "permission_requested";
} | {
  session_id: string;
  request_id: string;
  kind: PermissionResolutionKind;
  event: "permission_resolved";
} | {
  request: McpTrustApprovalRequest;
  event: "mcp_trust_approval_requested";
} | {
  session_id: string;
  request_id: string;
  kind: McpTrustResolutionKind;
  event: "mcp_trust_approval_resolved";
} | {
  session_id: string;
  tool_use_id: string;
  tool_name: string;
  tool_input?: string;
  event: "tool_use_started";
} | {
  session_id: string;
  tool_use_id: string;
  tool_name: string;
  progress: unknown;
  event: "tool_progress";
} | {
  session_id: string;
  hook_event_name: string;
  progress: unknown;
  event: "hook_progress";
} | {
  session_id: string;
  hook_event_name: string;
  message: string;
  is_error: boolean;
  event: "hook_notice";
} | {
  session_id: string;
  tool_use_id: string;
  tool_name: string;
  kind: ToolUseCompletionKind;
  event: "tool_use_completed";
} | {
  message: TranscriptMessage;
  provider: ProviderId;
  fallback_from?: ProviderId | null;
  usage: TokenUsage;
  event: "assistant_message_completed";
} | {
  session_id: string;
  provider: ProviderId;
  fallback_provider: ProviderId;
  reason: string;
  event: "assistant_message_discarded";
} | {
  session_id: string;
  duration_ms: number;
  summary?: string | null;
  original_message_count: number;
  compacted_message_count: number;
  provider_generated: boolean;
  fallback_reason?: string | null;
  event: "context_compacted";
} | {
  session_id: string;
  kind: TurnCancellationKind;
  partial?: TranscriptMessage | null;
  usage?: TokenUsage | null;
  event: "turn_cancelled";
} | {
  session_id: string;
  provider: ProviderId;
  fallback_from?: ProviderId | null;
  usage: TokenUsage;
  event: "turn_finished";
} | {
  session_id: string;
  outcome: BudgetOutcome;
  blocked: boolean;
  total_usd: number;
  max_budget_usd: number;
  pricing_known: boolean;
  event: "budget";
} | {
  session_id?: string;
  turn_id?: string | null;
  tool_use_id?: string;
  request_id: string;
  deadline?: string | null;
  questions?: AskUserQuestionSpec[];
  question?: string;
  options?: string[];
  event: "ask_user_question_requested";
} | {
  request_id: string;
  outcome?: AskUserResponseOutcome | null;
  answer?: string | null;
  event: "ask_user_question_resolved";
} | {
  session_id: string;
  task_id: string;
  status: string;
  command?: string | null;
  exit_code?: number | null;
  signal?: number | null;
  event: "local_task_progress";
} | {
  session_id: string;
  task: BackgroundTaskView;
  event: "background_task_updated";
} | {
  session_id?: string | null;
  provider?: ProviderId | null;
  category?: StreamErrorCategory | null;
  message: string;
  suggestion?: string | null;
  event: "error";
};

/** Payload for [`method::NOTIFICATION_STREAM_EVENT`](crate::method::NOTIFICATION_STREAM_EVENT)
notifications. Wraps a single [`StreamEvent`] from the protocol crate
together with the `subscription_id` that identifies the turn subscription
that produced it. */
export interface StreamEventNotification {
  subscription_id: string;
  event: StreamEvent;
}

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

export type EmptyParams = Record<string, unknown>;

/** Successful method result whose response envelope intentionally has no data payload.

Transports normalize an omitted `data` field to JSON `null`, which is the
serde representation of this unit struct. */
export type NoData = null;

export interface SessionIdParams {
  session_id: string;
}

/** Canonical persistent-goal state shared by transcripts and every client. */
export type SessionGoal = {
  goal_id: string;
  revision: number;
  session_id: string;
  objective: string;
  status: SessionGoalStatus;
  token_budget?: number | null;
  tokens_used?: number;
  elapsed_seconds?: number;
  created_at: string;
  updated_at: string;
  stop_reason?: string | null;
  last_goal_turn_id?: string | null;
};

/** Lifecycle state for the single persistent goal owned by a session. */
export type SessionGoalStatus = "active" | "paused" | "blocked" | "usage_limited" | "budget_limited" | "complete";

export type SessionGoalGetResult = {
  goal?: SessionGoal | null;
};

/** Create or mutate the current session goal.

Existing-goal mutations require `expected_revision`. `replace` is the
explicit user-authority operation for replacing a terminal goal; model
tools never set it. */
export type SessionGoalSetParams = {
  session_id: string;
  expected_revision?: number | null;
  replace?: boolean;
  objective?: string | null;
  status?: SessionGoalStatus | null;
  token_budget?: number | null;
};

export interface SessionGoalSetResult {
  goal: SessionGoal;
}

export interface SessionGoalClearResult {
  cleared: boolean;
}

export interface SessionGoalContinueParams {
  session_id: string;
  goal_id: string;
  expected_revision: number;
}

export type SessionGoalNotStartedReason = "missing" | "stale_revision" | "inactive" | "usage_limited" | "budget_limited" | "pending_user_input" | "active_turn" | "client_not_capable";

export type SessionGoalContinueResult = {
  subscription_id: string;
  turn_id: string;
  goal: SessionGoal;
  outcome: "started";
} | {
  reason: SessionGoalNotStartedReason;
  goal?: SessionGoal | null;
  outcome: "not_started";
};

export type SessionModelOption = {
  value?: string | null;
  label: string;
  description: string;
  current: boolean;
};

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
  mode: PermissionMode;
}

export interface PermissionSetModeParams {
  mode: PermissionMode;
}

export interface PermissionRuleParams {
  kind: PermissionRuleKind;
  rule: string;
}

export interface SessionPermissionRuleParams {
  session_id: string;
  kind: PermissionRuleKind;
  rule: string;
}

export interface PermissionRuleUpdateResult {
  path: string;
  rule: string;
  changed: boolean;
  kind: PermissionRuleKind;
  target: PermissionRuleTargetScope;
  source: SettingSource;
  effective_rules: EffectivePermissionRules;
}

export interface PermissionOverview {
  permissions: PermissionContext;
  allow_all: boolean;
  effective_rules?: EffectivePermissionRules;
  settings_allowed_rules: string[];
  settings_denied_rules: string[];
  startup_allowed_rules: string[];
  startup_denied_rules: string[];
  edited_allowed_rules: string[];
  edited_denied_rules: string[];
  runtime_allowed_rules: string[];
  runtime_denied_rules: string[];
  configured_additional_directories: string[];
  session_additional_directories: string[];
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
  model_selection: EffectiveModelSelection;
}

/** Set or clear the per-session thinking-token override. */
export type SetThinkingBudgetParams = {
  session_id: string;
  max_thinking_tokens?: number | null;
};

export type ThinkingBudgetResult = {
  session_id: string;
  max_thinking_tokens?: number | null;
};

export type ProvidersResult = {
  default_provider: string;
  fallback_provider?: string | null;
  max_retries: number;
  resolutions: ProviderResolutionOverview[];
};

export interface ThemeParams {
  theme: ThemeSetting;
}

export interface ThemeResult {
  theme: ThemeSetting;
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
  mode: EditorModeSetting;
}

export interface EditorModeResult {
  editor_mode: EditorModeSetting;
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

/** Seed one validated file identity into the shared stale-write guard. */
export interface SeedReadStateParams {
  session_id: string;
  path: string;
  mtime: number;
}

export interface SeedReadStateResult {
  session_id: string;
  path: string;
  mtime: number;
  seeded: boolean;
}

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

/** Cancel one background task owned by a session. */
export interface CancelAsyncTaskParams {
  session_id: string;
  task_id: string;
}

export interface CancelAsyncTaskResult {
  session_id: string;
  task_id: string;
  outcome: AsyncCancellationResultKind;
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

/** Secret-free status projection for a configured MCP server.

Mutation-only configuration (endpoint, arguments, environment, headers and
auth values) is deliberately absent from this read DTO. */
export type McpServerStatusOverview = {
  id: string;
  transport: string;
  enabled: boolean;
  status: string;
  trust: string;
  summary: string;
  auth_mode: string;
  error?: string | null;
};

export type McpStatusResult = McpServerStatusOverview[];

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

export type AdditionalDirectoryInfo = {
  path: string;
  has_claude_md?: boolean;
  git_branch?: string | null;
  repo_root?: string | null;
};

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

export type AsyncCancellationResultKind = "signalled" | "already_terminal" | "not_found";

export type AuthMethod = "api_key" | "o_auth_device" | "chatgpt";

export interface AuthStatusEntry {
  provider: string;
  method: AuthMethod;
  source_summary: string;
  persisted: boolean;
  usable: boolean;
  active: boolean;
}

export type BackgroundTaskProgressEvent = {
  timestamp: string;
  event: string;
  step_key?: string | null;
  kind?: string | null;
  message?: string | null;
  output?: string | null;
  child_session_id?: string | null;
};

export type BackgroundTaskView = {
  task_id: string;
  session_id: string;
  kind: BackgroundTaskViewKind;
  status: BackgroundTaskViewStatus;
  description: string;
  cwd: string;
  created_at: string;
  updated_at: string;
  started_at?: string | null;
  finished_at?: string | null;
  pid?: number | null;
  exit_code?: number | null;
  signal?: number | null;
  error?: string | null;
  model?: string | null;
  provider?: ProviderId | null;
  permission_mode?: string | null;
  agent_type?: string | null;
  child_session_id?: string | null;
  cancellation_reason?: string | null;
  label?: string | null;
  log_tail?: unknown | null;
  progress_events?: unknown | null;
  workflow_steps?: unknown | null;
};

export type BackgroundTaskViewKind = "BackgroundJob" | "LocalAgent" | "LocalShell" | "Workflow";

export type BackgroundTaskViewStatus = "Queued" | "PermissionPending" | "Running" | "Interrupting" | "Completed" | "Failed" | "Cancelled" | "Orphaned" | "Unknown";

/** Outcome of a pre-request `maxBudgetUsd` check. `Exceeded` means the
accumulated cost has reached or passed the cap; `UnknownPricing` means some
accumulated usage came from an unpriced model so the running total
understates real spend. */
export type BudgetOutcome = "exceeded" | "unknown_pricing";

export interface CacheCreationUsage {
  ephemeral_1h_input_tokens?: number;
  ephemeral_5m_input_tokens?: number;
}

/** Identifies the connecting client. */
export interface ClientInfo {
  name: string;
  version: string;
}

export type ContextWindowOptions = {
  disable_1m_context: boolean;
  max_context_tokens_override?: number | null;
  auto_compact_window_override?: number | null;
};

export type DiscoveredHook = {
  event: string;
  provenance: HookProvenance;
  matcher?: string | null;
  command: string;
  trusted: boolean;
  validation: HookValidationStatus;
};

export type EditorModeSetting = "normal" | "vim";

export type EffectiveModelSelection = {
  persisted: PersistedModelSetting;
  runtime_override: RuntimeModelOverride;
  requested_model?: string | null;
  source: ModelSelectionSource;
  provider: string;
  resolution: ProviderModelSelection;
};

/** Source-preserving permission projection. Matching remains owned by config's
structured parser; this DTO only describes effective rule provenance. */
export interface EffectivePermissionRules {
  managed: PermissionRuleGroup;
  settings: SourcedPermissionRuleGroup[];
  startup: PermissionRuleGroup;
  session: PermissionRuleGroup;
  runtime_added: PermissionRuleGroup;
  remembered: PermissionRuleGroup;
  settings_locked: boolean;
  allow_managed_permission_rules_only: boolean;
  precedence: PermissionRuleEffect[];
}

export type EffortLevel = "low" | "medium" | "high" | "max";

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

export type MaxOutputTokenOptions = {
  max_output_tokens_override?: number | null;
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

export interface McpResourceSlashSuggestion {
  server_id: string;
  uri: string;
  name: string;
  description: string;
}

export type McpResourceSummary = {
  uri: string;
  name: string;
  mime_type: string;
  description: string;
  annotations?: McpAnnotations | null;
};

export interface McpServerSlashSuggestion {
  id: string;
  summary: string;
}

export type McpServerSource = McpPluginSource & {
  kind: "plugin";
};

export type McpServerStatus = "disabled" | "starting" | "ready" | "failed" | "unauthorized" | "restarting" | "stopped";

export type McpServerTrust = "unknown" | "trusted" | "denied";

export interface McpSlashSuggestionCatalog {
  servers: McpServerSlashSuggestion[];
  tools: McpToolSlashSuggestion[];
  resources: McpResourceSlashSuggestion[];
}

export interface McpToolSlashSuggestion {
  server_id: string;
  name: string;
  provider_name: string;
  description: string;
}

export interface McpToolSpec {
  name: string;
  summary: string;
}

/** Transport selected for an MCP server.

This is a wire DTO. Runtime transport behavior remains owned by
`orbcode-mcp` and is converted at the app-server boundary. */
export type McpTransport = "stdio" | "http" | "https" | "streamable_http" | "web_socket";

export interface McpTrustApprovalRequest {
  request_id: string;
  session_id: string;
  server_id: string;
  tool_name: string;
}

export type McpTrustResolutionKind = "trusted" | "denied" | "interrupted";

export type MemorySource = {
  kind: MemorySourceKind;
  label: string;
  path?: string | null;
  status: MemorySourceStatus;
  writable: boolean;
  trust_boundary?: string | null;
  scope?: string | null;
  skipped_reason?: string | null;
  content?: string | null;
};

export type MemorySourceKind = "managed" | "user" | "project" | "local" | "team" | "agent" | "skill";

export type MemorySourceStatus = "loaded" | "empty" | "missing" | "skipped";

export type MessageRole = "system" | "user" | "assistant";

export type ModelOptionOverview = {
  value?: string | null;
  label: string;
  description: string;
  current: boolean;
};

export type ModelSelectionSource = "runtime" | "environment" | "persisted" | "provider_default";

export interface OutputStyleOptionOverview {
  value: string;
  label: string;
  description: string;
  current: boolean;
}

/** Protocol-owned permission mode used by session-scoped client controls. */
export type PermissionMode = "default" | "acceptEdits" | "bypassPermissions" | "dontAsk" | "plan" | "auto";

export interface PermissionRequest {
  request_id: string;
  session_id: string;
  tool_use_id: string;
  tool_name: string;
  tool_input: string;
  requires_tools_permission: boolean;
  requires_network_permission: boolean;
}

export type PermissionResolutionKind = "approved" | "denied" | "interrupted";

export type PermissionRuleEffect = "deny" | "ask" | "allow";

export interface PermissionRuleGroup {
  allow: string[];
  deny: string[];
  ask: string[];
}

export type PermissionRuleKind = "allow" | "deny";

export type PermissionRuleOverview = {
  raw: string;
  tool_name: string;
  rule_content?: string | null;
};

/** Storage scope for a permission-rule mutation. The endpoint-specific
parameter DTOs expose this before app-server opens either storage path. */
export type PermissionRuleTargetScope = "settings" | "session";

export type PersistedModelSetting = {
  value?: string | null;
  source?: SettingSource | null;
  locked: boolean;
};

export type ProviderId = "anthropic" | "openai" | "gemini" | "grok";

export type ProviderModelSelection = {
  requested_setting?: string | null;
  family?: string | null;
  model: string;
  request_model: string;
  display_label: string;
  display_name: string;
  capabilities: string[];
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

export type RuntimeModelOverride = {
  kind: "inherit";
} | {
  kind: "default";
} | {
  kind: "model";
  model: string;
};

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

export interface ServerToolUseUsage {
  web_search_requests?: number;
  web_fetch_requests?: number;
}

export type SessionRecord = {
  session_id: string;
  title?: string | null;
  custom_title?: string | null;
  created_at: string;
  updated_at: string;
  cwd?: string | null;
  git_branch?: string | null;
  model?: string | null;
  provider?: ProviderId | null;
  additional_directories?: string[];
  session_allowed_tools?: string[];
  session_disallowed_tools?: string[];
  session_effort?: EffortLevel | null;
  goal?: SessionGoal | null;
  messages: TranscriptMessage[];
};

export type SessionStatus = {
  kind: "available";
} | {
  reason: string;
  kind: "corrupt";
};

export type SessionSummary = {
  session_id: string;
  title?: string | null;
  custom_title?: string | null;
  message_count: number;
  created_at: string;
  updated_at: string;
  cwd?: string | null;
  git_branch?: string | null;
  model?: string | null;
  provider?: ProviderId | null;
  transcript_path?: string | null;
  status?: SessionStatus;
  total_input_tokens?: number;
  total_output_tokens?: number;
  duration_secs?: number | null;
};

export type SettingSource = "defaults" | "user" | "project" | "local" | "managed" | "cli" | "environment" | "session";

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

export interface SourcedPermissionRuleGroup {
  source: SettingSource;
  active: boolean;
  mutable: boolean;
  rules: PermissionRuleGroup;
}

export type StreamErrorCategory = "auth" | "rate_limit" | "overload" | "network" | "invalid_request" | "prompt_too_long" | "max_output" | "server_error" | "account_suspended" | "unsupported_provider" | "interrupted" | "retry_exhausted" | "other";

export interface TaskOverview {
  id: string;
  subject: string;
  description: string;
  status: string;
}

export type ThemeSetting = "auto" | "dark" | "light" | "dark-daltonized" | "light-daltonized" | "dark-ansi" | "light-ansi";

export type TokenUsage = {
  input_tokens?: number;
  cache_creation_input_tokens?: number;
  cache_read_input_tokens?: number;
  output_tokens?: number;
  server_tool_use?: ServerToolUseUsage;
  service_tier?: string | null;
  cache_creation?: CacheCreationUsage;
  iterations?: UsageIteration[];
  speed?: string | null;
  total_tokens?: number;
};

export type TokenWarningOptions = {
  auto_compact_enabled: boolean;
  auto_compact_percent_override?: number | null;
  blocking_limit_override?: number | null;
};

export type ToolOverview = {
  name: string;
  summary: string;
  requires_tools_permission: boolean;
  requires_network_permission: boolean;
  provider_hidden: boolean;
  unavailable_reason?: string | null;
};

export type ToolUseCompletionKind = "success" | "execution_failed" | "permission_denied" | "interrupted" | "cancelled" | "unknown_tool";

export type TranscriptBlock = {
  text: string;
  kind: "text";
} | {
  text: string;
  signature?: string | null;
  kind: "thinking";
} | {
  id: string;
  name: string;
  input: string;
  kind: "tool_use";
} | {
  tool_use_id: string;
  content: string;
  is_error: boolean;
  metadata?: string | null;
  kind: "tool_result";
};

export type TranscriptMessage = {
  id: string;
  role: MessageRole;
  content: string;
  blocks?: TranscriptBlock[];
  stop_reason?: string | null;
  usage?: TokenUsage | null;
  created_at: string;
  is_synthetic?: boolean;
};

export type TurnCancellationKind = "before_response" | "assistant_streaming" | "tool_stage";

export type TurnContext = {
  cwd: string;
  additional_directories?: string[];
  additional_directory_details?: AdditionalDirectoryInfo[];
  repo_root?: string | null;
  cwd_relative_to_repo?: string | null;
  current_date: string;
  git_branch?: string | null;
  git_default_branch?: string | null;
  git_user?: string | null;
  git_status?: string | null;
  git_recent_commits?: string | null;
  git_remote?: string | null;
  git_worktree_state?: WorktreeState | null;
  trusted_project?: boolean | null;
  memory_sources?: MemorySource[];
  claude_md?: string | null;
};

export interface UsageIteration {
  input_tokens?: number;
  output_tokens?: number;
}

export type WorkflowStepView = {
  step_key: string;
  parent_key?: string | null;
  depth: number;
  kind: string;
  label: string;
  status: WorkflowStepViewStatus;
  started_at?: string | null;
  finished_at?: string | null;
  output?: string | null;
  error?: string | null;
  child_session_id?: string | null;
};

export type WorkflowStepViewStatus = "Pending" | "Running" | "Completed" | "Failed" | "Cancelled";

export type WorktreeState = "normal" | "detached" | "linked";

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
  "settings/set_thinking_budget",
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
  "mcp/status",
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
  "tools/seed_read_state",
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
  "session/control_state",
  "session/set_permission_mode",
  "session/set_model",
  "session/set_effort",
  "session/goal/get",
  "session/goal/set",
  "session/goal/clear",
  "session/goal/continue",
  "background/create",
  "background/list",
  "background/detail",
  "background/cancel",
  "background/log",
  "background/events",
  "background/list_summary",
  "background/subscribe",
  "background/cancel_async",
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
