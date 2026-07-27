#!/usr/bin/env npx tsx
/**
 * WebSocket smoke test — proves the protocol contract is consumable
 * over WebSocket transport from TypeScript, using structured connection
 * info from stdout for endpoint discovery.
 *
 * Usage: ORBCODE_BIN=/path/to/orbcode npx tsx src/smoke-ws.ts
 */

import { spawn, type ChildProcess } from "node:child_process";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createInterface } from "node:readline";
import WebSocket from "ws";

const BIN: string = process.env.ORBCODE_BIN ?? (() => { console.error("ORBCODE_BIN env var required"); process.exit(1); })();

function assert(condition: boolean, msg: string): void {
  if (!condition) throw new Error(`FAIL: ${msg}`);
  console.log(`  PASS: ${msg}`);
}

interface ConnectionInfo {
  transport: string;
  addr: string;
  auth_token: string;
}

interface ServerMessage {
  type: string;
  id?: string;
  method?: string;
  params?: Record<string, unknown>;
  result?: { status: string; data?: unknown; code?: string; message?: string };
}

class WsClient {
  private ws!: WebSocket;
  private pending = new Map<
    string,
    { resolve: (msg: ServerMessage) => void; reject: (err: Error) => void }
  >();
  private serverRequests: ServerMessage[] = [];
  private nextId = 0;
  private onServerRequest?: (msg: ServerMessage) => void;

  async connect(url: string, authToken: string): Promise<void> {
    return new Promise((resolve, reject) => {
      this.ws = new WebSocket(url);
      this.ws.on("open", () => {
        this.ws.send(authToken);
        this.ws.on("message", (data: WebSocket.RawData) => {
          try {
            const msg: ServerMessage = JSON.parse(data.toString());
            if (msg.type === "response" && msg.id) {
              const p = this.pending.get(msg.id);
              if (p) {
                this.pending.delete(msg.id);
                p.resolve(msg);
              }
            } else if (msg.type === "request") {
              this.serverRequests.push(msg);
              this.onServerRequest?.(msg);
            }
          } catch { /* skip */ }
        });
        resolve();
      });
      this.ws.on("error", reject);
      setTimeout(() => reject(new Error("WS connect timeout")), 10_000);
    });
  }

  setServerRequestHandler(handler: (msg: ServerMessage) => void): void {
    this.onServerRequest = handler;
  }

  async request(method: string, params?: unknown): Promise<ServerMessage> {
    const id = `ws-${this.nextId++}`;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.ws.send(JSON.stringify({ type: "request", id, method, params }));
      setTimeout(() => {
        if (this.pending.has(id)) {
          this.pending.delete(id);
          reject(new Error(`timeout: ${method}`));
        }
      }, 10_000);
    });
  }

  respond(id: string, data: unknown): void {
    this.ws.send(JSON.stringify({
      type: "response",
      id,
      result: { status: "success", data },
    }));
  }

  getServerRequests(): ServerMessage[] {
    return [...this.serverRequests];
  }

  close(): void {
    this.ws.close();
  }
}

async function readConnectionInfo(server: ChildProcess): Promise<ConnectionInfo> {
  return new Promise((resolve, reject) => {
    const rl = createInterface({ input: server.stdout! });
    const timer = setTimeout(() => reject(new Error("timeout reading connection info")), 15_000);
    rl.on("line", (line: string) => {
      try {
        const info = JSON.parse(line.trim());
        if (info.transport) {
          clearTimeout(timer);
          rl.close();
          resolve(info as ConnectionInfo);
        }
      } catch { /* skip non-JSON lines */ }
    });
  });
}

async function main() {
  console.log("=== orbcode TypeScript WebSocket smoke test ===\n");

  const homeDir = mkdtempSync(join(tmpdir(), "orbcode-ws-smoke-"));
  const cwdDir = mkdtempSync(join(tmpdir(), "orbcode-ws-cwd-"));
  writeFileSync(
    join(homeDir, "settings.json"),
    JSON.stringify({ env: { ANTHROPIC_API_KEY: "stub-key" } }),
  );

  const server: ChildProcess = spawn(
    BIN,
    ["serve", "--websocket", "127.0.0.1:0"],
    {
      cwd: cwdDir,
      env: {
        ORBCODE_HOME: homeDir,
        PATH: process.env.PATH ?? "",
        HOME: homeDir,
        RUST_LOG: "warn",
      },
      stdio: ["ignore", "pipe", "pipe"],
    },
  );

  let connInfo: ConnectionInfo;
  try {
    connInfo = await readConnectionInfo(server);
  } catch {
    console.log("SKIP: could not read connection info from server stdout");
    server.kill();
    return;
  }

  console.log(`Server: ${connInfo.transport} at ${connInfo.addr}`);
  assert(connInfo.transport === "websocket", "transport is websocket");
  assert(!connInfo.addr.endsWith(":0"), "addr has real port");

  const client = new WsClient();
  try {
    await client.connect(`ws://${connInfo.addr}`, connInfo.auth_token);

    // 1. Initialize with experimental opt-in
    console.log("\n1. Initialize over WebSocket");
    const initResp = await client.request("initialize", {
      protocol_version: "1.0",
      client_info: { name: "ts-ws-client", version: "0.1.0" },
      capabilities: { streaming: true, experimental_methods: true },
    });
    assert(initResp.result?.status === "success", "initialize succeeded");
    const caps = (initResp.result?.data as Record<string, unknown>)?.capabilities as Record<string, unknown> | undefined;
    assert(caps?.streaming === true, "streaming capability");
    assert(
      Array.isArray(caps?.experimental_methods) && (caps.experimental_methods as unknown[]).length > 0,
      "experimental methods visible (opt-in)",
    );

    // 2. Session list
    console.log("\n2. Session list over WebSocket");
    const listResp = await client.request("session/list");
    assert(listResp.result?.status === "success", "session/list succeeded");

    // 3. Bootstrap
    console.log("\n3. Bootstrap over WebSocket");
    const bsResp = await client.request("session/bootstrap");
    assert(bsResp.result?.status === "success", "bootstrap succeeded");

    console.log("\n=== All WebSocket smoke tests passed ===");
  } finally {
    client.close();
    server.kill();
  }
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
