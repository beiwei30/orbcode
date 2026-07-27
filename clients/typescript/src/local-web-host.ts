#!/usr/bin/env npx tsx
/**
 * Minimal local web host for orbcode.
 *
 * This is a real browser shell over the generated app-server protocol:
 * the Node host only owns process lifecycle and static assets, while the
 * browser connects directly to `orbcode serve --websocket`.
 */

import { spawn, type ChildProcess } from "node:child_process";
import { mkdtempSync, writeFileSync } from "node:fs";
import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
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

const BIN = process.env.ORBCODE_BIN;
const HOST = "127.0.0.1";
const SMOKE = process.argv.includes("--smoke");

if (!BIN) {
  console.error("ORBCODE_BIN env var required");
  process.exit(1);
}

interface ConnectionInfo {
  transport: "websocket";
  addr: string;
  auth_token: string;
}

interface HostConfig {
  websocket_url: string;
  auth_token: string;
  origin: string;
}

interface HostRuntime {
  origin: string;
  server: ReturnType<typeof createServer>;
  appServer: ChildProcess;
  config: HostConfig;
  close: () => Promise<void>;
}

async function main(): Promise<void> {
  const runtime = await startHost();

  if (SMOKE) {
    try {
      await runSmoke(runtime);
      console.log("\n=== All local web host smoke tests passed ===");
    } finally {
      await runtime.close();
    }
    return;
  }

  console.log(`orbcode local web host: ${runtime.origin}`);
  console.log("Press Ctrl-C to stop.");

  const shutdown = async () => {
    await runtime.close();
    process.exit(0);
  };
  process.on("SIGINT", shutdown);
  process.on("SIGTERM", shutdown);
}

async function startHost(): Promise<HostRuntime> {
  let currentConfig: HostConfig | undefined;
  const httpServer = createServer((req, res) => {
    handleRequest(req, res, () => {
      if (!currentConfig) throw new Error("host config not ready");
      return currentConfig;
    });
  });

  const origin = await listen(httpServer);
  currentConfig = await spawnAppServer(origin);

  return {
    origin,
    server: httpServer,
    appServer: appServerProcess,
    config: currentConfig,
    close: async () => {
      await closeHttp(httpServer);
      appServerProcess.kill();
      await waitForExit(appServerProcess, 2_000);
    },
  };
}

let appServerProcess: ChildProcess;

function listen(server: ReturnType<typeof createServer>): Promise<string> {
  return new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, HOST, () => {
      const addr = server.address();
      if (!addr || typeof addr === "string") {
        reject(new Error("HTTP server did not bind to a TCP address"));
        return;
      }
      resolve(`http://${HOST}:${addr.port}`);
    });
  });
}

async function spawnAppServer(origin: string): Promise<HostConfig> {
  const env = { ...process.env };
  if (SMOKE) {
    const homeDir = mkdtempSync(join(tmpdir(), "orbcode-web-host-"));
    const cwdDir = mkdtempSync(join(tmpdir(), "orbcode-web-host-cwd-"));
    writeFileSync(join(homeDir, "settings.json"), JSON.stringify({
      env: {
        ANTHROPIC_API_KEY: "stub-key",
        ANTHROPIC_BASE_URL: "mock://anthropic?scenario=tool_use&key=bash&command=echo+web",
      },
    }));
    env.ORBCODE_HOME = homeDir;
    env.HOME = homeDir;
    env.ORBCODE_WEB_HOST_CWD = cwdDir;
  }

  appServerProcess = spawn(
    BIN!,
    ["serve", "--websocket", `${HOST}:0`, "--allowed-origin", origin],
    {
      cwd: env.ORBCODE_WEB_HOST_CWD ?? process.cwd(),
      env: {
        ...env,
        PATH: process.env.PATH ?? "",
        RUST_LOG: env.RUST_LOG ?? "warn",
      },
      stdio: ["ignore", "pipe", "pipe"],
    },
  );

  const info = await readConnectionInfo(appServerProcess);
  return {
    websocket_url: `ws://${info.addr}`,
    auth_token: info.auth_token,
    origin,
  };
}

function readConnectionInfo(server: ChildProcess): Promise<ConnectionInfo> {
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
        // Skip non-JSON tracing lines.
      }
    });
    server.once("exit", (code) => {
      clearTimeout(timer);
      reject(new Error(`orbcode serve exited before ready: ${code}`));
    });
  });
}

function handleRequest(
  req: IncomingMessage,
  res: ServerResponse,
  config: () => HostConfig,
): void {
  const url = new URL(req.url ?? "/", "http://localhost");
  if (url.pathname === "/") {
    send(res, 200, "text/html; charset=utf-8", html());
  } else if (url.pathname === "/styles.css") {
    send(res, 200, "text/css; charset=utf-8", css());
  } else if (url.pathname === "/app.js") {
    send(res, 200, "text/javascript; charset=utf-8", browserJs());
  } else if (url.pathname === "/host-config.json") {
    send(res, 200, "application/json; charset=utf-8", JSON.stringify(config()));
  } else if (url.pathname === "/healthz") {
    send(res, 200, "application/json; charset=utf-8", JSON.stringify({ ok: true }));
  } else {
    send(res, 404, "text/plain; charset=utf-8", "not found");
  }
}

function send(res: ServerResponse, status: number, contentType: string, body: string): void {
  res.writeHead(status, {
    "content-type": contentType,
    "cache-control": "no-store",
  });
  res.end(body);
}

function html(): string {
  return `<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>orbcode Local Host</title>
  <link rel="stylesheet" href="/styles.css">
</head>
<body>
  <main class="shell">
    <header class="topbar">
      <div>
        <h1>orbcode Local Host</h1>
        <p id="subtitle">Protocol-only browser shell</p>
      </div>
      <div class="status" id="status">Starting</div>
    </header>

    <section class="grid">
      <section class="panel session">
        <h2>Session</h2>
        <dl>
          <div><dt>Server</dt><dd id="serverInfo">-</dd></div>
          <div><dt>Session</dt><dd id="sessionId">-</dd></div>
          <div><dt>Methods</dt><dd id="capabilities">-</dd></div>
        </dl>
        <div class="actions">
          <button id="connectBtn" type="button">Reconnect</button>
          <button id="bootstrapBtn" type="button">Bootstrap</button>
        </div>
      </section>

      <section class="panel composer">
        <h2>Turn</h2>
        <textarea id="prompt" rows="6">List the current session state.</textarea>
        <button id="submitBtn" type="button">Submit Turn</button>
      </section>
    </section>

    <section class="panel request" id="requestPanel" hidden>
      <div>
        <h2 id="requestTitle">Server Request</h2>
        <pre id="requestBody"></pre>
      </div>
      <div class="actions">
        <button id="approveBtn" type="button">Approve</button>
        <button id="denyBtn" type="button">Deny</button>
      </div>
    </section>

    <section class="panel stream">
      <h2>Stream</h2>
      <ol id="events"></ol>
    </section>
  </main>
  <script type="module" src="/app.js"></script>
</body>
</html>`;
}

function css(): string {
  return `:root {
  color-scheme: light;
  --bg: #f7f8fb;
  --surface: #ffffff;
  --surface-strong: #eef2f7;
  --text: #18202e;
  --muted: #667085;
  --border: #d7dde8;
  --accent: #0f766e;
  --accent-strong: #115e59;
  --danger: #b42318;
  --shadow: 0 16px 40px rgba(31, 41, 55, 0.09);
  font-family: Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
}

* { box-sizing: border-box; }
body {
  margin: 0;
  min-height: 100vh;
  background: var(--bg);
  color: var(--text);
}

button, textarea { font: inherit; }
button {
  border: 1px solid var(--accent);
  background: var(--accent);
  color: #fff;
  border-radius: 6px;
  padding: 9px 14px;
  font-size: 14px;
  font-weight: 650;
  cursor: pointer;
}
button:hover { background: var(--accent-strong); }
button:disabled {
  cursor: not-allowed;
  opacity: 0.55;
}

.shell {
  width: min(1180px, calc(100vw - 32px));
  margin: 0 auto;
  padding: 28px 0;
}
.topbar {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 20px;
  margin-bottom: 18px;
}
h1, h2, p { margin: 0; }
h1 {
  font-size: 28px;
  line-height: 1.1;
  letter-spacing: 0;
}
h2 {
  font-size: 14px;
  line-height: 1.25;
  letter-spacing: 0;
  text-transform: uppercase;
  color: var(--muted);
}
#subtitle {
  margin-top: 6px;
  color: var(--muted);
  font-size: 15px;
}
.status {
  min-width: 118px;
  text-align: center;
  border: 1px solid var(--border);
  border-radius: 6px;
  background: var(--surface);
  padding: 8px 12px;
  color: var(--muted);
  font-size: 13px;
  font-weight: 700;
}
.status.ready {
  border-color: rgba(15, 118, 110, 0.25);
  color: var(--accent-strong);
  background: #ecfdf3;
}
.status.error {
  border-color: rgba(180, 35, 24, 0.25);
  color: var(--danger);
  background: #fef3f2;
}
.grid {
  display: grid;
  grid-template-columns: minmax(0, 0.9fr) minmax(360px, 1.1fr);
  gap: 16px;
}
.panel {
  border: 1px solid var(--border);
  border-radius: 8px;
  background: var(--surface);
  box-shadow: var(--shadow);
  padding: 18px;
}
dl {
  display: grid;
  gap: 10px;
  margin: 16px 0;
}
dl div {
  display: grid;
  grid-template-columns: 80px minmax(0, 1fr);
  gap: 12px;
}
dt {
  color: var(--muted);
  font-size: 13px;
}
dd {
  margin: 0;
  min-width: 0;
  overflow-wrap: anywhere;
  font-size: 13px;
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
}
.actions {
  display: flex;
  flex-wrap: wrap;
  gap: 10px;
}
textarea {
  width: 100%;
  resize: vertical;
  min-height: 130px;
  margin: 16px 0 12px;
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: 12px;
  color: var(--text);
  background: #fbfcfe;
}
.request {
  display: grid;
  grid-template-columns: minmax(0, 1fr) auto;
  gap: 16px;
  align-items: start;
  margin-top: 16px;
  border-color: #f6c177;
  background: #fffbeb;
}
.request[hidden] { display: none; }
pre {
  margin: 12px 0 0;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
  font-size: 12px;
  line-height: 1.45;
}
.stream { margin-top: 16px; }
ol {
  list-style: none;
  padding: 0;
  margin: 16px 0 0;
  display: grid;
  gap: 8px;
}
li {
  border: 1px solid var(--border);
  border-radius: 6px;
  background: var(--surface-strong);
  padding: 10px 12px;
  font-size: 13px;
  line-height: 1.45;
  overflow-wrap: anywhere;
}

@media (max-width: 760px) {
  .shell { width: min(100vw - 24px, 1180px); padding: 18px 0; }
  .topbar, .request { display: grid; }
  .grid { grid-template-columns: 1fr; }
  h1 { font-size: 24px; }
  dl div { grid-template-columns: 1fr; gap: 4px; }
}`;
}

function browserJs(): string {
  return `const state = {
  ws: null,
  nextId: 0,
  pending: new Map(),
  sessionId: null,
  activeRequest: null,
};

const $ = (id) => document.getElementById(id);
const statusEl = $("status");
const eventsEl = $("events");
const submitBtn = $("submitBtn");
const connectBtn = $("connectBtn");
const bootstrapBtn = $("bootstrapBtn");
const requestPanel = $("requestPanel");

connectBtn.addEventListener("click", connect);
bootstrapBtn.addEventListener("click", bootstrap);
submitBtn.addEventListener("click", submitTurn);
$("approveBtn").addEventListener("click", () => respondToActiveRequest("approve"));
$("denyBtn").addEventListener("click", () => respondToActiveRequest("deny"));

setReady(false);
connect().catch((error) => fail(error));

async function connect() {
  setStatus("Connecting");
  setReady(false);
  const config = await fetch("/host-config.json", { cache: "no-store" }).then((r) => r.json());
  if (state.ws) state.ws.close();
  state.ws = new WebSocket(config.websocket_url);
  await new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error("WebSocket connect timeout")), 10000);
    state.ws.addEventListener("open", () => {
      state.ws.send(config.auth_token);
      clearTimeout(timer);
      resolve();
    }, { once: true });
    state.ws.addEventListener("error", () => reject(new Error("WebSocket connection failed")), { once: true });
  });
  state.ws.addEventListener("message", (event) => dispatch(JSON.parse(event.data)));
  state.ws.addEventListener("close", () => {
    setStatus("Disconnected", "error");
    setReady(false);
  });
  await initialize();
  await bootstrap();
}

async function initialize() {
  const response = await request("initialize", {
    protocol_version: "1.0",
    client_info: { name: "orbcode-local-web-host", version: "0.1.0" },
    capabilities: { streaming: true, experimental_methods: true },
  });
  const init = expectSuccess(response.result, "initialize");
  $("serverInfo").textContent = init.server_info.name + " " + init.server_info.version;
  $("capabilities").textContent = init.capabilities.stable_methods.length + " stable, "
    + init.capabilities.experimental_methods.length + " experimental";
}

async function bootstrap() {
  const response = await request("session/bootstrap");
  const data = expectSuccess(response.result, "session/bootstrap");
  state.sessionId = data.session && data.session.session_id ? data.session.session_id : null;
  $("sessionId").textContent = state.sessionId || "-";
  setStatus("Ready", "ready");
  setReady(true);
  appendEvent("bootstrap", data);
}

async function submitTurn() {
  if (!state.sessionId) return;
  const prompt = $("prompt").value.trim();
  if (!prompt) return;
  setReady(false);
  setStatus("Running");
  const response = await request("turn/submit", {
    session_id: state.sessionId,
    prompt,
  });
  expectSuccess(response.result, "turn/submit");
  appendEvent("turn/submit", { prompt });
}

function request(method, params) {
  const id = "web-" + state.nextId++;
  const msg = { type: "request", id, method, params };
  return new Promise((resolve, reject) => {
    state.pending.set(id, { resolve, reject });
    state.ws.send(JSON.stringify(msg));
    setTimeout(() => {
      if (state.pending.has(id)) {
        state.pending.delete(id);
        reject(new Error("timeout waiting for " + method));
      }
    }, 30000);
  });
}

function dispatch(msg) {
  if (msg.type === "response") {
    const pending = state.pending.get(msg.id);
    if (pending) {
      state.pending.delete(msg.id);
      pending.resolve(msg);
    }
    return;
  }
  if (msg.type === "notification") {
    appendEvent(msg.method, msg.params);
    const eventName = msg.params && msg.params.event && msg.params.event.event;
    if (eventName === "turn_finished" || eventName === "turn_cancelled") {
      setStatus("Ready", "ready");
      setReady(true);
    }
    return;
  }
  if (msg.type === "request") {
    state.activeRequest = msg;
    $("requestTitle").textContent = msg.method;
    $("requestBody").textContent = JSON.stringify(msg.params, null, 2);
    requestPanel.hidden = false;
    appendEvent("server-request", { method: msg.method });
  }
}

function respondToActiveRequest(decision) {
  if (!state.activeRequest) return;
  const requestMsg = state.activeRequest;
  const data = serverRequestResponseData(requestMsg.method, requestMsg.params, decision);
  state.ws.send(JSON.stringify({
    type: "response",
    id: requestMsg.id,
    result: { status: "success", data },
  }));
  appendEvent("server-request/respond", { method: requestMsg.method, data });
  state.activeRequest = null;
  requestPanel.hidden = true;
}

${serverRequestResponseData.toString()}
${requestIdFromParams.toString()}

function expectSuccess(result, context) {
  if (!result || result.status !== "success") {
    throw new Error(context + " failed: " + JSON.stringify(result));
  }
  return result.data;
}

function appendEvent(label, data) {
  const item = document.createElement("li");
  item.textContent = label + ": " + JSON.stringify(data);
  eventsEl.prepend(item);
  while (eventsEl.children.length > 80) eventsEl.lastElementChild.remove();
}

function setStatus(text, kind = "") {
  statusEl.textContent = text;
  statusEl.className = "status" + (kind ? " " + kind : "");
}

function setReady(ready) {
  submitBtn.disabled = !ready;
  bootstrapBtn.disabled = !state.ws || state.ws.readyState !== WebSocket.OPEN;
}

function fail(error) {
  console.error(error);
  setStatus("Error", "error");
  appendEvent("error", { message: String(error.message || error) });
}`;
}

async function runSmoke(runtime: HostRuntime): Promise<void> {
  assertServerRequestResponseShapes();

  assert(runtime.origin.startsWith(`http://${HOST}:`), "HTTP host binds localhost");
  const health = await fetch(`${runtime.origin}/healthz`);
  assert(health.ok, "health endpoint responds");
  const page = await fetch(`${runtime.origin}/`).then((res) => res.text());
  assert(page.includes("orbcode Local Host"), "serves browser shell");
  const appJs = await fetch(`${runtime.origin}/app.js`).then((res) => res.text());
  new Function(appJs);
  assert(appJs.includes("function serverRequestResponseData"), "browser script includes server-request response helper");
  assert(appJs.includes("function requestIdFromParams"), "browser script includes request-id helper");
  assertBrowserServerRequestResponseShapes(appJs);
  const config = await fetch(`${runtime.origin}/host-config.json`).then((res) => res.json()) as HostConfig;
  assert(config.origin === runtime.origin, "config exposes same origin");
  assert(config.websocket_url.startsWith(`ws://${HOST}:`), "config exposes localhost websocket");
  assert(config.auth_token.length > 0, "config exposes auth token");

  const client = new SmokeWsClient();
  await client.connect(config.websocket_url, config.auth_token, runtime.origin);
  const init = await client.initialize();
  assert(init.capabilities.streaming === true, "web client sees streaming capability");
  assert(init.capabilities.server_request_methods.includes("permission/request"), "web client sees permission requests");

  const bootstrap = await client.request("session/bootstrap");
  const bootstrapData = expectSuccess(bootstrap.result, "session/bootstrap") as { session?: { session_id?: string } };
  const sessionId = bootstrapData.session?.session_id;
  assert(!!sessionId, "web client bootstraps a session");

  const submit = await client.request("turn/submit", {
    session_id: sessionId,
    prompt: "trigger local web host permission request",
  });
  expectSuccess(submit.result, "turn/submit");
  const permission = await waitForPermissionRequest(client);
  client.respondSuccess(permission.id, { decision: "deny" });
  await waitForTurnFinished(client);
  assert(true, "web client handles permission request and stream completion");
  await client.close();
}

function assertServerRequestResponseShapes(): void {
  const permission = serverRequestResponseData(
    "permission/request",
    { request_id: "perm-1" },
    "deny",
  );
  assert(
    JSON.stringify(permission) === JSON.stringify({ decision: "deny" }),
    "permission response uses PermissionDecisionWire shape",
  );

  const mcpTrust = serverRequestResponseData(
    "mcp_trust/request",
    { request_id: "mcp-1" },
    "approve",
  );
  assert(
    JSON.stringify(mcpTrust) === JSON.stringify({ request_id: "mcp-1", decision: "trust" }),
    "MCP trust response includes request_id and trust decision",
  );

  const askUserApprove = serverRequestResponseData(
    "ask_user/request",
    { request_id: "ask-1" },
    "approve",
  );
  assert(
    JSON.stringify(askUserApprove) === JSON.stringify({ request_id: "ask-1", answer: "approved" }),
    "AskUser approve response includes request_id and answer",
  );

  const askUserDeny = serverRequestResponseData(
    "ask_user/request",
    { request_id: "ask-2" },
    "deny",
  );
  assert(
    JSON.stringify(askUserDeny) === JSON.stringify({ request_id: "ask-2", answer: null }),
    "AskUser deny response includes request_id and null answer",
  );
}

function assertBrowserServerRequestResponseShapes(appJs: string): void {
  const helperSource = [
    extractFunctionSource(appJs, "requestIdFromParams"),
    extractFunctionSource(appJs, "serverRequestResponseData"),
  ].join("\n");
  const run = new Function(
    `${helperSource}; return {
      mcp: serverRequestResponseData("mcp_trust/request", { request_id: "mcp-browser" }, "approve"),
      ask: serverRequestResponseData("ask_user/request", { request_id: "ask-browser" }, "deny")
    };`,
  ) as () => { mcp: unknown; ask: unknown };
  const result = run();
  assert(
    JSON.stringify(result.mcp) === JSON.stringify({ request_id: "mcp-browser", decision: "trust" }),
    "browser MCP trust helper executes without ReferenceError",
  );
  assert(
    JSON.stringify(result.ask) === JSON.stringify({ request_id: "ask-browser", answer: null }),
    "browser AskUser helper executes without ReferenceError",
  );
}

function extractFunctionSource(source: string, name: string): string {
  const start = source.indexOf(`function ${name}`);
  if (start < 0) throw new Error(`missing ${name} in browser script`);
  let depth = 0;
  let bodyStarted = false;
  for (let i = start; i < source.length; i += 1) {
    const char = source[i];
    if (char === "{") {
      depth += 1;
      bodyStarted = true;
    } else if (char === "}") {
      depth -= 1;
      if (bodyStarted && depth === 0) {
        return source.slice(start, i + 1);
      }
    }
  }
  throw new Error(`unterminated ${name} in browser script`);
}

function serverRequestResponseData(
  method: string,
  params: unknown,
  decision: "approve" | "deny",
): unknown {
  if (method === "mcp_trust/request") {
    return {
      request_id: requestIdFromParams(params),
      decision: decision === "approve" ? "trust" : "deny",
    };
  }
  if (method === "ask_user/request") {
    return {
      request_id: requestIdFromParams(params),
      answer: decision === "approve" ? "approved" : null,
    };
  }
  return { decision };
}

function requestIdFromParams(params: unknown): string {
  if (
    params
    && typeof params === "object"
    && "request_id" in params
    && typeof (params as { request_id?: unknown }).request_id === "string"
  ) {
    return (params as { request_id: string }).request_id;
  }
  if (
    params
    && typeof params === "object"
    && "server_id" in params
    && typeof (params as { server_id?: unknown }).server_id === "string"
  ) {
    return (params as { server_id: string }).server_id;
  }
  return "";
}

class SmokeWsClient {
  private ws!: WebSocket;
  private nextId = 0;
  private pending = new Map<
    string,
    { resolve: (msg: ServerResponseEnvelope & { type: "response" }) => void; reject: (err: Error) => void }
  >();
  private requests: Array<ServerRequestEnvelope & { type: "request" }> = [];
  private notifications: ServerMessage[] = [];

  connect(url: string, token: string, origin: string): Promise<void> {
    return new Promise((resolve, reject) => {
      const ws = new WebSocket(url, { headers: { Origin: origin } });
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
    const msg = JSON.parse(data.toString()) as ServerMessage;
    if (msg.type === "response") {
      const pending = this.pending.get(msg.id);
      if (pending) {
        this.pending.delete(msg.id);
        pending.resolve(msg);
      }
    } else if (msg.type === "request") {
      this.requests.push(msg);
    } else {
      this.notifications.push(msg);
    }
  }

  request(method: string, params?: unknown): Promise<ServerResponseEnvelope & { type: "response" }> {
    const id = `web-smoke-${this.nextId++}`;
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

  initialize(): Promise<InitializeResult> {
    return this.request("initialize", {
      protocol_version: "1.0",
      client_info: { name: "orbcode-local-web-host-smoke", version: "0.1.0" },
      capabilities: { streaming: true, experimental_methods: true },
    }).then((response) => expectSuccess(response.result, "initialize") as InitializeResult);
  }

  takeRequests(): Array<ServerRequestEnvelope & { type: "request" }> {
    const requests = this.requests;
    this.requests = [];
    return requests;
  }

  hasTurnFinished(): boolean {
    return this.notifications.some((msg) => {
      if (msg.type !== "notification" || msg.method !== "stream/event") return false;
      const event = (msg.params as { event?: { event?: string } }).event;
      return event?.event === "turn_finished";
    });
  }

  respondSuccess(id: string, data: unknown): void {
    this.ws.send(JSON.stringify({
      type: "response",
      id,
      result: { status: "success", data },
    }));
  }

  close(): Promise<void> {
    return new Promise((resolve) => {
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

async function waitForPermissionRequest(
  client: SmokeWsClient,
): Promise<ServerRequestEnvelope & { type: "request" }> {
  const deadline = Date.now() + 30_000;
  while (Date.now() < deadline) {
    for (const request of client.takeRequests()) {
      if (request.method === "permission/request") return request;
    }
    await sleep(100);
  }
  throw new Error("timed out waiting for permission/request");
}

async function waitForTurnFinished(client: SmokeWsClient): Promise<void> {
  const deadline = Date.now() + 30_000;
  while (Date.now() < deadline) {
    if (client.hasTurnFinished()) return;
    await sleep(100);
  }
  throw new Error("timed out waiting for turn_finished");
}

function assert(condition: boolean, msg: string): void {
  if (!condition) throw new Error(`FAIL: ${msg}`);
  console.log(`  PASS: ${msg}`);
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function closeHttp(server: ReturnType<typeof createServer>): Promise<void> {
  return new Promise((resolve) => server.close(() => resolve()));
}

function waitForExit(child: ChildProcess, timeoutMs: number): Promise<void> {
  return new Promise((resolve) => {
    const timer = setTimeout(resolve, timeoutMs);
    child.once("exit", () => {
      clearTimeout(timer);
      resolve();
    });
  });
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
