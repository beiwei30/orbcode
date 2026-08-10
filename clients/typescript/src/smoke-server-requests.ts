#!/usr/bin/env npx tsx
/** Server-request smoke over the shared WebSocket protocol client. */

import { spawn, type ChildProcess } from "node:child_process";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createInterface } from "node:readline";
import { ProtocolClient, expectSuccess } from "./protocol-client.js";
import { WebSocketProtocolTransport } from "./websocket-transport.js";

const BIN: string = process.env.ORBCODE_BIN ?? (() => {
  console.error("ORBCODE_BIN env var required");
  process.exit(1);
})();

function assert(condition: boolean, message: string): void {
  if (!condition) throw new Error(`FAIL: ${message}`);
  console.log(`  PASS: ${message}`);
}

interface ConnectionInfo {
  transport: "websocket";
  addr: string;
  auth_token: string;
}

async function readConnectionInfo(server: ChildProcess): Promise<ConnectionInfo> {
  return new Promise((resolve, reject) => {
    const lines = createInterface({ input: server.stdout! });
    const timer = setTimeout(() => reject(new Error("connection-info timeout")), 15_000);
    lines.on("line", (line: string) => {
      try {
        const info = JSON.parse(line.trim()) as Partial<ConnectionInfo>;
        if (info.transport === "websocket" && info.addr && info.auth_token) {
          clearTimeout(timer);
          lines.close();
          resolve(info as ConnectionInfo);
        }
      } catch {
        // Skip non-JSON lines.
      }
    });
  });
}

async function main(): Promise<void> {
  console.log("=== orbcode TypeScript server-request smoke test ===\n");

  const homeDir = mkdtempSync(join(tmpdir(), "orbcode-sreq-"));
  const cwdDir = mkdtempSync(join(tmpdir(), "orbcode-sreq-cwd-"));
  writeFileSync(
    join(homeDir, "settings.json"),
    JSON.stringify({
      env: {
        ANTHROPIC_API_KEY: "stub-key",
        ANTHROPIC_BASE_URL:
          "mock://anthropic?scenario=tool_use&key=bash&command=echo+test",
      },
    }),
  );

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

  let client: ProtocolClient | undefined;
  try {
    const info = await readConnectionInfo(server);
    const transport = await WebSocketProtocolTransport.connect(
      `ws://${info.addr}`,
      info.auth_token,
    );
    client = new ProtocolClient(transport, {
      requestIdPrefix: "server-request",
      requestTimeoutMs: 30_000,
      clientName: "ts-server-request-test",
    });

    console.log("1. Initialize");
    await client.initialize();
    assert(true, "initialize succeeded");

    console.log("\n2. Bootstrap");
    const bootstrap = await client.request("session/bootstrap");
    const data = expectSuccess(bootstrap.result, "session/bootstrap") as {
      session?: { session_id?: string };
      session_id?: string;
    };
    const sessionId = data.session?.session_id ?? data.session_id;
    assert(!!sessionId, `got session_id: ${sessionId}`);

    console.log("\n3. Submit turn (triggers permission/request)");
    const submitted = await client.request("turn/submit", {
      session_id: sessionId,
      prompt: "test prompt",
    });
    expectSuccess(submitted.result, "turn/submit");
    assert(true, "turn/submit accepted");

    console.log("\n4. Waiting for permission/request...");
    const deadline = Date.now() + 30_000;
    let permissionHandled = false;
    let turnFinished = false;
    while (Date.now() < deadline && !turnFinished) {
      await new Promise((resolve) => setTimeout(resolve, 100));
      for (const request of client.takeServerRequests()) {
        if (request.method === "permission/request") {
          await client.respondSuccess(request.id, { decision: "deny" });
          permissionHandled = true;
        }
      }
      turnFinished = client.getNotifications().some((notification) => {
        if (notification.method !== "stream/event") return false;
        const event = (notification.params as { event?: { event?: string } }).event;
        return event?.event === "turn_finished";
      });
    }

    assert(permissionHandled, "permission/request was received and denied");
    assert(turnFinished, "turn finished after permission deny");
    console.log("\n=== All server-request smoke tests passed ===");
  } finally {
    await client?.close();
    server.kill();
  }
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
