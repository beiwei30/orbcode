#!/usr/bin/env npx tsx

import type {
  ClientMessage,
  InitializeResult,
} from "../../typescript/src/generated/protocol.js";
import {
  ProtocolClient,
  type ProtocolTransport,
} from "../../typescript/src/protocol-client.js";
import { DESKTOP_INTERACTIVE_QUESTIONS } from "../src/app-controller.js";
import {
  DESKTOP_VERSION,
  SUPPORTED_PROTOCOL_VERSION,
  validateDesktopCompatibility,
} from "../src/compatibility.js";

const required = {
  protocol_version: SUPPORTED_PROTOCOL_VERSION,
  server_info: { name: "orbcode", version: DESKTOP_VERSION },
  capabilities: {
    streaming: true,
    stable_methods: [
      "session/bootstrap",
      "session/list",
      "session/rename",
      "session/fork",
      "turn/submit",
      "turn/cancel",
      "settings/output_style_options",
      "settings/active_output_style",
      "settings/set_output_style",
      "settings/sandbox",
      "settings/update_sandbox",
      "settings/is_locked",
    ],
    experimental_methods: [
      "session/acp_delete",
      "session/control_state",
      "session/set_permission_mode",
      "session/set_model",
      "session/set_effort",
    ],
    server_notification_methods: ["stream/event"],
    server_request_methods: [
      "permission/request",
      "mcp_trust/request",
      "ask_user/request",
    ],
  },
} satisfies InitializeResult;

validateDesktopCompatibility(required);
expectFailure(
  { ...required, protocol_version: "2.0" },
  "protocol mismatch fails before session work",
);
expectFailure(
  { ...required, server_info: { ...required.server_info, version: "9.9.9" } },
  "helper version mismatch fails before session work",
);
expectFailure(
  {
    ...required,
    capabilities: {
      ...required.capabilities,
      server_request_methods: ["permission/request"],
    },
  },
  "missing interactive lifecycle methods fail closed",
);
await desktopCapabilityDeclarationIsTruthful();

console.log("desktop compatibility smoke passed");

async function desktopCapabilityDeclarationIsTruthful(): Promise<void> {
  const sent: ClientMessage[] = [];
  const transport: ProtocolTransport = {
    async send(message) {
      sent.push(message);
      throw new Error("initialize recorded");
    },
    subscribe() {
      return () => {};
    },
    async close() {},
  };
  const client = new ProtocolClient(transport);
  try {
    await client.initialize({
      experimentalMethods: true,
      interactiveQuestions: DESKTOP_INTERACTIVE_QUESTIONS,
    });
  } catch (error) {
    if (!(error instanceof Error) || error.message !== "initialize recorded") throw error;
  } finally {
    await client.close();
  }

  const initialize = sent[0];
  if (initialize?.type !== "request" || initialize.method !== "initialize") {
    throw new Error("FAIL: desktop initialize request was not recorded");
  }
  const capabilities = (initialize.params as {
    capabilities?: { interactive_questions?: typeof DESKTOP_INTERACTIVE_QUESTIONS };
  }).capabilities?.interactive_questions;
  if (
    !capabilities?.single_select ||
    !capabilities.multi_select ||
    !capabilities.free_text ||
    capabilities.previews ||
    capabilities.annotations ||
    capabilities.special_outcomes
  ) {
    throw new Error(
      `FAIL: desktop declared unsupported AskUser behavior: ${JSON.stringify(capabilities)}`,
    );
  }
}

function expectFailure(result: InitializeResult, description: string): void {
  try {
    validateDesktopCompatibility(result);
  } catch {
    return;
  }
  throw new Error(`FAIL: ${description}`);
}
