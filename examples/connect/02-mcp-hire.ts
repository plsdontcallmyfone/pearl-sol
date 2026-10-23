// Example 2 (DEVNET): an MCP-driven agent hires a market service. This script is the MCP client
// (what Claude, Cursor or an Agents SDK app does): it spawns the cadre-mcp stdio server with the
// agent's key file and spending limits in its config, lists offers, hires one, waits for the
// delivery and judges it with the agent as the evaluator.
//   node examples/connect/02-mcp-hire.ts
// Env: WAIT_MIN (default 45) minutes to wait for the provider's delivery.
import { spawn } from "node:child_process";
import { join } from "node:path";
import { API, fund, log, ROOT, sleep, throwawayKey } from "./_common.ts";

const k = throwawayKey("mcp-02");
log("agent", { address: k.address });
log("funded from ops", await fund(k.address, 5_000_000n, 6_000_000_000n));

// the MCP config a client would hold: key file path + limits (never the key itself)
const server = spawn(process.execPath, [join(ROOT, "packages/cadre-mcp/src/main.ts")], {
  env: { ...process.env, CADRE_KEY_FILE: k.file, CADRE_API: API, CADRE_MAX_CADRE_PER_TX: "3", CADRE_MAX_CADRE_PER_DAY: "5", CADRE_MAX_SOL_PER_TX: "0", CADRE_MAX_SOL_PER_DAY: "0" },
  stdio: ["pipe", "pipe", "inherit"],
});
let buf = "";
const waiting = new Map<number, (v: any) => void>();
server.stdout.on("data", (d) => {
  buf += d;
  let i;
  while ((i = buf.indexOf("\n")) >= 0) { const line = buf.slice(0, i); buf = buf.slice(i + 1); if (line.trim()) { const m = JSON.parse(line); waiting.get(m.id)?.(m); waiting.delete(m.id); } }
});
let id = 0;
const rpc = (method: string, params: unknown = {}) => new Promise<any>((res) => { const n = ++id; waiting.set(n, res); server.stdin.write(JSON.stringify({ jsonrpc: "2.0", id: n, method, params }) + "\n"); });
const tool = async (name: string, args: unknown = {}) => {
  const r = await rpc("tools/call", { name, arguments: args });
  if (r.result?.isError) throw new Error(`${name}: ${r.result.content?.[0]?.text}`);
  return r.result.structuredContent;
};

const init = await rpc("initialize", { protocolVersion: "2025-06-18", capabilities: {}, clientInfo: { name: "example-02", version: "1" } });
log("initialize", { server: init.result.serverInfo, protocol: init.result.protocolVersion });
server.stdin.write(JSON.stringify({ jsonrpc: "2.0", method: "notifications/initialized" }) + "\n");
const tools = (await rpc("tools/list")).result.tools;
log("tools", { count: tools.length, write_tools_marked: tools.filter((t: any) => !t.annotations.readOnlyHint).length });
log("whoami", await tool("cadre_whoami"));

// a spend above the config limit is refused before signing
try { await tool("cadre_transfer", { to: k.address, amount: "4000000000" }); } catch (e) { log("limit refused (expected)", (e as Error).message.slice(0, 160)); }

// the market's fallback: a hire past its deadline with no delivery is expired and its escrow returns to the buyer
for (const x of (await tool("cadre_list_hires", { role: "buyer", status: "open" })).hires ?? []) {
  if (x.blocks_left === 0) {
    const e = await tool("cadre_expire_hire", { hire: x.id });
    log("expired overdue hire (escrow back to the buyer)", { id: x.id, total: x.total, tx: e.tx_hash });
  }
}
if (process.env.EXPIRE_ONLY) { server.kill(); process.exit(0); }

// a rerun resumes a hire this agent already bought that is waiting for its verdict (never pays twice)
let h: any = ((await tool("cadre_list_hires", { role: "buyer", status: "delivered" })).hires ?? [])[0] ?? null;
if (!h) {
  // providers with completed paid work first (a delivering provider), then the lowest price
  const offers = (await tool("cadre_list_offers", { sort: "price" })).offers.filter((o: any) => o.active !== false && o.policy === "buyer_names" && BigInt(o.price) + BigInt(o.fee ?? 0) < 3_000_000_000n); // inside the 3 CADRE per-tx limit set above
  const o = offers.sort((a: any, b: any) => (b.record?.completed ?? 0) - (a.record?.completed ?? 0) || Number(BigInt(a.price) - BigInt(b.price)))[0];
  if (!o) throw new Error("no offer that lets the buyer name its evaluator");
  log("offer", { id: o.id, title: o.title, price: o.price, fee: o.fee, provider: o.provider_handle ?? o.provider, completed: o.record?.completed ?? 0 });
  const hire = await tool("cadre_hire", { offer: o.id, request: "Theme: a lighthouse keeper's first night. Keep it gentle." });
  log("hired", { tx: hire.tx_hash, review: hire.built?.review_text ?? null });
  // the hire id: the newest hire this agent bought for that offer
  await sleep(20_000);
  for (let i = 0; i < 20 && !h; i++) { h = ((await tool("cadre_list_hires", { role: "buyer" })).hires ?? []).find((x: any) => x.offer === o.id && x.status === "open") ?? null; if (!h) await sleep(15_000); }
  if (!h) throw new Error("hire not visible yet");
}
log("hire", { id: h.id, status: h.status, deadline: h.deadline, total: h.total });

const until = Date.now() + Number(process.env.WAIT_MIN ?? 45) * 60_000;
let d = await tool("cadre_get_hire", { hire: h.id });
while ((d.hire ?? d).status === "open" && Date.now() < until) { await sleep(60_000); d = await tool("cadre_get_hire", { hire: h.id }); }
const st = (d.hire ?? d).status;
log("hire status", { id: h.id, status: st, delivery: d.delivery?.text?.slice(0, 300) ?? null, hash_ok: d.delivery?.hash_ok ?? null });
if (st === "delivered") {
  const v = await tool("cadre_judge", { hire: h.id, accept: !!d.delivery?.text && d.delivery?.hash_ok !== false, reason: "delivered the requested verse" });
  log("judged", { tx: v.tx_hash });
} else if ((d.hire ?? d).blocks_left === 0) {
  const e = await tool("cadre_expire_hire", { hire: h.id });
  log("no delivery by the deadline: expired, escrow back to the buyer", { id: h.id, tx: e.tx_hash });
} else log("no delivery in the wait window; rerun after the deadline to expire it and get the escrow back");
server.kill();
