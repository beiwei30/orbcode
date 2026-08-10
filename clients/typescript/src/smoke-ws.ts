#!/usr/bin/env npx tsx
/** WebSocket smoke for the shared transport-independent protocol client. */

import { spawn, type ChildProcess } from "node:child_process";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createInterface } from "node:readline";
import { ProtocolClient, expectSuccess } from "./protocol-client.js";
import { WebSocketProtocolTransport } from "./websocket-transport.js";

const ALLOWED_ORIGIN = "https://reference-client.orbcode.test";

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
    const timer = setTimeout(
      () => reject(new Error("timeout reading connection info")),
      15_000,
    );
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
  console.log("=== orbcode TypeScript WebSocket smoke test ===\n");

  const homeDir = mkdtempSync(join(tmpdir(), "orbcode-ws-smoke-"));
  const cwdDir = mkdtempSync(join(tmpdir(), "orbcode-ws-cwd-"));
  writeFileSync(
    join(homeDir, "settings.json"),
    JSON.stringify({ env: { ANTHROPIC_API_KEY: "stub-key" } }),
  );

  const server = spawn(
    BIN,
    [
      "serve",
      "--websocket",
      "127.0.0.1:0",
      "--allowed-origin",
      ALLOWED_ORIGIN,
    ],
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

  let client: ProtocolClient | undefined;
  try {
    const info = await readConnectionInfo(server);
    console.log(`Server: ${info.transport} at ${info.addr}`);
    assert(!info.addr.endsWith(":0"), "addr has real port");

    const transport = await WebSocketProtocolTransport.connect(
      `ws://${info.addr}`,
      info.auth_token,
      { origin: ALLOWED_ORIGIN },
    );
    client = new ProtocolClient(transport, {
      requestIdPrefix: "ws",
      clientName: "ts-ws-client",
    });

    console.log("\n1. Initialize over WebSocket");
    const initialized = await client.initialize({ experimentalMethods: true });
    assert(initialized.capabilities.streaming, "streaming capability");
    assert(
      initialized.capabilities.experimental_methods.length > 0,
      "experimental methods visible (opt-in)",
    );

    console.log("\n2. Session list over WebSocket");
    const list = await client.request("session/list");
    assert(expectSuccess(list.result, "session/list") !== undefined, "session/list succeeded");

    console.log("\n3. Bootstrap over WebSocket");
    const bootstrap = await client.request("session/bootstrap");
    assert(
      expectSuccess(bootstrap.result, "session/bootstrap") !== undefined,
      "bootstrap succeeded",
    );

    console.log("\n=== All WebSocket smoke tests passed ===");
  } finally {
    await client?.close();
    server.kill();
  }
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
