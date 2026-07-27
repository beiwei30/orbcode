#!/usr/bin/env npx tsx
/**
 * Stdio smoke test — proves the protocol contract is consumable from TypeScript.
 *
 * Usage: ORBCODE_BIN=/path/to/orbcode npx tsx src/smoke-stdio.ts
 */

import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { OrbCodeClient } from "./client.js";

const BIN = process.env.ORBCODE_BIN ?? (() => { console.error("ORBCODE_BIN env var required"); process.exit(1); })();

const homeDir = mkdtempSync(join(tmpdir(), "orbcode-ts-smoke-"));
const cwdDir = mkdtempSync(join(tmpdir(), "orbcode-ts-cwd-"));
writeFileSync(
  join(homeDir, "settings.json"),
  JSON.stringify({ env: { ANTHROPIC_API_KEY: "stub-key" } }),
);

async function assert(condition: boolean, msg: string): Promise<void> {
  if (!condition) {
    throw new Error(`FAIL: ${msg}`);
  }
  console.log(`  PASS: ${msg}`);
}

async function main() {
  console.log("=== orbcode TypeScript stdio smoke test ===\n");

  const client = new OrbCodeClient(BIN, homeDir, cwdDir);

  try {
    // 1. Initialize with default capabilities
    console.log("1. Initialize (default capabilities)");
    const init = await client.initialize();
    await assert(init.protocol_version === "1.0", "protocol_version = 1.0");
    await assert(init.server_info.name === "orbcode", "server_info.name = orbcode");
    await assert(init.capabilities.streaming === true, "streaming = true");
    await assert(
      init.capabilities.experimental_methods.length === 0,
      "no experimental methods for default client",
    );
    await assert(
      init.capabilities.stable_methods.length > 0,
      "stable methods present",
    );
    await assert(
      init.capabilities.server_request_methods.includes("permission/request"),
      "permission/request in server_request_methods",
    );
    await assert(
      init.capabilities.server_request_methods.includes("ask_user/request"),
      "ask_user/request in server_request_methods",
    );

    // 2. Session list (empty)
    console.log("\n2. Session list");
    const sessions = await client.sessionList();
    await assert(Array.isArray(sessions), "session list is array");

    // 3. Bootstrap
    console.log("\n3. Bootstrap session");
    const bootstrap = await client.bootstrap();
    await assert(bootstrap !== null, "bootstrap returns data");

    console.log("\n=== All stdio smoke tests passed ===");
  } finally {
    await client.close();
  }
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
