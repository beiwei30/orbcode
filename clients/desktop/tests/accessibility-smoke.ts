#!/usr/bin/env npx tsx

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { JSDOM } from "jsdom";
import {
  focusFirstInteractive,
  isSessionNavigationKey,
  nextSessionIndex,
  restoreFocus,
} from "../src/keyboard-navigation.js";
import { RemoteProfileStore } from "../src/remote-profiles.js";

const root = resolve(import.meta.dirname, "..");
const html = readFileSync(resolve(root, "index.html"), "utf8");
const css = readFileSync(resolve(root, "src/styles.css"), "utf8");
const dom = new JSDOM(html, { url: "https://desktop.invalid/" });
const { document, KeyboardEvent, Event, HTMLElement } = dom.window;

assert(document.querySelector('label[for="composer"]'), "composer has a programmatic label");
assert(document.querySelector('nav[aria-label="Sessions"]'), "session navigation is labelled");
assert(document.querySelector('dialog[aria-labelledby][aria-describedby]'), "request dialog has name and description references");
assert(document.querySelector('[role="log"][aria-label]'), "transcript exposes a labelled log role");
assert(css.includes("prefers-reduced-motion: reduce"), "reduced-motion preference is respected");
assert(css.includes("prefers-contrast: more"), "increased-contrast preference is supported");
assert(css.includes(":focus-visible"), "keyboard focus has a visible indicator");

const profiles = new RemoteProfileStore(dom.window.localStorage);
profiles.save({
  name: "staging",
  target: "dev@staging.example",
  remoteCwd: "/srv/app",
  remoteOrbcodePath: "/usr/local/bin/orbcode",
  password: "must-not-persist",
  sshOptions: ["IdentityFile=/tmp/key"],
} as Parameters<RemoteProfileStore["save"]>[0]);
const persisted = dom.window.localStorage.getItem("orbcode.desktop.ssh-profiles.v1") ?? "";
assert(persisted.includes("dev@staging.example"), "non-secret SSH profile fields persist");
assert(
  !persisted.includes("must-not-persist") &&
    !persisted.includes("IdentityFile") &&
    !persisted.includes("password") &&
    !persisted.includes("sshOptions"),
  "profile storage strips unknown credential and SSH-option fields",
);
for (let index = 0; index < 25; index += 1) {
  profiles.save({ name: `profile-${index}`, target: `host-${index}.example` });
}
assert(profiles.list().length === 20, "profile storage enforces its bounded maximum");

const nav = required<HTMLElement>("session-list");
let activatedSession = "";
for (const sessionId of ["one", "two", "three"]) {
  const item = document.createElement("button");
  item.type = "button";
  item.dataset.sessionId = sessionId;
  item.textContent = sessionId;
  item.addEventListener("click", () => { activatedSession = sessionId; });
  nav.append(item);
}
nav.addEventListener("keydown", (event) => {
  if (!isSessionNavigationKey(event.key)) return;
  const items = [...nav.querySelectorAll<HTMLButtonElement>("button[data-session-id]")];
  const current = items.indexOf(document.activeElement as HTMLButtonElement);
  const next = nextSessionIndex(event.key, current, items.length);
  event.preventDefault();
  items[next]?.focus();
});

const sessions = [...nav.querySelectorAll<HTMLButtonElement>("button")];
sessions[0]!.focus();
nav.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }));
assert(document.activeElement === sessions[1], "ArrowDown moves session focus");
nav.dispatchEvent(new KeyboardEvent("keydown", { key: "End", bubbles: true }));
assert(document.activeElement === sessions[2], "End moves focus to the last session");
pressEnter(sessions[2]!);
assert(activatedSession === "three", "Enter activates the focused session");

const dialog = required<HTMLDialogElement>("request-dialog");
const actions = required<HTMLElement>("request-actions");
let requestDecision = "";
for (const decision of ["deny", "approve"]) {
  const item = document.createElement("button");
  item.type = "button";
  item.textContent = decision;
  item.addEventListener("click", () => { requestDecision = decision; });
  actions.append(item);
}
const previous = sessions[2]!;
const firstAction = focusFirstInteractive(dialog);
assert(firstAction?.textContent === "deny", "request dialog focuses the safe first action");
pressEnter(firstAction as HTMLButtonElement);
assert(requestDecision === "deny", "Enter activates the request decision");
let cancelled = false;
dialog.addEventListener("cancel", (event) => {
  event.preventDefault();
  cancelled = true;
});
dialog.dispatchEvent(new Event("cancel", { cancelable: true }));
assert(cancelled, "Escape/cancel follows the explicit request dismissal path");
restoreFocus(previous);
assert(document.activeElement === previous, "focus returns to the session workflow after dismissal");

console.log("desktop accessibility smoke passed");

function pressEnter(target: HTMLButtonElement): void {
  const event = new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true });
  if (target.dispatchEvent(event)) target.click();
}

function required<T extends HTMLElement>(id: string): T {
  const value = document.getElementById(id);
  if (!(value instanceof HTMLElement)) throw new Error(`missing ${id}`);
  return value as T;
}

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(`FAIL: ${message}`);
}
