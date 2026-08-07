import type {
  AskUserAnswerValue,
  AskUserQuestionRequest,
  BootstrapState,
  InteractiveQuestionsCapability,
  McpTrustApprovalRequest,
  McpTrustDecisionWire,
  OutputStyleOptionOverview,
  PermissionDecisionWire,
  PermissionMode,
  PermissionRequest,
  SandboxLocalSettings,
  ServerNotificationEnvelope,
  ServerRequestEnvelope,
  SessionControlState,
  SessionSummary,
  StreamEvent,
  StreamEventNotification,
  TranscriptMessage,
} from "../../typescript/src/generated/protocol.js";
import type { ProtocolClient } from "../../typescript/src/protocol-client.js";
import type {
  BinarySource,
  ConnectionStatus,
  ConnectionKind,
} from "./desktop-transport.js";
import { validateDesktopCompatibility } from "./compatibility.js";

export interface ConnectionOverview {
  kind: ConnectionKind;
  label: string;
  binarySource?: BinarySource;
  generation: number;
  childPid?: number;
}

export type InteractiveRequest =
  | {
      envelopeId: string;
      kind: "permission";
      request: PermissionRequest;
      responding: boolean;
    }
  | {
      envelopeId: string;
      kind: "mcp_trust";
      request: McpTrustApprovalRequest;
      responding: boolean;
    }
  | {
      envelopeId: string;
      kind: "ask_user";
      request: AskUserQuestionRequest;
      responding: boolean;
    };

export interface SettingLocks {
  model: boolean;
  effort: boolean;
  permissions: boolean;
  sandbox: boolean;
  outputStyle: boolean;
}

export interface DesktopAppState {
  phase: "disconnected" | "connecting" | "connected" | "reconnecting";
  connection?: ConnectionOverview;
  sessions: SessionSummary[];
  bootstrap?: BootstrapState;
  controls?: SessionControlState;
  outputStyles: OutputStyleOptionOverview[];
  activeOutputStyle?: string;
  sandbox?: SandboxLocalSettings;
  locks: SettingLocks;
  assistantDraft: string;
  thinkingDraft: string;
  streaming: boolean;
  subscriptionId?: string;
  requests: InteractiveRequest[];
  diagnostic?: string;
}

export type DesktopAppListener = (state: Readonly<DesktopAppState>) => void;

export type DesktopProtocolClient = Pick<
  ProtocolClient,
  | "initialize"
  | "sessionList"
  | "bootstrap"
  | "renameSession"
  | "forkSession"
  | "deleteSession"
  | "sessionControlState"
  | "setSessionPermissionMode"
  | "setSessionModel"
  | "setSessionEffort"
  | "submitTurn"
  | "cancelTurn"
  | "outputStyleOptions"
  | "activeOutputStyle"
  | "setOutputStyle"
  | "sandboxSettings"
  | "updateSandbox"
  | "settingLocked"
  | "respondToPermission"
  | "respondToMcpTrust"
  | "respondToAskUser"
  | "subscribeServerRequests"
  | "subscribeNotifications"
  | "subscribeClose"
  | "close"
>;

export interface DesktopConnectionLike {
  status: ConnectionStatus;
  client: DesktopProtocolClient;
}

export const DESKTOP_INTERACTIVE_QUESTIONS = {
  single_select: true,
  multi_select: true,
  free_text: true,
  previews: false,
  annotations: false,
  special_outcomes: false,
} as const satisfies InteractiveQuestionsCapability;

const DEFAULT_LOCKS: SettingLocks = {
  model: false,
  effort: false,
  permissions: false,
  sandbox: false,
  outputStyle: false,
};

export class DesktopAppController {
  private state: DesktopAppState = {
    phase: "disconnected",
    sessions: [],
    outputStyles: [],
    locks: { ...DEFAULT_LOCKS },
    assistantDraft: "",
    thinkingDraft: "",
    streaming: false,
    requests: [],
  };
  private readonly listeners = new Set<DesktopAppListener>();
  private client?: DesktopProtocolClient;
  private unsubscribeNotification?: () => void;
  private unsubscribeServerRequest?: () => void;
  private unsubscribeClose?: () => void;
  private epoch = 0;
  private activeTurnSessionId?: string;
  private pendingStreamEvents: StreamEventNotification[] = [];

  get snapshot(): Readonly<DesktopAppState> {
    return this.state;
  }

  subscribe(listener: DesktopAppListener): () => void {
    this.listeners.add(listener);
    listener(this.state);
    return () => this.listeners.delete(listener);
  }

  async attach(
    connection: DesktopConnectionLike,
    preferredSessionId?: string,
    originLabel?: string,
  ): Promise<void> {
    const epoch = ++this.epoch;
    const reconnecting = this.state.phase !== "disconnected";
    await this.detachClient();
    this.clearActiveTurn();
    this.client = connection.client;
    this.unsubscribeNotification = connection.client.subscribeNotifications(
      (notification) => this.onNotification(notification),
    );
    this.unsubscribeServerRequest = connection.client.subscribeServerRequests(
      (request) => this.onServerRequest(request),
    );
    this.unsubscribeClose = connection.client.subscribeClose((reason) =>
      this.onClientClosed(connection.client, reason),
    );
    this.patch({
      phase: reconnecting ? "reconnecting" : "connecting",
      connection: {
        kind: connection.status.kind ?? "local",
        label: originLabel ?? (connection.status.kind === "ssh" ? "SSH remote" : "This Mac"),
        binarySource: connection.status.binary_source,
        generation: connection.status.generation,
        childPid: connection.status.child_pid,
      },
      diagnostic: undefined,
      requests: [],
    });

    try {
      const initialized = await connection.client.initialize({
        experimentalMethods: true,
        interactiveQuestions: DESKTOP_INTERACTIVE_QUESTIONS,
      });
      validateDesktopCompatibility(initialized);
      this.assertCurrent(epoch);
      const sessions = await connection.client.sessionList();
      this.assertCurrent(epoch);
      const sessionId =
        preferredSessionId &&
        sessions.some((session) => session.session_id === preferredSessionId)
          ? preferredSessionId
          : sessions[0]?.session_id;
      const bootstrap = await connection.client.bootstrap(
        sessionId ? { session_id: sessionId } : {},
      );
      this.assertCurrent(epoch);
      this.patch({ phase: "connected", sessions, bootstrap });
      await this.refreshControls(epoch);
      this.runInBackground(this.refreshSessions());
    } catch (error) {
      if (epoch !== this.epoch) return;
      await this.detachClient();
      this.patch({
        phase: "disconnected",
        connection: undefined,
        diagnostic: errorMessage(error, "Connection failed"),
      });
      throw error;
    }
  }

  async disconnect(): Promise<void> {
    ++this.epoch;
    await this.detachClient();
    this.clearActiveTurn();
    this.patch({
      phase: "disconnected",
      connection: undefined,
      streaming: false,
      subscriptionId: undefined,
      assistantDraft: "",
      thinkingDraft: "",
      requests: [],
    });
  }

  async newSession(): Promise<void> {
    if (this.state.streaming) throw new Error("Cancel the active turn before creating a session");
    const client = this.requireClient();
    const bootstrap = await client.bootstrap();
    this.patch({ bootstrap, assistantDraft: "", thinkingDraft: "" });
    await Promise.all([this.refreshSessions(), this.refreshControls(this.epoch)]);
  }

  async loadSession(sessionId: string): Promise<void> {
    if (this.state.streaming) throw new Error("Cancel the active turn before changing sessions");
    const bootstrap = await this.requireClient().bootstrap({ session_id: sessionId });
    this.patch({ bootstrap, assistantDraft: "", thinkingDraft: "" });
    await this.refreshControls(this.epoch);
  }

  async renameSession(sessionId: string, newTitle: string): Promise<void> {
    await this.requireClient().renameSession({ session_id: sessionId, new_title: newTitle });
    await this.refreshSessions();
  }

  async forkSession(sessionId: string): Promise<void> {
    if (this.state.streaming) throw new Error("Cancel the active turn before forking a session");
    const forked = await this.requireClient().forkSession({ session_id: sessionId });
    await this.loadSession(forked.session_id);
    await this.refreshSessions();
  }

  async deleteSession(sessionId: string): Promise<void> {
    if (sessionId === this.state.bootstrap?.session.session_id && this.state.streaming) {
      throw new Error("Cancel the active turn before deleting this session");
    }
    const session = this.state.sessions.find((candidate) => candidate.session_id === sessionId);
    const cwd = session?.cwd ??
      (sessionId === this.state.bootstrap?.session.session_id
        ? this.state.bootstrap.cwd
        : undefined);
    if (!cwd) throw new Error("The session has no working directory for deletion");
    await this.requireClient().deleteSession({ session_id: sessionId, cwd });
    const sessions = await this.requireClient().sessionList();
    this.patch({ sessions });
    if (sessionId === this.state.bootstrap?.session.session_id) {
      const next = sessions[0]?.session_id;
      const bootstrap = await this.requireClient().bootstrap(
        next ? { session_id: next } : {},
      );
      this.patch({ bootstrap });
      await this.refreshControls(this.epoch);
    }
  }

  async submitTurn(prompt: string): Promise<void> {
    const trimmed = prompt.trim();
    if (!trimmed) return;
    if (this.state.streaming) throw new Error("A turn is already active");
    const sessionId = this.requireSessionId();
    this.activeTurnSessionId = sessionId;
    this.pendingStreamEvents = [];
    this.patch({
      streaming: true,
      assistantDraft: "",
      thinkingDraft: "",
      diagnostic: undefined,
    });
    try {
      const result = await this.requireClient().submitTurn({
        session_id: sessionId,
        prompt: trimmed,
      });
      this.patch({ subscriptionId: result.subscription_id });
      this.flushPendingStreamEvents(sessionId, result.subscription_id);
    } catch (error) {
      this.clearActiveTurn();
      this.patch({ streaming: false, diagnostic: errorMessage(error, "Turn failed") });
      throw error;
    }
  }

  async cancelTurn(): Promise<void> {
    const cancelled = await this.requireClient().cancelTurn(this.requireSessionId());
    if (!cancelled) this.patch({ diagnostic: "No active turn was available to cancel." });
  }

  async setPermissionMode(mode: PermissionMode): Promise<void> {
    const controls = await this.requireClient().setSessionPermissionMode({
      session_id: this.requireSessionId(),
      mode,
    });
    this.patch({ controls });
  }

  async setModel(model: string | null): Promise<void> {
    const controls = await this.requireClient().setSessionModel({
      session_id: this.requireSessionId(),
      model,
    });
    this.patch({ controls });
  }

  async setEffort(effort: string | null): Promise<void> {
    const controls = await this.requireClient().setSessionEffort({
      session_id: this.requireSessionId(),
      effort,
    });
    this.patch({ controls });
  }

  async setSandboxEnabled(enabled: boolean): Promise<void> {
    const sandbox = await this.requireClient().updateSandbox({ enabled });
    this.patch({ sandbox });
  }

  async setOutputStyle(style: string): Promise<void> {
    const result = await this.requireClient().setOutputStyle(style);
    const outputStyles = this.state.outputStyles.map((option) => ({
      ...option,
      current: option.value === result.style,
    }));
    this.patch({ activeOutputStyle: result.style, outputStyles });
  }

  async respondPermission(
    envelopeId: string,
    decision: PermissionDecisionWire,
  ): Promise<void> {
    await this.respondOnce(envelopeId, (client) =>
      client.respondToPermission(envelopeId, decision),
    );
  }

  async respondMcpTrust(
    envelopeId: string,
    decision: McpTrustDecisionWire,
  ): Promise<void> {
    await this.respondOnce(envelopeId, (client) =>
      client.respondToMcpTrust(envelopeId, decision),
    );
  }

  async answerAskUser(
    envelopeId: string,
    answers: Record<string, AskUserAnswerValue>,
  ): Promise<void> {
    const item = this.state.requests.find(
      (request) => request.envelopeId === envelopeId && request.kind === "ask_user",
    );
    if (!item || item.kind !== "ask_user") throw new Error("AskUser request is no longer pending");
    await this.respondOnce(envelopeId, (client) =>
      client.respondToAskUser(envelopeId, {
        request_id: item.request.request_id,
        outcome: { outcome: "answered", answers },
      }),
    );
  }

  async rejectAskUser(envelopeId: string): Promise<void> {
    const item = this.state.requests.find(
      (request) => request.envelopeId === envelopeId && request.kind === "ask_user",
    );
    if (!item || item.kind !== "ask_user") throw new Error("AskUser request is no longer pending");
    await this.respondOnce(envelopeId, (client) =>
      client.respondToAskUser(envelopeId, {
        request_id: item.request.request_id,
        outcome: { outcome: "rejected" },
      }),
    );
  }

  private async refreshSessions(): Promise<void> {
    const client = this.client;
    if (!client) return;
    const sessions = await client.sessionList();
    if (client === this.client) this.patch({ sessions });
  }

  private async refreshControls(epoch: number): Promise<void> {
    const client = this.requireClient();
    const sessionId = this.requireSessionId();
    const [controls, outputStyles, activeOutputStyle, sandbox, locks] =
      await Promise.all([
        client.sessionControlState(sessionId),
        client.outputStyleOptions(),
        client.activeOutputStyle(),
        client.sandboxSettings(),
        Promise.all([
          client.settingLocked("model"),
          client.settingLocked("effortLevel"),
          client.settingLocked("permissions"),
          client.settingLocked("sandbox"),
          client.settingLocked("outputStyle"),
        ]),
      ]);
    this.assertCurrent(epoch);
    this.patch({
      controls,
      outputStyles,
      activeOutputStyle: activeOutputStyle.name,
      sandbox,
      locks: {
        model: locks[0],
        effort: locks[1],
        permissions: locks[2],
        sandbox: locks[3],
        outputStyle: locks[4],
      },
    });
  }

  private onNotification(notification: ServerNotificationEnvelope): void {
    if (notification.method !== "stream/event") return;
    const payload = notification.params as StreamEventNotification;
    if (
      !this.activeTurnSessionId ||
      this.state.bootstrap?.session.session_id !== this.activeTurnSessionId
    ) return;
    if (!this.state.subscriptionId) {
      this.pendingStreamEvents.push(payload);
      return;
    }
    if (payload.subscription_id !== this.state.subscriptionId) return;
    this.applyStreamEvent(payload.event);
  }

  private flushPendingStreamEvents(sessionId: string, subscriptionId: string): void {
    const pending = this.pendingStreamEvents;
    this.pendingStreamEvents = [];
    for (const payload of pending) {
      if (
        this.activeTurnSessionId !== sessionId ||
        this.state.bootstrap?.session.session_id !== sessionId ||
        this.state.subscriptionId !== subscriptionId
      ) break;
      if (payload.subscription_id === subscriptionId) this.applyStreamEvent(payload.event);
    }
  }

  private applyStreamEvent(event: StreamEvent): void {
    switch (event.event) {
      case "user_message":
        this.appendMessage(event.message);
        break;
      case "assistant_message_started":
        this.patch({ assistantDraft: "", thinkingDraft: "" });
        break;
      case "assistant_delta":
        this.patch({ assistantDraft: this.state.assistantDraft + event.delta });
        break;
      case "thinking_delta":
        this.patch({ thinkingDraft: this.state.thinkingDraft + event.delta });
        break;
      case "assistant_message_completed":
        this.appendMessage(event.message);
        this.patch({ assistantDraft: "", thinkingDraft: "" });
        break;
      case "turn_cancelled":
        if (event.partial) this.appendMessage(event.partial);
        this.finishTurn();
        break;
      case "turn_finished":
        this.finishTurn();
        break;
      case "error":
        this.clearActiveTurn();
        this.patch({
          diagnostic: [event.message, event.suggestion].filter(Boolean).join(" "),
          streaming: false,
          subscriptionId: undefined,
        });
        break;
      default:
        break;
    }
  }

  private appendMessage(message: TranscriptMessage): void {
    const bootstrap = this.state.bootstrap;
    if (!bootstrap) return;
    const messages = [...bootstrap.session.messages];
    const existing = messages.findIndex((candidate) => candidate.id === message.id);
    if (existing >= 0) messages[existing] = message;
    else messages.push(message);
    this.patch({
      bootstrap: {
        ...bootstrap,
        session: { ...bootstrap.session, messages },
      },
    });
  }

  private finishTurn(): void {
    this.clearActiveTurn();
    this.patch({
      streaming: false,
      subscriptionId: undefined,
      assistantDraft: "",
      thinkingDraft: "",
    });
    this.runInBackground(this.refreshSessions());
  }

  private onServerRequest(envelope: ServerRequestEnvelope): void {
    let request: InteractiveRequest | undefined;
    if (envelope.method === "permission/request") {
      request = {
        envelopeId: envelope.id,
        kind: "permission",
        request: envelope.params as PermissionRequest,
        responding: false,
      };
    } else if (envelope.method === "mcp_trust/request") {
      request = {
        envelopeId: envelope.id,
        kind: "mcp_trust",
        request: envelope.params as McpTrustApprovalRequest,
        responding: false,
      };
    } else if (envelope.method === "ask_user/request") {
      request = {
        envelopeId: envelope.id,
        kind: "ask_user",
        request: envelope.params as AskUserQuestionRequest,
        responding: false,
      };
    }
    if (!request) return;
    const existing = this.state.requests.findIndex(
      (candidate) => candidate.envelopeId === envelope.id,
    );
    this.patch({
      requests:
        existing < 0
          ? [...this.state.requests, request]
          : this.state.requests.map((candidate, index) =>
              index === existing ? request : candidate,
            ),
    });
  }

  private onClientClosed(client: DesktopProtocolClient, reason: string): void {
    if (client !== this.client) return;
    this.clearActiveTurn();
    this.client = undefined;
    this.unsubscribeNotification?.();
    this.unsubscribeNotification = undefined;
    this.unsubscribeServerRequest?.();
    this.unsubscribeServerRequest = undefined;
    this.unsubscribeClose?.();
    this.unsubscribeClose = undefined;
    this.patch({
      phase: "disconnected",
      connection: undefined,
      streaming: false,
      subscriptionId: undefined,
      requests: [],
      diagnostic: reason,
    });
    void client.close().catch(() => {
      // The child is already terminal; reconnect replacement also performs
      // idempotent host cleanup if this best-effort disconnect cannot return.
    });
  }

  private async respondOnce(
    envelopeId: string,
    send: (client: DesktopProtocolClient) => Promise<void>,
  ): Promise<void> {
    const request = this.state.requests.find((item) => item.envelopeId === envelopeId);
    if (!request || request.responding) throw new Error("Request is no longer pending");
    const respondingRequest = { ...request, responding: true } as InteractiveRequest;
    this.patch({
      requests: this.state.requests.map((item) =>
        item === request ? respondingRequest : item,
      ),
    });
    try {
      await send(this.requireClient());
      this.patch({
        requests: this.state.requests.filter((item) => item !== respondingRequest),
      });
    } catch (error) {
      this.patch({
        requests: this.state.requests.map((item) =>
          item === respondingRequest ? { ...item, responding: false } : item,
        ),
        diagnostic: errorMessage(error, "Request response failed"),
      });
      throw error;
    }
  }

  private clearActiveTurn(): void {
    this.activeTurnSessionId = undefined;
    this.pendingStreamEvents = [];
  }

  private requireClient(): DesktopProtocolClient {
    if (!this.client) throw new Error("Not connected");
    return this.client;
  }

  private requireSessionId(): string {
    const sessionId = this.state.bootstrap?.session.session_id;
    if (!sessionId) throw new Error("No active session");
    return sessionId;
  }

  private assertCurrent(epoch: number): void {
    if (epoch !== this.epoch) throw new Error("Connection was replaced");
  }

  private async detachClient(): Promise<void> {
    const client = this.client;
    this.client = undefined;
    this.unsubscribeNotification?.();
    this.unsubscribeNotification = undefined;
    this.unsubscribeServerRequest?.();
    this.unsubscribeServerRequest = undefined;
    this.unsubscribeClose?.();
    this.unsubscribeClose = undefined;
    await client?.close();
  }

  private patch(patch: Partial<DesktopAppState>): void {
    this.state = { ...this.state, ...patch };
    for (const listener of this.listeners) listener(this.state);
  }

  private runInBackground(operation: Promise<void>): void {
    void operation.catch((error: unknown) => {
      if (this.client) {
        this.patch({ diagnostic: errorMessage(error, "Background refresh failed") });
      }
    });
  }
}

function errorMessage(error: unknown, fallback: string): string {
  return error instanceof Error ? error.message : fallback;
}
