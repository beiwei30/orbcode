/**
 * Transport-independent correlation for the generated app-server protocol.
 *
 * Transports own framing, authentication, and lifecycle only. This class owns
 * request IDs, response correlation, timeouts, notifications, and server
 * requests for stdio, WebSocket, and future host IPC transports alike.
 */

import type {
  AcpDeleteSessionParams,
  ActiveOutputStyleResult,
  AskUserQuestionRequest,
  AskUserQuestionResponse,
  BootstrapParams,
  BootstrapState,
  ClientMessage,
  EditorModeResult,
  EditorModeSetting,
  InitializeResult,
  InteractiveQuestionsCapability,
  McpTrustDecisionWire,
  OutputStyleOptionsResult,
  OutputStyleResult,
  PermissionDecisionWire,
  PermissionOverview,
  ResponseResult,
  SandboxLocalSettings,
  SandboxSettingsUpdate,
  ServerMessage,
  SessionControlState,
  SessionForkParams,
  SessionForkResult,
  SessionListResult,
  SessionRenameParams,
  SetSessionEffortParams,
  SetSessionModelParams,
  SetSessionPermissionModeParams,
  SettingLockedResult,
  ThemeResult,
  ThemeSetting,
  TurnCancelResult,
  TurnSubmitParams,
  TurnSubmitResult,
} from "./generated/protocol.js";

export type ServerResponseMessage = Extract<
  ServerMessage,
  { type: "response" }
>;
export type ServerRequestMessage = Extract<ServerMessage, { type: "request" }>;
export type ServerNotificationMessage = Extract<
  ServerMessage,
  { type: "notification" }
>;

export type ProtocolTransportEvent =
  | { kind: "message"; message: ServerMessage }
  | { kind: "closed"; reason: string };

export interface ProtocolTransport {
  send(message: ClientMessage): Promise<void>;
  subscribe(listener: (event: ProtocolTransportEvent) => void): () => void;
  close(): Promise<void>;
}

export interface ProtocolClientOptions {
  requestIdPrefix?: string;
  requestTimeoutMs?: number;
  clientName?: string;
  clientVersion?: string;
}

export type ServerRequestListener = (request: ServerRequestMessage) => void;
export type NotificationListener = (
  notification: ServerNotificationMessage,
) => void;
export type ProtocolCloseListener = (reason: string) => void;

interface PendingRequest {
  resolve: (message: ServerResponseMessage) => void;
  reject: (error: Error) => void;
  timer: ReturnType<typeof setTimeout>;
}

export class ProtocolClient {
  private readonly pending = new Map<string, PendingRequest>();
  private readonly serverRequests: ServerRequestMessage[] = [];
  private readonly notifications: ServerNotificationMessage[] = [];
  private readonly serverRequestListeners = new Set<ServerRequestListener>();
  private readonly notificationListeners = new Set<NotificationListener>();
  private readonly closeListeners = new Set<ProtocolCloseListener>();
  private readonly respondingServerRequests = new Set<string>();
  private readonly respondedServerRequests = new Set<string>();
  private readonly serverRequestDeliveries = new Map<string, number>();
  private readonly unsubscribe: () => void;
  private readonly requestIdPrefix: string;
  private readonly requestTimeoutMs: number;
  private readonly clientName: string;
  private readonly clientVersion: string;
  private nextId = 0;
  private closed = false;
  private cleanedUp = false;

  constructor(
    private readonly transport: ProtocolTransport,
    options: ProtocolClientOptions = {},
  ) {
    this.requestIdPrefix = options.requestIdPrefix ?? "req";
    this.requestTimeoutMs = options.requestTimeoutMs ?? 10_000;
    this.clientName = options.clientName ?? "ts-reference-client";
    this.clientVersion = options.clientVersion ?? "0.1.0";
    this.unsubscribe = transport.subscribe((event) => this.dispatch(event));
  }

  private dispatch(event: ProtocolTransportEvent): void {
    if (event.kind === "closed") {
      this.closed = true;
      this.rejectPending(event.reason);
      for (const listener of this.closeListeners) listener(event.reason);
      return;
    }

    const message = event.message;
    if (message.type === "response") {
      const pending = this.pending.get(message.id);
      if (pending) {
        clearTimeout(pending.timer);
        this.pending.delete(message.id);
        pending.resolve(message);
      }
    } else if (message.type === "request") {
      this.serverRequestDeliveries.set(
        message.id,
        (this.serverRequestDeliveries.get(message.id) ?? 0) + 1,
      );
      this.respondedServerRequests.delete(message.id);
      this.serverRequests.push(message);
      for (const listener of this.serverRequestListeners) listener(message);
    } else if (message.type === "notification") {
      this.notifications.push(message);
      for (const listener of this.notificationListeners) listener(message);
    }
  }

  async request(method: string, params?: unknown): Promise<ServerResponseMessage> {
    if (this.closed) {
      throw new Error("protocol transport is closed");
    }

    const id = `${this.requestIdPrefix}-${this.nextId++}`;
    const message: ClientMessage = {
      type: "request",
      id,
      method,
      ...(params === undefined ? {} : { params }),
    };

    return new Promise<ServerResponseMessage>((resolve, reject) => {
      const timer = setTimeout(() => {
        if (this.pending.delete(id)) {
          reject(new Error(`timeout waiting for response to ${method}`));
        }
      }, this.requestTimeoutMs);
      this.pending.set(id, { resolve, reject, timer });

      void this.transport.send(message).catch((error: unknown) => {
        const pending = this.pending.get(id);
        if (!pending) return;
        clearTimeout(pending.timer);
        this.pending.delete(id);
        pending.reject(asError(error, `failed to send ${method}`));
      });
    });
  }

  async respond(id: string, result: ResponseResult): Promise<void> {
    if (this.closed) {
      throw new Error("protocol transport is closed");
    }
    if (
      this.respondingServerRequests.has(id) ||
      this.respondedServerRequests.has(id)
    ) {
      throw new Error(`server request ${id} already has a response`);
    }
    this.respondingServerRequests.add(id);
    const delivery = this.serverRequestDeliveries.get(id) ?? 0;
    const request = this.serverRequests.find((candidate) => candidate.id === id);
    const message: ClientMessage = { type: "response", id, result };
    try {
      await this.transport.send(message);
      const index = request === undefined ? -1 : this.serverRequests.indexOf(request);
      if (index >= 0) this.serverRequests.splice(index, 1);
      if ((this.serverRequestDeliveries.get(id) ?? 0) === delivery) {
        this.respondedServerRequests.add(id);
        this.serverRequestDeliveries.delete(id);
      } else {
        this.respondedServerRequests.delete(id);
      }
    } finally {
      this.respondingServerRequests.delete(id);
    }
  }

  async respondSuccess(id: string, data?: unknown): Promise<void> {
    await this.respond(id, {
      status: "success",
      ...(data === undefined ? {} : { data }),
    });
  }

  async initialize(
    options: {
      experimentalMethods?: boolean;
      interactiveQuestions?: boolean | InteractiveQuestionsCapability;
    } = {},
  ): Promise<InitializeResult> {
    const interactiveQuestions =
      options.interactiveQuestions === true
        ? {
            single_select: true,
            multi_select: true,
            free_text: true,
            previews: true,
            annotations: true,
            special_outcomes: true,
          }
        : options.interactiveQuestions || undefined;
    const response = await this.request("initialize", {
      protocol_version: "1.0",
      client_info: { name: this.clientName, version: this.clientVersion },
      capabilities: {
        streaming: true,
        experimental_methods: options.experimentalMethods ?? false,
        ...(interactiveQuestions
          ? { interactive_questions: interactiveQuestions }
          : {}),
      },
    });
    return expectSuccess(response.result, "initialize") as InitializeResult;
  }

  async sessionList(): Promise<SessionListResult> {
    return this.requestData<SessionListResult>("session/list");
  }

  async permissionOverview(): Promise<PermissionOverview> {
    return this.requestData<PermissionOverview>("permission/overview");
  }

  async bootstrap(params: BootstrapParams = {}): Promise<BootstrapState> {
    return this.requestData<BootstrapState>("session/bootstrap", params);
  }

  async renameSession(params: SessionRenameParams): Promise<void> {
    await this.requestData("session/rename", params);
  }

  async forkSession(params: SessionForkParams): Promise<SessionForkResult> {
    return this.requestData<SessionForkResult>("session/fork", params);
  }

  async deleteSession(params: AcpDeleteSessionParams): Promise<void> {
    await this.requestData("session/acp_delete", params);
  }

  async sessionControlState(sessionId: string): Promise<SessionControlState> {
    return this.requestData<SessionControlState>("session/control_state", {
      session_id: sessionId,
    });
  }

  async setSessionPermissionMode(
    params: SetSessionPermissionModeParams,
  ): Promise<SessionControlState> {
    return this.requestData<SessionControlState>(
      "session/set_permission_mode",
      params,
    );
  }

  async setSessionModel(
    params: SetSessionModelParams,
  ): Promise<SessionControlState>;
  async setSessionModel(
    sessionId: string,
    model: string | null,
  ): Promise<SessionControlState>;
  async setSessionModel(
    paramsOrSessionId: SetSessionModelParams | string,
    model?: string | null,
  ): Promise<SessionControlState> {
    const params: SetSessionModelParams =
      typeof paramsOrSessionId === "string"
        ? { session_id: paramsOrSessionId, model: model ?? null }
        : paramsOrSessionId;
    return this.requestData<SessionControlState>("session/set_model", params);
  }

  async clearSessionModelOverride(
    sessionId: string,
  ): Promise<SessionControlState> {
    return this.setSessionModel({
      session_id: sessionId,
      model: null,
      inherit: true,
    });
  }

  async setSessionEffort(
    params: SetSessionEffortParams,
  ): Promise<SessionControlState> {
    return this.requestData<SessionControlState>("session/set_effort", params);
  }

  async submitTurn(params: TurnSubmitParams): Promise<TurnSubmitResult> {
    return this.requestData<TurnSubmitResult>("turn/submit", params);
  }

  async cancelTurn(sessionId: string): Promise<boolean> {
    const result = await this.requestData<TurnCancelResult>("turn/cancel", {
      session_id: sessionId,
    });
    return result.cancelled;
  }

  async outputStyleOptions(): Promise<OutputStyleOptionsResult> {
    return this.requestData<OutputStyleOptionsResult>(
      "settings/output_style_options",
    );
  }

  async activeOutputStyle(): Promise<ActiveOutputStyleResult> {
    return this.requestData<ActiveOutputStyleResult>(
      "settings/active_output_style",
    );
  }

  async setOutputStyle(style: string): Promise<OutputStyleResult> {
    return this.requestData<OutputStyleResult>("settings/set_output_style", {
      style,
    });
  }

  async sandboxSettings(): Promise<SandboxLocalSettings> {
    return this.requestData<SandboxLocalSettings>("settings/sandbox");
  }

  async updateSandbox(
    update: SandboxSettingsUpdate,
  ): Promise<SandboxLocalSettings> {
    await this.requestData("settings/update_sandbox", update);
    return this.sandboxSettings();
  }

  async settingLocked(key: string): Promise<boolean> {
    const result = await this.requestData<SettingLockedResult>(
      "settings/is_locked",
      { key },
    );
    return result.locked;
  }

  async theme(): Promise<ThemeResult> {
    return this.requestData<ThemeResult>("settings/theme");
  }

  async setTheme(theme: ThemeSetting): Promise<ThemeResult> {
    return this.requestData<ThemeResult>("settings/set_theme", { theme });
  }

  async editorMode(): Promise<EditorModeResult> {
    return this.requestData<EditorModeResult>("settings/editor_mode");
  }

  async setEditorMode(mode: EditorModeSetting): Promise<EditorModeResult> {
    return this.requestData<EditorModeResult>("settings/set_editor_mode", {
      mode,
    });
  }

  async respondToPermission(
    requestId: string,
    decision: PermissionDecisionWire,
  ): Promise<void> {
    await this.respondSuccess(requestId, decision);
  }

  async respondToMcpTrust(
    requestId: string,
    decision: McpTrustDecisionWire,
  ): Promise<void> {
    await this.respondSuccess(requestId, decision);
  }

  async respondToAskUser(
    requestId: string,
    response: AskUserQuestionResponse,
  ): Promise<void> {
    await this.respondSuccess(requestId, response);
  }

  subscribeServerRequests(listener: ServerRequestListener): () => void {
    this.serverRequestListeners.add(listener);
    return () => this.serverRequestListeners.delete(listener);
  }

  subscribeNotifications(listener: NotificationListener): () => void {
    this.notificationListeners.add(listener);
    return () => this.notificationListeners.delete(listener);
  }

  subscribeClose(listener: ProtocolCloseListener): () => void {
    this.closeListeners.add(listener);
    return () => this.closeListeners.delete(listener);
  }

  getServerRequests(): ServerRequestMessage[] {
    return [...this.serverRequests];
  }

  takeServerRequests(): ServerRequestMessage[] {
    return this.serverRequests.splice(0);
  }

  getNotifications(): ServerNotificationMessage[] {
    return [...this.notifications];
  }

  clearNotifications(): void {
    this.notifications.splice(0);
  }

  async close(): Promise<void> {
    if (this.cleanedUp) return;
    this.cleanedUp = true;
    if (!this.closed) await this.cancelPendingServerRequests();
    this.closed = true;
    this.unsubscribe();
    this.rejectPending("protocol client closed");
    await this.transport.close();
  }

  private async requestData<T = unknown>(
    method: string,
    params?: unknown,
  ): Promise<T> {
    const response = await this.request(method, params);
    return expectSuccess(response.result, method) as T;
  }

  private async cancelPendingServerRequests(): Promise<void> {
    const pending = this.serverRequests.filter(
      ({ id }) =>
        !this.respondingServerRequests.has(id) &&
        !this.respondedServerRequests.has(id),
    );
    await Promise.allSettled(
      pending.map((request) => {
        if (request.method === "permission/request") {
          return this.respondToPermission(request.id, { decision: "deny" });
        }
        if (request.method === "mcp_trust/request") {
          return this.respondToMcpTrust(request.id, "deny");
        }
        if (request.method === "ask_user/request") {
          const params = request.params as AskUserQuestionRequest;
          return this.respondToAskUser(request.id, {
            request_id: params.request_id,
            outcome: { outcome: "cancelled", reason: "disconnect" },
          });
        }
        return this.respond(request.id, {
          status: "error",
          code: "invalid_request",
          message: "client disconnected before handling server request",
        });
      }),
    );
  }

  private rejectPending(reason: string): void {
    for (const pending of this.pending.values()) {
      clearTimeout(pending.timer);
      pending.reject(new Error(reason));
    }
    this.pending.clear();
  }
}

export function expectSuccess(result: ResponseResult, context: string): unknown {
  if (result.status !== "success") {
    throw new Error(`${context} failed (${result.code}): ${result.message}`);
  }
  return result.data;
}

function asError(error: unknown, fallback: string): Error {
  return error instanceof Error ? error : new Error(fallback);
}
