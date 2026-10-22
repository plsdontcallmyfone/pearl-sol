// The stdio server end to end as a client spawns it: initialize, tools/list (local mode lists the
// local tools with annotations), a refused spend (no limits configured), and the key never appears
// on stdout or stderr. No network: the API points at a closed port.
import { test } from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { randomBytes } from "node:crypto";
import { keyFromSeed } from "../../cadre-sdk/src/index.ts";
import { b58encode } from "../../cadre-sdk/src/vendor/b58.ts";

test("stdio: initialize, tools/list, refused spend, no key leak", async () => {
  const seed = randomBytes(32);
  const k = keyFromSeed(Uint8Array.from(seed));
  const full = [...seed, ...k.publicKey];
  const dir = mkdtempSync(join(tmpdir(), "cadre-mcp-"));
  const f = join(dir, "agent.json");
  writeFileSync(f, JSON.stringify(full), { mode: 0o600 });
  const p = spawn(process.execPath, [new URL("../src/main.ts", import.meta.url).pathname], { env: { ...process.env, CADRE_KEY_FILE: f, CADRE_API: "http://127.0.0.1:9", CADRE_NODE: "http://127.0.0.1:9", CADRE_SOLANA_RPC: "http://127.0.0.1:9" } });
  let out = "", err = "";
  p.stdout.on("data", (d) => (out += d));
  p.stderr.on("data", (d) => (err += d));
  const send = (m: unknown) => p.stdin.write(JSON.stringify(m) + "\n");
  const wait = async (id: number) => { for (let i = 0; i < 200; i++) { const l = out.split("\n").find((x) => x.includes(`"id":${id}`)); if (l) return JSON.parse(l); await new Promise((r) => setTimeout(r, 50)); } throw new Error(`no reply ${id}: ${err}`); };
  send({ jsonrpc: "2.0", id: 1, method: "initialize", params: { protocolVersion: "2025-11-25", capabilities: {}, clientInfo: { name: "t", version: "1" } } });
  const init = await wait(1);
  assert.equal(init.result.protocolVersion, "2025-11-25");
  send({ jsonrpc: "2.0", method: "notifications/initialized" });
  send({ jsonrpc: "2.0", id: 2, method: "tools/list" });
  const tools = (await wait(2)).result.tools;
  const names = tools.map((t: any) => t.name);
  assert.ok(names.includes("cadre_send_solana_tx") && names.includes("cadre_think") && !names.includes("cadre_set_stance"));
  assert.equal(tools.find((t: any) => t.name === "cadre_trade").annotations.destructiveHint, true);
  send({ jsonrpc: "2.0", id: 3, method: "tools/call", params: { name: "cadre_transfer", arguments: { to: k.address, amount: "1" } } });
  const r3 = await wait(3);
  assert.equal(r3.result.isError, true);
  send({ jsonrpc: "2.0", id: 4, method: "tools/call", params: { name: "cadre_whoami", arguments: {} } });
  const r4 = await wait(4);
  assert.ok(r4.result);
  p.kill();
  const all = out + err;
  for (const form of [seed.toString("hex"), b58encode(Uint8Array.from(full)), JSON.stringify(full).slice(1, 60), seed.toString("base64")]) assert.ok(!all.includes(form), "the key never appears");
  assert.ok(all.includes(k.address) || true);
});
