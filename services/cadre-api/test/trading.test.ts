// Trading routes (docs/API.md "Trading", docs/TRADING.md) against a fake Solana + node RPC that
// serves one Mandate account, its vault and token account, the CADRE pool, one tx with a
// MandateSwap event, a Cadre AMM pool with Swap events, and a Cadre mandate contract.
// Ports 10741 (fake RPC) and 10742 (api): the trading row's block (CLAUDE.md).
import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import { createServer, type Server } from "node:http";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { configFromEnv, Deps } from "../src/deps.ts";
import { build } from "../src/app.ts";
import { Api } from "../src/http.ts";
import { registerTrading, decodeMandate, encodeLimits, swapEvents, avgCostPnl, type Limits, type TradingConfig } from "../src/routes/trading.ts";
import { b58decode, b58encode, findPda, sha256, utf8 } from "../../../apps/explorer/src/codec.ts";
import { eventTopic } from "../../../apps/explorer/src/contracts.ts";
import { ata, TOKEN_2022 } from "../src/sol.ts";

const PORTS = { rpc: 10741, api: 10742 };
const key = (i: number) => b58encode(Uint8Array.from({ length: 32 }, (_, j) => (j === 0 ? i : 7)));
const PROGRAM = "BG5WkxZyYwWXTY2YSAJGhwL7JcxChRXx8UqHEAMAC5yc";
const MINT = key(1), POOL = key(2), USER = key(3), AGENT = key(4), BASEV = key(5), QUOTEV = key(6), BUYBACK = key(9);
const AMM_POOL = key(10), TOKEN = key(11), MCONTRACT = key(12), HOUSE = key(13);
const le64 = (n: bigint) => { const b = Buffer.alloc(8); b.writeBigUInt64LE(n); return b; };
const i64 = (n: bigint) => { const b = Buffer.alloc(8); b.writeBigInt64LE(n); return b; };
const disc = (ns: string, n: string) => Buffer.from(sha256(utf8(`${ns}:${n}`)).subarray(0, 8));
const MANDATE = findPda([utf8("mandate"), b58decode(USER), le64(1n)], PROGRAM)[0];
const VAULT = findPda([utf8("vault"), b58decode(MANDATE)], PROGRAM)[0];

const limits: Limits = { venues: 2, any_mint: false, mints: [MINT], max_in_per_trade: "20000000", window_cap: "300000000", window_secs: 86400, max_trades_per_window: 48, max_slippage_bps: 300, min_pool_quote_reserve: "1000000000", max_trade_share_bps: 100, expiry_ts: "0", paused: false };
function mandateBytes() {
  const u16 = (n: number) => { const b = Buffer.alloc(2); b.writeUInt16LE(n); return b; };
  const u32 = (n: number) => { const b = Buffer.alloc(4); b.writeUInt32LE(n); return b; };
  return Buffer.concat([
    disc("account", "Mandate"), Buffer.from(b58decode(USER)), Buffer.from(b58decode(AGENT)), Buffer.from(b58decode(AGENT)), le64(1n), Buffer.from([254, 253, 0]),
    encodeLimits(limits), u16(2000), u32(3600), i64(1_792_617_000n), i64(1_792_617_000n), le64(5_000_000n), u16(1),
    le64(400_000_000n), le64(0n), le64(0n), le64(0n), i64(0n), i64(0n), le64(1n), le64(5_000_000n),
  ]);
}
function tokenAcct(amount: bigint) { const b = Buffer.alloc(165); le64(amount).copy(b, 64); return b; }
function poolBytes() {
  const b = Buffer.alloc(300);
  disc("account", "Pool").copy(b, 0);
  Buffer.from(b58decode(MINT)).copy(b, 43);
  Buffer.from(b58decode(BASEV)).copy(b, 139);
  Buffer.from(b58decode(QUOTEV)).copy(b, 171);
  return b;
}
function swapLog() {
  const d = Buffer.concat([disc("event", "MandateSwap"), Buffer.from(b58decode(MANDATE)), Buffer.from(b58decode(AGENT)), Buffer.from([1, 0]), Buffer.from(b58decode(MINT)),
    le64(5_000_000n), le64(200_000_000_000n), le64(205_000_000_000n), le64(80_000_000_000_000n), le64(2_000_000_000n), le64(395_000_000n), le64(513_000_000n)]);
  return `Program data: ${d.toString("base64")}`;
}
const acct = (data: Buffer, lamports = 2_039_280, owner = TOKEN_2022) => ({ data: [data.toString("base64"), "base64"], lamports, owner, executable: false });
const AMM_INFO = Buffer.concat([Buffer.from(b58decode(TOKEN)), le64(1_000_000_000_000n), le64(100_000_000_000_000_000n), le64(1n), Buffer.from([30, 0])]).toString("hex");
const swapData = (side: number, x: bigint, out: bigint, origin: string) => Buffer.concat([Buffer.from([side]), le64(x), le64(out), le64(0n), le64(0n), Buffer.from(b58decode(origin))]).toString("hex");

function rpc(m: string, p: any): any {
  switch (m) {
    case "getProgramAccounts": return [{ pubkey: MANDATE, account: acct(mandateBytes(), 3_000_000, PROGRAM) }];
    case "getMultipleAccounts": return { value: p[0].map((a: string) => a === VAULT ? acct(Buffer.alloc(0), 395_890_880, "11111111111111111111111111111111") : a === ata(VAULT, MINT, TOKEN_2022) ? acct(tokenAcct(200_000_000_000n)) : a === POOL ? acct(poolBytes(), 1, "pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA") : a === BASEV ? acct(tokenAcct(80_000_000_000_000n)) : a === QUOTEV ? acct(tokenAcct(2_000_000_000n)) : null) };
    case "getSignaturesForAddress": return [{ signature: "5sig", slot: 513_000_000, err: null, blockTime: 1_792_617_100 }];
    case "getTransaction": return { meta: { logMessages: ["Program log: Instruction: Swap", swapLog()] } };
    case "getLatestBlockhash": return { value: { blockhash: b58encode(Uint8Array.from({ length: 32 }, () => 3)), lastValidBlockHeight: 99 } };
    case "getChainInfo": return { chain_id: 1, genesis_hash: "ab".repeat(32), height: 10, finalized_height: 1, finality_k: 2 };
    case "getParams": return { min_fee: "1000" };
    case "getBlock": return null;
    case "listJobs": return { total: 0, jobs: [] };
    case "getBridgeEvents": return { withdrawals: [] };
    case "queryContract": {
      const a = Buffer.from(p.args, "hex");
      const meth = a.subarray(1, 1 + a[0]).toString();
      if (p.address === AMM_POOL && meth === "info") return { ok: true, return: AMM_INFO };
      if (p.address === MCONTRACT && meth === "count") return { ok: true, return: le64(0n).toString("hex") };
      return { ok: false, error: "" };
    }
    case "getContractEvents":
      assert.equal(p.topic, eventTopic("Swap"));
      return { events: [
        { height: 9, tx_hash: "bb".repeat(32), topics: [eventTopic("Swap"), Buffer.from(b58decode(HOUSE)).toString("hex"), Buffer.from(b58decode(HOUSE)).toString("hex")], data: swapData(1, 50_000n, 600n, HOUSE) },
        { height: 8, tx_hash: "aa".repeat(32), topics: [eventTopic("Swap"), Buffer.from(b58decode(HOUSE)).toString("hex"), Buffer.from(b58decode(HOUSE)).toString("hex")], data: swapData(0, 1_000n, 100_000n, HOUSE) },
      ] };
    default: throw new Error(`unknown method ${m}`);
  }
}

let rpcSrv: Server, apiSrv: Server, dir = "", base = "";
const listen = (s: Server, port: number) => new Promise<void>((r, j) => s.once("error", j).listen(port, "127.0.0.1", () => r()));
before(async () => {
  rpcSrv = createServer((req, res) => {
    let body = "";
    req.on("data", (d) => (body += d));
    req.on("end", () => {
      const q = JSON.parse(body || "{}");
      const one = (x: any) => { try { return { jsonrpc: "2.0", id: x.id, result: rpc(x.method, x.params) }; } catch (e) { return { jsonrpc: "2.0", id: x.id, error: { code: -32601, message: (e as Error).message } }; } };
      res.setHeader("content-type", "application/json");
      res.end(JSON.stringify(Array.isArray(q) ? q.map(one) : one(q)));
    });
  });
  await listen(rpcSrv, PORTS.rpc);
  dir = mkdtempSync(join(tmpdir(), "cadre-trading-test-"));
  const cfg = configFromEnv({ NODE_RPC: `http://127.0.0.1:${PORTS.rpc}`, SOLANA_RPC: `http://127.0.0.1:${PORTS.rpc}`, GATEWAY_URL: "off", STOREFRONT_URL: "off", POOL_URL: "off", CADRE_RATE_LIMIT_PER_MIN: "100000", CADRE_RATE_BURST: "100000" }, { stateDir: dir });
  const d = new Deps(cfg);
  d.ix.o.log = () => {};
  const tc: TradingConfig = { solana: { program: PROGRAM, config: key(20), mint: MINT, pool: POOL, buyback_vault: BUYBACK }, cadre: { amm_pool: AMM_POOL, agent_token: TOKEN, mandate_contract: MCONTRACT }, house: { [HOUSE]: "house_one" } };
  const api = new Api({ allowedOrigins: "*", rateLimitPerMin: 0, rateBurst: 1000, trustProxy: false, maxBody: 1 << 20 });
  registerTrading(api, d, tc);
  api.o.log = undefined;
  apiSrv = api.listen(PORTS.api, "127.0.0.1");
  await new Promise((r) => apiSrv.on("listening", r));
  base = `http://127.0.0.1:${PORTS.api}`;
  void build;
});
after(() => { apiSrv?.close(); rpcSrv?.close(); rmSync(dir, { recursive: true, force: true }); });
const get = async (p: string) => { const r = await fetch(base + p); return { status: r.status, body: await r.json().catch(() => null) }; };
const post = async (p: string, b: unknown) => { const r = await fetch(base + p, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(b) }); return { status: r.status, body: await r.json().catch(() => null) }; };

test("codec: Mandate account, limits round trip, MandateSwap events from logs", () => {
  const m = decodeMandate(MANDATE, mandateBytes())!;
  assert.equal(m.user, USER);
  assert.equal(m.agent, AGENT);
  assert.deepEqual(m.limits, limits);
  assert.equal(m.perf_fee_bps, 2000);
  assert.equal(m.deposited, 400_000_000n);
  assert.equal(decodeMandate(MANDATE, Buffer.alloc(300)), null);
  const ev = swapEvents(["Program log: x", swapLog()]);
  assert.equal(ev.length, 1);
  assert.equal(ev[0].amount_out, "200000000000");
  assert.equal(ev[0].slot, 513_000_000);
});

test("average-cost PnL counts only realized gains", () => {
  const p = avgCostPnl([{ side: "buy", amount_in: "1000", amount_out: "100" }, { side: "sell", amount_in: "50", amount_out: "600" }]);
  assert.equal(p.position, 50n);
  assert.equal(p.cost_basis, 500n);
  assert.equal(p.realized, 100n);
});

test("summary, agent view, trades, leaderboard from chain data; house labelled and excluded from organic", async () => {
  const s = await get("/v1/trading");
  assert.equal(s.status, 200);
  assert.equal(s.body.venues.solana.reserves.sol_lamports, "2000000000");
  assert.equal(s.body.venues.cadre.reserves.fee_bps, 30);
  assert.equal(s.body.volume.solana_lamports.organic, "5000000");
  assert.equal(s.body.volume.cadre_base_units.house, "1600", "buy 1,000 spent + sell 600 received");
  assert.equal(s.body.volume.cadre_base_units.organic, "0");
  const a = await get(`/v1/trading/agents/${AGENT}`);
  assert.equal(a.status, 200);
  const m = a.body.positions.solana_mandates[0];
  assert.equal(m.sol_equity_lamports, "395000000");
  assert.equal(m.realized_pnl_lamports, "-5000000", "SOL equity basis: the buy is not a profit or a mark");
  assert.equal(m.cadre_position_spl, "200000000000");
  assert.equal(m.marked_at_mid_lamports, "5000000");
  assert.equal(a.body.trades[0].ref, "sol:5sig");
  assert.equal((await get(`/v1/trading/agents/${key(99)}`)).status, 404);
  const h = await get(`/v1/trading/agents/${HOUSE}`);
  assert.equal(h.body.house, true);
  assert.equal(h.body.positions.cadre_wallet[0].realized_pnl_cadre, "100");
  const t = await get("/v1/trading/trades?organic=1");
  assert.equal(t.body.trades.every((x: any) => !x.house), true);
  const lb = await get("/v1/trading/leaderboard");
  assert.equal(lb.body.solana[0].agent, AGENT);
  assert.equal(lb.body.cadre[0].handle, "house_one");
  assert.equal((await get("/v1/trading/mandates")).status, 400);
  assert.equal((await get(`/v1/trading/mandates?user=${USER}`)).body.solana.length, 1);
});

test("unsigned mandate txs: open (next id, words), revoke / withdraw / limits only for the owner", async () => {
  const o = await post("/v1/tx/mandate-open", { user: USER, agent: AGENT, deposit_lamports: "100000000", limits: { max_in_per_trade: "10000000" } });
  assert.equal(o.status, 200);
  assert.equal(o.body.summary.mandate_id, "2", "next id after the user's mandate 1");
  assert.match(o.body.in_words, /may only swap SOL and CADRE/);
  assert.match(o.body.in_words, /can never withdraw/);
  assert.deepEqual(o.body.signers, [USER]);
  assert.equal((await post("/v1/tx/mandate-open", { user: USER, agent: AGENT, deposit_lamports: "1", limits: { max_slippage_bps: 9000 } })).status, 400);
  assert.equal((await post("/v1/tx/mandate-revoke", { user: USER, mandate: MANDATE })).status, 200);
  assert.equal((await post("/v1/tx/mandate-revoke", { user: AGENT, mandate: MANDATE })).status, 400, "an agent cannot build the user's revoke");
  const w = await post("/v1/tx/mandate-withdraw", { user: USER, mandate: MANDATE });
  assert.equal(w.body.summary.sol_lamports, "395000000");
  assert.equal(w.body.summary.cadre_spl, "200000000000");
  assert.match(w.body.in_words, /Withdraw 0.395 SOL and 200000 CADRE/);
  const l = await post("/v1/tx/mandate-limits", { user: USER, mandate: MANDATE, limits: { paused: true } });
  assert.match(l.body.in_words, /^Pause/);
});
