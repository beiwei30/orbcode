/** Node stdio transport for the shared generated-protocol client. */

import { type ChildProcess, spawn } from "node:child_process";
import { createInterface, type Interface } from "node:readline";
import type { ClientMessage, ServerMessage } from "./generated/protocol.js";
import {
  ProtocolClient,
  type ProtocolTransport,
  type ProtocolTransportEvent,
} from "./protocol-client.js";

export class StdioProtocolTransport implements ProtocolTransport {
  private readonly process: ChildProcess;
  private readonly lines: Interface;
  private readonly listeners = new Set<
    (event: ProtocolTransportEvent) => void
  >();
  private closed = false;

  constructor(binPath: string, homeDir: string, cwd: string) {
    this.process = spawn(binPath, ["serve", "--stdio"], {
      cwd,
      env: {
        ORBCODE_HOME: homeDir,
        PATH: process.env.PATH,
        HOME: homeDir,
        RUST_LOG: "warn",
      },
      stdio: ["pipe", "pipe", "ignore"],
    });

    this.lines = createInterface({ input: this.process.stdout! });
    this.lines.on("line", (line: string) => {
      try {
        this.emit({
          kind: "message",
          message: JSON.parse(line) as ServerMessage,
        });
      } catch {
        // A malformed line is not a canonical message. The Rust transport
        // enforces strict framing; this minimal Node reference ignores it.
      }
    });
    this.process.once("error", () => {
      this.emitClosed("stdio child process failed");
    });
    this.process.once("close", () => {
      this.emitClosed("stdio child process exited");
    });
  }

  async send(message: ClientMessage): Promise<void> {
    if (this.closed || !this.process.stdin?.writable) {
      throw new Error("stdio child input is closed");
    }
    const line = `${JSON.stringify(message)}\n`;
    await new Promise<void>((resolve, reject) => {
      this.process.stdin!.write(line, (error) => {
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
    if (this.closed) return;
    this.closed = true;
    this.lines.close();
    this.process.stdin?.end();
    if (await this.waitForExit(2_000)) return;
    this.process.kill("SIGTERM");
    if (await this.waitForExit(2_000)) return;
    this.process.kill("SIGKILL");
    await this.waitForExit(2_000);
  }

  private emit(event: ProtocolTransportEvent): void {
    for (const listener of this.listeners) listener(event);
  }

  private emitClosed(reason: string): void {
    if (this.closed) return;
    this.closed = true;
    this.emit({ kind: "closed", reason });
  }

  private async waitForExit(timeoutMs: number): Promise<boolean> {
    if (this.process.exitCode !== null || this.process.signalCode !== null) {
      return true;
    }
    return new Promise<boolean>((resolve) => {
      const timer = setTimeout(() => resolve(false), timeoutMs);
      this.process.once("close", () => {
        clearTimeout(timer);
        resolve(true);
      });
    });
  }
}

/** Backward-compatible stdio convenience used by the existing smoke tests. */
export class OrbCodeClient extends ProtocolClient {
  constructor(binPath: string, homeDir: string, cwd: string) {
    super(new StdioProtocolTransport(binPath, homeDir, cwd));
  }
}
