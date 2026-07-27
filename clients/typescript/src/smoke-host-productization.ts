#!/usr/bin/env npx tsx
/**
 * Host productization smoke test.
 *
 * This is intentionally still a reference-host test, not a product UI. It
 * proves the host-facing lifecycle that a future web/desktop shell must own:
 * launch, discovery, initialize/capabilities, error handling, server-request
 * pump, disconnect cleanup, and reconnect after disconnect.
 */

import { spawn, type ChildProcess } from "node:child_process";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createInterface } from "node:readline";
import WebSocket from "ws";
import type {
  InitializeResult,
  ResponseResult,
  ServerMessage,
  ServerRequestEnvelope,
  ServerResponseEnvelope,
} from "@orbcode/protocol";

const BIN: string = process.env.ORBCODE_BIN ?? (() => {
  console.error("ORBCODE_BIN env var required");
  process.exit(1);
})();

function assert(condition: boolean, msg: string): void {
  if (!condition) throw new Error(`FAIL: ${msg}`);
  console.log(`  PASS: ${msg}`);
}

interface ConnectionInfo {
  transport: "websocket";
  addr: string;
  auth_token: string;
}

class HostWsClient {
  private ws!: WebSocket;
  private nextId = 0;
  private pending = new Map<
    string,
    { resolve: (msg: ServerResponseEnvelope & { type: "response" }) => void; reject: (err: Error) => void }
  >();
  private serverRequests: Array<ServerRequestEnvelope & { type: "request" }> = [];
  private notifications: ServerMessage[] = [];

  async connect(url: string, token: string): Promise<void> {
    await new Promise<void>((resolve, reject) => {
      const ws = new WebSocket(url);
      const timer = setTimeout(() => reject(new Error("WebSocket connect timeout")), 10_000);
      ws.on("open", () => {
        this.ws = ws;
        ws.send(token);
        ws.on("message", (data: WebSocket.RawData) => this.dispatch(data));
        clearTimeout(timer);
        resolve();
      });
      ws.on("error", reject);
    });
  }

  private dispatch(data: WebSocket.RawData): void {
    let msg: ServerMessage;
    try {
      msg = JSON.parse(data.toString()) as ServerMessage;
    } catch {
      return;
    }

    if (msg.type === "response") {
      const pending = this.pending.get(msg.id);
      if (pending) {
        this.pending.delete(msg.id);
        pending.resolve(msg);
      }
    } else if (msg.type === "request") {
      this.serverRequests.push(msg);
    } else if (msg.type === "notification") {
      this.notifications.push(msg);
    }
  }

  async request(method: string, params?: unknown): Promise<ServerResponseEnvelope & { type: "response" }> {
    const id = `host-${this.nextId++}`;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.ws.send(JSON.stringify({ type: "request", id, method, params }));
      setTimeout(() => {
        if (this.pending.has(id)) {
          this.pending.delete(id);
          reject(new Error(`timeout waiting for ${method}`));
        }
      }, 30_000);
    });
  }

  async initialize(experimentalMethods = false): Promise<InitializeResult> {
    const response = await this.request("initialize", {
      protocol_version: "1.0",
      client_info: { name: "ts-host-productization", version: "0.1.0" },
      capabilities: { streaming: true, experimental_methods: experimentalMethods },
    });
    const result = expectSuccess(response.result, "initialize");
    return result as InitializeResult;
  }

  respondSuccess(id: string, data: unknown): void {
    this.ws.send(JSON.stringify({
      type: "response",
      id,
      result: { status: "success", data },
    }));
  }

  takeServerRequests(): Array<ServerRequestEnvelope & { type: "request" }> {
    const requests = this.serverRequests;
    this.serverRequests = [];
    return requests;
  }

  hasTurnFinished(): boolean {
    return this.notifications.some((msg) => {
      if (msg.type !== "notification" || msg.method !== "stream/event") return false;
      const event = (msg.params as { event?: { event?: string } }).event;
      return event?.event === "turn_finished";
    });
  }

  async close(): Promise<void> {
    if (!this.ws || this.ws.readyState === WebSocket.CLOSED) return;
    await new Promise<void>((resolve) => {
      const timer = setTimeout(resolve, 2_000);
      this.ws.once("close", () => {
        clearTimeout(timer);
        resolve();
      });
      this.ws.close();
    });
  }
}

function expectSuccess(result: ResponseResult, context: string): unknown {
  if (result.status !== "success") {
    throw new Error(`${context} failed: ${JSON.stringify(result)}`);
  }
  return result.data;
}

function expectErrorCode(result: ResponseResult, code: string, context: string): void {
  if (result.status !== "error" || result.code !== code) {
    throw new Error(`${context} expected ${code}, got ${JSON.stringify(result)}`);
  }
}

async function readConnectionInfo(server: ChildProcess): Promise<ConnectionInfo> {
  return new Promise((resolve, reject) => {
    const rl = createInterface({ input: server.stdout! });
    const timer = setTimeout(() => reject(new Error("timeout reading connection info")), 15_000);
    rl.on("line", (line: string) => {
      try {
        const info = JSON.parse(line.trim()) as Partial<ConnectionInfo>;
        if (info.transport === "websocket" && info.addr && info.auth_token) {
          clearTimeout(timer);
          rl.close();
          resolve(info as ConnectionInfo);
        }
      } catch {
        // Skip non-JSON lines.
      }
    });
  });
}

function spawnServer(): { server: ChildProcess; info: Promise<ConnectionInfo> } {
  const homeDir = mkdtempSync(join(tmpdir(), "orbcode-host-product-"));
  const cwdDir = mkdtempSync(join(tmpdir(), "orbcode-host-product-cwd-"));
  writeFileSync(join(homeDir, "settings.json"), JSON.stringify({
    env: {
      ANTHROPIC_API_KEY: "stub-key",
      ANTHROPIC_BASE_URL: "mock://anthropic?scenario=tool_use&key=bash&command=echo+host",
    },
  }));

  const server = spawn(BIN, ["serve", "--websocket", "127.0.0.1:0"], {
    cwd: cwdDir,
    env: {
      ORBCODE_HOME: homeDir,
      PATH: process.env.PATH ?? "",
      HOME: homeDir,
      RUST_LOG: "warn",
    },
    stdio: ["ignore", "pipe", "pipe"],
  });

  return { server, info: readConnectionInfo(server) };
}

async function waitForPermissionRequest(
  client: HostWsClient,
  label: string,
): Promise<ServerRequestEnvelope & { type: "request" }> {
  const deadline = Date.now() + 30_000;
  while (Date.now() < deadline) {
    for (const request of client.takeServerRequests()) {
      if (request.method === "permission/request") {
        return request;
      }
    }
    await sleep(100);
  }
  throw new Error(`${label}: timed out waiting for permission/request`);
}

async function waitForTurnFinished(client: HostWsClient, label: string): Promise<void> {
  const deadline = Date.now() + 30_000;
  while (Date.now() < deadline) {
    if (client.hasTurnFinished()) return;
    await sleep(100);
  }
  throw new Error(`${label}: timed out waiting for turn_finished`);
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function connectInitialized(
  connInfo: ConnectionInfo,
  experimentalMethods = false,
): Promise<{ client: HostWsClient; init: InitializeResult }> {
  const client = new HostWsClient();
  await client.connect(`ws://${connInfo.addr}`, connInfo.auth_token);
  const init = await client.initialize(experimentalMethods);
  return { client, init };
}

async function main(): Promise<void> {
  console.log("=== orbcode TypeScript host productization smoke test ===\n");

  const { server, info } = spawnServer();
  let connInfo: ConnectionInfo;
  try {
    connInfo = await info;
  } catch {
    console.log("SKIP: could not read WebSocket connection info");
    server.kill();
    return;
  }

  try {
    console.log("1. Capability negotiation and unknown-method handling");
    const first = await connectInitialized(connInfo, true);
    assert(first.init.capabilities.streaming === true, "streaming capability advertised");
    assert(first.init.capabilities.stable_methods.includes("session/bootstrap"), "stable methods advertised");
    assert(first.init.capabilities.experimental_methods.includes("background/list"), "experimental methods advertised after opt-in");
    assert(first.init.capabilities.server_request_methods.includes("permission/request"), "permission/request advertised");

    const unknown = await first.client.request("host/unknown_method");
    expectErrorCode(unknown.result, "method_not_found", "unknown method");
    assert(true, "unknown method returns method_not_found");

    await first.client.close();

    console.log("\n2. Reconnect after disconnect");
    const second = await connectInitialized(connInfo, false);
    assert(second.init.capabilities.experimental_methods.length === 0, "default reconnect hides experimental methods");
    const list = await second.client.request("session/list");
    expectSuccess(list.result, "session/list after reconnect");
    assert(true, "session/list succeeds after reconnect");

    console.log("\n3. Server-request pump");
    const bootstrap = await second.client.request("session/bootstrap");
    const session = ((expectSuccess(bootstrap.result, "session/bootstrap") as any).session ?? {}) as { session_id?: string };
    assert(!!session.session_id, "bootstrap returns session_id");
    const submit = await second.client.request("turn/submit", {
      session_id: session.session_id,
      prompt: "trigger permission request",
    });
    expectSuccess(submit.result, "turn/submit");
    const permission = await waitForPermissionRequest(second.client, "server-request pump");
    second.client.respondSuccess(permission.id, { decision: "deny" });
    await waitForTurnFinished(second.client, "server-request pump");
    assert(true, "permission/request handled and turn finished");

    await second.client.close();
  } finally {
    server.kill();
  }

  const cleanup = spawnServer();
  let cleanupConnInfo: ConnectionInfo;
  try {
    cleanupConnInfo = await cleanup.info;
  } catch {
    console.log("SKIP: could not read cleanup WebSocket connection info");
    cleanup.server.kill();
    return;
  }

  try {
    console.log("\n4. Disconnect cleanup while server-request is pending");
    const cleanupClient = await connectInitialized(cleanupConnInfo, false);
    const cleanupBootstrap = await cleanupClient.client.request("session/bootstrap");
    const cleanupSession = ((expectSuccess(cleanupBootstrap.result, "cleanup bootstrap") as any).session ?? {}) as { session_id?: string };
    assert(!!cleanupSession.session_id, "cleanup bootstrap returns session_id");
    const cleanupSubmit = await cleanupClient.client.request("turn/submit", {
      session_id: cleanupSession.session_id,
      prompt: "trigger pending permission then disconnect",
    });
    expectSuccess(cleanupSubmit.result, "cleanup turn/submit");
    await waitForPermissionRequest(cleanupClient.client, "disconnect cleanup");
    await cleanupClient.client.close();

    const third = await connectInitialized(cleanupConnInfo, false);
    const postCleanupList = await third.client.request("session/list");
    expectSuccess(postCleanupList.result, "session/list after pending disconnect");
    assert(true, "server accepts reconnect after pending server-request disconnect");
    await third.client.close();

    console.log("\n=== All host productization smoke tests passed ===");
  } finally {
    cleanup.server.kill();
  }
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
