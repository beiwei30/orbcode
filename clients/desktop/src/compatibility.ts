import type { InitializeResult } from "../../typescript/src/generated/protocol.js";

export const DESKTOP_VERSION = "0.0.1";
export const SUPPORTED_PROTOCOL_VERSION = "1.0";

const REQUIRED_CLIENT_METHODS = [
  "session/bootstrap",
  "session/list",
  "session/rename",
  "session/fork",
  "session/acp_delete",
  "session/control_state",
  "session/set_permission_mode",
  "session/set_model",
  "session/set_effort",
  "turn/submit",
  "turn/cancel",
  "settings/output_style_options",
  "settings/active_output_style",
  "settings/set_output_style",
  "settings/sandbox",
  "settings/update_sandbox",
  "settings/is_locked",
] as const;

const REQUIRED_NOTIFICATION_METHODS = ["stream/event"] as const;
const REQUIRED_SERVER_REQUEST_METHODS = [
  "permission/request",
  "mcp_trust/request",
  "ask_user/request",
] as const;

export function validateDesktopCompatibility(result: InitializeResult): void {
  if (result.protocol_version !== SUPPORTED_PROTOCOL_VERSION) {
    throw new Error(
      `Incompatible Orbcode protocol ${result.protocol_version}; desktop requires ${SUPPORTED_PROTOCOL_VERSION}.`,
    );
  }
  if (result.server_info.version !== DESKTOP_VERSION) {
    throw new Error(
      `Incompatible Orbcode ${result.server_info.version}; desktop ${DESKTOP_VERSION} requires a matching helper or remote installation.`,
    );
  }
  if (!result.capabilities.streaming) {
    throw new Error("Incompatible Orbcode helper: streaming support is required.");
  }

  const clientMethods = new Set([
    ...result.capabilities.stable_methods,
    ...result.capabilities.experimental_methods,
  ]);
  requireMethods(clientMethods, REQUIRED_CLIENT_METHODS, "client methods");
  requireMethods(
    new Set(result.capabilities.server_notification_methods),
    REQUIRED_NOTIFICATION_METHODS,
    "notification methods",
  );
  requireMethods(
    new Set(result.capabilities.server_request_methods),
    REQUIRED_SERVER_REQUEST_METHODS,
    "interactive request methods",
  );
}

function requireMethods(
  actual: Set<string>,
  required: readonly string[],
  category: string,
): void {
  const missing = required.filter((method) => !actual.has(method));
  if (missing.length) {
    throw new Error(
      `Incompatible Orbcode helper: missing ${category}: ${missing.join(", ")}.`,
    );
  }
}
