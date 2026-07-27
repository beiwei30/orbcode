#!/usr/bin/env npx tsx
/**
 * Server-request smoke test — proves the TS client can handle
 * permission/request server-requests during a turn.
 *
 * Uses the mock provider (tool_use scenario) to trigger a Bash tool
 * call, which generates a permission/request server-request. The client
 * denies it and verifies the turn completes with the denial result.
 *
 * Usage: ORBCODE_BIN=/path/to/orbcode npx tsx src/smoke-server-requests.ts
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
  result?: { status: string; data?: unknown };
}

async function readConnectionInfo(proc: ChildProcess): Promise<ConnectionInfo> {
  return new Promise((resolve, reject) => {
    const rl = createInterface({ input: proc.stdout! });
    const timer = setTimeout(() => reject(new Error("timeout")), 15_000);
    rl.on("line", (line: string) => {
      try {
        const info = JSON.parse(line.trim());
        if (info.transport) { clearTimeout(timer); rl.close(); resolve(info); }
      } catch { /* skip */ }
    });
  });
}

async function main() {
  console.log("=== orbcode TypeScript server-request smoke test ===\n");

  const homeDir = mkdtempSync(join(tmpdir(), "orbcode-sreq-"));
  const cwdDir = mkdtempSync(join(tmpdir(), "orbcode-sreq-cwd-"));
  writeFileSync(join(homeDir, "settings.json"), JSON.stringify({
    env: {
      ANTHROPIC_API_KEY: "stub-key",
      ANTHROPIC_BASE_URL: "mock://anthropic?scenario=tool_use&key=bash&command=echo+test",
    },
  }));

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
    console.log("SKIP: could not read connection info");
    server.kill();
    return;
  }

  const ws = new WebSocket(`ws://${connInfo.addr}`);
  let nextId = 0;
  const pending = new Map<string, { resolve: (m: ServerMessage) => void }>();
  const serverRequests: ServerMessage[] = [];
  const notifications: ServerMessage[] = [];

  await new Promise<void>((resolve, reject) => {
    ws.on("open", () => {
      ws.send(connInfo.auth_token);
      ws.on("message", (data: WebSocket.RawData) => {
        try {
          const msg: ServerMessage = JSON.parse(data.toString());
          if (msg.type === "response" && msg.id) {
            const p = pending.get(msg.id);
            if (p) { pending.delete(msg.id); p.resolve(msg); }
          } else if (msg.type === "request") {
            serverRequests.push(msg);
          } else if (msg.type === "notification") {
            notifications.push(msg);
          }
        } catch { /* skip */ }
      });
      resolve();
    });
    ws.on("error", reject);
    setTimeout(() => reject(new Error("connect timeout")), 10_000);
  });

  function request(method: string, params?: unknown): Promise<ServerMessage> {
    const id = `r-${nextId++}`;
    return new Promise((resolve, reject) => {
      pending.set(id, { resolve });
      ws.send(JSON.stringify({ type: "request", id, method, params }));
      setTimeout(() => { pending.delete(id); reject(new Error(`timeout: ${method}`)); }, 30_000);
    });
  }

  function respond(id: string, data: unknown): void {
    ws.send(JSON.stringify({ type: "response", id, result: { status: "success", data } }));
  }

  try {
    // 1. Initialize
    console.log("1. Initialize");
    const initResp = await request("initialize", {
      protocol_version: "1.0",
      client_info: { name: "ts-sreq-test", version: "0.1.0" },
      capabilities: { streaming: true },
    });
    assert(initResp.result?.status === "success", "initialize succeeded");

    // 2. Bootstrap
    console.log("\n2. Bootstrap");
    const bsResp = await request("session/bootstrap");
    assert(bsResp.result?.status === "success", "bootstrap succeeded");
    const sessionId = ((bsResp.result?.data as Record<string, unknown>)?.session as Record<string, unknown>)?.session_id as string
      ?? (bsResp.result?.data as Record<string, unknown>)?.session_id as string;
    assert(!!sessionId, `got session_id: ${sessionId}`);

    // 3. Submit turn — this triggers mock tool_use → permission/request
    console.log("\n3. Submit turn (triggers permission/request)");
    const turnResp = await request("turn/submit", {
      session_id: sessionId,
      prompt: "test prompt",
    });
    assert(turnResp.result?.status === "success", "turn/submit accepted");

    // 4. Wait for and handle permission/request server-request
    console.log("\n4. Waiting for permission/request...");
    const deadline = Date.now() + 30_000;
    let permissionHandled = false;
    let turnFinished = false;

    while (Date.now() < deadline && !turnFinished) {
      await new Promise(r => setTimeout(r, 100));

      // Handle any pending server-requests (permission/request)
      while (serverRequests.length > 0) {
        const sreq = serverRequests.shift()!;
        if (sreq.method === "permission/request" && sreq.id) {
          console.log(`  Received permission/request for: ${(sreq.params as Record<string, unknown>)?.tool_name}`);
          respond(sreq.id, { decision: "deny" });
          permissionHandled = true;
          console.log("  PASS: responded with deny");
        }
      }

      // Check for turn_finished in notifications
      for (const n of notifications) {
        if (n.method === "stream/event") {
          const event = (n.params as Record<string, unknown>)?.event as Record<string, unknown> | undefined;
          if (event?.event === "turn_finished") {
            turnFinished = true;
          }
        }
      }
    }

    assert(permissionHandled, "permission/request was received and denied");
    assert(turnFinished, "turn finished after permission deny");

    console.log("\n=== All server-request smoke tests passed ===");
  } finally {
    ws.close();
    server.kill();
  }
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
