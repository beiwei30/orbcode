import type {
  AskUserAnswerValue,
  AskUserQuestionSpec,
  PermissionMode,
  TranscriptBlock,
  TranscriptMessage,
} from "../../typescript/src/generated/protocol.js";
import { DesktopAppController, type DesktopAppState, type InteractiveRequest } from "./app-controller.js";
import {
  connectLocal,
  connectSsh,
  type DesktopConnection,
} from "./desktop-transport.js";
import {
  focusFirstInteractive,
  isSessionNavigationKey,
  nextSessionIndex,
  restoreFocus,
} from "./keyboard-navigation.js";
import { RemoteProfileStore } from "./remote-profiles.js";

const controller = new DesktopAppController();
let reconnectFactory: (() => Promise<DesktopConnection>) | undefined;
let reconnectOriginLabel: string | undefined;
let lastFocusedElement: HTMLElement | null = null;
let renderedRequestId: string | undefined;

const status = element("status");
const diagnostic = element("diagnostic");
const warning = element("development-warning");
const originBadge = element("origin-badge");
const connectionPanel = element("connection-panel");
const workspace = element("workspace");
const sessionList = element("session-list");
const transcript = element("transcript");
const streamStatus = element("stream-status");
const composer = textarea("composer");
const disconnectButton = button("disconnect");
const reconnectButton = button("reconnect");
const requestDialog = dialog("request-dialog");
const profileStore = new RemoteProfileStore(window.localStorage);
let remoteProfiles = profileStore.list();
renderRemoteProfiles();

form("local-form").addEventListener("submit", (event) => {
  event.preventDefault();
  const cwd = input("local-cwd").value.trim();
  reconnectFactory = () => connectLocal(cwd ? { cwd } : {});
  reconnectOriginLabel = "This Mac";
  void connect(reconnectFactory, undefined, reconnectOriginLabel);
});

form("ssh-form").addEventListener("submit", (event) => {
  event.preventDefault();
  const target = input("ssh-target").value.trim();
  const remoteCwd = input("ssh-cwd").value.trim();
  const remoteOrbcodePath = input("ssh-orbcode-path").value.trim();
  if (!target) return;
  reconnectFactory = () =>
    connectSsh({
      target,
      ...(remoteCwd ? { remote_cwd: remoteCwd } : {}),
      ...(remoteOrbcodePath ? { remote_orbcode_path: remoteOrbcodePath } : {}),
    });
  reconnectOriginLabel = target;
  void connect(reconnectFactory, undefined, reconnectOriginLabel);
});

select("ssh-profile").addEventListener("change", (event) => {
  const name = (event.currentTarget as HTMLSelectElement).value;
  const profile = remoteProfiles.find((item) => item.name === name);
  button("delete-ssh-profile").disabled = !profile;
  if (!profile) return;
  input("ssh-target").value = profile.target;
  input("ssh-cwd").value = profile.remoteCwd ?? "";
  input("ssh-orbcode-path").value = profile.remoteOrbcodePath ?? "";
});
button("save-ssh-profile").addEventListener("click", () => {
  const target = input("ssh-target").value.trim();
  if (!target) {
    diagnostic.textContent = "Enter an SSH target before saving a profile.";
    input("ssh-target").focus();
    return;
  }
  const name = window.prompt("Profile name", target)?.trim();
  if (!name) return;
  remoteProfiles = profileStore.save({
    name,
    target,
    ...(input("ssh-cwd").value.trim() ? { remoteCwd: input("ssh-cwd").value.trim() } : {}),
    ...(input("ssh-orbcode-path").value.trim()
      ? { remoteOrbcodePath: input("ssh-orbcode-path").value.trim() }
      : {}),
  });
  renderRemoteProfiles(name);
});
button("delete-ssh-profile").addEventListener("click", () => {
  const name = select("ssh-profile").value;
  if (!name) return;
  remoteProfiles = profileStore.remove(name);
  renderRemoteProfiles();
});

disconnectButton.addEventListener("click", () => void controller.disconnect());
reconnectButton.addEventListener("click", () => {
  if (reconnectFactory) {
    void connect(
      reconnectFactory,
      controller.snapshot.bootstrap?.session.session_id,
      reconnectOriginLabel,
    );
  }
});
button("new-session").addEventListener("click", () => run(() => controller.newSession()));
button("rename-session").addEventListener("click", () => {
  const session = controller.snapshot.bootstrap?.session;
  if (!session) return;
  const title = window.prompt("Rename session", session.custom_title ?? session.title ?? "");
  if (title?.trim()) void run(() => controller.renameSession(session.session_id, title.trim()));
});
button("fork-session").addEventListener("click", () => {
  const sessionId = controller.snapshot.bootstrap?.session.session_id;
  if (sessionId) void run(() => controller.forkSession(sessionId));
});
button("delete-session").addEventListener("click", () => {
  const session = controller.snapshot.bootstrap?.session;
  if (!session) return;
  if (window.confirm(`Delete “${session.custom_title ?? session.title ?? "this session"}”? This cannot be undone.`)) {
    void run(() => controller.deleteSession(session.session_id));
  }
});

form("composer-form").addEventListener("submit", (event) => {
  event.preventDefault();
  const prompt = composer.value;
  if (!prompt.trim()) return;
  void run(async () => {
    await controller.submitTurn(prompt);
    composer.value = "";
  });
});
composer.addEventListener("keydown", (event) => {
  if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
    event.preventDefault();
    form("composer-form").requestSubmit();
  }
});
button("cancel-turn").addEventListener("click", () => run(() => controller.cancelTurn()));

select("model-control").addEventListener("change", (event) => {
  const value = (event.currentTarget as HTMLSelectElement).value;
  void run(() => controller.setModel(value || null));
});
select("effort-control").addEventListener("change", (event) => {
  const value = (event.currentTarget as HTMLSelectElement).value;
  void run(() => controller.setEffort(value || null));
});
select("permission-control").addEventListener("change", (event) => {
  const value = (event.currentTarget as HTMLSelectElement).value as PermissionMode;
  void run(() => controller.setPermissionMode(value));
});
select("output-style-control").addEventListener("change", (event) => {
  void run(() => controller.setOutputStyle((event.currentTarget as HTMLSelectElement).value));
});
input("sandbox-control").addEventListener("change", (event) => {
  void run(() => controller.setSandboxEnabled((event.currentTarget as HTMLInputElement).checked));
});

sessionList.addEventListener("keydown", (event) => {
  if (!isSessionNavigationKey(event.key)) return;
  const items = [...sessionList.querySelectorAll<HTMLButtonElement>("button[data-session-id]")];
  if (!items.length) return;
  const current = Math.max(0, items.indexOf(document.activeElement as HTMLButtonElement));
  const next = nextSessionIndex(event.key, current, items.length);
  event.preventDefault();
  items[next]?.focus();
});

requestDialog.addEventListener("cancel", (event) => {
  event.preventDefault();
  const request = controller.snapshot.requests[0];
  if (request) void dismissRequest(request);
});

controller.subscribe(render);

async function connect(
  factory: () => Promise<DesktopConnection>,
  preferredSessionId?: string,
  originLabel?: string,
): Promise<void> {
  try {
    const connection = await factory();
    await controller.attach(connection, preferredSessionId, originLabel);
  } catch {
    // The controller exposes the actionable protocol/host diagnostic.
  }
}

function render(state: Readonly<DesktopAppState>): void {
  const connected = state.phase === "connected";
  const busy = state.phase === "connecting" || state.phase === "reconnecting";
  status.textContent = busy
    ? state.phase === "reconnecting" ? "Reconnecting…" : "Connecting…"
    : connected ? "Connected" : "Disconnected";
  connectionPanel.hidden = connected || busy;
  workspace.hidden = !connected;
  disconnectButton.disabled = state.phase === "disconnected";
  reconnectButton.hidden = !reconnectFactory || connected || busy;
  diagnostic.textContent = state.diagnostic ?? "";
  warning.hidden = state.connection?.binarySource !== "development_override";
  originBadge.textContent = state.connection
    ? `${state.connection.kind === "ssh" ? "Remote · SSH" : "Local"} · ${state.connection.label} · generation ${state.connection.generation}`
    : "Not connected";

  if (connected) {
    renderSessions(state);
    renderConversation(state);
    renderControls(state);
  }
  renderRequest(state.requests[0]);
}

function renderSessions(state: Readonly<DesktopAppState>): void {
  const activeId = state.bootstrap?.session.session_id;
  sessionList.replaceChildren(
    ...state.sessions.map((session) => {
      const item = document.createElement("button");
      item.type = "button";
      item.className = "session-item";
      item.dataset.sessionId = session.session_id;
      item.setAttribute("aria-current", session.session_id === activeId ? "page" : "false");
      item.tabIndex = session.session_id === activeId ? 0 : -1;
      const title = document.createElement("strong");
      title.textContent = session.custom_title ?? session.title ?? "Untitled session";
      const detail = document.createElement("span");
      detail.textContent = `${session.message_count} messages`;
      item.append(title, detail);
      item.addEventListener("click", () => run(() => controller.loadSession(session.session_id)));
      return item;
    }),
  );
}

function renderConversation(state: Readonly<DesktopAppState>): void {
  const bootstrap = state.bootstrap;
  if (!bootstrap) return;
  const session = bootstrap.session;
  element("conversation-heading").textContent = session.custom_title ?? session.title ?? "New session";
  element("conversation-cwd").textContent = bootstrap.cwd;
  element("conversation-origin").textContent = state.connection?.kind === "ssh"
    ? `Remote workspace · ${state.connection.label}`
    : `Local workspace · ${state.connection?.label ?? "This Mac"}`;

  const nodes = session.messages.map(renderMessage);
  if (state.thinkingDraft) nodes.push(renderDraft("Thinking", state.thinkingDraft, "thinking"));
  if (state.assistantDraft) nodes.push(renderDraft("Assistant", state.assistantDraft, "assistant"));
  transcript.replaceChildren(...nodes);
  if (state.streaming) transcript.scrollTop = transcript.scrollHeight;

  button("cancel-turn").hidden = !state.streaming;
  button("submit-turn").disabled = state.streaming;
  composer.disabled = state.streaming;
  streamStatus.textContent = state.streaming ? "Orbcode is responding" : "Orbcode is ready";
}

function renderMessage(message: TranscriptMessage): HTMLElement {
  const article = document.createElement("article");
  article.className = `message message-${message.role}`;
  article.setAttribute("aria-label", `${capitalize(message.role)} message`);
  const label = document.createElement("p");
  label.className = "message-label";
  label.textContent = capitalize(message.role);
  article.append(label);
  const blocks = message.blocks?.length ? message.blocks : [{ kind: "text", text: message.content } as TranscriptBlock];
  for (const block of blocks) article.append(renderBlock(block));
  return article;
}

function renderBlock(block: TranscriptBlock): HTMLElement {
  if (block.kind === "text") return textBlock(block.text);
  const card = document.createElement(block.kind === "thinking" ? "details" : "section");
  card.className = `content-block block-${block.kind}`;
  if (block.kind === "thinking") {
    const summary = document.createElement("summary");
    summary.textContent = "Thinking";
    card.append(summary, textBlock(block.text));
  } else if (block.kind === "tool_use") {
    const heading = document.createElement("h3");
    heading.textContent = `Tool · ${block.name}`;
    const input = document.createElement("pre");
    input.textContent = block.input;
    card.append(heading, input);
  } else {
    const heading = document.createElement("h3");
    heading.textContent = block.is_error ? "Tool result · error" : "Tool result";
    const content = document.createElement("pre");
    content.textContent = block.content;
    card.append(heading, content);
    if (block.metadata) {
      const metadata = document.createElement("details");
      const summary = document.createElement("summary");
      summary.textContent = "Metadata";
      const value = document.createElement("pre");
      value.textContent = block.metadata;
      metadata.append(summary, value);
      card.append(metadata);
    }
  }
  return card;
}

function renderDraft(labelText: string, content: string, kind: string): HTMLElement {
  const article = document.createElement("article");
  article.className = `message message-${kind} is-streaming`;
  const label = document.createElement("p");
  label.className = "message-label";
  label.textContent = `${labelText} · streaming`;
  article.append(label, textBlock(content));
  return article;
}

function textBlock(text: string): HTMLElement {
  const paragraph = document.createElement("p");
  paragraph.className = "message-text";
  paragraph.textContent = text;
  return paragraph;
}

function renderControls(state: Readonly<DesktopAppState>): void {
  const controls = state.controls;
  if (!controls) return;
  fillSelect(
    select("model-control"),
    controls.model_options.map((option) => ({ value: option.value ?? "", label: option.label, selected: option.current })),
    state.locks.model,
  );
  fillSelect(
    select("effort-control"),
    [{ value: "", label: "Provider default", selected: !controls.effort_level }].concat(
      controls.effort_options.map((effort) => ({ value: effort, label: capitalize(effort), selected: effort === controls.effort_level })),
    ),
    state.locks.effort,
  );
  const permissionModes: PermissionMode[] = ["default", "acceptEdits", "dontAsk", "plan", "auto", "bypassPermissions"];
  fillSelect(
    select("permission-control"),
    permissionModes.map((mode) => ({ value: mode, label: mode, selected: mode === controls.permission_mode })),
    state.locks.permissions,
  );
  fillSelect(
    select("output-style-control"),
    state.outputStyles.map((option) => ({ value: option.value, label: option.label, selected: option.value === state.activeOutputStyle })),
    state.locks.outputStyle,
  );
  const sandbox = input("sandbox-control");
  sandbox.checked = state.sandbox?.enabled ?? false;
  sandbox.disabled = state.locks.sandbox;
  const locked = Object.entries(state.locks).filter(([, value]) => value).map(([key]) => key);
  element("settings-lock-note").textContent = locked.length ? `Managed settings lock: ${locked.join(", ")}` : "";
}

function fillSelect(
  target: HTMLSelectElement,
  options: { value: string; label: string; selected: boolean }[],
  disabled: boolean,
): void {
  target.replaceChildren(
    ...options.map((option) => {
      const element = document.createElement("option");
      element.value = option.value;
      element.textContent = option.label;
      element.selected = option.selected;
      return element;
    }),
  );
  target.disabled = disabled;
}

function renderRequest(request?: InteractiveRequest): void {
  if (!request) {
    if (requestDialog.open) requestDialog.close();
    renderedRequestId = undefined;
    restoreFocus(lastFocusedElement);
    lastFocusedElement = null;
    return;
  }
  if (request.envelopeId === renderedRequestId) {
    setRequestControlsDisabled(request.responding);
    return;
  }
  renderedRequestId = request.envelopeId;
  lastFocusedElement = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  element("request-fields").replaceChildren();
  const actions = element("request-actions");
  actions.replaceChildren();

  if (request.kind === "permission") {
    element("request-title").textContent = `Allow ${request.request.tool_name}?`;
    element("request-origin").textContent = `${visibleConnectionOrigin()} · session ${request.request.session_id} · tool ${request.request.tool_use_id}`;
    element("request-detail").replaceChildren(textBlock(request.request.tool_input || "No tool input supplied."));
    actions.append(
      actionButton("Deny", "danger", () => controller.respondPermission(request.envelopeId, { decision: "deny" })),
      actionButton("Allow once", "", () => controller.respondPermission(request.envelopeId, { decision: "approve" })),
    );
  } else if (request.kind === "mcp_trust") {
    element("request-title").textContent = `Trust MCP server ${request.request.server_id}?`;
    element("request-origin").textContent = `${visibleConnectionOrigin()} · session ${request.request.session_id} · tool ${request.request.tool_name}`;
    element("request-detail").replaceChildren(textBlock("Trust lets this server participate in the current tool workflow; permission rules still apply."));
    actions.append(
      actionButton("Deny", "danger", () => controller.respondMcpTrust(request.envelopeId, "deny")),
      actionButton("Trust server", "", () => controller.respondMcpTrust(request.envelopeId, "trust")),
    );
  } else {
    element("request-title").textContent = "Orbcode needs your input";
    element("request-origin").textContent = `${visibleConnectionOrigin()} · session ${request.request.session_id || "current"} · tool ${request.request.tool_use_id || "AskUserQuestion"}`;
    element("request-detail").replaceChildren();
    const questions = canonicalQuestions(request.request);
    element("request-fields").append(...questions.map(renderQuestion));
    actions.append(
      actionButton("Reject", "danger", () => controller.rejectAskUser(request.envelopeId)),
      actionButton("Submit answer", "", () => controller.answerAskUser(request.envelopeId, collectAnswers(questions))),
    );
  }
  setRequestControlsDisabled(request.responding);
  if (!requestDialog.open) requestDialog.showModal();
  focusFirstInteractive(requestDialog);
}

function setRequestControlsDisabled(disabled: boolean): void {
  for (const control of requestDialog.querySelectorAll<
    HTMLButtonElement | HTMLInputElement | HTMLSelectElement | HTMLTextAreaElement
  >("button, input, select, textarea")) {
    control.disabled = disabled;
  }
}

function canonicalQuestions(request: Extract<InteractiveRequest, { kind: "ask_user" }>['request']): AskUserQuestionSpec[] {
  if (request.questions?.length) return request.questions;
  return [{
    id: "question-1",
    header: "Question",
    question: request.question ?? "Please choose an answer",
    options: (request.options ?? []).map((label, index) => ({ id: `option-${index + 1}`, label })),
    allow_free_text: !(request.options?.length),
  }];
}

function renderQuestion(question: AskUserQuestionSpec): HTMLElement {
  const fieldset = document.createElement("fieldset");
  fieldset.dataset.questionId = question.id;
  fieldset.dataset.multiSelect = String(question.multi_select ?? false);
  fieldset.dataset.freeText = String(question.allow_free_text ?? !question.options?.length);
  const legend = document.createElement("legend");
  legend.textContent = question.question;
  fieldset.append(legend);
  for (const option of question.options ?? []) {
    const label = document.createElement("label");
    label.className = "choice-row";
    const choice = document.createElement("input");
    choice.type = question.multi_select ? "checkbox" : "radio";
    choice.name = `question-${question.id}`;
    choice.value = option.id;
    label.append(choice, document.createTextNode(option.label));
    fieldset.append(label);
    if (option.description) {
      const description = document.createElement("p");
      description.className = "choice-description";
      description.textContent = option.description;
      fieldset.append(description);
    }
  }
  if (question.allow_free_text ?? !question.options?.length) {
    const label = document.createElement("label");
    label.textContent = "Your answer";
    const freeText = document.createElement("input");
    freeText.name = `text-${question.id}`;
    freeText.dataset.freeText = "true";
    label.append(freeText);
    fieldset.append(label);
  }
  return fieldset;
}

function collectAnswers(questions: AskUserQuestionSpec[]): Record<string, AskUserAnswerValue> {
  const answers: Record<string, AskUserAnswerValue> = {};
  for (const question of questions) {
    const fieldset = requestDialog.querySelector<HTMLFieldSetElement>(`fieldset[data-question-id="${CSS.escape(question.id)}"]`);
    if (!fieldset) continue;
    const text = fieldset.querySelector<HTMLInputElement>('input[data-free-text="true"]')?.value.trim();
    if (text) {
      answers[question.id] = { kind: "text", text };
      continue;
    }
    const selected = [...fieldset.querySelectorAll<HTMLInputElement>('input:checked')].map((item) => item.value);
    answers[question.id] = question.multi_select
      ? { kind: "selected_many", option_ids: selected }
      : { kind: "selected", option_id: selected[0] ?? "" };
  }
  return answers;
}

function actionButton(
  label: string,
  className: string,
  action: () => Promise<void>,
): HTMLButtonElement {
  const item = document.createElement("button");
  item.type = "button";
  item.textContent = label;
  item.className = className;
  item.addEventListener("click", () => run(action));
  return item;
}

function dismissRequest(request: InteractiveRequest): Promise<void> {
  if (request.kind === "permission") return controller.respondPermission(request.envelopeId, { decision: "deny" });
  if (request.kind === "mcp_trust") return controller.respondMcpTrust(request.envelopeId, "deny");
  return controller.rejectAskUser(request.envelopeId);
}

function renderRemoteProfiles(selectedName = ""): void {
  const target = select("ssh-profile");
  target.replaceChildren(
    option("", "No saved profile"),
    ...remoteProfiles.map((profile) => option(profile.name, profile.name)),
  );
  target.value = selectedName;
  button("delete-ssh-profile").disabled = !selectedName;
}

function option(value: string, label: string): HTMLOptionElement {
  const item = document.createElement("option");
  item.value = value;
  item.textContent = label;
  return item;
}

function visibleConnectionOrigin(): string {
  const connection = controller.snapshot.connection;
  if (!connection) return "Disconnected origin";
  const origin = connection.kind === "ssh"
    ? `Remote SSH ${connection.label}`
    : `Local ${connection.label}`;
  const cwd = controller.snapshot.bootstrap?.cwd;
  return cwd ? `${origin} · cwd ${cwd}` : origin;
}

async function run(action: () => Promise<void>): Promise<void> {
  try {
    await action();
  } catch (error) {
    diagnostic.textContent = error instanceof Error ? error.message : "Operation failed";
  }
}

function capitalize(value: string): string {
  return value ? value[0]!.toUpperCase() + value.slice(1) : value;
}

function element(id: string): HTMLElement {
  const value = document.getElementById(id);
  if (!value) throw new Error(`missing element ${id}`);
  return value;
}

function input(id: string): HTMLInputElement {
  const value = element(id);
  if (!(value instanceof HTMLInputElement)) throw new Error(`${id} is not an input`);
  return value;
}

function textarea(id: string): HTMLTextAreaElement {
  const value = element(id);
  if (!(value instanceof HTMLTextAreaElement)) throw new Error(`${id} is not a textarea`);
  return value;
}

function button(id: string): HTMLButtonElement {
  const value = element(id);
  if (!(value instanceof HTMLButtonElement)) throw new Error(`${id} is not a button`);
  return value;
}

function select(id: string): HTMLSelectElement {
  const value = element(id);
  if (!(value instanceof HTMLSelectElement)) throw new Error(`${id} is not a select`);
  return value;
}

function form(id: string): HTMLFormElement {
  const value = element(id);
  if (!(value instanceof HTMLFormElement)) throw new Error(`${id} is not a form`);
  return value;
}

function dialog(id: string): HTMLDialogElement {
  const value = element(id);
  if (!(value instanceof HTMLDialogElement)) throw new Error(`${id} is not a dialog`);
  return value;
}
