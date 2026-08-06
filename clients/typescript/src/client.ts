/**
 * Minimal orbcode protocol client for stdio NDJSON transport.
 *
 * Imports types from the generated protocol contract — no hand-written
 * protocol types, no Rust facade linking.
 */

import { ChildProcess, spawn } from "node:child_process";
import { createInterface, Interface } from "node:readline";
import type {
  ClientRequestEnvelope,
  ServerResponseEnvelope,
  ServerNotificationEnvelope,
  ServerRequestEnvelope,
  InitializeParams,
  InitializeResult,
  ResponseResult,
  ErrorCode,
  ServerCapabilities,
  ClientCapabilities,
  BootstrapState,
  PermissionOverview,
  SessionControlState,
  SetSessionModelParams,
  ThemeResult,
  ThemeSetting,
  EditorModeResult,
  EditorModeSetting,
} from "@orbcode/protocol";

export type ServerMessage =
  | (ServerResponseEnvelope & { type: "response" })
  | (ServerNotificationEnvelope & { type: "notification" })
  | (ServerRequestEnvelope & { type: "request" });

export class OrbCodeClient {
  private process: ChildProcess;
  private rl: Interface;
  private pending = new Map<
    string,
    {
      resolve: (msg: ServerMessage) => void;
      reject: (err: Error) => void;
    }
  >();
  private serverRequests: ServerMessage[] = [];
  private notifications: ServerMessage[] = [];
  private nextId = 0;

  constructor(binPath: string, homeDir: string, cwd: string) {
    this.process = spawn(binPath, ["serve", "--stdio"], {
      cwd,
      env: {
        ORBCODE_HOME: homeDir,
        PATH: process.env.PATH,
        HOME: homeDir,
        RUST_LOG: "warn",
      },
      stdio: ["pipe", "pipe", "ignore"],
    });

    this.rl = createInterface({ input: this.process.stdout! });
    this.rl.on("line", (line: string) => {
      try {
        const msg = JSON.parse(line) as ServerMessage;
        this.dispatch(msg);
      } catch {
        // skip non-JSON lines
      }
    });
  }

  private dispatch(msg: ServerMessage): void {
    if (msg.type === "response" && "id" in msg) {
      const p = this.pending.get(msg.id);
      if (p) {
        this.pending.delete(msg.id);
        p.resolve(msg);
      }
    } else if (msg.type === "request") {
      this.serverRequests.push(msg);
    } else if (msg.type === "notification") {
      this.notifications.push(msg);
    }
  }

  async request(method: string, params?: unknown): Promise<ServerMessage> {
    const id = `req-${this.nextId++}`;
    const msg = { type: "request" as const, id, method, params };
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      const line = JSON.stringify(msg) + "\n";
      this.process.stdin!.write(line);
      setTimeout(() => {
        if (this.pending.has(id)) {
          this.pending.delete(id);
          reject(new Error(`timeout waiting for response to ${method}`));
        }
      }, 10_000);
    });
  }

  async respond(id: string, data: unknown): Promise<void> {
    const msg = {
      type: "response",
      id,
      result: { status: "success", data },
    };
    const line = JSON.stringify(msg) + "\n";
    this.process.stdin!.write(line);
  }

  async initialize(
    opts: { experimentalMethods?: boolean } = {},
  ): Promise<InitializeResult> {
    const resp = await this.request("initialize", {
      protocol_version: "1.0",
      client_info: { name: "ts-reference-client", version: "0.1.0" },
      capabilities: {
        streaming: true,
        experimental_methods: opts.experimentalMethods ?? false,
      },
    });
    const result = (resp as any).result;
    if (result?.status !== "success") {
      throw new Error(`initialize failed: ${JSON.stringify(result)}`);
    }
    return result.data as InitializeResult;
  }

  async sessionList(): Promise<unknown[]> {
    const resp = await this.request("session/list");
    const result = (resp as any).result;
    if (result?.status !== "success") {
      throw new Error(`session/list failed: ${JSON.stringify(result)}`);
    }
    return result.data as unknown[];
  }

  async bootstrap(): Promise<BootstrapState> {
    const resp = await this.request("session/bootstrap");
    const result = (resp as any).result;
    if (result?.status !== "success") {
      throw new Error(`session/bootstrap failed: ${JSON.stringify(result)}`);
    }
    return result.data as BootstrapState;
  }

  async permissionOverview(): Promise<PermissionOverview> {
    const resp = await this.request("permission/overview");
    const result = (resp as any).result;
    if (result?.status !== "success") {
      throw new Error(`permission/overview failed: ${JSON.stringify(result)}`);
    }
    return result.data as PermissionOverview;
  }

  async sessionControlState(sessionId: string): Promise<SessionControlState> {
    const resp = await this.request("session/control_state", {
      session_id: sessionId,
    });
    const result = (resp as any).result;
    if (result?.status !== "success") {
      throw new Error(`session/control_state failed: ${JSON.stringify(result)}`);
    }
    return result.data as SessionControlState;
  }

  async setSessionModel(
    sessionId: string,
    model: string | null,
  ): Promise<SessionControlState> {
    const params: SetSessionModelParams = {
      session_id: sessionId,
      model,
    };
    const resp = await this.request("session/set_model", params);
    const result = (resp as any).result;
    if (result?.status !== "success") {
      throw new Error(`session/set_model failed: ${JSON.stringify(result)}`);
    }
    return result.data as SessionControlState;
  }

  async clearSessionModelOverride(sessionId: string): Promise<SessionControlState> {
    const params: SetSessionModelParams = {
      session_id: sessionId,
      model: null,
      inherit: true,
    };
    const resp = await this.request("session/set_model", params);
    const result = (resp as any).result;
    if (result?.status !== "success") {
      throw new Error(`session/set_model clear failed: ${JSON.stringify(result)}`);
    }
    return result.data as SessionControlState;
  }

  async theme(): Promise<ThemeResult> {
    const resp = await this.request("settings/theme");
    const result = (resp as any).result;
    if (result?.status !== "success") {
      throw new Error(`settings/theme failed: ${JSON.stringify(result)}`);
    }
    return result.data as ThemeResult;
  }

  async setTheme(theme: ThemeSetting): Promise<ThemeResult> {
    const resp = await this.request("settings/set_theme", { theme });
    const result = (resp as any).result;
    if (result?.status !== "success") {
      throw new Error(`settings/set_theme failed: ${JSON.stringify(result)}`);
    }
    return result.data as ThemeResult;
  }

  async editorMode(): Promise<EditorModeResult> {
    const resp = await this.request("settings/editor_mode");
    const result = (resp as any).result;
    if (result?.status !== "success") {
      throw new Error(`settings/editor_mode failed: ${JSON.stringify(result)}`);
    }
    return result.data as EditorModeResult;
  }

  async setEditorMode(mode: EditorModeSetting): Promise<EditorModeResult> {
    const resp = await this.request("settings/set_editor_mode", { mode });
    const result = (resp as any).result;
    if (result?.status !== "success") {
      throw new Error(`settings/set_editor_mode failed: ${JSON.stringify(result)}`);
    }
    return result.data as EditorModeResult;
  }

  getServerRequests(): ServerMessage[] {
    return [...this.serverRequests];
  }

  getNotifications(): ServerMessage[] {
    return [...this.notifications];
  }

  clearNotifications(): void {
    this.notifications = [];
  }

  async close(): Promise<void> {
    this.rl.close();
    this.process.stdin!.end();
    await new Promise<void>((resolve) => {
      this.process.on("close", () => resolve());
      setTimeout(resolve, 5000);
    });
  }
}
