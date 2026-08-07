/** Tauri IPC transport for the shared generated-protocol client. */

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  ClientMessage,
  ServerMessage,
} from "../../typescript/src/generated/protocol.js";
import {
  ProtocolClient,
  type ProtocolTransport,
  type ProtocolTransportEvent,
} from "../../typescript/src/protocol-client.js";

const PROTOCOL_EVENT = "orbcode://protocol";
const CONNECTION_EXIT_EVENT = "orbcode://connection-exit";

export type ConnectionKind = "local" | "ssh";
export type BinarySource = "bundled" | "development_override";

export interface ConnectionStatus {
  active: boolean;
  generation: number;
  kind?: ConnectionKind;
  binary_source?: BinarySource;
  child_pid?: number;
}

export interface LocalConnectionInput {
  cwd?: string;
}

export interface SshConnectionInput {
  target: string;
  remote_cwd?: string;
  remote_orbcode_path?: string;
  options?: string[];
}

interface HostProtocolEvent {
  generation: number;
  message: ServerMessage;
}

interface ProtocolReply {
  generation: number;
  message?: ServerMessage;
}

export interface HostExitDiagnostics {
  pid: number;
  reason: string;
  termination: string;
  success: boolean;
  exit_code: number | null;
  signal: number | null;
}

interface HostConnectionExitEvent {
  generation: number;
  diagnostics: HostExitDiagnostics;
}

export interface DesktopConnection {
  status: ConnectionStatus;
  client: ProtocolClient;
}

export class DesktopIpcTransport implements ProtocolTransport {
  private readonly listeners = new Set<
    (event: ProtocolTransportEvent) => void
  >();
  private unlisten?: UnlistenFn;
  private unlistenExit?: UnlistenFn;
  private closed = false;
  private disconnectSent = false;

  private constructor(readonly generation: number) {}

  static async attach(generation: number): Promise<DesktopIpcTransport> {
    const transport = new DesktopIpcTransport(generation);
    transport.unlisten = await listen<HostProtocolEvent>(
      PROTOCOL_EVENT,
      ({ payload }) => {
        if (!transport.closed && payload.generation === generation) {
          transport.emit({ kind: "message", message: payload.message });
        }
      },
    );
    transport.unlistenExit = await listen<HostConnectionExitEvent>(
      CONNECTION_EXIT_EVENT,
      ({ payload }) => {
        if (!transport.closed && payload.generation === generation) {
          transport.closed = true;
          transport.emit({
            kind: "closed",
            reason: formatExitDiagnostic(payload.diagnostics),
          });
        }
      },
    );
    return transport;
  }

  async send(message: ClientMessage): Promise<void> {
    if (this.closed) throw new Error("desktop IPC transport is closed");
    const reply = await invoke<ProtocolReply>("protocol_send", {
      generation: this.generation,
      message,
    });
    if (reply.generation !== this.generation) {
      throw new Error("stale desktop IPC reply rejected");
    }
    if (reply.message) {
      this.emit({ kind: "message", message: reply.message });
    }
  }

  subscribe(listener: (event: ProtocolTransportEvent) => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  async close(): Promise<void> {
    if (this.disconnectSent) return;
    this.disconnectSent = true;
    this.closed = true;
    this.unlisten?.();
    this.unlisten = undefined;
    this.unlistenExit?.();
    this.unlistenExit = undefined;
    try {
      await invoke("disconnect", { generation: this.generation });
    } catch {
      // A later generation already owns the host; it must not be disconnected
      // by this stale renderer transport.
    }
  }

  private emit(event: ProtocolTransportEvent): void {
    for (const listener of this.listeners) listener(event);
  }
}

function formatExitDiagnostic(diagnostic: HostExitDiagnostics): string {
  const status = diagnostic.exit_code !== null
    ? `exit ${diagnostic.exit_code}`
    : diagnostic.signal !== null
      ? `signal ${diagnostic.signal}`
      : diagnostic.reason;
  return `Orbcode connection ended (${status}); the session can be resumed after reconnecting.`;
}

export async function connectLocal(
  input: LocalConnectionInput = {},
): Promise<DesktopConnection> {
  const status = await invoke<ConnectionStatus>("connect_local", { input });
  return attachClient(status, "orbcode-desktop-local");
}

export async function connectSsh(
  input: SshConnectionInput,
): Promise<DesktopConnection> {
  const status = await invoke<ConnectionStatus>("connect_ssh", { input });
  return attachClient(status, "orbcode-desktop-ssh");
}

export async function getConnectionStatus(): Promise<ConnectionStatus> {
  return invoke<ConnectionStatus>("connection_status");
}

async function attachClient(
  status: ConnectionStatus,
  clientName: string,
): Promise<DesktopConnection> {
  if (!status.active) throw new Error("desktop host did not activate a connection");
  const transport = await DesktopIpcTransport.attach(status.generation);
  return {
    status,
    client: new ProtocolClient(transport, {
      requestIdPrefix: `desktop-${status.generation}`,
      requestTimeoutMs: 30_000,
      clientName,
    }),
  };
}
