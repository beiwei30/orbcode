#!/usr/bin/env npx tsx

import type { InitializeResult } from "../../typescript/src/generated/protocol.js";
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

console.log("desktop compatibility smoke passed");

function expectFailure(result: InitializeResult, description: string): void {
  try {
    validateDesktopCompatibility(result);
  } catch {
    return;
  }
  throw new Error(`FAIL: ${description}`);
}
