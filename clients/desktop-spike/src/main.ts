import { invoke } from "@tauri-apps/api/core";
import type {
  ClientMessage,
  InitializeParams,
  InitializeResult,
  ServerMessage,
} from "../../typescript/src/generated/protocol";
import "./styles.css";

interface ProbeResult {
  response: string;
  child_pid: number;
  exit_code: number | null;
  termination: "graceful" | "killed_after_timeout";
  stderr_tail: string;
}

const button = document.querySelector<HTMLButtonElement>("#run-probe");
const status = document.querySelector<HTMLElement>("#status");
const output = document.querySelector<HTMLElement>("#result");

if (!button || !status || !output) {
  throw new Error("desktop spike markup is incomplete");
}

const initializeParams: InitializeParams = {
  protocol_version: "1.0",
  client_info: { name: "orbcode-desktop-spike", version: "0.0.0" },
  capabilities: { streaming: true, experimental_methods: false },
};

const initializeRequest: ClientMessage = {
  type: "request",
  id: "desktop-spike-init-1",
  method: "initialize",
  params: initializeParams,
};

button.addEventListener("click", async () => {
  button.disabled = true;
  status.textContent = "Running canonical initialize through host IPC…";
  output.textContent = "";

  try {
    const probe = await invoke<ProbeResult>("run_initialize_probe", {
      request: JSON.stringify(initializeRequest),
    });
    const message = JSON.parse(probe.response) as ServerMessage;

    if (message.type !== "response" || message.result.status !== "success") {
      throw new Error("probe child returned a non-success response");
    }

    const initialized = message.result.data as InitializeResult;
    if (initialized.protocol_version !== "1.0") {
      throw new Error(`unexpected protocol ${initialized.protocol_version}`);
    }
    if (probe.termination !== "graceful") {
      throw new Error("probe child needed forced termination");
    }

    status.textContent = `Protocol ${initialized.protocol_version}; child ${probe.child_pid} reaped.`;
    output.textContent = JSON.stringify(
      {
        server: initialized.server_info,
        exit_code: probe.exit_code,
        stderr_tail: probe.stderr_tail,
      },
      null,
      2,
    );
  } catch (error) {
    status.textContent = error instanceof Error ? error.message : String(error);
  } finally {
    button.disabled = false;
    button.focus();
  }
});
