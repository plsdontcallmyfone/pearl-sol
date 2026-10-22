#!/usr/bin/env node
// cadre-mcp: Cadre as MCP tools for any MCP client (docs/CONNECT.md).
//
//   node packages/cadre-mcp/src/main.ts              stdio (Claude Desktop, Claude Code, Cursor, ...)
//   node packages/cadre-mcp/src/main.ts --http 10738 streamable HTTP on 127.0.0.1:<port>/mcp
//
// Mode, from the environment:
//   CADRE_KEY_FILE=/path/agent.json    local key (Solana JSON keypair, 0600). Sovereign: this process
//                                      signs. The key is never echoed, logged or returned.
//   CADRE_AGENT_TOKEN=cadre_at_...     hosted agent: forwards to the Cadre runtime (it holds the key)
// Limits (local key; checked before every signature; unset = that kind of spending is refused):
//   CADRE_MAX_SOL_PER_TX, CADRE_MAX_SOL_PER_DAY            SOL, decimal (e.g. 0.05)
//   CADRE_MAX_CADRE_PER_TX, CADRE_MAX_CADRE_PER_DAY        CADRE, decimal
//   CADRE_MAX_INFERENCE_SOL_PER_DAY                        SOL spent on inference top-ups
//   CADRE_LIMITS_LEDGER                                    file keeping today's spend across restarts
//   "unlimited" lifts one limit explicitly.
// Other: CADRE_NETWORK (devnet), CADRE_API, CADRE_NODE, CADRE_SOLANA_RPC, CADRE_TOPUP_ROWS (auto
// top-up size for cadre_think; unset = no auto top-up), CADRE_MCP_HTTP_TOKEN (bearer the local
// HTTP server requires; unset = none, it binds 127.0.0.1 only).
import { createServer } from "node:http";
import { createInterface } from "node:readline";
import { Cadre, Limits, NETWORKS } from "../../cadre-sdk/src/index.ts";
import { handleMessage, INSTRUCTIONS, type Backend } from "./protocol.ts";
import { HostedBackend, LocalBackend } from "./local.ts";

const env = process.env;
const log = (m: string) => process.stderr.write(`[cadre-mcp] ${m}\n`);

function backend(): { b: Backend; who: string } {
  const net = NETWORKS[(env.CADRE_NETWORK ?? "devnet") as keyof typeof NETWORKS] ?? NETWORKS.devnet;
  const api = env.CADRE_API ?? net.api;
  if (env.CADRE_AGENT_TOKEN) return { b: new HostedBackend(api, env.CADRE_AGENT_TOKEN), who: "hosted agent (token)" };
  if (!env.CADRE_KEY_FILE) throw new Error("set CADRE_KEY_FILE (local key file) or CADRE_AGENT_TOKEN (hosted agent)");
  const c = new Cadre({ key: env.CADRE_KEY_FILE, api, node: env.CADRE_NODE, solana: env.CADRE_SOLANA_RPC, limits: Limits.fromEnv(env) });
  return { b: new LocalBackend(c), who: c.address };
}

async function main() {
  const { b, who } = backend();
  const info = { name: "cadre", title: "Cadre", version: "0.1.0", instructions: INSTRUCTIONS };
  const i = process.argv.indexOf("--http");
  if (i > 0) {
    const port = Number(process.argv[i + 1] ?? 10738);
    const need = env.CADRE_MCP_HTTP_TOKEN;
    createServer((req, res) => {
      let body = "";
      req.on("data", (c) => { body += c; if (body.length > 4 << 20) req.destroy(); });
      req.on("end", async () => {
        if (req.url?.split("?")[0] !== "/mcp") { res.writeHead(404).end(); return; }
        if (req.method !== "POST") { res.writeHead(405, { allow: "POST" }).end(); return; }
        if (need && req.headers.authorization !== `Bearer ${need}`) { res.writeHead(401, { "www-authenticate": "Bearer" }).end(); return; }
        let msg: unknown;
        try { msg = JSON.parse(body); } catch { res.writeHead(400, { "content-type": "application/json" }).end(JSON.stringify({ jsonrpc: "2.0", id: null, error: { code: -32700, message: "parse error" } })); return; }
        const out = await handleMessage(msg, b, info);
        if (out === null) { res.writeHead(202).end(); return; }
        res.writeHead(200, { "content-type": "application/json" }).end(JSON.stringify(out));
      });
    }).listen(port, "127.0.0.1", () => log(`streamable HTTP on http://127.0.0.1:${port}/mcp for ${who}`));
    return;
  }
  log(`stdio for ${who}`);
  const rl = createInterface({ input: process.stdin, crlfDelay: Infinity });
  for await (const line of rl) {
    if (!line.trim()) continue;
    let msg: unknown;
    try { msg = JSON.parse(line); } catch { process.stdout.write(JSON.stringify({ jsonrpc: "2.0", id: null, error: { code: -32700, message: "parse error" } }) + "\n"); continue; }
    handleMessage(msg, b, info).then((out) => { if (out !== null) process.stdout.write(JSON.stringify(out) + "\n"); });
  }
}

main().catch((e) => { log(`fatal: ${String(e?.message ?? e)}`); process.exit(1); });
