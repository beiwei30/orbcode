#!/usr/bin/env npx tsx

import { spawn, type ChildProcess } from "node:child_process";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { isAbsolute, join, resolve } from "node:path";
import { createInterface } from "node:readline";
import { ProtocolClient } from "../../typescript/src/protocol-client.js";
import { WebSocketProtocolTransport } from "../../typescript/src/websocket-transport.js";
import { DesktopAppController } from "../src/app-controller.js";

const configuredBin = process.env.ORBCODE_BIN ?? (() => {
  throw new Error("ORBCODE_BIN env var required");
})();
const BIN = isAbsolute(configuredBin) ? configuredBin : resolve(process.cwd(), configuredBin);

interface ConnectionInfo {
  transport: "websocket";
  addr: string;
  auth_token: string;
}

interface TestServer {
  child: ChildProcess;
  info: ConnectionInfo;
  cwd: string;
}

async function main(): Promise<void> {
  console.log("=== desktop UI productization E2E ===");
  await toolTurn("deny", "local");
  await toolTurn("approve", "ssh");
  await askUserTurn();
  await mcpTrustTurn();
  await cancelTurn();
  await sessionMutationsBlockedDuringTurn();
  await reconnectAndResume();
  await disconnectWithPendingRequest();
  await remoteLossIsResumable();
  console.log("=== desktop UI productization E2E passed ===");
}

async function toolTurn(
  decision: "approve" | "deny",
  kind: "local" | "ssh",
): Promise<void> {
  const server = await spawnServer(
    "tool_then_success&key=bash&command=echo+desktop-ui",
  );
  const controller = new DesktopAppController();
  try {
    await controller.attach(
      await connection(server, 1, kind),
      undefined,
      kind === "ssh" ? "desktop-test.example" : "This Mac",
    );
    assert(controller.snapshot.connection?.kind === kind, `${kind} uses the shared UI controller`);
    await controller.submitTurn(`exercise ${decision} path`);
    try {
      await waitFor(
        () => controller.snapshot.requests[0]?.kind === "permission",
        `permission request for ${decision}`,
      );
    } catch (error) {
      console.error("controller state:", JSON.stringify(controller.snapshot, null, 2));
      throw error;
    }
    const request = controller.snapshot.requests[0];
    assert(request?.kind === "permission", "permission surface preserves typed request");
    await controller.respondPermission(request.envelopeId, { decision });
    await expectRejected(
      () => controller.respondPermission(request.envelopeId, { decision }),
      `${decision} permission cannot be answered twice`,
    );
    await waitFor(() => !controller.snapshot.streaming, `${decision} turn completion`);

    const blocks = controller.snapshot.bootstrap?.session.messages.flatMap(
      (message) => message.blocks ?? [],
    ) ?? [];
    assert(blocks.some((block) => block.kind === "tool_use"), `${decision} transcript keeps tool_use block`);
    assert(blocks.some((block) => block.kind === "tool_result"), `${decision} transcript keeps tool_result block`);
    assert(controller.snapshot.requests.length === 0, `${decision} request resolves exactly once`);
    console.log(`  PASS: finite tool turn (${decision})`);
  } finally {
    await controller.disconnect();
    await stopServer(server);
  }
}

async function askUserTurn(): Promise<void> {
  const input = encodeURIComponent(JSON.stringify({
    questions: [{
      id: "database",
      header: "Database",
      question: "Which database?",
      options: [{ id: "postgres", label: "PostgreSQL" }],
      allow_free_text: false,
    }],
  }));
  const server = await spawnServer(
    `tool_then_success&key=AskUserQuestion&input=${input}`,
  );
  const controller = new DesktopAppController();
  try {
    await controller.attach(await connection(server, 1));
    await controller.submitTurn("ask a typed question");
    await waitFor(
      () => controller.snapshot.requests[0]?.kind === "ask_user",
      "AskUser request",
    );
    const request = controller.snapshot.requests[0];
    assert(request?.kind === "ask_user", "AskUser surface preserves typed request");
    await controller.answerAskUser(request.envelopeId, {
      database: { kind: "selected", option_id: "mysql" },
    });
    await waitFor(
      () => {
        const retry = controller.snapshot.requests[0];
        return retry?.kind === "ask_user" &&
          retry.envelopeId === request.envelopeId &&
          retry.request.validation_error !== undefined &&
          !retry.responding;
      },
      "AskUser validation retry",
    );
    const retry = controller.snapshot.requests[0];
    assert(
      retry?.kind === "ask_user" && retry.envelopeId === request.envelopeId,
      "AskUser validation failure reuses and reopens the envelope ID",
    );
    assert(
      retry?.kind === "ask_user" && retry.request.validation_error?.code === "unknown_option",
      "AskUser retry preserves the server validation error",
    );
    await controller.answerAskUser(request.envelopeId, {
      database: { kind: "selected", option_id: "postgres" },
    });
    await expectRejected(
      () => controller.rejectAskUser(request.envelopeId),
      "AskUser request cannot be answered twice",
    );
    await waitFor(() => !controller.snapshot.streaming, "AskUser turn completion");
    assert(controller.snapshot.requests.length === 0, "AskUser request resolves exactly once");
    console.log("  PASS: AskUser typed response");
  } finally {
    await controller.disconnect();
    await stopServer(server);
  }
}

async function sessionMutationsBlockedDuringTurn(): Promise<void> {
  const server = await spawnServer("hang");
  const controller = new DesktopAppController();
  const activeConnection = await connection(server, 1);
  try {
    await controller.attach(activeConnection);
    const sessionId = controller.snapshot.bootstrap?.session.session_id;
    assert(!!sessionId, "active-turn mutation test has a session");

    await controller.submitTurn("keep this session active");
    const before = await activeConnection.client.sessionList();
    await expectRejected(
      () => controller.newSession(),
      "new session is rejected before an active-turn side effect",
    );
    await expectRejected(
      () => controller.forkSession(sessionId!),
      "fork is rejected before an active-turn side effect",
    );

    const after = await activeConnection.client.sessionList();
    assert(
      after.map((session) => session.session_id).join("\n") ===
        before.map((session) => session.session_id).join("\n"),
      "blocked new/fork operations create no server sessions",
    );
    assert(
      controller.snapshot.bootstrap?.session.session_id === sessionId,
      "active-turn events remain bound to their original session",
    );

    await controller.cancelTurn();
    await waitFor(() => !controller.snapshot.streaming, "mutation-test cancellation");
    console.log("  PASS: active turn blocks session mutation side effects");
  } finally {
    await controller.disconnect();
    await stopServer(server);
  }
}

async function mcpTrustTurn(): Promise<void> {
  const input = encodeURIComponent(JSON.stringify({ text: "desktop" }));
  const server = await spawnServer(
    `tool_then_success&key=mcp__docs__echo&input=${input}`,
    { withMcp: true },
  );
  const controller = new DesktopAppController();
  try {
    await controller.attach(await connection(server, 1));
    await controller.submitTurn("exercise MCP trust");
    await waitFor(
      () => controller.snapshot.requests[0]?.kind === "permission",
      "MCP tool permission request",
    );
    const permission = controller.snapshot.requests[0];
    assert(permission?.kind === "permission", "MCP tool uses the permission surface first");
    await controller.respondPermission(permission.envelopeId, { decision: "approve" });
    await expectRejected(
      () => controller.respondPermission(permission.envelopeId, { decision: "approve" }),
      "MCP tool permission cannot be answered twice",
    );

    await waitFor(
      () => controller.snapshot.requests[0]?.kind === "mcp_trust",
      "MCP trust request",
    );
    const trust = controller.snapshot.requests[0];
    assert(trust?.kind === "mcp_trust", "MCP trust surface preserves typed request");
    await controller.respondMcpTrust(trust.envelopeId, "trust");
    await expectRejected(
      () => controller.respondMcpTrust(trust.envelopeId, "trust"),
      "MCP trust request cannot be answered twice",
    );
    await waitFor(() => !controller.snapshot.streaming, "MCP trust turn completion");
    assert(controller.snapshot.requests.length === 0, "MCP trust request resolves exactly once");
    console.log("  PASS: MCP trust typed response");
  } finally {
    await controller.disconnect();
    await stopServer(server);
  }
}

async function cancelTurn(): Promise<void> {
  const server = await spawnServer("hang");
  const controller = new DesktopAppController();
  try {
    await controller.attach(await connection(server, 1));
    await controller.submitTurn("wait until cancelled");
    await waitFor(() => controller.snapshot.streaming, "streaming state");
    await controller.cancelTurn();
    await waitFor(() => !controller.snapshot.streaming, "cancelled terminal state");
    assert(controller.snapshot.subscriptionId === undefined, "cancel clears subscription state");
    console.log("  PASS: cancel");
  } finally {
    await controller.disconnect();
    await stopServer(server);
  }
}

async function reconnectAndResume(): Promise<void> {
  const server = await spawnServer("success");
  const controller = new DesktopAppController();
  try {
    await controller.attach(await connection(server, 1));
    const sessionId = controller.snapshot.bootstrap?.session.session_id;
    assert(!!sessionId, "initial session exists");
    await controller.submitTurn("persist this turn");
    await waitFor(() => !controller.snapshot.streaming, "initial turn completion");
    const messageCount = controller.snapshot.bootstrap?.session.messages.length ?? 0;
    await controller.disconnect();

    await controller.attach(await connection(server, 2), sessionId);
    assert(controller.snapshot.bootstrap?.session.session_id === sessionId, "reconnect resumes selected session");
    assert(
      (controller.snapshot.bootstrap?.session.messages.length ?? 0) >= messageCount,
      "reconnect replays transcript",
    );
    console.log("  PASS: reconnect and resume");
  } finally {
    await controller.disconnect();
    await stopServer(server);
  }
}

async function disconnectWithPendingRequest(): Promise<void> {
  const server = await spawnServer("tool_then_success&key=bash&command=echo+pending");
  const controller = new DesktopAppController();
  try {
    await controller.attach(await connection(server, 1));
    await controller.submitTurn("leave permission pending");
    await waitFor(() => controller.snapshot.requests.length === 1, "pending request");
    await controller.disconnect();
    assert(controller.snapshot.phase === "disconnected", "pending disconnect reaches disconnected state");
    assert(controller.snapshot.requests.length === 0, "pending disconnect clears renderer request");
    console.log("  PASS: pending-request disconnect");
  } finally {
    await controller.disconnect();
    await stopServer(server);
  }
}

async function remoteLossIsResumable(): Promise<void> {
  const server = await spawnServer("success");
  const controller = new DesktopAppController();
  try {
    await controller.attach(
      await connection(server, 1, "ssh"),
      undefined,
      "sleep-test.example",
    );
    const sessionId = controller.snapshot.bootstrap?.session.session_id;
    assert(!!sessionId, "remote session exists before connection loss");
    server.child.kill("SIGKILL");
    await waitFor(() => controller.snapshot.phase === "disconnected", "remote loss state");
    assert(
      controller.snapshot.bootstrap?.session.session_id === sessionId,
      "remote loss retains resumable session identity",
    );
    assert(
      controller.snapshot.diagnostic?.includes("closed") ||
        controller.snapshot.diagnostic?.includes("failed"),
      "remote loss reports a connection diagnostic",
    );
    console.log("  PASS: remote loss is resumable");
  } finally {
    await controller.disconnect();
    await stopServer(server);
  }
}

async function spawnServer(
  scenario: string,
  options: { withMcp?: boolean } = {},
): Promise<TestServer> {
  const home = mkdtempSync(join(tmpdir(), "orbcode-desktop-ui-home-"));
  const cwd = mkdtempSync(join(tmpdir(), "orbcode-desktop-ui-cwd-"));
  writeFileSync(
    join(home, "settings.json"),
    JSON.stringify({
      env: {
        ANTHROPIC_API_KEY: "desktop-ui-stub",
        ANTHROPIC_BASE_URL: `mock://anthropic?scenario=${scenario}`,
      },
    }),
  );
  if (options.withMcp) writeFakeMcpProject(cwd);
  const child = spawn(BIN, ["serve", "--websocket", "127.0.0.1:0"], {
    cwd,
    env: {
      ORBCODE_HOME: home,
      HOME: home,
      PATH: process.env.PATH ?? "",
      RUST_LOG: "warn",
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  const info = await readConnectionInfo(child);
  return { child, info, cwd };
}

function writeFakeMcpProject(cwd: string): void {
  const serverPath = join(cwd, "desktop-fake-mcp.mjs");
  writeFileSync(
    serverPath,
    `import { createInterface } from "node:readline";
const lines = createInterface({ input: process.stdin });
for await (const line of lines) {
  if (!line.trim()) continue;
  const request = JSON.parse(line);
  if (request.method === "notifications/initialized") continue;
  let result;
  if (request.method === "initialize") {
    result = { protocolVersion: "2024-11-05", capabilities: { tools: {} }, serverInfo: { name: "desktop-fake-mcp", version: "0.1.0" } };
  } else if (request.method === "tools/list") {
    result = { tools: [{ name: "echo", description: "Echo desktop input", inputSchema: { type: "object", properties: { text: { type: "string" } } } }] };
  } else if (request.method === "tools/call" && request.params?.name === "echo") {
    result = { content: [{ type: "text", text: \`echo: \${request.params.arguments?.text ?? ""}\` }], isError: false };
  } else {
    process.stdout.write(JSON.stringify({ jsonrpc: "2.0", id: request.id, error: { code: -32602, message: "unknown fake MCP request" } }) + "\\n");
    continue;
  }
  process.stdout.write(JSON.stringify({ jsonrpc: "2.0", id: request.id, result }) + "\\n");
}`,
  );
  writeFileSync(
    join(cwd, ".mcp.json"),
    JSON.stringify({
      mcpServers: {
        docs: { command: process.execPath, args: [serverPath] },
      },
    }),
  );
}

async function connection(
  server: TestServer,
  generation: number,
  kind: "local" | "ssh" = "local",
) {
  const transport = await WebSocketProtocolTransport.connect(
    `ws://${server.info.addr}`,
    server.info.auth_token,
  );
  return {
    status: {
      active: true,
      generation,
      kind,
      ...(kind === "local" ? { binary_source: "bundled" as const } : {}),
    },
    client: new ProtocolClient(transport, {
      requestIdPrefix: `desktop-ui-${generation}`,
      requestTimeoutMs: 30_000,
      clientName: "desktop-ui-e2e",
    }),
  };
}

function readConnectionInfo(child: ChildProcess): Promise<ConnectionInfo> {
  return new Promise((resolve, reject) => {
    const lines = createInterface({ input: child.stdout! });
    const timer = setTimeout(() => reject(new Error("connection info timeout")), 15_000);
    lines.on("line", (line) => {
      try {
        const value = JSON.parse(line) as Partial<ConnectionInfo>;
        if (value.transport === "websocket" && value.addr && value.auth_token) {
          clearTimeout(timer);
          lines.close();
          resolve(value as ConnectionInfo);
        }
      } catch {
        // Ignore non-protocol process output.
      }
    });
    child.once("exit", (code) => {
      clearTimeout(timer);
      reject(new Error(`orbcode exited before readiness (${code ?? "signal"})`));
    });
    child.once("error", (error) => {
      clearTimeout(timer);
      reject(error);
    });
  });
}

async function stopServer(server: TestServer): Promise<void> {
  if (server.child.exitCode !== null) return;
  server.child.kill("SIGTERM");
  await Promise.race([
    new Promise<void>((resolve) => server.child.once("exit", () => resolve())),
    new Promise<void>((resolve) => setTimeout(resolve, 3_000)),
  ]);
  if (server.child.exitCode === null) server.child.kill("SIGKILL");
}

async function waitFor(
  predicate: () => boolean,
  description: string,
  timeoutMs = 20_000,
): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (predicate()) return;
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
  throw new Error(`timeout waiting for ${description}`);
}

function assert(condition: boolean, message: string): asserts condition {
  if (!condition) throw new Error(`FAIL: ${message}`);
}

async function expectRejected(
  operation: () => Promise<void>,
  message: string,
): Promise<void> {
  try {
    await operation();
  } catch {
    return;
  }
  throw new Error(`FAIL: ${message}`);
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
