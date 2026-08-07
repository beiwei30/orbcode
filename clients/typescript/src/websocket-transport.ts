/** Node WebSocket transport for the shared generated-protocol client. */

import WebSocket from "ws";
import type { ClientMessage, ServerMessage } from "./generated/protocol.js";
import type {
  ProtocolTransport,
  ProtocolTransportEvent,
} from "./protocol-client.js";

export class WebSocketProtocolTransport implements ProtocolTransport {
  private readonly listeners = new Set<
    (event: ProtocolTransportEvent) => void
  >();
  private closed = false;

  private constructor(private readonly socket: WebSocket) {
    socket.on("message", (data: WebSocket.RawData) => {
      try {
        this.emit({
          kind: "message",
          message: JSON.parse(data.toString()) as ServerMessage,
        });
      } catch {
        // Ignore non-protocol frames in this reference transport.
      }
    });
    socket.once("error", () => this.emitClosed("websocket transport failed"));
    socket.once("close", () => this.emitClosed("websocket transport closed"));
  }

  static async connect(
    url: string,
    authenticationToken: string,
    timeoutMs = 10_000,
  ): Promise<WebSocketProtocolTransport> {
    const socket = new WebSocket(url);
    await new Promise<void>((resolve, reject) => {
      const timer = setTimeout(() => {
        socket.terminate();
        reject(new Error("websocket connect timeout"));
      }, timeoutMs);
      socket.once("open", () => {
        clearTimeout(timer);
        socket.send(authenticationToken, (error) => {
          if (error) reject(error);
          else resolve();
        });
      });
      socket.once("error", () => {
        clearTimeout(timer);
        reject(new Error("websocket connect failed"));
      });
    });
    return new WebSocketProtocolTransport(socket);
  }

  async send(message: ClientMessage): Promise<void> {
    if (this.closed || this.socket.readyState !== WebSocket.OPEN) {
      throw new Error("websocket transport is closed");
    }
    await new Promise<void>((resolve, reject) => {
      this.socket.send(JSON.stringify(message), (error) => {
        if (error) reject(error);
        else resolve();
      });
    });
  }

  subscribe(listener: (event: ProtocolTransportEvent) => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  async close(): Promise<void> {
    if (this.closed || this.socket.readyState === WebSocket.CLOSED) return;
    this.closed = true;
    await new Promise<void>((resolve) => {
      const timer = setTimeout(() => {
        this.socket.terminate();
        resolve();
      }, 2_000);
      this.socket.once("close", () => {
        clearTimeout(timer);
        resolve();
      });
      this.socket.close();
    });
  }

  private emit(event: ProtocolTransportEvent): void {
    for (const listener of this.listeners) listener(event);
  }

  private emitClosed(reason: string): void {
    if (this.closed) return;
    this.closed = true;
    this.emit({ kind: "closed", reason });
  }
}
